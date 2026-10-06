//! Source-local validation for nonvirtual MetaMake providers.
//!
//! A plain `#MM` declaration causes MetaMake to invoke Make in the declaring
//! makefile. A concrete target with the same spelling in another makefile is
//! not evidence that this provider's recipe is represented. Run this check on
//! each [`ParsedMmakefile`] before merging parse results into the global graph.

use std::collections::BTreeSet;

use crate::ast::{ModuleType, ParsedMmakefile};

/// Recognize deliberately disabled literal owners without executing comments.
/// Only `##MM name : ...`, or a bare `##MM` immediately followed by the
/// commented `#name : ...`, is proof. Prose, variables, continuations and
/// indentation are not promoted to declarations. Classic MetaMake scans #MM
/// at column zero and does not execute either of these disabled forms.
pub fn disabled_owners(content: &str) -> Vec<String> {
    let mut result = BTreeSet::new();
    let mut bare = false;
    for line in content.lines() {
        let line = line.trim_end_matches('\r');
        let declaration = if let Some(rest) = line.strip_prefix("##MM") {
            bare = rest.trim().is_empty();
            if bare {
                continue;
            }
            rest.strip_prefix([' ', '\t'])
        } else if bare {
            bare = false;
            line.strip_prefix('#')
        } else {
            None
        };
        if let Some(declaration) = declaration {
            if declaration.ends_with('\\') {
                continue;
            }
            if let Some((name, _)) = declaration.split_once(':') {
                let name = name.trim();
                if !name.is_empty()
                    && name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+')
                    })
                {
                    result.insert(name.to_owned());
                }
            }
        }
    }
    result.into_iter().collect()
}

/// A nonvirtual source provider that has no concrete source-local CMake
/// capability to replace its Make invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedProvider {
    pub(super) owner: String,
    pub(super) reason: String,
}

/// Checks each plain `#MM` or bare-marker provider against endpoints materialized
/// by declarations in this same makefile. `meta_rules` are intentionally not
/// producers: their generated utility targets only model dependency edges.
#[must_use]
///
/// With a selected target, a provider spelled with selector placeholders
/// (`includes-asm_h-${AROS_TARGET_CPU}`) is matched by its exact resolved
/// name, as a local producer registers it concretely.
pub fn validate(
    parsed: &ParsedMmakefile,
    target: Option<&crate::TargetContext>,
) -> Vec<RejectedProvider> {
    let mut local_producers = BTreeSet::new();
    for header in &parsed.assembly_headers {
        local_producers.insert(header.owner.clone());
        local_producers.insert(header.aggregate_owner.clone());
    }
    local_producers.extend(
        parsed
            .arch_endpoint_effects
            .iter()
            .map(|effect| effect.endpoint.clone()),
    );
    local_producers.extend(
        parsed
            .host_header_aggregates
            .iter()
            .map(|rule| rule.owner.clone()),
    );

    // Every admitted build declaration below emits its own named CMake target.
    local_producers.extend(
        parsed
            .targets
            .iter()
            .filter(|target| target.module_type != ModuleType::ModuleHeaders)
            .map(|target| target.mmake_name.clone()),
    );
    local_producers.extend(
        parsed
            .directory_setups
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .genmodule_header_rules
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    for declaration in &parsed.host_header_rules {
        local_producers.insert(declaration.owner.clone());
        local_producers.insert(declaration.setup_owner.clone());
    }
    local_producers.extend(
        parsed
            .genmodule_writefiles_rules
            .iter()
            .map(|rule| rule.owner.clone()),
    );
    local_producers.extend(
        parsed
            .host_file_generators
            .iter()
            .map(|rule| rule.owner.clone()),
    );
    local_producers.extend(
        parsed
            .sdk_text_rules
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .sfd_header_rules
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .source_text_rules
            .iter()
            .map(|rule| rule.owner.clone()),
    );
    local_producers.extend(parsed.sdk_file_copies.iter().map(|rule| rule.owner.clone()));
    local_producers.extend(parsed.sdk_asset_rules.iter().map(|rule| rule.owner.clone()));
    local_producers.extend(
        parsed
            .source_value_rules
            .iter()
            .map(|rule| rule.owner.clone()),
    );
    local_producers.extend(
        parsed
            .sdk_object_groups
            .iter()
            .map(|rule| rule.owner.clone()),
    );
    local_producers.extend(
        parsed
            .literal_object_groups
            .iter()
            .map(|rule| rule.owner.clone()),
    );
    local_producers.extend(
        parsed
            .fetches
            .iter()
            .map(|declaration| declaration.name.clone()),
    );
    local_producers.extend(
        parsed
            .packages
            .iter()
            .map(|declaration| declaration.mmake.clone()),
    );
    local_producers.extend(parsed.external_cmake.iter().flat_map(|declaration| {
        [
            declaration.mmake_name.clone(),
            declaration.provider_target.clone(),
        ]
    }));
    local_producers.extend(parsed.configure_builds.iter().flat_map(|declaration| {
        std::iter::once(declaration.mmake_name.clone()).chain(declaration.provider_target.clone())
    }));
    local_producers.extend(
        parsed
            .grub_builds
            .iter()
            .map(|declaration| declaration.mmake_name.clone()),
    );
    local_producers.extend(
        parsed
            .ahi_builds
            .iter()
            .map(|declaration| declaration.mmake_name.clone()),
    );
    local_producers.extend(
        parsed
            .python_outputs
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .flexcat_sources
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .flexcat_headers
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .ilbm_sources
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .catalogs
            .iter()
            .map(|declaration| declaration.mmake.clone()),
    );
    local_producers.extend(
        parsed
            .copy_includes
            .iter()
            .map(|declaration| declaration.name.clone()),
    );
    local_producers.extend(
        parsed
            .copy_directories
            .iter()
            .map(|declaration| declaration.name.clone()),
    );
    local_producers.extend(
        parsed
            .header_transforms
            .iter()
            .map(|declaration| declaration.name.clone()),
    );
    local_producers.extend(
        parsed
            .define_headers
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .bison_outputs
            .iter()
            .map(|declaration| declaration.owner.clone()),
    );
    local_producers.extend(
        parsed
            .icon_targets
            .iter()
            .map(|declaration| declaration.mmake.clone()),
    );
    // Resolved icon output sets also emit the named target. Normally the
    // persistent IconTarget is present alongside each set; retain this source
    // of ownership for hand-built ParsedMmakefile values as well.
    local_producers.extend(
        parsed
            .icons
            .iter()
            .map(|declaration| declaration.mmake.clone()),
    );
    local_producers.extend(
        parsed
            .binary_objects
            .iter()
            .map(|declaration| declaration.name.clone()),
    );

    // These aliases exist only when the corresponding CMake module helper
    // emits them. In particular, an arbitrary `<name>-includes` token is not
    // made concrete by its suffix alone.
    for target in &parsed.targets {
        let full_genmodule = target.module_type == ModuleType::Library
            && (target.genmodule_only
                || target
                    .genmodule_linklibs
                    .as_ref()
                    .is_some_and(|metadata| metadata.enabled));
        if target.genmodule_abi || target.module_type == ModuleType::Abi || full_genmodule {
            local_producers.insert(format!("{}-includes", target.mmake_name));
        }
        if target.module_type == ModuleType::Abi || full_genmodule {
            local_producers.insert(format!("{}-linklib", target.mmake_name));
        }
        if target.module_type == ModuleType::Abi {
            local_producers.insert(format!("{}-fd", target.mmake_name));
        }
        if target.module_type == ModuleType::Library
            && !target.genmodule_only
            && target
                .genmodule_linklibs
                .as_ref()
                .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
        {
            local_producers.insert(format!("{}-linklib-rel", target.mmake_name));
        }
    }

    parsed
        .make_meta_providers
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|owner| {
            let resolved_locally = || {
                owner.contains("${")
                    && target
                        .and_then(|target| crate::graph::native_endpoint(owner, target).ok())
                        .is_some_and(|resolved| local_producers.contains(&resolved))
            };
            !local_producers.contains(*owner) && !resolved_locally()
        })
        .map(|owner| RejectedProvider {
            owner: owner.clone(),
            reason: format!(
                "nonvirtual Make provider {owner} has no concrete producer in its declaring mmakefile"
            ),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{GenmoduleLinklibs, TargetDefinition};
    use std::path::PathBuf;

    #[test]
    fn disabled_owner_proofs_are_literal_adjacent_original_comments() {
        assert_eq!(
            disabled_owners(
                "##MM\r\n#metadata-owner : $(AROS_LIB)/pkgconfig/owner.pc\r\n##MM\tinline-owner :\n"
            ),
            vec!["inline-owner", "metadata-owner"]
        );
        for unproven in [
            "#MM owner :\n",
            "#owner :\n",
            " ##MM owner :\n",
            "##MM\n\n#owner :\n",
            "##MM owner : \\\n",
            "##MM owner other :\n",
            "##MM owner-${CPU} :\n",
            "##MM prose\n#owner :\n",
        ] {
            assert!(disabled_owners(unproven).is_empty(), "{unproven:?}");
        }
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
            dir_path: PathBuf::new(),
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

    #[test]
    fn a_provider_cannot_borrow_a_concrete_target_from_another_mmakefile() {
        let abi_file = ParsedMmakefile {
            targets: vec![target("fixture-abi", ModuleType::Abi)],
            ..ParsedMmakefile::default()
        };
        let provider_file = ParsedMmakefile {
            make_meta_providers: vec!["fixture-abi-includes".to_owned()],
            ..ParsedMmakefile::default()
        };

        assert!(validate(&abi_file, None).is_empty());
        let rejected = validate(&provider_file, None);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].owner, "fixture-abi-includes");
    }

    #[test]
    fn generated_includes_alias_requires_an_actual_abi_or_full_genmodule_target() {
        let abi = target("fixture-abi", ModuleType::Abi);
        let mut full_library = target("fixture-full", ModuleType::Library);
        full_library.genmodule_linklibs = Some(GenmoduleLinklibs {
            enabled: true,
            ..GenmoduleLinklibs::default()
        });
        let mut genmodule_only = target("fixture-only", ModuleType::Library);
        genmodule_only.genmodule_only = true;
        let ordinary = target("fixture-ordinary", ModuleType::Device);
        let valid = ParsedMmakefile {
            targets: vec![abi, full_library, genmodule_only],
            make_meta_providers: vec![
                "fixture-abi-includes".to_owned(),
                "fixture-full-includes".to_owned(),
                "fixture-only-includes".to_owned(),
            ],
            ..ParsedMmakefile::default()
        };
        assert!(validate(&valid, None).is_empty());

        let invalid = ParsedMmakefile {
            targets: vec![ordinary],
            make_meta_providers: vec!["fixture-ordinary-includes".to_owned()],
            ..ParsedMmakefile::default()
        };
        assert_eq!(
            validate(&invalid, None)[0].owner,
            "fixture-ordinary-includes"
        );
    }

    #[test]
    fn relative_linklib_alias_matches_the_full_library_helper_branch() {
        let mut full_library = target("fixture-full", ModuleType::Library);
        full_library.genmodule_linklibs = Some(GenmoduleLinklibs {
            enabled: true,
            has_relative: true,
            ..GenmoduleLinklibs::default()
        });
        let mut genmodule_only = target("fixture-only", ModuleType::Library);
        genmodule_only.genmodule_only = true;
        genmodule_only.genmodule_linklibs = Some(GenmoduleLinklibs {
            enabled: true,
            has_relative: true,
            ..GenmoduleLinklibs::default()
        });
        let parsed = ParsedMmakefile {
            targets: vec![full_library, genmodule_only],
            make_meta_providers: vec![
                "fixture-full-linklib-rel".to_owned(),
                "fixture-only-linklib-rel".to_owned(),
            ],
            ..ParsedMmakefile::default()
        };

        let rejected = validate(&parsed, None);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].owner, "fixture-only-linklib-rel");
    }

    #[test]
    fn explicit_source_local_directory_capability_proves_its_provider() {
        let parsed = ParsedMmakefile {
            directory_setups: vec![crate::directory_setup::DirectorySetupDecl {
                owner: "fixture-prepare".to_owned(),
                directories: vec!["${AROS_BUILD_DIR}/gen/include".to_owned()],
            }],
            make_meta_providers: vec!["fixture-prepare".to_owned()],
            ..ParsedMmakefile::default()
        };
        assert!(validate(&parsed, None).is_empty());
    }

    #[test]
    fn placeholder_provider_matches_its_resolved_local_producer_only_with_a_target() {
        let parsed = ParsedMmakefile {
            directory_setups: vec![crate::directory_setup::DirectorySetupDecl {
                owner: "fixture-riscv".to_owned(),
                directories: vec!["${AROS_BUILD_DIR}/gen/include".to_owned()],
            }],
            make_meta_providers: vec!["fixture-${AROS_TARGET_CPU}".to_owned()],
            ..ParsedMmakefile::default()
        };
        assert_eq!(validate(&parsed, None).len(), 1);
        let riscv = crate::TargetContext {
            cpu: Some("riscv".to_owned()),
            ..crate::TargetContext::default()
        };
        assert!(validate(&parsed, Some(&riscv)).is_empty());
        let arm = crate::TargetContext {
            cpu: Some("arm".to_owned()),
            ..crate::TargetContext::default()
        };
        assert_eq!(validate(&parsed, Some(&arm)).len(), 1);
    }
}
