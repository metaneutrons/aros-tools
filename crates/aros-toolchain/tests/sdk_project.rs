#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use aros_common::sdk_manifest::{SdkAbiValue, SdkIdentity, SdkSourceIdentity, SdkTarget};
use aros_common::sdk_release_lock::{SdkArtifactDigest, SdkReleaseLock, SDK_RELEASE_LOCK_SCHEMA};
use aros_common::{sha256_bytes, toolchain_tree_inventory_excluding, CommitState, Sha256Digest};
use aros_toolchain::sdk_package::{self, SdkPackageRequest};
use aros_toolchain::sdk_project::{self, SdkProjectRequest};
use aros_toolchain::sdk_store::{self, SdkStoreRequest};

struct Fixture {
    identity: SdkIdentity,
    lock: SdkReleaseLock,
    candidate_lock: PathBuf,
    candidate_bytes: Vec<u8>,
    request: SdkProjectRequest,
}

fn identity(name: &str, version: &str, data_model: &str) -> SdkIdentity {
    SdkIdentity {
        sdk_id: "core".into(),
        version: version.into(),
        target: SdkTarget {
            target_triple: "i686-aros".into(),
            abi: BTreeMap::from([("data_model".into(), SdkAbiValue::Text(data_model.into()))]),
        },
        source: SdkSourceIdentity {
            commit: "1".repeat(40),
            git_tree: "2".repeat(40),
            content_sha256: sha256_bytes(format!("{name} source").as_bytes()),
            contract_sha256: sha256_bytes(format!("{name} contract").as_bytes()),
        },
        source_date_epoch: 946_684_800,
    }
}

fn fixture(root: &Path, name: &str, version: &str, data_model: &str) -> Fixture {
    let identity = identity(name, version, data_model);
    let candidate_root = root.join(format!("{name}-sdk-source"));
    fs::create_dir_all(candidate_root.join("bin")).unwrap();
    fs::create_dir_all(candidate_root.join("include")).unwrap();
    fs::write(candidate_root.join("bin/aros"), b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(
        candidate_root.join("bin/aros"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::write(
        candidate_root.join("include/aros.h"),
        b"#define AROS_SDK 1\n",
    )
    .unwrap();
    fs::write(
        candidate_root.join("toolchain-manifest.json"),
        b"fixture source metadata\n",
    )
    .unwrap();

    // Measure the package input tree independently of sdk_package's result.
    let (tree_hex, _) = toolchain_tree_inventory_excluding(&candidate_root, &["sdk-manifest.json"])
        .expect("measure fixture SDK source tree");
    let tree_sha256 = Sha256Digest::parse(&tree_hex).unwrap();
    let packaged = sdk_package::package(&SdkPackageRequest {
        candidate_root,
        output_dir: root.join(format!("{name}-package")),
        identity: identity.clone(),
        expected_tree_sha256: tree_sha256.clone(),
        forbidden_prefixes: vec![PathBuf::from("/tmp/aros-sdk-project-fixture-private")],
    })
    .expect("package test SDK");

    // Derive artifact hashes and sizes from the files on disk, not the package
    // API's returned measurements.
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
    lock.validate().expect("valid fixture release lock");

    let candidate_lock = root.join(format!("{name}-candidate-lock.json"));
    let candidate_bytes = lock.canonical_bytes().unwrap();
    fs::write(&candidate_lock, &candidate_bytes).unwrap();

    let store = root.join("sdk-store");
    sdk_store::install(&SdkStoreRequest {
        store: store.clone(),
        lock: lock.clone(),
        archive: packaged.archive,
    })
    .expect("install test SDK in store");

    let project_parent = root.join(format!("{name}-project"));
    fs::create_dir_all(&project_parent).unwrap();
    let project_lock = project_parent.join("aros-sdk.lock.json");
    let request = SdkProjectRequest {
        candidate_lock: candidate_lock.clone(),
        candidate_sha256: sha256_bytes(&candidate_bytes),
        project_lock,
        store,
    };
    Fixture {
        identity,
        lock,
        candidate_lock,
        candidate_bytes,
        request,
    }
}

fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn preview_apply_readback_and_reuse_are_bounded_and_idempotent() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();
    let mut selected = fixture(&root, "primary", "1.2.3", "ilp32");
    let project_parent = selected.request.project_lock.parent().unwrap();
    let source_lock = project_parent.join("aros-source-lock.json");
    let generation_sentinel = project_parent.join("build/generation/sentinel");
    fs::create_dir_all(generation_sentinel.parent().unwrap()).unwrap();
    fs::write(&source_lock, b"source lock remains independent\n").unwrap();
    fs::write(&generation_sentinel, b"generation remains independent\n").unwrap();

    let candidate_before = fs::read(&selected.candidate_lock).unwrap();
    let preview = sdk_project::preview(&selected.request).expect("preview explicit SDK selection");
    assert!(preview.will_write);
    assert_eq!(preview.identity, selected.identity);
    assert_eq!(preview.tree_sha256, selected.lock.tree_sha256);
    assert!(!selected.request.project_lock.exists());
    assert_eq!(
        fs::read(&selected.candidate_lock).unwrap(),
        candidate_before
    );

    let applied = sdk_project::apply(&selected.request, &preview.confirmation)
        .expect("apply confirmed project selection");
    let canonical = selected.lock.canonical_bytes().unwrap();
    assert_eq!(applied.lock_sha256, sha256_bytes(&canonical));
    assert_eq!(applied.identity, selected.identity);
    assert!(!applied.already_selected);
    assert_eq!(fs::read(&selected.request.project_lock).unwrap(), canonical);
    assert_eq!(
        fs::read(&selected.candidate_lock).unwrap(),
        candidate_before
    );
    assert_eq!(
        fs::read(&source_lock).unwrap(),
        b"source lock remains independent\n"
    );
    assert_eq!(
        fs::read(&generation_sentinel).unwrap(),
        b"generation remains independent\n"
    );

    let snapshot = sdk_project::read_project_lock(&selected.request.project_lock)
        .expect("read canonical project selection");
    assert_eq!(snapshot.lock, selected.lock);
    assert_eq!(snapshot.raw_bytes, canonical);
    assert_eq!(snapshot.lock_sha256, applied.lock_sha256);
    snapshot
        .revalidate()
        .expect("revalidate unchanged project lock");

    let metadata_before_reuse = fs::metadata(&selected.request.project_lock).unwrap();
    let reuse_plan = sdk_project::preview(&selected.request).expect("preview existing selection");
    assert!(!reuse_plan.will_write);
    let reused = sdk_project::apply(&selected.request, &reuse_plan.confirmation)
        .expect("reuse identical canonical selection");
    let metadata_after_reuse = fs::metadata(&selected.request.project_lock).unwrap();
    assert!(reused.already_selected);
    assert_eq!(metadata_before_reuse.ino(), metadata_after_reuse.ino());
    assert_eq!(fs::read(&selected.request.project_lock).unwrap(), canonical);
    assert_eq!(
        fs::read(&selected.candidate_lock).unwrap(),
        selected.candidate_bytes
    );
    // Keep the request mutable in this test to prove callers may select an
    // explicit second project path without any root discovery behavior.
    selected.request.project_lock = root.join("another-app/aros-sdk.lock.json");
    fs::create_dir_all(selected.request.project_lock.parent().unwrap()).unwrap();
    let second = sdk_project::preview(&selected.request).unwrap();
    assert!(second.will_write);
    assert_eq!(second.project_lock, selected.request.project_lock);

    let old_snapshot = sdk_project::read_project_lock(snapshot.path()).unwrap();
    fs::write(old_snapshot.path(), b"external edit\n").unwrap();
    assert!(old_snapshot.revalidate().is_err());
}

#[test]
fn stale_confirmation_and_changed_candidate_fail_before_project_publication() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();

    let selected = fixture(&root, "candidate-stale", "1.2.3", "ilp32");
    let plan = sdk_project::preview(&selected.request).unwrap();
    let stale = sdk_project::apply(&selected.request, &sha256_bytes(b"wrong token"))
        .expect_err("reject an unrelated confirmation token");
    assert_eq!(
        stale.diagnostics().diagnostics[0]
            .context
            .as_ref()
            .and_then(|context| context.commit_state),
        Some(CommitState::RolledBack)
    );
    assert!(!selected.request.project_lock.exists());
    fs::write(
        &selected.candidate_lock,
        b"candidate changed after preview\n",
    )
    .unwrap();
    assert!(sdk_project::apply(&selected.request, &plan.confirmation).is_err());
    assert!(!selected.request.project_lock.exists());

    let selected = fixture(&root, "token-stale", "1.2.3", "ilp32");
    let plan = sdk_project::preview(&selected.request).unwrap();
    assert!(sdk_project::apply(&selected.request, &sha256_bytes(b"wrong token")).is_err());
    assert!(!selected.request.project_lock.exists());
    assert_eq!(
        fs::read(&selected.candidate_lock).unwrap(),
        selected.candidate_bytes
    );
    assert_eq!(
        plan.confirmation,
        sdk_project::preview(&selected.request)
            .unwrap()
            .confirmation
    );
}

#[test]
fn stale_prior_lock_snapshot_is_rejected_without_changing_the_newer_bytes() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();
    let old = fixture(&root, "old", "1.2.3", "ilp32");
    let new = fixture(&root, "new", "1.2.4", "ilp32");
    let project_lock = root.join("application/aros-sdk.lock.json");
    fs::create_dir_all(project_lock.parent().unwrap()).unwrap();
    fs::write(&project_lock, old.lock.canonical_bytes().unwrap()).unwrap();

    let mut request = new.request;
    request.project_lock = project_lock.clone();
    let plan = sdk_project::preview(&request).expect("preview same-ABI upgrade");
    assert!(plan.will_write);
    let bytes_after_preview = new.lock.canonical_bytes().unwrap();
    fs::write(&project_lock, &bytes_after_preview).unwrap();
    assert!(sdk_project::apply(&request, &plan.confirmation).is_err());
    assert_eq!(fs::read(&project_lock).unwrap(), bytes_after_preview);
}

#[test]
fn same_abi_lock_can_be_replaced_with_compare_and_swap() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();
    let old = fixture(&root, "upgrade-old", "1.2.3", "ilp32");
    let new = fixture(&root, "upgrade-new", "1.2.4", "ilp32");
    let project_lock = root.join("application/aros-sdk.lock.json");
    fs::create_dir_all(project_lock.parent().unwrap()).unwrap();
    fs::write(&project_lock, old.lock.canonical_bytes().unwrap()).unwrap();

    let mut request = new.request.clone();
    request.project_lock = project_lock.clone();
    let plan = sdk_project::preview(&request).unwrap();
    let selection = sdk_project::apply(&request, &plan.confirmation).unwrap();
    assert_eq!(
        fs::read(&project_lock).unwrap(),
        new.lock.canonical_bytes().unwrap()
    );
    assert_eq!(selection.lock_sha256, new.lock.digest().unwrap());
    assert!(!selection.already_selected);
}

#[test]
fn malformed_project_lock_and_abi_changes_are_rejected_without_repair() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();
    let selected = fixture(&root, "malformed-prior", "1.2.3", "ilp32");
    fs::write(&selected.request.project_lock, b"{not-json\n").unwrap();
    assert!(sdk_project::preview(&selected.request).is_err());
    assert_eq!(
        fs::read(&selected.request.project_lock).unwrap(),
        b"{not-json\n"
    );

    let old = fixture(&root, "abi-old", "1.2.3", "ilp32");
    let new = fixture(&root, "abi-new", "1.2.4", "lp64");
    let project_lock = root.join("abi-application/aros-sdk.lock.json");
    fs::create_dir_all(project_lock.parent().unwrap()).unwrap();
    let old_bytes = old.lock.canonical_bytes().unwrap();
    fs::write(&project_lock, &old_bytes).unwrap();
    let mut request = new.request;
    request.project_lock = project_lock.clone();
    assert!(sdk_project::preview(&request).is_err());
    assert_eq!(fs::read(&project_lock).unwrap(), old_bytes);
}

#[test]
fn corrupt_or_absent_store_is_neither_repaired_nor_created_by_preview() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();
    let selected = fixture(&root, "corrupt-store", "1.2.3", "ilp32");
    let envelope = sdk_store::envelope_path(&selected.request.store, &selected.lock).unwrap();
    let header = envelope.join("sdk/include/aros.h");
    fs::write(&header, b"tampered SDK header\n").unwrap();
    assert!(sdk_project::preview(&selected.request).is_err());
    assert_eq!(fs::read(&header).unwrap(), b"tampered SDK header\n");
    assert!(!selected.request.project_lock.exists());

    let selected = fixture(&root, "absent-store", "1.2.3", "ilp32");
    let absent = root.join("store-that-does-not-exist");
    let mut request = selected.request;
    request.store = absent.clone();
    assert!(sdk_project::preview(&request).is_err());
    assert!(!absent.exists());
    assert!(!request.project_lock.exists());
}

#[test]
fn symlink_hardlink_and_store_namespace_paths_are_rejected() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();
    let selected = fixture(&root, "path-checks", "1.2.3", "ilp32");

    let candidate_symlink = root.join("candidate-link.json");
    symlink(&selected.candidate_lock, &candidate_symlink).unwrap();
    let mut request = selected.request.clone();
    request.candidate_lock = candidate_symlink;
    assert!(sdk_project::preview(&request).is_err());

    let candidate_hardlink = root.join("candidate-hardlink.json");
    fs::hard_link(&selected.candidate_lock, &candidate_hardlink).unwrap();
    request = selected.request.clone();
    request.candidate_lock = candidate_hardlink;
    assert!(sdk_project::preview(&request).is_err());

    let linked_project = root.join("linked-project/aros-sdk.lock.json");
    fs::create_dir_all(linked_project.parent().unwrap()).unwrap();
    let project_target = root.join("outside-project-lock.json");
    let canonical = selected.lock.canonical_bytes().unwrap();
    fs::write(&project_target, &canonical).unwrap();
    symlink(&project_target, &linked_project).unwrap();
    request = selected.request.clone();
    request.project_lock = linked_project;
    assert!(sdk_project::preview(&request).is_err());
    assert_eq!(fs::read(&project_target).unwrap(), canonical);

    let hardlinked_project = root.join("hardlinked-project/aros-sdk.lock.json");
    fs::create_dir_all(hardlinked_project.parent().unwrap()).unwrap();
    let hardlink_target = root.join("outside-hardlinked-lock.json");
    fs::write(&hardlink_target, selected.lock.canonical_bytes().unwrap()).unwrap();
    fs::hard_link(&hardlink_target, &hardlinked_project).unwrap();
    request = selected.request.clone();
    request.project_lock = hardlinked_project;
    assert!(sdk_project::preview(&request).is_err());
    assert_eq!(
        fs::read(&hardlink_target).unwrap(),
        selected.lock.canonical_bytes().unwrap()
    );

    request = selected.request.clone();
    request.project_lock = selected.request.store.join("project-lock.json");
    assert!(sdk_project::preview(&request).is_err());
    assert!(!request.project_lock.exists());
}

#[test]
fn unnormalized_and_identical_candidate_project_paths_are_rejected() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();
    let selected = fixture(&root, "normalized-paths", "1.2.3", "ilp32");

    let mut request = selected.request.clone();
    let leaf = selected
        .candidate_lock
        .file_name()
        .unwrap()
        .to_str()
        .unwrap();
    request.candidate_lock = PathBuf::from(format!("{}/./{leaf}", root.display()));
    assert!(sdk_project::preview(&request).is_err());

    request = selected.request.clone();
    request.project_lock = request.candidate_lock.clone();
    assert!(sdk_project::preview(&request).is_err());
    assert_eq!(
        fs::read(&selected.candidate_lock).unwrap(),
        selected.candidate_bytes
    );
}

#[test]
fn malformed_candidate_is_rejected_without_creating_a_project_lock() {
    let temporary = root();
    let root = temporary.path().canonicalize().unwrap();
    let selected = fixture(&root, "bad-candidate", "1.2.3", "ilp32");
    let bad_bytes = b"{\"schema\":\"not-an-sdk-lock\"}\n";
    fs::write(&selected.candidate_lock, bad_bytes).unwrap();
    let mut request = selected.request.clone();
    request.candidate_sha256 = sha256_bytes(bad_bytes);
    assert!(sdk_project::preview(&request).is_err());
    assert!(!request.project_lock.exists());
    assert_eq!(fs::read(&selected.candidate_lock).unwrap(), bad_bytes);
}
