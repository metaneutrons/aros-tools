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
fn transpiler_invocations_carry_explicit_upstream_selectors() {
    let cmake = file("CMakeLists.txt").expect("engine CMakeLists.txt");
    for selector in [
        "AROS_MESA_VERSION",
        "AROS_TARGET_LLVM_VER",
        "AROS_TARGET_LLVM_RUNTIMES_STYLE",
        "AROS_TARGET_RUST",
        "AROS_TARGET_RUST_VER",
    ] {
        assert!(
            cmake.contains(&format!("set({selector} \"\" CACHE STRING")),
            "missing cache selector {selector}"
        );
    }
    for flag in [
        "--mesa-version",
        "--target-llvm-ver",
        "--target-llvm-runtimes-style",
        "--target-rust",
        "--target-rust-ver",
    ] {
        assert_eq!(
            cmake.matches(&format!("\"{flag}\"")).count(),
            3,
            "{flag} must reach both transpiler passes and invocation recording"
        );
    }
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

#[test]
fn pc_boot_iso_uses_eltorito_and_the_source_module_order() {
    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize engine");
    let source = directory.path().join("aros-source");
    fs::create_dir_all(source.join("arch/x86_64-pc/boot")).expect("module directory");
    fs::create_dir_all(source.join("workbench/s")).expect("startup directory");
    fs::write(
        source.join("arch/x86_64-pc/boot/modules.default"),
        "/boot/@arch.dir@/kernel.@pkg.fmt@\n/boot/@arch.dir@/aros-bsp.pkg.@pkg.fmt@\n/boot/aros-base.pkg.@pkg.fmt@\n",
    )
    .expect("module list");
    fs::write(source.join("workbench/s/Startup-Sequence"), "EndCLI\n").expect("startup sequence");
    let fixture = directory.path().join("fixture");
    fs::create_dir(&fixture).expect("fixture directory");
    fs::write(
        fixture.join("CMakeLists.txt"),
        format!(
            r#"cmake_minimum_required(VERSION 3.22)
project(BootIsoContract NONE)
set(AROS_SOURCE_DIR "{}")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_BOOT_ISO "${{CMAKE_BINARY_DIR}}/aros-x86_64-pc.iso")
set(AROS_MKISOFS_BIN /usr/bin/true)
add_custom_target(aros-grub2-iso-assets)
add_custom_target(workbench-c)
include("{}/PcBootIso.cmake")
aros_add_pc_boot_iso()
"#,
            cmake_path(&source),
            cmake_path(&engine),
        ),
    )
    .expect("fixture CMakeLists");
    let build = directory.path().join("build");
    let output = Command::new("cmake")
        .args(["-G", "Ninja", "-S"])
        .arg(&fixture)
        .arg("-B")
        .arg(&build)
        .output()
        .expect("configure fixture");
    assert!(
        output.status.success(),
        "boot-iso configure failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(build.join("gen/boot-iso/grub.cfg")).expect("GRUB config");
    assert!(config.contains("insmod multiboot2"));
    assert!(config.contains("multiboot2 /boot/pc/bootstrap vesa=800x600x32"));
    let kernel = config
        .find("module2 /boot/pc/kernel")
        .expect("kernel module");
    let bsp = config
        .find("module2 /boot/pc/aros-bsp.pkg")
        .expect("BSP module");
    let base = config
        .find("module2 /boot/aros-base.pkg")
        .expect("base module");
    assert!(kernel < bsp && bsp < base, "source module order changed");
    assert!(!config.contains('@'), "unexpanded module placeholder");
    let ninja = fs::read_to_string(build.join("build.ninja")).expect("Ninja graph");
    for required in [
        "boot/grub/i386-pc/grub2_eltorito",
        "-no-emul-boot",
        "-boot-info-table",
        "aros-grub2-iso-assets",
    ] {
        assert!(ninja.contains(required), "missing ISO contract: {required}");
    }

    // A completed SYS tree is an input, not a dependency that re-runs the
    // package producer (which deliberately refuses to overwrite packages).
    let sys_boot = build.join("SYS/boot");
    fs::create_dir_all(sys_boot.join("pc")).expect("PC boot directory");
    fs::create_dir_all(sys_boot.join("grub/i386-pc")).expect("GRUB boot directory");
    for relative in [
        "pc/bootstrap",
        "pc/kernel",
        "pc/aros-bsp.pkg",
        "aros-base.pkg",
        "grub/i386-pc/grub2_eltorito",
    ] {
        fs::write(sys_boot.join(relative), "fixture\n").expect("SYS input");
    }
    let output = Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .args(["--target", "boot-iso"])
        .output()
        .expect("assemble fixture");
    assert!(
        !output.status.success(),
        "fake ISO composer unexpectedly passed"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("did not produce a regular ISO"),
        "missing final ISO was not diagnosed:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        fs::read_to_string(build.join("gen/boot-iso/stage/boot/grub/grub.cfg"))
            .expect("staged config"),
        config
    );
    assert!(build
        .join("gen/boot-iso/stage/S/Startup-Sequence")
        .is_file());
    assert!(!sys_boot.join("grub/grub.cfg").exists());
    assert!(!build.join("SYS/S/Startup-Sequence").exists());

    #[cfg(unix)]
    {
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).expect("outside directory");
        std::os::unix::fs::symlink(&outside, build.join("SYS/S")).expect("unsafe SYS destination");
        let output = Command::new("cmake")
            .arg("--build")
            .arg(&build)
            .args(["--target", "boot-iso"])
            .output()
            .expect("reject unsafe destination");
        assert!(!output.status.success(), "symlinked SYS/S was accepted");
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("unsafe SYS directory"),
            "symlink rejection was not diagnosed:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(!outside.join("Startup-Sequence").exists());
    }
}

#[test]
fn pc_boot_iso_verifier_rejects_data_iso_and_accepts_boot_catalog() {
    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize engine");
    let verify = engine.join("VerifyPcBootIso.cmake");
    let iso = directory.path().join("boot.iso");
    let mut bytes = vec![0_u8; 131_072];
    fs::write(&iso, &bytes).expect("data ISO fixture");
    let run = || {
        Command::new("cmake")
            .arg(format!("-DISO_PATH={}", iso.display()))
            .arg("-P")
            .arg(&verify)
            .output()
            .expect("run ISO verifier")
    };
    assert!(!run().status.success(), "data-only ISO was accepted");

    let record = 17 * 2048;
    bytes[record..record + 7].copy_from_slice(b"\0CD001\x01");
    bytes[record + 7..record + 30].copy_from_slice(b"EL TORITO SPECIFICATION");
    bytes[record + 71..record + 75].copy_from_slice(&20_u32.to_le_bytes());
    let catalog = 20 * 2048;
    bytes[catalog] = 1;
    bytes[catalog + 30..catalog + 32].copy_from_slice(&[0x55, 0xaa]);
    bytes[catalog + 32..catalog + 34].copy_from_slice(&[0x88, 0]);
    fs::write(&iso, &bytes).expect("boot ISO without image fixture");
    assert!(
        !run().status.success(),
        "catalog without boot image was accepted"
    );
    bytes[catalog + 38..catalog + 40].copy_from_slice(&4_u16.to_le_bytes());
    bytes[catalog + 40..catalog + 44].copy_from_slice(&40_u32.to_le_bytes());
    fs::write(&iso, &bytes).expect("empty image fixture");
    assert!(!run().status.success(), "empty boot image was accepted");
    bytes[40 * 2048] = 0xe8;
    fs::write(&iso, bytes).expect("boot ISO fixture");
    let output = run();
    assert!(
        output.status.success(),
        "valid boot catalog was rejected:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
