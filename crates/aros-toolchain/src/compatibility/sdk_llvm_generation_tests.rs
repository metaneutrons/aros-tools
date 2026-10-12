//! Synthetic source-bound LLVM SDK generation and packaging coverage.
//!
//! The committed source, generated SDK bytes and compiler drivers are fixtures.
//! They exercise retained identity and digest wiring, not a real LLVM build.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::{
    sha256_bytes, ArosCompilerIdentity, ArosToolchainManifest, CancellationToken, Sha256Digest,
};
use serde_json::{json, Value};

use super::llvm_sdk_tests::{self, BindingMode, Fixture as CompatibilityFixture};
use crate::compatibility::{
    NativeCompatibilityExpectedPackage, NativeCompatibilityRequest,
    PortableNativeCompatibilityRequest, TwoRootRelocation,
};
use crate::native_candidate::{
    guarded_llvm_package_from_payload_for_host, prepare_llvm_matrix_release,
    with_llvm_matrix_complete_request, GuardedPackage, ReleaseCompatibilityReadbackRequestV2,
};
use crate::package_extract::ExtractedPackage;
use crate::package_verify::VerifiedPackage;
use crate::profiles::{Profile, Profiles};
use crate::recipe::{GitObjectId, Recipe};
use crate::sdk_generation::{self, SdkGenerationRequest};
use crate::sdk_generation_lanes::ExpectedSdkGenerationLane;
use crate::sdk_generation_matrix::{
    readback_sdk_generation_matrix, SdkGenerationMatrixReadbackRequest,
};
use crate::sdk_package::SdkVerificationRequest;
use crate::sdk_source::VerifiedSdkSource;

struct GenerationFixture {
    _temporary: tempfile::TempDir,
    compatibility: CompatibilityFixture,
    request: SdkGenerationRequest,
    execution_root: PathBuf,
    package_output: PathBuf,
}

#[test]
fn generates_packages_and_exports_the_exact_source_bound_llvm_sdk() {
    let fixture = generation_fixture();
    let (completed, execution) =
        sdk_generation::generate_with_export(&fixture.request, &CancellationToken::default())
            .expect("synthetic verified LLVM source should complete generation and export");
    let llvm_sdk = execution
        .report()
        .llvm_sdk
        .as_ref()
        .expect("execution should retain its LLVM SDK link report");
    assert_eq!(execution.report().probes.reports.len(), 6);
    assert_eq!(completed.identity(), fixture.request.source.identity());
    assert_eq!(completed.tree_sha256(), &llvm_sdk.sdk_inventory_sha256);
    completed.revalidate().unwrap();

    let record_bytes = fs::read(completed.record_path()).unwrap();
    assert_eq!(completed.record_sha256(), &sha256_bytes(&record_bytes));
    let record: Value = serde_json::from_slice(&record_bytes).unwrap();
    assert_eq!(record["schema"], "aros-llvm-sdk-generation-record-v1");
    assert_eq!(
        record["identity"],
        serde_json::to_value(completed.identity()).unwrap()
    );
    assert_eq!(
        record["binding_sha256"],
        llvm_sdk.binding_sha256.to_string()
    );
    let environment =
        crate::compatibility::derive_native_compatibility_environment_identity_with_epoch(
            &fixture.request.compatibility.host_python,
            &fixture.request.compatibility.host_tools,
            &fixture.request.compatibility.host,
            fixture.request.compatibility.consumer_source_date_epoch,
        )
        .unwrap();
    assert_eq!(
        record["environment_sha256"],
        environment.sdk_environment_sha256.to_string()
    );
    assert_eq!(
        record["compatibility_receipt_sha256"],
        execution.report().receipt.sha256.to_string()
    );
    assert_eq!(
        record["sdk_link_receipt_sha256"],
        llvm_sdk.receipt_sha256.to_string()
    );
    assert_eq!(
        record["sdk_inventory_sha256"],
        llvm_sdk.sdk_inventory_sha256.to_string()
    );
    assert_eq!(record["sdk_entries"], llvm_sdk.sdk_entries);

    let evidence = execution.evidence().files().collect::<Vec<_>>();
    let exported_binding = evidence
        .iter()
        .find(|(name, _)| *name == "llvm-sdk-aros-native-consumer-binding.json")
        .map(|(_, bytes)| *bytes)
        .expect("portable export should include the source-bound LLVM CMake binding");
    assert_eq!(sha256_bytes(exported_binding), llvm_sdk.binding_sha256);
    let input_claims = crate::compatibility::NativeCompatibilityInputClaims::parse(
        execution.inputs().bytes(),
        execution.inputs().sha256(),
    )
    .unwrap();
    let input_document: Value = serde_json::from_slice(input_claims.bytes()).unwrap();
    assert_eq!(input_document["llvm_sdk"]["identity"], record["identity"]);

    let packaged = sdk_generation::package_generated(&completed, &fixture.package_output, &[])
        .expect("only the completed LLVM SDK should be packaged");
    assert!(packaged.archive.is_file());
    assert!(packaged.manifest.is_file());
    assert!(packaged.checksum.is_file());
    completed.revalidate().unwrap();
}

#[test]
fn revalidation_rejects_changed_llvm_binding_sdk_and_generation_record() {
    for changed in ["binding", "sdk", "record"] {
        let fixture = generation_fixture();
        let completed = sdk_generation::generate(&fixture.request, &CancellationToken::default())
            .expect("synthetic verified LLVM source should complete generation");
        let path = match changed {
            "binding" => fixture.compatibility.binding_path.clone(),
            "sdk" => fixture
                .request
                .compatibility
                .cmake_build_root
                .join("products/ci-selected-sdk-layout/include/public.h"),
            "record" => completed.record_path().to_path_buf(),
            _ => unreachable!(),
        };
        append_byte(&path);
        let error = completed
            .revalidate()
            .expect_err("retained LLVM generation must reject changed bytes");
        match changed {
            "binding" => assert!(
                error.to_string().contains("binding") || error.to_string().contains("LLVM"),
                "unexpected binding mutation rejection: {error}"
            ),
            "sdk" => assert!(
                error.to_string().contains("SDK") || error.to_string().contains("LLVM"),
                "unexpected SDK mutation rejection: {error}"
            ),
            "record" => assert!(error
                .to_string()
                .contains("completed SDK generation record changed")),
            _ => unreachable!(),
        }
        assert!(!fixture.package_output.exists());
    }
}

#[test]
fn rejects_mixed_llvm_source_selection_before_execution_outputs() {
    let mut fixture = generation_fixture();
    fixture.request.compatibility.source_preset = Some("pc-x86_64".into());

    let error = sdk_generation::generate(&fixture.request, &CancellationToken::default())
        .expect_err("LLVM generation must reject a GNU source-preset selector");
    assert!(error.to_string().contains("LLVM SDK generation"));
    assert_no_execution_outputs(&fixture);
    assert!(!fixture.request.record_path.exists());
    assert!(!fixture.package_output.exists());
}

#[test]
fn rejects_missing_source_epoch_and_legacy_package_before_execution_outputs() {
    for mismatch in ["source", "epoch", "first-package", "second-package"] {
        let mut fixture = generation_fixture();
        let expected = match mismatch {
            "source" => {
                fixture.request.compatibility.llvm_sdk_source = None;
                "verified generic source selection"
            }
            "epoch" => {
                fixture.request.compatibility.consumer_source_date_epoch = Some(0);
                "epoch differs"
            }
            "first-package" => {
                fixture
                    .request
                    .compatibility
                    .relocation
                    .first
                    .verified
                    .manifest
                    .schema = 1;
                "family-v2 compiler package"
            }
            "second-package" => {
                fixture
                    .request
                    .compatibility
                    .relocation
                    .second
                    .verified
                    .manifest
                    .schema = 1;
                "family-v2 compiler package"
            }
            _ => unreachable!(),
        };
        let error = sdk_generation::generate(&fixture.request, &CancellationToken::default())
            .expect_err("invalid LLVM generation selection must fail before execution");
        assert!(error.to_string().contains(expected), "{mismatch}: {error}");
        assert_no_execution_outputs(&fixture);
        assert!(!fixture.request.record_path.exists());
        assert!(!fixture.package_output.exists());
    }
}

#[test]
fn downloaded_llvm_generation_readback_is_rootless_and_rejects_closed_record_and_artifact_mutations(
) {
    let mut fixture = generation_fixture();
    let source_lock = llvm_source_lock_bytes();
    let profile_bytes = llvm_profiles_bytes(
        fixture
            .request
            .compatibility
            .upstream_source_commit
            .as_str(),
    );
    align_package_source_with_release_profiles(&mut fixture, &source_lock, &profile_bytes);
    let inputs = release_inputs_for_generation(&fixture.request, &source_lock, &profile_bytes);
    let (completed, execution) =
        sdk_generation::generate_with_export(&fixture.request, &CancellationToken::default())
            .expect("synthetic LLVM generation should produce real package and portable bytes");
    let record_bytes = fs::read(completed.record_path()).unwrap();
    let record_sha256 = sha256_bytes(&record_bytes);
    let record: Value = serde_json::from_slice(&record_bytes).unwrap();

    let packaged = sdk_generation::package_generated(&completed, &fixture.package_output, &[])
        .expect("generated LLVM SDK should package before rootless readback");
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
    let selected_identity = fixture.request.source.identity().clone();
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
        .expectations_with_llvm_sdk(
            package_expectation,
            &fixture.request.profiles,
            &fixture.request.compatibility.profile,
            &selected_identity,
        )
        .unwrap();
    let compatibility = crate::compatibility::PortableNativeCompatibilityRequest {
        directory: downloaded_evidence.canonicalize().unwrap(),
        manifest_sha256: execution.evidence().manifest_sha256().clone(),
        expected,
    };
    assert_eq!(
        sha256_bytes(&profile_bytes),
        *fixture.request.compatibility.profile.document_sha256(),
        "test release inputs must use the exact compatibility profiles bytes"
    );

    let package_before = snapshot_flat_files(&package.package_dir);
    let evidence_before = snapshot_flat_files(&compatibility.directory);
    let readback = ReadbackSelection {
        inputs: &inputs,
        record_bytes: &record_bytes,
        record_sha256: &record_sha256,
        package: &package,
        compatibility: &compatibility,
        sdk_id: "pc-x86_64",
        compiler_group_id: "llvm",
    };
    let moved_execution_root = fixture
        .execution_root
        .parent()
        .unwrap()
        .join("moved-execution");
    fs::rename(&fixture.execution_root, &moved_execution_root).unwrap();
    assert!(!fixture.execution_root.exists());

    let baseline = read_generation(&readback)
        .expect("downloaded LLVM package and portable proof should join without original roots");
    assert_eq!(baseline.record_sha256(), &record_sha256);
    assert_eq!(baseline.package().archive_sha256, packaged.archive_sha256);
    assert_eq!(
        baseline.compatibility_receipt_sha256(),
        &execution.report().receipt.sha256
    );
    assert_eq!(
        baseline.sdk_link_receipt_sha256(),
        &execution.report().llvm_sdk.as_ref().unwrap().receipt_sha256
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
        (
            "binding_sha256",
            json!(sha256_bytes(b"substituted LLVM binding").to_string()),
            "record differs from its LLVM SDK binding",
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
    assert_rehashed_record_rejected(readback, &unknown, "not closed LLVM JSON");
    let canonical_record = crate::canonical::bytes(&record).unwrap();
    let duplicate_record = String::from_utf8(canonical_record)
        .unwrap()
        .replacen(
            "\"schema\":",
            "\"schema\":\"aros-llvm-sdk-generation-record-v1\",\"schema\":",
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
        "not closed LLVM JSON",
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
    wrong_family["schema"] = json!("aros-sdk-generation-record-v1");
    assert_rehashed_record_rejected(
        readback,
        &wrong_family,
        "schema or assurance differs from selected LLVM policy",
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
    let wrong_profile = unrelated_profile(
        fixture
            .request
            .compatibility
            .upstream_source_commit
            .as_str(),
    );
    let wrong_profile_compatibility = crate::compatibility::PortableNativeCompatibilityRequest {
        expected: crate::compatibility::NativeCompatibilityReceiptExpectations {
            profile: &wrong_profile,
            ..compatibility.expected.clone()
        },
        ..compatibility.clone()
    };
    assert_generation_rejected(
        ReadbackSelection {
            compatibility: &wrong_profile_compatibility,
            ..readback
        },
        "preset is not present in the recipe-selected profiles",
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

    let alternate_compiler = ArosCompilerIdentity::Llvm {
        version: "18.2.0".into(),
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
    missing_evidence_files
        .remove("llvm-sdk-original-c.elf")
        .unwrap();
    assert_portable_inventory_mutation_rejected(
        readback,
        &missing_evidence_files,
        (downloads.path(), "missing-evidence"),
        "differs from its exact inventory",
    );
    let mut extra_evidence_files = evidence_files;
    extra_evidence_files.insert("unexpected-member.bin".into(), b"extra".to_vec());
    assert_portable_inventory_mutation_rejected(
        readback,
        &extra_evidence_files,
        (downloads.path(), "extra-evidence"),
        "directory differs from its exact inventory",
    );
}

#[test]
fn complete_llvm_sdk_generation_matrix_joins_three_synthetic_host_rows() {
    const SDK_RELEASE_ID: &str = "v1.2.3";
    const COMPILER_GROUP: &str = "llvm";

    let source_lock = llvm_source_lock_bytes();
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
        // The host labels exercise indexed rows in one process. They do not
        // claim execution on Linux ARM64, Linux x86-64 or macOS ARM64 hosts.
        let mut fixture = generation_fixture_for_host(host);
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

        let profiles = llvm_profiles_bytes(
            fixture
                .request
                .compatibility
                .upstream_source_commit
                .as_str(),
        );
        let parsed_profiles = Profiles::parse(&profiles)
            .expect("shared upstream commit should parse into the selected profiles");
        let compiler_profile = parsed_profiles
            .select("pc-x86_64")
            .expect("matrix generation selects pc-x86_64")
            .clone();
        fixture.request.profiles = parsed_profiles.clone();
        fixture.compatibility.profiles = parsed_profiles;
        fixture.request.compatibility.profile = compiler_profile.clone();
        fixture.compatibility.request.profile = compiler_profile;
        align_package_source_with_release_profiles(&mut fixture, &source_lock, &profiles);
        let recipe =
            generation_recipe_bytes(&fixture.request.compatibility, &source_lock, &profiles);

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
        let left = guarded_llvm_package_from_payload_for_host(
            host,
            &recipe,
            &source_lock,
            &profiles,
            "pc-x86_64",
            &payload_root,
            SDK_RELEASE_ID,
        );
        let right = guarded_llvm_package_from_payload_for_host(
            host,
            &recipe,
            &source_lock,
            &profiles,
            "pc-x86_64",
            &payload_root,
            SDK_RELEASE_ID,
        );
        let left_extracted = crate::package_extract::verify_and_extract_with_format(
            &crate::package_extract::PackageExtractionRequest {
                verification: left.verification().clone(),
                output_root: fixture
                    .execution_root
                    .join(format!("compiler-extracted-left-{host}")),
            },
            crate::package::PackageFormat::CompilerFamilyV2,
        )
        .expect("left LLVM compiler package should reverify and extract its exact archive");
        let right_extracted = crate::package_extract::verify_and_extract_with_format(
            &crate::package_extract::PackageExtractionRequest {
                verification: right.verification().clone(),
                output_root: fixture
                    .execution_root
                    .join(format!("compiler-extracted-right-{host}")),
            },
            crate::package::PackageFormat::CompilerFamilyV2,
        )
        .expect("right LLVM compiler package should reverify and extract its exact archive");
        let compiler_package = left_extracted.verified.clone();
        assert_eq!(compiler_package, right_extracted.verified);
        assert_eq!(compiler_package, *right.verified_package());
        assert_eq!(compiler_package.manifest.host, host);
        let compiler_source_commit =
            GitObjectId::try_from(compiler_package.manifest.source_commit.clone()).unwrap();

        // Route actual extracted V2 package roots into native compatibility;
        // retained package expectations are not edited after execution.
        fixture.compatibility.request = fixture.request.compatibility.clone();
        fixture.compatibility.request.relocation = TwoRootRelocation {
            first: ExtractedPackage {
                root: left_extracted.root,
                verified: compiler_package.clone(),
            },
            second: ExtractedPackage {
                root: right_extracted.root,
                verified: compiler_package.clone(),
            },
        };
        fixture.request.compatibility = fixture.compatibility.request.clone();

        let (inputs, collection, documents) =
            release_input_material_for_matrix_generation(&fixture.request, &source_lock, &profiles);
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
                .expect("each synthetic host row should execute LLVM generation and export");
        let record_bytes = fs::read(completed.record_path()).unwrap();
        let record_sha256 = completed.record_sha256().clone();
        let sdk_tree_sha256 = completed.tree_sha256().clone();
        let sdk_package = sdk_generation::package_generated(
            &completed,
            &fixture.package_output,
            &[PathBuf::from("/sdk-generation-matrix-forbidden-prefix")],
        )
        .expect("the exact generated LLVM SDK should package for each host row");
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
        rows.push(GeneratedLlvmMatrixLane {
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

    let prepared = prepare_llvm_matrix_release(
        canonical_collection.unwrap(),
        canonical_documents.unwrap(),
        sdk_identity.unwrap(),
        package_pairs,
        &rows[0].sdk_package,
        &rows[0].sdk_tree_sha256,
    );
    with_llvm_matrix_complete_request(&prepared, |complete| {
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
                .expect("LLVM package identity is present");
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
                .expectations_with_llvm_sdk(
                    expected_package,
                    &row.fixture.request.profiles,
                    &row.fixture.request.compatibility.profile,
                    row.fixture.request.source.identity(),
                )
                .expect("exported expectations bind the exact host compiler and SDK source");
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
            owned_selections.push(LlvmMatrixSelection {
                lane,
                package,
                compatibility,
                record_bytes: row.record_bytes.clone(),
                record_sha256: row.record_sha256.clone(),
            });
        }

        let generations = llvm_matrix_generation_requests(inputs, &owned_selections);
        let matrix = readback_sdk_generation_matrix(&SdkGenerationMatrixReadbackRequest {
            complete,
            generations,
        })
        .expect("complete three-host LLVM generation rows should join the indexed release");
        assert_eq!(matrix.generations().len(), 3);
        for indexed in matrix.generations() {
            assert_eq!(
                indexed.lane().sdk_id(),
                rows[0].fixture.request.source.identity().sdk_id.as_str()
            );
            assert_eq!(indexed.lane().group_id(), COMPILER_GROUP);
            assert_eq!(indexed.lane().profile(), "pc-x86_64");
            assert_eq!(
                indexed.generation().package().archive_sha256,
                rows.iter()
                    .find(|row| row.host == indexed.lane().host())
                    .unwrap()
                    .sdk_package
                    .archive_sha256
            );
        }

        let mut missing_host = llvm_matrix_generation_requests(inputs, &owned_selections);
        let missing_lane = missing_host.keys().next().unwrap().clone();
        missing_host.remove(&missing_lane);
        assert_llvm_matrix_rejected(
            complete,
            missing_host,
            "SDK generation matrix lacks exact input-derived host/profile coverage",
        );

        let mut changed_record = owned_selections.clone();
        let record = &mut changed_record[0];
        let mut record_value: Value = serde_json::from_slice(&record.record_bytes).unwrap();
        record_value["environment_sha256"] = json!(sha256_bytes(b"wrong LLVM SDK environment"));
        record.record_bytes = crate::canonical::bytes(&record_value).unwrap();
        record.record_sha256 = sha256_bytes(&record.record_bytes);
        assert_llvm_matrix_rejected(
            complete,
            llvm_matrix_generation_requests(inputs, &changed_record),
            "SDK generation record differs from its independent source/environment selection",
        );
    });
}

struct GeneratedLlvmMatrixLane {
    host: String,
    fixture: GenerationFixture,
    compiler_package: VerifiedPackage,
    compiler_source_commit: GitObjectId,
    record_bytes: Vec<u8>,
    record_sha256: Sha256Digest,
    sdk_package: crate::sdk_package::SdkPackageOutput,
    sdk_tree_sha256: Sha256Digest,
    execution: crate::compatibility::NativeCompatibilityExecutionExport,
    compatibility_directory: PathBuf,
}

#[derive(Clone)]
struct LlvmMatrixSelection<'a> {
    lane: ExpectedSdkGenerationLane,
    package: crate::sdk_package::SdkVerificationRequest,
    compatibility: PortableNativeCompatibilityRequest<'a>,
    record_bytes: Vec<u8>,
    record_sha256: Sha256Digest,
}

fn llvm_matrix_generation_requests<'a>(
    inputs: &'a crate::release_inputs::ReleaseInputs,
    selections: &'a [LlvmMatrixSelection<'a>],
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

fn assert_llvm_matrix_rejected<'e>(
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
    .expect_err("mutated LLVM matrix selection must be rejected");
    assert!(
        error.to_string().contains(expected_message),
        "unexpected LLVM matrix rejection: {error}"
    );
}

fn generation_fixture() -> GenerationFixture {
    generation_fixture_for_host("linux-x86_64")
}

fn generation_fixture_for_host(host: &'static str) -> GenerationFixture {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let execution_root = root.join("execution");
    fs::create_dir(&execution_root).unwrap();
    let compatibility =
        llvm_sdk_tests::fixture_for_host(&execution_root, BindingMode::Valid, "x86_64", 62, host);
    let source = compatibility
        .request
        .llvm_sdk_source
        .as_ref()
        .expect("synthetic LLVM fixture should select its generic SDK source")
        .clone();
    let record_root = root.join("generation-records");
    let package_root = root.join("packages");
    fs::create_dir(&record_root).unwrap();
    fs::create_dir(&package_root).unwrap();
    let request = SdkGenerationRequest {
        source,
        compatibility: compatibility.request.clone(),
        profiles: compatibility.profiles.clone(),
        record_path: record_root.join("sdk-generation.record.json"),
    };
    GenerationFixture {
        _temporary: temporary,
        compatibility,
        request,
        execution_root,
        package_output: package_root.join("sdk"),
    }
}

fn bind_generation_fixture_to_shared_source(
    fixture: &mut GenerationFixture,
    source: &VerifiedSdkSource,
) {
    llvm_sdk_tests::bind_fixture_to_shared_source(&mut fixture.compatibility, source);
    fixture.request.source = source.clone();
    fixture.request.compatibility = fixture.compatibility.request.clone();
}

fn bind_generation_fixture_to_shared_upstream_source(
    fixture: &mut GenerationFixture,
    upstream_root: &Path,
    upstream_commit: &GitObjectId,
    ports_sources: &crate::compatibility_ports::CompatibilityPortsSources,
) {
    fixture.compatibility.request.upstream_source_root = upstream_root.to_path_buf();
    fixture.compatibility.request.upstream_source_commit = upstream_commit.clone();
    fixture.compatibility.request.ports_sources = ports_sources.clone();
    fixture.request.compatibility = fixture.compatibility.request.clone();
}

fn align_package_source_with_release_profiles(
    fixture: &mut GenerationFixture,
    source_lock: &[u8],
    profiles: &[u8],
) {
    let source_commit = fixture.request.compatibility.upstream_source_commit.clone();
    let mut request = fixture.request.compatibility.clone();
    request.package_source_commit = Some(source_commit);
    let recipe = Recipe::parse(&generation_recipe_bytes(&request, source_lock, profiles)).unwrap();
    let mut first = request.relocation.first.verified.clone();
    let mut second = request.relocation.second.verified.clone();
    for (root, verified) in [
        (&request.relocation.first.root, &mut first),
        (&request.relocation.second.root, &mut second),
    ] {
        let (tree_sha256, files) = aros_common::toolchain_tree_inventory(root).unwrap();
        verified.manifest.tree_sha256 = tree_sha256;
        verified.manifest.files = files;
        verified.manifest.recipe_sha256 = recipe.sha256().to_string();
        verified.manifest.source_lock_sha256 = sha256_bytes(source_lock).to_string();
        verified.manifest.profiles_sha256 = sha256_bytes(profiles).to_string();
        verified.manifest.source_commit = request
            .package_source_commit
            .as_ref()
            .unwrap()
            .as_str()
            .to_owned();
        verified.manifest.producer_commit = recipe.producer().0.as_str().to_owned();
        verified.manifest.tools_commit = recipe.tools().0.as_str().to_owned();
        verified.manifest.source_date_epoch = recipe.source_date_epoch();
        verified.manifest.validate().unwrap();
        fs::write(
            root.join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE),
            serde_json::to_vec(&verified.manifest).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(first.manifest, second.manifest);
    request.relocation.first.verified = first;
    request.relocation.second.verified = second;
    fixture.request.compatibility = request;
}

fn release_inputs_for_generation(
    request: &SdkGenerationRequest,
    source_lock: &[u8],
    profiles: &[u8],
) -> crate::release_inputs::ReleaseInputs {
    release_input_material_for_generation(request, source_lock, profiles, true).0
}

fn release_input_material_for_matrix_generation(
    request: &SdkGenerationRequest,
    source_lock: &[u8],
    profiles: &[u8],
) -> (
    crate::release_inputs::ReleaseInputs,
    Vec<u8>,
    BTreeMap<String, Vec<u8>>,
) {
    release_input_material_for_generation(request, source_lock, profiles, false)
}

fn release_input_material_for_generation(
    request: &SdkGenerationRequest,
    source_lock: &[u8],
    profiles: &[u8],
    include_alternate_sdk: bool,
) -> (
    crate::release_inputs::ReleaseInputs,
    Vec<u8>,
    BTreeMap<String, Vec<u8>>,
) {
    let profile_matrix = Profiles::parse(profiles).unwrap();
    let package_commit = request
        .compatibility
        .package_source_commit
        .as_ref()
        .expect("fixture selects a package source commit");
    assert_eq!(profile_matrix.upstream_commit(), package_commit);
    assert_eq!(
        sha256_bytes(profiles),
        *request.compatibility.profile.document_sha256(),
        "release inputs must bind the exact LLVM fixture profiles bytes"
    );
    let recipe_bytes = generation_recipe_bytes(&request.compatibility, source_lock, profiles);
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
        "group_id": "llvm",
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
    sdks.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    let collection = crate::canonical::bytes(&json!({
        "schema": "aros-toolchain-release-inputs-v2",
        "producer_commit": "cccccccccccccccccccccccccccccccccccccccc",
        "tools_commit": "dddddddddddddddddddddddddddddddddddddddd",
        "hosts": crate::release_inputs::ACTIVE_HOSTS,
        "groups": [{
            "id": "llvm",
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
    request: &NativeCompatibilityRequest,
    source_lock: &[u8],
    profiles: &[u8],
) -> Vec<u8> {
    let package_commit = request
        .package_source_commit
        .as_ref()
        .expect("fixture selects a package source commit");
    let profile_matrix = Profiles::parse(profiles).unwrap();
    assert_eq!(profile_matrix.upstream_commit(), package_commit);
    let source_tree = git_text(&request.upstream_source_root, &["rev-parse", "HEAD:"]);
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

fn llvm_source_lock_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-source-lock-v2",
        "family": "llvm",
        "version": "18.1.0",
        "sources": [{
            "component": "llvm",
            "version": "18.1.0",
            "purpose": "toolchain-component",
            "filename": "llvm-18.1.0.tar.xz",
            "url": "https://example.invalid/llvm-18.1.0.tar.xz",
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

fn llvm_profiles_bytes(upstream_commit: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v1",
        "upstream_commit": upstream_commit,
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

fn unrelated_profile(upstream_commit: &str) -> Profile {
    let mut document: Value =
        serde_json::from_slice(&llvm_profiles_bytes(upstream_commit)).unwrap();
    document["profiles"][0]["name"] = json!("unselected-profile");
    Profiles::parse(&crate::canonical::bytes(&document).unwrap())
        .unwrap()
        .select("unselected-profile")
        .unwrap()
        .clone()
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

fn assert_no_execution_outputs(fixture: &GenerationFixture) {
    assert!(!fixture.compatibility.cmake_log.exists());
    for root in [
        &fixture.request.compatibility.cmake_build_root,
        &fixture.request.compatibility.upstream_build_root,
        &fixture.request.compatibility.standalone_output_root,
        &fixture.request.compatibility.reports_root,
    ] {
        assert!(!root.exists(), "preflight created {}", root.display());
    }
}

fn append_byte(path: &Path) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"\nmutation")
        .unwrap();
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?} failed in {}",
        root.display()
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
