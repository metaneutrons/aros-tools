#![cfg(unix)]
//! Actual local SDK CLI stages, without a checkout or compiler execution.

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    identity: PathBuf,
    identity_sha256: String,
    tree_sha256: String,
    candidate: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let candidate = root.join("candidate");
        fs::create_dir_all(candidate.join("include")).unwrap();
        fs::write(candidate.join("include/aros.h"), b"#define AROS 1\n").unwrap();
        // This input must not be silently excluded by compiler inventory rules.
        fs::write(
            candidate.join("toolchain-manifest.json"),
            b"ordinary SDK data\n",
        )
        .unwrap();
        let identity = SdkIdentity {
            sdk_id: "riscv32-aros-ilp32f".into(),
            version: "0.0.0-local".into(),
            target: SdkTarget {
                target_triple: "riscv-aros".into(),
                abi: BTreeMap::from([("abi".into(), SdkAbiValue::Text("ilp32f".into()))]),
            },
            source: SdkSourceIdentity {
                commit: "1".repeat(40),
                git_tree: "2".repeat(40),
                content_sha256: sha256_bytes(b"fixture source"),
                contract_sha256: sha256_bytes(b"fixture source SDK contract"),
            },
            source_date_epoch: 1,
        };
        let bytes = serde_json::to_vec(&identity).unwrap();
        let identity_sha256 = sha256_bytes(&bytes).to_string();
        let identity = root.join("identity.json");
        fs::write(&identity, bytes).unwrap();
        let (tree_sha256, _) = toolchain_tree_inventory_excluding(&candidate, &[]).unwrap();
        Self {
            _temp: temp,
            root,
            identity,
            identity_sha256,
            tree_sha256,
            candidate,
        }
    }

    fn command(&self, leaf: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.root)
            .args(["--diagnostic-format", "json", "sdk", leaf])
            .arg("--identity")
            .arg(&self.identity)
            .arg("--identity-sha256")
            .arg(&self.identity_sha256)
            .arg("--tree-sha256")
            .arg(&self.tree_sha256)
            .arg("--forbidden-prefix")
            .arg(self.root.join("never-used-build-root"));
        command
    }

    fn package(&self, output: &Path) -> Output {
        self.command("package")
            .arg("--candidate-root")
            .arg(&self.candidate)
            .arg("--output-dir")
            .arg(output)
            .output()
            .unwrap()
    }

    fn readback(&self, leaf: &str, package: &Path, result: &Value) -> Command {
        let mut command = self.command(leaf);
        command
            .arg("--package-dir")
            .arg(package)
            .arg("--archive-sha256")
            .arg(result["result"]["archive_sha256"].as_str().unwrap())
            .arg("--archive-size")
            .arg(
                result["result"]["archive_size"]
                    .as_u64()
                    .unwrap()
                    .to_string(),
            );
        command
    }
}

fn success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["schema"], "aros-sdk-local-stage-v2");
    assert_eq!(
        result["assurance"],
        "local-byte-consistency-not-release-admission"
    );
    result
}

#[test]
fn sdk_local_cli_packages_verifies_and_extracts_outside_a_checkout() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let result = success(&fixture.package(&package));
    success(
        &fixture
            .readback("verify-package", &package, &result)
            .output()
            .unwrap(),
    );
    let extracted = fixture.root.join("extracted");
    success(
        &fixture
            .readback("extract", &package, &result)
            .arg("--output-root")
            .arg(&extracted)
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read(extracted.join("include/aros.h")).unwrap(),
        b"#define AROS 1\n"
    );
    assert_eq!(
        fs::read(extracted.join("toolchain-manifest.json")).unwrap(),
        b"ordinary SDK data\n"
    );
    assert!(extracted.join("sdk-manifest.json").is_file());
    assert!(!fixture.candidate.join("sdk-manifest.json").exists());
    assert!(
        !fixture.package(&package).status.success(),
        "must never reuse output"
    );
    assert!(!fixture
        .readback("extract", &package, &result)
        .arg("--output-root")
        .arg(&extracted)
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn sdk_cli_requires_independent_selections_and_rejects_changed_input() {
    let fixture = Fixture::new();
    let absent = fixture.root.join("must-remain-absent");
    fs::write(&fixture.identity, b"{}\n").unwrap();
    assert!(!fixture.package(&absent).status.success());
    assert!(!absent.exists());

    let missing = Command::new(env!("CARGO_BIN_EXE_aros"))
        .args(["sdk", "verify-package", "--package-dir", "/unused"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
}

#[test]
fn sdk_cli_refuses_a_changed_package_before_creating_an_extraction_root() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let result = success(&fixture.package(&package));
    fs::write(package.join("unselected-extra"), b"unselected").unwrap();
    let absent = fixture.root.join("must-remain-absent");
    assert!(!fixture
        .readback("extract", &package, &result)
        .arg("--output-root")
        .arg(&absent)
        .output()
        .unwrap()
        .status
        .success());
    assert!(!absent.exists());
}

#[test]
fn sdk_cli_rejects_wrong_archive_selections_before_extraction() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let result = success(&fixture.package(&package));
    for (name, option, value) in [
        ("wrong-hash", "--archive-sha256", "4".repeat(64)),
        ("wrong-size", "--archive-size", "1".to_owned()),
        ("zero-size", "--archive-size", "0".to_owned()),
    ] {
        let absent = fixture.root.join(name);
        let mut command = fixture.command("extract");
        command
            .arg("--package-dir")
            .arg(&package)
            .arg("--archive-sha256")
            .arg(if option == "--archive-sha256" {
                value.clone()
            } else {
                result["result"]["archive_sha256"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .arg("--archive-size")
            .arg(if option == "--archive-size" {
                value
            } else {
                result["result"]["archive_size"]
                    .as_u64()
                    .unwrap()
                    .to_string()
            })
            .arg("--output-root")
            .arg(&absent);
        assert!(!command.output().unwrap().status.success());
        assert!(!absent.exists());
    }
}

fn release_fixture(
    fixture: &Fixture,
    package: &Path,
    result: &Value,
) -> (
    PathBuf,
    String,
    aros_common::sdk_release_lock::SdkReleaseLock,
) {
    use aros_common::sdk_release_lock::{
        SdkArtifactDigest, SdkReleaseLock, SDK_RELEASE_LOCK_SCHEMA,
    };
    let identity: SdkIdentity =
        serde_json::from_slice(&fs::read(&fixture.identity).unwrap()).unwrap();
    let archive = package.join(identity.asset_name().unwrap());
    let manifest = package.join(format!("{}.manifest.json", identity.asset_name().unwrap()));
    let archive_bytes = fs::read(&archive).unwrap();
    let manifest_bytes = fs::read(&manifest).unwrap();
    let lock = SdkReleaseLock {
        schema: SDK_RELEASE_LOCK_SCHEMA.into(),
        release_tag: "v1.0.0".into(),
        base_url: "https://example.invalid/releases/v1.0.0".into(),
        identity,
        tree_sha256: aros_common::Sha256Digest::parse(
            result["result"]["tree_sha256"].as_str().unwrap(),
        )
        .unwrap(),
        archive: SdkArtifactDigest {
            sha256: sha256_bytes(&archive_bytes),
            size: archive_bytes.len() as u64,
        },
        manifest: SdkArtifactDigest {
            sha256: sha256_bytes(&manifest_bytes),
            size: manifest_bytes.len() as u64,
        },
    };
    let bytes = lock.canonical_bytes().unwrap();
    let digest = sha256_bytes(&bytes).to_string();
    let path = fixture.root.join("release-lock.json");
    fs::write(&path, bytes).unwrap();
    (path, digest, lock)
}

fn managed_command(fixture: &Fixture, leaf: &str, lock: &Path, digest: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
    command
        .current_dir(&fixture.root)
        .env("AROS_HOME", fixture.root.join("state"))
        .args(["--diagnostic-format", "json", "sdk", leaf, "--release-lock"])
        .arg(lock)
        .args(["--lock-sha256", digest]);
    command
}

fn managed_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema"], "aros-sdk-operation-v2");
    assert_eq!(
        value["assurance"],
        "selected-byte-consistency-not-publisher-authentication"
    );
    value
}

#[test]
fn sdk_managed_cli_acquires_installs_lists_verifies_and_prints_application_sdk_path() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let (lock_path, digest, lock) = release_fixture(&fixture, &package, &packaged);
    let cache = fixture.root.join("cache");
    let cache_entry = cache.join(lock.digest().unwrap().as_str());
    fs::create_dir_all(&cache_entry).unwrap();
    let asset = lock.identity.asset_name().unwrap();
    for name in [&asset, &format!("{asset}.manifest.json")] {
        fs::copy(package.join(name), cache_entry.join(name)).unwrap();
    }
    let downloaded = managed_success(
        &managed_command(&fixture, "download", &lock_path, &digest)
            .arg("--cache")
            .arg(&cache)
            .arg("--offline")
            .output()
            .unwrap(),
    );
    assert_eq!(downloaded["result"]["installed"], false);
    assert_eq!(
        downloaded["result"]["archive"],
        cache_entry.join(&asset).to_str().unwrap()
    );
    assert!(!fixture.root.join("state/sdks").exists());
    let installed = managed_success(
        &managed_command(&fixture, "install", &lock_path, &digest)
            .arg("--cache")
            .arg(&cache)
            .arg("--offline")
            .output()
            .unwrap(),
    );
    assert_eq!(installed["result"]["already_installed"], false);
    let payload = installed["result"]["payload"].as_str().unwrap();
    assert_eq!(
        fs::read(Path::new(payload).join("include/aros.h")).unwrap(),
        b"#define AROS 1\n"
    );
    let reused = managed_success(
        &managed_command(&fixture, "install", &lock_path, &digest)
            .arg("--archive")
            .arg(package.join(&asset))
            .output()
            .unwrap(),
    );
    assert_eq!(reused["result"]["already_installed"], true);
    managed_success(
        &managed_command(&fixture, "verify", &lock_path, &digest)
            .output()
            .unwrap(),
    );
    let path = managed_command(&fixture, "path", &lock_path, &digest)
        .output()
        .unwrap();
    assert!(path.status.success());
    assert_eq!(
        String::from_utf8(path.stdout).unwrap(),
        format!("{payload}\n")
    );
    let listed = managed_success(
        &Command::new(env!("CARGO_BIN_EXE_aros"))
            .current_dir(&fixture.root)
            .env("AROS_HOME", fixture.root.join("state"))
            .args(["sdk", "list"])
            .output()
            .unwrap(),
    );
    assert_eq!(listed["result"]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(listed["result"]["entries"][0]["integrity"], "not-checked");
    assert_eq!(listed["result"]["findings"].as_array().unwrap().len(), 0);
}

#[test]
fn sdk_cli_rejects_changed_selection_poisoned_cache_and_unsafe_argument_combinations() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let (lock_path, digest, lock) = release_fixture(&fixture, &package, &packaged);
    let asset = lock.identity.asset_name().unwrap();
    let cache = fixture.root.join("cache");
    for leaf in ["download", "install"] {
        assert!(!managed_command(&fixture, leaf, &lock_path, &digest)
            .arg("--cache")
            .arg(&cache)
            .arg("--offline")
            .output()
            .unwrap()
            .status
            .success());
        assert!(!fixture.root.join("state/sdks").exists());
    }
    assert!(!cache.exists(), "offline absence must not create a cache");
    for conflicting in ["--offline", "--cache"] {
        let mut command = managed_command(&fixture, "install", &lock_path, &digest);
        command
            .arg("--archive")
            .arg(package.join(&asset))
            .arg(conflicting);
        if conflicting == "--cache" {
            command.arg(&cache);
        }
        assert!(!command.output().unwrap().status.success());
    }
    let cache_entry = cache.join(lock.digest().unwrap().as_str());
    fs::create_dir_all(&cache_entry).unwrap();
    fs::write(cache_entry.join(&asset), b"poison").unwrap();
    fs::copy(
        package.join(format!("{asset}.manifest.json")),
        cache_entry.join(format!("{asset}.manifest.json")),
    )
    .unwrap();
    assert!(!managed_command(&fixture, "install", &lock_path, &digest)
        .arg("--cache")
        .arg(&cache)
        .arg("--offline")
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(fs::read(cache_entry.join(&asset)).unwrap(), b"poison");
    assert!(!fixture.root.join("state/sdks").exists());
    fs::write(&lock_path, b"{}\n").unwrap();
    assert!(!managed_command(&fixture, "install", &lock_path, &digest)
        .arg("--archive")
        .arg(package.join(&asset))
        .output()
        .unwrap()
        .status
        .success());
    assert!(!fixture.root.join("state/sdks").exists());
}

#[test]
fn sdk_cli_metadata_listing_is_nonmutating_and_full_verification_detects_corruption() {
    let fixture = Fixture::new();
    let store = fixture.root.join("store");
    let listed = managed_success(
        &Command::new(env!("CARGO_BIN_EXE_aros"))
            .current_dir(&fixture.root)
            .args(["sdk", "list", "--store"])
            .arg(&store)
            .output()
            .unwrap(),
    );
    assert_eq!(listed["result"]["store_state"], "absent");
    assert!(!store.exists());
    for budget in ["0", "100001"] {
        assert!(!Command::new(env!("CARGO_BIN_EXE_aros"))
            .args(["sdk", "list", "--store"])
            .arg(&store)
            .args(["--max-entries", budget])
            .output()
            .unwrap()
            .status
            .success());
    }
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let (lock_path, digest, lock) = release_fixture(&fixture, &package, &packaged);
    for leaf in ["verify", "path"] {
        assert!(!managed_command(&fixture, leaf, &lock_path, &digest)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap()
            .status
            .success());
        assert!(
            !store.exists(),
            "read-only verification must not create a store"
        );
    }
    let installed = managed_success(
        &managed_command(&fixture, "install", &lock_path, &digest)
            .arg("--store")
            .arg(&store)
            .arg("--archive")
            .arg(package.join(lock.identity.asset_name().unwrap()))
            .output()
            .unwrap(),
    );
    let header = Path::new(installed["result"]["payload"].as_str().unwrap()).join("include/aros.h");
    fs::write(&header, b"corrupt\n").unwrap();
    for leaf in ["verify", "path"] {
        assert!(!managed_command(&fixture, leaf, &lock_path, &digest)
            .arg("--store")
            .arg(&store)
            .output()
            .unwrap()
            .status
            .success());
    }
    assert_eq!(
        fs::read(header).unwrap(),
        b"corrupt\n",
        "verification must not repair"
    );
}
