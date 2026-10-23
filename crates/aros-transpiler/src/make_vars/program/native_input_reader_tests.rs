//! Native startup selection composed with descriptor-bound source reads.

use super::{MakeProgramPaths, MakeVariableProgram};
use crate::dirs::DirVars;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
    make_native_invocation::NativeMakeInvocation,
    make_observer::{command_line_values_fragment, environment_values_fragment, pre_file_fragment},
    make_startup_arguments::LiteralMakeStartupArguments,
    measure_tree_content_cas,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::{Command, Output, Stdio},
};

fn make_binary() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

fn ordered_arguments() -> Vec<String> {
    [
        "-r",
        "-R",
        "-n",
        "-s",
        "--no-print-directory",
        "-f",
        "first.mk",
        "-f",
        "second.mk",
        "-f",
        "first.mk",
        "probe",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn synthetic_environment() -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())])
}

fn native_output(
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    observer: &str,
) -> Output {
    let mut command = Command::new(make_binary());
    command
        .args(["--eval", observer])
        .args(arguments)
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.output().unwrap()
}

/// Obtain a genuine pre-file observation, then stop parsing before any -f file.
fn pre_file_observation(
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> (Vec<u8>, BTreeSet<String>) {
    let startup = LiteralMakeStartupArguments::admit(arguments).unwrap();
    let cli = command_line_values_fragment(startup.command_line_names()).unwrap();
    let discovery = format!(
        "{}{}$(error AROS_TEST_STOP_BEFORE_FILES)\n",
        pre_file_fragment(),
        cli
    );
    let first = native_output(cwd, arguments, environment, &discovery);
    assert!(!first.status.success());
    let namespace = ValidatedMakeNamespace::from_make_stdout(&first.stdout).unwrap();
    let environment_names = namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
        .map(|entry| entry.name)
        .collect::<BTreeSet<_>>();
    let complete = format!(
        "{}{}{}$(error AROS_TEST_STOP_BEFORE_FILES)\n",
        pre_file_fragment(),
        cli,
        environment_values_fragment(&environment_names).unwrap()
    );
    let output = native_output(cwd, arguments, environment, &complete);
    assert!(!output.status.success());
    (output.stdout, environment_names)
}

fn invocation(
    arguments: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
) -> NativeMakeInvocation {
    NativeMakeInvocation::new(
        arguments.to_vec(),
        cwd.to_owned(),
        environment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    )
    .unwrap()
}

fn paths<'a>(dirs: &'a DirVars, cwd: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: cwd,
        relative_dir: Path::new(""),
        top_directory: cwd,
        working_directory: cwd,
    }
}

fn snapshot(cwd: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        cwd,
        &[MakeInputRootSelection {
            root: cwd.to_owned(),
            content_sha256: measure_tree_content_cas(cwd)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
}

#[test]
fn native_captured_reader_preserves_repeated_input_order_and_assignment_timing() {
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    std::fs::write(
        cwd.join("first.mk"),
        concat!(
            "VALUE := first\n",
            "FIRST_SIMPLE := $(VALUE)\n",
            "FIRST_RECURSIVE = $(VALUE)\n",
            "FIRST_SEEN := $(MAKEFILE_LIST)\n",
        ),
    )
    .unwrap();
    std::fs::write(
        cwd.join("second.mk"),
        concat!(
            "VALUE := second\n",
            "SECOND_SEEN := $(MAKEFILE_LIST)\n",
            "SECOND_SIMPLE := $(FIRST_SIMPLE)\n",
            "SECOND_RECURSIVE := $(FIRST_RECURSIVE)\n",
        ),
    )
    .unwrap();

    let arguments = ordered_arguments();
    let environment = synthetic_environment();
    let (stdout, expected_environment_names) = pre_file_observation(&cwd, &arguments, &environment);
    let captured_invocation = invocation(&arguments, &cwd, &environment);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &cwd);
    let snapshot = snapshot(&cwd);
    let (mut program, file, reads) = MakeVariableProgram::read_native_captured_inputs(
        &paths,
        &stdout,
        &captured_invocation,
        &expected_environment_names,
        &snapshot,
    )
    .unwrap();

    let identities = reads
        .reads()
        .iter()
        .map(|read| (read.requested.as_str(), read.identity.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        vec![
            ("first.mk", "first.mk"),
            ("second.mk", "second.mk"),
            ("first.mk", "first.mk"),
        ]
    );
    let mut event_inputs = Vec::<&str>::new();
    for event in &file.events {
        if event_inputs.last().copied() != Some(event.input.as_str()) {
            event_inputs.push(event.input.as_str());
        }
    }
    assert_eq!(event_inputs, vec!["first.mk", "second.mk", "first.mk"]);

    let (observed, evaluation_reads) = program
        .evaluate_captured(
            concat!(
                "$(MAKEFILE_LIST)|$(FIRST_SEEN)|$(SECOND_SEEN)|$(VALUE)|",
                "$(FIRST_SIMPLE)|$(FIRST_RECURSIVE)|",
                "$(SECOND_SIMPLE)|$(SECOND_RECURSIVE)"
            ),
            &paths,
            &snapshot,
        )
        .unwrap();
    assert!(evaluation_reads.reads().is_empty());
    assert_eq!(
        observed,
        concat!(
            "first.mk second.mk first.mk|first.mk second.mk first.mk|",
            "first.mk second.mk|first|first|first|first|second"
        )
    );

    let stale_directory = tempfile::tempdir().unwrap();
    let stale_cwd = stale_directory.path().canonicalize().unwrap();
    let stale_invocation = invocation(&arguments, &stale_cwd, &environment);
    let error = MakeVariableProgram::read_native_captured_inputs(
        &paths,
        &stdout,
        &stale_invocation,
        &expected_environment_names,
        &snapshot,
    )
    .err()
    .expect("mismatched cwd must be refused");
    assert!(error.contains("cwd"));

    let mut controlled_environment = environment;
    let control_value = "synthetic-preload-value";
    controlled_environment.insert("MAKEFILES".into(), control_value.into());
    let controlled_invocation = invocation(&arguments, &cwd, &controlled_environment);
    let error = MakeVariableProgram::read_native_captured_inputs(
        &paths,
        &stdout,
        &controlled_invocation,
        &expected_environment_names,
        &snapshot,
    )
    .err()
    .expect("startup control environment must be refused");
    assert_eq!(error, "unsupported native inherited startup controls");
    assert!(!error.contains(control_value));
}

#[test]
fn native_captured_reader_refuses_late_missing_and_unsupported_inputs_atomically() {
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    std::fs::write(cwd.join("first.mk"), "PARTIAL := synthetic-value\n").unwrap();
    std::fs::write(cwd.join("unsupported.mk"), b"UNSUPPORTED := \xff\n").unwrap();
    let environment = synthetic_environment();
    let snapshot = snapshot(&cwd);
    let dirs = DirVars::default();
    let paths = paths(&dirs, &cwd);

    let missing_arguments = [
        "-r",
        "-R",
        "-n",
        "-s",
        "--no-print-directory",
        "-f",
        "first.mk",
        "-f",
        "missing.mk",
        "probe",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let (missing_stdout, missing_environment_names) =
        pre_file_observation(&cwd, &missing_arguments, &environment);
    let missing_invocation = invocation(&missing_arguments, &cwd, &environment);
    assert!(MakeVariableProgram::read_native_captured_inputs(
        &paths,
        &missing_stdout,
        &missing_invocation,
        &missing_environment_names,
        &snapshot,
    )
    .is_err());
    let mut missing_program = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        &missing_stdout,
        &missing_invocation,
        &missing_environment_names,
    )
    .unwrap();
    assert!(missing_program
        .read_captured_files(&["first.mk", "missing.mk"], &paths, &snapshot)
        .is_err());
    assert!(missing_program.events().is_empty());
    let (value, reads) = missing_program
        .evaluate_captured("$(PARTIAL)", &paths, &snapshot)
        .unwrap();
    assert!(value.is_empty());
    assert!(reads.reads().is_empty());

    let unsupported_arguments = [
        "-r",
        "-R",
        "-n",
        "-s",
        "--no-print-directory",
        "-f",
        "first.mk",
        "-f",
        "unsupported.mk",
        "probe",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let (unsupported_stdout, unsupported_environment_names) =
        pre_file_observation(&cwd, &unsupported_arguments, &environment);
    let unsupported_invocation = invocation(&unsupported_arguments, &cwd, &environment);
    assert!(MakeVariableProgram::read_native_captured_inputs(
        &paths,
        &unsupported_stdout,
        &unsupported_invocation,
        &unsupported_environment_names,
        &snapshot,
    )
    .is_err());
    let mut unsupported_program = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        &unsupported_stdout,
        &unsupported_invocation,
        &unsupported_environment_names,
    )
    .unwrap();
    let error = unsupported_program
        .read_captured_files(&["first.mk", "unsupported.mk"], &paths, &snapshot)
        .expect_err("unsupported source bytes must be refused");
    assert!(error.contains("UTF-8"));
    assert!(unsupported_program.events().is_empty());
    let (value, reads) = unsupported_program
        .evaluate_captured("$(PARTIAL)", &paths, &snapshot)
        .unwrap();
    assert!(value.is_empty());
    assert!(reads.reads().is_empty());
}
