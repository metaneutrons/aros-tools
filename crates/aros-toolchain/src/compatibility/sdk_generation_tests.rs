//! End-to-end regression coverage for source-bound GNU SDK generation.

#![cfg(unix)]

use crate::compatibility::sdk_generation_matrix_cli_tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::{sha256_bytes, ArosCompilerIdentity, ArosToolchainManifest, CancellationToken};
use serde_json::{json, Value};

use super::{
    amend_cmake_to_build_native_sdk, native_consumer_binding, shell_quote, Fixture,
    NativeCompatibilityRequest, NativeSdkBindingMode, NATIVE_CONSUMER_CONTRACT_FILE,
};
use crate::compatibility::{
    NativeCompatibilityExecutionExport, NativeCompatibilityExpectedPackage,
    NativeCompatibilityInputClaims, PortableNativeCompatibilityRequest, TwoRootRelocation,
};
use crate::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceRequest,
};
use crate::native_candidate::{
    guarded_gnu_package_from_payload_for_host, prepare_gnu_matrix_release,
    with_gnu_matrix_complete_request, GuardedPackage, ReleaseCompatibilityReadbackRequestV2,
};
use crate::package_extract::ExtractedPackage;
use crate::package_verify::VerifiedPackage;
use crate::recipe::{GitObjectId, Recipe};
use crate::sdk_generation::{self, SdkGenerationRequest};
use crate::sdk_generation_lanes::ExpectedSdkGenerationLane;
use crate::sdk_generation_matrix::{
    readback_sdk_generation_matrix, SdkGenerationMatrixReadbackRequest,
};
use crate::sdk_package::{self, SdkPackageOutput, SdkVerificationRequest};
use crate::sdk_source::{verify_sdk_source, SdkSourceVerificationRequest};

const GENERATION_RECORD: &str = "sdk-generation.record.json";
const CONTRACT_FILE: &str = NATIVE_CONSUMER_CONTRACT_FILE;

struct GenerationFixture {
    _temporary: tempfile::TempDir,
    compatibility_fixture: Fixture,
    request: SdkGenerationRequest,
    execution_root: PathBuf,
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
fn generate_with_export_returns_the_exact_sdk_portable_closure_for_its_record() {
    let fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
    let (completed, execution) =
        sdk_generation::generate_with_export(&fixture.request, &CancellationToken::default())
            .expect("verified GNU source should return generation and portable evidence");

    let report = execution.report();
    let native_sdk = report
        .native_sdk
        .as_ref()
        .expect("SDK generation must execute all four ordinary links");
    assert_eq!(report.probes.reports.len(), 6);
    assert_eq!(
        execution.evidence().receipt_sha256(),
        &report.receipt.sha256
    );
    completed.revalidate().unwrap();

    let input_claims = NativeCompatibilityInputClaims::parse(
        execution.inputs().bytes(),
        execution.inputs().sha256(),
    )
    .unwrap();
    let input_document: Value = serde_json::from_slice(input_claims.bytes()).unwrap();
    assert_eq!(
        input_document["schema"],
        "aros-toolchain-compatibility-inputs-v2"
    );
    assert!(input_document["native_sdk"].is_object());
    assert_eq!(
        input_document["sdk_source_tree_sha256"],
        completed.identity().source.content_sha256.to_string()
    );

    let evidence_files: BTreeMap<&str, &[u8]> =
        execution.evidence().files().collect::<BTreeMap<_, _>>();
    let sdk_names = evidence_files
        .keys()
        .filter(|name| name.starts_with("sdk-"))
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    let mut expected_sdk_names = BTreeSet::from([
        "sdk-aros-native-consumer-binding.json".to_owned(),
        "sdk-native-sdk-links.receipt.json".to_owned(),
        "sdk-native-sdk-inventory.json".to_owned(),
    ]);
    for label in ["original", "relocated"] {
        for language in ["c", "cxx"] {
            expected_sdk_names.insert(format!("sdk-{label}-{language}.elf"));
            expected_sdk_names.insert(format!("sdk-{label}-{language}.map"));
        }
    }
    for language in ["c", "cxx"] {
        expected_sdk_names.insert(format!("sdk-standalone-{language}.report.json"));
        for command in [1, 2] {
            for stream in ["stdout", "stderr"] {
                expected_sdk_names
                    .insert(format!("sdk-standalone-{language}.{command}.{stream}.log"));
            }
        }
    }
    assert_eq!(sdk_names, expected_sdk_names);
    assert!(evidence_files.contains_key("compatibility-measurement.json"));
    assert_eq!(
        &sha256_bytes(evidence_files["native-compatibility.receipt.json"]),
        execution.evidence().receipt_sha256()
    );

    let record_bytes = fs::read(completed.record_path()).unwrap();
    assert_eq!(completed.record_sha256(), &sha256_bytes(&record_bytes));
    let record: Value = serde_json::from_slice(&record_bytes).unwrap();
    assert_eq!(
        record["assurance"],
        "local-generation-binding-not-release-admission"
    );
    assert_eq!(
        record["compatibility_receipt_sha256"],
        report.receipt.sha256.to_string()
    );
    assert_eq!(
        record["sdk_link_receipt_sha256"],
        sha256_bytes(evidence_files["sdk-native-sdk-links.receipt.json"]).to_string()
    );
    assert_eq!(
        record["sdk_inventory_sha256"],
        native_sdk.sdk_inventory_sha256.to_string()
    );
    assert_eq!(
        record["sdk_entries"],
        serde_json::json!(native_sdk.sdk_entries)
    );
    assert_eq!(completed.tree_sha256(), &native_sdk.sdk_inventory_sha256);

    let packaged = sdk_generation::package_generated(&completed, &fixture.package_output, &[])
        .expect("the exported execution's opaque completion remains packageable");
    assert!(packaged.archive.is_file());
}

#[test]
fn generate_with_export_completion_rejects_input_record_and_sdk_mutations() {
    for mutation in [
        RetainedMutation::Source,
        RetainedMutation::Record,
        RetainedMutation::Sdk,
    ] {
        let fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
        let (completed, execution) =
            sdk_generation::generate_with_export(&fixture.request, &CancellationToken::default())
                .expect("baseline generation and export should complete once");
        assert_eq!(execution.report().probes.reports.len(), 6);

        let path = match mutation {
            RetainedMutation::Source => fixture.materialized_source_root.join("native-sdk-probe.c"),
            RetainedMutation::Record => completed.record_path().to_path_buf(),
            RetainedMutation::Sdk => fixture
                .compatibility_fixture
                .request
                .cmake_build_root
                .join("SYS/Developer/lib/client.a"),
            RetainedMutation::Map | RetainedMutation::Elf => unreachable!(),
        };
        append_byte(&path);
        let error = completed
            .revalidate()
            .expect_err("returned completion must reject changed retained bytes");
        assert!(
            match mutation {
                RetainedMutation::Source => {
                    error.to_string().contains("source") || error.to_string().contains("contract")
                }
                RetainedMutation::Record => error
                    .to_string()
                    .contains("completed SDK generation record changed"),
                RetainedMutation::Sdk => {
                    error.to_string().contains("SDK")
                        || error.to_string().contains("native")
                        || error.to_string().contains("compatibility")
                }
                RetainedMutation::Map | RetainedMutation::Elf => unreachable!(),
            },
            "unexpected {mutation:?} mutation rejection: {error}"
        );
        assert!(!fixture.package_output.exists());
    }
}

#[test]
fn downloaded_gnu_generation_readback_is_rootless_and_rejects_closed_record_and_artifact_mutations()
{
    let mut fixture = generation_fixture(NativeSdkBindingMode::Valid, 0);
    let source_lock = include_bytes!("../../tests/fixtures/gnu-source-lock-v3.json").to_vec();
    let profile_bytes = super::gnu_profiles(
        fixture
            .request
            .compatibility
            .upstream_source_commit
            .as_str(),
    );
    align_package_source_with_release_profiles(&mut fixture, &source_lock, &profile_bytes);
    let inputs =
        release_inputs_for_generation(&fixture.request, "gnu-rv32", &source_lock, &profile_bytes);
    let (completed, execution) =
        sdk_generation::generate_with_export(&fixture.request, &CancellationToken::default())
            .expect("source-bound GNU generation should produce real package and portable bytes");
    let record_bytes = fs::read(completed.record_path()).unwrap();
    let record_sha256 = sha256_bytes(&record_bytes);
    let record: Value = serde_json::from_slice(&record_bytes).unwrap();

    let packaged = sdk_generation::package_generated(&completed, &fixture.package_output, &[])
        .expect("generated GNU SDK should package before rootless readback");
    let downloads = tempfile::tempdir().unwrap();
    let downloaded_package = downloads.path().join("package");
    copy_flat_files(&packaged.output_dir, &downloaded_package);
    let package = SdkVerificationRequest {
        package_dir: downloaded_package.canonicalize().unwrap(),
        identity: completed.identity().clone(),
        expected_tree_sha256: completed.tree_sha256().clone(),
        expected_archive_sha256: packaged.archive_sha256.clone(),
        expected_archive_size: packaged.archive_size,
        forbidden_prefixes: Vec::new(),
    };

    let evidence_files = execution
        .evidence()
        .files()
        .map(|(name, bytes)| (name.to_owned(), bytes.to_vec()))
        .collect::<BTreeMap<_, _>>();
    let downloaded_evidence = downloads.path().join("compatibility");
    write_flat_files(&downloaded_evidence, &evidence_files);
    let compiler = fixture
        .request
        .compatibility
        .relocation
        .first
        .verified
        .manifest
        .compiler
        .as_ref()
        .expect("fixture package must retain its compiler identity");
    let package_expectation = crate::compatibility::NativeCompatibilityExpectedPackage {
        manifest: &fixture
            .request
            .compatibility
            .relocation
            .first
            .verified
            .manifest,
        archive_sha256: &fixture
            .request
            .compatibility
            .relocation
            .first
            .verified
            .archive_sha256,
        archive_size: fixture
            .request
            .compatibility
            .relocation
            .first
            .verified
            .archive_size,
        compiler,
        source_commit: fixture
            .request
            .compatibility
            .package_source_commit
            .as_ref()
            .unwrap(),
        host: &fixture.request.compatibility.host,
    };
    let expected = execution
        .inputs()
        .expectations(
            package_expectation,
            &fixture.request.profiles,
            &fixture.request.compatibility.profile,
            true,
        )
        .unwrap();
    let compatibility = crate::compatibility::PortableNativeCompatibilityRequest {
        directory: downloaded_evidence.canonicalize().unwrap(),
        manifest_sha256: execution.evidence().manifest_sha256().clone(),
        expected,
    };
    let readback = ReadbackSelection {
        inputs: &inputs,
        record_bytes: &record_bytes,
        record_sha256: &record_sha256,
        package: &package,
        compatibility: &compatibility,
        sdk_id: "example-sdk",
        compiler_group_id: "gnu-rv32",
    };
    let package_before = snapshot_flat_files(&package.package_dir);
    let evidence_before = snapshot_flat_files(&compatibility.directory);
    let moved_execution_root = fixture
        .execution_root
        .parent()
        .unwrap()
        .join("moved-execution");
    fs::rename(&fixture.execution_root, &moved_execution_root).unwrap();
    assert!(!fixture.execution_root.exists());

    let baseline = read_generation(&readback)
        .expect("downloaded GNU package and portable proof should join without original roots");
    assert_eq!(baseline.record_sha256(), &record_sha256);
    assert_eq!(baseline.package().archive_sha256, packaged.archive_sha256);
    assert_eq!(
        baseline.compatibility_receipt_sha256(),
        &execution.report().receipt.sha256
    );
    assert_eq!(
        baseline.sdk_link_receipt_sha256(),
        &execution
            .report()
            .native_sdk
            .as_ref()
            .unwrap()
            .receipt_sha256
    );
    assert_eq!(snapshot_flat_files(&package.package_dir), package_before);
    assert_eq!(
        snapshot_flat_files(&compatibility.directory),
        evidence_before
    );

    let mut stale_record = record_bytes.clone();
    stale_record.push(b' ');
    assert_generation_rejected(
        ReadbackSelection {
            record_bytes: &stale_record,
            ..readback
        },
        "differs from its selected raw digest",
    );

    let mut changed_identity = record.clone();
    changed_identity["identity"]["sdk_id"] = json!("substituted-sdk");
    assert_rehashed_record_rejected(
        readback,
        &changed_identity,
        "record differs from its independent source/environment selection",
    );
    for (field, value, reason) in [
        (
            "source_profile",
            json!("substituted-preset"),
            "record differs from its independent source/environment selection",
        ),
        (
            "environment_sha256",
            json!(sha256_bytes(b"substituted environment").to_string()),
            "record differs from its independent source/environment selection",
        ),
        (
            "compatibility_receipt_sha256",
            json!(sha256_bytes(b"substituted aggregate receipt").to_string()),
            "record differs from its complete compatibility receipt",
        ),
        (
            "sdk_link_receipt_sha256",
            json!(sha256_bytes(b"substituted SDK links receipt").to_string()),
            "record differs from its complete SDK link inventory",
        ),
        (
            "sdk_inventory_sha256",
            json!(sha256_bytes(b"substituted inventory").to_string()),
            "record differs from the actual measured SDK package inventory",
        ),
    ] {
        let mut altered = record.clone();
        altered[field] = value;
        assert_rehashed_record_rejected(readback, &altered, reason);
    }
    let mut changed_count = record.clone();
    changed_count["sdk_entries"] = json!(record["sdk_entries"].as_u64().unwrap() + 1);
    assert_rehashed_record_rejected(
        readback,
        &changed_count,
        "record differs from the actual measured SDK package inventory",
    );

    let mut unknown = record.clone();
    unknown["unknown"] = json!(true);
    assert_rehashed_record_rejected(readback, &unknown, "not closed GNU JSON");
    let canonical_record = crate::canonical::bytes(&record).unwrap();
    let duplicate_record = String::from_utf8(canonical_record)
        .unwrap()
        .replacen(
            "\"schema\":",
            "\"schema\":\"aros-sdk-generation-record-v1\",\"schema\":",
            1,
        )
        .into_bytes();
    let duplicate_record_sha256 = sha256_bytes(&duplicate_record);
    assert_generation_rejected(
        ReadbackSelection {
            record_bytes: &duplicate_record,
            record_sha256: &duplicate_record_sha256,
            ..readback
        },
        "not closed GNU JSON",
    );
    let noncanonical_record = serde_json::to_vec(&record).unwrap();
    let noncanonical_record_sha256 = sha256_bytes(&noncanonical_record);
    assert_generation_rejected(
        ReadbackSelection {
            record_bytes: &noncanonical_record,
            record_sha256: &noncanonical_record_sha256,
            ..readback
        },
        "not canonical JSON",
    );
    let mut wrong_family = record;
    wrong_family["schema"] = json!("aros-llvm-sdk-generation-record-v1");
    assert_rehashed_record_rejected(
        readback,
        &wrong_family,
        "schema or assurance differs from selected GNU policy",
    );

    assert_generation_rejected(
        ReadbackSelection {
            sdk_id: "missing-sdk",
            ..readback
        },
        "no independently selected product",
    );
    assert_generation_rejected(
        ReadbackSelection {
            compiler_group_id: "missing-group",
            ..readback
        },
        "no independently selected compiler group",
    );
    assert_generation_rejected(
        ReadbackSelection {
            sdk_id: "other-sdk",
            ..readback
        },
        "source or package differs from its independently selected identity",
    );
    let wrong_profile = fixture.request.profiles.select("rv32-alternate").unwrap();
    let wrong_profile_compatibility = crate::compatibility::PortableNativeCompatibilityRequest {
        expected: crate::compatibility::NativeCompatibilityReceiptExpectations {
            profile: wrong_profile,
            ..compatibility.expected.clone()
        },
        ..compatibility.clone()
    };
    assert_generation_rejected(
        ReadbackSelection {
            compatibility: &wrong_profile_compatibility,
            ..readback
        },
        "compiler selection differs from its independent release inputs",
    );

    for field in [
        "recipe_sha256",
        "source_lock_sha256",
        "profiles_sha256",
        "source_commit",
        "producer_commit",
        "tools_commit",
        "source_date_epoch",
    ] {
        let mut manifest = fixture
            .request
            .compatibility
            .relocation
            .first
            .verified
            .manifest
            .clone();
        match field {
            "recipe_sha256" => manifest.recipe_sha256 = sha256_bytes(b"other recipe").to_string(),
            "source_lock_sha256" => {
                manifest.source_lock_sha256 = sha256_bytes(b"other source lock").to_string();
            }
            "profiles_sha256" => {
                manifest.profiles_sha256 = sha256_bytes(b"other profiles").to_string();
            }
            "source_commit" => manifest.source_commit = "e".repeat(40),
            "producer_commit" => manifest.producer_commit = "f".repeat(40),
            "tools_commit" => manifest.tools_commit = "a".repeat(40),
            "source_date_epoch" => manifest.source_date_epoch += 1,
            _ => unreachable!(),
        }
        manifest.validate().unwrap();
        assert_manifest_selection_rejected(
            readback,
            &manifest,
            "compiler selection differs from its independent release inputs",
        );
    }

    let alternate_compiler = match compiler {
        ArosCompilerIdentity::Gnu { target, .. } => ArosCompilerIdentity::Gnu {
            gcc_version: "16.3.0".into(),
            binutils_version: "2.47".into(),
            target: target.clone(),
        },
        ArosCompilerIdentity::Llvm { .. } => unreachable!("GNU fixture selects GNU compiler"),
    };
    assert_compiler_selection_rejected(
        readback,
        &alternate_compiler,
        "compiler selection differs from its independent release inputs",
    );

    let changed_upstream_tree = GitObjectId::try_from("e".repeat(40)).unwrap();
    let changed_tree_compatibility = crate::compatibility::PortableNativeCompatibilityRequest {
        expected: crate::compatibility::NativeCompatibilityReceiptExpectations {
            upstream_source_tree: &changed_upstream_tree,
            ..compatibility.expected.clone()
        },
        ..compatibility.clone()
    };
    assert_generation_rejected(
        ReadbackSelection {
            compatibility: &changed_tree_compatibility,
            ..readback
        },
        "compiler selection differs from its independent release inputs",
    );

    let tampered_downloads = tempfile::tempdir().unwrap();
    let tampered_package_dir = tampered_downloads.path().join("package");
    copy_flat_files(&package.package_dir, &tampered_package_dir);
    let archive_name = fs::read_dir(&tampered_package_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .find(|name| name.ends_with(".tar.xz"))
        .unwrap();
    append_byte(&tampered_package_dir.join(archive_name));
    let tampered_package = SdkVerificationRequest {
        package_dir: tampered_package_dir.canonicalize().unwrap(),
        ..package.clone()
    };
    assert_generation_rejected(
        ReadbackSelection {
            package: &tampered_package,
            ..readback
        },
        "archive differs from its independently selected hash or size",
    );

    let mut missing_evidence_files = evidence_files.clone();
    missing_evidence_files.remove("sdk-original-c.elf").unwrap();
    assert_portable_inventory_mutation_rejected(
        readback,
        &missing_evidence_files,
        (downloads.path(), "missing-evidence"),
        "directory differs from its exact inventory",
    );
    let mut extra_evidence_files = evidence_files;
    extra_evidence_files.insert("unexpected-evidence.bin".into(), b"extra".to_vec());
    assert_portable_inventory_mutation_rejected(
        readback,
        &extra_evidence_files,
        (downloads.path(), "extra-evidence"),
        "directory differs from its exact inventory",
    );
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

    let error =
        sdk_generation::generate_with_export(&fixture.request, &CancellationToken::default())
            .expect_err("an existing generation record must not be adopted or executed");
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
    generation_fixture_for_host(mode, source_epoch, "linux-x86_64")
}

fn generation_fixture_for_host(
    mode: NativeSdkBindingMode,
    source_epoch: u64,
    host: &'static str,
) -> GenerationFixture {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let fixture_root = root.join("fixture");
    let record_root = root.join("records");
    let package_root = root.join("packages");
    fs::create_dir(&fixture_root).unwrap();
    fs::create_dir(&record_root).unwrap();
    fs::create_dir(&package_root).unwrap();

    let mut compatibility_fixture =
        super::gnu_native_sdk_fixture_for_host(&fixture_root, mode, host);
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
        execution_root: fixture_root,
        original_source_root,
        materialized_source_root: materialized.root,
        package_output,
    }
}

fn bind_generation_fixture_to_shared_source(
    fixture: &mut GenerationFixture,
    source: &crate::sdk_source::VerifiedSdkSource,
) {
    fixture.request.source = source.clone();
    fixture.materialized_source_root = source.materialized_source_root().to_path_buf();
    fixture
        .compatibility_fixture
        .request
        .preparation
        .source_root = source.materialized_source_root().to_path_buf();
    fixture
        .compatibility_fixture
        .request
        .preparation
        .source_tree_sha256 = source.identity().source.content_sha256.clone();
    let contract_path = source.materialized_source_root().join(CONTRACT_FILE);
    let contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    let binding = native_consumer_binding(
        &fixture.compatibility_fixture.request,
        &contract_path,
        &contract,
    );
    refresh_static_helper_binding(&mut fixture.compatibility_fixture.request, &binding);
    amend_cmake_to_build_native_sdk(
        &fixture.compatibility_fixture.request,
        &binding,
        NativeSdkBindingMode::Valid,
    );
    fixture.request.compatibility = fixture.compatibility_fixture.request.clone();
}

fn bind_generation_fixture_to_shared_upstream_source(
    fixture: &mut GenerationFixture,
    upstream_root: &Path,
    upstream_commit: &GitObjectId,
    ports_sources: &crate::compatibility_ports::CompatibilityPortsSources,
) {
    fixture.compatibility_fixture.request.upstream_source_root = upstream_root.to_path_buf();
    fixture.compatibility_fixture.request.upstream_source_commit = upstream_commit.clone();
    fixture.compatibility_fixture.request.ports_sources = ports_sources.clone();
    fixture.request.compatibility = fixture.compatibility_fixture.request.clone();
}

fn align_package_source_with_release_profiles(
    fixture: &mut GenerationFixture,
    source_lock: &[u8],
    profiles: &[u8],
) {
    // ReleaseInputs binds the recipe source commit to the exact profiles
    // upstream commit. Keep the fixture's profile bytes and source tree intact
    // while selecting that same commit in its synthetic compiler packages.
    let source_commit = fixture
        .request
        .compatibility
        .upstream_source_commit
        .as_str()
        .to_owned();
    fixture.request.compatibility.package_source_commit =
        Some(GitObjectId::try_from(source_commit.clone()).unwrap());
    let recipe_bytes = generation_recipe_bytes(&fixture.request, source_lock, profiles);
    let recipe = Recipe::parse(&recipe_bytes).unwrap();
    for package_root in [
        &fixture.request.compatibility.relocation.first.root,
        &fixture.request.compatibility.relocation.second.root,
    ] {
        let manifest_path = package_root.join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE);
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["source_commit"] = json!(source_commit.clone());
        manifest["recipe_sha256"] = json!(recipe.sha256().to_string());
        manifest["source_lock_sha256"] = json!(sha256_bytes(source_lock).to_string());
        manifest["profiles_sha256"] = json!(sha256_bytes(profiles).to_string());
        manifest["producer_commit"] = json!(recipe.producer().0.as_str());
        manifest["tools_commit"] = json!(recipe.tools().0.as_str());
        manifest["source_date_epoch"] = json!(recipe.source_date_epoch());
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
    let first =
        super::refresh_gnu_package_manifest(&fixture.request.compatibility.relocation.first.root);
    let second =
        super::refresh_gnu_package_manifest(&fixture.request.compatibility.relocation.second.root);
    assert_eq!(first, second);
    fixture
        .request
        .compatibility
        .relocation
        .first
        .verified
        .manifest = first;
    fixture
        .request
        .compatibility
        .relocation
        .second
        .verified
        .manifest = second;
}

fn release_inputs_for_generation(
    request: &SdkGenerationRequest,
    group_id: &str,
    source_lock: &[u8],
    profiles: &[u8],
) -> crate::release_inputs::ReleaseInputs {
    release_input_material_for_generation(request, group_id, source_lock, profiles, true).0
}

fn release_input_material_for_matrix_generation(
    request: &SdkGenerationRequest,
    group_id: &str,
    source_lock: &[u8],
    profiles: &[u8],
) -> (
    crate::release_inputs::ReleaseInputs,
    Vec<u8>,
    BTreeMap<String, Vec<u8>>,
) {
    release_input_material_for_generation(request, group_id, source_lock, profiles, false)
}

fn release_input_material_for_generation(
    request: &SdkGenerationRequest,
    group_id: &str,
    source_lock: &[u8],
    profiles: &[u8],
    include_alternate_sdk: bool,
) -> (
    crate::release_inputs::ReleaseInputs,
    Vec<u8>,
    BTreeMap<String, Vec<u8>>,
) {
    let profile_matrix = crate::profiles::Profiles::parse(profiles).unwrap();
    let package_commit = request
        .compatibility
        .package_source_commit
        .as_ref()
        .expect("fixture selects a package source commit");
    assert_eq!(profile_matrix.upstream_commit(), package_commit);
    let recipe_bytes = generation_recipe_bytes(request, source_lock, profiles);
    let mut documents = BTreeMap::from([
        ("recipe.json".to_owned(), recipe_bytes),
        ("source-lock.json".to_owned(), source_lock.to_vec()),
        ("profiles.json".to_owned(), profiles.to_vec()),
    ]);
    let identity = request.source.identity();
    let mut alternate_identity = identity.clone();
    alternate_identity.sdk_id = "other-sdk".into();
    for (filename, identity) in [
        ("sdk-identity.json", identity),
        ("other-sdk-identity.json", &alternate_identity),
    ] {
        if filename == "other-sdk-identity.json" && !include_alternate_sdk {
            continue;
        }
        documents.insert(
            filename.to_owned(),
            crate::canonical::bytes(&serde_json::to_value(identity).unwrap()).unwrap(),
        );
    }
    let document_ref = |filename: &str| {
        json!({
            "file": filename,
            "sha256": sha256_bytes(documents.get(filename).unwrap()).to_string(),
        })
    };
    let compiler_profiles = json!([{
        "group_id": group_id,
        "profile": request.compatibility.profile.name(),
    }]);
    let mut sdks = vec![json!({
        "id": identity.sdk_id,
        "identity": document_ref("sdk-identity.json"),
        "source_preset": request.source.source_profile().name,
        "compiler_profiles": compiler_profiles,
    })];
    if include_alternate_sdk {
        sdks.push(json!({
            "id": alternate_identity.sdk_id,
            "identity": document_ref("other-sdk-identity.json"),
            "source_preset": request.source.source_profile().name,
            "compiler_profiles": compiler_profiles,
        }));
    }
    let collection = crate::canonical::bytes(&json!({
        "schema": "aros-toolchain-release-inputs-v2",
        "producer_commit": "cccccccccccccccccccccccccccccccccccccccc",
        "tools_commit": "dddddddddddddddddddddddddddddddddddddddd",
        "hosts": crate::release_inputs::ACTIVE_HOSTS,
        "groups": [{
            "id": group_id,
            "recipe": document_ref("recipe.json"),
            "source_lock": document_ref("source-lock.json"),
            "profiles": document_ref("profiles.json"),
        }],
        "sdks": sdks,
    }))
    .unwrap();
    let inputs = crate::release_inputs::ReleaseInputs::parse(&collection, &documents).unwrap();
    (inputs, collection, documents)
}

fn generation_recipe_bytes(
    request: &SdkGenerationRequest,
    source_lock: &[u8],
    profiles: &[u8],
) -> Vec<u8> {
    let package_commit = request
        .compatibility
        .package_source_commit
        .as_ref()
        .expect("fixture selects a package source commit");
    let profile_matrix = crate::profiles::Profiles::parse(profiles).unwrap();
    assert_eq!(profile_matrix.upstream_commit(), package_commit);
    let source_tree = git_text(
        &request.compatibility.upstream_source_root,
        &["rev-parse", "HEAD:"],
    );
    let mut recipe = json!({
        "schema": "aros-toolchain-recipe-v2",
        "source_commit": package_commit.as_str(),
        "source_tree": source_tree,
        "producer_commit": "cccccccccccccccccccccccccccccccccccccccc",
        "producer_tree": "4444444444444444444444444444444444444444",
        "tools_commit": "dddddddddddddddddddddddddddddddddddddddd",
        "tools_tree": "6666666666666666666666666666666666666666",
        "source_date_epoch": 1_u64,
        "source_lock_sha256": sha256_bytes(source_lock).to_string(),
        "profiles_sha256": sha256_bytes(profiles).to_string(),
        "patches": []
    });
    recipe["recipe_sha256"] =
        json!(sha256_bytes(&crate::canonical::bytes(&recipe).unwrap()).to_string());
    crate::canonical::bytes(&recipe).unwrap()
}

fn gnu_rv32_matrix_profiles(upstream_commit: &str) -> Vec<u8> {
    let mut document: Value =
        serde_json::from_slice(&super::gnu_profiles(upstream_commit)).unwrap();
    let profiles = document["profiles"].as_array_mut().unwrap();
    profiles.retain(|profile| profile["name"] == "rv32-compat");
    assert_eq!(
        profiles.len(),
        1,
        "matrix fixture selects one GNU compiler profile"
    );
    serde_json::to_vec(&document).unwrap()
}

fn alternate_sdk_profile_inputs(
    fixture: &GenerationFixture,
    group_id: &str,
    source_lock: &[u8],
) -> crate::release_inputs::ReleaseInputs {
    let upstream_commit = fixture
        .request
        .compatibility
        .upstream_source_commit
        .as_str();
    let profiles = super::gnu_profiles(upstream_commit);
    let (_, mut collection, documents) = release_input_material_for_generation(
        &fixture.request,
        group_id,
        source_lock,
        &profiles,
        true,
    );
    let mut value: Value = serde_json::from_slice(&collection).unwrap();
    let compiler_profiles = value["sdks"][0]["compiler_profiles"]
        .as_array_mut()
        .unwrap();
    compiler_profiles.push(json!({ "group_id": group_id, "profile": "rv32-alternate" }));
    compiler_profiles.sort_by(|left, right| {
        (
            left["group_id"].as_str().unwrap(),
            left["profile"].as_str().unwrap(),
        )
            .cmp(&(
                right["group_id"].as_str().unwrap(),
                right["profile"].as_str().unwrap(),
            ))
    });
    collection = crate::canonical::bytes(&value).unwrap();
    crate::release_inputs::ReleaseInputs::parse(&collection, &documents).unwrap()
}

#[derive(Clone, Copy)]
struct ReadbackSelection<'a> {
    inputs: &'a crate::release_inputs::ReleaseInputs,
    record_bytes: &'a [u8],
    record_sha256: &'a aros_common::Sha256Digest,
    package: &'a SdkVerificationRequest,
    compatibility: &'a crate::compatibility::PortableNativeCompatibilityRequest<'a>,
    sdk_id: &'a str,
    compiler_group_id: &'a str,
}

fn read_generation(
    selection: &ReadbackSelection<'_>,
) -> Result<crate::sdk_generation::SdkGenerationReadback, crate::ContractError> {
    sdk_generation::readback_generation(&crate::sdk_generation::SdkGenerationReadbackRequest {
        inputs: selection.inputs,
        sdk_id: selection.sdk_id,
        compiler_group_id: selection.compiler_group_id,
        record_bytes: selection.record_bytes,
        record_sha256: selection.record_sha256,
        package: selection.package,
        compatibility: selection.compatibility,
    })
}

fn assert_generation_rejected(selection: ReadbackSelection<'_>, expected_reason: &str) {
    let package_before = snapshot_flat_files(&selection.package.package_dir);
    let evidence_before = snapshot_flat_files(&selection.compatibility.directory);
    let error =
        read_generation(&selection).expect_err("mutated generation inputs must be rejected");
    assert!(
        error.to_string().contains(expected_reason),
        "expected rejection containing {expected_reason:?}, got {error}"
    );
    assert_eq!(
        snapshot_flat_files(&selection.package.package_dir),
        package_before
    );
    assert_eq!(
        snapshot_flat_files(&selection.compatibility.directory),
        evidence_before
    );
}

fn assert_rehashed_record_rejected(
    selection: ReadbackSelection<'_>,
    record: &Value,
    expected_reason: &str,
) {
    let bytes = crate::canonical::bytes(record).unwrap();
    let digest = sha256_bytes(&bytes);
    assert_generation_rejected(
        ReadbackSelection {
            record_bytes: &bytes,
            record_sha256: &digest,
            ..selection
        },
        expected_reason,
    );
}

fn assert_manifest_selection_rejected(
    selection: ReadbackSelection<'_>,
    manifest: &ArosToolchainManifest,
    expected_reason: &str,
) {
    let compiler = selection.compatibility.expected.package.compiler;
    let altered_compatibility =
        with_package_expectation(selection.compatibility, manifest, compiler);
    assert_generation_rejected(
        ReadbackSelection {
            compatibility: &altered_compatibility,
            ..selection
        },
        expected_reason,
    );
}

fn assert_compiler_selection_rejected(
    selection: ReadbackSelection<'_>,
    compiler: &ArosCompilerIdentity,
    expected_reason: &str,
) {
    let mut manifest = selection.compatibility.expected.package.manifest.clone();
    manifest.compiler = Some(compiler.clone());
    manifest.validate().unwrap();
    let altered_compatibility =
        with_package_expectation(selection.compatibility, &manifest, compiler);
    assert_generation_rejected(
        ReadbackSelection {
            compatibility: &altered_compatibility,
            ..selection
        },
        expected_reason,
    );
}

fn with_package_expectation<'input: 'value, 'value>(
    compatibility: &crate::compatibility::PortableNativeCompatibilityRequest<'input>,
    manifest: &'value ArosToolchainManifest,
    compiler: &'value ArosCompilerIdentity,
) -> crate::compatibility::PortableNativeCompatibilityRequest<'value> {
    crate::compatibility::PortableNativeCompatibilityRequest {
        expected: crate::compatibility::NativeCompatibilityReceiptExpectations {
            package: crate::compatibility::NativeCompatibilityExpectedPackage {
                manifest,
                compiler,
                ..compatibility.expected.package.clone()
            },
            ..compatibility.expected.clone()
        },
        ..compatibility.clone()
    }
}

fn assert_portable_inventory_mutation_rejected(
    selection: ReadbackSelection<'_>,
    files: &BTreeMap<String, Vec<u8>>,
    destination: (&Path, &str),
    expected_reason: &str,
) {
    let (downloads_root, directory_name) = destination;
    let directory = downloads_root.join(directory_name);
    write_flat_files(&directory, files);
    let altered_compatibility = crate::compatibility::PortableNativeCompatibilityRequest {
        directory: directory.canonicalize().unwrap(),
        ..selection.compatibility.clone()
    };
    assert_generation_rejected(
        ReadbackSelection {
            compatibility: &altered_compatibility,
            ..selection
        },
        expected_reason,
    );
}

fn copy_flat_files(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
    }
}

fn write_flat_files(destination: &Path, files: &BTreeMap<String, Vec<u8>>) {
    fs::create_dir(destination).unwrap();
    for (name, bytes) in files {
        assert_eq!(Path::new(name).file_name().unwrap(), name.as_str());
        fs::write(destination.join(name), bytes).unwrap();
    }
}

fn snapshot_flat_files(directory: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
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

struct GeneratedMatrixLane {
    host: String,
    fixture: GenerationFixture,
    compiler_package: VerifiedPackage,
    compiler_source_commit: GitObjectId,
    record_bytes: Vec<u8>,
    record_sha256: aros_common::Sha256Digest,
    sdk_package: SdkPackageOutput,
    sdk_tree_sha256: aros_common::Sha256Digest,
    execution: NativeCompatibilityExecutionExport,
    compatibility_directory: PathBuf,
}

#[derive(Clone)]
struct MatrixSelection<'a> {
    lane: ExpectedSdkGenerationLane,
    package: SdkVerificationRequest,
    compatibility: PortableNativeCompatibilityRequest<'a>,
    record_bytes: Vec<u8>,
    record_sha256: aros_common::Sha256Digest,
}

fn matrix_generation_requests<'a>(
    inputs: &'a crate::release_inputs::ReleaseInputs,
    selections: &'a [MatrixSelection<'a>],
) -> BTreeMap<ExpectedSdkGenerationLane, crate::sdk_generation::SdkGenerationReadbackRequest<'a>> {
    selections
        .iter()
        .map(|selection| {
            (
                selection.lane.clone(),
                crate::sdk_generation::SdkGenerationReadbackRequest {
                    inputs,
                    sdk_id: selection.lane.sdk_id(),
                    compiler_group_id: selection.lane.group_id(),
                    record_bytes: &selection.record_bytes,
                    record_sha256: &selection.record_sha256,
                    package: &selection.package,
                    compatibility: &selection.compatibility,
                },
            )
        })
        .collect()
}

fn assert_matrix_rejected<'e>(
    complete: &ReleaseCompatibilityReadbackRequestV2<'e>,
    generations: BTreeMap<
        ExpectedSdkGenerationLane,
        crate::sdk_generation::SdkGenerationReadbackRequest<'e>,
    >,
    expected_message: &str,
) {
    let error = readback_sdk_generation_matrix(&SdkGenerationMatrixReadbackRequest {
        complete,
        generations,
    })
    .expect_err("mutated matrix selection must be rejected");
    assert!(
        error.to_string().contains(expected_message),
        "unexpected matrix rejection: {error}"
    );
}

#[test]
fn complete_gnu_sdk_generation_matrix_joins_three_synthetic_host_rows() {
    const SDK_RELEASE_ID: &str = "v1.2.3";
    const COMPILER_GROUP: &str = "gnu-rv32";

    let source_lock = include_bytes!("../../tests/fixtures/gnu-source-lock-v3.json").to_vec();
    let mut shared_source = None;
    let mut shared_upstream_source: Option<(PathBuf, GitObjectId)> = None;
    let mut shared_ports_sources = None;
    let mut canonical_recipe = None;
    let mut canonical_profiles = None;
    let mut canonical_collection = None;
    let mut canonical_documents = None;
    let mut sdk_identity = None;
    let mut package_pairs = BTreeMap::<String, (GuardedPackage, GuardedPackage)>::new();
    let mut rows = Vec::new();

    for host in crate::release_inputs::ACTIVE_HOSTS.iter().copied() {
        // These labels select three independent synthetic host rows. All
        // processes run in this test process; this is not cross-host CI proof.
        let mut fixture = generation_fixture_for_host(NativeSdkBindingMode::Valid, 0, host);
        if let Some(source) = &shared_source {
            bind_generation_fixture_to_shared_source(&mut fixture, source);
            let (upstream_root, upstream_commit) = shared_upstream_source
                .as_ref()
                .expect("the first host row selects the shared upstream source");
            bind_generation_fixture_to_shared_upstream_source(
                &mut fixture,
                upstream_root,
                upstream_commit,
                shared_ports_sources
                    .as_ref()
                    .expect("the first host row selects the shared port-source root"),
            );
        } else {
            shared_source = Some(fixture.request.source.clone());
            shared_upstream_source = Some((
                fixture.request.compatibility.upstream_source_root.clone(),
                fixture.request.compatibility.upstream_source_commit.clone(),
            ));
            shared_ports_sources = Some(fixture.request.compatibility.ports_sources.clone());
        }
        let profiles = gnu_rv32_matrix_profiles(
            fixture
                .request
                .compatibility
                .upstream_source_commit
                .as_str(),
        );
        let parsed_profiles = crate::profiles::Profiles::parse(&profiles)
            .expect("shared upstream commit should parse into the selected profiles");
        let compiler_profile = parsed_profiles
            .select("rv32-compat")
            .expect("matrix generation selects rv32-compat")
            .clone();
        fixture.request.profiles = parsed_profiles.clone();
        fixture.compatibility_fixture.profiles = parsed_profiles;
        fixture.request.compatibility.profile = compiler_profile.clone();
        fixture.compatibility_fixture.request.profile = compiler_profile;
        align_package_source_with_release_profiles(&mut fixture, &source_lock, &profiles);
        let recipe = generation_recipe_bytes(&fixture.request, &source_lock, &profiles);

        if let Some(expected) = &canonical_recipe {
            assert_eq!(&recipe, expected, "host rows must share exact recipe bytes");
            assert_eq!(
                &profiles,
                canonical_profiles.as_ref().unwrap(),
                "host rows must share exact profile bytes"
            );
        } else {
            canonical_recipe = Some(recipe.clone());
            canonical_profiles = Some(profiles.clone());
        }

        let payload_root = fixture.request.compatibility.relocation.first.root.clone();
        let left = guarded_gnu_package_from_payload_for_host(
            host,
            &recipe,
            &source_lock,
            &profiles,
            "rv32-compat",
            &payload_root,
            SDK_RELEASE_ID,
        );
        let right = guarded_gnu_package_from_payload_for_host(
            host,
            &recipe,
            &source_lock,
            &profiles,
            "rv32-compat",
            &payload_root,
            SDK_RELEASE_ID,
        );
        let left_extracted = crate::package_extract::verify_and_extract(
            &crate::package_extract::PackageExtractionRequest {
                verification: left.verification().clone(),
                output_root: fixture.execution_root.join("compiler-extracted-left"),
            },
        )
        .expect("left compiler package should reverify and extract its exact archive");
        let right_extracted = crate::package_extract::verify_and_extract(
            &crate::package_extract::PackageExtractionRequest {
                verification: right.verification().clone(),
                output_root: fixture.execution_root.join("compiler-extracted-right"),
            },
        )
        .expect("right compiler package should reverify and extract its exact archive");
        let compiler_package = left_extracted.verified.clone();
        assert_eq!(compiler_package, right_extracted.verified);
        assert_eq!(compiler_package, *right.verified_package());
        let compiler_source_commit =
            GitObjectId::try_from(compiler_package.manifest.source_commit.clone()).unwrap();
        assert_eq!(compiler_package.manifest.host, host);

        let verified = compiler_package.clone();
        // Keep the selected recipe/profile/source bindings established above
        // when replacing the fixture's synthetic compiler package roots.
        fixture.compatibility_fixture.request = fixture.request.compatibility.clone();
        fixture.compatibility_fixture.request.relocation = TwoRootRelocation {
            first: ExtractedPackage {
                root: left_extracted.root,
                verified: verified.clone(),
            },
            second: ExtractedPackage {
                root: right_extracted.root,
                verified,
            },
        };
        fixture.request.compatibility = fixture.compatibility_fixture.request.clone();

        let (inputs, collection, documents) = release_input_material_for_matrix_generation(
            &fixture.request,
            COMPILER_GROUP,
            &source_lock,
            &profiles,
        );
        assert_eq!(inputs.expected_sdk_generation_lanes().len(), 3);
        if let Some(expected) = &canonical_collection {
            assert_eq!(
                &collection, expected,
                "all hosts must share one release input"
            );
            assert_eq!(
                &documents,
                canonical_documents.as_ref().unwrap(),
                "all hosts must select the same release documents"
            );
        } else {
            canonical_collection = Some(collection);
            canonical_documents = Some(documents);
            sdk_identity = Some(fixture.request.source.identity().clone());
        }

        let (completed, execution) =
            sdk_generation::generate_with_export(&fixture.request, &CancellationToken::default())
                .expect("each synthetic host row should execute GNU generation and export");
        let record_bytes = fs::read(completed.record_path()).unwrap();
        let record_sha256 = completed.record_sha256().clone();
        let sdk_tree_sha256 = completed.tree_sha256().clone();
        let forbidden_prefixes = vec![PathBuf::from("/sdk-generation-matrix-forbidden-prefix")];
        let sdk_package = sdk_generation::package_generated(
            &completed,
            &fixture.package_output,
            &forbidden_prefixes,
        )
        .expect("the exact generated SDK should package for each host row");
        let compatibility_directory = fixture
            .execution_root
            .join(format!("downloaded-compatibility-{host}"));
        write_flat_files(
            &compatibility_directory,
            &execution
                .evidence()
                .files()
                .map(|(name, bytes)| (name.to_owned(), bytes.to_vec()))
                .collect(),
        );

        package_pairs.insert(host.to_owned(), (left, right));
        rows.push(GeneratedMatrixLane {
            host: host.to_owned(),
            fixture,
            compiler_package,
            compiler_source_commit,
            record_bytes,
            record_sha256,
            sdk_package,
            sdk_tree_sha256,
            execution,
            compatibility_directory,
        });
    }

    assert_eq!(rows.len(), crate::release_inputs::ACTIVE_HOSTS.len());
    for row in rows.iter().skip(1) {
        assert_eq!(
            fs::read(&row.sdk_package.archive).unwrap(),
            fs::read(&rows[0].sdk_package.archive).unwrap(),
            "all hosts must generate identical SDK archive bytes"
        );
        assert_eq!(
            fs::read(&row.sdk_package.manifest).unwrap(),
            fs::read(&rows[0].sdk_package.manifest).unwrap(),
            "all hosts must generate identical SDK manifest bytes"
        );
    }

    let prepared = prepare_gnu_matrix_release(
        canonical_collection.unwrap(),
        canonical_documents.unwrap(),
        sdk_identity.unwrap(),
        package_pairs,
        &rows[0].sdk_package,
        &rows[0].sdk_tree_sha256,
    );
    with_gnu_matrix_complete_request(&prepared, |complete| {
        let inputs = &complete.builds.packages.inputs;
        let mut owned_selections = Vec::new();
        for lane in inputs.expected_sdk_generation_lanes() {
            let row = rows
                .iter()
                .find(|row| row.host == lane.host())
                .expect("one independently generated row per selected host");
            let compiler = row
                .compiler_package
                .manifest
                .compiler
                .as_ref()
                .expect("GNU package identity is present");
            let expected_package = NativeCompatibilityExpectedPackage {
                manifest: &row.compiler_package.manifest,
                archive_sha256: &row.compiler_package.archive_sha256,
                archive_size: row.compiler_package.archive_size,
                compiler,
                source_commit: &row.compiler_source_commit,
                host: &row.host,
            };
            let expected = row
                .execution
                .inputs()
                .expectations(
                    expected_package,
                    &row.fixture.request.profiles,
                    &row.fixture.request.compatibility.profile,
                    true,
                )
                .expect("exported proof expectations bind the exact host compiler package");
            let compatibility = PortableNativeCompatibilityRequest {
                directory: row.compatibility_directory.canonicalize().unwrap(),
                manifest_sha256: row.execution.evidence().manifest_sha256().clone(),
                expected,
            };
            let package = SdkVerificationRequest {
                package_dir: row.sdk_package.output_dir.canonicalize().unwrap(),
                identity: row.fixture.request.source.identity().clone(),
                expected_tree_sha256: row.sdk_tree_sha256.clone(),
                expected_archive_sha256: row.sdk_package.archive_sha256.clone(),
                expected_archive_size: row.sdk_package.archive_size,
                forbidden_prefixes: vec![PathBuf::from("/sdk-generation-matrix-forbidden-prefix")],
            };
            owned_selections.push(MatrixSelection {
                lane,
                package,
                compatibility,
                record_bytes: row.record_bytes.clone(),
                record_sha256: row.record_sha256.clone(),
            });
        }
        let generations = matrix_generation_requests(inputs, &owned_selections);
        let matrix_request = SdkGenerationMatrixReadbackRequest {
            complete,
            generations,
        };
        let matrix = readback_sdk_generation_matrix(&matrix_request)
            .expect("complete three-host GNU generation rows should join the indexed release");
        assert_eq!(matrix.generations().len(), 3);
        for indexed in matrix.generations() {
            assert_eq!(indexed.lane().sdk_id(), "example-sdk");
            assert_eq!(indexed.lane().group_id(), COMPILER_GROUP);
            assert_eq!(indexed.lane().profile(), "rv32-compat");
            assert_eq!(
                indexed.generation().package().archive_sha256,
                rows.iter()
                    .find(|row| row.host == indexed.lane().host())
                    .unwrap()
                    .sdk_package
                    .archive_sha256
            );
        }

        let cli_generation_inputs = owned_selections
            .iter()
            .map(|selection| {
                let row = rows
                    .iter()
                    .find(|row| row.host == selection.lane.host())
                    .expect("one generated input observation per selected host");
                (
                    selection.lane.clone(),
                    (
                        row.execution.inputs().bytes(),
                        row.execution.inputs().sha256(),
                    ),
                )
            })
            .collect();
        sdk_generation_matrix_cli_tests::probe_if_configured(
            &matrix_request,
            &cli_generation_inputs,
        );

        let mut missing_host = matrix_generation_requests(inputs, &owned_selections);
        let missing_lane = missing_host.keys().next().unwrap().clone();
        missing_host.remove(&missing_lane);
        assert_matrix_rejected(
            complete,
            missing_host,
            "SDK generation matrix lacks exact input-derived host/profile coverage",
        );

        let alternate_inputs = alternate_sdk_profile_inputs(
            &rows[0].fixture,
            COMPILER_GROUP,
            inputs.groups()[0].source_lock_bytes(),
        );
        let alternate_lanes = alternate_inputs.expected_sdk_generation_lanes();
        let extra_product_lane = alternate_lanes
            .iter()
            .find(|lane| lane.sdk_id() == "other-sdk" && lane.profile() == "rv32-compat")
            .unwrap()
            .clone();
        let source_selection = owned_selections
            .iter()
            .find(|selection| selection.lane.host() == extra_product_lane.host())
            .unwrap();
        let mut extra_product = matrix_generation_requests(inputs, &owned_selections);
        extra_product.insert(
            extra_product_lane.clone(),
            crate::sdk_generation::SdkGenerationReadbackRequest {
                inputs,
                sdk_id: extra_product_lane.sdk_id(),
                compiler_group_id: extra_product_lane.group_id(),
                record_bytes: &source_selection.record_bytes,
                record_sha256: &source_selection.record_sha256,
                package: &source_selection.package,
                compatibility: &source_selection.compatibility,
            },
        );
        assert_matrix_rejected(
            complete,
            extra_product,
            "SDK generation matrix lacks exact input-derived host/profile coverage",
        );

        let alternate_profile_lane = alternate_lanes
            .iter()
            .find(|lane| lane.sdk_id() == "example-sdk" && lane.profile() == "rv32-alternate")
            .unwrap()
            .clone();
        let profile_selection = owned_selections
            .iter()
            .find(|selection| selection.lane.host() == alternate_profile_lane.host())
            .unwrap();
        let mut wrong_profile = matrix_generation_requests(inputs, &owned_selections);
        let replaced_profile_lane = profile_selection.lane.clone();
        wrong_profile.remove(&replaced_profile_lane);
        wrong_profile.insert(
            alternate_profile_lane,
            crate::sdk_generation::SdkGenerationReadbackRequest {
                inputs,
                sdk_id: "example-sdk",
                compiler_group_id: COMPILER_GROUP,
                record_bytes: &profile_selection.record_bytes,
                record_sha256: &profile_selection.record_sha256,
                package: &profile_selection.package,
                compatibility: &profile_selection.compatibility,
            },
        );
        assert_matrix_rejected(
            complete,
            wrong_profile,
            "SDK generation matrix lacks exact input-derived host/profile coverage",
        );

        let mut wrong_group = matrix_generation_requests(inputs, &owned_selections);
        let group_lane = wrong_group.keys().next().unwrap().clone();
        wrong_group.get_mut(&group_lane).unwrap().compiler_group_id = "other-group";
        assert_matrix_rejected(
            complete,
            wrong_group,
            "SDK generation matrix selectors differ from their input-derived tuple",
        );

        let mut wrong_product_selector = matrix_generation_requests(inputs, &owned_selections);
        let product_lane = wrong_product_selector.keys().next().unwrap().clone();
        wrong_product_selector
            .get_mut(&product_lane)
            .unwrap()
            .sdk_id = "other-sdk";
        assert_matrix_rejected(
            complete,
            wrong_product_selector,
            "SDK generation matrix selectors differ from their input-derived tuple",
        );

        let mut wrong_host = owned_selections.clone();
        wrong_host[0].compatibility.expected.package.host = "not-an-indexed-host";
        assert_matrix_rejected(
            complete,
            matrix_generation_requests(inputs, &wrong_host),
            "SDK generation matrix selectors differ from their input-derived tuple",
        );

        let mut wrong_compiler = owned_selections.clone();
        let mismatched_host = wrong_compiler
            .iter_mut()
            .find(|selection| selection.lane.host() != rows[0].host)
            .unwrap();
        let canonical_compiler = &rows[0].compiler_package;
        let expected = &mut mismatched_host.compatibility.expected.package;
        expected.manifest = &canonical_compiler.manifest;
        expected.archive_sha256 = &canonical_compiler.archive_sha256;
        expected.archive_size = canonical_compiler.archive_size;
        expected.compiler = canonical_compiler.manifest.compiler.as_ref().unwrap();
        expected.source_commit = &rows[0].compiler_source_commit;
        assert_matrix_rejected(
            complete,
            matrix_generation_requests(inputs, &wrong_compiler),
            "SDK generation compiler differs from the measured indexed package",
        );

        let mut changed_record = owned_selections.clone();
        let record = &mut changed_record[0];
        let mut record_value: Value = serde_json::from_slice(&record.record_bytes).unwrap();
        record_value["environment_sha256"] = json!(sha256_bytes(b"wrong SDK environment"));
        record.record_bytes = crate::canonical::bytes(&record_value).unwrap();
        record.record_sha256 = sha256_bytes(&record.record_bytes);
        assert_matrix_rejected(
            complete,
            matrix_generation_requests(inputs, &changed_record),
            "SDK generation record differs from its independent source/environment selection",
        );

        let mut aliased_root = owned_selections.clone();
        aliased_root[0].package.package_dir = complete.builds.packages.directory.clone();
        assert_matrix_rejected(
            complete,
            matrix_generation_requests(inputs, &aliased_root),
            "SDK generation matrix roots must be physically nonoverlapping",
        );
    });
}
