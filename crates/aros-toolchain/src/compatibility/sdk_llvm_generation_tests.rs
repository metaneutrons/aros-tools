//! Synthetic source-bound LLVM SDK generation and packaging coverage.
//!
//! The committed source, generated SDK bytes and compiler drivers are fixtures.
//! They exercise retained identity and digest wiring, not a real LLVM build.

#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, CancellationToken};
use serde_json::Value;

use super::llvm_sdk_tests::{self, BindingMode, Fixture as CompatibilityFixture};
use crate::sdk_generation::{self, SdkGenerationRequest};

struct GenerationFixture {
    _temporary: tempfile::TempDir,
    compatibility: CompatibilityFixture,
    request: SdkGenerationRequest,
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

fn generation_fixture() -> GenerationFixture {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let compatibility = llvm_sdk_tests::fixture(root, BindingMode::Valid, "x86_64", 62);
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
        package_output: package_root.join("sdk"),
    }
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
