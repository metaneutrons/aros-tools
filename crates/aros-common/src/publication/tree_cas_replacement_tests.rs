use crate::publication::{measure_tree_content_cas, TreeContentCas};
use std::{
    fs,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

fn measure(path: &Path) -> TreeContentCas {
    measure_tree_content_cas(path).unwrap()
}

fn new_root(parent: &Path, name: &str) -> PathBuf {
    let root = parent.join(name);
    fs::create_dir(&root).unwrap();
    root
}

fn replace_from_outside(parent: &Path, root: &Path, relative: &str, bytes: &[u8]) {
    let staged = parent.join("staged-replacement");
    fs::write(&staged, bytes).unwrap();
    let target_mode = fs::metadata(root.join(relative)).unwrap().mode() & 0o7777;
    fs::set_permissions(&staged, fs::Permissions::from_mode(target_mode)).unwrap();
    fs::rename(&staged, root.join(relative)).unwrap();
}

fn identity_and_mode(path: &Path) -> (u64, u64, u32) {
    let metadata = fs::metadata(path).unwrap();
    (metadata.dev(), metadata.ino(), metadata.mode() & 0o7777)
}

fn archiver_digest(path: &Path) -> crate::Sha256Digest {
    let mut file = fs::File::open(path).unwrap();
    crate::sha256_reader(&mut file).unwrap().digest
}

fn run_archiver(program: &Path, directory: &Path, args: &[&str]) -> crate::ProcessOutput {
    let mut command = Command::new(program);
    command.args(args).env_clear().current_dir(directory);
    let output =
        crate::run_output_with_timeout(&mut command, 4096, Duration::from_secs(10)).unwrap();
    assert!(!output.timed_out, "archiver invocation timed out: {args:?}");
    assert!(
        output.status.success(),
        "archiver invocation failed: {args:?}: {}",
        output.stderr.rendered_lossy()
    );
    assert!(!output.stdout.is_truncated());
    assert!(!output.stderr.is_truncated());
    output
}

#[test]
fn accepts_atomic_regular_file_replacement_and_a_fresh_file() {
    let temporary = tempfile::tempdir().unwrap();
    let root = new_root(temporary.path(), "measured");
    let outputs = root.join("outputs");
    fs::create_dir(&outputs).unwrap();
    fs::write(outputs.join("archive.a"), b"old archive").unwrap();
    fs::set_permissions(outputs.join("archive.a"), fs::Permissions::from_mode(0o640)).unwrap();
    fs::write(root.join("input.cfg"), b"stable input").unwrap();

    let before = measure(&root);
    let before_root = identity_and_mode(&root);
    let before_outputs = identity_and_mode(&outputs);
    let before_file = fs::metadata(outputs.join("archive.a")).unwrap();
    let before_input_digest = before.regular_file_digest("input.cfg").cloned();

    replace_from_outside(
        temporary.path(),
        &root,
        "outputs/archive.a",
        b"new archive bytes",
    );
    fs::write(outputs.join("fresh.o"), b"fresh member").unwrap();
    let after = measure(&root);
    let after_file = fs::metadata(outputs.join("archive.a")).unwrap();

    assert_eq!(before_file.dev(), after_file.dev());
    assert_ne!(before_file.ino(), after_file.ino());
    assert_eq!(after_file.mode() & 0o7777, 0o640);
    assert_eq!(identity_and_mode(&root), before_root);
    assert_eq!(identity_and_mode(&outputs), before_outputs);
    assert_eq!(
        before_input_digest,
        after.regular_file_digest("input.cfg").cloned()
    );
    assert_eq!(fs::read(root.join("input.cfg")).unwrap(), b"stable input");
    assert_eq!(
        fs::read(outputs.join("archive.a")).unwrap(),
        b"new archive bytes"
    );

    assert!(before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["outputs/archive.a"],
        &["outputs/fresh.o"],
        &[],
    ));
    assert!(!before.matches_output_transition(
        &after,
        &["outputs/archive.a"],
        &["outputs/fresh.o"],
        &[],
    ));
}

#[test]
fn rejects_an_unselected_temporary_file_left_inside_the_measured_tree() {
    let temporary = tempfile::tempdir().unwrap();
    let root = new_root(temporary.path(), "measured");
    fs::write(root.join("archive.a"), b"old archive").unwrap();
    let before = measure(&root);

    fs::write(root.join(".archive.a.tmp"), b"leftover temporary file").unwrap();
    replace_from_outside(temporary.path(), &root, "archive.a", b"new archive");
    let after = measure(&root);

    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["archive.a"],
        &[],
        &[],
    ));
}

#[test]
fn rejects_changes_to_unselected_files_and_unselected_artifacts() {
    let temporary = tempfile::tempdir().unwrap();

    let root = new_root(temporary.path(), "changed-input");
    fs::write(root.join("archive.a"), b"old archive").unwrap();
    fs::write(root.join("input.cfg"), b"stable input").unwrap();
    let before = measure(&root);
    replace_from_outside(temporary.path(), &root, "archive.a", b"new archive");
    fs::write(root.join("input.cfg"), b"changed input").unwrap();
    let after = measure(&root);
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["archive.a"],
        &[],
        &[],
    ));

    let root = new_root(temporary.path(), "unselected-artifact");
    fs::write(root.join("archive.a"), b"old archive").unwrap();
    let before = measure(&root);
    replace_from_outside(temporary.path(), &root, "archive.a", b"new archive");
    fs::write(root.join("unselected.o"), b"unselected output").unwrap();
    let after = measure(&root);
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["archive.a"],
        &[],
        &[],
    ));
}

#[test]
fn rejects_empty_duplicate_overlapping_and_malformed_selections() {
    let temporary = tempfile::tempdir().unwrap();
    let root = new_root(temporary.path(), "measured");
    fs::write(root.join("archive.a"), b"archive").unwrap();
    let snapshot = measure(&root);

    assert!(!snapshot.matches_output_transition_with_replacements(&snapshot, &[], &[], &[], &[],));
    assert!(snapshot.matches_output_transition_with_replacements(
        &snapshot,
        &[],
        &["archive.a"],
        &[],
        &[],
    ));
    assert!(!snapshot.matches_output_transition_with_replacements(
        &snapshot,
        &[],
        &["archive.a", "archive.a"],
        &[],
        &[],
    ));
    assert!(!snapshot.matches_output_transition_with_replacements(
        &snapshot,
        &["archive.a"],
        &["archive.a"],
        &[],
        &[],
    ));
    assert!(!snapshot.matches_output_transition_with_replacements(
        &snapshot,
        &[],
        &["archive.a"],
        &["archive.a/member.o"],
        &[],
    ));

    for malformed in ["", "/absolute", ".", "..", "a/.", "a/..", "a/", "a//b"] {
        assert!(
            !snapshot.matches_output_transition_with_replacements(
                &snapshot,
                &[],
                &[malformed],
                &[],
                &[],
            ),
            "accepted malformed selection {malformed:?}"
        );
    }
}

#[test]
fn rejects_symbolic_link_and_directory_replacements() {
    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("link-target");
    fs::write(&target, b"target bytes").unwrap();

    let root = new_root(temporary.path(), "link-case");
    fs::write(root.join("output"), b"regular file").unwrap();
    let before = measure(&root);
    fs::remove_file(root.join("output")).unwrap();
    symlink(&target, root.join("output")).unwrap();
    let after = measure(&root);
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["output"],
        &[],
        &[],
    ));

    let root = new_root(temporary.path(), "replaced-parent-case");
    let outputs = root.join("outputs");
    fs::create_dir(&outputs).unwrap();
    fs::set_permissions(&outputs, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(outputs.join("output"), b"same regular file").unwrap();
    let before = measure(&root);
    let old_outputs = temporary.path().join("old-outputs");
    fs::rename(&outputs, &old_outputs).unwrap();
    fs::create_dir(&outputs).unwrap();
    fs::set_permissions(&outputs, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(outputs.join("output"), b"same regular file").unwrap();
    let after = measure(&root);
    assert_ne!(
        fs::metadata(&old_outputs).unwrap().ino(),
        fs::metadata(&outputs).unwrap().ino()
    );
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["outputs/output"],
        &[],
        &[],
    ));

    let root = new_root(temporary.path(), "directory-case");
    fs::write(root.join("output"), b"regular file").unwrap();
    let before = measure(&root);
    fs::remove_file(root.join("output")).unwrap();
    fs::create_dir(root.join("output")).unwrap();
    let after = measure(&root);
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["output"],
        &[],
        &[],
    ));
}

#[test]
fn rejects_deletion_and_creation_under_a_replacement_selection() {
    let temporary = tempfile::tempdir().unwrap();

    let root = new_root(temporary.path(), "deleted");
    fs::write(root.join("output"), b"old output").unwrap();
    let before = measure(&root);
    fs::remove_file(root.join("output")).unwrap();
    let after = measure(&root);
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["output"],
        &[],
        &[],
    ));

    let root = new_root(temporary.path(), "created");
    let before = measure(&root);
    fs::write(root.join("output"), b"new output").unwrap();
    let after = measure(&root);
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["output"],
        &[],
        &[],
    ));
}

#[test]
fn rejects_mode_changes_during_regular_file_replacement() {
    let temporary = tempfile::tempdir().unwrap();
    let root = new_root(temporary.path(), "measured");
    fs::write(root.join("output"), b"old output").unwrap();
    fs::set_permissions(root.join("output"), fs::Permissions::from_mode(0o644)).unwrap();
    let before = measure(&root);

    let staged = temporary.path().join("staged-replacement");
    fs::write(&staged, b"new output").unwrap();
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(&staged, root.join("output")).unwrap();
    let after = measure(&root);

    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["output"],
        &[],
        &[],
    ));

    let root = new_root(temporary.path(), "parent-mode-case");
    let outputs = root.join("outputs");
    fs::create_dir(&outputs).unwrap();
    fs::set_permissions(&outputs, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(outputs.join("output"), b"old output").unwrap();
    fs::set_permissions(outputs.join("output"), fs::Permissions::from_mode(0o640)).unwrap();
    let before = measure(&root);
    replace_from_outside(temporary.path(), &root, "outputs/output", b"new output");
    fs::set_permissions(&outputs, fs::Permissions::from_mode(0o700)).unwrap();
    let after = measure(&root);
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["outputs/output"],
        &[],
        &[],
    ));
}

#[test]
fn rejects_replacement_of_the_measured_root() {
    let temporary = tempfile::tempdir().unwrap();
    let root = new_root(temporary.path(), "root");
    let replacement = new_root(temporary.path(), "replacement");
    fs::write(root.join("output"), b"same output").unwrap();
    fs::write(replacement.join("output"), b"same output").unwrap();
    let before = measure(&root);

    let old_root = temporary.path().join("old-root");
    fs::rename(&root, &old_root).unwrap();
    fs::rename(&replacement, &root).unwrap();
    let after = measure(&root);

    let before_identity = fs::metadata(&old_root).unwrap();
    let after_identity = fs::metadata(&root).unwrap();
    assert_eq!(before_identity.dev(), after_identity.dev());
    assert_ne!(before_identity.ino(), after_identity.ino());
    assert!(!before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["output"],
        &[],
        &[],
    ));
}

#[test]
#[ignore = "requires explicit AROS_TEST_ARCHIVER and AROS_TEST_ARCHIVER_SHA256"]
fn actual_archiver_can_atomically_replace_an_archive_while_adding_a_member() {
    let program = PathBuf::from(
        std::env::var_os("AROS_TEST_ARCHIVER")
            .expect("set AROS_TEST_ARCHIVER to a canonical absolute archiver path"),
    );
    assert!(program.is_absolute(), "archiver path must be absolute");
    assert_eq!(
        fs::canonicalize(&program).unwrap(),
        program,
        "AROS_TEST_ARCHIVER must already be canonical"
    );
    let expected_digest = crate::Sha256Digest::parse(
        &std::env::var("AROS_TEST_ARCHIVER_SHA256")
            .expect("set AROS_TEST_ARCHIVER_SHA256 to the archiver's SHA-256"),
    )
    .unwrap();
    assert_eq!(archiver_digest(&program), expected_digest);

    let temporary = tempfile::tempdir().unwrap();
    fs::write(temporary.path().join("first.o"), b"first member fixture").unwrap();
    fs::write(temporary.path().join("second.o"), b"second member fixture").unwrap();

    run_archiver(&program, temporary.path(), &["rcs", "archive.a", "first.o"]);
    let before = measure(temporary.path());
    let before_archive = fs::metadata(temporary.path().join("archive.a")).unwrap();

    run_archiver(&program, temporary.path(), &["rs", "archive.a", "second.o"]);
    let after = measure(temporary.path());
    let after_archive = fs::metadata(temporary.path().join("archive.a")).unwrap();

    assert_eq!(before_archive.dev(), after_archive.dev());
    assert_ne!(before_archive.ino(), after_archive.ino());
    assert!(!before.matches_output_transition(&after, &["archive.a"], &[], &[],));
    assert!(before.matches_output_transition_with_replacements(
        &after,
        &[],
        &["archive.a"],
        &[],
        &[],
    ));

    let listing = run_archiver(&program, temporary.path(), &["t", "archive.a"]);
    let members: Vec<_> = std::str::from_utf8(listing.stdout.exact_bytes().unwrap())
        .unwrap()
        .lines()
        .collect();
    assert_eq!(members, vec!["first.o", "second.o"]);
    assert_eq!(archiver_digest(&program), expected_digest);
}
