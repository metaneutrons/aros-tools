//! Parser-input identities for native producer endpoints.
//!
//! This index is intentionally built from declarations in one
//! [`ParsedMmakefile`], before cross-file graph binding. It records producer
//! owners and aliases, not prerequisite names: a dependency written in one
//! file does not make that file the producer of the dependency endpoint.
//!
//! Some graph passes synthesize owners only after they join declarations from
//! multiple files. Exact deterministic source-object and directory aliases
//! are indexed through the graph's shared identity helpers; generated script
//! owners are instead mapped from [`script_input_identities`] after consumer
//! binding. The caller must fail closed when any other reached available
//! endpoint has no indexed parser input.

use crate::ast::{InventoryTargetIdentity, ParsedMmakefile, TargetDefinition};
use crate::parser::TargetContext;
use crate::ModuleType;
use std::collections::BTreeSet;
use std::path::Path;

/// Returns native producer endpoint identities declared by one parser input,
/// using the same selector concretization as graph endpoint resolution.
///
/// Unresolved names are not indexed, preserving the caller's fail-closed check.
#[must_use]
pub fn endpoints(parsed: &ParsedMmakefile, context: &TargetContext) -> BTreeSet<String> {
    let mut endpoints = BTreeSet::new();

    for target in &parsed.targets {
        add_target_endpoints(target, &mut endpoints);
    }
    for target in &parsed.source_inventory_targets {
        add_inventory_endpoints(target, &mut endpoints);
    }

    // These are the names that selection_edges adds directly for
    // include_meta=false. Meta rules are included as named aggregates because
    // a native closure can reach a source-declared aggregate even when its
    // dependencies are not producer declarations in this file.
    for rule in parsed.meta_rules.iter().chain(&parsed.explicit_meta_rules) {
        endpoints.insert(rule.name.clone());
    }
    endpoints.extend(parsed.make_meta_providers.iter().cloned());

    // Parser diagnostics may make a named owner available to graph traversal.
    // This is input attribution for the diagnostic, not a claim that the
    // owner has a concrete native producer.
    endpoints.extend(
        parsed
            .native_graph_errors
            .iter()
            .chain(&parsed.capability_errors)
            .filter_map(|diagnostic| {
                diagnostic
                    .context
                    .as_ref()
                    .and_then(|context| context.target.clone())
            }),
    );

    endpoints.extend(parsed.fetches.iter().map(|decl| decl.name.clone()));
    endpoints.extend(
        parsed
            .directory_setups
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(
        parsed
            .genmodule_header_rules
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(
        parsed
            .genmodule_writefiles_rules
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(
        parsed
            .host_file_generators
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(
        parsed
            .sfd_header_rules
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(parsed.sdk_text_rules.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(
        parsed
            .source_text_rules
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(
        parsed
            .source_value_rules
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(parsed.sdk_file_copies.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(parsed.sdk_asset_rules.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(
        parsed
            .sdk_object_groups
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(
        parsed
            .literal_object_groups
            .iter()
            .map(|decl| decl.owner.clone()),
    );

    // The archive interface is formed by the source-archive binder only after
    // complete member coverage is proved. Indexing its exact deterministic
    // alias here preserves the parser input that declared the archive without
    // pretending the partial projection is itself an available producer.
    for archive in &parsed.source_archive_projections {
        endpoints.insert(archive.owner.clone());
        endpoints.insert(format!("{}-archive", archive.owner));
    }
    for (_, owner) in parsed.source_archive_commands.keys() {
        endpoints.insert(owner.clone());
    }
    for group in &parsed.source_compile_projections {
        endpoints.insert(group.owner.clone());
        if let Some(parent) = &group.parent {
            endpoints.insert(parent.clone());
        } else {
            endpoints.insert(crate::graph::private_source_object_owner(group));
        }
    }

    for declaration in &parsed.host_header_rules {
        endpoints.insert(declaration.owner.clone());
        endpoints.insert(declaration.setup_owner.clone());
    }
    endpoints.extend(
        parsed
            .host_header_aggregates
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(parsed.packages.iter().map(|decl| decl.mmake.clone()));
    for declaration in &parsed.external_cmake {
        endpoints.insert(declaration.mmake_name.clone());
        endpoints.insert(declaration.provider_target.clone());
    }
    for declaration in &parsed.configure_builds {
        endpoints.insert(declaration.mmake_name.clone());
        if let Some(provider) = &declaration.provider_target {
            endpoints.insert(provider.clone());
        }
    }
    endpoints.extend(
        parsed
            .grub_builds
            .iter()
            .map(|decl| decl.mmake_name.clone()),
    );
    endpoints.extend(parsed.ahi_builds.iter().map(|decl| decl.mmake_name.clone()));
    endpoints.extend(parsed.python_outputs.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(parsed.flexcat_sources.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(parsed.flexcat_headers.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(parsed.ilbm_sources.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(parsed.catalogs.iter().map(|decl| decl.mmake.clone()));
    endpoints.extend(parsed.copy_includes.iter().map(|decl| decl.name.clone()));
    endpoints.extend(parsed.copy_directories.iter().map(|decl| decl.name.clone()));
    endpoints.extend(parsed.bison_outputs.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(parsed.icon_targets.iter().map(|decl| decl.mmake.clone()));
    endpoints.extend(parsed.icons.iter().map(|decl| decl.mmake.clone()));
    endpoints.extend(parsed.define_headers.iter().map(|decl| decl.owner.clone()));
    endpoints.extend(
        parsed
            .header_transforms
            .iter()
            .map(|decl| decl.name.clone()),
    );

    // A successful tree-wide HIDD-stub join synthesizes this target from all
    // source-local declarations. Every declaring parser input is a real input
    // origin for that one aggregate producer.
    if !parsed.hidd_stubs.is_empty() {
        endpoints.insert("linklibs-hiddstubs".to_owned());
    }

    // Parser-side source projections become native graph producers only after
    // later binding. Preserve their declared owner names; private source
    // directory aliases above use the exact graph identity helper, while any
    // other unindexed post-binding alias remains fail-closed at the caller.
    endpoints.extend(
        parsed
            .layered_header_projections
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    endpoints.extend(
        parsed
            .source_header_pipelines
            .iter()
            .map(|decl| decl.owner.clone()),
    );
    for ((file, owner), declaration) in &parsed.source_directory_groups {
        endpoints.insert(declaration.owner.clone());
        endpoints.insert(crate::graph::private_source_directory_owner(file, owner));
    }

    // `%rule_link_binary` creates its own named object producer. Its consumer
    // is a prerequisite endpoint, not an origin of that producer.
    endpoints.extend(
        parsed
            .binary_objects
            .iter()
            .filter(|decl| decl.applies_to(context))
            .map(|decl| decl.name.clone()),
    );

    endpoints
        .into_iter()
        .filter_map(|endpoint| crate::graph::native_endpoint(&endpoint, context).ok())
        .collect()
}

/// Parser inputs directly associated with each exact native script-output
/// identity. Consumers are intentionally absent: they do not establish the
/// parser origin of the generated producer.
pub type ScriptInputIdentity = (String, Vec<String>);

/// Returns each parsed script path and its ordered declared output set.
#[must_use]
pub fn script_input_identities(parsed: &ParsedMmakefile) -> BTreeSet<ScriptInputIdentity> {
    parsed
        .script_outputs
        .iter()
        .map(|declaration| {
            let mut outputs = Vec::with_capacity(1 + declaration.additional_outputs.len());
            outputs.push(declaration.output.clone());
            outputs.extend(declaration.additional_outputs.iter().cloned());
            (declaration.script.clone(), outputs)
        })
        .collect()
}

fn add_target_endpoints(target: &TargetDefinition, endpoints: &mut BTreeSet<String>) {
    let owner = &target.mmake_name;
    if target.module_type == ModuleType::ModuleHeaders {
        endpoints.insert(format!("{owner}-includes"));
        return;
    }

    endpoints.insert(owner.clone());
    if target.genmodule_abi {
        endpoints.insert(format!("{owner}-includes"));
    }
    if emits_client_linklib(
        &target.module_type,
        target.genmodule_only,
        target.genmodule_linklibs.as_ref(),
    ) {
        endpoints.insert(format!("{owner}-includes"));
        endpoints.insert(format!("{owner}-linklib"));
        if !target.genmodule_only
            && target
                .genmodule_linklibs
                .as_ref()
                .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
        {
            endpoints.insert(format!("{owner}-linklib-rel"));
        }
    }
    if target.module_type == ModuleType::ProgramGroup {
        for source in target
            .source_files
            .iter()
            .chain(&target.cxx_source_files)
            .chain(&target.objc_source_files)
            .chain(&target.asm_source_files)
        {
            if let Some(stem) = Path::new(source).file_stem() {
                endpoints.insert(format!("{owner}-{}", stem.to_string_lossy()));
            }
        }
    }
}

fn add_inventory_endpoints(target: &InventoryTargetIdentity, endpoints: &mut BTreeSet<String>) {
    let owner = &target.mmake_name;
    endpoints.insert(owner.clone());
    if target.genmodule_abi {
        endpoints.insert(format!("{owner}-includes"));
    }
    if emits_client_linklib(
        &target.module_type,
        target.genmodule_only,
        target.genmodule_linklibs.as_ref(),
    ) {
        endpoints.insert(format!("{owner}-includes"));
        endpoints.insert(format!("{owner}-linklib"));
        if !target.genmodule_only
            && target
                .genmodule_linklibs
                .as_ref()
                .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
        {
            endpoints.insert(format!("{owner}-linklib-rel"));
        }
    }
}

fn emits_client_linklib(
    module_type: &ModuleType,
    genmodule_only: bool,
    linklibs: Option<&crate::ast::GenmoduleLinklibs>,
) -> bool {
    *module_type == ModuleType::Abi
        || *module_type == ModuleType::Library
            && (genmodule_only || linklibs.is_some_and(|metadata| metadata.enabled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::MetaTargetRule;
    use crate::copy_includes::ScriptOutputDecl;
    use crate::source_archive_rules::{ArchiveMembers, SourceArchiveDecl};
    use crate::source_compile_rules::SourceCompileGroupDecl;
    use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticStage};

    #[test]
    fn indexes_source_owners_and_bound_archive_alias_without_dependencies() {
        let mut parsed = ParsedMmakefile::default();
        parsed.meta_rules.push(MetaTargetRule {
            name: "local-aggregate".into(),
            dependencies: vec!["foreign-producer".into()],
        });
        parsed.source_archive_projections.push(SourceArchiveDecl {
            owner: "local-archive".into(),
            file: "compiler/example/mmakefile.src".into(),
            line: 10,
            owner_line: 8,
            output: "${AROS_BUILD_DIR}/libexample.a".into(),
            members: ArchiveMembers::Exact(Vec::new()),
        });
        parsed
            .source_compile_projections
            .push(SourceCompileGroupDecl {
                owner: "local-objects".into(),
                parent: Some("local-aggregate".into()),
                file: "compiler/example/mmakefile.src".into(),
                line: 12,
                archive_output: None,
                objects: Vec::new(),
            });

        let indexed = endpoints(&parsed, &TargetContext::default());
        assert!(indexed.contains("local-aggregate"));
        assert!(indexed.contains("local-archive"));
        assert!(indexed.contains("local-archive-archive"));
        assert!(indexed.contains("local-objects"));
        assert!(!indexed.contains("foreign-producer"));
    }

    #[test]
    fn indexes_named_parser_diagnostic_contexts_as_input_provenance() {
        let mut parsed = ParsedMmakefile::default();
        let named = |target: &str| {
            Diagnostic::error(
                DiagnosticCode::CapabilityDrift,
                DiagnosticStage::CapabilityValidation,
                "fixture diagnostic",
            )
            .with_context(DiagnosticContext {
                target: Some(target.to_owned()),
                ..Default::default()
            })
        };
        parsed.native_graph_errors.push(named("native-error-owner"));
        parsed
            .capability_errors
            .push(named("capability-error-owner"));

        let indexed = endpoints(&parsed, &TargetContext::default());
        assert!(indexed.contains("native-error-owner"));
        assert!(indexed.contains("capability-error-owner"));
    }

    #[test]
    fn script_identity_keeps_declared_outputs_in_order_and_omits_consumers() {
        let mut parsed = ParsedMmakefile::default();
        parsed.script_outputs.push(ScriptOutputDecl {
            directory: "compiler/example".into(),
            output: "${AROS_BUILD_DIR}/gen/generated.c".into(),
            additional_outputs: vec!["${AROS_BUILD_DIR}/gen/generated.h".into()],
            script: "${AROS_SOURCE_DIR}/compiler/example/generate.py".into(),
            arguments: Vec::new(),
            stdout: false,
            working_directory: None,
            depends: vec!["${AROS_SOURCE_DIR}/input.xml".into()],
            consumer_source_stems: vec!["generated".into()],
            consumer_targets: vec!["consumer-target".into()],
        });

        assert_eq!(
            script_input_identities(&parsed),
            BTreeSet::from([(
                "${AROS_SOURCE_DIR}/compiler/example/generate.py".into(),
                vec![
                    "${AROS_BUILD_DIR}/gen/generated.c".into(),
                    "${AROS_BUILD_DIR}/gen/generated.h".into(),
                ],
            )])
        );
    }
}
