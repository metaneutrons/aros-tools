//! Synthetic rootless export/read-back coverage for retained native evidence.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aros_common::{sha256_bytes, ArosCompilerIdentity, ArosToolchainManifest, Sha256Digest};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::retained::ExpectedRetainedEvidence;
use super::{
    export_retained_native_compatibility, readback_portable_native_compatibility,
    NativeCompatibilityReceiptExpectations, PortableNativeCompatibilityRequest,
    PORTABLE_NATIVE_COMPATIBILITY_MANIFEST,
};
use crate::compatibility::{
    execute_native_compatibility_with_readback, CompatibilityHelperReport,
    CompatibilityHostToolReport, NativeCompatibilityExpectedPortSource,
};
use crate::profiles::{Profile, Profiles};
use crate::recipe::GitObjectId;

#[derive(Debug, Clone)]
struct OwnedExpectations {
    manifest: ArosToolchainManifest,
    archive_sha256: Sha256Digest,
    archive_size: u64,
    compiler: ArosCompilerIdentity,
    package_source_commit: GitObjectId,
    host: String,
    profiles: Profiles,
    profile: Profile,
    gnu_source_preset: Option<String>,
    cmake_build_required: bool,
    sdk_consumer_source_tree_sha256: Sha256Digest,
    engine_api_version: u32,
    engine_sha256: Sha256Digest,
    helpers: BTreeMap<String, CompatibilityHelperReport>,
    host_tools: BTreeMap<String, CompatibilityHostToolReport>,
    sdk_environment_sha256: Sha256Digest,
    standalone_environment_sha256: Sha256Digest,
    upstream_source_commit: GitObjectId,
    upstream_source_tree: GitObjectId,
    ports_sources: Vec<NativeCompatibilityExpectedPortSource>,
}

impl OwnedExpectations {
    fn copy_from(value: &NativeCompatibilityReceiptExpectations<'_>) -> Self {
        Self {
            manifest: value.package.manifest.clone(),
            archive_sha256: value.package.archive_sha256.clone(),
            archive_size: value.package.archive_size,
            compiler: value.package.compiler.clone(),
            package_source_commit: value.package.source_commit.clone(),
            host: value.package.host.to_owned(),
            profiles: value.profiles.clone(),
            profile: value.profile.clone(),
            gnu_source_preset: value.gnu_source_preset.map(str::to_owned),
            cmake_build_required: value.cmake_build_required,
            sdk_consumer_source_tree_sha256: value.sdk_consumer_source_tree_sha256.clone(),
            engine_api_version: value.engine_api_version,
            engine_sha256: value.engine_sha256.clone(),
            helpers: value.helpers.clone(),
            host_tools: value.host_tools.clone(),
            sdk_environment_sha256: value.sdk_environment_sha256.clone(),
            standalone_environment_sha256: value.standalone_environment_sha256.clone(),
            upstream_source_commit: value.upstream_source_commit.clone(),
            upstream_source_tree: value.upstream_source_tree.clone(),
            ports_sources: value.ports_sources.to_vec(),
        }
    }

    fn request(&self) -> NativeCompatibilityReceiptExpectations<'_> {
        NativeCompatibilityReceiptExpectations {
            package: super::NativeCompatibilityExpectedPackage {
                manifest: &self.manifest,
                archive_sha256: &self.archive_sha256,
                archive_size: self.archive_size,
                compiler: &self.compiler,
                source_commit: &self.package_source_commit,
                host: &self.host,
            },
            profiles: &self.profiles,
            profile: &self.profile,
            gnu_source_preset: self.gnu_source_preset.as_deref(),
            cmake_build_required: self.cmake_build_required,
            sdk_consumer_source_tree_sha256: &self.sdk_consumer_source_tree_sha256,
            engine_api_version: self.engine_api_version,
            engine_sha256: &self.engine_sha256,
            helpers: &self.helpers,
            host_tools: &self.host_tools,
            sdk_environment_sha256: &self.sdk_environment_sha256,
            standalone_environment_sha256: &self.standalone_environment_sha256,
            upstream_source_commit: &self.upstream_source_commit,
            upstream_source_tree: &self.upstream_source_tree,
            ports_sources: &self.ports_sources,
        }
    }
}

struct ExportedFixture {
    _download_root: TempDir,
    directory: PathBuf,
    original_root: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
    manifest_sha256: Sha256Digest,
    receipt_sha256: Sha256Digest,
    expected: OwnedExpectations,
}

#[derive(Debug, PartialEq, Eq)]
enum SnapshotEntry {
    Regular {
        size: u64,
        sha256: Sha256Digest,
        links: u64,
        mode: u32,
    },
    Symlink(PathBuf),
}

fn make_exported_fixture(
    original_root: &TempDir,
    request: &super::NativeCompatibilityRequest,
    profiles: &Profiles,
) -> ExportedFixture {
    let expected = {
        let retained = ExpectedRetainedEvidence::prepare(request, profiles).unwrap();
        let selected = retained.portable_expectations(request, profiles).unwrap();
        OwnedExpectations::copy_from(&selected)
    };

    let execution = execute_native_compatibility_with_readback(
        request,
        profiles,
        &aros_common::CancellationToken::default(),
    )
    .unwrap();
    let exported = export_retained_native_compatibility(request, profiles).unwrap();
    assert_eq!(exported.receipt_sha256(), &execution.receipt.sha256);
    let files = exported
        .files()
        .map(|(name, bytes)| (name.to_owned(), bytes.to_vec()))
        .collect::<BTreeMap<_, _>>();
    let manifest_sha256 = exported.manifest_sha256().clone();
    let receipt_sha256 = exported.receipt_sha256().clone();

    let download_root = tempfile::tempdir().unwrap();
    let directory = download_root.path().join("evidence");
    fs::create_dir(&directory).unwrap();
    for (name, bytes) in &files {
        fs::write(directory.join(name), bytes).unwrap();
    }
    let directory = directory.canonicalize().unwrap();
    let original_root = original_root.path().to_path_buf();

    ExportedFixture {
        _download_root: download_root,
        directory,
        original_root,
        files,
        manifest_sha256,
        receipt_sha256,
        expected,
    }
}

fn make_gnu_export(width: u8) -> ExportedFixture {
    let original_root = tempfile::tempdir().unwrap();
    let fixture = super::gnu_tests::gnu_fixture(original_root.path(), width, false);
    let exported = make_exported_fixture(&original_root, &fixture.request, &fixture.profiles);
    drop(fixture);
    drop(original_root);
    assert!(!exported.original_root.exists());
    exported
}

fn make_gnu_consumer_build_export() -> ExportedFixture {
    let original_root = tempfile::tempdir().unwrap();
    let mut fixture = super::gnu_tests::gnu_fixture(original_root.path(), 32, false);
    let producer_profile = fixture.request.profile.clone();
    super::gnu_tests::write_native_consumer_contract_fixture(
        &mut fixture.request,
        &producer_profile,
        false,
    );
    let exported = make_exported_fixture(&original_root, &fixture.request, &fixture.profiles);
    drop(fixture);
    drop(original_root);
    assert!(!exported.original_root.exists());
    exported
}

fn make_llvm_export() -> ExportedFixture {
    let original_root = tempfile::tempdir().unwrap();
    let fixture = super::llvm_v2_tests::llvm_v2_fixture(original_root.path());
    let profiles = super::tests::fixture_profiles(fixture.request.upstream_source_commit.as_str());
    let exported = make_exported_fixture(&original_root, &fixture.request, &profiles);
    drop(fixture);
    drop(profiles);
    drop(original_root);
    assert!(!exported.original_root.exists());
    exported
}

static GNU_RV32_EXPORT: OnceLock<ExportedFixture> = OnceLock::new();

fn gnu_rv32_export() -> &'static ExportedFixture {
    GNU_RV32_EXPORT.get_or_init(|| make_gnu_export(32))
}

fn write_export(directory: &Path, files: &BTreeMap<String, Vec<u8>>) {
    fs::create_dir(directory).unwrap();
    for (name, bytes) in files {
        assert_eq!(Path::new(name).file_name().unwrap(), name.as_str());
        fs::write(directory.join(name), bytes).unwrap();
    }
}

fn request_for<'a>(
    directory: &Path,
    manifest_sha256: Sha256Digest,
    expected: &'a OwnedExpectations,
) -> PortableNativeCompatibilityRequest<'a> {
    PortableNativeCompatibilityRequest {
        directory: directory.to_path_buf(),
        manifest_sha256,
        expected: expected.request(),
    }
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, SnapshotEntry> {
    let mut output = BTreeMap::new();
    let metadata = fs::symlink_metadata(root).unwrap();
    if metadata.file_type().is_symlink() {
        output.insert(
            root.to_path_buf(),
            SnapshotEntry::Symlink(fs::read_link(root).unwrap()),
        );
        return output;
    }
    snapshot_directory(root, &mut output);
    output
}

fn snapshot_directory(directory: &Path, output: &mut BTreeMap<PathBuf, SnapshotEntry>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            snapshot_directory(&path, output);
        } else if kind.is_symlink() {
            output.insert(
                path.clone(),
                SnapshotEntry::Symlink(fs::read_link(path).unwrap()),
            );
        } else {
            let metadata = fs::symlink_metadata(&path).unwrap();
            let measured = aros_common::sha256_file(&path).unwrap();
            output.insert(
                path,
                SnapshotEntry::Regular {
                    size: measured.size,
                    sha256: measured.digest,
                    links: metadata.nlink(),
                    mode: metadata.mode() & 0o777,
                },
            );
        }
    }
}

fn assert_ax0703(error: &crate::ContractError, expected_fragment: &str) {
    let diagnostics = &error.diagnostics().diagnostics;
    assert_eq!(
        diagnostics[0].code,
        aros_common::DiagnosticCode::ProducerCompatibility,
        "expected AX0703: {error}"
    );
    assert!(
        error.to_string().contains(expected_fragment),
        "expected {expected_fragment:?}, got {error}"
    );
}

fn assert_rejected_without_mutation(
    fixture: &ExportedFixture,
    label: &str,
    expected_fragment: &str,
    mutate: impl FnOnce(&Path, &mut OwnedExpectations, &mut Sha256Digest),
) {
    let owner = tempfile::tempdir().unwrap();
    let directory = owner.path().join("evidence");
    write_export(&directory, &fixture.files);
    let directory = directory.canonicalize().unwrap();
    let mut expected = fixture.expected.clone();
    let mut manifest_sha256 = fixture.manifest_sha256.clone();
    mutate(&directory, &mut expected, &mut manifest_sha256);
    let request = request_for(&directory, manifest_sha256, &expected);
    let before = snapshot_tree(&directory);
    let error = readback_portable_native_compatibility(&request).unwrap_err();
    assert_ax0703(&error, expected_fragment);
    assert_eq!(
        snapshot_tree(&directory),
        before,
        "{label} changed downloaded evidence"
    );
}

fn rewrite_manifest(directory: &Path, manifest: &Value, selected_digest: &mut Sha256Digest) {
    let bytes = crate::canonical::bytes(manifest).unwrap();
    fs::write(
        directory.join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST),
        &bytes,
    )
    .unwrap();
    *selected_digest = sha256_bytes(&bytes);
}

fn manifest_value(directory: &Path) -> Value {
    serde_json::from_slice(
        &fs::read(directory.join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST)).unwrap(),
    )
    .unwrap()
}

fn reseal_member(directory: &Path, name: &str, bytes: &[u8], selected_digest: &mut Sha256Digest) {
    fs::write(directory.join(name), bytes).unwrap();
    let mut manifest = manifest_value(directory);
    manifest["files"][name]["sha256"] = json!(sha256_bytes(bytes));
    manifest["files"][name]["size"] = json!(bytes.len() as u64);
    rewrite_manifest(directory, &manifest, selected_digest);
}

#[test]
fn portable_export_roundtrips_rootless_gnu_and_llvm_elf_inventories() {
    let fixtures = [
        make_gnu_export(32),
        make_gnu_export(64),
        make_gnu_consumer_build_export(),
        make_llvm_export(),
    ];
    for fixture in fixtures {
        assert!(!fixture.original_root.exists());
        assert!(fixture
            .files
            .contains_key(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST));
        assert!(fixture
            .files
            .contains_key("native-compatibility.receipt.json"));
        assert!(fixture
            .files
            .keys()
            .any(|name| name.ends_with(".report.json")));
        assert!(fixture
            .files
            .keys()
            .any(|name| name.ends_with(".stdout.log")));
        assert!(fixture
            .files
            .keys()
            .any(|name| name.ends_with(".stderr.log")));
        assert!(fixture.files.keys().any(|name| Path::new(name)
            .extension()
            .is_some_and(|extension| extension == "o")));
        if fixture.expected.cmake_build_required {
            assert!(fixture.files.contains_key("cmake-consumer.1.stdout.log"));
            assert!(fixture.files.contains_key("cmake-consumer.2.stdout.log"));
        }

        let request = request_for(
            &fixture.directory,
            fixture.manifest_sha256.clone(),
            &fixture.expected,
        );
        let before = snapshot_tree(&fixture.directory);
        let readback = readback_portable_native_compatibility(&request).unwrap();
        assert_eq!(readback.manifest_sha256(), &fixture.manifest_sha256);
        assert_eq!(readback.receipt().receipt_sha256, fixture.receipt_sha256);
        assert_eq!(readback.receipt().phases.len(), 6);
        let expected_target_count = if matches!(
            &fixture.expected.compiler,
            ArosCompilerIdentity::Llvm { .. }
        ) && fixture.expected.profile.name() == "pc-x86_64"
        {
            2
        } else {
            1
        };
        assert_eq!(readback.standalone().targets.len(), expected_target_count);
        assert_eq!(snapshot_tree(&fixture.directory), before);
    }
}

#[test]
fn rejects_wrong_raw_manifest_and_closed_inventory_mutations() {
    let fixture = gnu_rv32_export();

    assert_rejected_without_mutation(
        fixture,
        "wrong externally selected raw manifest digest",
        "portable compatibility manifest differs from its selected raw digest",
        |_, _, digest| *digest = sha256_bytes(b"wrong selected manifest"),
    );
    assert_rejected_without_mutation(
        fixture,
        "unknown manifest field",
        "portable compatibility manifest is not closed JSON",
        |root, _, digest| {
            let mut manifest = manifest_value(root);
            manifest["unselected_field"] = json!(true);
            rewrite_manifest(root, &manifest, digest);
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "missing manifest member",
        "portable compatibility manifest differs from its canonical closed inventory",
        |root, _, digest| {
            let mut manifest = manifest_value(root);
            manifest["files"]
                .as_object_mut()
                .unwrap()
                .remove("cmake-consumer.report.json");
            rewrite_manifest(root, &manifest, digest);
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "extra manifest member",
        "portable compatibility manifest differs from its canonical closed inventory",
        |root, _, digest| {
            let mut manifest = manifest_value(root);
            manifest["files"]["unexpected.bin"] = json!({"sha256": sha256_bytes(b"x"), "size": 1});
            rewrite_manifest(root, &manifest, digest);
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "noncanonical manifest bytes",
        "portable compatibility manifest differs from its canonical closed inventory",
        |root, _, digest| {
            let path = root.join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST);
            let original = fs::read(&path).unwrap();
            let mut bytes = b" ".to_vec();
            bytes.extend_from_slice(&original);
            fs::write(path, &bytes).unwrap();
            *digest = sha256_bytes(&bytes);
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "duplicate manifest field",
        "native compatibility metadata is malformed or has duplicate keys",
        |root, _, digest| {
            let manifest = manifest_value(root);
            let schema = serde_json::to_string(&manifest["schema"]).unwrap();
            let files = serde_json::to_string(&manifest["files"]).unwrap();
            let bytes = format!("{{\"files\":{files},\"schema\":{schema},\"schema\":{schema}}}");
            fs::write(root.join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST), &bytes).unwrap();
            *digest = sha256_bytes(bytes.as_bytes());
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "extra downloaded member",
        "portable compatibility directory differs from its exact inventory",
        |root, _, _| fs::write(root.join("unexpected.data"), b"unexpected\n").unwrap(),
    );
    assert_rejected_without_mutation(
        fixture,
        "missing downloaded member",
        "portable compatibility directory differs from its exact inventory",
        |root, _, _| fs::remove_file(root.join("cmake-consumer.report.json")).unwrap(),
    );
    assert_rejected_without_mutation(
        fixture,
        "missing downloaded command log",
        "portable compatibility directory differs from its exact inventory",
        |root, _, _| fs::remove_file(root.join("cmake-consumer.stdout.log")).unwrap(),
    );
    assert_rejected_without_mutation(
        fixture,
        "missing downloaded standalone ELF",
        "portable compatibility directory differs from its exact inventory",
        |root, _, _| fs::remove_file(root.join("c-riscv.o")).unwrap(),
    );
}

#[test]
fn rejects_member_changes_aliases_and_bounds_without_mutation() {
    let fixture = gnu_rv32_export();

    assert_rejected_without_mutation(
        fixture,
        "changed report bytes without remeasurement",
        "portable compatibility member differs from its selected manifest",
        |root, _, _| {
            fs::write(root.join("cmake-consumer.report.json"), b"changed report\n").unwrap();
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "changed retained log with a resealed outer manifest",
        "native compatibility retained command log bytes differ from their report hashes",
        |root, _, digest| {
            reseal_member(
                root,
                "cmake-consumer.stdout.log",
                b"changed retained log\n",
                digest,
            );
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "invalid ELF with a resealed outer manifest",
        "standalone output is not a supported little-endian ELF object",
        |root, _, digest| reseal_member(root, "c-riscv.o", b"not an ELF object", digest),
    );
    assert_rejected_without_mutation(
        fixture,
        "leaf symlink",
        "cannot safely open portable compatibility member",
        |root, _, _| {
            let member = root.join("cmake-consumer.report.json");
            let target = root.join("upstream-configure.report.json");
            fs::remove_file(&member).unwrap();
            symlink(target, member).unwrap();
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "hard-linked evidence aliases",
        "portable compatibility member is oversized, non-regular or physically aliased",
        |root, _, _| {
            let source = root.join("cmake-consumer.stdout.log");
            let target = root.join("cmake-consumer.stderr.log");
            fs::remove_file(&target).unwrap();
            fs::hard_link(source, target).unwrap();
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "oversized metadata member",
        "portable compatibility member is oversized, non-regular or physically aliased",
        |root, _, _| {
            fs::write(
                root.join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST),
                vec![b' '; crate::canonical::MAX_DOCUMENT_BYTES + 1],
            )
            .unwrap();
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "oversized retained log",
        "portable compatibility member is oversized, non-regular or physically aliased",
        |root, _, _| {
            fs::write(
                root.join("cmake-consumer.stdout.log"),
                vec![b'x'; 256 * 1024 + 257],
            )
            .unwrap();
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "oversized sparse ELF",
        "portable compatibility member is oversized, non-regular or physically aliased",
        |root, _, _| {
            let path = root.join("c-riscv.o");
            let file = fs::OpenOptions::new().write(true).open(path).unwrap();
            file.set_len(128 * 1024 * 1024 + 1).unwrap();
        },
    );
}

#[test]
fn rejects_independent_package_profile_engine_source_environment_and_ports_changes() {
    let fixture = gnu_rv32_export();

    assert_rejected_without_mutation(
        fixture,
        "wrong independent package source commit",
        "expected package compiler, host, or package source commit differs from its manifest",
        |_, expected, _| {
            expected.package_source_commit = GitObjectId::try_from("f".repeat(40)).unwrap();
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong independent package archive digest",
        "native compatibility receipt package, profile, preset, or SDK source binding differs from expected inputs",
        |_, expected, _| expected.archive_sha256 = sha256_bytes(b"other archive"),
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong independent compiler",
        "expected package compiler, host, or package source commit differs from its manifest",
        |_, expected, _| match &mut expected.compiler {
            ArosCompilerIdentity::Gnu { gcc_version, .. } => *gcc_version = "17.1.0".into(),
            ArosCompilerIdentity::Llvm { version } => *version = "17.1.0".into(),
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong independent host",
        "expected package compiler, host, or package source commit differs from its manifest",
        |_, expected, _| expected.host = "linux-aarch64".to_owned(),
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong selected profile",
        "portable compatibility directory differs from its exact inventory",
        |_, expected, _| {
            expected.profile = expected.profiles.select("rv64-compat").unwrap().clone();
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong upstream source tree",
        "native compatibility receipt upstream commit or tree differs from expected inputs",
        |_, expected, _| {
            expected.upstream_source_tree = GitObjectId::try_from("f".repeat(40)).unwrap();
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong independent SDK source tree",
        "native compatibility receipt package, profile, preset, or SDK source binding differs from expected inputs",
        |_, expected, _| expected.sdk_consumer_source_tree_sha256 = sha256_bytes(b"other SDK source"),
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong engine identity",
        "native compatibility phase mixes engine, helper, or SDK source identities",
        |_, expected, _| expected.engine_sha256 = sha256_bytes(b"other engine"),
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong helper identity",
        "native compatibility phase mixes engine, helper, or SDK source identities",
        |_, expected, _| {
            expected.helpers.values_mut().next().unwrap().sha256 = sha256_bytes(b"other helper");
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong host-tool identity",
        "native compatibility SDK phase host-tool or environment identity differs from independent expectation",
        |_, expected, _| expected.host_tools.values_mut().next().unwrap().sha256 = sha256_bytes(b"other host tool"),
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong SDK environment identity",
        "native compatibility SDK phase host-tool or environment identity differs from independent expectation",
        |_, expected, _| expected.sdk_environment_sha256 = sha256_bytes(b"other SDK environment"),
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong standalone environment identity",
        "native compatibility standalone environment identity differs from independent expectation",
        |_, expected, _| {
            expected.standalone_environment_sha256 = sha256_bytes(b"other standalone environment");
        },
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong locked ports closure",
        "native compatibility receipt ports closure differs from the exact locked payload closure",
        |_, expected, _| expected.ports_sources[0].sha256 = sha256_bytes(b"other ports source"),
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong GNU source preset",
        "native compatibility receipt package, profile, preset, or SDK source binding differs from expected inputs",
        |_, expected, _| expected.gnu_source_preset = Some("other-source-preset".to_owned()),
    );
    assert_rejected_without_mutation(
        fixture,
        "wrong CMake build requirement",
        "portable compatibility directory differs from its exact inventory",
        |_, expected, _| expected.cmake_build_required = true,
    );
}

#[test]
fn rejects_symlinked_root_and_revalidates_local_factory_inputs() {
    let fixture = gnu_rv32_export();
    let owner = tempfile::tempdir().unwrap();
    let expected = fixture.expected.clone();
    let before = snapshot_tree(&fixture.directory);
    let alias = owner.path().join("root-link");
    symlink(&fixture.directory, &alias).unwrap();
    let request = request_for(&alias, fixture.manifest_sha256.clone(), &expected);
    let error = readback_portable_native_compatibility(&request).unwrap_err();
    assert_ax0703(
        &error,
        "portable compatibility root must be an absolute real directory without links",
    );
    assert_eq!(
        snapshot_tree(&fixture.directory),
        before,
        "symlink-root failure mutated files"
    );

    let parent_alias = owner.path().join("parent-link");
    symlink(fixture.directory.parent().unwrap(), &parent_alias).unwrap();
    let linked_child = parent_alias.join(fixture.directory.file_name().unwrap());
    let request = request_for(&linked_child, fixture.manifest_sha256.clone(), &expected);
    let error = readback_portable_native_compatibility(&request).unwrap_err();
    assert_ax0703(
        &error,
        "portable compatibility root must be an absolute real directory without links",
    );
    assert_eq!(
        snapshot_tree(&fixture.directory),
        before,
        "parent-symlink failure mutated files"
    );

    let original = tempfile::tempdir().unwrap();
    let local = super::gnu_tests::gnu_fixture(original.path(), 32, false);
    execute_native_compatibility_with_readback(
        &local.request,
        &local.profiles,
        &aros_common::CancellationToken::default(),
    )
    .unwrap();

    let phase = local.request.reports_root.join("standalone-c.report.json");
    let saved_report = fs::read(&phase).unwrap();
    fs::remove_file(&phase).unwrap();
    assert_local_export_rejected(
        original.path(),
        &local.request,
        &local.profiles,
        "retained compatibility phase report",
    );
    fs::write(&phase, saved_report).unwrap();

    let elf = local.request.standalone_output_root.join("c-riscv.o");
    let saved_elf = fs::read(&elf).unwrap();
    fs::write(&elf, b"mutated standalone ELF").unwrap();
    assert_local_export_rejected(
        original.path(),
        &local.request,
        &local.profiles,
        "standalone output is not a supported little-endian ELF object",
    );
    fs::write(&elf, saved_elf).unwrap();

    let report = local
        .request
        .reports_root
        .join("cmake-consumer.report.json");
    let alias = original.path().join("report-hard-link.json");
    fs::hard_link(&report, &alias).unwrap();
    assert_local_export_rejected(
        original.path(),
        &local.request,
        &local.profiles,
        "physically aliased",
    );
    fs::remove_file(alias).unwrap();
}

fn assert_local_export_rejected(
    fixture_root: &Path,
    request: &super::NativeCompatibilityRequest,
    profiles: &Profiles,
    expected_fragment: &str,
) {
    let before = snapshot_tree(fixture_root);
    let error = export_retained_native_compatibility(request, profiles).unwrap_err();
    assert_ax0703(&error, expected_fragment);
    assert_eq!(snapshot_tree(fixture_root), before);
}
