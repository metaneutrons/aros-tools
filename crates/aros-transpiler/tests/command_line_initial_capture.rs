//! Opt-in command-line values are observed from native Make in synthetic files.
//!
//! The pre-file and source-prelude cases intentionally use different argv
//! shapes. They exercise their respective observation phases, not production
//! nested-invocation or receipt parity.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

use aros_common::{
    make_initial_values::{
        MakeInitialValueFlavor, ValidatedMakeCommandLineValues, ValidatedMakeInitialValues,
        MAKE_COMMAND_LINE_VALUES_METADATA_KEY,
    },
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
    make_observer::{command_line_values_fragment, pre_file_fragment, source_prelude_fragment},
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::{MakeInitialFlavor, MakeInitialValue, MakeProgramPaths, MakeVariableProgram},
};

const ENV_INPUT_VALUE: &str = "synthetic-input-before-source";
const ENV_INPUT_AFTER_SOURCE: &str = "synthetic-input-after-source";
const PRIVATE_ENV_VALUE: &str = "synthetic-private-environment-31f4";
const PRIVATE_CLI_VALUE: &str = "synthetic-private-command-9cda";

#[derive(Clone, Copy)]
enum ObserverPhase {
    PreFile,
    SourcePrelude,
}

impl ObserverPhase {
    fn fragment(self) -> String {
        match self {
            Self::PreFile => pre_file_fragment(),
            Self::SourcePrelude => source_prelude_fragment(),
        }
    }
}

fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

const fn native_make() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

fn observer_fragment(
    phase: ObserverPhase,
    expected_cli_names: Option<&BTreeSet<String>>,
) -> String {
    let mut fragment = phase.fragment();
    if let Some(expected) = expected_cli_names {
        fragment.push_str(
            &command_line_values_fragment(expected)
                .expect("synthetic expected names are safe for the CLI observer"),
        );
    }
    fragment
}

fn run_native(
    phase: ObserverPhase,
    expected_cli_names: Option<&BTreeSet<String>>,
    source: &str,
    command_line: &[String],
) -> (Output, TempDir, PathBuf, String) {
    let directory = tempfile::tempdir().expect("temporary synthetic Make directory");
    let selected_input = directory.path().join("selected.mk");
    fs::write(&selected_input, source).expect("write synthetic selected Makefile");
    let selected_identity = selected_input
        .to_str()
        .expect("temporary path is UTF-8")
        .to_owned();

    let mut command = Command::new(native_make());
    command.args(["-r", "-n", "-s"]);
    if matches!(phase, ObserverPhase::PreFile) {
        command
            .arg("--eval")
            .arg(observer_fragment(phase, expected_cli_names));
    } else {
        let observer_path = directory.path().join("observer.mk");
        let observer = observer_fragment(phase, expected_cli_names);
        fs::write(&observer_path, observer).expect("write synthetic source prelude");
        command
            .arg("-f")
            .arg(&observer_path)
            .arg("-o")
            .arg(&observer_path);
    }
    command
        .arg("-f")
        .arg(&selected_input)
        .arg("-o")
        .arg(&selected_input)
        .arg("probe")
        .args(command_line)
        .current_dir(directory.path())
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").expect("test runner supplies PATH"),
        )
        .env("ENV_INPUT", ENV_INPUT_VALUE)
        .env("PRIVATE_ENV", PRIVATE_ENV_VALUE);

    let output = command.output().expect("run synthetic native Make query");
    (output, directory, selected_input, selected_identity)
}

fn one_line(output: &Output, prefix: &str) -> String {
    assert!(output.status.success(), "synthetic Make query failed");
    let text = std::str::from_utf8(&output.stdout).expect("Make stdout is UTF-8");
    let rows = text
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "expected one synthetic observation line");
    rows[0].to_owned()
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

const fn to_initial_flavor(flavor: MakeInitialValueFlavor) -> MakeInitialFlavor {
    match flavor {
        MakeInitialValueFlavor::Simple => MakeInitialFlavor::Simple,
        MakeInitialValueFlavor::Recursive => MakeInitialFlavor::Recursive,
    }
}

fn make_paths<'a>(root: &'a Path, dirs: &'a DirVars) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn make_initial_values<'a>(
    defaults: &'a ValidatedMakeInitialValues,
    command_line: &'a ValidatedMakeCommandLineValues,
) -> Vec<MakeInitialValue<'a>> {
    defaults
        .entries()
        .chain(command_line.entries())
        .map(|entry| MakeInitialValue {
            name: &entry.name,
            value: &entry.value,
            flavor: to_initial_flavor(entry.flavor),
            origin: entry.origin,
        })
        .collect()
}

#[test]
fn native_cli_observations_replay_in_pre_file_and_source_prelude_phases() {
    let expected_cli_names = names(&["CLI_R", "CLI_S"]);
    let cli_args = vec![
        "CLI_R=prefix-$(LATER)".to_owned(),
        "CLI_R+=tail".to_owned(),
        "CLI_S:=$(ENV_INPUT)".to_owned(),
    ];
    let source = concat!(
        "LATER := source-later\n",
        "ENV_INPUT := synthetic-input-after-source\n",
        "CLI_R := ordinary-file-attempt\n",
        "CLI_S := ordinary-file-attempt\n",
        "RESULT := $(CLI_R)|$(CLI_S)|$(ENV_INPUT)\n",
        "probe:\n",
        "\t$(info OBSERVED=$(RESULT))\n",
    );

    for phase in [ObserverPhase::PreFile, ObserverPhase::SourcePrelude] {
        let (output, _directory, selected_input, selected_identity) =
            run_native(phase, Some(&expected_cli_names), source, &cli_args);
        let native_result = one_line(&output, "OBSERVED=");
        let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
        assert_eq!(
            namespace.origin("CLI_R"),
            Some(MakeVariableOrigin::CommandLine)
        );
        assert_eq!(
            namespace.origin("CLI_S"),
            Some(MakeVariableOrigin::CommandLine)
        );

        let defaults =
            ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap();
        let cli_values = ValidatedMakeCommandLineValues::from_make_stdout(
            &output.stdout,
            &namespace,
            &expected_cli_names,
        )
        .unwrap();
        let cli_records = cli_values.to_observation().entries;
        assert_eq!(cli_records.len(), 2);
        assert_eq!(cli_records[0].name, "CLI_R");
        assert_eq!(cli_records[0].origin, MakeVariableOrigin::CommandLine);
        assert_eq!(cli_records[0].flavor, MakeInitialValueFlavor::Recursive);
        assert_eq!(cli_records[0].value, "prefix-$(LATER) tail");
        assert_eq!(cli_records[1].name, "CLI_S");
        assert_eq!(cli_records[1].origin, MakeVariableOrigin::CommandLine);
        assert_eq!(cli_records[1].flavor, MakeInitialValueFlavor::Simple);
        assert_eq!(cli_records[1].value, ENV_INPUT_VALUE);
        assert!(defaults.entries().all(|entry| {
            matches!(
                entry.origin,
                MakeVariableOrigin::File | MakeVariableOrigin::Default
            ) && entry.name != "CLI_R"
                && entry.name != "CLI_S"
        }));
        assert!(!output_text(&output).contains(PRIVATE_ENV_VALUE));

        let selected_text = fs::read_to_string(&selected_input).unwrap();
        let root = selected_input.parent().unwrap();
        let dirs = DirVars::default();
        let paths = make_paths(root, &dirs);
        let mut initial = make_initial_values(&defaults, &cli_values);
        initial.extend([
            MakeInitialValue {
                name: "ENV_INPUT",
                value: ENV_INPUT_VALUE,
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Environment,
            },
            MakeInitialValue {
                name: "PRIVATE_ENV",
                value: PRIVATE_ENV_VALUE,
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Environment,
            },
        ]);

        // This synthetic native Make also reports automatic MAKELEVEL/MFLAGS
        // with environment origin. Admit their observed names only; their
        // values are not reconstructed or claimed as a production child context.
        // The other names come from this test's closed env_clear map.
        let mut environment_names = names(&["PATH", "ENV_INPUT", "PRIVATE_ENV"]);
        for automatic in ["MAKELEVEL", "MFLAGS"] {
            if namespace.origin(automatic) == Some(MakeVariableOrigin::Environment) {
                environment_names.insert(automatic.into());
            }
        }
        let native_environment_names = namespace
            .to_observation()
            .entries
            .into_iter()
            .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
            .map(|entry| entry.name)
            .collect::<BTreeSet<_>>();
        assert_eq!(native_environment_names, environment_names);

        let mut program = MakeVariableProgram::new_source_program_with_environment(
            &paths,
            &namespace,
            &initial,
            &environment_names,
        )
        .unwrap();
        program
            .read_file(&selected_identity, &selected_text, &paths, &|_, _| Ok(None))
            .unwrap();
        assert_eq!(
            program.evaluate("$(RESULT)", &paths).unwrap(),
            native_result
        );
        assert_eq!(
            native_result,
            format!("prefix-source-later tail|{ENV_INPUT_VALUE}|{ENV_INPUT_AFTER_SOURCE}")
        );
    }
}

#[test]
fn command_line_observer_roundtrips_encoded_argument_bytes() {
    let expected_cli_names = names(&["CLI_RAW"]);
    let raw = "literal $(error SHOULD_NOT_RUN), (paren) %|;\n\tend";
    let cli_args = vec![format!("CLI_RAW={raw}")];
    // This synthetic argv is already accepted by the test harness. The observer
    // checks raw-value transport only; it does not authorize CLI syntax/options.
    for phase in [ObserverPhase::PreFile, ObserverPhase::SourcePrelude] {
        let (output, _directory, _, _) = run_native(
            phase,
            Some(&expected_cli_names),
            ".PHONY: probe\nprobe:\n",
            &cli_args,
        );
        assert!(output.status.success(), "synthetic Make query failed");
        let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
        let values = ValidatedMakeCommandLineValues::from_make_stdout(
            &output.stdout,
            &namespace,
            &expected_cli_names,
        )
        .unwrap();
        let record = values.entries().next().unwrap();
        assert_eq!(record.name, "CLI_RAW");
        assert_eq!(record.value, raw);
        assert_eq!(record.flavor, MakeInitialValueFlavor::Recursive);
        assert_eq!(record.origin, MakeVariableOrigin::CommandLine);
    }
}

#[test]
fn default_observers_exclude_private_cli_and_environment_values_in_both_phases() {
    let cli_args = vec![format!("PRIVATE_CLI={PRIVATE_CLI_VALUE}")];
    for phase in [ObserverPhase::PreFile, ObserverPhase::SourcePrelude] {
        let (output, _directory, _, _) = run_native(phase, None, "probe:\n", &cli_args);
        assert!(output.status.success(), "synthetic Make query failed");
        let retained = output_text(&output);
        assert!(!retained.contains(PRIVATE_CLI_VALUE));
        assert!(!retained.contains(PRIVATE_ENV_VALUE));
        assert!(!retained.contains(MAKE_COMMAND_LINE_VALUES_METADATA_KEY));
        let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
        let defaults =
            ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap();
        assert!(!defaults.entries().any(|entry| {
            entry.value.contains(PRIVATE_CLI_VALUE) || entry.value.contains(PRIVATE_ENV_VALUE)
        }));
    }
}

#[test]
fn expected_membership_and_helper_collisions_fail_before_cli_packet_output() {
    let cli_args = vec![
        "CLI_SAFE=synthetic-cli-safe-sentinel".to_owned(),
        "CLI_OTHER=synthetic-cli-other-sentinel".to_owned(),
    ];
    for phase in [ObserverPhase::PreFile, ObserverPhase::SourcePrelude] {
        for expected in [
            names(&["CLI_SAFE"]),
            names(&["CLI_SAFE", "CLI_OTHER", "EXTRA"]),
        ] {
            let (output, _directory, _, _) =
                run_native(phase, Some(&expected), "probe:\n", &cli_args);
            assert!(!output.status.success());
            let retained = output_text(&output);
            assert!(!retained.contains(MAKE_COMMAND_LINE_VALUES_METADATA_KEY));
            assert!(!retained.contains("synthetic-cli-safe-sentinel"));
            assert!(!retained.contains("synthetic-cli-other-sentinel"));
        }

        for helper in ["__AROS_COMMAND_VALUES_ITEM", "__AROS_COMMAND_VALUES_NL"] {
            let helper_args = vec![format!("{helper}=synthetic-helper-sentinel")];
            let (output, _directory, _, _) =
                run_native(phase, Some(&BTreeSet::new()), "probe:\n", &helper_args);
            assert!(!output.status.success());
            let retained = output_text(&output);
            assert!(!retained.contains(MAKE_COMMAND_LINE_VALUES_METADATA_KEY));
            assert!(!retained.contains("synthetic-helper-sentinel"));
        }
    }
}
