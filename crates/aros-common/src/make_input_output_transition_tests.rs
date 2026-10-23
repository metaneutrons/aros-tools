//! Focused tests for checking selected Make output transitions against an
//! immutable input snapshot.

use crate::{
    make_input_snapshot::{
        MakeInputReadKind, MakeInputRootSelection, MakeInputSnapshot, VerifiedMakeInputReads,
    },
    measure_tree_content_cas_bounded, Sha256Digest, TreeTraversalLimits,
};
use std::path::Path;

fn selected_digest(root: &Path) -> Sha256Digest {
    measure_tree_content_cas_bounded(root, TreeTraversalLimits::new(1024, 1024 * 1024).unwrap())
        .unwrap()
        .payload_digest_excluding(None)
}

fn snapshot(working_directory: &Path, roots: &[&Path]) -> MakeInputSnapshot {
    let selected = roots
        .iter()
        .map(|root| MakeInputRootSelection {
            root: root.to_path_buf(),
            content_sha256: selected_digest(root),
        })
        .collect::<Vec<_>>();
    MakeInputSnapshot::capture(working_directory, &selected).unwrap()
}

fn assert_refusal(result: Result<(), String>, expected: &str) {
    let error = result.expect_err("output transition should be refused");
    assert!(
        error.contains(expected),
        "expected refusal containing {expected:?}, got {error:?}"
    );
}

#[test]
fn accepts_exact_fresh_output_without_renewing_the_original_snapshot() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().join("build");
    std::fs::create_dir(&output_root).unwrap();
    std::fs::write(output_root.join("input.mk"), b"original input\n").unwrap();
    let output_root = output_root.canonicalize().unwrap();
    let before = snapshot(&output_root, &[&output_root]);
    let ((), reads) = before
        .transaction(|reader| {
            assert_eq!(
                reader.read("input.mk")?.as_deref(),
                Some("original input\n")
            );
            Ok(())
        })
        .unwrap();

    std::fs::write(output_root.join("generated.mk"), b"fresh output\n").unwrap();
    let after = snapshot(&output_root, &[&output_root]);

    before
        .verify_output_transition(
            &after,
            &output_root,
            (&[], &["generated.mk"], &[]),
            &[&reads],
        )
        .unwrap();
    assert!(before.verify().is_err());
    after.verify().unwrap();
}

#[test]
fn rejects_an_unselected_new_file() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().canonicalize().unwrap();
    let before = snapshot(&output_root, &[&output_root]);
    std::fs::write(output_root.join("selected.mk"), b"selected\n").unwrap();
    std::fs::write(output_root.join("unexpected.mk"), b"unselected\n").unwrap();
    let after = snapshot(&output_root, &[&output_root]);

    assert_refusal(
        before.verify_output_transition(&after, &output_root, (&[], &["selected.mk"], &[]), &[]),
        "outside selected effects",
    );
}

#[test]
fn accepts_selected_archive_mutation_and_fresh_member_but_not_an_empty_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("archive.a"), "old archive bytes").unwrap();
    std::fs::write(root.join("input.o"), "immutable member").unwrap();
    let before = snapshot(&root, &[&root]);
    assert!(before.observe_output_root(&root.join("unknown")).is_err());
    let ((), reads) = before
        .transaction(|reader| {
            reader.read("input.o")?;
            Ok(())
        })
        .unwrap();
    std::fs::write(root.join("archive.a"), "new archive bytes").unwrap();
    std::fs::write(root.join("member.o.gca"), "new member").unwrap();
    let after = snapshot(&root, &[&root]);
    before
        .verify_output_transition(
            &after,
            &root,
            (&["archive.a"], &["member.o.gca"], &[]),
            &[&reads],
        )
        .unwrap();
    assert_refusal(
        before.verify_output_transition(&after, &root, (&[], &[], &[]), &[&reads]),
        "outside selected effects",
    );
}

#[test]
fn explicit_regular_output_replacement_cannot_replace_a_retained_input() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("build");
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(root.join("archive.a"), "old archive").unwrap();
    std::fs::write(root.join("input.mk"), "retained source input").unwrap();
    let before = snapshot(&root, &[&root]);
    let ((), reads) = before
        .transaction(|reader| {
            reader.read("input.mk")?;
            Ok(())
        })
        .unwrap();
    let pending = temporary.path().join("pending");
    std::fs::write(&pending, "new archive").unwrap();
    std::fs::rename(&pending, root.join("archive.a")).unwrap();
    let after = before.observe_output_root(&root).unwrap();
    assert!(
        before.verify().is_err(),
        "observation must not renew the original snapshot"
    );
    before
        .verify_output_transition_with_replacements(
            &after,
            &root,
            (&[], &[], &[]),
            &["archive.a"],
            &[&reads],
        )
        .unwrap();
    assert_refusal(
        before.verify_output_transition(&after, &root, (&["archive.a"], &[], &[]), &[&reads]),
        "outside selected effects",
    );
    std::fs::write(&pending, "retained source input").unwrap();
    std::fs::rename(&pending, root.join("input.mk")).unwrap();
    let after = snapshot(&root, &[&root]);
    assert_refusal(
        before.verify_output_transition_with_replacements(
            &after,
            &root,
            (&[], &[], &[]),
            &["archive.a", "input.mk"],
            &[&reads],
        ),
        "mutate a retained input",
    );
}

#[test]
fn rejects_immutable_root_changes_and_rewritten_original_inputs() {
    for rewrite_read_input in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let output_root = temporary.path().join("build");
        let input_root = temporary.path().join("inputs");
        std::fs::create_dir(&output_root).unwrap();
        std::fs::create_dir(&input_root).unwrap();
        let original_input = input_root.join("original.mk");
        let other_input = input_root.join("other.mk");
        std::fs::write(&original_input, b"original input\n").unwrap();
        std::fs::write(&other_input, b"other input\n").unwrap();
        let output_root = output_root.canonicalize().unwrap();
        let input_root = input_root.canonicalize().unwrap();
        let original_input = original_input.canonicalize().unwrap();
        let other_input = other_input.canonicalize().unwrap();
        let before = snapshot(&output_root, &[&output_root, &input_root]);
        let reads: Option<VerifiedMakeInputReads> = if rewrite_read_input {
            Some(
                before
                    .transaction(|reader| {
                        let identity = original_input.to_str().unwrap();
                        assert_eq!(reader.read(identity)?.as_deref(), Some("original input\n"));
                        Ok(())
                    })
                    .unwrap()
                    .1,
            )
        } else {
            None
        };

        let changed_input = if rewrite_read_input {
            original_input
        } else {
            other_input
        };
        std::fs::write(changed_input, b"rewritten input\n").unwrap();
        std::fs::write(output_root.join("generated.mk"), b"fresh output\n").unwrap();
        let after = snapshot(&output_root, &[&output_root, &input_root]);

        assert_refusal(
            before.verify_output_transition(
                &after,
                &output_root,
                (&[], &["generated.mk"], &[]),
                &reads.iter().collect::<Vec<_>>(),
            ),
            "changed an immutable root",
        );
        assert!(before.verify().is_err());
        after.verify().unwrap();
    }
}

#[test]
fn refuses_a_retained_file_even_when_selected_mutable_without_byte_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().canonicalize().unwrap();
    std::fs::write(output_root.join("retained.mk"), b"retained input\n").unwrap();
    let before = snapshot(&output_root, &[&output_root]);
    let ((), reads) = before
        .transaction(|reader| {
            reader.read("retained.mk")?;
            Ok(())
        })
        .unwrap();
    let after = snapshot(&output_root, &[&output_root]);

    assert_refusal(
        before.verify_output_transition(
            &after,
            &output_root,
            (&["retained.mk"], &[], &[]),
            &[&reads],
        ),
        "attempts to mutate a retained input",
    );
}

#[test]
fn refuses_an_optional_missing_input_that_becomes_an_explicit_fresh_output() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().canonicalize().unwrap();
    let before = snapshot(&output_root, &[&output_root]);
    let ((), reads) = before
        .transaction(|reader| {
            assert_eq!(reader.read("optional.mk")?, None);
            Ok(())
        })
        .unwrap();

    std::fs::write(output_root.join("optional.mk"), b"new output\n").unwrap();
    let after = snapshot(&output_root, &[&output_root]);

    assert_refusal(
        before.verify_output_transition(
            &after,
            &output_root,
            (&[], &["optional.mk"], &[]),
            &[&reads],
        ),
        "changed retained file reads",
    );
}

#[test]
fn refuses_changed_cwd_or_root_bindings() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().join("build");
    let alternate_cwd = output_root.join("work");
    let first_input_root = temporary.path().join("inputs-a");
    let second_input_root = temporary.path().join("inputs-b");
    std::fs::create_dir_all(&alternate_cwd).unwrap();
    std::fs::create_dir(&first_input_root).unwrap();
    std::fs::create_dir(&second_input_root).unwrap();
    let output_root = output_root.canonicalize().unwrap();
    let alternate_cwd = alternate_cwd.canonicalize().unwrap();
    let first_input_root = first_input_root.canonicalize().unwrap();
    let second_input_root = second_input_root.canonicalize().unwrap();
    let before = snapshot(&output_root, &[&output_root, &first_input_root]);
    let changed_cwd = snapshot(&alternate_cwd, &[&output_root, &first_input_root]);

    assert_refusal(
        before.verify_output_transition(&changed_cwd, &output_root, (&[], &[], &[]), &[]),
        "changed input context",
    );

    std::fs::write(output_root.join("generated.mk"), b"fresh output\n").unwrap();
    let changed_root = snapshot(&output_root, &[&output_root, &second_input_root]);
    assert_refusal(
        before.verify_output_transition(
            &changed_root,
            &output_root,
            (&[], &["generated.mk"], &[]),
            &[],
        ),
        "changed root bindings",
    );
}

#[cfg(unix)]
#[test]
fn refuses_a_symbolic_link_as_a_fresh_output_file() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().canonicalize().unwrap();
    std::fs::write(output_root.join("target.mk"), b"target\n").unwrap();
    let before = snapshot(&output_root, &[&output_root]);
    symlink("target.mk", output_root.join("linked.mk")).unwrap();
    let after = snapshot(&output_root, &[&output_root]);

    assert_refusal(
        before.verify_output_transition(&after, &output_root, (&[], &["linked.mk"], &[]), &[]),
        "outside selected effects",
    );
}

#[test]
fn refuses_a_new_cwd_file_that_shadows_the_original_include_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().canonicalize().unwrap();
    let include_root = output_root.join("inc");
    std::fs::create_dir(&include_root).unwrap();
    std::fs::write(include_root.join("optional.mk"), b"included from inc\n").unwrap();
    let before = snapshot(&output_root, &[&output_root])
        .with_include_directories(&["inc".into()])
        .unwrap();
    let (included, reads) = before
        .transaction(|reader| Ok(reader.include("optional.mk")?.unwrap()))
        .unwrap();
    assert_eq!(included.identity, "inc/optional.mk");
    assert_eq!(included.text, "included from inc\n");
    assert_eq!(reads.reads()[0].kind, MakeInputReadKind::Include);
    assert_eq!(reads.reads()[0].requested, "optional.mk");
    assert_eq!(reads.reads()[0].identity, "inc/optional.mk");

    std::fs::write(output_root.join("optional.mk"), b"cwd shadow\n").unwrap();
    let after = snapshot(&output_root, &[&output_root])
        .with_include_directories(&["inc".into()])
        .unwrap();

    assert_refusal(
        before.verify_output_transition(
            &after,
            &output_root,
            (&[], &["optional.mk"], &[]),
            &[&reads],
        ),
        "changed retained file reads",
    );
}

#[test]
fn refuses_a_new_include_candidate_when_the_original_optional_include_was_absent() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().canonicalize().unwrap();
    let include_root = output_root.join("inc");
    std::fs::create_dir(&include_root).unwrap();
    let before = snapshot(&output_root, &[&output_root])
        .with_include_directories(&["inc".into()])
        .unwrap();
    let (included, reads) = before
        .transaction(|reader| reader.include("optional.mk"))
        .unwrap();
    assert!(included.is_none());
    assert_eq!(reads.reads()[0].kind, MakeInputReadKind::Include);
    assert_eq!(reads.reads()[0].requested, "optional.mk");
    assert_eq!(reads.reads()[0].identity, "optional.mk");
    assert_eq!(reads.reads()[0].content_sha256, None);

    std::fs::write(include_root.join("optional.mk"), b"new include candidate\n").unwrap();
    let after = snapshot(&output_root, &[&output_root])
        .with_include_directories(&["inc".into()])
        .unwrap();

    assert_refusal(
        before.verify_output_transition(
            &after,
            &output_root,
            (&[], &["inc/optional.mk"], &[]),
            &[&reads],
        ),
        "changed retained file reads",
    );
}

#[test]
fn accepts_an_unrelated_fresh_output_when_include_search_is_unchanged() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().canonicalize().unwrap();
    let include_root = output_root.join("inc");
    std::fs::create_dir(&include_root).unwrap();
    std::fs::write(include_root.join("optional.mk"), b"stable include\n").unwrap();
    let before = snapshot(&output_root, &[&output_root])
        .with_include_directories(&["inc".into()])
        .unwrap();
    let (included, reads) = before
        .transaction(|reader| Ok(reader.include("optional.mk")?.unwrap()))
        .unwrap();
    assert_eq!(included.identity, "inc/optional.mk");
    assert_eq!(reads.reads()[0].kind, MakeInputReadKind::Include);
    assert_eq!(reads.reads()[0].requested, "optional.mk");
    assert_eq!(reads.reads()[0].identity, "inc/optional.mk");

    std::fs::write(output_root.join("generated.mk"), b"unrelated output\n").unwrap();
    let after = snapshot(&output_root, &[&output_root])
        .with_include_directories(&["inc".into()])
        .unwrap();

    before
        .verify_output_transition(
            &after,
            &output_root,
            (&[], &["generated.mk"], &[]),
            &[&reads],
        )
        .unwrap();
}
