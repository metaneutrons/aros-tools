use crate::arch_sources::ArchSourceDecl;
use crate::ast::{DefineHeaderDecl, MetaTargetRule, ModuleType, TargetDefinition};
use crate::catalogs::CatalogDecl;
use crate::copy_includes::{AdhocHeaderRule, CopyIncludesDecl, HeaderTransformDecl};
use crate::fetch::FetchDecl;
use crate::icons::{IconSet, IconTarget};
use crate::includes::ArchIncludeDecl;
use crate::packages::{runtime_name, ResolvedPackageMember};
use aros_common::{ArosError, Result};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Dependency Graph for parallel target building and cycle detection.
#[derive(Debug, Default)]
pub struct DependencyGraph {
    pub targets: HashMap<String, TargetDefinition>,
    pub meta_targets: HashMap<String, HashSet<String>>,
    /// Every unique `%build_icons` mmake id. This is separate from `targets`:
    /// icons are generated runtime resources, not compiled modules.
    pub icon_targets: HashMap<String, IconTarget>,
    /// Resolved icon declarations in source order. Duplicate mmake ids are
    /// intentional and their output rules must be aggregated by CMake.
    pub icons: Vec<IconSet>,
    /// Fully resolved `%build_catalogs` declarations. These are generated
    /// runtime resources rather than compiled module targets.
    pub catalogs: Vec<CatalogDecl>,
    /// Every `%set_archincludes` declaration in the tree, keyed by `modname`.
    pub arch_decls: HashMap<String, Vec<ArchIncludeDecl>>,
    /// Every resolved `%copy_includes` declaration, deduplicated.
    pub copy_includes: Vec<CopyIncludesDecl>,
    /// Hand-written header staging rules found anywhere in the tree.
    pub adhoc_header_rules: Vec<AdhocHeaderRule>,
    /// Safe hand-written header transforms, with graph-resolved ordering.
    pub header_transforms: Vec<HeaderTransformDecl>,
    /// Declaration-owned literal define headers, with their compile consumers.
    pub define_headers: Vec<DefineHeaderDecl>,
    /// `%build_archspecific` declarations, keyed by the target they extend.
    pub arch_sources: HashMap<String, Vec<ArchSourceDecl>>,
    /// `%fetch` declarations for third-party sources.
    pub fetches: Vec<FetchDecl>,
    /// `%make_package` and `%link_kickstart` declarations.
    pub packages: Vec<crate::packages::PackageDecl>,
}

/// Splits an `arch/<cpu>-<platform>/...` path into its two components.
fn arch_of(dir: &std::path::Path) -> Option<(String, String)> {
    let s = dir.to_string_lossy().replace('\\', "/");
    let rest = s.strip_prefix("arch/")?;
    let first = rest.split('/').next()?;
    let (cpu, platform) = first.split_once('-')?;
    Some((cpu.to_owned(), platform.to_owned()))
}

fn define_header_compile_targets(mmake: &str, target: &TargetDefinition) -> Vec<String> {
    match target.module_type {
        ModuleType::ProgramGroup => {
            let mut members = target
                .source_files
                .iter()
                .chain(&target.cxx_source_files)
                .chain(&target.objc_source_files)
                .chain(&target.asm_source_files)
                .filter_map(|source| std::path::Path::new(source).file_stem())
                .map(|stem| format!("{mmake}-{}", stem.to_string_lossy()))
                .collect::<Vec<_>>();
            members.sort();
            members.dedup();
            members
        }
        // Despite having no declaration sources, this is a real compiler
        // target: aros_add_library(GENMODULE_ONLY) creates the runtime with
        // libentry plus the generated start/end sources under the mmake id.
        ModuleType::Library if target.genmodule_only => vec![mmake.to_owned()],
        // These declarations materialise only utility/package orchestration
        // under their mmake id. An ABI's compiling target is its generated
        // client archive and does not consume the declaration's `uselibs`.
        ModuleType::Abi | ModuleType::Package | ModuleType::Custom => Vec::new(),
        _ => vec![mmake.to_owned()],
    }
}

/// Whether a candidate's directory can serve a declaration made under `ctx`.
///
/// The rule matches AROS_ARCH_SOURCE_DIRS in cmake/AROS.cmake: "all" is a
/// wildcard in either position and "native" is a platform shared by every
/// non-hosted target. A candidate outside arch/ is architecture-neutral and
/// always eligible.
///
/// This is what separates the six targets named `serial` across four
/// architectures, so a package declared in arch/i386-pc/boot picks
/// kernel-pc-i386-serial and not the Amiga or Unix one.
fn arch_compatible(candidate: Option<&(String, String)>, ctx: Option<&(String, String)>) -> bool {
    let Some((cand_cpu, cand_plat)) = candidate else {
        return true;
    };
    let Some((ctx_cpu, ctx_plat)) = ctx else {
        // A declaration outside arch/ describes the portable base, so an
        // architecture-specific candidate cannot be meant.
        return false;
    };
    let cpu_compatible = cand_cpu == "all"
        || cand_cpu == ctx_cpu
        || matches!(
            (ctx_cpu.as_str(), cand_cpu.as_str()),
            ("x86_64", "i386") | ("aarch64", "arm") | ("riscv64", "riscv")
        );
    cpu_compatible && (cand_plat == "all" || cand_plat == "native" || cand_plat == ctx_plat)
}

/// The runtime basename a target definition produces.
///
/// An explicit/effective `modsuffix` replaces the module type. Otherwise a
/// custom or simple module's declared type wins over the coarse AST kind.
/// This mirrors the output naming used by the CMake builders and lets package
/// resolution match an authoritative filename rather than a potentially
/// same-named module of another kind.
fn target_runtime_name(target: &TargetDefinition) -> Option<String> {
    // An ABI skeleton publishes headers and a static link stub, not a runtime
    // module. Its declared modtype must therefore never make it eligible for a
    // package member such as `library=foo`.
    if matches!(target.module_type, ModuleType::Abi) {
        return None;
    }
    if let Some(suffix) = target.mod_suffix.as_deref() {
        return Some(runtime_name(suffix, &target.target_name));
    }
    if let Some(declared) = target.declared_mod_type.as_deref() {
        return match declared {
            "printer" => Some(target.target_name.clone()),
            "usbclass" | "btclass" => Some(runtime_name("class", &target.target_name)),
            kind => Some(runtime_name(kind, &target.target_name)),
        };
    }
    let kind = match target.module_type {
        ModuleType::Library => "library",
        ModuleType::Device => "device",
        ModuleType::Resource => "resource",
        ModuleType::Hidd => "hidd",
        ModuleType::Datatype => "datatype",
        ModuleType::Gadget => "gadget",
        ModuleType::Mcc => "mcc",
        _ => return None,
    };
    Some(runtime_name(kind, &target.target_name))
}

/// Whether a raw `-l<name>` consumer can find this declaration's archive in
/// the target SDK library directory.
///
/// Full genmodule/ABI client archives are public by construction. An ordinary
/// link library is public only when it already owns, or may safely be promoted
/// to, its canonical SDK archive name. In-tree/private `libdir=` outputs are
/// deliberately excluded: the direct AROS linker rule has no proven search
/// path for them.
fn has_public_link_archive(target: &TargetDefinition) -> bool {
    match target.module_type {
        ModuleType::Abi => true,
        ModuleType::Library => {
            target.genmodule_only
                || target
                    .genmodule_linklibs
                    .as_ref()
                    .is_some_and(|metadata| metadata.enabled)
        }
        ModuleType::LinkLib => target.canonical_linklib_output || target.canonical_linklib_eligible,
        _ => false,
    }
}

impl DependencyGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_target(&mut self, target: TargetDefinition) {
        self.targets.insert(target.mmake_name.clone(), target);
    }

    pub fn add_icons(&mut self, targets: Vec<IconTarget>, sets: Vec<IconSet>) {
        for target in targets {
            self.icon_targets
                .entry(target.mmake.clone())
                .or_insert(target);
        }
        self.icons.extend(sets);
    }

    pub fn add_catalogs(&mut self, declarations: Vec<CatalogDecl>) {
        self.catalogs.extend(declarations);
    }

    pub fn add_fetches(&mut self, decls: Vec<FetchDecl>) {
        for d in decls {
            if !self.fetches.iter().any(|f| f.name == d.name) {
                self.fetches.push(d);
            }
        }
    }

    pub fn add_header_transforms(&mut self, decls: Vec<HeaderTransformDecl>) {
        self.header_transforms.extend(decls);
    }

    pub fn add_define_headers(&mut self, declarations: Vec<DefineHeaderDecl>) {
        self.define_headers.extend(declarations);
    }

    /// Gives every target which compiles against a declaration-owned literal
    /// header a direct edge to the real header owner.
    ///
    /// The source provider itself always consumes the header. Any target whose
    /// resolved `uselibs` names that provider consumes the public provider
    /// headers too; this covers a device which includes its HAL declarations
    /// without relying on a transitive link-order edge.
    pub fn resolve_define_headers(&mut self) -> Vec<String> {
        let target_links: Vec<(Vec<String>, Vec<String>)> = self
            .targets
            .iter()
            .map(|(mmake, target)| {
                (
                    define_header_compile_targets(mmake, target),
                    target.link_libs.clone(),
                )
            })
            .collect();
        let compile_targets: HashMap<String, Vec<String>> = self
            .targets
            .iter()
            .map(|(mmake, target)| (mmake.clone(), define_header_compile_targets(mmake, target)))
            .collect();
        let mut unresolved = Vec::new();
        let mut edges = BTreeSet::new();

        for header in &mut self.define_headers {
            let Some(provider_consumers) = compile_targets.get(&header.provider) else {
                unresolved.push(format!(
                    "{}:{}: {} provider {} has no concrete target",
                    header.file, header.line, header.owner, header.provider
                ));
                continue;
            };
            if provider_consumers.is_empty() {
                unresolved.push(format!(
                    "{}:{}: {} provider {} has no compiling target",
                    header.file, header.line, header.owner, header.provider
                ));
                continue;
            }

            header.consumers.extend(provider_consumers.iter().cloned());
            for (consumers, providers) in &target_links {
                if providers.contains(&header.provider) {
                    header.consumers.extend(consumers.iter().cloned());
                }
            }
            header.consumers.sort();
            header.consumers.dedup();
            for consumer in &header.consumers {
                edges.insert((consumer.clone(), header.owner.clone()));
            }
        }

        for (consumer, owner) in edges {
            self.meta_targets.entry(consumer).or_default().insert(owner);
        }
        unresolved
    }

    /// Joins promoted header recipes to the fetch which owns their input and
    /// to every concrete target compiling from that same port subtree.
    ///
    /// The fetch dependency belongs on the custom command itself; attaching it
    /// only to a sibling/meta target lets Ninja race a cache-empty transform.
    /// Consumers likewise receive a direct edge so ordinary static linklibs,
    /// which have no genmodule config to reveal the include, remain safe.
    pub fn resolve_header_transforms(&mut self) -> Vec<String> {
        const PORTS_ROOT: &str = "${AROS_PORTS_DIR}";

        let mut fetches: Vec<(String, String)> = self
            .fetches
            .iter()
            .filter(|fetch| {
                fetch.destination == PORTS_ROOT
                    || fetch.destination.starts_with("${AROS_PORTS_DIR}/")
            })
            .map(|fetch| {
                (
                    fetch.name.clone(),
                    fetch.destination.trim_end_matches('/').to_owned(),
                )
            })
            .collect();
        fetches.sort_by(|left, right| {
            right
                .1
                .len()
                .cmp(&left.1.len())
                .then_with(|| left.0.cmp(&right.0))
        });

        let target_sources: Vec<(String, ModuleType, bool, Vec<String>)> = self
            .targets
            .values()
            .map(|target| {
                let mut sources: Vec<String> = target
                    .source_files
                    .iter()
                    .chain(&target.cxx_source_files)
                    .chain(&target.objc_source_files)
                    .chain(&target.asm_source_files)
                    .cloned()
                    .collect();
                for (_, directory, files) in &target.arch_sources {
                    sources.extend(files.iter().map(|file| format!("{directory}/{file}")));
                }
                (
                    target.mmake_name.clone(),
                    target.module_type.clone(),
                    target.linklib_name.is_some() || target.genmodule_only,
                    sources,
                )
            })
            .collect();

        let mut unresolved = Vec::new();
        for transform in &mut self.header_transforms {
            let owner = fetches.iter().find(|(_, destination)| {
                transform.input == *destination
                    || transform
                        .input
                        .strip_prefix(destination)
                        .is_some_and(|tail| tail.starts_with('/'))
            });
            let Some((fetch, _)) = owner else {
                unresolved.push(format!(
                    "{}:{}: {} input {} has no matching %fetch owner",
                    transform.file, transform.line, transform.name, transform.input
                ));
                continue;
            };
            transform.dependencies.push(fetch.clone());

            let input_dir = transform
                .input
                .rsplit_once('/')
                .map_or(transform.input.as_str(), |(directory, _)| directory);
            for (mmake, module_type, has_client_linklib, sources) in &target_sources {
                let consumes_tree = sources.iter().any(|source| {
                    source == input_dir
                        || source
                            .strip_prefix(input_dir)
                            .is_some_and(|tail| tail.starts_with('/'))
                });
                if !consumes_tree {
                    continue;
                }
                transform.consumers.push(mmake.clone());
                if *module_type == ModuleType::Library && *has_client_linklib {
                    transform.consumers.push(format!("{mmake}-linklib"));
                }
            }
            transform.dependencies.sort();
            transform.dependencies.dedup();
            transform.consumers.sort();
            transform.consumers.dedup();
        }
        unresolved
    }

    /// Adds direct fetch prerequisites for concrete targets compiling port
    /// sources and returns port sources for which no `%fetch` owner exists.
    ///
    /// A sibling target's dependency is insufficient: Ninja may build the
    /// consumer directly, before the archive has been unpacked. Ownership is
    /// structural and deliberately limited to real declarations. When fetch
    /// destinations overlap, the longest path-component prefix wins.
    pub fn resolve_port_source_fetches(&mut self) -> Vec<String> {
        const PORTS_ROOT: &str = "${AROS_PORTS_DIR}";

        let mut owners: Vec<&FetchDecl> = self
            .fetches
            .iter()
            .filter(|fetch| {
                let destination = fetch.destination.trim_end_matches('/');
                destination == PORTS_ROOT || destination.starts_with("${AROS_PORTS_DIR}/")
            })
            .collect();
        owners.sort_by(|left, right| {
            right
                .destination
                .trim_end_matches('/')
                .len()
                .cmp(&left.destination.trim_end_matches('/').len())
                .then_with(|| left.name.cmp(&right.name))
        });

        let mut edges = BTreeSet::new();
        let mut unowned = BTreeSet::new();
        for target in self.targets.values() {
            for source in target
                .source_files
                .iter()
                .chain(&target.cxx_source_files)
                .chain(&target.objc_source_files)
                .chain(&target.asm_source_files)
            {
                if source != PORTS_ROOT && !source.starts_with("${AROS_PORTS_DIR}/") {
                    continue;
                }
                let owner = owners.iter().copied().find(|fetch| {
                    let destination = fetch.destination.trim_end_matches('/');
                    source == destination
                        || source
                            .strip_prefix(destination)
                            .is_some_and(|tail| tail.starts_with('/'))
                });
                if let Some(owner) = owner {
                    edges.insert((target.mmake_name.clone(), owner.name.clone()));
                } else {
                    unowned.insert(format!("{}|{source}", target.mmake_name));
                }
            }
        }

        for (target, fetch) in edges {
            self.meta_targets.entry(target).or_default().insert(fetch);
        }
        unowned.into_iter().collect()
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
    pub fn resolve_arch_sources(&mut self) {
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
                    for opt in &d.compile_options {
                        let e = (d.tag.clone(), opt.clone());
                        if !target.arch_compile_options.contains(&e) {
                            target.arch_compile_options.push(e);
                        }
                    }
                }
            }
        }
    }

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
                    Some(c) if c.len() == 1 => {
                        if link_flag_libraries.contains(name)
                            && !self
                                .targets
                                .get(&c[0].0)
                                .is_some_and(has_public_link_archive)
                        {
                            rejected_link_options.push((mmake.clone(), name.clone()));
                            unresolved.push(format!(
                                "{}: {mmake} link option -l{name} has no public SDK archive",
                                target.dir_path.display()
                            ));
                            continue;
                        }
                        let id = c[0].1.clone();
                        if explicitly_linked.contains(name) && !ids.contains(&id) {
                            ids.push(id);
                        } else if link_flag_libraries.contains(name) {
                            link_option_edges.push((mmake.clone(), id));
                        }
                        if link_flag_libraries.contains(name)
                            && !promote_canonical.contains(&c[0].0)
                        {
                            promote_canonical.push(c[0].0.clone());
                        }
                    }
                    Some(c) => {
                        // Two link libraries share a libname when one is the
                        // extra 32-bit flavour a 64-bit target keeps for its
                        // bootstrap. The main target wants the other one.
                        let main: Vec<&(String, String)> = c
                            .iter()
                            .filter(|(declaration, _)| {
                                self.targets
                                    .get(declaration)
                                    .is_some_and(|t| !t.variant_32bit)
                            })
                            .collect();
                        if main.len() == 1 {
                            if link_flag_libraries.contains(name)
                                && !self
                                    .targets
                                    .get(&main[0].0)
                                    .is_some_and(has_public_link_archive)
                            {
                                rejected_link_options.push((mmake.clone(), name.clone()));
                                unresolved.push(format!(
                                    "{}: {mmake} link option -l{name} has no public SDK archive",
                                    target.dir_path.display()
                                ));
                                continue;
                            }
                            let id = main[0].1.clone();
                            if explicitly_linked.contains(name) && !ids.contains(&id) {
                                ids.push(id);
                            } else if link_flag_libraries.contains(name) {
                                link_option_edges.push((mmake.clone(), id));
                            }
                            if link_flag_libraries.contains(name)
                                && !promote_canonical.contains(&main[0].0)
                            {
                                promote_canonical.push(main[0].0.clone());
                            }
                        } else {
                            if link_flag_libraries.contains(name) {
                                rejected_link_options.push((mmake.clone(), name.clone()));
                            }
                            let declarations: Vec<&str> = c
                                .iter()
                                .map(|(declaration, _)| declaration.as_str())
                                .collect();
                            unresolved.push(format!(
                                "{}: {mmake} uselibs={name} is ambiguous ({})",
                                target.dir_path.display(),
                                declarations.join(", ")
                            ));
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

    pub fn add_adhoc_header_rules(&mut self, rules: Vec<AdhocHeaderRule>) {
        self.adhoc_header_rules.extend(rules);
    }

    pub fn add_copy_includes(&mut self, decls: Vec<CopyIncludesDecl>) {
        for decl in decls {
            let dup = self.copy_includes.iter().any(|d| {
                d.name == decl.name
                    && d.dest == decl.dest
                    && d.source_dir == decl.source_dir
                    && d.patterns == decl.patterns
                    && d.flatten == decl.flatten
            });
            if !dup {
                self.copy_includes.push(decl);
            }
        }
    }

    pub fn add_arch_decls(&mut self, decls: Vec<ArchIncludeDecl>) {
        for decl in decls {
            self.arch_decls
                .entry(decl.modname.clone())
                .or_default()
                .push(decl);
        }
    }

    /// Resolves each target's `%get_archincludes` requests against the
    /// declarations collected from the whole tree.
    ///
    /// The join key is `modname`, not `mainmmake`: `rom/exec` asks for both the
    /// `exec` and the `kernel` architecture includes, and those are declared
    /// under different `mainmmake` values. Results are ordered by `pri`, which
    /// is the order the Make build's wildcard glob produces.
    pub fn resolve_arch_includes(&mut self) {
        for decls in self.arch_decls.values_mut() {
            decls.sort_by(|a, b| a.pri.cmp(&b.pri).then_with(|| a.dir.cmp(&b.dir)));
        }

        for target in self.targets.values_mut() {
            let mut resolved: Vec<(String, String)> = Vec::new();
            for module in &target.arch_modules {
                if let Some(decls) = self.arch_decls.get(module) {
                    for decl in decls {
                        let entry = (decl.tag.clone(), decl.dir.clone());
                        if !resolved.contains(&entry) {
                            resolved.push(entry);
                        }
                    }
                }
            }
            // The target may already carry entries contributed by an
            // architecture make.opts; keep those, appended after the
            // %set_archincludes ones so the priority order is preserved.
            for entry in std::mem::take(&mut target.arch_includes) {
                if !resolved.contains(&entry) {
                    resolved.push(entry);
                }
            }
            target.arch_includes = resolved;
        }
    }

    pub fn add_meta_rule(&mut self, rule: MetaTargetRule) {
        self.meta_targets
            .entry(rule.name)
            .or_default()
            .extend(rule.dependencies);
    }

    /// Flattens strongly connected groups of pure #MM targets.
    ///
    /// GNU Make tolerates a circular prerequisite by dropping an edge during
    /// traversal. CMake rejects the same utility-target cycle at generate
    /// time. A cycle of phony aggregators denotes a shared prerequisite
    /// closure, so every member receives the union of the group's external
    /// dependencies and all internal edges are removed. The result is
    /// deterministic and preserves what building either entry point prepares.
    ///
    /// Returns one report line per flattened component.
    pub fn flatten_meta_cycles(&mut self) -> Vec<String> {
        fn visit(
            node: &str,
            graph: &HashMap<String, HashSet<String>>,
            nodes: &HashSet<String>,
            seen: &mut HashSet<String>,
            order: &mut Vec<String>,
        ) {
            if !seen.insert(node.to_owned()) {
                return;
            }
            let mut next: Vec<&str> = graph
                .get(node)
                .into_iter()
                .flatten()
                .map(String::as_str)
                .filter(|dep| nodes.contains(*dep))
                .collect();
            next.sort_unstable();
            for dep in next {
                visit(dep, graph, nodes, seen, order);
            }
            order.push(node.to_owned());
        }

        fn visit_reverse(
            node: &str,
            reverse: &HashMap<String, Vec<String>>,
            seen: &mut HashSet<String>,
            component: &mut Vec<String>,
        ) {
            if !seen.insert(node.to_owned()) {
                return;
            }
            component.push(node.to_owned());
            if let Some(next) = reverse.get(node) {
                for dep in next {
                    visit_reverse(dep, reverse, seen, component);
                }
            }
        }

        let nodes: HashSet<String> = self.meta_targets.keys().cloned().collect();
        let mut names: Vec<String> = nodes.iter().cloned().collect();
        names.sort_unstable();
        let mut seen = HashSet::new();
        let mut order = Vec::with_capacity(names.len());
        for name in &names {
            visit(name, &self.meta_targets, &nodes, &mut seen, &mut order);
        }

        let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
        for (from, deps) in &self.meta_targets {
            for to in deps.iter().filter(|dep| nodes.contains(*dep)) {
                reverse.entry(to.clone()).or_default().push(from.clone());
            }
        }
        for incoming in reverse.values_mut() {
            incoming.sort_unstable();
            incoming.dedup();
        }

        seen.clear();
        let mut components = Vec::new();
        while let Some(name) = order.pop() {
            if seen.contains(&name) {
                continue;
            }
            let mut component = Vec::new();
            visit_reverse(&name, &reverse, &mut seen, &mut component);
            component.sort_unstable();
            let cyclic = component.len() > 1
                || self
                    .meta_targets
                    .get(&component[0])
                    .is_some_and(|deps| deps.contains(&component[0]));
            if cyclic {
                components.push(component);
            }
        }

        components.sort();
        let mut reports = Vec::new();
        for component in components {
            let members: HashSet<&str> = component.iter().map(String::as_str).collect();
            let mut external = BTreeSet::new();
            for name in &component {
                if let Some(deps) = self.meta_targets.get(name) {
                    external.extend(
                        deps.iter()
                            .filter(|dep| !members.contains(dep.as_str()))
                            .cloned(),
                    );
                }
            }
            for name in &component {
                if let Some(deps) = self.meta_targets.get_mut(name) {
                    deps.retain(|dep| !members.contains(dep.as_str()));
                    deps.extend(external.iter().cloned());
                }
            }
            let kind = if component
                .iter()
                .any(|name| self.targets.contains_key(name) || self.icon_targets.contains_key(name))
            {
                "build/meta"
            } else {
                "meta"
            };
            reports.push(format!(
                "flattened {kind} cycle [{}]; shared external dependencies [{}]",
                component.join(" -> "),
                external.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        reports
    }

    /// Verifies that no cyclic dependencies exist in the graph.
    ///
    /// # Errors
    /// Returns an error if a dependency cycle is detected.
    pub fn validate_cycles(&self) -> Result<()> {
        let mut visited = HashSet::new();
        let mut rec_stack = HashSet::new();

        for target_name in self.targets.keys() {
            if !visited.contains(target_name) {
                self.check_cycle(target_name, &mut visited, &mut rec_stack)?;
            }
        }

        Ok(())
    }

    fn check_cycle(
        &self,
        node: &str,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
    ) -> Result<()> {
        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());

        if let Some(target) = self.targets.get(node) {
            for dep in &target.dependencies {
                if visited.contains(dep) {
                    if rec_stack.contains(dep) {
                        return Err(ArosError::DependencyCycle {
                            target: format!("{node} -> {dep}"),
                        });
                    }
                } else if self.targets.contains_key(dep) {
                    self.check_cycle(dep, visited, rec_stack)?;
                }
            }
        }

        rec_stack.remove(node);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        arch_compatible, define_header_compile_targets, target_runtime_name, DependencyGraph,
    };
    use crate::ast::{DefineHeaderDecl, MetaTargetRule, ModuleType};
    use crate::copy_includes::CopyIncludesDecl;
    use crate::dirs::DirVars;
    use crate::packages::{PackageDecl, ResolvedPackageMember};
    use crate::{parse_mmakefile_with_dirs, parse_mmakefile_with_dirs_and_context, TargetContext};
    use std::collections::HashSet;
    use std::path::Path;
    use walkdir::WalkDir;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
    }

    #[test]
    fn identical_header_copies_keep_each_distinct_mmake_owner() {
        let first = CopyIncludesDecl {
            name: "first-includes".to_owned(),
            dest: "GL".to_owned(),
            source_dir: "${AROS_PORTS_DIR}/example/include/GL".to_owned(),
            patterns: vec!["gl.h".to_owned()],
            flatten: true,
        };
        let mut second = first.clone();
        second.name = "second-includes".to_owned();

        let mut graph = DependencyGraph::new();
        graph.add_copy_includes(vec![first.clone(), first, second]);

        assert_eq!(graph.copy_includes.len(), 2);
        assert_eq!(graph.copy_includes[0].name, "first-includes");
        assert_eq!(graph.copy_includes[1].name, "second-includes");
    }

    fn package_graph(
        target_file: &str,
        package_file: &str,
        kind: &str,
        name: &str,
    ) -> DependencyGraph {
        let root = root();
        let dirs = DirVars::load(&root);
        let parsed = parse_mmakefile_with_dirs(&root.join(target_file), &root, &dirs).unwrap();
        let mut graph = DependencyGraph::new();
        for target in parsed.targets {
            graph.add_target(target);
        }
        graph.add_packages(vec![PackageDecl {
            file: package_file.to_owned(),
            mmake: "test-package".to_owned(),
            output: "${AROS_BOOT_ARCH_DIR}/test.pkg".to_owned(),
            members: vec![(kind.to_owned(), name.to_owned())],
            startup: None,
            uselibs: Vec::new(),
            is_kickstart: false,
            resolved: Vec::new(),
            arch: String::new(),
        }]);
        graph
    }

    #[test]
    fn an_abi_skeleton_is_a_linklib_provider_but_never_a_runtime_member() {
        let root = root();
        let dirs = DirVars::load(&root);
        let parsed = parse_mmakefile_with_dirs(
            &root.join("rom/bluetooth/classes/mmakefile.src"),
            &root,
            &dirs,
        )
        .unwrap();
        let abi = parsed
            .targets
            .into_iter()
            .find(|target| target.mmake_name == "kernel-bluetooth-btclass")
            .expect("btclass ABI target");
        assert_eq!(target_runtime_name(&abi), None);

        let mut consumer = parse_mmakefile_with_dirs(
            &root.join("workbench/libs/version/mmakefile.src"),
            &root,
            &dirs,
        )
        .unwrap()
        .targets
        .into_iter()
        .find(|target| target.mmake_name == "workbench-libs-version")
        .expect("version target");
        consumer.mmake_name = "test-abi-consumer".to_owned();
        consumer.use_libs = vec!["btclass".to_owned()];

        let mut graph = DependencyGraph::new();
        graph.add_target(abi);
        graph.add_target(consumer);
        let unresolved = graph.resolve_use_libs();
        assert!(unresolved.is_empty(), "{unresolved:#?}");
        assert_eq!(
            graph.targets["test-abi-consumer"].link_libs,
            ["kernel-bluetooth-btclass-linklib"]
        );

        graph.add_packages(vec![PackageDecl {
            file: "test/mmakefile.src".to_owned(),
            mmake: "test-package".to_owned(),
            output: "${AROS_BOOT_ARCH_DIR}/test.pkg".to_owned(),
            members: vec![("library".to_owned(), "btclass".to_owned())],
            startup: None,
            uselibs: Vec::new(),
            is_kickstart: false,
            resolved: Vec::new(),
            arch: String::new(),
        }]);
        let unresolved = graph.resolve_packages();
        assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
        assert!(unresolved[0].contains("(btclass.library) has no target"));
        assert!(graph.packages[0].resolved.is_empty());
    }

    #[test]
    fn a_library_provides_its_module_and_explicit_linklib_names() {
        let root = root();
        let dirs = DirVars::load(&root);
        let parsed = parse_mmakefile_with_dirs_and_context(
            &root.join("workbench/libs/z/mmakefile.src"),
            &root,
            &dirs,
            &TargetContext {
                cpu: Some("x86_64".to_owned()),
                platform: Some("pc".to_owned()),
                family: Some(String::new()),
                variant: Some(String::new()),
                toolchain: Some("llvm".to_owned()),
                cpu32: Some("i386".to_owned()),
                use_mmu: Some("1".to_owned()),
                float_abi: Some(String::new()),
            },
        )
        .unwrap();

        let mut alias_consumer = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "workbench-libs-z-minigzip")
            .expect("minigzip")
            .clone();
        alias_consumer.mmake_name = "z-alias-consumer".to_owned();
        alias_consumer.use_libs = vec!["z".to_owned()];
        alias_consumer.link_libs.clear();

        let mut external_flag_consumer = alias_consumer.clone();
        external_flag_consumer.mmake_name = "raw-external-consumer".to_owned();
        external_flag_consumer.use_libs.clear();
        external_flag_consumer.link_options = vec!["-lprivate-port-runtime".to_owned()];

        let mut graph = DependencyGraph::new();
        for target in parsed.targets {
            graph.add_target(target);
        }
        graph.add_target(alias_consumer);
        graph.add_target(external_flag_consumer);
        for relative in [
            "compiler/crt/posixc/mmakefile.src",
            "compiler/crt/stdc/mmakefile.src",
            "compiler/pthread/mmakefile.src",
        ] {
            let provider = parse_mmakefile_with_dirs_and_context(
                &root.join(relative),
                &root,
                &dirs,
                &TargetContext {
                    cpu: Some("x86_64".to_owned()),
                    platform: Some("pc".to_owned()),
                    family: Some(String::new()),
                    variant: Some(String::new()),
                    toolchain: Some("llvm".to_owned()),
                    cpu32: Some("i386".to_owned()),
                    use_mmu: Some("1".to_owned()),
                    float_abi: Some(String::new()),
                },
            )
            .unwrap();
            for mut target in provider.targets.into_iter().filter(|target| {
                matches!(
                    target.mmake_name.as_str(),
                    "compiler-posixc" | "compiler-stdc" | "linklibs-pthread"
                )
            }) {
                if target.mmake_name != "linklibs-pthread" {
                    target.use_libs.clear();
                }
                graph.add_target(target);
            }
        }
        let unresolved = graph.resolve_use_libs();
        assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
        assert!(
            unresolved[0].contains(
                "raw-external-consumer link option -lprivate-port-runtime has no link library"
            ),
            "{unresolved:#?}"
        );
        assert_eq!(
            graph.targets["workbench-libs-z-minigzip"].link_libs,
            ["workbench-libs-z-linklib"]
        );
        assert_eq!(
            graph.targets["z-alias-consumer"].link_libs,
            ["workbench-libs-z-linklib"]
        );
        assert!(graph.targets["raw-external-consumer"].link_libs.is_empty());
        assert!(graph.targets["raw-external-consumer"]
            .link_options
            .is_empty());
        assert!(graph
            .meta_targets
            .get("raw-external-consumer")
            .is_none_or(HashSet::is_empty));
        assert_eq!(
            graph.targets["workbench-libs-z"].link_libs,
            ["compiler-posixc-linklib-rel", "compiler-stdc-linklib-rel"]
        );
        for consumer in [
            "workbench-libs-z",
            "workbench-libs-z-minigzip",
            "z-alias-consumer",
        ] {
            assert!(graph.meta_targets[consumer].contains("linklibs-pthread"));
            assert_eq!(graph.targets[consumer].link_options, ["-lpthread"]);
        }
        assert!(
            graph.targets["compiler-posixc"]
                .genmodule_linklibs
                .as_ref()
                .unwrap()
                .enabled
        );
        assert!(
            graph.targets["compiler-stdc"]
                .genmodule_linklibs
                .as_ref()
                .unwrap()
                .enabled
        );
        assert!(graph.targets["linklibs-pthread"].canonical_linklib_output);
    }

    #[test]
    fn zlib_sources_and_transformed_header_have_direct_fetch_edges() {
        let root = root();
        let dirs = DirVars::load(&root);
        let parsed = parse_mmakefile_with_dirs_and_context(
            &root.join("workbench/libs/z/mmakefile.src"),
            &root,
            &dirs,
            &TargetContext {
                cpu: Some("x86_64".to_owned()),
                platform: Some("pc".to_owned()),
                family: Some(String::new()),
                variant: Some(String::new()),
                toolchain: Some("llvm".to_owned()),
                cpu32: Some("i386".to_owned()),
                use_mmu: Some("1".to_owned()),
                float_abi: Some(String::new()),
            },
        )
        .unwrap();

        let mut graph = DependencyGraph::new();
        for target in parsed.targets {
            graph.add_target(target);
        }
        graph.add_fetches(parsed.fetches);
        graph.add_header_transforms(parsed.header_transforms);
        assert!(graph.resolve_port_source_fetches().is_empty());
        for target in [
            "workbench-libs-z",
            "linklibs-z-static",
            "linklibs-z-nogzip-static",
            "workbench-libs-z-minigzip",
        ] {
            assert!(
                graph.meta_targets[target].contains("zlib-fetch"),
                "{target}"
            );
        }

        assert!(graph.resolve_header_transforms().is_empty());
        assert_eq!(graph.header_transforms.len(), 1);
        let transform = &graph.header_transforms[0];
        assert_eq!(transform.dependencies, ["zlib-fetch"]);
        assert_eq!(
            transform.consumers,
            [
                "linklibs-z-nogzip-static",
                "linklibs-z-static",
                "workbench-libs-z",
                "workbench-libs-z-linklib",
                "workbench-libs-z-minigzip",
            ]
        );
    }

    #[test]
    fn atheros_hal_header_has_direct_provider_and_device_edges() {
        let root = root();
        let dirs = DirVars::load(&root);
        let context = TargetContext {
            cpu: Some("x86_64".to_owned()),
            platform: Some("pc".to_owned()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("llvm".to_owned()),
            cpu32: Some("i386".to_owned()),
            use_mmu: Some("1".to_owned()),
            float_abi: Some(String::new()),
        };
        let mut graph = DependencyGraph::new();
        for relative in [
            "workbench/devs/networks/atheros5000/hal/mmakefile.src",
            "workbench/devs/networks/atheros5000/mmakefile.src",
        ] {
            let parsed =
                parse_mmakefile_with_dirs_and_context(&root.join(relative), &root, &dirs, &context)
                    .unwrap();
            for target in parsed.targets {
                graph.add_target(target);
            }
            for rule in parsed.meta_rules {
                graph.add_meta_rule(rule);
            }
            graph.add_define_headers(parsed.define_headers);
        }

        let unresolved = graph.resolve_use_libs();
        assert!(unresolved.is_empty(), "{unresolved:#?}");
        assert_eq!(
            graph.targets["workbench-devs-networks-atheros5000"].link_libs,
            ["workbench-devs-networks-atheros5000-hal"]
        );
        let unresolved = graph.resolve_define_headers();
        assert!(unresolved.is_empty(), "{unresolved:#?}");
        assert_eq!(graph.define_headers.len(), 1);
        assert_eq!(
            graph.define_headers[0].consumers,
            [
                "workbench-devs-networks-atheros5000",
                "workbench-devs-networks-atheros5000-hal",
            ]
        );
        for consumer in [
            "workbench-devs-networks-atheros5000",
            "workbench-devs-networks-atheros5000-hal",
        ] {
            assert!(graph.meta_targets[consumer]
                .contains("workbench-devs-networks-atheros5000-hal-opts"));
        }
    }

    #[test]
    fn define_header_program_group_consumers_expand_to_compile_members() {
        let root = root();
        let dirs = DirVars::load(&root);
        let context = TargetContext {
            cpu: Some("x86_64".to_owned()),
            platform: Some("pc".to_owned()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("llvm".to_owned()),
            cpu32: Some("i386".to_owned()),
            use_mmu: Some("1".to_owned()),
            float_abi: Some(String::new()),
        };
        let hal = parse_mmakefile_with_dirs_and_context(
            &root.join("workbench/devs/networks/atheros5000/hal/mmakefile.src"),
            &root,
            &dirs,
            &context,
        )
        .unwrap();
        let mut programs = parse_mmakefile_with_dirs_and_context(
            &root.join("tools/dtdesc/mmakefile.src"),
            &root,
            &dirs,
            &context,
        )
        .unwrap()
        .targets
        .into_iter()
        .find(|target| target.mmake_name == "tools-dtdesc")
        .expect("dtdesc program group");
        assert_eq!(programs.module_type, ModuleType::ProgramGroup);
        programs.link_libs = vec!["workbench-devs-networks-atheros5000-hal".to_owned()];

        let mut graph = DependencyGraph::new();
        for target in hal.targets {
            graph.add_target(target);
        }
        graph.add_define_headers(hal.define_headers);
        graph.add_target(programs);

        let unresolved = graph.resolve_define_headers();
        assert!(unresolved.is_empty(), "{unresolved:#?}");
        assert_eq!(
            graph.define_headers[0].consumers,
            [
                "tools-dtdesc-createdtdesc",
                "tools-dtdesc-examinedtdesc",
                "workbench-devs-networks-atheros5000-hal",
            ]
        );
        assert!(!graph.meta_targets.contains_key("tools-dtdesc"));
        for member in ["tools-dtdesc-createdtdesc", "tools-dtdesc-examinedtdesc"] {
            assert!(
                graph.meta_targets[member].contains("workbench-devs-networks-atheros5000-hal-opts")
            );
        }
    }

    #[test]
    fn genmodule_only_library_mmake_id_is_still_a_compile_target() {
        let root = root();
        let dirs = DirVars::load(&root);
        let target = parse_mmakefile_with_dirs(
            &root.join("workbench/libs/version/mmakefile.src"),
            &root,
            &dirs,
        )
        .unwrap()
        .targets
        .into_iter()
        .find(|target| target.mmake_name == "workbench-libs-version")
        .expect("version genmodule-only library");
        assert_eq!(target.module_type, ModuleType::Library);
        assert!(target.genmodule_only);
        assert_eq!(
            define_header_compile_targets(&target.mmake_name, &target),
            ["workbench-libs-version"]
        );
    }

    #[test]
    fn define_header_without_a_concrete_provider_stays_unresolved() {
        let mut graph = DependencyGraph::new();
        graph.add_define_headers(vec![DefineHeaderDecl {
            owner: "example-options".to_owned(),
            file: "example/options.mk".to_owned(),
            line: 7,
            output: "${AROS_BUILD_DIR}/example/options.h".to_owned(),
            definitions: vec!["EXAMPLE 1".to_owned()],
            dependencies: vec!["${CMAKE_SOURCE_DIR}/example/options.mk".to_owned()],
            provider: "missing-provider".to_owned(),
            consumers: Vec::new(),
        }]);

        let unresolved = graph.resolve_define_headers();
        assert_eq!(
            unresolved,
            ["example/options.mk:7: example-options provider missing-provider has no concrete target"]
        );
        assert!(graph.meta_targets.is_empty());
        assert!(graph.define_headers[0].consumers.is_empty());
    }

    #[test]
    fn a_meta_cycle_becomes_one_shared_external_dependency_closure() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "a".to_owned(),
            dependencies: vec!["b".to_owned(), "x".to_owned()],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: "b".to_owned(),
            dependencies: vec!["a".to_owned(), "y".to_owned()],
        });

        let reports = graph.flatten_meta_cycles();
        assert_eq!(reports.len(), 1);
        let expected: HashSet<String> = ["x", "y"].into_iter().map(str::to_owned).collect();
        assert_eq!(graph.meta_targets["a"], expected);
        assert_eq!(graph.meta_targets["b"], expected);
    }

    #[test]
    fn an_acyclic_meta_graph_is_unchanged() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "a".to_owned(),
            dependencies: vec!["b".to_owned()],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: "b".to_owned(),
            dependencies: vec!["leaf".to_owned()],
        });
        let before = graph.meta_targets.clone();
        assert!(graph.flatten_meta_cycles().is_empty());
        assert_eq!(graph.meta_targets, before);
    }

    #[test]
    fn a_meta_self_loop_is_removed_without_losing_other_dependencies() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "test".to_owned(),
            dependencies: vec!["test".to_owned(), "test-leaf".to_owned()],
        });

        let reports = graph.flatten_meta_cycles();
        assert_eq!(reports.len(), 1);
        assert_eq!(
            graph.meta_targets["test"],
            std::iter::once("test-leaf".to_owned()).collect()
        );
    }

    #[test]
    fn port_sources_depend_directly_on_the_longest_fetch_destination_owner() {
        let root = root();
        let dirs = DirVars::load(&root);
        let target = TargetContext {
            cpu: Some("x86_64".to_owned()),
            platform: Some("pc".to_owned()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("llvm".to_owned()),
            cpu32: Some("i386".to_owned()),
            use_mmu: Some("1".to_owned()),
            float_abi: Some(String::new()),
        };
        let parsed = parse_mmakefile_with_dirs_and_context(
            &root.join("workbench/libs/png/mmakefile.src"),
            &root,
            &dirs,
            &target,
        )
        .unwrap();

        let mut graph = DependencyGraph::new();
        for target in parsed.targets.clone() {
            graph.add_target(target);
        }
        graph.add_fetches(parsed.fetches.clone());
        assert!(graph.resolve_port_source_fetches().is_empty());
        assert!(graph.meta_targets["workbench-libs-png"].contains("libpng-fetch"));
        assert!(graph.meta_targets["linklibs-png-nostdio"].contains("libpng-fetch"));

        let mut consumer = parsed
            .targets
            .into_iter()
            .find(|target| target.mmake_name == "workbench-libs-png")
            .unwrap();
        consumer.mmake_name = "synthetic-consumer".to_owned();
        consumer.source_files = vec![
            "${AROS_PORTS_DIR}/libpng/version/source".to_owned(),
            "${AROS_PORTS_DIR}/ownerless/source".to_owned(),
        ];
        consumer.cxx_source_files.clear();
        consumer.objc_source_files.clear();
        consumer.asm_source_files.clear();

        let template = parsed.fetches.into_iter().next().unwrap();
        let mut broad = template.clone();
        broad.name = "libpng-broad-fetch".to_owned();
        broad.destination = "${AROS_PORTS_DIR}/libpng".to_owned();
        let mut narrow = template;
        narrow.name = "libpng-narrow-fetch".to_owned();
        narrow.destination = "${AROS_PORTS_DIR}/libpng/version".to_owned();

        let mut graph = DependencyGraph::new();
        graph.add_target(consumer);
        graph.add_fetches(vec![broad, narrow]);
        assert_eq!(
            graph.resolve_port_source_fetches(),
            ["synthetic-consumer|${AROS_PORTS_DIR}/ownerless/source"]
        );
        assert_eq!(
            graph.meta_targets["synthetic-consumer"],
            std::iter::once("libpng-narrow-fetch".to_owned()).collect()
        );
    }

    #[test]
    fn wider_cpus_accept_their_32_bit_compatible_candidates() {
        for (ctx_cpu, cand_cpu) in [("x86_64", "i386"), ("aarch64", "arm"), ("riscv64", "riscv")] {
            let candidate = (cand_cpu.to_owned(), "pc".to_owned());
            let context = (ctx_cpu.to_owned(), "pc".to_owned());
            assert!(arch_compatible(Some(&candidate), Some(&context)));
            assert!(!arch_compatible(Some(&context), Some(&candidate)));
        }
    }

    #[test]
    fn a_unique_foreign_package_candidate_is_rejected() {
        let mut graph = package_graph(
            "arch/all-linux/hidd/linuxinput/mmakefile.src",
            "arch/x86_64-pc/boot/mmakefile.src",
            "hidd",
            "linuxinput",
        );
        let unresolved = graph.resolve_packages();
        assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
        assert!(unresolved[0].contains("no target for this architecture"));
        assert!(graph.packages[0].resolved.is_empty());
    }

    #[test]
    fn a_unique_compatible_32_bit_candidate_is_accepted() {
        let mut graph = package_graph(
            "arch/i386-pc/drivers/serial.hidd/mmakefile.src",
            "arch/x86_64-pc/boot/mmakefile.src",
            "hidd",
            "serial",
        );
        let unresolved = graph.resolve_packages();
        assert!(unresolved.is_empty(), "{unresolved:#?}");
        assert_eq!(
            graph.packages[0].resolved[0].target,
            "kernel-pc-i386-serial"
        );
    }

    #[test]
    fn one_explicit_foreign_package_candidate_is_accepted() {
        let mut graph = package_graph(
            "arch/all-linux/hidd/linuxinput/mmakefile.src",
            "arch/x86_64-pc/boot/mmakefile.src",
            "hidd",
            "linuxinput",
        );
        graph.add_meta_rule(MetaTargetRule {
            name: "test-package".to_owned(),
            dependencies: vec!["kernel-hidd-linuxinput-kobj".to_owned()],
        });
        let unresolved = graph.resolve_packages();
        assert!(unresolved.is_empty(), "{unresolved:#?}");
        assert_eq!(
            graph.packages[0].resolved[0].target,
            "kernel-hidd-linuxinput"
        );
    }

    #[test]
    fn multiple_explicit_candidates_are_still_architecture_filtered() {
        let mut graph = package_graph(
            "arch/i386-pc/drivers/serial.hidd/mmakefile.src",
            "arch/x86_64-pc/boot/mmakefile.src",
            "hidd",
            "serial",
        );
        let root = root();
        let dirs = DirVars::load(&root);
        let parsed = parse_mmakefile_with_dirs(
            &root.join("arch/m68k-amiga/hidd/serial/mmakefile.src"),
            &root,
            &dirs,
        )
        .unwrap();
        for target in parsed.targets {
            graph.add_target(target);
        }
        graph.add_meta_rule(MetaTargetRule {
            name: "test-package".to_owned(),
            dependencies: vec![
                "kernel-pc-i386-serial-kobj".to_owned(),
                "amiga-m68k-hidd-serial-kobj".to_owned(),
            ],
        });

        let unresolved = graph.resolve_packages();
        assert!(unresolved.is_empty(), "{unresolved:#?}");
        assert_eq!(
            graph.packages[0].resolved[0].target,
            "kernel-pc-i386-serial"
        );
    }

    #[test]
    fn package_targets_and_runtime_names_stay_aligned() {
        let root = root();
        let dirs = DirVars::load(&root);
        let mut graph = DependencyGraph::new();
        for relative in [
            "rom/filesys/ram/mmakefile.src",
            "rom/log/serial/mmakefile.src",
            "rom/usb/classes/bootkeyboard/mmakefile.src",
            "rom/usb/classes/hid/mmakefile.src",
            "workbench/devs/USB/classes/HID/mmakefile.src",
        ] {
            let parsed = parse_mmakefile_with_dirs(&root.join(relative), &root, &dirs).unwrap();
            for target in parsed.targets {
                graph.add_target(target);
            }
        }
        graph.add_packages(vec![PackageDecl {
            file: "rom/test/mmakefile.src".to_owned(),
            mmake: "test-package".to_owned(),
            output: "${AROS_BOOT_DIR}/test.pkg".to_owned(),
            members: vec![
                ("handler".to_owned(), "ram".to_owned()),
                ("logger".to_owned(), "serial".to_owned()),
                ("class".to_owned(), "USB/bootkeyboard".to_owned()),
                ("class".to_owned(), "USB/hid".to_owned()),
                ("handler".to_owned(), "missing".to_owned()),
                // `$^` removes this duplicate producer. Its whole pair must
                // disappear, not just the target side.
                ("handler".to_owned(), "ram".to_owned()),
            ],
            startup: None,
            uselibs: Vec::new(),
            is_kickstart: false,
            resolved: Vec::new(),
            arch: String::new(),
        }]);

        let unresolved = graph.resolve_packages();
        assert_eq!(unresolved.len(), 1, "{unresolved:#?}");
        assert!(unresolved[0].contains("handler=missing"));
        assert_eq!(
            graph.packages[0].resolved,
            vec![
                ResolvedPackageMember {
                    target: "kernel-fs-ram".to_owned(),
                    runtime_name: "ram-handler".to_owned(),
                },
                ResolvedPackageMember {
                    target: "kernel-log-serial".to_owned(),
                    runtime_name: "serial.logger".to_owned(),
                },
                ResolvedPackageMember {
                    target: "kernel-usb-classes-bootkeyboard".to_owned(),
                    runtime_name: "bootkeyboard.class".to_owned(),
                },
                ResolvedPackageMember {
                    target: "kernel-usb-classes-hid".to_owned(),
                    runtime_name: "hid.class".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn real_tree_packages_resolve_to_exact_runtime_files() {
        let root = root();
        let dirs = DirVars::load(&root);
        let target = TargetContext {
            cpu: Some("x86_64".to_owned()),
            platform: Some("pc".to_owned()),
            family: Some(String::new()),
            variant: Some(String::new()),
            toolchain: Some("llvm".to_owned()),
            cpu32: Some("i386".to_owned()),
            use_mmu: Some("1".to_owned()),
            float_abi: Some(String::new()),
        };
        let skip_dirs = ["build", "target", ".git"];
        let mut files: Vec<_> = WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| {
                !entry.file_type().is_dir()
                    || entry.depth() == 0
                    || !skip_dirs
                        .iter()
                        .any(|dir| entry.file_name().to_string_lossy() == *dir)
            })
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name() == "mmakefile.src")
            .map(walkdir::DirEntry::into_path)
            .collect();
        files.sort();

        let mut graph = DependencyGraph::new();
        for file in files {
            let parsed =
                parse_mmakefile_with_dirs_and_context(&file, &root, &dirs, &target).unwrap();
            for target in parsed.targets {
                graph.add_target(target);
            }
            graph.add_packages(parsed.packages);
            for rule in parsed.meta_rules {
                graph.add_meta_rule(rule);
            }
        }

        let unresolved = graph.resolve_packages();
        assert_eq!(
            unresolved,
            vec![concat!(
                "arch/ppc-chrp/efika/boot/mmakefile.src: ",
                "kernel-package-chrp-ppc-usb class=USB/storage ",
                "(storage.class) has no target"
            )
            .to_owned()]
        );

        let packages: Vec<_> = graph
            .packages
            .iter()
            .filter(|package| !package.is_kickstart)
            .collect();
        let kickstarts: Vec<_> = graph
            .packages
            .iter()
            .filter(|package| package.is_kickstart)
            .collect();
        assert_eq!(packages.len(), 17);
        assert_eq!(kickstarts.len(), 4);
        assert_eq!(
            packages
                .iter()
                .map(|package| package.resolved.len())
                .sum::<usize>(),
            397
        );
        assert_eq!(
            kickstarts
                .iter()
                .map(|package| package.resolved.len())
                .sum::<usize>(),
            19
        );

        for package in &graph.packages {
            for member in &package.resolved {
                let target = &graph.targets[&member.target];
                assert_eq!(
                    target_runtime_name(target).as_deref(),
                    Some(member.runtime_name.as_str()),
                    "{}: {}",
                    package.mmake,
                    member.target
                );
            }
        }
    }
}
