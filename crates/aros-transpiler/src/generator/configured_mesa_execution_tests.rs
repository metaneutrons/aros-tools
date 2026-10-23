//! Real tiny compiler-command test of generated CMake, not Mesa qualification.

use super::targets::emit_concrete_targets;
use crate::capability::mesa::configured_driver::MesaConfiguredDriver;
use crate::capability::mesa::configured_graph::{ConfiguredMesaGraph, ConfiguredMesaTarget};
use crate::capability::mesa::configured_source_context::MesaSourceCompileContext;
use crate::capability::mesa::ordered_contract::OrderedMesaCompilerOptions;
use crate::{dirs::DirVars, parse_mmakefile_with_dirs, DependencyGraph};
use aros_common::mesa_configuration::{
    MesaMakeCoordinates, MesaRecipeConfiguration, MesaRecipeFlagValue, MesaTargetLto,
};
use std::{collections::BTreeMap, fs, process::Command};

fn configured_mesa_payload() -> ConfiguredMesaGraph {
    let coordinates = MesaMakeCoordinates {
        source_root: "/nominal/source".into(),
        build_root: "/nominal/build".into(),
        generated_root: "/nominal/build/bin/target/gen".into(),
        ports_root: "/nominal/build/bin/target/Ports".into(),
    };
    ConfiguredMesaGraph {
        driver: MesaConfiguredDriver {
            target_triple: "arm-unknown-aros".into(),
            recipe: MesaRecipeConfiguration {
                make_coordinates: coordinates,
                safety_cflags: MesaRecipeFlagValue::Undefined,
                target_lto: MesaTargetLto::No,
                lto_cflags: vec![],
                cflags_iquote: vec!["-iquote".into()],
                cflags_iquote_end: vec![],
                target_cc: "/selected/bin/clang".into(),
                target_cxx: "/selected/bin/clang++".into(),
                target_sysroot: vec!["--sysroot".into(), "/nominal/build/sdk/Developer".into()],
            },
        },
        targets: BTreeMap::from([(
            "configured".into(),
            ConfiguredMesaTarget {
                options: OrderedMesaCompilerOptions {
                    c: vec![
                        "-DCONFIGURED=1".into(),
                        "-DORDER=1".into(),
                        "-UORDER".into(),
                        "-DORDER=2".into(),
                        "-DORDER=2".into(),
                    ],
                    cxx: vec!["-DCONFIGURED=1".into(), "-DCXX_ONLY=1".into()],
                    vc4_bridge: vec![],
                    gca_abi_flags: String::new(),
                },
                source_context: MesaSourceCompileContext {
                    object_directory: "/nominal/build/bin/target/gen/module/26.0.0".into(),
                    quote_extra: "/nominal/build/module".into(),
                    python_build_directory: None,
                },
            },
        )]),
    }
}

#[test]
fn configured_generator_preserves_order_companions_and_structural_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    // The real engine source and generated child source live beside, not
    // inside, one another. A binary tree below this fixture source would hide
    // CMake's mandatory explicit-binary-directory rule for that layout.
    let binary_temporary = tempfile::tempdir().unwrap();
    let binary = binary_temporary.path();
    fs::create_dir(root.join("module")).unwrap();
    fs::create_dir(root.join("engine")).unwrap();
    fs::create_dir(root.join("quotes")).unwrap();
    fs::write(
        root.join("quotes/structural.h"),
        "#define QUOTE_PATH_OK 1\n",
    )
    .unwrap();
    let source = r#"
#include "structural.h"
#if !QUOTE_PATH_OK || !STRUCTURAL_DEFINITION
#error structural builder inputs lost
#endif
#ifdef CONFIGURED
#if defined(PARENT_DIRECTORY) || defined(PARENT_GLOBAL) || defined(PARENT_RELEASE) || defined(BUILDER_LEAK)
#error inherited flags leaked into configured target
#endif
#ifdef __cplusplus
#if !CXX_ONLY || defined(ORDER)
#error configured language isolation lost
#endif
#else
#if ORDER != 2 || defined(CXX_ONLY)
#error configured order or language isolation lost
#endif
#endif
#else
#if !PARENT_DIRECTORY || !PARENT_GLOBAL || !PARENT_RELEASE || !BUILDER_LEAK
#error ordinary target flags changed
#endif
#endif
int fixture;
"#;
    fs::write(root.join("module/probe.c"), source).unwrap();
    fs::write(root.join("module/probexx.cpp"), source).unwrap();
    let recipe = root.join("module/mmakefile.src");
    fs::write(&recipe, "%build_linklib mmake=configured libname=configured files=probe cxxfiles=probexx\n%build_linklib mmake=ordinary libname=ordinary files=probe cxxfiles=probexx\n").unwrap();
    let parsed = parse_mmakefile_with_dirs(&recipe, root, &DirVars::load(root)).unwrap();
    let mut graph = DependencyGraph::new();
    for target in parsed.targets {
        graph.add_target(target);
    }
    graph.configured_mesa = Some(configured_mesa_payload());
    let mut generated = String::new();
    emit_concrete_targets(&mut generated, &graph, &BTreeMap::new());
    fs::write(
        root.join("engine/OrderedCompileContract.cmake"),
        aros_cmake_engine::file("OrderedCompileContract.cmake").unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("engine/PythonGenerators.cmake"),
        aros_cmake_engine::file("PythonGenerators.cmake").unwrap(),
    )
    .unwrap();
    // This fixture checks how the target emitter isolates and orders actual
    // compiler arguments. Driver identity and source producer binding have
    // dedicated fixtures, so these stubs accept the complete payload while
    // retaining the explicitly seeded sysroot used by the compile assertions.
    fs::write(
        root.join("engine/ConfiguredMesaDriver.cmake"),
        r"
function(_aros_bind_mesa_compiler_roles expected_c expected_cxx triple nominal_build nominal_gen nominal_ports)
endfunction()
",
    )
    .unwrap();
    fs::write(
        root.join("engine/ConfiguredMesaSources.cmake"),
        r"
function(_aros_bind_mesa_genmodule_context target owner declaration make_objdir make_gendir)
endfunction()
function(_aros_bind_mesa_python_context target owner build_root make_root make_gendir)
endfunction()
",
    )
    .unwrap();
    let project = r#"
cmake_minimum_required(VERSION 3.24)
project(ConfiguredMesaGenerated LANGUAGES C CXX)
set(CMAKE_C_FLAGS "-DPARENT_GLOBAL=1")
set(CMAKE_CXX_FLAGS "-DPARENT_GLOBAL=1")
set(CMAKE_C_FLAGS_RELEASE "-DPARENT_RELEASE=1")
set(CMAKE_CXX_FLAGS_RELEASE "-DPARENT_RELEASE=1")
add_compile_options(-DPARENT_DIRECTORY=1)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
set(AROS_BUILD_DIR "${CMAKE_BINARY_DIR}")
set(AROS_CMAKE_ENGINE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/engine")
# This fixture tests argument preservation only. The sentinel enables the
# emitter's sysroot-restoration branch; it does not validate driver identity.
set(_aros_mesa_driver_roles_bound TRUE)
set(_aros_mesa_sysroot_options --sysroot "${CMAKE_BINARY_DIR}/SDK path/Developer")
function(aros_add_linklib)
    cmake_parse_arguments(ARG "" "TARGET;MMAKE_ID;DIRECTORY" "SOURCES;CXX_SOURCES" ${ARGN})
    set(_targets "${ARG_MMAKE_ID}")
    if(ARG_MMAKE_ID STREQUAL "configured")
        list(APPEND _targets "${ARG_MMAKE_ID}-companion")
    endif()
    foreach(_target IN LISTS _targets)
        add_library("${_target}" OBJECT "${ARG_DIRECTORY}/probe.c" "${ARG_DIRECTORY}/probexx.cpp")
        target_compile_options("${_target}" PRIVATE "-iquote${AROS_SOURCE_DIR}/quotes" -DBUILDER_LEAK=1)
        target_compile_definitions("${_target}" PRIVATE STRUCTURAL_DEFINITION=1)
    endforeach()
endfunction()
if(NOT GENERATED_FILE)
    set(GENERATED_FILE "${CMAKE_CURRENT_SOURCE_DIR}/generated.cmake")
endif()
# AROS.cmake defines source binding helpers in the parent scope before the
# generated configured-Mesa subdirectory uses them.
include("${AROS_CMAKE_ENGINE_DIR}/ConfiguredMesaSources.cmake")
include("${GENERATED_FILE}")
include("${AROS_CMAKE_ENGINE_DIR}/PythonGenerators.cmake")
add_custom_target(configured-generated)
set_property(TARGET configured-generated PROPERTY AROS_PYTHON_OUTPUT_OWNER TRUE)
aros_bind_python_output_consumers(
    OWNER configured-generated CONSUMERS configured configured-companion)
foreach(_consumer IN ITEMS configured configured-companion)
    get_target_property(_dependencies "${_consumer}" MANUALLY_ADDED_DEPENDENCIES)
    if(NOT "configured-generated" IN_LIST _dependencies)
        message(FATAL_ERROR "configured Python-owner dependency was not retained")
    endif()
endforeach()
"#;
    fs::write(root.join("CMakeLists.txt"), project).unwrap();
    fs::write(root.join("generated.cmake"), &generated).unwrap();
    let configure = Command::new("cmake")
        .args(["-S"])
        .arg(root)
        .arg("-B")
        .arg(binary.join("build"))
        .args(["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release"])
        .output()
        .unwrap();
    assert!(
        configure.status.success(),
        "{}{}",
        String::from_utf8_lossy(&configure.stdout),
        String::from_utf8_lossy(&configure.stderr)
    );
    let build = Command::new("cmake")
        .arg("--build")
        .arg(binary.join("build"))
        .args(["--parallel", "1"])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let commands: serde_json::Value =
        serde_json::from_slice(&fs::read(binary.join("build/compile_commands.json")).unwrap())
            .unwrap();
    let commands = commands.as_array().unwrap();
    assert_eq!(commands.len(), 6);
    let mut configured_c = 0;
    let mut configured_sysroots = 0;
    for command in commands {
        let command = command["command"].as_str().unwrap();
        if command.contains("-DCONFIGURED=1") {
            assert_eq!(command.matches("--sysroot").count(), 1, "{command}");
            assert!(command.contains("SDK path/Developer"), "{command}");
            configured_sysroots += 1;
        } else {
            assert!(!command.contains("--sysroot"), "{command}");
        }
        if command.contains("-DCONFIGURED=1") && command.contains("-DORDER=1") {
            assert_eq!(command.matches("-DORDER=2").count(), 2, "{command}");
            assert!(
                command.contains("-DORDER=1 -UORDER -DORDER=2 -DORDER=2"),
                "{command}"
            );
            configured_c += 1;
        }
    }
    assert_eq!(
        configured_c, 2,
        "original and companion C commands must be projected"
    );
    assert_eq!(
        configured_sysroots, 4,
        "both languages of main and companion retain the driver prefix"
    );

    // A separate invalid copy must fail the actual compiler assertions. This
    // demonstrates that clearing global C flags is effective, not just emitted.
    // Removing only the directory COMPILE_OPTIONS reset is not an effective
    // counter for these endpoints: the later target reset also removes them.
    let reset = "set(CMAKE_C_FLAGS \"\")";
    assert_eq!(generated.matches(reset).count(), 1);
    fs::write(
        root.join("counter.cmake"),
        generated.replace(reset, "# deliberately omitted global C flag reset"),
    )
    .unwrap();
    let configure = Command::new("cmake")
        .arg("-S")
        .arg(root)
        .arg("-B")
        .arg(binary.join("counter-build"))
        .args(["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release"])
        .arg(format!(
            "-DGENERATED_FILE={}",
            root.join("counter.cmake").display()
        ))
        .output()
        .unwrap();
    assert!(
        configure.status.success(),
        "{}",
        String::from_utf8_lossy(&configure.stderr)
    );
    let counter = Command::new("cmake")
        .arg("--build")
        .arg(binary.join("counter-build"))
        .args(["--parallel", "1"])
        .output()
        .unwrap();
    assert!(!counter.status.success());
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&counter.stdout),
        String::from_utf8_lossy(&counter.stderr)
    );
    assert!(
        diagnostic.contains("inherited flags leaked into configured target"),
        "{diagnostic}"
    );
}
