use super::{
    MakeFileEventKind, MakeFileProgram, MakeIncludeResolver, MakeInitialFlavor, MakeInitialValue,
    MakeProgramPaths, MakeVariableProgram, VariableFlavor,
};
use crate::{dirs::DirVars, parser::TargetContext};
use aros_common::make_namespace::{
    MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
};
use std::{cell::RefCell, path::Path};

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new("."),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build"),
    }
}

fn read_into(
    program: &mut MakeVariableProgram,
    source: &str,
    includes: &MakeIncludeResolver<'_>,
) -> MakeFileProgram {
    let dirs = DirVars::from_config_text("");
    program
        .read_file("/dev/stdin", source, &paths(&dirs), includes)
        .unwrap()
}

fn read(
    source: &str,
    includes: &MakeIncludeResolver<'_>,
) -> (MakeVariableProgram, MakeFileProgram) {
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let file = read_into(&mut program, source, includes);
    (program, file)
}

fn value(program: &mut MakeVariableProgram, expression: &str) -> String {
    let dirs = DirVars::from_config_text("");
    program.evaluate(expression, &paths(&dirs)).unwrap()
}

fn namespace(origin: MakeVariableOrigin) -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
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
                origin,
            },
        ],
    })
    .unwrap()
}

fn initial_list(
    origin: MakeVariableOrigin,
    flavor: MakeInitialFlavor,
    value: &str,
) -> MakeVariableProgram {
    let dirs = DirVars::from_config_text("");
    MakeVariableProgram::new_with_initial_values(
        &TargetContext::default(),
        &[],
        &paths(&dirs),
        &namespace(origin),
        &[MakeInitialValue {
            name: "MAKEFILE_LIST",
            value,
            flavor,
            origin,
        }],
    )
    .unwrap()
}

fn null_include(name: &str, from: &str) -> Result<Option<String>, String> {
    match name {
        "/dev/null" => Ok(Some(String::new())),
        _ => Err(format!("unexpected synthetic include {name} from {from}")),
    }
}

fn no_include(_: &str, from: &str) -> Result<Option<String>, String> {
    if from.is_empty() {
        return Err("include lookup requires an input identity".into());
    }
    Ok(None)
}

fn native(source: &str, expression: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let input = format!("{source}$(info AROS_TEST_RESULT={expression})\n.PHONY: probe\nprobe:\n");
    let mut child = Command::new(make)
        .args([
            "-rR",
            "--no-print-directory",
            "-s",
            "-n",
            "-f",
            "/dev/stdin",
        ])
        .args(args)
        .arg("probe")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .envs(env.iter().copied())
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
        "synthetic Make oracle failed: {}",
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
fn root_repeated_includes_eval_and_absent_optional_input_keep_read_order() {
    let calls = RefCell::new(Vec::new());
    let includes = |name: &str, from: &str| -> Result<Option<String>, String> {
        calls.borrow_mut().push((name.to_owned(), from.to_owned()));
        match name {
            "/dev/null" => Ok(Some(String::new())),
            "/dev/null/AROS_TEST_ABSENT" => Ok(None),
            _ => Err(format!("unexpected synthetic include {name} from {from}")),
        }
    };
    let source = concat!(
        "ROOT := $(MAKEFILE_LIST)\ninclude /dev/null\nFIRST := $(MAKEFILE_LIST)\n",
        "include /dev/null\nSECOND := $(MAKEFILE_LIST)\n",
        "$(eval EVAL_SNAPSHOT := $(MAKEFILE_LIST))\n",
        "-include /dev/null/AROS_TEST_ABSENT\nOPTIONAL := $(MAKEFILE_LIST)\n",
    );
    let (mut program, file) = read(source, &includes);
    let expected = "/dev/stdin|/dev/stdin /dev/null|/dev/stdin /dev/null /dev/null|/dev/stdin /dev/null /dev/null|/dev/stdin /dev/null /dev/null";
    assert_eq!(
        value(
            &mut program,
            "$(ROOT)|$(FIRST)|$(SECOND)|$(EVAL_SNAPSHOT)|$(OPTIONAL)"
        ),
        expected
    );
    assert_eq!(
        calls.into_inner(),
        vec![
            ("/dev/null".into(), "/dev/stdin".into()),
            ("/dev/null".into(), "/dev/stdin".into()),
            ("/dev/null/AROS_TEST_ABSENT".into(), "/dev/stdin".into()),
        ]
    );
    let seen = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Include {
                requested, present, ..
            } => Some((requested.as_str(), *present)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        seen,
        vec![
            ("/dev/null", true),
            ("/dev/null", true),
            ("/dev/null/AROS_TEST_ABSENT", false)
        ]
    );
    assert_eq!(
        program
            .events()
            .iter()
            .filter(|event| event.input_effect)
            .count(),
        4
    );
    assert!(program
        .events()
        .iter()
        .filter(|event| event.input_effect)
        .all(|event| event.text.starts_with("MAKEFILE_LIST ")));
    assert!(program
        .events()
        .iter()
        .filter(|event| !event.text.starts_with("MAKEFILE_LIST "))
        .all(|event| !event.input_effect));
    assert_eq!(
        native(
            source,
            "$(ROOT)|$(FIRST)|$(SECOND)|$(EVAL_SNAPSHOT)|$(OPTIONAL)",
            &[],
            &[]
        ),
        expected
    );
}

#[test]
fn simple_recursive_and_recreated_lists_match_make_flavor_rules() {
    for (source, expected) in [
        (
            "LATER :=\nMAKEFILE_LIST := $(LATER)\ninclude /dev/null\nLATER := changed\n",
            "/dev/null",
        ),
        (
            "LATER :=\nMAKEFILE_LIST = $(LATER)\ninclude /dev/null\nLATER := changed\n",
            "changed /dev/null",
        ),
    ] {
        let (mut program, _) = read(source, &null_include);
        let modeled = value(&mut program, "$(MAKEFILE_LIST)");
        assert_eq!(modeled, expected);
        assert_eq!(modeled, native(source, "$(MAKEFILE_LIST)", &[], &[]));
    }
    let source = "LATER :=\nMAKEFILE_LIST = $(LATER)\ninclude /dev/null\nundefine MAKEFILE_LIST\ninclude /dev/null\n";
    let (mut program, _) = read(source, &null_include);
    assert_eq!(value(&mut program, "$(MAKEFILE_LIST)"), "/dev/null");
    assert_eq!(
        program.state.origins.get("MAKEFILE_LIST"),
        Some(&MakeVariableOrigin::File)
    );
    assert!(matches!(
        program.state.flavors.get("MAKEFILE_LIST"),
        Some(&VariableFlavor::Recursive)
    ));
    assert_eq!(
        native(
            source,
            "$(MAKEFILE_LIST)|$(origin MAKEFILE_LIST)|$(flavor MAKEFILE_LIST)",
            &[],
            &[]
        ),
        "/dev/null|file|recursive"
    );
}

#[test]
fn environment_eval_and_command_line_origins_match_first_input_rules() {
    let source = "SNAPSHOT := $(MAKEFILE_LIST)\n";
    let mut env = initial_list(
        MakeVariableOrigin::Environment,
        MakeInitialFlavor::Recursive,
        "environment-prefix",
    );
    read_into(&mut env, source, &no_include);
    assert_eq!(value(&mut env, "$(SNAPSHOT)"), "/dev/stdin");
    assert_eq!(
        env.state.origins.get("MAKEFILE_LIST"),
        Some(&MakeVariableOrigin::File)
    );
    assert!(matches!(
        env.state.flavors.get("MAKEFILE_LIST"),
        Some(&VariableFlavor::Simple)
    ));
    assert_eq!(
        native(
            source,
            "$(SNAPSHOT)",
            &[],
            &[("MAKEFILE_LIST", "environment-prefix")]
        ),
        "/dev/stdin"
    );

    let mut eval = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    eval.execute("MAKEFILE_LIST := eval-prefix", 0, &paths(&dirs))
        .unwrap();
    read_into(&mut eval, source, &no_include);
    assert_eq!(value(&mut eval, "$(SNAPSHOT)"), "/dev/stdin");
    assert_eq!(
        native(
            source,
            "$(SNAPSHOT)",
            &["--eval=MAKEFILE_LIST := eval-prefix"],
            &[]
        ),
        "/dev/stdin"
    );

    let mut cli = initial_list(
        MakeVariableOrigin::CommandLine,
        MakeInitialFlavor::Simple,
        "command-line-prefix",
    );
    let cli_source = "SNAPSHOT := $(MAKEFILE_LIST)\ninclude /dev/null\nAFTER := $(MAKEFILE_LIST)\n";
    read_into(&mut cli, cli_source, &null_include);
    let expected = "command-line-prefix|command-line-prefix";
    assert_eq!(value(&mut cli, "$(SNAPSHOT)|$(AFTER)"), expected);
    assert_eq!(
        native(
            cli_source,
            "$(SNAPSHOT)|$(AFTER)",
            &["MAKEFILE_LIST=command-line-prefix"],
            &[]
        ),
        expected
    );
}

#[test]
fn failed_read_rolls_back_first_input_and_simple_filename_dollars_are_literal() {
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let before = program.events().to_vec();
    assert!(!program.state.file_read_started);
    let dirs = DirVars::from_config_text("");
    assert!(program
        .read_file(
            "failed.mk",
            "TRANSIENT := value\nnot a supported Make statement\n",
            &paths(&dirs),
            &no_include
        )
        .is_err());
    assert_eq!(program.events().to_vec(), before);
    assert!(!program.state.file_read_started);
    read_into(&mut program, "", &no_include);
    assert!(program.state.file_read_started);
    assert_eq!(value(&mut program, "$(MAKEFILE_LIST)"), "/dev/stdin");
    let mut literal = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let dirs = DirVars::from_config_text("");
    literal
        .read_file("retry$cash.mk", "", &paths(&dirs), &no_include)
        .unwrap();
    assert_eq!(value(&mut literal, "$(MAKEFILE_LIST)"), "retry$cash.mk");

    let prior = literal.events().to_vec();
    assert!(literal
        .read_file("reserved\u{e000}.mk", "", &paths(&dirs), &no_include)
        .is_err());
    assert_eq!(literal.events(), prior);
    assert_eq!(value(&mut literal, "$(MAKEFILE_LIST)"), "retry$cash.mk");
}
