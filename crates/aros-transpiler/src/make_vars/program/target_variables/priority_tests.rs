use super::super::{MakeInitialFlavor, MakeInitialValue};
use super::{MakeProgramPaths, MakeVariableProgram};
use crate::{dirs::DirVars, parser::TargetContext};
use aros_common::make_namespace::{
    MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
};
use std::{fmt::Write as _, path::Path};

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new("unit"),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build/unit"),
    }
}

#[test]
fn command_line_priority_does_not_skip_immediate_rhs_evaluation() {
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
    program
        .read_file(
            "priority.mk",
            "X = global\nt: X := local\nt: Y := $(X)\n",
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
    assert_eq!(
        program
            .evaluate_target_expression(&["t"], "$(X)|$(Y)|$(origin X)", &paths)
            .unwrap(),
        "command|command|command line"
    );
    program
        .read_file(
            "ignored-append.mk",
            "t: X += $(eval LEAK := changed)\n",
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
    assert_eq!(
        program.evaluate("$(origin LEAK)", &paths).unwrap(),
        "undefined"
    );
    let before = program.events().to_vec();
    assert!(program
        .read_file(
            "effects.mk",
            "t: X := $(eval LEAK := changed)\n",
            &paths,
            &|_, _| Ok(None)
        )
        .is_err());
    assert_eq!(program.events(), before);
    assert_eq!(
        program.evaluate("$(origin LEAK)", &paths).unwrap(),
        "undefined"
    );
}

#[test]
fn inherited_append_preserves_literal_dollars_from_simple_parent() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    program
        .read_file(
            "dollars.mk",
            "X := $$literal\nroot: X := $$parent\nchild: X += $$child\n",
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
    assert_eq!(
        program
            .evaluate_target_expression(&["root", "child"], "$(X)|$(value X)", &paths)
            .unwrap(),
        "$parent $child|$$child"
    );
    assert_eq!(program.evaluate("$(X)", &paths).unwrap(), "$literal");
    for chain in [
        vec!["root child"],
        vec![" root"],
        vec!["child "],
        vec!["%.obj"],
    ] {
        assert!(program
            .evaluate_target_expression(&chain, "$(X)", &paths)
            .is_err());
    }
}

#[test]
fn target_count_limit_has_a_positive_and_one_more_counter() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for (count, accepted) in [(16_384, true), (16_385, false)] {
        let targets = (0..count)
            .map(|index| format!("t{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        assert_eq!(
            program
                .read_file(
                    "count.mk",
                    &format!("{targets}: X = value\n"),
                    &paths,
                    &|_, _| Ok(None)
                )
                .is_ok(),
            accepted
        );
        if accepted {
            assert_eq!(program.state.target_variables.len(), count);
            // Admission at the name-count limit does not reset the independent
            // cumulative work budget for later copies/queries of that state.
            assert!(matches!(
                program.evaluate_target_expression(&["t16383"], "$(X)", &paths),
                Err(crate::make_expr::MakeExprError::ResourceLimit { .. })
            ));
        } else {
            assert!(program
                .evaluate_target_expression(&["t0"], "$(X)", &paths)
                .is_err());
        }
    }
}

#[test]
fn immediate_reads_do_not_copy_unrelated_global_history_per_target() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut source = String::new();
    for index in 0..128 {
        writeln!(source, "PADDING{index} = {}", "x".repeat(8192)).unwrap();
    }
    for index in 0..16 {
        write!(source, "t{index}: X = local{index}\nt{index}: Y := $(X)\n").unwrap();
    }
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    program
        .read_file("bounded.mk", &source, &paths, &|_, _| Ok(None))
        .unwrap();
    assert_eq!(
        program
            .evaluate_target_expression(&["t15"], "$(Y)", &paths)
            .unwrap(),
        "local15"
    );
}

#[test]
fn immediate_eval_cannot_hide_effects_that_only_add_file_events() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for effect in ["$(eval u: Z = changed)", "$(eval .PHONY: injected)"] {
        let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
        let error = program
            .read_file(
                "effects.mk",
                &format!("t: X := {effect}\n"),
                &paths,
                &|_, _| Ok(None),
            )
            .unwrap_err();
        assert!(error.to_string().contains("unsupported state effects"));
        assert!(program.state.target_variables.is_empty());
        assert!(program.events().is_empty());
    }
}

#[test]
fn absent_default_is_local_and_decided_before_parent_inheritance() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &["X"]).unwrap();
    program
        .read_file(
            "defaults.mk",
            "root: X = parent\nchild: X ?= first\nchild: X ?= second\n",
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
    assert_eq!(
        program
            .evaluate_target_expression(&["root", "child"], "$(X)|$(origin X)|$(flavor X)", &paths)
            .unwrap(),
        "first|file|recursive"
    );
    assert_eq!(
        program.evaluate("$(origin X)", &paths).unwrap(),
        "undefined"
    );
}
