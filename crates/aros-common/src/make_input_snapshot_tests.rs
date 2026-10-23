//! Bounded transaction tests for caller-selected Make input snapshots.

use crate::{
    local_source::LocalSourceIdentity,
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    measure_tree_content_cas_bounded, sha256_bytes, Sha256Digest, TreeTraversalLimits,
};
use std::{path::Path, process::Command};

fn selected_digest(root: &Path) -> Sha256Digest {
    measure_tree_content_cas_bounded(root, TreeTraversalLimits::new(1024, 1024 * 1024).unwrap())
        .unwrap()
        .payload_digest_excluding(None)
}

fn snapshot(root: &Path, working_directory: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        working_directory,
        &[MakeInputRootSelection {
            root: root.to_path_buf(),
            content_sha256: selected_digest(root),
        }],
    )
    .unwrap()
}

fn local_source_checkout() -> tempfile::TempDir {
    fn git(root: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let temporary = tempfile::tempdir().unwrap();
    git(temporary.path(), &["init", "-q"]);
    git(
        temporary.path(),
        &["config", "user.name", "Make input test"],
    );
    git(
        temporary.path(),
        &["config", "user.email", "make-input@example.invalid"],
    );
    std::fs::write(temporary.path().join("input.c"), b"original source\n").unwrap();
    std::fs::write(temporary.path().join(".gitignore"), b"build/\n").unwrap();
    git(temporary.path(), &["add", "input.c", ".gitignore"]);
    git(temporary.path(), &["commit", "-qm", "Make input fixture"]);
    std::fs::create_dir(temporary.path().join("build")).unwrap();
    temporary
}

#[test]
fn capture_requires_the_independently_selected_root_digest() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("input.mk"), b"selected content\n").unwrap();

    let error = MakeInputSnapshot::capture(
        &root,
        &[MakeInputRootSelection {
            root: root.clone(),
            content_sha256: sha256_bytes(b"a different selected root"),
        }],
    )
    .unwrap_err();

    assert!(error.contains("independently selected content"));
}

#[test]
fn local_source_capture_keeps_the_original_identity_and_partitions_exact_build_output() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let build_root = source_root.join("build/fixture");
    std::fs::create_dir(&build_root).unwrap();
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let original_source_digest = identity.content_sha256.clone();
    std::fs::write(build_root.join("output.mk"), b"generated make input\n").unwrap();
    let build_digest = selected_digest(&build_root);
    let working_directory = build_root.canonicalize().unwrap();
    let snapshot = MakeInputSnapshot::capture_with_local_source(
        &working_directory,
        &source_root,
        &identity,
        &[MakeInputRootSelection {
            root: build_root.clone(),
            content_sha256: build_digest.clone(),
        }],
    )
    .unwrap();

    assert!(snapshot.matches_root_selection(&source_root, &original_source_digest));
    assert!(snapshot.matches_root_selection(&build_root, &build_digest));
    let (contents, reads) = snapshot
        .transaction(|reader| {
            Ok((
                reader.read("../../input.c")?.unwrap(),
                reader.read("output.mk")?.unwrap(),
            ))
        })
        .unwrap();
    assert_eq!(
        contents,
        ("original source\n".into(), "generated make input\n".into())
    );
    assert_eq!(reads.reads().len(), 2);
    assert_eq!(reads.reads()[0].requested, "../../input.c");
    assert_eq!(reads.reads()[1].requested, "output.mk");
}

#[test]
fn local_source_transition_replay_preserves_identity_and_exact_generated_partition() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let build_root = source_root.join("build/fixture");
    std::fs::create_dir(&build_root).unwrap();
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let selected = MakeInputSnapshot::capture_with_local_source(
        &build_root,
        &source_root,
        &identity,
        &[MakeInputRootSelection {
            root: build_root.clone(),
            content_sha256: selected_digest(&build_root),
        }],
    )
    .unwrap();
    let (_, reads) = selected
        .transaction(|reader| reader.read("../../input.c"))
        .unwrap();
    std::fs::write(build_root.join("generated.mk"), b"fresh output\n").unwrap();
    let output = measure_tree_content_cas_bounded(
        &build_root,
        TreeTraversalLimits::new(1024, 1024 * 1024).unwrap(),
    )
    .unwrap();
    selected
        .verify_read_groups_with_complete_root_transitions(&[(&build_root, &output)], &[&reads])
        .unwrap();
    assert!(selected.matches_root_selection(&source_root, &identity.content_sha256));
    assert!(
        selected.verify().is_err(),
        "the original output baseline is unchanged"
    );

    std::fs::write(source_root.join("input.c"), b"changed source\n").unwrap();
    let error = selected
        .verify_read_groups_with_complete_root_transitions(&[(&build_root, &output)], &[&reads])
        .unwrap_err();
    assert!(error.contains("actual local source inputs differ from the pre-build snapshot"));
}

#[test]
fn local_source_capture_rejects_a_wrong_original_identity() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let mut wrong_identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    wrong_identity.content_sha256 = sha256_bytes(b"not the selected local source");

    let error = MakeInputSnapshot::capture_with_local_source(
        &source_root,
        &source_root,
        &wrong_identity,
        &[],
    )
    .unwrap_err();

    assert!(error.contains("pre-build snapshot"));
}

#[test]
fn local_source_snapshot_rejects_content_and_payload_preserving_metadata_drift() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let source_file = source_root.join("input.c");
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let snapshot =
        MakeInputSnapshot::capture_with_local_source(&source_root, &source_root, &identity, &[])
            .unwrap();

    std::fs::write(&source_file, b"changed source\n").unwrap();
    assert!(snapshot.verify().is_err());

    std::fs::write(&source_file, b"original source\n").unwrap();
    #[cfg(unix)]
    {
        let snapshot = MakeInputSnapshot::capture_with_local_source(
            &source_root,
            &source_root,
            &identity,
            &[],
        )
        .unwrap();
        let original_digest = identity.content_sha256;
        let permissions = std::fs::metadata(&source_file).unwrap().permissions();
        let replacement = source_root.join("replacement");
        std::fs::write(&replacement, b"original source\n").unwrap();
        std::fs::set_permissions(&replacement, permissions).unwrap();
        std::fs::rename(replacement, &source_file).unwrap();
        let recaptured = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
        assert_eq!(recaptured.content_sha256, original_digest);
        assert!(snapshot.verify().is_err());
    }
}

#[test]
fn local_source_exclusions_refuse_missing_probes_and_incomplete_directory_lists() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let snapshot =
        MakeInputSnapshot::capture_with_local_source(&source_root, &source_root, &identity, &[])
            .unwrap();

    assert!(snapshot
        .transaction(|reader| reader.read(".git/absent"))
        .is_err());
    assert!(snapshot
        .transaction(|reader| reader.entry_kind(".git/absent"))
        .is_err());
    assert!(snapshot
        .transaction(|reader| reader.wildcard_entry_kind("build/fixture/absent"))
        .is_err());
    let mut missing_root_error = None;
    let missing_root = snapshot.transaction(|reader| {
        missing_root_error = Some(reader.wildcard_entry_kind("absent").unwrap_err());
        Ok(())
    });
    assert!(missing_root.is_err());
    assert!(missing_root_error.as_deref().is_some_and(|error| {
        error.contains("Make query directory differs from captured selection")
    }));
    assert!(snapshot
        .transaction(|reader| reader.directory_entries("."))
        .is_err());
}

#[test]
fn local_source_wildcard_can_traverse_a_known_child_beneath_excluded_git_metadata() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let genmodule = source_root.join("tools/genmodule");
    std::fs::create_dir_all(&genmodule).unwrap();
    std::fs::write(genmodule.join("genmodule.c"), b"source\n").unwrap();
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let snapshot =
        MakeInputSnapshot::capture_with_local_source(&source_root, &source_root, &identity, &[])
            .unwrap();

    let ((kind, entries, contents, absent), reads) = snapshot
        .transaction(|reader| {
            Ok((
                reader.wildcard_entry_kind("tools/genmodule")?,
                reader.directory_entries("tools/genmodule")?,
                reader.read("tools/genmodule/genmodule.c")?,
                reader.wildcard_entry_kind("tools/genmodule/absent.h")?,
            ))
        })
        .unwrap();

    assert_eq!(kind, Some(crate::TreeContentEntryKind::Directory));
    assert_eq!(
        entries,
        Some(vec![(
            "genmodule.c".into(),
            crate::TreeContentEntryKind::RegularFile,
        )])
    );
    assert_eq!(contents.as_deref(), Some("source\n"));
    assert_eq!(absent, None);
    assert_eq!(reads.reads().len(), 1);
}

#[test]
fn local_source_generated_subtree_exclusion_keeps_exact_build_sibling_queryable() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let generated = source_root.join("build/fixture");
    let sibling = source_root.join("build/fixture-sibling");
    std::fs::create_dir(&generated).unwrap();
    std::fs::write(generated.join("generated.mk"), b"generated\n").unwrap();
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(sibling.join("sibling.mk"), b"sibling\n").unwrap();
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let snapshot =
        MakeInputSnapshot::capture_with_local_source(&source_root, &source_root, &identity, &[])
            .unwrap();

    let ((kind, entries, contents), reads) = snapshot
        .transaction(|reader| {
            Ok((
                reader.wildcard_entry_kind("build/fixture-sibling")?,
                reader.directory_entries("build/fixture-sibling")?,
                reader.read("build/fixture-sibling/sibling.mk")?,
            ))
        })
        .unwrap();

    assert_eq!(kind, Some(crate::TreeContentEntryKind::Directory));
    assert_eq!(
        entries,
        Some(vec![(
            "sibling.mk".into(),
            crate::TreeContentEntryKind::RegularFile,
        )])
    );
    assert_eq!(contents.as_deref(), Some("sibling\n"));
    assert_eq!(reads.reads().len(), 1);
}

#[test]
fn local_source_known_child_traversal_refuses_nonexcluded_parent_drift() {
    for remove_known_child in [false, true] {
        let temporary = local_source_checkout();
        let source_root = temporary.path().canonicalize().unwrap();
        let genmodule = source_root.join("tools/genmodule");
        std::fs::create_dir_all(&genmodule).unwrap();
        std::fs::write(genmodule.join("genmodule.c"), b"source\n").unwrap();
        let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
        let snapshot = MakeInputSnapshot::capture_with_local_source(
            &source_root,
            &source_root,
            &identity,
            &[],
        )
        .unwrap();

        let mut query_error = None;
        let result = snapshot.transaction(|reader| {
            if remove_known_child {
                std::fs::remove_dir_all(&genmodule).map_err(|error| error.to_string())?;
            } else {
                std::fs::write(source_root.join("unexpected"), b"unselected\n")
                    .map_err(|error| error.to_string())?;
            }
            query_error = Some(reader.wildcard_entry_kind("tools/genmodule").unwrap_err());
            Ok(())
        });

        assert!(result.is_err());
        assert!(query_error.as_deref().is_some_and(|error| {
            error.contains("Make query directory differs from captured selection")
        }));
    }
}

#[cfg(unix)]
#[test]
fn local_source_known_child_traversal_does_not_follow_a_changed_symlink_ancestor() {
    use std::os::unix::fs::symlink;

    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let tools = source_root.join("tools");
    std::fs::create_dir_all(tools.join("genmodule")).unwrap();
    std::fs::write(tools.join("genmodule/genmodule.c"), b"source\n").unwrap();
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let snapshot =
        MakeInputSnapshot::capture_with_local_source(&source_root, &source_root, &identity, &[])
            .unwrap();

    let mut query_error = None;
    let result = snapshot.transaction(|reader| {
        std::fs::rename(&tools, source_root.join(".git/tools-held"))
            .map_err(|error| error.to_string())?;
        symlink(".", &tools).map_err(|error| error.to_string())?;
        query_error = Some(reader.wildcard_entry_kind("tools/genmodule").unwrap_err());
        Ok(())
    });

    assert!(result.is_err());
    assert!(query_error
        .as_deref()
        .is_some_and(|error| error.contains("cannot safely list Make query directory")));
}

#[test]
fn local_source_directory_queries_do_not_hide_excluded_physical_names() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let generated = source_root.join("build/fixture");
    std::fs::create_dir(&generated).unwrap();
    std::fs::write(generated.join("output.mk"), b"generated\n").unwrap();
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let snapshot =
        MakeInputSnapshot::capture_with_local_source(&source_root, &source_root, &identity, &[])
            .unwrap();

    assert!(snapshot.verify().is_ok());
    assert!(snapshot
        .transaction(|reader| reader.directory_entries("build"))
        .is_err());
    let missing = snapshot
        .transaction(|reader| reader.wildcard_entry_kind("build/not-present"))
        .unwrap_err();
    assert!(missing.contains("Make query directory differs from captured selection"));
    assert!(snapshot
        .transaction(|reader| reader.read("build/fixture/output.mk"))
        .is_err());
}

#[test]
fn local_source_capture_rejects_parent_sibling_git_and_linked_overlaps() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let sibling = source_root.join("build/fixture-sibling");
    std::fs::create_dir(&sibling).unwrap();
    std::fs::write(sibling.join("output.mk"), b"sibling\n").unwrap();
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();

    for selected_root in [source_root.join("build"), sibling, source_root.join(".git")] {
        let error = MakeInputSnapshot::capture_with_local_source(
            &source_root,
            &source_root,
            &identity,
            &[MakeInputRootSelection {
                content_sha256: selected_digest(&selected_root),
                root: selected_root,
            }],
        )
        .unwrap_err();
        assert!(error.contains("overlap"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        // Git metadata is outside the source identity, so this linked path
        // does not alter the retained LocalSourceIdentity before root checks.
        let linked = source_root.join(".git/linked-selected-root");
        symlink(source_root.join(".git/objects"), &linked).unwrap();
        let error = MakeInputSnapshot::capture_with_local_source(
            &source_root,
            &source_root,
            &identity,
            &[MakeInputRootSelection {
                root: linked,
                content_sha256: sha256_bytes(b"not used for a linked root"),
            }],
        )
        .unwrap_err();
        assert!(error.contains("not canonical"));
    }
}

#[test]
fn reads_preserve_exact_spellings_order_and_old_file_arguments() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("first.mk"), b"first\n").unwrap();
    std::fs::write(root.join("second.mk"), b"second\n").unwrap();
    let snapshot = snapshot(&root, &root);

    let ((), verified) = snapshot
        .transaction(|reader| {
            assert_eq!(reader.read("first.mk")?, Some("first\n".into()));
            assert_eq!(reader.read("second.mk")?, Some("second\n".into()));
            assert_eq!(reader.read("first.mk")?, Some("first\n".into()));
            assert_eq!(reader.read("./first.mk")?, Some("first\n".into()));
            Ok(())
        })
        .unwrap();

    let reads = verified.reads();
    assert_eq!(reads.len(), 4);
    assert_eq!(
        reads
            .iter()
            .map(|read| (read.requested.as_str(), read.identity.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("first.mk", "first.mk"),
            ("second.mk", "second.mk"),
            ("first.mk", "first.mk"),
            ("./first.mk", "./first.mk"),
        ]
    );
    assert_eq!(
        reads
            .iter()
            .map(|read| read.path.clone())
            .collect::<Vec<_>>(),
        vec![
            root.join("first.mk"),
            root.join("second.mk"),
            root.join("first.mk"),
            root.join("first.mk"),
        ]
    );
    assert_eq!(reads[0].path, reads[2].path);
    assert_eq!(reads[0].content_sha256, Some(sha256_bytes(b"first\n")));
    assert_eq!(reads[1].content_sha256, Some(sha256_bytes(b"second\n")));
    assert_eq!(
        verified.old_file_arguments(),
        vec![
            "--old-file=first.mk".to_owned(),
            "--old-file=second.mk".to_owned(),
            "--old-file=./first.mk".to_owned(),
        ]
    );
}

#[test]
fn root_selection_comparison_is_exact_and_does_not_adopt_later_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("input.mk"), b"selected\n").unwrap();
    let digest = selected_digest(&root);
    let snapshot = snapshot(&root, &root);
    assert!(snapshot.matches_root_selection(&root, &digest));
    assert!(!snapshot.matches_root_selection(&root, &sha256_bytes(b"foreign")));
    assert!(!snapshot.matches_root_selection(root.parent().unwrap(), &digest));
    assert!(!snapshot
        .matches_root_selection(&root.join("nested"), &selected_digest(&root.join("nested"))));
    assert!(!snapshot.matches_root_selection(&root.join("."), &digest));
    std::fs::write(root.join("input.mk"), b"changed\n").unwrap();
    // The comparison reports the retained selection, not live verification.
    assert!(snapshot.matches_root_selection(&root, &digest));
    assert!(!snapshot.matches_root_selection(&root, &selected_digest(&root)));
    assert!(snapshot.transaction(|_| Ok(())).is_err());
}

#[test]
fn byte_reads_preserve_arbitrary_bytes_identity_and_digest() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let bytes = [b'#', b' ', 0xa9, 0xff, b'\n'];
    std::fs::write(root.join("raw.mk"), bytes).unwrap();
    let snapshot = snapshot(&root, &root);

    let (input, verified) = snapshot
        .transaction(|reader| reader.read_bytes("./raw.mk"))
        .unwrap();
    let input = input.unwrap();
    assert_eq!(input.identity, "./raw.mk");
    assert_eq!(input.bytes, bytes);
    assert_eq!(
        verified.reads().len(),
        1,
        "one probe yields one read record"
    );
    assert_eq!(verified.reads()[0].requested, "./raw.mk");
    assert_eq!(verified.reads()[0].identity, "./raw.mk");
    assert_eq!(
        verified.reads()[0].content_sha256,
        Some(sha256_bytes(&bytes))
    );

    let diagnostic = format!("{input:?}");
    assert!(diagnostic.contains("bytes_len"));
    assert!(!diagnostic.contains("raw.mk"));
    assert!(!diagnostic.contains("a9"));

    let result = snapshot.transaction(|reader| {
        let error = reader.read("raw.mk").unwrap_err();
        assert!(error.contains("not UTF-8"));
        Ok(())
    });
    assert!(result.unwrap_err().contains("refused read"));
}

#[test]
fn include_search_requires_a_binding_and_uses_its_order() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("first")).unwrap();
    std::fs::create_dir(root.join("second")).unwrap();
    std::fs::write(root.join("first/shared.mk"), b"from first\n").unwrap();
    std::fs::write(root.join("second/shared.mk"), b"from second\n").unwrap();
    let unbound = snapshot(&root, &root);

    let mut observed_error = None;
    let unbound_result = unbound.transaction(|reader| {
        observed_error = Some(reader.include("optional.mk").unwrap_err());
        Ok(())
    });
    assert!(unbound_result.is_err());
    assert!(observed_error
        .as_deref()
        .unwrap()
        .contains("not been independently bound"));

    let explicitly_empty = unbound.clone().with_include_directories(&[]).unwrap();
    let (absent, absent_reads) = explicitly_empty
        .transaction(|reader| reader.include("optional.mk"))
        .unwrap();
    assert_eq!(absent, None);
    assert_eq!(absent_reads.reads().len(), 1);
    assert_eq!(absent_reads.reads()[0].identity, "optional.mk");
    assert_eq!(absent_reads.reads()[0].content_sha256, None);

    let first_then_second = vec!["first".to_owned(), "second".to_owned()];
    let ordered = unbound
        .clone()
        .with_include_directories(&first_then_second)
        .unwrap();
    let (first, first_reads) = ordered
        .transaction(|reader| reader.include("shared.mk"))
        .unwrap();
    let first = first.unwrap();
    assert_eq!(first.identity, "first/shared.mk");
    assert_eq!(first.text, "from first\n");
    assert_eq!(first_reads.reads()[0].requested, "shared.mk");
    assert_eq!(first_reads.reads()[0].identity, "first/shared.mk");

    let second_then_first = vec!["second".to_owned(), "first".to_owned()];
    let reversed = unbound
        .with_include_directories(&second_then_first)
        .unwrap();
    let (second, _) = reversed
        .transaction(|reader| reader.include("shared.mk"))
        .unwrap();
    let second = second.unwrap();
    assert_eq!(second.identity, "second/shared.mk");
    assert_eq!(second.text, "from second\n");
}

#[test]
fn byte_includes_keep_search_identity_absence_and_strict_text_wrapper() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("first")).unwrap();
    let bytes = [b'#', b' ', 0xa9, b'\n', b'V', b'=', b'x', b'\n'];
    std::fs::write(root.join("first/raw.mk"), bytes).unwrap();
    let base = snapshot(&root, &root);
    let unbound_result = base.transaction(|reader| {
        let error = reader.include_bytes("raw.mk").unwrap_err();
        assert!(error.contains("not been independently bound"));
        Ok(())
    });
    assert!(unbound_result.unwrap_err().contains("refused read"));

    let search = vec!["first".to_owned()];
    let bound = base.clone().with_include_directories(&search).unwrap();
    let (included, verified) = bound
        .transaction(|reader| reader.include_bytes("raw.mk"))
        .unwrap();
    let included = included.unwrap();
    assert_eq!(included.identity, "first/raw.mk");
    assert_eq!(included.bytes, bytes);
    assert_eq!(verified.reads().len(), 1);
    assert_eq!(verified.reads()[0].requested, "raw.mk");
    assert_eq!(verified.reads()[0].identity, "first/raw.mk");
    assert_eq!(
        verified.reads()[0].content_sha256,
        Some(sha256_bytes(&bytes))
    );

    let strict_result = bound.transaction(|reader| {
        let error = reader.include("raw.mk").unwrap_err();
        assert!(error.contains("not UTF-8"));
        Ok(())
    });
    assert!(strict_result.unwrap_err().contains("refused read"));

    let no_search = base.with_include_directories(&[]).unwrap();
    let (missing, verified) = no_search
        .transaction(|reader| reader.include_bytes("optional.mk"))
        .unwrap();
    assert!(missing.is_none());
    assert_eq!(verified.reads().len(), 1);
    assert_eq!(verified.reads()[0].requested, "optional.mk");
    assert_eq!(verified.reads()[0].identity, "optional.mk");
    assert_eq!(verified.reads()[0].content_sha256, None);
}

#[cfg(unix)]
#[test]
fn paths_refuse_symlink_parent_traversal_but_allow_a_real_parent() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("actualdir")).unwrap();
    std::fs::write(root.join("file.mk"), b"safe\n").unwrap();
    symlink("actualdir", root.join("linkdir")).unwrap();
    let snapshot = snapshot(&root, &root);

    let ((), verified) = snapshot
        .transaction(|reader| {
            assert_eq!(reader.read("actualdir/../file.mk")?, Some("safe\n".into()));
            Ok(())
        })
        .unwrap();
    assert_eq!(verified.reads()[0].identity, "actualdir/../file.mk");
    assert_eq!(verified.reads()[0].path, root.join("file.mk"));

    let mut observed_error = None;
    let result = snapshot.transaction(|reader| {
        observed_error = Some(reader.read("linkdir/../file.mk").unwrap_err());
        Ok(())
    });
    assert!(result.is_err());
    assert!(observed_error
        .as_deref()
        .unwrap()
        .contains("captured real directory"));
}

#[test]
fn transaction_rejects_root_mutation_after_a_successful_callback() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("input.mk"), b"before\n").unwrap();
    let snapshot = snapshot(&root, &root);

    let result = snapshot.transaction(|_| {
        std::fs::write(root.join("input.mk"), b"after\n").map_err(|error| error.to_string())?;
        Ok("callback result")
    });

    assert!(
        result.is_err(),
        "mutated inputs must not yield a read manifest"
    );
    assert!(result.unwrap_err().contains("root changed"));
}

#[cfg(unix)]
#[test]
fn wildcard_missing_ancestors_require_proved_absence_and_preserve_refusals() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("known")).unwrap();
    std::fs::write(root.join("regular"), b"fixture").unwrap();
    symlink("known", root.join("link")).unwrap();
    let selected = snapshot(&root, &root);
    for identity in [
        "known/missing/deep/file".to_owned(),
        format!("{}/missing/deep/file", root.display()),
    ] {
        let (kind, reads) = selected
            .transaction(|reader| reader.wildcard_entry_kind(&identity))
            .unwrap();
        assert!(kind.is_none());
        assert!(reads.reads().is_empty());
        assert!(
            selected
                .transaction(|reader| reader.entry_kind(&identity))
                .is_err(),
            "ordinary membership contract must remain strict"
        );
    }
    for identity in [
        "link/deep/file",
        "regular/deep/file",
        "missing/../known",
        "../outside/file",
    ] {
        assert!(selected
            .transaction(|reader| reader.wildcard_entry_kind(identity))
            .is_err());
    }
    if root.join("KNOWN").exists() {
        assert!(
            selected
                .transaction(|reader| reader.wildcard_entry_kind("KNOWN/missing/file"))
                .is_err(),
            "case-folded aliases must not establish absence"
        );
    }
    let result = selected.transaction(|reader| {
        assert!(reader.wildcard_entry_kind("new/deep/file")?.is_none());
        std::fs::create_dir(root.join("new")).map_err(|_| "fixture mutation failed")?;
        Ok(())
    });
    assert!(
        result.is_err(),
        "an absence observation must not survive changed captured roots"
    );
}

#[test]
fn transaction_rejects_root_mutation_after_a_failed_callback() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("input.mk"), b"before\n").unwrap();
    let snapshot = snapshot(&root, &root);
    let mut callback_ran = false;

    let result = snapshot.transaction(|_| {
        callback_ran = true;
        std::fs::write(root.join("input.mk"), b"after\n").map_err(|error| error.to_string())?;
        Err::<(), String>("callback failed".into())
    });

    assert!(callback_ran);
    assert!(
        result.is_err(),
        "mutated inputs must not yield a read manifest"
    );
    assert!(result.unwrap_err().contains("root changed"));
}

#[test]
fn transaction_rejects_a_swallowed_reader_error() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("input.mk"), b"captured\n").unwrap();
    let snapshot = snapshot(&root, &root);
    let mut observed_error = None;

    let result = snapshot.transaction(|reader| {
        observed_error = Some(reader.read("../outside.mk").unwrap_err());
        Ok("callback ignored the refusal")
    });

    assert!(observed_error
        .as_deref()
        .unwrap()
        .contains("leaves captured roots"));
    assert!(
        result.is_err(),
        "a refused read must prevent transaction publication"
    );
    assert!(result.unwrap_err().contains("refused read"));
}

#[test]
fn directory_only_read_suffixes_never_become_regular_file_reads() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("file.mk"), "VALUE := selected\n").unwrap();
    let captured = snapshot(&root, &root);
    for identity in ["file.mk/", "file.mk/.", "file.mk/.//"] {
        let result = captured.transaction(|reader| reader.read(identity));
        assert!(
            result.is_err(),
            "a directory-only identity must never read a regular file"
        );
    }
    assert_eq!(
        captured
            .transaction(|reader| reader.read("file.mk"))
            .unwrap()
            .0
            .as_deref(),
        Some("VALUE := selected\n")
    );
}

#[cfg(unix)]
#[test]
fn distinct_existing_case_sensitive_roots_are_not_conflated() {
    use std::os::unix::fs::MetadataExt as _;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let upper = root.join("Upper");
    let lower = root.join("upper");
    std::fs::create_dir(&upper).unwrap();
    match std::fs::create_dir(&lower) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // A case-folding filesystem has one directory, not this fixture.
            assert_eq!(
                std::fs::metadata(&upper).unwrap().ino(),
                std::fs::metadata(&lower).unwrap().ino()
            );
            return;
        }
        Err(error) => panic!("cannot create case-sensitive fixture: {error}"),
    }
    let selections = [&upper, &lower].map(|path| MakeInputRootSelection {
        root: path.clone(),
        content_sha256: selected_digest(path),
    });
    let captured = MakeInputSnapshot::capture(&upper, &selections).unwrap();
    captured.verify().unwrap();
    // Reusing the same directory is still rejected, never resolved by order.
    assert!(
        MakeInputSnapshot::capture(&upper, &[selections[0].clone(), selections[0].clone()])
            .is_err()
    );
}

#[test]
fn captured_directory_queries_bind_names_kinds_absence_and_no_read_claim() {
    use crate::TreeContentEntryKind::{Directory, RegularFile};
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("child")).unwrap();
    std::fs::write(root.join("file.mk"), "selected\n").unwrap();
    let captured = snapshot(&root, &root);
    let ((), reads) = captured
        .transaction(|reader| {
            assert_eq!(reader.entry_kind(".")?, Some(Directory));
            assert_eq!(reader.entry_kind("file.mk")?, Some(RegularFile));
            assert_eq!(reader.entry_kind("missing.mk")?, None);
            assert_eq!(reader.entry_kind("file.mk/")?, None);
            assert_eq!(reader.entry_kind("file.mk/.")?, None);
            assert_eq!(
                reader.directory_entries(".")?,
                Some(vec![
                    ("child".into(), Directory),
                    ("file.mk".into(), RegularFile)
                ])
            );
            assert_eq!(reader.directory_entries("child")?, Some(vec![]));
            assert_eq!(reader.directory_entries("missing")?, None);
            Ok(())
        })
        .unwrap();
    assert!(
        reads.reads().is_empty(),
        "queries are not Make include reads"
    );
    let error = captured.transaction(|reader| {
        assert!(reader.directory_entries("file.mk").is_err());
        Ok(())
    });
    assert!(error.unwrap_err().contains("refused read"));
    let error = captured.transaction(|reader| {
        reader.entry_kind("file.mk")?;
        std::fs::write(root.join("file.mk"), "changed\n").map_err(|error| error.to_string())?;
        Ok(())
    });
    assert!(error.unwrap_err().contains("root changed"));
}

#[cfg(unix)]
#[test]
fn captured_directory_queries_never_follow_a_symbolic_link() {
    use crate::TreeContentEntryKind::SymbolicLink;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("actual")).unwrap();
    std::os::unix::fs::symlink("actual", root.join("link")).unwrap();
    let captured = snapshot(&root, &root);
    assert_eq!(
        captured
            .transaction(|reader| reader.entry_kind("link"))
            .unwrap()
            .0,
        Some(SymbolicLink)
    );
    for name in ["link/", "link/.", "link/child", "link/../actual"] {
        assert!(
            captured
                .transaction(|reader| reader.entry_kind(name))
                .is_err(),
            "must refuse symbolic traversal: {name}"
        );
    }
    assert!(captured
        .transaction(|reader| reader.directory_entries("link"))
        .is_err());
}

#[test]
fn captured_membership_never_calls_a_native_case_alias_absent() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("Selected.mk"), "selected\n").unwrap();
    std::fs::create_dir(root.join("SelectedDir")).unwrap();
    let captured = snapshot(&root, &root);
    for alias in ["selected.mk", "selecteddir"] {
        let exists_natively = root.join(alias).try_exists().unwrap();
        let result = captured.transaction(|reader| reader.entry_kind(alias));
        if exists_natively {
            assert!(result.is_err(), "an alias is not proved absence");
        } else {
            assert_eq!(result.unwrap().0, None);
        }
        let result = captured.transaction(|reader| reader.directory_entries(alias));
        if exists_natively {
            assert!(result.is_err(), "an alias is not a missing directory");
        } else {
            assert_eq!(result.unwrap().0, None);
        }
    }
}

#[test]
fn captured_realpath_returns_selected_normalized_paths_without_input_read_claims() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("dir")).unwrap();
    std::fs::write(root.join("file.mk"), "selected\n").unwrap();
    let captured = snapshot(&root, &root);
    let ((), reads) = captured
        .transaction(|reader| {
            assert_eq!(reader.realpath("file.mk")?, Some(root.join("file.mk")));
            assert_eq!(
                reader.realpath("./dir/../file.mk")?,
                Some(root.join("file.mk"))
            );
            assert_eq!(reader.realpath("dir/.")?, Some(root.join("dir")));
            assert_eq!(reader.realpath("dir/")?, Some(root.join("dir")));
            assert_eq!(reader.realpath("missing.mk")?, None);
            assert_eq!(reader.realpath("file.mk/")?, Some(root.join("file.mk")));
            assert_eq!(reader.realpath("file.mk/.")?, Some(root.join("file.mk")));
            Ok(())
        })
        .unwrap();
    assert!(reads.reads().is_empty());
    assert!(captured
        .transaction(|reader| reader.realpath("../outside"))
        .is_err());
    let result = captured.transaction(|reader| {
        reader.realpath("file.mk")?;
        std::fs::write(root.join("file.mk"), "changed\n").map_err(|error| error.to_string())?;
        Ok(())
    });
    assert!(result.unwrap_err().contains("root changed"));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn captured_realpath_matches_native_original_token_path_limit() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("file.mk"), "selected\n").unwrap();
    let limit = usize::try_from(libc::PATH_MAX).unwrap();
    let below = format!("{}file.mk", "./".repeat((limit - "file.mk".len() - 1) / 2));
    let mut boundary = below.clone();
    boundary.extend(std::iter::repeat_n('/', limit - boundary.len()));
    assert!(below.len() < limit);
    assert_eq!(boundary.len(), limit);
    let captured = snapshot(&root, &root);
    captured
        .transaction(|reader| {
            assert_eq!(reader.realpath(&below)?, Some(root.join("file.mk")));
            assert_eq!(reader.realpath(&boundary)?, None);
            Ok(())
        })
        .unwrap();
    let output = std::process::Command::new(if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    })
    .current_dir(&root)
    .env_clear()
    .env("PATH", std::env::var_os("PATH").unwrap())
    .env("LC_ALL", "C")
    .args(["-r", "-n", "-s", "-f", "/dev/null", "--eval"])
    .arg(format!("$(info BELOW=$(realpath {below}))"))
    .arg("--eval")
    .arg(format!("$(info BOUNDARY=$(realpath {boundary}))"))
    .args(["--eval", "probe:;"])
    .output()
    .unwrap();
    assert!(output.status.success());
    assert_eq!(
        std::str::from_utf8(&output.stdout).unwrap(),
        format!("BELOW={}\nBOUNDARY=\n", root.join("file.mk").display())
    );
}

#[cfg(unix)]
#[test]
fn captured_realpath_refusals_poison_caught_errors_and_never_follow_links() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("file.mk"), "selected\n").unwrap();
    std::os::unix::fs::symlink("file.mk", root.join("link")).unwrap();
    let captured = snapshot(&root, &root);
    let result = captured.transaction(|reader| {
        assert!(reader.realpath("link").is_err());
        Ok(())
    });
    assert!(result.unwrap_err().contains("refused read"));
    assert!(captured
        .transaction(|reader| reader.realpath("link/../file.mk"))
        .is_err());
}

#[test]
fn same_root_bindings_ignore_working_directories_and_include_observations() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("first-cwd")).unwrap();
    std::fs::create_dir(root.join("second-cwd")).unwrap();
    std::fs::write(root.join("input.mk"), b"selected\n").unwrap();

    let first = snapshot(&root, &root.join("first-cwd"))
        .with_include_directories(&[root.join("first-cwd").to_str().unwrap().to_owned()])
        .unwrap();
    let second = snapshot(&root, &root.join("second-cwd"))
        .with_include_directories(&[root.join("second-cwd").to_str().unwrap().to_owned()])
        .unwrap();

    assert!(first.same_root_bindings(&second));

    // Binding equality compares retained root observations only. Callers
    // still verify the snapshots separately before relying on their contents.
    std::fs::write(root.join("input.mk"), b"changed\n").unwrap();
    assert!(first.same_root_bindings(&second));
    assert!(first.verify().is_err());
    assert!(second.verify().is_err());
}

#[test]
fn same_root_bindings_reject_different_paths_with_equal_content_hashes() {
    let temporary = tempfile::tempdir().unwrap();
    let first_root = temporary.path().join("first");
    let second_root = temporary.path().join("second");
    std::fs::create_dir(&first_root).unwrap();
    std::fs::create_dir(&second_root).unwrap();
    std::fs::write(first_root.join("input.mk"), b"same selected bytes\n").unwrap();
    std::fs::write(second_root.join("input.mk"), b"same selected bytes\n").unwrap();
    let first_root = first_root.canonicalize().unwrap();
    let second_root = second_root.canonicalize().unwrap();
    let first = snapshot(&first_root, &first_root);
    let second = snapshot(&second_root, &second_root);

    assert_eq!(selected_digest(&first_root), selected_digest(&second_root));
    assert!(!first.same_root_bindings(&second));
}

#[cfg(unix)]
#[test]
fn same_root_bindings_reject_fresh_metadata_for_the_same_content() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let input = root.join("input.mk");
    std::fs::write(&input, b"same selected bytes\n").unwrap();
    let original = snapshot(&root, &root);
    let original_digest = selected_digest(&root);

    // Keep both inodes alive until rename so the replacement cannot reuse the
    // original file identity while retaining its exact bytes.
    let replacement = root.join("replacement.mk");
    std::fs::write(&replacement, b"same selected bytes\n").unwrap();
    std::fs::rename(replacement, &input).unwrap();
    let recaptured = snapshot(&root, &root);

    assert_eq!(selected_digest(&root), original_digest);
    assert!(!original.same_root_bindings(&recaptured));
}

#[test]
fn same_root_bindings_preserve_root_selection_order() {
    let temporary = tempfile::tempdir().unwrap();
    let first_root = temporary.path().join("first");
    let second_root = temporary.path().join("second");
    std::fs::create_dir(&first_root).unwrap();
    std::fs::create_dir(&second_root).unwrap();
    std::fs::write(first_root.join("input.mk"), b"first\n").unwrap();
    std::fs::write(second_root.join("input.mk"), b"second\n").unwrap();
    let first_root = first_root.canonicalize().unwrap();
    let second_root = second_root.canonicalize().unwrap();
    let first_selection = MakeInputRootSelection {
        content_sha256: selected_digest(&first_root),
        root: first_root.clone(),
    };
    let second_selection = MakeInputRootSelection {
        content_sha256: selected_digest(&second_root),
        root: second_root.clone(),
    };
    let first = MakeInputSnapshot::capture(
        &first_root,
        &[first_selection.clone(), second_selection.clone()],
    )
    .unwrap();
    let reversed =
        MakeInputSnapshot::capture(&second_root, &[second_selection, first_selection]).unwrap();

    assert!(!first.same_root_bindings(&reversed));
}

#[test]
fn same_root_bindings_reject_complete_and_local_source_measurement_domains() {
    let temporary = local_source_checkout();
    let source_root = temporary.path().canonicalize().unwrap();
    let complete = snapshot(&source_root, &source_root);
    let identity = LocalSourceIdentity::capture(&source_root, "fixture").unwrap();
    let local_source =
        MakeInputSnapshot::capture_with_local_source(&source_root, &source_root, &identity, &[])
            .unwrap();

    assert!(!complete.same_root_bindings(&local_source));
}
