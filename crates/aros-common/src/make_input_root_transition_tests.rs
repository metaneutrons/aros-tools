//! Focused tests for replaying reads across selected complete-root transitions.

use super::{measure, MakeInputRootSelection, MakeInputSnapshot, Root, RootMeasurement, MAX_ROOTS};
use crate::{local_source::LocalSourceIdentity, sha256_bytes, TreeContentCas};
use std::path::{Path, PathBuf};

fn tree(root: &Path) -> TreeContentCas {
    measure(root).unwrap()
}

fn snapshot(working_directory: &Path, roots: &[&Path]) -> MakeInputSnapshot {
    let selected = roots
        .iter()
        .map(|root| MakeInputRootSelection {
            root: root.to_path_buf(),
            content_sha256: tree(root).payload_digest_excluding(None),
        })
        .collect::<Vec<_>>();
    MakeInputSnapshot::capture(working_directory, &selected).unwrap()
}

#[test]
fn accepts_output_tree_replacement_and_preserves_all_other_baselines_and_context() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().join("build");
    let source_root = temporary.path().join("source");
    let compiler_root = temporary.path().join("compiler");
    let evidence_root = temporary.path().join("evidence");
    for root in [&output_root, &source_root, &compiler_root, &evidence_root] {
        std::fs::create_dir(root).unwrap();
    }
    let include_root = output_root.join("inc");
    std::fs::create_dir(&include_root).unwrap();
    std::fs::write(include_root.join("fragment.mk"), b"stable include\n").unwrap();
    std::fs::write(source_root.join("source.c"), b"stable source\n").unwrap();
    std::fs::write(compiler_root.join("compiler"), b"stable compiler\n").unwrap();
    std::fs::write(evidence_root.join("evidence.json"), b"stable evidence\n").unwrap();

    let output_root = output_root.canonicalize().unwrap();
    let source_root = source_root.canonicalize().unwrap();
    let compiler_root = compiler_root.canonicalize().unwrap();
    let evidence_root = evidence_root.canonicalize().unwrap();
    let before = snapshot(
        &output_root,
        &[&output_root, &source_root, &compiler_root, &evidence_root],
    )
    .with_include_directories(&["inc".into()])
    .unwrap();
    let output_baseline = tree(&output_root).payload_digest_excluding(None);
    let source_baseline = tree(&source_root).payload_digest_excluding(None);
    let compiler_baseline = tree(&compiler_root).payload_digest_excluding(None);
    let evidence_baseline = tree(&evidence_root).payload_digest_excluding(None);
    let (included, reads) = before
        .transaction(|reader| Ok(reader.include("fragment.mk")?.unwrap()))
        .unwrap();
    assert_eq!(included.identity, "inc/fragment.mk");
    assert_eq!(reads.reads().len(), 1);

    std::fs::write(output_root.join("generated.mk"), b"fresh output\n").unwrap();
    let expected_output_tree = tree(&output_root);
    before
        .verify_read_groups_with_complete_root_transitions(
            &[(&output_root, &expected_output_tree)],
            &[&reads],
        )
        .unwrap();

    assert!(before.matches_root_selection(&output_root, &output_baseline));
    assert!(before.matches_root_selection(&source_root, &source_baseline));
    assert!(before.matches_root_selection(&compiler_root, &compiler_baseline));
    assert!(before.matches_root_selection(&evidence_root, &evidence_baseline));
    assert!(before.verify().is_err());
}

#[test]
fn refuses_changed_retained_file_and_new_optional_include() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().join("build");
    std::fs::create_dir(&output_root).unwrap();
    std::fs::write(output_root.join("input.mk"), b"original input\n").unwrap();
    let output_root = output_root.canonicalize().unwrap();
    let before = snapshot(&output_root, &[&output_root]);
    let ((), reads) = before
        .transaction(|reader| {
            reader.read("input.mk")?;
            Ok(())
        })
        .unwrap();

    std::fs::write(output_root.join("input.mk"), b"changed input\n").unwrap();
    std::fs::write(output_root.join("generated.mk"), b"fresh output\n").unwrap();
    let expected_output_tree = tree(&output_root);
    assert!(before
        .verify_read_groups_with_complete_root_transitions(
            &[(&output_root, &expected_output_tree)],
            &[&reads],
        )
        .is_err());

    let include_root = output_root.join("inc");
    std::fs::create_dir(&include_root).unwrap();
    let before_include = snapshot(&output_root, &[&output_root])
        .with_include_directories(&["inc".into()])
        .unwrap();
    let (included, include_reads) = before_include
        .transaction(|reader| reader.include("optional.mk"))
        .unwrap();
    assert!(included.is_none());

    std::fs::write(include_root.join("optional.mk"), b"new include candidate\n").unwrap();
    let expected_output_tree = tree(&output_root);
    assert!(before_include
        .verify_read_groups_with_complete_root_transitions(
            &[(&output_root, &expected_output_tree)],
            &[&include_reads],
        )
        .is_err());
}

#[test]
fn refuses_wrong_unknown_duplicate_noncanonical_and_local_source_replacements() {
    let temporary = tempfile::tempdir().unwrap();
    let output_root = temporary.path().join("build");
    let other_root = temporary.path().join("other");
    let nested = temporary.path().join("nested");
    std::fs::create_dir(&output_root).unwrap();
    std::fs::create_dir(&other_root).unwrap();
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(output_root.join("input.mk"), b"input\n").unwrap();
    std::fs::write(other_root.join("foreign.mk"), b"foreign tree\n").unwrap();
    let output_root = output_root.canonicalize().unwrap();
    let other_root = other_root.canonicalize().unwrap();
    let before = snapshot(&output_root, &[&output_root]);
    let expected_tree = tree(&output_root);
    let foreign_tree = tree(&other_root);

    assert!(before
        .verify_read_groups_with_complete_root_transitions(&[], &[])
        .is_err());
    let too_many = vec![(output_root.as_path(), &expected_tree); MAX_ROOTS + 1];
    assert!(before
        .verify_read_groups_with_complete_root_transitions(&too_many, &[])
        .is_err());
    assert!(before
        .verify_read_groups_with_complete_root_transitions(
            &[(other_root.as_path(), &foreign_tree)],
            &[],
        )
        .is_err());
    assert!(before
        .verify_read_groups_with_complete_root_transitions(
            &[
                (output_root.as_path(), &expected_tree),
                (output_root.as_path(), &expected_tree),
            ],
            &[],
        )
        .is_err());
    let noncanonical_root = nested.join("..").join("build");
    assert!(before
        .verify_read_groups_with_complete_root_transitions(
            &[(noncanonical_root.as_path(), &expected_tree)],
            &[],
        )
        .is_err());

    let source_identity = LocalSourceIdentity {
        schema: "aros-local-source-v1".into(),
        head_baseline: "0".repeat(40),
        submodules_sha256: sha256_bytes(b""),
        generated_subtree: "build/fixture".into(),
        content_sha256: expected_tree.payload_digest_excluding(None),
        entry_count: expected_tree.entry_count(),
        regular_file_bytes: expected_tree.regular_file_bytes().unwrap(),
    };
    let local_source = MakeInputSnapshot {
        roots: vec![Root {
            path: output_root.clone(),
            tree: expected_tree.clone(),
            measurement: RootMeasurement::LocalSource(source_identity),
        }]
        .into(),
        working_directory: output_root.clone(),
        include_directories: None,
    };
    assert!(local_source
        .verify_read_groups_with_complete_root_transitions(
            &[(output_root.as_path(), &expected_tree)],
            &[],
        )
        .is_err());

    assert!(before
        .verify_read_groups_with_complete_root_transitions(
            &[(output_root.as_path(), &foreign_tree)],
            &[],
        )
        .is_err());
}

#[test]
fn refuses_replacement_that_invalidates_retained_cwd_or_include_directory() {
    for change_cwd in [true, false] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let directory = root.join("selected-directory");
        std::fs::create_dir(&directory).unwrap();
        let before = if change_cwd {
            snapshot(&directory, &[&root])
        } else {
            snapshot(&root, &[&root])
                .with_include_directories(&["selected-directory".into()])
                .unwrap()
        };
        std::fs::remove_dir(&directory).unwrap();
        std::fs::write(&directory, b"not a directory\n").unwrap();
        let changed = tree(&root);
        let error = before
            .verify_read_groups_with_complete_root_transitions(&[(&root, &changed)], &[])
            .unwrap_err();
        assert!(error.contains("captured real directory"));
    }
}

#[test]
fn refuses_drift_in_untouched_source_compiler_or_evidence_roots() {
    for changed_root_index in 0..3 {
        let temporary = tempfile::tempdir().unwrap();
        let output_root = temporary.path().join("build");
        let source_root = temporary.path().join("source");
        let compiler_root = temporary.path().join("compiler");
        let evidence_root = temporary.path().join("evidence");
        for root in [&output_root, &source_root, &compiler_root, &evidence_root] {
            std::fs::create_dir(root).unwrap();
        }
        for (root, name, contents) in [
            (&source_root, "source.c", b"source baseline".as_slice()),
            (&compiler_root, "compiler", b"compiler baseline".as_slice()),
            (
                &evidence_root,
                "evidence.json",
                b"evidence baseline".as_slice(),
            ),
        ] {
            std::fs::write(root.join(name), contents).unwrap();
        }
        let output_root = output_root.canonicalize().unwrap();
        let source_root = source_root.canonicalize().unwrap();
        let compiler_root = compiler_root.canonicalize().unwrap();
        let evidence_root = evidence_root.canonicalize().unwrap();
        let before = snapshot(
            &output_root,
            &[&output_root, &source_root, &compiler_root, &evidence_root],
        );
        let untouched_roots: [&PathBuf; 3] = [&source_root, &compiler_root, &evidence_root];
        let changed_root = untouched_roots[changed_root_index];
        std::fs::write(changed_root.join("changed"), b"drift\n").unwrap();
        std::fs::write(output_root.join("generated.mk"), b"fresh output\n").unwrap();
        let expected_output_tree = tree(&output_root);

        assert!(before
            .verify_read_groups_with_complete_root_transitions(
                &[(output_root.as_path(), &expected_output_tree)],
                &[],
            )
            .is_err());
    }
}
