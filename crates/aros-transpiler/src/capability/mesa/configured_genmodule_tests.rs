use super::inventory;
use crate::ast::{
    GenmoduleLinklibs, InventoryTargetIdentity, ModuleMacroForm, ModuleType, TargetDefinition,
};
use crate::genmodule_header_rules::GenmoduleType;
use crate::genmodule_writefiles_rules::GenmoduleWritefilesRuleDecl;
use crate::{DependencyGraph, TargetContext};
use std::collections::BTreeSet;
use std::path::PathBuf;

const GL_NORMAL: &str =
    "@AROS_GENMODULE|normal|stackstubs,regcallstubs,autoinit,getlibbase|gl|library|gl.conf";
const GL_REL: &str = "@AROS_GENMODULE|rel|stackstubs,regcallstubs|gl|library|gl.conf";

fn mesa_x86_64() -> TargetContext {
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

fn target(name: &str, directory: &str, module: &str, module_type: ModuleType) -> TargetDefinition {
    TargetDefinition {
        mmake_name: name.to_owned(),
        target_name: module.to_owned(),
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

fn runtime(name: &str, directory: &str, module: &str, module_type: ModuleType) -> TargetDefinition {
    let mut result = target(name, directory, module, module_type);
    result.module_macro = Some(ModuleMacroForm::Full);
    result
}

fn writefiles_rule(owner: &str, directory: &str, module: &str) -> GenmoduleWritefilesRuleDecl {
    GenmoduleWritefilesRuleDecl {
        owner: owner.to_owned(),
        file: format!("{directory}/mmakefile.src"),
        line: 1,
        declaring_dir: directory.to_owned(),
        config: format!("{directory}/{module}.conf"),
        module: module.to_owned(),
        modtype: GenmoduleType::Library,
    }
}

fn marker_consumer(name: &str, marker: &str) -> TargetDefinition {
    let mut result = target(name, "workbench/libs/gl", name, ModuleType::LinkLib);
    result.source_files.push(marker.to_owned());
    result
}

fn fixture_graph() -> DependencyGraph {
    let mut graph = DependencyGraph::default();

    let mut mesa_gl = runtime(
        "mesa3dgl-library",
        "workbench/libs/mesa",
        "mesa3dgl26-0",
        ModuleType::Library,
    );
    mesa_gl.config_file = Some("${AROS_SOURCE_DIR}/workbench/libs/gl/gl.conf".into());
    mesa_gl.config_override_file =
        Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa3dgl.conf".into());
    graph.targets.insert(mesa_gl.mmake_name.clone(), mesa_gl);

    let gallium_library = runtime(
        "workbench-libs-gallium",
        "workbench/libs/gallium",
        "gallium",
        ModuleType::Library,
    );
    graph
        .targets
        .insert(gallium_library.mmake_name.clone(), gallium_library);
    let gallium_hidd = runtime(
        "hidd-gallium",
        "workbench/hidds/gallium",
        "gallium",
        ModuleType::Hidd,
    );
    graph
        .targets
        .insert(gallium_hidd.mmake_name.clone(), gallium_hidd);

    let gl_normal = marker_consumer("workbench-libs-gl-linklib", GL_NORMAL);
    graph
        .targets
        .insert(gl_normal.mmake_name.clone(), gl_normal);
    let gl_rel = marker_consumer("workbench-libs-gl-linklib-rel", GL_REL);
    graph.targets.insert(gl_rel.mmake_name.clone(), gl_rel);
    graph.genmodule_writefiles_rules.push(writefiles_rule(
        "workbench-libs-gl-gen-stubs",
        "workbench/libs/gl",
        "gl",
    ));

    let unrelated = runtime(
        "ordinary-library",
        "workbench/libs/ordinary",
        "ordinary",
        ModuleType::Library,
    );
    graph
        .targets
        .insert(unrelated.mmake_name.clone(), unrelated);

    graph.native_selected_runtime_modules = Some(BTreeSet::from([
        "mesa3dgl-library".to_owned(),
        "workbench-libs-gallium".to_owned(),
        "hidd-gallium".to_owned(),
    ]));
    // Standalone LinkLib targets survive native graph retention directly.
    // This set is reserved for a module builder's synthetic archive endpoints.
    graph.native_selected_client_archives = Some(BTreeSet::new());
    graph
}

#[test]
fn projects_runtime_configs_and_generated_client_lanes() {
    let result = inventory(&fixture_graph(), &mesa_x86_64()).unwrap();
    assert_eq!(result.schema, "aros-mesa-genmodule-inventory-v1");
    assert_eq!(result.records.len(), 5);

    let mesa_gl = result
        .records
        .iter()
        .find(|record| record.owner == "mesa3dgl-library")
        .unwrap();
    assert_eq!(mesa_gl.module, "mesa3dgl26-0");
    assert_eq!(mesa_gl.module_type, "library");
    assert_eq!(mesa_gl.base_configuration, "workbench/libs/gl/gl.conf");
    assert_eq!(
        mesa_gl.override_configuration.as_deref(),
        Some("workbench/libs/mesa/mesa3dgl.conf")
    );
    assert_eq!(mesa_gl.consumer_path, "runtime/mesa3dgl-library");
    assert!(mesa_gl.components.is_empty());

    // The same module spelling is valid for the library and HIDD producers;
    // ownership identity is the declaration tuple, not the module name alone.
    let gallium: Vec<_> = result
        .records
        .iter()
        .filter(|record| record.module == "gallium")
        .collect();
    assert_eq!(gallium.len(), 2);
    assert_eq!(gallium[0].owner, "hidd-gallium");
    assert_eq!(gallium[0].module_type, "hidd");
    assert_eq!(gallium[1].owner, "workbench-libs-gallium");
    assert_eq!(gallium[1].module_type, "library");

    let normal = result
        .records
        .iter()
        .find(|record| record.consumer_path.ends_with("/normal"))
        .unwrap();
    assert_eq!(normal.owner, "workbench-libs-gl-gen-stubs");
    assert_eq!(normal.base_configuration, "workbench/libs/gl/gl.conf");
    assert_eq!(
        normal.components,
        ["stackstubs", "regcallstubs", "autoinit", "getlibbase"]
    );
    let relative = result
        .records
        .iter()
        .find(|record| record.consumer_path.ends_with("/rel"))
        .unwrap();
    assert_eq!(
        relative.consumer_path,
        "marker/workbench-libs-gl-linklib-rel/rel"
    );
    assert_eq!(relative.components, ["stackstubs", "regcallstubs"]);
    assert!(!result
        .records
        .iter()
        .any(|record| record.module == "ordinary"));
}

#[test]
fn a_marker_only_selected_graph_does_not_require_a_runtime_module() {
    let mut graph = fixture_graph();
    graph.native_selected_runtime_modules = Some(BTreeSet::new());
    let result = inventory(&graph, &mesa_x86_64()).unwrap();
    assert_eq!(result.records.len(), 2);
    assert!(result
        .records
        .iter()
        .all(|record| record.consumer_path.starts_with("marker/")));
}

#[test]
fn module_client_only_selection_keeps_its_exact_producer_without_runtime_activation() {
    let mut graph = fixture_graph();
    graph.native_selected_runtime_modules = Some(BTreeSet::new());
    graph.native_selected_client_archives = Some(BTreeSet::from([
        "mesa3dgl-library-linklib".into(),
        "mesa3dgl-library-linklib-rel".into(),
    ]));
    graph
        .targets
        .get_mut("mesa3dgl-library")
        .unwrap()
        .genmodule_linklibs = Some(crate::ast::GenmoduleLinklibs {
        enabled: true,
        has_relative: true,
        ..Default::default()
    });
    let result = inventory(&graph, &mesa_x86_64()).unwrap();
    let module_clients: Vec<_> = result
        .records
        .iter()
        .filter(|record| record.owner == "mesa3dgl-library")
        .collect();
    assert_eq!(module_clients.len(), 2);
    assert!(module_clients
        .iter()
        .all(|record| record.consumer_path.starts_with("module-client/")));
    assert!(module_clients
        .iter()
        .all(|record| record.override_configuration.as_deref()
            == Some("workbench/libs/mesa/mesa3dgl.conf")));
    assert_eq!(
        result.records.len(),
        4,
        "standalone markers remain separate"
    );
    graph
        .targets
        .get_mut("mesa3dgl-library")
        .unwrap()
        .genmodule_linklibs
        .as_mut()
        .unwrap()
        .has_relative = false;
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("relative client lane is not declared"));
}

#[test]
fn unselected_graph_and_missing_runtime_macro_are_not_invented_as_complete_selection() {
    assert!(inventory(&DependencyGraph::default(), &mesa_x86_64())
        .unwrap_err()
        .contains("source-selected native graph"));
    let mut graph = fixture_graph();
    graph
        .targets
        .get_mut("mesa3dgl-library")
        .unwrap()
        .module_macro = None;
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("no supported module declaration"));
}

#[test]
fn marker_selection_prunes_unselected_concrete_consumers_and_requires_exact_producer() {
    let mut graph = fixture_graph();
    graph
        .retain_native_selection(
            &BTreeSet::from([
                "workbench-libs-gl-linklib".into(),
                "workbench-libs-gl-gen-stubs".into(),
            ]),
            &mesa_x86_64(),
        )
        .unwrap();
    let selected = inventory(&graph, &mesa_x86_64()).unwrap();
    assert_eq!(selected.records.len(), 1);
    assert_eq!(
        selected.records[0].consumer_path,
        "marker/workbench-libs-gl-linklib/normal"
    );
    graph.genmodule_writefiles_rules[0].config = "workbench/libs/gl/other.conf".into();
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("maps to 0 writefiles producers"));
}

#[test]
fn module_client_lanes_are_not_broadened_or_added_to_hidd_builders() {
    for (suffix, expected) in [("-linklib", "normal"), ("-linklib-rel", "rel")] {
        let mut graph = fixture_graph();
        graph.native_selected_runtime_modules = Some(BTreeSet::new());
        graph.native_selected_client_archives =
            Some(BTreeSet::from([format!("mesa3dgl-library{suffix}")]));
        graph
            .targets
            .get_mut("mesa3dgl-library")
            .unwrap()
            .genmodule_linklibs = Some(crate::ast::GenmoduleLinklibs {
            enabled: true,
            has_relative: true,
            ..Default::default()
        });
        let result = inventory(&graph, &mesa_x86_64()).unwrap();
        let clients: Vec<_> = result
            .records
            .iter()
            .filter(|record| record.owner == "mesa3dgl-library")
            .collect();
        assert_eq!(clients.len(), 1);
        assert_eq!(
            clients[0].consumer_path,
            format!("module-client/mesa3dgl-library/{expected}")
        );
    }
    let mut graph = fixture_graph();
    graph.native_selected_client_archives = Some(BTreeSet::from(["hidd-gallium-linklib".into()]));
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("no client archive builder"));
}

#[test]
fn selectors_must_be_present_but_supported_empty_values_are_distinct_from_missing() {
    let mut missing = mesa_x86_64();
    missing.cpu32 = None;
    assert!(inventory(&DependencyGraph::default(), &missing)
        .unwrap_err()
        .contains("explicit cpu32 selector"));

    let arm = TargetContext {
        cpu: Some("arm".into()),
        platform: Some("raspi".into()),
        toolchain: Some("llvm".into()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".into()),
        float_abi: Some("hard".into()),
        mesa_version: Some("26.0.0".into()),
        ..TargetContext::default()
    };
    let mut empty_selected = DependencyGraph::default();
    empty_selected.native_selected_runtime_modules = Some(BTreeSet::new());
    empty_selected.native_selected_client_archives = Some(BTreeSet::new());
    assert!(inventory(&empty_selected, &arm).unwrap().records.is_empty());

    let mut wrong_version = mesa_x86_64();
    wrong_version.mesa_version = None;
    assert!(inventory(&DependencyGraph::default(), &wrong_version)
        .unwrap_err()
        .contains("explicit Mesa 26.0.0 selector"));
}

#[test]
fn unknown_runtime_types_and_unsafe_runtime_configs_are_rejected() {
    let mut graph = fixture_graph();
    let mesa_gl = graph.targets.get_mut("mesa3dgl-library").unwrap();
    mesa_gl.module_type = ModuleType::Custom;
    mesa_gl.declared_mod_type = Some("mystery".into());
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("unknown genmodule type `mystery`"));

    let mut graph = fixture_graph();
    graph
        .targets
        .get_mut("mesa3dgl-library")
        .unwrap()
        .config_file = Some("${OTHER_ROOT}/mesa.conf".into());
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("base configuration is not a safe source-relative path"));

    let mut graph = fixture_graph();
    graph
        .targets
        .get_mut("mesa3dgl-library")
        .unwrap()
        .config_file = Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/../outside.conf".into());
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("lexically normalized"));
}

#[test]
fn selected_markers_fail_closed_on_malformed_or_unowned_tokens() {
    for marker in [
        "@AROS_GENMODULE|sideways|stackstubs|gl|library|gl.conf",
        "@AROS_GENMODULE|normal|regcallstubs,stackstubs|gl|library|gl.conf",
        "@AROS_GENMODULE|normal|stackstubs|gl|not-a-type|gl.conf",
        "@AROS_GENMODULE|normal|stackstubs|gl|library|../gl.conf",
        "@AROS_GENMODULE|normal||gl|library|gl.conf",
        "prefix@AROS_GENMODULE|normal|stackstubs|gl|library|gl.conf",
    ] {
        let mut graph = fixture_graph();
        graph
            .targets
            .get_mut("workbench-libs-gl-linklib")
            .unwrap()
            .source_files = vec![marker.to_owned()];
        assert!(inventory(&graph, &mesa_x86_64()).is_err(), "{marker}");
    }

    let mut graph = fixture_graph();
    graph
        .targets
        .get_mut("workbench-libs-gl-linklib")
        .unwrap()
        .source_files = vec![GL_NORMAL.to_owned(), GL_NORMAL.to_owned()];
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("duplicate genmodule owner lane"));

    let mut graph = fixture_graph();
    graph.genmodule_writefiles_rules.push(writefiles_rule(
        "different-producer",
        "workbench/libs/gl",
        "gl",
    ));
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("maps to 2 writefiles producers"));

    let mut graph = fixture_graph();
    graph
        .targets
        .get_mut("workbench-libs-gl-linklib-rel")
        .unwrap()
        .source_files = vec!["@AROS_GENMODULE|rel|stackstubs|posixc|library|other.conf".into()];
    let mut second = writefiles_rule("workbench-libs-gl-gen-stubs", "workbench/libs/gl", "posixc");
    second.config = "workbench/libs/gl/other.conf".into();
    graph.genmodule_writefiles_rules.push(second);
    assert!(inventory(&graph, &mesa_x86_64())
        .unwrap_err()
        .contains("conflicting declaration tuples"));
}

#[test]
fn inventory_records_are_serializable_data() {
    let result = inventory(&fixture_graph(), &mesa_x86_64()).unwrap();
    let json = serde_json::to_value(&result).unwrap();
    let record = json["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["consumer_path"] == "runtime/mesa3dgl-library")
        .unwrap();
    assert_eq!(record["module"], "mesa3dgl26-0");
    assert!(record["components"].as_array().unwrap().is_empty());
}

fn pending_identity(
    name: &str,
    directory: &str,
    module: &str,
    module_type: ModuleType,
) -> InventoryTargetIdentity {
    InventoryTargetIdentity::from(&runtime(name, directory, module, module_type))
}

#[test]
fn selected_pending_runtime_preserves_its_exact_configuration_paths() {
    let mut graph = fixture_graph();
    graph.targets.remove("mesa3dgl-library");
    let mut pending = pending_identity(
        "mesa3dgl-library",
        "workbench/libs/mesa",
        "mesa3dgl26-0",
        ModuleType::Library,
    );
    pending.config_file = Some("${AROS_SOURCE_DIR}/workbench/libs/gl/gl.conf".into());
    pending.config_override_file =
        Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa3dgl.conf".into());
    graph.inventory_targets.push(pending);

    let result = inventory(&graph, &mesa_x86_64()).unwrap();
    let runtime = result
        .records
        .iter()
        .find(|record| record.owner == "mesa3dgl-library")
        .unwrap();
    assert_eq!(runtime.consumer_path, "runtime/mesa3dgl-library");
    assert_eq!(runtime.base_configuration, "workbench/libs/gl/gl.conf");
    assert_eq!(
        runtime.override_configuration.as_deref(),
        Some("workbench/libs/mesa/mesa3dgl.conf")
    );
}

#[test]
fn selected_pending_client_only_emits_exact_client_lanes_without_runtime() {
    let mut graph = fixture_graph();
    graph.targets.remove("mesa3dgl-library");
    graph.targets.remove("workbench-libs-gl-linklib");
    graph.targets.remove("workbench-libs-gl-linklib-rel");
    graph.native_selected_runtime_modules = Some(BTreeSet::new());
    graph.native_selected_client_archives = Some(BTreeSet::from([
        "mesa3dgl-library-linklib".into(),
        "mesa3dgl-library-linklib-rel".into(),
    ]));
    let mut pending = pending_identity(
        "mesa3dgl-library",
        "workbench/libs/mesa",
        "mesa3dgl26-0",
        ModuleType::Library,
    );
    pending.config_file = Some("${AROS_SOURCE_DIR}/workbench/libs/gl/gl.conf".into());
    pending.config_override_file =
        Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa3dgl.conf".into());
    pending.genmodule_linklibs = Some(GenmoduleLinklibs {
        enabled: true,
        has_relative: true,
        ..GenmoduleLinklibs::default()
    });
    graph.inventory_targets.push(pending);

    let result = inventory(&graph, &mesa_x86_64()).unwrap();
    let clients: Vec<_> = result
        .records
        .iter()
        .filter(|record| record.owner == "mesa3dgl-library")
        .collect();
    assert_eq!(clients.len(), 2);
    assert_eq!(
        clients
            .iter()
            .map(|record| record.consumer_path.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "module-client/mesa3dgl-library/normal",
            "module-client/mesa3dgl-library/rel",
        ])
    );
    assert!(clients.iter().all(|record| {
        record.base_configuration == "workbench/libs/gl/gl.conf"
            && record.override_configuration.as_deref() == Some("workbench/libs/mesa/mesa3dgl.conf")
    }));
}

#[test]
fn unselected_supported_pending_declaration_is_omitted() {
    let mut graph = fixture_graph();
    graph.inventory_targets.push(pending_identity(
        "mesa3d-linklib-compiler",
        "workbench/libs/mesa/libcompiler",
        "mesa3dcompiler",
        ModuleType::Library,
    ));

    let result = inventory(&graph, &mesa_x86_64()).unwrap();
    assert!(!result
        .records
        .iter()
        .any(|record| record.owner == "mesa3d-linklib-compiler"));
}

#[test]
fn concrete_declaration_wins_over_same_owner_pending_fallback() {
    let mut graph = fixture_graph();
    let mut pending = pending_identity(
        "mesa3dgl-library",
        "workbench/libs/mesa",
        "pending-module-name",
        ModuleType::Library,
    );
    pending.config_file = Some("workbench/libs/mesa/pending.conf".into());
    graph.inventory_targets.push(pending);

    let result = inventory(&graph, &mesa_x86_64()).unwrap();
    let records: Vec<_> = result
        .records
        .iter()
        .filter(|record| record.owner == "mesa3dgl-library")
        .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].module, "mesa3dgl26-0");
    assert_eq!(records[0].base_configuration, "workbench/libs/gl/gl.conf");
    assert_eq!(
        records[0].override_configuration.as_deref(),
        Some("workbench/libs/mesa/mesa3dgl.conf")
    );
}
