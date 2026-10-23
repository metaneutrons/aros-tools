#![cfg(unix)]

use super::{bind_literal_program_batches, bind_literal_programs, RecipeProgramStep};
use crate::make_shell_data::parse_literal_recipe;
use aros_common::{
    captured_programs::{CapturedProgramSet, ProgramRootSelection, ProgramSelection},
    measure_tree_content_cas, sha256_bytes,
};
use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};

const PROGRAM_BYTES: &[u8] = b"opaque compiler fixture bytes; never executed";

#[test]
fn and_or_bindings_keep_every_operand_and_refuse_ambiguous_roles() {
    use crate::make_shell_data::LiteralRecipeStatement;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let invocation = root.join("selected");
    executable(&invocation, PROGRAM_BYTES);
    let programs = CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("selected", 0, invocation.clone(), PROGRAM_BYTES)],
    )
    .unwrap();
    let word = shell_quote(invocation.to_str().unwrap());
    let statements = parse_literal_recipe(&format!("false && {word} one || {word} two")).unwrap();
    let bound = bind_literal_programs(&statements, &programs).unwrap();
    assert_eq!(bound.commands().len(), 3);
    assert_eq!(bound.unresolved().count(), 1);
    assert!(bound.unique_role("selected").is_err());
    for (index, command) in bound.commands().iter().enumerate() {
        assert_eq!(
            command.position(),
            [
                RecipeProgramStep::Statement(0),
                RecipeProgramStep::AndOrOperand(index),
                RecipeProgramStep::Statement(0),
            ]
        );
    }
    let LiteralRecipeStatement::AndOr { first, .. } = &statements[0] else {
        panic!()
    };
    let malformed = LiteralRecipeStatement::AndOr {
        first: first.clone(),
        rest: Vec::new(),
    };
    assert!(bind_literal_programs(&[malformed], &programs).is_err());
}

#[test]
fn batch_keeps_order_and_refuses_late_or_aggregate_overflow_atomically() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let invocation = root.join("selected");
    executable(&invocation, PROGRAM_BYTES);
    let programs = CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("selected", 0, invocation.clone(), PROGRAM_BYTES)],
    )
    .unwrap();
    let selected = parse_literal_recipe(&shell_quote(invocation.to_str().unwrap())).unwrap();
    let unknown = parse_literal_recipe("echo still-visible").unwrap();
    let batch = bind_literal_program_batches(&[&selected, &unknown], &programs).unwrap();
    assert!(batch[0].unique_role("selected").is_ok());
    assert_eq!(batch[1].unresolved().count(), 1);
    let invalid = vec![selected[0].clone(); 4097];
    assert!(bind_literal_program_batches(&[&selected, &invalid], &programs).is_err());
    assert!(bind_literal_program_batches(&vec![selected.as_slice(); 65_537], &programs).is_err());
    let large = parse_literal_recipe(&"x".repeat(1024 * 1024 - 1)).unwrap();
    assert!(bind_literal_program_batches(&vec![large.as_slice(); 65], &programs).is_err());
}

fn executable(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn root_selection(root: &Path) -> ProgramRootSelection {
    let root = root.canonicalize().unwrap();
    ProgramRootSelection {
        content_sha256: measure_tree_content_cas(&root)
            .unwrap()
            .payload_digest_excluding(None),
        root,
    }
}

fn program(
    role: &str,
    root_index: usize,
    invocation_path: PathBuf,
    bytes: &[u8],
) -> ProgramSelection {
    ProgramSelection {
        role: role.to_owned(),
        root_index,
        invocation_path,
        executable_sha256: sha256_bytes(bytes),
    }
}

fn shell_quote(word: &str) -> String {
    let mut quoted = String::from("'");
    for character in word.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

#[test]
fn binds_only_the_exact_absolute_invocation_word() {
    let selected_root = tempfile::tempdir().unwrap();
    let foreign_root = tempfile::tempdir().unwrap();
    let selected = selected_root
        .path()
        .canonicalize()
        .unwrap()
        .join("bin/tool");
    let foreign = foreign_root.path().canonicalize().unwrap().join("bin/tool");
    executable(&selected, PROGRAM_BYTES);
    executable(&foreign, PROGRAM_BYTES);
    let programs = CapturedProgramSet::capture(
        &[root_selection(selected_root.path())],
        &[program(
            "target-compiler",
            0,
            selected.clone(),
            PROGRAM_BYTES,
        )],
    )
    .unwrap();

    let statements = parse_literal_recipe(&format!(
        "{} private-argument; {}; bin/tool; tool",
        shell_quote(selected.to_str().unwrap()),
        shell_quote(foreign.to_str().unwrap()),
    ))
    .unwrap();
    let bindings = bind_literal_programs(&statements, &programs).unwrap();

    assert_eq!(bindings.commands().len(), 4);
    assert_eq!(
        bindings.commands()[0].program().unwrap().role(),
        "target-compiler"
    );
    assert_eq!(
        bindings.commands()[0].position(),
        [RecipeProgramStep::Statement(0)]
    );
    assert!(bindings.commands()[1..]
        .iter()
        .all(|command| command.program().is_none()));
    assert_eq!(bindings.unresolved().count(), 3);
    assert_eq!(
        bindings
            .unique_role("target-compiler")
            .unwrap()
            .command()
            .words()[0],
        selected.to_str().unwrap()
    );
}

#[test]
fn a_program_valued_argument_does_not_bind_its_command() {
    let root = tempfile::tempdir().unwrap();
    let selected = root.path().canonicalize().unwrap().join("bin/compiler");
    executable(&selected, PROGRAM_BYTES);
    let programs = CapturedProgramSet::capture(
        &[root_selection(root.path())],
        &[program("compiler", 0, selected.clone(), PROGRAM_BYTES)],
    )
    .unwrap();
    let statements = parse_literal_recipe(&format!(
        "echo {}; {} -c input.c",
        shell_quote(selected.to_str().unwrap()),
        shell_quote(selected.to_str().unwrap()),
    ))
    .unwrap();

    let bindings = bind_literal_programs(&statements, &programs).unwrap();

    assert_eq!(bindings.commands().len(), 2);
    assert!(bindings.commands()[0].program().is_none());
    assert_eq!(
        bindings.commands()[0]
            .argument_role(1, "compiler")
            .unwrap()
            .invocation_path(),
        selected
    );
    assert!(bindings.commands()[0].argument_role(0, "compiler").is_err());
    assert!(bindings.commands()[0].argument_role(2, "compiler").is_err());
    assert!(bindings.commands()[0]
        .argument_role(1, "other-role")
        .is_err());
    assert_eq!(
        bindings.commands()[0].command().words()[1],
        selected.to_str().unwrap()
    );
    assert_eq!(bindings.commands()[1].program().unwrap().role(), "compiler");
    assert_eq!(bindings.unresolved().count(), 1);
}

#[test]
fn pipeline_stages_keep_positions_unknown_commands_and_auxiliary_alias_roles() {
    use crate::make_shell_data::LiteralRecipeStatement;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let driver = root.join("driver");
    let alias = root.join("driver-alias");
    let dispatch = root.join("dispatch");
    executable(&driver, PROGRAM_BYTES);
    executable(&dispatch, PROGRAM_BYTES);
    symlink("driver", &alias).unwrap();
    let programs = CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[
            program("driver", 0, driver, PROGRAM_BYTES),
            program("alias", 0, alias.clone(), PROGRAM_BYTES),
            program("dispatcher", 0, dispatch.clone(), PROGRAM_BYTES),
        ],
    )
    .unwrap();
    let statements = parse_literal_recipe(&format!(
        "if cat 'private input' | {} {} rs 'private archive'; then echo yes; else echo failed | cat; fi",
        shell_quote(dispatch.to_str().unwrap()),
        shell_quote(alias.to_str().unwrap()),
    ))
    .unwrap();
    let bound = bind_literal_programs(&statements, &programs).unwrap();
    assert_eq!(bound.commands().len(), 5);
    assert_eq!(bound.unresolved().count(), 4);
    let selected = bound.unique_role("dispatcher").unwrap();
    assert_eq!(
        selected.position(),
        [
            RecipeProgramStep::Statement(0),
            RecipeProgramStep::Condition,
            RecipeProgramStep::Statement(0),
            RecipeProgramStep::PipelineStage(1),
        ]
    );
    selected.argument_role(1, "alias").unwrap();
    assert!(selected.argument_role(1, "driver").is_err());
    assert!(bound.unique_role("alias").is_err());
    assert_eq!(
        bound.commands()[4].position(),
        [
            RecipeProgramStep::Statement(0),
            RecipeProgramStep::Else,
            RecipeProgramStep::Statement(0),
            RecipeProgramStep::PipelineStage(1),
        ]
    );
    let LiteralRecipeStatement::If { condition, .. } = &statements[0] else {
        panic!()
    };
    let LiteralRecipeStatement::Pipeline(stages) = &condition[0] else {
        panic!()
    };
    for malformed in [Vec::new(), vec![stages[0].clone()]] {
        assert!(
            bind_literal_programs(&[LiteralRecipeStatement::Pipeline(malformed)], &programs)
                .is_err()
        );
    }
    assert!(bind_literal_programs(
        &[LiteralRecipeStatement::Pipeline(vec![
            stages[0].clone();
            4097
        ])],
        &programs
    )
    .is_err());
    for private in ["private input", "private archive", "dispatcher"] {
        assert!(!format!("{bound:?}").contains(private));
    }
}

#[test]
fn retains_every_if_branch_and_rejects_ambiguous_role_occurrences() {
    let root = tempfile::tempdir().unwrap();
    let compiler = root.path().canonicalize().unwrap().join("bin/compiler");
    executable(&compiler, PROGRAM_BYTES);
    let programs = CapturedProgramSet::capture(
        &[root_selection(root.path())],
        &[program("compiler", 0, compiler.clone(), PROGRAM_BYTES)],
    )
    .unwrap();
    let compiler_word = shell_quote(compiler.to_str().unwrap());
    let statements = parse_literal_recipe(&format!(
        "if test -f input.c; then {compiler_word} -c input.c; else {compiler_word} --version; fi; echo diagnostic"
    ))
    .unwrap();

    let bindings = bind_literal_programs(&statements, &programs).unwrap();

    assert_eq!(bindings.commands().len(), 4);
    assert_eq!(
        bindings.commands()[0].position(),
        [
            RecipeProgramStep::Statement(0),
            RecipeProgramStep::Condition,
            RecipeProgramStep::Statement(0),
        ]
    );
    assert_eq!(
        bindings.commands()[1].position(),
        [
            RecipeProgramStep::Statement(0),
            RecipeProgramStep::Then,
            RecipeProgramStep::Statement(0),
        ]
    );
    assert_eq!(
        bindings.commands()[2].position(),
        [
            RecipeProgramStep::Statement(0),
            RecipeProgramStep::Else,
            RecipeProgramStep::Statement(0),
        ]
    );
    assert_eq!(
        bindings.commands()[3].position(),
        [RecipeProgramStep::Statement(1)]
    );
    assert_eq!(bindings.commands()[0].command().words()[0], "test");
    assert_eq!(bindings.commands()[3].command().words()[0], "echo");
    assert!(bindings.commands()[1..=2]
        .iter()
        .all(|command| command.program().is_some()));
    assert_eq!(bindings.unresolved().count(), 2);
    assert!(bindings.unique_role("compiler").is_err());
}

#[test]
fn distinct_selected_alias_roles_remain_distinct_for_identical_executable_bytes() {
    let root = tempfile::tempdir().unwrap();
    let primary = root.path().canonicalize().unwrap().join("bin/compiler");
    let alias = root
        .path()
        .canonicalize()
        .unwrap()
        .join("bin/compiler-alias");
    executable(&primary, PROGRAM_BYTES);
    symlink("compiler", &alias).unwrap();
    let programs = CapturedProgramSet::capture(
        &[root_selection(root.path())],
        &[
            program("primary-compiler", 0, primary.clone(), PROGRAM_BYTES),
            program("compat-compiler", 0, alias.clone(), PROGRAM_BYTES),
        ],
    )
    .unwrap();
    let statements = parse_literal_recipe(&format!(
        "{}; {}",
        shell_quote(alias.to_str().unwrap()),
        shell_quote(primary.to_str().unwrap()),
    ))
    .unwrap();

    let bindings = bind_literal_programs(&statements, &programs).unwrap();

    let compat = bindings
        .unique_role("compat-compiler")
        .unwrap()
        .program()
        .unwrap();
    let primary = bindings
        .unique_role("primary-compiler")
        .unwrap()
        .program()
        .unwrap();
    assert_eq!(compat.executable_sha256(), primary.executable_sha256());
    assert_eq!(compat.resolved_path(), primary.resolved_path());
    assert_ne!(compat.invocation_path(), primary.invocation_path());
    assert_eq!(compat.role(), "compat-compiler");
    assert_eq!(primary.role(), "primary-compiler");

    let arguments =
        parse_literal_recipe(&format!("echo {}", shell_quote(alias.to_str().unwrap()))).unwrap();
    let arguments = bind_literal_programs(&arguments, &programs).unwrap();
    let command = &arguments.commands()[0];
    assert!(command.argument_role(1, "compat-compiler").is_ok());
    assert!(command.argument_role(1, "primary-compiler").is_err());
}

#[test]
fn root_drift_refuses_the_complete_binding_result() {
    let root = tempfile::tempdir().unwrap();
    let compiler = root.path().canonicalize().unwrap().join("bin/compiler");
    executable(&compiler, PROGRAM_BYTES);
    let programs = CapturedProgramSet::capture(
        &[root_selection(root.path())],
        &[program("compiler", 0, compiler.clone(), PROGRAM_BYTES)],
    )
    .unwrap();
    let statements = parse_literal_recipe(&format!(
        "{} -c input.c",
        shell_quote(compiler.to_str().unwrap()),
    ))
    .unwrap();
    fs::write(
        root.path().join("later-entry"),
        b"root changed after capture",
    )
    .unwrap();

    let result = bind_literal_programs(&statements, &programs);

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(!error.contains("compiler"));
    assert!(!error.contains(compiler.to_str().unwrap()));
}

#[test]
fn program_drift_inside_result_derivation_refuses_the_whole_callback() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let invocation = root.join("selected");
    executable(&invocation, PROGRAM_BYTES);
    let programs = CapturedProgramSet::capture(
        &[root_selection(&root)],
        &[program("selected", 0, invocation.clone(), PROGRAM_BYTES)],
    )
    .unwrap();
    let selected = parse_literal_recipe(&shell_quote(invocation.to_str().unwrap())).unwrap();
    let result = super::with_literal_program_bindings(&[&selected], &programs, |bindings| {
        assert!(bindings[0].unique_role("selected").is_ok());
        fs::write(&invocation, b"changed inside callback").unwrap();
        Ok(bindings)
    });
    assert!(result.is_err());
}

#[test]
fn binding_debug_redacts_program_role_and_argument_values() {
    let root = tempfile::tempdir().unwrap();
    let compiler = root
        .path()
        .canonicalize()
        .unwrap()
        .join("bin/private-program-name");
    let program_bytes = b"private-program-bytes";
    executable(&compiler, program_bytes);
    let role = "PRIVATE_ROLE_VALUE";
    let argument = "PRIVATE_ARGUMENT_VALUE";
    let programs = CapturedProgramSet::capture(
        &[root_selection(root.path())],
        &[program(role, 0, compiler.clone(), program_bytes)],
    )
    .unwrap();
    let statements = parse_literal_recipe(&format!(
        "{} {argument}",
        shell_quote(compiler.to_str().unwrap()),
    ))
    .unwrap();
    let bindings = bind_literal_programs(&statements, &programs).unwrap();

    let debug = format!(
        "{bindings:?} {:?}",
        bindings.commands()[0].program().unwrap()
    );

    assert!(!debug.contains(role));
    assert!(!debug.contains(argument));
    assert!(!debug.contains(compiler.to_str().unwrap()));
    assert!(!debug.contains("private-program-bytes"));
}
