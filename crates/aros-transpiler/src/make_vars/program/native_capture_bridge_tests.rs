//! The native invocation bridge composes captured inputs with one native packet.

use super::{MakeProgramPaths, MakeVariableProgram};
use crate::dirs::DirVars;
use aros_common::{
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
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

const SOURCE: &str = concat!(
    "LATER := early\n",
    "EARLY := $(SYNTH_ENV)\n",
    "LATER := late\n",
    "1CLI := source-is-ignored\n",
    "probe:\n",
);
const OBSERVED_EXPRESSION: &str = concat!(
    "$(origin SYNTH_ENV)|$(flavor SYNTH_ENV)|$(value SYNTH_ENV)|",
    "$(EARLY)|$(SYNTH_ENV)|",
    "$(origin 1CLI)|$(flavor 1CLI)|$(value 1CLI)",
);

fn make_binary() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

fn startup_arguments() -> Vec<String> {
    [
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
        "1CLI=command-value",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn synthetic_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PATH".into(), std::env::var("PATH").unwrap()),
        ("SYNTH_ENV".into(), "$(LATER)".into()),
        ("1CLI".into(), "synthetic-original-environment".into()),
    ])
}

fn native_output(
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
    observer: &str,
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
        .write_all(format!("{SOURCE}\n$(info BRIDGE_ACTUAL={OBSERVED_EXPRESSION})\n").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
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

fn native_record(output: &Output) -> String {
    assert!(
        output.status.success(),
        "synthetic GNU Make observation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let rows = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("BRIDGE_ACTUAL="))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "expected one synthetic native source record");
    rows[0].to_owned()
}

#[test]
fn native_bridge_replays_order_and_retains_original_export_membership() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let arguments = startup_arguments();
    let environment = synthetic_environment();
    let invocation = NativeMakeInvocation::new(
        arguments.clone(),
        cwd.clone(),
        environment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    )
    .unwrap();

    let startup = LiteralMakeStartupArguments::admit(&arguments).unwrap();
    let command_line_names = startup.command_line_names().clone();
    let discovery = format!(
        "{}{}",
        pre_file_fragment(),
        command_line_values_fragment(&command_line_names).unwrap()
    );
    let namespace_output = native_output(&cwd, &arguments, &environment, &discovery);
    let namespace = ValidatedMakeNamespace::from_make_stdout(&namespace_output.stdout).unwrap();
    let expected_environment_names = namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
        .map(|entry| entry.name)
        .collect::<BTreeSet<_>>();
    assert!(!expected_environment_names.contains("1CLI"));
    assert!(command_line_names.contains("1CLI"));

    let full_observation = format!(
        "{}{}{}",
        pre_file_fragment(),
        command_line_values_fragment(&command_line_names).unwrap(),
        environment_values_fragment(&expected_environment_names).unwrap()
    );
    let output = native_output(&cwd, &arguments, &environment, &full_observation);
    let expected = native_record(&output);

    let dirs = DirVars::default();
    let paths = path_context(&dirs, &cwd);
    let mut program = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        &output.stdout,
        &invocation,
        &expected_environment_names,
    )
    .unwrap();
    program
        .read_file("/dev/stdin", SOURCE, &paths, &|_, _| Ok(None))
        .unwrap();
    // Inspect both retained metadata and the native expression semantics.
    assert_eq!(
        program.state.origins.get("SYNTH_ENV"),
        Some(&MakeVariableOrigin::Environment)
    );
    assert_eq!(
        program.state.origins.get("1CLI"),
        Some(&MakeVariableOrigin::CommandLine)
    );
    for name in ["SYNTH_ENV", "1CLI"] {
        assert!(
            program.state.flavors.get(name) == Some(&crate::make_vars::VariableFlavor::Recursive)
        );
    }
    let fields = expected.split('|').collect::<Vec<_>>();
    assert_eq!(fields.len(), 8);
    assert_eq!(
        program.evaluate(OBSERVED_EXPRESSION, &paths).unwrap(),
        expected
    );
    assert_eq!(
        program
            .state
            .scope
            .raw_at("SYNTH_ENV", usize::MAX)
            .as_deref(),
        Some(fields[2])
    );
    assert_eq!(
        program
            .evaluate("$(EARLY)|$(SYNTH_ENV)|$(1CLI)", &paths)
            .unwrap(),
        format!("{}|{}", fields[3..5].join("|"), fields[7])
    );
    assert_eq!(
        expected,
        concat!(
            "environment|recursive|$(LATER)|early|late|",
            "command line|recursive|command-value"
        )
    );

    // 1CLI is absent from the final environment-origin namespace because argv
    // overrides it. Its original captured membership still proves its inherited
    // export flag, which cannot be inferred from the final command-line origin.
    let exports = program.source_export_values(&paths).unwrap();
    assert_eq!(
        exports.get("1CLI").map(String::as_str),
        Some("command-value")
    );
}

#[test]
fn bridge_rejects_empty_and_stale_captured_working_directories() {
    let expected_directory = tempfile::tempdir().unwrap();
    let stale_directory = tempfile::tempdir().unwrap();
    let expected_cwd = expected_directory.path().canonicalize().unwrap();
    let stale_cwd = stale_directory.path().canonicalize().unwrap();
    let dirs = DirVars::default();
    let paths = path_context(&dirs, &expected_cwd);
    let arguments = startup_arguments();
    let secret = "synthetic-private-capture-value-65bf";

    assert!(NativeMakeInvocation::new(
        arguments.clone(),
        PathBuf::new(),
        [("PRIVATE_INPUT".into(), secret.into())],
    )
    .is_err());

    let stale = NativeMakeInvocation::new(
        arguments,
        stale_cwd,
        [("PRIVATE_INPUT".into(), secret.into())],
    )
    .unwrap();
    let error = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        secret.as_bytes(),
        &stale,
        &BTreeSet::new(),
    )
    .err()
    .expect("stale captured cwd must be refused");
    assert!(error.contains("cwd"));
    assert!(!error.contains(secret));
}

#[test]
fn bridge_rejects_empty_or_unsupported_arguments_without_disclosing_data() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let dirs = DirVars::default();
    let paths = path_context(&dirs, &cwd);
    let secret = "synthetic-private-startup-value-c8a1";
    let environment = [("PRIVATE_INPUT".into(), secret.into())];

    for arguments in [Vec::new(), {
        let mut arguments = startup_arguments();
        arguments.push(format!("--eval={secret}"));
        arguments
    }] {
        let invocation =
            NativeMakeInvocation::new(arguments, cwd.clone(), environment.clone()).unwrap();
        let error = MakeVariableProgram::from_native_invocation_observation(
            &paths,
            secret.as_bytes(),
            &invocation,
            &BTreeSet::new(),
        )
        .err()
        .expect("empty and unsupported startup arguments must be refused");
        assert!(error.contains("startup arguments"));
        assert!(!error.contains(secret));
    }
}
