//! End-to-end regression coverage for source-bound GNU SDK generation.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::{sha256_bytes, CancellationToken};
use serde_json::Value;

use super::{
    amend_cmake_to_build_native_sdk, gnu_native_sdk_fixture, native_consumer_binding, shell_quote,
    Fixture, NativeCompatibilityRequest, NativeSdkBindingMode, NATIVE_CONSUMER_CONTRACT_FILE,
};
use crate::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceRequest,
};
use crate::recipe::GitObjectId;
use crate::sdk_generation::{self, SdkGenerationRequest};
use crate::sdk_package::{self, SdkVerificationRequest};
use crate::sdk_source::{verify_sdk_source, SdkSourceVerificationRequest};

const GENERATION_RECORD: &str = "sdk-generation.record.json";
const CONTRACT_FILE: &str = NATIVE_CONSUMER_CONTRACT_FILE;

struct GenerationFixture {
    _temporary: tempfile::TempDir,
    compatibility_fixture: Fixture,
    request: SdkGenerationRequest,
    original_source_root: PathBuf,
    materialized_source_root: PathBuf,
    package_output: PathBuf,
}

#[derive(Debug, Clone, Copy)]
enum RetainedMutation {
    Sdk,
    Map,
    Elf,
    Record,
    Source,
}

#[test]
fn generates_and_packages_the_exact_six_phase_four_link_sdk_with_epoch_zero() {
    let fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
    assert_eq!(fixture.request.source.identity().source_date_epoch, 0);
    assert_eq!(
        fixture.request.compatibility.consumer_source_date_epoch,
        Some(0)
    );

    let completed = sdk_generation::generate(&fixture.request, &CancellationToken::default())
        .expect("verified GNU source should complete generation");
    assert_eq!(completed.identity(), fixture.request.source.identity());
    assert_eq!(
        completed.record_path(),
        fixture.request.record_path.canonicalize().unwrap()
    );
    let record: Value =
        serde_json::from_slice(&fs::read(completed.record_path()).unwrap()).unwrap();
    assert_eq!(record["schema"], "aros-sdk-generation-record-v1");
    assert_eq!(record["identity"]["source_date_epoch"], 0);
    assert_eq!(record["sdk_entries"], 5);

    let compatibility_receipt: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .compatibility_fixture
                .request
                .reports_root
                .join("native-compatibility.receipt.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        compatibility_receipt["phase_reports"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    let sdk_receipt_bytes = fs::read(
        fixture
            .compatibility_fixture
            .request
            .reports_root
            .join("native-sdk-links/native-sdk-links.receipt.json"),
    )
    .unwrap();
    let sdk_receipt: Value = serde_json::from_slice(&sdk_receipt_bytes).unwrap();
    assert_eq!(sdk_receipt["outputs"].as_object().unwrap().len(), 4);
    assert_eq!(sdk_receipt["maps"].as_object().unwrap().len(), 4);
    assert_eq!(
        record["sdk_link_receipt_sha256"],
        sha256_bytes(&sdk_receipt_bytes).to_string()
    );
    assert_eq!(
        record["sdk_inventory_sha256"],
        sdk_receipt["sdk_inventory_sha256"]
    );
    assert_eq!(record["sdk_entries"], sdk_receipt["sdk_entries"]);

    let forbidden_prefixes = vec![PathBuf::from("/sdk-generation-unrelated-forbidden-prefix")];
    let packaged =
        sdk_generation::package_generated(&completed, &fixture.package_output, &forbidden_prefixes)
            .expect("only the generated SDK should be packaged");
    let verified = sdk_package::verify(&SdkVerificationRequest {
        package_dir: packaged.output_dir.clone(),
        identity: completed.identity().clone(),
        expected_tree_sha256: completed.tree_sha256().clone(),
        expected_archive_sha256: packaged.archive_sha256.clone(),
        expected_archive_size: packaged.archive_size,
        forbidden_prefixes,
    })
    .expect("generated package should verify against its selected identity and tree");
    assert_eq!(verified.manifest.identity, *completed.identity());
    assert_eq!(verified.manifest.tree_sha256, *completed.tree_sha256());
    assert!(packaged.archive.is_file());
    assert!(packaged.manifest.is_file());
    assert!(packaged.checksum.is_file());
}

#[test]
fn rejects_epoch_preset_and_source_digest_mismatches_before_execution() {
    let mut fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
    for epoch in [None, Some(1)] {
        fixture.request.compatibility.consumer_source_date_epoch = epoch;
        let error = sdk_generation::generate(&fixture.request, &CancellationToken::default())
            .expect_err("generation must use the source-selected epoch");
        assert!(error
            .to_string()
            .contains("SDK generation epoch differs from its selected product identity"));
        assert_preexecution_outputs_absent(&fixture);
    }

    fixture.request.compatibility.consumer_source_date_epoch = Some(0);
    fixture.request.compatibility.source_preset = Some("other-source-preset".into());
    let error = sdk_generation::generate(&fixture.request, &CancellationToken::default())
        .expect_err("generation must use the verified source preset");
    assert!(error
        .to_string()
        .contains("SDK generation preset differs from its verified source profile"));
    assert_preexecution_outputs_absent(&fixture);

    fixture.request.compatibility.source_preset = Some("source-rv32-preset".into());
    fixture.request.compatibility.preparation.source_tree_sha256 =
        sha256_bytes(b"different projected SDK source");
    let error = sdk_generation::generate(&fixture.request, &CancellationToken::default())
        .expect_err("generation must use the verified source projection digest");
    assert!(error
        .to_string()
        .contains("SDK generation source differs from its verified committed projection"));
    assert_preexecution_outputs_absent(&fixture);
}

#[test]
fn rejects_dirty_original_or_materialized_source_before_execution() {
    for original in [true, false] {
        let fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
        let root = if original {
            &fixture.original_source_root
        } else {
            &fixture.materialized_source_root
        };
        append_byte(&root.join("native-sdk-probe.c"));
        let error = sdk_generation::generate(&fixture.request, &CancellationToken::default())
            .expect_err("changed committed SDK source must not execute");
        assert!(
            error.to_string().contains("source") || error.to_string().contains("contract"),
            "unexpected dirty-source rejection: {error}"
        );
        assert_preexecution_outputs_absent(&fixture);
    }
}

#[test]
fn stale_or_missing_native_sdk_binding_never_publishes_a_generation_record() {
    for mode in [NativeSdkBindingMode::Stale, NativeSdkBindingMode::Missing] {
        let fixture = generation_fixture(mode, 0);
        let error = sdk_generation::generate(&fixture.request, &CancellationToken::default())
            .expect_err("invalid native SDK binding must reject generation");
        assert!(
            error.to_string().contains("binding")
                || error.to_string().contains("source")
                || error.to_string().contains("consumer"),
            "unexpected fake-binding rejection: {error}"
        );
        assert!(!fixture.request.record_path.exists());
        assert!(!fixture.package_output.exists());
    }
}

#[test]
fn packaging_rejects_changed_sdk_links_record_or_source_without_creating_package_output() {
    for mutation in [
        RetainedMutation::Sdk,
        RetainedMutation::Map,
        RetainedMutation::Elf,
        RetainedMutation::Record,
        RetainedMutation::Source,
    ] {
        let fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
        let completed = sdk_generation::generate(&fixture.request, &CancellationToken::default())
            .expect("baseline SDK generation should complete");
        let path = match mutation {
            RetainedMutation::Sdk => fixture
                .compatibility_fixture
                .request
                .cmake_build_root
                .join("SYS/Developer/lib/client.a"),
            RetainedMutation::Map => fixture
                .compatibility_fixture
                .request
                .reports_root
                .join("native-sdk-links/original-c.map"),
            RetainedMutation::Elf => fixture
                .compatibility_fixture
                .request
                .reports_root
                .join("native-sdk-links/original-c.elf"),
            RetainedMutation::Record => completed.record_path().to_path_buf(),
            RetainedMutation::Source => fixture.materialized_source_root.join("native-sdk-probe.c"),
        };
        append_byte(&path);
        let error = sdk_generation::package_generated(&completed, &fixture.package_output, &[])
            .expect_err("stale generation evidence must not be packaged");
        assert!(
            error.to_string().contains("SDK")
                || error.to_string().contains("native")
                || error.to_string().contains("source")
                || error.to_string().contains("record"),
            "unexpected packaging rejection for {mutation:?}: {error}"
        );
        assert!(!fixture.package_output.exists());
    }
}

#[test]
fn packaging_refuses_protected_roots_and_any_directory_containing_the_record() {
    let fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
    let completed = sdk_generation::generate(&fixture.request, &CancellationToken::default())
        .expect("baseline SDK generation should complete");
    let destinations = [
        (
            fixture.materialized_source_root.join("nested-package"),
            "SDK generation output overlaps protected inputs or execution roots",
        ),
        (
            fixture
                .compatibility_fixture
                .request
                .reports_root
                .join("nested-package"),
            "SDK generation output overlaps protected inputs or execution roots",
        ),
        (
            fixture.request.record_path.parent().unwrap().to_path_buf(),
            "SDK package output overlaps its generation record",
        ),
        (
            fixture
                .request
                .compatibility
                .host_tools
                .root
                .join("nested-package"),
            "SDK generation output overlaps protected inputs or execution roots",
        ),
        (
            fixture
                .request
                .compatibility
                .ports_sources
                .root
                .join("nested-package"),
            "SDK generation output overlaps protected inputs or execution roots",
        ),
        (
            fixture.request.compatibility.host_python.import_roots()[0].join("nested-package"),
            "SDK generation output overlaps protected inputs or execution roots",
        ),
    ];
    for (destination, expected) in destinations {
        let error = sdk_generation::package_generated(&completed, &destination, &[])
            .expect_err("package output must be disjoint from every protected root and record");
        assert!(
            error.to_string().contains(expected),
            "unexpected output-overlap rejection: {error}"
        );
        let asset = completed.identity().asset_name().unwrap();
        assert!(!destination.join(asset).exists());
    }
}

#[test]
fn refuses_an_occupied_record_without_running_or_overwriting_it() {
    let fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
    let sentinel = b"operator-owned record sentinel\n";
    fs::create_dir(&fixture.request.record_path).unwrap();
    let sentinel_path = fixture.request.record_path.join("operator-owned.txt");
    fs::write(&sentinel_path, sentinel).unwrap();

    let error = sdk_generation::generate(&fixture.request, &CancellationToken::default())
        .expect_err("an existing generation record must not be adopted");
    assert!(error
        .to_string()
        .contains("SDK generation record must be absent"));
    assert_eq!(fs::read(&sentinel_path).unwrap(), sentinel);
    assert!(!fixture
        .compatibility_fixture
        .request
        .cmake_build_root
        .exists());
    assert!(!fixture.compatibility_fixture.request.reports_root.exists());
    assert!(!fixture.package_output.exists());
}

fn generation_fixture(mode: NativeSdkBindingMode, source_epoch: u64) -> GenerationFixture {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let fixture_root = root.join("fixture");
    let record_root = root.join("records");
    let package_root = root.join("packages");
    fs::create_dir(&fixture_root).unwrap();
    fs::create_dir(&record_root).unwrap();
    fs::create_dir(&package_root).unwrap();

    let mut compatibility_fixture = gnu_native_sdk_fixture(&fixture_root, mode);
    compatibility_fixture.request.consumer_source_date_epoch = Some(source_epoch);
    let original_source_root = compatibility_fixture
        .request
        .preparation
        .source_root
        .clone();
    let committed_source = commit_source(&original_source_root);
    let materialized_source_root = fixture_root.join("sdk-source-projection");
    let materialized = materialize_engine_free_source(&EngineFreeSourceRequest {
        source_root: original_source_root.clone(),
        expected_source: committed_source,
        output_root: materialized_source_root,
    })
    .expect("fixture consumer source should materialize from committed Git");
    compatibility_fixture.request.preparation.source_root = materialized.root.clone();
    compatibility_fixture.request.preparation.source_tree_sha256 =
        materialized.source_tree_sha256.clone();

    let contract_path = materialized.root.join(CONTRACT_FILE);
    let contract_bytes = fs::read(&contract_path).unwrap();
    let contract: Value = serde_json::from_slice(&contract_bytes).unwrap();
    let binding =
        native_consumer_binding(&compatibility_fixture.request, &contract_path, &contract);
    refresh_static_helper_binding(&mut compatibility_fixture.request, &binding);
    amend_cmake_to_build_native_sdk(&compatibility_fixture.request, &binding, mode);

    let abi: BTreeMap<String, SdkAbiValue> =
        serde_json::from_value(contract["abi"].clone()).unwrap();
    let identity = SdkIdentity {
        sdk_id: "example-sdk".into(),
        version: "1.0.0".into(),
        target: SdkTarget {
            target_triple: contract["abi"]["target_triple"]
                .as_str()
                .unwrap()
                .to_owned(),
            abi,
        },
        source: SdkSourceIdentity {
            commit: materialized.source_commit.clone(),
            git_tree: materialized.source_tree.clone(),
            content_sha256: materialized.source_tree_sha256.clone(),
            contract_sha256: sha256_bytes(&contract_bytes),
        },
        source_date_epoch: source_epoch,
    };
    let source = verify_sdk_source(&SdkSourceVerificationRequest {
        source_root: original_source_root.clone(),
        materialized_source_root: materialized.root.clone(),
        source_preset: "source-rv32-preset".into(),
        identity,
    })
    .expect("fixture consumer source should verify against its committed projection");
    assert_eq!(source.identity().source_date_epoch, source_epoch);

    let record_path = record_root.join(GENERATION_RECORD);
    let package_output = package_root.join("sdk");
    let request = SdkGenerationRequest {
        source,
        compatibility: compatibility_fixture.request.clone(),
        profiles: compatibility_fixture.profiles.clone(),
        record_path,
    };
    GenerationFixture {
        _temporary: temporary,
        compatibility_fixture,
        request,
        original_source_root,
        materialized_source_root: materialized.root,
        package_output,
    }
}

fn commit_source(root: &Path) -> CommittedSourceIdentity {
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "sealed GNU SDK source fixture"]);
    let commit = git_text(root, &["rev-parse", "HEAD^0"]);
    let tree = git_text(root, &["rev-parse", "HEAD:"]);
    CommittedSourceIdentity {
        commit: GitObjectId::try_from(commit).unwrap(),
        tree: GitObjectId::try_from(tree).unwrap(),
    }
}

fn git(root: &Path, arguments: &[&str]) {
    let output = git_command(root, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = git_command(root, arguments);
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn git_command(root: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "user.name=AROS SDK generation test",
            "-c",
            "user.email=aros-sdk-generation-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(arguments)
        .output()
        .unwrap()
}

fn refresh_static_helper_binding(request: &mut NativeCompatibilityRequest, binding: &Value) {
    let rendered = serde_json::to_string(binding).unwrap();
    let helper = request
        .preparation
        .helpers
        .get_mut("aros-transpiler")
        .unwrap();
    let script = format!("#!/bin/sh\nprintf '%s\\n' {}\n", shell_quote(&rendered));
    fs::write(&helper.path, script.as_bytes()).unwrap();
    fs::set_permissions(&helper.path, fs::Permissions::from_mode(0o755)).unwrap();
    helper.sha256 = sha256_bytes(script.as_bytes());
    helper.size = script.len() as u64;
}

fn assert_preexecution_outputs_absent(fixture: &GenerationFixture) {
    assert!(!fixture.request.record_path.exists());
    assert!(!fixture.package_output.exists());
    assert!(!fixture
        .compatibility_fixture
        .request
        .cmake_build_root
        .exists());
    assert!(!fixture.compatibility_fixture.request.reports_root.exists());
}

fn append_byte(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    bytes.push(b'!');
    fs::write(path, bytes).unwrap();
}
