//! Fault, race, recovery, containment, and portability tests.
use super::*;

#[test]
fn payload_casefold_keys_accept_utf8_without_weakening_cross_host_safety() {
    assert_eq!(
        payload_casefold_path_key(Path::new("share/Größe/marker-ä.txt")).unwrap(),
        payload_casefold_path_key(Path::new("share/größe/MARKER-Ä.TXT")).unwrap()
    );
    assert_eq!(
        payload_casefold_path_key(Path::new("share/café/marker.txt")).unwrap(),
        payload_casefold_path_key(Path::new("share/cafe\u{301}/MARKER.TXT")).unwrap()
    );
    for unsafe_path in ["../escape", "share/CON", "share/name:", "share/name."] {
        assert!(payload_casefold_path_key(Path::new(unsafe_path)).is_err());
    }
}

#[cfg(unix)]
#[test]
fn nofollow_regular_reader_refuses_a_final_symlink() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("target");
    let link = temporary.path().join("link");
    std::fs::write(&target, b"fixture").unwrap();
    symlink(&target, &link).unwrap();
    assert!(open_regular_file_nofollow(&link).is_err());
}

#[test]
fn bounded_regular_reader_rejects_an_oversized_control_document() {
    let temporary = tempfile::tempdir().unwrap();
    let document = temporary.path().join("candidate.lock");
    std::fs::write(&document, b"12345").unwrap();

    let error = measure_regular_file_bounded(&document, 4).unwrap_err();
    assert!(error.to_string().contains("4-byte read limit"));
    assert_eq!(
        measure_regular_file_bounded(&document, 5)
            .unwrap()
            .unwrap()
            .1,
        b"12345"
    );
}

#[cfg(unix)]
#[test]
fn advisory_lock_has_one_live_holder_and_can_be_reacquired_after_release() {
    let temporary = tempfile::tempdir().unwrap();
    let lock_path = temporary.path().join("management/store.lock");

    let first = AdvisoryFileLock::acquire(&lock_path).unwrap();
    first.revalidate().unwrap();
    assert!(AdvisoryFileLock::acquire(&lock_path).is_err());
    drop(first);

    let second = AdvisoryFileLock::acquire(&lock_path).unwrap();
    second.revalidate().unwrap();
}

#[cfg(unix)]
#[test]
fn advisory_lock_probe_is_read_only_and_distinguishes_live_ownership() {
    let temporary = tempfile::tempdir().unwrap();
    let lock_path = temporary.path().join("management/store.lock");

    assert_eq!(
        probe_advisory_file_lock(&lock_path).unwrap().state,
        AdvisoryLockState::Absent
    );
    assert!(!lock_path.exists());

    let guard = AdvisoryFileLock::acquire(&lock_path).unwrap();
    assert_eq!(
        probe_advisory_file_lock(&lock_path).unwrap().state,
        AdvisoryLockState::Held
    );
    drop(guard);

    assert_eq!(
        probe_advisory_file_lock(&lock_path).unwrap().state,
        AdvisoryLockState::Unheld
    );
}

#[cfg(unix)]
#[test]
fn advisory_lock_revalidation_rejects_a_substituted_lock_path() {
    let temporary = tempfile::tempdir().unwrap();
    let lock_path = temporary.path().join("management/store.lock");
    let guard = AdvisoryFileLock::acquire(&lock_path).unwrap();
    let original_identity = guard.identity().unwrap();
    let displaced = temporary.path().join("management/displaced.lock");
    std::fs::rename(&lock_path, &displaced).unwrap();
    std::fs::write(&lock_path, b"unlocked replacement").unwrap();

    let replacement = probe_advisory_file_lock(&lock_path).unwrap();
    assert_ne!(replacement.identity, Some(original_identity));
    assert_eq!(replacement.state, AdvisoryLockState::Unheld);
    assert!(guard.revalidate().is_err());
}

#[test]
fn publication_lock_names_have_one_shared_validation_contract() {
    let journal =
        publication_journal_path(Path::new("/tmp/managed-envelope"), "prepared-tree").unwrap();
    let lock = publication_journal_lock_path(&journal).unwrap();
    assert!(is_publication_journal_lock_name(
        lock.file_name().unwrap().to_str().unwrap()
    ));
    assert!(!is_publication_journal_lock_name(
        ".aros-lock-not-a-digest-0000000000000000"
    ));
}

#[cfg(unix)]
#[test]
fn snapshot_bound_tree_removal_never_follows_a_link_or_touches_a_sibling() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let owned = temporary.path().join("owned");
    let sibling = temporary.path().join("sibling");
    std::fs::create_dir_all(owned.join("nested")).unwrap();
    std::fs::write(owned.join("nested/payload"), b"owned").unwrap();
    std::fs::write(&sibling, b"must-survive").unwrap();
    symlink(&sibling, owned.join("outside-link")).unwrap();
    let limits = TreeTraversalLimits::new(16, 1024).unwrap();
    let snapshot = measure_tree_content_cas_bounded(&owned, limits).unwrap();

    remove_tree_from_snapshot_nofollow(&owned, &snapshot, limits).unwrap();

    assert!(!owned.exists());
    assert_eq!(std::fs::read(&sibling).unwrap(), b"must-survive");
}

#[cfg(unix)]
#[test]
fn snapshot_bound_tree_removal_refuses_a_changed_tree_without_deleting_it() {
    let temporary = tempfile::tempdir().unwrap();
    let owned = temporary.path().join("owned");
    std::fs::create_dir_all(owned.join("nested")).unwrap();
    let payload = owned.join("nested/payload");
    std::fs::write(&payload, b"before").unwrap();
    let limits = TreeTraversalLimits::new(16, 1024).unwrap();
    let snapshot = measure_tree_content_cas_bounded(&owned, limits).unwrap();
    std::fs::write(&payload, b"after").unwrap();

    assert!(remove_tree_from_snapshot_nofollow(&owned, &snapshot, limits).is_err());
    assert_eq!(std::fs::read(payload).unwrap(), b"after");
}

#[cfg(unix)]
#[test]
fn snapshot_bound_tree_removal_refuses_a_group_writable_payload() {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = tempfile::tempdir().unwrap();
    let owned = temporary.path().join("owned");
    std::fs::create_dir(&owned).unwrap();
    let payload = owned.join("payload");
    std::fs::write(&payload, b"fixture").unwrap();
    let limits = TreeTraversalLimits::new(16, 1024).unwrap();
    let snapshot = measure_tree_content_cas_bounded(&owned, limits).unwrap();
    std::fs::set_permissions(&payload, std::fs::Permissions::from_mode(0o664)).unwrap();

    let error = remove_tree_from_snapshot_nofollow(&owned, &snapshot, limits).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(owned.is_dir());
    assert_eq!(std::fs::read(payload).unwrap(), b"fixture");
}

#[cfg(unix)]
#[test]
fn snapshot_bound_tree_removal_refuses_a_hard_linked_payload() {
    let temporary = tempfile::tempdir().unwrap();
    let owned = temporary.path().join("owned");
    std::fs::create_dir(&owned).unwrap();
    let payload = owned.join("payload");
    let alias = temporary.path().join("payload-alias");
    std::fs::write(&payload, b"fixture").unwrap();
    std::fs::hard_link(&payload, &alias).unwrap();
    let limits = TreeTraversalLimits::new(16, 1024).unwrap();
    let snapshot = measure_tree_content_cas_bounded(&owned, limits).unwrap();

    let error = remove_tree_from_snapshot_nofollow(&owned, &snapshot, limits).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(owned.is_dir());
    assert_eq!(std::fs::read(&payload).unwrap(), b"fixture");
    assert_eq!(std::fs::read(alias).unwrap(), b"fixture");
}

#[cfg(unix)]
#[test]
fn snapshot_bound_tree_removal_refuses_a_group_writable_ancestor() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let temporary = tempfile::tempdir().unwrap();
    let owned = temporary.path().join("owned");
    std::fs::create_dir(&owned).unwrap();
    std::fs::write(owned.join("payload"), b"fixture").unwrap();
    let limits = TreeTraversalLimits::new(16, 1024).unwrap();
    let snapshot = measure_tree_content_cas_bounded(&owned, limits).unwrap();
    let mode = std::fs::metadata(temporary.path()).unwrap().mode() & 0o7777;
    std::fs::set_permissions(
        temporary.path(),
        std::fs::Permissions::from_mode(mode | 0o020),
    )
    .unwrap();

    let result = remove_tree_from_snapshot_nofollow(&owned, &snapshot, limits);

    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(mode)).unwrap();
    let error = result.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(owned.is_dir());
}

#[cfg(unix)]
#[test]
fn snapshot_bound_tree_removal_permits_a_sticky_writable_ancestor() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let temporary = tempfile::tempdir().unwrap();
    let owned = temporary.path().join("owned");
    std::fs::create_dir(&owned).unwrap();
    std::fs::write(owned.join("payload"), b"fixture").unwrap();
    let limits = TreeTraversalLimits::new(16, 1024).unwrap();
    let snapshot = measure_tree_content_cas_bounded(&owned, limits).unwrap();
    let mode = std::fs::metadata(temporary.path()).unwrap().mode() & 0o7777;
    std::fs::set_permissions(
        temporary.path(),
        std::fs::Permissions::from_mode(mode | 0o1022),
    )
    .unwrap();

    let result = remove_tree_from_snapshot_nofollow(&owned, &snapshot, limits);

    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(mode)).unwrap();
    result.unwrap();
    assert!(!owned.exists());
}

#[cfg(unix)]
#[test]
fn snapshot_bound_tree_removal_refuses_a_same_name_swap_before_unlink() {
    let temporary = tempfile::tempdir().unwrap();
    let owned = temporary.path().join("owned");
    let payload = owned.join("payload");
    std::fs::create_dir(&owned).unwrap();
    std::fs::write(&payload, b"approved").unwrap();
    let limits = TreeTraversalLimits::new(16, 1024).unwrap();
    let snapshot = measure_tree_content_cas_bounded(&owned, limits).unwrap();
    let displaced = owned.join("displaced-approved");
    let displaced_for_action = displaced.clone();

    let result = at_boundary(
        "snapshot-removal-before-regular-unlink",
        |path| path.file_name() == Some(std::ffi::OsStr::new("payload")),
        move |path| {
            std::fs::rename(path, &displaced_for_action).unwrap();
            std::fs::write(path, b"unapproved replacement").unwrap();
        },
        || remove_tree_from_snapshot_nofollow(&owned, &snapshot, limits),
    );

    assert!(result.is_err());
    assert_eq!(std::fs::read(&payload).unwrap(), b"unapproved replacement");
    assert_eq!(std::fs::read(displaced).unwrap(), b"approved");
}

#[cfg(unix)]
#[test]
fn bounded_nofollow_directory_listing_refuses_a_symlink() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path().join("directory");
    let link = temporary.path().join("link");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("entry"), b"fixture").unwrap();
    symlink(&directory, &link).unwrap();

    assert_eq!(
        directory_entry_names_nofollow_bounded(&directory, 4).unwrap(),
        vec![std::ffi::OsString::from("entry")]
    );
    assert!(directory_entry_names_nofollow_bounded(&link, 4).is_err());
}

#[cfg(unix)]
struct BoundaryAction {
    point: &'static str,
    matches: Box<dyn Fn(&Path) -> bool>,
    action: Box<dyn FnOnce(&Path)>,
}

#[cfg(unix)]
std::thread_local! {
    static BOUNDARY_ACTION: std::cell::RefCell<Option<BoundaryAction>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Inject on the tested thread at an exact boundary, never after a timing guess.
/// This entire module and both call sites are absent from non-test builds.
#[cfg(unix)]
pub(super) fn run_boundary(point: &str, path: &Path) {
    let action = BOUNDARY_ACTION.with_borrow_mut(|slot| {
        if slot
            .as_ref()
            .is_some_and(|hook| hook.point == point && (hook.matches)(path))
        {
            slot.take()
        } else {
            None
        }
    });
    if let Some(hook) = action {
        (hook.action)(path);
    }
}

#[cfg(unix)]
fn at_boundary<T>(
    point: &'static str,
    matches: impl Fn(&Path) -> bool + 'static,
    action: impl FnOnce(&Path) + 'static,
    operation: impl FnOnce() -> T,
) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BOUNDARY_ACTION.with_borrow_mut(|slot| *slot = None);
        }
    }
    BOUNDARY_ACTION.with_borrow_mut(|slot| {
        assert!(slot.is_none(), "boundary actions must not overlap");
        *slot = Some(BoundaryAction {
            point,
            matches: Box::new(matches),
            action: Box::new(action),
        });
    });
    let _reset = Reset;
    let result = operation();
    assert!(
        BOUNDARY_ACTION.with_borrow(Option::is_none),
        "selected boundary was not exercised"
    );
    result
}

#[cfg(unix)]
struct FaultAction {
    point: &'static str,
    matches: Box<dyn Fn(&Path) -> bool>,
    error: std::io::Error,
}

#[cfg(unix)]
std::thread_local! {
    static FAULT_ACTION: std::cell::RefCell<Option<FaultAction>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Return one selected test error at an exact publication boundary.
///
/// The hook is thread-local and one-shot, so it cannot affect another
/// publication running in a different test thread or persist into a retry.
#[cfg(unix)]
pub(super) fn run_fault_point(point: &str, path: &Path) -> std::io::Result<()> {
    let action = FAULT_ACTION.with_borrow_mut(|slot| {
        if slot
            .as_ref()
            .is_some_and(|hook| hook.point == point && (hook.matches)(path))
        {
            slot.take()
        } else {
            None
        }
    });
    if let Some(action) = action {
        Err(action.error)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn with_fault_point<T>(
    point: &'static str,
    matches: impl Fn(&Path) -> bool + 'static,
    error: std::io::Error,
    operation: impl FnOnce() -> T,
) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAULT_ACTION.with_borrow_mut(|slot| *slot = None);
        }
    }
    FAULT_ACTION.with_borrow_mut(|slot| {
        assert!(slot.is_none(), "fault actions must not overlap");
        *slot = Some(FaultAction {
            point,
            matches: Box::new(matches),
            error,
        });
    });
    let _reset = Reset;
    let result = operation();
    assert!(
        FAULT_ACTION.with_borrow(Option::is_none),
        "selected fault boundary was not exercised"
    );
    result
}

#[cfg(unix)]
static FAULT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
struct FaultEnvironmentGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl Drop for FaultEnvironmentGuard {
    fn drop(&mut self) {
        std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");
        std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_PATH");
        std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
        std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");
    }
}

#[cfg(unix)]
fn lock_fault_environment() -> FaultEnvironmentGuard {
    let lock = FAULT_ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_PATH");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");
    FaultEnvironmentGuard { _lock: lock }
}

#[cfg(unix)]
fn scoped_test_point(point: &str) -> String {
    format!(
        "{point}@{}",
        std::thread::current()
            .name()
            .expect("Rust test threads have stable names")
    )
}

#[test]
fn portable_names_reject_traversal_and_windows_aliases() {
    for invalid in ["", ".", "..", "a/b", "a\\b", "CON", "lpt1.txt", "x. "] {
        assert!(PortableOutputName::new(invalid).is_err(), "{invalid}");
    }
    assert_eq!(
        PortableOutputName::new("exec.library").unwrap().as_str(),
        "exec.library"
    );
}

#[test]
fn case_aliases_share_one_publication_journal_namespace() {
    let root = Path::new("/tmp/aros-publication-namespace-test");
    assert_eq!(
        publication_journal_path(&root.join("Output"), "tree").unwrap(),
        publication_journal_path(&root.join("output"), "tree").unwrap()
    );
}

#[test]
fn canonical_source_rejects_escape() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert!(canonical_source_file(root.path(), outside.path()).is_err());
}

#[cfg(unix)]
#[test]
fn replacement_uses_a_new_inode_without_mutating_existing_hardlinks() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    let hardlink = root.path().join("hardlink");
    std::fs::write(&target, b"before").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::fs::hard_link(&target, &hardlink).unwrap();
    let original = std::fs::metadata(&target).unwrap();

    let mut transaction = DurableFileSet::new(root.path().join("journal")).unwrap();
    transaction.stage_write(&target, b"after").unwrap();
    transaction.commit().unwrap();

    let published = std::fs::metadata(&target).unwrap();
    assert_ne!(published.ino(), original.ino());
    assert_eq!(std::fs::read(&hardlink).unwrap(), b"before");
    assert_eq!(original.permissions().mode() & 0o777, 0o640);
}

#[cfg(unix)]
#[test]
fn durable_file_set_enforces_only_exact_portable_output_modes() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    let target = root.path().join("tool");

    let mut first = DurableFileSet::new(&journal).unwrap();
    assert!(first
        .stage_write_mode(&target, b"executable", 0o755)
        .unwrap());
    first.commit().unwrap();
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o755
    );

    let mut unchanged = DurableFileSet::new(&journal).unwrap();
    assert!(!unchanged
        .stage_write_mode(&target, b"executable", 0o755)
        .unwrap());
    unchanged.commit().unwrap();

    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut mode_only = DurableFileSet::new(&journal).unwrap();
    assert!(mode_only
        .stage_write_mode(&target, b"executable", 0o755)
        .unwrap());
    mode_only.commit().unwrap();
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o755
    );

    let mut invalid = DurableFileSet::new(&journal).unwrap();
    assert!(invalid
        .stage_write_mode(&target, b"changed", 0o777)
        .is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"executable");
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn durable_file_set_mode_is_part_of_original_state_cas() {
    use std::os::unix::fs::PermissionsExt as _;

    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    let target = root.path().join("tool");
    std::fs::write(&target, b"before").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();

    let mut transaction = DurableFileSet::new(&journal).unwrap();
    transaction
        .stage_write_mode(&target, b"after", 0o755)
        .unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();

    let error = transaction.commit().unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::RecoveryIncomplete
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"before");
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    assert!(journal.exists());
}

#[cfg(unix)]
#[test]
fn durable_file_set_accepts_exactly_one_commit_marker() {
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    let sidecar = root.path().join("z-sidecar");
    let marker = root.path().join("a-marker");
    std::fs::write(&marker, b"old marker").unwrap();

    let mut transaction = DurableFileSet::new(&journal).unwrap();
    transaction.stage_write(&sidecar, b"sidecar").unwrap();
    transaction
        .stage_commit_marker(&marker, b"new marker")
        .unwrap();
    assert!(transaction
        .stage_commit_marker(&root.path().join("other-marker"), b"other")
        .is_err());
    transaction.commit().unwrap();

    assert_eq!(std::fs::read(sidecar).unwrap(), b"sidecar");
    assert_eq!(std::fs::read(marker).unwrap(), b"new marker");
    assert!(!journal.exists());
}

#[cfg(unix)]
#[test]
fn commit_marker_cannot_also_be_an_ordinary_change() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("marker");
    let mut transaction = DurableFileSet::new(root.path().join("journal")).unwrap();
    transaction.stage_write(&marker, b"ordinary").unwrap();

    assert!(transaction.stage_commit_marker(&marker, b"marker").is_err());
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn replacement_cas_rejects_in_place_content_changes() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    std::fs::write(&target, b"expected").unwrap();
    let (identity, expected) = measure_regular_file(&target).unwrap().unwrap();
    let expected = sha256_bytes(&expected);

    // A truncate-and-write keeps the inode on ordinary filesystems. The
    // digest half of the CAS must still detect it.
    std::fs::write(&target, b"raced").unwrap();
    let result = publish_atomic_file(
        &target,
        b"replacement",
        AtomicFilePolicy::ReplaceIf {
            identity,
            sha256: expected,
        },
    );

    assert!(result.is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"raced");
}

#[cfg(unix)]
#[test]
fn casefold_collisions_fail_before_commit() {
    let root = tempfile::tempdir().unwrap();
    let mut transaction = DurableFileSet::new(root.path().join("journal")).unwrap();
    transaction
        .stage_write(&root.path().join("Foo.h"), b"one")
        .unwrap();
    assert!(transaction
        .stage_write(&root.path().join("foo.h"), b"two")
        .is_err());
    assert!(!root.path().join("Foo.h").exists());
}

#[cfg(unix)]
#[test]
fn no_clobber_file_set_rejects_existing_and_rolls_back_mid_apply() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("install.journal");
    let existing = root.path().join("existing");
    std::fs::write(&existing, b"owned").unwrap();
    let mut conflict = DurableFileSet::new(&journal).unwrap();
    assert_eq!(
        conflict
            .stage_create_mode(&existing, b"replacement", 0o755)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&existing).unwrap(), b"owned");
    drop(conflict);

    let first = root.path().join("first");
    let second = root.path().join("second");
    let mut transaction = DurableFileSet::new(&journal).unwrap();
    transaction
        .stage_create_mode(&first, b"one", 0o755)
        .unwrap();
    transaction
        .stage_create_mode(&second, b"two", 0o755)
        .unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_FAIL_AT",
        scoped_test_point("after-apply-0"),
    );
    let result = transaction.commit();
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");
    assert!(result.is_err());
    assert!(!first.exists());
    assert!(!second.exists());
}

#[cfg(unix)]
#[test]
fn symlink_parent_is_rejected() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    let mut transaction = DurableFileSet::new(root.path().join("journal")).unwrap();
    assert!(transaction
        .stage_write(&root.path().join("escape/file"), b"blocked")
        .is_err());
    assert!(!outside.path().join("file").exists());
}

#[cfg(unix)]
#[test]
fn racing_no_clobber_publishers_have_exactly_one_winner() {
    use std::sync::{Arc, Barrier};

    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("winner");
    let barrier = Arc::new(Barrier::new(3));
    #[allow(clippy::needless_collect)]
    let workers: Vec<_> = [b"one".as_slice(), b"two".as_slice()]
        .into_iter()
        .map(|contents| {
            let target = target.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                publish_atomic_file(&target, contents, AtomicFilePolicy::NoClobber)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(matches!(
        std::fs::read(target).unwrap().as_slice(),
        b"one" | b"two"
    ));
}

#[cfg(unix)]
#[test]
fn flat_tree_is_complete_and_casefold_safe() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("tree");
    let exec = PortableOutputName::new("exec.library").unwrap();
    let dos = PortableOutputName::new("dos.library").unwrap();
    publish_flat_tree_noclobber(&destination, &[(exec, b"exec"), (dos, b"dos")]).unwrap();
    assert_eq!(
        std::fs::read(destination.join("exec.library")).unwrap(),
        b"exec"
    );
    assert_eq!(
        std::fs::read(destination.join("dos.library")).unwrap(),
        b"dos"
    );

    let collision = root.path().join("collision");
    assert!(publish_flat_tree_noclobber(
        &collision,
        &[
            (PortableOutputName::new("Foo").unwrap(), b"one"),
            (PortableOutputName::new("foo").unwrap(), b"two"),
        ],
    )
    .is_err());
    assert!(!collision.exists());
}

#[cfg(unix)]
#[test]
fn flat_tree_post_rename_failure_preserves_complete_destination() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("tree");
    std::env::set_var(
        "AROS_PUBLICATION_TEST_FAIL_AT",
        scoped_test_point("tree-after-rename-before-sync"),
    );
    let result = publish_flat_tree_noclobber(
        &destination,
        &[
            (PortableOutputName::new("one").unwrap(), b"first"),
            (PortableOutputName::new("two").unwrap(), b"second"),
        ],
    );
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::CommitStateUncertain
    );
    assert!(!is_rollback_incomplete(&error));
    assert_eq!(std::fs::read(destination.join("one")).unwrap(), b"first");
    assert_eq!(std::fs::read(destination.join("two")).unwrap(), b"second");

    // A retry performs only marker cleanup and then reports the existing
    // complete no-clobber destination. It never removes published bytes.
    let retry = publish_flat_tree_noclobber(
        &destination,
        &[
            (PortableOutputName::new("one").unwrap(), b"first"),
            (PortableOutputName::new("two").unwrap(), b"second"),
        ],
    );
    assert_eq!(retry.unwrap_err().kind(), ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(destination.join("one")).unwrap(), b"first");
}

#[cfg(unix)]
#[test]
fn path_scoped_publication_failure_targets_only_the_nominated_file() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("blocked");
    let permitted = root.path().join("permitted");
    std::env::set_var("AROS_PUBLICATION_TEST_FAIL_PATH", &blocked);

    assert!(publish_atomic_file(&blocked, b"blocked", AtomicFilePolicy::NoClobber).is_err());
    assert!(!blocked.exists());
    publish_atomic_file(&permitted, b"permitted", AtomicFilePolicy::NoClobber).unwrap();
    assert_eq!(std::fs::read(permitted).unwrap(), b"permitted");
}

#[cfg(unix)]
#[test]
fn file_post_rename_failure_preserves_target_and_committed_recovery_marker() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("output");
    std::env::set_var(
        "AROS_PUBLICATION_TEST_FAIL_AT",
        scoped_test_point("file-after-rename-before-sync"),
    );
    let result = publish_atomic_file(&target, b"complete", AtomicFilePolicy::NoClobber);
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::CommitStateUncertain
    );
    assert!(!is_rollback_incomplete(&error));
    assert_eq!(std::fs::read(&target).unwrap(), b"complete");
    assert!(std::fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".aros-file-")
    }));

    let retry = publish_atomic_file(&target, b"complete", AtomicFilePolicy::NoClobber);
    assert_eq!(retry.unwrap_err().kind(), ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&target).unwrap(), b"complete");
    assert!(!std::fs::read_dir(root.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".aros-file-")
    }));
}

#[cfg(unix)]
#[test]
fn flat_tree_recovery_refuses_unowned_deterministic_stage() {
    use std::os::unix::ffi::OsStrExt as _;

    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("tree");
    let digest = sha256_bytes(destination.file_name().unwrap().as_bytes()).to_string();
    let stage = root
        .path()
        .join(format!(".aros-tree-stage-{}", &digest[..32]));
    std::fs::create_dir(&stage).unwrap();
    std::fs::write(stage.join("sentinel"), b"not ours").unwrap();

    let error = publish_flat_tree_noclobber(
        &destination,
        &[(PortableOutputName::new("one").unwrap(), b"first")],
    )
    .unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::RecoveryIncomplete
    );
    assert_eq!(std::fs::read(stage.join("sentinel")).unwrap(), b"not ours");
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn flat_tree_recovers_a_crash_during_owned_marker_write() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("tree");
    let digest = sha256_bytes(
        destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_bytes(),
    )
    .to_string();
    let stage = root
        .path()
        .join(format!(".aros-tree-stage-{}", &digest[..32]));
    std::fs::create_dir(&stage).unwrap();
    // A process can die after O_EXCL creation but before the complete
    // owner record is written. The schema prefix distinguishes that
    // owned partial record from arbitrary user data.
    std::fs::write(stage.join("owner.json"), b"AROS-FLAT-TREE-STAGE-").unwrap();

    let receipt = publish_flat_tree_noclobber(
        &destination,
        &[(PortableOutputName::new("one").unwrap(), b"first")],
    )
    .unwrap();
    assert_eq!(receipt.recovery(), RecoveryOutcome::RemovedTreeStage);
    assert_eq!(std::fs::read(destination.join("one")).unwrap(), b"first");
    assert!(!stage.exists());
}

#[cfg(unix)]
#[test]
fn prepared_tree_publishes_nested_files_and_symlinks_without_following() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("published");
    std::fs::create_dir_all(staging.join("nested")).unwrap();
    std::fs::write(staging.join("nested/file"), b"payload").unwrap();
    symlink("nested/file", staging.join("link")).unwrap();

    publish_prepared_tree_noclobber(&staging, &destination).unwrap();
    assert!(!staging.exists());
    assert_eq!(
        std::fs::read(destination.join("nested/file")).unwrap(),
        b"payload"
    );
    assert_eq!(
        std::fs::read_link(destination.join("link")).unwrap(),
        Path::new("nested/file")
    );
}

#[cfg(unix)]
#[test]
fn prepared_source_tree_preserves_non_ascii_source_names_without_weakening_outputs() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("published");
    std::fs::create_dir_all(staging.join("ANIM-5")).unwrap();
    std::fs::write(staging.join("ANIM-5/Ara±a.anim"), b"tracked source").unwrap();

    let generated_error = publish_prepared_tree_noclobber(&staging, &destination).unwrap_err();
    assert_eq!(
        publication_failure_class(&generated_error),
        PublicationFailureClass::UnsafeTarget
    );
    assert!(staging.exists());
    assert!(!destination.exists());

    publish_prepared_source_tree_noclobber(&staging, &destination).unwrap();
    assert_eq!(
        std::fs::read(destination.join("ANIM-5/Ara±a.anim")).unwrap(),
        b"tracked source"
    );
}

#[cfg(unix)]
#[test]
fn prepared_source_tree_rejects_unsafe_separator_names() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("published");
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("unsafe\\name"), b"tracked source").unwrap();

    let error = publish_prepared_source_tree_noclobber(&staging, &destination).unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::UnsafeTarget
    );
    assert!(staging.exists());
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn prepared_source_tree_rejects_casefold_collisions() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("published");
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("Source"), b"one").unwrap();
    std::fs::write(staging.join("source"), b"two").unwrap();
    if std::fs::read_dir(&staging).unwrap().count() < 2 {
        return;
    }

    let error = publish_prepared_source_tree_noclobber(&staging, &destination).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);
    assert!(staging.exists());
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn prepared_source_tree_storage_full_before_rename_retains_staging() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let staging = root_path.join("staging");
    let destination = root_path.join("published");
    std::fs::create_dir_all(staging.join("source dir")).unwrap();
    std::fs::write(
        staging.join("source dir/source file ±.txt"),
        b"source payload",
    )
    .unwrap();
    let expected_staging = staging.clone();

    let result = with_fault_point(
        "prepared-tree-after-sync-before-rename",
        move |path| path == expected_staging,
        std::io::Error::new(
            ErrorKind::StorageFull,
            "injected storage-full staging failure",
        ),
        || publish_prepared_source_tree_noclobber(&staging, &destination),
    );

    let error = result.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::StorageFull);
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::Io
    );
    assert!(staging.exists());
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read(staging.join("source dir/source file ±.txt")).unwrap(),
        b"source payload"
    );
}

#[cfg(unix)]
#[test]
fn prepared_source_tree_storage_full_after_rename_retains_complete_destination() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let staging = root_path.join("staging");
    let destination = root_path.join("published");
    std::fs::create_dir_all(staging.join("source dir")).unwrap();
    std::fs::write(
        staging.join("source dir/source file ±.txt"),
        b"source payload",
    )
    .unwrap();
    let expected_destination = destination.clone();

    let result = with_fault_point(
        "prepared-tree-after-rename-before-sync",
        move |path| path == expected_destination,
        std::io::Error::new(
            ErrorKind::StorageFull,
            "injected storage-full parent sync failure",
        ),
        || publish_prepared_source_tree_noclobber(&staging, &destination),
    );

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::CommitStateUncertain
    );
    assert!(!is_rollback_incomplete(&error));
    assert!(!staging.exists());
    assert_eq!(
        std::fs::read(destination.join("source dir/source file ±.txt")).unwrap(),
        b"source payload"
    );
}

#[cfg(unix)]
#[test]
fn prepared_tree_requires_same_parent_and_no_clobber() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let staging = first.path().join("staging");
    std::fs::create_dir(&staging).unwrap();
    let error =
        publish_prepared_tree_noclobber(&staging, &second.path().join("published")).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    assert!(staging.exists());

    let destination = first.path().join("published");
    std::fs::create_dir(&destination).unwrap();
    let error = publish_prepared_tree_noclobber(&staging, &destination).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);
    assert!(staging.exists());
}

#[cfg(unix)]
#[test]
fn prepared_tree_exchange_is_atomic_and_retains_the_previous_tree() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("published");
    std::fs::create_dir(&staging).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(staging.join("new"), b"new payload").unwrap();
    std::fs::write(destination.join("old"), b"old payload").unwrap();

    exchange_prepared_tree(&staging, &destination).unwrap();
    assert_eq!(
        std::fs::read(destination.join("new")).unwrap(),
        b"new payload"
    );
    assert_eq!(std::fs::read(staging.join("old")).unwrap(), b"old payload");
    assert!(!destination.join("old").exists());
}

#[cfg(unix)]
#[test]
fn prepared_tree_exchange_post_commit_failure_is_typed_and_retains_both_trees() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("published");
    std::fs::create_dir(&staging).unwrap();
    std::fs::create_dir(&destination).unwrap();
    std::fs::write(staging.join("new"), b"new payload").unwrap();
    std::fs::write(destination.join("old"), b"old payload").unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_FAIL_AT",
        scoped_test_point("prepared-tree-after-exchange-before-sync"),
    );
    let result = exchange_prepared_tree(&staging, &destination);
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::CommitStateUncertain
    );
    assert_eq!(
        std::fs::read(destination.join("new")).unwrap(),
        b"new payload"
    );
    assert_eq!(std::fs::read(staging.join("old")).unwrap(), b"old payload");
}

#[cfg(unix)]
#[test]
fn prepared_tree_rejects_nested_casefold_collisions_before_rename() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("published");
    std::fs::create_dir_all(staging.join("nested")).unwrap();
    std::fs::write(staging.join("nested/Foo.h"), b"one").unwrap();
    std::fs::write(staging.join("nested/foo.h"), b"two").unwrap();
    if std::fs::read_dir(staging.join("nested")).unwrap().count() < 2 {
        // The host volume itself already collapses the aliases, so it
        // cannot represent the collision this cross-platform guard is
        // intended to reject.
        return;
    }

    let error = publish_prepared_tree_noclobber(&staging, &destination).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);
    assert!(staging.exists());
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn prepared_tree_post_rename_failure_reports_uncertain_and_preserves_tree() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join("staging");
    let destination = root.path().join("published");
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(staging.join("file"), b"payload").unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_FAIL_AT",
        scoped_test_point("prepared-tree-after-rename-before-sync"),
    );
    let result = publish_prepared_tree_noclobber(&staging, &destination);
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::CommitStateUncertain
    );
    assert!(!staging.exists());
    assert_eq!(std::fs::read(destination.join("file")).unwrap(), b"payload");
}

#[cfg(unix)]
#[test]
fn committed_journal_cleanup_failure_is_recovered_observably() {
    use std::os::unix::fs::PermissionsExt as _;

    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    let target = root.path().join("target");
    let mut transaction = DurableFileSet::new(&journal).unwrap();
    transaction
        .stage_write_mode(&target, b"published", 0o755)
        .unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_FAIL_AT",
        scoped_test_point("journal-remove"),
    );
    let result = transaction.commit();
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::RecoveryIncomplete
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"published");
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o755
    );
    assert!(journal.exists());
    assert!(std::fs::read_to_string(&journal)
        .unwrap()
        .contains("\"desired_mode\":493"));

    let recovered = DurableFileSet::new(&journal).unwrap();
    assert_eq!(
        recovered.recovery_outcome(),
        RecoveryOutcome::CompletedCleanup
    );
    assert!(!journal.exists());
    assert_eq!(std::fs::read(&target).unwrap(), b"published");
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o7777,
        0o755
    );
}

#[cfg(unix)]
#[test]
fn committed_marker_prewrite_failure_rolls_back_complete_set() {
    use std::os::unix::fs::PermissionsExt as _;

    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::write(&first, b"before").unwrap();
    std::fs::set_permissions(&first, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut transaction = DurableFileSet::new(&journal).unwrap();
    transaction
        .stage_write_mode(&first, b"after", 0o755)
        .unwrap();
    transaction.stage_write(&second, b"new").unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_FAIL_AT",
        scoped_test_point("before-committed-journal"),
    );
    let result = transaction.commit();
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");

    assert!(result.is_err());
    assert_eq!(std::fs::read(&first).unwrap(), b"before");
    assert_eq!(
        std::fs::metadata(&first).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    assert!(!second.exists());
    assert!(!journal.exists());
}

#[cfg(unix)]
#[test]
fn stable_reader_rejects_in_place_concurrent_write() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    std::fs::write(&target, vec![b'a'; 1024 * 1024]).unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_PAUSE_AT",
        scoped_test_point("read-before-final-stat"),
    );
    std::env::set_var("AROS_PUBLICATION_TEST_PAUSE_MS", "300");
    let raced = target.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(raced, vec![b'b'; 1024 * 1024]).unwrap();
    });
    let result = measure_regular_file(&target);
    writer.join().unwrap();
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");
    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn staged_digest_race_never_publishes_or_deletes_the_changed_stage() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    let target = root.path().join("target");
    let mut transaction = DurableFileSet::new(&journal).unwrap();
    transaction.stage_write(&target, b"intended").unwrap();
    let root_path = root.path().to_path_buf();
    // Seeing staged bytes does not prove their readback and journal update
    // completed. Inject only after both, without sleeps or a scheduled racer.
    let result = at_boundary(
        "before-apply",
        |_| true,
        move |_| {
            let stage = std::fs::read_dir(root_path)
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| {
                    entry.file_name().to_string_lossy().contains(".aros-stage-")
                        && std::fs::read(entry.path()).is_ok_and(|contents| contents == b"intended")
                })
                .expect("verified operation stage must exist");
            std::fs::write(stage.path(), b"raced-stage").unwrap();
        },
        || transaction.commit(),
    );

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::RecoveryIncomplete
    );
    assert!(!target.exists());
    let retained = std::fs::read_dir(root.path())
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().contains(".aros-stage-"))
        .expect("digest-mismatched stage must be retained for inspection");
    assert_eq!(std::fs::read(retained.path()).unwrap(), b"raced-stage");
    assert!(journal.exists());
}

#[cfg(unix)]
#[test]
fn unverified_stage_readback_race_has_the_distinct_io_failure_contract() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let journal = root.path().join("journal");
    let target = root.path().join("target");
    let mut transaction = DurableFileSet::new(&journal).unwrap();
    transaction.stage_write(&target, b"intended").unwrap();
    // A slow initial stage fsync can let a timer-driven test land here,
    // before stage ownership had been verified and recorded in the journal.
    let error = at_boundary(
        "stage-before-readback",
        |path| std::fs::read(path).is_ok_and(|bytes| bytes == b"intended"),
        |path| std::fs::write(path, b"raced-stage").unwrap(),
        || transaction.commit(),
    )
    .unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::Io
    );
    assert!(error
        .to_string()
        .contains("failed identity/digest/mode readback"));
    assert!(!target.exists());
    assert!(!journal.exists());
    assert!(std::fs::read_dir(root.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".aros-stage-")
    }));
}

#[cfg(unix)]
#[test]
fn prepared_tree_content_cas_rejects_concurrent_in_place_mutation() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    let staging = root.path().join("staging");
    std::fs::create_dir(&destination).unwrap();
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(destination.join("value"), b"before").unwrap();
    std::fs::write(staging.join("value"), b"intended").unwrap();
    let expected = measure_tree_content_cas(&destination).unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_PAUSE_AT",
        scoped_test_point("prepared-tree-after-content-cas-before-exchange"),
    );
    std::env::set_var("AROS_PUBLICATION_TEST_PAUSE_MS", "300");
    let raced = destination.join("value");
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(raced, b"concurrent").unwrap();
    });
    let result = exchange_prepared_tree_if_unchanged(&staging, &destination, &expected);
    writer.join().unwrap();
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(destination.join("value")).unwrap(),
        b"concurrent"
    );
    assert_eq!(std::fs::read(staging.join("value")).unwrap(), b"intended");
}

#[cfg(unix)]
#[test]
fn prepared_tree_staging_content_cas_rejects_mutation_before_sync() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    let staging = root.path().join("staging");
    std::fs::create_dir(&destination).unwrap();
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(destination.join("value"), b"before").unwrap();
    std::fs::write(staging.join("value"), b"intended").unwrap();
    let expected = measure_tree_content_cas(&destination).unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_PAUSE_AT",
        scoped_test_point("prepared-tree-after-stage-content-cas-before-sync"),
    );
    std::env::set_var("AROS_PUBLICATION_TEST_PAUSE_MS", "300");
    let raced = staging.join("value");
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(raced, b"raced-stage").unwrap();
    });
    let result = exchange_prepared_tree_if_unchanged(&staging, &destination, &expected);
    writer.join().unwrap();
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");
    assert!(result.is_err());
    assert_eq!(std::fs::read(destination.join("value")).unwrap(), b"before");
    assert_eq!(
        std::fs::read(staging.join("value")).unwrap(),
        b"raced-stage"
    );
}

#[cfg(unix)]
#[test]
fn prepared_tree_post_exchange_content_race_is_compensated() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    let staging = root.path().join("staging");
    std::fs::create_dir(&destination).unwrap();
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(destination.join("value"), b"before").unwrap();
    std::fs::write(staging.join("value"), b"intended").unwrap();
    let expected = measure_tree_content_cas(&destination).unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_PAUSE_AT",
        scoped_test_point("prepared-tree-after-exchange-before-content-cas"),
    );
    std::env::set_var("AROS_PUBLICATION_TEST_PAUSE_MS", "1000");
    let raced = destination.join("value");
    let writer = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if std::fs::read(&raced).is_ok_and(|contents| contents == b"intended") {
                std::fs::write(&raced, b"raced-installed-tree").unwrap();
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "atomic exchange never exposed the prepared tree"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });
    let result = exchange_prepared_tree_if_unchanged(&staging, &destination, &expected);
    writer.join().unwrap();
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");
    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::Io
    );
    assert_eq!(std::fs::read(destination.join("value")).unwrap(), b"before");
    assert_eq!(
        std::fs::read(staging.join("value")).unwrap(),
        b"raced-installed-tree"
    );
}

#[test]
fn uncertain_incomplete_rollback_has_typed_class_flag_and_recovery_guidance() {
    let error = io_failure(
        PublicationError::commit_state_uncertain_with_incomplete_rollback(
            "compensating exchange cannot be proven",
        ),
    );
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::CommitStateUncertain
    );
    assert!(is_rollback_incomplete(&error));
    assert!(error
        .to_string()
        .contains("inspect the retained paths before retrying"));
}

#[cfg(unix)]
#[test]
fn prepared_tree_foreign_staging_binding_never_triggers_compensating_exchange() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    let staging = root.path().join("staging");
    let saved = root.path().join("staging.saved");
    std::fs::create_dir(&destination).unwrap();
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(destination.join("value"), b"before").unwrap();
    std::fs::write(staging.join("value"), b"intended").unwrap();
    let expected = measure_tree_content_cas(&destination).unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_PAUSE_AT",
        scoped_test_point("prepared-tree-after-exchange-before-content-cas"),
    );
    std::env::set_var("AROS_PUBLICATION_TEST_PAUSE_MS", "400");
    let raced_destination = destination.clone();
    let raced_staging = staging.clone();
    let writer = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let installed_tree_is_visible = std::fs::read(raced_destination.join("value"))
                .is_ok_and(|contents| contents == b"intended");
            let previous_tree_is_staged = std::fs::read(raced_staging.join("value"))
                .is_ok_and(|contents| contents == b"before");
            if installed_tree_is_visible && previous_tree_is_staged {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "atomic exchange never exposed both exchanged trees"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::fs::rename(&raced_staging, &saved).unwrap();
        std::fs::create_dir(&raced_staging).unwrap();
        std::fs::write(raced_staging.join("value"), b"foreign").unwrap();
        std::fs::write(raced_destination.join("value"), b"raced-installed-tree").unwrap();
    });
    let result = exchange_prepared_tree_if_unchanged(&staging, &destination, &expected);
    writer.join().unwrap();
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::CommitStateUncertain
    );
    assert!(is_rollback_incomplete(&error));
    assert!(!error.to_string().contains("restored"));
    assert_eq!(
        std::fs::read(destination.join("value")).unwrap(),
        b"raced-installed-tree"
    );
    assert_eq!(std::fs::read(staging.join("value")).unwrap(), b"foreign");
    assert_eq!(
        std::fs::read(root.path().join("staging.saved/value")).unwrap(),
        b"before"
    );
}

#[cfg(unix)]
#[test]
fn prepared_tree_compensation_sync_failure_is_uncertain_and_incomplete() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("destination");
    let staging = root.path().join("staging");
    std::fs::create_dir(&destination).unwrap();
    std::fs::create_dir(&staging).unwrap();
    std::fs::write(destination.join("value"), b"before").unwrap();
    std::fs::write(staging.join("value"), b"intended").unwrap();
    let expected = measure_tree_content_cas(&destination).unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_PAUSE_AT",
        scoped_test_point("prepared-tree-after-exchange-before-content-cas"),
    );
    std::env::set_var("AROS_PUBLICATION_TEST_PAUSE_MS", "300");
    std::env::set_var(
        "AROS_PUBLICATION_TEST_FAIL_AT",
        scoped_test_point("prepared-tree-after-compensating-exchange-before-sync"),
    );
    let raced = destination.join("value");
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(raced, b"raced-installed-tree").unwrap();
    });
    let result = exchange_prepared_tree_if_unchanged(&staging, &destination, &expected);
    writer.join().unwrap();
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");
    std::env::remove_var("AROS_PUBLICATION_TEST_FAIL_AT");

    let error = result.unwrap_err();
    assert_eq!(
        publication_failure_class(&error),
        PublicationFailureClass::CommitStateUncertain
    );
    assert!(is_rollback_incomplete(&error));
    assert!(!error.to_string().contains("restored"));
}

#[cfg(unix)]
#[test]
fn tree_content_cas_double_pass_rejects_early_entry_race() {
    let _environment = lock_fault_environment();
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("a"), b"before").unwrap();
    std::fs::write(tree.join("z"), vec![b'z'; 16 * 1024 * 1024]).unwrap();
    std::env::set_var(
        "AROS_PUBLICATION_TEST_PAUSE_AT",
        scoped_test_point("tree-content-cas-between-passes"),
    );
    std::env::set_var("AROS_PUBLICATION_TEST_PAUSE_MS", "300");
    let raced = tree.join("a");
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        std::fs::write(raced, b"after").unwrap();
    });
    let result = measure_tree_content_cas(&tree);
    writer.join().unwrap();
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_AT");
    std::env::remove_var("AROS_PUBLICATION_TEST_PAUSE_MS");
    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn bounded_snapshot_copy_preserves_a_stable_tree_without_following_links() {
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    let destination = temporary.path().join("destination");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(source.join("bin")).unwrap();
    std::fs::write(source.join("bin/aros-collect"), b"fixture collector").unwrap();
    let mut permissions = std::fs::metadata(source.join("bin/aros-collect"))
        .unwrap()
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(source.join("bin/aros-collect"), permissions).unwrap();
    symlink("aros-collect", source.join("bin/collect-aros")).unwrap();
    std::fs::create_dir(&destination).unwrap();

    let limits = TreeTraversalLimits::new(16, 1024).unwrap();
    let expected = measure_tree_content_cas_bounded(&source, limits).unwrap();
    let copied =
        copy_tree_from_snapshot_nofollow(&source, &destination, &expected, limits).unwrap();

    assert_eq!(
        expected.payload_digest_excluding(None),
        copied.payload_digest_excluding(None)
    );
    assert_eq!(
        std::fs::read(destination.join("bin/aros-collect")).unwrap(),
        b"fixture collector"
    );
    assert_eq!(
        std::fs::read_link(destination.join("bin/collect-aros")).unwrap(),
        Path::new("aros-collect")
    );
}

#[cfg(unix)]
#[test]
fn bounded_snapshot_refuses_a_tree_that_exceeds_its_entry_budget() {
    let temporary = tempfile::tempdir().unwrap();
    let tree = temporary.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("one"), b"one").unwrap();
    std::fs::write(tree.join("two"), b"two").unwrap();

    let error = measure_tree_content_cas_bounded(&tree, TreeTraversalLimits::new(1, 1024).unwrap())
        .unwrap_err();
    assert!(error.to_string().contains("entry traversal limit"));
}
