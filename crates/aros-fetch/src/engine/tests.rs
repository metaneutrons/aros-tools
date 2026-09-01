use super::budget::{MAX_ARCHIVE_ENTRY_BYTES, MAX_ARCHIVE_EXPANDED_BYTES};
use super::*;
use aros_common::DiagnosticCode;

#[cfg(unix)]
static FETCH_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn archive_paths_cannot_escape_the_staging_root() {
    assert!(validate_archive_path(Path::new("source/file.c"), "fixture.tar.gz").is_ok());
    let error = validate_archive_path(Path::new("../owned"), "fixture.tar.gz")
        .unwrap_err()
        .into_diagnostic();
    assert_eq!(error.code, DiagnosticCode::FetchExtraction);
}

#[test]
fn archive_links_cannot_escape_the_staging_root() {
    assert!(validate_link_target(
        Path::new("source/include/current"),
        Path::new("../public"),
        "fixture.tar.gz"
    )
    .is_ok());
    assert!(validate_link_target(
        Path::new("source/link"),
        Path::new("../../outside"),
        "fixture.tar.gz"
    )
    .is_err());
    assert!(validate_link_target(
        Path::new("source/link"),
        Path::new("/outside"),
        "fixture.tar.gz"
    )
    .is_err());
}

#[test]
fn origins_reject_ambiguous_paths_and_embedded_credentials() {
    assert!(expand_origin("cache://../private", "fixture.tar.gz").is_err());
    assert!(expand_origin("https://user:secret@example.test", "fixture.tar.gz").is_err());
    assert!(expand_origin("https://example.test/releases", "fixture.tar.gz").is_ok());
}

#[test]
fn tar_and_zip_budget_rejects_entry_count_single_entry_and_total_bombs() {
    let mut count = ExtractionBudget {
        entries: MAX_ARCHIVE_ENTRIES,
        expanded_bytes: 0,
    };
    assert!(count.account(0, "bomb.tar.gz", Path::new("extra")).is_err());

    let mut single = ExtractionBudget::default();
    assert!(single
        .account(
            MAX_ARCHIVE_ENTRY_BYTES + 1,
            "bomb.zip",
            Path::new("oversized")
        )
        .is_err());

    let mut total = ExtractionBudget {
        entries: 1,
        expanded_bytes: MAX_ARCHIVE_EXPANDED_BYTES,
    };
    assert!(total
        .account(1, "bomb.zip", Path::new("aggregate"))
        .is_err());
}

#[cfg(unix)]
#[test]
fn fetch_lock_rejects_symlink_swap_before_nofollow_open() {
    use std::os::unix::fs::symlink;

    let _environment = FETCH_ENV_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let lock = root.path().join("candidate.lock");
    let outside = root.path().join("outside");
    std::fs::write(&outside, b"outside").unwrap();
    std::env::set_var("AROS_FETCH_TEST_PAUSE_AT", "lock-before-open");
    std::env::set_var("AROS_FETCH_TEST_PAUSE_MS", "300");
    let attempted = lock.clone();
    let worker = std::thread::spawn(move || FetchLock::acquire_path(&attempted));
    std::thread::sleep(std::time::Duration::from_millis(100));
    symlink(&outside, &lock).unwrap();
    let result = worker.join().unwrap();
    std::env::remove_var("AROS_FETCH_TEST_PAUSE_AT");
    std::env::remove_var("AROS_FETCH_TEST_PAUSE_MS");
    assert!(result.is_err());
    assert_eq!(std::fs::read(outside).unwrap(), b"outside");
}
