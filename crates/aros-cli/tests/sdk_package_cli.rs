#![cfg(unix)]
//! Actual local SDK CLI stages, without a checkout or compiler execution.

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding, Sha256Digest};
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
        self.command_with_selection(leaf, &self.identity_sha256, &self.tree_sha256)
    }

    fn command_with_selection(
        &self,
        leaf: &str,
        identity_sha256: &str,
        tree_sha256: &str,
    ) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.root)
            .args(["--diagnostic-format", "json", "sdk", leaf])
            .arg("--identity")
            .arg(&self.identity)
            .arg("--identity-sha256")
            .arg(identity_sha256)
            .arg("--tree-sha256")
            .arg(tree_sha256)
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

    fn sdk_asset_name(&self) -> String {
        let identity: SdkIdentity =
            serde_json::from_slice(&fs::read(&self.identity).unwrap()).unwrap();
        identity.asset_name().unwrap()
    }

    fn sbom_command(
        &self,
        package: &Path,
        output: &Path,
        identity_sha256: &str,
        tree_sha256: &str,
        archive_sha256: &str,
        archive_size: u64,
    ) -> Command {
        let mut command = self.command_with_selection("sbom", identity_sha256, tree_sha256);
        command
            .arg("--package-dir")
            .arg(package)
            .arg("--archive-sha256")
            .arg(archive_sha256)
            .arg("--archive-size")
            .arg(archive_size.to_string())
            .arg("--output")
            .arg(output);
        command
    }

    fn selected_sbom_command(&self, package: &Path, output: &Path, result: &Value) -> Command {
        self.sbom_command(
            package,
            output,
            &self.identity_sha256,
            &self.tree_sha256,
            result["result"]["archive_sha256"].as_str().unwrap(),
            result["result"]["archive_size"].as_u64().unwrap(),
        )
    }

    fn sdk_verification_request(
        &self,
        package: &Path,
        result: &Value,
    ) -> aros_toolchain::sdk_package::SdkVerificationRequest {
        aros_toolchain::sdk_package::SdkVerificationRequest {
            package_dir: package.to_path_buf(),
            identity: serde_json::from_slice(&fs::read(&self.identity).unwrap()).unwrap(),
            expected_tree_sha256: Sha256Digest::parse(&self.tree_sha256).unwrap(),
            expected_archive_sha256: Sha256Digest::parse(
                result["result"]["archive_sha256"].as_str().unwrap(),
            )
            .unwrap(),
            expected_archive_size: result["result"]["archive_size"].as_u64().unwrap(),
            forbidden_prefixes: vec![self.root.join("never-used-build-root")],
        }
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

fn assert_rejected(output: &Output, expected_reason: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "failed command emitted a result");
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(diagnostic["diagnostics"][0]["severity"], "error");
    let message = diagnostic["diagnostics"][0]["message"]
        .as_str()
        .expect("JSON diagnostic message");
    assert!(
        message.contains(expected_reason),
        "expected failure containing {expected_reason:?}, got {message:?}"
    );
}

fn assert_rejected_without_sbom(output: &Output, expected_reason: &str, sbom: &Path) {
    assert_rejected(output, expected_reason);
    assert!(!sbom.exists(), "rejected command created an SPDX output");
}

fn different_digest(digest: &str) -> String {
    let first = if digest.starts_with('0') { '1' } else { '0' };
    format!("{first}{}", &digest[1..])
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

fn assert_exact_sdk_package_members(package: &Path, asset: &str) {
    let names = fs::read_dir(package)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names,
        std::collections::BTreeSet::from([
            asset.to_owned(),
            format!("{asset}.manifest.json"),
            format!("{asset}.sha256"),
        ])
    );
}

#[test]
fn sdk_sbom_cli_is_deterministic_and_keeps_metadata_out_of_the_package() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let asset = fixture.sdk_asset_name();
    assert_exact_sdk_package_members(&package, &asset);

    let expected = aros_toolchain::sdk_sbom::build_measured_sdk_sbom(
        &fixture.sdk_verification_request(&package, &packaged),
    )
    .unwrap();
    let output_name = format!("{asset}.spdx.json");
    let first_stage = fixture.root.join("sbom-stage-a");
    let second_stage = fixture.root.join("sbom-stage-b");
    fs::create_dir(&first_stage).unwrap();
    fs::create_dir(&second_stage).unwrap();
    let first_path = first_stage.join(&output_name);
    let second_path = second_stage.join(&output_name);

    let first = success(
        &fixture
            .selected_sbom_command(&package, &first_path, &packaged)
            .output()
            .unwrap(),
    );
    let second = success(
        &fixture
            .selected_sbom_command(&package, &second_path, &packaged)
            .output()
            .unwrap(),
    );
    assert_eq!(fs::read(&first_path).unwrap(), expected);
    assert_eq!(fs::read(&second_path).unwrap(), expected);
    assert_eq!(
        fs::read(&first_path).unwrap(),
        fs::read(&second_path).unwrap()
    );
    for (result, path) in [(&first, &first_path), (&second, &second_path)] {
        assert_eq!(result["result"]["output"], path.to_str().unwrap());
        assert_eq!(
            result["result"]["sha256"],
            sha256_bytes(&expected).to_string()
        );
        assert_eq!(result["result"]["size"], expected.len() as u64);
    }
    assert_exact_sdk_package_members(&package, &asset);
    success(
        &fixture
            .readback("verify-package", &package, &packaged)
            .output()
            .unwrap(),
    );
}

#[test]
fn sdk_sbom_cli_rejects_bad_selections_without_creating_output() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let asset = fixture.sdk_asset_name();
    let output_name = format!("{asset}.spdx.json");
    let archive_sha256 = packaged["result"]["archive_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let archive_size = packaged["result"]["archive_size"].as_u64().unwrap();
    let wrong_archive_sha256 = different_digest(&archive_sha256);
    let wrong_tree_sha256 = different_digest(&fixture.tree_sha256);

    let bad_identity_output = fixture.root.join("bad-identity").join(&output_name);
    fs::create_dir(bad_identity_output.parent().unwrap()).unwrap();
    let selected_identity_bytes = fs::read(&fixture.identity).unwrap();
    fs::write(&fixture.identity, b"{}\n").unwrap();
    let bad_identity = fixture
        .sbom_command(
            &package,
            &bad_identity_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            &archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    fs::write(&fixture.identity, &selected_identity_bytes).unwrap();
    assert_rejected_without_sbom(
        &bad_identity,
        "SDK identity document differs from its independently selected SHA-256",
        &bad_identity_output,
    );

    let bad_archive_output = fixture.root.join("bad-archive").join(&output_name);
    fs::create_dir(bad_archive_output.parent().unwrap()).unwrap();
    let bad_archive = fixture
        .sbom_command(
            &package,
            &bad_archive_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            &wrong_archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected_without_sbom(
        &bad_archive,
        "SDK archive differs from its independently selected hash or size",
        &bad_archive_output,
    );

    let bad_tree_output = fixture.root.join("bad-tree").join(&output_name);
    fs::create_dir(bad_tree_output.parent().unwrap()).unwrap();
    let bad_tree = fixture
        .sbom_command(
            &package,
            &bad_tree_output,
            &fixture.identity_sha256,
            &wrong_tree_sha256,
            &archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected_without_sbom(
        &bad_tree,
        "SDK manifest tree digest differs from the selected tree digest",
        &bad_tree_output,
    );

    let archive_path = package.join(&asset);
    let original_archive = fs::read(&archive_path).unwrap();
    fs::write(&archive_path, b"corrupt archive\n").unwrap();
    let corrupt_archive_output = fixture.root.join("corrupt-archive").join(&output_name);
    fs::create_dir(corrupt_archive_output.parent().unwrap()).unwrap();
    let corrupt_archive = fixture
        .sbom_command(
            &package,
            &corrupt_archive_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            &archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected_without_sbom(
        &corrupt_archive,
        "SDK archive differs from its independently selected hash or size",
        &corrupt_archive_output,
    );
    fs::write(&archive_path, original_archive).unwrap();

    let missing_package = fixture.root.join("missing-package");
    let missing_output = fixture
        .root
        .join("missing-package-output")
        .join(&output_name);
    fs::create_dir(missing_output.parent().unwrap()).unwrap();
    let missing_package_output = fixture
        .sbom_command(
            &missing_package,
            &missing_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            &archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected_without_sbom(
        &missing_package_output,
        "SDK directory is inaccessible",
        &missing_output,
    );
}

#[test]
fn sdk_sbom_cli_preserves_occupied_outputs_and_rejects_unsafe_paths() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let asset = fixture.sdk_asset_name();
    let output_name = format!("{asset}.spdx.json");
    let archive_sha256 = packaged["result"]["archive_sha256"].as_str().unwrap();
    let archive_size = packaged["result"]["archive_size"].as_u64().unwrap();

    let regular_output = fixture.root.join("regular-stage").join(&output_name);
    fs::create_dir(regular_output.parent().unwrap()).unwrap();
    fs::write(&regular_output, b"regular sentinel\n").unwrap();
    let regular_rejection = fixture
        .sbom_command(
            &package,
            &regular_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected(
        &regular_rejection,
        "SDK SPDX output already exists or is unavailable",
    );
    assert_eq!(fs::read(&regular_output).unwrap(), b"regular sentinel\n");

    let symlink_target = fixture.root.join("symlink-sentinel");
    fs::write(&symlink_target, b"symlink sentinel\n").unwrap();
    let symlink_output = fixture.root.join("symlink-stage").join(&output_name);
    fs::create_dir(symlink_output.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&symlink_target, &symlink_output).unwrap();
    let symlink_rejection = fixture
        .sbom_command(
            &package,
            &symlink_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected(
        &symlink_rejection,
        "SDK SPDX output already exists or is unavailable",
    );
    assert!(fs::symlink_metadata(&symlink_output)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&symlink_output).unwrap(), symlink_target);
    assert_eq!(fs::read(&symlink_target).unwrap(), b"symlink sentinel\n");

    let relative_output = PathBuf::from(&output_name);
    let relative_rejection = fixture
        .sbom_command(
            &package,
            &relative_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected_without_sbom(
        &relative_rejection,
        "SDK SPDX output must be an absolute <archive>.spdx.json path",
        &fixture.root.join(&output_name),
    );
    assert!(!fixture.root.join(&output_name).exists());

    let misnamed_output = fixture.root.join("misnamed.spdx.json");
    let misnamed_rejection = fixture
        .sbom_command(
            &package,
            &misnamed_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected_without_sbom(
        &misnamed_rejection,
        "SDK SPDX output must be an absolute <archive>.spdx.json path",
        &misnamed_output,
    );
    assert!(!misnamed_output.exists());

    let package_output = package.join(&output_name);
    let package_rejection = fixture
        .sbom_command(
            &package,
            &package_output,
            &fixture.identity_sha256,
            &fixture.tree_sha256,
            archive_sha256,
            archive_size,
        )
        .output()
        .unwrap();
    assert_rejected_without_sbom(
        &package_rejection,
        "SDK SPDX output must be outside the SDK package",
        &package_output,
    );
    assert!(!package_output.exists());
    assert_exact_sdk_package_members(&package, &asset);
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

fn project_command(fixture: &Fixture, leaf: &str, project_lock: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
    command
        .current_dir(&fixture.root)
        .env("AROS_HOME", fixture.root.join("state"))
        .args(["sdk", leaf, "--project-lock"])
        .arg(project_lock);
    command
}

#[test]
fn sdk_project_cli_previews_applies_reuses_and_reads_a_portable_application_lock() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let (lock_path, digest, lock) = release_fixture(&fixture, &package, &packaged);
    let installed = managed_success(
        &managed_command(&fixture, "install", &lock_path, &digest)
            .arg("--archive")
            .arg(package.join(lock.identity.asset_name().unwrap()))
            .output()
            .unwrap(),
    );
    let app = fixture.root.join("app");
    fs::create_dir(&app).unwrap();
    let project = app.join("aros-sdk.lock.json");
    let preview = managed_success(
        &managed_command(&fixture, "select", &lock_path, &digest)
            .arg("--project-lock")
            .arg(&project)
            .output()
            .unwrap(),
    );
    assert_eq!(preview["result"]["will_write"], true);
    assert!(!project.exists());
    assert_eq!(
        fs::read_dir(&app).unwrap().count(),
        0,
        "preview creates no guard"
    );
    let token = preview["result"]["confirmation"].as_str().unwrap();
    let selected = managed_success(
        &managed_command(&fixture, "select", &lock_path, &digest)
            .arg("--project-lock")
            .arg(&project)
            .args(["--apply", token])
            .output()
            .unwrap(),
    );
    assert_eq!(selected["result"]["already_selected"], false);
    let bytes = fs::read(&project).unwrap();
    assert_eq!(bytes, lock.canonical_bytes().unwrap());
    assert_eq!(
        fs::read(&lock_path).unwrap(),
        bytes,
        "candidate is immutable"
    );
    assert!(!String::from_utf8(bytes.clone())
        .unwrap()
        .contains(fixture.root.to_str().unwrap()));
    managed_success(
        &project_command(&fixture, "verify", &project)
            .output()
            .unwrap(),
    );
    let path = project_command(&fixture, "path", &project)
        .output()
        .unwrap();
    assert!(
        path.status.success(),
        "{}",
        String::from_utf8_lossy(&path.stderr)
    );
    assert_eq!(
        String::from_utf8(path.stdout).unwrap(),
        format!("{}\n", installed["result"]["payload"].as_str().unwrap())
    );
    let reuse = managed_success(
        &managed_command(&fixture, "select", &lock_path, &digest)
            .arg("--project-lock")
            .arg(&project)
            .output()
            .unwrap(),
    );
    assert_eq!(reuse["result"]["will_write"], false);
    let reused = managed_success(
        &managed_command(&fixture, "select", &lock_path, &digest)
            .arg("--project-lock")
            .arg(&project)
            .args(["--apply", reuse["result"]["confirmation"].as_str().unwrap()])
            .output()
            .unwrap(),
    );
    assert_eq!(reused["result"]["already_selected"], true);
    assert_eq!(fs::read(&project).unwrap(), bytes);
    fs::write(
        Path::new(installed["result"]["payload"].as_str().unwrap()).join("include/aros.h"),
        b"corrupt",
    )
    .unwrap();
    for leaf in ["verify", "path"] {
        assert!(!project_command(&fixture, leaf, &project)
            .output()
            .unwrap()
            .status
            .success());
    }
    assert_eq!(
        fs::read(&project).unwrap(),
        bytes,
        "verification must not alter the project lock"
    );
}

#[test]
fn sdk_project_cli_rejects_stale_token_and_unsafe_local_authority_without_repair() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let (lock_path, digest, lock) = release_fixture(&fixture, &package, &packaged);
    managed_success(
        &managed_command(&fixture, "install", &lock_path, &digest)
            .arg("--archive")
            .arg(package.join(lock.identity.asset_name().unwrap()))
            .output()
            .unwrap(),
    );
    let project = fixture.root.join("aros-sdk.lock.json");
    assert!(!managed_command(&fixture, "select", &lock_path, &digest)
        .arg("--project-lock")
        .arg(&project)
        .args(["--apply", &"0".repeat(64)])
        .output()
        .unwrap()
        .status
        .success());
    assert!(!project.exists());
    let preview = managed_success(
        &managed_command(&fixture, "select", &lock_path, &digest)
            .arg("--project-lock")
            .arg(&project)
            .output()
            .unwrap(),
    );
    fs::write(&project, b"{}\n").unwrap();
    assert!(!managed_command(&fixture, "select", &lock_path, &digest)
        .arg("--project-lock")
        .arg(&project)
        .args([
            "--apply",
            preview["result"]["confirmation"].as_str().unwrap()
        ])
        .output()
        .unwrap()
        .status
        .success());
    for leaf in ["verify", "path"] {
        assert!(!project_command(&fixture, leaf, &project)
            .output()
            .unwrap()
            .status
            .success());
    }
    assert_eq!(fs::read(&project).unwrap(), b"{}\n");
    let symlink = fixture.root.join("symlink-sdk.json");
    std::os::unix::fs::symlink(&lock_path, &symlink).unwrap();
    let hardlink = fixture.root.join("hardlink-sdk.json");
    fs::hard_link(&lock_path, &hardlink).unwrap();
    for input in [&symlink, &hardlink] {
        for leaf in ["verify", "path"] {
            assert!(!project_command(&fixture, leaf, input)
                .output()
                .unwrap()
                .status
                .success());
        }
    }
    assert_eq!(
        fs::read(&lock_path).unwrap(),
        lock.canonical_bytes().unwrap()
    );
    assert!(!fixture.root.join("aros-toolchains.lock.toml").exists());
}

#[test]
fn sdk_project_cli_restores_a_fresh_checkout_from_verified_offline_cache() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let (_, _, lock) = release_fixture(&fixture, &package, &packaged);
    let application = fixture.root.join("application");
    fs::create_dir(&application).unwrap();
    let project = application.join("aros-sdk.lock.json");
    let project_bytes = lock.canonical_bytes().unwrap();
    fs::write(&project, &project_bytes).unwrap();
    let cache = fixture.root.join("cache");
    for leaf in ["download", "install"] {
        assert!(!project_command(&fixture, leaf, &project)
            .arg("--cache")
            .arg(&cache)
            .arg("--offline")
            .output()
            .unwrap()
            .status
            .success());
        assert!(
            !cache.exists(),
            "offline absence cannot reserve cache state"
        );
        assert!(!fixture.root.join("state/sdks").exists());
    }
    let entry = cache.join(lock.digest().unwrap().as_str());
    fs::create_dir_all(&entry).unwrap();
    let archive = lock.identity.asset_name().unwrap();
    for name in [&archive, &format!("{archive}.manifest.json")] {
        fs::copy(package.join(name), entry.join(name)).unwrap();
    }
    let downloaded = managed_success(
        &project_command(&fixture, "download", &project)
            .arg("--cache")
            .arg(&cache)
            .arg("--offline")
            .output()
            .unwrap(),
    );
    assert_eq!(
        downloaded["result"]["lock_sha256"],
        lock.digest().unwrap().as_str()
    );
    assert_eq!(downloaded["result"]["installed"], false);
    assert!(!fixture.root.join("state/sdks").exists());
    let installed = managed_success(
        &project_command(&fixture, "install", &project)
            .arg("--cache")
            .arg(&cache)
            .arg("--offline")
            .output()
            .unwrap(),
    );
    assert_eq!(installed["result"]["already_installed"], false);
    managed_success(
        &project_command(&fixture, "verify", &project)
            .output()
            .unwrap(),
    );
    let path = project_command(&fixture, "path", &project)
        .output()
        .unwrap();
    assert!(path.status.success());
    assert_eq!(
        String::from_utf8(path.stdout).unwrap(),
        format!("{}\n", installed["result"]["payload"].as_str().unwrap())
    );
    let reused = managed_success(
        &project_command(&fixture, "install", &project)
            .arg("--archive")
            .arg(package.join(&archive))
            .output()
            .unwrap(),
    );
    assert_eq!(reused["result"]["already_installed"], true);
    assert_eq!(fs::read(&project).unwrap(), project_bytes);
    assert_eq!(
        fs::read_dir(&application).unwrap().count(),
        1,
        "acquisition does not create a selection guard"
    );
    assert!(!fixture.root.join("aros-toolchains.lock.toml").exists());
}

#[test]
fn sdk_project_acquisition_rejects_unsafe_config_and_corrupt_cache_without_repair() {
    let fixture = Fixture::new();
    let package = fixture.root.join("package");
    let packaged = success(&fixture.package(&package));
    let (_, _, lock) = release_fixture(&fixture, &package, &packaged);
    let project = fixture.root.join("project-sdk.json");
    fs::write(&project, lock.canonical_bytes().unwrap()).unwrap();
    let cache = fixture.root.join("cache");
    let link = fixture.root.join("unsafe-project-link.json");
    std::os::unix::fs::symlink(&project, &link).unwrap();
    let hardlink = fixture.root.join("unsafe-project-hardlink.json");
    fs::hard_link(&project, &hardlink).unwrap();
    let malformed = fixture.root.join("malformed-project.json");
    fs::write(&malformed, b"{}\n").unwrap();
    for input in [&link, &hardlink, &malformed] {
        for leaf in ["download", "install"] {
            assert!(!project_command(&fixture, leaf, input)
                .arg("--cache")
                .arg(&cache)
                .arg("--offline")
                .output()
                .unwrap()
                .status
                .success());
        }
    }
    assert!(!cache.exists());
    assert!(!fixture.root.join("state/sdks").exists());
    // Use a distinct single-link copy; do not repair the intentionally aliased authority.
    let canonical = fixture.root.join("safe-project.json");
    let bytes = lock.canonical_bytes().unwrap();
    fs::write(&canonical, &bytes).unwrap();
    let entry = cache.join(lock.digest().unwrap().as_str());
    fs::create_dir_all(&entry).unwrap();
    let archive = lock.identity.asset_name().unwrap();
    fs::write(entry.join(&archive), b"poison").unwrap();
    fs::copy(
        package.join(format!("{archive}.manifest.json")),
        entry.join(format!("{archive}.manifest.json")),
    )
    .unwrap();
    for leaf in ["download", "install"] {
        assert!(!project_command(&fixture, leaf, &canonical)
            .arg("--cache")
            .arg(&cache)
            .arg("--offline")
            .output()
            .unwrap()
            .status
            .success());
    }
    assert_eq!(fs::read(entry.join(&archive)).unwrap(), b"poison");
    assert_eq!(fs::read(&canonical).unwrap(), bytes);
    assert!(!fixture.root.join("state/sdks").exists());
}
