//! Tests for the embedded engine and its placement.

use super::{api_version, digest, file, file_count, materialize, paths, STAMP_FILE};
use std::fs;

#[test]
fn the_engine_is_embedded_whole() {
    assert!(file_count() > 100, "only {} files embedded", file_count());
    // Three files that anchor the three kinds of content: the entry point, the
    // largest module and the version declaration the contract rests on.
    assert!(file("CMakeLists.txt").is_some());
    assert!(file("AROS.cmake").is_some());
    assert!(file("EngineVersion.cmake").is_some());
    assert!(file("no/such/file.cmake").is_none());
}

#[test]
fn paths_are_sorted_and_relative() {
    let all: Vec<_> = paths().collect();
    let mut sorted = all.clone();
    sorted.sort_unstable();
    assert_eq!(all, sorted, "the embedded table is not sorted");
    for path in all {
        assert!(!path.starts_with('/'), "{path} is absolute");
        assert!(!path.contains(".."), "{path} escapes the engine root");
    }
}

#[test]
fn the_api_version_comes_from_the_engine() {
    let declared = file("EngineVersion.cmake").expect("version file");
    let expected = declared
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("set(AROS_CMAKE_ENGINE_API_VERSION")?
                .trim_start()
                .strip_suffix(')')?
                .trim()
                .parse::<u32>()
                .ok()
        })
        .expect("a version in the engine file");
    assert_eq!(api_version(), expected);
}

#[test]
fn the_digest_is_a_sha256() {
    assert_eq!(digest().len(), 64);
    assert!(digest().bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn placement_writes_every_file_and_stamps_it() {
    let directory = tempfile::tempdir().expect("temp dir");
    let placement = materialize(directory.path()).expect("materialize");

    assert!(!placement.reused);
    assert_eq!(placement.written, file_count());
    assert_eq!(placement.digest, digest());
    for path in paths() {
        assert!(directory.path().join(path).is_file(), "{path} missing");
    }
    let stamp = fs::read_to_string(directory.path().join(STAMP_FILE)).expect("stamp");
    assert_eq!(stamp.trim(), digest());
}

#[test]
fn a_second_placement_rewrites_nothing() {
    let directory = tempfile::tempdir().expect("temp dir");
    materialize(directory.path()).expect("first");
    let again = materialize(directory.path()).expect("second");

    assert!(again.reused, "the second call rewrote the engine");
    assert_eq!(again.written, 0);
}

#[test]
fn a_stale_module_is_removed() {
    // The reason this matters: an engine module left behind by an earlier
    // version is still visible to `include()`, so a directory that merely
    // contains the current engine is not the same as one that holds only it.
    let directory = tempfile::tempdir().expect("temp dir");
    materialize(directory.path()).expect("first");

    let stale = directory.path().join("RemovedInThisVersion.cmake");
    fs::write(&stale, "message(FATAL_ERROR \"stale\")\n").expect("write stale");
    fs::remove_file(directory.path().join(STAMP_FILE)).expect("drop stamp");

    let placement = materialize(directory.path()).expect("second");
    assert!(!stale.exists(), "the stale module survived");
    assert_eq!(placement.removed, 1);
}

#[test]
fn a_missing_file_defeats_a_matching_stamp() {
    // The stamp is a claim about the directory, not proof of it.
    let directory = tempfile::tempdir().expect("temp dir");
    materialize(directory.path()).expect("first");
    fs::remove_file(directory.path().join("AROS.cmake")).expect("remove a module");

    let placement = materialize(directory.path()).expect("second");
    assert!(
        !placement.reused,
        "a missing module was reported as present"
    );
    assert!(directory.path().join("AROS.cmake").is_file());
}

#[test]
fn nested_directories_survive_placement() {
    let directory = tempfile::tempdir().expect("temp dir");
    materialize(directory.path()).expect("materialize");
    assert!(directory.path().join("tests").is_dir());
    assert!(directory.path().join("toolchains/AROS.cmake").is_file());
}
