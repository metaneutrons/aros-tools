//! Source-contract selection of a closed native dependency graph.
//!
//! Discovery remains whole-tree. Selection happens only after cross-file
//! providers, source inventories and link edges have been resolved. An
//! unavailable selected capability or an unowned capability failure is fatal.

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
    let legacy = context
        .platform
        .as_ref()
        .zip(context.cpu.as_ref())
        .map(|(platform, cpu)| format!("{platform}-{cpu}"));
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
    /// Traverse every known reachable edge even when a sibling is rejected.
    /// Missing or rejected nodes are reported, never replaced by fake owners.
    /// This deliberately does not authorize graph publication or execution.
    #[must_use]
    pub fn audit_native_dependency_graph(
        &self,
        roots: &[String],
        context: &TargetContext,
        diagnostics: &[Diagnostic],
    ) -> super::NativeGraphAudit {
        let mut report = super::NativeGraphAudit::new(roots);
        let mut edges = Edges::new();
        for (raw, dependencies) in self.selection_edges(diagnostics, true, context).edges {
            add(
                &mut edges,
                &endpoint(&raw, context).unwrap_or(raw),
                dependencies,
            );
        }
        let concrete: BTreeSet<_> = self
            .selection_edges(&[], false, context)
            .edges
            .into_keys()
            .filter_map(|name| endpoint(&name, context).ok())
            .collect();
        let providers: BTreeSet<_> = self
            .make_meta_providers
            .iter()
            .filter_map(|name| endpoint(name, context).ok())
            .collect();
        let explicit: BTreeSet<_> = self
            .explicit_meta_edges
            .iter()
            .filter_map(|(owner, dependency)| {
                Some((
                    endpoint(owner, context).ok()?,
                    endpoint(dependency, context).ok()?,
                ))
            })
            .collect();
        let mut pending: BTreeSet<_> = roots.iter().cloned().collect();
        let mut parents = BTreeMap::new();
        while let Some(raw) = pending.pop_first() {
            let Ok(name) = endpoint(&raw, context) else {
                report.unresolved_selectors.insert(raw);
                continue;
            };
            if !report.reachable.insert(name.clone()) {
                continue;
            }
            if providers.contains(&name) && !concrete.contains(&name) {
                report
                    .unproven_make_providers
                    .push(super::audit::audit_endpoint(&name, &parents, &explicit));
            }
            let Some(dependencies) = edges.get(&name) else {
                report
                    .missing_endpoints
                    .push(super::audit::audit_endpoint(&name, &parents, &explicit));
                continue;
            };
            for dependency in dependencies {
                if let Ok(bound) = endpoint(dependency, context) {
                    if !report.reachable.contains(&bound) {
                        parents.entry(bound).or_insert_with(|| name.clone());
                    }
                }
                pending.insert(dependency.clone());
            }
        }
        for diagnostic in diagnostics {
            let owner = diagnostic
                .context
                .as_ref()
                .and_then(|ctx| ctx.target.as_ref());
            let Some(bound_owner) = owner.and_then(|owner| endpoint(owner, context).ok()) else {
                report.unowned_capability_failures.push(diagnostic.clone());
                continue;
            };
            if report.reachable.contains(&bound_owner) {
                report.selected_capability_failures.push(diagnostic.clone());
            } else {
                report.unrelated_capability_failure_count += 1;
            }
        }
        report.cold_compilation_identities = self
            .inventory_targets
            .iter()
            .filter_map(|target| endpoint(&target.mmake_name, context).ok())
            .filter(|name| report.reachable.contains(name))
            .collect();
        for projection in &self.source_archive_projections {
            if self
                .source_archives
                .iter()
                .any(|archive| archive.declaration == *projection)
            {
                continue;
            }
            if report.reachable.contains(&projection.owner) {
                report
                    .partial_source_projections
                    .push(super::audit::PartialSourceProjection {
                        family: "source-archive",
                        owner: projection.owner.clone(),
                        outputs: 1,
                        unresolved_contracts: vec![
                            "exact registered compiler output ownership".into(),
                            "source-bound archive macro and tool roles".into(),
                        ],
                    });
            }
        }
        for projection in &self.layered_header_projections {
            if self.source_layered_headers.contains(projection) {
                continue;
            }
            if report.reachable.contains(&projection.owner) {
                report
                    .partial_source_projections
                    .push(super::audit::PartialSourceProjection {
                        family: "layered-header-copy",
                        owner: projection.owner.clone(),
                        outputs: projection.copies.len(),
                        unresolved_contracts: projection.unresolved_prerequisites.clone(),
                    });
            }
        }
        for projection in &self.source_compile_projections {
            if self
                .source_archives
                .iter()
                .any(|archive| archive.compile_groups.contains(projection))
                || self.literal_object_groups.iter().any(|group| {
                    group.owner == projection.owner
                        && group.file == projection.file
                        && group.objects == projection.objects
                })
            {
                continue;
            }
            let selected_owner = projection.parent.as_ref().unwrap_or(&projection.owner);
            if report.reachable.contains(selected_owner) {
                report
                    .partial_source_projections
                    .push(super::audit::PartialSourceProjection {
                        family: "source-compile",
                        owner: projection.owner.clone(),
                        outputs: projection.objects.len(),
                        unresolved_contracts: vec![
                            "typed object, archive and prerequisite ownership binding".into(),
                        ],
                    });
            }
        }
        report
            .partial_source_projections
            .sort_by(|a, b| (&a.family, &a.owner).cmp(&(&b.family, &b.owner)));
        macro_rules! family {
            ($kind:literal, $owners:expr) => {
                report.producer_families.insert(
                    $kind.to_owned(),
                    $owners
                        .filter_map(|owner| endpoint(owner, context).ok())
                        .filter(|name| report.reachable.contains(name))
                        .collect(),
                );
            };
        }
        macro_rules! owned_family {
            ($kind:literal, $owners:expr) => {
                report.producer_families.insert(
                    $kind.to_owned(),
                    $owners
                        .into_iter()
                        .filter_map(|owner| endpoint(&owner, context).ok())
                        .filter(|name| report.reachable.contains(name))
                        .collect(),
                );
            };
        }
        family!("compilation", self.targets.keys().map(String::as_str));
        family!(
            "assembly-header",
            self.assembly_headers
                .iter()
                .flat_map(|header| [header.owner.as_str(), header.aggregate_owner.as_str()])
        );
        family!("architecture-metadata", self.arch_endpoint_effects.iter()
            .filter(|effect| effect.applies_to(context) && !matches!(effect.data, crate::arch_endpoint_effects::ArchEndpointEffectData::ArchModuleObjects { .. })).map(|effect| effect.endpoint.as_str()));
        family!("architecture-objects", self.arch_endpoint_effects.iter()
            .filter(|effect| effect.applies_to(context) && matches!(effect.data, crate::arch_endpoint_effects::ArchEndpointEffectData::ArchModuleObjects { .. })).map(|effect| effect.endpoint.as_str()));
        family!(
            "make-meta-provider",
            self.make_meta_providers.iter().map(String::as_str)
        );
        family!("fetch", self.fetches.iter().map(|rule| rule.name.as_str()));
        family!(
            "host-c-file",
            self.host_file_generators
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "host-header",
            self.host_header_rules
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "host-header-aggregate",
            self.host_header_aggregates
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "directory",
            self.directory_setups.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "genmodule-header",
            self.genmodule_header_rules
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "genmodule-writefiles",
            self.genmodule_writefiles_rules
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "sdk-object",
            self.sdk_object_groups
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "literal-object",
            self.literal_object_groups
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "source-archive",
            self.source_archives
                .iter()
                .map(|archive| archive.declaration.owner.as_str())
        );
        family!(
            "source-layered-header",
            self.source_layered_headers
                .iter()
                .map(|aggregate| aggregate.owner.as_str())
        );
        family!(
            "sdk-file-copy",
            self.sdk_file_copies.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "sdk-asset-rule",
            self.sdk_asset_rules.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "sdk-text",
            self.sdk_text_rules.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "source-text",
            self.source_text_rules
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "source-value",
            self.source_value_rules
                .iter()
                .map(|rule| rule.owner.as_str())
        );
        family!(
            "sfd-header",
            self.sfd_header_rules.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "header-copy",
            self.copy_includes.iter().map(|rule| rule.name.as_str())
        );
        family!(
            "package",
            self.packages.iter().map(|rule| rule.mmake.as_str())
        );
        owned_family!(
            "genmodule-includes",
            self.targets
                .iter()
                .filter(|(_, target)| {
                    target.module_type == ModuleType::ModuleHeaders
                        || target.genmodule_abi
                        || target.module_type == ModuleType::Abi
                        || target.module_type == ModuleType::Library
                            && (target.genmodule_only
                                || target
                                    .genmodule_linklibs
                                    .as_ref()
                                    .is_some_and(|metadata| metadata.enabled))
                })
                .map(|(owner, _)| format!("{owner}-includes"))
        );
        owned_family!(
            "genmodule-linklib",
            self.targets
                .iter()
                .filter(|(_, target)| {
                    target.module_type == ModuleType::Abi
                        || target.module_type == ModuleType::Library
                            && (target.genmodule_only
                                || target
                                    .genmodule_linklibs
                                    .as_ref()
                                    .is_some_and(|metadata| metadata.enabled))
                })
                .map(|(owner, _)| format!("{owner}-linklib"))
        );
        owned_family!(
            "genmodule-linklib-relative",
            self.targets
                .iter()
                .filter(|(_, target)| {
                    !target.genmodule_only
                        && matches!(target.module_type, ModuleType::Abi | ModuleType::Library)
                        && target
                            .genmodule_linklibs
                            .as_ref()
                            .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
                })
                .map(|(owner, _)| format!("{owner}-linklib-rel"))
        );
        owned_family!(
            "program-group-object",
            self.targets.iter().flat_map(|(owner, target)| {
                (target.module_type == ModuleType::ProgramGroup)
                    .then_some(target)
                    .into_iter()
                    .flat_map(move |target| {
                        target
                            .source_files
                            .iter()
                            .chain(&target.cxx_source_files)
                            .chain(&target.objc_source_files)
                            .chain(&target.asm_source_files)
                            .filter_map(move |source| {
                                std::path::Path::new(source)
                                    .file_stem()
                                    .map(|stem| format!("{owner}-{}", stem.to_string_lossy()))
                            })
                    })
            })
        );
        family!(
            "external-cmake",
            self.external_cmake
                .iter()
                .map(|rule| rule.mmake_name.as_str())
        );
        family!(
            "external-cmake-provider",
            self.external_cmake
                .iter()
                .map(|rule| rule.provider_target.as_str())
        );
        family!(
            "configure-build",
            self.configure_builds
                .iter()
                .map(|rule| rule.mmake_name.as_str())
        );
        family!(
            "configure-provider",
            self.configure_builds
                .iter()
                .filter_map(|rule| rule.provider_target.as_deref())
        );
        family!(
            "grub-build",
            self.grub_builds.iter().map(|rule| rule.mmake_name.as_str())
        );
        family!(
            "ahi-build",
            self.ahi_builds.iter().map(|rule| rule.mmake_name.as_str())
        );
        family!(
            "python-output",
            self.python_outputs.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "script-output",
            self.script_outputs.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "define-header",
            self.define_headers.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "header-transform",
            self.header_transforms.iter().map(|rule| rule.name.as_str())
        );
        family!(
            "flexcat-source",
            self.flexcat_sources.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "catalog",
            self.catalogs.iter().map(|rule| rule.mmake.as_str())
        );
        family!(
            "copy-directory",
            self.copy_directories.iter().map(|rule| rule.name.as_str())
        );
        family!(
            "flexcat-header",
            self.flexcat_headers.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "ilbm-source",
            self.ilbm_sources.iter().map(|rule| rule.owner.as_str())
        );
        family!(
            "bison-output",
            self.bison_outputs.iter().map(|rule| rule.owner.as_str())
        );
        family!("icon", self.icon_targets.keys().map(String::as_str));
        family!(
            "binary-object",
            self.binary_objects.iter().map(|rule| rule.name.as_str())
        );
        report.strict_validation_error = self
            .selected_dependency_closure(roots, context, diagnostics)
            .err()
            .map(|error| error.to_string());
        report
    }

    /// Remove phantom `includes-*` prerequisites for proven plain link-library providers.
    ///
    /// The endpoint set includes both raw declarations and their selected-profile
    /// bindings so an unresolved selector remains conservatively known.
    pub fn omit_native_plain_linklib_headers(&mut self, context: &TargetContext) -> Vec<String> {
        let known: BTreeSet<_> = self
            .selection_edges(&[], true, context)
            .edges
            .into_keys()
            .chain(self.make_meta_providers.iter().cloned())
            .flat_map(|name| {
                let bound = endpoint(&name, context).ok();
                std::iter::once(name).chain(bound)
            })
            .collect();
        self.omit_implicit_plain_linklib_headers(&known)
    }

    /// Validate legacy optionality declarations without changing source edges.
    /// Neither a selector nor a commented-out owner makes an active MetaMake
    /// dependency optional. Missing endpoints remain upstream review errors.
    /// Existing and rejected producers retain their ordinary capability checks.
    /// The legacy return type is retained, but no omissions are authorized.
    ///
    /// # Errors
    /// Rejects undeclared edges and unresolved selectors. Concrete owners still
    /// require their ordinary source-local capability proof during selection.
    pub fn omit_absent_optional_meta_dependencies(
        &self,
        declarations: &[NativeOptionalMetaDependency],
        context: &TargetContext,
        diagnostics: &[Diagnostic],
    ) -> Result<Vec<String>> {
        let active_diagnostics: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .context
                    .as_ref()
                    .and_then(|context| context.mode.as_deref())
                    != Some(crate::sdk_text_rules::DISABLED_OWNER_DIAGNOSTIC_MODE)
            })
            .cloned()
            .collect();
        let known: BTreeSet<_> = self
            .selection_edges(&active_diagnostics, true, context)
            .edges
            .into_keys()
            .chain(self.make_meta_providers.iter().cloned())
            .filter_map(|name| endpoint(&name, context).ok())
            .collect();
        let mut missing = Vec::new();
        for declaration in declarations {
            let valid_absence = match declaration.absence {
                NativeMetaAbsence::Selector => declaration.dependency.contains("${AROS_TARGET_"),
                NativeMetaAbsence::DisabledOwner => {
                    !declaration.dependency.is_empty()
                        && declaration.dependency.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric()
                                || matches!(byte, b'_' | b'-' | b'.' | b'+')
                        })
                }
            };
            if !valid_absence
                || !self
                    .meta_targets
                    .get(&declaration.target)
                    .is_some_and(|dependencies| dependencies.contains(&declaration.dependency))
            {
                return Err(failure(format!(
                    "optional MetaMake edge {} -> {} is not an exact source-proven declaration in {}",
                    declaration.target, declaration.dependency, declaration.recipe
                )));
            }
            let dependency = endpoint(&declaration.dependency, context)?;
            if !known.contains(&dependency) {
                let reason = match declaration.absence {
                    NativeMetaAbsence::DisabledOwner => "its owner is commented out",
                    NativeMetaAbsence::Selector => "a selector does not establish optionality",
                };
                missing.push(Diagnostic::error(
                            DiagnosticCode::GraphValidation,
                            DiagnosticStage::GraphValidation,
                            format!(
                                "active MetaMake dependency {} -> {dependency} has no active producer; {reason}", declaration.target
                            ),
                        )
                        .with_location(aros_common::SourceLocation {
                            path: declaration.recipe.clone(),
                            line: None,
                            column: None,
                        })
                        .with_hint("fix upstream: verify and correct the dependency or its source-owned producer; an optionality declaration cannot erase an active mmake dependency"));
            }
        }
        if !missing.is_empty() {
            return Err(ArosError::Diagnostics(DiagnosticSet::new(missing)));
        }
        Ok(Vec::new())
    }

    /// Resolve source-declared core/package/library requirements to unique
    /// existing producers. Board names never determine these producers.
    ///
    /// # Errors
    /// Rejects missing, foreign or ambiguous runtime/archive providers and an
    /// incomplete resolved package before generating any selected graph.
    pub fn native_contract_roots(
        &self,
        contract: &NativeBuildContract,
        context: &TargetContext,
    ) -> Result<Vec<String>> {
        let arch = context
            .cpu
            .as_ref()
            .zip(context.platform.as_ref())
            .map(|(cpu, platform)| (cpu.clone(), platform.clone()))
            .ok_or_else(|| failure("native selection requires explicit CPU and platform"))?;
        let mut roots = BTreeSet::new();
        for (names, suffix) in [
            (&contract.core.resources, "resource"),
            (&contract.core.libraries, "library"),
            (&contract.core.devices, "device"),
        ] {
            for name in names {
                let runtime = format!("{name}.{suffix}");
                let candidates: Vec<_> = self
                    .targets
                    .values()
                    .filter(|target| {
                        !target.variant_32bit
                            && target_runtime_name(target).as_deref() == Some(&runtime)
                            && arch_compatible(arch_of(&target.dir_path).as_ref(), Some(&arch))
                    })
                    .map(|target| target.mmake_name.clone())
                    .chain(
                        self.inventory_targets
                            .iter()
                            .filter(|target| {
                                !target.variant_32bit
                                    && inventory_runtime_name(target).as_deref() == Some(&runtime)
                                    && arch_compatible(
                                        arch_of(&target.dir_path).as_ref(),
                                        Some(&arch),
                                    )
                            })
                            .map(|target| target.mmake_name.clone()),
                    )
                    .collect();
                if candidates.len() != 1 {
                    return Err(failure(format!(
                        "native runtime {runtime} requires one applicable producer; found {candidates:?}"
                    )));
                }
                roots.extend(candidates);
            }
        }
        for name in &contract.core.link_libraries {
            let candidates: Vec<_> = self
                .targets
                .values()
                .filter(|target| {
                    target.target_name == *name
                        && !target.variant_32bit
                        && target.linklib_output_dir.is_none()
                        && has_public_link_archive(target)
                        && arch_compatible(arch_of(&target.dir_path).as_ref(), Some(&arch))
                })
                .map(|target| match target.module_type {
                    ModuleType::Abi | ModuleType::Library => {
                        format!("{}-linklib", target.mmake_name)
                    }
                    _ => target.mmake_name.clone(),
                })
                .chain(
                    self.inventory_targets
                        .iter()
                        .filter(|target| {
                            target.target_name == *name
                                && !target.variant_32bit
                                && target.linklib_output_dir.is_none()
                                && inventory_has_public_link_archive(target)
                                && arch_compatible(arch_of(&target.dir_path).as_ref(), Some(&arch))
                        })
                        .map(|target| match target.module_type {
                            ModuleType::Abi | ModuleType::Library => {
                                format!("{}-linklib", target.mmake_name)
                            }
                            _ => target.mmake_name.clone(),
                        }),
                )
                .chain(
                    self.source_archives
                        .iter()
                        .filter(|archive| {
                            archive.library_name() == name
                                && arch_compatible(
                                    arch_of(std::path::Path::new(&archive.declaration.file))
                                        .as_ref(),
                                    Some(&arch),
                                )
                        })
                        .map(crate::source_archive_binding::BoundSourceArchive::provider_target),
                )
                .collect();
            if candidates.len() != 1 {
                return Err(failure(format!(
                    "native archive {name} requires one applicable producer; found {candidates:?}"
                )));
            }
            roots.extend(candidates);
        }
        let packages: Vec<_> = self
            .packages
            .iter()
            .filter(|package| package.mmake == contract.package.target)
            .collect();
        let [package] = packages.as_slice() else {
            return Err(failure(
                "native package requires one exact source declaration",
            ));
        };
        let expected: BTreeSet<_> = package
            .members
            .iter()
            .map(|(kind, name)| crate::packages::runtime_name(kind, name))
            .chain(
                package
                    .startup
                    .iter()
                    .map(|name| format!("{name}.resource")),
            )
            .collect();
        let actual: BTreeSet<_> = package
            .resolved
            .iter()
            .map(|member| member.runtime_name.clone())
            .collect();
        if expected != actual {
            return Err(failure(format!(
                "native package has unresolved members: {:?}",
                expected.difference(&actual).collect::<Vec<_>>()
            )));
        }
        roots.insert(package.mmake.clone());
        Ok(roots.into_iter().collect())
    }

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

    /// Compute a deterministic, fail-closed dependency closure before pruning.
    ///
    /// # Errors
    /// Rejects missing endpoints/selectors, every unowned capability failure,
    /// and any rejected capability reachable from a requested root.
    pub fn selected_dependency_closure(
        &self,
        roots: &[String],
        context: &TargetContext,
        diagnostics: &[Diagnostic],
    ) -> Result<BTreeSet<String>> {
        if roots.is_empty() {
            return Err(failure("a selected graph requires at least one root"));
        }
        let SelectionEdges {
            edges: raw_edges,
            ambiguous_copy_includes: raw_ambiguities,
        } = self.selection_edges(diagnostics, true, context);
        let concrete_endpoints: BTreeSet<_> = self
            .selection_edges(&[], false, context)
            .edges
            .into_keys()
            .filter_map(|name| endpoint(&name, context).ok())
            .collect();
        let make_providers: BTreeSet<_> = self
            .make_meta_providers
            .iter()
            .filter_map(|name| endpoint(name, context).ok())
            .collect();
        let mut edges = Edges::new();
        for (name, dependencies) in raw_edges {
            // An unselected icon/architecture aggregate may contain a selector
            // that this profile does not provide. Do not reject the unrelated
            // graph, but never permit that unresolved endpoint to be selected.
            let name = endpoint(&name, context).unwrap_or(name);
            add(&mut edges, &name, dependencies);
        }
        let mut ambiguities = BTreeMap::new();
        for (raw_name, owners) in raw_ambiguities {
            if let Ok(name) = endpoint(&raw_name, context) {
                ambiguities
                    .entry(name)
                    .or_insert_with(Vec::new)
                    .extend(owners);
            }
        }
        let mut selected = BTreeSet::new();
        let mut pending = roots.to_vec();
        let mut parents = BTreeMap::new();
        while let Some(raw) = pending.pop() {
            let name = endpoint(&raw, context)?;
            self.validate_selected_assembly_owner(&name)?;
            if let Some((owner, _)) = self.targets.iter().find(|(owner, target)| {
                target.module_type == ModuleType::ModuleHeaders
                    && [
                        "",
                        "-quick",
                        "-kobj",
                        "-kobj-quick",
                        "-linklib",
                        "-linklib-rel",
                    ]
                    .iter()
                    .any(|suffix| name == format!("{owner}{suffix}"))
            }) {
                let failures = diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic
                            .context
                            .as_ref()
                            .and_then(|ctx| ctx.target.as_ref())
                            == Some(owner)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if !failures.is_empty() {
                    return Err(ArosError::Diagnostics(DiagnosticSet::new(failures)));
                }
                return Err(failure(format!(
                    "{name} has only a source-header projection; its runtime and client archives are unsupported"
                )));
            }
            if !selected.insert(name.clone()) {
                continue;
            }
            if self.sdk_asset_rules.iter().any(|rule| rule.owner == name) {
                if self.writefiles_has_other_producer(&name)
                    || self
                        .genmodule_writefiles_rules
                        .iter()
                        .any(|rule| rule.owner == name)
                    || self
                        .host_file_generators
                        .iter()
                        .any(|rule| rule.owner == name)
                {
                    return Err(failure(format!(
                        "selected SDK asset {name} has conflicting concrete producers"
                    )));
                }
                self.sdk_asset_dependencies(&name, Some(context))
                    .map_err(failure)?;
            }
            let literal_owners = self
                .literal_object_groups
                .iter()
                .filter(|group| group.owner == name)
                .count();
            if literal_owners > 1
                || literal_owners == 1
                    && (self.targets.contains_key(&name)
                        || self
                            .sdk_object_groups
                            .iter()
                            .any(|group| group.owner == name)
                        || self
                            .host_header_aggregates
                            .iter()
                            .any(|rule| rule.owner == name)
                        || self.directory_setups.iter().any(|rule| rule.owner == name)
                        || self
                            .genmodule_header_rules
                            .iter()
                            .any(|rule| rule.owner == name)
                        || self
                            .host_header_rules
                            .iter()
                            .any(|rule| rule.owner == name || rule.setup_owner == name)
                        || self.sdk_file_copies.iter().any(|rule| rule.owner == name)
                        || self.sdk_text_rules.iter().any(|rule| rule.owner == name)
                        || self.source_text_rules.iter().any(|rule| rule.owner == name)
                        || self
                            .source_value_rules
                            .iter()
                            .any(|rule| rule.owner == name)
                        || self.sfd_header_rules.iter().any(|rule| rule.owner == name)
                        || self.header_transforms.iter().any(|rule| rule.name == name))
            {
                return Err(failure(format!(
                    "selected literal object group {name} has conflicting concrete producers"
                )));
            }
            let sdk_object_owners = self
                .sdk_object_groups
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            if sdk_object_owners > 1
                || sdk_object_owners == 1
                    && (self.targets.contains_key(&name)
                        || self
                            .host_header_aggregates
                            .iter()
                            .any(|rule| rule.owner == name)
                        || self.directory_setups.iter().any(|rule| rule.owner == name)
                        || self
                            .genmodule_header_rules
                            .iter()
                            .any(|rule| rule.owner == name)
                        || self
                            .host_header_rules
                            .iter()
                            .any(|rule| rule.owner == name || rule.setup_owner == name)
                        || self.sdk_file_copies.iter().any(|rule| rule.owner == name)
                        || self.sdk_text_rules.iter().any(|rule| rule.owner == name)
                        || self.source_text_rules.iter().any(|rule| rule.owner == name)
                        || self
                            .source_value_rules
                            .iter()
                            .any(|rule| rule.owner == name)
                        || self.sfd_header_rules.iter().any(|rule| rule.owner == name)
                        || self.header_transforms.iter().any(|rule| rule.name == name))
            {
                return Err(failure(format!(
                    "selected SDK object group {name} has conflicting concrete producers"
                )));
            }
            if make_providers.contains(&name) && !concrete_endpoints.contains(&name) {
                // A rejected source recipe has a more precise explanation
                // than the generic missing-endpoint consequence. Report only
                // this selected owner's diagnostics; unrelated rejections
                // still cannot poison the selected slice.
                let owned_failures: Vec<_> = diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic
                            .context
                            .as_ref()
                            .and_then(|context| context.target.as_ref())
                            .is_some_and(|owner| {
                                endpoint(owner, context).is_ok_and(|owner| owner == name)
                            })
                    })
                    .cloned()
                    .collect();
                if !owned_failures.is_empty() {
                    return Err(ArosError::Diagnostics(DiagnosticSet::new(owned_failures)));
                }
                return Err(failure(format!(
                    "selected nonvirtual Make provider {name} has no proven recipe; an empty virtual declaration cannot replace it"
                )));
            }
            let directory_owners = self
                .directory_setups
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            let sfd_owners = self
                .sfd_header_rules
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            let aggregate_owners = self
                .host_header_aggregates
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            if aggregate_owners > 1
                || aggregate_owners == 1
                    && (directory_owners != 0
                        || self.targets.contains_key(&name)
                        || self
                            .genmodule_header_rules
                            .iter()
                            .any(|rule| rule.owner == name)
                        || self
                            .host_header_rules
                            .iter()
                            .any(|rule| rule.owner == name || rule.setup_owner == name)
                        || self.sdk_file_copies.iter().any(|rule| rule.owner == name)
                        || self.sdk_text_rules.iter().any(|rule| rule.owner == name)
                        || self.source_text_rules.iter().any(|rule| rule.owner == name)
                        || self
                            .source_value_rules
                            .iter()
                            .any(|rule| rule.owner == name)
                        || sfd_owners != 0
                        || self.header_transforms.iter().any(|rule| rule.name == name))
            {
                return Err(failure(format!(
                    "selected host-header aggregate {name} has conflicting concrete producers"
                )));
            }
            if directory_owners > 1
                || directory_owners == 1 && (sfd_owners != 0 || self.targets.contains_key(&name))
            {
                return Err(failure(format!(
                    "selected directory setup {name} has conflicting concrete producers"
                )));
            }
            let header_owners = self
                .genmodule_header_rules
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            self.validate_file_stamp_owners(&name)?;
            if header_owners > 1
                || header_owners == 1
                    && (directory_owners != 0
                        || sfd_owners != 0
                        || self.targets.contains_key(&name))
            {
                return Err(failure(format!(
                    "selected genmodule header stamp {name} has conflicting concrete producers"
                )));
            }
            let host_header_owners = self
                .host_header_rules
                .iter()
                .filter(|declaration| declaration.owner == name || declaration.setup_owner == name)
                .count();
            if host_header_owners > 1
                || host_header_owners == 1
                    && (directory_owners != 0
                        || header_owners != 0
                        || sfd_owners != 0
                        || self.targets.contains_key(&name))
            {
                return Err(failure(format!(
                    "selected host header rule {name} has conflicting concrete producers"
                )));
            }
            let sdk_text_owners = self
                .sdk_text_rules
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            if sdk_text_owners > 1
                || sdk_text_owners == 1
                    && (directory_owners != 0
                        || header_owners != 0
                        || host_header_owners != 0
                        || sfd_owners != 0
                        || self.targets.contains_key(&name))
            {
                return Err(failure(format!(
                    "selected SDK text rule {name} has conflicting concrete producers"
                )));
            }
            let source_text_owners = self
                .source_text_rules
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            if source_text_owners > 1
                || source_text_owners == 1
                    && (sdk_text_owners != 0
                        || directory_owners != 0
                        || header_owners != 0
                        || host_header_owners != 0
                        || sfd_owners != 0
                        || self.targets.contains_key(&name))
            {
                return Err(failure(format!(
                    "selected source text rule {name} has conflicting concrete producers"
                )));
            }
            let source_value_owners = self
                .source_value_rules
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            if source_value_owners > 1
                || source_value_owners == 1
                    && (self.targets.contains_key(&name)
                        || directory_owners != 0
                        || header_owners != 0
                        || aggregate_owners != 0
                        || host_header_owners != 0
                        || sdk_text_owners != 0
                        || source_text_owners != 0
                        || sfd_owners != 0
                        || self.sdk_file_copies.iter().any(|rule| rule.owner == name)
                        || self.sdk_object_groups.iter().any(|rule| rule.owner == name)
                        || self
                            .literal_object_groups
                            .iter()
                            .any(|rule| rule.owner == name)
                        || self.header_transforms.iter().any(|rule| rule.name == name))
            {
                return Err(failure(format!(
                    "selected source value rule {name} has conflicting concrete producers"
                )));
            }
            let sdk_copy_owners = self
                .sdk_file_copies
                .iter()
                .filter(|declaration| declaration.owner == name)
                .count();
            if sdk_copy_owners > 1
                || sdk_copy_owners == 1
                    && (source_text_owners != 0
                        || sdk_text_owners != 0
                        || directory_owners != 0
                        || header_owners != 0
                        || host_header_owners != 0
                        || sfd_owners != 0
                        || self.targets.contains_key(&name))
            {
                return Err(failure(format!(
                    "selected SDK file copy {name} has conflicting concrete producers"
                )));
            }
            if sfd_owners > 1
                || sfd_owners == 1
                    && (directory_owners != 0
                        || aggregate_owners != 0
                        || header_owners != 0
                        || host_header_owners != 0
                        || sdk_text_owners != 0
                        || source_text_owners != 0
                        || sdk_copy_owners != 0
                        || self.targets.contains_key(&name)
                        || self.header_transforms.iter().any(|rule| rule.name == name))
            {
                return Err(failure(format!(
                    "selected SFD header rule {name} has conflicting concrete producers"
                )));
            }
            if let Some(ambiguities) = ambiguities.get(&name) {
                let owners = ambiguities
                    .iter()
                    .map(|ambiguity| {
                        format!(
                            "{} -> [{}]",
                            ambiguity.source,
                            ambiguity.providers.join(", ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(failure(format!(
                    "selected %copy_includes owner {name} has ambiguous longest %fetch owners: {owners}"
                )));
            }
            let dependencies = edges.get(&name).ok_or_else(|| {
                let mut chain = vec![name.clone()];
                let mut cursor = &name;
                while let Some(parent) = parents.get(cursor) {
                    if chain.contains(parent) {
                        break;
                    }
                    chain.push(parent.clone());
                    cursor = parent;
                }
                chain.reverse();
                failure(format!(
                    "selected dependency {name} has no proven endpoint (path: {})",
                    chain.join(" -> ")
                ))
            })?;
            for dependency in dependencies {
                let concrete = endpoint(dependency, context)?;
                if !roots.contains(&concrete) && !selected.contains(&concrete) {
                    parents.entry(concrete).or_insert_with(|| name.clone());
                }
                pending.push(dependency.clone());
            }
        }
        let mut source_value_outputs = BTreeMap::<String, String>::new();
        self.validate_selected_assembly_outputs(&selected)?;
        for declaration in self
            .source_value_rules
            .iter()
            .filter(|declaration| selected.contains(&declaration.owner))
        {
            let key = declaration.output.to_ascii_lowercase();
            if let Some(previous) = source_value_outputs.insert(key, declaration.owner.clone()) {
                return Err(failure(format!(
                    "selected source value rules {previous} and {} both produce {}",
                    declaration.owner, declaration.output
                )));
            }
        }
        let mut sfd_output_owners = BTreeMap::<String, String>::new();
        for declaration in self
            .sfd_header_rules
            .iter()
            .filter(|declaration| selected.contains(&declaration.owner))
        {
            for job in &declaration.jobs {
                if let Some(previous) =
                    sfd_output_owners.insert(job.sdk_output.clone(), declaration.owner.clone())
                {
                    return Err(failure(format!(
                        "selected SFD header rules {previous} and {} both produce SDK header {}",
                        declaration.owner, job.sdk_output
                    )));
                }
            }
        }
        let mut sdk_object_outputs =
            BTreeMap::<String, (&str, &crate::sdk_objects::SdkObjectDecl)>::new();
        for group in self
            .sdk_object_groups
            .iter()
            .filter(|group| selected.contains(&group.owner))
        {
            for object in &group.objects {
                for path in [&object.output, &object.intermediate] {
                    let key = path.to_ascii_lowercase();
                    if let Some((previous_owner, previous)) = sdk_object_outputs.get(&key) {
                        if **previous != *object {
                            return Err(failure(format!(
                                "selected SDK object groups {previous_owner} and {} have conflicting ownership of {path}",
                                group.owner
                            )));
                        }
                    } else {
                        sdk_object_outputs.insert(key, (&group.owner, object));
                    }
                }
            }
        }
        let mut literal_outputs = BTreeMap::new();
        for group in self
            .literal_object_groups
            .iter()
            .filter(|group| selected.contains(&group.owner))
        {
            for object in &group.objects {
                let key = object.output.to_ascii_lowercase();
                if sdk_object_outputs.contains_key(&key) {
                    return Err(failure(format!(
                        "selected literal and SDK objects share output {}",
                        object.output
                    )));
                }
                if let Some((prior_owner, prior)) =
                    literal_outputs.insert(key, (&group.owner, object))
                {
                    if prior != object {
                        return Err(failure(format!(
                            "selected literal object groups {prior_owner} and {} conflict at {}",
                            group.owner, object.output
                        )));
                    }
                }
            }
        }
        let failures: Vec<_> = diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .context
                    .as_ref()
                    .and_then(|context| context.target.as_ref())
                    .is_none_or(|owner| {
                        endpoint(owner, context).map_or(true, |owner| selected.contains(&owner))
                    })
            })
            .cloned()
            .collect();
        if !failures.is_empty() {
            return Err(ArosError::Diagnostics(DiagnosticSet::new(failures)));
        }
        Ok(selected)
    }

    /// Retain declarations needed to materialise a proven selection. Global
    /// in-tree SDK headers remain context, not extra runtime build roots.
    ///
    /// # Errors
    /// Rejects unresolved endpoint expressions in the retained dependency map.
    pub fn retain_native_selection(
        &mut self,
        selected: &BTreeSet<String>,
        context: &TargetContext,
    ) -> Result<()> {
        let is_selected =
            |name: &str| endpoint(name, context).is_ok_and(|name| selected.contains(&name));
        // An object endpoint owns a real subset of its module's compilation
        // state. Keep that declaration without introducing a dependency back
        // to the module (which could create a module/architecture cycle).
        let architecture_owners: BTreeSet<_> = self
            .arch_endpoint_effects
            .iter()
            .filter(|effect| effect.applies_to(context) && is_selected(&effect.endpoint))
            .filter_map(|effect| match &effect.data {
                crate::arch_endpoint_effects::ArchEndpointEffectData::ArchModuleObjects {
                    mainmmake,
                    ..
                } => Some(mainmmake.clone()),
                _ => None,
            })
            .collect();
        let declaration_needed = |name: &str| {
            is_selected(name)
                || architecture_owners.contains(name)
                || ["-linklib", "-linklib-rel", "-includes", "-fd", "-kobj"]
                    .iter()
                    .any(|suffix| is_selected(&format!("{name}{suffix}")))
        };
        self.targets.retain(|name, target| {
            declaration_needed(name)
                || target.module_type == ModuleType::ProgramGroup
                    && target
                        .source_files
                        .iter()
                        .chain(&target.cxx_source_files)
                        .chain(&target.objc_source_files)
                        .chain(&target.asm_source_files)
                        .filter_map(|source| std::path::Path::new(source).file_stem())
                        .any(|stem| is_selected(&format!("{name}-{}", stem.to_string_lossy())))
        });
        let mut retained_meta = std::collections::HashMap::new();
        for (name, dependencies) in &self.meta_targets {
            if !is_selected(name) {
                continue;
            }
            let name = endpoint(name, context)?;
            let dependencies = dependencies
                .iter()
                .filter(|dependency| is_selected(dependency))
                .map(|dependency| endpoint(dependency, context))
                .collect::<Result<std::collections::HashSet<_>>>()?;
            retained_meta
                .entry(name)
                .or_insert_with(std::collections::HashSet::new)
                .extend(dependencies);
        }
        self.meta_targets = retained_meta;
        self.explicit_meta_edges = self
            .explicit_meta_edges
            .iter()
            .filter(|(owner, dependency)| is_selected(owner) && is_selected(dependency))
            .map(|(owner, dependency)| {
                Ok((endpoint(owner, context)?, endpoint(dependency, context)?))
            })
            .collect::<Result<std::collections::HashSet<_>>>()?;
        self.fetches
            .retain(|declaration| is_selected(&declaration.name));
        self.source_inventory_fetches
            .retain(|name| is_selected(name));
        self.packages
            .retain(|declaration| is_selected(&declaration.mmake));
        self.default_link_set
            .retain(|item| is_selected(&item.archive));
        self.external_cmake.retain(|declaration| {
            is_selected(&declaration.mmake_name) || is_selected(&declaration.provider_target)
        });
        self.configure_builds.retain(|declaration| {
            is_selected(&declaration.mmake_name)
                || declaration
                    .provider_target
                    .as_ref()
                    .is_some_and(|provider| is_selected(provider))
        });
        self.grub_builds
            .retain(|declaration| is_selected(&declaration.mmake_name));
        self.ahi_builds
            .retain(|declaration| is_selected(&declaration.mmake_name));
        self.python_outputs
            .retain(|declaration| is_selected(&declaration.owner));
        self.directory_setups
            .retain(|declaration| is_selected(&declaration.owner));
        self.arch_endpoint_effects
            .retain(|effect| effect.applies_to(context) && is_selected(&effect.endpoint));
        self.assembly_headers
            .retain(|header| is_selected(&header.owner) || is_selected(&header.aggregate_owner));
        self.genmodule_header_rules
            .retain(|declaration| is_selected(&declaration.owner));
        self.genmodule_writefiles_rules
            .retain(|declaration| is_selected(&declaration.owner));
        self.host_file_generators
            .retain(|declaration| is_selected(&declaration.owner));
        self.sfd_header_rules
            .retain(|declaration| is_selected(&declaration.owner));
        self.host_header_rules.retain(|declaration| {
            is_selected(&declaration.owner) || is_selected(&declaration.setup_owner)
        });
        self.host_header_aggregates
            .retain(|declaration| is_selected(&declaration.owner));
        // Native selection admits only the source-bound aggregate capability.
        // The historical all-tree helper invents mirrors and is not native proof.
        self.host_generated_headers.clear();
        self.sdk_text_rules
            .retain(|declaration| is_selected(&declaration.owner));
        self.source_text_rules
            .retain(|declaration| is_selected(&declaration.owner));
        self.source_value_rules
            .retain(|declaration| is_selected(&declaration.owner));
        self.sdk_file_copies
            .retain(|declaration| is_selected(&declaration.owner));
        self.sdk_asset_rules
            .retain(|declaration| is_selected(&declaration.owner));
        self.sdk_program_outputs.retain(|declaration| {
            is_selected(&declaration.owner)
                && Self::sdk_program_applicable(declaration, Some(context))
        });
        self.sdk_object_groups
            .retain(|declaration| is_selected(&declaration.owner));
        self.literal_object_groups
            .retain(|declaration| is_selected(&declaration.owner));
        self.source_archives.retain(|archive| {
            is_selected(&archive.declaration.owner) || is_selected(&archive.provider_target())
        });
        self.script_outputs
            .retain(|declaration| is_selected(&declaration.owner));
        self.define_headers
            .retain(|declaration| is_selected(&declaration.owner));
        self.header_transforms
            .retain(|declaration| is_selected(&declaration.name));
        self.source_layered_headers
            .retain(|aggregate| is_selected(&aggregate.owner));
        self.flexcat_sources
            .retain(|declaration| is_selected(&declaration.owner));
        self.flexcat_headers
            .retain(|declaration| is_selected(&declaration.owner));
        self.ilbm_sources
            .retain(|declaration| is_selected(&declaration.owner));
        self.bison_outputs
            .retain(|declaration| is_selected(&declaration.owner));
        self.catalogs
            .retain(|declaration| is_selected(&declaration.mmake));
        self.copy_directories
            .retain(|declaration| is_selected(&declaration.name));
        self.icon_targets.retain(|name, _| is_selected(name));
        self.icons
            .retain(|declaration| is_selected(&declaration.mmake));
        self.binary_objects.retain(|declaration| {
            is_selected(&declaration.name) && declaration.applies_to(context)
        });
        // Fetched SDK headers are only admitted with their selected provider;
        // in-tree SDK copies are configure context and remain available.
        self.copy_includes.retain(|declaration| {
            is_selected(&declaration.name) || !declaration.source_dir.contains("${AROS_PORTS_DIR}")
        });
        self.arch_sources.retain(|name, _| declaration_needed(name));
        self.pending_script_outputs.clear();
        let mut selected_clients = BTreeSet::new();
        for target in self.targets.values() {
            for suffix in ["-linklib", "-linklib-rel"] {
                let raw = format!("{}{suffix}", target.mmake_name);
                if selected.contains(&endpoint(&raw, context)?) {
                    selected_clients.insert(raw);
                }
            }
        }
        self.native_selected_client_archives = Some(selected_clients);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{GenmoduleLinklibs, MetaTargetRule, TargetDefinition};
    use crate::fetch::FetchDecl;
    use aros_common::{DiagnosticCode, DiagnosticContext, DiagnosticStage};

    #[test]
    fn optional_cut_cannot_hide_an_independent_required_alias_ingress() {
        let mut graph = DependencyGraph::new();
        for (name, dependency) in [
            ("optional", "alias"),
            ("required", "alias"),
            ("alias", "missing"),
        ] {
            graph.add_meta_rule(MetaTargetRule {
                name: name.into(),
                dependencies: vec![dependency.into()],
            });
        }
        let cuts = BTreeSet::from([("optional".into(), "alias".into())]);
        let context = TargetContext::default();
        let optional_only = graph
            .native_required_metadata_reachability(&["optional".into()], &context, &cuts)
            .unwrap();
        assert!(!optional_only.contains("alias"));
        let shared = graph
            .native_required_metadata_reachability(
                &["optional".into(), "required".into()],
                &context,
                &cuts,
            )
            .unwrap();
        assert!(shared.contains("alias"));
        assert!(shared.contains("missing"));
    }

    #[test]
    fn writefiles_stamp_rejects_selected_python_and_flexcat_owner_collisions() {
        let mut graph = DependencyGraph::new();
        graph.genmodule_writefiles_rules.push(
            crate::genmodule_writefiles_rules::GenmoduleWritefilesRuleDecl {
                owner: "shared-generator".into(),
                file: "unit/mmakefile.src".into(),
                line: 1,
                declaring_dir: "unit".into(),
                config: "unit/module.conf".into(),
                module: "module".into(),
                modtype: crate::genmodule_header_rules::GenmoduleType::Library,
            },
        );
        graph.python_outputs.push(
            serde_json::from_value(serde_json::json!({
                "owner": "shared-generator", "source_root": "ports/source",
                "build_root": "gen/python", "fetch_target": "fixture-fetch",
                "source_inputs": [], "jobs": [], "audited_source_dir": "ports/source",
                "local_patch_files": [], "consumers": [], "dir_path": "unit"
            }))
            .unwrap(),
        );
        graph.add_meta_rule(MetaTargetRule {
            name: "fixture-fetch".into(),
            dependencies: vec![],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: "unrelated-root".into(),
            dependencies: vec![],
        });
        assert!(
            graph
                .selected_dependency_closure(
                    &["unrelated-root".into()],
                    &TargetContext::default(),
                    &[]
                )
                .is_ok(),
            "an unselected collision must not poison unrelated graphs"
        );
        let error = graph
            .selected_dependency_closure(
                &["shared-generator".into()],
                &TargetContext::default(),
                &[],
            )
            .unwrap_err();
        assert!(error.to_string().contains(
            "selected genmodule writefiles stamp shared-generator has conflicting concrete producers"
        ), "{error}");
        graph.python_outputs.clear();
        graph.flexcat_sources.push(
            serde_json::from_value(serde_json::json!({
                "owner": "shared-generator", "declaring_dir": "unit", "line": 1,
                "source": "locale.c", "header": "locale.h", "description": "unit/module.cd",
                "header_template": "unit/header.sd", "source_template": "unit/source.sd",
                "catalog_destination": null, "catalog_name": null, "catalog_source_dir": null,
                "languages": [], "consumers": []
            }))
            .unwrap(),
        );
        assert!(graph
            .selected_dependency_closure(&["unrelated-root".into()], &TargetContext::default(), &[])
            .is_ok());
        let error = graph
            .selected_dependency_closure(
                &["shared-generator".into()],
                &TargetContext::default(),
                &[],
            )
            .unwrap_err();
        assert!(error.to_string().contains(
            "selected genmodule writefiles stamp shared-generator has conflicting concrete producers"
        ), "{error}");
    }

    fn sdk_group(owner: &str) -> crate::sdk_objects::SdkObjectGroupDecl {
        crate::sdk_objects::SdkObjectGroupDecl {
            owner: owner.into(),
            file: "compiler/example/mmakefile.src".into(),
            line: 1,
            objects: vec![crate::sdk_objects::SdkObjectDecl {
                source: "compiler/example/entry.c".into(),
                intermediate: "${AROS_BUILD_DIR}/gen/compiler/example/entry.o".into(),
                output: "${AROS_DEVELOPER_LIB_DIR}/entry.o".into(),
                language: "C".into(),
                defines: Vec::new(),
                undefines: Vec::new(),
                options: Vec::new(),
                includes: Vec::new(),
                line: 4,
            }],
        }
    }

    fn literal_group(owner: &str) -> crate::literal_objects::LiteralObjectGroupDecl {
        crate::literal_objects::LiteralObjectGroupDecl {
            owner: owner.into(),
            file: "compiler/example/mmakefile.src".into(),
            line: 2,
            objects: vec![crate::literal_objects::LiteralObjectDecl {
                source: "${AROS_SOURCE_DIR}/compiler/example/entry.c".into(),
                output: "${AROS_BUILD_DIR}/gen/compiler/example/entry.o".into(),
                language: "C".into(),
                arguments: vec!["-DX=1".into(), "-UX".into(), "-DX=2".into()],
                line: 1,
            }],
        }
    }

    #[test]
    fn literal_object_groups_are_real_selected_providers() {
        let mut graph = DependencyGraph::new();
        graph.literal_object_groups = vec![
            literal_group("literal-selected"),
            literal_group("unrelated"),
        ];
        graph.make_meta_providers.insert("literal-selected".into());
        let context = TargetContext::default();
        let selected = graph
            .selected_dependency_closure(&["literal-selected".into()], &context, &[])
            .unwrap();
        graph.retain_native_selection(&selected, &context).unwrap();
        assert_eq!(graph.literal_object_groups.len(), 1);
        assert_eq!(graph.literal_object_groups[0].owner, "literal-selected");
    }

    #[test]
    fn assembly_header_selection_keeps_its_source_prerequisites_and_emits_real_producers() {
        let mut graph = DependencyGraph::new();
        graph
            .assembly_headers
            .push(crate::assembly_headers::AssemblyHeaderDecl {
                owner: "provider-riscv".into(),
                aggregate_owner: "headers".into(),
                aggregate_dependencies: vec!["prep".into(), "provider-riscv".into()],
                file: "compiler/include/mmakefile.src".into(),
                line: 15,
                source: "${AROS_SOURCE_DIR}/compiler/include/asm.c".into(),
                assembly_output: "${AROS_BUILD_DIR}/gen/include/asm.s".into(),
                header_output: "${AROS_BUILD_DIR}/GENINCDIR/aros/riscv/asm.h".into(),
                header_root: "${AROS_BUILD_DIR}/GENINCDIR".into(),
                arguments: vec!["-DX=1".into(), "-UX".into(), "-DX=2".into()],
                token: ".asciz".into(),
            });
        graph.add_meta_rule(MetaTargetRule {
            name: "prep".into(),
            dependencies: vec![],
        });
        let context = TargetContext::default();
        let selected = graph
            .selected_dependency_closure(&["provider-riscv".into()], &context, &[])
            .unwrap();
        assert!(selected.contains("prep"));
        graph.retain_native_selection(&selected, &context).unwrap();
        assert_eq!(graph.assembly_headers.len(), 1);
        let cmake = crate::generator::generate_cmake(&graph);
        assert!(cmake.contains("aros_generate_assembly_header("));
        assert!(cmake.contains("TOKEN \".asciz\""));
        assert!(cmake.find("-DX=1").unwrap() < cmake.find("-UX").unwrap());
        assert!(cmake.find("-UX").unwrap() < cmake.find("-DX=2").unwrap());
        assert!(!cmake.contains("if(NOT TARGET \"provider-riscv\")"));
        graph.targets.insert(
            "provider-riscv".into(),
            target("provider-riscv", ModuleType::Program),
        );
        assert!(graph
            .selected_dependency_closure(&["provider-riscv".into()], &context, &[])
            .is_err());
    }

    #[test]
    fn assembly_headers_share_aggregates_without_sibling_dependency_cycles() {
        let mut graph = DependencyGraph::new();
        for name in ["first", "second"] {
            graph
                .assembly_headers
                .push(crate::assembly_headers::AssemblyHeaderDecl {
                    owner: name.into(),
                    aggregate_owner: "headers".into(),
                    aggregate_dependencies: vec!["prep".into(), "first".into(), "second".into()],
                    file: "compiler/include/mmakefile.src".into(),
                    line: 15,
                    source: format!("${{AROS_SOURCE_DIR}}/compiler/include/{name}.c"),
                    assembly_output: format!("${{AROS_BUILD_DIR}}/gen/include/{name}.s"),
                    header_output: format!("${{AROS_BUILD_DIR}}/GENINCDIR/aros/{name}.h"),
                    header_root: "${AROS_BUILD_DIR}/GENINCDIR".into(),
                    arguments: vec![],
                    token: ".ascii".into(),
                });
        }
        graph.add_meta_rule(MetaTargetRule {
            name: "prep".into(),
            dependencies: vec![],
        });
        let context = TargetContext::default();
        let selected = graph
            .selected_dependency_closure(&["headers".into()], &context, &[])
            .unwrap();
        for name in ["headers", "first", "second", "prep"] {
            assert!(selected.contains(name));
        }
        let first = graph
            .selected_dependency_closure(&["first".into()], &context, &[])
            .unwrap();
        assert!(!first.contains("second"));
        graph.retain_native_selection(&selected, &context).unwrap();
        let cmake = crate::generator::generate_cmake(&graph);
        assert_eq!(cmake.matches("aros_generate_assembly_header(").count(), 2);
        for declaration in cmake.split("aros_generate_assembly_header(").skip(1) {
            let declaration = declaration.split_once("\n)").unwrap().0;
            assert!(declaration.contains("prep"));
            assert!(!declaration.contains("DEPENDS \"first\""));
            assert!(!declaration.contains("DEPENDS \"second\""));
        }
        graph.assembly_headers[1].header_output = graph.assembly_headers[0].header_output.clone();
        assert!(graph
            .selected_dependency_closure(&["headers".into()], &context, &[])
            .is_err());
    }

    #[test]
    fn selected_architecture_objects_retain_their_module_declaration_without_a_back_edge() {
        use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
        let mut graph = DependencyGraph::new();
        let context = TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("esp32p4".into()),
            ..TargetContext::default()
        };
        // Inventory preparation uses the same declaration retention contract
        // as a full compile graph, without fabricating compile targets.
        graph
            .arch_sources
            .insert("kernel-example".into(), Vec::new());
        graph.arch_sources.insert("unrelated".into(), Vec::new());
        graph.arch_endpoint_effects.push(ArchEndpointEffect {
            recipe: "arch/riscv-all/example/mmakefile.src".into(),
            line: 1,
            endpoint: "kernel-example-riscv".into(),
            dependencies: vec!["kernel-example-riscv-includes".into()],
            data: ArchEndpointEffectData::ArchModuleObjects {
                mainmmake: "kernel-example".into(),
                tag: "riscv".into(),
                module_sources: vec!["entry".into()],
                directory: "arch/riscv-all/example".into(),
            },
        });
        let selected = BTreeSet::from([
            "kernel-example-riscv".into(),
            "kernel-example-riscv-includes".into(),
        ]);
        let edges = graph.selection_edges(&[], false, &context).edges;
        assert_eq!(
            edges["kernel-example-riscv"],
            BTreeSet::from(["kernel-example-riscv-includes".into()])
        );
        graph.retain_native_selection(&selected, &context).unwrap();
        assert!(graph.arch_sources.contains_key("kernel-example"));
        assert!(!graph.arch_sources.contains_key("unrelated"));
        assert_eq!(graph.arch_endpoint_effects.len(), 1);
    }

    #[test]
    fn native_selection_normalizes_handwritten_edge_provenance_with_the_graph() {
        let mut graph = DependencyGraph::new();
        graph.add_explicit_meta_rule(MetaTargetRule {
            name: "consumer-${AROS_TARGET_CPU}".into(),
            dependencies: vec!["linklibs-${AROS_TARGET_CPU}".into()],
        });
        let context = TargetContext {
            cpu: Some("riscv".into()),
            ..TargetContext::default()
        };
        let selected = ["consumer-riscv".into(), "linklibs-riscv".into()]
            .into_iter()
            .collect();
        graph.retain_native_selection(&selected, &context).unwrap();
        assert!(graph.meta_targets["consumer-riscv"].contains("linklibs-riscv"));
        assert_eq!(
            graph.explicit_meta_edges,
            std::iter::once(("consumer-riscv".into(), "linklibs-riscv".into())).collect()
        );
    }

    #[test]
    fn literal_object_groups_refuse_conflicting_kinds_and_outputs() {
        let context = TargetContext::default();
        let mut graph = DependencyGraph::new();
        graph.literal_object_groups = vec![literal_group("shared")];
        graph
            .directory_setups
            .push(crate::directory_setup::DirectorySetupDecl {
                owner: "shared".into(),
                directories: vec!["${AROS_BUILD_DIR}/gen/compiler/example".into()],
            });
        assert!(graph
            .selected_dependency_closure(&["shared".into()], &context, &[])
            .is_err());
        graph.directory_setups.clear();
        graph.literal_object_groups.push(literal_group("other"));
        graph.literal_object_groups[1].objects[0]
            .arguments
            .push("-g".into());
        assert!(graph
            .selected_dependency_closure(&["shared".into(), "other".into()], &context, &[])
            .is_err());
        graph.literal_object_groups.pop();
        graph.sdk_object_groups.push(sdk_group("sdk"));
        assert!(graph
            .selected_dependency_closure(&["shared".into(), "sdk".into()], &context, &[])
            .is_err());
    }

    #[test]
    fn sdk_object_groups_are_real_providers_and_prune_unselected_groups() {
        let mut graph = DependencyGraph::new();
        graph.sdk_object_groups = vec![sdk_group("sdk-selected"), sdk_group("sdk-unrelated")];
        graph.make_meta_providers.insert("sdk-selected".into());
        let context = TargetContext::default();
        let selected = graph
            .selected_dependency_closure(&["sdk-selected".into()], &context, &[])
            .unwrap();
        graph.retain_native_selection(&selected, &context).unwrap();
        assert_eq!(graph.sdk_object_groups.len(), 1);
        assert_eq!(graph.sdk_object_groups[0].owner, "sdk-selected");
    }

    #[test]
    fn shared_sdk_objects_require_identical_declarations_and_unique_provider_kinds() {
        let mut graph = DependencyGraph::new();
        graph.sdk_object_groups = vec![sdk_group("sdk-a"), sdk_group("sdk-b")];
        let roots = ["sdk-a".into(), "sdk-b".into()];
        let context = TargetContext::default();
        assert!(graph
            .selected_dependency_closure(&roots, &context, &[])
            .is_ok());
        graph.sdk_object_groups[1].objects[0]
            .defines
            .push("ALTERED=1".into());
        assert!(graph
            .selected_dependency_closure(&roots, &context, &[])
            .unwrap_err()
            .to_string()
            .contains("conflicting ownership"));
        graph.sdk_object_groups[1].objects[0].defines.clear();
        graph
            .directory_setups
            .push(crate::directory_setup::DirectorySetupDecl {
                owner: "sdk-a".into(),
                directories: vec!["${AROS_BUILD_DIR}/gen/compiler/example".into()],
            });
        assert!(graph
            .selected_dependency_closure(&roots, &context, &[])
            .unwrap_err()
            .to_string()
            .contains("conflicting concrete producers"));
    }

    #[test]
    fn native_binary_dependencies_preserve_the_cmake_architecture_gate() {
        let context = TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("esp32p4".into()),
            ..TargetContext::default()
        };
        let mut graph = DependencyGraph::default();
        graph.add_target(target("kernel", ModuleType::Program));
        for tag in ["pc-i386", "pc-x86_64", "esp32p4-riscv"] {
            graph
                .binary_objects
                .push(crate::binary_objects::BinaryObjectDecl {
                    name: "same-image".into(),
                    output: format!("{tag}/wrapped.o"),
                    directory: format!("arch/{tag}"),
                    sources: vec!["image".into()],
                    start: "0".into(),
                    ldflags: Vec::new(),
                    consumer: "kernel".into(),
                    arch_tag: tag.into(),
                });
        }
        let selected = graph
            .selected_dependency_closure(&["kernel".into()], &context, &[])
            .unwrap();
        assert!(selected.contains("same-image"));
        graph.retain_native_selection(&selected, &context).unwrap();
        assert_eq!(graph.binary_objects.len(), 1);
        assert_eq!(graph.binary_objects[0].arch_tag, "esp32p4-riscv");
        graph.binary_objects.clear();
        graph
            .binary_objects
            .push(crate::binary_objects::BinaryObjectDecl {
                name: "foreign".into(),
                output: "foreign/wrapped.o".into(),
                directory: "arch/x86_64-pc".into(),
                sources: vec!["image".into()],
                start: "0".into(),
                ldflags: Vec::new(),
                consumer: "kernel".into(),
                arch_tag: "pc-x86_64".into(),
            });
        let selected = graph
            .selected_dependency_closure(&["kernel".into()], &context, &[])
            .unwrap();
        assert!(!selected.contains("foreign"));
    }

    fn target(name: &str, module_type: ModuleType) -> TargetDefinition {
        TargetDefinition {
            mmake_name: name.to_owned(),
            target_name: name.to_owned(),
            module_type,
            module_macro: None,
            kobj_scoped_inputs: None,
            genmodule_only: false,
            genmodule_abi: false,
            empty_archive: false,
            source_files: Vec::new(),
            cxx_source_files: Vec::new(),
            always_cxx_link: false,
            no_startup: false,
            detach: false,
            objc_source_files: Vec::new(),
            asm_source_files: Vec::new(),
            use_libs: Vec::new(),
            dependencies: Vec::new(),
            dir_path: std::path::PathBuf::new(),
            target_dir: None,
            variant_32bit: false,
            link_libs: Vec::new(),
            declared_mod_type: None,
            mod_suffix: None,
            linklib_name: None,
            config_file: None,
            config_override_file: None,
            genmodule_linklibs: None,
            config_relative_libraries: Vec::new(),
            linklib_output_dir: None,
            canonical_linklib_output: false,
            canonical_linklib_eligible: false,
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
        }
    }

    fn native_contract_with_link_libraries(names: &[&str]) -> NativeBuildContract {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "profile": "fixture",
            "board": "fixture-board",
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "qualification": "experimental-unqualified",
            "inputs": [],
            "abi": {
                "source_cpu": "riscv",
                "target_triple": "riscv-aros",
                "isa": "rv32imafc",
                "abi": "ilp32f",
                "code_model": "medany",
                "flavour": "standalone",
                "platform_smp": false,
                "use_mmu": false
            },
            "core": {
                "recipe": "kernel/makefile.src",
                "linker_script": "kernel/linker.lds",
                "resources": [],
                "libraries": [],
                "devices": [],
                "link_libraries": names,
                "compiler_runtime_role": "libgcc",
                "residency_check": "kernel/check.sh",
                "residency_policy": {
                    "algorithm": "riscv32-xip-v1",
                    "section": ".sramtext",
                    "flash_start": 1_073_741_824,
                    "flash_end": 1_140_850_688,
                    "sram_start": 1_341_128_704,
                    "sram_end": 1_341_652_992
                }
            },
            "package": {
                "recipe": "package/makefile.src",
                "format": "aros-pkg-v1",
                "target": "fixture-package",
                "limit_from_board": "FIXTURE_PACKAGE_LIMIT"
            },
            "media": {
                "chip": "fixture-chip",
                "board_rules": "board/board.mk",
                "partition_table": "boot/partitions.csv",
                "core_partition": "core",
                "package_partition": "package",
                "development_volume_offset_from_board": "FIXTURE_VOLUME_OFFSET",
                "bootloader_configuration": "boot/config.defaults",
                "bootloader_patch": "boot/fixture.diff",
                "idf_version": "6.0.1"
            }
        }))
        .unwrap()
    }

    fn bound_source_archive(
        owner: &str,
        library: &str,
        file: &str,
    ) -> crate::source_archive_binding::BoundSourceArchive {
        crate::source_archive_binding::BoundSourceArchive {
            declaration: crate::source_archive_rules::SourceArchiveDecl {
                owner: owner.into(),
                file: file.into(),
                line: 12,
                owner_line: 8,
                output: format!("${{AROS_BUILD_DIR}}/SYS/Developer/lib/lib{library}.a"),
                members: crate::source_archive_rules::ArchiveMembers::Exact(Vec::new()),
            },
            command: crate::source_archive_command::SourceArchiveCommand {
                flags: vec!["cr".into()],
            },
            members: Vec::new(),
            compile_groups: Vec::new(),
            producer_owners: BTreeSet::new(),
        }
    }

    fn add_empty_native_package(graph: &mut DependencyGraph) {
        graph.packages.push(crate::packages::PackageDecl {
            file: "package/makefile.src".into(),
            mmake: "fixture-package".into(),
            output: "${AROS_BUILD_DIR}/fixture.pkg".into(),
            members: Vec::new(),
            startup: None,
            uselibs: Vec::new(),
            is_kickstart: false,
            resolved: Vec::new(),
            arch: String::new(),
        });
    }

    fn native_context() -> TargetContext {
        TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("esp32p4".into()),
            ..TargetContext::default()
        }
    }

    #[test]
    fn native_contract_roots_bind_exact_source_archive_names() {
        let mut graph = DependencyGraph::new();
        graph.source_archives.push(bound_source_archive(
            "linklibs-foo-source",
            "foo",
            "compiler/libfoo/mmakefile.src",
        ));
        graph.source_archives.push(bound_source_archive(
            "linklibs-foobar-source",
            "foobar",
            "compiler/libfoobar/mmakefile.src",
        ));
        add_empty_native_package(&mut graph);

        let roots = graph
            .native_contract_roots(
                &native_contract_with_link_libraries(&["foo"]),
                &native_context(),
            )
            .unwrap();

        assert!(roots.contains(&"linklibs-foo-source-archive".to_owned()));
        assert!(!roots.contains(&"linklibs-foobar-source-archive".to_owned()));
    }

    #[test]
    fn native_contract_archive_root_rejects_source_and_target_collision() {
        let mut graph = DependencyGraph::new();
        graph.source_archives.push(bound_source_archive(
            "linklibs-foo-source",
            "foo",
            "compiler/libfoo/mmakefile.src",
        ));
        let mut target = target("native-foo", ModuleType::LinkLib);
        target.target_name = "foo".into();
        target.canonical_linklib_output = true;
        graph.targets.insert(target.mmake_name.clone(), target);
        add_empty_native_package(&mut graph);

        let error = graph
            .native_contract_roots(
                &native_contract_with_link_libraries(&["foo"]),
                &native_context(),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("native archive foo requires one applicable producer"));
        assert!(error.to_string().contains("native-foo"));
        assert!(error.to_string().contains("linklibs-foo-source-archive"));
    }

    #[test]
    fn native_contract_archive_root_keeps_architecture_and_32bit_filters() {
        let mut graph = DependencyGraph::new();
        graph.source_archives.push(bound_source_archive(
            "linklibs-foo-source",
            "foo",
            "compiler/libfoo/mmakefile.src",
        ));
        let mut legacy = target("legacy-foo", ModuleType::LinkLib);
        legacy.target_name = "foo".into();
        legacy.variant_32bit = true;
        legacy.canonical_linklib_output = true;
        graph.targets.insert(legacy.mmake_name.clone(), legacy);
        add_empty_native_package(&mut graph);

        let roots = graph
            .native_contract_roots(
                &native_contract_with_link_libraries(&["foo"]),
                &native_context(),
            )
            .unwrap();
        assert!(roots.contains(&"linklibs-foo-source-archive".to_owned()));

        graph.source_archives[0].declaration.file =
            "arch/i386-pc/compiler/libfoo/mmakefile.src".into();
        let error = graph
            .native_contract_roots(
                &native_contract_with_link_libraries(&["foo"]),
                &native_context(),
            )
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("native archive foo requires one applicable producer"));
        assert!(error.to_string().contains("found []"));
    }

    fn fetch(name: &str, destination: &str) -> FetchDecl {
        FetchDecl {
            name: name.to_owned(),
            archive: name.to_owned(),
            suffixes: "tar.gz".to_owned(),
            origins: "cache://".to_owned(),
            checksums: String::new(),
            location: "${AROS_PORTS_SOURCE_DIR}".to_owned(),
            destination: destination.to_owned(),
            base: destination.to_owned(),
            patch_origins: String::new(),
            patches: String::new(),
            dir: "ports".to_owned(),
        }
    }

    fn copy_includes(name: &str, source_dir: &str) -> crate::CopyIncludesDecl {
        crate::CopyIncludesDecl {
            name: name.to_owned(),
            dest: "fixture".to_owned(),
            source_dir: source_dir.to_owned(),
            patterns: vec!["fixture.h".to_owned()],
            excludes: Vec::new(),
            flatten: false,
            proven_empty: false,
        }
    }

    fn diagnostic(owner: Option<&str>) -> Diagnostic {
        Diagnostic::error(
            DiagnosticCode::CapabilityDrift,
            DiagnosticStage::CapabilityValidation,
            "fixture capability is unavailable",
        )
        .with_context(DiagnosticContext {
            target: owner.map(str::to_owned),
            ..DiagnosticContext::default()
        })
    }

    fn optional_variant() -> NativeOptionalMetaDependency {
        NativeOptionalMetaDependency {
            recipe: "compiler/mmakefile.src".into(),
            target: "includes".into(),
            dependency: "includes-${AROS_TARGET_PLATFORM}-${AROS_TARGET_VARIANT}".into(),
            absence: NativeMetaAbsence::Selector,
        }
    }

    fn optional_graph() -> (DependencyGraph, TargetContext) {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "includes".into(),
            dependencies: vec![optional_variant().dependency, "required-headers".into()],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: "required-headers".into(),
            dependencies: vec![],
        });
        let context = TargetContext {
            platform: Some("fixture".into()),
            variant: Some(String::new()),
            ..TargetContext::default()
        };
        (graph, context)
    }

    #[test]
    fn optional_selector_cannot_erase_an_active_virtual_dependency() {
        let (mut graph, context) = optional_graph();
        assert!(graph
            .selected_dependency_closure(&["includes".into()], &context, &[])
            .is_err());
        let error = graph
            .omit_absent_optional_meta_dependencies(&[optional_variant()], &context, &[])
            .unwrap_err()
            .to_string();
        assert!(error.contains("includes-fixture-"));
        assert!(error.contains("fix upstream"));
        assert!(error.contains("compiler/mmakefile.src"));
        assert!(graph.meta_targets["includes"].contains(&optional_variant().dependency));
        graph
            .meta_targets
            .get_mut("includes")
            .unwrap()
            .insert("literal-typo".into());
        assert!(graph
            .selected_dependency_closure(&["includes".into()], &context, &[])
            .is_err());
    }

    #[test]
    fn optional_selector_preserves_present_and_rejected_providers() {
        for rejected in [false, true] {
            let (mut graph, context) = optional_graph();
            graph.add_meta_rule(MetaTargetRule {
                name: "includes-fixture-".into(),
                dependencies: vec![],
            });
            let diagnostics = if rejected {
                graph.make_meta_providers.insert("includes-fixture-".into());
                vec![diagnostic(Some("includes-fixture-"))]
            } else {
                vec![]
            };
            assert!(graph
                .omit_absent_optional_meta_dependencies(
                    &[optional_variant()],
                    &context,
                    &diagnostics
                )
                .unwrap()
                .is_empty());
            let selected =
                graph.selected_dependency_closure(&["includes".into()], &context, &diagnostics);
            if rejected {
                assert!(selected
                    .unwrap_err()
                    .to_string()
                    .contains("fixture capability"));
            } else {
                assert!(selected.unwrap().contains("includes-fixture-"));
            }
        }
    }

    #[test]
    fn disabled_owner_dependency_is_an_upstream_error_not_an_optional_omission() {
        let declaration = NativeOptionalMetaDependency {
            recipe: "compiler/mmakefile.src".into(),
            target: "includes".into(),
            dependency: "disabled-pkgconfig".into(),
            absence: NativeMetaAbsence::DisabledOwner,
        };
        let context = TargetContext::default();
        for active in [false, true] {
            let mut graph = DependencyGraph::new();
            graph.add_meta_rule(MetaTargetRule {
                name: "includes".into(),
                dependencies: vec![declaration.dependency.clone()],
            });
            let mut attributed = diagnostic(Some(&declaration.dependency));
            attributed.context.as_mut().unwrap().mode =
                Some(crate::sdk_text_rules::DISABLED_OWNER_DIAGNOSTIC_MODE.into());
            let mut diagnostics = vec![attributed];
            if active {
                // A separate active-but-rejected declaration still vetoes omission.
                diagnostics.push(diagnostic(Some(&declaration.dependency)));
            }
            let error = graph.omit_absent_optional_meta_dependencies(
                std::slice::from_ref(&declaration),
                &context,
                &diagnostics,
            );
            if active {
                assert!(error.unwrap().is_empty());
            } else {
                let error = error.unwrap_err().to_string();
                assert!(error.contains("fix upstream"));
                assert!(error.contains("compiler/mmakefile.src"));
                assert!(error.contains("disabled-pkgconfig"));
            }
            assert!(graph.meta_targets["includes"].contains("disabled-pkgconfig"));
            assert!(graph
                .selected_dependency_closure(&["includes".into()], &context, &diagnostics)
                .is_err());
        }
    }

    #[test]
    fn disabled_owner_attribution_never_hides_a_declared_provider() {
        let declaration = NativeOptionalMetaDependency {
            recipe: "compiler/mmakefile.src".into(),
            target: "includes".into(),
            dependency: "disabled-pkgconfig".into(),
            absence: NativeMetaAbsence::DisabledOwner,
        };
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "includes".into(),
            dependencies: vec![declaration.dependency.clone()],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: declaration.dependency.clone(),
            dependencies: vec![],
        });
        let mut attributed = diagnostic(Some(&declaration.dependency));
        attributed.context.as_mut().unwrap().mode =
            Some(crate::sdk_text_rules::DISABLED_OWNER_DIAGNOSTIC_MODE.into());
        let diagnostics = [attributed];
        let context = TargetContext::default();
        assert!(graph
            .omit_absent_optional_meta_dependencies(&[declaration], &context, &diagnostics)
            .unwrap()
            .is_empty());
        assert!(graph
            .selected_dependency_closure(&["includes".into()], &context, &diagnostics)
            .is_err());
    }

    #[test]
    fn disabled_owner_attribution_without_optional_contract_remains_fatal() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "includes".into(),
            dependencies: vec!["disabled-pkgconfig".into()],
        });
        let mut attributed = diagnostic(Some("disabled-pkgconfig"));
        attributed.context.as_mut().unwrap().mode =
            Some(crate::sdk_text_rules::DISABLED_OWNER_DIAGNOSTIC_MODE.into());
        assert!(graph
            .selected_dependency_closure(
                &["includes".into()],
                &TargetContext::default(),
                &[attributed],
            )
            .is_err());
    }

    #[test]
    fn optional_selectors_reject_unknown_literal_and_unproven_edges() {
        let (graph, mut context) = optional_graph();
        context.variant = None;
        assert!(graph
            .omit_absent_optional_meta_dependencies(&[optional_variant()], &context, &[])
            .is_err());
        // Validation is transactional: the failed probe leaves the graph intact.
        assert!(graph.meta_targets["includes"].contains(&optional_variant().dependency));
        context.variant = Some(String::new());
        let mut wrong = optional_variant();
        wrong.dependency = "literal-typo".into();
        assert!(graph
            .omit_absent_optional_meta_dependencies(&[wrong], &context, &[])
            .is_err());
        wrong = optional_variant();
        wrong.target = "unproven".into();
        assert!(graph
            .omit_absent_optional_meta_dependencies(&[wrong], &context, &[])
            .is_err());
    }

    #[test]
    fn optional_architecture_edge_does_not_prove_concrete_owner_capability() {
        for supported in [false, true] {
            let (mut graph, context) = optional_graph();
            graph.make_meta_providers.insert("includes".into());
            if supported {
                graph.add_target(target("includes", ModuleType::LinkLib));
            }
            let diagnostics = if supported {
                vec![]
            } else {
                vec![diagnostic(Some("includes"))]
            };
            assert!(graph
                .omit_absent_optional_meta_dependencies(
                    &[optional_variant()],
                    &context,
                    &diagnostics
                )
                .is_err());
            let selected =
                graph.selected_dependency_closure(&["includes".into()], &context, &diagnostics);
            assert!(selected.is_err());
        }
    }

    #[test]
    fn exact_closed_roots_exclude_unrelated_rejected_capabilities() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "core".into(),
            dependencies: vec!["archive".into()],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: "archive".into(),
            dependencies: vec!["headers".into()],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: "headers".into(),
            dependencies: vec![],
        });
        let result = graph
            .selected_dependency_closure(
                &["core".into()],
                &TargetContext::default(),
                &[diagnostic(Some("unrelated"))],
            )
            .unwrap();
        assert_eq!(
            result,
            BTreeSet::from(["core".into(), "archive".into(), "headers".into()])
        );
    }

    #[test]
    fn unselected_dynamic_aggregate_does_not_relax_selected_endpoints() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "core".into(),
            dependencies: vec![],
        });
        graph.add_meta_rule(MetaTargetRule {
            name: "icons-${UNKNOWN}".into(),
            dependencies: vec!["also-${UNKNOWN}".into()],
        });
        assert!(graph
            .selected_dependency_closure(&["core".into()], &TargetContext::default(), &[])
            .is_ok());
        assert!(graph
            .selected_dependency_closure(
                &["icons-${UNKNOWN}".into()],
                &TargetContext::default(),
                &[]
            )
            .is_err());
        graph.add_meta_rule(MetaTargetRule {
            name: "core".into(),
            dependencies: vec!["icons-${UNKNOWN}".into()],
        });
        assert!(graph
            .selected_dependency_closure(&["core".into()], &TargetContext::default(), &[])
            .is_err());
    }

    #[test]
    fn selected_unowned_missing_and_dynamic_dependencies_fail_closed() {
        let mut graph = DependencyGraph::new();
        graph.add_meta_rule(MetaTargetRule {
            name: "core".into(),
            dependencies: vec!["rejected".into()],
        });
        assert!(graph
            .selected_dependency_closure(
                &["core".into()],
                &TargetContext::default(),
                &[diagnostic(Some("rejected"))]
            )
            .is_err());
        assert!(graph
            .selected_dependency_closure(
                &["core".into()],
                &TargetContext::default(),
                &[diagnostic(None)]
            )
            .is_err());
        assert!(graph
            .selected_dependency_closure(&["missing".into()], &TargetContext::default(), &[])
            .is_err());
        assert!(graph
            .selected_dependency_closure(&["core".into()], &TargetContext::default(), &[])
            .is_err());
        assert!(graph
            .selected_dependency_closure(&[], &TargetContext::default(), &[])
            .is_err());
        assert!(endpoint("core-${AROS_TARGET_CPU}", &TargetContext::default()).is_err());
        assert!(endpoint("core-${UNKNOWN}", &TargetContext::default()).is_err());
    }

    #[test]
    fn dependency_selector_uses_explicit_source_context() {
        let context = TargetContext {
            cpu: Some("riscv".into()),
            platform: Some("fixture".into()),
            ..TargetContext::default()
        };
        assert_eq!(
            endpoint("kernel-${AROS_TARGET_LEGACY_PLATFORM}", &context).unwrap(),
            "kernel-fixture-riscv"
        );
        assert!(endpoint("../core", &context).is_err());
    }

    #[test]
    fn genmodule_only_library_does_not_invent_a_relative_archive_endpoint() {
        let mut graph = DependencyGraph::new();
        let mut library = target("fixture-library", ModuleType::Library);
        library.genmodule_only = true;
        library.genmodule_linklibs = Some(GenmoduleLinklibs {
            enabled: true,
            has_relative: true,
            ..GenmoduleLinklibs::default()
        });
        graph.targets.insert(library.mmake_name.clone(), library);
        let error = graph
            .selected_dependency_closure(
                &["fixture-library-linklib-rel".to_owned()],
                &TargetContext::default(),
                &[],
            )
            .unwrap_err();
        assert!(error.to_string().contains("has no proven endpoint"));
    }

    #[test]
    fn genmodule_only_library_archive_selects_runtime_dependencies_and_guarded_defaults() {
        let mut graph = DependencyGraph::new();
        let mut library = target("fixture-library", ModuleType::Library);
        library.genmodule_only = true;
        library.genmodule_linklibs = Some(GenmoduleLinklibs {
            enabled: true,
            ..GenmoduleLinklibs::default()
        });
        library.dependencies = vec!["runtime-prerequisite".to_owned()];
        library.link_libs = vec!["runtime-link-provider".to_owned()];
        library.spec_switches = vec!["static".to_owned()];
        graph.targets.insert(library.mmake_name.clone(), library);
        for name in [
            "fixture-library-includes",
            "runtime-prerequisite",
            "runtime-link-provider",
            "stdc-static-provider",
            "stdc-dynamic-provider",
        ] {
            graph.add_meta_rule(MetaTargetRule {
                name: name.to_owned(),
                dependencies: Vec::new(),
            });
        }
        graph.add_meta_rule(MetaTargetRule {
            name: "fixture-library-linklib".to_owned(),
            dependencies: vec!["fixture-library-includes".to_owned()],
        });
        graph.default_link_set = vec![
            crate::graph::ResolvedDefaultLinkItem {
                name: "stdc.static".to_owned(),
                archive: "stdc-static-provider".to_owned(),
                require_absent: Vec::new(),
                require_present: vec!["nostdc".to_owned()],
            },
            crate::graph::ResolvedDefaultLinkItem {
                name: "stdc".to_owned(),
                archive: "stdc-dynamic-provider".to_owned(),
                require_absent: vec!["nostdc".to_owned()],
                require_present: Vec::new(),
            },
        ];

        let selected = graph
            .selected_dependency_closure(
                &["fixture-library-linklib".to_owned()],
                &TargetContext::default(),
                &[],
            )
            .unwrap();

        assert!(selected.contains("fixture-library"));
        assert!(selected.contains("fixture-library-includes"));
        assert!(selected.contains("runtime-prerequisite"));
        assert!(selected.contains("runtime-link-provider"));
        assert!(selected.contains("stdc-static-provider"));
        assert!(!selected.contains("stdc-dynamic-provider"));
    }

    #[test]
    fn full_library_relative_archive_alias_selects_runtime_but_abi_alias_does_not() {
        let mut graph = DependencyGraph::new();
        let mut library = target("full-library", ModuleType::Library);
        library.source_files = vec!["library.c".to_owned()];
        library.genmodule_linklibs = Some(GenmoduleLinklibs {
            enabled: true,
            has_relative: true,
            ..GenmoduleLinklibs::default()
        });
        graph.targets.insert(library.mmake_name.clone(), library);
        let abi = target("abi-only", ModuleType::Abi);
        graph.targets.insert(abi.mmake_name.clone(), abi);

        let relative = graph
            .selected_dependency_closure(
                &["full-library-linklib-rel".to_owned()],
                &TargetContext::default(),
                &[],
            )
            .unwrap();
        assert!(relative.contains("full-library"));

        graph
            .default_link_set
            .push(crate::graph::ResolvedDefaultLinkItem {
                name: "archive".to_owned(),
                archive: "default-archive".to_owned(),
                require_absent: Vec::new(),
                require_present: Vec::new(),
            });
        graph.add_meta_rule(MetaTargetRule {
            name: "default-archive".to_owned(),
            dependencies: Vec::new(),
        });
        let abi_archive = graph
            .selected_dependency_closure(
                &["abi-only-linklib".to_owned()],
                &TargetContext::default(),
                &[],
            )
            .unwrap();
        assert!(abi_archive.contains("abi-only-linklib"));
        assert!(!abi_archive.contains("abi-only"));
        assert!(!abi_archive.contains("default-archive"));
    }

    #[test]
    fn fetched_copy_includes_uses_the_unique_longest_owner() {
        let mut graph = DependencyGraph::new();
        graph.fetches = vec![
            fetch("broad-fetch", "${AROS_PORTS_DIR}/fixture"),
            fetch("header-fetch", "${AROS_PORTS_DIR}/fixture/source"),
        ];
        graph.copy_includes = vec![copy_includes(
            "fixture-headers",
            "${AROS_PORTS_DIR}/fixture/source/include",
        )];

        let selected = graph
            .selected_dependency_closure(
                &["fixture-headers".to_owned()],
                &TargetContext::default(),
                &[],
            )
            .unwrap();

        assert!(selected.contains("header-fetch"));
        assert!(!selected.contains("broad-fetch"));
    }

    #[test]
    fn fetched_copy_includes_reject_selected_ties_but_ignore_unselected_ties() {
        let mut graph = DependencyGraph::new();
        graph.fetches = vec![
            fetch("first-fetch", "${AROS_PORTS_DIR}/fixture"),
            fetch("second-fetch", "${AROS_PORTS_DIR}/fixture"),
        ];
        graph.copy_includes = vec![copy_includes(
            "fixture-headers",
            "${AROS_PORTS_DIR}/fixture/include",
        )];
        graph.add_meta_rule(MetaTargetRule {
            name: "unrelated-root".to_owned(),
            dependencies: Vec::new(),
        });

        assert!(graph
            .selected_dependency_closure(
                &["unrelated-root".to_owned()],
                &TargetContext::default(),
                &[],
            )
            .is_ok());
        assert!(graph
            .selected_dependency_closure(
                &["fixture-headers".to_owned()],
                &TargetContext::default(),
                &[],
            )
            .is_err());
    }
}
