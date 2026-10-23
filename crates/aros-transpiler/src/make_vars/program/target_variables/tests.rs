use super::super::MakeFileEventKind;
use super::{MakeProgramPaths, MakeVariableProgram};
use crate::{dirs::DirVars, make_rule_header::RuleHeader, parser::TargetContext};
use std::path::Path;

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new("unit"),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build/unit"),
    }
}

fn program(undefined: &[&str]) -> MakeVariableProgram {
    MakeVariableProgram::new(&TargetContext::default(), undefined)
        .expect("synthetic initial Make state is valid")
}

#[test]
fn target_expression_batch_shares_scope_order_and_keeps_globals_unchanged() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut variables = program(&[]);
    read(
        &mut variables,
        "VALUE := global\nparent: VALUE += inherited\nchild: VALUE += local\n",
        &paths,
    );
    let values = variables
        .evaluate_target_expressions(
            &["parent", "child"],
            &["$(VALUE)", "$(flavor VALUE)", "$(value VALUE)"],
            &paths,
        )
        .unwrap();
    assert_eq!(values, ["global inherited local", "recursive", "local"]);
    assert_eq!(variables.evaluate("$(VALUE)", &paths).unwrap(), "global");
    assert!(variables
        .evaluate_target_expressions(
            &["parent", "child"],
            &["$(VALUE)", "$(eval VALUE := corrupt)", "$(VALUE)"],
            &paths
        )
        .is_err());
    assert_eq!(variables.evaluate("$(VALUE)", &paths).unwrap(), "global");
}

#[test]
fn target_expression_batch_checks_count_before_staging() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut variables = program(&[]);
    assert!(matches!(
        variables.evaluate_target_expressions(
            &["target"],
            &vec![""; super::super::MAX_STATEMENTS + 1],
            &paths
        ),
        Err(crate::make_expr::MakeExprError::ResourceLimit {
            resource: "Make target expression batch count",
            ..
        })
    ));
}

fn read(program: &mut MakeVariableProgram, source: &str, paths: &MakeProgramPaths<'_>) {
    program
        .read_file("synthetic.mk", source, paths, &|_, _| Ok(None))
        .expect("synthetic Make source parses");
}

fn target_value(
    program: &mut MakeVariableProgram,
    chain: &[&str],
    expression: &str,
    paths: &MakeProgramPaths<'_>,
) -> String {
    program
        .evaluate_target_expression(chain, expression, paths)
        .expect("selected target scope evaluates")
}

#[test]
fn immediate_target_assignments_follow_source_order_and_expand_per_target() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        concat!(
            "X = global\n",
            "t: X = local\n",
            "t: Y := $(X)\n",
            "u: Y := $(X)\n",
            "u: X = local\n",
            "a: X = alpha\n",
            "b: X = bravo\n",
            "a b: Z := $(X)\n",
        ),
        &paths,
    );

    assert_eq!(
        target_value(&mut program, &["t"], "$(X)|$(Y)", &paths),
        "local|local"
    );
    assert_eq!(
        target_value(&mut program, &["u"], "$(X)|$(Y)", &paths),
        "local|global"
    );
    assert_eq!(
        target_value(&mut program, &["a"], "$(X)|$(Z)", &paths),
        "alpha|alpha"
    );
    assert_eq!(
        target_value(&mut program, &["b"], "$(X)|$(Z)", &paths),
        "bravo|bravo"
    );
    assert_eq!(program.evaluate("$(X)", &paths).unwrap(), "global");
}

#[test]
fn target_and_global_flavors_are_independent_and_append_uses_current_globals() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        concat!(
            "BASE = base-before\n",
            "APPEND = append-before\n",
            "X = $(BASE)\n",
            "SIMPLE := simple-before\n",
            "t: X += +$(APPEND)\n",
            "t: SIMPLE += +$(APPEND)\n",
            "t: FROZEN := $(X)\n",
            "BASE = base-after\n",
            "APPEND = append-after\n",
            "SIMPLE := simple-after\n",
        ),
        &paths,
    );

    assert_eq!(
        target_value(&mut program, &["t"], "$(value X)|$(X)|$(flavor X)", &paths,),
        "+$(APPEND)|base-after +append-after|recursive"
    );
    assert_eq!(
        target_value(
            &mut program,
            &["t"],
            "$(value SIMPLE)|$(SIMPLE)|$(flavor SIMPLE)",
            &paths,
        ),
        "+$(APPEND)|simple-after +append-after|recursive"
    );
    assert_eq!(
        target_value(&mut program, &["t"], "$(FROZEN)|$(flavor FROZEN)", &paths,),
        "base-before +append-before|simple"
    );
    assert_eq!(
        program
            .evaluate("$(X)|$(flavor X)|$(SIMPLE)|$(flavor SIMPLE)", &paths)
            .unwrap(),
        "base-after|recursive|simple-after|simple"
    );
}

#[test]
fn conditional_target_assignment_does_not_survive_later_global_undefine() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        "X = global-before\nt: X ?= local-fallback\nundefine X\n",
        &paths,
    );

    assert_eq!(
        target_value(
            &mut program,
            &["t"],
            "$(origin X)|$(flavor X)|[$(X)]",
            &paths,
        ),
        "undefined|undefined|[]"
    );
}

#[test]
fn later_target_set_replaces_prior_append_and_retains_its_flavor() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        concat!(
            "X = global\n",
            "t: X += +append\n",
            "t: X := replacement\n",
            "u: X += +append\n",
            "u: X = replacement\n",
        ),
        &paths,
    );

    assert_eq!(
        target_value(&mut program, &["t"], "$(value X)|$(X)|$(flavor X)", &paths,),
        "replacement|replacement|simple"
    );
    assert_eq!(
        target_value(&mut program, &["u"], "$(value X)|$(X)|$(flavor X)", &paths,),
        "replacement|replacement|recursive"
    );
}

#[test]
fn empty_and_missing_global_append_bases_have_no_leading_separator() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["MISSING"]);
    read(
        &mut program,
        "X =\nempty: X += right\nabsent: MISSING += right\n",
        &paths,
    );

    for target in ["empty", "absent"] {
        let variable = if target == "empty" { "X" } else { "MISSING" };
        let expression = format!("$(value {variable})|[$({variable})]|$(flavor {variable})");
        assert_eq!(
            target_value(&mut program, &[target], &expression, &paths),
            "right|[right]|recursive"
        );
    }
}

#[test]
fn empty_append_preserves_inherited_separator_but_is_noop_after_local_append() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        concat!(
            "X = base\n",
            "empty: X +=\n",
            "repeat: X += one\n",
            "repeat: X +=\n",
        ),
        &paths,
    );

    assert_eq!(
        target_value(
            &mut program,
            &["empty"],
            "$(value X)|[$(X)]|$(flavor X)",
            &paths,
        ),
        "|[base ]|recursive"
    );
    assert_eq!(
        target_value(
            &mut program,
            &["repeat"],
            "$(value X)|[$(X)]|$(flavor X)",
            &paths,
        ),
        "one|[base one]|recursive"
    );
}

#[test]
fn target_chain_inherits_and_shadows_without_leaking_between_parents() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        concat!(
            "A = global-a\n",
            "X = global-x\n",
            "Y = global-y\n",
            "root: A = root-a\n",
            "root: X = root-x\n",
            "child: X = child-x\n",
            "child: Y = child-y\n",
            "grand: Z = grand-z\n",
            "other: A = other-a\n",
            "other: X = other-x\n",
        ),
        &paths,
    );

    assert_eq!(
        target_value(
            &mut program,
            &["root", "child", "grand"],
            "$(A)|$(X)|$(Y)|$(Z)",
            &paths,
        ),
        "root-a|child-x|child-y|grand-z"
    );
    assert_eq!(
        target_value(
            &mut program,
            &["other", "grand"],
            "$(A)|$(X)|$(Y)|$(Z)",
            &paths,
        ),
        "other-a|other-x|global-y|grand-z"
    );
    assert_eq!(
        program.evaluate("$(A)|$(X)|$(Y)", &paths).unwrap(),
        "global-a|global-x|global-y"
    );
    assert!(program
        .evaluate_target_expression(&["root", "child", "root"], "$(X)", &paths)
        .is_err());
}

#[test]
fn pattern_assignment_rhs_freezes_globally_not_against_previous_pattern_locals() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let file = program
        .read_file(
            "synthetic.mk",
            concat!(
                "X = global\n",
                "%.o: X = pattern\n",
                "%.o: Y := $(X)\n",
                "z.o: Y := $(X)\n",
            ),
            &paths,
            &|_, _| Ok(None),
        )
        .expect("pattern target assignments are retained");

    let frozen_y = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Rule(RuleHeader::TargetAssignment {
                targets,
                assignment,
            }) if assignment.starts_with("Y := ") => Some((targets.as_str(), assignment.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        frozen_y,
        vec![("%.o", "Y := global"), ("z.o", "Y := global")]
    );
    assert_eq!(
        program
            .evaluate_target_expression(&["z.o"], "$(X)|$(Y)", &paths)
            .unwrap(),
        "pattern|global"
    );
}

#[test]
fn interpreted_graph_first_parent_selects_shared_prerequisite_variable_scope() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["X"]);
    let file = program
        .read_file(
            "parents.mk",
            concat!(
                "left: X = left-scope\nright: X = right-scope\n",
                "left: | shared.o\nright: before shared.o\n",
                "before:\nshared.o:\n%.o: X += pattern\n",
            ),
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
    let rules = crate::make_interpreted_rules::collect(&file).unwrap();
    let instances = rules
        .rules
        .iter()
        .flat_map(|rule| rule.instantiate_explicit().unwrap())
        .collect();
    let graph = crate::make_rule_graph::compose(instances).unwrap();
    for (goals, expected) in [
        (["left", "right"], "left-scope pattern"),
        (["right", "left"], "right-scope pattern"),
    ] {
        let traversal =
            crate::make_rule_graph::traversal::select_explicit(&graph, &goals, &|_| Ok(false))
                .unwrap();
        let index = traversal
            .visits()
            .iter()
            .position(|visit| visit.target() == "shared.o")
            .unwrap();
        let chain = traversal.scope_chain(index).unwrap();
        assert_eq!(chain, [goals[0], "shared.o"]);
        assert_eq!(
            program
                .evaluate_target_expression(&chain, "$(X)", &paths)
                .unwrap(),
            expected
        );
        assert_eq!(
            traversal
                .visits()
                .iter()
                .filter(|visit| visit.target() == "shared.o")
                .count(),
            1
        );
    }
    assert_eq!(
        program.evaluate("$(origin X)", &paths).unwrap(),
        "undefined"
    );
}

#[test]
fn target_rhs_effect_is_refused_and_failed_file_read_rolls_back() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["TRANSIENT"]);
    read(&mut program, "GLOBAL := before\n", &paths);
    let prior_events = program.events().to_vec();

    assert!(program
        .read_file(
            "effects.mk",
            "TRANSIENT := transient\nt: X := $(eval GLOBAL := changed)\n",
            &paths,
            &|_, _| Ok(None),
        )
        .is_err());

    assert_eq!(program.events(), prior_events.as_slice());
    assert_eq!(program.evaluate("$(GLOBAL)", &paths).unwrap(), "before");
    assert_eq!(program.evaluate("$(TRANSIENT)", &paths).unwrap(), "");
}

#[test]
fn target_expression_rejects_effects_without_publishing_them() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["LEAK"]);
    read(&mut program, "t: X = local\n", &paths);
    let prior_events = program.events().to_vec();

    assert!(program
        .evaluate_target_expression(&["t"], "$(eval LEAK := changed)", &paths)
        .is_err());
    assert_eq!(program.events(), prior_events.as_slice());
    assert_eq!(
        program.evaluate("$(LEAK)|$(origin LEAK)", &paths).unwrap(),
        "|undefined"
    );
}

fn native_make_stdin(source: &str, goals: &[&str]) -> String {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let mut child = Command::new(make)
        .current_dir(std::env::temp_dir())
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .args(["-rR", "-s", "--no-print-directory", "-f", "-"])
        .args(goals)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("GNU Make is available for bounded stdin differential tests");
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin
        .write_all(source.as_bytes())
        .expect("synthetic Makefile is delivered on stdin");
    drop(stdin);
    let output = child
        .wait_with_output()
        .expect("GNU Make process completes");
    assert!(
        output.status.success(),
        "GNU Make failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("GNU Make output is UTF-8")
}

#[test]
fn native_gnu_make_confirms_ordered_and_per_target_simple_assignments() {
    let output = native_make_stdin(
        concat!(
            "X = global\n",
            "t: X = local\n",
            "t: Y := $(X)\n",
            "u: Y := $(X)\n",
            "u: X = local\n",
            "a: X = alpha\n",
            "b: X = bravo\n",
            "a b: Z := $(X)\n",
            ".PHONY: t u a b\n",
            "t:\n\t@printf '%s:%s|%s\\n' '$@' '$(X)' '$(Y)'\n",
            "u:\n\t@printf '%s:%s|%s\\n' '$@' '$(X)' '$(Y)'\n",
            "a:\n\t@printf '%s:%s|%s\\n' '$@' '$(X)' '$(Z)'\n",
            "b:\n\t@printf '%s:%s|%s\\n' '$@' '$(X)' '$(Z)'\n",
        ),
        &["t", "u", "a", "b"],
    );
    assert_eq!(
        output,
        "t:local|local\nu:local|global\na:alpha|alpha\nb:bravo|bravo\n"
    );
}

#[test]
fn native_gnu_make_confirms_deferred_append_overlay_and_raw_value() {
    let output = native_make_stdin(
        concat!(
            "BASE = base-before\n",
            "APPEND = append-before\n",
            "X = $(BASE)\n",
            "t: X += +$(APPEND)\n",
            "BASE = base-after\n",
            "APPEND = append-after\n",
            ".PHONY: t\n",
            "t:\n",
            "\t@printf 'raw=[%s]|expanded=[%s]|flavor=[%s]\\n' '$(value X)' '$(X)' '$(flavor X)'\n",
        ),
        &["t"],
    );
    assert_eq!(
        output,
        "raw=[+$(APPEND)]|expanded=[base-after +append-after]|flavor=[recursive]\n"
    );
}

#[test]
fn native_gnu_make_confirms_empty_append_whitespace() {
    let output = native_make_stdin(
        concat!(
            "X = base\n",
            "empty: X +=\n",
            "repeat: X += one\n",
            "repeat: X +=\n",
            ".PHONY: empty repeat\n",
            "empty:\n",
            "\t@printf 'empty=[%s]|raw=[%s]\\n' '$(X)' '$(value X)'\n",
            "repeat:\n",
            "\t@printf 'repeat=[%s]|raw=[%s]\\n' '$(X)' '$(value X)'\n",
        ),
        &["empty", "repeat"],
    );
    assert_eq!(
        output,
        "empty=[base ]|raw=[]\nrepeat=[base one]|raw=[one]\n"
    );
}
