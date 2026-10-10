//! Filesystem tests for the bounded compatibility evidence publisher.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use aros_common::sha256_bytes;

use super::{preflight, publish_files};

fn temporary_root() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

const INPUTS: &[u8] =
    br#"{"schema":"aros-toolchain-compatibility-inputs-v1","fixture":"selected"}"#;

fn evidence_files() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([
        (
            "compatibility-measurement.json".into(),
            br#"{"schema":"aros-toolchain-compatibility-measurement-v2"}"#.to_vec(),
        ),
        (
            "native-compatibility.receipt.json".into(),
            br#"{"schema":"aros-toolchain-native-compatibility-receipt-v3"}"#.to_vec(),
        ),
        (
            "cmake-consumer.report.json".into(),
            br#"{"phase":"cmake-consumer"}"#.to_vec(),
        ),
        (
            "cmake-consumer.stdout.log".into(),
            b"configure output\n".to_vec(),
        ),
        (
            "cmake-consumer.stderr.log".into(),
            b"warning output\n".to_vec(),
        ),
        (
            "standalone-c.o".into(),
            vec![0x7f, b'E', b'L', b'F', 0, 1, 2, 255],
        ),
    ])
}

fn publish_fixture(
    destination: &Path,
    inputs: &[u8],
    files: &BTreeMap<String, Vec<u8>>,
) -> miette::Result<()> {
    publish_files(
        destination,
        inputs,
        files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    )
}

fn assert_absent(path: &Path) {
    assert!(matches!(
        fs::symlink_metadata(path),
        Err(ref error) if error.kind() == ErrorKind::NotFound
    ));
}

fn assert_preflight_rejected(destination: &Path, protected: &[&Path], message: &str) {
    let error = preflight(destination, protected).unwrap_err();
    assert!(
        error.to_string().contains(message),
        "expected {message:?}, got {error}"
    );
}

fn snapshot_flat_directory(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name();
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            assert!(metadata.is_file() && !metadata.file_type().is_symlink());
            (PathBuf::from(name), fs::read(entry.path()).unwrap())
        })
        .collect()
}

#[test]
fn publish_preserves_exact_input_bytes_and_the_closed_evidence_file_set() {
    let temporary = temporary_root();
    let destination = temporary.path().join("compatibility-export");
    let files = evidence_files();

    publish_fixture(&destination, INPUTS, &files).unwrap();

    let input_path = destination.join("inputs.json");
    let actual_inputs = fs::read(&input_path).unwrap();
    assert_eq!(actual_inputs, INPUTS);
    assert_eq!(sha256_bytes(&actual_inputs), sha256_bytes(INPUTS));

    let evidence_root = destination.join("evidence");
    let mut actual_names = fs::read_dir(&evidence_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    actual_names.sort();
    let expected_names = files.keys().cloned().collect::<Vec<_>>();
    assert_eq!(actual_names, expected_names);

    for (name, expected_bytes) in &files {
        let actual_bytes = fs::read(evidence_root.join(name)).unwrap();
        assert_eq!(actual_bytes, *expected_bytes, "member {name}");
        assert_eq!(
            sha256_bytes(&actual_bytes),
            sha256_bytes(expected_bytes),
            "member digest {name}"
        );
    }
}

#[test]
fn occupied_destination_is_rejected_without_changing_existing_contents() {
    let temporary = temporary_root();
    let destination = temporary.path().join("occupied");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"keep these bytes").unwrap();
    let before = snapshot_flat_directory(&destination);

    let error = publish_fixture(&destination, INPUTS, &evidence_files()).unwrap_err();

    assert!(error
        .to_string()
        .contains("already exists or is unavailable"));
    assert_eq!(snapshot_flat_directory(&destination), before);
}

#[test]
fn preflight_rejects_symlinked_ancestor_and_leaf_without_following_them() {
    let temporary = temporary_root();
    let real_parent = temporary.path().join("real-parent");
    fs::create_dir(&real_parent).unwrap();
    let ancestor_link = temporary.path().join("ancestor-link");
    symlink(&real_parent, &ancestor_link).unwrap();

    assert_preflight_rejected(&ancestor_link.join("export"), &[], "no symlink components");
    assert!(fs::symlink_metadata(&ancestor_link)
        .unwrap()
        .file_type()
        .is_symlink());

    let leaf_target = temporary.path().join("leaf-target");
    fs::create_dir(&leaf_target).unwrap();
    fs::write(leaf_target.join("sentinel"), b"target unchanged").unwrap();
    let leaf_link = temporary.path().join("leaf-link");
    symlink(&leaf_target, &leaf_link).unwrap();

    assert_preflight_rejected(&leaf_link, &[], "already exists or is unavailable");
    assert!(fs::symlink_metadata(&leaf_link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read(leaf_target.join("sentinel")).unwrap(),
        b"target unchanged"
    );
}

#[test]
fn preflight_rejects_relative_dotdot_and_missing_parent_destinations() {
    let temporary = temporary_root();

    assert_preflight_rejected(
        Path::new("relative/export"),
        &[],
        "absolute normalized path",
    );

    let dotdot = temporary.path().join("child/../export");
    assert_preflight_rejected(&dotdot, &[], "absolute normalized path");

    let missing_parent = temporary.path().join("not-created/export");
    assert_preflight_rejected(&missing_parent, &[], "parent must already exist");
    assert_absent(&temporary.path().join("not-created"));
}

#[test]
fn preflight_rejects_nested_overlaps_in_both_directions_and_resolves_aliases() {
    let temporary = temporary_root();
    let root = temporary.path();

    // Destination is an absent ancestor of an absent protected input root.
    let destination_ancestor = root.join("new-output");
    let missing_input_below = destination_ancestor.join("future-input");
    assert_preflight_rejected(
        &destination_ancestor,
        &[&missing_input_below],
        "separate from every input",
    );

    // Destination is below an existing protected input root.
    let input_root = root.join("selected-input");
    fs::create_dir(&input_root).unwrap();
    let destination_below = input_root.join("nested-output");
    assert_preflight_rejected(
        &destination_below,
        &[&input_root],
        "separate from every input",
    );

    // Resolve a missing protected path through a symlink alias before comparing.
    let real_root = root.join("real-input-root");
    let real_nested = real_root.join("nested");
    fs::create_dir_all(&real_nested).unwrap();
    let alias_root = root.join("input-alias");
    symlink(&real_root, &alias_root).unwrap();
    let destination = real_nested.join("export");
    let aliased_input = alias_root.join("nested/export/future-input");
    assert_preflight_rejected(&destination, &[&aliased_input], "separate from every input");
    assert_absent(&destination);
    assert_absent(&real_nested.join("export/future-input"));
}

#[test]
fn prospective_output_suffixes_cannot_hide_overlap_by_case() {
    let temporary = temporary_root();
    let destination = temporary.path().join("EvidenceOut");
    let protected = temporary.path().join("evidenceout/future-input");
    assert_preflight_rejected(&destination, &[&protected], "separate from every input");
    assert_absent(&destination);
    assert_absent(&protected);
    // A shared textual prefix is not a path-component overlap.
    let separate = temporary.path().join("EvidenceOut-other");
    preflight(&destination, &[&separate]).unwrap();
}

#[test]
fn existing_case_and_unicode_aliases_cannot_hide_protected_roots() {
    let temporary = temporary_root();
    for (original, alias) in [
        ("SelectedRoot", "selectedroot"),
        ("Caf\u{e9}Root", "Cafe\u{301}Root"),
    ] {
        let root = temporary.path().join(original);
        fs::create_dir(&root).unwrap();
        let alias = temporary.path().join(alias);
        if !alias.is_dir() {
            // Linux's case-sensitive test filesystem has no such alias.
            continue;
        }
        let destination = alias.join("export");
        assert_preflight_rejected(&destination, &[&root], "separate from every input");
        assert_absent(&root.join("export"));
    }
    let selected = temporary.path().join("SelectedRoot");
    let separate = temporary.path().join("separate-export");
    preflight(&separate, &[&selected]).unwrap();
}

#[test]
fn unsafe_member_name_prevents_creation_of_the_final_output() {
    let temporary = temporary_root();
    let destination = temporary.path().join("unsafe-export");
    let files = BTreeMap::from([("../escape.log".to_owned(), b"not published".to_vec())]);

    let error = publish_fixture(&destination, INPUTS, &files).unwrap_err();

    assert!(error.to_string().contains("unsafe member name"));
    assert_absent(&destination);
    assert_absent(&temporary.path().join("escape.log"));
}

#[test]
fn casefold_member_collision_leaves_no_final_output_root() {
    let temporary = temporary_root();
    let destination = temporary.path().join("colliding-export");
    let files = BTreeMap::from([
        ("Report.log".to_owned(), b"first".to_vec()),
        ("report.log".to_owned(), b"second".to_vec()),
    ]);

    let error = publish_fixture(&destination, INPUTS, &files).unwrap_err();

    assert!(error
        .to_string()
        .contains("cannot durably stage complete compatibility evidence"));
    assert_absent(&destination);
}
