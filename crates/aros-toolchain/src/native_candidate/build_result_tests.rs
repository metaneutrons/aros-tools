//! Synthetic adapter tests; fixtures are local files and execute no producer.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, Sha256Digest};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::super::{persist_finished_candidate, PhaseReceipt, PHASES};
use super::{readback_finished_build_result, FinishedBuildResultRequest};
use crate::native_candidate::FinishedCandidateRequest;
use crate::plan::{Executor, Identity};
use crate::profiles::{Profile, Profiles};
use crate::recipe::{GitObjectId, Recipe};
use crate::source_lock::SourceLock;

const SOURCE_COMMIT: &str = "1111111111111111111111111111111111111111";
const PRODUCER_COMMIT: &str = "3333333333333333333333333333333333333333";
const TOOLS_COMMIT: &str = "5555555555555555555555555555555555555555";

struct Fixture {
    temporary: TempDir,
    work: PathBuf,
    output: PathBuf,
    build_result: PathBuf,
    recipe: Recipe,
    source_lock: SourceLock,
    profile: Profile,
    identity: Identity,
    phase_receipt_digests: [Sha256Digest; 6],
    finished_candidate_digest: Sha256Digest,
    build_result_sha256: Sha256Digest,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let work = create_canonical_dir(&temporary.path().join("work"));
        let output = create_canonical_dir(&temporary.path().join("output"));
        let build_result = temporary.path().join("build-result.json");
        let receipts = work.join("native-lifecycle/receipts");
        let stage = output.join(".aros-native-toolchain-stage/toolchain/bin");
        let candidate = output.join("toolchain/bin");
        fs::create_dir_all(&receipts).unwrap();
        fs::create_dir_all(&stage).unwrap();
        fs::create_dir_all(&candidate).unwrap();
        fs::write(
            stage.join("llvm-config"),
            b"checkpoint removed by cleanup\n",
        )
        .unwrap();
        write_file(
            &candidate.join("aros-collect"),
            b"synthetic published collector\n",
            0o755,
        );

        let source_lock_bytes = fixture_source_lock();
        let profiles_bytes = fixture_profiles();
        let recipe = Recipe::parse(&fixture_recipe(&source_lock_bytes, &profiles_bytes)).unwrap();
        let source_lock = SourceLock::parse(&source_lock_bytes).unwrap();
        let profile = Profiles::parse(&profiles_bytes)
            .unwrap()
            .select("pc-x86_64")
            .unwrap()
            .clone();
        let tools_commit = git_id(TOOLS_COMMIT);
        let identity = Identity {
            recipe_sha256: recipe.sha256().clone(),
            source_commit: recipe.source().0.clone(),
            producer_commit: recipe.producer().0.clone(),
            tools_commit: tools_commit.clone(),
            host: "linux-x86_64",
            target_profile: profile.name().to_owned(),
            executor: Executor {
                contract_id: Some("aros-toolchain-producer-v1"),
                contract_sha256: Some(sha256_bytes(b"synthetic producer contract")),
                tools_commit: Some(tools_commit),
                binary_sha256: sha256_bytes(b"synthetic frontend observation"),
                origin_evidence_sha256: None,
            },
        };
        let mut fixture = Self {
            temporary,
            work,
            output,
            build_result,
            recipe,
            source_lock,
            profile,
            identity,
            phase_receipt_digests: std::array::from_fn(|_| sha256_bytes(b"unset phase")),
            finished_candidate_digest: sha256_bytes(b"unset finished candidate"),
            build_result_sha256: sha256_bytes(b"unset build result"),
        };
        fixture.write_phase_chain();
        fixture.persist_finished_candidate();
        let build_result = fixture.build_result_value();
        fixture.write_build_result(&build_result);
        fixture
    }

    fn request<'a>(&'a self, digest: &'a Sha256Digest) -> FinishedBuildResultRequest<'a> {
        FinishedBuildResultRequest {
            work_dir: &self.work,
            output_dir: &self.output,
            recipe: &self.recipe,
            source_lock: &self.source_lock,
            profile: &self.profile,
            host: self.identity.host,
            build_result: &self.build_result,
            build_result_sha256: digest,
        }
    }

    fn selected_request(&self) -> FinishedBuildResultRequest<'_> {
        self.request(&self.build_result_sha256)
    }

    fn readback(&self) -> Result<super::super::FinishedCandidateReadback, crate::ContractError> {
        readback_finished_build_result(&self.selected_request())
    }

    fn receipts_dir(&self) -> PathBuf {
        self.work.join("native-lifecycle/receipts")
    }

    fn write_phase_chain(&mut self) {
        let directory = self.receipts_dir();
        let identity = serde_json::to_value(&self.identity).unwrap();
        let collector_path = self.output.join("toolchain/bin/aros-collect");
        let collector_bytes = fs::read(&collector_path).unwrap();
        for (index, phase) in PHASES.iter().enumerate() {
            let output_root = if index < 3 {
                self.work.join("native-lifecycle")
            } else if index == 5 {
                self.output.join("toolchain")
            } else {
                self.output.join(".aros-native-toolchain-stage")
            };
            let outputs = if index == 3 {
                vec![json!({
                    "path": "toolchain/bin/llvm-config",
                    "kind": "file",
                    "sha256": sha256_bytes(b"checkpoint removed by cleanup\n"),
                    "size": b"checkpoint removed by cleanup\n".len()
                })]
            } else if index == 5 {
                vec![json!({
                    "path": "toolchain/bin/aros-collect",
                    "kind": "file",
                    "sha256": sha256_bytes(&collector_bytes),
                    "size": collector_bytes.len()
                })]
            } else {
                Vec::new()
            };
            let previous = index
                .checked_sub(1)
                .map(|previous| self.phase_receipt_digests[previous].clone());
            let mut value = json!({
                "schema": "aros-toolchain-receipt-v1",
                "identity": identity,
                "phase": phase,
                "input_sha256": sha256_bytes(phase.as_bytes()),
                "output_root": output_root,
                "outputs": outputs,
                "previous_receipt_sha256": previous,
                "receipt_sha256": sha256_bytes(b"placeholder")
            });
            self.phase_receipt_digests[index] = replace_self_digest(&mut value);
            fs::write(
                directory.join(format!("{phase}.json")),
                crate::canonical::bytes(&value).unwrap(),
            )
            .unwrap();
        }
    }

    fn persist_finished_candidate(&mut self) {
        fs::remove_file(
            self.output
                .join(".aros-native-toolchain-stage/toolchain/bin/llvm-config"),
        )
        .unwrap();
        let placeholder = sha256_bytes(b"writer ignores selected digest");
        self.finished_candidate_digest = persist_finished_candidate(&FinishedCandidateRequest {
            work_dir: &self.work,
            output_dir: &self.output,
            recipe: &self.recipe,
            source_lock: &self.source_lock,
            profile: &self.profile,
            identity: &self.identity,
            phase_receipt_digests: &self.phase_receipt_digests,
            candidate_receipt_digest: &placeholder,
        })
        .unwrap();
    }

    fn build_result_value(&self) -> Value {
        let publish_bytes = fs::read(self.receipts_dir().join("publish.json")).unwrap();
        let publish: PhaseReceipt = serde_json::from_slice(&publish_bytes).unwrap();
        let mut evidence = PHASES
            .iter()
            .enumerate()
            .map(|(index, phase)| {
                json!({
                    "check": phase,
                    "status": "passed",
                    "report_sha256": self.phase_receipt_digests[index]
                })
            })
            .collect::<Vec<_>>();
        evidence.push(json!({
            "check": "finished-candidate",
            "status": "passed",
            "report_sha256": self.finished_candidate_digest
        }));
        evidence.push(json!({
            "check": "origin",
            "status": "not-run",
            "report_sha256": null
        }));
        json!({
            "schema": "aros-toolchain-result-v1",
            "operation": "build",
            "identity": self.identity,
            "output_root": &self.output,
            "outputs": publish.outputs,
            "evidence": evidence,
            "qualification": "local-only",
            "commit_state": "committed"
        })
    }

    fn write_build_result(&mut self, value: &Value) {
        let mut bytes = serde_json::to_vec_pretty(value).unwrap();
        bytes.push(b'\n');
        self.write_build_result_bytes(bytes);
    }

    fn write_build_result_bytes(&mut self, bytes: Vec<u8>) {
        self.build_result_sha256 = sha256_bytes(&bytes);
        fs::write(&self.build_result, bytes).unwrap();
    }
}

#[test]
fn accepts_result_after_old_llvm_checkpoint_cleanup_and_retains_result_bytes() {
    let fixture = Fixture::new();
    let before = snapshot_tree(fixture.temporary.path());
    let proof = fixture.readback().unwrap();
    assert_eq!(proof.receipt_sha256(), &fixture.finished_candidate_digest);
    assert_eq!(snapshot_tree(fixture.temporary.path()), before);

    fs::write(&fixture.build_result, b"changed after read-back\n").unwrap();
    assert!(proof.revalidate().is_err());
}

#[test]
fn rejects_result_digest_mismatch_without_writing() {
    let fixture = Fixture::new();
    let before = snapshot_tree(fixture.temporary.path());
    let wrong_digest = sha256_bytes(b"another selected result");
    assert!(readback_finished_build_result(&fixture.request(&wrong_digest)).is_err());
    assert_eq!(snapshot_tree(fixture.temporary.path()), before);
}

#[test]
fn rejects_closed_schema_partial_evidence_output_mismatch_and_origin_claims() {
    for mutation in [
        "unknown field",
        "nested unknown field",
        "missing nullable executor field",
        "missing nullable evidence field",
        "partial evidence",
        "reordered evidence",
        "qualified state",
        "uncommitted state",
        "outputs",
        "output root",
        "host",
        "origin",
    ] {
        let mut fixture = Fixture::new();
        let mut value = fixture.build_result_value();
        match mutation {
            "unknown field" => value["unrecognized"] = json!(true),
            "nested unknown field" => value["evidence"][0]["unrecognized"] = json!(true),
            "missing nullable executor field" => {
                value["identity"]["executor"]
                    .as_object_mut()
                    .unwrap()
                    .remove("origin_evidence_sha256");
            }
            "missing nullable evidence field" => {
                value["evidence"][7]
                    .as_object_mut()
                    .unwrap()
                    .remove("report_sha256");
            }
            "partial evidence" => {
                value["evidence"].as_array_mut().unwrap().pop();
            }
            "reordered evidence" => value["evidence"].as_array_mut().unwrap().swap(0, 1),
            "qualified state" => value["qualification"] = json!("release-qualified"),
            "uncommitted state" => value["commit_state"] = json!("staged"),
            "outputs" => value["outputs"] = json!([]),
            "output root" => value["output_root"] = json!("/different/output"),
            "host" => value["identity"]["host"] = json!("linux-aarch64"),
            "origin" => {
                value["identity"]["executor"]["origin_evidence_sha256"] =
                    json!(sha256_bytes(b"claimed origin").as_str());
                value["evidence"][7]["status"] = json!("passed");
                value["evidence"][7]["report_sha256"] = json!(sha256_bytes(b"origin report"));
            }
            _ => unreachable!(),
        }
        fixture.write_build_result(&value);
        let before = snapshot_tree(fixture.temporary.path());
        assert!(
            fixture.readback().is_err(),
            "build result accepted {mutation} mutation"
        );
        assert_eq!(snapshot_tree(fixture.temporary.path()), before);
    }
}

#[test]
fn rejects_duplicate_result_fields_without_writing() {
    let mut fixture = Fixture::new();
    let bytes = fs::read(&fixture.build_result).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let duplicated = text.replacen(
        "\"operation\": \"build\"",
        "\"operation\": \"build\",\n  \"operation\": \"build\"",
        1,
    );
    assert_ne!(duplicated, text);
    fixture.write_build_result_bytes(duplicated.into_bytes());
    let before = snapshot_tree(fixture.temporary.path());
    assert!(fixture.readback().is_err());
    assert_eq!(snapshot_tree(fixture.temporary.path()), before);
}

#[test]
fn rejects_symlink_result_and_oversized_result_without_writing() {
    for mutation in ["symlink", "oversized"] {
        let mut fixture = Fixture::new();
        match mutation {
            "symlink" => {
                let retained = fixture.temporary.path().join("retained.json");
                fs::rename(&fixture.build_result, &retained).unwrap();
                std::os::unix::fs::symlink(retained, &fixture.build_result).unwrap();
            }
            "oversized" => {
                fixture
                    .write_build_result_bytes(vec![b' '; crate::canonical::MAX_DOCUMENT_BYTES + 1]);
            }
            _ => unreachable!(),
        }
        let before = snapshot_tree(fixture.temporary.path());
        assert!(fixture.readback().is_err(), "accepted {mutation} result");
        assert_eq!(snapshot_tree(fixture.temporary.path()), before);
    }
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn create_canonical_dir(path: &Path) -> PathBuf {
    fs::create_dir(path).unwrap();
    path.canonicalize().unwrap()
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

fn fixture_recipe(source_lock: &[u8], profiles: &[u8]) -> Vec<u8> {
    let mut value = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": SOURCE_COMMIT,
        "source_tree": "2222222222222222222222222222222222222222",
        "producer_commit": PRODUCER_COMMIT,
        "producer_tree": "4444444444444444444444444444444444444444",
        "tools_commit": TOOLS_COMMIT,
        "tools_tree": "6666666666666666666666666666666666666666",
        "source_date_epoch": 946_684_800_u64,
        "source_lock_sha256": sha256_bytes(source_lock).as_str(),
        "profiles_sha256": sha256_bytes(profiles).as_str(),
        "patches": []
    });
    let digest = sha256_bytes(&crate::canonical::bytes(&value).unwrap());
    value["recipe_sha256"] = json!(digest.as_str());
    serde_json::to_vec(&value).unwrap()
}

fn git_id(value: &str) -> GitObjectId {
    GitObjectId::try_from(value.to_owned()).unwrap()
}

fn replace_self_digest(value: &mut Value) -> Sha256Digest {
    value
        .as_object_mut()
        .unwrap()
        .remove("receipt_sha256")
        .expect("receipt self-digest field");
    let digest = sha256_bytes(&crate::canonical::bytes(value).unwrap());
    value["receipt_sha256"] = json!(digest.as_str());
    digest
}

#[derive(Debug, PartialEq, Eq)]
enum SnapshotEntry {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, SnapshotEntry> {
    fn visit(root: &Path, directory: &Path, entries: &mut BTreeMap<PathBuf, SnapshotEntry>) {
        for item in fs::read_dir(directory).unwrap() {
            let item = item.unwrap();
            let path = item.path();
            let relative = path.strip_prefix(root).unwrap().to_owned();
            let kind = item.file_type().unwrap();
            if kind.is_dir() {
                entries.insert(relative, SnapshotEntry::Directory);
                visit(root, &path, entries);
            } else if kind.is_file() {
                entries.insert(relative, SnapshotEntry::File(fs::read(path).unwrap()));
            } else if kind.is_symlink() {
                entries.insert(
                    relative,
                    SnapshotEntry::Symlink(fs::read_link(path).unwrap()),
                );
            } else {
                panic!("unexpected fixture filesystem entry");
            }
        }
    }

    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}
