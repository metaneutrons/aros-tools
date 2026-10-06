//! Selected dependency closure and retained native selection.

use super::{
    add, endpoint, failure, ArosError, BTreeMap, BTreeSet, DependencyGraph, Diagnostic,
    DiagnosticSet, Edges, ModuleType, Result, SelectionEdges, TargetContext,
};

impl DependencyGraph {
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
