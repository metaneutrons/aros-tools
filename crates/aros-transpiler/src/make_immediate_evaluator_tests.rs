use super::collect_vars_with_immediate_evaluator;
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::parser::TargetContext;
use std::cell::Cell;
use std::path::Path;

#[test]
fn exact_immediate_values_freeze_functions_and_append_at_source_position() {
    let program = "INPUT = old\nOUTPUT := $(foreach item,a b,$(INPUT)-$(item))\nINPUT = new\nOUTPUT += $(INPUT)\nINPUT = later\n";
    let dirs = DirVars::load(Path::new("/nonexistent-fixture"));
    let (scope, _) = collect_vars_with_immediate_evaluator(
        program,
        &TargetContext::default(),
        &|raw, scope, line| {
            evaluate_make_expr(
                raw,
                &MakeExprContext::new(scope, &dirs, line, Path::new("/fixture"), Path::new(".")),
            )
            .map_err(|error| error.to_string())
        },
    )
    .unwrap();
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("/fixture"),
        Path::new("."),
    );
    assert_eq!(
        evaluate_make_expr("$(OUTPUT)", &context).unwrap(),
        "old-a old-b new"
    );
}

#[test]
fn immediate_results_preserve_literal_dollars_without_second_expansion() {
    let dirs = DirVars::load(Path::new("/nonexistent-fixture"));
    let (scope, _) = collect_vars_with_immediate_evaluator(
        "OUTPUT := arbitrary-input\nLATER = changed\n",
        &TargetContext::default(),
        &|_, _, _| Ok("$(LATER) $@".to_owned()),
    )
    .unwrap();
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        usize::MAX,
        Path::new("/fixture"),
        Path::new("."),
    );
    assert_eq!(
        evaluate_make_expr("$(OUTPUT)", &context).unwrap(),
        "$(LATER) $@"
    );
}

#[test]
fn evaluator_is_not_called_for_inactive_unknown_or_recursive_assignments() {
    let calls = Cell::new(0);
    let program = "ifeq (no,yes)\nINACTIVE := forbidden\nendif\nifeq ($(shell unknown),yes)\nUNKNOWN := forbidden\nendif\nRECURSIVE = unevaluated\nACTIVE := allowed\n";
    let (_, states) =
        collect_vars_with_immediate_evaluator(program, &TargetContext::default(), &|raw, _, _| {
            calls.set(calls.get() + 1);
            assert_eq!(raw, "allowed");
            Ok(raw.to_owned())
        })
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(states[1], Some(false));
    assert_eq!(states[4], None);
}

#[test]
fn evaluator_error_discards_partial_scope_and_stops_further_evaluation() {
    let calls = Cell::new(0);
    let result = collect_vars_with_immediate_evaluator(
        "FIRST := fail\nSECOND := must-not-run\n",
        &TargetContext::default(),
        &|_, _, _| {
            calls.set(calls.get() + 1);
            Err("explicit evaluator refusal".to_owned())
        },
    );
    assert!(matches!(result, Err(error) if error == "explicit evaluator refusal"));
    assert_eq!(calls.get(), 1);
}
