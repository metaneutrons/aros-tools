use super::configured_mesa::cmake_bracket_argument;
use crate::capability::mesa::configured_driver::MesaConfiguredDriver;
use crate::capability::mesa::configured_graph::{ConfiguredMesaGraph, ConfiguredMesaTarget};
use crate::capability::mesa::configured_source_context::MesaSourceCompileContext;
use crate::capability::mesa::ordered_contract::OrderedMesaCompilerOptions;
use crate::dirs::DirVars;
use crate::graph::DependencyGraph;
use crate::testing::TempTree;
use crate::{generate_cmake, parse_mmakefile_with_dirs};
use aros_common::mesa_configuration::{
    MesaMakeCoordinates, MesaRecipeConfiguration, MesaRecipeFlagValue, MesaTargetLto,
};
use std::collections::BTreeMap;
use std::fs;

const VC4: &str = "linklibs-gallium_vc4";

fn fixture() -> (TempTree, DependencyGraph) {
    let tree = TempTree::new();
    let module = tree.0.join("workbench/mesa");
    fs::create_dir_all(&module).unwrap();
    for source in ["vc4.c", "vc4xx.cpp", "ordinary.c", "hidd.c"] {
        fs::write(module.join(source), "int fixture;\n").unwrap();
    }
    let file = module.join("mmakefile.src");
    fs::write(
        &file,
        "%build_linklib mmake=linklibs-gallium_vc4 libname=gallium_vc4 files=vc4 cxxfiles=vc4xx\n\
         %build_linklib mmake=ordinary libname=ordinary files=ordinary\n\
         %build_module mmake=mesa-hidd modname=mesa-hidd modtype=hidd files=hidd\n",
    )
    .unwrap();
    let parsed = parse_mmakefile_with_dirs(&file, &tree.0, &DirVars::load(&tree.0)).unwrap();
    let mut graph = DependencyGraph::new();
    for mut target in parsed.targets {
        target.include_dirs = vec!["${AROS_SOURCE_DIR}/stale/include".into()];
        target.defines = vec!["STALE_DEFINE=1".into()];
        target.undefines = vec!["STALE_UNDEFINE".into()];
        target.compile_options = vec!["-fno-common".into()];
        target.arch_defines = vec![("armv7".into(), "STALE_ARCH_DEFINE=1".into())];
        target.arch_compile_options = vec![("armv7".into(), "-march=stale".into())];
        graph.add_target(target);
    }
    (tree, graph)
}

fn contract() -> OrderedMesaCompilerOptions {
    OrderedMesaCompilerOptions {
        c: vec![
            "-DC_ORDER=1".into(),
            "-UOLD_C".into(),
            "-DC_ORDER=2".into(),
            "-DC_ORDER=2".into(),
        ],
        cxx: vec![
            "-DCXX_ORDER=1".into(),
            "-UOLD_CXX".into(),
            "-DCXX_ORDER=2".into(),
        ],
        vc4_bridge: vec!["-DBRIDGE_ORDER=1".into(), "-UBRIDGE_OLD".into()],
        gca_abi_flags: "-march=armv8-a -DABI_WORD=1".into(),
    }
}

fn nominal_coordinates() -> MesaMakeCoordinates {
    MesaMakeCoordinates {
        source_root: "/nominal/source".into(),
        build_root: "/nominal/build".into(),
        generated_root: "/nominal/build/bin/target/gen".into(),
        ports_root: "/nominal/build/bin/target/Ports".into(),
    }
}

fn synthetic_driver() -> MesaConfiguredDriver {
    MesaConfiguredDriver {
        target_triple: "arm-unknown-aros".into(),
        recipe: MesaRecipeConfiguration {
            make_coordinates: nominal_coordinates(),
            safety_cflags: MesaRecipeFlagValue::Undefined,
            target_lto: MesaTargetLto::No,
            lto_cflags: vec![],
            cflags_iquote: vec!["-iquote".into()],
            cflags_iquote_end: vec![],
            target_cc: "/selected/bin/clang".into(),
            target_cxx: "/selected/bin/clang++".into(),
            target_sysroot: vec!["--sysroot".into(), "/nominal/build/sdk/Developer".into()],
        },
    }
}

fn synthetic_source_context() -> MesaSourceCompileContext {
    MesaSourceCompileContext {
        object_directory: "/nominal/build/bin/target/gen/workbench/mesa/26.0.0".into(),
        quote_extra: "/nominal/build/workbench/mesa".into(),
        python_build_directory: None,
    }
}

fn attach_configured_mesa(
    graph: &mut DependencyGraph,
    targets: impl IntoIterator<Item = (&'static str, OrderedMesaCompilerOptions)>,
) {
    graph.configured_mesa = Some(ConfiguredMesaGraph {
        driver: synthetic_driver(),
        targets: targets
            .into_iter()
            .map(|(name, options)| {
                (
                    name.into(),
                    ConfiguredMesaTarget {
                        options,
                        source_context: synthetic_source_context(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>(),
    });
}

fn configured_child(generated: &str) -> &str {
    let write = generated.find("file(WRITE").unwrap();
    let open_start = write + generated[write..].find('[').unwrap();
    let open_end = open_start + generated[open_start + 1..].find('[').unwrap() + 1;
    let equals = &generated[open_start + 1..open_end];
    let close = format!("]{equals}]");
    let payload_start = open_end + 1;
    let payload_end = payload_start + generated[payload_start..].find(&close).unwrap();
    &generated[payload_start..payload_end]
}

#[test]
fn configured_mesa_targets_are_isolated_and_keep_ordered_language_options() {
    let (_tree, mut graph) = fixture();
    attach_configured_mesa(&mut graph, [(VC4, contract()), ("mesa-hidd", contract())]);

    let generated = generate_cmake(&graph);
    let parent_target = generated.find("MMAKE_ID ordinary").unwrap();
    let wrapper = generated
        .find("file(WRITE \"${_aros_configured_mesa_dir}/CMakeLists.txt\"")
        .unwrap();
    let configured_target = generated.find("MMAKE_ID linklibs-gallium_vc4").unwrap();
    let child_end = generated
        .find("add_subdirectory(\"${_aros_configured_mesa_dir}\" \"${_aros_configured_mesa_dir}/build\")")
        .unwrap();
    assert!(
        parent_target < wrapper && wrapper < configured_target && configured_target < child_end
    );
    let parent_targets = &generated[parent_target..wrapper];
    assert!(parent_targets.contains("INCLUDES \"${AROS_SOURCE_DIR}/stale/include\""));
    assert!(parent_targets.contains("COMPILE_OPTIONS \"-fno-common\""));

    let child = configured_child(&generated);
    for reset in [
        "set(CMAKE_C_FLAGS \"\")",
        "set(CMAKE_CXX_FLAGS \"\")",
        "set(CMAKE_ASM_FLAGS \"\")",
        "^CMAKE_(C|CXX|ASM)_FLAGS_.+$",
        "set_property(DIRECTORY PROPERTY COMPILE_OPTIONS \"\")",
        "add_compile_options(${_aros_mesa_sysroot_options})",
    ] {
        assert!(
            child.contains(reset),
            "missing child flag reset {reset}:\n{child}"
        );
    }

    let vc4_owner = child.find("MMAKE_ID linklibs-gallium_vc4").unwrap();
    let vc4_declaration_start = child[..vc4_owner].rfind("aros_add_linklib(").unwrap();
    let vc4_declaration_end =
        vc4_declaration_start + child[vc4_declaration_start..].find("\n)\n").unwrap() + 3;
    let vc4_declaration = &child[vc4_declaration_start..vc4_declaration_end];
    assert!(vc4_declaration.contains("TARGET gallium_vc4"));
    assert!(vc4_declaration.contains("SOURCES \"vc4\""));
    assert!(vc4_declaration.contains("CXX_SOURCES \"vc4xx\""));
    for suppressed in [
        "INCLUDES ",
        "STALE_DEFINE",
        "STALE_UNDEFINE",
        "-fno-common",
        "STALE_ARCH_DEFINE",
        "-march=stale",
    ] {
        assert!(
            !vc4_declaration.contains(suppressed),
            "configured declaration retained {suppressed}:\n{vc4_declaration}"
        );
    }

    let c = concat!(
        "_aros_target_ordered_compile_contract(\"${_aros_configured_mesa_target}\" C\n",
        "    \"-DC_ORDER=1\"\n",
        "    \"-UOLD_C\"\n",
        "    \"-DC_ORDER=2\"\n",
        "    \"-DC_ORDER=2\"\n",
        ")"
    );
    let cxx = concat!(
        "_aros_target_ordered_compile_contract(\"${_aros_configured_mesa_target}\" CXX\n",
        "    \"-DCXX_ORDER=1\"\n",
        "    \"-UOLD_CXX\"\n",
        "    \"-DCXX_ORDER=2\"\n",
        ")"
    );
    assert_eq!(
        child.matches(c).count(),
        2,
        "each builder endpoint needs one full C sequence:\n{child}"
    );
    assert_eq!(
        child.matches(cxx).count(),
        2,
        "each builder endpoint needs one full CXX sequence:\n{child}"
    );
    assert!(child.contains(
        "get_property(_aros_configured_mesa_targets_before DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)"
    ));
    assert!(child.contains(
        "get_property(_aros_configured_mesa_targets_after DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)"
    ));
    assert!(child.contains("list(REMOVE_ITEM _aros_configured_mesa_new_targets \"${_aros_configured_mesa_old_target}\")"));
    assert!(child.contains("get_target_property(_aros_configured_mesa_target_type \"${_aros_configured_mesa_target}\" TYPE)"));
    assert!(child.contains("get_target_property(_aros_configured_mesa_imported \"${_aros_configured_mesa_target}\" IMPORTED)"));
    assert!(child.contains("set_property(TARGET \"${_aros_configured_mesa_target}\" PROPERTY AROS_MESA_CONFIGURED_CONTRACT TRUE)"));
    // aros_add_hidd installs structural libdefs/name preprocessor definitions
    // internally. With the source definitions withheld from the builder, the
    // configured projection must leave those builder-owned definitions alone.
    assert!(child.contains("aros_add_hidd(\n    TARGET mesa-hidd\n    MMAKE_ID mesa-hidd\n"));
    assert!(!child.contains("PROPERTY COMPILE_DEFINITIONS \"\""));
    assert!(child.contains(&format!(
        "PROPERTY AROS_MESA_GCA_ABI_FLAGS {}",
        cmake_bracket_argument("-march=armv8-a -DABI_WORD=1")
    )));
    assert!(child.contains(
        "PROPERTY AROS_MESA_C_OPTIONS \"-DC_ORDER=1\" \"-UOLD_C\" \"-DC_ORDER=2\" \"-DC_ORDER=2\""
    ));
    assert!(child
        .contains("PROPERTY AROS_MESA_VC4_BRIDGE_OPTIONS \"-DBRIDGE_ORDER=1\" \"-UBRIDGE_OLD\""));

    // Quote roots are installed by the builder, not by USER_INCLUDES for this
    // projection. The child snapshots only -iquote entries before clearing the
    // generic options, then restores those entries on the target.
    let snapshot = child
        .find("get_target_property(_aros_configured_mesa_existing_options")
        .unwrap();
    let quote_filter = child
        .find("if(_aros_configured_mesa_option MATCHES \"^-iquote\")")
        .unwrap();
    let clear = child.find("set_property(TARGET \"${_aros_configured_mesa_target}\" PROPERTY COMPILE_OPTIONS \"\")").unwrap();
    let restore = child
        .find("target_compile_options(\"${_aros_configured_mesa_target}\" PRIVATE")
        .unwrap();
    assert!(snapshot < quote_filter && quote_filter < clear && clear < restore);
    assert!(child.contains(
        "elseif(_aros_configured_mesa_target STREQUAL _aros_configured_mesa_main_target)"
    ));
    assert!(!child.contains("PROPERTY COMPILE_DEFINITIONS \"\""));
    assert!(child.contains("aros_build_mesa26_gallium_core_api()"));

    assert_eq!(
        generated
            .matches("aros_build_mesa26_gallium_core_api()")
            .count(),
        1
    );
    assert!(child.ends_with("aros_build_mesa26_gallium_core_api()\n\n"));
}

#[test]
fn configured_metadata_rejects_unprojected_assembler_and_32_bit_lanes() {
    let (_tree, mut graph) = fixture();
    attach_configured_mesa(&mut graph, [(VC4, contract())]);
    graph
        .targets
        .get_mut(VC4)
        .unwrap()
        .asm_source_files
        .push("unexpected.s".into());
    let asm_output = generate_cmake(&graph);
    assert!(asm_output.contains("unmeasured assembler option projection"));
    assert!(!asm_output.contains("aros-configured-mesa"));

    graph.targets.get_mut(VC4).unwrap().asm_source_files.clear();
    graph.targets.get_mut(VC4).unwrap().variant_32bit = true;
    let variant_output = generate_cmake(&graph);
    assert!(variant_output.contains("unmeasured 32-bit companion compiler projection"));
    assert!(!variant_output.contains("aros-configured-mesa"));
}

#[test]
fn absent_configured_payload_keeps_legacy_target_emission() {
    let (_tree, graph) = fixture();
    let generated = generate_cmake(&graph);

    assert!(!generated.contains("aros-configured-mesa"));
    assert!(!generated.contains("OrderedCompileContract.cmake"));
    assert!(generated.contains("MMAKE_ID linklibs-gallium_vc4"));
    assert!(generated.contains("INCLUDES \"${AROS_SOURCE_DIR}/stale/include\""));
    assert!(generated.contains("DEFINES \"STALE_DEFINE=1\""));
    assert!(generated.contains("UNDEFINES \"STALE_UNDEFINE\""));
    assert!(generated.contains("COMPILE_OPTIONS \"-fno-common\""));
    assert!(generated.contains("ARCH_DEFINES \"armv7|STALE_ARCH_DEFINE=1\""));
    assert!(generated.contains("ARCH_COMPILE_OPTIONS \"armv7|-march=stale\""));
}

#[test]
fn unsafe_or_missing_target_configured_metadata_fails_closed() {
    let (_tree, mut graph) = fixture();
    let mut unsafe_contract = contract();
    unsafe_contract.c[0] = "-DVALUE=$<CONFIG>".into();
    attach_configured_mesa(&mut graph, [(VC4, unsafe_contract)]);
    let unsafe_output = generate_cmake(&graph);
    assert!(unsafe_output.starts_with("message(FATAL_ERROR "));
    assert!(!unsafe_output.contains("MMAKE_ID linklibs-gallium_vc4"));
    assert!(!unsafe_output.contains("aros-configured-mesa"));

    let mut missing = DependencyGraph::new();
    attach_configured_mesa(&mut missing, [(VC4, contract())]);
    let missing_output = generate_cmake(&missing);
    assert!(missing_output.contains("configured Mesa compile metadata has no concrete target"));
    assert!(!missing_output.contains("aros-configured-mesa"));
}

#[test]
fn configured_mesa_rejects_empty_targets_and_malformed_driver_payloads() {
    let (_tree, mut graph) = fixture();
    graph.configured_mesa = Some(ConfiguredMesaGraph {
        driver: synthetic_driver(),
        targets: BTreeMap::new(),
    });
    assert!(generate_cmake(&graph).contains("configured Mesa selection has no compile targets"));

    attach_configured_mesa(&mut graph, [(VC4, contract())]);
    graph.configured_mesa.as_mut().unwrap().driver.target_triple = "bad$triple".into();
    let malformed_driver = generate_cmake(&graph);
    assert!(malformed_driver.contains("configured Mesa target triple"));
    assert!(!malformed_driver.contains("aros-configured-mesa"));
}

#[test]
fn measured_mesa_source_context_is_emitted_with_its_complete_payload() {
    let (_tree, mut graph) = fixture();
    attach_configured_mesa(&mut graph, [(VC4, contract())]);
    let generated = generate_cmake(&graph);
    assert!(generated.contains("_aros_bind_mesa_genmodule_context(\"${_aros_configured_mesa_target}\" \"linklibs-gallium_vc4\" \"${AROS_SOURCE_DIR}/workbench/mesa\""));
    assert!(generated.contains("/nominal/build/bin/target/gen/workbench/mesa/26.0.0"));
    assert!(generated.contains("_aros_bind_mesa_compiler_roles([[/selected/bin/clang]] [[/selected/bin/clang++]] [[arm-unknown-aros]]"));
    assert!(generated.contains("[[--sysroot]] [[/nominal/build/sdk/Developer]]"));
    graph.python_outputs.push(crate::ast::PythonOutputsDecl {
        owner: "mesa-python-owner".into(),
        source_root: "${AROS_PORTS_DIR}/mesa/source".into(),
        build_root: "${AROS_BUILD_DIR}/gen/workbench/mesa/26.0.0".into(),
        fetch_target: "mesa-fetch".into(),
        source_inputs: vec![],
        local_inputs: vec![],
        jobs: vec![],
        driver_script: None,
        requires_flex_bison: false,
        python_packages: vec![],
        audited_source_dir: "source".into(),
        local_patch_files: vec![],
        consumers: vec![VC4.into()],
        dir_path: "workbench/mesa".into(),
    });
    assert!(generate_cmake(&graph).contains("Python producer lacks source top_builddir"));
    graph
        .configured_mesa
        .as_mut()
        .unwrap()
        .targets
        .get_mut(VC4)
        .unwrap()
        .source_context
        .python_build_directory =
        Some("/nominal/build/bin/target/gen/workbench/mesa/26.0.0".into());
    let generated = generate_cmake(&graph);
    assert!(generated.contains("_aros_bind_mesa_python_context(\"${_aros_configured_mesa_target}\" \"mesa-python-owner\" \"${AROS_BUILD_DIR}/gen/workbench/mesa/26.0.0\" [[/nominal/build/bin/target/gen/workbench/mesa/26.0.0]]"));
    graph
        .configured_mesa
        .as_mut()
        .unwrap()
        .targets
        .get_mut(VC4)
        .unwrap()
        .source_context
        .python_build_directory = Some("/outside".into());
    assert!(generate_cmake(&graph).contains("Python root leaves measured GENDIR"));
    graph
        .configured_mesa
        .as_mut()
        .unwrap()
        .targets
        .get_mut(VC4)
        .unwrap()
        .source_context
        .python_build_directory = None;
    graph.python_outputs.clear();
    graph
        .configured_mesa
        .as_mut()
        .unwrap()
        .targets
        .get_mut(VC4)
        .unwrap()
        .source_context
        .object_directory = "/outside".into();
    assert!(generate_cmake(&graph).contains("object directory leaves measured GENDIR"));
}

#[test]
fn bracket_arguments_choose_a_delimiter_absent_from_the_payload() {
    let payload = "literal ]] ]=] ]==] closers";
    let bracket = cmake_bracket_argument(payload);
    assert!(bracket.starts_with("[===["));
    assert!(bracket.ends_with("]===]"));
    assert!(bracket.contains(payload));
}
