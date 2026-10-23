//! Captured recipe expressions and exports share one transient frame.

use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation, ValidatedMakeNamespace},
    measure_tree_content_cas,
};
use std::{
    collections::BTreeMap,
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .to_vec(),
    })
    .unwrap()
}

fn paths<'a>(dirs: &'a DirVars, root: &'a Path, cwd: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: cwd,
    }
}

fn snapshot(root: &Path, cwd: &Path) -> MakeInputSnapshot {
    let content_sha256 = measure_tree_content_cas(root)
        .unwrap()
        .payload_digest_excluding(None);
    MakeInputSnapshot::capture(
        cwd,
        &[MakeInputRootSelection {
            root: root.into(),
            content_sha256,
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap()
}

fn automatic(target: &str, first: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("@".into(), target.into()), ("<".into(), first.into())])
}

#[test]
fn known_generated_input_list_entry_preserves_cli_priority_and_refuses_drift() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = b"target: private override NAME := literal\n";
    std::fs::write(root.join("generated.mk"), source).unwrap();
    let selected = snapshot(&root, &root);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
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
                name: "MAKEFILE_LIST".into(),
                origin: MakeVariableOrigin::CommandLine,
            },
        ],
    })
    .unwrap();
    let mut program = MakeVariableProgram::new_source_program(
        &paths,
        &namespace,
        &[MakeInitialValue {
            name: "MAKEFILE_LIST",
            value: "command-line-list",
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::CommandLine,
        }],
    )
    .unwrap();
    let before = program.events().to_vec();
    assert!(program
        .record_captured_input_list_entry(
            "absent.mk",
            &aros_common::sha256_bytes(source),
            &paths,
            &selected
        )
        .is_err());
    assert_eq!(program.events(), before);
    let reads = program
        .record_captured_input_list_entry(
            "generated.mk",
            &aros_common::sha256_bytes(source),
            &paths,
            &selected,
        )
        .unwrap();
    assert_eq!(reads.reads().len(), 1);
    assert_eq!(
        program
            .evaluate("$(MAKEFILE_LIST)|$(origin MAKEFILE_LIST)", &paths)
            .unwrap(),
        "command-line-list|command line"
    );
    let before = program.events().to_vec();
    std::fs::write(root.join("generated.mk"), b"changed\n").unwrap();
    assert!(program
        .record_captured_input_list_entry(
            "generated.mk",
            &aros_common::sha256_bytes(source),
            &paths,
            &selected
        )
        .is_err());
    assert_eq!(program.events(), before);
}

fn native_recipe_values(cwd: &Path, source: &str, expression: &str) -> Vec<(String, String)> {
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let input = format!(
        "{source}\none.out two.out: %.out: src/%.src\n\t@printf 'OBSERVED_COMMAND=%s\\nOBSERVED_EXPORT=%s\\n' '{expression}' \"$$FRAME\"\n"
    );
    let mut child = Command::new(make)
        .current_dir(cwd)
        .args([
            "-rR",
            "-s",
            "--no-print-directory",
            "-f",
            "-",
            "one.out",
            "two.out",
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
        "synthetic GNU Make recipe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut commands = Vec::new();
    let mut exports = Vec::new();
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("OBSERVED_COMMAND=") {
            commands.push(value.to_owned());
        } else if let Some(value) = line.strip_prefix("OBSERVED_EXPORT=") {
            exports.push(value.to_owned());
        }
    }
    assert_eq!(
        commands.len(),
        2,
        "unexpected synthetic Make output: {stdout}"
    );
    assert_eq!(
        exports.len(),
        2,
        "unexpected synthetic Make output: {stdout}"
    );
    commands.into_iter().zip(exports).collect()
}

#[test]
fn recipe_expression_and_recursive_exports_share_captured_frame_and_paths() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let cwd = root.join("work");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::create_dir(cwd.join("data")).unwrap();
    std::fs::write(cwd.join("data/a.src"), "a\n").unwrap();
    std::fs::write(cwd.join("data/z.src"), "z\n").unwrap();
    let source = "export FRAME = $@|$<|$(wildcard data/*.src)|$(realpath data/z.src data/a.src)\n";
    std::fs::write(cwd.join("root.mk"), source).unwrap();

    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &cwd);
    let snapshot = snapshot(&root, &cwd);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();
    let automatic = automatic("obj/unit.o", "src/unit.c");
    let expression =
        "compile $@ <- $< : $(wildcard data/*.src) : $(realpath data/z.src data/a.src)";

    let (projection, _) = program
        .evaluate_captured_recipe(expression, &automatic, &paths, &snapshot)
        .unwrap();
    let wildcard = "data/a.src data/z.src";
    let realpaths = format!(
        "{} {}",
        cwd.join("data/z.src").display(),
        cwd.join("data/a.src").display()
    );
    assert_eq!(
        projection.expression(),
        format!("compile obj/unit.o <- src/unit.c : {wildcard} : {realpaths}")
    );
    assert_eq!(
        projection.source_exports()["FRAME"],
        format!("obj/unit.o|src/unit.c|{wildcard}|{realpaths}")
    );
}

#[test]
fn recipe_automatic_frame_is_not_retained_by_ordinary_evaluation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(
        root.join("root.mk"),
        "export FRAME = $@|$<\nexport STATIC := stable\n",
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();
    let events_before = program.events().to_vec();

    let (projection, _) = program
        .evaluate_captured_recipe(
            "$@|$<",
            &automatic("target.o", "source.c"),
            &paths,
            &snapshot,
        )
        .unwrap();
    assert_eq!(projection.expression(), "target.o|source.c");
    assert_eq!(projection.source_exports()["FRAME"], "target.o|source.c");
    assert!(program.evaluate("$@|$<", &paths).is_err());
    assert_eq!(program.evaluate("$(STATIC)", &paths).unwrap(), "stable");
    assert_eq!(program.events(), events_before);
}

#[test]
fn unsupported_and_unbound_automatic_values_fail_without_events() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("root.mk"), "export KEEP := stable\n").unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();
    let events_before = program.events().to_vec();

    let unsupported = BTreeMap::from([("CFLAGS".into(), "-DUNRELATED".into())]);
    assert!(program
        .evaluate_captured_recipe("constant", &unsupported, &paths, &snapshot)
        .is_err());
    let partial = automatic("target.o", "");
    for expression in ["$?", "$%", "$*"] {
        assert!(program
            .evaluate_captured_recipe(expression, &partial, &paths, &snapshot)
            .is_err());
    }
    assert_eq!(program.events(), events_before);
}

#[test]
fn recipe_eval_effects_are_refused_and_rolled_back() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("root.mk"), "export KEEP := stable\n").unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();
    let events_before = program.events().to_vec();

    assert!(program
        .evaluate_captured_recipe(
            "before$(eval LEAK := transient)after",
            &automatic("target.o", "source.c"),
            &paths,
            &snapshot,
        )
        .is_err());
    assert_eq!(program.events(), events_before);
    assert_eq!(program.evaluate("$(LEAK)", &paths).unwrap(), "");
}

#[test]
fn synthetic_native_recipe_matches_per_target_expression_and_export_frames() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let cwd = root.join("work");
    std::fs::create_dir(&cwd).unwrap();
    for directory in ["data", "src"] {
        std::fs::create_dir(cwd.join(directory)).unwrap();
    }
    for file in ["data/a.src", "data/z.src", "src/one.src", "src/two.src"] {
        std::fs::write(cwd.join(file), "synthetic input\n").unwrap();
    }
    let source = "export FRAME = $@|$<|$(wildcard data/*.src)|$(realpath data/z.src data/a.src)\n";
    std::fs::write(cwd.join("root.mk"), source).unwrap();
    let expression =
        "compile $@ <- $< : $(wildcard data/*.src) : $(realpath data/z.src data/a.src)";

    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &cwd);
    let snapshot = snapshot(&root, &cwd);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();
    let native = native_recipe_values(&cwd, source, expression);

    let mut projected = Vec::new();
    for (target, first) in [("one.out", "src/one.src"), ("two.out", "src/two.src")] {
        let automatic = automatic(target, first);
        let (projection, _) = program
            .evaluate_captured_recipe(expression, &automatic, &paths, &snapshot)
            .unwrap();
        projected.push((
            projection.expression().to_owned(),
            projection.source_exports()["FRAME"].clone(),
        ));
    }

    assert_eq!(projected, native);
    assert_ne!(projected[0], projected[1]);
}

#[test]
fn captured_target_recipe_binds_inheritance_automatic_exports_and_inventory() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("inputs")).unwrap();
    std::fs::write(root.join("inputs/selected.inc"), b"selected input").unwrap();
    std::fs::write(
        root.join("root.mk"),
        concat!(
            "FLAGS = global\nparent: FLAGS = parent\n",
            "%.out: FLAGS += pattern\n",
            "one.out: FLAGS += one\ntwo.out: FLAGS = two\n",
            "export FRAME = $(FLAGS)|$@|$(wildcard inputs/*.inc)\n",
        ),
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();
    let events = program.events().to_vec();
    for (target, expected) in [("one.out", "parent pattern one"), ("two.out", "two")] {
        let (projection, reads) = program
            .evaluate_captured_target_recipe(
                &["parent", target],
                "compile $@ $(FLAGS) $(wildcard inputs/*.inc)",
                &automatic(target, "source.c"),
                &paths,
                &snapshot,
            )
            .unwrap();
        assert_eq!(
            projection.expression(),
            format!("compile {target} {expected} inputs/selected.inc")
        );
        assert_eq!(
            projection.source_exports()["FRAME"],
            format!("{expected}|{target}|inputs/selected.inc")
        );
        // Wildcard enumeration validates the captured inventory, but is not a
        // content read and must not fabricate an include/read record.
        assert!(reads.reads().is_empty());
    }
    assert_eq!(program.events(), events);
    assert_eq!(program.evaluate("$(FLAGS)", &paths).unwrap(), "global");
    assert!(program
        .evaluate_captured_target_recipe(
            &["parent", "one.out"],
            "$(FLAGS)",
            &automatic("two.out", "source.c"),
            &paths,
            &snapshot
        )
        .is_err());
    assert!(program
        .evaluate_captured_target_recipe(
            &["parent", "one.out"],
            "$(eval FLAGS := changed)",
            &automatic("one.out", "source.c"),
            &paths,
            &snapshot
        )
        .is_err());
    assert_eq!(program.evaluate("$(FLAGS)", &paths).unwrap(), "global");
    std::fs::write(root.join("inputs/selected.inc"), b"changed input").unwrap();
    assert!(program
        .evaluate_captured_target_recipe(
            &["parent", "one.out"],
            "$(FLAGS)",
            &automatic("one.out", "source.c"),
            &paths,
            &snapshot
        )
        .is_err());
}

#[test]
fn target_recipe_batch_refuses_late_effects_frames_and_drift_without_state_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(
        root.join("root.mk"),
        concat!(
            "FLAGS = global\nparent: FLAGS = parent\n",
            "one.out: FLAGS += one\ntwo.out: FLAGS = two\n",
            "export FRAME = $(FLAGS)|$@|$<\n",
        ),
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();
    let events = program.events().to_vec();
    let one = automatic("one.out", "one.c");
    let two = automatic("two.out", "two.cc");
    let request = |expression| super::MakeTargetRecipeRequest {
        chain: &["parent", "one.out"],
        expression,
        automatic_variables: &one,
    };
    let requests = [
        request("$@ $(FLAGS)"),
        super::MakeTargetRecipeRequest {
            chain: &["parent", "two.out"],
            expression: "$@ $(FLAGS)",
            automatic_variables: &two,
        },
    ];
    let foreign_cwd = root.join("not-the-selected-cwd");
    let wrong_paths = MakeProgramPaths {
        working_directory: &foreign_cwd,
        ..paths
    };
    let before_work = program.state_work.get();
    assert!(program
        .evaluate_many_captured_target_recipes(&requests, &wrong_paths, &snapshot)
        .is_err());
    assert_eq!(program.state_work.get(), before_work);
    let (values, _) = program
        .evaluate_many_captured_target_recipes(&requests, &paths, &snapshot)
        .unwrap();
    assert_eq!(
        values
            .iter()
            .map(super::MakeRecipeProjection::expression)
            .collect::<Vec<_>>(),
        ["one.out parent one", "two.out two"]
    );
    assert_eq!(
        values[0].source_exports()["FRAME"],
        "parent one|one.out|one.c"
    );
    assert_eq!(values[1].source_exports()["FRAME"], "two|two.out|two.cc");
    for expression in ["$(eval FLAGS := changed)", "$(unsupported private)"] {
        assert!(program
            .evaluate_many_captured_target_recipes(
                &[request("$(FLAGS)"), request(expression)],
                &paths,
                &snapshot
            )
            .is_err());
        assert_eq!(program.events(), events);
        assert_eq!(program.evaluate("$(FLAGS)", &paths).unwrap(), "global");
    }
    let wrong = super::MakeTargetRecipeRequest {
        chain: &["parent", "two.out"],
        expression: "$(FLAGS)",
        automatic_variables: &one,
    };
    assert!(program
        .evaluate_many_captured_target_recipes(&[request("$(FLAGS)"), wrong], &paths, &snapshot)
        .is_err());
    assert!(program
        .evaluate_many_captured_target_recipes(
            &vec![request(""); super::MAX_STATEMENTS + 1],
            &paths,
            &snapshot
        )
        .is_err());
    std::fs::write(root.join("root.mk"), b"changed after selection\n").unwrap();
    assert!(program
        .evaluate_many_captured_target_recipes(&requests, &paths, &snapshot)
        .is_err());
    assert_eq!(program.events(), events);
}

#[test]
fn literal_shell_projection_keeps_make_prefixes_separate_from_command_words() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("input.mk"), b"selected synthetic input\n").unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    for (text, expected, expected_body, expected_command) in [
        (
            " \t@ - + @ tool 'quoted argument' 2>error",
            super::MakeRecipePrefixes {
                silent: true,
                ignore_errors: true,
                explicit_recursive: true,
            },
            "tool 'quoted argument' 2>error",
            "tool",
        ),
        (
            "'@tool' literal",
            super::MakeRecipePrefixes::default(),
            "'@tool' literal",
            "@tool",
        ),
        (
            "\\@tool literal",
            super::MakeRecipePrefixes::default(),
            "\\@tool literal",
            "@tool",
        ),
        (
            " \t tool literal  ",
            super::MakeRecipePrefixes::default(),
            "tool literal  ",
            "tool",
        ),
    ] {
        let (projection, _) = program
            .evaluate_captured_recipe(text, &BTreeMap::new(), &paths, &snapshot)
            .unwrap();
        let parsed = projection.parse_literal_shell().unwrap();
        assert_eq!(parsed.prefixes, expected);
        let (original_prefixes, original_body) = projection.original_shell_text().unwrap();
        assert_eq!(original_prefixes, expected);
        assert_eq!(original_body, expected_body);
        let [crate::make_shell_data::LiteralRecipeStatement::Command(command)] =
            parsed.statements.as_slice()
        else {
            panic!("expected one literal command")
        };
        assert_eq!(command.words()[0], expected_command);
        assert!(!format!("{parsed:?}").contains("quoted argument"));
    }
}

#[test]
fn original_shell_text_preserves_dynamic_body_without_literal_admission() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    // Make consumes $$, but the resulting shell text must retain quotes,
    // backticks, whitespace and short-circuit control exactly. No execution.
    let body = "for o in `cat 'generated list'` ; do printf '%s' \"$$o\" && printf 'done' ; done  ";
    let expected =
        "for o in `cat 'generated list'` ; do printf '%s' \"$o\" && printf 'done' ; done  ";
    let (projection, _) = program
        .evaluate_captured_recipe(
            &format!(" \t@-+ {body}"),
            &BTreeMap::new(),
            &paths,
            &snapshot,
        )
        .unwrap();
    let (prefixes, text) = projection.original_shell_text().unwrap();
    assert_eq!(text, expected);
    assert!(prefixes.silent && prefixes.ignore_errors && prefixes.explicit_recursive);
    assert!(projection.parse_literal_shell().is_err());
    assert_eq!(projection.expression(), format!(" \t@-+ {expected}"));
}
