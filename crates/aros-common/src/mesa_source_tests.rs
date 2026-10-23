use super::MesaSourceSnapshot;
use crate::local_source::LocalSourceIdentity;
use crate::{measure_tree_content_cas, sha256_bytes, TreeContentEntryKind};
#[cfg(unix)]
use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::{fs, path::Path, process::Command};
use tempfile::{tempdir, TempDir};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-c")
        .arg("user.name=Local Mesa source test")
        .arg("-c")
        .arg("user.email=mesa-source-test@example.invalid")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn checkout() -> TempDir {
    let temp = tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    fs::write(temp.path().join("input.c"), b"initial source\n").unwrap();
    fs::write(temp.path().join(".gitignore"), b"build/\n*.cache\n").unwrap();
    git(temp.path(), &["add", "input.c", ".gitignore"]);
    git(
        temp.path(),
        &["commit", "-qm", "Initial Mesa source fixture"],
    );
    fs::create_dir_all(temp.path().join("build/fixture")).unwrap();
    temp
}

#[test]
fn complete_snapshot_captures_verifies_and_rejects_changed_source() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("mesa.c"), b"Mesa source\n").unwrap();
    let tree = measure_tree_content_cas(temp.path()).unwrap();
    let expected = tree.payload_digest_excluding(None);

    let snapshot = MesaSourceSnapshot::capture(temp.path(), &expected, None).unwrap();
    snapshot.verify(temp.path()).unwrap();
    assert_eq!(
        snapshot.regular_input_digest("mesa.c"),
        Some(&sha256_bytes(b"Mesa source\n"))
    );
    assert_eq!(
        snapshot
            .entry_kinds()
            .map(|(path, kind)| (path.to_vec(), kind))
            .collect::<Vec<_>>(),
        vec![(b"mesa.c".to_vec(), TreeContentEntryKind::RegularFile)]
    );
    assert!(snapshot.regular_input_digest("./mesa.c").is_none());

    assert!(MesaSourceSnapshot::capture(
        temp.path(),
        &sha256_bytes(b"wrong complete-tree digest"),
        None,
    )
    .is_err());

    fs::write(temp.path().join("mesa.c"), b"changed Mesa source\n").unwrap();
    assert!(snapshot.verify(temp.path()).is_err());
}

#[test]
fn selected_snapshot_rejects_reselection_and_newly_adopted_bytes() {
    let temp = tempdir().unwrap();
    let other = tempdir().unwrap();
    fs::write(temp.path().join("input"), b"original").unwrap();
    fs::write(other.path().join("input"), b"original").unwrap();
    let expected = measure_tree_content_cas(temp.path())
        .unwrap()
        .payload_digest_excluding(None);
    let original = MesaSourceSnapshot::capture(temp.path(), &expected, None).unwrap();
    original
        .verify_selection(temp.path(), &expected, None)
        .unwrap();
    assert!(original
        .verify_selection(other.path(), &expected, None)
        .is_err());
    assert!(original
        .verify_selection(temp.path(), &sha256_bytes(b"foreign selection"), None)
        .is_err());

    fs::write(temp.path().join("input"), b"changed").unwrap();
    let changed = measure_tree_content_cas(temp.path())
        .unwrap()
        .payload_digest_excluding(None);
    let adopted = MesaSourceSnapshot::capture(temp.path(), &changed, None).unwrap();
    assert!(original
        .verify_selection(temp.path(), &expected, None)
        .is_err());
    assert!(adopted
        .verify_selection(temp.path(), &expected, None)
        .is_err());
}

#[test]
fn selected_local_snapshot_preserves_measurement_domain_and_exclusions() {
    let temp = checkout();
    let local = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    let snapshot =
        MesaSourceSnapshot::capture(temp.path(), &local.content_sha256, Some(&local)).unwrap();
    snapshot
        .verify_selection(temp.path(), &local.content_sha256, Some(&local))
        .unwrap();
    assert!(snapshot
        .verify_selection(temp.path(), &local.content_sha256, None)
        .is_err());
    let mut reselected = local.clone();
    reselected.generated_subtree = "build/other".into();
    assert!(snapshot
        .verify_selection(temp.path(), &local.content_sha256, Some(&reselected))
        .is_err());

    let complete_digest = measure_tree_content_cas(temp.path())
        .unwrap()
        .payload_digest_excluding(None);
    let complete = MesaSourceSnapshot::capture(temp.path(), &complete_digest, None).unwrap();
    assert!(complete
        .verify_selection(temp.path(), &complete_digest, Some(&local))
        .is_err());
    fs::write(temp.path().join("build/fixture/generated"), b"output").unwrap();
    snapshot
        .verify_selection(temp.path(), &local.content_sha256, Some(&local))
        .unwrap();
    fs::write(temp.path().join("ignored.cache"), b"changed source").unwrap();
    assert!(snapshot
        .verify_selection(temp.path(), &local.content_sha256, Some(&local))
        .is_err());
}

#[cfg(unix)]
#[test]
fn selected_complete_snapshot_retains_metadata_drift_checks() {
    use std::os::unix::fs::PermissionsExt as _;
    let temp = tempdir().unwrap();
    let input = temp.path().join("input");
    fs::write(&input, b"unchanged bytes").unwrap();
    let expected = measure_tree_content_cas(temp.path())
        .unwrap()
        .payload_digest_excluding(None);
    let snapshot = MesaSourceSnapshot::capture(temp.path(), &expected, None).unwrap();
    let mode = fs::metadata(&input).unwrap().permissions().mode();
    fs::set_permissions(&input, fs::Permissions::from_mode(mode ^ 0o100)).unwrap();
    assert!(snapshot
        .verify_selection(temp.path(), &expected, None)
        .is_err());
}

#[test]
fn selected_complete_snapshot_rejects_same_payload_new_filesystem_identity() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("input");
    fs::write(&input, b"unchanged bytes").unwrap();
    let expected = measure_tree_content_cas(temp.path())
        .unwrap()
        .payload_digest_excluding(None);
    let snapshot = MesaSourceSnapshot::capture(temp.path(), &expected, None).unwrap();
    let replacement = temp.path().join("replacement");
    fs::write(&replacement, b"unchanged bytes").unwrap();
    fs::rename(replacement, &input).unwrap();
    assert_eq!(
        measure_tree_content_cas(temp.path())
            .unwrap()
            .payload_digest_excluding(None),
        expected
    );
    assert!(snapshot
        .verify_selection(temp.path(), &expected, None)
        .is_err());
}

#[test]
#[cfg(unix)]
fn symbolic_link_targets_return_original_relative_and_absolute_bytes() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("target"), b"target bytes").unwrap();
    symlink("target", temp.path().join("relative-link")).unwrap();
    let absolute_target = "/outside/original-target";
    symlink(absolute_target, temp.path().join("absolute-link")).unwrap();
    let tree = measure_tree_content_cas(temp.path()).unwrap();
    let snapshot =
        MesaSourceSnapshot::capture(temp.path(), &tree.payload_digest_excluding(None), None)
            .unwrap();

    assert_eq!(
        snapshot.symbolic_link_targets().unwrap(),
        BTreeMap::from([
            (
                "absolute-link".to_owned(),
                absolute_target.as_bytes().to_vec()
            ),
            ("relative-link".to_owned(), b"target".to_vec()),
        ])
    );
}

#[test]
#[cfg(unix)]
fn captured_symbolic_link_targets_remain_original_after_target_drift() {
    let temp = tempdir().unwrap();
    symlink("original", temp.path().join("link")).unwrap();
    let tree = measure_tree_content_cas(temp.path()).unwrap();
    let snapshot =
        MesaSourceSnapshot::capture(temp.path(), &tree.payload_digest_excluding(None), None)
            .unwrap();
    let original = BTreeMap::from([("link".to_owned(), b"original".to_vec())]);

    assert_eq!(snapshot.captured_symbolic_link_targets().unwrap(), original);

    fs::remove_file(temp.path().join("link")).unwrap();
    symlink("changed", temp.path().join("link")).unwrap();

    assert_eq!(snapshot.captured_symbolic_link_targets().unwrap(), original);
    assert!(snapshot.symbolic_link_targets().is_err());
    assert!(snapshot.verify(temp.path()).is_err());
}

#[test]
#[cfg(unix)]
fn symbolic_link_target_access_does_not_invent_later_links() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("source"), b"source").unwrap();
    let tree = measure_tree_content_cas(temp.path()).unwrap();
    let snapshot =
        MesaSourceSnapshot::capture(temp.path(), &tree.payload_digest_excluding(None), None)
            .unwrap();

    symlink("source", temp.path().join("later-link")).unwrap();

    assert!(snapshot.symbolic_link_targets().is_err());
}

#[test]
#[cfg(unix)]
fn local_snapshot_returns_raw_target_bytes_for_an_in_tree_link() {
    let temp = checkout();
    symlink("input.c", temp.path().join("source-link")).unwrap();
    let local = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    let snapshot =
        MesaSourceSnapshot::capture(temp.path(), &local.content_sha256, Some(&local)).unwrap();

    assert_eq!(
        snapshot.symbolic_link_targets().unwrap().get("source-link"),
        Some(&b"input.c".to_vec())
    );
}

#[test]
fn local_snapshot_omits_only_the_selected_build_subtree() {
    let temp = checkout();
    fs::write(temp.path().join("build/fixture/excluded.c"), b"excluded").unwrap();
    fs::create_dir_all(temp.path().join("build/other")).unwrap();
    fs::write(temp.path().join("build/other/input.c"), b"selected sibling").unwrap();
    let local = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    assert_eq!(local.generated_subtree, "build/fixture");
    let snapshot =
        MesaSourceSnapshot::capture(temp.path(), &local.content_sha256, Some(&local)).unwrap();
    snapshot.verify(temp.path()).unwrap();

    let entries: Vec<_> = snapshot
        .entry_kinds()
        .map(|(path, kind)| (path.to_vec(), kind))
        .collect();
    assert!(entries.iter().all(|(path, _)| {
        path.as_slice() != b".git"
            && !path
                .strip_prefix(b".git")
                .is_some_and(|suffix| suffix.starts_with(b"/"))
            && path.as_slice() != b"build/fixture"
            && !path
                .strip_prefix(b"build/fixture")
                .is_some_and(|suffix| suffix.starts_with(b"/"))
    }));
    assert!(entries.iter().any(|(path, kind)| {
        path.as_slice() == b"build/other/input.c" && *kind == TreeContentEntryKind::RegularFile
    }));
    assert_eq!(
        snapshot.regular_input_digest("input.c"),
        Some(&sha256_bytes(b"initial source\n"))
    );
    assert!(snapshot.regular_input_digest(".git/config").is_none());
    assert!(snapshot
        .regular_input_digest("build/fixture/excluded.c")
        .is_none());
    assert_eq!(
        snapshot.regular_input_digest("build/other/input.c"),
        Some(&sha256_bytes(b"selected sibling"))
    );

    fs::write(
        temp.path().join("build/fixture/output.o"),
        b"generated output",
    )
    .unwrap();
    snapshot.verify(temp.path()).unwrap();

    fs::create_dir_all(temp.path().join("build/other")).unwrap();
    fs::write(
        temp.path().join("build/other/output.o"),
        b"sibling build output",
    )
    .unwrap();
    assert!(snapshot.verify(temp.path()).is_err());
}

#[test]
fn local_snapshot_rejects_dirty_ignored_and_untracked_source_changes() {
    for name in ["input.c", "ignored.cache", "untracked.c"] {
        let temp = checkout();
        let local = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
        let snapshot =
            MesaSourceSnapshot::capture(temp.path(), &local.content_sha256, Some(&local)).unwrap();

        fs::write(temp.path().join(name), format!("changed {name}\n")).unwrap();
        assert!(
            snapshot.verify(temp.path()).is_err(),
            "source change in {name} must invalidate the snapshot"
        );
    }
}

#[test]
fn local_snapshot_rejects_wrong_digest_and_malformed_generated_subtree() {
    let temp = checkout();
    let local = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();

    assert!(MesaSourceSnapshot::capture(
        temp.path(),
        &sha256_bytes(b"wrong local-source digest"),
        Some(&local),
    )
    .is_err());

    for generated_subtree in [
        "build",
        "build/fixture/child",
        "build/../fixture",
        "buildish/fixture",
        "build/fixture-lookalike",
    ] {
        let mut malformed = local.clone();
        malformed.generated_subtree = generated_subtree.into();
        assert!(
            MesaSourceSnapshot::capture(temp.path(), &local.content_sha256, Some(&malformed))
                .is_err(),
            "malformed generated subtree {generated_subtree:?} must be rejected"
        );
    }
}

#[test]
fn local_snapshot_rejects_index_and_committed_generated_source() {
    let temp = checkout();
    let local = LocalSourceIdentity::capture(temp.path(), "fixture").unwrap();
    let snapshot =
        MesaSourceSnapshot::capture(temp.path(), &local.content_sha256, Some(&local)).unwrap();
    let generated = temp.path().join("build/fixture/tracked.c");
    fs::write(&generated, b"tracked generated source").unwrap();
    git(temp.path(), &["add", "-f", "build/fixture/tracked.c"]);

    assert!(snapshot.verify(temp.path()).is_err());
    assert!(
        MesaSourceSnapshot::capture(temp.path(), &local.content_sha256, Some(&local))
            .unwrap_err()
            .contains("tracked")
    );

    git(temp.path(), &["commit", "-qm", "Track generated source"]);
    git(
        temp.path(),
        &["rm", "--cached", "-f", "build/fixture/tracked.c"],
    );
    assert!(
        MesaSourceSnapshot::capture(temp.path(), &local.content_sha256, Some(&local))
            .unwrap_err()
            .contains("tracked")
    );
}

#[test]
fn generated_path_permissions_use_canonical_paths_and_exact_subtrees() {
    let external = tempdir().unwrap();
    let external_file = external.path().join("output.bin");
    fs::write(&external_file, b"external output").unwrap();
    let external_file = external_file.canonicalize().unwrap();

    let complete_root = tempdir().unwrap();
    fs::write(complete_root.path().join("mesa.c"), b"source\n").unwrap();
    fs::create_dir_all(complete_root.path().join("build/fixture")).unwrap();
    let complete_tree = measure_tree_content_cas(complete_root.path()).unwrap();
    let complete = MesaSourceSnapshot::capture(
        complete_root.path(),
        &complete_tree.payload_digest_excluding(None),
        None,
    )
    .unwrap();
    let complete_root_path = complete_root.path().canonicalize().unwrap();
    let complete_source = complete_root_path.join("mesa.c").canonicalize().unwrap();
    assert!(!complete.permits_generated_path(&complete_root_path, &complete_root_path));
    assert!(!complete.permits_generated_path(&complete_root_path, &complete_source));
    assert!(complete.permits_generated_path(&complete_root_path, &external_file));
    assert!(
        !complete.permits_generated_path(&complete_root_path, complete_root_path.parent().unwrap())
    );
    assert!(!complete
        .permits_generated_path(&complete_root_path, &complete_root_path.join("../escape")));
    assert!(!complete.permits_generated_path(&complete_root_path, Path::new("relative")));
    assert!(!complete.permits_generated_path(external.path(), &external_file));
    let duplicate = tempdir().unwrap();
    fs::write(duplicate.path().join("mesa.c"), b"source\n").unwrap();
    fs::create_dir_all(duplicate.path().join("build/fixture")).unwrap();
    assert!(complete.verify(duplicate.path()).is_err());

    let local_root = checkout();
    let local = LocalSourceIdentity::capture(local_root.path(), "fixture").unwrap();
    let snapshot =
        MesaSourceSnapshot::capture(local_root.path(), &local.content_sha256, Some(&local))
            .unwrap();
    let local_root_path = local_root.path().canonicalize().unwrap();
    let selected = local_root_path.join("build/fixture/output.o");
    fs::write(&selected, b"selected generated output").unwrap();
    let sibling = local_root_path.join("build/other/output.o");
    fs::create_dir_all(sibling.parent().unwrap()).unwrap();
    fs::write(&sibling, b"sibling output").unwrap();
    let selected_lookalike = local_root_path.join("build/fixture-copy/output.o");
    fs::create_dir_all(selected_lookalike.parent().unwrap()).unwrap();
    fs::write(&selected_lookalike, b"prefix lookalike output").unwrap();

    assert!(snapshot.permits_generated_path(&local_root_path, &selected.canonicalize().unwrap()));
    assert!(!snapshot.permits_generated_path(&local_root_path, &local_root_path));
    assert!(!snapshot.permits_generated_path(&local_root_path, &sibling.canonicalize().unwrap()));
    assert!(!snapshot.permits_generated_path(
        &local_root_path,
        &selected_lookalike.canonicalize().unwrap()
    ));
    assert!(snapshot.permits_generated_path(&local_root_path, &external_file));

    let root_name = local_root_path.file_name().unwrap().to_string_lossy();
    let root_lookalike = tempfile::Builder::new()
        .prefix(&format!("{root_name}-lookalike"))
        .tempdir_in(local_root_path.parent().unwrap())
        .unwrap();
    let root_lookalike_file = root_lookalike.path().join("output.bin");
    fs::write(&root_lookalike_file, b"outside checkout").unwrap();
    assert!(snapshot.permits_generated_path(
        &local_root_path,
        &root_lookalike_file.canonicalize().unwrap()
    ));
}
