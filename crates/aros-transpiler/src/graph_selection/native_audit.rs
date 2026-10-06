//! Native dependency graph audit, optional-edge validation and contract roots.

use super::{
    add, arch_compatible, arch_of, endpoint, failure, has_public_link_archive,
    inventory_has_public_link_archive, inventory_runtime_name, target_runtime_name, ArosError,
    BTreeMap, BTreeSet, DependencyGraph, Diagnostic, DiagnosticCode, DiagnosticSet,
    DiagnosticStage, Edges, ModuleType, NativeBuildContract, NativeMetaAbsence,
    NativeOptionalMetaDependency, Result, TargetContext,
};

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
    ) -> super::super::NativeGraphAudit {
        let mut report = super::super::NativeGraphAudit::new(roots);
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
                    .push(super::super::audit::audit_endpoint(
                        &name, &parents, &explicit,
                    ));
            }
            let Some(dependencies) = edges.get(&name) else {
                report
                    .missing_endpoints
                    .push(super::super::audit::audit_endpoint(
                        &name, &parents, &explicit,
                    ));
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
                report.partial_source_projections.push(
                    super::super::audit::PartialSourceProjection {
                        family: "source-archive",
                        owner: projection.owner.clone(),
                        outputs: 1,
                        unresolved_contracts: vec![
                            "exact registered compiler output ownership".into(),
                            "source-bound archive macro and tool roles".into(),
                        ],
                    },
                );
            }
        }
        for projection in &self.layered_header_projections {
            if self.source_layered_headers.contains(projection) {
                continue;
            }
            if report.reachable.contains(&projection.owner) {
                report.partial_source_projections.push(
                    super::super::audit::PartialSourceProjection {
                        family: "layered-header-copy",
                        owner: projection.owner.clone(),
                        outputs: projection.copies.len(),
                        unresolved_contracts: projection.unresolved_prerequisites.clone(),
                    },
                );
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
                report.partial_source_projections.push(
                    super::super::audit::PartialSourceProjection {
                        family: "source-compile",
                        owner: projection.owner.clone(),
                        outputs: projection.objects.len(),
                        unresolved_contracts: vec![
                            "typed object, archive and prerequisite ownership binding".into(),
                        ],
                    },
                );
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
}
