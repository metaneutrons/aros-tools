#![cfg(unix)]

use std::{
    env,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

struct Fixture {
    _temporary: TempDir,
    source: PathBuf,
    build: PathBuf,
    header: PathBuf,
    c_object: PathBuf,
    cxx_object: PathBuf,
    ordered: bool,
}

impl Fixture {
    fn new(ordered: bool) -> Self {
        let temporary = tempfile::tempdir().expect("create literal-object fixture root");
        let root = fs::canonicalize(temporary.path()).expect("canonicalize fixture root");
        let source = root.join("source");
        let build = root.join("build");
        fs::create_dir_all(source.join("compiler/startup"))
            .expect("create fixture source and startup directories");
        fs::create_dir_all(source.join("fixture-modules"))
            .expect("create fixture CMake module directory");
        fs::create_dir_all(&build).expect("create fixture build directory");

        fs::write(
            source.join("archive_c.c"),
            "#include <generated/archive_header.h>\nint archive_c(void) { return ARCHIVE_HEADER_VALUE; }\n",
        )
        .expect("write C archive source");
        fs::write(
            source.join("archive_cpp.cpp"),
            "#include <generated/archive_header.h>\nint archive_cpp() { return ARCHIVE_HEADER_VALUE; }\n",
        )
        .expect("write C++ archive source");
        fs::write(
            source.join("compiler/startup/startup.c"),
            "int fixture_startup;\n",
        )
        .expect("write required startup fixture source");
        fs::write(
            source.join("compiler/startup/detach.c"),
            "int fixture_detach;\n",
        )
        .expect("write required detach fixture source");
        fs::write(
            source.join("fixture-modules/BootstrapSDK.cmake"),
            "function(aros_bootstrap_sdk_includes)\nendfunction()\n",
        )
        .expect("write isolated SDK bootstrap stub");
        fs::write(
            source.join("prepare-header.cmake"),
            r##"if(NOT DEFINED OUTPUT OR "${OUTPUT}" STREQUAL "")
    message(FATAL_ERROR "header output is required")
endif()
execute_process(COMMAND "${CMAKE_COMMAND}" -E sleep 2 RESULT_VARIABLE sleep_result)
if(NOT sleep_result STREQUAL "0")
    message(FATAL_ERROR "bounded header delay failed: ${sleep_result}")
endif()
get_filename_component(output_directory "${OUTPUT}" DIRECTORY)
file(MAKE_DIRECTORY "${output_directory}")
file(WRITE "${OUTPUT}" "#define ARCHIVE_HEADER_VALUE 73\n")
"##,
        )
        .expect("write delayed header preparation rule");

        let engine =
            fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engine/AROS.cmake"))
                .expect("canonicalize AROS.cmake");
        let project = r#"cmake_minimum_required(VERSION 3.22)
project(OrderedArchiveHeaderFixture LANGUAGES C CXX)
file(REAL_PATH "${CMAKE_C_COMPILER}" _fixture_real_c_compiler)
file(REAL_PATH "${CMAKE_CXX_COMPILER}" _fixture_real_cxx_compiler)
set(CMAKE_C_COMPILER "${_fixture_real_c_compiler}")
set(CMAKE_CXX_COMPILER "${_fixture_real_cxx_compiler}")
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_LLD_BIN FALSE)
set(CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/fixture-modules")
include(@AROS_ENGINE_CMAKE@)
file(WRITE "${CMAKE_BINARY_DIR}/fixture-roots.txt"
    "${AROS_SOURCE_DIR}\n${AROS_BUILD_DIR}\n${AROS_SDK_INCLUDE_DIR}\n${CMAKE_C_COMPILER}\n${CMAKE_CXX_COMPILER}\n")

set(_fixture_header "${AROS_SDK_INCLUDE_DIR}/generated/archive_header.h")
add_custom_command(
    OUTPUT "${_fixture_header}"
    COMMAND "${CMAKE_COMMAND}" "-DOUTPUT=${_fixture_header}"
        -P "${CMAKE_CURRENT_SOURCE_DIR}/prepare-header.cmake"
    DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/prepare-header.cmake"
    VERBATIM)
add_custom_target(header-preparation DEPENDS "${_fixture_header}")

set(_fixture_c_source "${AROS_SOURCE_DIR}/archive_c.c")
set(_fixture_cxx_source "${AROS_SOURCE_DIR}/archive_cpp.cpp")
set(_fixture_c_object "${AROS_BUILD_DIR}/gen/archive_c.o")
set(_fixture_cxx_object "${AROS_BUILD_DIR}/gen/archive_cpp.o")
aros_compile_literal_object(
    NAME archive-c SOURCE "${_fixture_c_source}"
    OUTPUT "${_fixture_c_object}" LANGUAGE C
    ARGUMENTS -I "${AROS_SDK_INCLUDE_DIR}")
aros_compile_literal_object(
    NAME archive-cxx SOURCE "${_fixture_cxx_source}"
    OUTPUT "${_fixture_cxx_object}" LANGUAGE CXX
    ARGUMENTS -I "${AROS_SDK_INCLUDE_DIR}")
aros_literal_object_group(
    NAME archive-group
    OBJECTS "${_fixture_c_object}" "${_fixture_cxx_object}")

add_custom_target(archive-wrapper)
add_dependencies(archive-wrapper archive-group header-preparation)
if(ORDERED_PROBE)
    aros_add_target_dependency(archive-group header-preparation)
endif()
"#
            .replace("@AROS_ENGINE_CMAKE@", &cmake_quote(&engine));
        fs::write(source.join("CMakeLists.txt"), project).expect("write fixture CMake project");

        let source = fs::canonicalize(source).expect("canonicalize fixture source root");
        let build = fs::canonicalize(build).expect("canonicalize fixture build root");
        let header = build.join("SDK/include/generated/archive_header.h");
        let c_object = build.join("gen/archive_c.o");
        let cxx_object = build.join("gen/archive_cpp.o");

        Self {
            _temporary: temporary,
            source,
            build,
            header,
            c_object,
            cxx_object,
            ordered,
        }
    }

    fn configure(&self, c_compiler: &Path, cxx_compiler: &Path) -> Output {
        Command::new("cmake")
            .arg("-S")
            .arg(&self.source)
            .arg("-B")
            .arg(&self.build)
            .arg("-G")
            .arg("Ninja")
            .arg(format!("-DCMAKE_C_COMPILER={}", c_compiler.display()))
            .arg(format!("-DCMAKE_CXX_COMPILER={}", cxx_compiler.display()))
            .arg(format!(
                "-DORDERED_PROBE={}",
                if self.ordered { "ON" } else { "OFF" }
            ))
            .output()
            .expect("run fresh Ninja CMake configure")
    }

    fn build_group(&self) -> Output {
        Command::new("cmake")
            .arg("--build")
            .arg(&self.build)
            .arg("--target")
            .arg("archive-group")
            .arg("--parallel")
            .arg("12")
            .arg("--verbose")
            .output()
            .expect("build fresh Ninja literal-object group with parallelism 12")
    }

    fn assert_recorded_roots(&self, c_compiler: &Path, cxx_compiler: &Path) {
        let roots = fs::read_to_string(self.build.join("fixture-roots.txt"))
            .expect("read exact fixture roots");
        let roots: Vec<_> = roots.lines().collect();
        assert_eq!(
            roots.len(),
            5,
            "fixture must record source, build, SDK, and drivers"
        );
        assert_eq!(roots[0], self.source.to_string_lossy());
        assert_eq!(roots[1], self.build.to_string_lossy());
        assert_eq!(roots[2], self.build.join("SDK/include").to_string_lossy());
        assert_eq!(roots[3], c_compiler.to_string_lossy());
        assert_eq!(roots[4], cxx_compiler.to_string_lossy());
    }
}

#[test]
fn literal_object_group_dependency_orders_c_and_cxx_after_header_preparation() {
    let c_compiler = host_compiler("CC", &["clang", "cc", "gcc"]);
    let cxx_compiler = host_compiler("CXX", &["clang++", "c++", "g++"]);

    let ordered = Fixture::new(true);
    let configure = ordered.configure(&c_compiler, &cxx_compiler);
    assert!(
        configure.status.success(),
        "ordered fixture configure failed:\n{}",
        output_text(&configure)
    );
    ordered.assert_recorded_roots(&c_compiler, &cxx_compiler);
    assert!(
        !ordered.header.exists(),
        "generated header must be absent before the ordered build"
    );

    let build = ordered.build_group();
    assert!(
        build.status.success(),
        "ordered literal-object build failed:\n{}",
        output_text(&build)
    );
    assert!(
        ordered.header.is_file(),
        "header preparation did not publish its output"
    );
    assert_nonempty_file(&ordered.c_object, "C object");
    assert_nonempty_file(&ordered.cxx_object, "C++ object");

    // The counterprobe changes only whether aros_add_target_dependency adds
    // the group-to-preparation edge. archive-wrapper keeps both as siblings.
    let unordered = Fixture::new(false);
    let configure = unordered.configure(&c_compiler, &cxx_compiler);
    assert!(
        configure.status.success(),
        "counterprobe configure failed:\n{}",
        output_text(&configure)
    );
    unordered.assert_recorded_roots(&c_compiler, &cxx_compiler);
    assert!(
        !unordered.header.exists(),
        "generated header must be absent before the counterprobe build"
    );

    let build = unordered.build_group();
    let diagnostics = output_text(&build);
    assert!(
        !build.status.success(),
        "counterprobe unexpectedly built without the group-to-preparation edge:\n{diagnostics}"
    );
    assert!(
        diagnostics.contains("generated/archive_header.h"),
        "counterprobe did not report the missing generated header:\n{diagnostics}"
    );
    let lower = diagnostics.to_ascii_lowercase();
    assert!(
        lower.contains("file not found") || lower.contains("no such file or directory"),
        "counterprobe did not capture the compiler's missing-header diagnostic:\n{diagnostics}"
    );
    assert!(
        diagnostics.contains("archive_c.c") || diagnostics.contains("archive_cpp.cpp"),
        "counterprobe failure did not identify a real C or C++ source compile:\n{diagnostics}"
    );
    assert!(
        !unordered.header.exists(),
        "counterprobe group build must not run its sibling header-preparation target"
    );
    assert!(
        !unordered.c_object.exists() && !unordered.cxx_object.exists(),
        "counterprobe must not publish an object after either compile failed"
    );
}

fn host_compiler(variable: &str, fallbacks: &[&str]) -> PathBuf {
    let mut names = Vec::<OsString>::new();
    if let Some(configured) = env::var_os(variable) {
        names.push(configured);
    }
    names.extend(fallbacks.iter().map(OsString::from));

    let search_path = env::var_os("PATH").unwrap_or_default();
    for name in names {
        let name_path = PathBuf::from(&name);
        let candidates: Vec<PathBuf> =
            if name_path.is_absolute() || name_path.components().count() > 1 {
                vec![name_path]
            } else {
                env::split_paths(&search_path)
                    .map(|directory| directory.join(&name))
                    .collect()
            };
        for candidate in candidates {
            let Ok(metadata) = fs::metadata(&candidate) else {
                continue;
            };
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
            return fs::canonicalize(candidate).expect("canonicalize host compiler");
        }
    }

    panic!("no executable host compiler found for {variable}");
}

fn assert_nonempty_file(path: &Path, description: &str) {
    let metadata =
        fs::metadata(path).unwrap_or_else(|_| panic!("missing {description}: {}", path.display()));
    assert!(
        metadata.is_file(),
        "{description} is not a regular file: {}",
        path.display()
    );
    assert!(
        metadata.len() > 0,
        "{description} is empty: {}",
        path.display()
    );
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn cmake_quote(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    format!("\"{}\"", normalized.replace('"', "\\\""))
}
