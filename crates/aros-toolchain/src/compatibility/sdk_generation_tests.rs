//! End-to-end regression coverage for source-bound GNU SDK generation.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::{sha256_bytes, ArosCompilerIdentity, ArosToolchainManifest, CancellationToken};
use serde_json::{json, Value};

use super::{
    amend_cmake_to_build_native_sdk, gnu_native_sdk_fixture, native_consumer_binding, shell_quote,
    Fixture, NativeCompatibilityRequest, NativeSdkBindingMode, NATIVE_CONSUMER_CONTRACT_FILE,
};
use crate::compatibility::NativeCompatibilityInputClaims;
use crate::compatibility_source::{
    materialize_engine_free_source, CommittedSourceIdentity, EngineFreeSourceRequest,
};
use crate::recipe::{GitObjectId, Recipe};
use crate::sdk_generation::{self, SdkGenerationRequest};
use crate::sdk_package::{self, SdkVerificationRequest};
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
        execution_root: fixture_root,
        original_source_root,
        materialized_source_root: materialized.root,
        package_output,
    }
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
        "sdks": [
            {
                "id": identity.sdk_id,
                "identity": document_ref("sdk-identity.json"),
                "source_preset": request.source.source_profile().name,
                "compiler_profiles": compiler_profiles,
            },
            {
                "id": alternate_identity.sdk_id,
                "identity": document_ref("other-sdk-identity.json"),
                "source_preset": request.source.source_profile().name,
                "compiler_profiles": compiler_profiles,
            },
        ],
    }))
    .unwrap();
    crate::release_inputs::ReleaseInputs::parse(&collection, &documents).unwrap()
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
