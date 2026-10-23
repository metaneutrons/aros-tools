use super::full_inventory;
use crate::ast::{GenmoduleLinklibs, ModuleMacroForm, ModuleType, TargetDefinition};
use crate::genmodule_header_rules::GenmoduleType;
use crate::genmodule_writefiles_rules::GenmoduleWritefilesRuleDecl;
use crate::{DependencyGraph, TargetContext};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn x86_64() -> TargetContext {
    TargetContext {
        cpu: Some("x86_64".into()),
        platform: Some("pc".into()),
        toolchain: Some("llvm".into()),
        cpu32: Some("i386".into()),
        use_mmu: Some("1".into()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.0.0".into()),
        ..TargetContext::default()
    }
}

fn aarch64() -> TargetContext {
    TargetContext {
        cpu: Some("aarch64".into()),
        platform: Some("raspi".into()),
        toolchain: Some("llvm".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".into()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.0.0".into()),
        ..TargetContext::default()
    }
}

fn target(
    owner: &str,
    directory: &str,
    module: &str,
    module_type: ModuleType,
    module_macro: ModuleMacroForm,
) -> TargetDefinition {
    TargetDefinition {
        mmake_name: owner.to_owned(),
        target_name: module.to_owned(),
        module_type,
        module_macro: Some(module_macro),
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
        dir_path: PathBuf::from(directory),
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
        selection_headers_only: false,
    }
}

fn mesa_gl() -> TargetDefinition {
    let mut gl = target(
        "mesa3dgl-library",
        "workbench/libs/mesa",
        "mesa3dgl26-0",
        ModuleType::Library,
        ModuleMacroForm::Full,
    );
    gl.config_file = Some("${AROS_SOURCE_DIR}/workbench/libs/gl/gl.conf".into());
    gl.config_override_file = Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa3dgl.conf".into());
    gl.genmodule_linklibs = Some(GenmoduleLinklibs {
        enabled: true,
        has_relative: true,
        ..Default::default()
    });
    gl
}

fn marker_consumer() -> TargetDefinition {
    let mut consumer = target(
        "workbench-libs-gl-linklib",
        "workbench/libs/gl",
        "gl",
        ModuleType::LinkLib,
        ModuleMacroForm::Simple,
    );
    consumer.source_files.push(
        "@AROS_GENMODULE|normal|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|gl.conf"
            .into(),
    );
    consumer
}

fn writefiles_rule() -> GenmoduleWritefilesRuleDecl {
    GenmoduleWritefilesRuleDecl {
        owner: "workbench-libs-gl-gen-stubs".into(),
        file: "workbench/libs/gl/mmakefile.src".into(),
        line: 1,
        declaring_dir: "workbench/libs/gl".into(),
        config: "workbench/libs/gl/gl.conf".into(),
        module: "gl".into(),
        modtype: GenmoduleType::Library,
    }
}

#[test]
fn full_scope_includes_runtime_and_declared_clients_without_native_roots() {
    let mut graph = DependencyGraph::default();
    graph.targets.insert("mesa3dgl-library".into(), mesa_gl());
    let marker = marker_consumer();
    graph.targets.insert(marker.mmake_name.clone(), marker);
    graph.genmodule_writefiles_rules.push(writefiles_rule());
    assert!(graph.native_selected_runtime_modules.is_none());
    assert!(graph.native_selected_client_archives.is_none());
    let owners_before: BTreeSet<_> = graph.targets.keys().cloned().collect();

    let result = full_inventory(&graph, &x86_64()).unwrap();
    let mesa_records: Vec<_> = result
        .records
        .iter()
        .filter(|record| record.owner == "mesa3dgl-library")
        .collect();
    assert_eq!(mesa_records.len(), 3);
    assert!(mesa_records
        .iter()
        .any(|record| record.consumer_path == "runtime/mesa3dgl-library"));
    assert!(mesa_records
        .iter()
        .any(|record| record.consumer_path == "module-client/mesa3dgl-library/normal"));
    assert!(mesa_records
        .iter()
        .any(|record| record.consumer_path == "module-client/mesa3dgl-library/rel"));
    let marker = result
        .records
        .iter()
        .find(|record| record.consumer_path == "marker/workbench-libs-gl-linklib/normal")
        .unwrap();
    assert_eq!(marker.owner, "workbench-libs-gl-gen-stubs");
    assert_eq!(marker.base_configuration, "workbench/libs/gl/gl.conf");
    assert_eq!(
        graph.targets.keys().cloned().collect::<BTreeSet<_>>(),
        owners_before
    );
    assert!(graph.native_selected_runtime_modules.is_none());
    assert!(graph.native_selected_client_archives.is_none());
}

#[test]
fn abi_only_is_client_only_and_runtime_only_retains_declared_client_lanes() {
    let mut abi = target(
        "mesa3dgl-library",
        "workbench/libs/mesa",
        "mesa3dgl26-0",
        ModuleType::Abi,
        ModuleMacroForm::AbiOnly,
    );
    abi.declared_mod_type = Some("library".into());
    abi.config_file = Some("${AROS_SOURCE_DIR}/workbench/libs/gl/gl.conf".into());
    let mut abi_graph = DependencyGraph::default();
    abi_graph.targets.insert(abi.mmake_name.clone(), abi);
    let abi_records = full_inventory(&abi_graph, &x86_64()).unwrap().records;
    assert_eq!(abi_records.len(), 1);
    assert_eq!(
        abi_records[0].consumer_path,
        "module-client/mesa3dgl-library/normal"
    );

    let mut runtime_only = mesa_gl();
    runtime_only.module_macro = Some(ModuleMacroForm::RuntimeOnly);
    let mut runtime_graph = DependencyGraph::default();
    runtime_graph
        .targets
        .insert(runtime_only.mmake_name.clone(), runtime_only);
    let runtime_records = full_inventory(&runtime_graph, &x86_64()).unwrap().records;
    assert_eq!(runtime_records.len(), 3);
    assert!(runtime_records
        .iter()
        .any(|row| row.consumer_path == "runtime/mesa3dgl-library"));
    assert!(runtime_records
        .iter()
        .any(|row| row.consumer_path == "module-client/mesa3dgl-library/normal"));
    assert!(runtime_records
        .iter()
        .any(|row| row.consumer_path == "module-client/mesa3dgl-library/rel"));
}

#[test]
fn custom_and_headers_only_declarations_do_not_become_runtime_records() {
    let mut custom = mesa_gl();
    custom.module_type = ModuleType::Custom;
    custom.declared_mod_type = Some("library".into());
    let mut graph = DependencyGraph::default();
    graph.targets.insert(custom.mmake_name.clone(), custom);
    assert!(full_inventory(&graph, &x86_64())
        .unwrap_err()
        .contains("unsupported module type"));

    let mut headers = mesa_gl();
    headers.selection_headers_only = true;
    let mut graph = DependencyGraph::default();
    graph.targets.insert(headers.mmake_name.clone(), headers);
    assert!(full_inventory(&graph, &x86_64())
        .unwrap()
        .records
        .is_empty());
}

#[test]
fn full_and_contract_root_scopes_remain_distinct_and_profile_specific() {
    let mut graph = DependencyGraph::default();
    graph
        .native_selected_runtime_modules
        .replace(BTreeSet::new());
    graph
        .native_selected_client_archives
        .replace(BTreeSet::new());
    graph.targets.insert("mesa3dgl-library".into(), mesa_gl());
    assert!(
        super::super::configured_genmodule::inventory(&graph, &x86_64())
            .unwrap()
            .records
            .is_empty()
    );
    assert_eq!(full_inventory(&graph, &x86_64()).unwrap().records.len(), 3);

    let mut v3d = target(
        "hidd-v3d",
        "arch/arm-native/soc/broadcom/2708/hidd/v3d",
        "v3d",
        ModuleType::Hidd,
        ModuleMacroForm::Full,
    );
    v3d.declared_mod_type = Some("hidd".into());
    let mut v3d_graph = DependencyGraph::default();
    v3d_graph.targets.insert(v3d.mmake_name.clone(), v3d);
    assert!(full_inventory(&v3d_graph, &x86_64())
        .unwrap()
        .records
        .is_empty());
    let aarch64_records = full_inventory(&v3d_graph, &aarch64()).unwrap().records;
    assert_eq!(aarch64_records.len(), 1);
    assert_eq!(aarch64_records[0].module_type, "hidd");
}

#[test]
fn duplicate_pending_owner_is_rejected() {
    let pending = crate::ast::InventoryTargetIdentity::from(&mesa_gl());
    let mut graph = DependencyGraph::default();
    graph.inventory_targets.extend([pending.clone(), pending]);
    assert!(full_inventory(&graph, &x86_64())
        .unwrap_err()
        .contains("duplicate pending genmodule declaration"));
}

#[test]
fn missing_or_incompatible_runtime_and_abi_forms_are_rejected() {
    for form in [
        None,
        Some(ModuleMacroForm::Simple),
        Some(ModuleMacroForm::AbiOnly),
    ] {
        let mut target = mesa_gl();
        target.module_macro = form;
        let mut pending_graph = DependencyGraph::default();
        pending_graph
            .inventory_targets
            .push(crate::ast::InventoryTargetIdentity::from(&target));
        assert!(full_inventory(&pending_graph, &x86_64())
            .unwrap_err()
            .contains("full-target runtime has no supported module declaration"));
        assert!(pending_graph.targets.is_empty());
        let mut graph = DependencyGraph::default();
        graph.targets.insert(target.mmake_name.clone(), target);
        assert!(full_inventory(&graph, &x86_64())
            .unwrap_err()
            .contains("full-target runtime has no supported module declaration"));
    }
    let mut target = mesa_gl();
    target.module_type = ModuleType::Abi;
    target.declared_mod_type = Some("library".into());
    let mut graph = DependencyGraph::default();
    graph.targets.insert(target.mmake_name.clone(), target);
    assert!(full_inventory(&graph, &x86_64())
        .unwrap_err()
        .contains("full-target ABI client has no supported module declaration"));
}

#[test]
fn concrete_pending_overlap_and_missing_client_metadata_are_rejected() {
    let mut graph = DependencyGraph::default();
    graph
        .inventory_targets
        .push(crate::ast::InventoryTargetIdentity::from(&mesa_gl()));
    graph.targets.insert("mesa3dgl-library".into(), mesa_gl());
    assert!(full_inventory(&graph, &x86_64())
        .unwrap_err()
        .contains("declarations overlap"));

    let mut target = mesa_gl();
    target.genmodule_linklibs = None;
    target.linklib_name = Some("mesa3dgl".into());
    let mut graph = DependencyGraph::default();
    graph.targets.insert(target.mmake_name.clone(), target);
    assert!(full_inventory(&graph, &x86_64())
        .unwrap_err()
        .contains("lacks parsed configuration metadata"));
    graph
        .targets
        .get_mut("mesa3dgl-library")
        .unwrap()
        .genmodule_linklibs = Some(GenmoduleLinklibs {
        enabled: false,
        has_relative: true,
        ..GenmoduleLinklibs::default()
    });
    assert_eq!(full_inventory(&graph, &x86_64()).unwrap().records.len(), 3);
}
