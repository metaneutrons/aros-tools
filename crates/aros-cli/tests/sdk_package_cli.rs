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
            .readback("verify", &package, &result)
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
        .args(["sdk", "verify", "--package-dir", "/unused"])
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
