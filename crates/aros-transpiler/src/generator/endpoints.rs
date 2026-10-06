use crate::ast::ModuleType;
use crate::graph::DependencyGraph;
use std::collections::HashSet;

/// Collects every name the generated file declares as a CMake target or
/// as a dependency endpoint, so `#MM` edges can be filtered against it.
pub(super) fn collect_endpoint_names(graph: &DependencyGraph) -> HashSet<String> {
    let mut all_targets: HashSet<String> =
        graph
            .targets
            .iter()
            .filter(|(_, target)| target.module_type != ModuleType::ModuleHeaders)
            .map(|(name, _)| name)
            .chain(graph.icon_targets.keys())
            .cloned()
            .chain(graph.catalogs.iter().map(|catalog| catalog.mmake.clone()))
            .chain(
                graph
                    .sdk_text_rules
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(graph.host_header_rules.iter().flat_map(|declaration| {
                [declaration.owner.clone(), declaration.setup_owner.clone()]
            }))
            .chain(
                graph
                    .host_file_generators
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .source_text_rules
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(graph.sdk_file_copies.iter().map(|rule| rule.owner.clone()))
            .chain(graph.sdk_asset_rules.iter().map(|rule| rule.owner.clone()))
            .chain(
                graph
                    .source_value_rules
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .literal_object_groups
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph.source_archives.iter().flat_map(|archive| {
                    [archive.declaration.owner.clone(), archive.provider_target()]
                }),
            )
            .chain(
                graph
                    .sdk_object_groups
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .host_header_aggregates
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .genmodule_header_rules
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .directory_setups
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .genmodule_writefiles_rules
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .flexcat_sources
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .flexcat_headers
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .ilbm_sources
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .header_transforms
                    .iter()
                    .map(|transform| transform.name.clone()),
            )
            .chain(
                graph
                    .define_headers
                    .iter()
                    .map(|header| header.owner.clone()),
            )
            .chain(
                graph
                    .copy_directories
                    .iter()
                    .map(|declaration| declaration.name.clone()),
            )
            .chain(
                graph
                    .python_outputs
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .script_outputs
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(graph.fetches.iter().map(|fetch| fetch.name.clone()))
            .chain(
                graph
                    .external_cmake
                    .iter()
                    .map(|declaration| declaration.mmake_name.clone()),
            )
            .chain(
                graph
                    .external_cmake
                    .iter()
                    .map(|declaration| declaration.provider_target.clone()),
            )
            .chain(
                graph
                    .configure_builds
                    .iter()
                    .map(|declaration| declaration.mmake_name.clone()),
            )
            .chain(
                graph
                    .configure_builds
                    .iter()
                    .filter_map(|declaration| declaration.provider_target.clone()),
            )
            .chain(
                graph
                    .grub_builds
                    .iter()
                    .map(|declaration| declaration.mmake_name.clone()),
            )
            .chain(
                graph
                    .ahi_builds
                    .iter()
                    .map(|declaration| declaration.mmake_name.clone()),
            )
            .chain(
                graph
                    .arch_endpoint_effects
                    .iter()
                    .map(|effect| effect.endpoint.clone()),
            )
            .collect();
    for header in &graph.assembly_headers {
        all_targets.insert(header.owner.clone());
        all_targets.insert(header.aggregate_owner.clone());
    }

    // The closed GRUB2 helper creates one shared source-fetch endpoint and
    // exposes the legacy alias itself.  Keep both names in the endpoint
    // registry so #MM edges retain their original ordering rather than being
    // silently filtered as unknown meta dependencies.
    if !graph.grub_builds.is_empty() {
        all_targets.insert("grub2-aros--fetch".to_owned());
        all_targets.insert("grub2-aros-fetch".to_owned());
    }

    // AHI invokes the explicitly materialised host `sfdc` tool through its
    // closed helper.  It has no legacy #MM declaration of its own, but keeping
    // the endpoint visible prevents an explicit future meta edge from being
    // silently discarded during generated-target filtering.
    if !graph.ahi_builds.is_empty() {
        all_targets.insert("host-sfdc".to_owned());
    }

    // Full genmodule and ABI declarations create product targets inside the
    // CMake helper rather than as independent AST declarations. They are still
    // real dependency endpoints: a raw `-lstdc`, for example, must order its
    // consumer after `compiler-stdc-linklib`. Keep these generated products in
    // the endpoint registry so the meta-edge filter cannot discard them.
    for (mmake, target) in &graph.targets {
        match target.module_type {
            ModuleType::Abi => {
                all_targets.insert(format!("{mmake}-linklib"));
            }
            ModuleType::Library
                if target.genmodule_only
                    || target
                        .genmodule_linklibs
                        .as_ref()
                        .is_some_and(|metadata| metadata.enabled) =>
            {
                all_targets.insert(format!("{mmake}-linklib"));
                if !target.genmodule_only
                    && target
                        .genmodule_linklibs
                        .as_ref()
                        .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
                {
                    all_targets.insert(format!("{mmake}-linklib-rel"));
                }
            }
            _ => {}
        }
    }
    all_targets
}
