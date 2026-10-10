//! Synthetic three-host indexed build/compatibility joins. Reports are test
//! documents; no compatibility command or compiler is executed.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, ArosCompilerIdentity, Sha256Digest};
use serde_json::json;
use tempfile::TempDir;

use super::release_readback_tests::{prepare_release, PreparedRelease};
use super::{
    readback_release_builds_v2, readback_release_compatibility_v2, ReleaseBuildReadbackRequestV2,
    ReleaseCompatibilityReadbackRequestV2,
};
use crate::compatibility::receipt_readback_tests::Fixture as ReceiptFixture;
use crate::compatibility::{
    native_compatibility_host_tools, verify_standalone_outputs_with_compilers,
    CompatibilityHostToolReport, CompatibilityPhase, NativeCompatibilityReceiptExpectations,
    PortableNativeCompatibilityRequest, StandaloneOutputRequest, StandaloneTargetArtifacts,
    PORTABLE_NATIVE_COMPATIBILITY_MANIFEST,
};
use crate::package_verify::VerifiedPackage;
use crate::recipe::GitObjectId;
use crate::release_index_v2::NativeReleaseArtifactV2;

const COMPATIBILITY_RECEIPT: &str = "native-compatibility.receipt.json";
const REQUIRED_PHASES: [CompatibilityPhase; 6] = [
    CompatibilityPhase::CmakeConsumer,
    CompatibilityPhase::UpstreamConfigure,
    CompatibilityPhase::UpstreamIncludes,
    CompatibilityPhase::UpstreamLinklibs,
    CompatibilityPhase::StandaloneC,
    CompatibilityPhase::StandaloneCxx,
];

struct LaneEvidence {
    fixture: ReceiptFixture,
    directory: PathBuf,
    manifest_sha256: Sha256Digest,
}

struct PreparedCompatibility {
    _root: TempDir,
    lanes: BTreeMap<String, LaneEvidence>,
}

#[derive(Default)]
struct LaneOverride<'a> {
    package_from: Option<&'a ReceiptFixture>,
    host: Option<&'a str>,
    profiles: Option<(&'a crate::profiles::Profiles, &'a crate::profiles::Profile)>,
}

fn prepare_compatibility(
    prepared: &PreparedRelease,
    proof: &super::ReleaseBuildReadbackV2,
) -> PreparedCompatibility {
    let root = tempfile::tempdir().unwrap();
    let mut lanes = BTreeMap::new();
    for artifact in prepared.packages.index.artifacts() {
        let build = proof
            .lanes()
            .iter()
            .find(|lane| lane.asset() == artifact.asset())
            .unwrap();
        let group = prepared
            .packages
            .inputs
            .groups()
            .iter()
            .find(|group| group.id() == artifact.group_id())
            .unwrap();
        let profile = group
            .profiles()
            .select(artifact.target_profile())
            .unwrap()
            .clone();
        let package = build.verified_package();
        let directory = root.path().join(artifact.asset());
        fs::create_dir(&directory).unwrap();

        let mut fixture = ReceiptFixture::with_cmake_build_required(false, false);
        bind_fixture_to_indexed_package(
            &mut fixture,
            package,
            artifact,
            group.profiles(),
            profile,
            group.recipe().source().1,
        );
        let standalone = write_and_verify_standalone(&directory, &fixture.compiler);
        fixture.standalone = standalone;
        update_reports_for_selected_host(&mut fixture);
        fixture.rebuild_receipt();

        let mut files = fixture_files(&fixture, &directory);
        let manifest = compatibility_manifest(&files);
        let manifest_sha256 = sha256_bytes(&manifest);
        files.insert(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST.into(), manifest);
        for (name, bytes) in &files {
            fs::write(directory.join(name), bytes).unwrap();
        }
        lanes.insert(
            artifact.asset().to_owned(),
            LaneEvidence {
                fixture,
                directory: directory.canonicalize().unwrap(),
                manifest_sha256,
            },
        );
    }
    PreparedCompatibility { _root: root, lanes }
}

fn bind_fixture_to_indexed_package(
    fixture: &mut ReceiptFixture,
    package: &VerifiedPackage,
    artifact: &NativeReleaseArtifactV2,
    profiles: &crate::profiles::Profiles,
    profile: crate::profiles::Profile,
    upstream_tree: &GitObjectId,
) {
    fixture.manifest = package.manifest.clone();
    fixture.archive_sha256 = package.archive_sha256.clone();
    fixture.archive_size = package.archive_size;
    fixture.compiler = package.manifest.compiler_identity().unwrap();
    fixture.package_source_commit =
        GitObjectId::try_from(package.manifest.source_commit.clone()).unwrap();
    fixture.host = artifact.host().to_owned();
    fixture.profiles = profiles.clone();
    fixture.profile = profile;
    fixture.source_preset = None;
    fixture.cmake_build_required = false;
    fixture.upstream_commit = profiles.upstream_commit().clone();
    fixture.upstream_tree = upstream_tree.clone();
}

fn write_and_verify_standalone(
    directory: &Path,
    compiler: &ArosCompilerIdentity,
) -> crate::compatibility::StandaloneOutputReport {
    let objects = [
        (
            "x86_64-unknown-aros",
            "c-x86_64.o",
            "cxx-x86_64.o",
            fixture_elf64("__TOOLCHAIN_LIST__", 62),
            fixture_elf64("__INIT_ARRAY_LIST__", 62),
        ),
        (
            "i386-unknown-aros",
            "c-i386.o",
            "cxx-i386.o",
            fixture_elf32("__TOOLCHAIN_LIST__", 3),
            fixture_elf32("__INIT_ARRAY_LIST__", 3),
        ),
    ];
    let mut targets = BTreeMap::new();
    let mut compilers = BTreeMap::new();
    for (triple, c_name, cxx_name, c_bytes, cxx_bytes) in objects {
        let c = directory.join(c_name);
        let cxx = directory.join(cxx_name);
        fs::write(&c, c_bytes).unwrap();
        fs::write(&cxx, cxx_bytes).unwrap();
        targets.insert(triple.to_owned(), StandaloneTargetArtifacts { c, cxx });
        compilers.insert(triple.to_owned(), compiler.clone());
    }
    verify_standalone_outputs_with_compilers(
        &StandaloneOutputRequest {
            output_root: directory.to_path_buf(),
            targets,
        },
        &compilers,
    )
    .unwrap()
}

fn update_reports_for_selected_host(fixture: &mut ReceiptFixture) {
    fixture.host_tools = native_compatibility_host_tools(&fixture.host)
        .unwrap()
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                CompatibilityHostToolReport {
                    sha256: sha256_bytes(format!("synthetic host role {name}").as_bytes()),
                    size: 16,
                },
            )
        })
        .collect();
    for phase in REQUIRED_PHASES {
        let report_bytes = fixture.phase_reports.get_mut(&phase).unwrap();
        let mut report: crate::compatibility::CompatibilityProbeReport =
            serde_json::from_slice(report_bytes).unwrap();
        report.host_tools = if matches!(
            phase,
            CompatibilityPhase::StandaloneC | CompatibilityPhase::StandaloneCxx
        ) {
            BTreeMap::new()
        } else {
            fixture.host_tools.clone()
        };
        *report_bytes = crate::canonical::bytes(&serde_json::to_value(report).unwrap()).unwrap();
    }
}

fn fixture_files(fixture: &ReceiptFixture, directory: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    files.insert(COMPATIBILITY_RECEIPT.into(), fixture.receipt.clone());
    for name in ["c-x86_64.o", "cxx-x86_64.o", "c-i386.o", "cxx-i386.o"] {
        files.insert(name.to_owned(), fs::read(directory.join(name)).unwrap());
    }
    for phase in REQUIRED_PHASES {
        let stem = phase_stem(phase);
        files.insert(
            format!("{stem}.report.json"),
            fixture.phase_reports[&phase].clone(),
        );
        let logs = &fixture.command_logs[&phase];
        for (index, command) in logs.iter().enumerate() {
            let command_stem = if logs.len() == 1 {
                stem.to_owned()
            } else {
                format!("{stem}.{}", index + 1)
            };
            files.insert(format!("{command_stem}.stdout.log"), command.stdout.clone());
            files.insert(format!("{command_stem}.stderr.log"), command.stderr.clone());
        }
    }
    files
}

fn fixture_elf64(symbol: &str, machine: u16) -> Vec<u8> {
    let mut object = crate::compatibility::fixture_elf64(symbol);
    object[16..18].copy_from_slice(&1_u16.to_le_bytes());
    object[18..20].copy_from_slice(&machine.to_le_bytes());
    object
}

fn fixture_elf32(symbol: &str, machine: u16) -> Vec<u8> {
    let mut object = crate::compatibility::fixture_elf32(symbol);
    object[16..18].copy_from_slice(&1_u16.to_le_bytes());
    object[18..20].copy_from_slice(&machine.to_le_bytes());
    object
}

fn compatibility_manifest(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let members = files
        .iter()
        .map(|(name, bytes)| {
            (
                name.clone(),
                json!({
                    "sha256": sha256_bytes(bytes),
                    "size": u64::try_from(bytes.len()).unwrap(),
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    crate::canonical::bytes(&json!({
        "schema": "aros-toolchain-compatibility-measurement-v2",
        "files": members,
    }))
    .unwrap()
}

fn phase_stem(phase: CompatibilityPhase) -> &'static str {
    match phase {
        CompatibilityPhase::CmakeConsumer => "cmake-consumer",
        CompatibilityPhase::UpstreamConfigure => "upstream-configure",
        CompatibilityPhase::UpstreamIncludes => "upstream-includes",
        CompatibilityPhase::UpstreamLinklibs => "upstream-linklibs",
        CompatibilityPhase::StandaloneC => "standalone-c",
        CompatibilityPhase::StandaloneCxx => "standalone-cxx",
    }
}

fn expected(fixture: &ReceiptFixture) -> NativeCompatibilityReceiptExpectations<'_> {
    let request = fixture.request();
    NativeCompatibilityReceiptExpectations {
        package: request.package,
        profiles: request.profiles,
        profile: request.profile,
        gnu_source_preset: request.gnu_source_preset,
        cmake_build_required: request.cmake_build_required,
        sdk_consumer_source_tree_sha256: request.sdk_consumer_source_tree_sha256,
        engine_api_version: request.engine_api_version,
        engine_sha256: request.engine_sha256,
        helpers: request.helpers,
        host_tools: request.host_tools,
        sdk_environment_sha256: request.sdk_environment_sha256,
        standalone_environment_sha256: request.standalone_environment_sha256,
        upstream_source_commit: request.upstream_source_commit,
        upstream_source_tree: request.upstream_source_tree,
        ports_sources: request.ports_sources,
    }
}

fn compatibility_request<'a>(
    builds: &'a ReleaseBuildReadbackRequestV2,
    evidence: &'a BTreeMap<String, LaneEvidence>,
    overrides: &'a BTreeMap<String, LaneOverride<'a>>,
) -> ReleaseCompatibilityReadbackRequestV2<'a> {
    let lanes = evidence
        .iter()
        .map(|(asset, lane)| {
            let mut selected = expected(&lane.fixture);
            if let Some(override_) = overrides.get(asset) {
                if let Some(package_fixture) = override_.package_from {
                    selected.package = expected(package_fixture).package;
                }
                if let Some(host) = override_.host {
                    selected.package.host = host;
                }
                if let Some((profiles, profile)) = override_.profiles {
                    selected.profiles = profiles;
                    selected.profile = profile;
                }
            }
            (
                asset.clone(),
                PortableNativeCompatibilityRequest {
                    directory: lane.directory.clone(),
                    manifest_sha256: lane.manifest_sha256.clone(),
                    expected: selected,
                },
            )
        })
        .collect();
    ReleaseCompatibilityReadbackRequestV2 { builds, lanes }
}

fn asset_for_host<'a>(builds: &'a ReleaseBuildReadbackRequestV2, host: &str) -> &'a str {
    let selected = builds
        .packages
        .index
        .artifacts()
        .iter()
        .filter(|artifact| artifact.host() == host && artifact.target_profile() == "pc-x86_64")
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 1, "host/profile selection must be unique");
    selected[0].asset()
}

fn clone_request<'a>(
    request: &ReleaseCompatibilityReadbackRequestV2<'a>,
) -> ReleaseCompatibilityReadbackRequestV2<'a> {
    ReleaseCompatibilityReadbackRequestV2 {
        builds: request.builds,
        lanes: request.lanes.clone(),
    }
}

fn assert_message(error: &crate::ContractError, expected: &str) {
    let diagnostics = &error.diagnostics().diagnostics;
    assert_eq!(diagnostics.len(), 1, "{error}");
    assert_eq!(diagnostics[0].message, expected, "{error}");
    let code = match expected {
        "package-set members differ between independent outputs"
        | "release build package bytes changed during read-back"
        | "release build evidence differs from independently selected raw bytes" => "AX0702",
        "checksum release index differs from its bound canonical bytes" => "AX0701",
        _ => "AX0703",
    };
    assert_eq!(diagnostics[0].code.to_string(), code, "{error}");
}

fn snapshot_directory(directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            snapshot_directory(&path, files);
        } else {
            assert!(kind.is_file() || kind.is_symlink());
            files.insert(path.clone(), fs::read(path).unwrap());
        }
    }
}

fn snapshot_request(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    snapshot_directory(&request.builds.packages.directory, &mut files);
    for build in request.builds.lanes.values() {
        for side in &build.builds {
            snapshot_directory(&side.package_dir, &mut files);
            files.insert(
                side.measurement.clone(),
                fs::read(&side.measurement).unwrap(),
            );
        }
        files.insert(
            build.comparison.clone(),
            fs::read(&build.comparison).unwrap(),
        );
    }
    files.insert(
        request.builds.subject_manifest.clone(),
        fs::read(&request.builds.subject_manifest).unwrap(),
    );
    for lane in request.lanes.values() {
        snapshot_directory(&lane.directory, &mut files);
    }
    files
}

fn assert_rejected_without_mutation(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
    expected: &str,
) {
    let before = snapshot_request(request);
    let error = readback_release_compatibility_v2(request).unwrap_err();
    assert_message(&error, expected);
    assert_eq!(
        snapshot_request(request),
        before,
        "read-back changed an input"
    );
}

#[test]
fn joins_synthetic_three_host_llvm_compatibility_after_original_build_roots_are_gone() {
    let mut prepared = prepare_release();
    let builds = prepared.request();
    let proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &proof);
    let no_overrides = BTreeMap::new();
    let request = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let original_roots = prepared.original_roots.clone();
    prepared.owners.clear();
    for root in &original_roots {
        assert!(
            !root.exists(),
            "original build root survived: {}",
            root.display()
        );
    }

    let readback = readback_release_compatibility_v2(&request).unwrap();
    assert_eq!(readback.builds().lanes().len(), 3);
    assert_eq!(readback.lanes().len(), 3);
    for lane in readback.lanes() {
        assert_eq!(
            lane.compatibility().standalone(),
            &compatibility.lanes[lane.asset()].fixture.standalone,
            "every returned C/C++ ELF hash, size and class must match measured fixture bytes",
        );
        assert_eq!(lane.compatibility().receipt().phases.len(), 6);
        assert_eq!(lane.compatibility().standalone().targets.len(), 2);
        assert!(lane
            .compatibility()
            .standalone()
            .targets
            .contains_key("x86_64-unknown-aros"));
        assert!(lane
            .compatibility()
            .standalone()
            .targets
            .contains_key("i386-unknown-aros"));
    }
}

#[test]
fn rejects_missing_extra_swapped_package_wrong_profile_and_wrong_host_selections() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &proof);
    let no_overrides = BTreeMap::new();
    let base = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let first = asset_for_host(&builds, "linux-aarch64").to_owned();
    let other = asset_for_host(&builds, "linux-x86_64").to_owned();

    let mut missing = clone_request(&base);
    missing.lanes.remove(&first);
    assert_rejected_without_mutation(
        &missing,
        "release compatibility selections must exactly cover the input-derived index",
    );

    let mut extra = clone_request(&base);
    let copied = extra.lanes[&first].clone();
    extra
        .lanes
        .insert("unindexed-package.tar.xz".into(), copied);
    assert_rejected_without_mutation(
        &extra,
        "release compatibility selections must exactly cover the input-derived index",
    );

    let mut package_override = BTreeMap::new();
    package_override.insert(
        first.clone(),
        LaneOverride {
            package_from: Some(&compatibility.lanes[&other].fixture),
            ..LaneOverride::default()
        },
    );
    let swapped = compatibility_request(&builds, &compatibility.lanes, &package_override);
    assert_rejected_without_mutation(
        &swapped,
        "release compatibility expectation differs from its actual indexed package",
    );

    let wrong_profiles = ReceiptFixture::with_cmake_build_required(false, false);
    let mut profile_override = BTreeMap::new();
    profile_override.insert(
        first.clone(),
        LaneOverride {
            profiles: Some((&wrong_profiles.profiles, &wrong_profiles.profile)),
            ..LaneOverride::default()
        },
    );
    let wrong_profile = compatibility_request(&builds, &compatibility.lanes, &profile_override);
    assert_rejected_without_mutation(
        &wrong_profile,
        "release compatibility profile or upstream source differs from its indexed input group",
    );

    let mut host_override = BTreeMap::new();
    host_override.insert(
        first.clone(),
        LaneOverride {
            host: Some(&compatibility.lanes[&other].fixture.host),
            ..LaneOverride::default()
        },
    );
    let wrong_host = compatibility_request(&builds, &compatibility.lanes, &host_override);
    assert_rejected_without_mutation(
        &wrong_host,
        "release compatibility expectation differs from its actual indexed package",
    );
}

#[test]
fn rejects_reused_and_nested_compatibility_roots_without_mutation() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &proof);
    let no_overrides = BTreeMap::new();
    let base = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let first = asset_for_host(&builds, "linux-aarch64").to_owned();
    let second = asset_for_host(&builds, "macos-aarch64").to_owned();

    let mut reused = clone_request(&base);
    let reused_path = reused.lanes[&first].directory.clone();
    reused.lanes.get_mut(&second).unwrap().directory = reused_path;
    assert_rejected_without_mutation(
        &reused,
        "release compatibility root identity is reused across selections",
    );

    let mut nested = clone_request(&base);
    let nested_root = nested.lanes[&first].directory.join("nested-evidence");
    fs::create_dir(&nested_root).unwrap();
    nested.lanes.get_mut(&second).unwrap().directory = nested_root;
    assert_rejected_without_mutation(
        &nested,
        "release compatibility roots must be canonical and nonoverlapping",
    );
}

#[test]
fn rejects_modified_receipt_log_and_elf_bytes_in_a_complete_join_without_writes() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &proof);
    let no_overrides = BTreeMap::new();
    let mut request = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let asset = asset_for_host(&builds, "linux-aarch64");
    let directory = request.lanes[asset].directory.clone();
    let manifest_path = directory.join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST);
    let original_manifest = fs::read(&manifest_path).unwrap();
    let original_digest = request.lanes[asset].manifest_sha256.clone();
    for (name, deep_error) in [
        (
            COMPATIBILITY_RECEIPT,
            "native compatibility metadata has trailing or malformed JSON",
        ),
        (
            "upstream-configure.stdout.log",
            "native compatibility retained command log bytes differ from their report hashes",
        ),
        (
            "c-x86_64.o",
            "standalone output is not a supported little-endian ELF object",
        ),
    ] {
        let path = directory.join(name);
        let original = fs::read(&path).unwrap();
        let changed = if name == "c-x86_64.o" {
            b"not an ELF object".to_vec()
        } else {
            let mut bytes = original.clone();
            bytes.push(b'x');
            bytes
        };
        fs::write(&path, &changed).unwrap();
        assert_rejected_without_mutation(
            &request,
            "portable compatibility member differs from its selected manifest",
        );
        // The complete join must also reject an internally rehashed export,
        // through receipt/log/ELF validation rather than just outer hashes.
        let mut manifest: serde_json::Value = serde_json::from_slice(&original_manifest).unwrap();
        manifest["files"][name]["sha256"] = json!(sha256_bytes(&changed));
        manifest["files"][name]["size"] = json!(changed.len());
        let bytes = crate::canonical::bytes(&manifest).unwrap();
        fs::write(&manifest_path, &bytes).unwrap();
        request.lanes.get_mut(asset).unwrap().manifest_sha256 = sha256_bytes(&bytes);
        assert_rejected_without_mutation(&request, deep_error);
        fs::write(path, original).unwrap();
        fs::write(&manifest_path, &original_manifest).unwrap();
        request.lanes.get_mut(asset).unwrap().manifest_sha256 = original_digest.clone();
    }
}

#[test]
fn release_build_final_reobserver_rejects_changed_selected_inputs_without_writes() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let proof = readback_release_builds_v2(&builds).unwrap();
    let asset = asset_for_host(&builds, "linux-aarch64").to_owned();

    let subject = builds.subject_manifest.clone();
    assert_reobserver_rejects(
        &builds,
        &proof,
        &subject,
        b"tampered subject list",
        "release build evidence differs from independently selected raw bytes",
    );

    let measurement = builds.lanes[&asset].builds[0].measurement.clone();
    assert_reobserver_rejects(
        &builds,
        &proof,
        &measurement,
        b"tampered portable build export",
        "release build evidence differs from independently selected raw bytes",
    );

    let comparison = builds.lanes[&asset].comparison.clone();
    assert_reobserver_rejects(
        &builds,
        &proof,
        &comparison,
        b"tampered comparison report",
        "release build evidence differs from independently selected raw bytes",
    );

    let final_member = builds.packages.directory.join("toolchain-index-v2.json");
    assert_reobserver_rejects(
        &builds,
        &proof,
        &final_member,
        b"tampered final index",
        "checksum release index differs from its bound canonical bytes",
    );

    let package_member = builds.lanes[&asset].builds[0]
        .package_dir
        .join(format!("{asset}.manifest.json"));
    assert_reobserver_rejects(
        &builds,
        &proof,
        &package_member,
        b"tampered A package manifest",
        "package-set members differ between independent outputs",
    );
    // Changing both sides identically still invalidates the earlier proof;
    // re-comparing A against B alone must not admit substituted payload bytes.
    let other_member = builds.lanes[&asset].builds[1]
        .package_dir
        .join(format!("{asset}.manifest.json"));
    let original_other = fs::read(&other_member).unwrap();
    fs::write(&other_member, b"identically substituted manifest").unwrap();
    assert_reobserver_rejects(
        &builds,
        &proof,
        &package_member,
        b"identically substituted manifest",
        "release build package bytes changed during read-back",
    );
    fs::write(other_member, original_other).unwrap();
}

fn assert_reobserver_rejects(
    request: &ReleaseBuildReadbackRequestV2,
    proof: &super::ReleaseBuildReadbackV2,
    path: &Path,
    changed: &[u8],
    expected: &str,
) {
    let original = fs::read(path).unwrap();
    fs::write(path, changed).unwrap();
    let mut snapshot = BTreeMap::new();
    snapshot_directory(&request.packages.directory, &mut snapshot);
    for lane in request.lanes.values() {
        for side in &lane.builds {
            snapshot_directory(&side.package_dir, &mut snapshot);
            snapshot.insert(
                side.measurement.clone(),
                fs::read(&side.measurement).unwrap(),
            );
        }
        snapshot.insert(lane.comparison.clone(), fs::read(&lane.comparison).unwrap());
    }
    snapshot.insert(
        request.subject_manifest.clone(),
        fs::read(&request.subject_manifest).unwrap(),
    );
    let error = super::release_readback::revalidate_release_builds_v2(request, proof).unwrap_err();
    assert_message(&error, expected);
    let mut after = BTreeMap::new();
    snapshot_directory(&request.packages.directory, &mut after);
    for lane in request.lanes.values() {
        for side in &lane.builds {
            snapshot_directory(&side.package_dir, &mut after);
            after.insert(
                side.measurement.clone(),
                fs::read(&side.measurement).unwrap(),
            );
        }
        after.insert(lane.comparison.clone(), fs::read(&lane.comparison).unwrap());
    }
    after.insert(
        request.subject_manifest.clone(),
        fs::read(&request.subject_manifest).unwrap(),
    );
    assert_eq!(after, snapshot, "reobserver changed an input");
    fs::write(path, original).unwrap();
}
