//! Synthetic local consistency tests for finished-candidate A/B joins.
//!
//! These fixtures prove receipt, package and root relationships only. They do
//! not establish independent execution or authenticate either producer.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use aros_common::{measure_tree_content_cas, sha256_bytes, DiagnosticCode, TreeContentCas};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::tests::{
    fixture_profiles, fixture_recipe, fixture_source_lock, package_request, Fixture,
};
use super::{
    compare_finished_candidate_packages, package_finished_candidate, FinishedCandidatePackage,
    FinishedCandidateReadback,
};
use crate::release_checksums_v2::{verify_final_checksums_v2, FinalChecksumsReadbackV2};
use crate::release_index_v2::{expected_artifacts, NativeReleaseArtifactV2, NativeReleaseIndexV2};
use crate::release_inputs::{ReleaseInputs, ACTIVE_HOSTS};

fn package_side(
    fixture: &mut Fixture,
    package_dir: PathBuf,
) -> (FinishedCandidateReadback, FinishedCandidatePackage) {
    fixture.remove_old_compiler_checkpoint();
    fixture.persist();
    let candidate = fixture.readback().unwrap();
    let request = package_request(fixture, package_dir);
    let package = package_finished_candidate(&request, &candidate).unwrap();
    (candidate, package)
}

#[test]
fn compares_two_distinct_complete_candidates_and_packages() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));

    assert_ne!(
        left_candidate.record.candidate_root,
        right_candidate.record.candidate_root
    );
    assert_ne!(
        left_package.package_output().output_dir,
        right_package.package_output().output_dir
    );
    let before = snapshot(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    );
    let comparison = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap();

    assert_eq!(comparison.report().schema, 1);
    assert_eq!(comparison.report().operation, "compare");
    assert!(comparison.report().byte_identical);
    assert_eq!(comparison.report().members.len(), 4);
    assert_eq!(
        comparison.candidate_receipt_digests(),
        [
            left_candidate.receipt_sha256(),
            right_candidate.receipt_sha256()
        ]
    );
    comparison.revalidate().unwrap();
    assert_eq!(
        snapshot(
            &left_candidate,
            &left_package,
            &right_candidate,
            &right_package,
        ),
        before
    );
}

#[test]
fn joins_complete_candidates_to_final_v2_checksums() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));
    let comparison = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap();
    let inputs = input_fixture(
        "llvm-pc",
        fixture_recipe(&fixture_source_lock(), &fixture_profiles()),
    );
    let index = release_index(
        &inputs,
        &left_package,
        "https://example.invalid/releases/one",
    );
    let artifact = selected_artifact(&index);
    let (release_directory, readback) = final_readback(&inputs, &index, &left_package, None);
    let before = measure_tree_content_cas(release_directory.path()).unwrap();

    comparison
        .validate_against_checksums_v2(&inputs.inputs, &index, artifact, &readback)
        .unwrap();

    assert_eq!(
        measure_tree_content_cas(release_directory.path()).unwrap(),
        before
    );
}

#[test]
fn rejects_a_valid_alternate_recipe_group_with_matching_lane_identity() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));
    let comparison = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap();
    let source_lock = fixture_source_lock();
    let profiles = fixture_profiles();
    let inputs = input_fixture("llvm-pc", fixture_recipe(&source_lock, &profiles));
    let mut alternate_recipe: Value =
        serde_json::from_slice(&fixture_recipe(&source_lock, &profiles)).unwrap();
    alternate_recipe["source_date_epoch"] = json!(946_684_801_u64);
    alternate_recipe
        .as_object_mut()
        .unwrap()
        .remove("recipe_sha256")
        .unwrap();
    let recipe_sha256 = sha256_bytes(&crate::canonical::bytes(&alternate_recipe).unwrap());
    alternate_recipe
        .as_object_mut()
        .unwrap()
        .insert("recipe_sha256".to_owned(), json!(recipe_sha256.as_str()));
    let alternate_recipe = serde_json::to_vec(&alternate_recipe).unwrap();
    let alternate_inputs = input_fixture("llvm-pc-alternate", alternate_recipe);
    let index = release_index(
        &alternate_inputs,
        &left_package,
        "https://example.invalid/releases/alternate",
    );
    let artifact = selected_artifact(&index);
    let (release_directory, readback) =
        final_readback(&alternate_inputs, &index, &left_package, None);
    let before = measure_tree_content_cas(release_directory.path()).unwrap();

    assert_ne!(
        inputs.inputs.groups()[0].recipe().sha256(),
        alternate_inputs.inputs.groups()[0].recipe().sha256()
    );
    assert_eq!(artifact.host(), "linux-x86_64");
    assert_eq!(artifact.target_profile(), "pc-x86_64");
    assert_eq!(
        artifact.target_profile(),
        left_candidate.record.identity.target_profile
    );
    assert_eq!(
        artifact.source_commit(),
        &left_candidate.record.identity.source_commit
    );
    assert_eq!(
        Some(artifact.compiler()),
        left_package.verified.manifest.compiler.as_ref()
    );
    assert_eq!(
        artifact.tree_sha256().as_str(),
        left_package.verified.manifest.tree_sha256.as_str()
    );
    assert_eq!(artifact.group_id(), "llvm-pc-alternate");

    let error = comparison
        .validate_against_checksums_v2(&alternate_inputs.inputs, &index, artifact, &readback)
        .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerComparison,
        "finished A/B comparison differs from the selected indexed group or lane",
    );
    assert_eq!(
        measure_tree_content_cas(release_directory.path()).unwrap(),
        before
    );
}

#[test]
fn rejects_a_different_index_even_when_the_selected_artifact_is_unchanged() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));
    let comparison = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap();
    let source_lock = fixture_source_lock();
    let profiles = fixture_profiles();
    let inputs = input_fixture("llvm-pc", fixture_recipe(&source_lock, &profiles));
    let index = release_index(
        &inputs,
        &left_package,
        "https://example.invalid/releases/one",
    );
    let other_index = release_index(
        &inputs,
        &left_package,
        "https://example.invalid/releases/two",
    );
    let artifact = selected_artifact(&index);
    assert_eq!(artifact, selected_artifact(&other_index));
    let (_, readback) = final_readback(&inputs, &index, &left_package, None);

    let error = comparison
        .validate_against_checksums_v2(&inputs.inputs, &other_index, artifact, &readback)
        .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerComparison,
        "finished A/B final checksums do not bind the selected index bytes",
    );
}

#[test]
fn rejects_a_wrong_measured_package_checksum_member() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));
    let comparison = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap();
    let inputs = input_fixture(
        "llvm-pc",
        fixture_recipe(&fixture_source_lock(), &fixture_profiles()),
    );
    let index = release_index(
        &inputs,
        &left_package,
        "https://example.invalid/releases/one",
    );
    let artifact = selected_artifact(&index);
    let wrong_checksum_name = format!("{}.sha256", artifact.asset());
    let (release_directory, readback) = final_readback(
        &inputs,
        &index,
        &left_package,
        Some((&wrong_checksum_name, b"synthetic incorrect checksum\n")),
    );
    let before = measure_tree_content_cas(release_directory.path()).unwrap();

    let error = comparison
        .validate_against_checksums_v2(&inputs.inputs, &index, artifact, &readback)
        .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerComparison,
        "native comparison member differs from its measured release bytes",
    );
    assert_eq!(
        measure_tree_content_cas(release_directory.path()).unwrap(),
        before
    );
}

#[test]
fn rejects_a_wrong_measured_package_member_size() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));
    let comparison = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap();
    let inputs = input_fixture(
        "llvm-pc",
        fixture_recipe(&fixture_source_lock(), &fixture_profiles()),
    );
    let index = release_index(
        &inputs,
        &left_package,
        "https://example.invalid/releases/one",
    );
    let artifact = selected_artifact(&index);
    let wrong_size_name = format!("{}.manifest.json", artifact.asset());
    let mut wrong_size = fs::read(&left_package.package_output().manifest).unwrap();
    wrong_size.push(b' ');
    let (release_directory, readback) = final_readback(
        &inputs,
        &index,
        &left_package,
        Some((&wrong_size_name, &wrong_size)),
    );
    let before = measure_tree_content_cas(release_directory.path()).unwrap();

    let error = comparison
        .validate_against_checksums_v2(&inputs.inputs, &index, artifact, &readback)
        .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerComparison,
        "native comparison member differs from its measured release bytes",
    );
    assert_eq!(
        measure_tree_content_cas(release_directory.path()).unwrap(),
        before
    );
}

#[test]
fn rejects_self_rehashed_substitution_of_each_final_input_document() {
    let packages = tempfile::tempdir().unwrap();
    let mut left = Fixture::new();
    let (left_candidate, left_package) = package_side(&mut left, packages.path().join("left"));
    let mut right = Fixture::new();
    let (right_candidate, right_package) = package_side(&mut right, packages.path().join("right"));
    let comparison = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap();
    let inputs = input_fixture(
        "llvm-pc",
        fixture_recipe(&fixture_source_lock(), &fixture_profiles()),
    );
    let index = release_index(
        &inputs,
        &left_package,
        "https://example.invalid/releases/one",
    );
    for name in [
        "toolchain-release-inputs-v2.json",
        "recipe.json",
        "source-lock.json",
        "profiles.json",
    ] {
        // The helper regenerates valid checksums for these substituted bytes.
        let (directory, readback) = final_readback(
            &inputs,
            &index,
            &left_package,
            Some((name, b"substituted input\n")),
        );
        let before = measure_tree_content_cas(directory.path()).unwrap();
        let error = comparison
            .validate_against_checksums_v2(
                &inputs.inputs,
                &index,
                selected_artifact(&index),
                &readback,
            )
            .unwrap_err();
        assert_diagnostic(
            &error,
            DiagnosticCode::ProducerComparison,
            "finished A/B final files differ from release input documents",
        );
        assert_eq!(measure_tree_content_cas(directory.path()).unwrap(), before);
    }
}

struct InputFixture {
    inputs: ReleaseInputs,
    collection: Vec<u8>,
    documents: BTreeMap<String, Vec<u8>>,
}

fn input_fixture(group_id: &str, recipe: Vec<u8>) -> InputFixture {
    let source_lock = fixture_source_lock();
    let profiles = fixture_profiles();
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

fn release_index(
    inputs: &InputFixture,
    package: &FinishedCandidatePackage,
    base_url: &str,
) -> NativeReleaseIndexV2 {
    let selected_archive = fs::read(&package.package_output().archive).unwrap();
    let unused_archive = b"synthetic non-selected lane archive\n";
    let unused_tree = sha256_bytes(b"synthetic non-selected lane tree");
    let artifacts = expected_artifacts(&inputs.inputs)
        .unwrap()
        .iter()
        .map(|lane| {
            let selected = lane.host == "linux-x86_64" && lane.target_profile == "pc-x86_64";
            let (archive_sha256, archive_size, tree_sha256) = if selected {
                assert_eq!(
                    package.package_output().archive.file_name().unwrap(),
                    lane.asset.as_str()
                );
                (
                    sha256_bytes(&selected_archive),
                    selected_archive.len() as u64,
                    package.verified.manifest.tree_sha256.clone(),
                )
            } else {
                (
                    sha256_bytes(unused_archive),
                    unused_archive.len() as u64,
                    unused_tree.as_str().to_owned(),
                )
            };
            json!({
                "group_id": lane.group_id,
                "asset": lane.asset,
                "sha256": archive_sha256.as_str(),
                "size": archive_size,
                "host": lane.host,
                "target_profile": lane.target_profile,
                "target_triple": lane.target_triple,
                "source_commit": lane.source_commit,
                "compiler": lane.compiler,
                "tree_sha256": tree_sha256,
                "enabled": true,
                "strip_components": 1,
                "required_paths": ["bin/clang"],
            })
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&json!({
        "schema": 2,
        "release_id": package.verified.manifest.release_id,
        "base_url": base_url,
        "inputs_sha256": inputs.inputs.collection_sha256(),
        "producer_commit": inputs.inputs.producer_commit(),
        "tools_commit": inputs.inputs.tools_commit(),
        "artifacts": artifacts,
    }))
    .unwrap();
    NativeReleaseIndexV2::parse(&bytes, &inputs.inputs).unwrap()
}

fn selected_artifact(index: &NativeReleaseIndexV2) -> &NativeReleaseArtifactV2 {
    index
        .artifacts()
        .iter()
        .find(|artifact| {
            artifact.host() == "linux-x86_64" && artifact.target_profile() == "pc-x86_64"
        })
        .unwrap()
}

fn final_readback(
    inputs: &InputFixture,
    index: &NativeReleaseIndexV2,
    package: &FinishedCandidatePackage,
    replacement: Option<(&str, &[u8])>,
) -> (TempDir, FinalChecksumsReadbackV2) {
    const CHECKSUMS_NAME: &str = "SHA256SUMS";
    const INPUTS_NAME: &str = "toolchain-release-inputs-v2.json";
    const PROVENANCE_NAME: &str = "toolchain-provenance.sigstore.json";
    const MANIFEST_SCHEMA_NAME: &str = "toolchain-manifest-v2.schema.json";
    const TREE_FIXTURE_NAME: &str = "tree-digest-v1.fixture.json";

    let directory = tempfile::tempdir().unwrap();
    let output = package.package_output();
    let selected_asset = selected_artifact(index).asset();
    let index_bytes = index.to_json_bytes().unwrap();
    let mut checksum_lines = Vec::new();
    for name in index.expected_inventory() {
        if name == CHECKSUMS_NAME {
            continue;
        }
        let mut bytes = if name == crate::release_index_v2::INDEX_NAME {
            index_bytes.clone()
        } else if name == INPUTS_NAME {
            inputs.collection.clone()
        } else if let Some(document) = inputs.documents.get(name) {
            document.clone()
        } else if name == selected_asset {
            fs::read(&output.archive).unwrap()
        } else if *name == format!("{selected_asset}.manifest.json") {
            fs::read(&output.manifest).unwrap()
        } else if *name == format!("{selected_asset}.sha256") {
            fs::read(&output.checksum).unwrap()
        } else if *name == format!("{selected_asset}.spdx.json") {
            fs::read(&output.sbom).unwrap()
        } else if name.ends_with(".manifest.json") {
            b"synthetic non-selected manifest\n".to_vec()
        } else if name.ends_with(".sha256") {
            b"synthetic non-selected checksum\n".to_vec()
        } else if name.ends_with(".spdx.json") {
            b"synthetic non-selected SBOM\n".to_vec()
        } else if name.ends_with(".tar.xz") {
            b"synthetic non-selected lane archive\n".to_vec()
        } else if matches!(
            name.as_str(),
            PROVENANCE_NAME | MANIFEST_SCHEMA_NAME | TREE_FIXTURE_NAME
        ) {
            b"synthetic unsigned release support bytes\n".to_vec()
        } else {
            panic!("unexpected release inventory member: {name}");
        };
        if let Some((replacement_name, replacement_bytes)) = replacement {
            if name == replacement_name {
                bytes = replacement_bytes.to_vec();
            }
        }
        fs::write(directory.path().join(name), &bytes).unwrap();
        checksum_lines.push(format!("{}  {name}\n", sha256_bytes(&bytes)));
    }
    checksum_lines.sort_unstable();
    fs::write(
        directory.path().join(CHECKSUMS_NAME),
        checksum_lines.concat(),
    )
    .unwrap();
    let readback =
        verify_final_checksums_v2(&directory.path().canonicalize().unwrap(), index).unwrap();
    (directory, readback)
}

#[test]
fn rejects_reusing_one_candidate_with_two_package_directories_without_mutation() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::new();
    fixture.remove_old_compiler_checkpoint();
    fixture.persist();
    let candidate = fixture.readback().unwrap();
    let left_request = package_request(&fixture, package_roots.path().join("left"));
    let left_package = package_finished_candidate(&left_request, &candidate).unwrap();
    let right_request = package_request(&fixture, package_roots.path().join("right"));
    let right_package = package_finished_candidate(&right_request, &candidate).unwrap();
    let before = snapshot(&candidate, &left_package, &candidate, &right_package);

    let error =
        compare_finished_candidate_packages(&candidate, &left_package, &candidate, &right_package)
            .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerComparison,
        "finished A/B work, output and package roots must be disjoint",
    );
    assert_eq!(
        snapshot(&candidate, &left_package, &candidate, &right_package),
        before
    );
}

#[test]
fn rejects_candidates_with_different_executor_identity_without_mutation() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    right_fixture.identity.executor.binary_sha256 =
        sha256_bytes(b"different synthetic frontend observation");
    right_fixture.write_phase_chain();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));
    let before = snapshot(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    );

    let error = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerComparison,
        "finished A/B candidates select different lane inputs or executors",
    );
    assert_eq!(
        snapshot(
            &left_candidate,
            &left_package,
            &right_candidate,
            &right_package,
        ),
        before
    );
}

#[test]
fn rejects_a_candidate_changed_after_packaging_without_mutation() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));

    fs::write(
        left_fixture.candidate().join("bin/clang"),
        b"changed after package creation\n",
    )
    .unwrap();
    let before = snapshot(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    );
    let error = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerState,
        "finished candidate payload changed after read-back",
    );
    assert_eq!(
        snapshot(
            &left_candidate,
            &left_package,
            &right_candidate,
            &right_package,
        ),
        before
    );
}

#[test]
fn rejects_a_changed_package_member_without_mutation() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));

    let mut archive = fs::read(&left_package.package_output().archive).unwrap();
    archive[0] ^= 1;
    fs::write(&left_package.package_output().archive, archive).unwrap();
    let before = snapshot(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    );
    let error = compare_finished_candidate_packages(
        &left_candidate,
        &left_package,
        &right_candidate,
        &right_package,
    )
    .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerComparison,
        "package-set members differ between independent outputs",
    );
    assert_eq!(
        snapshot(
            &left_candidate,
            &left_package,
            &right_candidate,
            &right_package,
        ),
        before
    );
}

#[test]
fn rejects_swapped_candidate_and_package_sides_without_mutation() {
    let package_roots = tempfile::tempdir().unwrap();
    let mut left_fixture = Fixture::new();
    let (left_candidate, left_package) =
        package_side(&mut left_fixture, package_roots.path().join("left"));
    let mut right_fixture = Fixture::new();
    let (right_candidate, right_package) =
        package_side(&mut right_fixture, package_roots.path().join("right"));
    let before = snapshot(
        &left_candidate,
        &right_package,
        &right_candidate,
        &left_package,
    );

    let error = compare_finished_candidate_packages(
        &left_candidate,
        &right_package,
        &right_candidate,
        &left_package,
    )
    .unwrap_err();
    assert_diagnostic(
        &error,
        DiagnosticCode::ProducerState,
        "finished candidate does not bind the selected package inputs",
    );
    assert_eq!(
        snapshot(
            &left_candidate,
            &right_package,
            &right_candidate,
            &left_package,
        ),
        before
    );
}

fn assert_diagnostic(error: &crate::ContractError, code: DiagnosticCode, message: &str) {
    let diagnostics = &error.diagnostics().diagnostics;
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, code);
    assert!(diagnostics[0].message.contains(message), "{error}");
}

fn snapshot(
    left_candidate: &FinishedCandidateReadback,
    left_package: &FinishedCandidatePackage,
    right_candidate: &FinishedCandidateReadback,
    right_package: &FinishedCandidatePackage,
) -> Vec<TreeContentCas> {
    let left_output = left_candidate.record.candidate_root.parent().unwrap();
    let right_output = right_candidate.record.candidate_root.parent().unwrap();
    let roots = [
        left_candidate.work_dir.as_path(),
        left_output,
        left_package.package_output().output_dir.as_path(),
        right_candidate.work_dir.as_path(),
        right_output,
        right_package.package_output().output_dir.as_path(),
    ];
    roots
        .iter()
        .map(|root| measure_tree_content_cas(root).unwrap())
        .collect()
}
