use super::{capture, TargetCapture};
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::collect_vars_with_context;
use crate::parser::TargetContext;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const SOURCE_ROOT: &str = "/source";
const RELATIVE_DIR: &str = "workbench/mesa";

fn selected(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn active_states(program: &str) -> Vec<Option<bool>> {
    vec![Some(true); program.lines().count()]
}

fn capture_from_source(
    program: &str,
    selected_variables: &BTreeSet<String>,
    states: &[Option<bool>],
) -> Result<TargetCapture, String> {
    let scope = collect_vars_with_context(program, &TargetContext::default());
    let dirs = DirVars::default();
    let source_root = Path::new(SOURCE_ROOT);
    let relative_dir = Path::new(RELATIVE_DIR);
    capture(
        program,
        selected_variables,
        states,
        &|expression, line, overlay| {
            // Values already frozen for one target must remain literal when
            // the Make expression evaluator resolves a later assignment.
            let escaped_overlay = overlay
                .iter()
                .map(|(name, value)| (name.clone(), value.replace('$', "$$")))
                .collect::<BTreeMap<_, _>>();
            let lookup = |name: &str| escaped_overlay.get(name).cloned();
            let context = MakeExprContext::new(&scope, &dirs, line, source_root, relative_dir)
                .with_lookup(&lookup)
                .without_filesystem();
            evaluate_make_expr(expression, &context).map_err(|error| error.to_string())
        },
    )
}

#[test]
fn captures_c_targets_in_order_with_per_target_overlay_and_frozen_globals() {
    let program = concat!(
        "USER_CPPFLAGS := -DUSER_BEFORE\n",
        "TARGET_ISA_CFLAGS := -DISA\n",
        "GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)\n",
        "GLOBAL_EXTRA := -DGLOBAL_BEFORE\n",
        "OBJS := build/a.o build/b.o\n",
        "DFILES := build/a.d build/b.d\n",
        "$(OBJS): EXTRA := $(GLOBAL_EXTRA) -DFIRST\n",
        "$(OBJS): CFLAGS ::= $(EXTRA) $(GLOBAL_EXTRA)\n",
        "$(OBJS): GCA_ABI_FLAGS ::= $(GCA_ABI_FLAGS)\n",
        "USER_CPPFLAGS := -DUSER_AFTER\n",
        "GLOBAL_EXTRA := -DGLOBAL_AFTER\n",
        "GCA_ABI_FLAGS := -DGCA_AFTER\n",
        "$(DFILES): EXTRA := $(GLOBAL_EXTRA) -DSECOND\n",
        "$(DFILES): CFLAGS := $(EXTRA) $(GLOBAL_EXTRA)\n",
        "$(DFILES): GCA_ABI_FLAGS := $(GCA_ABI_FLAGS)\n",
        "$(OBJS): EXTRA := $(GLOBAL_EXTRA) -DREPEATED",
    );
    let capture = capture_from_source(
        program,
        &selected(&["EXTRA", "CFLAGS", "GCA_ABI_FLAGS"]),
        &active_states(program),
    )
    .unwrap();

    for object in ["build/a.o", "build/b.o"] {
        assert_eq!(
            capture.targets[object]["EXTRA"],
            "-DGLOBAL_AFTER -DREPEATED"
        );
        // CFLAGS captures the earlier target-local EXTRA value and the
        // earlier global value even though EXTRA is assigned again later.
        assert_eq!(
            capture.targets[object]["CFLAGS"],
            "-DGLOBAL_BEFORE -DFIRST -DGLOBAL_BEFORE"
        );
        assert_eq!(
            capture.targets[object]["GCA_ABI_FLAGS"],
            "-DISA -DUSER_BEFORE"
        );
    }
    for dependency in ["build/a.d", "build/b.d"] {
        assert_eq!(
            capture.targets[dependency]["EXTRA"],
            "-DGLOBAL_AFTER -DSECOND"
        );
        assert_eq!(
            capture.targets[dependency]["CFLAGS"],
            "-DGLOBAL_AFTER -DSECOND -DGLOBAL_AFTER"
        );
        assert_eq!(capture.targets[dependency]["GCA_ABI_FLAGS"], "-DGCA_AFTER");
    }

    assert_eq!(capture.assignments.len(), 14);
    assert!(capture
        .assignments
        .windows(2)
        .all(|pair| pair[0].line <= pair[1].line));
    assert_eq!(
        capture
            .assignments
            .iter()
            .filter(|assignment| assignment.target == "build/a.o" && assignment.variable == "EXTRA")
            .map(|assignment| assignment.value.as_str())
            .collect::<Vec<_>>(),
        ["-DGLOBAL_BEFORE -DFIRST", "-DGLOBAL_AFTER -DREPEATED"]
    );
}

#[test]
fn ignores_globals_unselected_target_variables_and_recipe_text() {
    let program = concat!(
        "OBJECT := one.o\n",
        "CFLAGS := -DGLOBAL\n",
        "$(OBJECT): OTHER := -DUNSELECTED\n",
        "$(OBJECT):\n",
        "\t$(OBJECT): CFLAGS := -DRECIPE",
    );
    let captured =
        capture_from_source(program, &selected(&["CFLAGS"]), &active_states(program)).unwrap();

    assert!(captured.targets.is_empty());
    assert!(captured.assignments.is_empty());
}

#[test]
fn inert_define_bodies_do_not_invent_target_captures() {
    let program = "define deferred\nwrong.o: CFLAGS := -DINERT\ndefine nested\nalso-wrong.o: CFLAGS := -DINERT\nendef\nendef\nright.o: CFLAGS := -DACTIVE\n";
    let result =
        capture_from_source(program, &selected(&["CFLAGS"]), &active_states(program)).unwrap();
    assert_eq!(result.targets.len(), 1);
    assert_eq!(result.targets["right.o"]["CFLAGS"], "-DACTIVE");
    assert_eq!(result.assignments.len(), 1);
}

#[test]
fn unselected_target_dependencies_still_shadow_global_values() {
    let program = "EXTRA := -DGLOBAL\none.o: EXTRA := -DLOCAL\none.o: CFLAGS := $(EXTRA)\ntwo.o: CFLAGS := $(EXTRA)\n";
    let result =
        capture_from_source(program, &selected(&["CFLAGS"]), &active_states(program)).unwrap();
    assert_eq!(result.targets["one.o"]["CFLAGS"], "-DLOCAL");
    assert_eq!(result.targets["two.o"]["CFLAGS"], "-DGLOBAL");
    assert!(!result.targets["one.o"].contains_key("EXTRA"));
    assert_eq!(result.assignments.len(), 2);
    let unsupported = program.replace("EXTRA := -DLOCAL", "EXTRA = -DLOCAL");
    assert_eq!(
        capture_from_source(
            &unsupported,
            &selected(&["CFLAGS"]),
            &active_states(&unsupported)
        )
        .unwrap_err(),
        "Make target capture requires an immediate target assignment"
    );
}

#[test]
fn false_assignment_is_skipped_and_unknown_selected_assignment_is_rejected() {
    let program = "OBJECT := one.o\n$(OBJECT): CFLAGS := -DSELECTED";
    let mut states = active_states(program);
    states[1] = Some(false);
    let skipped = capture_from_source(program, &selected(&["CFLAGS"]), &states).unwrap();
    assert!(skipped.targets.is_empty());
    assert!(skipped.assignments.is_empty());

    states[1] = None;
    assert!(capture_from_source(program, &selected(&["CFLAGS"]), &states).is_err());
}

#[test]
fn unresolved_rhs_and_recursive_or_append_assignments_are_rejected() {
    let unresolved = "OBJECT := one.o\n$(OBJECT): CFLAGS := $(MISSING_FLAGS)";
    assert!(capture_from_source(
        unresolved,
        &selected(&["CFLAGS"]),
        &active_states(unresolved)
    )
    .is_err());

    for program in [
        "OBJECT := one.o\n$(OBJECT): CFLAGS = -DRECURSIVE",
        "OBJECT := one.o\n$(OBJECT): CFLAGS += -DAPPENDED",
    ] {
        assert!(
            capture_from_source(program, &selected(&["CFLAGS"]), &active_states(program)).is_err()
        );
    }
}

#[test]
fn pattern_targets_duplicate_expansions_invalid_variables_and_bad_states_are_rejected() {
    let pattern = "%.o: CFLAGS := -DPATTERN";
    assert!(capture_from_source(pattern, &selected(&["CFLAGS"]), &active_states(pattern)).is_err());

    let duplicate = "OBJECTS := one.o two.o\n$(OBJECTS) one.o: CFLAGS := -DDUPLICATE";
    assert!(
        capture_from_source(duplicate, &selected(&["CFLAGS"]), &active_states(duplicate)).is_err()
    );

    let program = "OBJECT := one.o\n$(OBJECT): CFLAGS := -DVALUE";
    assert!(
        capture_from_source(program, &selected(&["CFLAGS BAD"]), &active_states(program)).is_err()
    );

    assert!(capture_from_source(program, &selected(&["CFLAGS"]), &[]).is_err());
}

#[test]
fn malformed_selected_variable_syntax_is_not_silently_ignored() {
    let program = "OBJECT := one.o\n$(OBJECT): CFLAGS EXTRA := -DMALFORMED";
    assert!(capture_from_source(program, &selected(&["CFLAGS"]), &active_states(program)).is_err());
}
