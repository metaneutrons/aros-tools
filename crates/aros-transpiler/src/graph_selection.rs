//! Source-contract selection of a closed native dependency graph.
//!
//! Discovery remains whole-tree. Selection happens only after cross-file
//! providers, source inventories and link edges have been resolved. An
//! unavailable selected capability or an unowned capability failure is fatal.

#[path = "graph_selection/native_audit.rs"]
mod native_audit;
#[path = "graph_selection/selected_closure.rs"]
mod selected_closure;

use super::{
    arch_compatible, arch_of, has_public_link_archive, inventory_has_public_link_archive,
    inventory_runtime_name, target_runtime_name,
};
use crate::{DependencyGraph, ModuleType, TargetContext};
use aros_common::native_build_contract::{
    NativeBuildContract, NativeMetaAbsence, NativeOptionalMetaDependency,
};
use aros_common::{ArosError, Diagnostic, DiagnosticCode, DiagnosticSet, DiagnosticStage, Result};
use std::collections::{BTreeMap, BTreeSet};

type Edges = BTreeMap<String, BTreeSet<String>>;

fn inventory_archive_failure(owner: &str, name: &str, candidates: usize) -> Diagnostic {
    Diagnostic::error(DiagnosticCode::GraphValidation, DiagnosticStage::GraphValidation,
        format!("preparation archive request {owner}: {name} requires one applicable provider; found {candidates}"))
        .with_context(aros_common::DiagnosticContext { target: Some(owner.to_owned()), ..Default::default() })
}

impl DependencyGraph {
    /// Existing native endpoint identities, not a claim of capability support.
    pub(crate) fn native_metadata_endpoint_names(
        &self,
        context: &TargetContext,
    ) -> BTreeSet<String> {
        self.selection_edges(&[], true, context)
            .edges
            .into_keys()
            .filter_map(|name| endpoint(&name, context).ok())
            .collect()
    }

    /// Required reachability after cutting only independently contracted
    /// selector edges. Includes concrete native/link prerequisites: an alias
    /// reached through such a path cannot inherit another path's optionality.
    pub(crate) fn native_required_metadata_reachability(
        &self,
        roots: &[String],
        context: &TargetContext,
        cuts: &BTreeSet<(String, String)>,
    ) -> std::result::Result<BTreeSet<String>, String> {
        let mut edges = BTreeMap::<String, BTreeSet<String>>::new();
        let concrete_edges: BTreeSet<_> = self
            .selection_edges(&[], false, context)
            .edges
            .into_iter()
            .flat_map(|(parent, children)| {
                let parent = endpoint(&parent, context).ok();
                children.into_iter().filter_map(move |child| {
                    Some((parent.clone()?, endpoint(&child, context).ok()?))
                })
            })
            .collect();
        // Keep unresolved children until traversal: unrelated recipes cannot
        // veto scope, but a reachable unresolved endpoint cannot prove a cut.
        for (parent, children) in self.selection_edges(&[], true, context).edges {
            if let Ok(parent) = endpoint(&parent, context) {
                edges.entry(parent).or_default().extend(children);
            }
        }
        let mut pending: BTreeSet<_> = roots.iter().cloned().collect();
        let mut reached = BTreeSet::new();
        while let Some(name) = pending.pop_first() {
            if !reached.insert(name.clone()) {
                continue;
            }
            if let Some(children) = edges.get(&name) {
                for child in children {
                    let child = endpoint(child, context).map_err(|e| e.to_string())?;
                    let edge = (name.clone(), child.clone());
                    if !cuts.contains(&edge) || concrete_edges.contains(&edge) {
                        pending.insert(child);
                    }
                }
            }
        }
        Ok(reached)
    }

    /// Remove only the exact metadata edge established as an absent source
    /// selector hook. Concrete compile/link prerequisites are untouched.
    pub(crate) fn remove_native_meta_edge(
        &mut self,
        target: &str,
        dependency: &str,
        context: &TargetContext,
    ) {
        for (name, dependencies) in &mut self.meta_targets {
            if endpoint(name, context).ok().as_deref() == Some(target) {
                dependencies
                    .retain(|name| endpoint(name, context).ok().as_deref() != Some(dependency));
            }
        }
        self.explicit_meta_edges.retain(|(name, child)| {
            endpoint(name, context).ok().as_deref() != Some(target)
                || endpoint(child, context).ok().as_deref() != Some(dependency)
        });
    }

    /// Bind typed archive names during preparation using declaration metadata,
    /// not fabricated compilation targets. Full export retains its ordinary
    /// linker resolver and cannot use this projection.
    ///
    /// # Errors
    /// Rejects duplicate declaration identities. Archive failures are returned
    /// as owned diagnostics and rejected only if their declaration is selected.
    pub fn resolve_inventory_link_edges(
        &mut self,
        context: &TargetContext,
    ) -> Result<Vec<Diagnostic>> {
        use crate::ast::{InventoryTargetIdentity, ModuleMacroForm};
        #[derive(Clone, PartialEq, Eq)]
        struct Provider {
            owner: String,
            archive: String,
            arch: Option<(String, String)>,
            variant_32bit: bool,
            external: bool,
            public: bool,
            output_dir: Option<String>,
        }
        let required_relative: BTreeSet<_> = self
            .targets
            .values()
            .map(InventoryTargetIdentity::from)
            .chain(self.inventory_targets.iter().cloned())
            .flat_map(|target| {
                target.config_relative_libraries.into_iter().chain(
                    target
                        .genmodule_linklibs
                        .into_iter()
                        .filter(|metadata| metadata.enabled)
                        .flat_map(|metadata| metadata.relative_libraries),
                )
            })
            .collect();
        for target in self.targets.values_mut() {
            if target.module_type == ModuleType::Library
                && required_relative.contains(&target.target_name)
            {
                if let Some(metadata) = target.genmodule_linklibs.as_mut() {
                    if metadata.has_relative && metadata.inputs_exact {
                        metadata.enabled = true;
                    }
                }
            }
        }
        for target in &mut self.inventory_targets {
            if target.module_type == ModuleType::Library
                && required_relative.contains(&target.target_name)
            {
                if let Some(metadata) = target.genmodule_linklibs.as_mut() {
                    if metadata.has_relative && metadata.inputs_exact {
                        metadata.enabled = true;
                    }
                }
            }
        }
        let declarations: Vec<_> = self
            .targets
            .values()
            .map(InventoryTargetIdentity::from)
            .chain(self.inventory_targets.iter().cloned())
            .collect();
        let mut names = BTreeSet::new();
        for declaration in &declarations {
            if !names.insert(&declaration.mmake_name) {
                return Err(failure(format!(
                    "duplicate preparation declaration {}",
                    declaration.mmake_name
                )));
            }
        }
        let mut providers = BTreeMap::<String, Vec<Provider>>::new();
        for declaration in &declarations {
            let owner = &declaration.mmake_name;
            let mut aliases = Vec::new();
            match declaration.module_type {
                ModuleType::Abi => {
                    aliases.push((declaration.target_name.clone(), format!("{owner}-linklib")));
                    aliases.push((
                        format!("{}_rel", declaration.target_name),
                        format!("{owner}-linklib"),
                    ));
                }
                ModuleType::LinkLib => {
                    aliases.push((declaration.target_name.clone(), owner.clone()));
                }
                ModuleType::Library if inventory_has_public_link_archive(declaration) => {
                    for name in std::iter::once(&declaration.target_name)
                        .chain(declaration.linklib_name.iter())
                    {
                        aliases.push((name.clone(), format!("{owner}-linklib")));
                        if declaration
                            .genmodule_linklibs
                            .as_ref()
                            .is_some_and(|m| m.enabled && m.has_relative)
                        {
                            aliases.push((format!("{name}_rel"), format!("{owner}-linklib-rel")));
                        }
                    }
                }
                _ => {}
            }
            for (name, archive) in aliases {
                let record = Provider {
                    owner: owner.clone(),
                    archive,
                    arch: arch_of(&declaration.dir_path),
                    variant_32bit: declaration.variant_32bit,
                    external: false,
                    public: inventory_has_public_link_archive(declaration),
                    output_dir: declaration.linklib_output_dir.clone(),
                };
                let pool = providers.entry(name).or_default();
                if !pool.contains(&record) {
                    pool.push(record);
                }
            }
        }
        for archive in &self.source_archives {
            let Some(name) = archive
                .archive_basename()
                .strip_prefix("lib")
                .and_then(|name| name.strip_suffix(".a"))
            else {
                continue;
            };
            providers
                .entry(name.to_owned())
                .or_default()
                .push(Provider {
                    owner: archive.declaration.owner.clone(),
                    archive: archive.provider_target(),
                    arch: None,
                    variant_32bit: false,
                    external: false,
                    public: true,
                    output_dir: Some("${AROS_DEVELOPER_LIB_DIR}".into()),
                });
        }
        for (name, owner, archive) in self
            .external_cmake
            .iter()
            .map(|d| (&d.provided_library, &d.mmake_name, &d.provider_target))
            .chain(self.configure_builds.iter().filter_map(|d| {
                d.provided_library
                    .as_ref()
                    .zip(d.provider_target.as_ref())
                    .map(|(name, archive)| (name, &d.mmake_name, archive))
            }))
        {
            providers.entry(name.clone()).or_default().push(Provider {
                owner: owner.clone(),
                archive: archive.clone(),
                arch: None,
                variant_32bit: false,
                external: true,
                public: true,
                output_dir: None,
            });
        }
        let mut failures = Vec::new();
        for declaration in &declarations {
            let mut requested: BTreeMap<String, bool> = declaration
                .use_libs
                .iter()
                .map(|name| (name.clone(), false))
                .collect();
            requested.extend(
                declaration
                    .config_relative_libraries
                    .iter()
                    .map(|name| (format!("{name}_rel"), false)),
            );
            if let Some(metadata) = declaration
                .genmodule_linklibs
                .as_ref()
                .filter(|m| m.enabled)
            {
                requested.extend(
                    metadata
                        .relative_libraries
                        .iter()
                        .map(|name| (format!("{name}_rel"), false)),
                );
            }
            if !matches!(
                declaration.module_type,
                ModuleType::LinkLib | ModuleType::Abi
            ) {
                for name in declaration
                    .link_options
                    .iter()
                    .filter_map(|option| option.strip_prefix("-l"))
                    .filter(|name| !name.is_empty() && !name.starts_with(':'))
                {
                    requested.insert(name.to_owned(), true);
                }
            }
            for (name, raw) in requested {
                let arch = arch_of(&declaration.dir_path).or_else(|| {
                    context
                        .cpu
                        .as_ref()
                        .zip(context.platform.as_ref())
                        .map(|(cpu, platform)| (cpu.clone(), platform.clone()))
                });
                let pool = providers.get(&name).map(Vec::as_slice).unwrap_or_default();
                let applicable: Vec<_> = pool
                    .iter()
                    .filter(|provider| {
                        // A runtime declaration cannot consume the client
                        // archive generated from its own public interface.
                        // That would create a self dependency and obscure an
                        // independently declared implementation archive.
                        provider.owner != declaration.mmake_name
                            && !provider.variant_32bit
                            && arch_compatible(provider.arch.as_ref(), arch.as_ref())
                            && (!raw
                                || !provider.external
                                    && (provider.public
                                        || provider.output_dir.as_ref().is_some_and(|output| {
                                            declaration.link_options.iter().any(|option| {
                                                option.strip_prefix("-L") == Some(output.as_str())
                                            })
                                        })))
                    })
                    .collect();
                let [provider] = applicable.as_slice() else {
                    failures.push(inventory_archive_failure(
                        &declaration.mmake_name,
                        &name,
                        applicable.len(),
                    ));
                    continue;
                };
                let archive = &provider.archive;
                if let Some(target) = self.targets.get_mut(&declaration.mmake_name) {
                    if !target.link_libs.contains(archive) {
                        target.link_libs.push(archive.clone());
                    }
                }
                if let Some(target) = self
                    .inventory_targets
                    .iter_mut()
                    .find(|t| t.mmake_name == declaration.mmake_name)
                {
                    if !target.link_libs.contains(archive) {
                        target.link_libs.push(archive.clone());
                    }
                }
                let generated = format!("linklibs-{name}");
                // Preserve a source-owned spelling that already has a proven
                // archive or recipe endpoint; only implicit macro aliases may
                // be rebound to the typed provider.
                let source_owned = self.meta_targets.contains_key(&generated)
                    || self.make_meta_providers.contains(&generated)
                    || names.contains(&generated)
                    || self.fetches.iter().any(|f| f.name == generated)
                    || self.external_cmake.iter().any(|build| {
                        build.mmake_name == generated || build.provider_target == generated
                    })
                    || self.configure_builds.iter().any(|build| {
                        build.mmake_name == generated
                            || build.provider_target.as_deref() == Some(generated.as_str())
                    })
                    || self
                        .default_link_set
                        .iter()
                        .any(|item| item.archive == generated);
                if !source_owned {
                    let consumers = match declaration.module_macro {
                        Some(ModuleMacroForm::Full) => vec![
                            declaration.mmake_name.clone(),
                            format!("{}-kobj", declaration.mmake_name),
                        ],
                        Some(ModuleMacroForm::RuntimeOnly) => {
                            vec![format!("{}-kobj", declaration.mmake_name)]
                        }
                        Some(ModuleMacroForm::AbiOnly) => vec![declaration.mmake_name.clone()],
                        _ => Vec::new(),
                    };
                    for consumer in consumers {
                        if !self
                            .explicit_meta_edges
                            .contains(&(consumer.clone(), generated.clone()))
                        {
                            if let Some(dependencies) = self.meta_targets.get_mut(&consumer) {
                                if dependencies.remove(&generated) {
                                    dependencies.insert(archive.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(failures)
    }

    /// Protect source-written dependencies in both raw and explicitly selected
    /// spellings before typed library resolution. Unresolved selector edges
    /// remain in the graph and are diagnosed if the native slice selects them.
    pub fn bind_explicit_meta_provenance(&mut self, context: &TargetContext) {
        let mut aliases = Vec::new();
        for (owner, dependency) in &self.explicit_meta_edges {
            if let Ok(bound_dependency) = endpoint(dependency, context) {
                aliases.push((owner.clone(), bound_dependency.clone()));
                if let Ok(bound_owner) = endpoint(owner, context) {
                    aliases.push((bound_owner, bound_dependency));
                }
            }
        }
        self.explicit_meta_edges.extend(aliases);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CopyIncludesFetchAmbiguity {
    source: String,
    providers: Vec<String>,
}

#[derive(Debug, Default)]
struct SelectionEdges {
    edges: Edges,
    ambiguous_copy_includes: BTreeMap<String, Vec<CopyIncludesFetchAmbiguity>>,
}

fn failure(message: impl Into<String>) -> ArosError {
    ArosError::Diagnostics(DiagnosticSet::single(
        Diagnostic::error(DiagnosticCode::GraphValidation, DiagnosticStage::GraphValidation,
            message.into())
            .with_hint("correct the selected source contract or provide its missing dependency; no endpoint is inferred or discarded"),
    ))
}

/// Resolve only the concrete selector expressions admitted by the parser.
/// Unknown or absent selector values must not silently lose an edge.
///
/// # Errors
/// Rejects unresolved selectors and unsafe or empty endpoint names.
pub fn endpoint(raw: &str, context: &TargetContext) -> Result<String> {
    let mut value = raw.to_owned();
    let legacy = context.legacy_platform();
    for (name, replacement) in [
        ("AROS_TARGET_CPU", context.cpu.as_deref()),
        ("AROS_TARGET_PLATFORM", context.platform.as_deref()),
        ("AROS_TARGET_LEGACY_PLATFORM", legacy.as_deref()),
        ("AROS_TARGET_FAMILY", context.family.as_deref()),
        ("AROS_TARGET_VARIANT", context.variant.as_deref()),
        ("AROS_TARGET_CPU32", context.cpu32.as_deref()),
    ] {
        let expression = format!("${{{name}}}");
        if value.contains(&expression) {
            value = value.replace(
                &expression,
                replacement.ok_or_else(|| failure(format!("{raw} lacks selector {name}")))?,
            );
        }
    }
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(failure(format!(
            "unresolved or unsafe dependency endpoint: {raw}"
        )));
    }
    Ok(value)
}

fn add(edges: &mut Edges, from: &str, to: impl IntoIterator<Item = String>) {
    edges.entry(from.to_owned()).or_default().extend(to);
}

fn consumer_edges(edges: &mut Edges, owner: &str, consumers: &[String]) {
    add(edges, owner, []);
    for consumer in consumers {
        add(edges, consumer, [owner.to_owned()]);
    }
}

impl DependencyGraph {
    fn selection_edges(
        &self,
        diagnostics: &[Diagnostic],
        include_meta: bool,
        context: &TargetContext,
    ) -> SelectionEdges {
        let mut edges = Edges::new();
        let mut ambiguous_copy_includes = BTreeMap::new();
        for header in &self.assembly_headers {
            add(
                &mut edges,
                &header.owner,
                header
                    .aggregate_dependencies
                    .iter()
                    .filter(|dependency| {
                        !self
                            .assembly_headers
                            .iter()
                            .any(|candidate| &candidate.owner == *dependency)
                    })
                    .cloned(),
            );
            add(
                &mut edges,
                &header.aggregate_owner,
                header.aggregate_dependencies.iter().cloned(),
            );
        }
        for effect in &self.arch_endpoint_effects {
            if effect.applies_to(context) {
                add(
                    &mut edges,
                    &effect.endpoint,
                    effect.dependencies.iter().cloned(),
                );
            }
        }
        // A pending identity supplies declaration reachability only. It is
        // admitted exclusively for source preparation and never emitted as a
        // compile producer by the full export.
        for target in &self.inventory_targets {
            let owner = &target.mmake_name;
            add(
                &mut edges,
                owner,
                target.dependencies.iter().chain(&target.link_libs).cloned(),
            );
            if target.genmodule_abi {
                add(&mut edges, &format!("{owner}-includes"), []);
            }
            if target.module_type == ModuleType::Abi
                || target.module_type == ModuleType::Library
                    && (target.genmodule_only
                        || target
                            .genmodule_linklibs
                            .as_ref()
                            .is_some_and(|metadata| metadata.enabled))
            {
                add(&mut edges, &format!("{owner}-includes"), []);
                let runtime = (target.module_type == ModuleType::Library)
                    .then(|| owner.clone())
                    .into_iter();
                add(&mut edges, &format!("{owner}-linklib"), runtime.clone());
                if !target.genmodule_only
                    && target
                        .genmodule_linklibs
                        .as_ref()
                        .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
                {
                    add(&mut edges, &format!("{owner}-linklib-rel"), runtime);
                }
            }
            if !matches!(target.module_type, ModuleType::LinkLib | ModuleType::Abi) {
                let mut switches: BTreeSet<_> =
                    target.spec_switches.iter().map(String::as_str).collect();
                if switches.contains("static") {
                    switches.insert("nostdc");
                }
                add(
                    &mut edges,
                    owner,
                    self.default_link_set
                        .iter()
                        .filter(|item| {
                            item.require_absent
                                .iter()
                                .all(|s| !switches.contains(s.as_str()))
                                && item
                                    .require_present
                                    .iter()
                                    .all(|s| switches.contains(s.as_str()))
                        })
                        .map(|item| item.archive.clone()),
                );
            }
        }
        for (owner, target) in &self.targets {
            if target.module_type == ModuleType::ModuleHeaders {
                add(&mut edges, &format!("{owner}-includes"), []);
                continue;
            }
            add(
                &mut edges,
                owner,
                target.dependencies.iter().chain(&target.link_libs).cloned(),
            );
            // Only aliases actually emitted by the CMake helper are endpoints.
            // A full Library declaration also emits its runtime executable, so
            // selecting one of its client archives must retain the runtime's
            // dependencies. ABI aliases have no runtime module.
            if target.genmodule_abi {
                add(&mut edges, &format!("{owner}-includes"), []);
            }
            if target.module_type == ModuleType::Abi
                || target.module_type == ModuleType::Library
                    && (target.genmodule_only
                        || target
                            .genmodule_linklibs
                            .as_ref()
                            .is_some_and(|metadata| metadata.enabled))
            {
                // These aliases own real GENMODULE header/FD products. A
                // source #MM declaration is not an unmodelled Make recipe
                // when this exact helper already materialises its endpoint.
                add(&mut edges, &format!("{owner}-includes"), []);
                let runtime = (target.module_type == ModuleType::Library)
                    .then(|| owner.clone())
                    .into_iter();
                add(&mut edges, &format!("{owner}-linklib"), runtime.clone());
                if !target.genmodule_only
                    && target
                        .genmodule_linklibs
                        .as_ref()
                        .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
                {
                    add(&mut edges, &format!("{owner}-linklib-rel"), runtime);
                }
            }
            if !matches!(target.module_type, ModuleType::LinkLib | ModuleType::Abi) {
                // The AROS CMake helper treats -static as the nostdc spec
                // switch before evaluating the default link-set guards.
                let mut switches: BTreeSet<&str> =
                    target.spec_switches.iter().map(String::as_str).collect();
                if switches.contains("static") {
                    switches.insert("nostdc");
                }
                let defaults = self.default_link_set.iter().filter(|item| {
                    item.require_absent
                        .iter()
                        .all(|switch| !switches.contains(switch.as_str()))
                        && item
                            .require_present
                            .iter()
                            .all(|switch| switches.contains(switch.as_str()))
                });
                add(&mut edges, owner, defaults.map(|item| item.archive.clone()));
            }
            if target.module_type == ModuleType::ProgramGroup {
                for source in target
                    .source_files
                    .iter()
                    .chain(&target.cxx_source_files)
                    .chain(&target.objc_source_files)
                    .chain(&target.asm_source_files)
                {
                    if let Some(stem) = std::path::Path::new(source).file_stem() {
                        add(
                            &mut edges,
                            &format!("{owner}-{}", stem.to_string_lossy()),
                            [owner.clone()],
                        );
                    }
                }
            }
        }
        for (name, dependencies) in self.meta_targets.iter().filter(|_| include_meta) {
            // Match the generator's narrowly suppressed redundant ABI edge:
            // its helper already orders the exact includes/FD outputs, without
            // reaching unrelated SDK ports through includes-generate-deps.
            let dependencies = dependencies.iter().filter(|dependency| {
                !self.targets.values().any(|target| {
                    target.module_type == ModuleType::Abi
                        && *name == format!("{}-linklib", target.mmake_name)
                        && **dependency == format!("{}-includes", target.mmake_name)
                })
            });
            add(&mut edges, name, dependencies.cloned());
        }
        if include_meta {
            for name in &self.make_meta_providers {
                add(&mut edges, name, []);
            }
        }
        for fetch in &self.fetches {
            add(&mut edges, &fetch.name, []);
        }
        for declaration in &self.directory_setups {
            add(&mut edges, &declaration.owner, []);
        }
        for declaration in &self.genmodule_header_rules {
            add(&mut edges, &declaration.owner, []);
        }
        for declaration in &self.genmodule_writefiles_rules {
            add(&mut edges, &declaration.owner, []);
        }
        for declaration in &self.host_file_generators {
            add(&mut edges, &declaration.owner, []);
        }
        for declaration in &self.sfd_header_rules {
            add(&mut edges, &declaration.owner, []);
        }
        for declaration in &self.sdk_text_rules {
            add(
                &mut edges,
                &declaration.owner,
                [declaration.fetch_owner.clone()],
            );
        }
        for declaration in &self.source_text_rules {
            add(
                &mut edges,
                &declaration.owner,
                [declaration.fetch_owner.clone()],
            );
        }
        for declaration in &self.source_value_rules {
            add(&mut edges, &declaration.owner, []);
        }
        for declaration in &self.sdk_file_copies {
            add(
                &mut edges,
                &declaration.owner,
                declaration.fetch_owner.iter().cloned(),
            );
        }
        for declaration in &self.sdk_asset_rules {
            add(
                &mut edges,
                &declaration.owner,
                self.sdk_asset_dependencies(&declaration.owner, Some(context))
                    .unwrap_or_default(),
            );
        }
        for declaration in &self.sdk_object_groups {
            add(&mut edges, &declaration.owner, []);
        }
        for declaration in &self.literal_object_groups {
            add(&mut edges, &declaration.owner, []);
        }
        for archive in &self.source_archives {
            add(
                &mut edges,
                &archive.declaration.owner,
                archive.producer_owners.iter().cloned(),
            );
            add(
                &mut edges,
                &archive.provider_target(),
                [archive.declaration.owner.clone()],
            );
        }
        for declaration in &self.host_header_rules {
            add(
                &mut edges,
                &declaration.owner,
                [declaration.setup_owner.clone()],
            );
            add(&mut edges, &declaration.setup_owner, []);
        }
        for declaration in &self.host_header_aggregates {
            // Every file leaf is represented inside this checked producer;
            // the named aggregate is not an inferred empty utility target.
            add(&mut edges, &declaration.owner, []);
        }
        for package in &self.packages {
            add(
                &mut edges,
                &package.mmake,
                package.resolved.iter().map(|member| member.target.clone()),
            );
        }
        for declaration in &self.external_cmake {
            add(
                &mut edges,
                &declaration.mmake_name,
                [declaration.fetch_target.clone()],
            );
            add(
                &mut edges,
                &declaration.provider_target,
                [declaration.mmake_name.clone()],
            );
        }
        for declaration in &self.configure_builds {
            add(
                &mut edges,
                &declaration.mmake_name,
                declaration.dependency_targets.clone(),
            );
            if let Some(provider) = &declaration.provider_target {
                add(&mut edges, provider, [declaration.mmake_name.clone()]);
            }
        }
        for declaration in &self.grub_builds {
            add(&mut edges, &declaration.mmake_name, []);
        }
        for declaration in &self.ahi_builds {
            add(&mut edges, &declaration.mmake_name, []);
        }
        for declaration in &self.python_outputs {
            consumer_edges(&mut edges, &declaration.owner, &declaration.consumers);
            add(
                &mut edges,
                &declaration.owner,
                [declaration.fetch_target.clone()],
            );
            add(
                &mut edges,
                &declaration.owner,
                declaration
                    .python_packages
                    .iter()
                    .map(|package| package.fetch_target.clone()),
            );
        }
        for declaration in &self.script_outputs {
            consumer_edges(&mut edges, &declaration.owner, &declaration.consumers);
            add(
                &mut edges,
                &declaration.owner,
                declaration.dependency_targets.clone(),
            );
        }
        for declaration in &self.define_headers {
            consumer_edges(&mut edges, &declaration.owner, &declaration.consumers);
            add(
                &mut edges,
                &declaration.owner,
                [declaration.provider.clone()],
            );
        }
        for declaration in &self.header_transforms {
            consumer_edges(&mut edges, &declaration.name, &declaration.consumers);
            add(
                &mut edges,
                &declaration.name,
                declaration.dependencies.clone(),
            );
        }
        for declaration in &self.flexcat_sources {
            consumer_edges(&mut edges, &declaration.owner, &declaration.consumers);
        }
        for declaration in &self.catalogs {
            consumer_edges(&mut edges, &declaration.mmake, &declaration.consumers);
        }
        for name in self
            .copy_includes
            .iter()
            .map(|decl| &decl.name)
            .chain(self.copy_directories.iter().map(|decl| &decl.name))
            .chain(self.flexcat_headers.iter().map(|decl| &decl.owner))
            .chain(self.ilbm_sources.iter().map(|decl| &decl.owner))
            .chain(self.bison_outputs.iter().map(|decl| &decl.owner))
            .chain(self.icon_targets.keys())
        {
            add(&mut edges, name, []);
        }
        for declaration in &self.copy_directories {
            add(
                &mut edges,
                &declaration.name,
                declaration.dependencies.clone(),
            );
        }
        // A lifted copy's original source owner still owns a concrete recipe.
        // Keep this typed edge even without metadata so strict provider checks,
        // graph audits and optional-edge validation share the same proof.
        // An arbitrary phony alias or private-looking name cannot supply it.
        for owner in self.lifted_copy_aliases.keys() {
            if let Some(action) = self.lifted_copy_action(owner) {
                add(&mut edges, owner, [action.to_owned()]);
            }
        }
        for declaration in &self.copy_includes {
            let source = declaration.source_dir.trim_end_matches('/');
            let providers: Vec<_> = self
                .fetches
                .iter()
                .filter(|fetch| {
                    let destination = fetch.destination.trim_end_matches('/');
                    source == destination
                        || source
                            .strip_prefix(destination)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                })
                .collect();
            let longest = providers
                .iter()
                .map(|fetch| fetch.destination.trim_end_matches('/').len())
                .max();
            if let Some(longest) = longest {
                let owners: BTreeSet<_> = providers
                    .iter()
                    .filter(|fetch| fetch.destination.trim_end_matches('/').len() == longest)
                    .map(|fetch| fetch.name.clone())
                    .collect();
                if owners.len() > 1 {
                    // Keep ambiguity as owned metadata, not a synthetic edge:
                    // reject it only if a selected path reaches this staging
                    // owner, so unrelated ports do not poison the slice.
                    ambiguous_copy_includes
                        .entry(declaration.name.clone())
                        .or_insert_with(Vec::new)
                        .push(CopyIncludesFetchAmbiguity {
                            source: source.to_owned(),
                            providers: owners.into_iter().collect(),
                        });
                } else {
                    add(&mut edges, &declaration.name, owners);
                }
            }
        }
        for declaration in self
            .binary_objects
            .iter()
            .filter(|declaration| declaration.applies_to(context))
        {
            add(&mut edges, &declaration.name, []);
            add(
                &mut edges,
                &declaration.consumer,
                [declaration.name.clone()],
            );
        }
        // A rejected declaration is a known unavailable endpoint, not an
        // unknown empty aggregate. Its diagnostic is checked after traversal.
        for diagnostic in diagnostics {
            if let Some(owner) = diagnostic
                .context
                .as_ref()
                .and_then(|context| context.target.as_ref())
            {
                add(&mut edges, owner, []);
            }
        }
        SelectionEdges {
            edges,
            ambiguous_copy_includes,
        }
    }

    pub(super) fn writefiles_has_other_producer(&self, name: &str) -> bool {
        self.source_archives
            .iter()
            .any(|archive| archive.declaration.owner == name || archive.provider_target() == name)
            || self.targets.contains_key(name)
            || self
                .genmodule_header_rules
                .iter()
                .any(|rule| rule.owner == name)
            || self.directory_setups.iter().any(|rule| rule.owner == name)
            || self.sfd_header_rules.iter().any(|rule| rule.owner == name)
            || self
                .host_header_rules
                .iter()
                .any(|rule| rule.owner == name || rule.setup_owner == name)
            || self
                .host_header_aggregates
                .iter()
                .any(|rule| rule.owner == name)
            || self.sdk_text_rules.iter().any(|rule| rule.owner == name)
            || self.source_text_rules.iter().any(|rule| rule.owner == name)
            || self
                .source_value_rules
                .iter()
                .any(|rule| rule.owner == name)
            || self.sdk_file_copies.iter().any(|rule| rule.owner == name)
            || self.sdk_object_groups.iter().any(|rule| rule.owner == name)
            || self
                .literal_object_groups
                .iter()
                .any(|rule| rule.owner == name)
            || self.python_outputs.iter().any(|rule| rule.owner == name)
            || self.script_outputs.iter().any(|rule| rule.owner == name)
            || self.flexcat_sources.iter().any(|rule| rule.owner == name)
            || self.flexcat_headers.iter().any(|rule| rule.owner == name)
            || self.ilbm_sources.iter().any(|rule| rule.owner == name)
            || self.fetches.iter().any(|rule| rule.name == name)
            || self.packages.iter().any(|rule| rule.mmake == name)
            || self.catalogs.iter().any(|rule| rule.mmake == name)
            || self.define_headers.iter().any(|rule| rule.owner == name)
            || self.bison_outputs.iter().any(|rule| rule.owner == name)
            || self.icon_targets.values().any(|rule| rule.mmake == name)
            || self.icons.iter().any(|rule| rule.mmake == name)
            || self.binary_objects.iter().any(|rule| rule.name == name)
            || self.copy_directories.iter().any(|rule| rule.name == name)
            || self.lifted_copy_action(name).is_some()
            || self.header_transforms.iter().any(|rule| rule.name == name)
            || self
                .external_cmake
                .iter()
                .any(|rule| rule.mmake_name == name || rule.provider_target == name)
            || self.configure_builds.iter().any(|rule| {
                rule.mmake_name == name || rule.provider_target.as_deref() == Some(name)
            })
            || self.grub_builds.iter().any(|rule| rule.mmake_name == name)
            || self.ahi_builds.iter().any(|rule| rule.mmake_name == name)
            || self.copy_includes.iter().any(|rule| rule.name == name)
    }

    fn validate_selected_assembly_owner(&self, name: &str) -> Result<()> {
        let owners = self
            .assembly_headers
            .iter()
            .filter(|header| header.owner == name)
            .count();
        let aggregate = self
            .assembly_headers
            .iter()
            .any(|header| header.aggregate_owner == name);
        if owners > 1
            || owners != 0 && aggregate
            || (owners == 1 || aggregate)
                && (self.writefiles_has_other_producer(name)
                    || self
                        .genmodule_writefiles_rules
                        .iter()
                        .any(|rule| rule.owner == name)
                    || self
                        .host_file_generators
                        .iter()
                        .any(|rule| rule.owner == name)
                    || self
                        .arch_endpoint_effects
                        .iter()
                        .any(|effect| effect.endpoint == name))
        {
            return Err(failure(format!(
                "selected assembly header {name} has conflicting concrete producers"
            )));
        }
        Ok(())
    }

    fn validate_selected_assembly_outputs(&self, selected: &BTreeSet<String>) -> Result<()> {
        let mut outputs = BTreeMap::<String, String>::new();
        for header in self.assembly_headers.iter().filter(|header| {
            selected.contains(&header.owner) || selected.contains(&header.aggregate_owner)
        }) {
            for output in [&header.assembly_output, &header.header_output] {
                if let Some(previous) =
                    outputs.insert(output.to_ascii_lowercase(), header.owner.clone())
                {
                    return Err(failure(format!(
                        "selected assembly headers {previous} and {} both produce {output}",
                        header.owner
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_file_stamp_owners(&self, name: &str) -> Result<()> {
        let writefiles = self
            .genmodule_writefiles_rules
            .iter()
            .filter(|rule| rule.owner == name)
            .count();
        let host_files = self
            .host_file_generators
            .iter()
            .filter(|rule| rule.owner == name)
            .count();
        if host_files > 1
            || host_files == 1
                && (self.writefiles_has_other_producer(name)
                    || writefiles != 0
                    || self.sdk_asset_rules.iter().any(|rule| rule.owner == name))
        {
            return Err(failure(format!(
                "selected host-C file generator {name} has conflicting concrete producers"
            )));
        }
        if writefiles > 1
            || writefiles == 1
                && (self.writefiles_has_other_producer(name)
                    || host_files != 0
                    || self.sdk_asset_rules.iter().any(|rule| rule.owner == name))
        {
            return Err(failure(format!(
                "selected genmodule writefiles stamp {name} has conflicting concrete producers"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "graph_selection_tests.rs"]
mod tests;
