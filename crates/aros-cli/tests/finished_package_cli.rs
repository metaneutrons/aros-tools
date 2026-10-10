//! Synthetic process-boundary tests for guarded finished-build packaging.
//!
//! The result and receipts are self-consistent fixture data. They do not prove
//! that a compiler ran, authenticate the producer, or authorize a release.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aros_common::{
    measure_tree_content_cas, sha256_bytes, Sha256Digest, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use serde_json::{json, Value};
use tempfile::TempDir;

const SOURCE_COMMIT: &str = "1111111111111111111111111111111111111111";
const PRODUCER_COMMIT: &str = "3333333333333333333333333333333333333333";
const TOOLS_COMMIT: &str = "5555555555555555555555555555555555555555";
const PHASES: [&str; 6] = [
    "preflight",
    "environment",
    "configure",
    "compiler",
    "collector",
    "publish",
];

struct Fixture {
    _temporary: TempDir,
    root: PathBuf,
    work: PathBuf,
    output: PathBuf,
    candidate: PathBuf,
    package_dir: PathBuf,
    recipe_path: PathBuf,
    source_lock_path: PathBuf,
    profiles_path: PathBuf,
    environment_path: PathBuf,
    build_result_path: PathBuf,
    build_result_sha256: Sha256Digest,
    candidate_receipt_sha256: Sha256Digest,
    candidate_payload_sha256: Sha256Digest,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let work = create_dir(&root.join("work"));
        let output = create_dir(&root.join("output"));
        let candidate = output.join("toolchain");
        let receipts = work.join("native-lifecycle/receipts");
        let stage = output.join(".aros-native-toolchain-stage");
        fs::create_dir_all(&receipts).unwrap();
        fs::create_dir_all(stage.join("toolchain/bin")).unwrap();
        write_candidate(&candidate);
        let checkpoint = b"synthetic compiler checkpoint\n";
        fs::write(stage.join("toolchain/bin/llvm-config"), checkpoint).unwrap();

        let source_lock_bytes = fixture_source_lock();
        let profiles_bytes = fixture_profiles();
        let recipe = fixture_recipe(&source_lock_bytes, &profiles_bytes);
        let recipe_bytes = serde_json::to_vec(&recipe).unwrap();
        let recipe_path = root.join("recipe.json");
        let source_lock_path = root.join("source-lock.json");
        let profiles_path = root.join("profiles.json");
        let environment_path = root.join("build-environment.json");
        fs::write(&recipe_path, recipe_bytes).unwrap();
        fs::write(&source_lock_path, &source_lock_bytes).unwrap();
        fs::write(&profiles_path, &profiles_bytes).unwrap();
        fs::write(
            &environment_path,
            serde_json::to_vec(&json!({
                "schema": "aros-toolchain-build-environment-v1",
                "host": "linux-x86_64"
            }))
            .unwrap(),
        )
        .unwrap();

        let identity = json!({
            "recipe_sha256": recipe["recipe_sha256"],
            "source_commit": SOURCE_COMMIT,
            "producer_commit": PRODUCER_COMMIT,
            "tools_commit": TOOLS_COMMIT,
            "host": "linux-x86_64",
            "target_profile": "pc-x86_64",
            "executor": {
                "contract_id": "aros-toolchain-producer-v1",
                "contract_sha256": sha256_bytes(b"synthetic producer contract"),
                "tools_commit": TOOLS_COMMIT,
                "binary_sha256": sha256_bytes(b"synthetic frontend observation"),
                "origin_evidence_sha256": null
            }
        });
        let phase_receipt_digests =
            write_phase_chain(&work, &output, &candidate, &receipts, &identity, checkpoint);
        let snapshot = measure_tree_content_cas(&candidate).unwrap();
        let candidate_payload_sha256 = snapshot.payload_digest_excluding(None);
        let mut finished = json!({
            "schema": "aros-toolchain-finished-candidate-v2",
            "identity": identity,
            "compiler": {"family": "llvm", "version": "17.0.6"},
            "target_triple": "x86_64-unknown-aros",
            "source_lock_sha256": sha256_bytes(&source_lock_bytes),
            "profiles_sha256": sha256_bytes(&profiles_bytes),
            "candidate_root": candidate,
            "phase_receipt_digests": phase_receipt_digests,
            "payload_sha256": candidate_payload_sha256,
            "entry_count": u64::try_from(snapshot.entry_count()).unwrap(),
            "regular_file_bytes": snapshot.regular_file_bytes().unwrap(),
            "receipt_sha256": sha256_bytes(b"placeholder finished receipt")
        });
        let candidate_receipt_sha256 = seal(&mut finished, "receipt_sha256");
        write_canonical(&receipts.join("finished-candidate.json"), &finished);

        let publish_receipt: Value =
            serde_json::from_slice(&fs::read(receipts.join("publish.json")).unwrap()).unwrap();
        let mut evidence = PHASES
            .iter()
            .enumerate()
            .map(|(index, phase)| {
                json!({
                    "check": phase,
                    "status": "passed",
                    "report_sha256": phase_receipt_digests[index]
                })
            })
            .collect::<Vec<_>>();
        evidence.push(json!({
            "check": "finished-candidate",
            "status": "passed",
            "report_sha256": candidate_receipt_sha256
        }));
        evidence.push(json!({
            "check": "origin",
            "status": "not-run",
            "report_sha256": null
        }));
        let build_result = json!({
            "schema": "aros-toolchain-result-v1",
            "operation": "build",
            "identity": identity,
            "output_root": output,
            "outputs": publish_receipt["outputs"],
            "evidence": evidence,
            "qualification": "local-only",
            "commit_state": "committed"
        });
        let build_result_path = root.join("build-result.json");
        let build_result_sha256 = write_result(&build_result_path, &build_result);

        Self {
            _temporary: temporary,
            root: root.clone(),
            work,
            output,
            candidate,
            package_dir: root.join("package-set"),
            recipe_path,
            source_lock_path,
            profiles_path,
            environment_path,
            build_result_path,
            build_result_sha256,
            candidate_receipt_sha256,
            candidate_payload_sha256,
        }
    }

    fn package_command(
        &self,
        selected_result_sha256: &Sha256Digest,
        build_work_dir: &Path,
    ) -> Command {
        let mut command = self.producer_command("package");
        command
            .arg("--input-dir")
            .arg(&self.candidate)
            .arg("--output-dir")
            .arg(&self.package_dir)
            .args(["--package-format", "family-v2"])
            .arg("--build-result")
            .arg(&self.build_result_path)
            .arg("--build-result-sha256")
            .arg(selected_result_sha256.as_str())
            .arg("--build-work-dir")
            .arg(build_work_dir)
            .arg("--format")
            .arg("json");
        command
    }

    fn verify_package_command(&self) -> Command {
        let mut command = self.producer_command("verify-package");
        command.arg("--input-dir").arg(&self.package_dir).args([
            "--package-format",
            "family-v2",
            "--format",
            "json",
        ]);
        command
    }

    fn producer_command(&self, leaf: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command
            .current_dir(&self.root)
            .env_remove("AROS_LOG_FILE")
            .env_remove("AROS_LOG_LEVEL")
            .env_remove("AROS_LOG_FORMAT")
            .env_remove("AROS_DIAGNOSTIC_FORMAT")
            .arg("--diagnostic-format=json")
            .args(["toolchain", "producer", leaf])
            .arg("--recipe")
            .arg(&self.recipe_path)
            .arg("--source-lock")
            .arg(&self.source_lock_path)
            .arg("--profiles")
            .arg(&self.profiles_path)
            .args([
                "--preset",
                "pc-x86_64",
                "--release-id",
                "finished-cli-fixture",
            ])
            .args(["--host", "linux-x86_64"])
            .arg("--build-environment")
            .arg(&self.environment_path)
            .arg("--forbidden-prefix")
            .arg(&self.work)
            .arg("--forbidden-prefix")
            .arg(&self.output);
        command
    }

    fn rewrite_build_result(&mut self, value: &Value) {
        self.build_result_sha256 = write_result(&self.build_result_path, value);
    }
}

#[test]
fn packages_and_independently_verifies_a_synthetic_finished_build_result() {
    let fixture = Fixture::new();
    let output = fixture
        .package_command(&fixture.build_result_sha256, &fixture.work)
        .output()
        .unwrap();
    let result = output_json(&output);
    assert_eq!(result["operation"], "package");

    let package_dir = PathBuf::from(result["package_dir"].as_str().unwrap());
    let entries = fs::read_dir(&package_dir)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        entries.len(),
        4,
        "expected archive, manifest, checksum, and SBOM"
    );
    assert!(entries
        .iter()
        .all(|entry| entry.file_type().unwrap().is_file()));

    let manifest_path = PathBuf::from(result["manifest"].as_str().unwrap());
    let manifest: Value = serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["schema"], 2);
    assert_eq!(manifest["host"], "linux-x86_64");
    assert_eq!(manifest["target_profile"], "pc-x86_64");

    let retained = &result["finished_candidate"];
    assert_eq!(retained.as_object().unwrap().len(), 6);
    assert_eq!(
        retained["build_result_sha256"],
        fixture.build_result_sha256.as_str()
    );
    assert_eq!(
        retained["receipt_sha256"],
        fixture.candidate_receipt_sha256.as_str()
    );
    assert_eq!(
        retained["raw_payload_sha256"],
        fixture.candidate_payload_sha256.as_str()
    );
    assert_eq!(retained["package_tree_sha256"], manifest["tree_sha256"]);
    let portable_bytes = retained["portable_measurement"]
        .as_str()
        .unwrap()
        .as_bytes();
    assert_eq!(
        retained["portable_measurement_sha256"],
        sha256_bytes(portable_bytes).as_str()
    );
    let portable: Value = serde_json::from_slice(portable_bytes).unwrap();
    assert_eq!(
        portable["schema"],
        "aros-toolchain-finished-package-measurement-v2"
    );
    assert_eq!(portable["documents"].as_array().unwrap().len(), 8);
    assert_eq!(
        portable["package_members"]["members"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(
        portable["documents"][7]["content"]
            .as_str()
            .unwrap()
            .as_bytes(),
        fs::read(&fixture.build_result_path).unwrap()
    );
    assert_ne!(
        retained["raw_payload_sha256"], retained["package_tree_sha256"],
        "raw candidate and normalized package trees have distinct identities"
    );

    let verified = output_json(&fixture.verify_package_command().output().unwrap());
    assert_eq!(verified["operation"], "verify-package");
    assert_eq!(verified["sha256"], result["sha256"]);
}

#[test]
fn rejects_finished_package_counterprobes_without_touching_source_inputs() {
    for counterprobe in [
        "wrong external digest",
        "changed compiler payload",
        "collector-only result",
        "wrong work directory",
        "changed result with reselected digest",
        "wrong result host",
        "unknown result field",
    ] {
        let mut fixture = Fixture::new();
        let mut selected_digest = fixture.build_result_sha256.clone();
        let mut selected_work = fixture.work.clone();
        let expected_message = match counterprobe {
            "wrong external digest" => {
                selected_digest = sha256_bytes(b"unselected native build result");
                "selected native build result bytes differ from their external digest"
            }
            "changed compiler payload" => {
                fs::write(
                    fixture.candidate.join("bin/clang"),
                    b"changed compiler payload\n",
                )
                .unwrap();
                "finished candidate receipt does not match the complete payload"
            }
            "collector-only result" => {
                let mut result: Value =
                    serde_json::from_slice(&fs::read(&fixture.build_result_path).unwrap()).unwrap();
                result["evidence"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|entry| entry["check"] != "finished-candidate");
                fixture.rewrite_build_result(&result);
                selected_digest = fixture.build_result_sha256.clone();
                "complete ordered evidence set"
            }
            "wrong work directory" => {
                selected_work = create_dir(&fixture.root.join("wrong-work"));
                "candidate receipts directory is unsafe"
            }
            "changed result with reselected digest" => {
                let mut result: Value =
                    serde_json::from_slice(&fs::read(&fixture.build_result_path).unwrap()).unwrap();
                result["qualification"] = json!("release-qualified");
                fixture.rewrite_build_result(&result);
                selected_digest = fixture.build_result_sha256.clone();
                "differs from the selected operation, roots, host or local state"
            }
            "wrong result host" => {
                let mut result: Value =
                    serde_json::from_slice(&fs::read(&fixture.build_result_path).unwrap()).unwrap();
                result["identity"]["host"] = json!("linux-aarch64");
                fixture.rewrite_build_result(&result);
                selected_digest = fixture.build_result_sha256.clone();
                "differs from the selected operation, roots, host or local state"
            }
            "unknown result field" => {
                let mut result: Value =
                    serde_json::from_slice(&fs::read(&fixture.build_result_path).unwrap()).unwrap();
                result["future_field"] = json!(true);
                fixture.rewrite_build_result(&result);
                selected_digest = fixture.build_result_sha256.clone();
                "violates its closed result-v1 schema"
            }
            _ => unreachable!(),
        };

        let before = inventory(&fixture.root);
        let output = fixture
            .package_command(&selected_digest, &selected_work)
            .output()
            .unwrap();
        assert_failure(&output, "AX0801", expected_message);
        assert!(
            !fixture.package_dir.exists(),
            "{counterprobe} created package output despite rejection"
        );
        assert_eq!(
            inventory(&fixture.root),
            before,
            "{counterprobe} changed source receipts or candidate inputs"
        );
    }
}

fn fixture_source_lock() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v2",
        "family": "llvm",
        "version": "17.0.6",
        "sources": [{
            "component": "llvm",
            "version": "17.0.6",
            "purpose": "toolchain-component",
            "filename": "llvm-17.0.6.tar.xz",
            "url": "https://example.invalid/llvm-17.0.6.tar.xz",
            "sha256": "a".repeat(64),
            "size": 1
        }],
        "host_python_packages": [{
            "name": "wheel",
            "version": "0.43.0",
            "filename": "wheel-0.43.0.tar.gz",
            "url": "https://example.invalid/wheel-0.43.0.tar.gz",
            "sha256": "b".repeat(64),
            "size": 1,
            "source_root": "wheel-0.43.0",
            "python_path": "."
        }]
    }))
    .unwrap()
}

fn fixture_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v1",
        "upstream_commit": "7777777777777777777777777777777777777777",
        "profiles": [{
            "name": "pc-x86_64",
            "configure_target": "pc-x86_64",
            "upstream_output_target": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros",
            "cpu": "x86_64",
            "platform": "pc",
            "float_abi": "",
            "capabilities": ["c", "cxx", "standalone-collector"]
        }]
    }))
    .unwrap()
}

fn fixture_recipe(source_lock: &[u8], profiles: &[u8]) -> Value {
    let mut value = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": SOURCE_COMMIT,
        "source_tree": "2222222222222222222222222222222222222222",
        "producer_commit": PRODUCER_COMMIT,
        "producer_tree": "4444444444444444444444444444444444444444",
        "tools_commit": TOOLS_COMMIT,
        "tools_tree": "6666666666666666666666666666666666666666",
        "source_date_epoch": 946_684_800_u64,
        "source_lock_sha256": sha256_bytes(source_lock),
        "profiles_sha256": sha256_bytes(profiles),
        "patches": [],
        "recipe_sha256": sha256_bytes(b"placeholder recipe digest")
    });
    seal(&mut value, "recipe_sha256");
    value
}

fn write_candidate(candidate: &Path) {
    for relative in ["bin", "libexec", "target/empty", "include/empty"] {
        fs::create_dir_all(candidate.join(relative)).unwrap();
    }
    write_file(
        candidate,
        "bin/clang",
        b"synthetic compiler payload\n",
        0o751,
    );
    write_file(
        candidate,
        "bin/aros-collect",
        b"synthetic published collector\n",
        0o755,
    );
    fs::write(
        candidate.join("libexec/llvm-ar.real"),
        b"synthetic archiver payload\n",
    )
    .unwrap();
    fs::write(
        candidate.join(".installflag-fixture"),
        b"normalization marker\n",
    )
    .unwrap();
    fs::write(
        candidate.join(AROS_TOOLCHAIN_MANIFEST_FILE),
        b"synthetic prior embedded manifest\n",
    )
    .unwrap();
    symlink("clang", candidate.join("bin/clang++")).unwrap();
    symlink("../libexec/llvm-ar.real", candidate.join("bin/llvm-ar")).unwrap();
    symlink("aros-collect", candidate.join("bin/collect-aros")).unwrap();
    symlink("aros-collect", candidate.join("bin/collect-aros32")).unwrap();
}

fn write_phase_chain(
    work: &Path,
    output: &Path,
    candidate: &Path,
    receipts: &Path,
    identity: &Value,
    checkpoint: &[u8],
) -> Vec<Sha256Digest> {
    let mut digests: Vec<Sha256Digest> = Vec::with_capacity(PHASES.len());
    for (index, phase) in PHASES.iter().enumerate() {
        let output_root = if index < 3 {
            work.join("native-lifecycle")
        } else if index == 5 {
            candidate.to_path_buf()
        } else {
            output.join(".aros-native-toolchain-stage")
        };
        let outputs = if index == 3 {
            vec![json!({
                "path": "toolchain/bin/llvm-config",
                "kind": "file",
                "sha256": sha256_bytes(checkpoint),
                "size": checkpoint.len()
            })]
        } else if index == 5 {
            let collector = fs::read(candidate.join("bin/aros-collect")).unwrap();
            vec![json!({
                "path": "toolchain/bin/aros-collect",
                "kind": "file",
                "sha256": sha256_bytes(&collector),
                "size": collector.len()
            })]
        } else {
            Vec::new()
        };
        let previous = index
            .checked_sub(1)
            .map(|previous| json!(digests[previous].as_str()));
        let mut receipt = json!({
            "schema": "aros-toolchain-receipt-v1",
            "identity": identity,
            "phase": phase,
            "input_sha256": sha256_bytes(phase.as_bytes()),
            "output_root": output_root,
            "outputs": outputs,
            "previous_receipt_sha256": previous,
            "receipt_sha256": sha256_bytes(b"placeholder phase receipt")
        });
        let digest = seal(&mut receipt, "receipt_sha256");
        write_canonical(&receipts.join(format!("{phase}.json")), &receipt);
        digests.push(digest);
    }
    digests
}

fn write_file(root: &Path, relative: &str, bytes: &[u8], mode: u32) {
    let path = root.join(relative);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn create_dir(path: &Path) -> PathBuf {
    fs::create_dir(path).unwrap();
    path.canonicalize().unwrap()
}

fn seal(value: &mut Value, field: &str) -> Sha256Digest {
    value
        .as_object_mut()
        .unwrap()
        .remove(field)
        .expect("self-digest field");
    let digest = sha256_bytes(&aros_toolchain::canonical::bytes(value).unwrap());
    value[field] = json!(digest.as_str());
    digest
}

fn write_canonical(path: &Path, value: &Value) {
    fs::write(path, aros_toolchain::canonical::bytes(value).unwrap()).unwrap();
}

fn write_result(path: &Path, value: &Value) -> Sha256Digest {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    let digest = sha256_bytes(&bytes);
    fs::write(path, bytes).unwrap();
    digest
}

fn output_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_failure(output: &Output, code: &str, expected_message: &str) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(diagnostic["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(diagnostic["diagnostics"][0]["code"], code);
    assert!(
        diagnostic["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains(expected_message),
        "unexpected diagnostic: {diagnostic}"
    );
}

type Inventory = Vec<(PathBuf, u32, &'static str, Vec<u8>)>;

fn inventory(root: &Path) -> Inventory {
    fn visit(root: &Path, directory: &Path, result: &mut Inventory) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let mode = metadata.permissions().mode() & 0o7777;
            let (kind, bytes) = if metadata.file_type().is_dir() {
                visit(root, &path, result);
                ("directory", Vec::new())
            } else if metadata.file_type().is_symlink() {
                (
                    "symlink",
                    fs::read_link(&path)
                        .unwrap()
                        .to_string_lossy()
                        .as_bytes()
                        .to_vec(),
                )
            } else {
                ("file", fs::read(&path).unwrap())
            };
            result.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                mode,
                kind,
                bytes,
            ));
        }
    }

    let mut result = Vec::new();
    visit(root, root, &mut result);
    result.sort_by(|left, right| left.0.cmp(&right.0));
    result
}
