//! Transaction and expansion tests for the stateful Make variable program.

use super::{MakeProgramPaths, MakeVariableProgram};
use crate::dirs::DirVars;
use crate::make_expr::MakeExprError;
use crate::parser::TargetContext;
use std::path::Path;

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new("synthetic/module"),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build"),
    }
}

#[test]
fn initial_namespace_proves_absence_not_values_and_yields_to_source_writes() {
    use aros_common::make_namespace::ValidatedMakeNamespace;
    let namespace = ValidatedMakeNamespace::from_measurement(
        "v1:4;.VARIABLES|default;MAKE_VERSION|default;PRESENT|file;PRESENT-NAME|environment;end",
    )
    .unwrap();
    let dirs = DirVars::from_config_text(
        "PRESENT := stale-unrelated-value\nPRESENT-NAME := stale-hyphen-value\nABSENT := stale-absent-value",
    );
    let paths = paths(&dirs);
    let target = TargetContext::default();
    assert!(
        MakeVariableProgram::new_with_namespace(&target, &["PRESENT"], &paths, &namespace).is_err()
    );
    let mut program =
        MakeVariableProgram::new_with_namespace(&target, &[], &paths, &namespace).unwrap();
    assert_eq!(program.evaluate("$(ABSENT)", &paths).unwrap(), "");
    assert!(program.evaluate("$(PRESENT)", &paths).is_err());
    assert!(program.evaluate("$(PRESENT-NAME)", &paths).is_err());
    assert!(program.execute("PRESENT += guessed", 1, &paths).is_err());
    program.execute("ABSENT ?= first", 2, &paths).unwrap();
    assert_eq!(program.evaluate("$(ABSENT)", &paths).unwrap(), "first");
    program.execute("ABSENT += second", 3, &paths).unwrap();
    assert_eq!(
        program.evaluate("$(ABSENT)", &paths).unwrap(),
        "first second"
    );
    program
        .execute("PRESENT := measured-by-source", 4, &paths)
        .unwrap();
    assert_eq!(
        program.evaluate("$(PRESENT)", &paths).unwrap(),
        "measured-by-source"
    );
    program
        .observe("CONDITIONAL := maybe", 5, None, &paths)
        .unwrap();
    assert!(program.evaluate("$(CONDITIONAL)", &paths).is_err());
    assert!(program
        .execute("CONDITIONAL ?= guessed", 6, &paths)
        .is_err());
    assert_eq!(program.evaluate("$(not-a-scalar)", &paths).unwrap(), "");
    assert!(program.evaluate("$(@)", &paths).is_err());
    program
        .execute("1MISSING-NAME ?= first", 6, &paths)
        .unwrap();
    program
        .execute("1MISSING-NAME ?= second", 6, &paths)
        .unwrap();
    assert_eq!(
        program.evaluate("$(1MISSING-NAME)", &paths).unwrap(),
        "first"
    );
    program.execute("undefine PRESENT-NAME", 7, &paths).unwrap();
    assert_eq!(program.evaluate("$(PRESENT-NAME)", &paths).unwrap(), "");
    let mut partial = MakeVariableProgram::new(&target, &[]).unwrap();
    assert!(partial.evaluate("$(NEVER_SEEN)", &paths).is_err());
}

#[test]
fn measured_namespace_keeps_explicit_overlay_and_undefined_assertions() {
    use aros_common::make_namespace::ValidatedMakeNamespace;
    let namespace = ValidatedMakeNamespace::from_measurement(
        "v1:2;.VARIABLES|default;MAKE_VERSION|default;end",
    )
    .unwrap();
    let dirs = DirVars::from_config_text("MISSING-NAME := unrelated");
    let paths = paths(&dirs);
    let mut target = TargetContext::default();
    target
        .make_variables
        .insert("OVERLAY".into(), "bound-execution-value".into());
    let mut program =
        MakeVariableProgram::new_with_namespace(&target, &["EXPLICIT-NONE"], &paths, &namespace)
            .unwrap();
    assert_eq!(program.evaluate("$(MISSING-NAME)", &paths).unwrap(), "");
    assert_eq!(program.evaluate("$(EXPLICIT-NONE)", &paths).unwrap(), "");
    program.execute("OVERLAY ?= source", 1, &paths).unwrap();
    assert_eq!(
        program.evaluate("$(OVERLAY)", &paths).unwrap(),
        "bound-execution-value"
    );
}

fn event_snapshot(program: &MakeVariableProgram) -> Vec<(usize, usize, Option<bool>, String)> {
    program
        .events()
        .iter()
        .map(|event| {
            (
                event.ordinal,
                event.source_line,
                event.active,
                event.text.clone(),
            )
        })
        .collect()
}

#[test]
fn bound_coordinates_cannot_be_undefined_or_replaced() {
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);
    for name in ["SRCDIR", "TOP", "CURDIR"] {
        assert!(MakeVariableProgram::new(&TargetContext::default(), &[name]).is_err());
        let mut conflicting = TargetContext::default();
        conflicting
            .make_variables
            .insert(name.into(), "unbound".into());
        assert!(MakeVariableProgram::new(&conflicting, &[]).is_err());
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        let expression = format!("$({name})");
        let before = program.evaluate(&expression, &paths).unwrap();
        for statement in [format!("undefine {name}"), format!("{name} := changed")] {
            for activity in [Some(true), None] {
                let events = event_snapshot(&program);
                assert!(program.observe(&statement, 1, activity, &paths).is_err());
                assert_eq!(event_snapshot(&program), events);
                assert_eq!(program.evaluate(&expression, &paths).unwrap(), before);
            }
            program.observe(&statement, 1, Some(false), &paths).unwrap();
            assert_eq!(program.evaluate(&expression, &paths).unwrap(), before);
        }
    }
}

#[test]
fn initial_coordinate_assertions_require_independent_exact_path_binding() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut target = TargetContext::default();
    for (name, value) in [
        ("SRCDIR", "/synthetic/source"),
        ("TOP", "/synthetic/build"),
        ("CURDIR", "synthetic/module"),
    ] {
        target.make_variables.insert(name.into(), value.into());
    }
    assert!(MakeVariableProgram::new(&target, &[]).is_err());
    let mut program = MakeVariableProgram::new_with_paths(&target, &[], &paths).unwrap();
    assert_eq!(
        program
            .evaluate("$(SRCDIR)|$(TOP)|$(CURDIR)", &paths)
            .unwrap(),
        "${AROS_SOURCE_DIR}|${AROS_BUILD_DIR}|synthetic/module"
    );
    let drifted = MakeProgramPaths {
        working_directory: Path::new("/wrong"),
        ..paths
    };
    assert!(program.evaluate("$(TOP)", &drifted).is_err());
    assert!(program
        .observe("X := wrong", 0, Some(true), &drifted)
        .is_err());
    assert!(program.events().is_empty());
    for name in ["SRCDIR", "TOP", "CURDIR"] {
        let mut wrong = target.clone();
        wrong.make_variables.insert(name.into(), "/wrong".into());
        assert!(MakeVariableProgram::new_with_paths(&wrong, &[], &paths).is_err());
        assert!(MakeVariableProgram::new_with_paths(&target, &[name], &paths).is_err());
    }
}

#[test]
fn synthesized_configuration_is_bounded_and_part_of_definedness() {
    let mut target = TargetContext {
        cpu: Some("arm".into()),
        ..TargetContext::default()
    };
    assert!(MakeVariableProgram::new(&target, &["CPU"]).is_err());
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);
    let mut program = MakeVariableProgram::new(&target, &[]).unwrap();
    assert_eq!(program.evaluate("$(CPU)", &paths).unwrap(), "arm");
    target.cpu = Some("x".repeat(super::program::MAX_VALUE + 1));
    assert!(MakeVariableProgram::new(&target, &[]).is_err());
    target.cpu = Some("a".into());
    target.platform = Some("p".repeat(super::program::MAX_VALUE - 1));
    target.variant = Some(String::new());
    assert!(MakeVariableProgram::new(&target, &[]).is_err());
}

#[test]
fn unknown_branch_default_on_proved_definition_is_no_op() {
    let mut target = TargetContext::default();
    target.make_variables.insert("X".into(), "known".into());
    let mut program = MakeVariableProgram::new(&target, &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);
    program
        .observe("X ?= $(UNMEASURED)", 1, None, &paths)
        .unwrap();
    assert_eq!(program.evaluate("$(X)", &paths).unwrap(), "known");
    program.observe("undefine X", 2, None, &paths).unwrap();
    program.observe("X ?= branch", 3, None, &paths).unwrap();
    assert!(program.evaluate("$(X)", &paths).is_err());
}

#[test]
fn directory_lookup_does_not_prove_make_definedness() {
    let dirs = DirVars::from_config_text("PATH_FROM_DIRS := /synthetic/path");
    let paths = paths(&dirs);
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let before = program.evaluate("$(PATH_FROM_DIRS)", &paths).unwrap();
    assert!(!before.is_empty());
    program
        .observe("PATH_FROM_DIRS := inactive", 1, Some(false), &paths)
        .unwrap();
    for statement in ["PATH_FROM_DIRS ?= guessed", "PATH_FROM_DIRS += guessed"] {
        assert!(program.execute(statement, 2, &paths).is_err());
        assert_eq!(
            program.evaluate("$(PATH_FROM_DIRS)", &paths).unwrap(),
            before
        );
    }
}

#[test]
fn assignment_operators_keep_make_flavor_and_expansion_timing() {
    let context = TargetContext::default();
    let mut program = MakeVariableProgram::new(&context, &["DEFAULT"]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);

    for (line, statement) in [
        "BASE = early",
        "RECURSIVE = $(BASE)",
        "SIMPLE := $(BASE)",
        "APPENDED_RECURSIVE = prefix",
        "APPENDED_SIMPLE := prefix",
        "APPENDED_RECURSIVE += $(BASE)",
        "APPENDED_SIMPLE += $(BASE)",
        "DEFAULT ?= first",
        "DEFAULT ?= second",
        "BASE = late",
    ]
    .into_iter()
    .enumerate()
    {
        program
            .execute(statement, line + 1, &paths)
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }

    for (expression, expected) in [
        ("$(RECURSIVE)", "late"),
        ("$(SIMPLE)", "early"),
        ("$(APPENDED_RECURSIVE)", "prefix late"),
        ("$(APPENDED_SIMPLE)", "prefix early"),
        ("$(DEFAULT)", "first"),
    ] {
        assert_eq!(program.evaluate(expression, &paths).unwrap(), expected);
    }
}

#[test]
fn unmeasured_definedness_is_not_invented_by_default_append_or_false_assignments() {
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);
    assert!(program.execute("UNKNOWN ?= guessed", 1, &paths).is_err());
    assert!(program.execute("UNKNOWN += guessed", 1, &paths).is_err());
    program
        .observe("UNKNOWN := inactive", 1, Some(false), &paths)
        .unwrap();
    assert!(program.evaluate("$(UNKNOWN)", &paths).is_err());
    program.execute("UNKNOWN := proven", 2, &paths).unwrap();
    program.execute("UNKNOWN += suffix", 3, &paths).unwrap();
    assert_eq!(
        program.evaluate("$(UNKNOWN)", &paths).unwrap(),
        "proven suffix"
    );
}

#[test]
fn whitespace_and_pre_rhs_append_flavor_survive_nested_lhs_replacement() {
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);
    program.execute("A := left   ", 1, &paths).unwrap();
    program.execute("A += right  ", 2, &paths).unwrap();
    assert_eq!(program.evaluate("$(A)", &paths).unwrap(), "left    right  ");
    program.execute("X := a", 3, &paths).unwrap();
    program.execute("Y := y", 4, &paths).unwrap();
    program
        .execute("X += $(eval X = $$(Z))$(Y)", 5, &paths)
        .unwrap();
    program.execute("Z := later", 6, &paths).unwrap();
    assert_eq!(program.evaluate("$(X)", &paths).unwrap(), "$(Z) y");
    program.execute("Y := last", 7, &paths).unwrap();
    assert_eq!(program.evaluate("$(X)", &paths).unwrap(), "$(Z) y");
}

#[test]
fn eval_effects_commit_in_nested_expansion_and_are_visible_to_later_statements() {
    let context = TargetContext::default();
    let mut program = MakeVariableProgram::new(&context, &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);

    program.execute("VALUE = before", 1, &paths).unwrap();
    program.execute("FROZEN := $(VALUE)", 2, &paths).unwrap();
    program.evaluate("$(eval VALUE := after)", &paths).unwrap();
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "after");
    assert_eq!(program.evaluate("$(FROZEN)", &paths).unwrap(), "before");

    program
        .evaluate(
            "$(foreach item,alpha beta,$(eval GENERATED_$(item) := $(item)))",
            &paths,
        )
        .unwrap();
    program
        .execute("NEXT := $(GENERATED_alpha)-$(GENERATED_beta)", 3, &paths)
        .unwrap();
    assert_eq!(program.evaluate("$(NEXT)", &paths).unwrap(), "alpha-beta");
}

#[test]
fn failed_expression_and_assignment_fragments_roll_back_all_eval_effects() {
    let context = TargetContext::default();
    let mut program = MakeVariableProgram::new(&context, &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);

    let before_expression = event_snapshot(&program);
    assert!(program
        .evaluate("$(eval LEAK := expression)$(MISSING_AFTER_EVAL)", &paths)
        .is_err());
    assert_eq!(event_snapshot(&program), before_expression);

    let before_statement = event_snapshot(&program);
    assert!(program
        .execute(
            "BROKEN := $(eval LEAK := fragment)$(MISSING_AFTER_EVAL)",
            9,
            &paths,
        )
        .is_err());
    assert_eq!(event_snapshot(&program), before_statement);
    assert!(matches!(
        program.evaluate("$(LEAK)", &paths),
        Err(MakeExprError::UnresolvedVariables { .. })
    ));
}

#[test]
fn only_explicitly_undefined_names_expand_as_empty() {
    let context = TargetContext::default();
    let mut program = MakeVariableProgram::new(&context, &["KNOWN_EMPTY"]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);

    assert_eq!(program.evaluate("x$(KNOWN_EMPTY)y", &paths).unwrap(), "xy");
    assert!(matches!(
        program.evaluate("$(TYPO)", &paths),
        Err(MakeExprError::UnresolvedVariables { .. })
    ));
}

#[test]
fn false_and_unknown_assignments_are_isolated_until_an_unconditional_replacement() {
    let context = TargetContext {
        make_variables: [("FALSE_VALUE".into(), "configured".into())].into(),
        ..TargetContext::default()
    };
    let mut program = MakeVariableProgram::new(&context, &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);

    program
        .observe("FALSE_VALUE := branch", 1, Some(false), &paths)
        .unwrap();
    assert_eq!(
        program.evaluate("$(FALSE_VALUE)", &paths).unwrap(),
        "configured"
    );

    program
        .observe("CHOICE := speculative", 2, None, &paths)
        .unwrap();
    assert!(matches!(
        program.evaluate("$(CHOICE)", &paths),
        Err(MakeExprError::UnsafeVariable { name, .. }) if name == "CHOICE"
    ));

    program.execute("CHOICE := certain", 3, &paths).unwrap();
    assert_eq!(program.evaluate("$(CHOICE)", &paths).unwrap(), "certain");
    assert_eq!(
        program
            .events()
            .iter()
            .map(|event| event.active)
            .collect::<Vec<_>>(),
        vec![Some(false), None, Some(true)]
    );
}

#[test]
fn undefine_tombstone_masks_configured_fallback_and_allows_later_conditional_assignment() {
    let context = TargetContext {
        make_variables: [("CONFIGURED".into(), "fallback".into())].into(),
        ..TargetContext::default()
    };
    let mut program = MakeVariableProgram::new(&context, &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);

    assert_eq!(
        program.evaluate("$(CONFIGURED)", &paths).unwrap(),
        "fallback"
    );
    program.execute("undefine CONFIGURED", 1, &paths).unwrap();
    assert_eq!(program.evaluate("x$(CONFIGURED)y", &paths).unwrap(), "xy");
    program
        .execute("CONFIGURED ?= restored", 2, &paths)
        .unwrap();
    assert_eq!(
        program.evaluate("$(CONFIGURED)", &paths).unwrap(),
        "restored"
    );
}

#[test]
fn same_source_eval_writes_have_unique_increasing_ordinals_and_keep_their_line() {
    let context = TargetContext::default();
    let mut program = MakeVariableProgram::new(&context, &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);

    program
        .execute(
            "PAIR := $(eval FIRST := one)$(eval SECOND := two)",
            37,
            &paths,
        )
        .unwrap();

    let events = program.events();
    assert!(events
        .windows(2)
        .all(|pair| pair[0].ordinal < pair[1].ordinal));
    assert!(
        events
            .iter()
            .filter(|event| event.source_line == 37)
            .count()
            >= 2
    );
    assert_eq!(
        program.evaluate("$(FIRST) $(SECOND)", &paths).unwrap(),
        "one two"
    );
}

#[test]
fn unsupported_make_constructs_refuse_instead_of_being_dropped() {
    let context = TargetContext::default();
    let mut program = MakeVariableProgram::new(&context, &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);

    for statement in [
        "include forbidden.mk",
        "target: prerequisite",
        "define DYNAMIC\nVALUE := hidden\nendef",
        "COMMAND := $(shell uname -a)",
    ] {
        assert!(
            program.execute(statement, 1, &paths).is_err(),
            "unsupported statement was accepted: {statement}"
        );
    }
}
