//! Synthetic portable finished-package joins. No producer command is executed.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, Sha256Digest};
use serde_json::{json, Value};
use tempfile::TempDir;

use super::tests::{package_request, Fixture};
use super::{
    export_finished_package_measurement, package_finished_candidate,
    readback_finished_build_result, readback_finished_package_measurement,
    FinishedBuildResultRequest, FinishedCandidatePackage, FinishedCandidateReadback,
    FinishedPackageMeasurement, PortableFinishedPackageReadback, PortableFinishedPackageRequest,
    PHASES,
};
use crate::package::PackageRequest;
use crate::package_verify::PackageVerificationRequest;
use crate::plan::Identity;
use crate::recipe::GitObjectId;

const BUILD_RESULT_FILE: &str = "build-result.json";

pub(super) struct GuardedPackage {
    pub(super) fixture: Fixture,
    pub(super) candidate: FinishedCandidateReadback,
    pub(super) package_request: PackageRequest,
    pub(super) package: FinishedCandidatePackage,
    pub(super) verification: PackageVerificationRequest,
    pub(super) measurement: FinishedPackageMeasurement,
    pub(super) identity: Identity,
    _package_root: TempDir,
    _result_root: TempDir,
}

fn guarded_package() -> GuardedPackage {
    guarded_package_for_host("linux-x86_64")
}

pub(super) fn guarded_package_for_host(host: &'static str) -> GuardedPackage {
    let mut fixture = Fixture::new_for_host(host);
    fixture.remove_old_compiler_checkpoint();
    fixture.persist();

    let local = fixture.readback().unwrap();
    let package_root = tempfile::tempdir().unwrap();
    let selected_package = package_request(&fixture, package_root.path().join("package"));
    let result_root = tempfile::tempdir().unwrap();
    let result_path = result_root.path().join(BUILD_RESULT_FILE);
    let result_bytes = synthetic_build_result(&local, &fixture.identity);
    fs::write(&result_path, &result_bytes).unwrap();
    let result_sha256 = sha256_bytes(&result_bytes);
    let candidate_root = local.record.candidate_root.parent().unwrap().to_path_buf();
    let candidate = readback_finished_build_result(&FinishedBuildResultRequest {
        work_dir: &local.work_dir,
        output_dir: &candidate_root,
        recipe: &selected_package.recipe,
        source_lock: &selected_package.source_lock,
        profile: &selected_package.profile,
        host: &selected_package.host,
        build_result: &result_path,
        build_result_sha256: &result_sha256,
    })
    .unwrap();
    drop(local);
    let package = package_finished_candidate(&selected_package, &candidate).unwrap();
    let verification = verification_from_package_request(
        &selected_package,
        package.package_output().output_dir.clone(),
    );
    let measurement = export_finished_package_measurement(&candidate, &package).unwrap();
    let identity = fixture.identity.clone();
    GuardedPackage {
        fixture,
        candidate,
        package_request: selected_package,
        package,
        verification,
        measurement,
        identity,
        _package_root: package_root,
        _result_root: result_root,
    }
}

fn verification_from_package_request(
    request: &PackageRequest,
    package_dir: PathBuf,
) -> PackageVerificationRequest {
    PackageVerificationRequest {
        package_dir,
        release_id: request.release_id.clone(),
        host: request.host.clone(),
        recipe: request.recipe.clone(),
        source_lock: request.source_lock.clone(),
        profile: request.profile.clone(),
        build_environment: request.build_environment.clone(),
        forbidden_prefixes: request.forbidden_prefixes.clone(),
    }
}

fn synthetic_build_result(candidate: &FinishedCandidateReadback, identity: &Identity) -> Vec<u8> {
    let mut evidence = PHASES
        .iter()
        .enumerate()
        .map(|(index, phase)| {
            json!({
                "check": phase,
                "status": "passed",
                "report_sha256": candidate.record.phase_receipt_digests[index]
            })
        })
        .collect::<Vec<_>>();
    evidence.push(json!({
        "check": "finished-candidate",
        "status": "passed",
        "report_sha256": candidate.record.receipt_sha256
    }));
    evidence.push(json!({
            "check": "origin",
            "status": "not-run",
            "report_sha256": null
    }));
    let publish: Value = serde_json::from_slice(&candidate.receipts[5].1).unwrap();
    let mut bytes = serde_json::to_vec(&json!({
        "schema": "aros-toolchain-result-v1",
        "operation": "build",
        "identity": identity,
        "output_root": candidate.record.candidate_root.parent().unwrap(),
        "outputs": publish["outputs"],
        "evidence": evidence,
        "qualification": "local-only",
        "commit_state": "committed"
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

fn readback_bytes(
    bytes: &[u8],
    selected_sha256: &Sha256Digest,
    identity: &Identity,
    package: &PackageVerificationRequest,
) -> Result<PortableFinishedPackageReadback, crate::ContractError> {
    let directory = tempfile::tempdir().unwrap();
    let measurement = directory.path().join("measurement.json");
    fs::write(&measurement, bytes).unwrap();
    readback_finished_package_measurement(&PortableFinishedPackageRequest {
        measurement: &measurement,
        measurement_sha256: selected_sha256,
        identity,
        package,
    })
}

fn readback_rehashed(
    bytes: &[u8],
    identity: &Identity,
    package: &PackageVerificationRequest,
) -> Result<PortableFinishedPackageReadback, crate::ContractError> {
    readback_bytes(bytes, &sha256_bytes(bytes), identity, package)
}

fn mutate_transport(source: &[u8], mutation: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(source).unwrap();
    mutation(&mut value);
    serde_json::to_vec(&value).unwrap()
}

fn mutate_retained_document(
    source: &[u8],
    index: usize,
    mutation: impl FnOnce(&mut Value),
    rehash_self_digest: bool,
) -> Vec<u8> {
    mutate_transport(source, |measurement| {
        let mut document: Value =
            serde_json::from_str(measurement["documents"][index]["content"].as_str().unwrap())
                .unwrap();
        mutation(&mut document);
        if rehash_self_digest {
            replace_self_digest(&mut document);
        }
        measurement["documents"][index]["content"] =
            json!(String::from_utf8(crate::canonical::bytes(&document).unwrap()).unwrap());
    })
}

pub(super) fn mutate_collector_claim_consistently(source: &[u8]) -> Vec<u8> {
    mutate_transport(source, |measurement| {
        let mut publish: Value =
            serde_json::from_str(measurement["documents"][5]["content"].as_str().unwrap()).unwrap();
        publish["outputs"][0]["sha256"] = json!(sha256_bytes(b"forged collector payload"));
        let publish_digest = replace_self_digest(&mut publish);
        measurement["documents"][5]["content"] =
            json!(String::from_utf8(crate::canonical::bytes(&publish).unwrap()).unwrap());

        let mut finished: Value =
            serde_json::from_str(measurement["documents"][6]["content"].as_str().unwrap()).unwrap();
        finished["phase_receipt_digests"][5] = json!(publish_digest.as_str());
        let finished_digest = replace_self_digest(&mut finished);
        measurement["documents"][6]["content"] =
            json!(String::from_utf8(crate::canonical::bytes(&finished).unwrap()).unwrap());

        let mut result: Value =
            serde_json::from_str(measurement["documents"][7]["content"].as_str().unwrap()).unwrap();
        result["outputs"] = publish["outputs"].clone();
        result["evidence"][5]["report_sha256"] = json!(publish_digest.as_str());
        result["evidence"][6]["report_sha256"] = json!(finished_digest.as_str());
        measurement["documents"][7]["content"] =
            json!(String::from_utf8(serde_json::to_vec(&result).unwrap()).unwrap());
    })
}

fn mutate_package_members_report(source: &[u8]) -> Vec<u8> {
    mutate_transport(source, |measurement| {
        let changed_set_sha256 = {
            let members = measurement["package_members"]["members"]
                .as_array_mut()
                .unwrap();
            members[0]["sha256"] = json!(sha256_bytes(b"changed measured member"));
            sha256_bytes(&crate::canonical::bytes(&Value::Array(members.clone())).unwrap())
        };
        measurement["package_members"]["package_set_sha256"] = json!(changed_set_sha256.as_str());
    })
}

fn replace_self_digest(value: &mut Value) -> Sha256Digest {
    value
        .as_object_mut()
        .unwrap()
        .remove("receipt_sha256")
        .expect("self-digest field");
    let digest = sha256_bytes(&crate::canonical::bytes(value).unwrap());
    value["receipt_sha256"] = json!(digest.as_str());
    digest
}

fn assert_message(error: &crate::ContractError, expected: &str) {
    let diagnostics = &error.diagnostics().diagnostics;
    assert_eq!(diagnostics.len(), 1, "{error}");
    assert_eq!(diagnostics[0].message, expected, "{error}");
}

fn snapshot_package(path: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

pub(super) fn copy_package(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for (name, bytes) in snapshot_package(source) {
        fs::write(destination.join(name), bytes).unwrap();
    }
}

#[test]
fn exports_and_reads_back_a_copied_package_without_opening_original_roots() {
    let guarded = guarded_package();
    let collector_root = tempfile::tempdir().unwrap();
    let copied_package = collector_root.path().join("downloaded-package");
    copy_package(
        guarded.package.package_output().output_dir.as_path(),
        &copied_package,
    );
    let copied_measurement = collector_root.path().join("measurement.json");
    fs::write(&copied_measurement, guarded.measurement.bytes()).unwrap();

    let measurement_sha256 = guarded.measurement.sha256().clone();
    let build_result_sha256 = sha256_bytes(&guarded.candidate.receipts[7].1);
    let finished_receipt_sha256 = guarded.candidate.receipt_sha256().clone();
    let raw_payload_sha256 = guarded.candidate.payload_sha256().clone();
    let identity = guarded.identity.clone();
    let mut selected =
        verification_from_package_request(&guarded.package_request, copied_package.clone());
    let work_root = guarded.candidate.work_dir.clone();
    let output_root = guarded
        .candidate
        .record
        .candidate_root
        .parent()
        .unwrap()
        .to_path_buf();
    let original_package = guarded.package.package_output().output_dir.clone();
    drop(guarded);

    assert!(!work_root.exists());
    assert!(!output_root.exists());
    assert!(!original_package.exists());
    selected.package_dir = copied_package.clone();
    let before = snapshot_package(&copied_package);
    let measurement_before = fs::read(&copied_measurement).unwrap();
    let readback = readback_finished_package_measurement(&PortableFinishedPackageRequest {
        measurement: &copied_measurement,
        measurement_sha256: &measurement_sha256,
        identity: &identity,
        package: &selected,
    })
    .unwrap();

    assert_eq!(readback.measurement_sha256(), &measurement_sha256);
    assert_eq!(readback.build_result_sha256(), &build_result_sha256);
    assert_eq!(readback.finished_receipt_sha256(), &finished_receipt_sha256);
    assert_eq!(readback.raw_payload_sha256(), &raw_payload_sha256);
    assert_eq!(readback.package_members().members.len(), 4);
    for member in &readback.package_members().members {
        let bytes = fs::read(copied_package.join(&member.name)).unwrap();
        assert_eq!(member.sha256, sha256_bytes(&bytes));
        assert_eq!(member.size, u64::try_from(bytes.len()).unwrap());
    }
    assert_eq!(snapshot_package(&copied_package), before);
    assert_eq!(fs::read(copied_measurement).unwrap(), measurement_before);
}

#[test]
fn rehashed_outer_records_do_not_replace_the_selected_inner_chain() {
    let guarded = guarded_package();
    let valid = guarded.measurement.bytes();
    let identity = &guarded.identity;
    let package = &guarded.verification;
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "missing document",
            mutate_transport(valid, |value| {
                value["documents"].as_array_mut().unwrap().pop();
            }),
            "portable package measurement lacks its complete document set",
        ),
        (
            "reordered documents",
            mutate_transport(valid, |value| {
                value["documents"].as_array_mut().unwrap().swap(0, 1);
            }),
            "portable package documents are missing, reordered or oversized",
        ),
        (
            "duplicate document",
            mutate_transport(valid, |value| {
                let first = value["documents"][0].clone();
                value["documents"][1] = first;
            }),
            "portable package documents are missing, reordered or oversized",
        ),
        (
            "broken predecessor",
            mutate_retained_document(
                valid,
                2,
                |document| {
                    document["previous_receipt_sha256"] =
                        json!(sha256_bytes(b"wrong predecessor").as_str());
                },
                true,
            ),
            "native phase receipt differs from independently selected identity, phase, root or predecessor",
        ),
        (
            "bad phase self-digest",
            mutate_retained_document(
                valid,
                3,
                |document| document["receipt_sha256"] = json!(sha256_bytes(b"forged")),
                false,
            ),
            "retained candidate evidence self-digest differs",
        ),
        (
            "changed source identity",
            mutate_retained_document(
                valid,
                1,
                |document| {
                    document["identity"]["source_commit"] =
                        json!("7777777777777777777777777777777777777777");
                },
                true,
            ),
            "native phase receipt differs from independently selected identity, phase, root or predecessor",
        ),
        (
            "altered finished tree count",
            mutate_retained_document(
                valid,
                6,
                |document| {
                    document["entry_count"] =
                        json!(document["entry_count"].as_u64().unwrap() + 1);
                },
                true,
            ),
            "portable finished record differs from selected inputs or complete chain",
        ),
        (
            "collector-only finished tree claim",
            mutate_retained_document(
                valid,
                6,
                |document| {
                    document["payload_sha256"] = json!(sha256_bytes(b"synthetic published collector\n"));
                    document["entry_count"] = json!(1);
                    document["regular_file_bytes"] = json!(
                        b"synthetic published collector\n".len()
                    );
                },
                true,
            ),
            "portable finished record differs from selected inputs or complete chain",
        ),
        (
            "altered publish outputs",
            mutate_retained_document(
                valid,
                7,
                |document| document["outputs"] = json!([]),
                false,
            ),
            "portable build result outputs differ from publish receipt",
        ),
        (
            "rehashed collector claim diverges from package",
            mutate_collector_claim_consistently(valid),
            "portable publish collector differs from verified package payload",
        ),
        (
            "changed package-members report",
            mutate_package_members_report(valid),
            "portable measurement differs from downloaded package members",
        ),
        (
            "altered result executor",
            mutate_retained_document(
                valid,
                7,
                |document| {
                    document["identity"]["executor"]["binary_sha256"] =
                        json!(sha256_bytes(b"another executor"));
                },
                false,
            ),
            "portable package build result differs from selected executor or lane",
        ),
    ];

    let package_before = snapshot_package(&guarded.verification.package_dir);
    for (label, bytes, message) in cases {
        let error = readback_rehashed(&bytes, identity, package).unwrap_err();
        assert_message(&error, message);
        assert_eq!(
            snapshot_package(&guarded.verification.package_dir),
            package_before,
            "rejected {label} record changed downloaded package bytes"
        );
    }
}

#[test]
fn rejects_selected_digest_schema_and_transport_boundary_failures() {
    let guarded = guarded_package();
    let bytes = guarded.measurement.bytes();
    let package_before = snapshot_package(&guarded.verification.package_dir);

    let wrong_digest = sha256_bytes(b"different externally selected bytes");
    let error = readback_bytes(
        bytes,
        &wrong_digest,
        &guarded.identity,
        &guarded.verification,
    )
    .unwrap_err();
    assert_message(
        &error,
        "portable package measurement differs from selected raw bytes",
    );

    let unknown = mutate_transport(bytes, |value| value["unknown"] = json!(true));
    let error = readback_rehashed(&unknown, &guarded.identity, &guarded.verification).unwrap_err();
    assert_message(
        &error,
        "portable package measurement violates its closed schema",
    );

    let duplicate = String::from_utf8(bytes.to_vec())
        .unwrap()
        .replacen(
            "\"schema\":\"aros-toolchain-finished-package-measurement-v2\"",
            "\"schema\":\"aros-toolchain-finished-package-measurement-v2\",\"schema\":\"aros-toolchain-finished-package-measurement-v2\"",
            1,
        )
        .into_bytes();
    assert_ne!(duplicate, bytes);
    let error =
        readback_rehashed(&duplicate, &guarded.identity, &guarded.verification).unwrap_err();
    assert_message(
        &error,
        "portable package measurement violates its closed schema",
    );

    let symlink_root = tempfile::tempdir().unwrap();
    let measurement_target = symlink_root.path().join("measurement.json");
    fs::write(&measurement_target, bytes).unwrap();
    let linked_measurement = symlink_root.path().join("measurement-link.json");
    symlink(&measurement_target, &linked_measurement).unwrap();
    let error = readback_finished_package_measurement(&PortableFinishedPackageRequest {
        measurement: &linked_measurement,
        measurement_sha256: guarded.measurement.sha256(),
        identity: &guarded.identity,
        package: &guarded.verification,
    })
    .unwrap_err();
    assert_message(
        &error,
        "portable package measurement is unsafe or exceeds its byte bound",
    );

    let oversized = symlink_root.path().join("oversized.json");
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&oversized)
        .unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    let error = readback_finished_package_measurement(&PortableFinishedPackageRequest {
        measurement: &oversized,
        measurement_sha256: guarded.measurement.sha256(),
        identity: &guarded.identity,
        package: &guarded.verification,
    })
    .unwrap_err();
    assert_message(
        &error,
        "portable package measurement is unsafe or exceeds its byte bound",
    );
    assert_eq!(
        snapshot_package(&guarded.verification.package_dir),
        package_before
    );
}

#[test]
fn rejects_wrong_independent_executor_source_release_environment_and_package_bytes() {
    let guarded = guarded_package();
    let bytes = guarded.measurement.bytes();

    let mut wrong_executor = guarded.identity.clone();
    wrong_executor.executor.binary_sha256 = sha256_bytes(b"wrong executor");
    let error = readback_rehashed(bytes, &wrong_executor, &guarded.verification).unwrap_err();
    assert_message(
        &error,
        "portable package build result differs from selected executor or lane",
    );

    let mut wrong_source = guarded.identity.clone();
    wrong_source.source_commit =
        GitObjectId::try_from("7777777777777777777777777777777777777777".to_owned()).unwrap();
    let error = readback_rehashed(bytes, &wrong_source, &guarded.verification).unwrap_err();
    assert_message(
        &error,
        "portable package build result differs from selected executor or lane",
    );

    let mut wrong_release = guarded.verification.clone();
    wrong_release.release_id = "different-release".into();
    let error = readback_rehashed(bytes, &guarded.identity, &wrong_release).unwrap_err();
    assert_message(
        &error,
        "package manifest identity is not bound to the selected producer inputs",
    );

    let mut wrong_environment = guarded.verification.clone();
    wrong_environment
        .build_environment
        .insert("compiler".into(), json!("independent-mismatch"));
    let error = readback_rehashed(bytes, &guarded.identity, &wrong_environment).unwrap_err();
    assert_message(
        &error,
        "package manifest identity is not bound to the selected producer inputs",
    );

    let package_copy_root = tempfile::tempdir().unwrap();
    let package_copy = package_copy_root.path().join("package");
    copy_package(&guarded.verification.package_dir, &package_copy);
    let mut wrong_bytes =
        verification_from_package_request(&guarded.package_request, package_copy.clone());
    let archive = fs::read_dir(&package_copy)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|extension| extension == "xz"))
        .unwrap();
    fs::write(&archive, b"changed downloaded archive bytes\n").unwrap();
    wrong_bytes.package_dir = package_copy;
    let error = readback_rehashed(bytes, &guarded.identity, &wrong_bytes).unwrap_err();
    assert_message(
        &error,
        "package checksum sidecar does not match the measured archive",
    );
}

#[test]
fn export_requires_the_cli_result_and_rechecks_live_candidate_receipt_and_package() {
    {
        let mut fixture = Fixture::new();
        fixture.remove_old_compiler_checkpoint();
        fixture.persist();
        let candidate = fixture.readback().unwrap();
        let package_root = tempfile::tempdir().unwrap();
        let request = package_request(&fixture, package_root.path().join("package"));
        let package = package_finished_candidate(&request, &candidate).unwrap();
        let before = snapshot_package(package.package_output().output_dir.as_path());
        let error = export_finished_package_measurement(&candidate, &package).unwrap_err();
        assert_message(
            &error,
            "portable package export requires the retained CLI build result",
        );
        assert_eq!(
            snapshot_package(package.package_output().output_dir.as_path()),
            before
        );
    }

    for mutation in ["candidate", "receipt", "package"] {
        let guarded = guarded_package();
        let error = match mutation {
            "candidate" => {
                fs::write(
                    guarded.fixture.candidate().join("bin/clang"),
                    b"changed compiler bytes after proof\n",
                )
                .unwrap();
                export_finished_package_measurement(&guarded.candidate, &guarded.package)
                    .unwrap_err()
            }
            "receipt" => {
                fs::write(
                    guarded.fixture.receipts_dir().join("configure.json"),
                    b"changed receipt bytes\n",
                )
                .unwrap();
                export_finished_package_measurement(&guarded.candidate, &guarded.package)
                    .unwrap_err()
            }
            "package" => {
                fs::write(
                    guarded.package.package_output().archive.as_path(),
                    b"changed package archive bytes\n",
                )
                .unwrap();
                export_finished_package_measurement(&guarded.candidate, &guarded.package)
                    .unwrap_err()
            }
            _ => unreachable!(),
        };
        let expected = match mutation {
            "candidate" => "finished candidate payload changed after read-back",
            "receipt" => "finished candidate receipt bytes changed after read-back",
            "package" => "package checksum sidecar does not match the measured archive",
            _ => unreachable!(),
        };
        assert_message(&error, expected);
    }
}
