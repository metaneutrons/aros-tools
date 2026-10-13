//! Native SDK consumers need explicit include roots to outrank Clang's
//! built-in sysroot directories; ordinary legacy and GNU builds retain theirs.

use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

const FUNCTION_START: &str = "function(_aros_configure_default_sdk_header_context)";

fn production_function() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("engine/AROS.cmake");
    let source = fs::read_to_string(path).expect("read production AROS.cmake");
    let start = source
        .find(FUNCTION_START)
        .expect("find production SDK header context function");
    let function_end = source[start..]
        .find("endfunction()")
        .expect("find end of SDK header context function")
        + "endfunction()".len();
    source[start..start + function_end].to_owned()
}

fn configure(mode: &str) -> (TempDir, std::process::Output) {
    let temporary = TempDir::new().expect("create SDK header authority fixture");
    let source = temporary.path().join("source");
    let project = temporary.path().join("project");
    let build = temporary.path().join("build");
    for directory in [
        source.join("compiler/include"),
        source.join("arch/all-native/include"),
        project.clone(),
    ] {
        fs::create_dir_all(directory).expect("create fixture directory");
    }
    fs::write(
        project.join("probe.c"),
        "int sdk_header_probe(void) { return 0; }\n",
    )
    .expect("write compile probe");

    let script = r#"
cmake_minimum_required(VERSION 3.22)
project(SdkHeaderAuthority LANGUAGES C)

set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(AROS_SDK_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/include")
set(AROS_GENINC_DIR "${CMAKE_BINARY_DIR}/GENINCDIR")
set(AROS_SOURCE_DIR "@SOURCE@")

if("@MODE@" STREQUAL "sdk")
    set(AROS_TOOLCHAIN llvm)
    set(AROS_NATIVE_CONSUMER_CONTRACT fixture-contract)
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
    set(TEST_SOURCE_SELECTION_CURRENT TRUE)
    set(_schema aros-sdk-consumer-validation-v1)
elseif("@MODE@" STREQUAL "stale")
    set(AROS_TOOLCHAIN llvm)
    set(AROS_NATIVE_CONSUMER_CONTRACT fixture-contract)
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
    set(TEST_SOURCE_SELECTION_CURRENT FALSE)
    set(_schema aros-sdk-consumer-validation-v1)
elseif("@MODE@" STREQUAL "gnu")
    set(AROS_TOOLCHAIN gnu)
    set(AROS_NATIVE_CONSUMER_CONTRACT fixture-contract)
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
    set(TEST_SOURCE_SELECTION_CURRENT TRUE)
    set(_schema aros-sdk-consumer-validation-v1)
elseif("@MODE@" STREQUAL "legacy")
    set(AROS_TOOLCHAIN llvm)
    set(TEST_SOURCE_SELECTION_CURRENT FALSE)
    set(_schema "")
else()
    message(FATAL_ERROR "unknown test mode: @MODE@")
endif()

function(_aros_native_source_selection_validated out_validated)
    set(${out_validated} "${TEST_SOURCE_SELECTION_CURRENT}" PARENT_SCOPE)
endfunction()
set_property(GLOBAL PROPERTY AROS_NATIVE_CONSUMER_VALIDATED_SCHEMA "${_schema}")

@PRODUCTION_FUNCTION@
_aros_configure_default_sdk_header_context()
add_library(sdk-header-probe OBJECT probe.c)

get_target_property(_compile_options sdk-header-probe COMPILE_OPTIONS)
if(NOT _compile_options)
    set(_compile_options "")
endif()
list(FIND _compile_options "-nostdlibinc" _nostdlibinc_index)
if("@MODE@" STREQUAL "sdk")
    if(_nostdlibinc_index EQUAL -1)
        message(FATAL_ERROR "validated LLVM SDK target lacks -nostdlibinc: ${_compile_options}")
    endif()
else()
    if(NOT _nostdlibinc_index EQUAL -1)
        message(FATAL_ERROR "@MODE@ target unexpectedly gained -nostdlibinc: ${_compile_options}")
    endif()
endif()

get_target_property(_include_dirs sdk-header-probe INCLUDE_DIRECTORIES)
foreach(_root IN ITEMS
        "${AROS_GENINC_DIR}"
        "${AROS_SDK_INCLUDE_DIR}/aros/posixc"
        "${AROS_SDK_INCLUDE_DIR}/aros/stdc"
        "${AROS_SDK_INCLUDE_DIR}"
        "${AROS_SOURCE_DIR}/compiler/include"
        "${AROS_SOURCE_DIR}/arch/all-native/include")
    string(FIND "${_include_dirs}" "${_root}" _root_index)
    if(_root_index EQUAL -1)
        message(FATAL_ERROR "@MODE@ target lost explicit SDK/compiler include root ${_root}: ${_include_dirs}")
    endif()
endforeach()
"#;

    let script = script
        .replace("@SOURCE@", &source.display().to_string())
        .replace("@MODE@", mode)
        .replace("@PRODUCTION_FUNCTION@", &production_function());
    fs::write(project.join("CMakeLists.txt"), script).expect("write fixture CMakeLists.txt");
    let output = Command::new("cmake")
        .args(["-S"])
        .arg(&project)
        .args(["-B"])
        .arg(&build)
        .args(["-G", "Ninja"])
        .output()
        .expect("configure SDK header authority fixture");
    (temporary, output)
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn only_current_llvm_sdk_selection_disables_implicit_sysroot_headers() {
    for mode in ["sdk", "stale", "gnu", "legacy"] {
        let (_temporary, output) = configure(mode);
        assert!(
            output.status.success(),
            "{mode} header-context fixture failed:\n{}",
            output_text(&output)
        );
    }
}
