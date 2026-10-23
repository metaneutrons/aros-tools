//! Synthetic membership checks for native inventory capture evidence only.

#![cfg(all(test, unix))]

use super::check_evidence;
use aros_common::{
    make_native_invocation::PrivateNativeCaptureDirectory, measure_tree_content_cas,
    mesa_genmodule_collection::MESA_GENMODULE_SELECTION_FILE, publication_journal_path,
};
use aros_transpiler::publication::Publication;
use std::{
    ffi::OsString,
    fs,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

fn fresh_evidence_directory() -> (TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let evidence = temporary.path().join("evidence");
    fs::create_dir(&evidence).unwrap();
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o700)).unwrap();
    let evidence = evidence.canonicalize().unwrap();
    (temporary, evidence)
}

fn write_standard_evidence(directory: &Path) -> PrivateNativeCaptureDirectory {
    fs::write(
        directory.join(MESA_GENMODULE_SELECTION_FILE),
        b"synthetic inventory\n",
    )
    .unwrap();
    fs::write(directory.join(super::STDOUT), b"").unwrap();
    fs::write(directory.join(super::STDERR), b"").unwrap();
    PrivateNativeCaptureDirectory::open(directory).unwrap()
}

fn expected_lock_path(directory: &Path) -> PathBuf {
    Publication::for_output(directory.join(MESA_GENMODULE_SELECTION_FILE))
        .persistent_lock_path()
        .unwrap()
}

fn sorted_entry_names(directory: &Path) -> Vec<OsString> {
    let mut names = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn assert_refusal(held: &PrivateNativeCaptureDirectory, expected: &str) {
    let error = check_evidence(held).expect_err("invalid evidence must refuse");
    assert!(
        error.to_string().contains(expected),
        "unexpected refusal: {error}"
    );
}

#[test]
fn publication_inventory_lock_is_required_and_preserved() {
    let (_temporary, evidence) = fresh_evidence_directory();
    let selection_path = evidence.join(MESA_GENMODULE_SELECTION_FILE);
    let mut publication = Publication::for_output(selection_path.clone());
    let lock_path = publication.persistent_lock_path().unwrap();
    publication.present(selection_path, b"synthetic inventory\n".to_vec());
    publication.publish().unwrap();
    fs::write(evidence.join(super::STDOUT), b"").unwrap();
    fs::write(evidence.join(super::STDERR), b"").unwrap();
    let held = PrivateNativeCaptureDirectory::open(&evidence).unwrap();

    let before_names = sorted_entry_names(&evidence);
    let mut expected_names = vec![
        OsString::from(MESA_GENMODULE_SELECTION_FILE),
        OsString::from(super::STDOUT),
        OsString::from(super::STDERR),
        lock_path.file_name().unwrap().to_os_string(),
    ];
    expected_names.sort();
    assert_eq!(before_names, expected_names);
    assert_eq!(before_names.len(), 4);

    let before_lock = fs::symlink_metadata(&lock_path).unwrap();
    assert!(before_lock.file_type().is_file());
    assert_eq!(before_lock.len(), 0);
    assert_eq!(before_lock.nlink(), 1);
    let before_lock_bytes = fs::read(&lock_path).unwrap();

    check_evidence(&held).expect("valid Publication lock should be accepted");

    let after_lock = fs::symlink_metadata(&lock_path).unwrap();
    assert_eq!(sorted_entry_names(&evidence), before_names);
    assert_eq!(after_lock.dev(), before_lock.dev());
    assert_eq!(after_lock.ino(), before_lock.ino());
    assert_eq!(after_lock.nlink(), before_lock.nlink());
    assert_eq!(after_lock.len(), before_lock.len());
    assert_eq!(fs::read(&lock_path).unwrap(), before_lock_bytes);
}

#[test]
fn native_inventory_evidence_refuses_invalid_lock_membership_and_journal() {
    // Missing persistent lock.
    let (_temporary, evidence) = fresh_evidence_directory();
    let held = write_standard_evidence(&evidence);
    assert_refusal(&held, "publication lock is not a regular file");

    // A valid Publication lock for a different output does not bind this selection.
    let (_temporary, evidence) = fresh_evidence_directory();
    let held = write_standard_evidence(&evidence);
    let other_output = evidence.join("other-output.json");
    let mut other_publication = Publication::for_output(other_output.clone());
    other_publication.present(other_output.clone(), b"other\n".to_vec());
    other_publication.publish().unwrap();
    fs::remove_file(other_output).unwrap();
    assert_refusal(&held, "publication lock is not a regular file");

    // The expected lock name must still denote an empty regular file.
    let (_temporary, evidence) = fresh_evidence_directory();
    let held = write_standard_evidence(&evidence);
    fs::write(expected_lock_path(&evidence), b"not empty").unwrap();
    assert_refusal(&held, "publication lock must be empty and singly linked");

    // A symlink at the expected lock name is not a lock descriptor.
    let (temporary, evidence) = fresh_evidence_directory();
    let held = write_standard_evidence(&evidence);
    let link_target = temporary.path().join("lock-target");
    fs::write(&link_target, b"").unwrap();
    symlink(link_target, expected_lock_path(&evidence)).unwrap();
    assert_refusal(&held, "publication lock is not a regular file");

    // A hard link at the expected lock name fails the single-link requirement.
    let (temporary, evidence) = fresh_evidence_directory();
    let held = write_standard_evidence(&evidence);
    let hardlink_source = temporary.path().join("lock-source");
    fs::write(&hardlink_source, b"").unwrap();
    fs::hard_link(&hardlink_source, expected_lock_path(&evidence)).unwrap();
    assert_refusal(&held, "publication lock must be empty and singly linked");

    // Any extra regular evidence file remains an exact-membership refusal.
    let (_temporary, evidence) = fresh_evidence_directory();
    let held = write_standard_evidence(&evidence);
    fs::write(expected_lock_path(&evidence), b"").unwrap();
    fs::write(evidence.join("unexpected"), b"").unwrap();
    assert_refusal(&held, "evidence membership changed");

    // A leftover transaction journal is not accepted as completed evidence.
    let (_temporary, evidence) = fresh_evidence_directory();
    let held = write_standard_evidence(&evidence);
    fs::write(expected_lock_path(&evidence), b"").unwrap();
    let selection = evidence.join(MESA_GENMODULE_SELECTION_FILE);
    let journal = publication_journal_path(&selection, "transpiler").unwrap();
    fs::write(journal, b"unfinished publication\n").unwrap();
    assert_refusal(&held, "evidence membership changed");
}

/// Read-only membership check of actual publisher evidence, not owner reuse or
/// SDK qualification.
#[test]
#[ignore = "requires explicit retained native inventory evidence directory"]
fn actual_transpiler_inventory_evidence_retains_the_persistent_lock() {
    let evidence = PathBuf::from(
        std::env::var_os("AROS_TEST_NATIVE_INVENTORY_EVIDENCE")
            .expect("select actual native inventory evidence explicitly"),
    );
    assert!(evidence.is_absolute());
    let canonical = evidence.canonicalize().unwrap();
    assert_eq!(
        evidence, canonical,
        "selected evidence path must be canonical"
    );

    let before = measure_tree_content_cas(&canonical).unwrap();
    let held = PrivateNativeCaptureDirectory::open(&canonical).unwrap();
    check_evidence(&held).expect("actual Publication evidence should retain its exact lock");
    let after = measure_tree_content_cas(&canonical).unwrap();
    assert_eq!(
        before, after,
        "read-only evidence check must change no bytes"
    );
}
