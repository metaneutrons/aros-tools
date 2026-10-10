//! End-to-end CLI acquisition tests over synthetic, non-authenticating evidence.

#![cfg(unix)]

#[path = "support/release_evidence_v2.rs"]
mod support;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use aros_common::sha256_bytes;
use aros_toolchain::canonical;
use aros_toolchain::release_index::PackageComparisonReport;
use serde_json::{json, Value};
use support::{Fixture, BASE_URL, RELEASE_ID};

fn command(fixture: &Fixture, selection: &Value, name: &str) -> Output {
    let (selection_path, selection_sha256) = fixture.write_selection(name, selection);
    Command::new(env!("CARGO_BIN_EXE_aros"))
        .args([
            "toolchain",
            "producer",
            "verify-release-evidence",
            "--directory",
            fixture.release_directory.to_str().unwrap(),
            "--release-id",
            RELEASE_ID,
            "--base-url",
            BASE_URL,
            "--inputs-sha256",
            fixture.inputs_sha256.as_str(),
            "--index-sha256",
            fixture.index_sha256.as_str(),
            "--selection",
            selection_path.to_str().unwrap(),
            "--selection-sha256",
            selection_sha256.as_str(),
            "--subject-manifest",
            fixture.subject_manifest.to_str().unwrap(),
            "--subject-manifest-sha256",
            fixture.subject_manifest_sha256.as_str(),
            "--format",
            "json",
        ])
        .output()
        .unwrap()
}

fn assert_failure_is_read_only(
    fixture: &Fixture,
    selection: &Value,
    name: &str,
    expected_diagnostic: &str,
) {
    let output = command(fixture, selection, name);
    assert!(!output.status.success(), "unexpected success for {name}");
    assert!(
        output.stdout.is_empty(),
        "failure for {name} emitted a success-shaped stdout document: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected_diagnostic),
        "failure for {name} did not reach its intended rejection; expected {expected_diagnostic:?} in stderr:\n{stderr}"
    );
}

fn assert_unchanged_after_failure(
    fixture: &Fixture,
    selection: &Value,
    name: &str,
    expected_diagnostic: &str,
    extra_roots: &[PathBuf],
) {
    // Selection files are themselves transport inputs, so establish the exact
    // bytes before taking the read-only baseline.
    let _ = fixture.write_selection(name, selection);
    let release_before = support::snapshot_tree(&fixture.release_directory);
    let transport_before = support::snapshot_tree(&fixture.transport_directory);
    let extras_before = extra_roots
        .iter()
        .map(|root| support::snapshot_tree(root))
        .collect::<Vec<_>>();
    assert_failure_is_read_only(fixture, selection, name, expected_diagnostic);
    assert_eq!(
        support::snapshot_tree(&fixture.release_directory),
        release_before,
        "CLI mutated the final release during failed {name} acquisition"
    );
    assert_eq!(
        support::snapshot_tree(&fixture.transport_directory),
        transport_before,
        "CLI mutated selected transport evidence during failed {name} acquisition"
    );
    for (root, before) in extra_roots.iter().zip(extras_before) {
        assert_eq!(
            support::snapshot_tree(root),
            before,
            "CLI mutated selected external evidence during failed {name} acquisition"
        );
    }
}

fn asset_for_host(fixture: &Fixture, host: &str) -> String {
    let mut matching = fixture.lanes.iter().filter(|(_, lane)| lane.host == host);
    let (asset, _) = matching
        .next()
        .unwrap_or_else(|| panic!("fixture has no independently selected {host} lane"));
    assert!(
        matching.next().is_none(),
        "fixture has more than one independently selected {host} lane"
    );
    asset.clone()
}

fn copy_compatibility_for_mutation(
    fixture: &Fixture,
    selection: &mut Value,
    asset: &str,
    label: &str,
    member: &str,
) -> PathBuf {
    let source = PathBuf::from(
        selection["lanes"][asset]["compatibility"]["directory"]
            .as_str()
            .unwrap(),
    );
    let destination = fixture
        .transport_directory
        .join(format!("compatibility-mutated-{label}"));
    support::copy_tree(&source, &destination);
    let member_path = destination.join(member);
    let mut bytes = fs::read(&member_path).unwrap();
    bytes.push(b'X');
    fs::write(member_path, bytes).unwrap();
    let manifest_sha256 = support::update_compatibility_manifest(&destination);
    let destination = fs::canonicalize(destination).unwrap();
    selection["lanes"][asset]["compatibility"]["directory"] = json!(destination);
    selection["lanes"][asset]["compatibility"]["manifest_sha256"] = json!(manifest_sha256.as_str());
    destination
}

#[test]
fn process_collects_complete_synthetic_v2_closure_and_rejects_mutations_read_only() {
    // This exercises the actual compiled CLI. All payloads below are synthetic
    // byte declarations; success does not claim compilation or probe execution.
    let fixture = Fixture::new();
    let (selection_path, selection_sha256) = fixture.selection_path();
    let release_before_success = support::snapshot_tree(&fixture.release_directory);
    let transport_before_success = support::snapshot_tree(&fixture.transport_directory);
    let success = Command::new(env!("CARGO_BIN_EXE_aros"))
        .args([
            "toolchain",
            "producer",
            "verify-release-evidence",
            "--directory",
            fixture.release_directory.to_str().unwrap(),
            "--release-id",
            RELEASE_ID,
            "--base-url",
            BASE_URL,
            "--inputs-sha256",
            fixture.inputs_sha256.as_str(),
            "--index-sha256",
            fixture.index_sha256.as_str(),
            "--selection",
            selection_path.to_str().unwrap(),
            "--selection-sha256",
            selection_sha256.as_str(),
            "--subject-manifest",
            fixture.subject_manifest.to_str().unwrap(),
            "--subject-manifest-sha256",
            fixture.subject_manifest_sha256.as_str(),
            "--format",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        success.status.success(),
        "synthetic process acquisition failed:\n{}",
        String::from_utf8_lossy(&success.stderr)
    );
    assert_eq!(
        support::snapshot_tree(&fixture.release_directory),
        release_before_success,
        "successful CLI acquisition mutated the final release"
    );
    assert_eq!(
        support::snapshot_tree(&fixture.transport_directory),
        transport_before_success,
        "successful CLI acquisition mutated selected transport evidence"
    );
    let document: Value = serde_json::from_slice(&success.stdout).unwrap();
    assert_eq!(document["operation"], "verify-release-evidence");
    assert_eq!(document["assurance"], "byte-consistency-only");
    assert_eq!(document["lane_count"], 3);
    assert_eq!(document["build_count"], 6);
    assert_eq!(document["inputs_sha256"], fixture.inputs_sha256.as_str());
    assert_eq!(document["index_sha256"], fixture.index_sha256.as_str());
    assert_eq!(document["selection_sha256"], selection_sha256.as_str());
    assert!(document["subject_manifest_sha256"].is_string());
    assert!(document["checksums_sha256"].is_string());
    let lanes = document["lanes"].as_array().unwrap();
    assert_eq!(lanes.len(), 3);
    for lane in lanes {
        let selected = &fixture.selection["lanes"][lane["asset"].as_str().unwrap()];
        assert_eq!(
            lane["measurement_sha256"][0],
            selected["builds"][0]["measurement"]["sha256"]
        );
        assert_eq!(
            lane["measurement_sha256"][1],
            selected["builds"][1]["measurement"]["sha256"]
        );
        assert_eq!(lane["comparison_sha256"], selected["comparison"]["sha256"]);
        assert_eq!(
            lane["compatibility_manifest_sha256"],
            selected["compatibility"]["manifest_sha256"]
        );
        assert_eq!(
            lane["compatibility_inputs_sha256"],
            selected["compatibility"]["inputs"]["sha256"]
        );
        assert_eq!(lane["standalone"].as_object().unwrap().len(), 2);
    }

    // Exact index coverage is input-derived; neither a selected subset nor an
    // appended synthetic lane is accepted.
    let mut subset = fixture.selection.clone();
    let subset_asset = asset_for_host(&fixture, "linux-x86_64");
    subset["lanes"]
        .as_object_mut()
        .unwrap()
        .remove(&subset_asset);
    assert_unchanged_after_failure(
        &fixture,
        &subset,
        "subset.json",
        "evidence selection must exactly cover",
        &[],
    );

    let mut extra = fixture.selection.clone();
    let source_lane = extra["lanes"][&subset_asset].clone();
    extra["lanes"]["synthetic-extra.tar.xz"] = source_lane;
    assert_unchanged_after_failure(
        &fixture,
        &extra,
        "extra.json",
        "evidence selection must exactly cover",
        &[],
    );

    // Rehashing a changed declaration still cannot make it agree with the
    // independently measured package environment or executor identity.
    let mut environment = fixture.selection.clone();
    environment["lanes"][&subset_asset]["build_environment"]["fixture-observation"] =
        json!("changed-and-rehashed");
    assert_unchanged_after_failure(
        &fixture,
        &environment,
        "environment.json",
        "native compiler-family package failed read-back verification",
        &[],
    );

    let mut executor = fixture.selection.clone();
    executor["lanes"][&subset_asset]["builds"][0]["executor"]["binary_sha256"] =
        json!(sha256_bytes(b"different independently selected executor").as_str());
    assert_unchanged_after_failure(
        &fixture,
        &executor,
        "executor.json",
        "portable package build result differs from selected executor or lane",
        &[],
    );

    let mut changed_inputs = fixture.selection.clone();
    let original_inputs = PathBuf::from(
        changed_inputs["lanes"][&subset_asset]["compatibility"]["inputs"]["path"]
            .as_str()
            .unwrap(),
    );
    let mut input_value: Value =
        serde_json::from_slice(&fs::read(&original_inputs).unwrap()).unwrap();
    input_value["engine_sha256"] = json!(sha256_bytes(b"different selected engine").as_str());
    let changed_input_bytes = canonical::bytes(&input_value).unwrap();
    let changed_input_path = fixture
        .transport_directory
        .join("compatibility-inputs-rehashed.json");
    fs::write(&changed_input_path, &changed_input_bytes).unwrap();
    let changed_input_path = fs::canonicalize(changed_input_path).unwrap();
    changed_inputs["lanes"][&subset_asset]["compatibility"]["inputs"]["path"] =
        json!(changed_input_path);
    changed_inputs["lanes"][&subset_asset]["compatibility"]["inputs"]["sha256"] =
        json!(sha256_bytes(&changed_input_bytes).as_str());
    assert_unchanged_after_failure(
        &fixture,
        &changed_inputs,
        "inputs-rehashed.json",
        "native compatibility phase mixes engine, helper, or SDK source identities",
        &[],
    );

    // A changed package copy and selected raw-digest mismatch are rejected
    // without rewriting either the final inventory or downloaded package set.
    let package_mutation = fixture.selection.clone();
    let package_dir = PathBuf::from(
        package_mutation["lanes"][&subset_asset]["builds"][0]["package_dir"]
            .as_str()
            .unwrap(),
    );
    let archive = package_dir.join(&subset_asset);
    assert!(
        archive.is_file(),
        "selected package directory lacks its index asset"
    );
    let original_archive = fs::read(&archive).unwrap();
    let mut changed_archive = original_archive.clone();
    changed_archive.push(b'X');
    fs::write(&archive, changed_archive).unwrap();
    assert_unchanged_after_failure(
        &fixture,
        &package_mutation,
        "package-bytes.json",
        "package checksum sidecar does not match the measured archive",
        std::slice::from_ref(&package_dir),
    );
    fs::write(&archive, original_archive).unwrap();

    let mut bad_digest = fixture.selection.clone();
    bad_digest["lanes"][&subset_asset]["comparison"]["sha256"] =
        json!(sha256_bytes(b"wrong selected comparison bytes").as_str());
    assert_unchanged_after_failure(
        &fixture,
        &bad_digest,
        "bad-raw-digest.json",
        "release build evidence differs from independently selected raw bytes",
        &[],
    );

    // A coherently rehashed but changed comparison and compatibility member
    // reaches the aggregate validator and fails on the measured content join.
    let mut comparison_change = fixture.selection.clone();
    let original_comparison = PathBuf::from(
        comparison_change["lanes"][&subset_asset]["comparison"]["path"]
            .as_str()
            .unwrap(),
    );
    let mut report: PackageComparisonReport =
        serde_json::from_slice(&fs::read(&original_comparison).unwrap()).unwrap();
    report.members[0].size += 1;
    report.package_set_sha256 =
        sha256_bytes(&canonical::bytes(&serde_json::to_value(&report.members).unwrap()).unwrap());
    let mut comparison_bytes = canonical::bytes(&serde_json::to_value(&report).unwrap()).unwrap();
    comparison_bytes.push(b'\n');
    let comparison_copy = fixture.transport_directory.join("comparison-rehashed.json");
    fs::write(&comparison_copy, &comparison_bytes).unwrap();
    let comparison_copy = fs::canonicalize(comparison_copy).unwrap();
    comparison_change["lanes"][&subset_asset]["comparison"]["path"] = json!(comparison_copy);
    comparison_change["lanes"][&subset_asset]["comparison"]["sha256"] =
        json!(sha256_bytes(&comparison_bytes).as_str());
    assert_unchanged_after_failure(
        &fixture,
        &comparison_change,
        "comparison-rehashed.json",
        "release build comparison differs from measured A/B package bytes",
        &[],
    );

    for (label, member) in [("log", "cmake-consumer.stdout.log"), ("elf", "c-x86_64.o")] {
        let mut changed_compatibility = fixture.selection.clone();
        let changed_root = copy_compatibility_for_mutation(
            &fixture,
            &mut changed_compatibility,
            &subset_asset,
            label,
            member,
        );
        assert_unchanged_after_failure(
            &fixture,
            &changed_compatibility,
            &format!("compatibility-{label}.json"),
            if label == "log" {
                "native compatibility retained command log bytes differ from their report hashes"
            } else {
                "native compatibility receipt standalone ELF output claim differs from verified bytes"
            },
            &[changed_root],
        );
    }
}
