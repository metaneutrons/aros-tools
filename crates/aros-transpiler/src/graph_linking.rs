//! Link-library, default-link-set, and package resolution.

use super::{
    arch_compatible, arch_of, has_public_link_archive, needs_canonical_link_archive,
    raw_link_archive_visible, runtime_name, target_runtime_name, DefaultLinkSet, DependencyGraph,
    ModuleType, ResolvedDefaultLinkItem, ResolvedPackageMember, TargetDefinition,
};

impl DependencyGraph {
    pub fn add_packages(&mut self, decls: Vec<crate::packages::PackageDecl>) {
        self.packages.extend(decls);
    }

    /// Turns each package's `(kind, module name)` members into the mmake ids
    /// that build them.
    ///
    /// A declaration names `devs=ata`, meaning the file `ata.device`, and the
    /// target that builds it is `kernel-ata`. Only the module name is stated,
    /// so the lookup needs every mmakefile parsed, which is why this runs on
    /// the finished graph.
    ///
    /// Returns the members that resolved to nothing. Those matter: a package
    /// missing a module still builds, and the failure only appears when the
    /// system does not boot.
    /// Turns each target's `uselibs` names into the link-library targets that
    /// build them.
    ///
    /// `uselibs="debug"` means link against libdebug.a, which
    /// `%build_linklib libname=debug` builds under the mmake id
    /// `linklibs-debug`. Nothing resolved those names, so every builder linked
    /// an always-empty ARG_LIBS and no module linked against anything at all.
    ///
    /// Resolving here rather than in CMake also sidesteps a name clash:
    /// aros_link_libraries discards `debug`, `optimized` and `general` because
    /// target_link_libraries reads them as build-type keywords, and `debug` is
    /// a real AROS link library used by 20 declarations.
    ///
    /// Returns the names that matched no link library.
    pub fn resolve_use_libs(&mut self) -> Vec<String> {
        // A sourceful module may request relative libraries from its .conf
        // even though the `%build_module` invocation has no `uselibs=` text.
        // Enable only those full-module providers required by an already
        // enabled declaration (z1 is the first production case), and only
        // when every explicit linklib input was modelled exactly.
        let required_relative: std::collections::HashSet<String> = self
            .targets
            .values()
            .filter_map(|target| target.genmodule_linklibs.as_ref())
            .filter(|metadata| metadata.enabled)
            .flat_map(|metadata| metadata.relative_libraries.iter().cloned())
            .collect();
        for target in self.targets.values_mut() {
            if target.module_type != ModuleType::Library
                || !required_relative.contains(&target.target_name)
            {
                continue;
            }
            if let Some(metadata) = target.genmodule_linklibs.as_mut() {
                if metadata.has_relative && metadata.inputs_exact {
                    metadata.enabled = true;
                }
            }
        }

        // Keep the declaration identity for architecture/flavour selection
        // separate from the target a consumer may actually link. ABI-only
        // declarations expose a workflow aggregate under their mmake id; the
        // archive is the `<mmake>-linklib` product target.
        let mut by_name: std::collections::HashMap<String, Vec<(String, String)>> =
            std::collections::HashMap::new();
        for (mmake, target) in &self.targets {
            let mut providers: Vec<(String, Vec<String>)> = match target.module_type {
                ModuleType::Abi => vec![(
                    format!("{mmake}-linklib"),
                    vec![
                        target.target_name.clone(),
                        format!("{}_rel", target.target_name),
                    ],
                )],
                ModuleType::LinkLib => {
                    vec![(mmake.clone(), vec![target.target_name.clone()])]
                }
                // A full library module materialises its genmodule client-link
                // archive under `<mmake>-linklib`. The default provider name is
                // the module name; `linklibname=` publishes an additional
                // spelling for the same archive.
                ModuleType::Library
                    if target.genmodule_only
                        || target
                            .genmodule_linklibs
                            .as_ref()
                            .is_some_and(|metadata| metadata.enabled) =>
                {
                    let mut names = vec![target.target_name.clone()];
                    if let Some(alias) = &target.linklib_name {
                        if !names.contains(alias) {
                            names.push(alias.clone());
                        }
                    }
                    let mut providers = vec![(format!("{mmake}-linklib"), names.clone())];
                    if target
                        .genmodule_linklibs
                        .as_ref()
                        .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
                    {
                        let relative_names =
                            names.iter().map(|name| format!("{name}_rel")).collect();
                        providers.push((format!("{mmake}-linklib-rel"), relative_names));
                    }
                    providers
                }
                _ => continue,
            };
            for (link_target, names) in &mut providers {
                names.sort();
                names.dedup();
                for name in names {
                    by_name
                        .entry(name.clone())
                        .or_default()
                        .push((mmake.clone(), link_target.clone()));
                }
            }
        }
        for declaration in &self.external_cmake {
            by_name
                .entry(declaration.provided_library.clone())
                .or_default()
                .push((
                    declaration.mmake_name.clone(),
                    declaration.provider_target.clone(),
                ));
        }
        for declaration in &self.configure_builds {
            if let (Some(library), Some(provider)) = (
                declaration.provided_library.as_ref(),
                declaration.provider_target.as_ref(),
            ) {
                by_name
                    .entry(library.clone())
                    .or_default()
                    .push((declaration.mmake_name.clone(), provider.clone()));
            }
        }

        let mut unresolved = Vec::new();
        let mut resolved: Vec<(String, Vec<String>)> = Vec::new();
        let mut promote_canonical = Vec::new();
        let mut link_option_edges = Vec::new();
        let mut rejected_link_options = Vec::new();
        for (mmake, target) in &self.targets {
            let mut ids = Vec::new();
            let mut requested = target.use_libs.clone();
            if let Some(metadata) = target
                .genmodule_linklibs
                .as_ref()
                .filter(|metadata| metadata.enabled)
            {
                requested.extend(
                    metadata
                        .relative_libraries
                        .iter()
                        .map(|name| format!("{name}_rel")),
                );
            }
            let explicitly_linked: std::collections::HashSet<String> =
                requested.iter().cloned().collect();
            let mut link_flag_libraries: Vec<String> =
                if matches!(target.module_type, ModuleType::LinkLib | ModuleType::Abi) {
                    Vec::new()
                } else {
                    target
                        .link_options
                        .iter()
                        .filter_map(|option| option.strip_prefix("-l"))
                        .filter(|name| !name.is_empty() && !name.starts_with(':'))
                        .map(str::to_owned)
                        .collect()
                };
            link_flag_libraries.sort();
            link_flag_libraries.dedup();
            requested.extend(link_flag_libraries.iter().cloned());
            let mut seen_requested = std::collections::HashSet::new();
            requested.retain(|name| seen_requested.insert(name.clone()));
            for name in &requested {
                match by_name.get(name.as_str()) {
                    Some(c) => {
                        let raw_link = link_flag_libraries.contains(name);
                        // A raw linker name may have several declarations in
                        // the graph. Its concrete search path is authoritative:
                        // retain only public providers and private providers
                        // whose complete output directory is named by this
                        // consumer. This lets two private archives with the
                        // same basename remain safely disambiguated by -L.
                        let candidates: Vec<&(String, String)> = c
                            .iter()
                            .filter(|(declaration, _)| {
                                !raw_link
                                    || self.targets.get(declaration).is_some_and(|provider| {
                                        raw_link_archive_visible(provider, target)
                                    })
                            })
                            .collect();
                        if candidates.is_empty() {
                            rejected_link_options.push((mmake.clone(), name.clone()));
                            unresolved.push(format!(
                                "{}: {mmake} link option -l{name} has no public or matching private archive",
                                target.dir_path.display()
                            ));
                            continue;
                        }

                        // The usual duplicate is a native archive plus the
                        // 32-bit bootstrap flavour. Prefer the native one only
                        // when that leaves exactly one candidate; any other
                        // duplicate remains an explicit ambiguity.
                        let selected = if candidates.len() == 1 {
                            Some(candidates[0])
                        } else if candidates
                            .iter()
                            .any(|(declaration, _)| !self.targets.contains_key(declaration))
                        {
                            // An external interface and an ordinary archive
                            // publishing the same uselib name is a capability
                            // collision, not a native/32-bit flavour pair.
                            // Never let the ordinary-provider preference hide
                            // an external build that would otherwise own the
                            // request.
                            None
                        } else {
                            let main: Vec<_> = candidates
                                .iter()
                                .copied()
                                .filter(|(declaration, _)| {
                                    self.targets
                                        .get(declaration)
                                        .is_some_and(|provider| !provider.variant_32bit)
                                })
                                .collect();
                            (main.len() == 1).then_some(main[0])
                        };
                        let Some(selected) = selected else {
                            if raw_link {
                                rejected_link_options.push((mmake.clone(), name.clone()));
                            }
                            // Sorted, because `by_name` is filled while
                            // iterating a HashMap and this list is the only
                            // place that order reaches a report. Two runs of
                            // the transpiler produced the same bytes and the
                            // same lines here in a different order, which is
                            // how `aros golden capture` found it: a baseline
                            // from a producer that varies would report noise as
                            // regression forever. OPEN-POINTS 13.
                            let mut declarations: Vec<_> = candidates
                                .iter()
                                .map(|(declaration, _)| declaration.as_str())
                                .collect();
                            declarations.sort_unstable();
                            let request = if raw_link {
                                format!("link option -l{name}")
                            } else {
                                format!("uselibs={name}")
                            };
                            unresolved.push(format!(
                                "{}: {mmake} {request} is ambiguous ({})",
                                target.dir_path.display(),
                                declarations.join(", ")
                            ));
                            continue;
                        };

                        let id = selected.1.clone();
                        if explicitly_linked.contains(name) && !ids.contains(&id) {
                            ids.push(id);
                        } else if raw_link {
                            link_option_edges.push((mmake.clone(), id));
                        }
                        let provider = self.targets.get(&selected.0);
                        if raw_link
                            && provider.is_some_and(needs_canonical_link_archive)
                            && !promote_canonical.contains(&selected.0)
                        {
                            promote_canonical.push(selected.0.clone());
                        }
                    }
                    // Not every uselib is built here: some name a host library
                    // or a port that is not fetched. Reported, not guessed at.
                    None if explicitly_linked.contains(name) => unresolved.push(format!(
                        "{}: {mmake} uselibs={name} has no link library",
                        target.dir_path.display()
                    )),
                    // The generated rule invokes ld.lld directly. An opaque
                    // compiler/toolchain library cannot be assumed to exist,
                    // and preserving it would turn an otherwise unrelated
                    // target into a link failure. Keep the gap in the report,
                    // not in the generated command line.
                    None => {
                        rejected_link_options.push((mmake.clone(), name.clone()));
                        unresolved.push(format!(
                            "{}: {mmake} link option -l{name} has no link library",
                            target.dir_path.display()
                        ));
                    }
                }
            }
            if !ids.is_empty() {
                resolved.push((mmake.clone(), ids));
            }
        }

        for (mmake, ids) in resolved {
            if let Some(t) = self.targets.get_mut(&mmake) {
                t.link_libs = ids;
            }
        }
        for (mmake, name) in rejected_link_options {
            if let Some(target) = self.targets.get_mut(&mmake) {
                let option = format!("-l{name}");
                target.link_options.retain(|candidate| candidate != &option);
            }
        }
        for mmake in promote_canonical {
            if let Some(target) = self.targets.get_mut(&mmake) {
                if target.canonical_linklib_eligible {
                    target.canonical_linklib_output = true;
                }
            }
        }
        for (consumer, provider) in link_option_edges {
            self.meta_targets
                .entry(consumer)
                .or_default()
                .insert(provider);
        }
        unresolved
    }

    /// Binds each `-l<name>` of the compiler spec's default link set to the
    /// declaration that publishes `lib<name>.a`, and promotes an ordinary link
    /// library to its canonical SDK archive name when it does.
    ///
    /// Without this the spec's consumers are invisible to the graph: nothing in
    /// the mmakefile tree links `-lamiga` or `-ldos`, so `linklibs-amiga` kept
    /// the target-name-derived archive `liblinklibs-amiga.a` and no `libdos.a`
    /// was requested at all.
    ///
    /// The archive base name is `target_name` for both kinds of provider: an
    /// ordinary `%build_linklib libname=` and a module's client archive both
    /// name their output after it.
    ///
    /// Returns what could not be bound, one line per spec item.
    pub fn resolve_default_link_set(&mut self, set: &DefaultLinkSet) -> Vec<String> {
        let mut unresolved = Vec::new();
        let mut promote_canonical: Vec<String> = Vec::new();
        let mut resolved: Vec<ResolvedDefaultLinkItem> = Vec::new();

        for item in &set.items {
            let mut candidates: Vec<(&String, &TargetDefinition)> = self
                .targets
                .iter()
                .filter(|(_, target)| {
                    target.target_name == item.name
                        && target.linklib_output_dir.is_none()
                        && has_public_link_archive(target)
                })
                .collect();
            if candidates.is_empty() {
                unresolved.push(format!(
                    "-l{} has no declaration publishing lib{}.a",
                    item.name, item.name
                ));
                continue;
            }
            if candidates.len() > 1 {
                // The usual duplicate is the 32-bit bootstrap flavour of the
                // same archive; the spec means the native one.
                candidates.retain(|(_, target)| !target.variant_32bit);
            }
            if candidates.len() != 1 {
                let mut declarations: Vec<&str> =
                    candidates.iter().map(|(mmake, _)| mmake.as_str()).collect();
                declarations.sort_unstable();
                unresolved.push(format!(
                    "-l{} is ambiguous ({})",
                    item.name,
                    declarations.join(", ")
                ));
                continue;
            }
            let (mmake, provider) = candidates[0];
            let mmake = mmake.clone();
            if needs_canonical_link_archive(provider) && !promote_canonical.contains(&mmake) {
                promote_canonical.push(mmake.clone());
            }
            let archive = match provider.module_type {
                // A module publishes its client archive as a separate target.
                ModuleType::Library | ModuleType::Abi => format!("{mmake}-linklib"),
                _ => mmake.clone(),
            };
            resolved.push(ResolvedDefaultLinkItem {
                name: item.name.clone(),
                archive,
                require_absent: item.require_absent.clone(),
                require_present: item.require_present.clone(),
            });
        }

        for mmake in promote_canonical {
            if let Some(target) = self.targets.get_mut(&mmake) {
                if target.canonical_linklib_eligible {
                    target.canonical_linklib_output = true;
                }
            }
        }
        self.default_link_set = resolved;
        unresolved
    }

    pub fn resolve_packages(&mut self) -> Vec<String> {
        // Indexed by the exact basename each target installs. A name-only
        // lookup is unsafe: `hid` is both `hid.class` and `hid.hidd`, while a
        // package declaration authoritatively asks for one of those files.
        let mut by_runtime: std::collections::HashMap<String, Vec<&str>> =
            std::collections::HashMap::new();
        for (mmake, target) in &self.targets {
            if let Some(runtime) = target_runtime_name(target) {
                by_runtime.entry(runtime).or_default().push(mmake.as_str());
            }
        }
        for ids in by_runtime.values_mut() {
            ids.sort_unstable();
        }

        let mut unresolved = Vec::new();
        let mut resolved_all = Vec::new();
        for decl in &self.packages {
            let mut members: Vec<ResolvedPackageMember> = Vec::new();
            let decl_arch = arch_of(std::path::Path::new(
                decl.file
                    .strip_suffix("/mmakefile.src")
                    .unwrap_or(&decl.file),
            ));

            // The startup module has to be linked first: the bootstrap takes
            // its entry point from the first executable section of the first
            // module (elfloader.c:662).
            let ordered = decl
                .startup
                .iter()
                .map(|s| ("resource".to_owned(), s.clone()))
                .chain(decl.members.iter().cloned());

            for (kind, name) in ordered {
                let member_runtime = runtime_name(&kind, &name);
                let Some(pool) = by_runtime.get(&member_runtime) else {
                    unresolved.push(format!(
                        "{}: {} {kind}={name} ({member_runtime}) has no target",
                        decl.file, decl.mmake
                    ));
                    continue;
                };

                // A #MM dependency names a cross-architecture producer
                // authoritatively. A single explicit candidate therefore
                // survives even when its directory is foreign (mingw32 uses
                // an all-hosted module this way). With no explicit choice,
                // architecture filtering is mandatory even for a unique pool:
                // uniqueness does not make a foreign module applicable.
                let explicit: Vec<&str> = self
                    .meta_targets
                    .get(&decl.mmake)
                    .map(|deps| {
                        pool.iter()
                            .copied()
                            .filter(|id| {
                                deps.contains(*id)
                                    // `%link_kickstart` depends on each
                                    // module's generated kobj target rather
                                    // than its base mmake id.
                                    || deps.contains(&format!("{id}-kobj"))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let (candidates, allow_single_foreign) = if explicit.is_empty() {
                    (pool.clone(), false)
                } else {
                    (explicit, true)
                };
                let eligible: Vec<&str> = if allow_single_foreign && candidates.len() == 1 {
                    candidates
                } else {
                    candidates
                        .into_iter()
                        .filter(|id| {
                            let cand = self.targets.get(*id).and_then(|t| arch_of(&t.dir_path));
                            arch_compatible(cand.as_ref(), decl_arch.as_ref())
                        })
                        .collect()
                };

                match eligible.len() {
                    1 => {
                        // GNU Make's `$^` removes duplicate prerequisites. Do
                        // the same by producer target, but remove the complete
                        // pair so MODULES and MEMBER_NAMES stay aligned. The
                        // first declaration entry supplies the runtime name.
                        if !members.iter().any(|member| member.target == eligible[0]) {
                            members.push(ResolvedPackageMember {
                                target: eligible[0].to_owned(),
                                runtime_name: member_runtime,
                            });
                        }
                    }
                    0 => unresolved.push(format!(
                        "{}: {} {kind}={name} ({member_runtime}) has no target for this architecture (candidates: {})",
                        decl.file,
                        decl.mmake,
                        pool.join(", ")
                    )),
                    _ => unresolved.push(format!(
                        "{}: {} {kind}={name} ({member_runtime}) is ambiguous ({})",
                        decl.file,
                        decl.mmake,
                        eligible.join(", ")
                    )),
                }
            }
            resolved_all.push(members);
        }

        for (decl, members) in self.packages.iter_mut().zip(resolved_all) {
            decl.resolved = members;
        }
        unresolved
    }
}
