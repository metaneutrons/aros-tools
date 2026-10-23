use super::super::{MakeProgramPaths, MakeVariableProgram};
use super::stem_length;
use crate::{dirs::DirVars, parser::TargetContext};
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
fn equal_stem_pattern_operations_keep_source_order_across_patterns() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for (source, expected) in [
        (
            "a%.o: X = first\n%b.o: X = second\na%.o: X += +last\n",
            "second +last|second +last|recursive",
        ),
        (
            "%b.o: X = second\na%.o: X = first\na%.o: X += +last\n",
            "first +last|first +last|recursive",
        ),
    ] {
        let mut program = program(&[]);
        read(&mut program, source, &paths);
        assert_eq!(
            target_value(
                &mut program,
                &["ab.o"],
                "$(value X)|$(X)|$(flavor X)",
                &paths,
            ),
            expected
        );
    }
}

#[test]
fn broader_pattern_simple_set_precedes_narrower_pattern_append() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        "Y = old\na%.o: X += $(Y)\n%.o: X := fixed\nY = late\n",
        &paths,
    );

    assert_eq!(
        target_value(
            &mut program,
            &["ab.o"],
            "$(value X)|$(X)|$(flavor X)",
            &paths,
        ),
        "fixed late|fixed late|simple"
    );
}

#[test]
fn pattern_simple_set_freezes_at_read_but_append_rhs_is_deferred() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        "X = global\nY = old\n%.o: X := $(Y)\n%.o: X += $(Y)\nY = late\n",
        &paths,
    );

    assert_eq!(
        target_value(
            &mut program,
            &["file.o"],
            "$(value X)|$(X)|$(flavor X)|$(Y)",
            &paths,
        ),
        "old late|old late|simple|late"
    );
    assert_eq!(
        program.evaluate("$(X)|$(Y)", &paths).unwrap(),
        "global|late"
    );
}

#[test]
fn pattern_conditional_assignment_uses_final_global_definedness_and_target_precedence() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);

    let mut later_global = program(&["X"]);
    read(&mut later_global, "%.o: X ?= pattern\nX = later\n", &paths);
    assert_eq!(
        target_value(
            &mut later_global,
            &["file.o"],
            "$(X)|$(flavor X)|$(origin X)",
            &paths,
        ),
        "later|recursive|file"
    );

    let mut undefined_global = program(&[]);
    read(
        &mut undefined_global,
        "X = before\n%.o: X ?= pattern\nundefine X\n",
        &paths,
    );
    assert_eq!(
        target_value(
            &mut undefined_global,
            &["file.o"],
            "$(X)|$(flavor X)|$(origin X)",
            &paths,
        ),
        "pattern|recursive|file"
    );

    let mut inherited = program(&["X"]);
    read(
        &mut inherited,
        "root: X = parent\n%.o: X ?= pattern\n",
        &paths,
    );
    assert_eq!(
        target_value(
            &mut inherited,
            &["root", "file.o"],
            "$(X)|$(flavor X)",
            &paths,
        ),
        "pattern|recursive"
    );

    let mut explicit_leaf = program(&["X"]);
    read(
        &mut explicit_leaf,
        "%.o: X ?= pattern\nfile.o: X = leaf\n",
        &paths,
    );
    assert_eq!(
        target_value(&mut explicit_leaf, &["file.o"], "$(X)|$(flavor X)", &paths,),
        "leaf|recursive"
    );
}

#[test]
fn pattern_append_uses_inherited_parent_as_its_base_without_global_leak() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    read(
        &mut program,
        "X = global\nroot: X = parent\n%.o: X += frag\n",
        &paths,
    );

    assert_eq!(
        target_value(
            &mut program,
            &["root", "leaf.o"],
            "$(X)|$(value X)|$(flavor X)",
            &paths,
        ),
        "parent frag|frag|recursive"
    );
    assert_eq!(program.evaluate("$(X)", &paths).unwrap(), "global");
}

#[test]
fn pattern_append_rhs_sees_prior_pattern_values_but_not_inherited_or_leaf_values() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);

    let mut prior_pattern = program(&["Y"]);
    read(
        &mut prior_pattern,
        "X = global\n%.o: X = pattern\n%.o: Y := base\n%.o: Y += $(X)\n",
        &paths,
    );
    assert_eq!(
        target_value(
            &mut prior_pattern,
            &["leaf.o"],
            "$(X)|$(Y)|$(value Y)|$(flavor Y)",
            &paths,
        ),
        "pattern|base pattern|base pattern|simple"
    );
    assert_eq!(
        prior_pattern.evaluate("$(X)|$(origin Y)", &paths).unwrap(),
        "global|undefined"
    );

    let mut inherited = program(&[]);
    read(
        &mut inherited,
        "X = global\nroot: X = parent\nroot: child.o\n%.o: Y := base\n%.o: Y += $(X)\n",
        &paths,
    );
    assert_eq!(
        target_value(
            &mut inherited,
            &["root", "child.o"],
            "$(X)|$(Y)|$(value Y)|$(flavor Y)",
            &paths,
        ),
        "parent|base global|base global|simple"
    );

    let mut explicit_leaf = program(&[]);
    read(
        &mut explicit_leaf,
        "X = global\n%.o: X ?= pattern\n%.o: Y := base\n%.o: Y += $(X)\nleaf.o: X = leaf\n",
        &paths,
    );
    assert_eq!(
        target_value(
            &mut explicit_leaf,
            &["leaf.o"],
            "$(X)|$(Y)|$(value Y)|$(flavor Y)",
            &paths,
        ),
        "leaf|base global|base global|simple"
    );
}

#[test]
fn pattern_initialization_has_no_automatic_variables() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["@"]);
    read(
        &mut program,
        "%.o: X := prefix$@\n%.o: Y := prefix\n%.o: Y += $@\n",
        &paths,
    );

    assert_eq!(
        target_value(
            &mut program,
            &["ab.o"],
            "$(value X)|$(X)|$(value Y)|$(Y)|$(flavor X)|$(flavor Y)",
            &paths,
        ),
        "prefix|prefix|prefix|prefix|simple|simple"
    );
}

#[test]
fn pattern_matching_uses_full_target_spelling_and_only_the_first_percent() {
    assert_eq!(stem_length("a%.o", "dir/ab.o"), None);
    assert_eq!(stem_length("%.o", "dir/ab.o"), Some(6));
    assert_eq!(stem_length("a%b%c", "aXb%c"), Some(1));
    assert_eq!(stem_length("a%b%c", "aXbYc"), None);

    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["X"]);
    read(
        &mut program,
        "a%.o: X = a-pattern\n%.o: Y = generic-pattern\n",
        &paths,
    );
    assert_eq!(
        target_value(&mut program, &["dir/ab.o"], "$(origin X)|$(Y)", &paths,),
        "undefined|generic-pattern"
    );
}

#[test]
fn pattern_simple_dollars_are_literal_and_do_not_change_global_scope() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["Y"]);
    read(&mut program, "X = global\n%.o: Y := $$literal\n", &paths);

    assert_eq!(
        target_value(
            &mut program,
            &["leaf.o"],
            "$(Y)|$(value Y)|$(flavor Y)",
            &paths,
        ),
        "$literal|$literal|simple"
    );
    assert_eq!(
        program.evaluate("$(X)|$(origin Y)", &paths).unwrap(),
        "global|undefined"
    );
}

#[test]
fn deferred_pattern_append_effects_are_rejected_without_global_leak() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["LEAK"]);
    read(
        &mut program,
        "%.o: X := before\n%.o: X += $(eval LEAK := changed)\n",
        &paths,
    );
    let events = program.events().to_vec();

    assert!(program
        .evaluate_target_expression(&["leaf.o"], "$(X)", &paths)
        .is_err());
    assert_eq!(program.events(), events.as_slice());
    assert_eq!(
        program.evaluate("$(origin LEAK)", &paths).unwrap(),
        "undefined"
    );
}

#[test]
fn ignored_command_line_pattern_simple_set_still_evaluates_its_rhs() {
    use super::super::super::{MakeInitialFlavor, MakeInitialValue};
    use aros_common::make_namespace::{
        MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
    };

    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: vec![
            MakeNamespaceEntry {
                name: ".VARIABLES".into(),
                origin: MakeVariableOrigin::Default,
            },
            MakeNamespaceEntry {
                name: "MAKE_VERSION".into(),
                origin: MakeVariableOrigin::Default,
            },
            MakeNamespaceEntry {
                name: "X".into(),
                origin: MakeVariableOrigin::CommandLine,
            },
        ],
    })
    .unwrap();
    let mut program = MakeVariableProgram::new_source_program(
        &paths,
        &namespace,
        &[MakeInitialValue {
            name: "X",
            value: "command",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::CommandLine,
        }],
    )
    .unwrap();
    let events = program.events().to_vec();

    assert!(program
        .read_file(
            "ignored-pattern.mk",
            "%.o: X := $(eval LEAK := changed)\n",
            &paths,
            &|_, _| Ok(None),
        )
        .is_err());
    assert_eq!(program.events(), events.as_slice());
    assert_eq!(
        program.evaluate("$(origin LEAK)", &paths).unwrap(),
        "undefined"
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
fn native_gnu_make_confirms_pattern_order_specificity_and_deferred_rhs() {
    let output = native_make_stdin(
        concat!(
            "Y = old\n",
            "a%.o: X = first\n",
            "%b.o: X = second\n",
            "a%.o: X += +last\n",
            "%.o: Z := fixed\n",
            "a%.o: Z += $(Y)\n",
            "Y = late\n",
            ".PHONY: ab.o\n",
            "ab.o:\n",
            "\t@printf 'X=[%s]|raw=[%s]|flavor=[%s];Z=[%s]|raw=[%s]|flavor=[%s]\\n' '$(X)' '$(value X)' '$(flavor X)' '$(Z)' '$(value Z)' '$(flavor Z)'\n",
        ),
        &["ab.o"],
    );
    assert_eq!(
        output,
        "X=[second +last]|raw=[second +last]|flavor=[recursive];Z=[fixed late]|raw=[fixed late]|flavor=[simple]\n"
    );
}
