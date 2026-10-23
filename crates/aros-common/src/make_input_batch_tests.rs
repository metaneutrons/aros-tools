//! Multi-child transaction tests for caller-selected Make input snapshots.

use super::{MakeInputBatchLimits, MakeInputRootSelection, MakeInputSnapshot};
use crate::{
    local_source::LocalSourceIdentity, measure_tree_content_cas_bounded, TreeTraversalLimits,
};
use std::{path::Path, sync::Arc};

const TEST_LIMITS: MakeInputBatchLimits = MakeInputBatchLimits {
    maximum_children: 8,
    maximum_read_bytes: 1024 * 1024,
    maximum_reads: 128,
    maximum_queries: 128,
    maximum_query_names: 1024,
    maximum_query_bytes: 1024 * 1024,
    maximum_record_bytes: 1024 * 1024,
};

fn selected_digest(root: &Path) -> crate::Sha256Digest {
    measure_tree_content_cas_bounded(root, TreeTraversalLimits::new(1024, 1024 * 1024).unwrap())
        .unwrap()
        .payload_digest_excluding(None)
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let first = root.join("first-cwd");
    let second = root.join("second-cwd");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    let search = second.join("search");
    std::fs::create_dir(&search).unwrap();
    std::fs::write(first.join("first.mk"), b"first child\n").unwrap();
    std::fs::write(second.join("second.mk"), b"second child\n").unwrap();
    std::fs::write(search.join("included.mk"), b"included child\n").unwrap();
    (temporary, first, second)
}

fn snapshot(root: &Path) -> MakeInputSnapshot {
    snapshot_at(root, root)
}

fn snapshot_at(root: &Path, working_directory: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        working_directory,
        &[MakeInputRootSelection {
            root: root.to_path_buf(),
            content_sha256: selected_digest(root),
        }],
    )
    .unwrap()
}

fn directory_cost(path: &Path) -> (usize, usize) {
    let names = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    let bytes = names
        .iter()
        .map(|name| name.as_os_str().as_encoded_bytes().len())
        .sum();
    (names.len(), bytes)
}

fn setup_cost(first: &Path, second: &Path) -> (usize, usize, usize) {
    let search = second.join("search");
    let paths = [first, second, search.as_path()];
    let (names, directory_bytes) = paths.iter().fold((0, 0), |(names, bytes), path| {
        let (directory_names, directory_bytes) = directory_cost(path);
        (
            names + directory_names,
            bytes + directory_bytes + path.as_os_str().len(),
        )
    });
    (paths.len(), names, directory_bytes + "search".len())
}

fn directory_query_totals(root: &Path, first: &Path, second: &Path) -> (usize, usize, usize) {
    let (root_names, root_bytes) = directory_cost(root);
    let (first_names, first_bytes) = directory_cost(first);
    let (second_names, second_bytes) = directory_cost(second);
    let (setup_queries, setup_names, setup_bytes) = setup_cost(first, second);

    (
        setup_queries + 2,
        setup_names + 2 * root_names + first_names + second_names,
        setup_bytes + 2 * (1 + root_bytes) + first_bytes + second_bytes,
    )
}

#[test]
fn children_share_roots_and_keep_separate_cwds_and_include_order() {
    let (temporary, first, second) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let search_spelling = "search".to_owned();
    // The parent cwd has no `search` directory. Only the second child does.
    let snapshot = snapshot_at(&root, &first);

    let (first_text, first_reads, second_text, second_reads) = snapshot
        .batch_transaction(
            TEST_LIMITS,
            || Ok(()),
            |batch| {
                let (first_text, first_reads) =
                    batch.read_child(&first, None, |child, reader| {
                        assert_eq!(child.working_directory(), first.as_path());
                        assert_eq!(reader.snapshot().working_directory(), first.as_path());
                        assert!(Arc::ptr_eq(&snapshot.roots, &child.roots));
                        Ok(reader.read("first.mk")?.unwrap())
                    })?;
                let (second_text, second_reads) = batch.read_child(
                    &second,
                    Some(std::slice::from_ref(&search_spelling)),
                    |child, reader| {
                        assert_eq!(child.working_directory(), second.as_path());
                        assert_eq!(reader.working_directory(), second.as_path());
                        assert!(Arc::ptr_eq(&snapshot.roots, &child.roots));
                        let included = reader.include("included.mk")?.unwrap();
                        assert_eq!(included.identity, format!("{search_spelling}/included.mk"));
                        Ok(format!(
                            "{}{}",
                            reader.read("second.mk")?.unwrap(),
                            included.text
                        ))
                    },
                )?;
                Ok((first_text, first_reads, second_text, second_reads))
            },
        )
        .unwrap();

    assert_eq!(first_text, "first child\n");
    assert_eq!(first_reads.reads().len(), 1);
    assert_eq!(first_reads.reads()[0].requested, "first.mk");
    assert_eq!(second_text, "second child\nincluded child\n");
    assert_eq!(second_reads.reads().len(), 2);
    assert_eq!(second_reads.reads()[0].requested, "included.mk");
    assert_eq!(second_reads.reads()[1].requested, "second.mk");
}

#[test]
fn relative_include_directories_are_validated_against_each_child_cwd() {
    let (temporary, first, second) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot_at(&root, &first);
    let child_search = ["search".to_owned()];

    let valid = snapshot.batch_transaction(
        TEST_LIMITS,
        || Ok(()),
        |batch| {
            let (included, _) = batch.read_child(&second, Some(&child_search), |_, reader| {
                Ok(reader.include("included.mk")?.unwrap().text)
            })?;
            Ok(included)
        },
    );
    assert_eq!(valid.unwrap(), "included child\n");

    std::fs::create_dir(first.join("parent-only")).unwrap();
    let snapshot = snapshot_at(&root, &first);
    let parent_only_search = ["parent-only".to_owned()];
    let invalid = snapshot.batch_transaction(
        TEST_LIMITS,
        || Ok(()),
        |batch| {
            batch.read_child(&second, Some(&parent_only_search), |_, _| Ok(()))?;
            Ok(())
        },
    );
    assert!(invalid.is_err());
}

#[test]
fn changed_root_is_rejected_even_when_the_callback_fails() {
    let (temporary, _, _) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);

    let error = snapshot
        .batch_transaction(
            TEST_LIMITS,
            || Ok(()),
            |_| {
                std::fs::write(root.join("first-cwd/first.mk"), b"changed late\n").unwrap();
                Err::<(), String>("callback failed".into())
            },
        )
        .unwrap_err();

    assert!(error.contains("Make input root changed"));
}

#[test]
fn callback_failure_discards_successful_child_results() {
    let (temporary, first, _) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);

    let result = snapshot.batch_transaction(
        TEST_LIMITS,
        || Ok(()),
        |batch| {
            let (_text, _manifest) = batch.read_child(&first, None, |_, reader| {
                Ok(reader.read("first.mk")?.unwrap())
            })?;
            Err::<String, String>("later child processing failed".into())
        },
    );

    assert_eq!(result.unwrap_err(), "later child processing failed");
}

#[test]
fn child_count_limit_cannot_be_suppressed() {
    let (temporary, first, second) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);
    let limits = MakeInputBatchLimits {
        maximum_children: 1,
        ..TEST_LIMITS
    };

    let result = snapshot.batch_transaction(
        limits,
        || Ok(()),
        |batch| {
            batch.read_child(&first, None, |_, reader| reader.read("first.mk"))?;
            let _suppressed = batch.read_child(&second, None, |_, reader| reader.read("second.mk"));
            Ok(())
        },
    );

    assert!(result.is_err());
}

#[test]
fn read_byte_read_count_and_query_limits_aggregate_across_children() {
    let (temporary, first, second) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);

    let read_limited = MakeInputBatchLimits {
        maximum_reads: 1,
        ..TEST_LIMITS
    };
    let read_result = snapshot.batch_transaction(
        read_limited,
        || Ok(()),
        |batch| {
            batch.read_child(&first, None, |_, reader| reader.read("first.mk"))?;
            batch.read_child(&second, None, |_, reader| reader.read("second.mk"))?;
            Ok(())
        },
    );
    assert!(read_result
        .unwrap_err()
        .contains("read count exceeds limit"));

    let byte_limited = MakeInputBatchLimits {
        maximum_read_bytes: 4,
        ..TEST_LIMITS
    };
    let byte_result = snapshot.batch_transaction(
        byte_limited,
        || Ok(()),
        |batch| {
            batch.read_child(&first, None, |_, reader| reader.read("first.mk"))?;
            Ok(())
        },
    );
    assert!(byte_result.unwrap_err().contains("read bytes exceed limit"));

    let query_limited = MakeInputBatchLimits {
        maximum_queries: 1,
        ..TEST_LIMITS
    };
    let query_result = snapshot.batch_transaction(
        query_limited,
        || Ok(()),
        |batch| {
            batch.read_child(&first, None, |_, reader| reader.entry_kind("first.mk"))?;
            batch.read_child(&second, None, |_, reader| reader.entry_kind("second.mk"))?;
            Ok(())
        },
    );
    assert!(query_result
        .unwrap_err()
        .contains("query count exceeds limit"));
}

#[test]
fn directory_query_names_and_bytes_aggregate_across_children() {
    let (temporary, first, second) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);
    let (queries, query_names, query_bytes) = directory_query_totals(&root, &first, &second);
    assert_eq!(queries, 5);

    let exact_limits = MakeInputBatchLimits {
        maximum_queries: queries,
        maximum_query_names: query_names,
        maximum_query_bytes: query_bytes,
        ..TEST_LIMITS
    };
    let exact = snapshot.batch_transaction(
        exact_limits,
        || Ok(()),
        |batch| {
            batch.read_child(&first, None, |_, reader| {
                Ok(reader.directory_entries(".")?.unwrap())
            })?;
            batch.read_child(&second, Some(&["search".into()]), |_, reader| {
                Ok(reader.directory_entries(".")?.unwrap())
            })?;
            Ok(())
        },
    );
    assert!(exact.is_ok());

    let names_limited = MakeInputBatchLimits {
        maximum_query_names: query_names - 1,
        ..exact_limits
    };
    let names_result = snapshot.batch_transaction(
        names_limited,
        || Ok(()),
        |batch| {
            batch.read_child(&first, None, |_, reader| reader.directory_entries("."))?;
            batch.read_child(&second, Some(&["search".into()]), |_, reader| {
                reader.directory_entries(".")
            })?;
            Ok(())
        },
    );
    assert!(names_result
        .unwrap_err()
        .contains("query name count exceeds limit"));

    let bytes_limited = MakeInputBatchLimits {
        maximum_query_bytes: query_bytes - 1,
        ..exact_limits
    };
    let bytes_result = snapshot.batch_transaction(
        bytes_limited,
        || Ok(()),
        |batch| {
            batch.read_child(&first, None, |_, reader| reader.directory_entries("."))?;
            batch.read_child(&second, Some(&["search".into()]), |_, reader| {
                reader.directory_entries(".")
            })?;
            Ok(())
        },
    );
    assert!(bytes_result
        .unwrap_err()
        .contains("query bytes exceed limit"));
}

#[test]
fn setup_directory_scans_are_charged_before_child_callbacks() {
    let (temporary, first, second) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);
    let (setup_queries, setup_names, setup_bytes) = setup_cost(&first, &second);
    assert_eq!(setup_queries, 3);

    for limits in [
        MakeInputBatchLimits {
            maximum_queries: setup_queries - 1,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_query_names: setup_names - 1,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_query_bytes: setup_bytes - 1,
            ..TEST_LIMITS
        },
    ] {
        let callbacks = std::cell::Cell::new(0usize);
        let result = snapshot.batch_transaction(
            limits,
            || Ok(()),
            |batch| {
                batch.read_child(&first, None, |_, _| {
                    callbacks.set(callbacks.get() + 1);
                    Ok(())
                })?;
                batch.read_child(&second, Some(&["search".into()]), |_, _| {
                    callbacks.set(callbacks.get() + 1);
                    Ok(())
                })?;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(callbacks.get(), 1);
    }
}

#[test]
fn setup_refusal_stops_long_include_parent_traversal_early() {
    let (temporary, _, second) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);
    let repeated_parent_steps = (0..128).map(|_| "search/..").collect::<Vec<_>>().join("/");
    let include_directories = [format!("{repeated_parent_steps}/search")];
    let enumerations = std::cell::Cell::new(0usize);

    let result = snapshot.child_scope(&second, Some(&include_directories), |cost| {
        if cost.queries != 0 {
            let count = enumerations.get() + cost.queries;
            enumerations.set(count);
            if count == 2 {
                return Err("setup budget exhausted".into());
            }
        }
        Ok(())
    });

    assert!(result.is_err());
    assert_eq!(enumerations.get(), 2);
}

#[test]
fn normalized_include_spelling_bytes_are_counted_before_resolution() {
    let (temporary, _, second) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);
    let include = format!("{}search", "./".repeat(128));
    let include_directories = [include.clone()];
    let mut costs = Vec::new();

    let child = snapshot
        .child_scope(&second, Some(&include_directories), |cost| {
            costs.push(cost);
            Ok(())
        })
        .unwrap();

    assert_eq!(
        child.include_directories.as_deref(),
        Some(include_directories.as_slice())
    );
    assert_eq!(costs.len(), 3);
    assert_eq!(costs[0].queries, 1); // Child cwd enumeration.
    assert_eq!(costs[1].queries, 0); // Original spelling before normalization.
    assert_eq!(costs[1].query_names, 0);
    assert_eq!(costs[1].query_bytes, include.len());
    assert_eq!(costs[2].queries, 1); // Resolved include directory enumeration.
}

#[test]
fn record_byte_limit_aggregates_across_children() {
    let (temporary, first, _) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);
    let limits = MakeInputBatchLimits {
        maximum_record_bytes: 1,
        ..TEST_LIMITS
    };

    let result = snapshot.batch_transaction(
        limits,
        || Ok(()),
        |batch| {
            batch.read_child(&first, None, |_, reader| reader.read("first.mk"))?;
            Ok(())
        },
    );

    assert!(result.unwrap_err().contains("manifest bytes exceed limit"));
}

#[test]
fn refused_child_and_control_failure_remain_sticky_when_suppressed() {
    let (temporary, first, _) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);

    let refused = snapshot.batch_transaction(
        TEST_LIMITS,
        || Ok(()),
        |batch| {
            let _suppressed = batch.read_child(&first, None, |_, reader| {
                let _ = reader.read("../../outside.mk");
                Ok(())
            });
            Ok(())
        },
    );
    assert!(refused.is_err());

    let calls = std::cell::Cell::new(0usize);
    let callback_called = std::cell::Cell::new(false);
    let cancelled = snapshot.batch_transaction(
        TEST_LIMITS,
        || {
            let call = calls.get() + 1;
            calls.set(call);
            if call == 2 {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        },
        |batch| {
            let _suppressed = batch.read_child(&first, None, |_, _| {
                callback_called.set(true);
                Ok(())
            });
            Ok(())
        },
    );

    assert!(cancelled.is_err());
    assert!(!callback_called.get());
    assert_eq!(calls.get(), 4);
}

#[test]
fn zero_and_excessive_batch_limits_are_rejected() {
    let (temporary, _, _) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let snapshot = snapshot(&root);
    for limits in [
        MakeInputBatchLimits {
            maximum_children: 0,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_children: 4097,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_read_bytes: 0,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_read_bytes: 1024 * 1024 * 1024 + 1,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_reads: 0,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_reads: 1_000_001,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_queries: 0,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_queries: 1_000_001,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_query_names: 0,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_query_names: 1_000_001,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_query_bytes: 0,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_query_bytes: 64 * 1024 * 1024 + 1,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_record_bytes: 0,
            ..TEST_LIMITS
        },
        MakeInputBatchLimits {
            maximum_record_bytes: 64 * 1024 * 1024 + 1,
            ..TEST_LIMITS
        },
    ] {
        assert!(snapshot
            .batch_transaction(limits, || Ok(()), |_| Ok(()))
            .is_err());
    }
}

#[test]
fn batch_preserves_local_source_exclusion_behavior() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let git_metadata = root.join(".git");
    std::fs::create_dir(&git_metadata).unwrap();
    std::fs::write(root.join("input.mk"), b"selected\n").unwrap();
    let tree = super::measure(&root).unwrap();
    let identity = LocalSourceIdentity {
        schema: "aros-local-source-v1".into(),
        head_baseline: "test-baseline".into(),
        submodules_sha256: crate::sha256_bytes(b""),
        generated_subtree: "build/fixture".into(),
        content_sha256: tree.payload_digest_excluding(None),
        entry_count: tree.entry_count(),
        regular_file_bytes: tree.regular_file_bytes().unwrap(),
    };
    let snapshot = MakeInputSnapshot {
        roots: Arc::from(vec![super::Root {
            path: root.clone(),
            tree,
            measurement: super::RootMeasurement::LocalSource(identity),
        }]),
        working_directory: root,
        include_directories: None,
    };
    let mut check_control = || Ok(());
    let mut batch = super::MakeInputBatch {
        snapshot: &snapshot,
        limits: TEST_LIMITS,
        check_control: &mut check_control,
        children: 0,
        reads: 0,
        read_bytes: 0,
        queries: 0,
        query_names: 0,
        query_bytes: 0,
        record_bytes: 0,
        failure: None,
    };

    let error = batch
        .read_child(&git_metadata, None, |_, _| Ok(()))
        .unwrap_err();
    assert!(error.contains("excluded local source path"));
}
