#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::sdk_release_lock::{SdkArtifactDigest, SdkReleaseLock, SDK_RELEASE_LOCK_SCHEMA};
use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding};
use aros_toolchain::sdk_package::{self, SdkPackageRequest};
use aros_toolchain::sdk_store::{self, SdkStoreRequest};

const FORBIDDEN_PREFIX: &str = "/tmp/aros-sdk-store-private-fixture";

struct PackagedSdk {
    identity: SdkIdentity,
    lock: SdkReleaseLock,
    archive: PathBuf,
}

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

fn package_sdk(root: &Path, name: &str, identity: SdkIdentity) -> PackagedSdk {
    let candidate = root.join(format!("{name}-candidate"));
    fs::create_dir(&candidate).unwrap();
    fs::create_dir(candidate.join("bin")).unwrap();
    fs::create_dir(candidate.join("include")).unwrap();
    fs::create_dir_all(candidate.join("include/Größe")).unwrap();
    fs::write(candidate.join("bin/aros"), b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(
        candidate.join("bin/aros"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::write(candidate.join("include/aros.h"), b"#define AROS_SDK 1\n").unwrap();
    fs::write(
        candidate.join("include/Größe/über.h"),
        b"/* UTF-8 SDK header */\n",
    )
    .unwrap();
    fs::write(
        candidate.join("toolchain-manifest.json"),
        b"source metadata\n",
    )
    .unwrap();
    symlink("bin/aros", candidate.join("aros")).expect("create internal SDK link fixture");

    let (tree_hex, _) = toolchain_tree_inventory_excluding(&candidate, &["sdk-manifest.json"])
        .expect("fixture SDK inventory");
    let tree_sha256 = aros_common::Sha256Digest::parse(&tree_hex).unwrap();
    let output_dir = root.join(format!("{name}-package"));
    let packaged = sdk_package::package(&SdkPackageRequest {
        candidate_root: candidate,
        output_dir,
        identity: identity.clone(),
        expected_tree_sha256: tree_sha256.clone(),
        forbidden_prefixes: vec![PathBuf::from(FORBIDDEN_PREFIX)],
    })
    .expect("package deterministic SDK fixture");

    // Select lock values from the fixture files themselves, rather than copying
    // the package API's reported measurements into a release lock.
    let archive_bytes = fs::read(&packaged.archive).unwrap();
    let manifest_bytes = fs::read(&packaged.manifest).unwrap();
    let release_tag = format!("v{}", identity.version);
    let lock = SdkReleaseLock {
        schema: SDK_RELEASE_LOCK_SCHEMA.into(),
        release_tag: release_tag.clone(),
        base_url: format!("https://example.invalid/releases/{release_tag}"),
        identity: identity.clone(),
        tree_sha256,
        archive: SdkArtifactDigest {
            sha256: sha256_bytes(&archive_bytes),
            size: archive_bytes.len() as u64,
        },
        manifest: SdkArtifactDigest {
            sha256: sha256_bytes(&manifest_bytes),
            size: manifest_bytes.len() as u64,
        },
    };
    lock.validate().expect("fixture release lock is valid");

    PackagedSdk {
        identity,
        lock,
        archive: packaged.archive,
    }
}

fn request(store: &Path, package: &PackagedSdk) -> SdkStoreRequest {
    SdkStoreRequest {
        store: store.to_path_buf(),
        lock: package.lock.clone(),
        archive: package.archive.clone(),
    }
}

fn lock_fixture() -> SdkReleaseLock {
    let identity = identity();
    let release_tag = format!("v{}", identity.version);
    SdkReleaseLock {
        schema: SDK_RELEASE_LOCK_SCHEMA.into(),
        release_tag: release_tag.clone(),
        base_url: format!("https://example.invalid/releases/{release_tag}"),
        identity,
        tree_sha256: sha256_bytes(b"selected tree"),
        archive: SdkArtifactDigest {
            sha256: sha256_bytes(b"selected archive"),
            size: b"selected archive".len() as u64,
        },
        manifest: SdkArtifactDigest {
            sha256: sha256_bytes(b"selected manifest"),
            size: b"selected manifest".len() as u64,
        },
    }
}

fn expected_envelope(store: &Path, lock: &SdkReleaseLock) -> PathBuf {
    store
        .join("v2")
        .join(&lock.identity.sdk_id)
        .join(&lock.identity.version)
        .join(lock.digest().unwrap().to_string())
}

#[test]
fn envelope_path_rejects_relative_root_and_unnormalized_store_paths() {
    let lock = lock_fixture();
    for store in [
        Path::new("relative-store"),
        Path::new("/"),
        Path::new("/tmp/./sdk-store"),
        Path::new("/tmp//sdk-store"),
        Path::new("/tmp/../sdk-store"),
    ] {
        assert!(
            sdk_store::envelope_path(store, &lock).is_err(),
            "accepted malformed store path {}",
            store.display()
        );
    }
}

fn installed_fixture(root: &Path, name: &str) -> (PackagedSdk, PathBuf, PathBuf) {
    let package = package_sdk(root, name, identity());
    let store = root.join(format!("{name}-store"));
    let installed =
        sdk_store::install(&request(&store, &package)).expect("install valid SDK fixture");
    (package, store, installed.envelope)
}

#[test]
fn install_reuses_verifies_and_lists_a_host_independent_unicode_sdk() {
    let temporary = tempfile::tempdir().unwrap();
    let package = package_sdk(temporary.path(), "primary", identity());
    let store = temporary.path().join("store");
    let selected = request(&store, &package);

    let installed = sdk_store::install(&selected).expect("install valid SDK");
    let envelope = expected_envelope(&store, &package.lock);
    assert_eq!(installed.envelope, envelope);
    assert_eq!(installed.payload, envelope.join("sdk"));
    assert_eq!(installed.lock_sha256, package.lock.digest().unwrap());
    assert_eq!(installed.manifest.identity, package.identity);
    assert!(!installed.already_installed);
    assert_eq!(
        fs::read(installed.payload.join("include/Größe/über.h")).unwrap(),
        b"/* UTF-8 SDK header */\n"
    );

    let reused = sdk_store::install(&selected).expect("reuse identical SDK installation");
    assert!(reused.already_installed);
    assert_eq!(reused.envelope, installed.envelope);
    assert_eq!(reused.payload, installed.payload);

    let verified = sdk_store::verify(&store, &package.lock).expect("verify installed SDK");
    assert_eq!(verified.envelope, installed.envelope);
    assert_eq!(verified.payload, installed.payload);
    assert_eq!(verified.manifest.identity, package.identity);

    let inventory = sdk_store::inventory(&store, 10_000).expect("inventory installed SDK");
    assert!(!inventory.truncated);
    assert!(inventory.findings.is_empty());
    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.entries[0].lock, package.lock);
    assert_eq!(inventory.entries[0].integrity, "not-checked");
}

#[test]
fn modified_archive_is_rejected_without_publishing_a_final_envelope() {
    let temporary = tempfile::tempdir().unwrap();
    let package = package_sdk(temporary.path(), "corrupt-archive", identity());
    let store = temporary.path().join("store");
    let expected = expected_envelope(&store, &package.lock);
    let mut bytes = fs::read(&package.archive).unwrap();
    let index = bytes.len() / 2;
    bytes[index] ^= 0x01;
    fs::write(&package.archive, &bytes).unwrap();

    assert!(sdk_store::install(&request(&store, &package)).is_err());
    assert!(
        !expected.exists(),
        "failed verification must publish no envelope"
    );
}

#[test]
fn verify_of_absent_store_creates_neither_store_nor_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let package = package_sdk(temporary.path(), "verify-absent", identity());
    let store = temporary.path().join("absent-store");

    assert!(sdk_store::verify(&store, &package.lock).is_err());
    assert!(!store.exists());
    assert!(!store.join(".sdk-store.lock").exists());
}

#[test]
fn selected_manifest_or_tree_digest_mismatch_fails_before_creating_store() {
    for selected in ["manifest", "tree"] {
        let temporary = tempfile::tempdir().unwrap();
        let package = package_sdk(temporary.path(), selected, identity());
        let store = temporary.path().join("store");
        let mut lock = package.lock.clone();
        match selected {
            "manifest" => lock.manifest.sha256 = sha256_bytes(b"wrong selected manifest"),
            "tree" => lock.tree_sha256 = sha256_bytes(b"wrong selected tree"),
            _ => unreachable!(),
        }

        let request = SdkStoreRequest {
            store: store.clone(),
            lock,
            archive: package.archive,
        };
        assert!(
            sdk_store::install(&request).is_err(),
            "accepted an altered selected {selected} digest"
        );
        assert!(!store.exists(), "failed {selected} check created the store");
        assert!(
            !store.join(".sdk-store.lock").exists(),
            "failed {selected} check created a store lock"
        );
    }
}

#[test]
fn installed_header_manifest_completion_and_lock_tampering_is_rejected_without_repair() {
    for selected in ["header", "manifest", "completion", "lock"] {
        let temporary = tempfile::tempdir().unwrap();
        let (package, store, envelope) = installed_fixture(temporary.path(), selected);
        let (path, replacement) = match selected {
            "header" => (
                envelope.join("sdk/include/aros.h"),
                b"tampered header\n".as_slice(),
            ),
            "manifest" => (
                envelope.join("sdk/sdk-manifest.json"),
                b"tampered manifest\n".as_slice(),
            ),
            "completion" => (envelope.join(".complete"), b"incomplete\n".as_slice()),
            "lock" => (envelope.join("lock.json"), b"{}\n".as_slice()),
            _ => unreachable!(),
        };
        fs::write(&path, replacement).unwrap();

        assert!(
            sdk_store::verify(&store, &package.lock).is_err(),
            "verification accepted modified {selected}"
        );
        assert_eq!(
            fs::read(&path).unwrap(),
            replacement,
            "verification must not repair modified {selected}"
        );
    }
}

#[test]
fn extra_envelope_member_is_rejected_and_inventory_reports_malformed_metadata() {
    let temporary = tempfile::tempdir().unwrap();
    let (package, store, envelope) = installed_fixture(temporary.path(), "extra-member");
    let extra = envelope.join("unexpected.txt");
    fs::write(&extra, b"outside the closed envelope\n").unwrap();

    assert!(sdk_store::verify(&store, &package.lock).is_err());
    assert_eq!(fs::read(&extra).unwrap(), b"outside the closed envelope\n");

    let inventory = sdk_store::inventory(&store, 10_000).expect("malformed entry is reportable");
    assert!(
        !inventory.findings.is_empty(),
        "inventory must report the malformed envelope"
    );
    assert!(inventory
        .findings
        .iter()
        .all(|finding| !finding.path.as_os_str().is_empty() && !finding.reason.is_empty()));
}

#[test]
fn full_verification_rejects_hardlinked_payload_and_envelope_lock_files() {
    let temporary = tempfile::tempdir().unwrap();
    let (package, store, envelope) = installed_fixture(temporary.path(), "hardlinked-payload");
    let header = envelope.join("sdk/include/aros.h");
    let outside_header = temporary.path().join("outside-header.h");
    fs::write(&outside_header, b"#define AROS_SDK 1\n").unwrap();
    fs::remove_file(&header).unwrap();
    fs::hard_link(&outside_header, &header).unwrap();
    assert!(sdk_store::verify(&store, &package.lock).is_err());
    assert_eq!(fs::read(&outside_header).unwrap(), b"#define AROS_SDK 1\n");

    let (package, store, envelope) = installed_fixture(temporary.path(), "hardlinked-lock");
    let lock_file = envelope.join("lock.json");
    let outside_lock = temporary.path().join("outside-lock.json");
    let original_lock = fs::read(&lock_file).unwrap();
    fs::write(&outside_lock, &original_lock).unwrap();
    fs::remove_file(&lock_file).unwrap();
    fs::hard_link(&outside_lock, &lock_file).unwrap();
    assert!(sdk_store::verify(&store, &package.lock).is_err());
    assert_eq!(fs::read(&outside_lock).unwrap(), original_lock);
}

#[test]
fn metadata_inventory_does_not_claim_payload_integrity_but_full_verify_detects_tampering() {
    let temporary = tempfile::tempdir().unwrap();
    let (package, store, envelope) = installed_fixture(temporary.path(), "metadata-only");
    let header = envelope.join("sdk/include/aros.h");
    fs::write(&header, b"#define AROS_SDK 999\n").unwrap();

    let inventory = sdk_store::inventory(&store, 10_000).expect("metadata-only observation");
    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.entries[0].integrity, "not-checked");
    assert_eq!(inventory.entries[0].lock, package.lock);
    assert!(sdk_store::verify(&store, &package.lock).is_err());
    assert_eq!(fs::read(&header).unwrap(), b"#define AROS_SDK 999\n");
}

#[test]
fn same_sdk_id_and_version_with_different_abi_use_distinct_lock_digest_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let first = package_sdk(temporary.path(), "abi-ilp32", identity());
    let mut second_identity = identity();
    second_identity
        .target
        .abi
        .insert("data_model".into(), SdkAbiValue::Text("lp64".into()));
    let second = package_sdk(temporary.path(), "abi-lp64", second_identity);
    assert_ne!(first.lock.digest().unwrap(), second.lock.digest().unwrap());

    let store = temporary.path().join("store");
    let first_install = sdk_store::install(&request(&store, &first)).unwrap();
    let second_install = sdk_store::install(&request(&store, &second)).unwrap();
    assert_eq!(
        first_install.envelope.parent(),
        second_install.envelope.parent()
    );
    assert_ne!(first_install.envelope, second_install.envelope);
    assert_eq!(
        first_install
            .envelope
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        first.lock.digest().unwrap().to_string()
    );
    assert_eq!(
        second_install
            .envelope
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        second.lock.digest().unwrap().to_string()
    );
}

#[test]
fn inventory_truncates_at_its_entry_budget_and_rejects_zero_budget() {
    let temporary = tempfile::tempdir().unwrap();
    let first = package_sdk(temporary.path(), "budget-first", identity());
    let mut second_identity = identity();
    second_identity
        .target
        .abi
        .insert("data_model".into(), SdkAbiValue::Text("lp64".into()));
    let second = package_sdk(temporary.path(), "budget-second", second_identity);
    let store = temporary.path().join("store");
    sdk_store::install(&request(&store, &first)).unwrap();
    sdk_store::install(&request(&store, &second)).unwrap();

    let bounded = sdk_store::inventory(&store, 1).expect("bounded inventory");
    assert!(bounded.truncated);
    assert!(sdk_store::inventory(&store, 0).is_err());
    assert!(sdk_store::inventory(&store, usize::MAX).is_err());
}

#[test]
fn inventory_does_not_create_missing_store_or_follow_store_symlink() {
    let temporary = tempfile::tempdir().unwrap();
    let missing = temporary.path().join("missing-store");
    let _ = sdk_store::inventory(&missing, 8);
    assert!(!missing.exists(), "read-only inventory created its store");

    let real = temporary.path().join("real-store");
    fs::create_dir(&real).unwrap();
    let alias = temporary.path().join("store-alias");
    symlink(&real, &alias).unwrap();
    let observation =
        sdk_store::inventory(&alias, 8).expect("report unsafe store without following it");
    assert_eq!(observation.store_state, "unsafe");
    assert!(observation.entries.is_empty());
    assert!(!observation.findings.is_empty());
    assert_eq!(fs::read_dir(&real).unwrap().count(), 0);
}

#[test]
fn installed_payload_permission_changes_are_rejected_without_repair() {
    for (member, altered_mode) in [
        ("sdk/include/aros.h", 0o666),
        ("sdk/include/aros.h", 0o600),
        ("sdk/bin/aros", 0o4755),
        ("sdk/include", 0o775),
        ("sdk/sdk-manifest.json", 0o666),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let (package, store, envelope) = installed_fixture(temporary.path(), "mode-tamper");
        let changed = envelope.join(member);
        fs::set_permissions(&changed, fs::Permissions::from_mode(altered_mode)).unwrap();
        assert!(sdk_store::verify(&store, &package.lock).is_err());
        assert!(sdk_store::install(&request(&store, &package)).is_err());
        assert_eq!(
            fs::metadata(&changed).unwrap().permissions().mode() & 0o7777,
            altered_mode
        );
    }
}

#[test]
fn install_and_verify_reject_symlinked_store_metadata_and_payload() {
    let temporary = tempfile::tempdir().unwrap();
    let package = package_sdk(temporary.path(), "symlink-store", identity());
    let real_store = temporary.path().join("real-store");
    fs::create_dir(&real_store).unwrap();
    let store_alias = temporary.path().join("store-alias");
    symlink(&real_store, &store_alias).unwrap();
    assert!(sdk_store::install(&request(&store_alias, &package)).is_err());
    assert!(!expected_envelope(&real_store, &package.lock).exists());

    let (package, store, envelope) = installed_fixture(temporary.path(), "symlink-members");
    let outside_lock = temporary.path().join("outside-lock.json");
    let original_lock = fs::read(envelope.join("lock.json")).unwrap();
    fs::write(&outside_lock, &original_lock).unwrap();
    fs::remove_file(envelope.join("lock.json")).unwrap();
    symlink(&outside_lock, envelope.join("lock.json")).unwrap();
    assert!(sdk_store::verify(&store, &package.lock).is_err());
    assert_eq!(fs::read(&outside_lock).unwrap(), original_lock);

    let (package, store, envelope) = installed_fixture(temporary.path(), "symlink-payload");
    let payload = envelope.join("sdk");
    let moved_payload = temporary.path().join("moved-sdk-payload");
    fs::rename(&payload, &moved_payload).unwrap();
    symlink(&moved_payload, &payload).unwrap();
    assert!(sdk_store::verify(&store, &package.lock).is_err());
    assert_eq!(
        fs::read(moved_payload.join("include/aros.h")).unwrap(),
        b"#define AROS_SDK 1\n"
    );
}
