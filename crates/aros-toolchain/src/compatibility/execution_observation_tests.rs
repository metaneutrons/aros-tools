//! Synthetic coverage for measuring compatibility inputs before execution.
//!
//! These fixtures exercise the local adapter with declared test drivers and
//! generated ELF objects. They do not qualify a real compiler or authenticate
//! an external execution, source, environment, or release.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aros_common::{
    sha256_bytes, ArosCompilerIdentity, ArosToolchainManifest, CancellationToken, Sha256Digest,
};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::{execute_native_compatibility_with_export, NativeCompatibilityInputClaims};
use crate::compatibility::{
    readback_portable_native_compatibility, NativeCompatibilityExpectedPackage,
    PortableNativeCompatibilityRequest, PORTABLE_NATIVE_COMPATIBILITY_MANIFEST,
};
use crate::profiles::{Profile, Profiles};
use crate::recipe::GitObjectId;
use crate::ContractError;

#[derive(Debug, Clone)]
struct FixtureBytesIdentity {
    sha256: Sha256Digest,
    size: u64,
}

impl FixtureBytesIdentity {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            sha256: sha256_bytes(bytes),
            size: u64::try_from(bytes.len()).unwrap(),
        }
    }
}

#[derive(Debug, Clone)]
struct IndependentSelection {
    manifest: ArosToolchainManifest,
    archive_sha256: Sha256Digest,
    archive_size: u64,
    compiler: ArosCompilerIdentity,
    source_commit: GitObjectId,
    host: String,
    profiles: Profiles,
    profile: Profile,
}

impl IndependentSelection {
    fn from_fixture(fixture: &super::super::gnu_tests::Fixture) -> Self {
        let package = &fixture.request.relocation.second.verified;
        Self {
            manifest: package.manifest.clone(),
            archive_sha256: package.archive_sha256.clone(),
            archive_size: package.archive_size,
            compiler: package
                .manifest
                .compiler
                .as_ref()
                .expect("GNU fixture package has a compiler identity")
                .clone(),
            source_commit: fixture
                .request
                .package_source_commit
                .as_ref()
                .expect("GNU fixture has an independently selected package source commit")
                .clone(),
            host: fixture.request.host.clone(),
            profiles: fixture.profiles.clone(),
            profile: fixture.request.profile.clone(),
        }
    }

    fn package(&self) -> NativeCompatibilityExpectedPackage<'_> {
        NativeCompatibilityExpectedPackage {
            manifest: &self.manifest,
            archive_sha256: &self.archive_sha256,
            archive_size: self.archive_size,
            compiler: &self.compiler,
            source_commit: &self.source_commit,
            host: &self.host,
        }
    }
}

struct DetachedExecution {
    _download: TempDir,
    directory: PathBuf,
    original_root: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
    input_bytes: Vec<u8>,
    input_sha256: Sha256Digest,
    measurement_sha256: Sha256Digest,
    receipt_sha256: Sha256Digest,
    c_fixture: FixtureBytesIdentity,
    cxx_fixture: FixtureBytesIdentity,
    selected: IndependentSelection,
}

static GNU_RV32_EXECUTION: OnceLock<DetachedExecution> = OnceLock::new();

fn detached_gnu_rv32_execution() -> &'static DetachedExecution {
    GNU_RV32_EXECUTION.get_or_init(|| {
        let original = tempfile::tempdir().unwrap();
        let original_root = original.path().to_path_buf();
        let fixture = super::super::gnu_tests::gnu_fixture(original.path(), 32, false);
        let selected = IndependentSelection::from_fixture(&fixture);
        let c_fixture = FixtureBytesIdentity::from_bytes(
            &fs::read(&fixture.request.standalone_fixtures.c).unwrap(),
        );
        let cxx_fixture = FixtureBytesIdentity::from_bytes(
            &fs::read(&fixture.request.standalone_fixtures.cxx).unwrap(),
        );
        let exported = execute_native_compatibility_with_export(
            &fixture.request,
            &fixture.profiles,
            &CancellationToken::default(),
        )
        .unwrap();
        let files = exported
            .evidence()
            .files()
            .map(|(name, bytes)| (name.to_owned(), bytes.to_vec()))
            .collect();
        let input_bytes = exported.inputs().bytes().to_vec();
        let input_sha256 = exported.inputs().sha256().clone();
        let measurement_sha256 = exported.evidence().manifest_sha256().clone();
        let receipt_sha256 = exported.report().receipt.sha256.clone();

        drop(exported);
        drop(fixture);
        drop(original);
        assert!(
            !original_root.exists(),
            "the original execution and package roots must be gone"
        );

        let download = tempfile::tempdir().unwrap();
        let directory = download.path().join("compatibility-evidence");
        write_export(&directory, &files);
        let directory = directory.canonicalize().unwrap();
        DetachedExecution {
            _download: download,
            directory,
            original_root,
            files,
            input_bytes,
            input_sha256,
            measurement_sha256,
            receipt_sha256,
            c_fixture,
            cxx_fixture,
            selected,
        }
    })
}

#[test]
fn measured_inputs_and_export_rebind_rootlessly_to_independent_selection() {
    let fixture = detached_gnu_rv32_execution();
    assert!(!fixture.original_root.exists());
    assert!(fixture
        .files
        .contains_key(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST));

    let claims =
        NativeCompatibilityInputClaims::parse(&fixture.input_bytes, &fixture.input_sha256).unwrap();
    assert_eq!(claims.bytes(), fixture.input_bytes);
    assert_eq!(claims.sha256(), &fixture.input_sha256);
    let serialized: Value = serde_json::from_slice(claims.bytes()).unwrap();
    for (field, identity) in [
        ("standalone_c_fixture", &fixture.c_fixture),
        ("standalone_cxx_fixture", &fixture.cxx_fixture),
    ] {
        assert_eq!(
            serialized[field]["sha256"],
            serde_json::to_value(&identity.sha256).unwrap()
        );
        assert_eq!(serialized[field]["size"], json!(identity.size));
        assert_eq!(serialized[field].as_object().unwrap().len(), 2);
    }
    let expected = claims
        .expectations(
            fixture.selected.package(),
            &fixture.selected.profiles,
            &fixture.selected.profile,
            false,
        )
        .unwrap();
    let request = PortableNativeCompatibilityRequest {
        directory: fixture.directory.clone(),
        manifest_sha256: fixture.measurement_sha256.clone(),
        expected,
    };

    let before = snapshot_files(&fixture.directory);
    let readback = readback_portable_native_compatibility(&request).unwrap();
    assert_eq!(readback.manifest_sha256(), &fixture.measurement_sha256);
    assert_eq!(readback.receipt().receipt_sha256, fixture.receipt_sha256);
    assert_eq!(readback.receipt().phases.len(), 6);
    assert_eq!(readback.standalone().targets.len(), 1);
    assert_eq!(snapshot_files(&fixture.directory), before);
}

#[test]
fn input_claim_parser_rejects_unselected_or_noncanonical_documents() {
    let fixture = detached_gnu_rv32_execution();
    let bytes = &fixture.input_bytes;

    for member in ["standalone_c_fixture", "standalone_cxx_fixture"] {
        for size in [0, super::MAX_STANDALONE_FIXTURE_BYTES + 1] {
            let mut invalid: Value = serde_json::from_slice(bytes).unwrap();
            invalid[member]["size"] = json!(size);
            let invalid = crate::canonical::bytes(&invalid).unwrap();
            assert_parse_rejected(
                &invalid,
                &sha256_bytes(&invalid),
                "invalid standalone fixture size",
            );
        }
    }

    let wrong_digest = Sha256Digest::parse(&"0".repeat(64)).unwrap();
    assert_parse_rejected(bytes, &wrong_digest, "differs from its selected raw digest");

    let mut unknown: Value = serde_json::from_slice(bytes).unwrap();
    unknown["unselected_field"] = json!(true);
    let unknown = crate::canonical::bytes(&unknown).unwrap();
    assert_parse_rejected(&unknown, &sha256_bytes(&unknown), "is not closed JSON");

    let text = std::str::from_utf8(bytes).unwrap();
    let closing_brace = text.rfind('}').unwrap();
    let duplicated = format!(
        "{},\"schema\":\"aros-toolchain-compatibility-inputs-v1\"}}",
        &text[..closing_brace]
    )
    .into_bytes();
    assert_parse_rejected(
        &duplicated,
        &sha256_bytes(&duplicated),
        "malformed or has duplicate keys",
    );

    let mut noncanonical = b" ".to_vec();
    noncanonical.extend_from_slice(bytes);
    assert_parse_rejected(
        &noncanonical,
        &sha256_bytes(&noncanonical),
        "is not canonical JSON",
    );

    let mut unsupported: Value = serde_json::from_slice(bytes).unwrap();
    unsupported["schema"] = json!("aros-toolchain-compatibility-inputs-v0");
    let unsupported = crate::canonical::bytes(&unsupported).unwrap();
    assert_parse_rejected(
        &unsupported,
        &sha256_bytes(&unsupported),
        "has an unsupported schema",
    );
}

#[test]
fn input_claims_reject_independent_package_profile_and_host_substitutions() {
    let fixture = detached_gnu_rv32_execution();
    let original =
        NativeCompatibilityInputClaims::parse(&fixture.input_bytes, &fixture.input_sha256).unwrap();

    let mut wrong_package = fixture.selected.clone();
    wrong_package.archive_sha256 = Sha256Digest::parse(&"f".repeat(64)).unwrap();
    assert_expectations_rejected(
        &original,
        &wrong_package,
        &wrong_package.profile,
        "independently selected package or profiles",
    );

    let mut wrong_host = fixture.selected.clone();
    wrong_host.host = "other-fixture-host".into();
    assert_expectations_rejected(
        &original,
        &wrong_host,
        &wrong_host.profile,
        "independently selected package or profiles",
    );

    let wrong_profile = fixture.selected.profiles.select("rv64-compat").unwrap();
    assert_expectations_rejected(
        &original,
        &fixture.selected,
        wrong_profile,
        "independently selected package or profiles",
    );
    assert_eq!(original.bytes(), fixture.input_bytes);
}

#[test]
fn legacy_package_export_is_rejected_before_any_compatibility_phase() {
    let temporary = tempfile::tempdir().unwrap();
    let mut fixture = super::super::gnu_tests::gnu_fixture(temporary.path(), 32, false);
    fixture.request.relocation.second.verified.manifest.schema = 1;

    let error = execute_native_compatibility_with_export(
        &fixture.request,
        &fixture.profiles,
        &CancellationToken::default(),
    )
    .unwrap_err();
    assert_ax0703(&error, "requires a compiler-family-v2 package");
    assert!(!fixture.request.reports_root.exists());
    assert!(!fixture.request.cmake_build_root.exists());
    assert!(!fixture.request.upstream_build_root.exists());
    assert!(!fixture.request.standalone_output_root.exists());
    let cmake_log = fixture
        .request
        .cmake_program
        .parent()
        .unwrap()
        .join("cmake-arguments.log");
    assert!(!cmake_log.exists(), "the CMake child must not have started");
}

#[test]
fn fixture_changed_by_an_earlier_phase_prevents_export() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = super::super::gnu_tests::gnu_fixture(temporary.path(), 32, false);
    let c_fixture = fixture.request.standalone_fixtures.c.clone();
    let original_c_fixture = fs::read(&c_fixture).unwrap();
    let cmake = fixture.request.cmake_program.clone();
    let cmake_script = fs::read_to_string(&cmake).unwrap();
    let mutation = format!(
        "\nprintf '%s\\n' '/* changed after input measurement */' >> {}\n",
        shell_quote(&c_fixture.to_string_lossy())
    );
    fs::write(&cmake, format!("{cmake_script}{mutation}")).unwrap();

    let error = execute_native_compatibility_with_export(
        &fixture.request,
        &fixture.profiles,
        &CancellationToken::default(),
    )
    .unwrap_err();
    assert_ax0703(
        &error,
        "standalone compatibility fixture changed after input observation",
    );
    assert!(fixture.request.reports_root.exists());
    assert!(
        cmake
            .parent()
            .unwrap()
            .join("cmake-arguments.log")
            .is_file(),
        "the mutation must occur in an earlier synthetic phase"
    );
    assert_ne!(fs::read(&c_fixture).unwrap(), original_c_fixture);
}

fn write_export(directory: &Path, files: &BTreeMap<String, Vec<u8>>) {
    fs::create_dir(directory).unwrap();
    for (name, bytes) in files {
        assert_eq!(Path::new(name).file_name().unwrap(), name.as_str());
        fs::write(directory.join(name), bytes).unwrap();
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn snapshot_files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                PathBuf::from(path.file_name().unwrap()),
                fs::read(path).unwrap(),
            )
        })
        .collect()
}

fn assert_parse_rejected(bytes: &[u8], digest: &Sha256Digest, fragment: &str) {
    let before = bytes.to_vec();
    let error = NativeCompatibilityInputClaims::parse(bytes, digest).unwrap_err();
    assert_ax0703(&error, fragment);
    assert_eq!(bytes, before, "parsing must not mutate input bytes");
}

fn assert_expectations_rejected(
    claims: &NativeCompatibilityInputClaims,
    selected: &IndependentSelection,
    profile: &Profile,
    fragment: &str,
) {
    let before = claims.bytes().to_vec();
    let error = claims
        .expectations(selected.package(), &selected.profiles, profile, false)
        .unwrap_err();
    assert_ax0703(&error, fragment);
    assert_eq!(claims.bytes(), before);
}

fn assert_ax0703(error: &ContractError, expected_fragment: &str) {
    assert_eq!(
        error.diagnostics().diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility,
        "expected AX0703: {error}"
    );
    assert!(
        error.to_string().contains(expected_fragment),
        "expected {expected_fragment:?}, got {error}"
    );
}
