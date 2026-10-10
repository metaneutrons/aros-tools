//! Tests for the embedded engine and its placement.

#[cfg(unix)]
#[path = "local_gnu_tests.rs"]
mod local_gnu_tests;

#[cfg(unix)]
#[path = "toolchain_identity_tests.rs"]
mod toolchain_identity_tests;

#[test]
fn sdk_text_first_match_preserves_gnu_sed_semantics_and_source_binding() {
    let directory = tempfile::tempdir().expect("SDK text differential fixture");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("exact embedded engine");
    let output = Command::new("cmake")
        .arg("-P")
        .arg(engine.join("tests/SdkTextFirstMatchTest.cmake"))
        .output()
        .expect("run SDK text differential fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

use super::{api_version, digest, file, file_count, materialize, paths, STAMP_FILE};
use std::fmt::Write as _;
use std::fs;
use std::process::Command;

#[test]
fn mesa26_runtime_publication_respects_verified_sdk_consumer_authority() {
    let directory = tempfile::tempdir().expect("Mesa26 runtime authority fixture");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("exact embedded engine");
    let output = Command::new("cmake")
        .arg("-P")
        .arg(engine.join("tests/Mesa26RuntimeTest.cmake"))
        .output()
        .expect("run Mesa26 runtime authority fixture");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn source_archives_preserve_order_and_refuse_unowned_members() {
    let directory = tempfile::tempdir().expect("source archive fixture");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize current engine");
    let output = Command::new("cmake")
        .arg("-P")
        .arg(engine.join("tests/SourceArchivesTest.cmake"))
        .output()
        .expect("run source archive fixture");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sdk_asset_rules_bind_exact_producers_and_reject_unsafe_publication() {
    let directory = tempfile::tempdir().expect("SDK asset fixture");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize current engine");
    let output = Command::new("cmake")
        .arg("-P")
        .arg(engine.join("tests/SdkAssetRulesTest.cmake"))
        .output()
        .expect("run SDK asset fixture");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sealed_host_c_file_generator_preserves_native_input_and_output_contracts() {
    let directory = tempfile::tempdir().expect("host-C generator fixture");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize current engine");
    let output = Command::new("cmake")
        .arg("-P")
        .arg(engine.join("tests/HostCFileGeneratorTest.cmake"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

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

    let directory = tempfile::tempdir().expect("version contract directory");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("exact embedded version implementation");
    let script = directory.path().join("require-version.cmake");
    let run = |required: u32| {
        fs::write(
            &script,
            format!(
                "include(\"{}/EngineVersion.cmake\")\naros_require_engine_api_version({required})\n",
                engine.display()
            ),
        )
        .expect("version probe script");
        Command::new("cmake")
            .arg("-P")
            .arg(&script)
            .output()
            .expect("version contract probe")
    };
    assert!(run(expected).status.success());
    let old = run(expected.saturating_sub(1));
    assert!(!old.status.success(), "older generated graph was accepted");
    assert!(String::from_utf8_lossy(&old.stderr).contains("Regenerate the graph"));
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
fn cold_source_preparation_never_substitutes_for_the_full_graph_export() {
    let cmake = file("CMakeLists.txt").expect("engine CMakeLists.txt");
    assert_eq!(cmake.matches("\"--source-inventory-only\"").count(), 1);
    let prepare = cmake.find("\"--source-inventory-only\"").unwrap();
    let fetch = cmake.find("aros_fetch_source_inventory(").unwrap();
    let full = cmake[fetch..].find("execute_process(").unwrap() + fetch;
    let include = cmake
        .find("include(\"${GENERATED_TARGETS_CMAKE}\")")
        .unwrap();
    assert!(prepare < fetch && fetch < full && full < include);
    // Both native selections require a complete graph export, including warm
    // trees without fetches. Neither may mistake inventory for a usable graph.
    assert!(cmake[fetch..full].contains("endforeach()\nendif()\nif(AROS_NATIVE_BUILD_CONTRACT OR AROS_NATIVE_CONSUMER_CONTRACT OR\n   AROS_SOURCE_INVENTORY_FETCH_COUNT GREATER 0)"));
    assert!(cmake[..prepare].ends_with(
        "if(AROS_NATIVE_BUILD_CONTRACT OR AROS_NATIVE_CONSUMER_CONTRACT)\n    list(APPEND _aros_inventory_prepare_args "
    ));
    assert!(cmake[prepare..fetch]
        .contains("NOT AROS_NATIVE_BUILD_CONTRACT AND NOT AROS_NATIVE_CONSUMER_CONTRACT AND\n    NOT EXISTS \"${GENERATED_TARGETS_CMAKE}\""));
    assert!(cmake[full..include]
        .contains("NOT TRANSPILER_RES EQUAL 0 OR NOT EXISTS \"${GENERATED_TARGETS_CMAKE}\""));
    assert!(cmake[full..include].contains("Fetched source inventories remain unresolved"));
}

#[test]
fn the_digest_is_a_sha256() {
    assert_eq!(digest().len(), 64);
    assert!(digest().bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn reference_genmodule_applies_and_tracks_declared_overrides() {
    let cmake = file("AROS.cmake").expect("module implementation");
    assert!(cmake.contains("function(aros_set_module_config_override mmake override)"));
    assert!(cmake.contains("list(APPEND _opts -o \"${_override}\")"));
    assert!(cmake.contains("list(APPEND _config_inputs \"${_override}\")"));
    assert!(cmake.contains("DEPENDS \"${AROS_HOST_GENMODULE}\" ${_config_inputs}"));
}

#[test]
fn cmake_media_receipt_measures_real_staged_files_and_rejects_changed_inputs() {
    use aros_common::media_profile::built_in_media_profiles;
    use aros_common::media_receipt::{parse_media_build_receipt, verify_media_build_receipt};

    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("embedded engine");
    let script = engine.join("scripts/EmitMediaBuildReceipt.cmake");
    let root = directory.path().join("payload with spaces");
    fs::create_dir_all(root.join("EFI/BOOT")).expect("boot dir");
    fs::create_dir_all(root.join("EFI/AROS")).expect("AROS dir");
    for (path, bytes) in [
        ("EFI/BOOT/BOOTRISCV64.EFI", b"loader".as_slice()),
        ("EFI/AROS/Image", b"image".as_slice()),
        ("aros-bsp.pkg", b"BSP".as_slice()),
        ("aros.cmd", b"command".as_slice()),
        ("startup.nsh", b"startup".as_slice()),
    ] {
        fs::write(root.join(path), bytes).expect("staged file");
    }
    let specs = "uefi-loader|EFI/BOOT/BOOTRISCV64.EFI;kernel-image|EFI/AROS/Image;bsp-package|aros-bsp.pkg;command-line|aros.cmd;startup-script|startup.nsh";
    let invoke = |mode: &str, selected_specs: &str| {
        Command::new("cmake")
            .arg(format!("-DROOT_DIR={}", root.display()))
            .arg("-DTARGET_PRESET=opensbi-riscv64")
            .arg("-DMODEL=milk-v-titan")
            .arg("-DTRANSPORT=uefi-esp")
            .arg(format!("-DFILE_SPECS={selected_specs}"))
            .arg(format!("-DMODE={mode}"))
            .arg("-P")
            .arg(&script)
            .output()
            .expect("CMake script must run")
    };
    let written = invoke("write", specs);
    assert!(
        written.status.success(),
        "CMake receipt failed: {}",
        String::from_utf8_lossy(&written.stderr)
    );
    let receipt_path = root.join("media-build-receipt.json");
    let receipt_bytes = fs::read(&receipt_path).expect("receipt");
    let receipt = parse_media_build_receipt(&receipt_bytes).expect("closed JSON receipt");
    let profile = built_in_media_profiles()
        .expect("reviewed profiles")
        .into_iter()
        .find(|item| item.profile.id == "milk-v-titan-uefi")
        .expect("Titan profile");
    verify_media_build_receipt(&root, &receipt, &profile.profile)
        .expect("CMake-produced receipt matches files and profile");
    assert!(invoke("verify", specs).status.success());

    fs::write(root.join("aros.cmd"), b"changed").expect("changed input");
    assert!(!invoke("verify", specs).status.success());
    assert_eq!(
        fs::read(&receipt_path).expect("preserved receipt"),
        receipt_bytes
    );
    assert!(!invoke("write", "bsp-package|../outside").status.success());
    assert!(
        !invoke("write", "bsp-package|aros-bsp.pkg;bsp-package|aros.cmd")
            .status
            .success()
    );
    assert_eq!(
        fs::read(&receipt_path).expect("preserved receipt"),
        receipt_bytes
    );

    let fixture_source = directory.path().join("cmake-fixture");
    let fixture_build = directory.path().join("cmake-build");
    fs::create_dir(&fixture_source).expect("fixture source");
    fs::write(
        fixture_source.join("CMakeLists.txt"),
        format!(
            r#"cmake_minimum_required(VERSION 3.22)
project(media_receipt_fixture NONE)
set(_specs "{specs}")
add_custom_target(media-receipt
    COMMAND "${{CMAKE_COMMAND}}"
        "-DROOT_DIR={root}"
        "-DTARGET_PRESET=opensbi-riscv64"
        "-DMODEL=milk-v-titan"
        "-DTRANSPORT=uefi-esp"
        "-DFILE_SPECS=${{_specs}}"
        "-DMODE=write" -P "{script}"
    VERBATIM)
"#,
            root = cmake_path(&root),
            script = cmake_path(&script)
        ),
    )
    .expect("CMake fixture");
    let configured = Command::new("cmake")
        .arg("-S")
        .arg(&fixture_source)
        .arg("-B")
        .arg(&fixture_build)
        .arg("-G")
        .arg("Ninja")
        .output()
        .expect("configure fixture");
    assert!(
        configured.status.success(),
        "configure failed: {}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let built = Command::new("cmake")
        .arg("--build")
        .arg(&fixture_build)
        .arg("--target")
        .arg("media-receipt")
        .output()
        .expect("run generated build command");
    assert!(
        built.status.success(),
        "generated command failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    parse_media_build_receipt(&fs::read(&receipt_path).expect("generated receipt"))
        .expect("generated receipt schema");

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        use std::time::{Duration, Instant};

        symlink(root.join("EFI"), root.join("linked")).expect("linked parent");
        assert!(!invoke("write", "uefi-loader|linked/BOOT/BOOTRISCV64.EFI")
            .status
            .success());
        assert!(!invoke(
            "write",
            "first-role|EFI/BOOT/BOOTRISCV64.EFI;second-role|efi/boot/bootriscv64.efi"
        )
        .status
        .success());

        assert!(Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .expect("mkfifo")
            .success());
        let mut child = Command::new("cmake")
            .arg(format!("-DROOT_DIR={}", root.display()))
            .arg("-DTARGET_PRESET=opensbi-riscv64")
            .arg("-DMODEL=milk-v-titan")
            .arg("-DTRANSPORT=uefi-esp")
            .arg("-DFILE_SPECS=bsp-package|pipe")
            .arg("-DMODE=write")
            .arg("-P")
            .arg(&script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("CMake FIFO probe");
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = child.try_wait().expect("probe status") {
                assert!(!status.success(), "FIFO must be rejected");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().expect("stop blocked probe");
                child.wait().expect("reap blocked probe");
                panic!("CMake blocked while measuring a FIFO");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    let graph = file("OpenSbiUefi.cmake").expect("Titan graph");
    assert!(graph.contains("-DMODE=write\" -P \"${_opensbi_receipt_script}"));
    assert!(graph.contains("-DMODE=verify\" -P \"${_opensbi_receipt_script}"));
}

#[allow(clippy::literal_string_with_formatting_args)] // Git's ^{tree} syntax is literal.
#[test]
fn cmake_media_receipt_binds_clean_git_and_release_toolchain_inputs() {
    use aros_common::media_receipt::parse_media_build_receipt;

    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("embedded engine");
    let source = directory.path().join("source");
    let toolchain = directory.path().join("toolchain");
    let payload = directory.path().join("payload");
    for path in [&source, &toolchain, &payload] {
        fs::create_dir(path).unwrap();
    }
    let git = |args: &[&str]| {
        let result = Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q"]);
    fs::write(source.join("source.txt"), b"source").unwrap();
    git(&["add", "source.txt"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "source",
    ]);
    let source_commit = git(&["rev-parse", "HEAD"]);
    let source_tree = git(&["rev-parse", "HEAD^{tree}"]);
    let tree_sha256 = "a".repeat(64);
    let manifest = format!("{{\"release_id\":\"test-1\",\"tree_sha256\":\"{tree_sha256}\"}}");
    fs::write(toolchain.join("toolchain-manifest.json"), manifest).unwrap();
    fs::write(payload.join("kernel"), b"kernel bytes").unwrap();
    let invoke = |mode: &str| {
        Command::new("cmake")
            .arg(format!("-DROOT_DIR={}", payload.display()))
            .arg("-DTARGET_PRESET=pc-x86_64")
            .arg("-DMODEL=pc")
            .arg("-DTRANSPORT=bios-iso")
            .arg("-DFILE_SPECS=bootstrap|kernel")
            .arg(format!("-DSOURCE_DIR={}", source.display()))
            .arg(format!("-DTOOLCHAIN_ROOT={}", toolchain.display()))
            .arg(format!("-DMODE={mode}"))
            .arg("-P")
            .arg(engine.join("scripts/EmitMediaBuildReceipt.cmake"))
            .output()
            .unwrap()
    };
    let written = invoke("write");
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    let receipt =
        parse_media_build_receipt(&fs::read(payload.join("media-build-receipt.json")).unwrap())
            .unwrap();
    assert_eq!(receipt.format_version, 2);
    let identity = receipt.build_identity.unwrap();
    assert_eq!(identity.source_commit, source_commit);
    assert_eq!(identity.source_tree, source_tree);
    assert_eq!(identity.toolchain_tree_sha256.as_str(), tree_sha256);
    assert!(invoke("verify").status.success());
    fs::write(source.join("source.txt"), b"dirty").unwrap();
    assert!(!invoke("verify").status.success());
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

#[test]
fn pc_bootstrap_multiboot_header_must_be_inside_first_eight_kib() {
    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize engine");
    let verifier = engine.join("scripts/VerifyPcBootstrap.cmake");
    let image = directory.path().join("bootstrap");

    for (offset, valid) in [(4096, true), (8180, true), (8192, false), (4097, false)] {
        let mut bytes = vec![0; (offset + 64).max(8192)];
        bytes[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 1, 1]);
        bytes[offset..offset + 12].copy_from_slice(&[
            0x02, 0xb0, 0xad, 0x1b, // Multiboot-1 magic
            0x03, 0x00, 0x00, 0x00, // flags
            0xfb, 0x4f, 0x52, 0xe4, // checksum
        ]);
        fs::write(&image, bytes).expect("write ELF fixture");
        let output = Command::new("cmake")
            .arg(format!("-DBOOTSTRAP_ELF={}", image.display()))
            .args(["-P", verifier.to_str().expect("UTF-8 verifier path")])
            .output()
            .expect("run bootstrap verifier");
        assert_eq!(
            output.status.success(),
            valid,
            "unexpected validation result at offset {offset}:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let mut invalid_checksum = vec![0; 8192];
    invalid_checksum[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 1, 1]);
    invalid_checksum[4096..4108].copy_from_slice(&[
        0x02, 0xb0, 0xad, 0x1b, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]);
    fs::write(&image, invalid_checksum).expect("write invalid checksum fixture");
    let output = Command::new("cmake")
        .arg(format!("-DBOOTSTRAP_ELF={}", image.display()))
        .args(["-P", verifier.to_str().expect("UTF-8 verifier path")])
        .output()
        .expect("run bootstrap verifier");
    assert!(!output.status.success(), "invalid checksum was accepted");
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
fn developer_library_file_copies_preserve_bytes_and_refuse_changed_contracts() {
    run_embedded_kobj_contract_test("DeveloperLibCopyTest.cmake");
}

#[test]
fn sdk_fd_file_copies_keep_the_existing_local_and_fetch_contracts() {
    run_embedded_kobj_contract_test("SdkFileCopiesTest.cmake");
}

#[test]
fn developer_bin_and_manual_copies_preserve_the_closed_copy_contract() {
    run_embedded_kobj_contract_test("DeveloperAssetCopiesTest.cmake");
}

#[test]
fn empty_header_copy_requires_explicit_proof_and_preserves_arch_selection() {
    run_embedded_kobj_contract_test("EmptyHeaderCopyTest.cmake");
}

#[test]
fn generated_header_inputs_require_exact_producers_and_safe_runtime_paths() {
    run_embedded_kobj_contract_test("GeneratedHeaderInputTest.cmake");
}

#[test]
fn source_declared_sdk_objects_compile_stage_and_refuse_unsafe_outputs() {
    run_embedded_kobj_contract_test("SdkObjectsTest.cmake");
}

#[test]
fn runtime_header_namespace_precedence_is_verified_by_real_compiles() {
    let directory = tempfile::tempdir().expect("runtime header namespace fixture");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize current engine");
    let output = Command::new("cmake")
        .arg("-DAROS_TEST_TOOLCHAIN=llvm")
        .args(["-P"])
        .arg(engine.join("tests/RuntimeHeaderNamespaceTest.cmake"))
        .output()
        .expect("run runtime header namespace fixture");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn literal_objects_preserve_command_order_and_refuse_unsafe_outputs() {
    run_embedded_kobj_contract_test("LiteralObjectsTest.cmake");
}

#[test]
fn demos_images_are_generated_before_their_consumer_compiles() {
    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize engine");
    let source = directory.path().join("source");
    let images = source.join("developer/demos/images");
    fs::create_dir_all(&images).expect("images source directory");
    fs::write(
        images.join("mmakefile"),
        "IMAGES := ArrowUp ArrowDown ArrowLeft ArrowRight ImageButton\n\
         demos-images-setup : $(IMAGEFILES)\n",
    )
    .expect("reviewed Make rule");
    fs::write(
        images.join("datfilt.awk"),
        "FNR == 1 { name = FILENAME; sub(/.*\\//, \"\", name); sub(/\\..*$/, \"\", name); print \"#define IMAGE_READY 1\" > (name \".h\") }\n",
    )
    .expect("fixture image generator");
    let mut consumer = String::new();
    for stem in [
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "ImageButton",
    ] {
        for variant in [0, 1] {
            let name = format!("{stem}{variant}");
            fs::write(images.join(format!("{name}.dat")), "X\n").expect("image data");
            writeln!(consumer, "#include \"images/{name}.h\"").expect("image include");
        }
    }
    consumer.push_str("int main(void) { return IMAGE_READY - 1; }\n");
    fs::write(source.join("developer/demos/demowin.c"), consumer).expect("image consumer");
    let project = directory.path().join("project");
    fs::create_dir(&project).expect("fixture project");
    fs::write(
        project.join("CMakeLists.txt"),
        format!(
            "cmake_minimum_required(VERSION 3.22)\n\
             project(DemosImages C)\n\
             set(AROS_SOURCE_DIR \"{}\")\n\
             add_executable(demos-demowin \"${{AROS_SOURCE_DIR}}/developer/demos/demowin.c\")\n\
             include(\"{}/DemosImages.cmake\")\n\
             aros_add_demos_images()\n",
            cmake_path(&source),
            cmake_path(&engine),
        ),
    )
    .expect("fixture CMake project");
    let build = directory.path().join("build");
    let configure = Command::new("cmake")
        .args(["-G", "Ninja", "-S"])
        .arg(&project)
        .arg("-B")
        .arg(&build)
        .output()
        .expect("configure image fixture");
    assert!(
        configure.status.success(),
        "image fixture configure failed: {}",
        String::from_utf8_lossy(&configure.stderr)
    );
    let compile = Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .args(["--target", "demos-demowin", "--parallel", "8"])
        .output()
        .expect("build image fixture");
    assert!(
        compile.status.success(),
        "image consumer raced its headers: {}{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    assert!(
        build.join("developer/demos/images/ArrowUp0.h").is_file(),
        "image header was not generated"
    );

    fs::write(images.join("mmakefile"), "IMAGES := ArrowUp\n").expect("mutated Make rule");
    let rejected = Command::new("cmake")
        .args(["-G", "Ninja", "-S"])
        .arg(&project)
        .arg("-B")
        .arg(directory.path().join("rejected"))
        .output()
        .expect("configure altered image fixture");
    assert!(
        !rejected.status.success(),
        "changed image rule was accepted"
    );
    assert!(String::from_utf8_lossy(&rejected.stderr)
        .contains("demos image inventory differs from the reviewed Make rule"));
}

#[cfg(unix)]
#[test]
fn package_rebuild_replaces_only_after_private_creation_and_inspection() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temp dir");
    let engine = directory.path().join("engine");
    materialize(&engine).expect("materialize engine");
    let script = engine.join("BuildPackage.cmake");
    let romtool = directory.path().join("romtool-stub");
    fs::write(
        &romtool,
        "#!/bin/sh\ncase \"$1 $2\" in\n\
         'pkg create') [ \"$3\" = --basename ] && [ \"$4\" = -o ] || exit 2; \
         [ \"$AROS_TEST_PKG_FAIL\" = 0 ] || exit 19; /bin/cp \"$6\" \"$5\" ;;\n\
         'pkg list') test -s \"$3\" ;;\n\
         *) exit 2 ;;\nesac\n",
    )
    .expect("stub romtool");
    fs::set_permissions(&romtool, fs::Permissions::from_mode(0o755)).expect("executable stub");
    let member = directory.path().join("member.elf");
    let output = directory.path().join("package.pkg");
    let run = |fail: bool| {
        Command::new("cmake")
            .arg(format!("-DPACKAGE_OUTPUT={}", output.display()))
            .arg(format!("-DPACKAGE_ROMTOOL={}", romtool.display()))
            .arg("-P")
            .arg(&script)
            .arg("--")
            .arg(&member)
            .env("AROS_TEST_PKG_FAIL", if fail { "1" } else { "0" })
            .output()
            .expect("run package publication")
    };
    fs::write(&member, b"original").expect("first member");
    assert!(run(false).status.success());
    assert_eq!(fs::read(&output).unwrap(), b"original");
    fs::write(&member, b"replacement").expect("changed member");
    assert!(
        !run(true).status.success(),
        "failed producer published output"
    );
    assert_eq!(fs::read(&output).unwrap(), b"original");
    assert!(
        run(false).status.success(),
        "incremental replacement failed"
    );
    assert_eq!(fs::read(&output).unwrap(), b"replacement");
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
set(AROS_CROSS_TOOLCHAIN_ROOT "${{CMAKE_BINARY_DIR}}/toolchain")
set(AROS_XORRISO_BIN /usr/bin/true)
set(AROS_MEDIA_CLI_BIN /usr/bin/true)
add_custom_target(aros-grub2-iso-assets)
add_custom_target(AROS)
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
    let iso_rule = ninja
        .lines()
        .find(|line| line.starts_with("build CMakeFiles/boot-iso |"))
        .expect("ISO custom command");
    for output in [
        "SYS/boot/pc/bootstrap",
        "SYS/boot/pc/kernel",
        "SYS/boot/pc/aros-bsp.pkg",
        "SYS/boot/aros-base.pkg",
    ] {
        assert!(
            iso_rule.contains(output),
            "ISO target does not depend on generated output {output}"
        );
    }
    for required in [
        "boot/grub/i386-pc/grub2_eltorito",
        "ComposePcBootIso.cmake",
        "aros-grub2-iso-assets",
        "image receipt --profile pc-bios-iso",
        "-DCPU_SIGNATURE=x86_64",
        "sys-tree=gen/boot-iso/stage",
    ] {
        assert!(ninja.contains(required), "missing ISO contract: {required}");
    }

    let graph = Command::new("ninja")
        .args([
            "-C",
            build.to_str().expect("UTF-8 build path"),
            "-t",
            "query",
            "boot-iso",
        ])
        .output()
        .expect("query boot-iso graph");
    assert!(graph.status.success(), "cannot query boot-iso graph");
    let graph = String::from_utf8(graph.stdout).expect("UTF-8 Ninja graph");
    assert!(
        graph.contains("    AROS\n"),
        "native SYS producer missing: {graph}"
    );
    assert!(
        graph.contains("    aros-grub2-iso-assets\n"),
        "audited GRUB producer missing: {graph}"
    );

    // The fixture's empty AROS/GRUB producers stand in for the complete
    // native targets; this packaging counterprobe supplies their SYS output.
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
    let private_grub = build.join("gen/grub2-iso-assets/x86_64/pc");
    fs::create_dir_all(&private_grub).expect("private GRUB directory");
    fs::write(private_grub.join("grub2_eltorito"), "fixture\n").expect("private GRUB image");
    fs::write(
        build.join("gen/grub2-iso-assets/x86_64/.grub2-iso-assets.stamp"),
        format!(
            "GRUB2 ISO assets: 2.16 x86_64\nEl Torito SHA256: {}\n",
            aros_common::sha256_bytes(b"fixture\n")
        ),
    )
    .expect("GRUB asset stamp");
    fs::remove_file(sys_boot.join("pc/kernel")).expect("remove required kernel");
    let missing_kernel = Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .args(["--target", "boot-iso"])
        .output()
        .expect("reject missing kernel");
    assert!(
        !missing_kernel.status.success(),
        "missing kernel was accepted"
    );
    let missing_kernel_output = format!(
        "{}{}",
        String::from_utf8_lossy(&missing_kernel.stdout),
        String::from_utf8_lossy(&missing_kernel.stderr)
    );
    assert!(
        missing_kernel_output.contains("SYS/boot/pc/kernel"),
        "missing kernel failure lacked its exact path: {missing_kernel_output}"
    );
    assert!(!build.join("aros-x86_64-pc.iso").exists());
    fs::write(sys_boot.join("pc/kernel"), "fixture\n").expect("restore kernel");
    fs::write(sys_boot.join("grub/i386-pc/grub2_eltorito"), "changed\n")
        .expect("tamper with GRUB output");
    let changed_grub = Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .args(["--target", "boot-iso"])
        .output()
        .expect("reject altered GRUB output");
    assert!(
        !changed_grub.status.success(),
        "altered GRUB input was accepted"
    );
    assert!(
        String::from_utf8_lossy(&changed_grub.stdout)
            .contains("GRUB input changed after audited staging"),
        "altered GRUB input lacked a precise diagnosis:\n{}",
        String::from_utf8_lossy(&changed_grub.stdout)
    );
    assert!(!build.join("aros-x86_64-pc.iso").exists());
    fs::write(sys_boot.join("grub/i386-pc/grub2_eltorito"), "fixture\n")
        .expect("restore GRUB output");
    fs::write(build.join("SYS/AROS.boot"), "arm\n").expect("wrong boot signature");
    let mismatched_signature = Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .args(["--target", "boot-iso"])
        .output()
        .expect("reject wrong boot signature");
    assert!(!mismatched_signature.status.success());
    assert!(String::from_utf8_lossy(&mismatched_signature.stdout).contains("mismatched AROS.boot"));
    fs::remove_file(build.join("SYS/AROS.boot")).expect("remove wrong signature");
    fs::write(build.join("media-build-receipt.json"), "{}\n")
        .expect("fake receipt for composer counterprobe");
    let output = Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .args(["--target", "boot-iso"])
        .output()
        .expect("assemble fixture");
    assert!(
        !output.status.success(),
        "fake media composer unexpectedly passed"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("artifact omits a regular file"),
        "missing verified artifact was not diagnosed:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        fs::read_to_string(build.join("gen/boot-iso/stage/boot/grub/grub.cfg"))
            .expect("staged config"),
        config
    );
    assert_eq!(
        fs::read_to_string(build.join("gen/boot-iso/stage/AROS.boot"))
            .expect("staged boot signature"),
        "x86_64\n"
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

#[test]
fn source_module_kobj_inputs_preserve_known_and_unknown_scope() {
    run_embedded_kobj_contract_test("ModuleKobjInputsTest.cmake");
}

#[test]
fn module_header_projection_never_creates_runtime_or_archive_targets() {
    run_embedded_kobj_contract_test("ModuleHeadersOnlyTest.cmake");
}

#[test]
fn sfd_headers_bind_sources_and_reject_mutation_without_output_loss() {
    run_embedded_kobj_contract_test("SfdHeadersTest.cmake");
}

#[test]
fn source_values_match_gnu_sed_and_reject_changed_or_unsafe_inputs() {
    run_embedded_kobj_contract_test("SourceValueRuleTest.cmake");
}

#[test]
fn native_kobj_rejects_changed_inputs_and_failed_publication() {
    run_embedded_kobj_contract_test("NativeKobjTest.cmake");
}

/// `PATH` with the workspace's built executables first.
///
/// An engine configure that finds `ld.lld` also requires `aros-collect`. The
/// test executable sits in `target/<profile>/deps`; the tools the workspace
/// test run built sit one level up. Without this, such a host fails these
/// fixtures only because the suite is not installed.
fn path_with_workspace_tools() -> std::ffi::OsString {
    let mut directories = Vec::new();
    if let Some(tools) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent().map(std::path::Path::to_path_buf))
    {
        directories.push(tools);
    }
    directories.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(directories).expect("PATH entries without separators")
}

fn run_embedded_kobj_contract_test(script: &str) {
    let directory = tempfile::tempdir().expect("fresh KOBJ test directory");
    let engine = directory.path().join("engine");
    let build = directory.path().join("build");
    materialize(&engine).expect("materialize exact embedded engine");
    let output = Command::new("cmake")
        .current_dir(directory.path())
        .env("PATH", path_with_workspace_tools())
        .arg(format!("-DENGINE_DIR={}", engine.display()))
        .arg(format!("-DTEST_BINARY_DIR={}", build.display()))
        .arg("-P")
        .arg(engine.join("tests").join(script))
        .output()
        .expect("run embedded KOBJ contract test");
    assert!(
        output.status.success(),
        "{script} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
