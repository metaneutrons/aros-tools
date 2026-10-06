//! Real vendor wheel preparation is opt-in and local-only, never a release test.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use aros_common::{
    measure_tree_content_cas_bounded, sha256_bytes, sha256_file, CancellationToken,
    TreeTraversalLimits,
};
use aros_toolchain::wheel_environment::{
    bind_wheel_environment_lock, prepare_wheel_environment, LockedWheel, WheelEnvironmentLock,
    WheelEnvironmentRequest, WheelRuntimePin,
};

fn fixture() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "format": "aros-python-wheels-v1",
        "qualification": "local-byte-lock-only",
        "host": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "runtime": {"version": "3.14.8", "executable_sha256": "1".repeat(64), "prefix_tree_sha256": "2".repeat(64)},
        "bootstrap_filename": "pip-1.0-py3-none-any.whl",
        "wheels": [{"name": "pip", "version": "1.0", "filename": "pip-1.0-py3-none-any.whl", "sha256": "3".repeat(64), "size_bytes": 100}]
    })
}

fn binds(value: &serde_json::Value) -> bool {
    let bytes = serde_json::to_vec(value).unwrap();
    bind_wheel_environment_lock(&bytes, &sha256_bytes(&bytes)).is_ok()
}

#[test]
fn lock_is_closed_bound_and_local_only() {
    let mut value = fixture();
    assert!(binds(&value));
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(bind_wheel_environment_lock(&bytes, &sha256_bytes(b"different")).is_err());
    value["qualification"] = "release-qualified".into();
    assert!(!binds(&value));
    value = fixture();
    value["activation_script"] = "execute-anything".into();
    assert!(!binds(&value));
    value = fixture();
    value["runtime"]["extra"] = true.into();
    assert!(!binds(&value));
    value = fixture();
    value["host"] = "unsupported-host".into();
    assert!(!binds(&value));
}

#[test]
fn lock_refuses_ambiguous_unsafe_or_unbounded_wheels() {
    for (field, replacement) in [
        ("name", serde_json::json!("Pip")),
        ("filename", serde_json::json!("../pip.whl")),
        ("filename", serde_json::json!("pip.whl --index-url=x")),
        ("filename", serde_json::json!("https://example.com/pip.whl")),
        ("filename", serde_json::json!(".pip.whl")),
        ("version", serde_json::json!("1.0\n--index-url=x")),
        ("size_bytes", serde_json::json!(0)),
        ("size_bytes", serde_json::json!(128 * 1024 * 1024 + 1)),
        ("sha256", serde_json::json!("unknown")),
    ] {
        let mut value = fixture();
        value["wheels"][0][field] = replacement;
        assert!(!binds(&value), "{field}");
    }
    let mut value = fixture();
    let duplicate = value["wheels"][0].clone();
    value["wheels"].as_array_mut().unwrap().push(duplicate);
    assert!(!binds(&value));
    value = fixture();
    value["bootstrap_filename"] = "missing.whl".into();
    assert!(!binds(&value));
    value = fixture();
    value["wheels"][0]["name"] = "not-pip".into();
    assert!(!binds(&value));
}

#[test]
fn cancelled_request_cannot_reserve_a_root_or_execute_a_runtime() {
    let parent = tempfile::tempdir().unwrap();
    let bytes = serde_json::to_vec(&fixture()).unwrap();
    let lock = bind_wheel_environment_lock(&bytes, &sha256_bytes(&bytes)).unwrap();
    let token = CancellationToken::default();
    token.cancel();
    let request = WheelEnvironmentRequest {
        lock: &lock,
        interpreter: std::path::Path::new("/does-not-exist"),
        runtime_prefix: parent.path(),
        wheel_cache: parent.path(),
        work_parent: parent.path(),
        timeout: Duration::from_secs(10),
        cancellation: &token,
    };
    assert!(prepare_wheel_environment(&request).is_err());
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
}

#[test]
#[ignore = "requires explicit measured wheelhouse, CPython binary and prefix paths"]
fn actual_vendor_wheels_create_a_fresh_offline_environment_and_refuse_tamper() {
    let wheel_root = PathBuf::from(std::env::var("AROS_TEST_VENDOR_WHEEL_ROOT").unwrap());
    let interpreter = PathBuf::from(std::env::var("AROS_TEST_VENDOR_PYTHON").unwrap())
        .canonicalize()
        .unwrap();
    let runtime_prefix = PathBuf::from(std::env::var("AROS_TEST_VENDOR_PYTHON_PREFIX").unwrap())
        .canonicalize()
        .unwrap();
    let runtime_version = std::env::var("AROS_TEST_VENDOR_PYTHON_VERSION").unwrap();
    let work_parent = PathBuf::from(std::env::var("AROS_TEST_VENDOR_WORK_PARENT").unwrap())
        .canonicalize()
        .unwrap();
    // The historical provisional cache inventory supplies only bytes/metadata.
    // No origin URLs, qualification claim or mutable existing venv is imported.
    let inventory: serde_json::Value =
        serde_json::from_slice(&fs::read(wheel_root.join("inventory.json")).unwrap()).unwrap();
    assert_eq!(
        inventory["artifact_status"],
        "provisional-measured-cache-coverage-only"
    );
    let wheels = inventory["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| LockedWheel {
            name: entry["name"]
                .as_str()
                .unwrap()
                .to_ascii_lowercase()
                .replace(['_', '.'], "-"),
            version: entry["version"].as_str().unwrap().to_owned(),
            filename: entry["wheel_filename"].as_str().unwrap().to_owned(),
            sha256: entry["sha256"].as_str().unwrap().to_owned(),
            size_bytes: entry["size_bytes"].as_u64().unwrap(),
        })
        .collect::<Vec<_>>();
    let bootstrap_filename = wheels
        .iter()
        .find(|wheel| wheel.name == "pip")
        .unwrap()
        .filename
        .clone();
    let tree = measure_tree_content_cas_bounded(
        &runtime_prefix,
        TreeTraversalLimits::new(300_000, 4 * 1024 * 1024 * 1024).unwrap(),
    )
    .unwrap();
    let lock = WheelEnvironmentLock {
        schema_version: 1,
        format: "aros-python-wheels-v1".into(),
        qualification: "local-byte-lock-only".into(),
        host: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        runtime: WheelRuntimePin {
            version: runtime_version,
            executable_sha256: sha256_file(&interpreter).unwrap().digest.to_string(),
            prefix_tree_sha256: tree.payload_digest_excluding(None).to_string(),
        },
        bootstrap_filename,
        wheels,
    };
    let raw = serde_json::to_vec_pretty(&lock).unwrap();
    let bound = bind_wheel_environment_lock(&raw, &sha256_bytes(&raw)).unwrap();
    let token = CancellationToken::default();
    let cache = wheel_root.join("wheelhouse");
    let mut request = WheelEnvironmentRequest {
        lock: &bound,
        interpreter: &interpreter,
        runtime_prefix: &runtime_prefix,
        wheel_cache: &cache,
        work_parent: &work_parent,
        timeout: Duration::from_secs(300),
        cancellation: &token,
    };
    let mut result = prepare_wheel_environment(&request).unwrap();
    result.revalidate().unwrap();
    assert_eq!(result.receipt.wheels.len(), 61);
    assert_eq!(result.receipt.phases.len(), 5);
    assert_eq!(result.receipt.qualification, "local-byte-lock-only");
    assert_eq!(
        sha256_file(&result.work_root.join("selected-inputs.json"))
            .unwrap()
            .digest,
        sha256_bytes(&raw)
    );
    println!("retained wheel environment: {}", result.work_root.display());
    println!(
        "receipt SHA256: {}",
        sha256_file(&result.work_root.join("environment.receipt.json"))
            .unwrap()
            .digest
    );
    let original_inventory = sha256_file(&wheel_root.join("inventory.json"))
        .unwrap()
        .digest;
    let malformed = tempfile::tempdir_in(&work_parent).unwrap();
    let malformed_path = malformed.path().canonicalize().unwrap();
    fs::write(
        malformed_path.join(&lock.wheels[0].filename),
        b"corrupted local counterprobe",
    )
    .unwrap();
    request.wheel_cache = &malformed_path;
    // Counterprobe parent is separate from the corrupted cache and runtime.
    let refusal_parent = tempfile::tempdir_in(&work_parent).unwrap();
    let refusal_path = refusal_parent.path().canonicalize().unwrap();
    request.work_parent = &refusal_path;
    assert!(prepare_wheel_environment(&request)
        .unwrap_err()
        .to_string()
        .contains("locked wheel"));
    assert_eq!(fs::read_dir(&refusal_path).unwrap().count(), 0);
    assert_eq!(
        sha256_file(&wheel_root.join("inventory.json"))
            .unwrap()
            .digest,
        original_inventory
    );
    // Substitution of a returned public path cannot adopt an ambient runtime.
    // Preserve the successfully prepared on-disk environment as evidence.
    result.interpreter.clone_from(&interpreter);
    assert!(result.revalidate().is_err());
}
