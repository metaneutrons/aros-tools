use super::PrivateNativeCaptureDirectory;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

fn private_parent() -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("create private test directory");
    let parent = temporary
        .path()
        .canonicalize()
        .expect("canonicalize private test directory");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))
        .expect("set exact private directory mode");
    (temporary, parent)
}

fn private_file(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, bytes).expect("create test file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("set exact private file mode");
}

#[test]
fn concurrent_call_claims_allocate_unique_bounded_slots() {
    let (_temporary, parent) = private_parent();
    let start = std::sync::Arc::new(std::sync::Barrier::new(8));
    let workers: [_; 8] = std::array::from_fn(|_| {
        let parent = parent.clone();
        let start = start.clone();
        std::thread::spawn(move || {
            start.wait();
            PrivateNativeCaptureDirectory::open(&parent)
                .expect("open private capture parent")
                .claim_observation_slot(8)
                .expect("claim one available call slot")
        })
    });

    let claimed = workers
        .into_iter()
        .map(|worker| worker.join().expect("claim worker completed"))
        .collect::<Vec<_>>();
    let slots = claimed
        .iter()
        .map(|(slot, _)| *slot)
        .collect::<BTreeSet<_>>();
    assert_eq!(slots, (0..8).collect());
    for (slot, directory) in &claimed {
        assert_eq!(directory.path(), parent.join(format!("call-{slot:04}")));
    }

    assert!(PrivateNativeCaptureDirectory::open(&parent)
        .expect("reopen private capture parent")
        .claim_observation_slot(8)
        .is_err());
}

#[test]
fn invalid_slot_bounds_fail_before_creating_entries() {
    let (_temporary, parent) = private_parent();
    let directory = PrivateNativeCaptureDirectory::open(&parent).expect("open parent");

    assert!(directory.claim_observation_slot(0).is_err());
    assert!(directory.claim_observation_slot(257).is_err());
    assert!(directory.claim_observation_slot_bounded(0).is_err());
    assert!(directory.claim_observation_slot_bounded(4097).is_err());
    assert_eq!(
        std::fs::read_dir(&parent)
            .expect("list private parent")
            .count(),
        0
    );
}

#[test]
fn bounded_claim_advances_past_the_legacy_slot_range() {
    let (_temporary, parent) = private_parent();
    for slot in 0..256 {
        private_file(&parent.join(format!("call-{slot:04}.claim")), b"");
    }

    let (slot, directory) = PrivateNativeCaptureDirectory::open(&parent)
        .expect("open parent")
        .claim_observation_slot_bounded(4096)
        .expect("claim next slot above legacy bound");
    assert_eq!(slot, 256);
    assert_eq!(directory.path(), parent.join("call-0256"));
}

fn assert_unsafe_claim_is_refused(kind: &str) {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let (_temporary, parent) = private_parent();
    let claim = parent.join("call-0000.claim");
    match kind {
        "symlink" => {
            let target = parent.join("claim-target");
            private_file(&target, b"");
            symlink(target, &claim).expect("create claim symlink");
        }
        "hardlink" => {
            private_file(&claim, b"");
            std::fs::hard_link(&claim, parent.join("claim-alias")).expect("create claim hard link");
        }
        "nonempty" => private_file(&claim, b"not-empty"),
        "nonprivate" => {
            private_file(&claim, b"");
            std::fs::set_permissions(&claim, std::fs::Permissions::from_mode(0o644))
                .expect("make claim nonprivate");
        }
        _ => unreachable!("listed test case"),
    }

    assert!(PrivateNativeCaptureDirectory::open(&parent)
        .expect("open parent")
        .claim_observation_slot(8)
        .is_err());
    assert!(!parent.join("call-0001.claim").exists());
    assert!(!parent.join("call-0001").exists());
}

#[test]
fn unsafe_existing_claims_refuse_without_advancing_to_another_slot() {
    for kind in ["symlink", "hardlink", "nonempty", "nonprivate"] {
        assert_unsafe_claim_is_refused(kind);
    }
}

#[test]
fn existing_child_without_claim_is_refused_and_new_claim_stays_consumed() {
    use std::os::unix::fs::PermissionsExt;

    let (_temporary, parent) = private_parent();
    let existing_child = parent.join("call-0000");
    std::fs::create_dir(&existing_child).expect("create unclaimed child");
    std::fs::set_permissions(&existing_child, std::fs::Permissions::from_mode(0o700))
        .expect("set exact child mode");
    let sentinel = existing_child.join("sentinel");
    private_file(&sentinel, b"preserve");

    assert!(PrivateNativeCaptureDirectory::open(&parent)
        .expect("open parent")
        .claim_observation_slot(2)
        .is_err());
    assert!(parent.join("call-0000.claim").is_file());
    assert_eq!(
        std::fs::read(&sentinel).expect("read existing child"),
        b"preserve"
    );

    let (slot, next) = PrivateNativeCaptureDirectory::open(&parent)
        .expect("reopen parent")
        .claim_observation_slot(2)
        .expect("skip consumed claim and allocate next slot");
    assert_eq!(slot, 1);
    assert_eq!(next.path(), parent.join("call-0001"));
    assert!(PrivateNativeCaptureDirectory::open(&parent)
        .expect("reopen parent after both slots are consumed")
        .claim_observation_slot(2)
        .is_err());
}

#[test]
fn replaced_parent_is_refused_without_publishing_into_either_namespace() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().expect("create private test directory");
    let root = temporary
        .path()
        .canonicalize()
        .expect("canonical test root");
    let parent = root.join("capture-parent");
    std::fs::create_dir(&parent).expect("create capture parent");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))
        .expect("set exact private directory mode");
    let held_path = root.join("held-parent");
    let directory = PrivateNativeCaptureDirectory::open(&parent).expect("open parent");

    std::fs::rename(&parent, &held_path).expect("rename held parent");
    std::fs::create_dir(&parent).expect("create replacement parent");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700))
        .expect("set exact replacement mode");

    assert!(directory.claim_observation_slot(8).is_err());
    assert!(!parent.join("call-0000.claim").exists());
    assert!(!held_path.join("call-0000.claim").exists());
}
