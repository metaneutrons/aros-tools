//! Synthetic regression coverage for the GNU six-phase compatibility adapter.
//!
//! The declared drivers copy generated RISC-V ELF objects. They exercise
//! command selection and ABI rejection, not compiler-runtime behavior.

#![cfg(unix)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use aros_common::elf::{self, riscv::TargetContract, AROS_ABI_VERSION, OS_ABI_AROS};
use aros_common::{
    measure_tree_content_cas, run_status, sha256_bytes, toolchain_tree_inventory,
    ArosCompilerIdentity, ArosToolchainManifest, CancellationToken, Sha256Digest,
};
use serde_json::{json, Value};

use super::{
    cmake_command, execute_native_compatibility, execute_native_compatibility_with_readback,
    helpers_root, validate_inputs, NativeCompatibilityRequest, OutputRoots,
};
use crate::compatibility::{
    run_probe, run_probe_set, CompatibilityCommand, CompatibilityEnvironment, CompatibilityPhase,
    CompatibilityProbeRequest, CompatibilityProbeSetRequest, TwoRootRelocation,
    CXX_COLLECTOR_SYMBOL, C_COLLECTOR_SYMBOL,
};
use crate::package_extract::ExtractedPackage;
use crate::package_verify::VerifiedPackage;
use crate::profiles::{Profile, Profiles};
use crate::recipe::GitObjectId;

const C_ROLE: &str = "drivers/declared-c-entry";
const CXX_ROLE: &str = "drivers/declared-cxx-entry";
const GCC_VERSION: &str = "16.2.0";
const BINUTILS_VERSION: &str = "2.47";
const PACKAGE_SOURCE_COMMIT: &str = "e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7";
const NATIVE_BUILD_CONTRACT_FILE: &str = "native-build-v1.json";
const NATIVE_BUILD_INPUT_FILE: &str = "native-contract-input.src";
const NATIVE_CONSUMER_CONTRACT_FILE: &str = "native-consumer-v1.json";
const NATIVE_CONSUMER_POLICY_FILE: &str = "native-consumer-policy.json";

pub(super) struct Fixture {
    pub(super) request: NativeCompatibilityRequest,
    pub(super) profiles: Profiles,
    cmake_log: PathBuf,
    make_log: PathBuf,
}

struct NativeSdkPortableSelection {
    inputs: super::NativeCompatibilityInputClaims,
    manifest: ArosToolchainManifest,
    archive_sha256: Sha256Digest,
    archive_size: u64,
    compiler: ArosCompilerIdentity,
    package_source_commit: GitObjectId,
    host: String,
    profiles: Profiles,
    profile: Profile,
}

impl NativeSdkPortableSelection {
    fn expected_package(&self) -> super::NativeCompatibilityExpectedPackage<'_> {
        super::NativeCompatibilityExpectedPackage {
            manifest: &self.manifest,
            archive_sha256: &self.archive_sha256,
            archive_size: self.archive_size,
            compiler: &self.compiler,
            source_commit: &self.package_source_commit,
            host: &self.host,
        }
    }

    fn expectations(
        &self,
        native_sdk_required: bool,
    ) -> Result<super::NativeCompatibilityReceiptExpectations<'_>, crate::ContractError> {
        self.inputs.expectations(
            self.expected_package(),
            &self.profiles,
            &self.profile,
            native_sdk_required,
        )
    }
}

#[test]
fn executes_all_gnu_phases_for_rv32_with_declared_driver_roles() {
    let temporary = tempfile::tempdir().unwrap();
    assert_successful_gnu_execution(&gnu_fixture(temporary.path(), 32, false), 32);
}

#[test]
fn executes_all_gnu_phases_for_rv64_with_declared_driver_roles() {
    let temporary = tempfile::tempdir().unwrap();
    assert_successful_gnu_execution(&gnu_fixture(temporary.path(), 64, false), 64);
}

#[test]
fn executes_consumer_v2_with_six_compatibility_phases_and_four_native_sdk_links() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = gnu_native_sdk_fixture(temporary.path(), NativeSdkBindingMode::Valid);
    let report = execute_native_compatibility_with_readback(
        &fixture.request,
        &fixture.profiles,
        &CancellationToken::default(),
    )
    .expect("consumer-v2 GNU compatibility should include ordinary SDK links");

    assert_eq!(report.probes.reports.len(), 6);
    assert!(report.native_sdk.is_some());
    let native_sdk = report.native_sdk.as_ref().unwrap();
    assert_eq!(
        native_sdk.receipt,
        fixture
            .request
            .reports_root
            .join("native-sdk-links/native-sdk-links.receipt.json")
            .canonicalize()
            .unwrap()
    );
    assert_eq!(native_sdk.sdk_entries, 5);
    let receipt: Value = serde_json::from_slice(&fs::read(&report.receipt.path).unwrap()).unwrap();
    assert_eq!(
        receipt["schema"],
        "aros-toolchain-native-compatibility-receipt-v5"
    );
    assert_eq!(receipt["phase_reports"].as_array().unwrap().len(), 6);
    assert_eq!(
        receipt["native_sdk"]["receipt_sha256"],
        native_sdk.receipt_sha256.to_string()
    );
    assert_eq!(
        receipt["native_sdk"]["inventory_sha256"],
        native_sdk.sdk_inventory_sha256.to_string()
    );
    assert_eq!(receipt["native_sdk"]["entries"], native_sdk.sdk_entries);

    let sdk_receipt: Value =
        serde_json::from_slice(&fs::read(&native_sdk.receipt).unwrap()).unwrap();
    assert_eq!(sdk_receipt["outputs"].as_object().unwrap().len(), 4);
    for language in ["c", "cxx"] {
        assert_eq!(
            sdk_receipt["outputs"][format!("original-{language}")]["sha256"],
            sdk_receipt["outputs"][format!("relocated-{language}")]["sha256"]
        );
    }
    for source in ["native-sdk-probe.c", "native-sdk-probe.cpp"] {
        assert!(fixture
            .request
            .preparation
            .source_root
            .join(source)
            .is_file());
    }
}

#[test]
fn consumer_v2_portable_sdk_evidence_roundtrips_rootlessly_and_rejects_mutations() {
    let original_root = tempfile::tempdir().unwrap();
    let original_root_path = original_root.path().to_path_buf();
    let fixture = gnu_native_sdk_fixture(original_root.path(), NativeSdkBindingMode::Valid);
    let execution = super::execute_native_compatibility_with_export(
        &fixture.request,
        &fixture.profiles,
        &CancellationToken::default(),
    )
    .expect("consumer-v2 GNU compatibility export should include ordinary SDK links");

    assert_eq!(execution.report().probes.reports.len(), 6);
    assert!(execution.report().native_sdk.is_some());
    let receipt_sha256 = execution.report().receipt.sha256.clone();
    let input_claims = super::NativeCompatibilityInputClaims::parse(
        execution.inputs().bytes(),
        execution.inputs().sha256(),
    )
    .unwrap();
    let input_document: Value = serde_json::from_slice(execution.inputs().bytes()).unwrap();
    assert_eq!(
        input_document["schema"],
        "aros-toolchain-compatibility-inputs-v2"
    );
    assert!(input_document["native_sdk"].is_object());

    let manifest = fixture.request.relocation.first.verified.manifest.clone();
    let compiler = manifest.compiler_identity().unwrap();
    let selection = NativeSdkPortableSelection {
        inputs: input_claims,
        manifest,
        archive_sha256: fixture
            .request
            .relocation
            .first
            .verified
            .archive_sha256
            .clone(),
        archive_size: fixture.request.relocation.first.verified.archive_size,
        compiler,
        package_source_commit: fixture
            .request
            .package_source_commit
            .as_ref()
            .unwrap()
            .clone(),
        host: fixture.request.host.clone(),
        profiles: fixture.profiles.clone(),
        profile: fixture.request.profile.clone(),
    };
    let expected = selection.expectations(true).unwrap();
    assert!(expected.native_sdk.is_some());

    let mut downgraded_input_document: Value =
        serde_json::from_slice(selection.inputs.bytes()).unwrap();
    downgraded_input_document["schema"] = json!("aros-toolchain-compatibility-inputs-v1");
    downgraded_input_document
        .as_object_mut()
        .unwrap()
        .remove("native_sdk");
    let downgraded_input_bytes = crate::canonical::bytes(&downgraded_input_document).unwrap();
    let downgraded_input_sha256 = sha256_bytes(&downgraded_input_bytes);
    let downgraded_input_claims = super::NativeCompatibilityInputClaims::parse(
        &downgraded_input_bytes,
        &downgraded_input_sha256,
    )
    .unwrap();
    let downgrade_error = downgraded_input_claims
        .expectations(
            selection.expected_package(),
            &selection.profiles,
            &selection.profile,
            true,
        )
        .unwrap_err();
    assert_eq!(
        downgrade_error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility
    );
    assert!(downgrade_error
        .to_string()
        .contains("SDK requirement differs from independent source policy"));

    let files = execution
        .evidence()
        .files()
        .map(|(name, bytes)| (name.to_owned(), bytes.to_vec()))
        .collect::<BTreeMap<_, _>>();
    let manifest_sha256 = execution.evidence().manifest_sha256().clone();
    assert_eq!(
        files.keys().filter(|name| name.starts_with("sdk-")).count(),
        21
    );
    let aggregate: Value =
        serde_json::from_slice(&files["native-compatibility.receipt.json"]).unwrap();
    assert_eq!(
        aggregate["schema"],
        "aros-toolchain-native-compatibility-receipt-v5"
    );

    let download_root = tempfile::tempdir().unwrap();
    let downloaded = download_root.path().join("evidence");
    write_native_sdk_portable_files(&downloaded, &files);
    let downloaded = downloaded.canonicalize().unwrap();
    drop(execution);
    drop(fixture);
    drop(original_root);
    assert!(!original_root_path.exists());

    let readback =
        super::readback_portable_native_compatibility(&super::PortableNativeCompatibilityRequest {
            directory: downloaded,
            manifest_sha256: manifest_sha256.clone(),
            expected,
        })
        .expect("portable SDK export should read without original execution roots");
    assert_eq!(readback.manifest_sha256(), &manifest_sha256);
    assert_eq!(readback.receipt().receipt_sha256, receipt_sha256);
    assert_eq!(readback.receipt().phases.len(), 6);
    assert!(readback.native_sdk().is_some());

    let mut missing_elf = files.clone();
    missing_elf.remove("sdk-original-c.elf");
    assert_native_sdk_portable_rejected(
        &missing_elf,
        &manifest_sha256,
        &selection,
        "omitted SDK ELF",
        "portable compatibility directory differs from its exact inventory",
    );

    let mut changed_elf = files.clone();
    changed_elf.get_mut("sdk-original-c.elf").unwrap()[0] ^= 1;
    assert_native_sdk_portable_rejected(
        &changed_elf,
        &manifest_sha256,
        &selection,
        "changed SDK ELF without resealing",
        "portable compatibility member differs from its selected manifest",
    );

    let mut changed_map = files.clone();
    let map = "sdk-original-c.map";
    changed_map
        .get_mut(map)
        .unwrap()
        .extend_from_slice(b"changed map\n");
    let changed_map_manifest = reseal_native_sdk_portable_member(&mut changed_map, map);
    assert_native_sdk_portable_rejected(
        &changed_map,
        &changed_map_manifest,
        &selection,
        "resealed altered SDK map",
        "portable native SDK receipt differs from its link map",
    );

    let mut changed_log = files.clone();
    let log = "sdk-standalone-c.1.stdout.log";
    changed_log
        .get_mut(log)
        .unwrap()
        .extend_from_slice(b"changed retained linker trace\n");
    let changed_log_manifest = reseal_native_sdk_portable_member(&mut changed_log, log);
    assert_native_sdk_portable_rejected(
        &changed_log,
        &changed_log_manifest,
        &selection,
        "resealed altered SDK log",
        "portable native SDK retained logs differ from their report",
    );

    let mut downgraded_receipt = files;
    let aggregate_name = "native-compatibility.receipt.json";
    let mut aggregate: Value = serde_json::from_slice(&downgraded_receipt[aggregate_name]).unwrap();
    aggregate["schema"] = json!("aros-toolchain-native-compatibility-receipt-v3");
    aggregate.as_object_mut().unwrap().remove("native_sdk");
    downgraded_receipt.insert(
        aggregate_name.into(),
        crate::canonical::bytes(&aggregate).unwrap(),
    );
    let downgraded_manifest =
        reseal_native_sdk_portable_member(&mut downgraded_receipt, aggregate_name);
    assert_native_sdk_portable_rejected(
        &downgraded_receipt,
        &downgraded_manifest,
        &selection,
        "v2 input selection paired with downgraded aggregate receipt",
        "native compatibility receipt schema does not match the expected compiler family",
    );
}

fn write_native_sdk_portable_files(directory: &Path, files: &BTreeMap<String, Vec<u8>>) {
    fs::create_dir(directory).unwrap();
    for (name, bytes) in files {
        assert_eq!(Path::new(name).file_name().unwrap(), name.as_str());
        fs::write(directory.join(name), bytes).unwrap();
    }
}

fn assert_native_sdk_portable_rejected(
    files: &BTreeMap<String, Vec<u8>>,
    manifest_sha256: &Sha256Digest,
    selection: &NativeSdkPortableSelection,
    label: &str,
    expected_message: &str,
) {
    let owner = tempfile::tempdir().unwrap();
    let directory = owner.path().join("downloaded");
    write_native_sdk_portable_files(&directory, files);
    let directory = directory.canonicalize().unwrap();
    let error =
        super::readback_portable_native_compatibility(&super::PortableNativeCompatibilityRequest {
            directory,
            manifest_sha256: manifest_sha256.clone(),
            expected: selection.expectations(true).unwrap(),
        })
        .unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility,
        "{label}: {error}"
    );
    assert!(
        error.to_string().contains(expected_message),
        "{label}: expected {expected_message:?}, got {error}"
    );
}

fn reseal_native_sdk_portable_member(
    files: &mut BTreeMap<String, Vec<u8>>,
    name: &str,
) -> Sha256Digest {
    let bytes = files.get(name).unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&files[super::PORTABLE_NATIVE_COMPATIBILITY_MANIFEST]).unwrap();
    manifest["files"][name]["sha256"] = json!(sha256_bytes(bytes));
    manifest["files"][name]["size"] = json!(bytes.len() as u64);
    let bytes = crate::canonical::bytes(&manifest).unwrap();
    let digest = sha256_bytes(&bytes);
    files.insert(super::PORTABLE_NATIVE_COMPATIBILITY_MANIFEST.into(), bytes);
    digest
}

#[test]
fn missing_v2_native_sdk_binding_blocks_aggregate_receipt() {
    assert_v2_binding_failure_has_no_aggregate_receipt(NativeSdkBindingMode::Missing);
}

#[test]
fn stale_v2_native_sdk_binding_blocks_aggregate_receipt() {
    assert_v2_binding_failure_has_no_aggregate_receipt(NativeSdkBindingMode::Stale);
}

fn assert_v2_binding_failure_has_no_aggregate_receipt(mode: NativeSdkBindingMode) {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = gnu_native_sdk_fixture(temporary.path(), mode);
    let result = execute_native_compatibility(&fixture.request, &CancellationToken::default());
    assert!(result.is_err(), "{mode:?} SDK binding must fail");
    assert!(!fixture
        .request
        .reports_root
        .join("native-compatibility.receipt.json")
        .exists());
    assert!(fixture
        .request
        .cmake_build_root
        .join("SYS/Developer/include/native-sdk-fixture.h")
        .is_file());
    assert!(fixture
        .request
        .reports_root
        .join("standalone-cxx.report.json")
        .is_file());
}

#[test]
fn rejects_missing_or_mismatched_expected_package_source_commit_before_cmake() {
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        fixture.request.package_source_commit = None;
        assert_rejected_before_cmake_with_message(
            &fixture,
            "missing package source commit",
            "family-v2 compatibility requires the recipe-bound package source commit",
        );
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        fixture.request.package_source_commit =
            Some(GitObjectId::try_from("f".repeat(40)).unwrap());
        assert_rejected_before_cmake_with_message(
            &fixture,
            "mismatched package source commit",
            "family-v2 compatibility source/profile contract differs from the package identity",
        );
    }
}

#[test]
fn rejects_profile_compiler_role_and_manifest_mismatches_before_cmake() {
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        fixture.request.profile = fixture.profiles.select("rv32-alternate").unwrap().clone();
        assert_rejected_before_cmake(&fixture);
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        fixture.request.relocation.first.verified.manifest.compiler =
            Some(compiler_for(&fixture.request.profile, "17.1.0"));
        assert_rejected_before_cmake(&fixture);
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        fixture
            .request
            .relocation
            .first
            .verified
            .manifest
            .profiles_sha256 = "e".repeat(64);
        fixture
            .request
            .relocation
            .second
            .verified
            .manifest
            .profiles_sha256 = "e".repeat(64);
        assert_rejected_before_cmake(&fixture);
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        fixture
            .request
            .relocation
            .first
            .verified
            .manifest
            .source_commit = "f".repeat(40);
        fixture
            .request
            .relocation
            .second
            .verified
            .manifest
            .source_commit = "f".repeat(40);
        assert_rejected_before_cmake(&fixture);
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let fixture = gnu_fixture(temporary.path(), 32, false);
        fs::write(
            fixture.request.relocation.second.root.join(C_ROLE),
            b"changed declared compiler role bytes\n",
        )
        .unwrap();
        assert_rejected_before_cmake(&fixture);
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let fixture = gnu_fixture(temporary.path(), 32, false);
        let embedded = fixture
            .request
            .relocation
            .second
            .root
            .join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE);
        let mut manifest: Value = serde_json::from_slice(&fs::read(&embedded).unwrap()).unwrap();
        manifest["source_commit"] = json!("f".repeat(40));
        fs::write(&embedded, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert_rejected_before_cmake(&fixture);
    }
}

#[test]
fn rejects_unbound_gnu_source_presets_before_cmake() {
    let cases = [
        // Selection itself is mandatory, even when the declaration has one
        // otherwise-valid target.
        (
            "missing selected preset",
            None,
            Some("rv32-compat"),
            "riscv32",
            "fixture",
            "gnu",
        ),
        // A selected name must resolve in this measured source declaration.
        (
            "unknown selected preset",
            Some("unknown-source-preset"),
            Some("rv32-compat"),
            "riscv32",
            "fixture",
            "gnu",
        ),
        // The source target is intentionally distinct from the producer
        // compiler profile and must bind it explicitly.
        (
            "mismatched compiler profile",
            Some("source-rv32-preset"),
            Some("not-rv32-compat"),
            "riscv32",
            "fixture",
            "gnu",
        ),
        (
            "missing compiler profile",
            Some("source-rv32-preset"),
            None,
            "riscv32",
            "fixture",
            "gnu",
        ),
        (
            "mismatched source CPU",
            Some("source-rv32-preset"),
            Some("rv32-compat"),
            "riscv64",
            "fixture",
            "gnu",
        ),
        (
            "mismatched source platform",
            Some("source-rv32-preset"),
            Some("rv32-compat"),
            "riscv32",
            "not-fixture",
            "gnu",
        ),
        (
            "mismatched source toolchain",
            Some("source-rv32-preset"),
            Some("rv32-compat"),
            "riscv32",
            "fixture",
            "llvm",
        ),
    ];

    for (label, selected, compiler_profile, arch, platform, toolchain) in cases {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        fixture.request.source_preset = selected.map(str::to_owned);
        let declaration = source_preset_toml(
            "source-rv32-preset",
            compiler_profile,
            arch,
            platform,
            toolchain,
            None,
            None,
        );
        write_source_declaration(&mut fixture.request, &declaration);
        assert_rejected_before_cmake_with_label(&fixture, label);
    }
}

#[test]
fn rejects_source_float_abi_conflicting_with_the_compiler_before_cmake() {
    let temporary = tempfile::tempdir().unwrap();
    let mut fixture = gnu_fixture(temporary.path(), 32, false);
    let declaration = source_preset_toml(
        "source-rv32-preset",
        Some(fixture.request.profile.name()),
        "riscv32",
        fixture.request.profile.platform(),
        "gnu",
        Some("lp64d"),
        None,
    );
    write_source_declaration(&mut fixture.request, &declaration);
    assert_rejected_before_cmake_with_message(
        &fixture,
        "conflicting source float ABI",
        "GNU compatibility source preset differs from the compiler profile",
    );
}

#[test]
fn forwards_bootstrap_abi_and_bound_native_contract_to_cmake() {
    let temporary = tempfile::tempdir().unwrap();
    let mut fixture = gnu_fixture(temporary.path(), 32, false);
    let producer_profile = fixture.request.profile.clone();
    write_native_contract_fixture(&mut fixture.request, &producer_profile);
    assert_successful_gnu_execution(&fixture, 32);
}

#[test]
fn forwards_bound_native_consumer_contract_without_boot_media_bindings() {
    for also_declares_native_build_contract in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        let producer_profile = fixture.request.profile.clone();
        if also_declares_native_build_contract {
            write_native_contract_fixture(&mut fixture.request, &producer_profile);
        }
        let contract_path = write_native_consumer_contract_fixture(
            &mut fixture.request,
            &producer_profile,
            also_declares_native_build_contract,
        );
        let arguments = cmake_arguments(&fixture.request);
        let contract_sha256 = sha256_bytes(&fs::read(&contract_path).unwrap());

        for selector in [
            format!(
                "-DAROS_NATIVE_CONSUMER_CONTRACT={}",
                contract_path.display()
            ),
            format!("-DAROS_NATIVE_CONSUMER_CONTRACT_SHA256={contract_sha256}"),
            "-DAROS_ABI_FLAVOUR=standalone".to_owned(),
            "-DAROS_ABI_PLATFORM_SMP=OFF".to_owned(),
        ] {
            assert_eq!(
                arguments
                    .iter()
                    .filter(|argument| **argument == selector)
                    .count(),
                1,
                "expected exactly one CMake selector {selector}"
            );
        }
        for selector in [
            "-DAROS_NATIVE_BUILD_CONTRACT=",
            "-DAROS_NATIVE_BUILD_CONTRACT_SHA256=",
            "-DAROS_NATIVE_BOARD=",
            "-DAROS_NATIVE_CORE=",
            "-DAROS_NATIVE_PACKAGE=",
            "-DAROS_NATIVE_MEDIA=",
        ] {
            assert!(
                arguments
                    .iter()
                    .all(|argument| !argument.starts_with(selector)),
                "consumer qualification must not pass {selector}"
            );
        }
    }
}

#[test]
fn builds_only_the_source_declared_native_consumer_roots_in_the_cmake_phase() {
    let temporary = tempfile::tempdir().unwrap();
    let mut fixture = gnu_fixture(temporary.path(), 32, false);
    let producer_profile = fixture.request.profile.clone();
    let contract_path =
        write_native_consumer_contract_fixture(&mut fixture.request, &producer_profile, false);
    let contract: Value = serde_json::from_slice(&fs::read(contract_path).unwrap()).unwrap();
    let declared_roots = contract["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|root| root.as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(declared_roots, ["includes", "linklibs"]);

    let report = execute_native_compatibility_with_readback(
        &fixture.request,
        &fixture.profiles,
        &CancellationToken::default(),
    )
    .unwrap();
    let consumer_phase = report
        .probes
        .reports
        .get(&CompatibilityPhase::CmakeConsumer)
        .unwrap();
    assert_eq!(consumer_phase.commands.len(), 2);

    let reports_root = &fixture.request.reports_root;
    let configure_arguments =
        fs::read_to_string(reports_root.join("cmake-consumer.1.stdout.log")).unwrap();
    assert!(configure_arguments.lines().any(|argument| argument == "-S"));
    assert!(!configure_arguments
        .lines()
        .any(|argument| argument == "--build"));

    let build_arguments = fs::read_to_string(reports_root.join("cmake-consumer.2.stdout.log"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(build_arguments.first().map(String::as_str), Some("--build"));
    assert_eq!(
        build_arguments.get(1).map(String::as_str),
        fixture
            .request
            .cmake_build_root
            .canonicalize()
            .unwrap()
            .to_str()
    );
    assert_eq!(
        build_arguments.get(2).map(String::as_str),
        Some("--parallel")
    );
    let expected_parallel_jobs = fixture.request.make_jobs.to_string();
    assert_eq!(
        build_arguments.get(3).map(String::as_str),
        Some(expected_parallel_jobs.as_str())
    );
    assert_eq!(build_arguments.get(4).map(String::as_str), Some("--target"));
    assert_eq!(&build_arguments[5..], declared_roots.as_slice());
}

#[test]
fn stops_before_later_phases_and_receipt_when_native_consumer_build_fails() {
    let temporary = tempfile::tempdir().unwrap();
    let mut fixture = gnu_fixture(temporary.path(), 32, false);
    let producer_profile = fixture.request.profile.clone();
    write_native_consumer_contract_fixture(&mut fixture.request, &producer_profile, false);
    amend_cmake_to_fail_native_consumer_build(&fixture.request);

    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility
    );

    let reports_root = &fixture.request.reports_root;
    assert!(reports_root.join("cmake-consumer.1.stdout.log").is_file());
    assert!(reports_root.join("cmake-consumer.2.stdout.log").is_file());
    assert!(!reports_root.join("cmake-consumer.report.json").exists());
    assert!(!reports_root.join("upstream-configure.report.json").exists());
    assert!(!reports_root
        .join("native-compatibility.receipt.json")
        .exists());

    let invocations = fs::read_to_string(&fixture.cmake_log).unwrap();
    let invocation_markers = invocations
        .lines()
        .filter(|line| *line == "-- invocation --")
        .count();
    assert_eq!(invocation_markers, 2);
    assert!(invocations.find("\n-S\n").unwrap() < invocations.find("\n--build\n").unwrap());
}

#[test]
fn rejects_invalid_declared_native_consumer_before_cmake_without_build_fallback() {
    for mutation in [
        "bad source input digest",
        "changed inventoried input",
        "mismatched source profile",
        "compiler ABI binding",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        let producer_profile = fixture.request.profile.clone();
        // This is a valid full build contract. A bad declared consumer must
        // reject instead of silently falling back to this broader contract.
        write_native_contract_fixture(&mut fixture.request, &producer_profile);
        let consumer_path =
            write_native_consumer_contract_fixture(&mut fixture.request, &producer_profile, true);
        match mutation {
            "bad source input digest" => {
                let mut contract: Value =
                    serde_json::from_slice(&fs::read(&consumer_path).unwrap()).unwrap();
                contract["inputs"][1]["sha256"] = json!("0".repeat(64));
                fs::write(&consumer_path, serde_json::to_vec(&contract).unwrap()).unwrap();
            }
            "changed inventoried input" => {
                fs::write(
                    fixture
                        .request
                        .preparation
                        .source_root
                        .join(NATIVE_CONSUMER_POLICY_FILE),
                    b"changed native consumer policy\n",
                )
                .unwrap();
            }
            "mismatched source profile" => {
                let mut contract: Value =
                    serde_json::from_slice(&fs::read(&consumer_path).unwrap()).unwrap();
                contract["profile"] = json!("different-source-profile");
                fs::write(&consumer_path, serde_json::to_vec(&contract).unwrap()).unwrap();
            }
            "compiler ABI binding" => {
                let mut contract: Value =
                    serde_json::from_slice(&fs::read(&consumer_path).unwrap()).unwrap();
                contract["abi"]["isa"] = json!("rv32imafc");
                fs::write(&consumer_path, serde_json::to_vec(&contract).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        refresh_source_digest(&mut fixture.request);
        let expected_message = if mutation == "compiler ABI binding" {
            "GNU source native consumer contract differs from the compiler"
        } else {
            "GNU source native consumer contract is invalid"
        };
        assert_rejected_before_cmake_with_message(&fixture, mutation, expected_message);
    }
}

#[test]
fn rejects_native_contract_source_input_and_compiler_mismatches_before_cmake() {
    for mutation in [
        "contract bytes",
        "inventoried input bytes",
        "compiler ABI binding",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = gnu_fixture(temporary.path(), 32, false);
        let profile = fixture.request.profile.clone();
        let contract_path = write_native_contract_fixture(&mut fixture.request, &profile);
        let expected_message = match mutation {
            "contract bytes" => {
                fs::write(&contract_path, b"not valid JSON").unwrap();
                "GNU source native build contract is invalid"
            }
            "inventoried input bytes" => {
                fs::write(
                    fixture
                        .request
                        .preparation
                        .source_root
                        .join(NATIVE_BUILD_INPUT_FILE),
                    b"mutated inventoried input\n",
                )
                .unwrap();
                "GNU source native build contract is invalid"
            }
            "compiler ABI binding" => {
                let mut contract: Value =
                    serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
                contract["abi"]["isa"] = json!("rv32imafc");
                fs::write(&contract_path, serde_json::to_vec(&contract).unwrap()).unwrap();
                "GNU source native build contract differs from the compiler"
            }
            _ => unreachable!(),
        };
        refresh_source_digest(&mut fixture.request);
        assert_rejected_before_cmake_with_message(&fixture, mutation, expected_message);
    }
}

#[test]
fn rejects_standalone_output_with_wrong_riscv_float_abi_after_all_six_phases() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = gnu_fixture(temporary.path(), 64, true);
    let cmake_log = fixture.cmake_log.clone();
    let reports_root = fixture.request.reports_root.clone();
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();

    assert!(error
        .to_string()
        .contains("source-bound RISC-V target contract"));
    assert!(cmake_log.is_file());
    for phase in [
        "cmake-consumer",
        "upstream-configure",
        "upstream-includes",
        "upstream-linklibs",
        "standalone-c",
        "standalone-cxx",
    ] {
        assert!(reports_root.join(format!("{phase}.report.json")).is_file());
    }
    assert!(!reports_root
        .join("native-compatibility.receipt.json")
        .exists());
}

#[test]
fn rejects_a_package_tree_changed_by_a_successful_phase_before_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = gnu_fixture(temporary.path(), 32, false);
    let cmake_program = fixture.request.cmake_program.clone();
    let cmake_log = shell_quote(&fixture.cmake_log.to_string_lossy());
    let first_role = shell_quote(
        &fixture
            .request
            .relocation
            .first
            .root
            .join(C_ROLE)
            .to_string_lossy(),
    );
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > {cmake_log}\nprintf 'post-phase mutation\\n' >> {first_role}\n"
    );
    executable(&cmake_program, script.as_bytes());

    let reports_root = fixture.request.reports_root.clone();
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("extracted package differs from its verified inventory"));
    for phase in [
        "cmake-consumer",
        "upstream-configure",
        "upstream-includes",
        "upstream-linklibs",
        "standalone-c",
        "standalone-cxx",
    ] {
        assert!(reports_root.join(format!("{phase}.report.json")).is_file());
    }
    assert!(!reports_root
        .join("native-compatibility.receipt.json")
        .exists());
}

#[test]
fn rejects_a_missing_gnu_phase_before_starting_any_command() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = gnu_fixture(temporary.path(), 32, false);
    let probe_set = CompatibilityProbeSetRequest {
        probes: vec![CompatibilityProbeRequest {
            phase: CompatibilityPhase::CmakeConsumer,
            commands: vec![CompatibilityCommand {
                program: fixture.request.cmake_program.clone(),
                arguments: Vec::new(),
            }],
            environment: CompatibilityEnvironment::Poisoned {
                variables: std::iter::once(("PATH".to_owned(), "/nonexistent".to_owned()))
                    .collect(),
            },
            current_dir: fixture.request.cmake_build_root.clone(),
            reports_root: fixture.request.reports_root.clone(),
            timeout: fixture.request.timeout,
            preparation: fixture.request.preparation.clone(),
        }],
    };

    let error = run_probe_set(&probe_set, &CancellationToken::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("does not contain every required phase"));
    assert!(!fixture.cmake_log.exists());
    assert!(!fixture.request.reports_root.exists());
}

#[test]
fn native_probe_preserves_declared_argv0_for_two_symlinked_compiler_roles() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = super::tests::request(temporary.path());
    let root = temporary.path().join("argv0-native-helper");
    fs::create_dir(&root).unwrap();
    let source = root.join("argv0-check.c");
    let helper = root.join("argv0-check");
    fs::write(
        &source,
        br#"#include <string.h>
int main(int argc, char **argv) {
    const char *name = strrchr(argv[0], '/');
    name = name ? name + 1 : argv[0];
    if (strcmp(name, "declared-c-driver") != 0 &&
        strcmp(name, "declared-cxx-driver") != 0) return 73;
    return argc == 2 && strcmp(argv[1], "probe") == 0 ? 0 : 74;
}
"#,
    )
    .unwrap();
    let compiler = Path::new("/usr/bin/cc");
    assert!(
        compiler.is_file(),
        "this native argv[0] regression requires /usr/bin/cc"
    );
    let mut compile = Command::new(compiler);
    compile.arg(&source).arg("-o").arg(&helper);
    let status = run_status(&mut compile).unwrap();
    assert!(status.status.success(), "failed to compile argv[0] helper");

    let c_alias = root.join("declared-c-driver");
    let cxx_alias = root.join("declared-cxx-driver");
    symlink(&helper, &c_alias).unwrap();
    symlink(&helper, &cxx_alias).unwrap();

    let direct_request = alias_probe_request(
        &request,
        helper,
        CompatibilityPhase::StandaloneC,
        &root,
        "direct",
    );
    assert!(run_probe(&direct_request, &CancellationToken::default()).is_err());

    let c_report = run_probe(
        &alias_probe_request(
            &request,
            c_alias,
            CompatibilityPhase::StandaloneC,
            &root,
            "alias-c",
        ),
        &CancellationToken::default(),
    )
    .unwrap();
    let cxx_report = run_probe(
        &alias_probe_request(
            &request,
            cxx_alias,
            CompatibilityPhase::StandaloneCxx,
            &root,
            "alias-cxx",
        ),
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(
        c_report.commands[0].program_sha256,
        cxx_report.commands[0].program_sha256
    );
    assert_ne!(
        c_report.commands[0].command_sha256,
        cxx_report.commands[0].command_sha256
    );
}

fn alias_probe_request(
    request: &NativeCompatibilityRequest,
    program: PathBuf,
    phase: CompatibilityPhase,
    root: &Path,
    suffix: &str,
) -> CompatibilityProbeRequest {
    let current_dir = root.join(format!("cwd-{suffix}"));
    let reports_root = root.join(format!("reports-{suffix}"));
    fs::create_dir(&current_dir).unwrap();
    fs::create_dir(&reports_root).unwrap();
    CompatibilityProbeRequest {
        phase,
        commands: vec![CompatibilityCommand {
            program,
            arguments: vec!["probe".into()],
        }],
        environment: CompatibilityEnvironment::Poisoned {
            variables: std::iter::once(("PATH".to_owned(), "/nonexistent".to_owned())).collect(),
        },
        current_dir,
        reports_root,
        timeout: request.timeout,
        preparation: request.preparation.clone(),
    }
}

fn assert_successful_gnu_execution(fixture: &Fixture, width: u8) {
    let request = &fixture.request;
    let environment = crate::compatibility::derive_native_compatibility_environment_identity(
        &request.host_python,
        &request.host_tools,
        &request.host,
    )
    .unwrap();
    let report = execute_native_compatibility_with_readback(
        request,
        &fixture.profiles,
        &CancellationToken::default(),
    )
    .unwrap();
    for (phase, probe) in &report.probes.reports {
        if matches!(
            phase,
            CompatibilityPhase::StandaloneC | CompatibilityPhase::StandaloneCxx
        ) {
            assert_eq!(
                probe.environment_sha256,
                environment.standalone_environment_sha256
            );
            assert!(probe.host_tools.is_empty());
        } else {
            assert_eq!(probe.environment_sha256, environment.sdk_environment_sha256);
            assert_eq!(probe.host_tools, environment.host_tools);
        }
    }
    let package_source_commit = request.package_source_commit.as_ref().unwrap();
    assert_ne!(package_source_commit, &request.upstream_source_commit);
    assert_eq!(
        request.relocation.second.verified.manifest.source_commit,
        package_source_commit.as_str()
    );
    let phases = report
        .probes
        .reports
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        phases,
        BTreeSet::from([
            CompatibilityPhase::CmakeConsumer,
            CompatibilityPhase::UpstreamConfigure,
            CompatibilityPhase::UpstreamIncludes,
            CompatibilityPhase::UpstreamLinklibs,
            CompatibilityPhase::StandaloneC,
            CompatibilityPhase::StandaloneCxx,
        ])
    );
    assert_eq!(report.standalone.targets.len(), 1);
    assert_eq!(
        report.standalone.targets.keys().next().map(String::as_str),
        Some(request.profile.target_triple())
    );

    let cmake_arguments = fs::read_to_string(&fixture.cmake_log).unwrap();
    for selector in [
        "-DAROS_TOOLCHAIN=gnu".to_owned(),
        format!(
            "-DAROS_TARGET_PROFILE={}",
            request.source_preset.as_deref().unwrap()
        ),
        format!("-DAROS_CROSS_TOOLCHAIN_PROFILE={}", request.profile.name()),
        format!("-DAROS_TARGET_TRIPLE={}", request.profile.target_triple()),
        format!("-DAROS_TARGET_CPU={}", request.profile.cpu()),
        format!("-DAROS_TARGET_FAMILY=fixture-family-rv{width}"),
        format!("-DAROS_TARGET_VARIANT=fixture-variant-rv{width}"),
        "-DAROS_TARGET_CPU32=fixture-cpu32".to_owned(),
        "-DAROS_ENABLE_MMU=OFF".to_owned(),
        "-DAROS_MESA_VERSION=fixture-mesa".to_owned(),
        "-DAROS_TARGET_BOOTLOADER=fixture-bootloader".to_owned(),
        "-DAROS_ABI_FLAVOUR=standalone".to_owned(),
        "-DAROS_ABI_PLATFORM_SMP=OFF".to_owned(),
    ] {
        assert!(
            cmake_arguments.contains(&selector),
            "missing CMake selector {selector}"
        );
    }
    let native_contract = request
        .preparation
        .source_root
        .join(NATIVE_BUILD_CONTRACT_FILE);
    if native_contract.is_file() {
        let canonical_contract = native_contract.canonicalize().unwrap();
        let contract_sha256 = sha256_bytes(&fs::read(&canonical_contract).unwrap());
        for selector in [
            format!(
                "-DAROS_NATIVE_BUILD_CONTRACT={}",
                canonical_contract.display()
            ),
            format!("-DAROS_NATIVE_BUILD_CONTRACT_SHA256={contract_sha256}"),
        ] {
            assert!(
                cmake_arguments.contains(&selector),
                "missing native-contract CMake binding {selector}"
            );
        }
    }

    let configure_log =
        fs::read_to_string(request.reports_root.join("upstream-configure.stdout.log")).unwrap();
    assert!(configure_log.contains("--with-toolchain=gnu"));
    assert!(configure_log.contains(&format!("--with-gcc-version={GCC_VERSION}")));
    assert!(configure_log.contains(&format!("--with-binutils-version={BINUTILS_VERSION}")));
    assert!(!configure_log.contains("--with-toolchain=llvm"));
    assert!(!configure_log.contains("--with-llvm-version="));
    let make_log = fs::read_to_string(&fixture.make_log).unwrap();
    assert!(make_log.contains("includes"));
    assert!(make_log.contains("linklibs"));

    let target = request.profile.target().unwrap();
    let build_root = request
        .upstream_build_root
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .join(request.upstream_build_root.file_name().unwrap());
    let sysroot = build_root
        .join("bin")
        .join(request.profile.upstream_output_target())
        .join("AROS/Developer");
    let c_log = fs::read_to_string(request.reports_root.join("standalone-c.stdout.log")).unwrap();
    let cxx_log =
        fs::read_to_string(request.reports_root.join("standalone-cxx.stdout.log")).unwrap();
    for (log, role) in [(&c_log, C_ROLE), (&cxx_log, CXX_ROLE)] {
        assert!(log.contains(&format!("declared-role=<{role}>")));
        assert!(log.contains(&format!("arg=<-march={}>", target.isa())));
        assert!(log.contains(&format!("arg=<-mabi={}>", target.abi())));
        assert!(log.contains(&format!("arg=<-mcmodel={}>", target.code_model())));
        let alignment_flag = if target.unaligned_access() {
            "-mno-strict-align"
        } else {
            "-mstrict-align"
        };
        assert!(log.contains(&format!("arg=<{alignment_flag}>")));
        assert!(log.contains(&format!("arg=<--sysroot={}>", sysroot.display())));
        assert!(!log.contains("--target="));
        assert!(log.contains("path=</nonexistent>"));
    }

    assert!(report.receipt.path.is_file());
    let receipt: Value = serde_json::from_slice(&fs::read(&report.receipt.path).unwrap()).unwrap();
    assert_eq!(
        receipt["schema"],
        "aros-toolchain-native-compatibility-receipt-v3"
    );
    assert_eq!(receipt["package"]["compiler"]["family"], "gnu");
    assert_eq!(
        receipt["package"]["target_triple"],
        request.profile.target_triple()
    );
    assert_eq!(
        receipt["package"]["source_preset"],
        request.source_preset.as_deref().unwrap()
    );
    assert_eq!(
        receipt["package"]["source_tree_sha256"],
        request.preparation.source_tree_sha256.to_string()
    );
    assert_eq!(receipt["phase_reports"].as_array().unwrap().len(), 6);
    assert_eq!(
        receipt["standalone_targets"][request.profile.target_triple()]["c"]["class"],
        if width == 32 { "elf32" } else { "elf64" }
    );
}

#[test]
fn forwards_only_verified_source_declared_host_generator_cache() {
    let temporary = tempfile::tempdir().unwrap();
    let mut fixture = gnu_fixture(temporary.path(), 32, false);
    let profile = fixture.request.profile.clone();
    let contract = write_native_consumer_contract_fixture(&mut fixture.request, &profile, false);
    add_host_generator_inputs(&mut fixture.request, &contract);
    let cache = fixture
        .request
        .host_generator_cache_root
        .as_ref()
        .unwrap()
        .canonicalize()
        .unwrap();
    let inputs = validate_inputs(&fixture.request).unwrap();
    let staged = inputs.host_generator_cache.as_ref().unwrap().root();
    assert_ne!(staged, cache);
    assert_eq!(
        fs::read(staged.join("fixture.txt")).unwrap(),
        b"sealed host fixture\n"
    );
    assert!(
        cmake_arguments_for_inputs(&fixture.request, &inputs).contains(&format!(
            "-DAROS_NATIVE_HOST_INPUT_DIRECTORY={}",
            staged.display()
        ))
    );
    fs::write(cache.join("fixture.txt"), b"corrupt host input").unwrap();
    assert!(validate_inputs(&fixture.request).is_err());
    assert!(!fixture.request.cmake_build_root.exists());
}

#[test]
fn requires_declared_host_generator_cache_before_creating_outputs() {
    let temporary = tempfile::tempdir().unwrap();
    let mut fixture = gnu_fixture(temporary.path(), 32, false);
    let profile = fixture.request.profile.clone();
    let contract = write_native_consumer_contract_fixture(&mut fixture.request, &profile, false);
    add_host_generator_inputs(&mut fixture.request, &contract);
    fixture.request.host_generator_cache_root = None;
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("host generator inputs require an explicit"));
    assert!(!fixture.request.cmake_build_root.exists());
    assert!(!fixture.request.reports_root.exists());
}

fn add_host_generator_inputs(request: &mut NativeCompatibilityRequest, contract_path: &Path) {
    let source = &request.preparation.source_root;
    let mut contract: serde_json::Value =
        serde_json::from_slice(&fs::read(contract_path).unwrap()).unwrap();
    for path in [
        "Makefile.in",
        "configure.in",
        "fixture/mmakefile.src",
        "fixture/Makefile",
        "fixture/generator.c",
    ] {
        let full_path = source.join(path);
        fs::create_dir_all(full_path.parent().unwrap()).unwrap();
        fs::write(&full_path, b"source-owned fixture\n").unwrap();
        contract["inputs"].as_array_mut().unwrap().push(json!({
            "path": path, "sha256": sha256_bytes(b"source-owned fixture\n")
        }));
    }
    let data = b"sealed host fixture\n";
    contract["host_file_generators"] = json!([{
        "owner": "fixture-host-generator", "recipe": "fixture/mmakefile.src",
        "tool_recipe": "fixture/Makefile", "tool_source": "fixture/generator.c",
        "tool_variable": "FIXTURE_GENERATOR", "output": "gen/fixture-output.c",
        "input_directory": "gen/fixture-inputs", "compile_flags": ["-O2"],
        "arguments": ["@INPUT_DIRECTORY@", "@OUTPUT_DIRECTORY@", "fixture-output.c"],
        "inputs": [{"filename": "fixture.txt", "url": "https://example.invalid/v1/fixture.txt",
                    "size": data.len(), "sha256": sha256_bytes(data)}]
    }]);
    fs::write(contract_path, serde_json::to_vec_pretty(&contract).unwrap()).unwrap();
    let cache = request
        .cmake_build_root
        .parent()
        .unwrap()
        .join("host-generator-cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join("fixture.txt"), data).unwrap();
    request.host_generator_cache_root = Some(cache);
    refresh_source_digest(request);
}

fn cmake_arguments(request: &NativeCompatibilityRequest) -> Vec<String> {
    let inputs = validate_inputs(request).unwrap();
    cmake_arguments_for_inputs(request, &inputs)
}

fn cmake_arguments_for_inputs(
    request: &NativeCompatibilityRequest,
    inputs: &super::Inputs,
) -> Vec<String> {
    let outputs = OutputRoots {
        cmake_build: request.cmake_build_root.clone(),
        upstream_build: request.upstream_build_root.clone(),
        standalone: request.standalone_output_root.clone(),
        reports: request.reports_root.clone(),
    };
    let helpers_root = helpers_root(&request.preparation).unwrap();
    cmake_command(request, inputs, &outputs, &helpers_root)
        .unwrap()
        .arguments
}

fn assert_rejected_before_cmake(fixture: &Fixture) {
    assert_rejected_before_cmake_with_label(fixture, "GNU compatibility input should be rejected");
}

fn assert_rejected_before_cmake_with_label(fixture: &Fixture, label: &str) {
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility,
        "{label}"
    );
    assert!(
        !fixture.cmake_log.exists(),
        "{label} must fail before CMake"
    );
    assert!(
        !fixture.request.reports_root.exists(),
        "{label} must not create phase reports"
    );
}

fn assert_rejected_before_cmake_with_message(
    fixture: &Fixture,
    label: &str,
    expected_message: &str,
) {
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility,
        "{label}"
    );
    assert!(
        error.to_string().contains(expected_message),
        "{label}: unexpected failure: {error}"
    );
    assert!(
        !fixture.cmake_log.exists(),
        "{label} must fail before CMake"
    );
    assert!(
        !fixture.request.reports_root.exists(),
        "{label} must not create phase reports"
    );
}

pub(super) fn gnu_fixture(root: &Path, width: u8, wrong_float_abi: bool) -> Fixture {
    let (mut request, cmake_log, make_log) = super::tests::request(root);
    amend_cmake_to_log_invocations(&request);
    amend_configure_to_log_arguments(&mut request);
    request.package_source_commit =
        Some(GitObjectId::try_from(PACKAGE_SOURCE_COMMIT.to_owned()).unwrap());
    let profiles = Profiles::parse(&gnu_profiles(request.upstream_source_commit.as_str())).unwrap();
    let selected_name = if width == 32 {
        "rv32-compat"
    } else {
        "rv64-compat"
    };
    let profile = profiles.select(selected_name).unwrap().clone();
    request.profile = profile.clone();
    let source_preset = format!("source-rv{width}-preset");
    request.source_preset = Some(source_preset.clone());
    let source_declaration = source_preset_toml(
        &source_preset,
        Some(profile.name()),
        if width == 32 { "riscv32" } else { "riscv64" },
        profile.platform(),
        "gnu",
        None,
        None,
    );
    write_source_declaration(&mut request, &source_declaration);
    let compiler = compiler_for(&profile, GCC_VERSION);
    let class = if width == 32 {
        elf::Class::Elf32
    } else {
        elf::Class::Elf64
    };
    let c_fixture = root.join(format!("gnu-rv{width}-c.elf"));
    let cxx_fixture = root.join(format!("gnu-rv{width}-cxx.elf"));
    fs::write(
        &c_fixture,
        riscv_elf(
            class,
            C_COLLECTOR_SYMBOL,
            profile.target().unwrap(),
            wrong_float_abi,
        ),
    )
    .unwrap();
    fs::write(
        &cxx_fixture,
        riscv_elf(
            class,
            CXX_COLLECTOR_SYMBOL,
            profile.target().unwrap(),
            wrong_float_abi,
        ),
    )
    .unwrap();

    let first = root.join(format!("gnu-rv{width}-first"));
    let second = root.join(format!("gnu-rv{width}-second"));
    write_gnu_package_root(
        &first,
        &request,
        &profile,
        &compiler,
        &c_fixture,
        &cxx_fixture,
    );
    write_gnu_package_root(
        &second,
        &request,
        &profile,
        &compiler,
        &c_fixture,
        &cxx_fixture,
    );
    let verified = VerifiedPackage {
        manifest: ArosToolchainManifest::load(&second).unwrap(),
        archive_sha256: Sha256Digest::parse(&"a".repeat(64)).unwrap(),
        archive_size: 123,
    };
    request.relocation = TwoRootRelocation {
        first: ExtractedPackage {
            root: first,
            verified: verified.clone(),
        },
        second: ExtractedPackage {
            root: second,
            verified,
        },
    };
    Fixture {
        request,
        profiles,
        cmake_log,
        make_log,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeSdkBindingMode {
    Valid,
    Missing,
    Stale,
}

fn gnu_native_sdk_fixture(root: &Path, mode: NativeSdkBindingMode) -> Fixture {
    let mut fixture = gnu_fixture(root, 32, false);
    let profile = fixture.request.profile.clone();
    let contract_path =
        write_native_consumer_contract_fixture(&mut fixture.request, &profile, false);
    let source_root = fixture.request.preparation.source_root.clone();
    let mut contract: Value = serde_json::from_slice(&fs::read(&contract_path).unwrap()).unwrap();
    contract["schema"] = json!("aros-native-consumer-contract-v2");

    let c_source = "native-sdk-probe.c";
    let cxx_source = "native-sdk-probe.cpp";
    let probe_bytes =
        b"#include <native-sdk-fixture.h>\nint main(void) { return native_sdk_fixture_value(); }\n";
    let cxx_probe_bytes =
        b"#include <native-sdk-fixture.h>\nint main() { return native_sdk_fixture_value(); }\n";
    fs::write(source_root.join(c_source), probe_bytes).unwrap();
    fs::write(source_root.join(cxx_source), cxx_probe_bytes).unwrap();
    for (path, bytes) in [
        (c_source, probe_bytes.as_slice()),
        (cxx_source, cxx_probe_bytes.as_slice()),
    ] {
        contract["inputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"path": path, "sha256": sha256_bytes(bytes)}));
    }
    contract["native_sdk_link_probes"] = json!({
        "c": {"source": c_source, "libraries": ["client"]},
        "cxx": {"source": cxx_source, "libraries": ["client"]}
    });
    fs::write(
        &contract_path,
        serde_json::to_vec_pretty(&contract).unwrap(),
    )
    .unwrap();
    refresh_source_digest(&mut fixture.request);

    let binding = native_consumer_binding(&fixture.request, &contract_path, &contract);
    let helper = fixture
        .request
        .preparation
        .helpers
        .get_mut("aros-transpiler")
        .unwrap();
    let rendered_binding = serde_json::to_string(&binding).unwrap();
    let helper_script = format!(
        "#!/bin/sh\nprintf '%s\\n' {}\n",
        shell_quote(&rendered_binding)
    );
    fs::write(&helper.path, helper_script.as_bytes()).unwrap();
    fs::set_permissions(&helper.path, fs::Permissions::from_mode(0o755)).unwrap();
    helper.sha256 = sha256_bytes(helper_script.as_bytes());
    helper.size = helper_script.len() as u64;

    amend_cmake_to_build_native_sdk(&fixture.request, &binding, mode);
    let fixture_root = fixture.request.relocation.first.root.parent().unwrap();
    let c_elf = fixture_root.join("gnu-rv32-c.elf");
    let cxx_elf = fixture_root.join("gnu-rv32-cxx.elf");
    let second_package_root = fixture
        .request
        .relocation
        .second
        .root
        .canonicalize()
        .unwrap();
    for package_root in [
        fixture.request.relocation.first.root.clone(),
        second_package_root.clone(),
    ] {
        executable(
            &package_root.join(C_ROLE),
            &native_sdk_shell_driver(&c_elf, C_ROLE, &second_package_root),
        );
        executable(
            &package_root.join(CXX_ROLE),
            &native_sdk_shell_driver(&cxx_elf, CXX_ROLE, &second_package_root),
        );
    }
    let first_manifest = refresh_gnu_package_manifest(&fixture.request.relocation.first.root);
    let second_manifest = refresh_gnu_package_manifest(&fixture.request.relocation.second.root);
    assert_eq!(first_manifest, second_manifest);
    fixture.request.relocation.first.verified.manifest = first_manifest;
    fixture.request.relocation.second.verified.manifest = second_manifest;
    fixture
}

fn native_consumer_binding(
    request: &NativeCompatibilityRequest,
    contract_path: &Path,
    contract: &Value,
) -> Value {
    let contract_path = contract_path.canonicalize().unwrap();
    let source_root = request.preparation.source_root.canonicalize().unwrap();
    let contract_bytes = fs::read(&contract_path).unwrap();
    json!({
        "schema": "aros-native-consumer-validation-v1",
        "qualification": "source-binding-not-graph-or-build-proof",
        "source_dir": source_root.to_string_lossy(),
        "contract_path": contract_path.to_string_lossy(),
        "contract_sha256": sha256_bytes(&contract_bytes),
        "profile": contract["profile"],
        "abi": contract["abi"],
        "exec_smp": false,
        "input_paths": contract["inputs"].as_array().unwrap().iter()
            .map(|input| input["path"].clone()).collect::<Vec<_>>(),
        "sdk_include_relative": "SYS/Developer/include"
    })
}

fn amend_cmake_to_build_native_sdk(
    request: &NativeCompatibilityRequest,
    binding: &Value,
    mode: NativeSdkBindingMode,
) {
    let cmake_log = shell_quote(&cmake_log_path(request));
    let mut binding = binding.clone();
    if mode == NativeSdkBindingMode::Stale {
        binding["profile"] = json!("stale-source-profile");
    }
    let binding_line = if mode == NativeSdkBindingMode::Missing {
        String::new()
    } else {
        format!(
            "printf '%s\\n' {} > \"$build_root/aros-native-consumer-binding.json\" || exit 53\n",
            shell_quote(&serde_json::to_string(&binding).unwrap())
        )
    };
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' '-- invocation --' >> {cmake_log}\n\
         printf '%s\\n' \"$@\" >> {cmake_log}\n\
         if [ \"$1\" = --build ]; then\n\
         build_root= previous=\n\
         for arg do if [ \"$previous\" = --build ]; then build_root=$arg; break; fi; previous=$arg; done\n\
         [ -n \"$build_root\" ] || exit 51\n\
         /bin/mkdir -p \"$build_root/SYS/Developer/include\" \"$build_root/SYS/Developer/lib\" || exit 52\n\
         printf '%s\\n' '#ifndef NATIVE_SDK_FIXTURE_H' '#define NATIVE_SDK_FIXTURE_H' 'static inline int native_sdk_fixture_value(void) {{ return 0; }}' '#endif' > \"$build_root/SYS/Developer/include/native-sdk-fixture.h\" || exit 54\n\
         printf '%s\\n' 'synthetic native SDK client archive' > \"$build_root/SYS/Developer/lib/client.a\" || exit 55\n\
         /bin/chmod 0640 \"$build_root/SYS/Developer/lib/client.a\" || exit 56\n\
         /bin/ln -s client.a \"$build_root/SYS/Developer/lib/libclient.a\" || exit 57\n\
         {binding_line}\
         fi\n\
         printf '%s\\n' \"$@\"\n"
    );
    executable(&request.cmake_program, script.as_bytes());
}

fn native_sdk_shell_driver(fixture: &Path, role: &str, second_package_root: &Path) -> Vec<u8> {
    let fixture = shell_quote(&fixture.to_string_lossy());
    let second_package_root = shell_quote(&second_package_root.to_string_lossy());
    format!(
        "#!/bin/sh\n\
         [ \"$PATH\" = /nonexistent ] || exit 41\n\
         sysroot= map= out= source= library= previous= freestanding=no\n\
         for arg do\n\
         case \"$arg\" in\n\
         --sysroot=*) sysroot=${{arg#--sysroot=}} ;;\n\
         -Wl,-Map=*,--cref,--trace) map=${{arg#-Wl,-Map=}}; map=${{map%,--cref,--trace}} ;;\n\
         -lclient) library=yes ;;\n\
         *.c|*.cpp) source=$arg ;;\n\
         -c|-r) exit 42 ;;\n\
         -nostdlib|-nostartfiles|-nodefaultlibs) freestanding=yes ;;\n\
         esac\n\
         if [ \"$previous\" = -o ]; then out=$arg; fi\n\
         previous=$arg\n\
         done\n\
         if [ -n \"$map\" ]; then\n\
         [ \"$freestanding\" != yes ] || exit 42\n\
         case \"$0\" in {second_package_root}/*) exit 48 ;; esac\n\
         [ -n \"$sysroot\" ] && [ -n \"$out\" ] && [ -n \"$source\" ] && [ \"$library\" = yes ] || exit 43\n\
         [ -f \"$source\" ] && [ -f \"$sysroot/include/native-sdk-fixture.h\" ] && [ -f \"$sysroot/lib/client.a\" ] && [ -f \"$sysroot/lib/libclient.a\" ] || exit 44\n\
         trace=\"$sysroot/lib/libclient.a\"\n\
         printf 'synthetic application map\\n%s\\n' \"$trace\" > \"$map\" || exit 45\n\
         printf '%s\\n' \"$trace\"\n\
         /bin/cp {fixture} \"$out\" || exit 46\n\
         exit 0\n\
         fi\n\
         printf 'path=<%s>\\n' \"$PATH\"\n\
         printf 'declared-role=<{role}>\\n'\n\
         for arg do printf 'arg=<%s>\\n' \"$arg\"; done\n\
         [ -n \"$out\" ] || exit 47\n\
         /bin/cp {fixture} \"$out\"\n"
    )
    .into_bytes()
}

fn refresh_gnu_package_manifest(root: &Path) -> ArosToolchainManifest {
    let (tree_sha256, files) = toolchain_tree_inventory(root).unwrap();
    let path = root.join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE);
    let mut manifest: ArosToolchainManifest =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest.tree_sha256 = tree_sha256;
    manifest.files = files;
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    ArosToolchainManifest::load(root).unwrap()
}

fn amend_cmake_to_log_invocations(request: &NativeCompatibilityRequest) {
    let cmake_log = shell_quote(&cmake_log_path(request));
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' '-- invocation --' >> {cmake_log}\nprintf '%s\\n' \"$@\" >> {cmake_log}\nprintf '%s\\n' \"$@\"\n"
    );
    executable(&request.cmake_program, script.as_bytes());
}

fn amend_cmake_to_fail_native_consumer_build(request: &NativeCompatibilityRequest) {
    let cmake_log = shell_quote(&cmake_log_path(request));
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' '-- invocation --' >> {cmake_log}\nprintf '%s\\n' \"$@\" >> {cmake_log}\nprintf '%s\\n' \"$@\"\nif [ \"$1\" = --build ]; then printf 'synthetic native consumer build failure\\n' >&2; exit 23; fi\n"
    );
    executable(&request.cmake_program, script.as_bytes());
}

fn cmake_log_path(request: &NativeCompatibilityRequest) -> String {
    request
        .cmake_program
        .parent()
        .unwrap()
        .join("cmake-arguments.log")
        .to_string_lossy()
        .into_owned()
}

fn source_preset_toml(
    name: &str,
    compiler_profile: Option<&str>,
    arch: &str,
    platform: &str,
    toolchain: &str,
    float_abi: Option<&str>,
    native_build_contract: Option<&str>,
) -> String {
    let compiler_profile = compiler_profile
        .map(|profile| format!("toolchain_profile = \"{profile}\"\n"))
        .unwrap_or_default();
    let width = if arch == "riscv32" { 32 } else { 64 };
    let float_abi = float_abi.unwrap_or(if width == 32 { "ilp32f" } else { "lp64d" });
    let native_build_contract = native_build_contract
        .map(|path| format!("native_build_contract = \"{path}\"\n"))
        .unwrap_or_default();
    format!(
        "[[targets]]\nname = \"{name}\"\n{compiler_profile}arch = \"{arch}\"\nplatform = \"{platform}\"\nbsp = \"fixture\"\nfloat_abi = \"{float_abi}\"\nbootloader = \"fixture-bootloader\"\n{native_build_contract}\n[targets.transpiler]\nfamily = \"fixture-family-rv{width}\"\nvariant = \"fixture-variant-rv{width}\"\ntoolchain = \"{toolchain}\"\ncpu32 = \"fixture-cpu32\"\nuse_mmu = false\nmesa_version = \"fixture-mesa\"\n\n[targets.bootstrap_abi]\nflavour = \"standalone\"\nplatform_smp = false\n"
    )
}

fn write_source_declaration(request: &mut NativeCompatibilityRequest, declaration: &str) {
    let path = request.preparation.source_root.join("aros-targets.toml");
    fs::write(&path, declaration).unwrap();
    refresh_source_digest(request);
}

fn refresh_source_digest(request: &mut NativeCompatibilityRequest) {
    request.preparation.source_tree_sha256 =
        measure_tree_content_cas(&request.preparation.source_root)
            .unwrap()
            .payload_digest_excluding(None);
}

fn write_native_contract_fixture(
    request: &mut NativeCompatibilityRequest,
    producer_profile: &Profile,
) -> PathBuf {
    assert_eq!(producer_profile.cpu(), "riscv");
    let source_profile = request.source_preset.as_deref().unwrap();
    let source_root = &request.preparation.source_root;
    fs::write(
        source_root.join(NATIVE_BUILD_INPUT_FILE),
        b"synthetic source-bound native contract input\n",
    )
    .unwrap();
    let target = producer_profile.target().unwrap();
    let input_sha256 = sha256_bytes(b"synthetic source-bound native contract input\n");
    let contract = json!({
        "schema_version": 1,
        "profile": source_profile,
        "board": "fixture",
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "qualification": "experimental-unqualified",
        "inputs": [{
            "path": NATIVE_BUILD_INPUT_FILE,
            "sha256": input_sha256,
        }],
        "abi": {
            "source_cpu": "riscv",
            "target_triple": producer_profile.target_triple(),
            "isa": target.isa(),
            "abi": target.abi(),
            "code_model": target.code_model(),
            "flavour": "standalone",
            "platform_smp": false,
            "use_mmu": false,
        },
        "core": {
            "recipe": NATIVE_BUILD_INPUT_FILE,
            "linker_script": NATIVE_BUILD_INPUT_FILE,
            "resources": ["kernel"],
            "libraries": ["exec"],
            "devices": ["timer"],
            "link_libraries": ["exec"],
            "compiler_runtime_role": "libgcc",
            "residency_check": NATIVE_BUILD_INPUT_FILE,
            "residency_policy": {
                "algorithm": "riscv32-xip-v1",
                "section": ".sramtext",
                "flash_start": 1_073_741_824_u64,
                "flash_end": 1_140_850_688_u64,
                "sram_start": 1_341_128_704_u64,
                "sram_end": 1_341_652_992_u64,
            },
        },
        "package": {
            "recipe": NATIVE_BUILD_INPUT_FILE,
            "format": "aros-pkg-v1",
            "target": "fixture-native-package",
            "limit_from_board": "FIXTURE_PACKAGE_LIMIT",
        },
        "media": {
            "chip": "fixture-chip",
            "board_rules": NATIVE_BUILD_INPUT_FILE,
            "partition_table": NATIVE_BUILD_INPUT_FILE,
            "core_partition": "core_partition",
            "package_partition": "package_partition",
            "development_volume_offset_from_board": "FIXTURE_VOLUME_OFFSET",
            "bootloader_configuration": NATIVE_BUILD_INPUT_FILE,
            "bootloader_patch": NATIVE_BUILD_INPUT_FILE,
            "idf_version": "6.0.1",
        },
    });
    let bytes = serde_json::to_vec_pretty(&contract).unwrap();
    let contract_path = source_root.join(NATIVE_BUILD_CONTRACT_FILE);
    fs::write(&contract_path, bytes).unwrap();

    let declaration = source_preset_toml(
        source_profile,
        Some(producer_profile.name()),
        "riscv32",
        producer_profile.platform(),
        "gnu",
        Some(producer_profile.float_abi()),
        Some(NATIVE_BUILD_CONTRACT_FILE),
    );
    write_source_declaration(request, &declaration);
    contract_path.canonicalize().unwrap()
}

pub(super) fn write_native_consumer_contract_fixture(
    request: &mut NativeCompatibilityRequest,
    producer_profile: &Profile,
    also_declare_native_build_contract: bool,
) -> PathBuf {
    assert_eq!(producer_profile.cpu(), "riscv");
    let source_profile = request.source_preset.clone().unwrap();
    let source_root = request.preparation.source_root.clone();
    fs::write(source_root.join(NATIVE_CONSUMER_POLICY_FILE), b"{}\n").unwrap();
    let native_build_contract =
        also_declare_native_build_contract.then_some(NATIVE_BUILD_CONTRACT_FILE);
    let mut declaration = source_preset_toml(
        &source_profile,
        Some(producer_profile.name()),
        "riscv32",
        producer_profile.platform(),
        "gnu",
        Some(producer_profile.float_abi()),
        native_build_contract,
    );
    declaration = declaration.replace(
        "bootloader = \"fixture-bootloader\"\n",
        &format!(
            "bootloader = \"fixture-bootloader\"\nnative_consumer_contract = \"{NATIVE_CONSUMER_CONTRACT_FILE}\"\n"
        ),
    );
    write_source_declaration(request, &declaration);

    let target = producer_profile.target().unwrap();
    let profile_bytes = fs::read(source_root.join("aros-targets.toml")).unwrap();
    let policy_bytes = fs::read(source_root.join(NATIVE_CONSUMER_POLICY_FILE)).unwrap();
    let contract = json!({
        "schema": "aros-native-consumer-contract-v1",
        "profile": source_profile,
        "source_baseline": "0123456789abcdef0123456789abcdef01234567",
        "roots": ["includes", "linklibs"],
        "inputs": [
            {
                "path": "aros-targets.toml",
                "sha256": sha256_bytes(&profile_bytes),
            },
            {
                "path": NATIVE_CONSUMER_POLICY_FILE,
                "sha256": sha256_bytes(&policy_bytes),
            },
        ],
        "abi": {
            "source_cpu": "riscv",
            "target_triple": producer_profile.target_triple(),
            "isa": target.isa(),
            "abi": target.abi(),
            "code_model": target.code_model(),
            "flavour": "standalone",
            "platform_smp": false,
            "use_mmu": false,
        },
        "metamake_projection": NATIVE_CONSUMER_POLICY_FILE,
    });
    let contract_path = source_root.join(NATIVE_CONSUMER_CONTRACT_FILE);
    fs::write(
        &contract_path,
        serde_json::to_vec_pretty(&contract).unwrap(),
    )
    .unwrap();
    refresh_source_digest(request);
    contract_path.canonicalize().unwrap()
}

fn write_gnu_package_root(
    root: &Path,
    request: &NativeCompatibilityRequest,
    profile: &Profile,
    compiler: &ArosCompilerIdentity,
    c_fixture: &Path,
    cxx_fixture: &Path,
) {
    fs::create_dir(root).unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("drivers")).unwrap();
    executable(&root.join(C_ROLE), &shell_driver(c_fixture, C_ROLE));
    executable(&root.join(CXX_ROLE), &shell_driver(cxx_fixture, CXX_ROLE));
    for role in [
        "assembler",
        "linker",
        "archive",
        "ranlib",
        "strip",
        "collector",
        "nm",
        "objcopy",
        "objdump",
    ] {
        executable(&root.join("bin").join(role), b"#!/bin/sh\nexit 0\n");
    }
    let layout = json!({
        "schema": "aros-toolchain-tools-v3",
        "compiler": compiler,
        "target_triple": profile.target_triple(),
        "tools": {
            "c": C_ROLE, "cxx": CXX_ROLE,
            "assembler": "bin/assembler", "linker": "bin/linker",
            "archive": "bin/archive", "ranlib": "bin/ranlib",
            "strip": "bin/strip", "collector": "bin/collector",
            "nm": "bin/nm", "objcopy": "bin/objcopy", "objdump": "bin/objdump"
        }
    });
    fs::write(
        root.join(aros_common::toolchain_layout::TOOLCHAIN_TOOLS_FILE),
        serde_json::to_vec(&layout).unwrap(),
    )
    .unwrap();

    // The embedded manifest describes a measured inventory. It is excluded
    // from that inventory to avoid a self-referential digest.
    let (tree_sha256, files) = aros_common::toolchain_tree_inventory(root).unwrap();
    let manifest = ArosToolchainManifest {
        schema: 2,
        release_id: "toolchain-v2-gnu-fixture".into(),
        host: request.host.clone(),
        target_profile: profile.name().into(),
        target_triple: profile.target_triple().into(),
        tree_sha256,
        llvm_version: None,
        compiler: Some(compiler.clone()),
        recipe_sha256: "a".repeat(64),
        source_lock_sha256: "b".repeat(64),
        profiles_sha256: profile.document_sha256().to_string(),
        source_commit: request
            .package_source_commit
            .as_ref()
            .unwrap()
            .as_str()
            .into(),
        producer_commit: "c".repeat(40),
        tools_commit: "d".repeat(40),
        source_date_epoch: 1,
        capabilities: profile.capabilities().to_vec(),
        build_environment: serde_json::Map::new(),
        files,
    };
    manifest.validate().unwrap();
    fs::write(
        root.join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

fn shell_driver(fixture: &Path, role: &str) -> Vec<u8> {
    let fixture = shell_quote(&fixture.to_string_lossy());
    format!(
        "#!/bin/sh\n[ \"$PATH\" = /nonexistent ] || exit 41\nprintf 'path=<%s>\\n' \"$PATH\"\nprintf 'declared-role=<{role}>\\n'\nfor arg do printf 'arg=<%s>\\n' \"$arg\"; done\nout=\nprevious=\nfor arg do if [ \"$previous\" = -o ]; then out=$arg; fi; previous=$arg; done\n[ -n \"$out\" ] || exit 42\n/bin/cp {fixture} \"$out\"\n"
    )
    .into_bytes()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn compiler_for(profile: &Profile, gcc_version: &str) -> ArosCompilerIdentity {
    ArosCompilerIdentity::Gnu {
        gcc_version: gcc_version.into(),
        binutils_version: BINUTILS_VERSION.into(),
        target: profile.target().unwrap().clone(),
    }
}

fn float_flags(abi: &str) -> u32 {
    match abi {
        "ilp32" | "lp64" => 0,
        "ilp32f" | "lp64f" => 2,
        "ilp32d" | "lp64d" => 4,
        other => panic!("unsupported fixture ABI: {other}"),
    }
}

fn gnu_profiles(upstream_commit: &str) -> Vec<u8> {
    let rv32 = json!({
        "name": "rv32-compat",
        "configure_target": "fixture-rv32",
        "upstream_output_target": "fixture-rv32",
        "target_triple": "riscv-aros",
        "cpu": "riscv",
        "platform": "fixture",
        "float_abi": "ilp32f",
        "capabilities": ["c", "cxx", "libgcc", "libstdcxx", "libsupcxx", "standalone-collector"],
        "target": {
            "schema": "aros-riscv-target-v1",
            "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
            "abi": "ilp32f",
            "code_model": "medany",
            "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0",
            "unaligned_access": false,
            "atomic_abi": 0,
            "x3_reg_usage": 0
        }
    });
    let mut rv32_alternate = rv32.clone();
    rv32_alternate["name"] = json!("rv32-alternate");
    rv32_alternate["configure_target"] = json!("fixture-rv32-alternate");
    let rv64 = json!({
        "name": "rv64-compat",
        "configure_target": "fixture-rv64",
        "upstream_output_target": "fixture-rv64",
        "target_triple": "riscv64-aros",
        "cpu": "riscv64",
        "platform": "fixture",
        "float_abi": "lp64d",
        "capabilities": ["c", "cxx", "libgcc", "libstdcxx", "libsupcxx", "standalone-collector"],
        "target": {
            "schema": "aros-riscv-target-v1",
            "isa": "rva22u64",
            "abi": "lp64d",
            "code_model": "medany",
            "architecture": "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0",
            "unaligned_access": false,
            "atomic_abi": 0,
            "x3_reg_usage": 0
        }
    });
    serde_json::to_vec(&json!({
        "schema": "aros-toolchain-profiles-v2",
        "family": "gnu",
        "upstream_commit": upstream_commit,
        "profiles": [rv32, rv32_alternate, rv64]
    }))
    .unwrap()
}

fn amend_configure_to_log_arguments(request: &mut NativeCompatibilityRequest) {
    let configure = request.upstream_source_root.join("configure");
    let source = fs::read_to_string(&configure).unwrap();
    let logged = source.replacen("#!/bin/sh\n", "#!/bin/sh\nprintf 'arg=<%s>\\n' \"$@\"\n", 1);
    assert_ne!(logged, source);
    fs::write(&configure, logged).unwrap();
    let mut add = Command::new("git");
    add.current_dir(&request.upstream_source_root)
        .args(["add", "configure"]);
    assert!(add.status().unwrap().success());
    let mut commit = Command::new("git");
    commit.current_dir(&request.upstream_source_root).args([
        "commit",
        "-qm",
        "test: log configure arguments",
    ]);
    assert!(commit.status().unwrap().success());
    let mut revision = Command::new("git");
    revision
        .current_dir(&request.upstream_source_root)
        .args(["rev-parse", "HEAD"]);
    let output = revision.output().unwrap();
    assert!(output.status.success());
    let commit = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    request.upstream_source_commit = GitObjectId::try_from(commit).unwrap();
}

pub fn riscv_elf(
    class: elf::Class,
    symbol: &str,
    target: &TargetContract,
    wrong_float_abi: bool,
) -> Vec<u8> {
    let flags = float_flags(target.abi());
    let flags = if wrong_float_abi {
        if flags == 0 {
            2
        } else {
            0
        }
    } else {
        flags
    };
    make_elf(class, symbol, target.architecture(), flags)
}

fn make_elf(class: elf::Class, symbol: &str, architecture: &str, flags: u32) -> Vec<u8> {
    let (header_size, section_size, symbol_size) = match class {
        elf::Class::Elf32 => (52_usize, 40_usize, 16_usize),
        elf::Class::Elf64 => (64_usize, 64_usize, 24_usize),
    };
    let section_count = 5_usize;
    let section_table_offset = header_size;
    let section_names = b"\0.shstrtab\0.riscv.attributes\0.strtab\0.symtab\0";
    let shstrtab_name = section_name_offset(section_names, ".shstrtab");
    let attributes_name = section_name_offset(section_names, ".riscv.attributes");
    let strtab_name = section_name_offset(section_names, ".strtab");
    let symtab_name = section_name_offset(section_names, ".symtab");
    let attributes = encode_riscv_attributes(architecture);
    let mut symbol_names = vec![0_u8];
    symbol_names.extend_from_slice(symbol.as_bytes());
    symbol_names.push(0);
    let symbol_table_size = symbol_size * 2;

    let data_offset = section_table_offset + section_count * section_size;
    let shstrtab_offset = data_offset;
    let attributes_offset = shstrtab_offset + section_names.len();
    let strtab_offset = attributes_offset + attributes.len();
    let symtab_offset = strtab_offset + symbol_names.len();
    let mut bytes = vec![0_u8; symtab_offset + symbol_table_size];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = if class == elf::Class::Elf32 { 1 } else { 2 };
    bytes[5] = 1;
    bytes[6] = 1;
    bytes[7] = OS_ABI_AROS;
    bytes[8] = AROS_ABI_VERSION;
    write_u16(&mut bytes, 0x10, 1);
    write_u16(&mut bytes, 0x12, elf::riscv::MACHINE);
    write_u32(&mut bytes, 0x14, 1);
    match class {
        elf::Class::Elf32 => {
            write_u32(&mut bytes, 0x20, section_table_offset as u32);
            write_u32(&mut bytes, 0x24, flags);
            write_u16(&mut bytes, 0x28, header_size as u16);
            write_u16(&mut bytes, 0x2e, section_size as u16);
            write_u16(&mut bytes, 0x30, section_count as u16);
            write_u16(&mut bytes, 0x32, 1);
        }
        elf::Class::Elf64 => {
            write_u64(&mut bytes, 0x28, section_table_offset as u64);
            write_u32(&mut bytes, 0x30, flags);
            write_u16(&mut bytes, 0x34, header_size as u16);
            write_u16(&mut bytes, 0x3a, section_size as u16);
            write_u16(&mut bytes, 0x3c, section_count as u16);
            write_u16(&mut bytes, 0x3e, 1);
        }
    }
    write_section(
        &mut bytes,
        class,
        section_table_offset + section_size,
        shstrtab_name,
        elf::SHT_STRTAB,
        shstrtab_offset,
        section_names.len(),
        0,
        0,
        1,
        0,
    );
    write_section(
        &mut bytes,
        class,
        section_table_offset + 2 * section_size,
        attributes_name,
        elf::riscv::SHT_ATTRIBUTES,
        attributes_offset,
        attributes.len(),
        0,
        0,
        1,
        0,
    );
    write_section(
        &mut bytes,
        class,
        section_table_offset + 3 * section_size,
        strtab_name,
        elf::SHT_STRTAB,
        strtab_offset,
        symbol_names.len(),
        0,
        0,
        1,
        0,
    );
    write_section(
        &mut bytes,
        class,
        section_table_offset + 4 * section_size,
        symtab_name,
        elf::SHT_SYMTAB,
        symtab_offset,
        symbol_table_size,
        3,
        1,
        if class == elf::Class::Elf32 { 4 } else { 8 },
        symbol_size,
    );

    let symbol_entry = symtab_offset + symbol_size;
    write_u32(&mut bytes, symbol_entry, 1);
    match class {
        elf::Class::Elf32 => {
            bytes[symbol_entry + 12] = 0x10;
            write_u16(&mut bytes, symbol_entry + 14, 2);
        }
        elf::Class::Elf64 => {
            bytes[symbol_entry + 4] = 0x10;
            write_u16(&mut bytes, symbol_entry + 6, 2);
        }
    }
    bytes[shstrtab_offset..attributes_offset].copy_from_slice(section_names);
    bytes[attributes_offset..strtab_offset].copy_from_slice(&attributes);
    bytes[strtab_offset..symtab_offset].copy_from_slice(&symbol_names);
    bytes
}

fn section_name_offset(names: &[u8], name: &str) -> u32 {
    let mut needle = name.as_bytes().to_vec();
    needle.push(0);
    u32::try_from(
        names
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("fixture section name exists"),
    )
    .unwrap()
}

fn encode_riscv_attributes(architecture: &str) -> Vec<u8> {
    let mut tags = vec![4, 16, 5];
    tags.extend_from_slice(architecture.as_bytes());
    tags.push(0);
    let file_size = u32::try_from(5 + tags.len()).unwrap();
    let vendor_size = 10 + file_size;
    let mut bytes = vec![b'A'];
    bytes.extend_from_slice(&vendor_size.to_le_bytes());
    bytes.extend_from_slice(b"riscv\0");
    bytes.push(1);
    bytes.extend_from_slice(&file_size.to_le_bytes());
    bytes.extend_from_slice(&tags);
    bytes
}

#[allow(clippy::too_many_arguments)]
fn write_section(
    bytes: &mut [u8],
    class: elf::Class,
    at: usize,
    name: u32,
    kind: u32,
    offset: usize,
    size: usize,
    link: u32,
    info: u32,
    align: usize,
    entry_size: usize,
) {
    write_u32(bytes, at, name);
    write_u32(bytes, at + 4, kind);
    match class {
        elf::Class::Elf32 => {
            write_u32(bytes, at + 16, offset as u32);
            write_u32(bytes, at + 20, size as u32);
            write_u32(bytes, at + 24, link);
            write_u32(bytes, at + 28, info);
            write_u32(bytes, at + 32, align as u32);
            write_u32(bytes, at + 36, entry_size as u32);
        }
        elf::Class::Elf64 => {
            write_u64(bytes, at + 24, offset as u64);
            write_u64(bytes, at + 32, size as u64);
            write_u32(bytes, at + 40, link);
            write_u32(bytes, at + 44, info);
            write_u64(bytes, at + 48, align as u64);
            write_u64(bytes, at + 56, entry_size as u64);
        }
    }
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn executable(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
