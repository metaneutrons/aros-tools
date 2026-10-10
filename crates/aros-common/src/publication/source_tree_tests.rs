//! Bounded filtered source-tree measurement and link-closure tests.

use super::*;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::symlink;

fn source_tree(path: &Path, generated_subtree: Option<&str>) -> TreeContentCas {
    measure_source_tree_content_cas_bounded(
        path,
        generated_subtree,
        TreeTraversalLimits::new(512, 1024 * 1024).unwrap(),
    )
    .unwrap()
}

#[test]
fn prepared_tree_measures_regular_files_and_symlinks() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::create_dir(root.join("bin")).unwrap();
    std::fs::write(root.join("bin/collector"), b"fixture collector").unwrap();
    symlink("collector", root.join("bin/collect")).unwrap();

    let measured =
        measure_tree_content_cas_bounded(&root, TreeTraversalLimits::new(16, 1024).unwrap())
            .unwrap();
    assert_eq!(measured.entry_count(), 3);
    assert!(measured.has_symlinks());
}

#[test]
fn prepared_tree_enforces_entry_byte_and_depth_limits() {
    let entry_temporary = tempfile::tempdir().unwrap();
    let entry_root = create_source_root(&entry_temporary);
    std::fs::write(entry_root.join("one"), b"1").unwrap();
    std::fs::write(entry_root.join("two"), b"2").unwrap();
    let error =
        measure_tree_content_cas_bounded(&entry_root, TreeTraversalLimits::new(1, 1024).unwrap())
            .unwrap_err();
    assert!(error.to_string().contains("entry traversal limit"));

    let byte_temporary = tempfile::tempdir().unwrap();
    let byte_root = create_source_root(&byte_temporary);
    std::fs::write(byte_root.join("oversized.bin"), b"12345").unwrap();
    let error =
        measure_tree_content_cas_bounded(&byte_root, TreeTraversalLimits::new(8, 4).unwrap())
            .unwrap_err();
    assert!(error
        .to_string()
        .contains("4-byte regular-file traversal limit"));

    let depth_temporary = tempfile::tempdir().unwrap();
    let depth_root = create_source_root(&depth_temporary);
    let max_depth = 128;
    let mut nested = depth_root.clone();
    for _ in 0..max_depth {
        nested.push("d");
        std::fs::create_dir(&nested).unwrap();
    }
    let depth_limits = TreeTraversalLimits::new(512, 1024).unwrap();
    let measured = measure_tree_content_cas_bounded(&depth_root, depth_limits).unwrap();
    assert_eq!(measured.entry_count(), max_depth);

    let leaf = nested.join("leaf");
    std::fs::write(&leaf, b"leaf").unwrap();
    let error = measure_tree_content_cas_bounded(&depth_root, depth_limits).unwrap_err();
    assert!(error.to_string().contains("128-component depth limit"));
    std::fs::remove_file(leaf).unwrap();

    let link = nested.join("link");
    symlink("unused", &link).unwrap();
    let error = measure_tree_content_cas_bounded(&depth_root, depth_limits).unwrap_err();
    assert!(error.to_string().contains("128-component depth limit"));
    std::fs::remove_file(link).unwrap();

    nested.push("d");
    std::fs::create_dir(&nested).unwrap();
    let error = measure_tree_content_cas_bounded(&depth_root, depth_limits).unwrap_err();
    assert!(error.to_string().contains("128-component depth limit"));
}

#[cfg(debug_assertions)]
#[test]
fn prepared_tree_fifo_swap_during_open_exits_without_blocking() {
    use std::time::Duration;

    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    let target = root.join("payload.bin");
    std::fs::write(&target, b"payload").unwrap();
    let replacement = root.join("replacement.fifo");
    create_fifo(&replacement);
    let ready = temporary.path().join("pause-ready");

    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "publication::source_tree_tests::prepared_tree_fifo_swap_probe_child",
            "--nocapture",
        ])
        .env("AROS_TEST_PREPARED_TREE_FIFO_SWAP", &root)
        .env(
            "AROS_PUBLICATION_TEST_PAUSE_AT",
            "tree-content-cas-before-file-open",
        )
        .env("AROS_PUBLICATION_TEST_PAUSE_MS", "1200")
        .env(
            "AROS_PUBLICATION_TEST_PAUSE_READY_AT",
            "tree-content-cas-before-file-open",
        )
        .env("AROS_PUBLICATION_TEST_PAUSE_READY_FILE", &ready);
    let runner = std::thread::spawn(move || {
        crate::run_output_with_timeout(&mut child, 4096, Duration::from_secs(5)).unwrap()
    });
    let marker_deadline = std::time::Instant::now() + Duration::from_secs(4);
    while !ready.exists() && !runner.is_finished() && std::time::Instant::now() < marker_deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let pause_reached = ready.exists();
    if pause_reached {
        std::fs::rename(replacement, target).unwrap();
    }
    let output = runner.join().unwrap();

    assert!(
        pause_reached,
        "FIFO swap probe never reached the pre-open pause"
    );
    assert!(
        !output.timed_out,
        "prepared-tree measurement blocked on a FIFO"
    );
    assert!(output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(output.stdout.exact_bytes().unwrap()).contains("1 passed"),
        "FIFO child probe was not selected: {output:?}"
    );
}

#[cfg(debug_assertions)]
#[test]
fn prepared_tree_fifo_swap_probe_child() {
    let Some(root) = std::env::var_os("AROS_TEST_PREPARED_TREE_FIFO_SWAP") else {
        return;
    };
    let error = measure_tree_content_cas_bounded(
        Path::new(&root),
        TreeTraversalLimits::new(16, 1024).unwrap(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unsupported"));
}

fn create_source_root(temporary: &tempfile::TempDir) -> PathBuf {
    let root = temporary.path().join("source");
    std::fs::create_dir(&root).unwrap();
    root
}

#[test]
fn source_tree_excludes_git_and_exact_generated_subtree_but_measures_other_files() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::create_dir_all(root.join(".git/objects")).unwrap();
    std::fs::create_dir_all(root.join("build/preset")).unwrap();
    std::fs::create_dir_all(root.join("build/other")).unwrap();
    std::fs::write(root.join(".git/HEAD"), b"ref: main\n").unwrap();
    std::fs::write(root.join(".git/objects/metadata"), b"before").unwrap();
    std::fs::write(root.join("build/preset/image.bin"), b"before").unwrap();
    std::fs::write(root.join("build/other/kept.bin"), b"kept").unwrap();
    // This name looks like an ignored build output, but source measurement is
    // deliberately independent of Git's ignore rules.
    std::fs::write(root.join("ignored-looking.cache"), b"source").unwrap();

    let before = source_tree(&root, Some("build/preset"));
    assert!(!before.has_symlinks());
    std::fs::write(root.join(".git/HEAD"), b"ref: work\n").unwrap();
    std::fs::write(root.join(".git/objects/metadata"), b"after").unwrap();
    std::fs::write(root.join("build/preset/image.bin"), b"after").unwrap();
    let filtered_changes = source_tree(&root, Some("build/preset"));
    assert_eq!(
        before.payload_digest_excluding(None),
        filtered_changes.payload_digest_excluding(None)
    );

    std::fs::write(root.join("ignored-looking.cache"), b"changed source").unwrap();
    let changed_source = source_tree(&root, Some("build/preset"));
    assert_ne!(
        before.payload_digest_excluding(None),
        changed_source.payload_digest_excluding(None)
    );

    std::fs::write(root.join("build/other/kept.bin"), b"changed sibling").unwrap();
    let changed_sibling = source_tree(&root, Some("build/preset"));
    assert_ne!(
        changed_source.payload_digest_excluding(None),
        changed_sibling.payload_digest_excluding(None)
    );
}

#[test]
fn source_tree_skips_special_files_inside_filtered_directories() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("build/preset")).unwrap();
    std::fs::write(root.join("source.c"), b"source").unwrap();
    create_fifo(&root.join(".git/index.lock"));
    create_fifo(&root.join("build/preset/output.fifo"));

    assert_eq!(source_tree(&root, Some("build/preset")).entry_count(), 2);
}

#[test]
fn source_tree_internal_link_chain_binds_terminal_content() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::write(root.join("payload.bin"), b"first payload").unwrap();
    symlink("payload.bin", root.join("first-link")).unwrap();
    symlink("first-link", root.join("second-link")).unwrap();

    let before = source_tree(&root, None);
    assert!(before.has_symlinks());
    std::fs::write(root.join("payload.bin"), b"second payload").unwrap();
    let after = source_tree(&root, None);
    assert_ne!(
        before.payload_digest_excluding(None),
        after.payload_digest_excluding(None)
    );
}

#[test]
fn source_tree_rejects_unsafe_or_unresolved_links() {
    let outside = tempfile::NamedTempFile::new().unwrap();
    assert_rejected_link(outside.path().as_os_str(), None, |_| {});
    assert_rejected_link(OsStr::new("../outside"), None, |_| {});
    assert_rejected_link(OsStr::new("missing-target"), None, |_| {});
    assert_rejected_link(OsStr::new(".git/metadata"), None, |root| {
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/metadata"), b"metadata").unwrap();
    });
    assert_rejected_link(
        OsStr::new("build/preset/generated.bin"),
        Some("build/preset"),
        |root| {
            std::fs::create_dir_all(root.join("build/preset")).unwrap();
            std::fs::write(root.join("build/preset/generated.bin"), b"generated").unwrap();
        },
    );
    assert_rejected_link(OsStr::new("regular-file/child"), None, |root| {
        std::fs::write(root.join("regular-file"), b"file").unwrap();
    });

    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    symlink("cycle-b", root.join("cycle-a")).unwrap();
    symlink("cycle-a", root.join("cycle-b")).unwrap();
    assert!(measure_source_tree_content_cas_bounded(
        &root,
        None,
        TreeTraversalLimits::new(32, 1024).unwrap(),
    )
    .is_err());
}

#[test]
fn source_tree_rejects_directory_aliases_that_expose_excluded_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("build/preset")).unwrap();
    std::fs::write(root.join("build/preset/generated.bin"), b"generated").unwrap();
    symlink("../build", root.join("src/build-alias")).unwrap();
    assert!(measure_source_tree_content_cas_bounded(
        &root,
        Some("build/preset"),
        TreeTraversalLimits::new(64, 4096).unwrap(),
    )
    .is_err());

    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::create_dir_all(root.join("build/preset")).unwrap();
    symlink("build", root.join("build-alias")).unwrap();
    assert!(measure_source_tree_content_cas_bounded(
        &root,
        Some("build-alias/preset"),
        TreeTraversalLimits::new(64, 4096).unwrap(),
    )
    .is_err());

    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("build/preset")).unwrap();
    std::fs::write(root.join("build/preset/generated.bin"), b"generated").unwrap();
    symlink("..", root.join("src/root-alias")).unwrap();
    assert!(measure_source_tree_content_cas_bounded(
        &root,
        Some("build/preset"),
        TreeTraversalLimits::new(64, 4096).unwrap(),
    )
    .is_err());
}

#[test]
fn source_tree_rejects_directory_link_cycles_without_exclusions() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::create_dir_all(root.join("src")).unwrap();
    symlink("..", root.join("src/root-alias")).unwrap();
    assert!(measure_source_tree_content_cas_bounded(
        &root,
        None,
        TreeTraversalLimits::new(64, 4096).unwrap(),
    )
    .is_err());
}

#[test]
fn source_tree_rejects_special_files_in_measured_closure() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    create_fifo(&root.join("source.fifo"));
    assert!(measure_source_tree_content_cas_bounded(
        &root,
        None,
        TreeTraversalLimits::new(32, 1024).unwrap(),
    )
    .is_err());
}

#[test]
fn source_tree_enforces_entry_byte_and_depth_ceilings() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    std::fs::write(root.join("a"), b"a").unwrap();
    std::fs::write(root.join("b"), b"b").unwrap();
    assert!(measure_source_tree_content_cas_bounded(
        &root,
        None,
        TreeTraversalLimits::new(1, 1024).unwrap(),
    )
    .is_err());

    let byte_temporary = tempfile::tempdir().unwrap();
    let byte_root = create_source_root(&byte_temporary);
    std::fs::write(byte_root.join("oversized.bin"), b"12345").unwrap();
    assert!(measure_source_tree_content_cas_bounded(
        &byte_root,
        None,
        TreeTraversalLimits::new(8, 4).unwrap(),
    )
    .is_err());

    let depth_temporary = tempfile::tempdir().unwrap();
    let depth_root = create_source_root(&depth_temporary);
    let mut nested = depth_root.clone();
    for _ in 0..=128 {
        nested.push("d");
        std::fs::create_dir(&nested).unwrap();
    }
    std::fs::write(nested.join("leaf"), b"leaf").unwrap();
    assert!(measure_source_tree_content_cas_bounded(
        &depth_root,
        None,
        TreeTraversalLimits::new(512, 1024).unwrap(),
    )
    .is_err());
}

#[test]
fn source_tree_requires_generated_exclusion_to_be_a_normal_relative_path() {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    for invalid in [
        "",
        "/absolute",
        "../parent",
        "build/./preset",
        "build\\preset",
    ] {
        assert!(measure_source_tree_content_cas_bounded(
            &root,
            Some(invalid),
            TreeTraversalLimits::new(32, 1024).unwrap(),
        )
        .is_err());
    }
}

fn assert_rejected_link(
    target: &OsStr,
    generated_subtree: Option<&str>,
    prepare: impl FnOnce(&Path),
) {
    let temporary = tempfile::tempdir().unwrap();
    let root = create_source_root(&temporary);
    prepare(&root);
    symlink(target, root.join("link")).unwrap();
    assert!(
        measure_source_tree_content_cas_bounded(
            &root,
            generated_subtree,
            TreeTraversalLimits::new(64, 4096).unwrap(),
        )
        .is_err(),
        "accepted unsafe source link target {:?}",
        target.as_bytes()
    );
}

fn create_fifo(path: &Path) {
    let mut command = std::process::Command::new(which::which("mkfifo").unwrap());
    command.arg(path);
    let output =
        crate::run_output_with_timeout(&mut command, 4096, std::time::Duration::from_secs(5))
            .unwrap();
    assert!(!output.timed_out && output.status.success(), "{output:?}");
}
