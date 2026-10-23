//! Actual Source template -> generic Make -> literal shell AST vs GNU dry run.
//! The selected compile_q template is data; no compiler is launched.

use super::*;
use crate::{
    dirs::DirVars,
    genmf_projection::{expand_text, Limits},
    make_interpreted_rules,
    make_vars::{MakeProgramPaths, MakeTargetRecipeRequest, MakeVariableProgram},
};
use aros_common::{
    captured_programs::{CapturedProgramSet, ProgramRootSelection, ProgramSelection},
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{
        MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
    },
    measure_tree_content_cas, sha256_bytes,
};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

#[test]
fn actual_source_compile_wrapper_matches_gnu_shell_ast_without_compiler_execution() {
    let source = PathBuf::from(
        std::env::var_os("AROS_TEST_MESA26_SOURCE_ROOT").expect("real Source fixture is required"),
    )
    .canonicalize()
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let root_text = root.to_str().unwrap();
    assert!(!root_text.chars().any(char::is_whitespace));
    let program_directory = tempfile::tempdir().unwrap();
    let program_root = program_directory.path().canonicalize().unwrap();
    let sink = program_root.join("argv-sink");
    std::fs::write(
        &sink,
        "#!/bin/sh\nprintf '%s\\000' \"$@\" > \"$CAPTURE_ARGS\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&sink, std::fs::Permissions::from_mode(0o700)).unwrap();
    let programs = CapturedProgramSet::capture(
        &[ProgramRootSelection {
            root: program_root.clone(),
            content_sha256: measure_tree_content_cas(&program_root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
        &[ProgramSelection {
            role: "selected-command".into(),
            root_index: 0,
            invocation_path: sink.clone(),
            executable_sha256: sha256_bytes(&std::fs::read(&sink).unwrap()),
        }],
    )
    .unwrap();
    let template = source.join("config/make.tmpl");
    let expanded = expand_text(
        &format!("unit.o: unit.c\n\t%compile_q cmd=\"{}\" opt=\"$(OPTIONS)\" from=unit.c to=unit.o iquote=-iquote iquote_end= incextra=includes\n", sink.to_str().unwrap()),
        &source.join("synthetic-compile-wrapper.mmakefile.src"),
        &template,
        Limits::default(),
    ).unwrap();
    // Select this explicit macro invocation's actual retained output, not a
    // copied compile formula or the automatically appended common macro.
    let recipe: String = expanded
        .provenance
        .iter()
        .filter(|span| {
            span.macro_stack
                .first()
                .is_some_and(|frame| frame.name == "compile_q")
        })
        .map(|span| &expanded.text[span.output_start_byte..span.output_end_byte])
        .collect();
    assert!(recipe.contains(sink.to_str().unwrap()));
    let text = format!(
        "Q := @\nIF := if\nTEST := test\nECHO := echo\nNOP := :\nGENDIR := gen\nTOP := {root_text}\nSRCDIR := {root_text}\nCURDIR := .\nOPTIONS := -DORDER -UORDER -DORDER '-DSPACE=a b'\nroot: unit.o\nunit.o: unit.c\n{recipe}"
    );
    std::fs::write(root.join("input.mk"), &text).unwrap();
    std::fs::write(
        root.join("unit.c"),
        "synthetic compiler input, never compiled\n",
    )
    .unwrap();
    let snapshot = MakeInputSnapshot::capture(
        &root,
        &[MakeInputRootSelection {
            root: root.clone(),
            content_sha256: measure_tree_content_cas(&root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .to_vec(),
    })
    .unwrap();
    let mut variables = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
    let (file, _) = variables
        .read_captured_file("input.mk", &paths, &snapshot)
        .unwrap();
    let interpreted = make_interpreted_rules::collect(&file).unwrap();
    let rules: Vec<_> = interpreted
        .rules
        .iter()
        .flat_map(|rule| rule.instantiate_explicit().unwrap())
        .collect();
    let object = rules.iter().find(|rule| rule.target == "unit.o").unwrap();
    let automatic = object.automatic_variables();
    let requests: Vec<_> = object
        .recipes
        .iter()
        .map(|recipe| MakeTargetRecipeRequest {
            chain: &["root", "unit.o"],
            expression: &recipe.text,
            automatic_variables: &automatic,
        })
        .collect();
    let (values, _) = variables
        .evaluate_many_captured_target_recipes(&requests, &paths, &snapshot)
        .unwrap();
    let shells: Vec<_> = values
        .iter()
        .map(|value| value.parse_literal_shell().unwrap())
        .collect();
    assert!(shells.iter().all(|shell| shell.prefixes.silent
        && !shell.prefixes.ignore_errors
        && !shell.prefixes.explicit_recursive));
    let projected: Vec<_> = shells
        .into_iter()
        .flat_map(|shell| shell.statements)
        .collect();
    let bindings =
        crate::make_shell_data::program_binding::bind_literal_programs(&projected, &programs)
            .unwrap();
    let selected = bindings.unique_role("selected-command").unwrap();
    assert_eq!(selected.program().unwrap().invocation_path(), sink);
    assert_eq!(
        selected.position(),
        [
            crate::make_shell_data::program_binding::RecipeProgramStep::Statement(1),
            crate::make_shell_data::program_binding::RecipeProgramStep::Condition,
            crate::make_shell_data::program_binding::RecipeProgramStep::Statement(0),
        ]
    );
    assert!(bindings.unresolved().count() >= 4);
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let program = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join(make))
        .find(|path| path.is_file())
        .unwrap()
        .canonicalize()
        .unwrap();
    let executable_sha = sha256_bytes(&std::fs::read(&program).unwrap());
    let oracle = Command::new(&program)
        .args([
            "-rR",
            "-n",
            "--no-print-directory",
            "-f",
            "input.mk",
            "root",
        ])
        .env_clear()
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(oracle.status.success(), "GNU dry run failed");
    assert_eq!(
        executable_sha,
        sha256_bytes(&std::fs::read(&program).unwrap())
    );
    let native = parse_literal_recipe(std::str::from_utf8(&oracle.stdout).unwrap()).unwrap();
    assert_eq!(projected, native);
    let LiteralRecipeStatement::If {
        condition,
        then_body,
        else_body,
    } = &projected[1]
    else {
        panic!("actual compile condition is missing")
    };
    assert!(!then_body.is_empty() && !else_body.is_empty());
    let [LiteralRecipeStatement::Command(command)] = condition.as_slice() else {
        panic!("actual command shape differs")
    };
    assert_eq!(command.words()[0], sink.to_str().unwrap());
    assert!(command
        .words()
        .windows(3)
        .any(|words| words == ["-DORDER", "-UORDER", "-DORDER"]));
    assert!(command.words().contains(&"-DSPACE=a b".to_owned()));
    assert_eq!(
        command.redirections()[1].kind(),
        RecipeRedirectionKind::DuplicateOutput
    );
    assert_eq!(command.redirections()[1].target(), "1");
    for output in ["unit.o", "gen/cerrors", "gen/errors"] {
        assert!(!root.join(output).exists());
    }
    // Independent quote/argv oracle: run the unchanged Source wrapper with a
    // private argv sink, NOT a compiler. A dry-run text comparison alone would
    // run the same parser on both sides and could miss a shell-tokenization bug.
    std::fs::create_dir(root.join("gen")).unwrap();
    let sink_sha256 = sha256_bytes(&std::fs::read(&sink).unwrap());
    let executed = Command::new(&program)
        .args(["-rR", "--no-print-directory", "-f", "input.mk", "root"])
        .current_dir(&root)
        .env_clear()
        .env("CAPTURE_ARGS", root.join("argv.bin"))
        .output()
        .unwrap();
    assert!(
        executed.status.success(),
        "private Source wrapper argv oracle failed"
    );
    let bytes = std::fs::read(root.join("argv.bin")).unwrap();
    assert!(bytes.ends_with(&[0]));
    let arguments: Vec<_> = bytes[..bytes.len() - 1]
        .split(|byte| *byte == 0)
        .map(|bytes| std::str::from_utf8(bytes).unwrap())
        .collect();
    assert_eq!(arguments, command.words()[1..]);
    assert!(!root.join("unit.o").exists());
    assert!(!root.join("gen/errors").exists());
    assert_eq!(
        std::fs::metadata(root.join("gen/cerrors")).unwrap().len(),
        0
    );
    assert_eq!(sink_sha256, sha256_bytes(&std::fs::read(&sink).unwrap()));
    assert_eq!(
        executable_sha,
        sha256_bytes(&std::fs::read(&program).unwrap())
    );
    for input in &expanded.template_snapshots {
        assert_eq!(input.bytes, std::fs::read(&input.path).unwrap());
    }
    programs.verify().unwrap();
}
