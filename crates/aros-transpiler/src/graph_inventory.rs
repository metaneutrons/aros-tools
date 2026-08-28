//! Target/catalog/fetch/header inventory resolution.

use super::{
    catalog_compile_target_for_source, define_header_compile_targets, normalize_root_relative_path,
    relative_catalog_source_parent, source_is_flexcat_product, source_shares_logical_directory,
    AhiBuildDecl, BTreeSet, CatalogDecl, ConfigureBuildDecl, DefineHeaderDecl, DependencyGraph,
    ExternalCMakeDecl, FetchDecl, FlexCatHeaderDecl, FlexCatSourceDecl, GrubBuildDecl, HashMap,
    HashSet, HeaderTransformDecl, IconSet, IconTarget, IlbmSourceDecl, ModuleType, Path,
    PythonOutputsDecl, TargetDefinition,
};

impl DependencyGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_target(&mut self, target: TargetDefinition) {
        self.targets.insert(target.mmake_name.clone(), target);
    }

    pub fn add_external_cmake(&mut self, declaration: ExternalCMakeDecl) {
        if !self
            .external_cmake
            .iter()
            .any(|existing| existing.mmake_name == declaration.mmake_name)
        {
            self.external_cmake.push(declaration);
        }
    }

    pub fn add_configure_build(&mut self, declaration: ConfigureBuildDecl) {
        if !self
            .configure_builds
            .iter()
            .any(|existing| existing.mmake_name == declaration.mmake_name)
        {
            self.configure_builds.push(declaration);
        }
    }

    pub fn add_grub_build(&mut self, declaration: GrubBuildDecl) {
        if !self
            .grub_builds
            .iter()
            .any(|existing| existing.mmake_name == declaration.mmake_name)
        {
            self.grub_builds.push(declaration);
        }
    }

    pub fn add_ahi_build(&mut self, declaration: AhiBuildDecl) {
        if !self
            .ahi_builds
            .iter()
            .any(|existing| existing.mmake_name == declaration.mmake_name)
        {
            self.ahi_builds.push(declaration);
        }
    }

    pub fn add_python_outputs(&mut self, declaration: PythonOutputsDecl) {
        if !self
            .python_outputs
            .iter()
            .any(|existing| existing.owner == declaration.owner)
        {
            self.python_outputs.push(declaration);
        }
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

    pub fn add_flexcat_sources(&mut self, declarations: Vec<FlexCatSourceDecl>) {
        for declaration in declarations {
            if !self
                .flexcat_sources
                .iter()
                .any(|existing| existing.owner == declaration.owner)
            {
                self.flexcat_sources.push(declaration);
            }
        }
    }

    pub fn add_flexcat_headers(&mut self, declarations: Vec<FlexCatHeaderDecl>) {
        for declaration in declarations {
            if !self.flexcat_headers.iter().any(|existing| {
                existing.owner == declaration.owner
                    && existing.declaring_dir == declaration.declaring_dir
                    && existing.header == declaration.header
            }) {
                self.flexcat_headers.push(declaration);
            }
        }
    }

    pub fn add_ilbm_sources(&mut self, declarations: Vec<IlbmSourceDecl>) {
        for declaration in declarations {
            if !self.ilbm_sources.iter().any(|existing| {
                existing.owner == declaration.owner
                    && existing.declaring_dir == declaration.declaring_dir
                    && existing.line == declaration.line
            }) {
                self.ilbm_sources.push(declaration);
            }
        }
    }

    /// Finds concrete compilation targets which require a relative
    /// `%build_catalogs source=` output.
    ///
    /// MetaMake generates such outputs next to their logical source files
    /// (for example `catalogs/../strings.h`). CMake deliberately rehomes them
    /// below its generated tree, so the source-tree relationship needs to be
    /// recorded before that move. Matching exact logical directories avoids a
    /// speculative include scan while covering both a module's own `locale`
    /// source and sibling declarations that compile `../locale`.
    ///
    /// Program groups are expanded to the individual executable that compiles
    /// the matching source. All resulting names are sorted and unique so
    /// generated CMake remains deterministic despite the graph's HashMap
    /// target storage.
    pub fn resolve_catalog_consumers(&mut self) {
        let targets = &self.targets;
        for catalog in &mut self.catalogs {
            let Some(source_parent) = relative_catalog_source_parent(catalog) else {
                catalog.consumers.clear();
                continue;
            };

            let mut consumers = BTreeSet::new();
            for target in targets.values() {
                let mut matches_source = |directory: &Path, source: &str| {
                    if source_shares_logical_directory(directory, source, &source_parent) {
                        if let Some(consumer) = catalog_compile_target_for_source(target, source) {
                            consumers.insert(consumer);
                        }
                    }
                };

                for source in target
                    .source_files
                    .iter()
                    .chain(&target.cxx_source_files)
                    .chain(&target.objc_source_files)
                    .chain(&target.asm_source_files)
                {
                    matches_source(&target.dir_path, source);
                }
                for (_, directory, files) in &target.arch_sources {
                    for source in files {
                        matches_source(Path::new(directory), source);
                    }
                }
            }
            catalog.consumers = consumers.into_iter().collect();
        }
    }

    /// Finds the exact concrete compilation target(s) that list a generated
    /// hand-written FlexCat C product. The logical source-tree path is matched
    /// before CMake rehomes the product below `gen/`; no include guessing or
    /// directory-wide dependency is involved.
    pub fn resolve_flexcat_source_consumers(&mut self) {
        let targets = &self.targets;
        for declaration in &mut self.flexcat_sources {
            let Some(expected) = normalize_root_relative_path(
                &Path::new(&declaration.declaring_dir).join(&declaration.source),
            ) else {
                declaration.consumers.clear();
                continue;
            };

            let mut consumers = BTreeSet::new();
            for target in targets.values() {
                let mut matches_source = |directory: &Path, source: &str| {
                    if source_is_flexcat_product(directory, source, &expected) {
                        if let Some(consumer) = catalog_compile_target_for_source(target, source) {
                            consumers.insert(consumer);
                        }
                    }
                };

                for source in target
                    .source_files
                    .iter()
                    .chain(&target.cxx_source_files)
                    .chain(&target.objc_source_files)
                    .chain(&target.asm_source_files)
                {
                    matches_source(&target.dir_path, source);
                }
                for (_, directory, files) in &target.arch_sources {
                    for source in files {
                        matches_source(Path::new(directory), source);
                    }
                }
            }
            declaration.consumers = consumers.into_iter().collect();
        }
    }

    pub fn add_fetches(&mut self, decls: Vec<FetchDecl>) {
        for d in decls {
            if !self.fetches.iter().any(|f| f.name == d.name) {
                self.fetches.push(d);
            }
        }
    }

    /// Binds deferred `${AROS_PORTS_DIR}` wildcard patterns to their most
    /// specific `%fetch` owner.
    pub fn resolve_source_inventory_fetches(&mut self, patterns: &[String]) -> Vec<String> {
        let mut unresolved = Vec::new();
        for pattern in patterns {
            let owner = self
                .fetches
                .iter()
                .filter(|fetch| {
                    pattern == &fetch.destination
                        || pattern
                            .strip_prefix(&fetch.destination)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                })
                .max_by_key(|fetch| fetch.destination.len());
            if let Some(fetch) = owner {
                if !self.source_inventory_fetches.contains(&fetch.name) {
                    self.source_inventory_fetches.push(fetch.name.clone());
                }
            } else {
                unresolved.push(pattern.clone());
            }
        }
        self.source_inventory_fetches.sort();
        unresolved
    }

    /// Adds fetched public-header trees to the configure-time inventory pass.
    ///
    /// The later CMake header-ownership pass follows private includes from
    /// concrete Port sources into `%copy_includes` products. On an empty build
    /// tree neither side exists yet, so a build could fetch a source archive
    /// and start compiling before the public header it includes was staged.
    /// Materialising only fetches that publish headers gives the second
    /// transpiler/configure pass enough source text to derive those exact
    /// edges without prefetching every third-party package.
    pub fn resolve_header_inventory_fetches(&mut self, ports_dir: Option<&Path>) {
        for declaration in &self.copy_includes {
            let source = declaration.source_dir.trim_end_matches('/');
            if let (Some(ports_dir), Some(relative)) =
                (ports_dir, source.strip_prefix("${AROS_PORTS_DIR}"))
            {
                let relative = relative.trim_start_matches('/');
                if ports_dir.join(relative).is_dir() {
                    continue;
                }
            }
            let owner = self
                .fetches
                .iter()
                .filter(|fetch| {
                    let destination = fetch.destination.trim_end_matches('/');
                    source == destination
                        || source
                            .strip_prefix(destination)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                })
                .max_by_key(|fetch| fetch.destination.len());
            if let Some(fetch) = owner {
                if !self.source_inventory_fetches.contains(&fetch.name) {
                    self.source_inventory_fetches.push(fetch.name.clone());
                }
            }
        }
        self.source_inventory_fetches.sort();
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

        // The host-generated-header capability publishes its primary SDK
        // output and mirrors the same file into GENINCDIR in one atomic build
        // rule. GNU Make spells that mirror as a second plain `$(CP)` rule;
        // after optional-`@` copy recognition it would otherwise become a
        // second Ninja producer for the same file. Consolidate only that exact
        // derived copy, leaving every other duplicate to CMake's hard error.
        let host_header_copies: HashSet<(String, String)> = self
            .host_generated_headers
            .iter()
            .map(|header| {
                (
                    format!("${{AROS_SDK_INCLUDE_DIR}}/{}", header.header),
                    format!("${{CMAKE_BINARY_DIR}}/GENINCDIR/{}", header.header),
                )
            })
            .collect();
        self.header_transforms.retain(|transform| {
            !transform.copy_only
                || !host_header_copies
                    .contains(&(transform.input.clone(), transform.output.clone()))
        });

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
            if let Some((fetch, _)) = owner {
                transform.dependencies.push(fetch.clone());
            } else if !transform.input.starts_with("${CMAKE_SOURCE_DIR}/") {
                // Repository-owned inputs are already present when CMake
                // configures. Ports inputs, on the other hand, must have an
                // exact fetch owner or a cache-empty Ninja build could race or
                // use stale material. Other generated inputs need a distinct
                // producer capability and must not be silently accepted here.
                unresolved.push(format!(
                    "{}:{}: {} input {} has no matching %fetch owner",
                    transform.file, transform.line, transform.name, transform.input
                ));
                continue;
            }

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
    /// sources or reading port-owned include trees, and returns paths for
    /// which no `%fetch` owner exists.
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
            for input in target
                .source_files
                .iter()
                .chain(&target.cxx_source_files)
                .chain(&target.objc_source_files)
                .chain(&target.asm_source_files)
                .chain(&target.include_dirs)
            {
                if input != PORTS_ROOT && !input.starts_with("${AROS_PORTS_DIR}/") {
                    continue;
                }
                let owner = owners.iter().copied().find(|fetch| {
                    let destination = fetch.destination.trim_end_matches('/');
                    input == destination
                        || input
                            .strip_prefix(destination)
                            .is_some_and(|tail| tail.starts_with('/'))
                });
                if let Some(owner) = owner {
                    edges.insert((target.mmake_name.clone(), owner.name.clone()));
                } else {
                    unowned.insert(format!("{}|{input}", target.mmake_name));
                }
            }
        }

        for (target, fetch) in edges {
            self.meta_targets.entry(target).or_default().insert(fetch);
        }
        unowned.into_iter().collect()
    }

    /// Joins recursive directory copies sourced from a fetched port to their
    /// one concrete `%fetch` owner.
    ///
    /// Unlike an ordinary #MM edge, this prerequisite belongs on the copy
    /// target itself: a direct `ninja compiler-boost-includes-copy` must not
    /// race an empty ports cache.  Equal-length matching owners are rejected
    /// rather than selected by iteration order, because either archive could
    /// otherwise populate the same source path.
    pub fn resolve_copy_directories(&mut self) -> Vec<String> {
        const PORTS_ROOT: &str = "${AROS_PORTS_DIR}";

        let owners: Vec<&FetchDecl> = self
            .fetches
            .iter()
            .filter(|fetch| {
                let destination = fetch.destination.trim_end_matches('/');
                destination == PORTS_ROOT || destination.starts_with("${AROS_PORTS_DIR}/")
            })
            .collect();
        let mut unresolved = BTreeSet::new();
        let mut resolved = Vec::with_capacity(self.copy_directories.len());

        for mut declaration in std::mem::take(&mut self.copy_directories) {
            let source_is_port = declaration.source == PORTS_ROOT
                || declaration.source.starts_with("${AROS_PORTS_DIR}/");
            if !source_is_port {
                resolved.push(declaration);
                continue;
            }

            let mut matching: Vec<&FetchDecl> = owners
                .iter()
                .copied()
                .filter(|fetch| {
                    let destination = fetch.destination.trim_end_matches('/');
                    declaration.source == destination
                        || declaration
                            .source
                            .strip_prefix(destination)
                            .is_some_and(|tail| tail.starts_with('/'))
                })
                .collect();
            let Some(longest) = matching
                .iter()
                .map(|fetch| fetch.destination.trim_end_matches('/').len())
                .max()
            else {
                unresolved.insert(format!(
                    "{}:{}: {} source {} has no matching %fetch owner",
                    declaration.file, declaration.line, declaration.name, declaration.source
                ));
                continue;
            };
            matching.retain(|fetch| fetch.destination.trim_end_matches('/').len() == longest);
            let matching_names: BTreeSet<String> = matching
                .into_iter()
                .map(|fetch| fetch.name.clone())
                .collect();
            if matching_names.len() != 1 {
                unresolved.insert(format!(
                    "{}:{}: {} source {} has ambiguous %fetch owners [{}]",
                    declaration.file,
                    declaration.line,
                    declaration.name,
                    declaration.source,
                    matching_names.into_iter().collect::<Vec<_>>().join(", ")
                ));
                continue;
            }
            declaration.dependencies = matching_names.into_iter().collect();
            resolved.push(declaration);
        }

        self.copy_directories = resolved;
        unresolved.into_iter().collect()
    }
}
