//! Closed input binding for offline external media preparation.

use aros_common::sha256_bytes;
use aros_toolchain::native_media_preparation::bind_native_media_preparation_inputs;
use serde_json::{json, Value};
use std::path::Path;

fn source(root: &Path, name: &str) -> Value {
    json!({
        "destination": root.join("prepared").join(name),
        "archive": name,
        "suffix": "tar.xz",
        "sha256": "d".repeat(64),
        "patches": []
    })
}

fn inputs(root: &Path) -> Value {
    let path = |name: &str| root.join(name);
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
        "idf_source": source(root, "esp-idf"),
        "compiler_source": source(root, "riscv32-esp-elf"),
        "cmake_source": source(root, "cmake"),
        "ninja_source": source(root, "ninja"),
        "idf_root": path("idf"),
        "compiler_root": path("compiler"),
        "cmake_root": path("cmake-root"),
        "cmake": path("cmake-root/bin/cmake"),
        "ninja": path("ninja-root/bin/ninja"),
        "git": path("git/bin/git"),
        "constraints": path("constraints.txt")
    })
}

fn bind(document: &Value) -> Result<(), aros_toolchain::ContractError> {
    let raw = serde_json::to_vec(document).unwrap();
    let expected = sha256_bytes(&raw);
    bind_native_media_preparation_inputs(&raw, &expected).map(|_| ())
}

#[test]
fn binds_the_exact_raw_document_and_accepts_only_explicit_absolute_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let document = inputs(temporary.path());
    let raw = serde_json::to_vec(&document).unwrap();
    let expected = sha256_bytes(&raw);
    assert!(bind_native_media_preparation_inputs(&raw, &expected).is_ok());

    let mut changed_raw = raw.clone();
    changed_raw.push(b'\n');
    assert!(bind_native_media_preparation_inputs(&changed_raw, &expected).is_err());
    assert!(bind_native_media_preparation_inputs(&raw, &sha256_bytes(b"different bytes")).is_err());

    let mut relative = document.clone();
    relative["interpreter"] = json!("tools/python3");
    assert!(bind(&relative).is_err());

    let mut traversing = document;
    traversing["interpreter"] = json!(temporary.path().join("tools/../python3"));
    assert!(bind(&traversing).is_err());
}

#[test]
fn rejects_unknown_fields_and_unsupported_input_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let document = inputs(temporary.path());

    let mut unknown = document.clone();
    unknown["ambient_fallback"] = json!(true);
    assert!(bind(&unknown).is_err());

    for (field, value) in [
        ("schema_version", json!(2)),
        ("format", json!("aros-native-media-inputs-v2")),
        ("qualification", json!("release-qualified")),
        ("provider", json!("arbitrary-provider")),
    ] {
        let mut unsupported = document.clone();
        unsupported[field] = value;
        assert!(bind(&unsupported).is_err(), "accepted unsupported {field}");
    }
}

#[test]
fn refuses_patch_parser_delimiters_and_unsupported_options() {
    let temporary = tempfile::tempdir().unwrap();
    let document = inputs(temporary.path());

    let mut delimiter = document.clone();
    delimiter["idf_source"]["patches"] = json!([{
        "name": "change.diff:injected",
        "subdirectory": null,
        "options": [],
        "sha256": "e".repeat(64)
    }]);
    assert!(bind(&delimiter).is_err());

    let mut unsupported = document;
    unsupported["idf_source"]["patches"] = json!([{
        "name": "change.diff",
        "subdirectory": "components/app",
        "options": ["--unsafe"],
        "sha256": "e".repeat(64)
    }]);
    assert!(bind(&unsupported).is_err());
}

#[test]
fn expired_preflight_and_cancellation_refuse_before_opening_inputs_or_reservation() {
    use aros_common::CancellationToken;
    use aros_toolchain::native_media_preparation::{
        prepare_native_media, NativeMediaPreparationRequest,
    };
    use std::time::{Duration, Instant};

    let temporary = tempfile::tempdir().unwrap();
    let raw = serde_json::to_vec(&inputs(temporary.path())).unwrap();
    let bound = bind_native_media_preparation_inputs(&raw, &sha256_bytes(&raw)).unwrap();
    let cancellation = CancellationToken::default();
    let source = temporary.path().join("absent-source");
    let work_parent = temporary.path().join("absent-work");
    let mut request = NativeMediaPreparationRequest {
        inputs: &bound,
        source_root: &source,
        preset: "fixture",
        work_parent: &work_parent,
        jobs: 1,
        started_at: Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
        timeout: Duration::from_secs(1),
        cancellation: &cancellation,
    };
    assert!(prepare_native_media(&request)
        .unwrap_err()
        .to_string()
        .contains("deadline expired"));
    assert!(!source.exists() && !work_parent.exists());
    request.started_at = Instant::now();
    cancellation.cancel();
    assert!(prepare_native_media(&request)
        .unwrap_err()
        .to_string()
        .contains("live positive budgets"));
    assert!(!source.exists() && !work_parent.exists());
}
