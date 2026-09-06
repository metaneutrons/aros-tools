//! Real filesystem tests. These do not authorize or run a compiler driver.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt as _};
use std::path::Path;

use aros_common::{measure_tree_content_cas, sha256_bytes, CancellationToken, DiagnosticCode};
use aros_toolchain::plan::{Backend, PlanRequest};
use aros_toolchain::workspace::RunDirectories;

const MARKER: &str = ".aros-toolchain-owner-v1.json";

struct Fixture {
    root: tempfile::TempDir,
    request: PlanRequest,
    token: CancellationToken,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        for name in ["source", "producer", "tools", "cache", "run parents"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        Self {
            request: PlanRequest {
                backend: Backend::LegacyPreview,
                preset: "fixture".into(),
                recipe: path.join("not-read-by-ownership"),
                source_dir: path.join("source"),
                producer_dir: path.join("producer"),
                tools_dir: path.join("tools"),
                work_dir: Some(path.join("run parents/Größe work")),
                output_dir: Some(path.join("run parents/output")),
                cache_dir: Some(path.join("cache")),
                jobs: Some(1),
                timeout_seconds: Some(30),
                offline: true,
            },
            root,
            token: CancellationToken::default(),
        }
    }

    fn reserve(&self) -> Result<RunDirectories, aros_toolchain::ContractError> {
        RunDirectories::reserve(
            &self.request,
            &sha256_bytes(b"synthetic operation binding"),
            &self.token,
        )
    }

    fn work(&self) -> &Path {
        self.request.work_dir.as_deref().unwrap()
    }
    fn output(&self) -> &Path {
        self.request.output_dir.as_deref().unwrap()
    }

    fn digest(&self) -> aros_common::Sha256Digest {
        measure_tree_content_cas(&self.root.path().canonicalize().unwrap())
            .unwrap()
            .payload_digest_excluding(None)
    }
}

#[test]
fn fresh_directories_are_private_locked_and_retained_on_drop() {
    let fixture = Fixture::new();
    let guard = fixture.reserve().unwrap();
    assert_eq!(guard.paths().work.as_deref(), Some(fixture.work()));
    guard.revalidate(&fixture.token).unwrap();
    for path in [fixture.work(), fixture.output()] {
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(
            fs::metadata(path.join(MARKER))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(path.join(MARKER)).unwrap()).unwrap();
        assert_eq!(
            record["owner_sha256"],
            sha256_bytes(b"synthetic operation binding").as_str()
        );
        let directory = fs::File::open(path).unwrap();
        assert_eq!(
            rustix::fs::flock(
                &directory,
                rustix::fs::FlockOperation::NonBlockingLockExclusive
            )
            .unwrap_err(),
            rustix::io::Errno::WOULDBLOCK,
        );
    }
    fs::write(fixture.work().join("partial evidence"), b"keep me").unwrap();
    let before = fixture.digest();
    drop(guard);
    assert_eq!(fixture.digest(), before);
    let directory = fs::File::open(fixture.work()).unwrap();
    rustix::fs::flock(
        &directory,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    )
    .unwrap();
    assert!(
        fixture.reserve().is_err(),
        "released lock never permits implicit resume"
    );
}

#[test]
fn existing_work_or_output_is_never_adopted_even_when_empty() {
    for existing_output in [false, true] {
        for with_content in [false, true] {
            let fixture = Fixture::new();
            let path = if existing_output {
                fixture.output()
            } else {
                fixture.work()
            };
            fs::create_dir(path).unwrap();
            if with_content {
                fs::write(path.join("foreign"), b"user data").unwrap();
            }
            let before = fixture.digest();
            let error = fixture.reserve().unwrap_err();
            assert_eq!(
                error.diagnostics().diagnostics[0].code,
                DiagnosticCode::ProducerState
            );
            assert_eq!(before, fixture.digest());
        }
    }
}

#[test]
fn explicit_release_unlocks_both_original_roots_after_cancellation_and_rename() {
    let fixture = Fixture::new();
    let guard = fixture.reserve().unwrap();
    fs::write(fixture.work().join("retained"), b"partial evidence").unwrap();
    let moved = fixture.root.path().join("moved work");
    fs::rename(fixture.work(), &moved).unwrap();
    fs::create_dir(fixture.work()).unwrap();
    fs::write(fixture.work().join("foreign"), b"keep").unwrap();
    fixture.token.cancel();
    let before = fixture.digest();
    guard.release().unwrap();
    for path in [moved.as_path(), fixture.output()] {
        let observer = fs::File::open(path).unwrap();
        rustix::fs::flock(
            &observer,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        )
        .unwrap();
    }
    assert_eq!(fixture.digest(), before);
}

#[test]
fn missing_parent_is_not_implicitly_created() {
    let mut fixture = Fixture::new();
    fixture.request.output_dir = Some(fixture.root.path().join("missing/parent/output"));
    let before = fixture.digest();
    assert!(fixture.reserve().is_err());
    assert_eq!(before, fixture.digest());
}

#[test]
fn overlap_is_rejected_before_mutation() {
    for target in [
        "source/build",
        "cache/output",
        "run parents/Größe work/nested",
    ] {
        let mut fixture = Fixture::new();
        fixture.request.output_dir = Some(fixture.root.path().join(target));
        let before = fixture.digest();
        assert_eq!(
            fixture.reserve().unwrap_err().diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerPreflight
        );
        assert_eq!(before, fixture.digest());
    }
}

#[test]
fn native_missing_budgets_and_cancellation_fail_before_mutation() {
    for case in 0..4 {
        let mut fixture = Fixture::new();
        match case {
            0 => {
                fixture.request.backend = Backend::Native;
                fixture.request.source_dir = "/absent".into();
            }
            1 => fixture.request.jobs = Some(0),
            2 => fixture.request.timeout_seconds = None,
            _ => fixture.token.cancel(),
        }
        let before = fixture.digest();
        assert!(fixture.reserve().is_err());
        assert_eq!(before, fixture.digest());
    }
}

#[test]
fn explicit_parent_symlink_is_canonicalized_without_adopting_a_link_leaf() {
    let mut fixture = Fixture::new();
    let parent = fixture.work().parent().unwrap().to_owned();
    let alias = fixture.root.path().join("alias");
    symlink(&parent, &alias).unwrap();
    fixture.request.work_dir = Some(alias.join("Größe work"));
    let guard = fixture.reserve().unwrap();
    assert_eq!(guard.paths().work, Some(parent.join("Größe work")));
    guard.revalidate(&fixture.token).unwrap();
}

#[test]
fn symlink_leaf_never_adopts_a_foreign_directory() {
    let fixture = Fixture::new();
    let foreign = fixture.root.path().join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("keep"), b"user data").unwrap();
    symlink(&foreign, fixture.output()).unwrap();
    let before = fixture.digest();
    assert!(fixture.reserve().is_err());
    assert_eq!(before, fixture.digest());
}

#[test]
fn moved_parent_and_replaced_root_fail_without_cleaning_either_tree() {
    for replace_parent in [false, true] {
        let fixture = Fixture::new();
        let guard = fixture.reserve().unwrap();
        let original = if replace_parent {
            fixture.work().parent().unwrap()
        } else {
            fixture.work()
        };
        let retained = fixture.root.path().join("retained");
        fs::rename(original, &retained).unwrap();
        fs::create_dir(original).unwrap();
        fs::write(original.join("foreign"), b"keep").unwrap();
        let before = fixture.digest();
        assert!(guard.revalidate(&fixture.token).is_err());
        drop(guard);
        assert_eq!(before, fixture.digest());
    }
}

#[test]
fn symlink_replacement_is_not_followed_during_revalidation() {
    let fixture = Fixture::new();
    let guard = fixture.reserve().unwrap();
    let moved = fixture.root.path().join("moved");
    fs::rename(fixture.work(), &moved).unwrap();
    symlink(&moved, fixture.work()).unwrap();
    let before = fixture.digest();
    assert!(guard.revalidate(&fixture.token).is_err());
    drop(guard);
    assert_eq!(before, fixture.digest());
}

#[test]
fn changed_replaced_and_hardlinked_markers_are_rejected() {
    for change in 0..3 {
        let fixture = Fixture::new();
        let guard = fixture.reserve().unwrap();
        let marker = fixture.work().join(MARKER);
        match change {
            0 => fs::write(&marker, b"different binding").unwrap(),
            1 => {
                let bytes = fs::read(&marker).unwrap();
                fs::rename(&marker, fixture.root.path().join("old marker")).unwrap();
                fs::write(&marker, bytes).unwrap();
            }
            _ => fs::hard_link(&marker, fixture.root.path().join("extra link")).unwrap(),
        }
        let before = fixture.digest();
        assert_eq!(
            guard
                .revalidate(&fixture.token)
                .unwrap_err()
                .diagnostics()
                .diagnostics[0]
                .code,
            DiagnosticCode::ProducerState
        );
        drop(guard);
        assert_eq!(before, fixture.digest());
    }
}

#[test]
fn fifo_marker_fails_without_blocking() {
    let fixture = Fixture::new();
    let guard = fixture.reserve().unwrap();
    let marker = fixture.work().join(MARKER);
    fs::rename(&marker, fixture.root.path().join("old marker")).unwrap();
    assert!(std::process::Command::new("mkfifo")
        .arg(&marker)
        .status()
        .unwrap()
        .success());
    let start = std::time::Instant::now();
    assert!(guard.revalidate(&fixture.token).is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

#[test]
fn cancelled_reservation_retains_evidence_and_rejects_next_boundary() {
    let fixture = Fixture::new();
    let guard = fixture.reserve().unwrap();
    fs::write(fixture.output().join("partial output"), b"not published").unwrap();
    fixture.token.cancel();
    let before = fixture.digest();
    assert!(guard.revalidate(&fixture.token).is_err());
    drop(guard);
    assert_eq!(before, fixture.digest());
}

#[test]
fn relaxing_root_or_marker_permissions_invalidates_ownership() {
    for marker in [false, true] {
        let fixture = Fixture::new();
        let guard = fixture.reserve().unwrap();
        let path = if marker {
            fixture.work().join(MARKER)
        } else {
            fixture.work().to_owned()
        };
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if marker { 0o644 } else { 0o755 }),
        )
        .unwrap();
        assert!(guard.revalidate(&fixture.token).is_err());
    }
}

#[test]
fn ownership_is_exclusive_across_exec_and_released_when_guard_drops() {
    let fixture = Fixture::new();
    let guard = fixture.reserve().unwrap();
    let probe = |expected_busy: bool| {
        let result = aros_common::run_output_with_timeout(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "probe_directory_lock", "--nocapture"])
                .env("AROS_GUARD_TEST_PATH", fixture.work())
                .env("AROS_GUARD_TEST_BUSY", expected_busy.to_string()),
            4096,
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            aros_common::bounded_output_detail(&result.stdout, &result.stderr)
        );
    };
    probe(true);
    drop(guard);
    probe(false);
}

#[test]
fn probe_directory_lock() {
    let Some(path) = std::env::var_os("AROS_GUARD_TEST_PATH") else {
        return;
    };
    let directory = fs::File::open(path).unwrap();
    let result = rustix::fs::flock(
        &directory,
        rustix::fs::FlockOperation::NonBlockingLockExclusive,
    );
    if std::env::var("AROS_GUARD_TEST_BUSY").unwrap() == "true" {
        assert_eq!(result.unwrap_err(), rustix::io::Errno::WOULDBLOCK);
    } else {
        result.unwrap();
    }
}

#[test]
fn concurrent_same_root_reservation_has_exactly_one_owner() {
    let fixture = Fixture::new();
    std::thread::scope(|scope| {
        let first = scope.spawn(|| fixture.reserve());
        let second = scope.spawn(|| fixture.reserve());
        let results = [first.join().unwrap(), second.join().unwrap()];
        assert_eq!(results.iter().filter(|value| value.is_ok()).count(), 1);
        for guard in results.iter().filter_map(|value| value.as_ref().ok()) {
            guard.revalidate(&fixture.token).unwrap();
        }
    });
}
