//! Tests for the embedded engine and its placement.

use super::{api_version, digest, file, file_count, materialize, paths, STAMP_FILE};
use std::fs;
use std::process::Command;

#[test]
fn the_engine_is_embedded_whole() {
    assert!(file_count() > 100, "only {} files embedded", file_count());
    // Three files that anchor the three kinds of content: the entry point, the
    // largest module and the version declaration the contract rests on.
    assert!(file("CMakeLists.txt").is_some());
    assert!(file("AROS.cmake").is_some());
    assert!(file("EngineVersion.cmake").is_some());
    assert!(file("no/such/file.cmake").is_none());
}

#[test]
fn paths_are_sorted_and_relative() {
    let all: Vec<_> = paths().collect();
    let mut sorted = all.clone();
    sorted.sort_unstable();
    assert_eq!(all, sorted, "the embedded table is not sorted");
    for path in all {
        assert!(!path.starts_with('/'), "{path} is absolute");
        assert!(!path.contains(".."), "{path} escapes the engine root");
    }
}

#[test]
fn the_api_version_comes_from_the_engine() {
    let declared = file("EngineVersion.cmake").expect("version file");
    let expected = declared
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("set(AROS_CMAKE_ENGINE_API_VERSION")?
                .trim_start()
                .strip_suffix(')')?
                .trim()
                .parse::<u32>()
                .ok()
        })
        .expect("a version in the engine file");
    assert_eq!(api_version(), expected);
}

#[test]
fn the_digest_is_a_sha256() {
    assert_eq!(digest().len(), 64);
    assert!(digest().bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn placement_writes_every_file_and_stamps_it() {
    let directory = tempfile::tempdir().expect("temp dir");
    let placement = materialize(directory.path()).expect("materialize");

    assert!(!placement.reused);
    assert_eq!(placement.written, file_count());
    assert_eq!(placement.digest, digest());
    for path in paths() {
        assert!(directory.path().join(path).is_file(), "{path} missing");
    }
    let stamp = fs::read_to_string(directory.path().join(STAMP_FILE)).expect("stamp");
    assert_eq!(stamp.trim(), digest());
}

#[test]
fn a_second_placement_rewrites_nothing() {
    let directory = tempfile::tempdir().expect("temp dir");
    materialize(directory.path()).expect("first");
    let again = materialize(directory.path()).expect("second");

    assert!(again.reused, "the second call rewrote the engine");
    assert_eq!(again.written, 0);
}

#[test]
fn a_stale_module_is_removed() {
    // The reason this matters: an engine module left behind by an earlier
    // version is still visible to `include()`, so a directory that merely
    // contains the current engine is not the same as one that holds only it.
    let directory = tempfile::tempdir().expect("temp dir");
    materialize(directory.path()).expect("first");

    let stale = directory.path().join("RemovedInThisVersion.cmake");
    fs::write(&stale, "message(FATAL_ERROR \"stale\")\n").expect("write stale");
    fs::remove_file(directory.path().join(STAMP_FILE)).expect("drop stamp");

    let placement = materialize(directory.path()).expect("second");
    assert!(!stale.exists(), "the stale module survived");
    assert_eq!(placement.removed, 1);
}

#[test]
fn a_missing_file_defeats_a_matching_stamp() {
    // The stamp is a claim about the directory, not proof of it.
    let directory = tempfile::tempdir().expect("temp dir");
    materialize(directory.path()).expect("first");
    fs::remove_file(directory.path().join("AROS.cmake")).expect("remove a module");

    let placement = materialize(directory.path()).expect("second");
    assert!(
        !placement.reused,
        "a missing module was reported as present"
    );
    assert!(directory.path().join("AROS.cmake").is_file());
}

#[test]
fn nested_directories_survive_placement() {
    let directory = tempfile::tempdir().expect("temp dir");
    materialize(directory.path()).expect("materialize");
    assert!(directory.path().join("tests").is_dir());
    assert!(directory.path().join("toolchains/AROS.cmake").is_file());
}

#[test]
fn compiler_cache_module_uses_the_frontend_selection_and_clears_stale_launchers() {
    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize");
    let module = engine.join("CompilerCache.cmake");
    let launcher = directory.path().join("frontend-selected-ccache");
    fs::write(&launcher, "fixture launcher\n").expect("launcher");

    let selected_script = directory.path().join("selected.cmake");
    fs::write(
        &selected_script,
        format!(
            r#"
set(AROS_COMPILER_CACHE_MODE "ccache")
set(AROS_COMPILER_CACHE_EXECUTABLE "{}")
include("{}")
foreach(_aros_language C CXX)
    if(NOT "${{CMAKE_${{_aros_language}}_COMPILER_LAUNCHER}}" STREQUAL "{}")
        message(FATAL_ERROR "${{_aros_language}} launcher did not preserve the frontend selection")
    endif()
endforeach()
if(DEFINED CMAKE_ASM_COMPILER_LAUNCHER)
    message(FATAL_ERROR "ASM must not claim unsupported CMake launcher support")
endif()
"#,
            cmake_path(&launcher),
            cmake_path(&module),
            cmake_path(&launcher),
        ),
    )
    .expect("selected script");
    run_cmake_script(&selected_script);

    let disabled_script = directory.path().join("disabled.cmake");
    fs::write(
        &disabled_script,
        format!(
            r#"
foreach(_aros_language C CXX ASM)
    set(CMAKE_${{_aros_language}}_COMPILER_LAUNCHER "stale" CACHE FILEPATH "fixture" FORCE)
    set(CMAKE_${{_aros_language}}_COMPILER_LAUNCHER "stale")
endforeach()
set(AROS_COMPILER_CACHE_EXECUTABLE "stale" CACHE FILEPATH "fixture" FORCE)
set(AROS_COMPILER_CACHE_EXECUTABLE "stale")
set(AROS_COMPILER_CACHE_MODE "off")
include("{}")
foreach(_aros_language C CXX ASM)
    if(DEFINED CMAKE_${{_aros_language}}_COMPILER_LAUNCHER)
        message(FATAL_ERROR "${{_aros_language}} launcher survived frontend off selection")
    endif()
    get_property(_aros_has_cache CACHE CMAKE_${{_aros_language}}_COMPILER_LAUNCHER PROPERTY TYPE SET)
    if(_aros_has_cache)
        message(FATAL_ERROR "${{_aros_language}} launcher cache entry survived frontend off selection")
    endif()
endforeach()
if(DEFINED AROS_COMPILER_CACHE_EXECUTABLE)
    message(FATAL_ERROR "stale frontend compiler-cache executable survived off selection")
endif()
"#,
            cmake_path(&module),
        ),
    )
    .expect("disabled script");
    run_cmake_script(&disabled_script);

    let nested_options =
        fs::read_to_string(engine.join("AROS.cmake")).expect("materialized nested-build module");
    assert!(
        nested_options.contains("-DAROS_COMPILER_CACHE_MODE=${AROS_COMPILER_CACHE_MODE}"),
        "nested CMake arguments must preserve the frontend cache policy"
    );
    assert!(
        nested_options
            .contains("-DAROS_COMPILER_CACHE_EXECUTABLE=${AROS_COMPILER_CACHE_EXECUTABLE}"),
        "nested CMake arguments must preserve the frontend-selected executable"
    );
}

/// The module is included after `project()` in the embedded engine.  This
/// fixture exercises that exact order and verifies that one frontend-selected
/// launcher is used for every compiled language, rather than merely checking
/// CMake variables in script mode.
#[cfg(unix)]
#[test]
fn compiler_cache_launcher_reaches_c_cxx_and_asm_build_rules() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize");
    let module = engine.join("CompilerCache.cmake");
    let source = directory.path().join("source");
    let build = directory.path().join("build");
    fs::create_dir(&source).expect("fixture source directory");

    let launcher = directory.path().join("frontend-selected-launcher");
    fs::write(
        &launcher,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$AROS_TEST_LAUNCHER_LOG\"\nexec \"$@\"\n",
    )
    .expect("launcher");
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).expect("launcher mode");
    let launcher_log = directory.path().join("launcher.log");

    fs::write(
        source.join("CMakeLists.txt"),
        r#"
cmake_minimum_required(VERSION 3.22)
project(compiler_cache_fixture LANGUAGES C CXX ASM)
include("${AROS_COMPILER_CACHE_MODULE}")
add_library(cache_c STATIC c.c)
add_library(cache_cxx STATIC cxx.cpp)
add_library(cache_asm STATIC asm.S)
"#,
    )
    .expect("CMakeLists");
    fs::write(
        source.join("c.c"),
        "int cache_fixture_c(void) { return 0; }\n",
    )
    .expect("C source");
    fs::write(
        source.join("cxx.cpp"),
        "extern \"C\" int cache_fixture_cxx(void) { return 0; }\n",
    )
    .expect("C++ source");
    // A `.S` source is preprocessed by CMake's ASM language rule; a bare
    // `.text` directive is valid for the native assemblers on both supported
    // host families without making the fixture architecture-specific.
    fs::write(source.join("asm.S"), ".text\n").expect("ASM source");

    let configure = Command::new("cmake")
        .args(["-S", source.to_str().expect("UTF-8 source path")])
        .args(["-B", build.to_str().expect("UTF-8 build path")])
        .arg(format!(
            "-DAROS_COMPILER_CACHE_MODULE={}",
            cmake_path(&module)
        ))
        .arg("-DAROS_COMPILER_CACHE_MODE=ccache")
        .arg(format!(
            "-DAROS_COMPILER_CACHE_EXECUTABLE={}",
            cmake_path(&launcher)
        ))
        .env("AROS_TEST_LAUNCHER_LOG", &launcher_log)
        .output()
        .expect("CMake must configure the compiler-cache fixture");
    assert!(
        configure.status.success(),
        "CMake configure failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&configure.stdout),
        String::from_utf8_lossy(&configure.stderr),
    );

    let compile = Command::new("cmake")
        .args(["--build", build.to_str().expect("UTF-8 build path")])
        .env("AROS_TEST_LAUNCHER_LOG", &launcher_log)
        .output()
        .expect("CMake must build the compiler-cache fixture");
    assert!(
        compile.status.success(),
        "CMake build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let invocations = fs::read_to_string(&launcher_log).expect("launcher invocation log");
    for source_name in ["c.c", "cxx.cpp"] {
        assert!(
            invocations.contains(source_name),
            "frontend-selected launcher did not receive {source_name}; invocations:\n{invocations}"
        );
    }
    assert!(
        !invocations.contains("asm.S"),
        "CMake must not claim unsupported ASM launcher coverage; invocations:\n{invocations}"
    );
}

fn cmake_path(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn run_cmake_script(script: &std::path::Path) {
    let output = Command::new("cmake")
        .args(["-P", script.to_str().expect("UTF-8 script path")])
        .output()
        .expect("CMake must be available for the embedded engine contract");
    assert!(
        output.status.success(),
        "CMake script {} failed:\nstdout:\n{}\nstderr:\n{}",
        script.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
