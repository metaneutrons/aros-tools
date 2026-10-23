use crate::arch_endpoint_effects::{ArchEndpointEffect, ArchEndpointEffectData};
use crate::arch_sources::ArchSourceDecl;
use crate::ast::{GenmoduleLinklibs, InventoryTargetIdentity, ModuleMacroForm, ModuleType};
use crate::{DependencyGraph, TargetContext};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn pending_identity(name: &str, module_type: ModuleType) -> InventoryTargetIdentity {
    InventoryTargetIdentity {
        mmake_name: name.into(),
        target_name: name.into(),
        module_type,
        dir_path: PathBuf::from(format!("workbench/libs/{name}")),
        variant_32bit: false,
        declared_mod_type: None,
        mod_suffix: None,
        linklib_output_dir: None,
        genmodule_abi: false,
        genmodule_only: false,
        module_macro: None,
        target_dir: None,
        linklib_name: None,
        config_file: None,
        config_override_file: None,
        genmodule_linklibs: None,
        config_relative_libraries: Vec::new(),
        canonical_linklib_output: false,
        canonical_linklib_eligible: false,
        empty_archive: false,
        dependencies: Vec::new(),
        link_libs: Vec::new(),
        use_libs: Vec::new(),
        link_options: Vec::new(),
        spec_switches: Vec::new(),
    }
}

#[test]
fn selected_pending_mesa_configuration_reaches_inventory_without_compile_target() {
    let mut identity = pending_identity("mesa3dgl-library", ModuleType::Library);
    identity.target_name = "mesa3dgl26-0".into();
    identity.dir_path = "workbench/libs/mesa".into();
    identity.module_macro = Some(ModuleMacroForm::Full);
    identity.config_file = Some("${AROS_SOURCE_DIR}/workbench/libs/gl/gl.conf".into());
    identity.config_override_file =
        Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa3dgl.conf".into());
    let mut graph = DependencyGraph::default();
    graph.inventory_targets.push(identity);
    let context = TargetContext {
        cpu: Some("arm".into()),
        platform: Some("raspi".into()),
        toolchain: Some("llvm".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".into()),
        float_abi: Some("hard".into()),
        mesa_version: Some("26.0.0".into()),
        ..TargetContext::default()
    };
    graph
        .retain_native_genmodule_inventory(&BTreeSet::from(["mesa3dgl-library".into()]), &context)
        .unwrap();
    let result =
        crate::capability::mesa::configured_genmodule::inventory(&graph, &context).unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].consumer_path, "runtime/mesa3dgl-library");
    assert_eq!(
        result.records[0].base_configuration,
        "workbench/libs/gl/gl.conf"
    );
    assert_eq!(
        result.records[0].override_configuration.as_deref(),
        Some("workbench/libs/mesa/mesa3dgl.conf")
    );
    assert!(graph.targets.is_empty());
    // The same declaration cannot acquire a runtime lane from an unrelated root.
    let mut unselected = graph;
    unselected
        .retain_native_genmodule_inventory(&BTreeSet::new(), &context)
        .unwrap();
    assert!(
        crate::capability::mesa::configured_genmodule::inventory(&unselected, &context)
            .unwrap()
            .records
            .is_empty()
    );
}

#[test]
fn selected_pending_runtime_and_client_preserve_configs_without_target_promotion() {
    let mut runtime = pending_identity("pending-runtime", ModuleType::Library);
    runtime.config_file = Some("workbench/libs/pending-runtime/module.conf".into());
    runtime.config_override_file = Some("workbench/libs/pending-runtime/override.conf".into());
    let client = pending_identity("pending-client", ModuleType::Library);
    let unselected = pending_identity("unselected-pending", ModuleType::Library);

    let mut graph = DependencyGraph {
        inventory_targets: vec![runtime, client, unselected],
        ..DependencyGraph::default()
    };
    let selected = BTreeSet::from([
        "pending-runtime".to_owned(),
        "pending-client-linklib".to_owned(),
    ]);

    graph
        .retain_native_genmodule_inventory(&selected, &TargetContext::default())
        .unwrap();

    assert_eq!(graph.targets.len(), 0, "pending identities are not targets");
    assert_eq!(
        graph
            .inventory_targets
            .iter()
            .map(|identity| identity.mmake_name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["pending-client", "pending-runtime"])
    );
    let retained_runtime = graph
        .inventory_targets
        .iter()
        .find(|identity| identity.mmake_name == "pending-runtime")
        .unwrap();
    assert_eq!(
        retained_runtime.config_file.as_deref(),
        Some("workbench/libs/pending-runtime/module.conf")
    );
    assert_eq!(
        retained_runtime.config_override_file.as_deref(),
        Some("workbench/libs/pending-runtime/override.conf")
    );
    assert_eq!(
        graph.native_selected_runtime_modules.as_ref().unwrap(),
        &BTreeSet::from(["pending-runtime".to_owned()])
    );
    assert_eq!(
        graph.native_selected_client_archives.as_ref().unwrap(),
        &BTreeSet::from(["pending-client-linklib".to_owned()])
    );
}

#[test]
fn selected_architecture_pending_endpoints_keep_runtime_and_client_lanes_separate() {
    let runtime = pending_identity("arch-runtime", ModuleType::Library);
    let mut client = pending_identity("arch-client", ModuleType::Library);
    client.genmodule_linklibs = Some(GenmoduleLinklibs {
        enabled: true,
        has_relative: true,
        ..GenmoduleLinklibs::default()
    });

    let mut graph = DependencyGraph {
        inventory_targets: vec![runtime, client],
        arch_endpoint_effects: vec![
            ArchEndpointEffect {
                recipe: "arch/x86_64/kernel/mmakefile.src".into(),
                line: 1,
                endpoint: "arch-runtime-arch-objects".into(),
                dependencies: Vec::new(),
                data: ArchEndpointEffectData::ArchModuleObjects {
                    mainmmake: "arch-runtime".into(),
                    tag: "x86_64".into(),
                    module_sources: vec!["runtime.S".into()],
                    linklib_sources: Vec::new(),
                    directory: "arch/x86_64/kernel".into(),
                },
            },
            ArchEndpointEffect {
                recipe: "arch/x86_64/libs/mmakefile.src".into(),
                line: 2,
                endpoint: "arch-client-arch-linklib".into(),
                dependencies: Vec::new(),
                data: ArchEndpointEffectData::ArchClientLinklibSources {
                    mainmmake: "arch-client".into(),
                    tag: "x86_64".into(),
                    module_sources: vec!["module.S".into()],
                    linklib_sources: vec!["client.S".into()],
                    directory: "arch/x86_64/libs".into(),
                    compiler: "target".into(),
                },
            },
        ],
        ..DependencyGraph::default()
    };
    graph.arch_sources.insert(
        "arch-client".into(),
        vec![ArchSourceDecl {
            mainmmake: "arch-client".into(),
            maindir: None,
            modname: None,
            tag: "x86_64".into(),
            dir: "arch/x86_64/libs".into(),
            files: vec!["module.S".into()],
            linklib_files: vec!["client.S".into()],
            extra_quote_include: None,
            include_dirs: Vec::new(),
            defines: Vec::new(),
            compile_options: Vec::new(),
            line: 2,
        }],
    );
    let context = TargetContext {
        cpu: Some("x86_64".into()),
        platform: Some("pc".into()),
        ..TargetContext::default()
    };
    let selected = BTreeSet::from([
        "arch-runtime-arch-objects".to_owned(),
        "arch-client-arch-linklib".to_owned(),
    ]);

    graph
        .retain_native_genmodule_inventory(&selected, &context)
        .unwrap();

    assert_eq!(graph.targets.len(), 0, "pending identities are not targets");
    assert_eq!(
        graph.native_selected_runtime_modules.as_ref().unwrap(),
        &BTreeSet::from(["arch-runtime".to_owned()])
    );
    assert_eq!(
        graph.native_selected_client_archives.as_ref().unwrap(),
        &BTreeSet::from([
            "arch-client-linklib".to_owned(),
            "arch-client-linklib-rel".to_owned(),
        ])
    );
    assert_eq!(graph.inventory_targets.len(), 2);
}
