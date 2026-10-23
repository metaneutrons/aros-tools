//! Regression tests for coalesced Make input root revalidation.

use super::{measure, MakeInputRootSelection, MakeInputSnapshot, RootMeasurement};
use crate::{local_source::LocalSourceIdentity, sha256_bytes, TreeContentCas};
use std::path::{Path, PathBuf};

fn make_root(parent: &Path, name: &str, contents: &[u8]) -> PathBuf {
    let root = parent.join(name);
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("input.mk"), contents).unwrap();
    root.canonicalize().unwrap()
}

fn capture(working_directory: &Path, roots: &[PathBuf]) -> MakeInputSnapshot {
    let selections = roots
        .iter()
        .map(|root| MakeInputRootSelection {
            root: root.clone(),
            content_sha256: measure(root).unwrap().payload_digest_excluding(None),
        })
        .collect::<Vec<_>>();
    MakeInputSnapshot::capture(working_directory, &selections).unwrap()
}

fn with_first_measurement(
    snapshot: &MakeInputSnapshot,
    measurement: RootMeasurement,
) -> MakeInputSnapshot {
    let mut changed = snapshot.clone();
    let mut roots = changed.roots.to_vec();
    roots[0].measurement = measurement;
    changed.roots = roots.into();
    changed
}

fn synthetic_local_source_identity(tree: &TreeContentCas) -> LocalSourceIdentity {
    LocalSourceIdentity {
        schema: "aros-local-source-v1".into(),
        head_baseline: "0123456789abcdef0123456789abcdef01234567".into(),
        submodules_sha256: sha256_bytes(b"synthetic submodule selection"),
        generated_subtree: "build/synthetic".into(),
        content_sha256: tree.payload_digest_excluding(None),
        entry_count: tree.entry_count(),
        regular_file_bytes: tree.regular_file_bytes().unwrap(),
    }
}

#[test]
fn same_read_context_requires_the_exact_working_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"context input\n");
    let nested_working_directory = root.join("working");
    std::fs::create_dir(&nested_working_directory).unwrap();
    let selected_roots = std::slice::from_ref(&root);
    let root_context = capture(&root, selected_roots);
    let nested_context = capture(&nested_working_directory, selected_roots);

    assert!(root_context.same_read_context(&root_context.clone()));
    assert!(root_context.same_root_bindings(&nested_context));
    assert!(!root_context.same_read_context(&nested_context));
}

#[test]
fn same_read_context_preserves_include_order_and_optional_presence() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"include context input\n");
    std::fs::create_dir(root.join("include-a")).unwrap();
    std::fs::create_dir(root.join("include-b")).unwrap();
    let base = capture(&root, std::slice::from_ref(&root));
    let include_a = String::from("include-a");
    let include_b = String::from("include-b");
    let ordered = base
        .clone()
        .with_include_directories(&[include_a.clone(), include_b.clone()])
        .unwrap();
    let reversed = base
        .clone()
        .with_include_directories(&[include_b, include_a])
        .unwrap();
    let explicitly_empty = base.clone().with_include_directories(&[]).unwrap();

    assert!(ordered.same_root_bindings(&reversed));
    assert!(!ordered.same_read_context(&reversed));
    assert!(base.same_root_bindings(&explicitly_empty));
    assert!(!base.same_read_context(&explicitly_empty));
}

#[test]
fn provisional_read_suffix_contains_only_reads_after_its_checkpoint() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"first input\n");
    std::fs::write(root.join("second.mk"), b"second input\n").unwrap();
    std::fs::write(root.join("third.mk"), b"third input\n").unwrap();
    let snapshot = capture(&root, std::slice::from_ref(&root));

    let ((provisional, checkpoint), committed) = snapshot
        .transaction(|reader| {
            assert_eq!(reader.read_checkpoint(), 0);
            reader.read("input.mk")?;
            let checkpoint = reader.read_checkpoint();
            reader.read("second.mk")?;
            reader.read("third.mk")?;
            let suffix = reader.provisional_reads_since(checkpoint)?;
            Ok((suffix, checkpoint))
        })
        .unwrap();

    assert_eq!(checkpoint, 1);
    assert_eq!(
        provisional
            .reads()
            .iter()
            .map(|read| read.requested.as_str())
            .collect::<Vec<_>>(),
        ["second.mk", "third.mk"]
    );
    assert_eq!(committed.reads().len(), 3);
}

#[test]
fn provisional_read_suffix_refuses_a_future_checkpoint() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"checkpoint input\n");
    let snapshot = capture(&root, std::slice::from_ref(&root));

    let error = snapshot
        .transaction(|reader| {
            assert_eq!(reader.read_checkpoint(), 0);
            reader.provisional_reads_since(1).map(|_| ())
        })
        .unwrap_err();

    assert!(error.contains("checkpoint"));
}

#[test]
fn swallowing_a_read_failure_still_poisons_the_enclosing_transaction() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"poison input\n");
    let snapshot = capture(&root, std::slice::from_ref(&root));

    let error = snapshot
        .transaction(|reader| {
            assert!(reader.read("../outside.mk").is_err());
            Ok(())
        })
        .unwrap_err();

    assert!(error.contains("contains a refused read"));
}

#[test]
fn joint_verification_measures_an_overlapping_root_once_and_checks_the_extra_root() {
    let temporary = tempfile::tempdir().unwrap();
    let shared = make_root(temporary.path(), "shared", b"shared input\n");
    let additional = make_root(temporary.path(), "additional", b"additional input\n");
    let original = capture(&shared, std::slice::from_ref(&shared));
    let later = capture(&shared, &[shared.clone(), additional.clone()]);

    // Success is evidence about these retained roots at this verification
    // point. It says nothing about whether they changed between observations.
    original.verify_with(&later).unwrap();

    let mut measured = Vec::new();
    original
        .verify_with_measurer(Some(&later), |root| {
            measured.push(root.path.clone());
            Ok(root.tree.clone())
        })
        .unwrap();
    assert_eq!(measured.len(), 2);
    assert_eq!(measured.iter().filter(|path| **path == shared).count(), 1);
    assert_eq!(
        measured.iter().filter(|path| **path == additional).count(),
        1
    );

    // A change confined to the additional root must also be observed.
    std::fs::write(additional.join("input.mk"), b"mutated additional input\n").unwrap();
    assert!(original.verify_with(&later).is_err());
}

#[test]
fn public_joint_verification_refuses_a_mutated_shared_file() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"captured input\n");
    let first = capture(&root, std::slice::from_ref(&root));
    let second = capture(&root, std::slice::from_ref(&root));

    std::fs::write(root.join("input.mk"), b"changed shared input\n").unwrap();

    assert!(first.verify_with(&second).is_err());
}

#[test]
fn conflicting_shared_domain_baselines_fail_before_measurement() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"first baseline\n");
    let first = capture(&root, std::slice::from_ref(&root));

    std::fs::write(root.join("input.mk"), b"second baseline\n").unwrap();
    let second = capture(&root, std::slice::from_ref(&root));
    assert!(!first.same_read_context(&second));

    let mut measurements = 0;
    let result = first.verify_with_measurer(Some(&second), |retained| {
        measurements += 1;
        Ok(retained.tree.clone())
    });

    assert!(result.is_err());
    assert_eq!(measurements, 0);
}

#[test]
fn joint_verification_propagates_a_shared_domain_measurement_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"stable input\n");
    let first = capture(&root, std::slice::from_ref(&root));
    let second = capture(&root, std::slice::from_ref(&root));
    let mut measurements = 0;

    let error = first
        .verify_with_measurer(Some(&second), |_| {
            measurements += 1;
            Err("synthetic measurement failure".into())
        })
        .unwrap_err();

    assert_eq!(error, "synthetic measurement failure");
    assert_eq!(measurements, 1);
}

#[test]
fn equal_paths_with_distinct_measurement_policies_are_measured_separately() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"policy grouping input\n");
    let complete = capture(&root, std::slice::from_ref(&root));
    let retained_tree = complete.roots[0].tree.clone();
    let synthetic_identity = synthetic_local_source_identity(&retained_tree);
    let local_source = with_first_measurement(
        &complete,
        RootMeasurement::LocalSource(synthetic_identity.clone()),
    );
    assert!(!complete.same_read_context(&local_source));
    let mut measured_policies = Vec::new();

    // This exercises only grouping by measurement policy. The injected
    // measurer does not authenticate a LocalSourceIdentity.
    complete
        .verify_with_measurer(Some(&local_source), |root| {
            measured_policies.push(root.measurement.clone());
            Ok(root.tree.clone())
        })
        .unwrap();

    assert_eq!(measured_policies.len(), 2);
    assert!(measured_policies.contains(&RootMeasurement::Complete));
    assert!(measured_policies.contains(&RootMeasurement::LocalSource(synthetic_identity)));
}

#[test]
fn distinct_local_source_identities_do_not_share_a_measurement() {
    let temporary = tempfile::tempdir().unwrap();
    let root = make_root(temporary.path(), "root", b"identity grouping input\n");
    let complete = capture(&root, std::slice::from_ref(&root));
    let first_identity = synthetic_local_source_identity(&complete.roots[0].tree);
    let mut second_identity = first_identity.clone();
    second_identity.generated_subtree = "build/other".into();
    let first = with_first_measurement(&complete, RootMeasurement::LocalSource(first_identity));
    let second = with_first_measurement(&complete, RootMeasurement::LocalSource(second_identity));
    assert!(!first.same_read_context(&second));
    let mut measurements = 0;
    // Grouping only; the synthetic identities are not authenticated by this probe.
    first
        .verify_with_measurer(Some(&second), |root| {
            measurements += 1;
            Ok(root.tree.clone())
        })
        .unwrap();
    assert_eq!(measurements, 2);
}
