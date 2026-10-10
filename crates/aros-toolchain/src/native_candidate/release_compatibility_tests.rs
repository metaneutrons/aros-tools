//! Synthetic three-host indexed build/compatibility joins. Reports are test
//! documents; no compatibility command or compiler is executed.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, ArosCompilerIdentity, Sha256Digest};
use serde_json::{json, Value};
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
use crate::qualification_evidence::EvidencePolicy;
use crate::qualification_evidence_v2::{QualificationEvidenceV2, SourceRunIdentityV2};
use crate::qualification_readback_v2::{
    readback_qualification_bytes_v2, QualificationByteReadbackRequestV2,
};
use crate::qualification_recording_v2::{
    record_qualification_bytes_v2, QualificationRecordingRequestV2,
};
use crate::recipe::GitObjectId;
use crate::recovery::{
    FailedStage, ObservedTag, RecoveryDecision, RecoveryHandoff, RecoveryOperation,
    ReleaseHandoffState,
};
use crate::recovery_v2::{
    readback_recovery_bytes_v2, RecoveryByteReadbackRequestV2, RecoveryRequestV2,
    RECOVERY_V2_SCHEMA,
};
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

        let mut fixture = ReceiptFixture::with_cmake_build_required(
            matches!(
                package.manifest.compiler_identity().unwrap(),
                ArosCompilerIdentity::Gnu { .. }
            ),
            false,
        );
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
    fixture.source_preset = if matches!(fixture.compiler, ArosCompilerIdentity::Gnu { .. }) {
        Some("source-rv32-preset".into())
    } else {
        None
    };
    fixture.cmake_build_required = false;
    fixture.upstream_commit = profiles.upstream_commit().clone();
    fixture.upstream_tree = upstream_tree.clone();
}

fn write_and_verify_standalone(
    directory: &Path,
    compiler: &ArosCompilerIdentity,
) -> crate::compatibility::StandaloneOutputReport {
    let objects = if let ArosCompilerIdentity::Gnu { target, .. } = compiler {
        vec![(
            "riscv-aros",
            "c-riscv.o",
            "cxx-riscv.o",
            crate::compatibility::fixture_riscv_elf(
                aros_common::elf::Class::Elf32,
                "__TOOLCHAIN_LIST__",
                target,
                false,
            ),
            crate::compatibility::fixture_riscv_elf(
                aros_common::elf::Class::Elf32,
                "__INIT_ARRAY_LIST__",
                target,
                false,
            ),
        )]
    } else {
        vec![
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
        ]
    };
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
    let objects: &[&str] = if matches!(fixture.compiler, ArosCompilerIdentity::Gnu { .. }) {
        &["c-riscv.o", "cxx-riscv.o"]
    } else {
        &["c-x86_64.o", "cxx-x86_64.o", "c-i386.o", "cxx-i386.o"]
    };
    for &name in objects {
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

fn qualification_policy() -> EvidencePolicy {
    EvidencePolicy {
        source_repository: "https://github.com/example/aros-toolchains".into(),
        source_workflow: ".github/workflows/qualification.yml".into(),
        signer_repository: "https://github.com/example/aros-toolchains".into(),
        signer_workflow: ".github/workflows/qualification.yml".into(),
        signer: "github-actions".into(),
        now: 150,
    }
}

fn qualification_evidence_value(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
    measured: &super::ReleaseCompatibilityReadbackV2,
    index_bytes: &[u8],
    policy: &EvidencePolicy,
    coverage: &str,
) -> Value {
    let packages = &request.builds.packages;
    let index = &packages.index;
    let lanes = index
        .artifacts()
        .iter()
        .map(|artifact| {
            let group = packages
                .inputs
                .groups()
                .iter()
                .find(|group| group.id() == artifact.group_id())
                .unwrap();
            let build = measured
                .builds()
                .lanes()
                .iter()
                .find(|lane| lane.asset() == artifact.asset())
                .unwrap();
            let compatibility = measured
                .lanes()
                .iter()
                .find(|lane| lane.asset() == artifact.asset())
                .unwrap();
            let measurements = build.measurement_sha256();
            json!({
                "group_id": artifact.group_id(),
                "asset": artifact.asset(),
                "host": artifact.host(),
                "target_profile": artifact.target_profile(),
                "target_triple": artifact.target_triple(),
                "source_commit": artifact.source_commit(),
                "compiler": artifact.compiler(),
                "recipe_sha256": group.recipe().sha256(),
                "source_lock_sha256": group.source_lock_reference().sha256(),
                "profiles_sha256": group.profiles_reference().sha256(),
                "archive_sha256": artifact.sha256(),
                "archive_size": artifact.size(),
                "tree_sha256": artifact.tree_sha256(),
                "build_a_report_sha256": measurements[0],
                "build_b_report_sha256": measurements[1],
                "comparison_report_sha256": build.comparison_sha256(),
                "compatibility_report_sha256": compatibility.compatibility().manifest_sha256()
            })
        })
        .collect::<Vec<_>>();
    let subject_manifest_sha256 = measured.builds().subject_manifest_sha256();
    json!({
        "schema": "aros-toolchain-qualification-evidence-v2",
        "created_at": 100,
        "expires_at": 300,
        "source_run": {
            "repository": policy.source_repository,
            "workflow": policy.source_workflow,
            "run_id": 42,
            "run_attempt": 1,
            "producer_commit": index.producer_commit(),
            "source_tag": "release-test",
            "tag_object": "9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a"
        },
        "release": {
            "release_id": index.release_id(),
            "base_url": index.base_url(),
            "inputs_sha256": packages.inputs.collection_sha256(),
            "release_index_sha256": sha256_bytes(index_bytes),
            "pre_attestation_checksums_sha256": subject_manifest_sha256,
            "checksums_sha256": measured.builds().checksums_sha256(),
            "provenance_sha256": measured.builds().provenance_sha256(),
            "producer_commit": index.producer_commit(),
            "tools_commit": index.tools_commit()
        },
        "attestation": {
            "repository": policy.signer_repository,
            "workflow": policy.signer_workflow,
            "signer": policy.signer,
            "subject_manifest_sha256": subject_manifest_sha256
        },
        "lanes": lanes,
        "coverage": coverage
    })
}

fn qualification_bytes(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
    measured: &super::ReleaseCompatibilityReadbackV2,
    index_bytes: &[u8],
    policy: &EvidencePolicy,
    coverage: &str,
) -> Vec<u8> {
    serde_json::to_vec(&qualification_evidence_value(
        request,
        measured,
        index_bytes,
        policy,
        coverage,
    ))
    .unwrap()
}

fn qualification_source_run(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
    index_bytes: &[u8],
    policy: &EvidencePolicy,
) -> SourceRunIdentityV2 {
    let measured = readback_release_compatibility_v2(request).unwrap();
    let fixture_claims =
        qualification_evidence_value(request, &measured, index_bytes, policy, "release-candidate");
    serde_json::from_value(fixture_claims["source_run"].clone()).unwrap()
}

fn recording_request<'a, 'e>(
    index_bytes: &'a [u8],
    complete: &'a ReleaseCompatibilityReadbackRequestV2<'e>,
    source_run: SourceRunIdentityV2,
    policy: EvidencePolicy,
    created_at: u64,
    expires_at: u64,
) -> QualificationRecordingRequestV2<'a, 'e> {
    QualificationRecordingRequestV2 {
        index_bytes,
        complete,
        source_run,
        policy,
        created_at,
        expires_at,
    }
}

fn assert_recording_rejected_without_mutation(
    request: &QualificationRecordingRequestV2<'_, '_>,
    expected_message: &str,
) {
    let before = snapshot_request(request.complete);
    let Err(error) = record_qualification_bytes_v2(request) else {
        panic!("qualification recording accepted an invalid selection");
    };
    let diagnostics = &error.diagnostics().diagnostics;
    assert_eq!(diagnostics.len(), 1, "{error}");
    assert_eq!(diagnostics[0].message, expected_message, "{error}");
    assert_eq!(diagnostics[0].code.to_string(), "AX0901", "{error}");
    assert_eq!(
        snapshot_request(request.complete),
        before,
        "qualification recording changed an input"
    );
}

fn assert_qualification_message(error: &crate::ContractError, expected: &str) {
    let diagnostics = &error.diagnostics().diagnostics;
    assert_eq!(diagnostics.len(), 1, "{error}");
    assert_eq!(diagnostics[0].message, expected, "{error}");
    assert_eq!(diagnostics[0].code.to_string(), "AX0901", "{error}");
}

fn assert_qualification_rejected_without_mutation(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
    evidence_bytes: &[u8],
    index_bytes: &[u8],
    policy: &EvidencePolicy,
    expected: &str,
) {
    let before = snapshot_request(request);
    let Err(error) = readback_qualification_bytes_v2(&QualificationByteReadbackRequestV2 {
        evidence_bytes,
        index_bytes,
        complete: request,
        policy,
    }) else {
        panic!("qualification byte read-back accepted an invalid claim");
    };
    assert_qualification_message(&error, expected);
    assert_eq!(
        snapshot_request(request),
        before,
        "qualification gate changed an input"
    );
}

fn assert_lower_layer_rejected_without_mutation(
    request: &ReleaseCompatibilityReadbackRequestV2<'_>,
    evidence_bytes: &[u8],
    index_bytes: &[u8],
    policy: &EvidencePolicy,
    expected: &str,
) {
    let before = snapshot_request(request);
    let Err(error) = readback_qualification_bytes_v2(&QualificationByteReadbackRequestV2 {
        evidence_bytes,
        index_bytes,
        complete: request,
        policy,
    }) else {
        panic!("qualification byte read-back accepted changed compatibility bytes");
    };
    assert_message(&error, expected);
    assert_eq!(
        snapshot_request(request),
        before,
        "qualification gate changed an input"
    );
}

#[test]
fn qualification_readback_joins_three_host_claims_to_complete_bytes_without_writes() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let build_proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &build_proof);
    let no_overrides = BTreeMap::new();
    let complete = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let independently_measured = readback_release_compatibility_v2(&complete).unwrap();
    let index_path = builds
        .packages
        .directory
        .join(crate::release_index_v2::INDEX_NAME);
    let index_bytes = fs::read(index_path).unwrap();
    let policy = qualification_policy();
    let evidence_bytes = qualification_bytes(
        &complete,
        &independently_measured,
        &index_bytes,
        &policy,
        "release-candidate",
    );
    let before = snapshot_request(&complete);

    let joined = readback_qualification_bytes_v2(&QualificationByteReadbackRequestV2 {
        evidence_bytes: &evidence_bytes,
        index_bytes: &index_bytes,
        complete: &complete,
        policy: &policy,
    })
    .unwrap();

    assert_eq!(joined.evidence().lanes.len(), 3);
    assert_eq!(joined.complete().builds().lanes().len(), 3);
    assert_eq!(joined.complete().lanes().len(), 3);
    assert_eq!(
        joined.evidence().release.checksums_sha256,
        *joined.complete().builds().checksums_sha256()
    );
    assert_eq!(
        joined.evidence().release.pre_attestation_checksums_sha256,
        *joined.complete().builds().subject_manifest_sha256()
    );
    assert_eq!(
        joined.evidence().release.provenance_sha256,
        *joined.complete().builds().provenance_sha256()
    );
    for lane in &joined.evidence().lanes {
        let build = joined
            .complete()
            .builds()
            .lanes()
            .iter()
            .find(|build| build.asset() == lane.asset)
            .unwrap();
        let compatibility = joined
            .complete()
            .lanes()
            .iter()
            .find(|compatibility| compatibility.asset() == lane.asset)
            .unwrap();
        let measurements = build.measurement_sha256();
        assert_eq!(lane.build_a_report_sha256, *measurements[0]);
        assert_eq!(lane.build_b_report_sha256, *measurements[1]);
        assert_eq!(lane.comparison_report_sha256, *build.comparison_sha256());
        assert_eq!(
            lane.compatibility_report_sha256,
            *compatibility.compatibility().manifest_sha256()
        );
    }
    assert_eq!(
        snapshot_request(&complete),
        before,
        "qualification gate changed an input"
    );
}

#[test]
fn qualification_readback_rejects_claim_substitutions_and_incomplete_coverage_without_writes() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let build_proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &build_proof);
    let no_overrides = BTreeMap::new();
    let complete = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let independently_measured = readback_release_compatibility_v2(&complete).unwrap();
    let index_path = builds
        .packages
        .directory
        .join(crate::release_index_v2::INDEX_NAME);
    let index_bytes = fs::read(index_path).unwrap();
    let policy = qualification_policy();
    let evidence_bytes = qualification_bytes(
        &complete,
        &independently_measured,
        &index_bytes,
        &policy,
        "release-candidate",
    );
    let valid: Value = serde_json::from_slice(&evidence_bytes).unwrap();

    let mut alternate_index_bytes = index_bytes.clone();
    alternate_index_bytes.push(b' ');
    let alternate_index = crate::release_index_v2::NativeReleaseIndexV2::parse(
        &alternate_index_bytes,
        &builds.packages.inputs,
    )
    .unwrap();
    assert_eq!(alternate_index, builds.packages.index);
    let mut resealed_index_claim = valid.clone();
    resealed_index_claim["release"]["release_index_sha256"] =
        json!(sha256_bytes(&alternate_index_bytes));
    let resealed_index_claim = serde_json::to_vec(&resealed_index_claim).unwrap();
    let parsed_claim =
        crate::qualification_evidence_v2::QualificationEvidenceV2::parse(&resealed_index_claim)
            .unwrap();
    parsed_claim
        .validate_against_index(&alternate_index_bytes, &builds.packages.inputs, &policy)
        .unwrap();
    assert_qualification_rejected_without_mutation(
        &complete,
        &resealed_index_claim,
        &alternate_index_bytes,
        &policy,
        "qualification index bytes differ from the complete selected release",
    );

    for (field, nonce) in [
        ("build_a_report_sha256", 0xf001_u64),
        ("build_b_report_sha256", 0xf002),
        ("comparison_report_sha256", 0xf003),
        ("compatibility_report_sha256", 0xf004),
    ] {
        let mut changed = valid.clone();
        changed["lanes"][0][field] = json!(format!("{nonce:064x}"));
        assert_qualification_rejected_without_mutation(
            &complete,
            &serde_json::to_vec(&changed).unwrap(),
            &index_bytes,
            &policy,
            "qualification report claims differ from the complete measured evidence bytes",
        );
    }

    let mut changed_checksums = valid.clone();
    changed_checksums["release"]["checksums_sha256"] = json!("f101".repeat(16));
    assert_qualification_rejected_without_mutation(
        &complete,
        &serde_json::to_vec(&changed_checksums).unwrap(),
        &index_bytes,
        &policy,
        "qualification release claims differ from measured checksums, subjects or provenance bytes",
    );

    let mut changed_subject = valid.clone();
    let new_subject = "f102".repeat(16);
    changed_subject["release"]["pre_attestation_checksums_sha256"] = json!(new_subject);
    changed_subject["attestation"]["subject_manifest_sha256"] = json!(new_subject);
    assert_qualification_rejected_without_mutation(
        &complete,
        &serde_json::to_vec(&changed_subject).unwrap(),
        &index_bytes,
        &policy,
        "qualification release claims differ from measured checksums, subjects or provenance bytes",
    );

    let mut changed_provenance = valid.clone();
    changed_provenance["release"]["provenance_sha256"] = json!("f103".repeat(16));
    assert_qualification_rejected_without_mutation(
        &complete,
        &serde_json::to_vec(&changed_provenance).unwrap(),
        &index_bytes,
        &policy,
        "qualification release claims differ from measured checksums, subjects or provenance bytes",
    );

    let mut diagnostic = valid.clone();
    diagnostic["coverage"] = json!("diagnostic");
    assert_qualification_rejected_without_mutation(
        &complete,
        &serde_json::to_vec(&diagnostic).unwrap(),
        &index_bytes,
        &policy,
        "qualification byte read-back requires complete release-candidate coverage",
    );

    let mut subset = valid;
    subset["lanes"].as_array_mut().unwrap().pop();
    assert_qualification_rejected_without_mutation(
        &complete,
        &serde_json::to_vec(&subset).unwrap(),
        &index_bytes,
        &policy,
        "qualification evidence v2 lacks the complete input-derived matrix",
    );
}

#[test]
fn qualification_readback_rejects_changed_compatibility_log_bytes_without_writes() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let build_proof = readback_release_builds_v2(&builds).unwrap();
    let mut compatibility = prepare_compatibility(&prepared, &build_proof);
    let no_overrides = BTreeMap::new();
    let original_request = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let independently_measured = readback_release_compatibility_v2(&original_request).unwrap();
    let index_path = builds
        .packages
        .directory
        .join(crate::release_index_v2::INDEX_NAME);
    let index_bytes = fs::read(index_path).unwrap();
    let policy = qualification_policy();
    let evidence_bytes = qualification_bytes(
        &original_request,
        &independently_measured,
        &index_bytes,
        &policy,
        "release-candidate",
    );
    drop(original_request);
    drop(independently_measured);

    let asset = asset_for_host(&builds, "linux-aarch64");
    let directory = compatibility.lanes[asset].directory.clone();
    let log_name = "upstream-configure.stdout.log";
    let log_path = directory.join(log_name);
    let mut changed_log = fs::read(&log_path).unwrap();
    changed_log.push(b'x');
    fs::write(&log_path, &changed_log).unwrap();

    let manifest_path = directory.join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST);
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["files"][log_name]["sha256"] = json!(sha256_bytes(&changed_log));
    manifest["files"][log_name]["size"] = json!(changed_log.len());
    let changed_manifest = crate::canonical::bytes(&manifest).unwrap();
    fs::write(&manifest_path, &changed_manifest).unwrap();
    compatibility.lanes.get_mut(asset).unwrap().manifest_sha256 = sha256_bytes(&changed_manifest);

    let changed_request = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    assert_lower_layer_rejected_without_mutation(
        &changed_request,
        &evidence_bytes,
        &index_bytes,
        &policy,
        "native compatibility retained command log bytes differ from their report hashes",
    );
}

#[test]
fn qualification_recording_derives_claims_and_roundtrips_through_byte_readback_without_writes() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let build_proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &build_proof);
    let no_overrides = BTreeMap::new();
    let complete = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let index_path = builds
        .packages
        .directory
        .join(crate::release_index_v2::INDEX_NAME);
    let index_bytes = fs::read(index_path).unwrap();
    let policy = qualification_policy();
    let before = snapshot_request(&complete);
    let source_run = qualification_source_run(&complete, &index_bytes, &policy);
    assert_eq!(snapshot_request(&complete), before);

    // Recording receives only selected producer identity and policy inputs.
    // The qualification claims themselves are derived from its own read-back.
    let request = recording_request(
        &index_bytes,
        &complete,
        source_run,
        policy.clone(),
        100,
        300,
    );
    let recorded = record_qualification_bytes_v2(&request).unwrap();
    assert_eq!(recorded.evidence().lanes.len(), 3);
    assert_eq!(recorded.complete().builds().lanes().len(), 3);
    assert_eq!(recorded.complete().lanes().len(), 3);
    assert_eq!(snapshot_request(&complete), before);

    let evidence_bytes = serde_json::to_vec(recorded.evidence()).unwrap();
    let verified = readback_qualification_bytes_v2(&QualificationByteReadbackRequestV2 {
        evidence_bytes: &evidence_bytes,
        index_bytes: &index_bytes,
        complete: &complete,
        policy: &policy,
    })
    .unwrap();
    assert_eq!(verified.evidence(), recorded.evidence());
    assert_eq!(verified.complete().builds().lanes().len(), 3);
    assert_eq!(verified.complete().lanes().len(), 3);
    assert_eq!(snapshot_request(&complete), before);
}

#[test]
fn qualification_recording_rejects_wrong_run_index_and_time_claims_without_writes() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let build_proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &build_proof);
    let no_overrides = BTreeMap::new();
    let complete = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let index_path = builds
        .packages
        .directory
        .join(crate::release_index_v2::INDEX_NAME);
    let index_bytes = fs::read(index_path).unwrap();
    let policy = qualification_policy();
    let source_run = qualification_source_run(&complete, &index_bytes, &policy);

    let mut wrong_producer = source_run.clone();
    wrong_producer.producer_commit = GitObjectId::try_from("8".repeat(40)).unwrap();
    let wrong_producer_request = recording_request(
        &index_bytes,
        &complete,
        wrong_producer,
        policy.clone(),
        100,
        300,
    );
    assert_recording_rejected_without_mutation(
        &wrong_producer_request,
        "qualification recording source producer differs from the selected index",
    );

    let mut alternate_index_bytes = index_bytes.clone();
    alternate_index_bytes.push(b' ');
    let alternate_index_request = recording_request(
        &alternate_index_bytes,
        &complete,
        source_run.clone(),
        policy.clone(),
        100,
        300,
    );
    assert_recording_rejected_without_mutation(
        &alternate_index_request,
        "qualification recording index bytes are not the selected canonical index",
    );

    let expired_request = recording_request(
        &index_bytes,
        &complete,
        source_run,
        policy.clone(),
        100,
        policy.now,
    );
    assert_recording_rejected_without_mutation(
        &expired_request,
        "qualification recording has invalid creation, expiry or policy time",
    );
}

#[test]
fn qualification_recording_rejects_changed_compatibility_report_and_log_without_writes() {
    let prepared = prepare_release();
    let builds = prepared.request();
    let build_proof = readback_release_builds_v2(&builds).unwrap();
    let mut compatibility = prepare_compatibility(&prepared, &build_proof);
    let no_overrides = BTreeMap::new();
    let original_request = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let index_path = builds
        .packages
        .directory
        .join(crate::release_index_v2::INDEX_NAME);
    let index_bytes = fs::read(index_path).unwrap();
    let policy = qualification_policy();
    let source_run = qualification_source_run(&original_request, &index_bytes, &policy);
    drop(original_request);

    let asset = asset_for_host(&builds, "linux-aarch64");
    let directory = compatibility.lanes[asset].directory.clone();
    let original_manifest_path = directory.join(PORTABLE_NATIVE_COMPATIBILITY_MANIFEST);
    let original_manifest = fs::read(&original_manifest_path).unwrap();
    let original_manifest_sha256 = compatibility.lanes[asset].manifest_sha256.clone();

    for (member, expected_message) in [
        (
            "upstream-configure.report.json",
            "native compatibility phase report bytes are not canonical JSON",
        ),
        (
            "upstream-configure.stdout.log",
            "native compatibility retained command log bytes differ from their report hashes",
        ),
    ] {
        let member_path = directory.join(member);
        let original_member = fs::read(&member_path).unwrap();
        let mut changed_member = original_member.clone();
        changed_member.push(b' ');
        fs::write(&member_path, &changed_member).unwrap();

        let mut manifest: Value = serde_json::from_slice(&original_manifest).unwrap();
        manifest["files"][member]["sha256"] = json!(sha256_bytes(&changed_member));
        manifest["files"][member]["size"] = json!(changed_member.len());
        let changed_manifest = crate::canonical::bytes(&manifest).unwrap();
        fs::write(&original_manifest_path, &changed_manifest).unwrap();
        compatibility.lanes.get_mut(asset).unwrap().manifest_sha256 =
            sha256_bytes(&changed_manifest);

        {
            let changed_request =
                compatibility_request(&builds, &compatibility.lanes, &no_overrides);
            let recording = recording_request(
                &index_bytes,
                &changed_request,
                source_run.clone(),
                policy.clone(),
                100,
                300,
            );
            let before = snapshot_request(&changed_request);
            let Err(error) = record_qualification_bytes_v2(&recording) else {
                panic!("qualification recording accepted changed compatibility bytes");
            };
            assert_message(&error, expected_message);
            assert_eq!(
                snapshot_request(&changed_request),
                before,
                "qualification recording changed an input"
            );
        }

        fs::write(member_path, original_member).unwrap();
        fs::write(&original_manifest_path, &original_manifest).unwrap();
        compatibility.lanes.get_mut(asset).unwrap().manifest_sha256 =
            original_manifest_sha256.clone();
    }
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

struct RecoveryFixtureV2 {
    _prepared: PreparedRelease,
    builds: super::ReleaseBuildReadbackRequestV2,
    compatibility: PreparedCompatibility,
    no_overrides: BTreeMap<String, LaneOverride<'static>>,
    index_bytes: Vec<u8>,
    policy: EvidencePolicy,
    evidence_bytes: Vec<u8>,
}

impl RecoveryFixtureV2 {
    fn complete_request(&self) -> ReleaseCompatibilityReadbackRequestV2<'_> {
        compatibility_request(&self.builds, &self.compatibility.lanes, &self.no_overrides)
    }
}

fn prepare_recovery_fixture_v2() -> RecoveryFixtureV2 {
    let prepared = prepare_release();
    prepare_recovery_fixture_from_release_v2(prepared)
}

fn prepare_recovery_fixture_from_release_v2(prepared: PreparedRelease) -> RecoveryFixtureV2 {
    let builds = prepared.request();
    let build_proof = readback_release_builds_v2(&builds).unwrap();
    let compatibility = prepare_compatibility(&prepared, &build_proof);
    let no_overrides = BTreeMap::new();
    let complete = compatibility_request(&builds, &compatibility.lanes, &no_overrides);
    let measured = readback_release_compatibility_v2(&complete).unwrap();
    let index_bytes = fs::read(
        builds
            .packages
            .directory
            .join(crate::release_index_v2::INDEX_NAME),
    )
    .unwrap();
    let policy = qualification_policy();
    let evidence_bytes = qualification_bytes(
        &complete,
        &measured,
        &index_bytes,
        &policy,
        "release-candidate",
    );
    RecoveryFixtureV2 {
        _prepared: prepared,
        builds,
        compatibility,
        no_overrides: BTreeMap::new(),
        index_bytes,
        policy,
        evidence_bytes,
    }
}

fn recovery_qualification_request<'a, 'e>(
    fixture: &'a RecoveryFixtureV2,
    complete: &'a ReleaseCompatibilityReadbackRequestV2<'e>,
    evidence_bytes: &'a [u8],
    policy: &'a EvidencePolicy,
) -> QualificationByteReadbackRequestV2<'a, 'e> {
    QualificationByteReadbackRequestV2 {
        evidence_bytes,
        index_bytes: &fixture.index_bytes,
        complete,
        policy,
    }
}

fn recovery_request_v2(
    evidence_bytes: &[u8],
    operation: RecoveryOperation,
    failed_stage: FailedStage,
    handoff: Option<RecoveryHandoff>,
) -> RecoveryRequestV2 {
    let evidence = QualificationEvidenceV2::parse(evidence_bytes).unwrap();
    RecoveryRequestV2 {
        schema: RECOVERY_V2_SCHEMA.to_owned(),
        qualification_sha256: sha256_bytes(evidence_bytes),
        operation,
        failed_stage,
        observed_run: evidence.source_run.clone(),
        source_tag: ObservedTag {
            name: evidence.source_run.source_tag.clone(),
            tag_object: evidence.source_run.tag_object.clone(),
            peeled_commit: evidence.source_run.producer_commit.clone(),
        },
        observed_attestation: Some(evidence.attestation),
        handoff,
    }
}

fn fresh_recovery_handoff(
    source_run: &SourceRunIdentityV2,
    state: ReleaseHandoffState,
) -> RecoveryHandoff {
    RecoveryHandoff {
        release_id: "recovered-release".into(),
        tag: ObservedTag {
            name: "release-recovered".into(),
            tag_object: GitObjectId::try_from(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
            )
            .unwrap(),
            peeled_commit: source_run.producer_commit.clone(),
        },
        state,
    }
}

fn assert_recovery_rejected_without_mutation(
    recovery: &RecoveryRequestV2,
    qualification: &QualificationByteReadbackRequestV2<'_, '_>,
    expected_message: &str,
    expected_code: &str,
) {
    let before = snapshot_request(qualification.complete);
    let error = readback_recovery_bytes_v2(&RecoveryByteReadbackRequestV2 {
        recovery,
        qualification,
    })
    .unwrap_err();
    let diagnostics = &error.diagnostics().diagnostics;
    assert_eq!(diagnostics.len(), 1, "{error}");
    assert_eq!(diagnostics[0].message, expected_message, "{error}");
    assert_eq!(diagnostics[0].code.to_string(), expected_code, "{error}");
    assert_eq!(
        snapshot_request(qualification.complete),
        before,
        "recovery read-back changed an input"
    );
}

// These records are synthetic local observations. A passing test does not
// authenticate the provider run, Git tag, signer, or attestation origin.
#[test]
fn recovery_v2_repackages_selected_compiler_family_lanes_without_changing_original_evidence() {
    use crate::repackage_v2::{repackage_verified_package_v2, VerifiedPackageRepackageRequestV2};

    for prepared in [
        prepare_release(),
        super::release_readback_tests::prepare_gnu_release(),
    ] {
        let mut fixture = prepare_recovery_fixture_from_release_v2(prepared);
        let missing_build_root = fixture.builds.packages.directory.join("missing-build-root");
        assert!(!missing_build_root.exists());
        fixture.builds.packages.forbidden_prefixes = vec![missing_build_root.clone()];
        let complete = fixture.complete_request();
        let qualification = recovery_qualification_request(
            &fixture,
            &complete,
            &fixture.evidence_bytes,
            &fixture.policy,
        );
        let evidence = QualificationEvidenceV2::parse(&fixture.evidence_bytes).unwrap();
        let handoff = fresh_recovery_handoff(&evidence.source_run, ReleaseHandoffState::Absent);
        let recovery = recovery_request_v2(
            &fixture.evidence_bytes,
            RecoveryOperation::PackagingRecovery,
            FailedStage::Packaging,
            Some(handoff.clone()),
        );
        let recovery = RecoveryByteReadbackRequestV2 {
            recovery: &recovery,
            qualification: &qualification,
        };
        let before = snapshot_request(&complete);
        for artifact in complete.builds.packages.index.artifacts() {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let output = repackage_verified_package_v2(&VerifiedPackageRepackageRequestV2 {
                recovery: &recovery,
                asset: artifact.asset(),
                extraction_roots: [root.join("extract-a"), root.join("extract-b")],
                output_dirs: [root.join("package-a"), root.join("package-b")],
            })
            .unwrap();
            let repackaged = &output.packages.repackaged;
            assert_eq!(
                output.packages.source.manifest.tree_sha256,
                artifact.tree_sha256().as_str()
            );
            assert_eq!(
                repackaged.first_verified.manifest.release_id,
                handoff.release_id
            );
            assert_eq!(repackaged.first_verified, repackaged.second_verified);
            let mut original_manifest = output.packages.source.manifest.clone();
            original_manifest.release_id.clone_from(&handoff.release_id);
            assert_eq!(repackaged.first_verified.manifest, original_manifest);
            assert_eq!(repackaged.first_verified.manifest.schema, 2);
            assert_eq!(output.comparison.members.len(), 4);
            assert_ne!(
                repackaged.first.archive_sha256,
                output.packages.source.archive_sha256
            );
            assert_eq!(snapshot_request(&complete), before);
            assert!(!missing_build_root.exists());
        }
    }
}

#[test]
fn recovery_v2_repackage_preserves_forbidden_build_roots_and_their_aliases() {
    use crate::repackage_v2::{repackage_verified_package_v2, VerifiedPackageRepackageRequestV2};
    use std::os::unix::fs::symlink;

    let mut fixture = prepare_recovery_fixture_v2();
    for selected_destination in 0..4 {
        for alias in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let build_root = root.join("original-build");
            fs::create_dir(&build_root).unwrap();
            let sentinel = build_root.join("retained.log");
            fs::write(&sentinel, b"original build evidence").unwrap();
            let prefix = if alias {
                let path = root.join("original-build-alias");
                symlink(&build_root, &path).unwrap();
                path
            } else {
                build_root.clone()
            };
            fixture.builds.packages.forbidden_prefixes = vec![prefix];
            let complete = fixture.complete_request();
            let qualification = recovery_qualification_request(
                &fixture,
                &complete,
                &fixture.evidence_bytes,
                &fixture.policy,
            );
            let evidence = QualificationEvidenceV2::parse(&fixture.evidence_bytes).unwrap();
            let request = recovery_request_v2(
                &fixture.evidence_bytes,
                RecoveryOperation::PackagingRecovery,
                FailedStage::Packaging,
                Some(fresh_recovery_handoff(
                    &evidence.source_run,
                    ReleaseHandoffState::Absent,
                )),
            );
            let recovery = RecoveryByteReadbackRequestV2 {
                recovery: &request,
                qualification: &qualification,
            };
            let before = snapshot_request(&complete);
            let mut destinations = [
                root.join("extract-a"),
                root.join("extract-b"),
                root.join("package-a"),
                root.join("package-b"),
            ];
            destinations[selected_destination] = build_root.join("new-output");
            let [first_extraction, second_extraction, first_package, second_package] =
                destinations.clone();
            let failure = repackage_verified_package_v2(&VerifiedPackageRepackageRequestV2 {
                recovery: &recovery,
                asset: complete.builds.packages.index.artifacts()[0].asset(),
                extraction_roots: [first_extraction, second_extraction],
                output_dirs: [first_package, second_package],
            })
            .unwrap_err();
            let diagnostics = failure.diagnostics();
            assert_eq!(diagnostics.diagnostics.len(), 1);
            assert_eq!(diagnostics.diagnostics[0].code.to_string(), "AX0901");
            assert_eq!(
                diagnostics.diagnostics[0].message,
                "recovery destinations overlap a forbidden build root"
            );
            for destination in destinations {
                assert!(!destination.exists());
            }
            assert_eq!(fs::read(&sentinel).unwrap(), b"original build evidence");
            assert_eq!(fs::read_dir(&build_root).unwrap().count(), 1);
            assert_eq!(snapshot_request(&complete), before);
        }
    }
}

#[test]
fn recovery_v2_repackage_rejects_bad_selections_and_destinations_before_writes() {
    use crate::repackage_v2::{repackage_verified_package_v2, VerifiedPackageRepackageRequestV2};
    use std::os::unix::fs::symlink;

    let fixture = prepare_recovery_fixture_v2();
    let complete = fixture.complete_request();
    let qualification = recovery_qualification_request(
        &fixture,
        &complete,
        &fixture.evidence_bytes,
        &fixture.policy,
    );
    let evidence = QualificationEvidenceV2::parse(&fixture.evidence_bytes).unwrap();
    let handoff = fresh_recovery_handoff(&evidence.source_run, ReleaseHandoffState::Absent);
    let packaging = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(handoff),
    );
    let replay = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::CompatibilityReplay,
        FailedStage::CompatibilityHarness,
        None,
    );
    let before = snapshot_request(&complete);
    let asset = complete.builds.packages.index.artifacts()[0].asset();
    for mutation in [
        "unknown-lane",
        "replay",
        "shared-root",
        "release-root",
        "existing",
        "symlink-parent",
        "non-normalized",
        "measurement-root",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let mut extracts = [root.join("extract-a"), root.join("extract-b")];
        let mut outputs = [root.join("package-a"), root.join("package-b")];
        let mut selected_asset = asset;
        let mut selected_recovery = &packaging;
        match mutation {
            "unknown-lane" => selected_asset = "not-selected.tar.xz",
            "replay" => selected_recovery = &replay,
            "shared-root" => outputs[1] = extracts[0].clone(),
            "release-root" => outputs[0] = complete.builds.packages.directory.join("new-output"),
            "existing" => {
                fs::create_dir(&outputs[0]).unwrap();
            }
            "symlink-parent" => {
                symlink(&root, root.join("alias")).unwrap();
                outputs[0] = root.join("alias/package-a");
            }
            "non-normalized" => outputs[0] = root.join("../outside"),
            "measurement-root" => {
                extracts[0] = complete.builds.lanes[asset].builds[0]
                    .measurement
                    .join("new");
            }
            _ => unreachable!(),
        }
        let recovery = RecoveryByteReadbackRequestV2 {
            recovery: selected_recovery,
            qualification: &qualification,
        };
        let error = repackage_verified_package_v2(&VerifiedPackageRepackageRequestV2 {
            recovery: &recovery,
            asset: selected_asset,
            extraction_roots: extracts,
            output_dirs: outputs,
        })
        .unwrap_err();
        let expected = match mutation {
            "unknown-lane" => "recovery asset is not a selected indexed lane",
            "replay" => "compatibility replay cannot execute packaging recovery",
            "shared-root" | "release-root" => {
                "recovery destinations overlap each other or original evidence"
            }
            "existing" => "package extraction root already exists and cannot be adopted",
            "symlink-parent" | "measurement-root" => {
                "recovery destination parent has a symlink component"
            }
            "non-normalized" => "recovery destinations must have normalized absolute paths",
            _ => unreachable!(),
        };
        let diagnostics = error.diagnostics();
        assert_eq!(diagnostics.diagnostics.len(), 1, "{mutation}: {error}");
        assert_eq!(diagnostics.diagnostics[0].message, expected, "{mutation}");
        assert_eq!(
            diagnostics.diagnostics[0].code.to_string(),
            if mutation == "existing" {
                "AX0602"
            } else {
                "AX0901"
            },
            "{mutation}"
        );
        assert!(!root.join("extract-a").exists(), "{mutation}");
        assert!(!root.join("extract-b").exists(), "{mutation}");
        assert!(!root.join("package-b").exists(), "{mutation}");
        assert_eq!(snapshot_request(&complete), before, "{mutation}");
    }
}

#[test]
fn recovery_v2_repackage_preserves_original_evidence_through_filesystem_aliases() {
    use crate::repackage_v2::{repackage_verified_package_v2, VerifiedPackageRepackageRequestV2};

    for (original, alternate) in [
        ("SelectedRoot", "selectedroot"),
        ("Caf\u{e9}Root", "Cafe\u{301}Root"),
    ] {
        let relocated = tempfile::tempdir().unwrap();
        let parent = relocated.path().canonicalize().unwrap();
        let original = parent.join(original);
        fs::create_dir(&original).unwrap();
        let alternate = parent.join(alternate);
        if !alternate.is_dir() {
            // These aliases do not exist on case/normalization-sensitive hosts.
            continue;
        }
        let mut prepared = prepare_release();
        for entry in fs::read_dir(&prepared.packages.directory).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            fs::copy(entry.path(), original.join(entry.file_name())).unwrap();
        }
        prepared.packages.directory = original;
        let fixture = prepare_recovery_fixture_from_release_v2(prepared);
        let complete = fixture.complete_request();
        let qualification = recovery_qualification_request(
            &fixture,
            &complete,
            &fixture.evidence_bytes,
            &fixture.policy,
        );
        let evidence = QualificationEvidenceV2::parse(&fixture.evidence_bytes).unwrap();
        let request = recovery_request_v2(
            &fixture.evidence_bytes,
            RecoveryOperation::PackagingRecovery,
            FailedStage::Packaging,
            Some(fresh_recovery_handoff(
                &evidence.source_run,
                ReleaseHandoffState::Absent,
            )),
        );
        let recovery = RecoveryByteReadbackRequestV2 {
            recovery: &request,
            qualification: &qualification,
        };
        let asset = complete.builds.packages.index.artifacts()[0].asset();
        let before = snapshot_request(&complete);
        let output_root = tempfile::tempdir().unwrap();
        let root = output_root.path().canonicalize().unwrap();
        let selected_output = alternate.join("new-package");
        let error = repackage_verified_package_v2(&VerifiedPackageRepackageRequestV2 {
            recovery: &recovery,
            asset,
            extraction_roots: [root.join("extract-a"), root.join("extract-b")],
            output_dirs: [selected_output.clone(), root.join("package-b")],
        })
        .unwrap_err();
        let diagnostics = error.diagnostics();
        assert_eq!(diagnostics.diagnostics.len(), 1);
        assert_eq!(diagnostics.diagnostics[0].code.to_string(), "AX0901");
        assert_eq!(
            diagnostics.diagnostics[0].message,
            "recovery destinations overlap each other or original evidence"
        );
        assert!(!selected_output.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        assert_eq!(snapshot_request(&complete), before);
    }
}

#[test]
fn recovery_v2_repackage_reacquires_original_measurements_before_writes() {
    use crate::repackage_v2::{repackage_verified_package_v2, VerifiedPackageRepackageRequestV2};

    let fixture = prepare_recovery_fixture_v2();
    let complete = fixture.complete_request();
    let qualification = recovery_qualification_request(
        &fixture,
        &complete,
        &fixture.evidence_bytes,
        &fixture.policy,
    );
    let evidence = QualificationEvidenceV2::parse(&fixture.evidence_bytes).unwrap();
    let recovery = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(fresh_recovery_handoff(
            &evidence.source_run,
            ReleaseHandoffState::Absent,
        )),
    );
    let recovery = RecoveryByteReadbackRequestV2 {
        recovery: &recovery,
        qualification: &qualification,
    };
    let selected = complete.builds.packages.index.artifacts()[0].asset();
    let measurement = &complete.builds.lanes[selected].builds[1].measurement;
    let mut modified = fs::read(measurement).unwrap();
    modified.push(b'\n');
    fs::write(measurement, modified).unwrap();
    let before = snapshot_request(&complete);
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let error = repackage_verified_package_v2(&VerifiedPackageRepackageRequestV2 {
        recovery: &recovery,
        asset: selected,
        extraction_roots: [root.join("extract-a"), root.join("extract-b")],
        output_dirs: [root.join("package-a"), root.join("package-b")],
    })
    .unwrap_err();
    let diagnostics = error.diagnostics();
    assert_eq!(diagnostics.diagnostics.len(), 1, "{error}");
    assert_eq!(
        diagnostics.diagnostics[0].message,
        "portable package measurement differs from selected raw bytes"
    );
    assert_eq!(diagnostics.diagnostics[0].code.to_string(), "AX0801");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    assert_eq!(snapshot_request(&complete), before);
}

#[test]
fn recovery_v2_accepts_packaging_handoff_and_harness_replay_as_opaque_byte_readbacks() {
    let fixture = prepare_recovery_fixture_v2();
    let complete = fixture.complete_request();
    let qualification = recovery_qualification_request(
        &fixture,
        &complete,
        &fixture.evidence_bytes,
        &fixture.policy,
    );
    let before = snapshot_request(&complete);
    let evidence = QualificationEvidenceV2::parse(&fixture.evidence_bytes).unwrap();

    let handoff = fresh_recovery_handoff(&evidence.source_run, ReleaseHandoffState::Absent);
    assert_eq!(
        handoff.tag.peeled_commit,
        evidence.source_run.producer_commit
    );
    assert_ne!(handoff.tag.tag_object, evidence.source_run.tag_object);
    let packaging = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(handoff.clone()),
    );
    let recovered = readback_recovery_bytes_v2(&RecoveryByteReadbackRequestV2 {
        recovery: &packaging,
        qualification: &qualification,
    })
    .unwrap();
    assert_eq!(
        recovered.decision(),
        &RecoveryDecision::Repackage {
            release_id: handoff.release_id,
            tag: handoff.tag,
        }
    );
    assert_eq!(recovered.qualification().evidence().lanes.len(), 3);
    assert_eq!(
        recovered.qualification().complete().builds().lanes().len(),
        3
    );
    assert_eq!(recovered.qualification().complete().lanes().len(), 3);

    let replay = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::CompatibilityReplay,
        FailedStage::CompatibilityHarness,
        None,
    );
    let replayed = readback_recovery_bytes_v2(&RecoveryByteReadbackRequestV2 {
        recovery: &replay,
        qualification: &qualification,
    })
    .unwrap();
    assert_eq!(replayed.decision(), &RecoveryDecision::ReplayCompatibility);
    assert_eq!(replayed.qualification().complete().lanes().len(), 3);
    assert_eq!(snapshot_request(&complete), before);
}

#[test]
fn recovery_v2_rejects_observation_operation_and_handoff_substitutions_without_writes() {
    let fixture = prepare_recovery_fixture_v2();
    let complete = fixture.complete_request();
    let qualification = recovery_qualification_request(
        &fixture,
        &complete,
        &fixture.evidence_bytes,
        &fixture.policy,
    );
    let evidence = QualificationEvidenceV2::parse(&fixture.evidence_bytes).unwrap();
    let valid = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::CompatibilityReplay,
        FailedStage::CompatibilityHarness,
        None,
    );

    let mut changed = valid.clone();
    changed.qualification_sha256 = sha256_bytes(b"another raw qualification document");
    assert_recovery_rejected_without_mutation(
        &changed,
        &qualification,
        "recovery request v2 differs from selected qualification bytes",
        "AX0901",
    );

    for change_attempt in [true, false] {
        let mut changed = valid.clone();
        if change_attempt {
            changed.observed_run.run_attempt += 1;
        } else {
            changed.observed_run.run_id += 1;
        }
        assert_recovery_rejected_without_mutation(
            &changed,
            &qualification,
            "recovery provider observation differs from the exact source run attempt",
            "AX0901",
        );
    }

    let mut missing_attestation = valid.clone();
    missing_attestation.observed_attestation = None;
    assert_recovery_rejected_without_mutation(
        &missing_attestation,
        &qualification,
        "recovery requires the exact external attestation observation",
        "AX0901",
    );

    let mut swapped_signer = valid.clone();
    swapped_signer.observed_attestation.as_mut().unwrap().signer = "another-signer".into();
    assert_recovery_rejected_without_mutation(
        &swapped_signer,
        &qualification,
        "recovery requires the exact external attestation observation",
        "AX0901",
    );

    let mut swapped_tag = valid.clone();
    swapped_tag.source_tag.name = "another-source-tag".into();
    assert_recovery_rejected_without_mutation(
        &swapped_tag,
        &qualification,
        "observed source tag does not match the immutable qualification identity",
        "AX0901",
    );

    for change_tag_object in [true, false] {
        let mut changed = valid.clone();
        if change_tag_object {
            changed.source_tag.tag_object =
                GitObjectId::try_from("dddddddddddddddddddddddddddddddddddddddd".to_owned())
                    .unwrap();
        } else {
            changed.source_tag.peeled_commit =
                GitObjectId::try_from("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned())
                    .unwrap();
        }
        assert_recovery_rejected_without_mutation(
            &changed,
            &qualification,
            "observed source tag does not match the immutable qualification identity",
            "AX0901",
        );
    }

    for failed_stage in [
        FailedStage::Compiler,
        FailedStage::Comparison,
        FailedStage::Compatibility,
    ] {
        let changed = recovery_request_v2(
            &fixture.evidence_bytes,
            RecoveryOperation::PackagingRecovery,
            failed_stage,
            Some(fresh_recovery_handoff(
                &evidence.source_run,
                ReleaseHandoffState::Absent,
            )),
        );
        assert_eq!(changed.operation, RecoveryOperation::PackagingRecovery);
        assert_recovery_rejected_without_mutation(
            &changed,
            &qualification,
            "recovery operation does not match the sole measured failed stage",
            "AX0901",
        );
    }

    let wrong_replay_stage = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::CompatibilityReplay,
        FailedStage::Compatibility,
        None,
    );
    assert_recovery_rejected_without_mutation(
        &wrong_replay_stage,
        &qualification,
        "recovery operation does not match the sole measured failed stage",
        "AX0901",
    );

    let packaging_without_handoff = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        None,
    );
    assert_recovery_rejected_without_mutation(
        &packaging_without_handoff,
        &qualification,
        "packaging recovery requires an observed fresh absent handoff",
        "AX0901",
    );

    let replay_with_handoff = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::CompatibilityReplay,
        FailedStage::CompatibilityHarness,
        Some(fresh_recovery_handoff(
            &evidence.source_run,
            ReleaseHandoffState::Absent,
        )),
    );
    assert_recovery_rejected_without_mutation(
        &replay_with_handoff,
        &qualification,
        "compatibility replay has no release handoff authority",
        "AX0901",
    );

    for state in [ReleaseHandoffState::Draft, ReleaseHandoffState::Published] {
        let invalid = recovery_request_v2(
            &fixture.evidence_bytes,
            RecoveryOperation::PackagingRecovery,
            FailedStage::Packaging,
            Some(fresh_recovery_handoff(&evidence.source_run, state)),
        );
        assert_recovery_rejected_without_mutation(
            &invalid,
            &qualification,
            "recovery handoff conflicts with the original identity or an existing release",
            "AX0901",
        );
    }

    let mut reused_source_tag =
        fresh_recovery_handoff(&evidence.source_run, ReleaseHandoffState::Absent);
    reused_source_tag.tag.name = evidence.source_run.source_tag.clone();
    let invalid = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(reused_source_tag),
    );
    assert_recovery_rejected_without_mutation(
        &invalid,
        &qualification,
        "recovery handoff conflicts with the original identity or an existing release",
        "AX0901",
    );

    let mut reused_tag_object =
        fresh_recovery_handoff(&evidence.source_run, ReleaseHandoffState::Absent);
    reused_tag_object.tag.tag_object = evidence.source_run.tag_object.clone();
    let invalid = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(reused_tag_object),
    );
    assert_recovery_rejected_without_mutation(
        &invalid,
        &qualification,
        "recovery handoff conflicts with the original identity or an existing release",
        "AX0901",
    );

    let mut wrong_peel = fresh_recovery_handoff(&evidence.source_run, ReleaseHandoffState::Absent);
    wrong_peel.tag.peeled_commit =
        GitObjectId::try_from("cccccccccccccccccccccccccccccccccccccccc".to_owned()).unwrap();
    let invalid = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(wrong_peel),
    );
    assert_recovery_rejected_without_mutation(
        &invalid,
        &qualification,
        "recovery handoff conflicts with the original identity or an existing release",
        "AX0901",
    );

    let mut reused_release =
        fresh_recovery_handoff(&evidence.source_run, ReleaseHandoffState::Absent);
    reused_release.release_id = evidence.release.release_id;
    let invalid = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(reused_release),
    );
    assert_recovery_rejected_without_mutation(
        &invalid,
        &qualification,
        "recovery handoff conflicts with the original identity or an existing release",
        "AX0901",
    );
}

#[test]
fn recovery_v2_rejects_expired_diagnostic_and_subset_qualification_bytes() {
    let fixture = prepare_recovery_fixture_v2();
    let complete = fixture.complete_request();
    let valid = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(fresh_recovery_handoff(
            &QualificationEvidenceV2::parse(&fixture.evidence_bytes)
                .unwrap()
                .source_run,
            ReleaseHandoffState::Absent,
        )),
    );

    let expired_policy = EvidencePolicy {
        now: 300,
        ..fixture.policy.clone()
    };
    let expired_qualification = recovery_qualification_request(
        &fixture,
        &complete,
        &fixture.evidence_bytes,
        &expired_policy,
    );
    assert_recovery_rejected_without_mutation(
        &valid,
        &expired_qualification,
        "qualification evidence v2 is not valid at the policy epoch",
        "AX0901",
    );

    let mut diagnostic: Value = serde_json::from_slice(&fixture.evidence_bytes).unwrap();
    diagnostic["coverage"] = json!("diagnostic");
    let diagnostic_bytes = serde_json::to_vec(&diagnostic).unwrap();
    let diagnostic_qualification =
        recovery_qualification_request(&fixture, &complete, &diagnostic_bytes, &fixture.policy);
    let diagnostic_recovery = recovery_request_v2(
        &diagnostic_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        valid.handoff.clone(),
    );
    assert_recovery_rejected_without_mutation(
        &diagnostic_recovery,
        &diagnostic_qualification,
        "qualification byte read-back requires complete release-candidate coverage",
        "AX0901",
    );

    let mut subset: Value = serde_json::from_slice(&fixture.evidence_bytes).unwrap();
    subset["lanes"].as_array_mut().unwrap().pop();
    let subset_bytes = serde_json::to_vec(&subset).unwrap();
    let subset_qualification =
        recovery_qualification_request(&fixture, &complete, &subset_bytes, &fixture.policy);
    let subset_recovery = recovery_request_v2(
        &subset_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        valid.handoff,
    );
    assert_recovery_rejected_without_mutation(
        &subset_recovery,
        &subset_qualification,
        "qualification evidence v2 lacks the complete input-derived matrix",
        "AX0901",
    );
}

#[test]
fn recovery_v2_propagates_changed_package_and_report_byte_diagnostics_without_writes() {
    let fixture = prepare_recovery_fixture_v2();
    let complete = fixture.complete_request();
    let qualification = recovery_qualification_request(
        &fixture,
        &complete,
        &fixture.evidence_bytes,
        &fixture.policy,
    );
    let recovery = recovery_request_v2(
        &fixture.evidence_bytes,
        RecoveryOperation::PackagingRecovery,
        FailedStage::Packaging,
        Some(fresh_recovery_handoff(
            &QualificationEvidenceV2::parse(&fixture.evidence_bytes)
                .unwrap()
                .source_run,
            ReleaseHandoffState::Absent,
        )),
    );
    let asset = asset_for_host(&fixture.builds, "linux-aarch64").to_owned();

    let package_member = complete.builds.lanes[&asset].builds[0]
        .package_dir
        .join(format!("{asset}.manifest.json"));
    let original_package = fs::read(&package_member).unwrap();
    let changed_package = b"tampered A package manifest".to_vec();
    fs::write(&package_member, &changed_package).unwrap();
    assert_recovery_rejected_without_mutation(
        &recovery,
        &qualification,
        "external package manifest is not valid JSON",
        "AX0602",
    );
    fs::write(&package_member, original_package).unwrap();

    let report = complete.builds.lanes[&asset].comparison.clone();
    let original_report = fs::read(&report).unwrap();
    let changed_report = b"tampered comparison report".to_vec();
    fs::write(&report, &changed_report).unwrap();
    assert_recovery_rejected_without_mutation(
        &recovery,
        &qualification,
        "release build evidence differs from independently selected raw bytes",
        "AX0702",
    );
    fs::write(report, original_report).unwrap();
}
