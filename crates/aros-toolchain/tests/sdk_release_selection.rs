#![cfg(unix)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::sdk_release_lock::SdkReleaseLock;
use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding, Sha256Digest};
use aros_toolchain::sdk_package::{
    self, SdkLockedArchiveRequest, SdkPackageRequest, SdkVerificationRequest,
};
use aros_toolchain::sdk_release_selection::{
    build_measured_sdk_release_lock, MeasuredSdkReleaseLockRequest,
};

struct Fixture {
    temporary: tempfile::TempDir,
    package_dir: PathBuf,
    request: MeasuredSdkReleaseLockRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStat {
    device: u64,
    inode: u64,
    mode: u32,
    links: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SnapshotKind {
    Regular(Vec<u8>),
    Symlink(PathBuf),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotEntry {
    name: OsString,
    stat: FileStat,
    kind: SnapshotKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageSnapshot {
    root: FileStat,
    entries: Vec<SnapshotEntry>,
}

fn identity() -> SdkIdentity {
    SdkIdentity {
        sdk_id: "core".into(),
        version: "0.0.0-local".into(),
        target: SdkTarget {
            target_triple: "i686-aros".into(),
            abi: BTreeMap::from([("data_model".into(), SdkAbiValue::Text("ilp32".into()))]),
        },
        source: SdkSourceIdentity {
            commit: "1".repeat(40),
            git_tree: "2".repeat(40),
            content_sha256: sha256_bytes(b"selected synthetic SDK source"),
            contract_sha256: sha256_bytes(b"selected synthetic SDK contract"),
        },
        source_date_epoch: 946_684_800,
    }
}

fn make_fixture() -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = temporary.path().join("candidate");
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
    symlink("bin/aros", candidate.join("aros")).unwrap();

    let (tree, _) = toolchain_tree_inventory_excluding(&candidate, &["sdk-manifest.json"])
        .expect("measure synthetic SDK candidate");
    let identity = identity();
    let package_dir = temporary.path().join("package");
    let package_output = sdk_package::package(&SdkPackageRequest {
        candidate_root: candidate,
        output_dir: package_dir.clone(),
        identity: identity.clone(),
        expected_tree_sha256: Sha256Digest::parse(&tree).unwrap(),
        forbidden_prefixes: vec![PathBuf::from("/tmp/aros-sdk-private-build-root")],
    })
    .expect("package synthetic SDK");
    Fixture {
        temporary,
        package_dir: package_dir.clone(),
        request: MeasuredSdkReleaseLockRequest {
            package: SdkVerificationRequest {
                package_dir,
                identity,
                expected_tree_sha256: Sha256Digest::parse(&tree).unwrap(),
                expected_archive_sha256: package_output.archive_sha256,
                expected_archive_size: package_output.archive_size,
                forbidden_prefixes: vec![PathBuf::from("/tmp/aros-sdk-private-build-root")],
            },
            release_tag: "v0.9.0".into(),
            base_url: "https://example.invalid/releases/v0.9.0".into(),
        },
    }
}

fn stat(metadata: &Metadata) -> FileStat {
    FileStat {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        size: metadata.size(),
        mtime: metadata.mtime(),
        mtime_nsec: metadata.mtime_nsec(),
        ctime: metadata.ctime(),
        ctime_nsec: metadata.ctime_nsec(),
    }
}

fn package_snapshot(package_dir: &Path) -> PackageSnapshot {
    let root = stat(&fs::symlink_metadata(package_dir).unwrap());
    let mut entries = fs::read_dir(package_dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let kind = if metadata.file_type().is_symlink() {
                SnapshotKind::Symlink(fs::read_link(&path).unwrap())
            } else if metadata.is_file() {
                SnapshotKind::Regular(fs::read(&path).unwrap())
            } else {
                SnapshotKind::Other
            };
            SnapshotEntry {
                name: entry.file_name(),
                stat: stat(&metadata),
                kind,
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    PackageSnapshot { root, entries }
}

fn assert_rejected_without_package_writes(request: &MeasuredSdkReleaseLockRequest, dir: &Path) {
    let before = package_snapshot(dir);
    assert!(build_measured_sdk_release_lock(request).is_err());
    assert_eq!(package_snapshot(dir), before);
}

fn archive_path(request: &MeasuredSdkReleaseLockRequest) -> PathBuf {
    request
        .package
        .package_dir
        .join(request.package.identity.asset_name().unwrap())
}

fn manifest_path(request: &MeasuredSdkReleaseLockRequest) -> PathBuf {
    request.package.package_dir.join(format!(
        "{}.manifest.json",
        request.package.identity.asset_name().unwrap()
    ))
}

#[test]
fn measured_lock_is_canonical_bound_to_the_verified_package_and_read_only() {
    let fixture = make_fixture();
    let before = package_snapshot(&fixture.package_dir);
    let lock = build_measured_sdk_release_lock(&fixture.request).unwrap();

    assert_eq!(lock.release_tag, "v0.9.0");
    assert_eq!(lock.base_url, "https://example.invalid/releases/v0.9.0");
    assert_eq!(lock.identity.version, "0.0.0-local");
    assert_eq!(lock.identity, fixture.request.package.identity);
    assert_eq!(
        lock.tree_sha256,
        fixture.request.package.expected_tree_sha256
    );
    assert_eq!(
        lock.archive.sha256,
        fixture.request.package.expected_archive_sha256
    );
    assert_eq!(
        lock.archive.size,
        fixture.request.package.expected_archive_size
    );
    let manifest = fs::read(manifest_path(&fixture.request)).unwrap();
    assert_eq!(lock.manifest.sha256, sha256_bytes(&manifest));
    assert_eq!(lock.manifest.size, manifest.len() as u64);

    let canonical = lock.canonical_bytes().unwrap();
    assert_eq!(SdkReleaseLock::from_bytes(&canonical).unwrap(), lock);
    let verified = sdk_package::verify_locked_archive(&SdkLockedArchiveRequest {
        archive: archive_path(&fixture.request),
        lock,
    })
    .unwrap();
    assert_eq!(verified.manifest.identity, fixture.request.package.identity);
    assert_eq!(package_snapshot(&fixture.package_dir), before);
}

#[test]
fn invalid_selection_identity_tree_and_archive_expectations_are_rejected_read_only() {
    let fixture = make_fixture();

    let mut request = fixture.request.clone();
    request.release_tag = "0.9.0".into();
    assert_rejected_without_package_writes(&request, &fixture.package_dir);

    let mut request = fixture.request.clone();
    request.base_url = "https://example.invalid/releases/v0.8.0".into();
    assert_rejected_without_package_writes(&request, &fixture.package_dir);

    let mut request = fixture.request.clone();
    request.package.identity.source.commit = "3".repeat(40);
    assert_rejected_without_package_writes(&request, &fixture.package_dir);

    let mut request = fixture.request.clone();
    request.package.expected_tree_sha256 = sha256_bytes(b"different SDK tree");
    assert_rejected_without_package_writes(&request, &fixture.package_dir);

    let mut request = fixture.request.clone();
    request.package.expected_archive_sha256 = sha256_bytes(b"different archive");
    assert_rejected_without_package_writes(&request, &fixture.package_dir);

    let mut request = fixture.request.clone();
    request.package.expected_archive_size += 1;
    assert_rejected_without_package_writes(&request, &fixture.package_dir);
}

#[test]
fn extra_corrupt_and_missing_package_members_are_rejected_without_more_writes() {
    let fixture = make_fixture();
    fs::write(fixture.package_dir.join("unexpected.txt"), b"extra").unwrap();
    assert_rejected_without_package_writes(&fixture.request, &fixture.package_dir);

    let fixture = make_fixture();
    let checksum = fs::read_dir(&fixture.package_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sha256")
        })
        .unwrap();
    fs::write(&checksum, b"corrupt checksum\n").unwrap();
    assert_rejected_without_package_writes(&fixture.request, &fixture.package_dir);

    let fixture = make_fixture();
    fs::write(manifest_path(&fixture.request), b"not canonical JSON\n").unwrap();
    assert_rejected_without_package_writes(&fixture.request, &fixture.package_dir);

    let fixture = make_fixture();
    let archive = archive_path(&fixture.request);
    let mut archive_bytes = fs::read(&archive).unwrap();
    archive_bytes[0] ^= 1;
    fs::write(&archive, archive_bytes).unwrap();
    assert_rejected_without_package_writes(&fixture.request, &fixture.package_dir);

    let fixture = make_fixture();
    fs::remove_file(manifest_path(&fixture.request)).unwrap();
    assert_rejected_without_package_writes(&fixture.request, &fixture.package_dir);

    let fixture = make_fixture();
    fs::remove_file(archive_path(&fixture.request)).unwrap();
    assert_rejected_without_package_writes(&fixture.request, &fixture.package_dir);
}

#[test]
fn symlinked_outer_archive_or_manifest_is_rejected_without_following_it() {
    let fixture = make_fixture();
    let outside = fixture.temporary.path().join("outside-manifest");
    fs::write(&outside, b"not a package manifest").unwrap();
    fs::remove_file(manifest_path(&fixture.request)).unwrap();
    symlink(&outside, manifest_path(&fixture.request)).unwrap();
    assert_rejected_without_package_writes(&fixture.request, &fixture.package_dir);

    let fixture = make_fixture();
    let outside = fixture.temporary.path().join("outside-archive");
    fs::write(&outside, b"not an SDK archive").unwrap();
    fs::remove_file(archive_path(&fixture.request)).unwrap();
    symlink(&outside, archive_path(&fixture.request)).unwrap();
    assert_rejected_without_package_writes(&fixture.request, &fixture.package_dir);
}

#[test]
fn locked_archive_rejects_changed_archive_and_manifest_digest_or_size() {
    let fixture = make_fixture();
    let lock = build_measured_sdk_release_lock(&fixture.request).unwrap();
    let archive = archive_path(&fixture.request);

    let mut wrong_archive_digest = lock.clone();
    wrong_archive_digest.archive.sha256 = sha256_bytes(b"wrong selected archive");
    assert!(
        sdk_package::verify_locked_archive(&SdkLockedArchiveRequest {
            archive: archive.clone(),
            lock: wrong_archive_digest,
        })
        .is_err()
    );

    let mut wrong_archive_size = lock.clone();
    wrong_archive_size.archive.size += 1;
    assert!(
        sdk_package::verify_locked_archive(&SdkLockedArchiveRequest {
            archive: archive.clone(),
            lock: wrong_archive_size,
        })
        .is_err()
    );

    let mut wrong_manifest_digest = lock.clone();
    wrong_manifest_digest.manifest.sha256 = sha256_bytes(b"wrong selected manifest");
    assert!(
        sdk_package::verify_locked_archive(&SdkLockedArchiveRequest {
            archive: archive.clone(),
            lock: wrong_manifest_digest,
        })
        .is_err()
    );

    let mut wrong_manifest_size = lock;
    wrong_manifest_size.manifest.size += 1;
    assert!(
        sdk_package::verify_locked_archive(&SdkLockedArchiveRequest {
            archive,
            lock: wrong_manifest_size,
        })
        .is_err()
    );
}
