//! Synthetic finished-candidate boundary tests.
//!
//! Fixture bytes stand in for compiler and collector outputs. These tests do
//! not execute an actual compiler or any producer command.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Path, PathBuf};

use aros_common::{
    copy_tree_from_snapshot_nofollow, sha256_bytes, Sha256Digest, AROS_TOOLCHAIN_MANIFEST_FILE,
};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

use super::{
    package_finished_candidate, persist_finished_candidate, readback_finished_candidate,
    FinishedCandidatePackage, FinishedCandidateReadback, FinishedCandidateRequest, PhaseOutput,
    PhaseReceipt, PHASES,
};
use crate::package::PackageRequest;
use crate::plan::{Executor, Identity};
use crate::profiles::{Profile, Profiles};
use crate::recipe::{GitObjectId, Recipe};
use crate::source_lock::SourceLock;

const SOURCE_COMMIT: &str = "1111111111111111111111111111111111111111";
const PRODUCER_COMMIT: &str = "3333333333333333333333333333333333333333";
const TOOLS_COMMIT: &str = "5555555555555555555555555555555555555555";
const CANDIDATE_RECEIPT: &str = "finished-candidate.json";

pub(super) struct Fixture {
    temporary: TempDir,
    work: PathBuf,
    output: PathBuf,
    recipe: Recipe,
    source_lock: SourceLock,
    profile: Profile,
    pub(super) identity: Identity,
    phase_receipt_digests: [Sha256Digest; 6],
    candidate_receipt_digest: Option<Sha256Digest>,
}

impl Fixture {
    pub(super) fn new() -> Self {
        Self::new_for_host("linux-x86_64")
    }

    pub(super) fn new_for_host(host: &'static str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let work = create_canonical_dir(&temporary.path().join("work"));
        let output = create_canonical_dir(&temporary.path().join("output"));
        let lifecycle = work.join("native-lifecycle");
        let receipts = lifecycle.join("receipts");
        let stage = output.join(".aros-native-toolchain-stage");
        let candidate = output.join("toolchain");
        fs::create_dir(&lifecycle).unwrap();
        fs::create_dir(&receipts).unwrap();
        fs::create_dir(&stage).unwrap();
        let old_llvm_config = stage.join("toolchain/bin/llvm-config");
        fs::create_dir_all(old_llvm_config.parent().unwrap()).unwrap();
        fs::write(&old_llvm_config, b"synthetic pre-cleanup llvm-config\n").unwrap();
        write_candidate(&candidate);

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
            host,
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
            recipe,
            source_lock,
            profile,
            identity,
            phase_receipt_digests: std::array::from_fn(|_| sha256_bytes(b"unset phase")),
            candidate_receipt_digest: None,
        };
        fixture.write_phase_chain();
        fixture
    }

    fn request<'a>(
        &'a self,
        identity: &'a Identity,
        phases: &'a [Sha256Digest; 6],
        candidate_receipt_digest: &'a Sha256Digest,
    ) -> FinishedCandidateRequest<'a> {
        FinishedCandidateRequest {
            work_dir: &self.work,
            output_dir: &self.output,
            recipe: &self.recipe,
            source_lock: &self.source_lock,
            profile: &self.profile,
            identity,
            phase_receipt_digests: phases,
            candidate_receipt_digest,
        }
    }

    fn selected_request(&self) -> FinishedCandidateRequest<'_> {
        self.request(
            &self.identity,
            &self.phase_receipt_digests,
            self.candidate_receipt_digest
                .as_ref()
                .expect("finished-candidate receipt has been persisted"),
        )
    }

    pub(super) fn persist(&mut self) -> Sha256Digest {
        let placeholder = sha256_bytes(b"writer ignores this selected digest");
        let digest = {
            let request = self.request(&self.identity, &self.phase_receipt_digests, &placeholder);
            persist_finished_candidate(&request).unwrap()
        };
        self.candidate_receipt_digest = Some(digest.clone());
        digest
    }

    pub(super) fn readback(&self) -> Result<FinishedCandidateReadback, crate::ContractError> {
        readback_finished_candidate(&self.selected_request())
    }

    pub(super) fn receipts_dir(&self) -> PathBuf {
        self.work.join("native-lifecycle/receipts")
    }

    pub(super) fn candidate(&self) -> PathBuf {
        self.output.join("toolchain")
    }

    pub(super) fn remove_old_compiler_checkpoint(&self) {
        fs::remove_file(
            self.output
                .join(".aros-native-toolchain-stage/toolchain/bin/llvm-config"),
        )
        .unwrap();
    }

    pub(super) fn write_phase_chain(&mut self) {
        let directory = self.receipts_dir();
        let identity =
            serde_json::to_value(super::owned_identity(&self.identity).unwrap()).unwrap();
        let collector_path = self.candidate().join("bin/aros-collect");
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
                let bytes = b"synthetic pre-cleanup llvm-config\n";
                vec![serde_json::to_value(PhaseOutput {
                    path: "toolchain/bin/llvm-config".into(),
                    kind: "file".into(),
                    sha256: sha256_bytes(bytes),
                    size: u64::try_from(bytes.len()).unwrap(),
                })
                .unwrap()]
            } else if index == 5 {
                vec![serde_json::to_value(PhaseOutput {
                    path: "toolchain/bin/aros-collect".into(),
                    kind: "file".into(),
                    sha256: sha256_bytes(&collector_bytes),
                    size: u64::try_from(collector_bytes.len()).unwrap(),
                })
                .unwrap()]
            } else {
                Vec::new()
            };
            let previous = index
                .checked_sub(1)
                .map(|previous| self.phase_receipt_digests[previous].clone());
            let mut value = serde_json::to_value(PhaseReceipt {
                schema: "aros-toolchain-receipt-v1".into(),
                identity: serde_json::from_value(identity.clone()).unwrap(),
                phase: (*phase).into(),
                input_sha256: sha256_bytes(phase.as_bytes()),
                output_root,
                outputs: serde_json::from_value(Value::Array(outputs)).unwrap(),
                previous_receipt_sha256: previous,
                receipt_sha256: sha256_bytes(b"placeholder"),
            })
            .unwrap();
            self.phase_receipt_digests[index] = replace_self_digest(&mut value);
            write_canonical(&directory.join(format!("{phase}.json")), &value);
        }
    }
}

#[test]
fn reads_complete_candidate_after_old_llvm_checkpoint_cleanup_and_packages_v2() {
    let mut fixture = Fixture::new();
    fixture.remove_old_compiler_checkpoint();
    let expected_receipt_digest = fixture.persist();
    let proof = fixture.readback().unwrap();

    assert_eq!(proof.receipt_sha256(), &expected_receipt_digest);
    assert_eq!(proof.entry_count(), 15);
    assert!(
        proof.regular_file_bytes() > u64::try_from(b"synthetic compiler payload\n".len()).unwrap()
    );
    assert!(!fixture
        .output
        .join(".aros-native-toolchain-stage/toolchain/bin/llvm-config")
        .exists());
    assert!(!fixture.candidate().join("bin/llvm-config").exists());

    let before = proof.payload_sha256().clone();
    let package_request = package_request(&fixture, fixture.temporary.path().join("package"));
    let package = package_finished_candidate(&package_request, &proof).unwrap();
    assert_finished_v2_package(&package);
    assert_eq!(package.candidate_receipt_sha256(), &expected_receipt_digest);
    assert_eq!(package.candidate_payload_sha256(), &before);
    assert_ne!(
        package.verified().manifest.tree_sha256,
        before.as_str(),
        "the normalized package inventory must not be reported as the raw candidate digest"
    );
    assert!(package
        .verified()
        .manifest
        .files
        .iter()
        .all(|entry| entry.path != ".installflag-fixture"));
    assert!(package.package_output().archive.is_file());
    assert_eq!(
        package.package_output().archive_sha256,
        package.verified().archive_sha256
    );
}

#[test]
fn rejects_complete_tree_mutations_including_compiler_links_modes_and_directories() {
    for mutation in [
        "compiler bytes",
        "link target",
        "raw mode",
        "add directory",
        "remove directory",
    ] {
        let mut fixture = Fixture::new();
        fixture.persist();
        let candidate = fixture.candidate();
        match mutation {
            "compiler bytes" => {
                fs::write(candidate.join("bin/clang"), b"changed compiler\n").unwrap();
            }
            "link target" => {
                fs::remove_file(candidate.join("bin/collect-aros")).unwrap();
                symlink("clang", candidate.join("bin/collect-aros")).unwrap();
            }
            "raw mode" => fs::set_permissions(
                candidate.join("bin/clang"),
                fs::Permissions::from_mode(0o755),
            )
            .unwrap(),
            "add directory" => fs::create_dir(candidate.join("added-empty-directory")).unwrap(),
            "remove directory" => fs::remove_dir(candidate.join("target/empty")).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            fixture.readback().is_err(),
            "finished candidate accepted {mutation} mutation"
        );
    }
}

#[test]
fn rejects_independently_selected_source_host_and_executor_changes() {
    for mutation in ["source", "host", "executor"] {
        let mut fixture = Fixture::new();
        fixture.persist();
        let mut identity = fixture.identity.clone();
        match mutation {
            "source" => identity.source_commit = git_id("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "host" => identity.host = "linux-aarch64",
            "executor" => identity.executor.binary_sha256 = sha256_bytes(b"different executor"),
            _ => unreachable!(),
        }
        let request = fixture.request(
            &identity,
            &fixture.phase_receipt_digests,
            fixture.candidate_receipt_digest.as_ref().unwrap(),
        );
        assert!(
            readback_finished_candidate(&request).is_err(),
            "finished candidate accepted changed expected {mutation}"
        );
    }
}

#[test]
fn rejects_broken_predecessor_even_when_receipt_and_expected_phase_digest_are_rehashed() {
    let mut fixture = Fixture::new();
    fixture.persist();
    let path = fixture.receipts_dir().join("configure.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["previous_receipt_sha256"] = json!(sha256_bytes(b"wrong predecessor").as_str());
    fixture.phase_receipt_digests[2] = replace_self_digest(&mut value);
    write_canonical(&path, &value);

    assert!(fixture.readback().is_err());
}

#[test]
fn rejects_duplicate_and_unknown_phase_receipt_fields() {
    {
        let mut fixture = Fixture::new();
        fixture.persist();
        let path = fixture.receipts_dir().join("configure.json");
        let bytes = fs::read(&path).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let duplicated = text.replace(
            "\"phase\":\"configure\"",
            "\"phase\":\"configure\",\"phase\":\"configure\"",
        );
        assert_ne!(duplicated, text);
        fs::write(path, duplicated).unwrap();
        assert!(fixture.readback().is_err());
    }
    {
        let mut fixture = Fixture::new();
        fixture.persist();
        let path = fixture.receipts_dir().join("configure.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["unrecognized"] = json!(true);
        fixture.phase_receipt_digests[2] = replace_self_digest(&mut value);
        write_canonical(&path, &value);
        assert!(fixture.readback().is_err());
    }
}

#[test]
fn rejects_missing_extra_and_symlink_receipt_inventory_entries() {
    for mutation in ["missing", "extra", "symlink"] {
        let mut fixture = Fixture::new();
        fixture.persist();
        let directory = fixture.receipts_dir();
        match mutation {
            "missing" => fs::remove_file(directory.join("configure.json")).unwrap(),
            "extra" => fs::write(directory.join("unexpected.txt"), b"extra\n").unwrap(),
            "symlink" => {
                fs::remove_file(directory.join("configure.json")).unwrap();
                symlink("preflight.json", directory.join("configure.json")).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            fixture.readback().is_err(),
            "finished candidate accepted {mutation} receipt inventory"
        );
    }
}

#[test]
fn changed_or_collector_only_self_rehashed_finished_records_fail_independent_digest() {
    {
        let mut fixture = Fixture::new();
        let independently_selected_digest = fixture.persist();
        let path = fixture.receipts_dir().join(CANDIDATE_RECEIPT);
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["entry_count"] = json!(value["entry_count"].as_u64().unwrap() + 1);
        replace_self_digest(&mut value);
        write_canonical(&path, &value);
        assert_eq!(
            fixture.candidate_receipt_digest.as_ref(),
            Some(&independently_selected_digest)
        );
        assert!(fixture.readback().is_err());
    }
    {
        let mut fixture = Fixture::new();
        let independently_selected_digest = fixture.persist();
        let candidate = fixture.candidate();
        for relative in [
            "bin/clang",
            "bin/clang++",
            "bin/llvm-ar",
            "bin/collect-aros",
            "bin/collect-aros32",
            "libexec/llvm-ar.real",
            ".installflag-fixture",
            AROS_TOOLCHAIN_MANIFEST_FILE,
        ] {
            fs::remove_file(candidate.join(relative)).unwrap();
        }
        fs::remove_dir(candidate.join("target/empty")).unwrap();
        fs::remove_dir(candidate.join("target")).unwrap();
        fs::remove_dir(candidate.join("include/empty")).unwrap();
        fs::remove_dir(candidate.join("include")).unwrap();
        fs::remove_dir(candidate.join("libexec")).unwrap();

        // Model a self-consistent collector-only record. The retained phase
        // chain still selects the published collector, but this rewritten
        // record is not the independently selected finished-candidate record.
        let snapshot = super::measure_payload(&candidate).unwrap();
        let path = fixture.receipts_dir().join(CANDIDATE_RECEIPT);
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["payload_sha256"] = json!(snapshot.payload_digest_excluding(None).as_str());
        value["entry_count"] = json!(u64::try_from(snapshot.entry_count()).unwrap());
        value["regular_file_bytes"] = json!(snapshot.regular_file_bytes().unwrap());
        replace_self_digest(&mut value);
        write_canonical(&path, &value);
        assert_eq!(
            fixture.candidate_receipt_digest.as_ref(),
            Some(&independently_selected_digest)
        );
        assert!(fixture.readback().is_err());
    }
}

#[test]
fn package_rejects_candidate_substitution_and_stale_readback_without_output() {
    {
        let mut fixture = Fixture::new();
        fixture.persist();
        let proof = fixture.readback().unwrap();
        let output = fixture.temporary.path().join("substituted-package");
        let mut request = package_request(&fixture, output.clone());
        request.candidate_root = fixture.output.join(".aros-native-toolchain-stage");
        assert!(package_finished_candidate(&request, &proof).is_err());
        assert!(!output.exists());
    }
    {
        let mut fixture = Fixture::new();
        fixture.persist();
        let proof = fixture.readback().unwrap();
        fs::write(
            fixture.candidate().join("bin/clang"),
            b"stale after proof\n",
        )
        .unwrap();
        let output = fixture.temporary.path().join("stale-proof-package");
        let request = package_request(&fixture, output.clone());
        assert!(package_finished_candidate(&request, &proof).is_err());
        assert!(!output.exists());
    }
}

#[test]
fn candidate_entry_budget_reserves_archive_root_and_generated_manifest() {
    let limits = super::payload_limits();
    assert_eq!(
        limits.max_entries,
        usize::try_from(crate::package_verify::MAX_ARCHIVE_ENTRIES).unwrap() - 2
    );
    assert_eq!(
        crate::package_verify::MAX_ARCHIVE_ENTRIES - u64::try_from(limits.max_entries).unwrap(),
        2,
        "the package archive needs room for its root and generated manifest"
    );
}

#[test]
fn candidate_byte_budget_reserves_package_verifier_metadata_allowance() {
    let limits = super::payload_limits();
    assert_eq!(
        limits.max_regular_file_bytes,
        crate::package_verify::MAX_EXPANDED_ARCHIVE_BYTES
            - crate::package_verify::MAX_METADATA_BYTES
    );
    assert_eq!(
        crate::package_verify::MAX_EXPANDED_ARCHIVE_BYTES - limits.max_regular_file_bytes,
        crate::package_verify::MAX_METADATA_BYTES,
        "the generated package metadata allowance must remain outside raw payload bytes"
    );
}

#[test]
fn nofollow_copy_rejects_candidate_changed_since_retained_snapshot() {
    let mut fixture = Fixture::new();
    fixture.persist();
    let proof = fixture.readback().unwrap();
    let candidate = fixture.candidate();
    fs::write(candidate.join("bin/clang"), b"changed after snapshot\n").unwrap();

    let destination_parent = tempfile::tempdir().unwrap();
    let destination = destination_parent.path().join("raw-stage");
    fs::create_dir(&destination).unwrap();
    let result = copy_tree_from_snapshot_nofollow(
        &candidate,
        &destination,
        proof.snapshot(),
        super::payload_limits(),
    );

    assert!(result.is_err());
    assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    assert_eq!(
        fs::read(candidate.join("bin/clang")).unwrap(),
        b"changed after snapshot\n",
        "rejected copy must leave its input candidate untouched"
    );
}

fn assert_finished_v2_package(package: &FinishedCandidatePackage) {
    assert_eq!(package.verified().manifest.schema, 2);
    assert_eq!(package.verified().manifest.host, "linux-x86_64");
    assert_eq!(package.verified().manifest.target_profile, "pc-x86_64");
    assert!(package.package_output().archive.is_file());
    assert!(package.package_output().manifest.is_file());
    assert!(package.package_output().checksum.is_file());
    assert!(package.package_output().sbom.is_file());
}

pub(super) fn package_request(fixture: &Fixture, output_dir: PathBuf) -> PackageRequest {
    PackageRequest {
        candidate_root: fixture.candidate(),
        output_dir,
        release_id: "finished-candidate-test".into(),
        host: fixture.identity.host.into(),
        recipe: fixture.recipe.clone(),
        source_lock: fixture.source_lock.clone(),
        profile: fixture.profile.clone(),
        build_environment: Map::new(),
        forbidden_prefixes: vec![fixture.work.clone(), fixture.output.clone()],
    }
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

fn write_file(root: &Path, relative: &str, bytes: &[u8], mode: u32) {
    let path = root.join(relative);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn create_canonical_dir(path: &Path) -> PathBuf {
    fs::create_dir(path).unwrap();
    path.canonicalize().unwrap()
}

pub(super) fn fixture_source_lock() -> Vec<u8> {
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

pub(super) fn fixture_profiles() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v1",
        "upstream_commit": SOURCE_COMMIT,
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

pub(super) fn fixture_recipe(source_lock: &[u8], profiles: &[u8]) -> Vec<u8> {
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
        .expect("self-digest field");
    let digest = sha256_bytes(&crate::canonical::bytes(value).unwrap());
    value["receipt_sha256"] = json!(digest.as_str());
    digest
}

fn write_canonical(path: &Path, value: &Value) {
    fs::write(path, crate::canonical::bytes(value).unwrap()).unwrap();
}
