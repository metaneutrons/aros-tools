//! Exercises the production CLIENTS_ONLY branch while keeping broad SDK and
//! genmodule metadata synthetic. Client source compilation and archive checks
//! still use CMake/Ninja and the host archiver. This is a control-flow and
//! archive-membership fixture, not a target-ABI qualification.

#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

const FIXTURE: &str = r#"
cmake_minimum_required(VERSION 3.22)
project(SdkClientsOnly C)
set(PROBE_MODE "@MODE@")
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/source")
set(AROS_DEVELOPER_LIB_DIR "${CMAKE_BINARY_DIR}/lib")
set(AROS_SDK_INCLUDE_DIR "${CMAKE_BINARY_DIR}/sdk/include")
set(AROS_DEFAULT_POSIXC_INCLUDE "${CMAKE_BINARY_DIR}/sdk/posixc")
set(AROS_TARGET_CPU riscv)
set(AROS_TARGET_PLATFORM fixture)
file(MAKE_DIRECTORY "${AROS_SOURCE_DIR}/module")
file(WRITE "${AROS_SOURCE_DIR}/one.c" "int contract_input(void) { return 0; }\n")
file(WRITE "${AROS_SOURCE_DIR}/module/generic_client.c"
    "int generic_client(void) { return 1; }\n")
file(WRITE "${AROS_SOURCE_DIR}/module/runtime.c"
    "int runtime_module(void) { return 1; }\n")

include("@ENGINE@/tests/NativeSourceSelectionFixture.cmake")
include("@ENGINE@/NativeConsumerContract.cmake")
include("@ENGINE@/GenmoduleTargets.cmake")
if(NOT PROBE_MODE STREQUAL "no-validation")
    aros_test_prepare_native_consumer_selection("${AROS_SOURCE_DIR}"
        "${CMAKE_BINARY_DIR}/native-consumer-contract-test.json")
    aros_validate_native_consumer_contract()
endif()

# Synthetic genmodule metadata models generated includes, normal/relative
# stubs and its aggregate target. Those generated C files are compiled into
# real client archives below.
function(_aros_generate_module_support out)
    cmake_parse_arguments(GM "ABI" "TARGET;MMAKE_ID;DIRECTORY;MODTYPE;MODSUFFIX" "" ${ARGN})
    if(PROBE_MODE MATCHES "^(datatype|gadget|mcc)$" AND NOT GM_MODTYPE STREQUAL PROBE_MODE)
        message(FATAL_ERROR "wrapper lost its source module type")
    endif()
    set(_gen "${CMAKE_CURRENT_BINARY_DIR}/gen/${GM_MMAKE_ID}")
    set(_include "${_gen}/include")
    file(MAKE_DIRECTORY "${_gen}" "${_include}")
    file(WRITE "${_gen}/genmodule_normal_stub.c"
        "int genmodule_normal_stub(void) { return 2; }\n")
    file(WRITE "${_gen}/genmodule_relative_stub.c"
        "int genmodule_relative_stub(void) { return 3; }\n")
    file(WRITE "${_gen}/genmodule_source_free_stub.c"
        "int genmodule_source_free_stub(void) { return 4; }\n")
    add_custom_target("${GM_MMAKE_ID}-includes-generated")
    add_custom_target("${GM_MMAKE_ID}-genmodfiles")
    set(${out}_INCLUDES_TARGET "${GM_MMAKE_ID}-includes-generated" PARENT_SCOPE)
    set(${out}_INCLUDE_DIR "${_include}" PARENT_SCOPE)
    set(${out}_GEN_DIR "${_gen}" PARENT_SCOPE)
    set(${out}_GENMODFILES_TARGET "${GM_MMAKE_ID}-genmodfiles" PARENT_SCOPE)
    set(${out}_FD_TARGET "" PARENT_SCOPE)
    set(${out}_HAS_REL_LINKLIB TRUE PARENT_SCOPE)
    set(${out}_NORMAL_LINKLIB_SOURCES "${_gen}/genmodule_normal_stub.c" PARENT_SCOPE)
    set(${out}_REL_LINKLIB_SOURCES "${_gen}/genmodule_relative_stub.c" PARENT_SCOPE)
    set(${out}_STUB_SOURCES "${_gen}/genmodule_source_free_stub.c" PARENT_SCOPE)
    set(${out}_LINKLIB_DEFINES "" PARENT_SCOPE)
    set(${out}_RUNTIME_DEFINES "" PARENT_SCOPE)
    if(PROBE_MODE STREQUAL "empty-client")
        set(${out}_NORMAL_LINKLIB_SOURCES "" PARENT_SCOPE)
        set(${out}_REL_LINKLIB_SOURCES "" PARENT_SCOPE)
    endif()
endfunction()

function(aros_resolve_source_lanes out directory)
    cmake_parse_arguments(RES "" "MMAKE_ID" "SOURCES" ${ARGN})
    if(RES_MMAKE_ID STREQUAL "client-only")
        message(FATAL_ERROR "CLIENTS_ONLY resolved runtime SOURCES")
    endif()
    set(_resolved "")
    foreach(_source IN LISTS RES_SOURCES)
        if(IS_ABSOLUTE "${_source}")
            set(_path "${_source}")
        else()
            set(_path "${directory}/${_source}")
        endif()
        if(NOT EXISTS "${_path}" OR IS_DIRECTORY "${_path}")
            message(FATAL_ERROR "client source did not resolve: ${_path}")
        endif()
        list(APPEND _resolved "${_path}")
    endforeach()
    set(${out} "${_resolved}" PARENT_SCOPE)
endfunction()

function(_aros_register_genmodule_public_includes target)
endfunction()
function(_aros_add_genmodule_quote_dirs target include_dir gen_dir)
endfunction()
function(_aros_add_genmodule_config_header_dependencies target config)
endfunction()
function(aros_gate_arch target directory)
endfunction()
function(aros_apply_includes target)
endfunction()
function(aros_apply_flags target)
endfunction()
function(aros_link_libraries target)
    message(FATAL_ERROR "CLIENTS_ONLY resolved runtime LIBS")
endfunction()
function(_aros_set_module_linker_language target)
    message(FATAL_ERROR "CLIENTS_ONLY configured a runtime linker")
endfunction()
function(aros_register_runtime_module_output target directory name)
    message(FATAL_ERROR "CLIENTS_ONLY registered a runtime output")
endfunction()

@PRODUCTION_FUNCTIONS@

if(PROBE_MODE STREQUAL "existing-runtime")
    add_executable(client-only EXCLUDE_FROM_ALL
        "${AROS_SOURCE_DIR}/module/runtime.c")
endif()

if(PROBE_MODE STREQUAL "source-free")
    set(_client_arguments
        TARGET fixture
        MMAKE_ID client-only
        DIRECTORY "${AROS_SOURCE_DIR}/module"
        GENMODULE_ONLY
        CLIENTS_ONLY)
else()
    set(_client_arguments
        TARGET fixture
        MMAKE_ID client-only
        DIRECTORY "${AROS_SOURCE_DIR}/module"
        GENMODULE_LINKLIBS
        CLIENTS_ONLY
        LINKLIB_SOURCES generic_client.c
        SOURCES missing-runtime-source.c
        LIBS missing-runtime-library)
endif()
if(PROBE_MODE STREQUAL "empty-client")
    list(REMOVE_ITEM _client_arguments LINKLIB_SOURCES generic_client.c)
    list(APPEND _client_arguments LINKLIB_NAME explicit-alias)
endif()
if(PROBE_MODE STREQUAL "no-validation")
    # No sealed source-selection result is supplied by the metadata double.
elseif(PROBE_MODE STREQUAL "both-archives-disabled")
    list(APPEND _client_arguments NO_NORMAL_CLIENT_ARCHIVE NO_RELATIVE_CLIENT_ARCHIVE)
endif()
if(PROBE_MODE MATCHES "^(datatype|gadget|mcc)$")
    cmake_language(CALL "aros_add_${PROBE_MODE}" ${_client_arguments})
else()
    aros_add_library(${_client_arguments})
endif()

if(PROBE_MODE MATCHES "^(valid|datatype|gadget|mcc|empty-client)$")
    foreach(_target client-only-linklib client-only-linklib-rel
            client-only-includes client-only-genmodfiles
            includes-fixture includes-fixture_rel
            linklibs-fixture linklibs-fixture_rel includes-all)
        if(NOT TARGET "${_target}")
            message(FATAL_ERROR "CLIENTS_ONLY omitted ${_target}")
        endif()
    endforeach()
    foreach(_target client-only client-only-linklib-objects)
        if(TARGET "${_target}")
            message(FATAL_ERROR "CLIENTS_ONLY unexpectedly created ${_target}")
        endif()
    endforeach()
    foreach(_target client-only-linklib client-only-linklib-rel)
        get_target_property(_dependencies "${_target}" MANUALLY_ADDED_DEPENDENCIES)
        if("client-only" IN_LIST _dependencies)
            message(FATAL_ERROR "client archive depends on a runtime target")
        endif()
    endforeach()
    file(WRITE "${CMAKE_BINARY_DIR}/passed" "client archives only")
elseif(PROBE_MODE STREQUAL "source-free")
    foreach(_target client-only-linklib client-only-includes
            client-only-genmodfiles includes-fixture includes-fixture_rel
            linklibs-fixture includes-all)
        if(NOT TARGET "${_target}")
            message(FATAL_ERROR "source-free CLIENTS_ONLY omitted ${_target}")
        endif()
    endforeach()
    foreach(_target client-only client-only-linklib-rel
            client-only-linklib-objects linklibs-fixture_rel)
        if(TARGET "${_target}")
            message(FATAL_ERROR "source-free CLIENTS_ONLY unexpectedly created ${_target}")
        endif()
    endforeach()
    file(WRITE "${CMAKE_BINARY_DIR}/passed" "source-free client archive only")
endif()
"#;

struct Fixture {
    _temp: TempDir,
    project: PathBuf,
    build: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let project = root.join("project");
        let build = root.join("build");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&build).unwrap();
        Self {
            _temp: temp,
            project,
            build,
        }
    }

    fn configure(&self, mode: &str) -> Output {
        let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("engine");
        let source = fs::read_to_string(engine.join("AROS.cmake")).unwrap();
        let aliases = extract_function(&source, "_aros_publish_library_client_aliases");
        let archive_claim = extract_function(&source, "_aros_claim_linklib_archive");
        let builder = extract_function(&source, "aros_add_library");
        let wrappers = ["aros_add_datatype", "aros_add_gadget", "aros_add_mcc"]
            .map(|name| extract_function(&source, name))
            .join("\n");
        let script = FIXTURE
            .replace("@MODE@", mode)
            .replace("@ENGINE@", &engine.display().to_string())
            .replace(
                "@PRODUCTION_FUNCTIONS@",
                &format!("{aliases}\n{archive_claim}\n{builder}\n{wrappers}"),
            );
        fs::write(self.project.join("CMakeLists.txt"), script).unwrap();
        Command::new("cmake")
            .args(["-S"])
            .arg(&self.project)
            .args(["-B"])
            .arg(&self.build)
            .args(["-G", "Ninja"])
            .output()
            .unwrap()
    }
}

fn extract_function(source: &str, name: &str) -> String {
    let marker = format!("function({name}");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {name}"));
    let line_end = source[start..].find('\n').unwrap() + start;
    let tail = &source[line_end + 1..];
    let end = tail
        .find("\nendfunction()")
        .unwrap_or_else(|| panic!("unterminated {name}"))
        + "\nendfunction()".len();
    format!("{}\n{}", &source[start..line_end], &tail[..end])
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn configure_failure(mode: &str, expected: &str) {
    let fixture = Fixture::new();
    let output = fixture.configure(mode);
    assert!(
        !output.status.success(),
        "accepted {mode}: {}",
        output_text(&output)
    );
    assert!(
        output_text(&output).contains(expected),
        "{}",
        output_text(&output)
    );
}

#[test]
fn clients_only_builds_source_and_genmodule_archives_without_runtime_resolution() {
    let fixture = Fixture::new();
    let configure = fixture.configure("valid");
    assert!(configure.status.success(), "{}", output_text(&configure));
    assert!(fixture.build.join("passed").is_file());

    for target in ["client-only-linklib", "client-only-linklib-rel"] {
        let build = Command::new("cmake")
            .arg("--build")
            .arg(&fixture.build)
            .args(["--target", target])
            .output()
            .unwrap();
        assert!(build.status.success(), "{}", output_text(&build));
    }

    let cache = fs::read_to_string(fixture.build.join("CMakeCache.txt")).unwrap();
    let archive_tool = cache
        .lines()
        .find_map(|line| line.strip_prefix("CMAKE_AR:FILEPATH="))
        .expect("CMake did not record its archiver");
    for (archive, expected_stub) in [
        ("libfixture.a", "genmodule_normal_stub.c.o"),
        ("libfixture_rel.a", "genmodule_relative_stub.c.o"),
    ] {
        let result = Command::new(archive_tool)
            .args(["t"])
            .arg(fixture.build.join("lib").join(archive))
            .output()
            .unwrap();
        assert!(result.status.success(), "{}", output_text(&result));
        let members = String::from_utf8_lossy(&result.stdout);
        assert!(
            members.lines().any(|member| member == "generic_client.c.o"),
            "{members}"
        );
        assert!(
            members.lines().any(|member| member == expected_stub),
            "{members}"
        );
        assert_eq!(
            members
                .lines()
                .filter(|member| *member != "__.SYMDEF SORTED")
                .count(),
            2,
            "{members}"
        );
    }
}

#[test]
fn empty_client_lists_keep_phony_endpoints_without_fabricating_archives() {
    let fixture = Fixture::new();
    let configure = fixture.configure("empty-client");
    assert!(configure.status.success(), "{}", output_text(&configure));
    assert!(fixture.build.join("passed").is_file());
    let build = Command::new("cmake")
        .arg("--build")
        .arg(&fixture.build)
        .args(["--target", "client-only-linklib", "client-only-linklib-rel"])
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", output_text(&build));
    assert!(!fixture.build.join("lib").exists());
}

#[test]
fn library_backed_module_wrappers_build_clients_without_runtime_sources() {
    for mode in ["datatype", "gadget", "mcc"] {
        let fixture = Fixture::new();
        let configure = fixture.configure(mode);
        assert!(
            configure.status.success(),
            "{mode}: {}",
            output_text(&configure)
        );
        assert!(fixture.build.join("passed").is_file());
        let build = Command::new("cmake")
            .arg("--build")
            .arg(&fixture.build)
            .args(["--target", "client-only-linklib", "client-only-linklib-rel"])
            .output()
            .unwrap();
        assert!(build.status.success(), "{mode}: {}", output_text(&build));
        assert!(fixture
            .build
            .join(format!("lib/libfixture.{mode}.a"))
            .is_file());
        assert!(fixture
            .build
            .join(format!("lib/libfixture_rel.{mode}.a"))
            .is_file());
        assert!(!fixture.build.join("lib/libfixture.a").exists());
        assert!(!fixture.build.join("lib/libfixture_rel.a").exists());
    }
}

#[test]
fn source_free_genmodule_clients_only_returns_after_its_archive() {
    let fixture = Fixture::new();
    let configure = fixture.configure("source-free");
    assert!(configure.status.success(), "{}", output_text(&configure));
    assert!(fixture.build.join("passed").is_file());

    let build = Command::new("cmake")
        .arg("--build")
        .arg(&fixture.build)
        .args(["--target", "client-only-linklib"])
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", output_text(&build));

    let cache = fs::read_to_string(fixture.build.join("CMakeCache.txt")).unwrap();
    let archive_tool = cache
        .lines()
        .find_map(|line| line.strip_prefix("CMAKE_AR:FILEPATH="))
        .expect("CMake did not record its archiver");
    let result = Command::new(archive_tool)
        .args(["t"])
        .arg(fixture.build.join("lib/libfixture.a"))
        .output()
        .unwrap();
    assert!(result.status.success(), "{}", output_text(&result));
    let members = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        members
            .lines()
            .filter(|member| *member != "__.SYMDEF SORTED")
            .collect::<Vec<_>>(),
        ["genmodule_source_free_stub.c.o"]
    );
}

#[test]
fn clients_only_requires_validated_source_and_at_least_one_archive() {
    configure_failure(
        "no-validation",
        "CLIENTS_ONLY requires a source-selected client archive",
    );
    configure_failure(
        "both-archives-disabled",
        "CLIENTS_ONLY requires a source-selected client archive",
    );
    configure_failure(
        "existing-runtime",
        "CLIENTS_ONLY cannot reuse a runtime target",
    );
}
