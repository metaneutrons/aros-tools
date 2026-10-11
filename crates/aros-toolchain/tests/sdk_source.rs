#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::sha256_bytes;
use aros_toolchain::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceOutput,
    EngineFreeSourceRequest,
};
use aros_toolchain::recipe::GitObjectId;
use aros_toolchain::sdk_source::{verify_sdk_source, SdkSourceVerificationRequest};
use serde_json::{json, Value};
use tempfile::TempDir;

const PROFILE: &str = "[[targets]]\nname='example-sdk'\narch='riscv64'\nplatform='opensbi'\nbsp='example'\nfloat_abi='lp64d'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=true\nmesa_version='26.0.0'\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=true\n";
const POLICY: &[u8] = b"{}";
const C_PROBE: &[u8] = b"/* sealed SDK C probe */\n";
const CXX_PROBE: &[u8] = b"/* sealed SDK C++ probe */\n";

struct Fixture {
    temporary: TempDir,
    source_root: PathBuf,
    materialized_source_root: PathBuf,
    materialized: EngineFreeSourceOutput,
    identity: SdkIdentity,
    consumer_bytes: Vec<u8>,
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "user.name=AROS SDK source test",
            "-c",
            "user.email=aros-sdk-source-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn source_identity(source_root: &Path) -> CommittedSourceIdentity {
    CommittedSourceIdentity {
        commit: GitObjectId::try_from(git_text(source_root, &["rev-parse", "HEAD^0"])).unwrap(),
        tree: GitObjectId::try_from(git_text(source_root, &["rev-parse", "HEAD:"])).unwrap(),
    }
}

fn abi_value() -> Value {
    json!({
        "source_cpu": "riscv64",
        "target_triple": "riscv64-aros",
        "isa": "rva22u64",
        "abi": "lp64d",
        "code_model": "medany",
        "flavour": "native",
        "platform_smp": true,
        "use_mmu": true,
    })
}

fn write_consumer_contract(root: &Path, v2: bool) -> Vec<u8> {
    fs::write(root.join("aros-targets.toml"), PROFILE).unwrap();
    fs::write(root.join("policy.json"), POLICY).unwrap();

    let mut inputs = vec![
        json!({
            "path": "aros-targets.toml",
            "sha256": sha256_bytes(PROFILE.as_bytes()),
        }),
        json!({
            "path": "policy.json",
            "sha256": sha256_bytes(POLICY),
        }),
    ];
    let mut document = json!({
        "schema": if v2 {
            "aros-native-consumer-contract-v2"
        } else {
            "aros-native-consumer-contract-v1"
        },
        "profile": "example-sdk",
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "roots": ["includes", "linklibs"],
        "metamake_projection": "policy.json",
        "inputs": inputs,
        "abi": abi_value(),
    });
    if v2 {
        fs::write(root.join("probe.c"), C_PROBE).unwrap();
        fs::write(root.join("probe.cpp"), CXX_PROBE).unwrap();
        inputs.clone_from(document["inputs"].as_array().unwrap());
        inputs.extend([
            json!({"path": "probe.c", "sha256": sha256_bytes(C_PROBE)}),
            json!({"path": "probe.cpp", "sha256": sha256_bytes(CXX_PROBE)}),
        ]);
        document["inputs"] = Value::Array(inputs);
        document["native_sdk_link_probes"] = json!({
            "c": {"source": "probe.c", "libraries": []},
            "cxx": {"source": "probe.cpp", "libraries": []},
        });
    }
    let bytes = serde_json::to_vec(&document).unwrap();
    fs::write(root.join("consumer.json"), &bytes).unwrap();
    bytes
}

fn create_fixture(v2: bool) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let source_root = temporary.path().join("source");
    fs::create_dir(&source_root).unwrap();
    git(&source_root, &["init", "-q"]);

    let consumer_bytes = write_consumer_contract(&source_root, v2);
    fs::create_dir(source_root.join("cmake")).unwrap();
    fs::write(
        source_root.join("cmake/engine.cmake"),
        "excluded source engine\n",
    )
    .unwrap();
    fs::create_dir(source_root.join("docs")).unwrap();
    fs::write(
        source_root.join("docs/source.txt"),
        "committed SDK source\n",
    )
    .unwrap();
    git(&source_root, &["add", "-A"]);
    git(
        &source_root,
        &["commit", "-qm", "sealed SDK source fixture"],
    );

    let committed = source_identity(&source_root);
    let materialized = materialize_engine_free_source(&EngineFreeSourceRequest {
        source_root: source_root.clone(),
        expected_source: committed,
        output_root: temporary.path().join("materialized"),
    })
    .unwrap();
    let contract_digest = sha256_bytes(&consumer_bytes);
    let abi: BTreeMap<String, SdkAbiValue> = serde_json::from_value(abi_value()).unwrap();
    let identity = SdkIdentity {
        sdk_id: "example-sdk".to_owned(),
        version: "1.0.0".to_owned(),
        target: SdkTarget {
            target_triple: "riscv64-aros".to_owned(),
            abi,
        },
        source: SdkSourceIdentity {
            commit: materialized.source_commit.clone(),
            git_tree: materialized.source_tree.clone(),
            content_sha256: materialized.source_tree_sha256.clone(),
            contract_sha256: contract_digest,
        },
        // Deliberately explicit and independent of the fixture commit timestamp.
        source_date_epoch: 946_684_800,
    };

    Fixture {
        temporary,
        source_root,
        materialized_source_root: materialized.root.clone(),
        materialized,
        identity,
        consumer_bytes,
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    Regular(Vec<u8>),
    Symlink(PathBuf),
    Other,
}

#[derive(Debug, PartialEq, Eq)]
struct SnapshotEntry {
    path: PathBuf,
    mode: u32,
    kind: EntryKind,
}

fn collect_snapshot(root: &Path, directory: &Path, entries: &mut Vec<SnapshotEntry>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink(fs::read_link(&path).unwrap())
        } else if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() {
            EntryKind::Regular(fs::read(&path).unwrap())
        } else {
            EntryKind::Other
        };
        entries.push(SnapshotEntry {
            path: relative,
            mode: metadata.permissions().mode() & 0o7777,
            kind,
        });
        if file_type.is_dir() {
            collect_snapshot(root, &path, entries);
        }
    }
}

fn snapshot(root: &Path) -> Vec<SnapshotEntry> {
    let mut entries = Vec::new();
    collect_snapshot(root, root, &mut entries);
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

fn top_level_entries(root: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

fn request(fixture: &Fixture, identity: SdkIdentity, preset: &str) -> SdkSourceVerificationRequest {
    SdkSourceVerificationRequest {
        source_root: fixture.source_root.clone(),
        materialized_source_root: fixture.materialized_source_root.clone(),
        source_preset: preset.to_owned(),
        identity,
    }
}

fn verify_rejects_without_writes(
    fixture: &Fixture,
    request: &SdkSourceVerificationRequest,
    expected_reason: &str,
) {
    let source_before = snapshot(&fixture.source_root);
    let materialized_before = snapshot(&fixture.materialized_source_root);
    let siblings_before = top_level_entries(fixture.temporary.path());
    let error = verify_sdk_source(request).expect_err("invalid SDK source was accepted");
    let message = &error.diagnostics().diagnostics[0].message;
    assert!(
        message.contains(expected_reason),
        "rejected SDK source for an unexpected reason: expected {expected_reason:?}, got {message:?}"
    );
    assert_eq!(snapshot(&fixture.source_root), source_before);
    assert_eq!(
        snapshot(&fixture.materialized_source_root),
        materialized_before
    );
    assert_eq!(top_level_entries(fixture.temporary.path()), siblings_before);
}

fn changed_object_id(value: &str) -> String {
    let mut changed = value.to_owned();
    let replacement = if changed.starts_with('0') { '1' } else { '0' };
    changed.replace_range(0..1, &replacement.to_string());
    changed
}

#[test]
fn binds_git_engine_free_cas_sealed_contract_abi_and_revalidates_read_only() {
    let fixture = create_fixture(true);
    let request = request(&fixture, fixture.identity.clone(), "example-sdk");
    let source_before = snapshot(&fixture.source_root);
    let projection_before = snapshot(&fixture.materialized_source_root);
    let siblings_before = top_level_entries(fixture.temporary.path());

    let verified = verify_sdk_source(&request).unwrap();
    assert_eq!(verified.identity(), &fixture.identity);
    assert_eq!(verified.source_profile().name, "example-sdk");
    assert_eq!(
        fixture.identity.source.commit,
        fixture.materialized.source_commit
    );
    assert_eq!(
        fixture.identity.source.git_tree,
        fixture.materialized.source_tree
    );
    assert_eq!(
        fixture.identity.source.content_sha256,
        fixture.materialized.source_tree_sha256
    );
    assert_eq!(
        fixture.identity.source.contract_sha256,
        sha256_bytes(&fixture.consumer_bytes)
    );
    assert_eq!(verified.identity().source_date_epoch, 946_684_800);
    assert_eq!(
        verified.materialized_source_root(),
        fixture.materialized_source_root.as_path()
    );
    assert!(!fixture.materialized_source_root.join("cmake").exists());
    verified.revalidate().unwrap();

    assert_eq!(snapshot(&fixture.source_root), source_before);
    assert_eq!(
        snapshot(&fixture.materialized_source_root),
        projection_before
    );
    assert_eq!(top_level_entries(fixture.temporary.path()), siblings_before);
}

#[test]
fn rejects_each_selected_identity_mismatch_without_writes() {
    let mut changed = create_fixture(true);
    changed.identity.source.commit = changed_object_id(&changed.identity.source.commit);
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "compatibility source checkout differs from the selected source identity",
    );

    let mut changed = create_fixture(true);
    changed.identity.source.git_tree = changed_object_id(&changed.identity.source.git_tree);
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "compatibility source checkout differs from the selected source identity",
    );

    let mut changed = create_fixture(true);
    changed.identity.source.content_sha256 = sha256_bytes(b"wrong engine-free content");
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "SDK source content differs from its selected engine-free digest",
    );

    let mut changed = create_fixture(true);
    changed.identity.source.contract_sha256 = sha256_bytes(b"wrong contract bytes");
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "SDK source contract differs from its selected digest",
    );
}

#[test]
fn rejects_target_triple_and_abi_divergence_without_writes() {
    let mut changed = create_fixture(true);
    changed.identity.target.target_triple = "riscv64-other".to_owned();
    changed.identity.target.abi.insert(
        "target_triple".to_owned(),
        SdkAbiValue::Text("riscv64-other".to_owned()),
    );
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );

    let mut changed = create_fixture(true);
    changed
        .identity
        .target
        .abi
        .insert("abi".to_owned(), SdkAbiValue::Text("lp64".to_owned()));
    verify_rejects_without_writes(
        &changed,
        &request(&changed, changed.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );

    let mut missing = create_fixture(true);
    missing.identity.target.abi.remove("use_mmu");
    verify_rejects_without_writes(
        &missing,
        &request(&missing, missing.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );

    let mut extra = create_fixture(true);
    extra
        .identity
        .target
        .abi
        .insert("extra_fact".to_owned(), SdkAbiValue::Boolean(true));
    verify_rejects_without_writes(
        &extra,
        &request(&extra, extra.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );

    let mut scalar = create_fixture(true);
    scalar.identity.target.abi.insert(
        "platform_smp".to_owned(),
        SdkAbiValue::Text("true".to_owned()),
    );
    verify_rejects_without_writes(
        &scalar,
        &request(&scalar, scalar.identity.clone(), "example-sdk"),
        "SDK target or ABI differs from its source contract",
    );
}

#[test]
fn rejects_unknown_preset_and_legacy_v1_contract_without_writes() {
    let fixture = create_fixture(true);
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "not-declared"),
        "SDK source preset is absent",
    );

    let fixture = create_fixture(false);
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "example-sdk"),
        "SDK source contract must declare v2 ordinary link probes",
    );
}

#[test]
fn rejects_changed_source_or_projection_and_revalidation_writes_nothing() {
    let fixture = create_fixture(true);
    let verified =
        verify_sdk_source(&request(&fixture, fixture.identity.clone(), "example-sdk")).unwrap();
    fs::write(
        fixture.source_root.join("docs/source.txt"),
        "changed original source\n",
    )
    .unwrap();
    let source_before = snapshot(&fixture.source_root);
    let projection_before = snapshot(&fixture.materialized_source_root);
    let siblings_before = top_level_entries(fixture.temporary.path());
    let error = verified
        .revalidate()
        .expect_err("changed original source was accepted during revalidation");
    let message = &error.diagnostics().diagnostics[0].message;
    assert!(
        message.contains("source file size or executable bit differs from its committed blob"),
        "revalidation rejected the changed original source for an unexpected reason: {message:?}"
    );
    assert_eq!(snapshot(&fixture.source_root), source_before);
    assert_eq!(
        snapshot(&fixture.materialized_source_root),
        projection_before
    );
    assert_eq!(top_level_entries(fixture.temporary.path()), siblings_before);

    let fixture = create_fixture(true);
    fs::write(
        fixture.materialized_source_root.join("docs/source.txt"),
        "changed projection\n",
    )
    .unwrap();
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "example-sdk"),
        "source projection differs from raw committed material",
    );

    let fixture = create_fixture(true);
    fs::write(
        fixture.materialized_source_root.join("probe.c"),
        b"changed sealed probe\n",
    )
    .unwrap();
    verify_rejects_without_writes(
        &fixture,
        &request(&fixture, fixture.identity.clone(), "example-sdk"),
        "SDK source consumer contract is not sealed to its profile and inputs",
    );
}
