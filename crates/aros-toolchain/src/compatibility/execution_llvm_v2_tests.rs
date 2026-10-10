//! Synthetic regression coverage for LLVM compiler-family-v2 compatibility.
//!
//! The declared drivers copy generated ELF fixture bytes. These tests exercise
//! package binding, relocation checks, command selection, and ELF rejection;
//! they do not prove execution of a real LLVM compiler or its runtime.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use aros_common::{
    sha256_bytes, toolchain_tree_inventory, ArosCompilerIdentity, ArosToolchainManifest,
    CancellationToken,
};
use serde_json::{json, Value};

use super::{
    execute_native_compatibility, execute_native_compatibility_with_readback,
    readback_retained_native_compatibility,
    tests::{fixture_profiles, request},
    NativeCompatibilityRequest,
};
use crate::package_extract::ExtractedPackage;
use crate::package_verify::VerifiedPackage;
use crate::profiles::{Profile, Profiles};
use crate::recipe::GitObjectId;

const LLVM_VERSION: &str = "18.1.0";
const PACKAGE_SOURCE_COMMIT: &str = "e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7";
const LLVM_V2_RECEIPT_SCHEMA: &str = "aros-toolchain-native-compatibility-receipt-v4";

pub(super) struct Fixture {
    pub(super) request: NativeCompatibilityRequest,
    cmake_log: PathBuf,
    make_log: PathBuf,
}

#[test]
fn executes_all_llvm_v2_phases_for_x64_and_i386_and_binds_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = llvm_v2_fixture(temporary.path());

    let profiles = fixture_profiles(fixture.request.upstream_source_commit.as_str());
    let report = execute_native_compatibility_with_readback(
        &fixture.request,
        &profiles,
        &CancellationToken::default(),
    )
    .unwrap();

    assert_eq!(report.probes.reports.len(), 6);
    assert_eq!(report.standalone.targets.len(), 2);
    assert!(report
        .standalone
        .targets
        .contains_key("x86_64-unknown-aros"));
    assert!(report.standalone.targets.contains_key("i386-unknown-aros"));
    assert!(fixture.cmake_log.is_file());
    assert!(fixture.make_log.is_file());
    assert_eq!(
        fixture
            .request
            .package_source_commit
            .as_ref()
            .unwrap()
            .as_str(),
        PACKAGE_SOURCE_COMMIT
    );
    assert_ne!(
        fixture.request.package_source_commit.as_ref().unwrap(),
        &fixture.request.upstream_source_commit
    );
    assert_eq!(
        fixture
            .request
            .relocation
            .second
            .verified
            .manifest
            .source_commit,
        PACKAGE_SOURCE_COMMIT
    );

    let receipt: Value = serde_json::from_slice(&fs::read(&report.receipt.path).unwrap()).unwrap();
    assert_eq!(receipt["schema"], LLVM_V2_RECEIPT_SCHEMA);
    assert_eq!(receipt["phase_reports"].as_array().unwrap().len(), 6);
    assert_eq!(receipt["standalone_targets"].as_object().unwrap().len(), 2);
    assert_eq!(receipt["package"]["compiler"]["family"], "llvm");
    assert_eq!(receipt["package"]["compiler"]["version"], LLVM_VERSION);
    assert_eq!(receipt["package"]["host"], fixture.request.host);
    assert_eq!(
        receipt["package"]["target_profile"],
        fixture.request.profile.name()
    );
    assert_eq!(
        receipt["package"]["target_triple"],
        fixture.request.profile.target_triple()
    );
    assert_eq!(
        receipt["package"]["archive_sha256"],
        fixture
            .request
            .relocation
            .second
            .verified
            .archive_sha256
            .as_str()
    );
    assert_eq!(
        receipt["package"]["archive_size"],
        fixture.request.relocation.second.verified.archive_size
    );
    assert_eq!(
        receipt["package"]["manifest_sha256"],
        canonical_manifest_sha256(&fixture.request.relocation.second.verified.manifest)
    );
    assert_eq!(
        receipt["package"]["tree_sha256"],
        fixture
            .request
            .relocation
            .second
            .verified
            .manifest
            .tree_sha256
    );
    assert_eq!(
        receipt["package"]["source_tree_sha256"],
        fixture.request.preparation.source_tree_sha256.as_str()
    );
    assert!(receipt["package"].get("source_preset").is_none());
    assert_eq!(
        sha256_bytes(&fs::read(&report.receipt.path).unwrap()),
        report.receipt.sha256
    );

    let before_public_readback = super::retained_tests::snapshot_tree(temporary.path());
    let public_readback =
        readback_retained_native_compatibility(&fixture.request, &profiles).unwrap();
    assert_eq!(public_readback.receipt_sha256, report.receipt.sha256);
    assert_eq!(
        super::retained_tests::snapshot_tree(temporary.path()),
        before_public_readback,
        "public readback changed retained roots, command markers or logs"
    );
}

#[test]
fn public_retained_readback_rejects_legacy_package_without_creating_or_mutating_roots() {
    let temporary = tempfile::tempdir().unwrap();
    let (request, _, _) = request(temporary.path());
    assert_eq!(request.relocation.second.verified.manifest.schema, 1);
    let profiles = fixture_profiles(request.upstream_source_commit.as_str());
    let before = super::retained_tests::snapshot_tree(temporary.path());

    let error = readback_retained_native_compatibility(&request, &profiles).unwrap_err();
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility,
        "legacy package readback must fail with AX0703"
    );
    assert!(error
        .to_string()
        .contains("retained native compatibility read-back requires a compiler-family-v2 package"));
    for root in [
        &request.cmake_build_root,
        &request.upstream_build_root,
        &request.standalone_output_root,
        &request.reports_root,
    ] {
        assert!(!root.exists(), "legacy readback created {}", root.display());
    }
    assert_eq!(
        super::retained_tests::snapshot_tree(temporary.path()),
        before,
        "legacy readback changed fixture inputs"
    );
}

#[test]
fn rejects_upstream_source_identity_and_profiles_document_mutations_before_cmake() {
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = llvm_v2_fixture(temporary.path());
        fixture.request.upstream_source_commit = GitObjectId::try_from("f".repeat(40)).unwrap();
        let error = execute_native_compatibility(&fixture.request, &CancellationToken::default())
            .unwrap_err();
        assert!(error.to_string().contains(
            "pristine upstream source commit differs from the recipe-bound profiles matrix"
        ));
        assert!(!fixture.cmake_log.exists());
        assert!(!fixture.request.reports_root.exists());
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = llvm_v2_fixture(temporary.path());
        fixture.request.profile =
            profile_with_changed_document(fixture.request.upstream_source_commit.as_str());
        assert_rejected_before_cmake(&fixture, "changed profiles document");
    }
}

#[test]
fn rejects_missing_or_mismatched_expected_package_source_commit_before_cmake() {
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = llvm_v2_fixture(temporary.path());
        fixture.request.package_source_commit = None;
        assert_rejected_before_cmake_with_message(
            &fixture,
            "missing package source commit",
            "family-v2 compatibility requires the recipe-bound package source commit",
        );
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let mut fixture = llvm_v2_fixture(temporary.path());
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
fn rejects_payload_and_embedded_manifest_mutations_in_either_root_before_cmake() {
    for root_index in [0, 1] {
        for mutation in ["payload", "embedded manifest"] {
            let temporary = tempfile::tempdir().unwrap();
            let fixture = llvm_v2_fixture(temporary.path());
            let package_root = if root_index == 0 {
                &fixture.request.relocation.first.root
            } else {
                &fixture.request.relocation.second.root
            };
            if mutation == "payload" {
                fs::write(package_root.join("bin/clang"), b"changed measured driver\n").unwrap();
            } else {
                let manifest_path = package_root.join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE);
                let mut embedded: Value =
                    serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
                embedded["release_id"] = json!("changed-embedded-identity");
                fs::write(&manifest_path, serde_json::to_vec(&embedded).unwrap()).unwrap();
            }
            assert_rejected_before_cmake(
                &fixture,
                &format!("{mutation} mutation in root {root_index}"),
            );
        }
    }
}

#[test]
fn rejects_wrong_elf_machine_and_non_relocatable_final_output() {
    {
        let temporary = tempfile::tempdir().unwrap();
        let fixture = llvm_v2_fixture(temporary.path());
        let c_fixture = root_of(&fixture.request).join("c.elf");
        set_elf_header(&c_fixture, 3, 1);
        assert_rejected_after_probes(&fixture, "selected target's AROS ELF identity");
    }
    {
        let temporary = tempfile::tempdir().unwrap();
        let fixture = llvm_v2_fixture(temporary.path());
        let c_fixture = root_of(&fixture.request).join("c.elf");
        set_elf_header(&c_fixture, 62, 2);
        assert_rejected_after_probes(&fixture, "selected target's AROS ELF identity");
    }
}

#[test]
fn detects_package_payload_tampering_by_a_probe_before_writing_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = llvm_v2_fixture(temporary.path());
    let cmake_log = shell_quote(&fixture.cmake_log.to_string_lossy());
    let package_payload = shell_quote(
        &fixture
            .request
            .relocation
            .first
            .root
            .join("bin/clang")
            .to_string_lossy(),
    );
    script(
        &fixture.request.cmake_program,
        &format!(
            "printf '%s\\n' \"$@\" > {cmake_log}\nprintf '\\nprobe-tamper' >> {package_payload}"
        ),
    );

    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();

    assert!(error
        .to_string()
        .contains("family-v2 compatibility extracted package differs from its verified inventory"));
    assert!(fixture.cmake_log.is_file());
    assert!(fixture
        .request
        .reports_root
        .join("standalone-cxx.report.json")
        .is_file());
    assert!(!fixture
        .request
        .reports_root
        .join("native-compatibility.receipt.json")
        .exists());
}

pub(super) fn llvm_v2_fixture(root: &Path) -> Fixture {
    let (mut request, cmake_log, make_log) = request(root);
    patch_legacy_fixture_headers(&request);

    let compiler = ArosCompilerIdentity::Llvm {
        version: LLVM_VERSION.into(),
    };
    request.package_source_commit =
        Some(GitObjectId::try_from(PACKAGE_SOURCE_COMMIT.to_owned()).unwrap());
    let first_manifest =
        write_llvm_v2_package_root(&request.relocation.first.root, &request, &compiler);
    let second_manifest =
        write_llvm_v2_package_root(&request.relocation.second.root, &request, &compiler);
    assert_eq!(first_manifest, second_manifest);

    let archive_identity = b"synthetic fixture archive identity";
    let verified = VerifiedPackage {
        manifest: second_manifest,
        archive_sha256: sha256_bytes(archive_identity),
        archive_size: u64::try_from(archive_identity.len()).unwrap(),
    };
    request.relocation = crate::compatibility::TwoRootRelocation {
        first: ExtractedPackage {
            root: request.relocation.first.root.clone(),
            verified: verified.clone(),
        },
        second: ExtractedPackage {
            root: request.relocation.second.root.clone(),
            verified,
        },
    };

    Fixture {
        request,
        cmake_log,
        make_log,
    }
}

fn write_llvm_v2_package_root(
    root: &Path,
    request: &NativeCompatibilityRequest,
    compiler: &ArosCompilerIdentity,
) -> ArosToolchainManifest {
    // The inventory is measured from each complete extracted root; the
    // embedded manifest is written afterward to avoid a self-reference.
    let (tree_sha256, files) = toolchain_tree_inventory(root).unwrap();
    let manifest = ArosToolchainManifest {
        schema: 2,
        release_id: "synthetic-toolchain-v2-llvm-fixture".into(),
        host: request.host.clone(),
        target_profile: request.profile.name().into(),
        target_triple: request.profile.target_triple().into(),
        tree_sha256,
        llvm_version: None,
        compiler: Some(compiler.clone()),
        recipe_sha256: "a".repeat(64),
        source_lock_sha256: "b".repeat(64),
        profiles_sha256: request.profile.document_sha256().to_string(),
        source_commit: request
            .package_source_commit
            .as_ref()
            .unwrap()
            .as_str()
            .into(),
        producer_commit: "c".repeat(40),
        tools_commit: "d".repeat(40),
        source_date_epoch: 1,
        capabilities: request.profile.capabilities().to_vec(),
        build_environment: serde_json::Map::new(),
        files,
    };
    manifest.validate().unwrap();
    fs::write(
        root.join(aros_common::AROS_TOOLCHAIN_MANIFEST_FILE),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    manifest
}

fn patch_legacy_fixture_headers(request: &NativeCompatibilityRequest) {
    let root = root_of(request);
    for (filename, machine) in [
        ("c.elf", 62),
        ("cxx.elf", 62),
        ("c-i386.elf", 3),
        ("cxx-i386.elf", 3),
    ] {
        set_elf_header(&root.join(filename), machine, 1);
    }
}

fn set_elf_header(path: &Path, machine: u16, kind: u16) {
    let mut bytes = fs::read(path).unwrap();
    assert!(bytes.starts_with(b"\x7fELF"));
    write_u16(&mut bytes, 16, kind);
    write_u16(&mut bytes, 18, machine);
    fs::write(path, bytes).unwrap();
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn root_of(request: &NativeCompatibilityRequest) -> PathBuf {
    request
        .preparation
        .source_root
        .parent()
        .unwrap()
        .to_path_buf()
}

fn profile_with_changed_document(upstream_commit: &str) -> Profile {
    let profiles = Profiles::parse(
        &serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v1",
            "upstream_commit": upstream_commit,
            "profiles": [{
                "name": "pc-x86_64",
                "configure_target": "changed-configure-target",
                "upstream_output_target": "pc-x86_64",
                "target_triple": "x86_64-unknown-aros",
                "cpu": "x86_64",
                "platform": "pc",
                "float_abi": "",
                "capabilities": ["c", "cxx", "standalone-collector"]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    profiles.select("pc-x86_64").unwrap().clone()
}

fn assert_rejected_before_cmake(fixture: &Fixture, label: &str) {
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert!(
        error
            .diagnostics()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == aros_common::DiagnosticCode::ProducerCompatibility),
        "{label}: expected a producer-compatibility diagnostic, got {error}"
    );
    assert!(!fixture.cmake_log.exists(), "{label}: CMake started");
    assert!(
        !fixture.request.reports_root.exists(),
        "{label}: reports were created"
    );
}

fn assert_rejected_before_cmake_with_message(
    fixture: &Fixture,
    label: &str,
    expected_message: &str,
) {
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert!(error
        .diagnostics()
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == aros_common::DiagnosticCode::ProducerCompatibility),
        "{label}: expected a producer-compatibility diagnostic, got {error}");
    assert!(
        error.to_string().contains(expected_message),
        "{label}: {error}"
    );
    assert!(!fixture.cmake_log.exists(), "{label}: CMake started");
    assert!(
        !fixture.request.reports_root.exists(),
        "{label}: reports were created"
    );
}

fn assert_rejected_after_probes(fixture: &Fixture, message: &str) {
    let error =
        execute_native_compatibility(&fixture.request, &CancellationToken::default()).unwrap_err();
    assert!(
        error.to_string().contains(message),
        "unexpected error: {error}"
    );
    assert!(fixture.cmake_log.is_file());
    assert!(fixture
        .request
        .reports_root
        .join("standalone-cxx.report.json")
        .is_file());
    assert!(!fixture
        .request
        .reports_root
        .join("native-compatibility.receipt.json")
        .exists());
}

fn canonical_manifest_sha256(manifest: &ArosToolchainManifest) -> String {
    let value = serde_json::to_value(manifest).unwrap();
    let bytes = crate::canonical::bytes(&value).unwrap();
    sha256_bytes(&bytes).to_string()
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
