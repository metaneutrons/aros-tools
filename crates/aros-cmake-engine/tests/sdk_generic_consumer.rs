//! Closed-response probes for the generic LLVM SDK consumer boundary.
//! Compiler metadata is simulated after `project(NONE)`; these checks do not
//! qualify native compiler binaries or an SDK.
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Probe {
    cpu: &'static str,
    triple: &'static str,
    width: &'static str,
    float_abi: &'static str,
    toolchain: &'static str,
    check_compiler: bool,
    response_edit: &'static str,
    after_validation: &'static str,
    compiler_edit: &'static str,
}

impl Probe {
    const fn new(
        cpu: &'static str,
        triple: &'static str,
        width: &'static str,
        float_abi: &'static str,
    ) -> Self {
        Self {
            cpu,
            triple,
            width,
            float_abi,
            toolchain: "llvm",
            check_compiler: false,
            response_edit: "",
            after_validation: "",
            compiler_edit: "",
        }
    }

    const fn compiler(mut self) -> Self {
        self.check_compiler = true;
        self
    }

    const fn response(mut self, edit: &'static str) -> Self {
        self.response_edit = edit;
        self
    }

    const fn after(mut self, edit: &'static str) -> Self {
        self.after_validation = edit;
        self
    }

    const fn compiler_edit(mut self, edit: &'static str) -> Self {
        self.check_compiler = true;
        self.compiler_edit = edit;
        self
    }

    const fn toolchain(mut self, value: &'static str) -> Self {
        self.toolchain = value;
        self
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    root: PathBuf,
    build: PathBuf,
    engine: PathBuf,
}

struct Outcome {
    output: Output,
    marker: bool,
    selected: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
    binding: Option<Vec<u8>>,
}

const CMAKE_PROBE: &str = r#"
cmake_minimum_required(VERSION 3.22)
project(SdkGenericConsumerProbe NONE)
file(REMOVE "${ROOT}/passed" "${ROOT}/selected-sdk-root")
include("${ENGINE}/NativeConsumerContract.cmake")
include("${ENGINE}/tests/NativeSourceSelectionFixture.cmake")
aros_test_prepare_native_consumer_selection("${ROOT}" "${ROOT}/consumer.json")

set(AROS_TARGET_CPU "@CPU@")
set(AROS_TARGET_TRIPLE "@TRIPLE@")
set(AROS_TARGET_PLATFORM fixture)
set(AROS_TOOLCHAIN "@TOOLCHAIN@")
set(GCC_CONFIG_FLOAT_ABI "@FLOAT_ABI@")
set(AROS_ABI_FLAVOUR fixture-flavour)
set(AROS_ABI_PLATFORM_SMP OFF)
set(AROS_ENABLE_MMU OFF)

file(READ "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json" _response)
string(JSON _legacy_abi GET "${_response}" abi)
string(JSON _response REMOVE "${_response}" abi)
string(JSON _response SET "${_response}" schema "\"aros-sdk-consumer-validation-v1\"")
set(_target "{}")
string(JSON _target SET "${_target}" target_triple "\"${AROS_TARGET_TRIPLE}\"")
set(_abi "{}")
string(JSON _abi SET "${_abi}" source_cpu "\"${AROS_TARGET_CPU}\"")
string(JSON _abi SET "${_abi}" pointer_width @WIDTH@)
string(JSON _abi SET "${_abi}" use_mmu false)
string(JSON _abi SET "${_abi}" flavour "\"${AROS_ABI_FLAVOUR}\"")
string(JSON _abi SET "${_abi}" platform_smp false)
if(NOT GCC_CONFIG_FLOAT_ABI STREQUAL "")
    string(JSON _abi SET "${_abi}" float_abi "\"${GCC_CONFIG_FLOAT_ABI}\"")
endif()
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")
@RESPONSE_EDIT@
if(RECONFIGURE_FAIL)
    string(JSON _response SET "${_response}" qualification "\"changed-proof\"")
endif()
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/native-consumer-validation-response.json" "${_response}\n")

aros_validate_native_consumer_contract()
set(AROS_DEVELOPER_INCLUDE_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/include")
aros_resolve_sdk_include_root(_selected_root)
if(NOT _selected_root STREQUAL AROS_DEVELOPER_INCLUDE_DIR)
    message(FATAL_ERROR "incorrect generic SDK root: ${_selected_root}")
endif()
@AFTER_VALIDATION@

if(@CHECK_COMPILER@)
    # Deliberately synthetic metadata: this exercises only CMake's configured-
    # compiler guard, not Clang, target code generation, or SDK qualification.
    set(CMAKE_C_COMPILER_ID Clang)
    set(CMAKE_CXX_COMPILER_ID Clang)
    set(CMAKE_C_COMPILER_TARGET "${AROS_TARGET_TRIPLE}")
    set(CMAKE_CXX_COMPILER_TARGET "${AROS_TARGET_TRIPLE}")
    set(CMAKE_SIZEOF_VOID_P "@POINTER_BYTES@")
    set(CMAKE_C_FLAGS "")
    set(CMAKE_CXX_FLAGS "")
    set(CMAKE_C_FLAGS_DEBUG "")
    set(CMAKE_C_FLAGS_RELEASE "")
    set(CMAKE_C_FLAGS_RELWITHDEBINFO "")
    set(CMAKE_C_FLAGS_MINSIZEREL "")
    set(CMAKE_CXX_FLAGS_DEBUG "")
    set(CMAKE_CXX_FLAGS_RELEASE "")
    set(CMAKE_CXX_FLAGS_RELWITHDEBINFO "")
    set(CMAKE_CXX_FLAGS_MINSIZEREL "")
    if(AROS_TARGET_CPU STREQUAL "arm")
        set(CMAKE_C_FLAGS "-mfloat-abi=${GCC_CONFIG_FLOAT_ABI}")
        set(CMAKE_CXX_FLAGS "-mfloat-abi=${GCC_CONFIG_FLOAT_ABI}")
    endif()
    @COMPILER_EDIT@
    aros_validate_native_consumer_compiler_target()
endif()
file(WRITE "${ROOT}/selected-sdk-root" "${_selected_root}")
file(WRITE "${ROOT}/passed" "selected and validated")
"#;

fn harness(probe: &Probe) -> Harness {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let build = root.join("build");
    fs::create_dir_all(&build).unwrap();
    fs::write(root.join("one.c"), "fixture-owned source input\n").unwrap();
    let engine = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("engine")
        .canonicalize()
        .unwrap();

    let mut script = CMAKE_PROBE.to_owned();
    for (placeholder, value) in [
        ("@CPU@", probe.cpu),
        ("@TRIPLE@", probe.triple),
        ("@WIDTH@", probe.width),
        ("@FLOAT_ABI@", probe.float_abi),
        ("@TOOLCHAIN@", probe.toolchain),
        (
            "@POINTER_BYTES@",
            if probe.width == "32" { "4" } else { "8" },
        ),
        ("@RESPONSE_EDIT@", probe.response_edit),
        ("@AFTER_VALIDATION@", probe.after_validation),
        ("@COMPILER_EDIT@", probe.compiler_edit),
        (
            "@CHECK_COMPILER@",
            if probe.check_compiler { "ON" } else { "OFF" },
        ),
    ] {
        script = script.replace(placeholder, value);
    }
    fs::write(root.join("CMakeLists.txt"), script).unwrap();
    // A failed configure must remove both artifacts left by an earlier run.
    fs::write(build.join("aros-native-consumer-binding.json"), b"stale\n").unwrap();
    fs::write(root.join("passed"), b"stale\n").unwrap();
    Harness {
        _temp: temp,
        root,
        build,
        engine,
    }
}

fn configure(harness: &Harness, reconfigure_fail: bool) -> Outcome {
    let mut command = Command::new("cmake");
    command
        .arg("-S")
        .arg(&harness.root)
        .arg("-B")
        .arg(&harness.build)
        .arg(format!("-DENGINE={}", harness.engine.display()))
        .arg(format!("-DROOT={}", harness.root.display()));
    if reconfigure_fail {
        command.arg("-DRECONFIGURE_FAIL=ON");
    }
    let output = command.output().unwrap();
    Outcome {
        output,
        marker: harness.root.join("passed").exists(),
        selected: fs::read(harness.root.join("selected-sdk-root")).ok(),
        response: fs::read(
            harness
                .build
                .join("native-consumer-validation-response.json"),
        )
        .ok(),
        binding: fs::read(harness.build.join("aros-native-consumer-binding.json")).ok(),
    }
}

fn diagnostic(outcome: &Outcome) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&outcome.output.stdout),
        String::from_utf8_lossy(&outcome.output.stderr)
    )
}

fn assert_failure(outcome: &Outcome, expected: &str) {
    let error = diagnostic(outcome);
    let normalized_error = error.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized_expected = expected.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        !outcome.output.status.success(),
        "unexpected configure success\n{error}"
    );
    assert!(!outcome.marker, "failure left a pass marker\n{error}");
    assert!(
        outcome.binding.is_none(),
        "failure left a consumer binding\n{error}"
    );
    assert!(
        normalized_error.contains(&normalized_expected),
        "missing {expected:?}\n{error}"
    );
}

fn assert_success(harness: &Harness, outcome: &Outcome) {
    let error = diagnostic(outcome);
    assert!(
        outcome.output.status.success() && outcome.marker,
        "generic SDK configure failed\n{error}"
    );
    let response = outcome.response.as_ref().expect("mock validation response");
    assert_eq!(
        outcome.binding.as_ref(),
        Some(response),
        "deferred binding must preserve exact response bytes"
    );
    let expected_root = format!("{}/SYS/Developer/include", harness.build.display());
    assert_eq!(
        outcome.selected.as_deref(),
        Some(expected_root.as_bytes()),
        "generic SDK consumer must select the source Developer include root"
    );
}

#[test]
fn generic_sdk_targets_bind_and_check_simulated_clang_metadata() {
    for probe in [
        Probe::new("x86_64", "x86_64-unknown-aros", "64", "").compiler(),
        Probe::new("aarch64", "aarch64-unknown-aros", "64", "").compiler(),
        Probe::new("arm", "arm-unknown-aros", "32", "hard").compiler(),
    ] {
        let harness = harness(&probe);
        let outcome = configure(&harness, false);
        assert_success(&harness, &outcome);
    }
}

#[test]
fn source_target_fields_are_closed_typed_and_match_the_requested_selection() {
    let probes = [
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _target SET "${_target}" target_triple "\"other-unknown-aros\"")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "source SDK target differs from the requested configuration",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _abi SET "${_abi}" pointer_width 16)
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "SDK pointer width must be explicit 32 or 64",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _abi SET "${_abi}" pointer_width "\"64\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "executable SDK ABI.pointer_width: must be NUMBER",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _abi SET "${_abi}" use_mmu "\"false\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "executable SDK ABI.use_mmu: must be BOOLEAN",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "").response(
                r#"string(JSON _abi SET "${_abi}" flavour "\"other-flavour\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "source SDK target differs from the requested configuration",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard").response(
                r#"string(JSON _abi SET "${_abi}" float_abi "\"soft\"")
string(JSON _target SET "${_target}" abi "${_abi}")
string(JSON _response SET "${_response}" target "${_target}")"#,
            ),
            "source SDK target differs from the requested configuration",
        ),
    ];
    for (probe, expected) in probes {
        let outcome = configure(&harness(&probe), false);
        assert_failure(&outcome, expected);
    }
}

#[test]
fn legacy_and_generic_response_shapes_cannot_be_mixed() {
    let generic_with_legacy_abi = Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
        .response(r#"string(JSON _response SET "${_response}" abi "${_legacy_abi}")"#);
    let outcome = configure(&harness(&generic_with_legacy_abi), false);
    assert_failure(&outcome, "consumer response: has unknown field 'abi'");

    let legacy_with_generic_target = Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
        .toolchain("gnu")
        .response(r#"string(JSON _response SET "${_response}" schema "\"aros-native-consumer-validation-v1\"")"#);
    let outcome = configure(&harness(&legacy_with_generic_target), false);
    assert_failure(&outcome, "consumer response: has unknown field 'target'");

    let generic_on_gnu = Probe::new("x86_64", "x86_64-unknown-aros", "64", "").toolchain("gnu");
    let outcome = configure(&harness(&generic_on_gnu), false);
    assert_failure(
        &outcome,
        "generic SDK compiler adapter requires the explicit LLVM toolchain",
    );
}

#[test]
fn configured_compiler_guards_reject_mismatches_and_overrides() {
    let cases = [
        (
            Probe::new("aarch64", "aarch64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_CXX_COMPILER_ID GNU)"),
            "CXX driver must be Clang with the exact SDK target triple",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_SIZEOF_VOID_P 4)"),
            "detected compiler pointer width differs from the SDK target",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard")
                .compiler_edit("set(CMAKE_C_FLAGS \"-mfloat-abi=soft\")"),
            "C float ABI flags differ from the selected SDK target",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard")
                .compiler_edit("set(CMAKE_CXX_FLAGS \"-mfloat-abi=hard -mfloat-abi=hard\")"),
            "ARM SDK requires exactly one selected float ABI flag per driver",
        ),
        (
            Probe::new("arm", "arm-unknown-aros", "32", "hard")
                .compiler_edit("set(CMAKE_C_FLAGS_RELEASE \"-mfloat-abi=hard\")"),
            "C float ABI flags differ from the selected SDK target",
        ),
        (
            Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
                .compiler_edit("set(CMAKE_C_FLAGS \"--target=other-unknown-aros\")"),
            "C flags cannot override the selected target or sysroot",
        ),
    ];
    for (probe, expected) in cases {
        let outcome = configure(&harness(&probe), false);
        assert_failure(&outcome, expected);
    }
}

#[test]
fn changed_selection_or_contract_after_validation_invalidates_admission() {
    for (probe, expected) in [
        (Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
            .after("set(AROS_TARGET_CPU aarch64)\naros_resolve_sdk_include_root(_late_root)"),
            "AROS_TARGET_CPU changed after validation"),
        (Probe::new("x86_64", "x86_64-unknown-aros", "64", "")
            .after("file(APPEND \"${AROS_NATIVE_CONSUMER_CONTRACT}\" \"changed\")\naros_resolve_sdk_include_root(_late_root)"),
            "contract changed after validation"),
    ] {
        let outcome = configure(&harness(&probe), false);
        assert_failure(&outcome, expected);
    }
}

#[test]
fn failed_reconfigure_removes_the_previous_binding_and_marker() {
    let probe = Probe::new("x86_64", "x86_64-unknown-aros", "64", "");
    let harness = harness(&probe);
    let first = configure(&harness, false);
    assert_success(&harness, &first);
    let second = configure(&harness, true);
    assert_failure(
        &second,
        "validation response differs from the requested binding",
    );
}
