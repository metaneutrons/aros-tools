//! Computed assignment names are selected from source data at read time.

use super::*;
use crate::make_rule_header::RuleHeader;
use aros_common::make_namespace::{
    MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
};
use std::{
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

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

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/selected/source"),
        relative_dir: Path::new(""),
        top_directory: Path::new("/selected/build"),
        working_directory: Path::new("/selected/build"),
    }
}

fn source_program(paths: &MakeProgramPaths<'_>) -> MakeVariableProgram {
    MakeVariableProgram::new_source_program(paths, &namespace(), &[]).unwrap()
}

/// Compare only controlled source text against GNU Make; the target has no recipe.
fn native(source: &str, expression: &str) -> String {
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let input = format!("{source}\n$(info AROS_TEST_RESULT={expression})\n.PHONY: probe\nprobe:\n");
    let mut child = Command::new(make)
        .args([
            "-rR",
            "--no-print-directory",
            "-s",
            "-n",
            "-f",
            "-",
            "probe",
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "synthetic computed-assignment Make query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let rows = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("AROS_TEST_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    rows[0].to_owned()
}

#[test]
fn computed_assignment_operators_freeze_names_and_preserve_rhs_flavor() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = source_program(&paths);
    let source = concat!(
        "PREFIX := ITEM_\n",
        "SUFFIX := BEFORE\n",
        "LATER := early\n",
        "$(PREFIX)$(SUFFIX) = $(LATER)\n",
        "SUFFIX := AFTER\n",
        "$(PREFIX)$(SUFFIX) := $(LATER)\n",
        "PREFIX := RENAMED_\n",
        "$(PREFIX)$(SUFFIX) ::= $(LATER)\n",
        "SELECTOR := SIMPLE_APPEND\n",
        "SIMPLE_APPEND := base\n",
        "$(SELECTOR) += $(LATER)\n",
        "SELECTOR := RECURSIVE_APPEND\n",
        "RECURSIVE_APPEND = base\n",
        "$(SELECTOR) += $(LATER)\n",
        "TOKEN := FOO=BAR\n",
        "$(subst =,_,${TOKEN}) ::= $(LATER)\n",
        "N := SHORT\n$N_suffix := short\n",
        "LATER := late\n",
    );
    program
        .read_file("synthetic.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();

    let expression = concat!(
        "$(ITEM_BEFORE)|$(ITEM_AFTER)|$(RENAMED_AFTER)|",
        "$(SIMPLE_APPEND)|$(RECURSIVE_APPEND)|$(FOO_BAR)|$(SELECTOR)|$(SHORT_suffix)",
    );
    let expected = "late|early|early|base early|base late|early|RECURSIVE_APPEND|short";
    assert_eq!(program.evaluate(expression, &paths).unwrap(), expected);
    assert_eq!(native(source, expression), expected);
}

#[test]
fn computed_conditional_evaluates_lhs_once_and_skips_a_defined_rhs() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = source_program(&paths);
    let source = concat!(
        "COUNT :=\n",
        "TARGET := kept\n",
        "$(eval COUNT += x)TARGET ?= $(error conditional RHS was expanded)\n",
        "DEFAULT_SELECTOR := CREATED\n",
        "${DEFAULT_SELECTOR} ?= created\n",
        "${DEFAULT_SELECTOR} ?= $(error existing default RHS was expanded)\n",
    );
    program
        .read_file("synthetic.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();

    let expression = "$(COUNT)|$(TARGET)|$(CREATED)";
    assert_eq!(
        program.evaluate(expression, &paths).unwrap(),
        "x|kept|created"
    );
    assert_eq!(native(source, expression), "x|kept|created");
}

#[test]
fn eval_can_read_a_computed_assignment_without_freezing_its_rhs() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = source_program(&paths);
    let source = concat!(
        "SELECTOR := GENERATED\n",
        "LATER := early\n",
        "$(eval $$(SELECTOR) = $$(LATER))\n",
        "SELECTOR := RENAMED\n",
        "LATER := late\n",
    );
    program
        .read_file("synthetic.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();

    let expression = "$(GENERATED)|$(RENAMED)";
    assert_eq!(program.evaluate(expression, &paths).unwrap(), "late|");
    assert_eq!(native(source, expression), "late|");
}

#[test]
fn rejected_computed_names_roll_back_values_and_events() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for (name, value) in [
        ("empty", ""),
        ("invalid", "two words"),
        ("injected", "INJECTED := unwanted"),
    ] {
        let mut program = source_program(&paths);
        program
            .read_file(
                "baseline.mk",
                "BASE := stable\nCOUNT := stable-count\n",
                &paths,
                &|_, _| Ok(None),
            )
            .unwrap();
        let prior_events = program.events().to_vec();
        let source = format!(
            "BASE := changed\nSTAGED := partial\nBAD_NAME := {value}\n\
             $(eval COUNT += leaked)$(BAD_NAME) := rejected\n"
        );

        assert!(
            program
                .read_file(name, &source, &paths, &|_, _| Ok(None))
                .is_err(),
            "{name} computed name must be refused"
        );
        assert_eq!(program.events(), prior_events);
        assert_eq!(
            program
                .evaluate("$(BASE)|$(COUNT)|$(STAGED)|$(INJECTED)", &paths)
                .unwrap(),
            "stable|stable-count||"
        );
    }
}

#[test]
fn skipped_conditional_does_not_expand_a_computed_lhs() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = source_program(&paths);
    let source = concat!(
        "COUNT :=\n",
        "ifeq (left,right)\n",
        "$(eval COUNT += leaked)TARGET := skipped\n",
        "endif\n",
    );
    program
        .read_file("synthetic.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();

    assert_eq!(program.evaluate("$(COUNT)|$(TARGET)", &paths).unwrap(), "|");
    assert_eq!(
        program
            .events()
            .iter()
            .filter(|event| !event.input_effect)
            .count(),
        1
    );
    assert_eq!(native(source, "$(COUNT)|$(TARGET)"), "|");
}

#[test]
fn target_specific_assignments_are_rule_data_not_global_computed_assignments() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = source_program(&paths);
    let source = "GLOBAL := before\ntarget: LOCAL = target-only\n";
    let file = program
        .read_file("synthetic.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();

    assert_eq!(
        program
            .events()
            .iter()
            .filter(|event| !event.input_effect)
            .count(),
        1
    );
    assert_eq!(
        program.evaluate("$(GLOBAL)|$(LOCAL)", &paths).unwrap(),
        "before|"
    );
    assert!(file.events.iter().any(|event| matches!(
        &event.kind,
        MakeFileEventKind::Rule(RuleHeader::TargetAssignment { targets, assignment })
            if targets == "target" && assignment == "LOCAL = target-only"
    )));

    let mut computed = source_program(&paths);
    computed
        .read_file(
            "baseline.mk",
            "SELECTOR := LOCAL\nGLOBAL := before\n",
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
    let prior_events = computed.events().to_vec();
    let error = computed
        .read_file(
            "target.mk",
            "target: $(SELECTOR) = target-only\n",
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("unsupported target-specific assignment syntax"));
    assert_eq!(computed.events(), prior_events);
    assert_eq!(
        computed.evaluate("$(GLOBAL)|$(LOCAL)", &paths).unwrap(),
        "before|"
    );
}

#[test]
fn computed_assignments_remain_source_only_in_legacy_coordinate_mode() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut projected = MakeVariableProgram::new_with_namespace(
        &TargetContext::default(),
        &[],
        &paths,
        &namespace(),
    )
    .unwrap();
    assert!(projected
        .read_file(
            "synthetic.mk",
            "NAME := COMPUTED\n$(NAME) := value\n",
            &paths,
            &|_, _| Ok(None),
        )
        .is_err());
    assert!(projected.events().is_empty());
    assert_eq!(projected.evaluate("$(NAME)", &paths).unwrap(), "");
    assert_eq!(
        projected.evaluate("$(TOP)", &paths).unwrap(),
        "${AROS_BUILD_DIR}"
    );
}
