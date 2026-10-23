//! Source-state inspection functions checked against synthetic GNU Make runs.

use super::*;
use aros_common::{
    make_namespace::{
        MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
    },
    make_native_invocation::NativeMakeInvocation,
    make_observer::{command_line_values_fragment, environment_values_fragment, pre_file_fragment},
    make_startup_arguments::LiteralMakeStartupArguments,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

fn make_binary() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

fn startup_arguments(command_line: &[(&str, &str)]) -> Vec<String> {
    let mut arguments = [
        "-r",
        "-n",
        "-s",
        "--no-print-directory",
        "-f",
        "/dev/stdin",
        "-o",
        "/dev/stdin",
        "-o",
        "/dev/null",
        "probe",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    arguments.extend(
        command_line
            .iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    arguments
}

fn synthetic_environment(extra: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())]);
    environment.extend(
        extra
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
    );
    environment
}

fn native_output(
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    observer: &str,
    source: &str,
    expression: &str,
) -> Output {
    let mut command = Command::new(make_binary());
    command
        .args(["--eval"])
        .arg(observer)
        .args(arguments)
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{source}\n$(info AROS_INSPECTION_ACTUAL={expression})\n").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn native_record(output: &Output) -> String {
    assert!(
        output.status.success(),
        "synthetic GNU Make inspection failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let rows = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("AROS_INSPECTION_ACTUAL="))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "expected one native inspection result");
    rows[0].to_owned()
}

fn path_context<'a>(dirs: &'a DirVars, cwd: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: cwd,
        relative_dir: Path::new(""),
        top_directory: cwd,
        working_directory: cwd,
    }
}

fn assert_matches_gnu_make(
    source: &str,
    expression: &str,
    imported_environment: &[(&str, &str)],
    command_line: &[(&str, &str)],
) -> String {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let arguments = startup_arguments(command_line);
    let environment = synthetic_environment(imported_environment);
    let invocation = NativeMakeInvocation::new(
        arguments.clone(),
        cwd.clone(),
        environment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    )
    .unwrap();
    let startup = LiteralMakeStartupArguments::admit(&arguments).unwrap();
    let cli_names = startup.command_line_names().clone();
    let discovery = format!(
        "{}{}",
        pre_file_fragment(),
        command_line_values_fragment(&cli_names).unwrap()
    );
    let discovery_output = native_output(
        &cwd,
        &arguments,
        &environment,
        &discovery,
        source,
        expression,
    );
    assert!(
        discovery_output.status.success(),
        "synthetic pre-file observation failed: {}",
        String::from_utf8_lossy(&discovery_output.stderr)
    );
    let namespace = ValidatedMakeNamespace::from_make_stdout(&discovery_output.stdout).unwrap();
    let environment_names = namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
        .map(|entry| entry.name)
        .collect::<BTreeSet<_>>();
    let observer = format!(
        "{}{}",
        discovery,
        environment_values_fragment(&environment_names).unwrap()
    );
    let observed_output = native_output(
        &cwd,
        &arguments,
        &environment,
        &observer,
        source,
        expression,
    );
    let expected = native_record(&observed_output);

    let dirs = DirVars::default();
    let paths = path_context(&dirs, &cwd);
    let mut program = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        &observed_output.stdout,
        &invocation,
        &environment_names,
    )
    .unwrap();
    program
        .read_file("/dev/stdin", source, &paths, &|_, _| Ok(None))
        .unwrap();
    assert_eq!(program.evaluate(expression, &paths).unwrap(), expected);
    expected
}

#[test]
fn origin_flavor_and_value_follow_imports_overrides_simple_values_and_undefine() {
    let source = concat!(
        "LATER := first\n",
        "FILE_SIMPLE := $(LATER)\n",
        "FILE_RECURSIVE = $(LATER)\n",
        "LATER := second\n",
        "undefine ERASED\n",
        "probe:\n",
    );
    let expression = concat!(
        "$(origin )|$(flavor )|$(value )|",
        "$(origin IMPORTED)|$(flavor IMPORTED)|$(value IMPORTED)|$(IMPORTED)|",
        "$(origin CLI_INPUT)|$(flavor CLI_INPUT)|$(value CLI_INPUT)|$(CLI_INPUT)|",
        "$(origin FILE_SIMPLE)|$(flavor FILE_SIMPLE)|$(value FILE_SIMPLE)|$(FILE_SIMPLE)|",
        "$(origin FILE_RECURSIVE)|$(flavor FILE_RECURSIVE)|$(value FILE_RECURSIVE)|$(FILE_RECURSIVE)|",
        "$(origin ERASED)|$(flavor ERASED)|$(value ERASED)",
    );
    assert_eq!(
        assert_matches_gnu_make(
            source,
            expression,
            &[("IMPORTED", "$(LATER)"), ("ERASED", "from-environment")],
            &[("CLI_INPUT", "command-literal")],
        ),
        concat!(
            "undefined|undefined||",
            "environment|recursive|$(LATER)|second|",
            "command line|recursive|command-literal|command-literal|",
            "file|simple|first|first|",
            "file|recursive|$(LATER)|second|",
            "undefined|undefined|"
        )
    );
}

#[test]
fn foreach_variables_report_automatic_origin_and_simple_flavor() {
    let source = "probe:\n";
    let expression = "$(foreach item,alpha beta,[$(origin item):$(flavor item):$(value item)])";
    assert_eq!(
        assert_matches_gnu_make(source, expression, &[], &[]),
        "[automatic:simple:alpha] [automatic:simple:beta]"
    );
}

#[test]
fn dynamic_names_and_value_keep_recursive_and_simple_dollars_literal() {
    let source = concat!(
        "LATER := early\n",
        "RAW_RECURSIVE = dollar $$ $(LATER)\n",
        "RAW_SIMPLE := dollar $$ $(LATER)\n",
        "DYNAMIC_NAME := RAW_RECURSIVE\n",
        "LATER := late\n",
        "probe:\n",
    );
    let expression = concat!(
        "$(origin $(DYNAMIC_NAME))|$(flavor $(DYNAMIC_NAME))|$(value $(DYNAMIC_NAME))|",
        "$(RAW_RECURSIVE)|$(origin RAW_SIMPLE)|$(flavor RAW_SIMPLE)|",
        "$(value RAW_SIMPLE)|$(RAW_SIMPLE)",
    );
    assert_eq!(
        assert_matches_gnu_make(source, expression, &[], &[]),
        "file|recursive|dollar $$ $(LATER)|dollar $ late|file|simple|dollar $ early|dollar $ early"
    );
}

#[test]
fn eval_of_value_reparses_the_raw_definition_at_the_second_expansion_stage() {
    let source = concat!(
        "LATER := before\n",
        "RAW_EVAL = GENERATED := $(LATER)\n",
        "LATER := after\n",
        "$(eval $(value RAW_EVAL))\n",
        "probe:\n",
    );
    let expression = concat!(
        "$(origin GENERATED)|$(flavor GENERATED)|$(value GENERATED)|$(GENERATED)|",
        "$(origin RAW_EVAL)|$(flavor RAW_EVAL)|$(value RAW_EVAL)",
    );
    assert_eq!(
        assert_matches_gnu_make(source, expression, &[], &[]),
        "file|simple|after|after|file|recursive|GENERATED := $(LATER)"
    );
}

#[test]
fn raw_value_is_inert_until_an_explicit_second_stage() {
    assert_eq!(
        assert_matches_gnu_make(
            "RAW = $(eval SHOULD_NOT_EXIST := changed)\nprobe:\n",
            "$(value RAW)|$(origin SHOULD_NOT_EXIST)",
            &[],
            &[],
        ),
        "$(eval SHOULD_NOT_EXIST := changed)|undefined"
    );
}

fn namespace(extra: &[(&str, MakeVariableOrigin)]) -> ValidatedMakeNamespace {
    let mut entries = vec![
        MakeNamespaceEntry {
            name: ".VARIABLES".into(),
            origin: MakeVariableOrigin::Default,
        },
        MakeNamespaceEntry {
            name: "MAKE_VERSION".into(),
            origin: MakeVariableOrigin::Default,
        },
    ];
    entries.extend(extra.iter().map(|(name, origin)| MakeNamespaceEntry {
        name: (*name).into(),
        origin: *origin,
    }));
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries,
    })
    .unwrap()
}

#[test]
fn measured_origin_does_not_fabricate_unmeasured_value_or_flavor() {
    let dirs = DirVars::default();
    let cwd: PathBuf = std::env::current_dir().unwrap();
    let paths = path_context(&dirs, &cwd);
    let measured_names = namespace(&[("UNMEASURED", MakeVariableOrigin::Environment)]);
    let mut program =
        MakeVariableProgram::new_source_program(&paths, &measured_names, &[]).unwrap();

    assert_eq!(
        program.evaluate("$(origin UNMEASURED)", &paths).unwrap(),
        "environment"
    );
    assert!(program.evaluate("$(flavor UNMEASURED)", &paths).is_err());
    assert!(program.evaluate("$(value UNMEASURED)", &paths).is_err());
}

#[test]
fn failed_dynamic_inspection_does_not_disclose_the_expanded_name_value() {
    let dirs = DirVars::default();
    let cwd = std::env::current_dir().unwrap();
    let paths = path_context(&dirs, &cwd);
    let private = "synthetic-private-inspection-61ca";
    let names = namespace(&[(private, MakeVariableOrigin::Environment)]);
    let mut program = MakeVariableProgram::new_source_program(&paths, &names, &[]).unwrap();
    program
        .execute(&format!("SELECTOR := {private}"), 0, &paths)
        .unwrap();
    let error = program
        .evaluate("$(flavor $(SELECTOR))", &paths)
        .unwrap_err();
    assert!(!error.to_string().contains(private));
    assert!(!format!("{error:?}").contains(private));
}

#[test]
fn repeated_raw_value_inspection_consumes_the_shared_expression_budget() {
    let dirs = DirVars::default();
    let cwd = std::env::current_dir().unwrap();
    let paths = path_context(&dirs, &cwd);
    let names = namespace(&[("RAW", MakeVariableOrigin::Environment)]);
    let raw = "$".repeat(20_000);
    let binding = MakeInitialValue {
        name: "RAW",
        value: &raw,
        flavor: MakeInitialFlavor::Recursive,
        origin: MakeVariableOrigin::Environment,
    };
    let mut program = MakeVariableProgram::new_source_program(&paths, &names, &[binding]).unwrap();
    let mut exhausted = false;
    for _ in 0..100 {
        match program.evaluate("$(value RAW)", &paths) {
            Ok(value) => assert_eq!(value, raw),
            Err(MakeExprError::ResourceLimit { .. }) => {
                exhausted = true;
                break;
            }
            Err(error) => panic!("unexpected raw inspection failure: {error:?}"),
        }
    }
    assert!(
        exhausted,
        "raw inspection must not reset its shared work budget"
    );
}
