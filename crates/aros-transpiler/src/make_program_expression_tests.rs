//! Expression protocol tests, not a production Make statement interpreter.

use super::{
    evaluate_make_expr, evaluate_make_program_expression, evaluate_make_rule_expansion,
    MakeExprContext, MakeExprError, MakeProgramBudget,
};
use crate::{dirs::DirVars, make_vars::collect_vars, parser::TargetContext};
use std::{cell::RefCell, collections::BTreeMap, path::Path};

#[derive(Default)]
struct State {
    budget: MakeProgramBudget,
    values: RefCell<BTreeMap<String, String>>,
    effects: RefCell<Vec<String>>,
}

fn expression(raw: &str, state: &State) -> Result<String, MakeExprError> {
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let lookup = |name: &str| state.values.borrow().get(name).cloned();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("unit"))
        .with_lookup(&lookup)
        .without_filesystem();
    let apply = |statements: &str| {
        state.effects.borrow_mut().push(statements.to_owned());
        // Deliberately small test callback: the expression layer does not parse
        // or promise production support for arbitrary generated programs.
        let (name, rhs) =
            statements
                .split_once(":=")
                .ok_or_else(|| MakeExprError::InvalidSyntax {
                    expression: "test statement".into(),
                    detail: "expected one simple assignment".into(),
                })?;
        let name = name.trim();
        if !crate::make_vars::literal_variable_name(name) || rhs.contains('\n') {
            return Err(MakeExprError::InvalidSyntax {
                expression: "test statement".into(),
                detail: "invalid test assignment".into(),
            });
        }
        let frozen = expression(rhs.trim_start(), state)?;
        state
            .values
            .borrow_mut()
            .insert(name.into(), frozen.replace('$', "$$"));
        Ok(())
    };
    evaluate_make_program_expression(raw, &context, &state.budget, &apply)
}

#[test]
fn foreach_observes_each_effect_before_the_next_lookup_and_iteration() {
    let state = State::default();
    state
        .values
        .borrow_mut()
        .insert("ACC".into(), String::new());
    assert_eq!(
        expression("$(foreach v,a b,$(eval ACC := $(ACC)$(v))$(ACC))", &state).unwrap(),
        "a ab"
    );
    assert_eq!(state.effects.borrow().as_slice(), ["ACC := a", "ACC := ab"]);
    assert_eq!(expression("$(ACC)", &state).unwrap(), "ab");
}

#[test]
fn eval_second_expansion_freezes_current_values_not_later_values() {
    let state = State::default();
    state
        .values
        .borrow_mut()
        .insert("VALUE".into(), "old".into());
    expression("$(eval FROZEN := $$(VALUE))", &state).unwrap();
    state
        .values
        .borrow_mut()
        .insert("VALUE".into(), "new".into());
    assert_eq!(state.effects.borrow().as_slice(), ["FROZEN := $(VALUE)"]);
    assert_eq!(expression("$(FROZEN)", &state).unwrap(), "old");
    expression(
        "$(if yes,$(eval SELECTED := active),$(eval invalid))",
        &state,
    )
    .unwrap();
    assert_eq!(expression("$(SELECTED)", &state).unwrap(), "active");
}

#[test]
fn original_double_expansion_idiom_observes_explicit_single_character_binding() {
    let state = State::default();
    state.values.borrow_mut().extend([
        ("TMP_DIR".into(), "include".into()),
        ("i".into(), String::new()),
    ]);
    expression("$(eval TMP_DIRREMAIN := $$$(TMP_DIR))", &state).unwrap();
    assert_eq!(expression("$(TMP_DIRREMAIN)", &state).unwrap(), "nclude");
    assert_eq!(
        expression("$(subst $(TMP_DIRREMAIN),,$(TMP_DIR))", &state).unwrap(),
        "i"
    );
    state.values.borrow_mut().insert("i".into(), "X".into());
    expression("$(eval TMP_DIRREMAIN := $$$(TMP_DIR))", &state).unwrap();
    assert_eq!(expression("$(TMP_DIRREMAIN)", &state).unwrap(), "Xnclude");
    assert_eq!(
        expression("$(subst $(TMP_DIRREMAIN),,$(TMP_DIR))", &state).unwrap(),
        "include"
    );
    state.values.borrow_mut().remove("i");
    assert!(
        expression("$(eval TMP_DIRREMAIN := $$$(TMP_DIR))", &state).is_err(),
        "missing binding must not become a guessed empty value"
    );
}

#[test]
fn nested_effects_are_synchronous_and_literal_dollars_survive_the_second_parse() {
    let state = State::default();
    assert_eq!(
        expression(
            "$(eval OUTER := before$(eval INNER := ready)$(INNER))$(OUTER)",
            &state
        )
        .unwrap(),
        "beforeready"
    );
    assert_eq!(
        state.effects.borrow().as_slice(),
        ["INNER := ready", "OUTER := beforeready"]
    );
    expression("$(eval LITERAL := $$$$(MISSING))", &state).unwrap();
    assert_eq!(expression("$(LITERAL)", &state).unwrap(), "$(MISSING)");
}

#[test]
fn ordinary_and_rule_only_apis_do_not_admit_program_effects() {
    let scope = crate::make_vars::collect_vars_with_context("", &TargetContext::default());
    let dirs = DirVars::default();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("unit"))
        .without_filesystem();
    assert!(evaluate_make_expr("$(eval X := wrong)", &context).is_err());
    assert!(evaluate_make_rule_expansion("$(eval X := wrong)", &context).is_err());
    let calls = std::cell::Cell::new(0);
    let apply = |_: &str| {
        calls.set(calls.get() + 1);
        Err(MakeExprError::InvalidSyntax {
            expression: "effect".into(),
            detail: "explicit refusal".into(),
        })
    };
    assert!(evaluate_make_program_expression(
        "$(eval first)$(eval must-not-run)",
        &context,
        &MakeProgramBudget::default(),
        &apply
    )
    .is_err());
    assert_eq!(calls.get(), 1);
    assert!(evaluate_make_program_expression(
        "$(shell forbidden)",
        &context,
        &MakeProgramBudget::default(),
        &apply
    )
    .is_err());
    assert_eq!(calls.get(), 1);
}

#[test]
fn effect_count_is_bounded_before_the_next_callback() {
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("unit"))
        .without_filesystem();
    let calls = std::cell::Cell::new(0usize);
    let apply = |_: &str| {
        calls.set(calls.get() + 1);
        Ok(())
    };
    let valid = format!("$(foreach v,{},$(eval effect))", "x ".repeat(16_384));
    assert!(evaluate_make_program_expression(
        &valid,
        &context,
        &MakeProgramBudget::default(),
        &apply
    )
    .is_ok());
    assert_eq!(calls.get(), 16_384);
    calls.set(0);
    let excessive = format!("$(foreach v,{},$(eval effect))", "x ".repeat(16_385));
    assert!(matches!(
        evaluate_make_program_expression(
            &excessive,
            &context,
            &MakeProgramBudget::default(),
            &apply
        ),
        Err(MakeExprError::ResourceLimit {
            resource: "synchronous eval effects",
            ..
        })
    ));
    assert_eq!(calls.get(), 16_384);
}

#[test]
fn callback_state_is_not_falsely_claimed_transactional() {
    let state = State::default();
    assert!(expression("$(eval X := applied)$(eval invalid)", &state).is_err());
    assert_eq!(expression("$(X)", &state).unwrap(), "applied");
}

#[test]
fn recursive_callbacks_share_depth_and_effect_limits_and_unwind_depth_on_error() {
    fn recursive(
        budget: &MakeProgramBudget,
        calls: &std::cell::Cell<usize>,
    ) -> Result<String, MakeExprError> {
        let scope = collect_vars("");
        let dirs = DirVars::default();
        let context =
            MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("unit"))
                .without_filesystem();
        let apply = |_: &str| {
            calls.set(calls.get() + 1);
            recursive(budget, calls).map(|_| ())
        };
        evaluate_make_program_expression("$(eval recurse)", &context, budget, &apply)
    }
    let budget = MakeProgramBudget::default();
    let calls = std::cell::Cell::new(0);
    assert!(matches!(
        recursive(&budget, &calls),
        Err(MakeExprError::ResourceLimit {
            resource: "synchronous eval depth",
            ..
        })
    ));
    assert_eq!(calls.get(), 32);
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("unit"))
        .without_filesystem();
    assert_eq!(
        evaluate_make_program_expression("recovered", &context, &budget, &|_| Ok(())).unwrap(),
        "recovered"
    );
    let shared = MakeProgramBudget::default();
    let part = format!("$(foreach v,{},$(eval effect))", "x ".repeat(8192));
    let apply = |_: &str| Ok(());
    assert!(evaluate_make_program_expression(&part, &context, &shared, &apply).is_ok());
    assert!(evaluate_make_program_expression(&part, &context, &shared, &apply).is_ok());
    assert!(matches!(
        evaluate_make_program_expression("$(eval over)", &context, &shared, &apply),
        Err(MakeExprError::ResourceLimit {
            resource: "synchronous eval effects",
            ..
        })
    ));
}

#[test]
fn independent_expressions_share_emitted_work_in_one_program() {
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("unit"))
        .without_filesystem();
    let budget = MakeProgramBudget::default();
    let megabyte = "x".repeat(1024 * 1024);
    for _ in 0..16 {
        assert!(
            evaluate_make_program_expression(&megabyte, &context, &budget, &|_| Ok(())).is_ok()
        );
    }
    assert!(matches!(
        evaluate_make_program_expression(&megabyte, &context, &budget, &|_| Ok(())),
        Err(MakeExprError::ResourceLimit {
            resource: "aggregate emitted bytes",
            ..
        })
    ));
}

#[test]
fn unsupported_unicode_single_character_references_refuse_without_utf8_slicing_panics() {
    let state = State::default();
    assert!(expression("$π", &state).is_err());
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("unit"))
        .without_filesystem();
    assert!(evaluate_make_expr("$π", &context).is_err());
    assert_eq!(expression("literal-π", &state).unwrap(), "literal-π");
}
