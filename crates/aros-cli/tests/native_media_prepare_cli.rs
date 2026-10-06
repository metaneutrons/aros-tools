//! Public CLI parser and closed-schema failure boundaries for image preparation.

#![cfg(unix)]

use aros_common::sha256_bytes;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

struct Fixture {
    temporary: TempDir,
    inputs: PathBuf,
    work_parent: PathBuf,
    sentinel: PathBuf,
    sentinel_bytes: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let inputs = temporary.path().join("inputs.json");
        fs::write(&inputs, b"{}").unwrap();
        let work_parent = temporary.path().join("private-work-parent");
        fs::create_dir(&work_parent).unwrap();
        fs::set_permissions(&work_parent, fs::Permissions::from_mode(0o700)).unwrap();
        let sentinel = work_parent.join("keep.txt");
        let sentinel_bytes = b"user-owned sentinel\n".to_vec();
        fs::write(&sentinel, &sentinel_bytes).unwrap();
        Self {
            temporary,
            inputs,
            work_parent,
            sentinel,
            sentinel_bytes,
        }
    }

    fn base_arguments(&self) -> Vec<String> {
        vec![
            "image".into(),
            "prepare".into(),
            "--preset".into(),
            "test-preset".into(),
            "--source-root".into(),
            self.temporary.path().join("source").display().to_string(),
            "--inputs".into(),
            self.inputs.display().to_string(),
            "--inputs-sha256".into(),
            "0".repeat(64),
            "--work-parent".into(),
            self.work_parent.display().to_string(),
            "--jobs".into(),
            "2".into(),
            "--timeout-seconds".into(),
            "30".into(),
        ]
    }

    fn invoke(&self, arguments: &[String]) -> Output {
        Command::new(aros())
            .current_dir(self.temporary.path())
            .args(arguments)
            .output()
            .expect("public aros binary executes")
    }

    fn assert_parent_untouched(&self) {
        assert_eq!(fs::read(&self.sentinel).unwrap(), self.sentinel_bytes);
        let children = fs::read_dir(&self.work_parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(children, vec![self.sentinel.file_name().unwrap()]);
    }
}

fn rejected(output: &Output, case: &str) -> String {
    assert!(
        !output.status.success(),
        "{case} unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn replace_value(arguments: &mut [String], option: &str, value: &str) {
    let index = arguments
        .iter()
        .position(|argument| argument == option)
        .expect("base CLI arguments include requested option");
    arguments[index + 1] = value.into();
}

fn typed_inputs(root: &Path) -> Value {
    let path = |name: &str| root.join(name);
    let source = |name: &str| {
        json!({
            "destination": path(&format!("prepared/{name}")),
            "archive": name,
            "suffix": "tar.xz",
            "sha256": "d".repeat(64),
            "patches": []
        })
    };
    json!({
        "schema_version": 1,
        "format": "aros-native-media-inputs-v1",
        "qualification": "local-byte-lock-only",
        "provider": "esp-idf-bootloader-v1",
        "target_profiles_sha256": "a".repeat(64),
        "native_contract_sha256": "b".repeat(64),
        "wheel_lock": { "path": path("wheel-lock.json"), "sha256": "c".repeat(64) },
        "idf_lock": { "path": path("idf-lock.json"), "sha256": "d".repeat(64) },
        "interpreter": path("python/bin/python3"),
        "runtime_prefix": path("python"),
        "wheel_cache": path("wheel-cache"),
        "idf_source": source("esp-idf"),
        "compiler_source": source("riscv32-esp-elf"),
        "cmake_source": source("cmake"),
        "ninja_source": source("ninja"),
        "idf_root": path("idf"),
        "compiler_root": path("compiler"),
        "cmake_root": path("cmake-root"),
        "cmake": path("cmake-root/bin/cmake"),
        "ninja": path("ninja-root/bin/ninja"),
        "git": path("git/bin/git"),
        "constraints": path("constraints.txt")
    })
}

fn assert_file_hash(path: &Value, expected_sha256: &Value) -> Vec<u8> {
    let path = Path::new(path.as_str().expect("receipt path is a string"));
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(
        sha256_bytes(&bytes).to_string(),
        expected_sha256
            .as_str()
            .expect("receipt SHA-256 is a string"),
        "file digest differs: {}",
        path.display()
    );
    bytes
}

fn assert_producer_phases_succeeded(receipt: &Value, producer: &str) {
    let phases = receipt["phases"]
        .as_array()
        .unwrap_or_else(|| panic!("{producer} receipt has no phase array"));
    assert!(
        !phases.is_empty(),
        "{producer} receipt has no producer phases"
    );
    for phase in phases {
        assert_eq!(
            phase["exit_code"].as_i64(),
            Some(0),
            "{producer} phase failed: {phase}"
        );
        assert!(
            !phase["timed_out"]
                .as_bool()
                .expect("phase has timeout status"),
            "{producer} phase timed out: {phase}"
        );
        assert!(
            !phase["cancelled"]
                .as_bool()
                .expect("phase has cancellation status"),
            "{producer} phase was cancelled: {phase}"
        );
    }
}

#[test]
fn binary_parser_requires_all_mandatory_fields() {
    let fixture = Fixture::new();
    let output = fixture.invoke(&["image".into(), "prepare".into()]);
    let stderr = rejected(&output, "missing image prepare arguments");
    for option in [
        "--preset",
        "--source-root",
        "--inputs",
        "--inputs-sha256",
        "--work-parent",
        "--jobs",
        "--timeout-seconds",
    ] {
        assert!(
            stderr.contains(option),
            "missing required {option}: {stderr}"
        );
    }
    fixture.assert_parent_untouched();
}

#[test]
fn binary_parser_rejects_nonpositive_budgets_and_malformed_hashes() {
    let fixture = Fixture::new();
    for (option, value) in [
        ("--jobs", "0"),
        ("--timeout-seconds", "0"),
        ("--inputs-sha256", "not-a-sha256"),
    ] {
        let mut arguments = fixture.base_arguments();
        replace_value(&mut arguments, option, value);
        rejected(&fixture.invoke(&arguments), option);
        fixture.assert_parent_untouched();
    }
}

#[test]
fn closed_schema_and_raw_hash_failures_preserve_the_private_parent() {
    let fixture = Fixture::new();
    let mut wrong_schema = typed_inputs(fixture.temporary.path());
    wrong_schema["schema_version"] = json!(2);
    let raw = serde_json::to_vec(&wrong_schema).unwrap();
    fs::write(&fixture.inputs, &raw).unwrap();

    let mut arguments = fixture.base_arguments();
    replace_value(
        &mut arguments,
        "--inputs-sha256",
        &sha256_bytes(&raw).to_string(),
    );
    rejected(
        &fixture.invoke(&arguments),
        "unsupported native media schema",
    );
    fixture.assert_parent_untouched();

    let mut unknown = typed_inputs(fixture.temporary.path());
    unknown["implicit_defaults"] = json!(true);
    let raw = serde_json::to_vec(&unknown).unwrap();
    fs::write(&fixture.inputs, &raw).unwrap();
    replace_value(
        &mut arguments,
        "--inputs-sha256",
        &sha256_bytes(&raw).to_string(),
    );
    rejected(&fixture.invoke(&arguments), "unknown closed-schema field");
    fixture.assert_parent_untouched();

    replace_value(&mut arguments, "--inputs-sha256", &"0".repeat(64));
    rejected(&fixture.invoke(&arguments), "raw input hash mismatch");
    fixture.assert_parent_untouched();
}

#[test]
#[ignore = "requires reviewed local ESP-IDF media inputs and external producer tools"]
fn public_prepare_process_emits_a_bound_external_media_receipt() {
    let inputs = PathBuf::from(
        env::var("AROS_TEST_NATIVE_MEDIA_INPUTS")
            .expect("set AROS_TEST_NATIVE_MEDIA_INPUTS to the reviewed inputs file"),
    );
    let inputs_sha256 = env::var("AROS_TEST_NATIVE_MEDIA_INPUTS_SHA256")
        .expect("set AROS_TEST_NATIVE_MEDIA_INPUTS_SHA256 to its reviewed raw SHA-256");
    let source_root = PathBuf::from(
        env::var("AROS_TEST_P4_SOURCE").expect("set AROS_TEST_P4_SOURCE to the P4 source tree"),
    );
    let work_parent = PathBuf::from(
        env::var("AROS_TEST_NATIVE_MEDIA_WORK_PARENT")
            .expect("set AROS_TEST_NATIVE_MEDIA_WORK_PARENT to an existing private directory"),
    );
    assert!(
        inputs.is_absolute(),
        "reviewed inputs path must be absolute"
    );
    assert!(source_root.is_absolute(), "P4 source path must be absolute");
    assert!(
        work_parent.is_absolute(),
        "work parent path must be absolute"
    );
    assert!(inputs.is_file(), "reviewed inputs file must exist");
    assert!(source_root.is_dir(), "P4 source root must exist");
    let work_parent_metadata = fs::symlink_metadata(&work_parent).unwrap();
    assert!(work_parent_metadata.file_type().is_dir());
    assert_eq!(
        work_parent_metadata.permissions().mode() & 0o077,
        0,
        "work parent must be private"
    );
    let inputs_raw = fs::read(&inputs).unwrap();
    assert_eq!(
        sha256_bytes(&inputs_raw).to_string(),
        inputs_sha256,
        "environment pin must match the exact reviewed input bytes"
    );

    let output = Command::new(aros())
        .args([
            "image",
            "prepare",
            "--preset",
            "esp32p4-d1001",
            "--source-root",
            source_root.to_str().expect("source path is UTF-8"),
            "--inputs",
            inputs.to_str().expect("inputs path is UTF-8"),
            "--inputs-sha256",
            &inputs_sha256,
            "--work-parent",
            work_parent.to_str().expect("work parent path is UTF-8"),
            "--jobs",
            "12",
            "--timeout-seconds",
            "900",
            "--format",
            "json",
        ])
        .output()
        .expect("public aros image prepare process executes");
    assert!(
        output.status.success(),
        "public media preparation failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: Value = serde_json::from_slice(&output.stdout)
        .expect("stdout must contain only the JSON result without mixed logs");
    assert_eq!(receipt["schema"], "aros-native-media-preparation-v1");
    assert_eq!(receipt["qualification"], "local-byte-lock-only");
    assert!(!receipt["complete_flash_plan"]
        .as_bool()
        .expect("receipt states complete flash plan status"));
    for unsupported_claim in [
        "native_build",
        "native_build_receipt",
        "flash_plan",
        "device",
        "device_written",
        "boot_qualified",
        "release_qualified",
    ] {
        assert!(
            receipt.get(unsupported_claim).is_none(),
            "preparation receipt makes an unsupported {unsupported_claim} claim"
        );
    }

    let work_root = PathBuf::from(
        receipt["work_root"]
            .as_str()
            .expect("receipt contains its owned work root"),
    );
    let persisted: Value =
        serde_json::from_slice(&fs::read(work_root.join("preparation.receipt.json")).unwrap())
            .expect("persisted preparation receipt parses");
    assert_eq!(persisted, receipt, "stdout and retained receipt differ");

    let bootloader = assert_file_hash(
        &receipt["bootloader_path"],
        &receipt["bootloader"]["sha256"],
    );
    assert_eq!(
        bootloader.len() as u64,
        receipt["bootloader"]["size_bytes"].as_u64().unwrap()
    );
    let partition_table = assert_file_hash(
        &receipt["partition_table_path"],
        &receipt["partition_table"]["sha256"],
    );
    assert_eq!(
        partition_table.len() as u64,
        receipt["partition_table"]["size_bytes"].as_u64().unwrap()
    );

    let python_receipt_bytes = assert_file_hash(
        &receipt["python_receipt"],
        &receipt["python_receipt_sha256"],
    );
    let python_receipt: Value = serde_json::from_slice(&python_receipt_bytes).unwrap();
    assert_eq!(
        python_receipt["format"],
        "aros-python-wheel-environment-receipt-v1"
    );
    assert_producer_phases_succeeded(&python_receipt, "wheel environment");

    let bootloader_receipt_bytes = assert_file_hash(
        &receipt["bootloader_receipt"],
        &receipt["bootloader_receipt_sha256"],
    );
    let bootloader_receipt: Value = serde_json::from_slice(&bootloader_receipt_bytes).unwrap();
    assert_eq!(
        bootloader_receipt["format"],
        "aros-idf-bootloader-receipt-v1"
    );
    assert_eq!(
        bootloader_receipt["python_receipt_sha256"],
        receipt["python_receipt_sha256"]
    );
    assert_eq!(
        bootloader_receipt["bootloader"]["sha256"],
        receipt["bootloader"]["sha256"]
    );
    assert_producer_phases_succeeded(&bootloader_receipt, "IDF bootloader");

    eprintln!(
        "native media preparation verified: work_root={} bootloader={} bytes sha256={} partition_table={} bytes sha256={}",
        work_root.display(),
        bootloader.len(),
        receipt["bootloader"]["sha256"].as_str().unwrap(),
        partition_table.len(),
        receipt["partition_table"]["sha256"].as_str().unwrap(),
    );
}
