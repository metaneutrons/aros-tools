//! The source graph owns SoftFloat's private platform header when a native
//! build or SDK consumer contract is selected. The hand-written fallback is
//! retained for ordinary legacy configurations.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

const SOFTFLOAT_FILE: &str = "compiler/softfloat/mmakefile.src";

struct Fixture {
    _temp: TempDir,
    source: PathBuf,
    project: PathBuf,
    build: PathBuf,
    source_hash: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let project = root.join("project");
        let build = root.join("build");
        fs::create_dir_all(source.join("compiler/softfloat")).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&build).unwrap();
        let source_bytes = b"# fixture for the source-owned SoftFloat producer\n";
        fs::write(source.join(SOFTFLOAT_FILE), source_bytes).unwrap();
        let source_hash = aros_common::sha256_bytes(source_bytes).as_str().to_owned();
        Self {
            _temp: temp,
            source,
            project,
            build,
            source_hash,
        }
    }

    fn configure(&self, mode: &str) -> Output {
        let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("engine")
            .canonicalize()
            .unwrap();
        let script = r#"
cmake_minimum_required(VERSION 3.22)
project(SoftFloatAuthority NONE)

function(aros_ilbm_header)
endfunction()

function(aros_copy_includes)
endfunction()

function(aros_generate_defines_header)
    cmake_parse_arguments(HEADER "" "OWNER;OUTPUT" "DEFINES;DEPENDS" ${ARGN})
    if(NOT HEADER_OWNER OR NOT HEADER_OUTPUT)
        message(FATAL_ERROR "invalid legacy SoftFloat fallback fixture")
    endif()
    set_property(GLOBAL PROPERTY AROS_TEST_SOFTFLOAT_FALLBACK TRUE)
    set_property(GLOBAL PROPERTY AROS_TEST_SOFTFLOAT_FALLBACK_OUTPUT "${HEADER_OUTPUT}")
    add_custom_target("${HEADER_OWNER}")
endfunction()

set(AROS_SOURCE_DIR "@SOURCE@")
set(AROS_TARGET_CPU arm)
set(AROS_TARGET_VARIANT "")
set(AROS_SDK_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SDK/include")
set(AROS_GENINC_DIR "${CMAKE_BINARY_DIR}/GENINCDIR")
if(NOT "@MODE@" STREQUAL "legacy")
    set(AROS_NATIVE_CONSUMER_CONTRACT TRUE)
endif()
include("@ENGINE@/GeneratedHeaders.cmake")

if("@MODE@" STREQUAL "legacy")
    if(NOT TARGET linklibs-softfloat-genfiles)
        message(FATAL_ERROR "legacy SoftFloat fallback target was not created")
    endif()
    get_property(_deps GLOBAL PROPERTY AROS_GENERATED_HEADER_DEPS)
    set(_expected "linklibs-softfloat|${CMAKE_BINARY_DIR}/gen/compiler/softfloat/platform.h")
    if(NOT _expected IN_LIST _deps)
        message(FATAL_ERROR "legacy SoftFloat header dependency was not recorded")
    endif()
    file(WRITE "${CMAKE_BINARY_DIR}/passed" "legacy fallback retained")
else()
    include("@ENGINE@/SourceLiteralTextRules.cmake")
    set(_softfloat_output "${CMAKE_BINARY_DIR}/gen/compiler/softfloat/platform.h")
    aros_write_source_literal_text(
        NAME linklibs-softfloat-genfiles
        OUTPUT "${_softfloat_output}"
        FILE compiler/softfloat/mmakefile.src
        FILE_SHA256 @SOURCE_HASH@
        TEXT [==[// Source-owned SoftFloat fixture
#define LITTLEENDIAN 1
]==])
    file(WRITE "${CMAKE_BINARY_DIR}/source-owner-created" "ok")

    if("@MODE@" STREQUAL "duplicate-name")
        aros_write_source_literal_text(
            NAME linklibs-softfloat-genfiles
            OUTPUT "${CMAKE_BINARY_DIR}/gen/compiler/softfloat/other.h"
            FILE compiler/softfloat/mmakefile.src
            FILE_SHA256 @SOURCE_HASH@
            TEXT [==[// duplicate target name
]==])
    elseif("@MODE@" STREQUAL "duplicate-output")
        aros_write_source_literal_text(
            NAME another-softfloat-owner
            OUTPUT "${_softfloat_output}"
            FILE compiler/softfloat/mmakefile.src
            FILE_SHA256 @SOURCE_HASH@
            TEXT [==[// duplicate output owner
]==])
    endif()

    if(NOT TARGET linklibs-softfloat-genfiles)
        message(FATAL_ERROR "source-owned SoftFloat target was not created")
    endif()
    get_property(_fallback GLOBAL PROPERTY AROS_TEST_SOFTFLOAT_FALLBACK)
    if(_fallback)
        message(FATAL_ERROR "legacy SoftFloat fallback ran under a source contract")
    endif()
    get_property(_deps GLOBAL PROPERTY AROS_GENERATED_HEADER_DEPS)
    set(_fallback_dependency "linklibs-softfloat|${_softfloat_output}")
    if(_fallback_dependency IN_LIST _deps)
        message(FATAL_ERROR "legacy SoftFloat dependency ran under a source contract")
    endif()
    file(WRITE "${CMAKE_BINARY_DIR}/passed" "source owner selected")
endif()
"#;
        let script = script
            .replace("@SOURCE@", &self.source.display().to_string())
            .replace("@ENGINE@", &engine.display().to_string())
            .replace("@MODE@", mode)
            .replace("@SOURCE_HASH@", &self.source_hash);
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

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn source_declared_sdk_producer_owns_the_softfloat_target_and_output() {
    let fixture = Fixture::new();
    let output = fixture.configure("source");
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(fixture.build.join("passed").is_file());
}

#[test]
fn native_softfloat_producer_name_and_output_duplicate_guards_remain_active() {
    for (mode, expected) in [
        ("duplicate-name", "NAME must be a unique safe target"),
        ("duplicate-output", "literal text output already belongs to"),
    ] {
        let fixture = Fixture::new();
        let output = fixture.configure(mode);
        assert!(
            !output.status.success(),
            "accepted {mode}: {}",
            output_text(&output)
        );
        assert!(
            output_text(&output).contains(expected),
            "expected {expected:?}: {}",
            output_text(&output)
        );
        assert!(
            fixture.build.join("source-owner-created").is_file(),
            "{mode} guard ran before the first source owner was accepted: {}",
            output_text(&output)
        );
    }
}

#[test]
fn legacy_full_build_keeps_the_hard_coded_softfloat_fallback() {
    let fixture = Fixture::new();
    let output = fixture.configure("legacy");
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(fixture.build.join("passed").is_file());
}
