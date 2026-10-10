//! Synthetic complete V2 build-evidence joins. No compiler or producer runs.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, Sha256Digest};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::portable_tests::{
    copy_package, guarded_gnu_package_for_host, guarded_package_for_host, GuardedPackage,
};
use super::{
    readback_release_builds_v2, ReleaseBuildLaneRequestV2, ReleaseBuildReadbackRequestV2,
    ReleaseBuildSideRequestV2,
};
use crate::release_attestation_manifest_v2::write_attestation_manifest_v2;
use crate::release_checksums_v2_writer::write_final_checksums_v2;
use crate::release_index::{compare_package_sets, write_package_comparison_report};
use crate::release_index_v2::PROVENANCE_NAME;
use crate::release_index_v2_builder::MeasuredReleaseIndexRequestV2;
use crate::release_index_v2_readback::IndexedPackageReadbackRequestV2;
use crate::release_index_v2_writer::write_measured_index_v2;
use crate::release_inputs::{ReleaseInputs, ACTIVE_HOSTS};

const RELEASE_ID: &str = "finished-candidate-test";
const BASE_URL: &str = "https://example.invalid/releases/portable-build-v2";
const INPUTS_NAME: &str = "toolchain-release-inputs-v2.json";
const MANIFEST_SCHEMA_NAME: &str = "toolchain-manifest-v2.schema.json";
const TREE_FIXTURE_NAME: &str = "tree-digest-v1.fixture.json";

struct InputFixture {
    inputs: ReleaseInputs,
    collection: Vec<u8>,
    documents: BTreeMap<String, Vec<u8>>,
}

struct ExpectedLane {
    asset: String,
    measurements: [Sha256Digest; 2],
    build_results: [Sha256Digest; 2],
    finished_receipts: [Sha256Digest; 2],
    comparison: Sha256Digest,
}

pub(super) struct PreparedRelease {
    _release_root: TempDir,
    _transport_root: TempDir,
    pub(super) packages: IndexedPackageReadbackRequestV2,
    subject_manifest: PathBuf,
    subject_manifest_sha256: Sha256Digest,
    pub(super) lanes: BTreeMap<String, ReleaseBuildLaneRequestV2>,
    expected_lanes: BTreeMap<String, ExpectedLane>,
    checksums_sha256: Sha256Digest,
    pub(super) owners: Vec<GuardedPackage>,
    pub(super) original_roots: Vec<PathBuf>,
}

impl PreparedRelease {
    pub(super) fn request(&self) -> ReleaseBuildReadbackRequestV2 {
        ReleaseBuildReadbackRequestV2 {
            packages: self.packages.clone(),
            subject_manifest: self.subject_manifest.clone(),
            subject_manifest_sha256: self.subject_manifest_sha256.clone(),
            lanes: self.lanes.clone(),
        }
    }

    fn release_dir(&self) -> &Path {
        &self.packages.directory
    }
}

fn input_fixture() -> InputFixture {
    input_fixture_for(
        "llvm-pc",
        super::tests::fixture_source_lock(),
        super::tests::fixture_profiles(),
    )
}

fn gnu_input_fixture() -> InputFixture {
    input_fixture_for(
        "gnu-rv32",
        super::tests::fixture_gnu_source_lock(),
        super::tests::fixture_gnu_profiles(),
    )
}

fn input_fixture_for(group_id: &str, source_lock: Vec<u8>, profiles: Vec<u8>) -> InputFixture {
    let recipe = super::tests::fixture_recipe(&source_lock, &profiles);
    let mut documents = BTreeMap::new();
    documents.insert("recipe.json".to_owned(), recipe);
    documents.insert("source-lock.json".to_owned(), source_lock);
    documents.insert("profiles.json".to_owned(), profiles);
    let collection = serde_json::to_vec(&json!({
        "schema": "aros-toolchain-release-inputs-v2",
        "producer_commit": "3333333333333333333333333333333333333333",
        "tools_commit": "5555555555555555555555555555555555555555",
        "hosts": ACTIVE_HOSTS,
        "groups": [{
            "id": group_id,
            "recipe": {
                "file": "recipe.json",
                "sha256": sha256_bytes(documents.get("recipe.json").unwrap()).as_str(),
            },
            "source_lock": {
                "file": "source-lock.json",
                "sha256": sha256_bytes(documents.get("source-lock.json").unwrap()).as_str(),
            },
            "profiles": {
                "file": "profiles.json",
                "sha256": sha256_bytes(documents.get("profiles.json").unwrap()).as_str(),
            },
        }],
    }))
    .unwrap();
    let inputs = ReleaseInputs::parse(&collection, &documents).unwrap();
    InputFixture {
        inputs,
        collection,
        documents,
    }
}

pub(super) fn prepare_release() -> PreparedRelease {
    prepare_release_for_family(false)
}

pub(super) fn prepare_gnu_release() -> PreparedRelease {
    prepare_release_for_family(true)
}

fn prepare_release_for_family(gnu: bool) -> PreparedRelease {
    let input = if gnu {
        gnu_input_fixture()
    } else {
        input_fixture()
    };
    let release_root = tempfile::tempdir().unwrap();
    let release_dir = release_root.path().canonicalize().unwrap();
    for (name, bytes) in &input.documents {
        fs::write(release_dir.join(name), bytes).unwrap();
    }
    fs::write(release_dir.join(INPUTS_NAME), &input.collection).unwrap();
    fs::write(
        release_dir.join(MANIFEST_SCHEMA_NAME),
        include_bytes!("../../../aros-common/tests/fixtures/toolchain-manifest-v2.schema.json"),
    )
    .unwrap();
    fs::write(
        release_dir.join(TREE_FIXTURE_NAME),
        include_bytes!("../../../aros-common/tests/fixtures/tree-digest-v1.fixture.json"),
    )
    .unwrap();

    let transport_root = tempfile::tempdir().unwrap();
    let transport_dir = transport_root.path().canonicalize().unwrap();
    let package_downloads = transport_dir.join("packages");
    let measurements = transport_dir.join("measurements");
    let comparisons = transport_dir.join("comparisons");
    fs::create_dir(&package_downloads).unwrap();
    fs::create_dir(&measurements).unwrap();
    fs::create_dir(&comparisons).unwrap();

    let mut owners = Vec::new();
    let mut lanes = BTreeMap::new();
    let mut expected_lanes = BTreeMap::new();
    let mut build_environments = BTreeMap::new();
    let mut required_paths = BTreeMap::new();

    for (host_index, host) in ACTIVE_HOSTS.iter().copied().enumerate() {
        let left = if gnu {
            guarded_gnu_package_for_host(host)
        } else {
            guarded_package_for_host(host)
        };
        let right = if gnu {
            guarded_gnu_package_for_host(host)
        } else {
            guarded_package_for_host(host)
        };
        let asset = left
            .package
            .package_output()
            .archive
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            right.package.package_output().archive.file_name().unwrap(),
            asset.as_str()
        );

        copy_package(
            left.package.package_output().output_dir.as_path(),
            &package_downloads.join(format!("{host_index}-left")),
        );
        copy_package(
            right.package.package_output().output_dir.as_path(),
            &package_downloads.join(format!("{host_index}-right")),
        );
        let left_package_dir = package_downloads
            .join(format!("{host_index}-left"))
            .canonicalize()
            .unwrap();
        let right_package_dir = package_downloads
            .join(format!("{host_index}-right"))
            .canonicalize()
            .unwrap();
        copy_package_to_flat_release(
            left.package.package_output().output_dir.as_path(),
            &release_dir,
        );

        let left_measurement = measurements.join(format!("{host_index}-left.json"));
        let right_measurement = measurements.join(format!("{host_index}-right.json"));
        fs::write(&left_measurement, left.measurement.bytes()).unwrap();
        fs::write(&right_measurement, right.measurement.bytes()).unwrap();
        let left_measurement = left_measurement.canonicalize().unwrap();
        let right_measurement = right_measurement.canonicalize().unwrap();

        let comparison = compare_package_sets(
            &left.package.package_output().output_dir,
            &right.package.package_output().output_dir,
        )
        .unwrap();
        let comparison_path = comparisons.join(format!("{asset}.comparison.json"));
        let report = write_package_comparison_report(&comparison_path, &comparison).unwrap();
        let comparison_sha256 = report.sha256.clone();
        let comparison_path = report.path.canonicalize().unwrap();

        let left_build_result = sha256_bytes(&left.candidate.receipts[7].1);
        let right_build_result = sha256_bytes(&right.candidate.receipts[7].1);
        let left_finished = left.candidate.receipt_sha256().clone();
        let right_finished = right.candidate.receipt_sha256().clone();
        let left_measurement_sha256 = left.measurement.sha256().clone();
        let right_measurement_sha256 = right.measurement.sha256().clone();
        let left_identity = left.identity.clone();
        let right_identity = right.identity.clone();

        lanes.insert(
            asset.clone(),
            ReleaseBuildLaneRequestV2 {
                builds: [
                    ReleaseBuildSideRequestV2 {
                        package_dir: left_package_dir,
                        measurement: left_measurement,
                        measurement_sha256: left_measurement_sha256.clone(),
                        identity: left_identity,
                    },
                    ReleaseBuildSideRequestV2 {
                        package_dir: right_package_dir,
                        measurement: right_measurement,
                        measurement_sha256: right_measurement_sha256.clone(),
                        identity: right_identity,
                    },
                ],
                comparison: comparison_path,
                comparison_sha256: comparison_sha256.clone(),
            },
        );
        expected_lanes.insert(
            asset.clone(),
            ExpectedLane {
                asset: asset.clone(),
                measurements: [left_measurement_sha256, right_measurement_sha256],
                build_results: [left_build_result, right_build_result],
                finished_receipts: [left_finished, right_finished],
                comparison: comparison_sha256,
            },
        );

        let environment = left.package_request.build_environment.clone();
        build_environments.insert(asset.clone(), environment);
        required_paths.insert(
            asset,
            vec![if gnu {
                "bin/fixture-c".to_owned()
            } else {
                "bin/clang".to_owned()
            }],
        );
        owners.push(left);
        owners.push(right);
    }

    let measured_request = MeasuredReleaseIndexRequestV2 {
        directory: release_dir.clone(),
        inputs: input.inputs.clone(),
        release_id: RELEASE_ID.into(),
        base_url: BASE_URL.into(),
        build_environments: build_environments.clone(),
        required_paths,
        forbidden_prefixes: Vec::new(),
    };
    let written_index = write_measured_index_v2(&measured_request).unwrap();
    let packages = IndexedPackageReadbackRequestV2 {
        directory: release_dir,
        inputs: input.inputs,
        index: written_index.index().clone(),
        build_environments,
        forbidden_prefixes: Vec::new(),
    };

    let subject_manifest = transport_dir.join("subjects.sha256");
    let subject = write_attestation_manifest_v2(&packages, &subject_manifest).unwrap();
    let subject_manifest = subject_manifest.canonicalize().unwrap();
    let subject_manifest_sha256 = subject.sha256().clone();

    fs::write(
        packages.directory.join(PROVENANCE_NAME),
        b"synthetic unsigned provenance evidence\n",
    )
    .unwrap();
    let checksums = write_final_checksums_v2(&packages).unwrap();

    let mut original_roots = Vec::new();
    for owner in &owners {
        original_roots.push(owner.candidate.work_dir.parent().unwrap().to_path_buf());
        original_roots.push(
            owner
                .package_request
                .output_dir
                .parent()
                .unwrap()
                .to_path_buf(),
        );
        original_roots.push(
            owner.candidate.receipts[7]
                .0
                .parent()
                .unwrap()
                .to_path_buf(),
        );
    }
    original_roots.sort();
    original_roots.dedup();

    PreparedRelease {
        _release_root: release_root,
        _transport_root: transport_root,
        packages,
        subject_manifest,
        subject_manifest_sha256,
        lanes,
        expected_lanes,
        checksums_sha256: checksums.sha256().clone(),
        owners,
        original_roots,
    }
}

fn copy_package_to_flat_release(source: &Path, destination: &Path) {
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        let output = destination.join(entry.file_name());
        assert!(
            !output.exists(),
            "duplicate package member {}",
            output.display()
        );
        fs::copy(entry.path(), output).unwrap();
    }
}

fn assert_message(error: &crate::ContractError, expected: &str) {
    let diagnostics = &error.diagnostics().diagnostics;
    assert_eq!(diagnostics.len(), 1, "{error}");
    assert_eq!(diagnostics[0].message, expected, "{error}");
}

fn snapshot_directory(directory: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            snapshot_directory(&path, files);
        } else {
            assert!(file_type.is_file() || file_type.is_symlink());
            files.insert(path.clone(), fs::read(path).unwrap());
        }
    }
}

fn snapshot_request(request: &ReleaseBuildReadbackRequestV2) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    snapshot_directory(&request.packages.directory, &mut files);
    for lane in request.lanes.values() {
        for side in &lane.builds {
            snapshot_directory(&side.package_dir, &mut files);
            files.insert(
                side.measurement.clone(),
                fs::read(&side.measurement).unwrap(),
            );
        }
        files.insert(lane.comparison.clone(), fs::read(&lane.comparison).unwrap());
    }
    files.insert(
        request.subject_manifest.clone(),
        fs::read(&request.subject_manifest).unwrap(),
    );
    files
}

fn assert_rejected_without_mutation(request: &ReleaseBuildReadbackRequestV2, expected: &str) {
    let before = snapshot_request(request);
    let error = readback_release_builds_v2(request).unwrap_err();
    assert_message(&error, expected);
    assert_eq!(
        snapshot_request(request),
        before,
        "read-back changed an input"
    );
}

#[test]
fn reads_complete_three_host_v2_build_evidence_after_dropping_original_roots() {
    let mut prepared = prepare_release();
    let request = prepared.request();
    let original_roots = prepared.original_roots.clone();
    prepared.owners.clear();
    for root in &original_roots {
        assert!(
            !root.exists(),
            "original build root survived: {}",
            root.display()
        );
    }

    let readback = readback_release_builds_v2(&request).unwrap();
    assert_eq!(readback.checksums_sha256(), &prepared.checksums_sha256);
    assert_eq!(
        readback.subject_manifest_sha256(),
        &prepared.subject_manifest_sha256
    );
    assert_eq!(readback.lanes().len(), ACTIVE_HOSTS.len());
    assert_eq!(readback.lanes().len(), 3);
    for (asset, expected) in &prepared.expected_lanes {
        let lane = readback
            .lanes()
            .iter()
            .find(|lane| lane.asset() == asset)
            .unwrap();
        assert_eq!(lane.asset(), expected.asset);
        assert_eq!(
            lane.measurement_sha256(),
            [&expected.measurements[0], &expected.measurements[1]]
        );
        assert_eq!(
            lane.build_result_sha256(),
            [&expected.build_results[0], &expected.build_results[1]]
        );
        assert_eq!(
            lane.finished_receipt_sha256(),
            [
                &expected.finished_receipts[0],
                &expected.finished_receipts[1]
            ]
        );
        assert_eq!(lane.comparison_sha256(), &expected.comparison);
    }
}

#[test]
fn rejects_incomplete_or_extra_lane_sets_and_same_package_pair() {
    let prepared = prepare_release();
    let mut request = prepared.request();
    let asset = request.lanes.keys().next().unwrap().clone();
    request.lanes.remove(&asset);
    assert_rejected_without_mutation(
        &request,
        "release build selections must exactly cover the input-derived index",
    );

    let mut request = prepared.request();
    let clone = request.lanes.values().next().unwrap().clone();
    request
        .lanes
        .insert("unindexed-package.tar.xz".into(), clone);
    assert_rejected_without_mutation(
        &request,
        "release build selections must exactly cover the input-derived index",
    );

    let mut request = prepared.request();
    let lane = request.lanes.values_mut().next().unwrap();
    lane.builds[1].package_dir = lane.builds[0].package_dir.clone();
    assert_rejected_without_mutation(
        &request,
        "release build package directories must be canonical and nonoverlapping",
    );
}

#[test]
fn rejects_reused_linked_or_symlinked_evidence_paths() {
    let prepared = prepare_release();
    let asset = prepared.lanes.keys().next().unwrap().clone();

    let mut request = prepared.request();
    let reused_path = request.lanes[&asset].builds[0].measurement.clone();
    request.lanes.get_mut(&asset).unwrap().builds[1].measurement = reused_path;
    assert_rejected_without_mutation(
        &request,
        "release build evidence file is reused across selections",
    );

    let mut request = prepared.request();
    let source = request.lanes[&asset].builds[0].measurement.clone();
    let hard_link = source
        .parent()
        .unwrap()
        .join("selected-measurement-hard-link.json");
    fs::hard_link(&source, &hard_link).unwrap();
    request.lanes.get_mut(&asset).unwrap().builds[1].measurement = hard_link.clone();
    assert_rejected_without_mutation(
        &request,
        "release build evidence must be singly linked regular files",
    );
    fs::remove_file(hard_link).unwrap();

    let mut request = prepared.request();
    let source = request.lanes[&asset].builds[0].measurement.clone();
    let leaf_link = source
        .parent()
        .unwrap()
        .join("selected-measurement-leaf-link.json");
    symlink(&source, &leaf_link).unwrap();
    request.lanes.get_mut(&asset).unwrap().builds[1].measurement = leaf_link;
    assert_rejected_without_mutation(
        &request,
        "release build evidence must be a no-follow regular file",
    );

    let mut request = prepared.request();
    let source = request.lanes[&asset].builds[0].measurement.clone();
    let source_parent = source.parent().unwrap();
    let parent_link = source_parent
        .parent()
        .unwrap()
        .join("measurement-parent-link");
    symlink(source_parent, &parent_link).unwrap();
    let linked_measurement = parent_link.join(source.file_name().unwrap());
    request.lanes.get_mut(&asset).unwrap().builds[1].measurement = linked_measurement;
    assert_rejected_without_mutation(
        &request,
        "release directory and every ancestor must be real directories",
    );
}

#[test]
fn rejects_wrong_selected_identity_input_host_environment_and_comparison_digest() {
    let prepared = prepare_release();
    let asset = prepared.lanes.keys().next().unwrap().clone();

    let mut request = prepared.request();
    request.lanes.get_mut(&asset).unwrap().builds[0]
        .identity
        .executor
        .binary_sha256 = sha256_bytes(b"wrong selected executor");
    assert_rejected_without_mutation(
        &request,
        "portable package build result differs from selected executor or lane",
    );

    let mut request = prepared.request();
    request.lanes.get_mut(&asset).unwrap().builds[0]
        .identity
        .recipe_sha256 = sha256_bytes(b"another selected recipe");
    assert_rejected_without_mutation(
        &request,
        "portable package build result differs from selected executor or lane",
    );

    let mut request = prepared.request();
    request.lanes.get_mut(&asset).unwrap().builds[0]
        .identity
        .host = if asset.contains("linux-x86_64") {
        "linux-aarch64"
    } else {
        "linux-x86_64"
    };
    assert_rejected_without_mutation(
        &request,
        "native build result differs from the selected operation, roots, host or local state",
    );

    let mut request = prepared.request();
    let environment = request.packages.build_environments.get_mut(&asset).unwrap();
    environment.insert(
        "compiler-observation".into(),
        json!("not independently selected"),
    );
    assert_rejected_without_mutation(
        &request,
        "native compiler-family package failed read-back verification",
    );

    let mut request = prepared.request();
    request.lanes.get_mut(&asset).unwrap().comparison_sha256 =
        sha256_bytes(b"wrong comparison digest");
    assert_rejected_without_mutation(
        &request,
        "release build evidence differs from independently selected raw bytes",
    );
}

#[test]
fn rejects_bad_comparison_reports_even_when_their_raw_digest_is_selected() {
    let prepared = prepare_release();
    let asset = prepared.lanes.keys().next().unwrap().clone();
    let original_request = prepared.request();
    let report_path = original_request.lanes[&asset].comparison.clone();
    let original_bytes = fs::read(&report_path).unwrap();

    let duplicate = String::from_utf8(original_bytes.clone())
        .unwrap()
        .replacen(
            "\"operation\":\"compare\"",
            "\"operation\":\"compare\",\"operation\":\"compare\"",
            1,
        )
        .into_bytes();
    assert_ne!(duplicate, original_bytes);
    fs::write(&report_path, &duplicate).unwrap();
    let mut request = prepared.request();
    request.lanes.get_mut(&asset).unwrap().comparison_sha256 = sha256_bytes(&duplicate);
    assert_rejected_without_mutation(
        &request,
        "native comparison report is not a closed JSON document",
    );
    fs::write(&report_path, &original_bytes).unwrap();

    let mut changed: Value = serde_json::from_slice(&original_bytes).unwrap();
    changed["members"][0]["sha256"] = json!(sha256_bytes(b"self-rehashed changed member"));
    changed["package_set_sha256"] = json!(sha256_bytes(
        &crate::canonical::bytes(&changed["members"]).unwrap()
    ));
    let mut changed_bytes = crate::canonical::bytes(&changed).unwrap();
    changed_bytes.push(b'\n');
    fs::write(&report_path, &changed_bytes).unwrap();
    let mut request = prepared.request();
    request.lanes.get_mut(&asset).unwrap().comparison_sha256 = sha256_bytes(&changed_bytes);
    assert_rejected_without_mutation(
        &request,
        "release build comparison differs from measured A/B package bytes",
    );
    fs::write(&report_path, original_bytes).unwrap();
}

#[test]
fn rejects_fully_rejoined_collector_substitution_against_downloaded_package() {
    let prepared = prepare_release();
    let mut request = prepared.request();
    let asset = request.lanes.keys().next().unwrap().clone();
    let side = &mut request.lanes.get_mut(&asset).unwrap().builds[0];
    let bytes = super::portable_tests::mutate_collector_claim_consistently(
        &fs::read(&side.measurement).unwrap(),
    );
    fs::write(&side.measurement, &bytes).unwrap();
    side.measurement_sha256 = sha256_bytes(&bytes);
    assert_rejected_without_mutation(
        &request,
        "portable publish collector differs from verified package payload",
    );
}

#[test]
fn rejects_changed_transport_subject_final_bytes_and_malformed_inventory() {
    let prepared = prepare_release();
    let request = prepared.request();
    let asset = request.lanes.keys().next().unwrap().clone();
    let side = request.lanes[&asset].builds[0].clone();
    let original_measurement = fs::read(&side.measurement).unwrap();
    fs::write(&side.measurement, b"changed portable transport\n").unwrap();
    assert_rejected_without_mutation(
        &request,
        "portable package measurement differs from selected raw bytes",
    );
    fs::write(&side.measurement, &original_measurement).unwrap();

    let original_subject = fs::read(&prepared.subject_manifest).unwrap();
    fs::write(&prepared.subject_manifest, b"changed subject inventory\n").unwrap();
    assert_rejected_without_mutation(
        &request,
        "subject manifest does not cover the exact measured canonical subjects",
    );
    fs::write(&prepared.subject_manifest, &original_subject).unwrap();

    let mut request = prepared.request();
    request.subject_manifest_sha256 = sha256_bytes(b"wrong subject selection");
    assert_rejected_without_mutation(
        &request,
        "release build subjects differ from the retained pre-attestation bytes",
    );

    let final_path = prepared.release_dir().join(PROVENANCE_NAME);
    let original_final = fs::read(&final_path).unwrap();
    fs::write(&final_path, b"changed final provenance bytes\n").unwrap();
    assert_rejected_without_mutation(
        &prepared.request(),
        "final release checksums do not cover the exact measured canonical inventory",
    );
    fs::write(&final_path, original_final).unwrap();

    let unexpected = prepared.release_dir().join("unexpected.tmp");
    fs::write(&unexpected, b"unexpected inventory member\n").unwrap();
    assert_rejected_without_mutation(
        &prepared.request(),
        "release directory does not contain the exact indexed inventory",
    );
    fs::remove_file(unexpected).unwrap();
}

#[test]
fn rejects_wrong_release_input_collection_and_selected_subject_manifest() {
    let prepared = prepare_release();
    let mut request = prepared.request();

    let alternate = input_fixture_for_group("other-valid-group");
    request.packages.inputs = alternate.inputs;
    assert_rejected_without_mutation(
        &request,
        "release index does not bind to the supplied release inputs",
    );

    let mut request = prepared.request();
    request.subject_manifest_sha256 = sha256_bytes(b"not the subject bytes");
    assert_rejected_without_mutation(
        &request,
        "release build subjects differ from the retained pre-attestation bytes",
    );
}

#[test]
fn rejects_changes_to_each_of_the_four_final_package_members() {
    let prepared = prepare_release();
    let lane = prepared.lanes.values().next().unwrap();
    let names = fs::read_dir(&lane.builds[0].package_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 4);
    for name in names {
        let path = prepared.release_dir().join(name);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"changed final package member\n").unwrap();
        assert_rejected_without_mutation(
            &prepared.request(),
            "native compiler-family package failed read-back verification",
        );
        fs::write(&path, original).unwrap();
    }
}

fn input_fixture_for_group(group_id: &str) -> InputFixture {
    let mut fixture = input_fixture();
    let mut collection: Value = serde_json::from_slice(&fixture.collection).unwrap();
    collection["groups"][0]["id"] = json!(group_id);
    fixture.collection = serde_json::to_vec(&collection).unwrap();
    fixture.inputs = ReleaseInputs::parse(&fixture.collection, &fixture.documents).unwrap();
    fixture
}
