//! Characterize the Git boundary required by the unchanged legacy producer.
//!
//! These negative protocol probes use only tiny, test-owned repositories.
//! Successful conversion and recursive gitlinks are tested exclusively through
//! `LegacySourceView` in `source_snapshots/legacy_views.rs`, not a second adapter.
//! No test launches a compiler.
#![cfg(unix)]

use std::{
    collections::BTreeSet, fs, os::unix::fs::PermissionsExt as _, path::Path, process::Command,
    time::Duration,
};

use aros_common::{run_output_with_input_and_control, CancellationToken};

const CAPTURE: usize = 1024 * 1024;
const HEAD_TREE: &str = "HEAD^{tree}";

fn invoke(root: &Path, args: &[&str], input: &[u8]) -> (bool, Vec<u8>) {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .current_dir(root)
        .args([
            "--no-pager",
            "--no-replace-objects",
            "--no-lazy-fetch",
            "--literal-pathspecs",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "gc.auto=0",
            "-c",
            "user.name=Legacy boundary fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args);
    let output = run_output_with_input_and_control(
        &mut command,
        input,
        CAPTURE,
        Duration::from_secs(10),
        &CancellationToken::default(),
    )
    .unwrap();
    assert!(!output.timed_out && !output.cancelled, "{args:?}");
    (
        output.status.success(),
        output.stdout.exact_bytes().unwrap().to_vec(),
    )
}

fn git(root: &Path, args: &[&str]) -> String {
    let (success, bytes) = invoke(root, args, &[]);
    assert!(success, "{args:?}");
    String::from_utf8(bytes).unwrap().trim().to_owned()
}

fn commit(root: &Path, message: &str) -> String {
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", message]);
    git(root, &["rev-parse", "HEAD"])
}

fn source(root: &Path) -> String {
    fs::create_dir(root).unwrap();
    git(root, &["init", "-q", "--template="]);
    fs::write(root.join("file"), b"previous version\n").unwrap();
    let parent = commit(root, "test: previous material");
    fs::write(root.join("file"), b"selected version\n").unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/Größe file"), b"raw\0binary\n").unwrap();
    commit(root, "test: selected material");
    parent
}

fn fresh_store(root: &Path, head: &str) {
    fs::create_dir(root).unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    for directory in ["objects", "objects/pack", "refs"] {
        fs::create_dir(root.join(".git").join(directory)).unwrap();
    }
    // Fixed fixture metadata only: no init templates, remotes, hooks, alternates,
    // refs, user configuration, credentials or executable filter definitions.
    fs::write(
        root.join(".git/config"),
        b"[core]\nrepositoryformatversion = 0\nbare = false\nfilemode = true\nlogallrefupdates = false\n",
    )
    .unwrap();
    fs::write(root.join(".git/HEAD"), format!("{head}\n")).unwrap();
    fs::write(root.join(".git/shallow"), format!("{head}\n")).unwrap();
}

fn pack(root: &Path, objects: &[&str]) -> (bool, Vec<u8>) {
    invoke(
        root,
        &[
            "pack-objects",
            "--stdout",
            "--no-reuse-object",
            "--window=0",
            "--depth=0",
            "--threads=1",
            "--compression=0",
        ],
        format!("{}\n", objects.join("\n")).as_bytes(),
    )
}

fn objects(root: &Path) -> BTreeSet<String> {
    git(
        root,
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname)",
        ],
    )
    .lines()
    .map(str::to_owned)
    .collect()
}

fn payload(root: &Path) {
    fs::write(root.join("file"), b"selected version\n").unwrap();
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/Größe file"), b"raw\0binary\n").unwrap();
}

#[test]
fn strict_import_rejects_incomplete_tree_and_corrupt_pack() {
    let temporary = tempfile::tempdir().unwrap();
    let original = temporary.path().join("original");
    source(&original);
    let head = git(&original, &["rev-parse", "HEAD"]);
    let tree = git(&original, &["rev-parse", HEAD_TREE]);
    let missing = temporary.path().join("missing");
    fresh_store(&missing, &head);
    let (success, bytes) = pack(&original, &[&tree]);
    assert!(success);
    assert!(!invoke(&missing, &["index-pack", "--stdin", "--strict"], &bytes).0);

    let corrupt = temporary.path().join("corrupt");
    fresh_store(&corrupt, &head);
    let blob = git(&original, &["rev-parse", "HEAD:file"]);
    let (success, mut bytes) = pack(&original, &[&blob]);
    assert!(success);
    *bytes.last_mut().unwrap() ^= 1;
    assert!(!invoke(&corrupt, &["index-pack", "--stdin", "--strict"], &bytes).0);
}

#[test]
fn valid_pack_checksum_does_not_bind_a_claimed_object_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let original = temporary.path().join("original");
    source(&original);
    let head = git(&original, &["rev-parse", "HEAD"]);
    let expected = git(&original, &["rev-parse", "HEAD:file"]);
    let wrong = git(&original, &["rev-parse", "HEAD:nested/Größe file"]);
    let view = temporary.path().join("view");
    fresh_store(&view, &head);
    let (success, bytes) = pack(&original, &[&wrong]);
    assert!(success);
    assert!(invoke(&view, &["index-pack", "--stdin", "--strict"], &bytes).0);
    assert_eq!(objects(&view), BTreeSet::from([wrong]));
    assert_ne!(objects(&view), BTreeSet::from([expected]));
}

#[test]
fn metadata_free_material_cannot_answer_legacy_identity_queries() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    payload(root);
    for args in [
        &["rev-parse", "HEAD"][..],
        &["rev-parse", HEAD_TREE][..],
        &["status", "--porcelain", "--untracked-files=no"][..],
    ] {
        assert!(!invoke(root, args, &[]).0);
    }
}

#[test]
fn poisoned_loose_object_cannot_keep_the_selected_identity_after_import() {
    let temporary = tempfile::tempdir().unwrap();
    let original = temporary.path().join("original");
    source(&original);
    let head = git(&original, &["rev-parse", "HEAD"]);
    let expected = git(&original, &["rev-parse", "HEAD:file"]);
    let wrong = git(&original, &["rev-parse", "HEAD:nested/Größe file"]);
    let loose = |oid: &str| {
        original
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..])
    };
    // Deliberately poison only this fixture's ODB, not a user's checkout.
    fs::set_permissions(loose(&expected), fs::Permissions::from_mode(0o600)).unwrap();
    fs::copy(loose(&wrong), loose(&expected)).unwrap();
    let (exported, bytes) = pack(&original, &[&expected]);
    if !exported {
        return; // A Git implementation may reject the corrupt source first.
    }
    let view = temporary.path().join("view");
    fresh_store(&view, &head);
    if invoke(&view, &["index-pack", "--stdin", "--strict"], &bytes).0 {
        assert!(!objects(&view).contains(&expected));
    }
}
