use super::{evaluate_make_expr, MakeExprContext, MakeExprError};
use crate::dirs::DirVars;
use crate::make_vars::{collect_vars, collect_vars_with_immediate_evaluator};
use crate::parser::TargetContext;
use std::path::Path;

#[test]
fn pure_pipeline_is_explicit_and_does_not_enable_other_shell_commands() {
    let scope = collect_vars("ECHO := echo\nROOT := C:/source\n");
    let dirs = DirVars::default();
    let plain = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("/source"),
        Path::new("."),
    )
    .without_filesystem();
    let expression = r"$(shell $(ECHO) $(ROOT) | sed 's/^\(.\):\//\/\1\//')";
    assert!(matches!(
        evaluate_make_expr(expression, &plain),
        Err(MakeExprError::UnsupportedFunction { name }) if name == "shell"
    ));
    let pure = plain.with_pure_text_pipeline();
    assert_eq!(evaluate_make_expr(expression, &pure).unwrap(), "/C/source");
    assert_eq!(
        evaluate_make_expr(
            r"$(shell echo $(SRCDIR) | sed 's/^\(.\):\//\/\1\//')",
            &pure
        )
        .unwrap(),
        "/source"
    );
    assert!(matches!(
        evaluate_make_expr("$(shell touch /must-not-execute)", &pure),
        Err(MakeExprError::InvalidSyntax { expression, .. }) if expression == "pure text pipeline"
    ));
    assert_eq!(
        evaluate_make_expr("$(if yes,known,$(shell touch /must-not-execute))", &pure).unwrap(),
        "known"
    );
    assert!(matches!(
        evaluate_make_expr("$(shell echo $(wildcard *.c) | sed 's/a/b/')", &pure),
        Err(MakeExprError::FilesystemAccessDisabled { .. })
    ));
}

#[test]
fn pipeline_results_freeze_at_the_original_immediate_assignment() {
    let program = concat!(
        "ECHO := echo\n",
        "ROOT := C:/before\n",
        "NORMAL := $(shell $(ECHO) $(ROOT) | sed 's/^\\(.\\):\\//\\/\\1\\//')\n",
        "ROOT := D:/after\n",
    );
    let dirs = DirVars::default();
    let (scope, _) = collect_vars_with_immediate_evaluator(
        program,
        &TargetContext::default(),
        &|raw, scope, line| {
            let context =
                MakeExprContext::new(scope, &dirs, line, Path::new("/source"), Path::new("."))
                    .with_pure_text_pipeline()
                    .without_filesystem();
            evaluate_make_expr(raw, &context).map_err(|error| error.to_string())
        },
    )
    .unwrap();
    let final_context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("/source"),
        Path::new("."),
    )
    .with_pure_text_pipeline()
    .without_filesystem();
    assert_eq!(
        evaluate_make_expr("$(NORMAL)", &final_context).unwrap(),
        "/C/before"
    );
    assert_eq!(
        evaluate_make_expr("$(ROOT)", &final_context).unwrap(),
        "D:/after"
    );
}

#[test]
fn unknown_pipeline_values_remain_errors_instead_of_host_environment_inputs() {
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("."))
        .with_pure_text_pipeline()
        .without_filesystem();
    assert!(matches!(
        evaluate_make_expr("$(shell echo $(MISSING_PIPELINE_DATA) | sed 's/a/b/')", &context),
        Err(MakeExprError::UnresolvedVariables { names, .. }) if names == ["MISSING_PIPELINE_DATA"]
    ));
}

#[test]
fn materialized_source_coordinates_have_a_preallocation_amplification_bound() {
    let source_text = format!("/{}", "x".repeat(4096));
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new(&source_text), Path::new("."))
        .with_pure_text_pipeline()
        .without_filesystem();
    assert_eq!(
        evaluate_make_expr("$(shell echo $(SRCDIR) | sed 's/x/x/')", &context).unwrap(),
        source_text
    );
    let expression = format!("$(shell echo {} | sed 's/x/x/')", "$(SRCDIR) ".repeat(2048));
    assert!(matches!(
        evaluate_make_expr(&expression, &context),
        Err(MakeExprError::InvalidSyntax { detail, .. })
            if detail.contains("source-token materialization exceeds value byte limit")
    ));
}
