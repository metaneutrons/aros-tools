#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding};
use aros_toolchain::sdk_package::{
    self, SdkPackageRequest, SdkVerificationRequest, VerifiedSdkPackage,
};

fn identity() -> SdkIdentity {
    SdkIdentity {
        sdk_id: "core".into(),
        version: "1.2.3".into(),
        target: SdkTarget {
            target_triple: "i686-aros".into(),
            abi: BTreeMap::from([("data_model".into(), SdkAbiValue::Text("ilp32".into()))]),
        },
        source: SdkSourceIdentity {
            commit: "1".repeat(40),
            git_tree: "2".repeat(40),
            content_sha256: sha256_bytes(b"selected SDK source"),
            contract_sha256: sha256_bytes(b"selected SDK source contract"),
        },
        source_date_epoch: 946_684_800,
    }
}

fn candidate(root: &Path) -> PathBuf {
    candidate_named(root, "candidate")
}

fn candidate_named(root: &Path, name: &str) -> PathBuf {
    let candidate = root.join(name);
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(candidate.join("bin")).unwrap();
    fs::create_dir(candidate.join("include")).unwrap();
    fs::write(candidate.join("bin/aros"), b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(
        candidate.join("bin/aros"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::write(candidate.join("include/aros.h"), b"#define AROS_SDK 1\n").unwrap();
    fs::write(
        candidate.join("toolchain-manifest.json"),
        b"source metadata\n",
    )
    .unwrap();
    symlink("bin/aros", candidate.join("aros")).expect("create internal SDK symbolic link fixture");
    candidate
}

fn tree_digest(candidate: &Path) -> aros_common::Sha256Digest {
    let (digest, _) = toolchain_tree_inventory_excluding(candidate, &["sdk-manifest.json"])
        .expect("fixture SDK inventory");
    aros_common::Sha256Digest::parse(&digest).unwrap()
}

fn package_request(candidate: &Path, output: &Path) -> SdkPackageRequest {
    SdkPackageRequest {
        candidate_root: candidate.to_path_buf(),
        output_dir: output.to_path_buf(),
        identity: identity(),
        expected_tree_sha256: tree_digest(candidate),
        forbidden_prefixes: vec![PathBuf::from("/tmp/aros-sdk-private-build-root")],
    }
}

fn verify_request(
    package_dir: &Path,
    identity: SdkIdentity,
    tree_sha256: aros_common::Sha256Digest,
    archive_sha256: aros_common::Sha256Digest,
    archive_size: u64,
) -> SdkVerificationRequest {
    SdkVerificationRequest {
        package_dir: package_dir.to_path_buf(),
        identity,
        expected_tree_sha256: tree_sha256,
        expected_archive_sha256: archive_sha256,
        expected_archive_size: archive_size,
        forbidden_prefixes: vec![PathBuf::from("/tmp/aros-sdk-private-build-root")],
    }
}

fn package_once(root: &Path) -> (PathBuf, VerifiedSdkPackage) {
    let candidate = candidate(root);
    let output = root.join("package");
    let request = package_request(&candidate, &output);
    let packaged = sdk_package::package(&request).expect("package valid SDK");
    let verified = sdk_package::verify(&verify_request(
        &output,
        request.identity,
        request.expected_tree_sha256,
        packaged.archive_sha256,
        packaged.archive_size,
    ))
    .expect("verify valid SDK package");
    (output, verified)
}

#[test]
fn deterministic_package_verifies_and_extracts_a_fresh_payload() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = candidate(temporary.path());
    let request = package_request(&candidate, &temporary.path().join("first"));
    let first = sdk_package::package(&request).unwrap();
    let second_request = package_request(&candidate, &temporary.path().join("second"));
    let second = sdk_package::package(&second_request).unwrap();

    assert_eq!(first.archive_sha256, second.archive_sha256);
    assert_eq!(first.archive_size, second.archive_size);
    assert_eq!(
        fs::read(first.manifest).unwrap(),
        fs::read(second.manifest).unwrap()
    );
    assert_eq!(
        fs::read(first.checksum).unwrap(),
        fs::read(second.checksum).unwrap()
    );

    let verification = verify_request(
        &first.output_dir,
        identity(),
        request.expected_tree_sha256,
        first.archive_sha256.clone(),
        first.archive_size,
    );
    let verified = sdk_package::verify(&verification).unwrap();
    assert_eq!(verified.manifest.identity, identity());
    let extracted_root = temporary.path().join("extracted-sdk");
    let extracted = sdk_package::verify_and_extract(&verification, &extracted_root).unwrap();
    assert_eq!(extracted, verified);
    assert_eq!(
        fs::read(extracted_root.join("toolchain-manifest.json")).unwrap(),
        b"source metadata\n"
    );
    assert_eq!(
        fs::read(extracted_root.join("bin/aros")).unwrap(),
        b"#!/bin/sh\nexit 0\n"
    );
    assert_eq!(
        fs::read_link(extracted_root.join("aros")).unwrap(),
        PathBuf::from("bin/aros")
    );
    assert!(extracted_root.join("sdk-manifest.json").is_file());
}

#[test]
fn verification_requires_selected_archive_hash_size_and_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let (package_dir, verified) = package_once(temporary.path());
    let tree = verified.manifest.tree_sha256.clone();
    let good = verify_request(
        &package_dir,
        identity(),
        tree,
        verified.archive_sha256.clone(),
        verified.archive_size,
    );

    let mut wrong_hash = good.clone();
    wrong_hash.expected_archive_sha256 = sha256_bytes(b"wrong archive");
    assert!(sdk_package::verify(&wrong_hash).is_err());

    let mut wrong_size = good.clone();
    wrong_size.expected_archive_size += 1;
    assert!(sdk_package::verify(&wrong_size).is_err());

    let mut wrong_identity = good;
    wrong_identity.identity.sdk_id = "aros-sdk-other".into();
    assert!(sdk_package::verify(&wrong_identity).is_err());
}

#[test]
fn verification_rejects_corrupt_archive_and_manifest_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    let (package_dir, verified) = package_once(temporary.path());
    let archive = package_dir.join("aros-sdk-core-1.2.3.tar.xz");
    let mut archive_bytes = fs::read(&archive).unwrap();
    let middle = archive_bytes.len() / 2;
    archive_bytes[middle] ^= 0x01;
    fs::write(&archive, archive_bytes).unwrap();
    let request = verify_request(
        &package_dir,
        identity(),
        verified.manifest.tree_sha256.clone(),
        verified.archive_sha256,
        verified.archive_size,
    );
    assert!(sdk_package::verify(&request).is_err());

    let temporary = tempfile::tempdir().unwrap();
    let (package_dir, verified) = package_once(temporary.path());
    fs::write(
        package_dir.join("aros-sdk-core-1.2.3.tar.xz.sha256"),
        b"0000000000000000000000000000000000000000000000000000000000000000  aros-sdk-core-1.2.3.tar.xz\n",
    )
    .unwrap();
    let request = verify_request(
        &package_dir,
        identity(),
        verified.manifest.tree_sha256,
        verified.archive_sha256,
        verified.archive_size,
    );
    assert!(sdk_package::verify(&request).is_err());

    let temporary = tempfile::tempdir().unwrap();
    let (package_dir, verified) = package_once(temporary.path());
    fs::write(
        package_dir.join("aros-sdk-core-1.2.3.tar.xz.manifest.json"),
        b"{}\n",
    )
    .unwrap();
    let request = verify_request(
        &package_dir,
        identity(),
        verified.manifest.tree_sha256,
        verified.archive_sha256,
        verified.archive_size,
    );
    assert!(sdk_package::verify(&request).is_err());
}

#[test]
fn package_rejects_wrong_tree_reserved_metadata_hardlinks_and_unsafe_links() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = candidate(temporary.path());
    let mut request = package_request(&candidate, &temporary.path().join("wrong-tree"));
    request.expected_tree_sha256 = sha256_bytes(b"wrong tree");
    assert!(sdk_package::package(&request).is_err());

    let reserved = temporary.path().join("reserved-candidate");
    fs::create_dir(&reserved).unwrap();
    fs::write(reserved.join("sdk-manifest.json"), b"reserved\n").unwrap();
    fs::write(reserved.join("payload"), b"SDK payload\n").unwrap();
    let request = package_request(&reserved, &temporary.path().join("reserved-output"));
    assert!(sdk_package::package(&request).is_err());

    let hardlinked = temporary.path().join("hardlinked-candidate");
    fs::create_dir(&hardlinked).unwrap();
    fs::write(hardlinked.join("payload"), b"SDK payload\n").unwrap();
    fs::hard_link(
        hardlinked.join("payload"),
        temporary.path().join("external-hardlink"),
    )
    .unwrap();
    let request = package_request(&hardlinked, &temporary.path().join("hardlink-output"));
    assert!(sdk_package::package(&request).is_err());

    let unsafe_link = temporary.path().join("unsafe-link-candidate");
    fs::create_dir(&unsafe_link).unwrap();
    fs::write(unsafe_link.join("payload"), b"SDK payload\n").unwrap();
    symlink("/etc/passwd", unsafe_link.join("external"))
        .expect("create Unix symbolic link fixture");
    let request = package_request(&unsafe_link, &temporary.path().join("unsafe-link-output"));
    assert!(sdk_package::package(&request).is_err());
}

#[test]
fn package_rejects_prefix_leaks_existing_outputs_and_missing_parents() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = candidate(temporary.path());
    fs::write(
        candidate.join("include/aros.h"),
        b"path=/tmp/aros-sdk-private-build-root\n",
    )
    .unwrap();
    let request = package_request(&candidate, &temporary.path().join("prefix-output"));
    let error = sdk_package::package(&request).unwrap_err();
    let message = &error.diagnostics().diagnostics[0].message;
    assert!(message.contains("include/aros.h"), "{message}");
    assert!(message.contains("forbidden build prefix"), "{message}");
    assert!(!request.output_dir.exists());

    let clean = candidate_named(temporary.path(), "clean-candidate");
    let existing = temporary.path().join("existing-output");
    fs::create_dir(&existing).unwrap();
    let request = package_request(&clean, &existing);
    assert!(sdk_package::package(&request).is_err());

    let missing_parent = temporary.path().join("missing-parent/output");
    let request = package_request(&clean, &missing_parent);
    assert!(sdk_package::package(&request).is_err());
    assert!(!temporary.path().join("missing-parent").exists());
}

#[test]
fn verification_rejects_extra_package_assets() {
    let temporary = tempfile::tempdir().unwrap();
    let (package_dir, verified) = package_once(temporary.path());
    fs::write(package_dir.join("unexpected.txt"), b"extra\n").unwrap();
    let request = verify_request(
        &package_dir,
        identity(),
        verified.manifest.tree_sha256,
        verified.archive_sha256,
        verified.archive_size,
    );
    assert!(sdk_package::verify(&request).is_err());
}

#[test]
fn sdk_package_roundtrips_long_unicode_paths_and_link_targets() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let candidate = candidate(&root);
    let directory = format!("Größe-{}", "long-name-".repeat(12));
    fs::create_dir(candidate.join("include").join(&directory)).unwrap();
    let target = format!("include/{directory}/header-😀.h");
    fs::write(candidate.join(&target), b"#define LONG_UNICODE_PATH 1\n").unwrap();
    symlink(&target, candidate.join("unicode-link")).unwrap();
    let request = package_request(&candidate, &root.join("package"));
    let packaged = sdk_package::package(&request).unwrap();
    let verified = verify_request(
        &packaged.output_dir,
        request.identity,
        request.expected_tree_sha256,
        packaged.archive_sha256,
        packaged.archive_size,
    );
    let extracted = root.join("extracted");
    sdk_package::verify_and_extract(&verified, &extracted).unwrap();
    assert_eq!(
        fs::read_link(extracted.join("unicode-link")).unwrap(),
        PathBuf::from(&target)
    );
    assert_eq!(
        fs::read(extracted.join("unicode-link")).unwrap(),
        b"#define LONG_UNICODE_PATH 1\n"
    );
}
