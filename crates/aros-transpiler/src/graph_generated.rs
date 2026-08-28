//! Generated-source, HIDD, and architecture-lane resolution.

use super::{
    ArchSourceDecl, BTreeSet, DependencyGraph, FetchDecl, HashSet, ModuleType, Path, PathBuf,
    ResolvedScriptOutput, TargetDefinition,
};

impl DependencyGraph {
    pub fn add_host_generated_headers(
        &mut self,
        decls: Vec<crate::host_generated_headers::HostGeneratedHeader>,
    ) {
        self.host_generated_headers.extend(decls);
    }

    pub fn add_hidd_stubs(&mut self, decls: Vec<crate::hidd_stubs::HiddStubsDecl>) {
        self.hidd_stubs.extend(decls);
    }

    pub fn add_script_outputs(&mut self, decls: Vec<crate::copy_includes::ScriptOutputDecl>) {
        self.pending_script_outputs.extend(decls);
    }

    pub fn add_bison_outputs(&mut self, decls: Vec<crate::copy_includes::BisonOutputDecl>) {
        self.bison_outputs.extend(decls);
    }

    /// Binds each reachable script-generated file to its consumers.
    ///
    /// A generated source is not on disk when CMake configures, so the target
    /// that declares it needs the generator registered first;
    /// `aros_resolve_sources` consults that registry before it probes the
    /// filesystem. Until then the source was dropped in silence and the compile
    /// failed on the generated *header* instead, one step away from the cause.
    ///
    /// Matching is by path, normalised in two ways that are not guesses:
    /// `${AROS_BUILD_DIR}` and `${CMAKE_BINARY_DIR}` are the same directory
    /// (`cmake/AROS.cmake:60`), and a source list carries stems while a rule
    /// names the file, so the extension is dropped from the rule's output.
    ///
    /// Recognisable rules for outputs outside the selected source and aggregate
    /// graph are deliberately omitted. Upstream routinely keeps recipes for
    /// several dependency versions in one file; activating all of them would
    /// require scripts which the selected archive does not contain. The return
    /// value is retained as the reporting boundary for future recognised-but-
    /// unsafe active forms.
    pub fn resolve_script_outputs(&mut self) -> Vec<String> {
        fn normalise(path: &str) -> String {
            let path = path.replace("${AROS_BUILD_DIR}", "${CMAKE_BINARY_DIR}");
            match path.rsplit_once('.') {
                // Only a suffix that looks like an extension, never a dot in a
                // directory name.
                Some((stem, extension))
                    if !extension.is_empty()
                        && extension.len() <= 4
                        && !extension.contains('/') =>
                {
                    stem.to_owned()
                }
                _ => path,
            }
        }

        let reports = Vec::new();
        let legacy_outputs: HashSet<String> = self
            .python_outputs
            .iter()
            .flat_map(|declaration| {
                declaration.jobs.iter().map(|job| {
                    normalise(&format!(
                        "{}/{}",
                        declaration.build_root.trim_end_matches('/'),
                        job.output.trim_start_matches('/')
                    ))
                })
            })
            .collect();
        let pending = std::mem::take(&mut self.pending_script_outputs);
        for decl in pending {
            let mut declared_outputs = vec![decl.output.clone()];
            declared_outputs.extend(decl.additional_outputs.iter().cloned());
            let wanted: Vec<String> = declared_outputs
                .iter()
                .map(|output| normalise(output))
                .collect();
            // A few complex Mesa groups still use the older, audited adapter.
            // Never declare the same output twice while those capabilities are
            // retired incrementally in favour of exact recipe translation.
            if wanted.iter().any(|output| legacy_outputs.contains(output)) {
                continue;
            }
            let mut consumers: Vec<String> = self
                .targets
                .iter()
                .filter(|(_, target)| {
                    let names_generated_source = target
                        .source_files
                        .iter()
                        .chain(target.cxx_source_files.iter())
                        .chain(target.asm_source_files.iter())
                        .any(|source| wanted.iter().any(|output| normalise(source) == *output));
                    let target_directory = target.dir_path.to_string_lossy().replace('\\', "/");
                    let names_header_consumer = target_directory == decl.directory
                        && !decl.consumer_source_stems.is_empty()
                        && target
                            .source_files
                            .iter()
                            .chain(target.cxx_source_files.iter())
                            .chain(target.asm_source_files.iter())
                            .filter_map(|source| {
                                Path::new(source).file_stem().and_then(|stem| stem.to_str())
                            })
                            .any(|stem| {
                                decl.consumer_source_stems
                                    .iter()
                                    .any(|candidate| candidate == stem)
                            });
                    names_generated_source || names_header_consumer
                })
                .map(|(mmake, _)| mmake.clone())
                .collect();

            // Some generated headers are collected into a dedicated #MM
            // target rather than named by a compile source list. Preserve
            // that explicit GNU Make edge as a CMake consumer edge.
            consumers.extend(
                self.meta_targets
                    .iter()
                    .filter(|(_, dependencies)| {
                        dependencies.iter().any(|dependency| {
                            wanted.iter().any(|output| normalise(dependency) == *output)
                        })
                    })
                    .map(|(name, _)| name.clone()),
            );
            consumers.extend(
                decl.consumer_targets
                    .iter()
                    .filter(|target| self.targets.contains_key(*target))
                    .cloned(),
            );
            consumers.sort();
            consumers.dedup();
            if consumers.is_empty() && decl.consumer_targets.is_empty() {
                // GNU Make may carry generator recipes for several upstream
                // versions in one mmakefile. A rule whose output is absent
                // from both the selected source inventory and every aggregate
                // prerequisite is unreachable in this configuration. Do not
                // activate it merely because its recipe is recognisable.
                continue;
            }
            let output_stem = Path::new(&decl.output)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("output")
                .chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
                .collect::<String>();
            let Some(owner_prefix) = consumers.first().or_else(|| decl.consumer_targets.first())
            else {
                continue;
            };
            let owner = format!("{owner_prefix}-{output_stem}-generated");
            for consumer in &decl.consumer_targets {
                if !self.targets.contains_key(consumer) {
                    self.meta_targets
                        .entry(consumer.clone())
                        .or_default()
                        .insert(owner.clone());
                }
            }

            // A script rule may live in an mmakefile in this repository while
            // the actual generator and XML inputs are unpacked by `%fetch`.
            // Match the narrowest owning destination for every referenced
            // path; depending on that fetch target lets clean Ninja builds
            // materialise the files before the custom command runs.
            let mut dependency_targets = BTreeSet::new();
            let mut referenced_paths = Vec::with_capacity(decl.depends.len() + 2);
            referenced_paths.push(decl.script.as_str());
            referenced_paths.extend(decl.depends.iter().map(String::as_str));
            if let Some(directory) = decl.working_directory.as_deref() {
                referenced_paths.push(directory);
            }
            for path in referenced_paths {
                let mut owners: Vec<&FetchDecl> = self
                    .fetches
                    .iter()
                    .filter(|fetch| {
                        let destination = fetch.destination.trim_end_matches('/');
                        path == destination
                            || path
                                .strip_prefix(destination)
                                .is_some_and(|tail| tail.starts_with('/'))
                    })
                    .collect();
                owners.sort_by(|left, right| {
                    right
                        .destination
                        .len()
                        .cmp(&left.destination.len())
                        .then_with(|| left.name.cmp(&right.name))
                });
                if let Some(fetch) = owners.first() {
                    dependency_targets.insert(fetch.name.clone());
                }
            }
            self.script_outputs.push(ResolvedScriptOutput {
                owner,
                script: decl.script,
                arguments: decl.arguments,
                outputs: declared_outputs,
                depends: decl.depends,
                stdout: decl.stdout,
                working_directory: decl.working_directory,
                dependency_targets: dependency_targets.into_iter().collect(),
                consumers,
            });
        }
        reports
    }

    /// Turns the `%make_hidd_stubs` declarations into the one link library they
    /// feed.
    ///
    /// `compiler/libhiddstubs/mmakefile.src` archives
    /// `$(call WILDCARD, $(GENDIR)/lib/hidd/*.o)`, and the contents of that
    /// directory are exactly the `$(STUBS)` of every `%make_hidd_stubs` in the
    /// tree. Once the tree is parsed that is a known list, so the wildcard needs
    /// no modelling: one archive with those sources is the same artefact.
    ///
    /// The synthesised declaration takes the mmake id and the archive name the
    /// reference uses, so `uselibs=hiddstubs` resolves through the ordinary
    /// path. Its directory is the source root, because the sources come from six
    /// different places and the macro applies no per-directory include flags:
    /// `%make_hidd_stubs` calls `%compile_q` with `$(CFLAGS) $(CPPFLAGS)`
    /// directly instead of the `%(mmake)_CFLAGS` lane that would add
    /// `$(USER_INCLUDES)` (`config/make.tmpl:3562` against `:1681`).
    ///
    /// Returns what it could not use, for reporting.
    pub fn resolve_hidd_stubs(&mut self) -> Vec<String> {
        const MMAKE: &str = "linklibs-hiddstubs";
        const ARCHIVE: &str = "hiddstubs";

        if self.hidd_stubs.is_empty() {
            return Vec::new();
        }

        let mut reports = Vec::new();
        if let Some(existing) = self.targets.get(MMAKE) {
            reports.push(format!(
                "{MMAKE} is already declared as {}, so the {}                  %make_hidd_stubs declaration(s) were not attached",
                existing.target_name,
                self.hidd_stubs.len()
            ));
            return reports;
        }

        let mut sources: Vec<String> = Vec::new();
        for decl in &self.hidd_stubs {
            for source in &decl.sources {
                if sources.contains(source) {
                    reports.push(format!(
                        "{}: %make_hidd_stubs hidd={} contributes {source},                          which another declaration already contributes",
                        decl.directory, decl.hidd
                    ));
                    continue;
                }
                sources.push(source.clone());
            }
        }
        if sources.is_empty() {
            return reports;
        }

        self.targets.insert(
            MMAKE.to_owned(),
            TargetDefinition {
                mmake_name: MMAKE.to_owned(),
                target_name: ARCHIVE.to_owned(),
                module_type: ModuleType::LinkLib,
                genmodule_only: false,
                empty_archive: false,
                source_files: sources,
                cxx_source_files: Vec::new(),
                always_cxx_link: false,
                objc_source_files: Vec::new(),
                asm_source_files: Vec::new(),
                use_libs: Vec::new(),
                dependencies: Vec::new(),
                dir_path: PathBuf::from("."),
                target_dir: None,
                link_libs: Vec::new(),
                variant_32bit: false,
                declared_mod_type: None,
                mod_suffix: None,
                // The archive name is TARGET here; `linklib_name` is the
                // separate `libname=` lane, which aros_add_linklib does not
                // accept for a plain link library.
                linklib_name: None,
                config_file: None,
                genmodule_linklibs: None,
                linklib_output_dir: None,
                // Public, and not by inference: `compiler/libhiddstubs`
                // states the output as `$(AROS_LIB)/libhiddstubs.a`, which is
                // the default target library directory, built with the default
                // target compiler and no port sources. Without this the
                // archive keeps its CMake target name and stays in the build
                // root, so a declaration that spells `-lhiddstubs` as a raw
                // link option cannot see it.
                canonical_linklib_output: true,
                canonical_linklib_eligible: true,
                compiler_flags: Vec::new(),
                include_dirs: Vec::new(),
                arch_modules: Vec::new(),
                arch_includes: Vec::new(),
                defines: Vec::new(),
                undefines: Vec::new(),
                compile_options: Vec::new(),
                link_options: Vec::new(),
                spec_switches: Vec::new(),
                driver_link_options: Vec::new(),
                isa_link_options: Vec::new(),
                arch_sources: Vec::new(),
                arch_defines: Vec::new(),
                arch_compile_options: Vec::new(),
                arch_source_options: Vec::new(),
            },
        );
        reports
    }

    pub fn add_binary_objects(&mut self, decls: Vec<crate::binary_objects::BinaryObjectDecl>) {
        self.binary_objects.extend(decls);
    }

    pub fn add_arch_sources(&mut self, decls: Vec<ArchSourceDecl>) {
        for d in decls {
            self.arch_sources
                .entry(d.mainmmake.clone())
                .or_default()
                .push(d);
        }
    }

    /// Attaches the architecture source overrides to their target.
    ///
    /// The join key is `mainmmake`, which the declaration states outright.
    ///
    /// A second declaration in the same directory can inherit the same
    /// overrides, because make.tmpl keys the arch objects on the object
    /// directory rather than on the target:
    ///
    ///   config/make.tmpl:3296  %build_archspecific writes its objects to
    ///                          $(GENDIR)/<maindir>/<modname>/arch
    ///   config/make.tmpl:2921  %build_linklib picks them up with
    ///                          $(wildcard $(OBJDIR)/arch/*.o) and filters the
    ///                          same basenames out of its own file list
    ///
    /// compiler/crt/stdc is the case in the tree: `linklibs-romhack` declares
    /// `objdir=$(GENDIR)/$(CURDIR)/stdc`, which is exactly compiler-stdc's arch
    /// object root, so its `setjmp` and `longjmp` come from
    /// arch/x86_64-all/stdc/*.s. Without this, romhack compiles
    /// compiler/crt/stdc/setjmp.c, which is nothing but
    /// `#error setjmp has to be implemented for each cpu`, and dos.library
    /// never links because romhack is its uselibs.
    ///
    /// Keyed on the directory rather than on the objdir string, which is
    /// broader than Make: a declaration in the directory with a deliberately
    /// separate objdir (`linklibs-libm` is one) would also inherit. Every such
    /// inheritance is returned so it stays visible rather than implied.
    /// Gives a `%build_archspecific` lane the tag of the lane that pulls it in.
    ///
    /// `arch=` is not always an architecture. `arch/i386-all/hidd/gfx` declares
    /// three lanes for one module -- `i386`, `x86_sse` and `x86_avx` -- and the
    /// last two are names, not targets: what attaches them is a MetaMake edge in
    /// the x86_64 file,
    ///
    /// ```text
    /// #MM- kernel-hidd-gfx-x86_64 : kernel-hidd-gfx-x86_sse kernel-hidd-gfx-x86_avx
    /// ```
    ///
    /// and the metatarget of a lane is `<mainmmake>-<arch>`. CMake selects a lane
    /// by matching its tag against the configured target's tags, so a lane whose
    /// tag is a name matches nothing and its sources are dropped. gfx.hidd then
    /// referenced 18 `convert_*_SSE2/_SSE3/_AVX` implementations that were never
    /// compiled, and the ELF loader refused the boot over the first of them.
    ///
    /// The rule is structural and closed over the declarations at hand: when an
    /// edge attaches `<mainmmake>-<a>` to `<mainmmake>-<b>` and both `a` and `b`
    /// are tags of lanes of that same mainmmake, lane `a` is also a lane of `b`.
    /// The lane keeps its own tag as well, so a target that really does select
    /// `x86_sse` is unaffected.
    ///
    /// Retagging here rather than in CMake because CMake reads the declarations
    /// before the meta edges exist.
    ///
    /// Returns the attachments made, for the record, and the lanes whose tag
    /// nothing attaches.
    pub fn resolve_arch_lane_attachments(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        // (mainmmake, lane tag) of every declaration, so an edge can be read as
        // an attachment only between two lanes of the same module.
        let mut lanes: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for decl in self.arch_sources.values().flatten() {
            lanes.insert((decl.mainmmake.clone(), decl.tag.clone()));
        }

        let mut attachments: Vec<(String, String, String)> = Vec::new();
        for (consumer, deps) in &self.meta_targets {
            for dep in deps {
                for (mainmmake, tag) in &lanes {
                    let prefix = format!("{mainmmake}-");
                    let Some(puller) = consumer.strip_prefix(&prefix) else {
                        continue;
                    };
                    if dep.strip_prefix(&prefix) != Some(tag.as_str()) {
                        continue;
                    }
                    if puller == tag.as_str() {
                        continue;
                    }
                    if !lanes.contains(&(mainmmake.clone(), puller.to_owned())) {
                        continue;
                    }
                    attachments.push((mainmmake.clone(), tag.clone(), puller.to_owned()));
                }
            }
        }
        attachments.sort();
        attachments.dedup();

        for (mainmmake, lane, puller) in attachments {
            let mut added = Vec::new();
            for decls in self.arch_sources.values_mut() {
                let clones: Vec<ArchSourceDecl> = decls
                    .iter()
                    .filter(|decl| decl.mainmmake == mainmmake && decl.tag == lane)
                    .map(|decl| ArchSourceDecl {
                        tag: puller.clone(),
                        ..decl.clone()
                    })
                    .collect();
                for clone in clones {
                    if decls
                        .iter()
                        .any(|decl| decl.tag == clone.tag && decl.files == clone.files)
                    {
                        continue;
                    }
                    added.extend(clone.files.iter().cloned());
                    decls.push(clone);
                }
            }
            if !added.is_empty() {
                notes.push(format!(
                    "{mainmmake}: lane {lane} also applies to {puller}, contributing {added:?}"
                ));
            }
        }
        notes
    }

    pub fn resolve_arch_sources(&mut self) -> Vec<String> {
        let mut inherited: Vec<String> = Vec::new();
        // (directory, tag, dir, files) of every declaration that names a
        // maindir, to offer to the other declarations living there.
        let offers: Vec<(String, String, String, Vec<String>)> = self
            .arch_sources
            .values()
            .flatten()
            .filter_map(|d| {
                d.maindir.as_ref().map(|maindir| {
                    (
                        maindir.trim_matches('/').to_owned(),
                        d.tag.clone(),
                        d.dir.clone(),
                        d.files.clone(),
                    )
                })
            })
            .collect();
        let owners: std::collections::HashSet<&String> = self.arch_sources.keys().collect();
        let mut adopt: Vec<(String, String, String, Vec<String>)> = Vec::new();
        for (mmake, target) in &self.targets {
            if owners.contains(mmake) {
                continue;
            }
            let directory = target.dir_path.to_string_lossy().replace('\\', "/");
            for (maindir, tag, dir, files) in &offers {
                if *maindir != directory {
                    continue;
                }
                let shadowed: Vec<String> = files
                    .iter()
                    .filter(|file| target.source_files.iter().any(|source| source == *file))
                    .cloned()
                    .collect();
                if shadowed.is_empty() {
                    continue;
                }
                inherited.push(format!(
                    "{directory}: {mmake} takes {} from arch={tag} {dir} \
                     (shared arch object root with the declaration in that directory)",
                    shadowed.join(",")
                ));
                adopt.push((mmake.clone(), tag.clone(), dir.clone(), shadowed));
            }
        }
        for (mmake, tag, dir, files) in adopt {
            if let Some(target) = self.targets.get_mut(&mmake) {
                target.arch_sources.push((tag, dir, files));
            }
        }
        inherited.sort();

        for (name, decls) in &self.arch_sources {
            if let Some(target) = self.targets.get_mut(name) {
                for d in decls {
                    target
                        .arch_sources
                        .push((d.tag.clone(), d.dir.clone(), d.files.clone()));
                    // The declaring file's own include paths and flags belong to
                    // this target too, but only for its architecture.
                    for inc in &d.include_dirs {
                        let e = (d.tag.clone(), inc.clone());
                        if !target.arch_includes.contains(&e) {
                            target.arch_includes.push(e);
                        }
                    }
                    for def in &d.defines {
                        let e = (d.tag.clone(), def.clone());
                        if !target.arch_defines.contains(&e) {
                            target.arch_defines.push(e);
                        }
                    }
                    // Per file, not per tag. Applying a lane's flags to the
                    // whole target would put -mavx2 on the baseline dispatcher
                    // that arch/x86_64-all/hidd/gfx/rgbconv_arch.c's own comment
                    // says must not have it, and after lane attachment two lanes
                    // with different flags share one tag.
                    for opt in &d.compile_options {
                        for file in &d.files {
                            let e = (d.tag.clone(), d.dir.clone(), file.clone(), opt.clone());
                            if !target.arch_source_options.contains(&e) {
                                target.arch_source_options.push(e);
                            }
                        }
                    }
                }
            }
        }
        inherited
    }
}
