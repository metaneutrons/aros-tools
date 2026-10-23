//! Ordered, atomic expression batches over one captured input selection.

use super::*;
use crate::dirs::DirVars;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation},
    measure_tree_content_cas,
};
use std::path::Path;

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .into_iter()
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .collect(),
    })
    .unwrap()
}

fn paths<'a>(dirs: &'a DirVars, root: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn snapshot(root: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        root,
        &[MakeInputRootSelection {
            root: root.to_owned(),
            content_sha256: measure_tree_content_cas(root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap()
}

#[test]
fn captured_batch_orders_effects_and_returns_independent_literal_results() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("a.c"), "a\n").unwrap();
    std::fs::write(root.join("b.c"), "b\n").unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();

    let (results, reads) = program
        .evaluate_many_captured(
            &[
                "$(eval ORDER := first)$(ORDER)",
                "$(wildcard *.c)",
                "$(realpath a.c)",
                "$(eval ORDER += second)$(ORDER)",
                "$(wildcard *.c)",
                "literal\n|;$$",
            ],
            &paths,
            &captured,
        )
        .unwrap();

    assert_eq!(
        results,
        [
            "first",
            "a.c b.c",
            root.join("a.c").to_str().unwrap(),
            "first second",
            "a.c b.c",
            "literal\n|;$",
        ]
    );
    assert_eq!(results[1], results[4]);
    assert_eq!(
        program.evaluate("$(ORDER)", &paths).unwrap(),
        "first second"
    );
    // These are membership/path queries, not Make file reads, so they do not
    // manufacture input identities in the verified read manifest.
    assert!(reads.reads().is_empty());
    assert!(reads.old_file_arguments().is_empty());
}

#[test]
fn captured_batch_expression_error_rolls_back_earlier_eval_effects() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program.execute("VALUE := before", 0, &paths).unwrap();
    let before = program.events().to_vec();

    let failure = program
        .evaluate_many_captured(
            &["$(eval VALUE := transient)$(VALUE)", "$(unsupported x)"],
            &paths,
            &captured,
        )
        .unwrap_err();

    assert!(
        failure.contains("unsupported"),
        "unexpected refusal: {failure}"
    );
    assert_eq!(program.events(), before);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "before");
}

#[test]
fn changed_captured_root_refuses_batch_and_preserves_prior_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("a.c"), "before\n").unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    assert_eq!(
        program
            .evaluate_many_captured(&["$(eval VALUE := prior)$(VALUE)"], &paths, &captured)
            .unwrap()
            .0,
        ["prior"]
    );
    let before = program.events().to_vec();

    std::fs::write(root.join("b.c"), "after\n").unwrap();
    let failure = program
        .evaluate_many_captured(
            &["$(eval VALUE := uncommitted)$(VALUE)", "$(wildcard *.c)"],
            &paths,
            &captured,
        )
        .unwrap_err();

    assert!(failure.contains("changed"), "unexpected refusal: {failure}");
    assert_eq!(program.events(), before);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "prior");
}

#[test]
fn captured_batch_checks_count_before_staging_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    program.execute("VALUE := retained", 0, &paths).unwrap();
    let before = program.events().to_vec();
    program.state_work.set(MAX_STATE_WORK);

    let expressions = vec![""; MAX_STATEMENTS + 1];
    let failure = program
        .evaluate_many_captured(&expressions, &paths, &captured)
        .unwrap_err();

    assert!(
        failure.contains("Make expression batch count"),
        "count refusal must precede staging: {failure}"
    );
    assert_eq!(program.events(), before);
    assert_eq!(program.state_work.get(), MAX_STATE_WORK);
}

#[test]
fn captured_batch_shares_work_and_result_storage_limits_across_expressions() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root);

    let mut work_limited = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    work_limited.state_work.set(MAX_STATE_WORK - 1_500);
    let failure = work_limited
        .evaluate_many_captured(&["a", "b"], &paths, &captured)
        .unwrap_err();
    assert!(
        failure.contains("Make state work"),
        "shared work refusal expected: {failure}"
    );
    assert_eq!(work_limited.state_work.get(), MAX_STATE_WORK - 468);
    assert!(work_limited.events().is_empty());

    let mut storage_limited = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let expressions = vec!["x"; MAX_STATEMENTS];
    let failure = storage_limited
        .evaluate_many_captured(&expressions, &paths, &captured)
        .unwrap_err();
    assert!(
        failure.contains("Make expression batch results"),
        "aggregate result-storage refusal expected: {failure}"
    );
    assert!(storage_limited.events().is_empty());
}
