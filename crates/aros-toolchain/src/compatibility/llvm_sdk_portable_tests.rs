//! Synthetic rootless portable proof coverage for LLVM SDK compatibility.
//!
//! The compiler drivers, source repository, CMake consumer binding and SDK
//! payload in this module are fixtures. These checks establish local
//! consistency only; they are not real compiler or authenticated-job evidence.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, CancellationToken, Sha256Digest};
use serde_json::{json, Value};

use super::super::llvm_sdk_portable::{self, LlvmSdkLinkSelection, LlvmSdkPortableExpectations};
use super::llvm_sdk_tests::{self, BindingMode, Fixture};
use super::{execute_native_compatibility, execute_native_compatibility_with_export};
use crate::compatibility::{
    readback_portable_native_compatibility, CompatibilityHelperReport, CompatibilityPreparation,
    LlvmSdkGeneratedRootRequest, NativeCompatibilityExpectedPackage,
    NativeCompatibilityInputClaims, PortableNativeCompatibilityRequest,
};

const BINDING_MEMBER: &str = "llvm-sdk-aros-native-consumer-binding.json";
const COMPILER_INVENTORY_MEMBER: &str = "llvm-sdk-compiler-inventory.json";

struct SelectedFixture {
    _temporary: tempfile::TempDir,
    original_root: PathBuf,
    relocated_root: PathBuf,
    fixture: Fixture,
    selection: LlvmSdkLinkSelection,
    proof_output: PathBuf,
}

#[test]
fn portable_llvm_sdk_proof_round_trips_from_bytes_after_original_root_is_moved() {
    let selected = selected_fixture();
    let owning = execute_native_compatibility_with_export(
        &selected.fixture.request,
        &selected.fixture.profiles,
        &CancellationToken::default(),
    )
    .unwrap_or_else(|error| panic_execution(&error, &selected.fixture));
    assert_eq!(owning.report().probes.reports.len(), 6);
    let local = owning
        .report()
        .llvm_sdk
        .as_ref()
        .expect("local LLVM SDK report");
    let selected_identity = selected
        .fixture
        .request
        .llvm_sdk_source
        .as_ref()
        .unwrap()
        .identity()
        .clone();
    let compiler = selected
        .fixture
        .request
        .relocation
        .first
        .verified
        .manifest
        .compiler_identity()
        .unwrap();
    let package = NativeCompatibilityExpectedPackage {
        manifest: &selected.fixture.request.relocation.first.verified.manifest,
        archive_sha256: &selected
            .fixture
            .request
            .relocation
            .first
            .verified
            .archive_sha256,
        archive_size: selected
            .fixture
            .request
            .relocation
            .first
            .verified
            .archive_size,
        compiler: &compiler,
        source_commit: selected
            .fixture
            .request
            .package_source_commit
            .as_ref()
            .unwrap(),
        host: &selected.fixture.request.host,
    };
    assert!(
        owning
            .inputs()
            .expectations(
                package.clone(),
                &selected.fixture.profiles,
                &selected.fixture.request.profile,
                false,
            )
            .is_err(),
        "legacy expectations omitted the selected LLVM SDK requirement"
    );
    let expected = owning
        .inputs()
        .expectations_with_llvm_sdk(
            package.clone(),
            &selected.fixture.profiles,
            &selected.fixture.request.profile,
            &selected_identity,
        )
        .unwrap();

    // Keep the target triple fixed while substituting only the source profile's
    // independent toolchain_profile mapping. Reseal canonical bytes and their
    // raw digest; parsing must reject the profile-mapping disagreement itself.
    let mut substituted_input: Value = serde_json::from_slice(owning.inputs().bytes()).unwrap();
    substituted_input["llvm_sdk"]["toolchain_profile"] = json!("other-compiler-profile");
    assert_eq!(
        substituted_input["target_triple"],
        json!(selected.fixture.request.profile.target_triple())
    );
    let substituted_bytes = crate::canonical::bytes(&substituted_input).unwrap();
    let substituted_sha256 = sha256_bytes(&substituted_bytes);
    let error =
        NativeCompatibilityInputClaims::parse(&substituted_bytes, &substituted_sha256).unwrap_err();
    assert!(
        error.to_string().contains(
            "LLVM SDK input observation differs from its source/compiler build selection"
        ),
        "toolchain_profile-only mutation did not reach the parser mapping check: {error}"
    );

    // Reseal both serialized profile labels to the same foreign name. This
    // passes internal input parsing but must still disagree with the separately
    // selected compiler/profile expected by the owning consumer.
    substituted_input["target_profile"] = json!("other-compiler-profile");
    let internally_consistent_bytes = crate::canonical::bytes(&substituted_input).unwrap();
    let internally_consistent_sha256 = sha256_bytes(&internally_consistent_bytes);
    let internally_consistent_claims = NativeCompatibilityInputClaims::parse(
        &internally_consistent_bytes,
        &internally_consistent_sha256,
    )
    .unwrap();
    let error = internally_consistent_claims
        .expectations_with_llvm_sdk(
            package,
            &selected.fixture.profiles,
            &selected.fixture.request.profile,
            &selected_identity,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("LLVM SDK source profile differs from the independent compiler profile"),
        "foreign source/profile mapping was rejected for an unrelated reason: {error}"
    );

    let download = tempfile::tempdir().unwrap();
    let evidence_root = download.path().join("evidence");
    fs::create_dir(&evidence_root).unwrap();
    for (name, bytes) in owning.evidence().files() {
        fs::write(evidence_root.join(name), bytes).unwrap();
    }
    let portable_request = PortableNativeCompatibilityRequest {
        directory: evidence_root.canonicalize().unwrap(),
        manifest_sha256: owning.evidence().manifest_sha256().clone(),
        expected,
    };

    fs::rename(&selected.original_root, &selected.relocated_root).unwrap();
    assert!(
        !selected.original_root.exists(),
        "original fixture root remains"
    );

    let portable = readback_portable_native_compatibility(&portable_request).unwrap();
    let portable_llvm = portable.llvm_sdk().expect("rootless LLVM SDK proof");
    assert_eq!(portable_llvm.identity(), &local.identity);
    assert_eq!(portable_llvm.binding_sha256(), &local.binding_sha256);
    assert_eq!(portable_llvm.receipt_sha256(), &local.receipt_sha256);
    assert_eq!(
        portable_llvm.inventory_sha256(),
        &local.sdk_inventory_sha256
    );
    assert_eq!(portable_llvm.entries(), local.sdk_entries);
}

#[test]
fn portable_llvm_sdk_proof_rejects_incomplete_and_unexpected_member_sets() {
    let selected = selected_fixture();
    let _execution =
        execute_native_compatibility(&selected.fixture.request, &CancellationToken::default())
            .unwrap_or_else(|error| panic_execution(&error, &selected.fixture));
    let files = gather_proof_files(
        &selected.fixture,
        &selected.selection,
        &selected.proof_output,
    );
    let expectations = OwnedExpectations::new(&selected.fixture.request.preparation);
    let baseline = readback_owned(&files, &selected.selection, &expectations).unwrap();
    assert_eq!(
        baseline.identity(),
        selected
            .fixture
            .request
            .llvm_sdk_source
            .as_ref()
            .unwrap()
            .identity()
    );

    let mut missing = files.clone();
    let removed = missing.pop_last().expect("nonempty proof").0;
    assert_readback_rejected(
        readback_owned(&missing, &selected.selection, &expectations),
        &format!("missing member {removed}"),
    );

    let mut extra = files;
    extra.insert(
        "unexpected-member.bin".into(),
        b"unselected bytes\n".to_vec(),
    );
    assert_readback_rejected(
        readback_owned(&extra, &selected.selection, &expectations),
        "unexpected member",
    );
}

#[test]
fn portable_llvm_sdk_proof_rejects_single_byte_changes_to_each_evidence_class() {
    let selected = selected_fixture();
    let _execution =
        execute_native_compatibility(&selected.fixture.request, &CancellationToken::default())
            .unwrap_or_else(|error| panic_execution(&error, &selected.fixture));
    let files = gather_proof_files(
        &selected.fixture,
        &selected.selection,
        &selected.proof_output,
    );
    let expectations = OwnedExpectations::new(&selected.fixture.request.preparation);

    assert_eq!(files.len(), 22, "closed portable proof inventory");
    for name in files.keys() {
        let mut changed = files.clone();
        let bytes = changed.get_mut(name).unwrap();
        if bytes.is_empty() {
            // Successful compiler stderr may be empty, but is still sealed.
            bytes.push(b'x');
        } else {
            bytes[0] ^= 1;
        }
        assert_readback_rejected(
            readback_owned(&changed, &selected.selection, &expectations),
            &format!("single-byte mutation in {name}"),
        );
    }
}

#[test]
fn portable_llvm_sdk_proof_rejects_noncanonical_and_duplicate_binding_json() {
    let selected = selected_fixture();
    let _execution =
        execute_native_compatibility(&selected.fixture.request, &CancellationToken::default())
            .unwrap_or_else(|error| panic_execution(&error, &selected.fixture));
    let files = gather_proof_files(
        &selected.fixture,
        &selected.selection,
        &selected.proof_output,
    );
    let expectations = OwnedExpectations::new(&selected.fixture.request.preparation);

    let mut noncanonical = files.clone();
    noncanonical
        .get_mut(BINDING_MEMBER)
        .expect("binding member")
        .push(b' ');
    assert_readback_rejected(
        readback_owned(&noncanonical, &selected.selection, &expectations),
        "noncanonical binding JSON",
    );

    let mut duplicate = files;
    let binding = std::str::from_utf8(duplicate.get(BINDING_MEMBER).unwrap()).unwrap();
    let duplicate_json = binding.replacen(
        "\"schema\":",
        "\"schema\":\"aros-sdk-consumer-validation-v1\",\"schema\":",
        1,
    );
    assert_ne!(duplicate_json, binding, "binding schema key was not found");
    duplicate.insert(BINDING_MEMBER.into(), duplicate_json.into_bytes());
    assert_readback_rejected(
        readback_owned(&duplicate, &selected.selection, &expectations),
        "duplicate binding JSON key",
    );
}

#[test]
fn portable_llvm_sdk_proof_rejects_a_different_independent_program_selection() {
    let selected = selected_fixture();
    let _execution =
        execute_native_compatibility(&selected.fixture.request, &CancellationToken::default())
            .unwrap_or_else(|error| panic_execution(&error, &selected.fixture));
    let files = gather_proof_files(
        &selected.fixture,
        &selected.selection,
        &selected.proof_output,
    );
    let other = selected_fixture();
    let other_selection = select_independently(&other.fixture);
    let expectations = OwnedExpectations::new(&selected.fixture.request.preparation);

    // Both selectors use the same SDK identity and fixture target, but point at
    // independently measured compiler/program roots. The source proof cannot
    // be relabeled with the other selection.
    assert_ne!(
        serde_json::to_value(&selected.selection).unwrap(),
        serde_json::to_value(&other_selection).unwrap()
    );
    assert_readback_rejected(
        readback_owned(&files, &other_selection, &expectations),
        "different independent program selection",
    );
}

#[test]
fn portable_llvm_sdk_proof_rejects_a_resealed_forged_command_report() {
    let selected = selected_fixture();
    let _execution =
        execute_native_compatibility(&selected.fixture.request, &CancellationToken::default())
            .unwrap_or_else(|error| panic_execution(&error, &selected.fixture));
    let mut files = gather_proof_files(
        &selected.fixture,
        &selected.selection,
        &selected.proof_output,
    );
    let expectations = OwnedExpectations::new(&selected.fixture.request.preparation);

    let report_name = "llvm-sdk-standalone-c.report.json";
    let receipt_name = files
        .keys()
        .find(|name| name.contains("receipt"))
        .expect("LLVM SDK link receipt")
        .clone();
    let mut report: Value = serde_json::from_slice(files.get(report_name).unwrap()).unwrap();
    report["commands"][0]["command_sha256"] =
        json!(sha256_bytes(b"forged selected command").as_str());
    let report_bytes = crate::canonical::bytes(&report).unwrap();
    files.insert(report_name.into(), report_bytes.clone());

    // Reseal the changed report into the proof receipt to bypass the ordinary
    // member-digest check and exercise independent command reconstruction.
    let mut receipt: Value = serde_json::from_slice(files.get(&receipt_name).unwrap()).unwrap();
    receipt["members"]["links/reports/standalone-c.report.json"]["sha256"] =
        json!(sha256_bytes(&report_bytes).as_str());
    receipt["members"]["links/reports/standalone-c.report.json"]["size"] =
        json!(report_bytes.len() as u64);
    files.insert(receipt_name, crate::canonical::bytes(&receipt).unwrap());

    let error = readback_owned(&files, &selected.selection, &expectations).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("LLVM SDK retained invocation differs from its selected command"),
        "resealed forged command did not reach independent command validation: {error}"
    );
}

#[derive(Clone)]
struct OwnedExpectations {
    engine_api_version: u32,
    engine_sha256: Sha256Digest,
    helpers: BTreeMap<String, CompatibilityHelperReport>,
}

impl OwnedExpectations {
    fn new(preparation: &CompatibilityPreparation) -> Self {
        Self {
            engine_api_version: preparation.engine_api_version,
            engine_sha256: preparation.engine_sha256.clone(),
            helpers: helper_reports(preparation),
        }
    }

    fn request(&self) -> LlvmSdkPortableExpectations<'_> {
        LlvmSdkPortableExpectations {
            engine_api_version: self.engine_api_version,
            engine_sha256: &self.engine_sha256,
            helpers: &self.helpers,
        }
    }
}

fn selected_fixture() -> SelectedFixture {
    let temporary = tempfile::tempdir().unwrap();
    let original_root = temporary.path().join("original-root");
    let relocated_root = temporary.path().join("renamed-original-root");
    fs::create_dir(&original_root).unwrap();
    let fixture = llvm_sdk_tests::fixture(&original_root, BindingMode::Valid, "x86_64", 62);
    let selection = select_independently(&fixture);
    let proof_output = fixture.request.reports_root.join("llvm-sdk-links");
    SelectedFixture {
        _temporary: temporary,
        original_root,
        relocated_root,
        fixture,
        selection,
        proof_output,
    }
}

fn select_independently(fixture: &Fixture) -> LlvmSdkLinkSelection {
    let cancellation = CancellationToken::default();
    let compiler = fixture
        .request
        .relocation
        .first
        .verified
        .manifest
        .compiler_identity()
        .unwrap();
    super::llvm_sdk::select(&fixture.request, &compiler)
        .unwrap()
        .expect("selected generic LLVM SDK source");
    let drivers = super::llvm_sdk::drivers(&fixture.request, &compiler, &cancellation)
        .unwrap()
        .expect("measured LLVM SDK drivers");
    let generated = LlvmSdkGeneratedRootRequest {
        source: fixture.request.llvm_sdk_source.as_ref().unwrap().clone(),
        preparation: fixture.request.preparation.clone(),
        cmake_build_root: fixture.request.cmake_build_root.clone(),
        timeout: fixture.request.timeout,
    };
    llvm_sdk_portable::select(
        &generated,
        &drivers,
        &fixture.request.reports_root.join("llvm-sdk-links"),
        &cancellation,
    )
    .unwrap()
}

fn gather_proof_files(
    fixture: &Fixture,
    selection: &LlvmSdkLinkSelection,
    proof_output: &Path,
) -> BTreeMap<String, Vec<u8>> {
    let mut members = BTreeMap::new();
    for (flat_name, (relative, limit)) in llvm_sdk_portable::expected_members() {
        let bytes = match flat_name.as_str() {
            BINDING_MEMBER => fs::read(&fixture.binding_path).unwrap(),
            COMPILER_INVENTORY_MEMBER => selection.compiler_inventory_bytes().to_vec(),
            _ => {
                let bytes = fs::read(proof_output.join(&relative))
                    .unwrap_or_else(|error| panic!("read {flat_name} ({relative}): {error}"));
                assert!(bytes.len() <= limit, "oversize member {flat_name}");
                bytes
            }
        };
        assert!(members.insert(flat_name.clone(), bytes).is_none());
    }
    members
}

fn helper_reports(
    preparation: &CompatibilityPreparation,
) -> BTreeMap<String, CompatibilityHelperReport> {
    preparation
        .helpers
        .iter()
        .map(|(name, helper)| {
            (
                name.clone(),
                CompatibilityHelperReport {
                    sha256: helper.sha256.clone(),
                    size: helper.size,
                },
            )
        })
        .collect()
}

fn readback_owned(
    files: &BTreeMap<String, Vec<u8>>,
    selection: &LlvmSdkLinkSelection,
    expectations: &OwnedExpectations,
) -> Result<super::super::llvm_sdk_portable::LlvmSdkPortableProof, crate::ContractError> {
    llvm_sdk_portable::readback(files, selection, &expectations.request())
}

fn assert_readback_rejected(
    result: Result<super::super::llvm_sdk_portable::LlvmSdkPortableProof, crate::ContractError>,
    case: &str,
) {
    result.expect_err(&format!("portable readback accepted {case}"));
}

fn panic_execution(error: &crate::ContractError, fixture: &Fixture) -> ! {
    panic!(
        "synthetic LLVM SDK execution failed: {error}\nCMake arguments:\n{}\nconfigure stdout:\n{}\nconfigure stderr:\n{}",
        retained_text(&fixture.cmake_log),
        retained_text(
            &fixture
                .request
                .reports_root
                .join("cmake-consumer.1.stdout.log")
        ),
        retained_text(
            &fixture
                .request
                .reports_root
                .join("cmake-consumer.1.stderr.log")
        ),
    )
}

fn retained_text(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| format!("<unavailable: {error}>"))
        .trim()
        .to_owned()
}
