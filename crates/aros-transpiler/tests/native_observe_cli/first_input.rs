use aros_common::{
    make_initial_values::{
        ValidatedMakeCommandLineValues, ValidatedMakeEnvironmentValues, ValidatedMakeInitialValues,
        ValidatedMakeParserControls,
    },
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
    make_native_invocation::PrivateNativeCaptureDirectory,
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    make_observer::{command_line_values_fragment, source_prelude_fragment},
    sha256_bytes,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    io::Write as _,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const STOP_EXPRESSION: &str = "$(error AROS_NATIVE_INITIAL_OBSERVATION_STOP)\n";

fn private_output_root(root: &Path) -> PathBuf {
    let output_root = root.join("output");
    fs::create_dir(&output_root).expect("create private first-input output directory");
    fs::set_permissions(&output_root, fs::Permissions::from_mode(0o700))
        .expect("set private first-input output mode");
    output_root
}

fn write_control(
    root: &Path,
    output_root: &Path,
    program: &Path,
    executable_sha256: &str,
    version: u32,
    phase: Option<Value>,
) -> PathBuf {
    let mut control = serde_json::json!({
        "version": version,
        "output_root": output_root,
        "program": program,
        "executable_sha256": executable_sha256,
        "context_sha256": super::context_digest(),
    });
    if let Some(phase) = phase {
        control["phase"] = phase;
    }
    let bytes = serde_json::to_vec(&control).expect("encode short-launch control");
    let directory = PrivateNativeCaptureDirectory::open(root).expect("bind private control root");
    directory
        .write_new(OsStr::new("control.json"), &bytes)
        .expect("retain private short-launch control");
    root.join("control.json")
}

fn short_observe(
    control: &Path,
    control_sha256: &str,
    cwd: &Path,
    arguments: &[&str],
    environment: &std::collections::BTreeMap<String, String>,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
        .env_clear()
        .envs(environment)
        .current_dir(cwd)
        .arg("--observe-make-launch")
        .arg(control)
        .arg(control_sha256)
        .arg("--")
        .args(arguments)
        .output()
        .expect("run short native observation launcher")
}

fn first_input_native_output(
    program: &Path,
    cwd: &Path,
    arguments: &[&str],
    environment: &std::collections::BTreeMap<String, String>,
) -> Output {
    let mut input = source_prelude_fragment();
    let command_names: BTreeSet<_> = arguments
        .iter()
        .filter_map(|argument| argument.split_once('=').map(|(name, _)| name.to_owned()))
        .collect();
    input.push_str(
        &command_line_values_fragment(&command_names)
            .expect("build selected command-line packet fragment"),
    );
    input.push_str(STOP_EXPRESSION);

    let mut child = Command::new(program)
        .args(["-f", "-"])
        .args(arguments)
        .env_clear()
        .envs(environment)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start synthetic native first-input comparison");
    child
        .stdin
        .take()
        .expect("synthetic native stdin")
        .write_all(input.as_bytes())
        .expect("write synthetic native first input");
    child
        .wait_with_output()
        .expect("collect synthetic native first-input comparison")
}

fn parse_namespace_and_packets(stdout: &[u8]) -> (ValidatedMakeNamespace, Vec<u8>, Vec<u8>) {
    let namespace = ValidatedMakeNamespace::from_make_stdout(stdout)
        .expect("parse common native namespace packet");
    let defaults = ValidatedMakeInitialValues::from_make_stdout(stdout, &namespace)
        .expect("parse common native default packet")
        .to_observation();
    let controls = ValidatedMakeParserControls::from_make_stdout(stdout, &namespace)
        .expect("parse common native parser-control packet")
        .entries()
        .cloned()
        .collect::<Vec<_>>();
    (
        namespace,
        serde_json::to_vec(&defaults).expect("encode parsed default packet"),
        serde_json::to_vec(&controls).expect("encode parsed parser-control packet"),
    )
}

fn assert_first_input_selection(output_root: &Path) -> RetainedMakeObservation {
    let document = fs::read(output_root.join("observation.json"))
        .expect("read retained first-input observation");
    let selection: Value = serde_json::from_slice(
        &fs::read(output_root.join("observation-selection.json"))
            .expect("read first-input selection"),
    )
    .expect("parse first-input selection");
    assert!(
        matches!(
            selection["qualification"].as_str(),
            Some("instrumented-first-input-observation-only")
        ),
        "first-input selection has an unexpected qualification"
    );
    let document_sha256 = sha256_bytes(&document);
    assert!(
        selection["document_sha256"].as_str() == Some(document_sha256.as_str()),
        "first-input selection digest differs from retained document"
    );
    let binding: MakeObservationBinding = serde_json::from_value(selection["binding"].clone())
        .expect("parse first-input observation binding");
    assert!(
        matches!(binding.phase, MakeObservationPhase::FirstInputPrelude),
        "retained binding does not identify the first-input phase"
    );
    assert!(
        binding.context_sha256.eq(&super::context_digest()),
        "retained binding changed the selected context"
    );
    RetainedMakeObservation::load(&document, &document_sha256, &binding)
        .expect("load selected first-input observation")
}

#[test]
fn first_input_observation_retains_invocation_and_stops_before_selected_files() {
    let temporary = tempfile::tempdir().expect("first-input observation fixture");
    let root = super::private_root(&temporary);
    let cwd = root.join("working directory");
    fs::create_dir(&cwd).expect("create first-input working directory");

    let makefile = r"$(file >parse-side-effect.marker,parsed)
$(shell printf shell >shell-side-effect.marker)
.PHONY: FORCE all
effects.mk: FORCE
	$(file >remake-side-effect.marker,remade)
all:
	$(file >recipe-side-effect.marker,expanded)
";
    fs::write(cwd.join("effects.mk"), makefile).expect("write selected side-effect Makefile");
    let program = super::find_make_program();
    let executable_sha256 = super::measure_executable(&program).to_string();
    let environment = super::success_environment(&root);
    let arguments = [
        "-s",
        "-r",
        "-n",
        "-f",
        "effects.mk",
        "-f",
        "effects.mk",
        "CLI_SECRET=cli-native-sentinel",
        "all",
    ];

    let counter = root.join("ordinary counter");
    fs::create_dir(&counter).expect("create ordinary Make counter directory");
    fs::write(counter.join("effects.mk"), makefile).expect("write ordinary counter Makefile");
    let ordinary = Command::new(&program)
        .args(arguments)
        .env_clear()
        .envs(&environment)
        .current_dir(&counter)
        .output()
        .expect("run controlled ordinary Make effect counter");
    assert!(ordinary.status.success(), "ordinary effect counter failed");
    for marker in [
        "parse-side-effect.marker",
        "shell-side-effect.marker",
        "remake-side-effect.marker",
        "recipe-side-effect.marker",
    ] {
        assert!(
            counter.join(marker).is_file(),
            "ordinary counter did not demonstrate each selected Make effect"
        );
    }

    let output_root = private_output_root(&root);
    let control = write_control(
        &root,
        &output_root,
        &program,
        &executable_sha256,
        2,
        Some("first-input-prelude".into()),
    );
    let control_sha256 =
        sha256_bytes(&fs::read(&control).expect("read retained first-input control")).to_string();
    let output = short_observe(&control, &control_sha256, &cwd, &arguments, &environment);

    assert!(
        output.status.success(),
        "first-input observation did not complete"
    );
    assert!(
        output.stdout.is_empty(),
        "successful observation wrote stdout"
    );
    assert!(
        output.stderr.is_empty(),
        "successful observation wrote stderr"
    );
    for marker in [
        "parse-side-effect.marker",
        "shell-side-effect.marker",
        "remake-side-effect.marker",
        "recipe-side-effect.marker",
    ] {
        assert!(
            !cwd.join(marker).exists(),
            "first-input observation reached a selected Make effect"
        );
    }
    assert!(
        !root.join("must-not-log").exists(),
        "private first-input entry opened normal logging"
    );

    for name in [
        "observation-started.json",
        "invocation.json",
        "discovery.stdout",
        "discovery.stderr",
        "discovery.status.json",
        "initial.stdout",
        "initial.stderr",
        "initial.status.json",
        "observation.json",
        "observation-selection.json",
    ] {
        super::assert_private_file(&output_root.join(name));
    }
    super::assert_status(&output_root.join("discovery.status.json"), 2);
    super::assert_status(&output_root.join("initial.status.json"), 2);

    let retained = assert_first_input_selection(&output_root);
    assert!(
        retained
            .invocation()
            .arguments()
            .iter()
            .map(String::as_str)
            .eq(arguments),
        "retained first-input argv changed or lost repeated file options"
    );
    assert!(
        retained.invocation().working_directory().eq(cwd.as_path()),
        "retained first-input cwd changed"
    );
    assert!(
        retained.invocation().environment().eq(&environment),
        "retained first-input environment changed"
    );
}

#[test]
fn first_input_packets_match_a_synthetic_native_boundary_and_preserve_makefile_list_priority() {
    let temporary = tempfile::tempdir().expect("synthetic first-input comparison fixture");
    let root = super::private_root(&temporary);
    let cwd = root.join("working directory");
    fs::create_dir(&cwd).expect("create synthetic comparison directory");
    fs::write(cwd.join("empty.mk"), "").expect("write selected empty Makefile");
    let program = super::find_make_program();
    let executable_sha256 = super::measure_executable(&program).to_string();
    let environment = super::success_environment(&root);

    for (case, arguments, expected_cli_value) in [
        (
            "reset",
            vec![
                "-s",
                "-r",
                "-n",
                "-f",
                "empty.mk",
                "CLI_SECRET=private",
                "all",
            ],
            None,
        ),
        (
            "command-line-priority",
            vec![
                "-s",
                "-r",
                "-n",
                "-f",
                "empty.mk",
                "MAKEFILE_LIST=command-list",
                "all",
            ],
            Some("command-list"),
        ),
    ] {
        let output_root = root.join(format!("output-{case}"));
        fs::create_dir(&output_root).expect("create case output directory");
        fs::set_permissions(&output_root, fs::Permissions::from_mode(0o700))
            .expect("set case output directory private");
        let control = root.join(format!("control-{case}.json"));
        let manifest = serde_json::json!({
            "version": 2,
            "output_root": output_root,
            "program": program,
            "executable_sha256": executable_sha256,
            "context_sha256": super::context_digest(),
            "phase": "first-input-prelude",
        });
        // Keep each control independently selected and privately retained.
        let bytes = serde_json::to_vec(&manifest).expect("encode case control");
        let directory = PrivateNativeCaptureDirectory::open(&root).expect("bind control root");
        directory
            .write_new(
                OsStr::new(control.file_name().expect("case control name")),
                &bytes,
            )
            .expect("retain case control");
        let control_sha256 = sha256_bytes(&bytes).to_string();
        let output = short_observe(&control, &control_sha256, &cwd, &arguments, &environment);
        assert!(
            output.status.success(),
            "synthetic first-input observation failed"
        );
        assert!(
            output.stdout.is_empty(),
            "synthetic observation wrote stdout"
        );
        assert!(
            output.stderr.is_empty(),
            "synthetic observation wrote stderr"
        );
        super::assert_status(&output_root.join("discovery.status.json"), 2);
        super::assert_status(&output_root.join("initial.status.json"), 2);
        let retained = assert_first_input_selection(&output_root);

        // Compare only the bounded first-input namespace/default/control packets
        // from this phase. This does not claim equivalence after selected-file parsing.
        let native = first_input_native_output(&program, &cwd, &arguments, &environment);
        assert!(
            native.status.code() == Some(2),
            "synthetic native boundary did not stop at the fixed first-input point"
        );
        let (observed_namespace, observed_defaults, observed_controls) =
            parse_namespace_and_packets(retained.initial_stdout());
        let (native_namespace, native_defaults, native_controls) =
            parse_namespace_and_packets(&native.stdout);
        assert!(
            observed_namespace
                .to_observation()
                .eq(&native_namespace.to_observation()),
            "first-input namespace differs from the synthetic native boundary"
        );
        assert!(
            observed_defaults.eq(&native_defaults),
            "first-input default packet differs from the synthetic native boundary"
        );
        assert!(
            observed_controls.eq(&native_controls),
            "first-input parser-control packet differs from the synthetic native boundary"
        );

        let namespace = ValidatedMakeNamespace::from_make_stdout(retained.initial_stdout())
            .expect("parse retained first-input namespace");
        let defaults =
            ValidatedMakeInitialValues::from_make_stdout(retained.initial_stdout(), &namespace)
                .expect("parse retained first-input defaults");
        let makeflags = defaults
            .entries()
            .find(|entry| entry.name == "MAKEFLAGS")
            .map(|entry| entry.value.clone())
            .or_else(|| {
                ValidatedMakeEnvironmentValues::from_make_stdout(
                    retained.initial_stdout(),
                    &namespace,
                    retained.environment_names(),
                )
                .expect("parse native MAKEFLAGS environment packet")
                .entries()
                .find(|entry| entry.name == "MAKEFLAGS")
                .map(|entry| entry.value.clone())
            })
            .expect("first-input default or environment packet includes MAKEFLAGS");
        assert!(
            !makeflags.contains("--eval")
                && ['s', 'r', 'n']
                    .into_iter()
                    .all(|flag| makeflags.contains(flag)),
            "first-input MAKEFLAGS omitted instrumentation or lost a selected short flag"
        );

        match expected_cli_value {
            None => {
                let makefile_list = defaults
                    .entries()
                    .find(|entry| entry.name == "MAKEFILE_LIST")
                    .expect("first-input list reset remains in the default packet");
                assert!(
                    makefile_list.value.is_empty()
                        && makefile_list.origin == MakeVariableOrigin::File
                        && matches!(
                            makefile_list.flavor,
                            aros_common::make_initial_values::MakeInitialValueFlavor::Simple
                        ),
                    "first-input list reset has the wrong value, origin, or flavor"
                );
            }
            Some(expected) => {
                let command_values = ValidatedMakeCommandLineValues::from_make_stdout(
                    retained.initial_stdout(),
                    &namespace,
                    &BTreeSet::from(["MAKEFILE_LIST".to_owned()]),
                )
                .expect("parse selected command-line MAKEFILE_LIST value");
                let makefile_list = command_values
                    .entries()
                    .find(|entry| entry.name == "MAKEFILE_LIST")
                    .expect("command-line packet retains MAKEFILE_LIST");
                assert!(
                    makefile_list.value == expected
                        && makefile_list.origin == MakeVariableOrigin::CommandLine
                        && matches!(
                            makefile_list.flavor,
                            aros_common::make_initial_values::MakeInitialValueFlavor::Recursive
                        )
                        && namespace.origin("MAKEFILE_LIST")
                            == Some(MakeVariableOrigin::CommandLine),
                    "first-input reset overrode command-line MAKEFILE_LIST priority"
                );
            }
        }
    }
}

#[test]
fn first_input_packet_composes_into_source_reader_against_pinned_native_make() {
    for inherited in [
        None,
        Some(("MAKEFLAGS", "-nrs")),
        Some(("MFLAGS", "-nrs")),
        Some(("MAKEFLAGS", "nrs --no-print-directory")),
        Some(("MFLAGS", "-nrs --no-print-directory")),
    ] {
        source_reader_replay(inherited);
    }
}

fn source_reader_replay(inherited: Option<(&str, &str)>) {
    use aros_transpiler::{
        dirs::DirVars,
        make_vars::{MakeProgramPaths, MakeVariableProgram},
    };

    let temporary = tempfile::tempdir().expect("first-input source replay fixture");
    let root = super::private_root(&temporary);
    let cwd = root.join("working directory");
    fs::create_dir(&cwd).expect("create first-input source replay directory");
    let source = concat!(
        "MODE := $(if $(findstring s,$(MAKEFLAGS)),quiet,audible)\n",
        "FIRST := $(MAKEFILE_LIST)\n",
        "LIST_ORIGIN := $(origin MAKEFILE_LIST)\n",
        "LIST_FLAVOR := $(flavor MAKEFILE_LIST)\n",
        "CAPTURE := $(MODE)|$(MAKEFLAGS)|$(FIRST)|$(MAKEFILE_LIST)|$(LIST_ORIGIN)|$(LIST_FLAVOR)\n",
        "probe:\n",
    );
    let selected_source = format!("{source}\n$(info ACTUAL=$(CAPTURE))\n");
    fs::write(cwd.join("selected.mk"), selected_source)
        .expect("write native selected source fixture");

    let program = super::find_make_program();
    let executable_sha256 = super::measure_executable(&program).to_string();
    let mut environment = super::success_environment(&root);
    let arguments = inherited.map_or_else(
        || vec!["-s", "-r", "-n", "-f", "selected.mk", "probe"],
        |(name, value)| {
            environment.insert(name.into(), value.into());
            vec!["-f", "selected.mk", "probe"]
        },
    );
    let output_root = private_output_root(&root);
    let control = write_control(
        &root,
        &output_root,
        &program,
        &executable_sha256,
        2,
        Some("first-input-prelude".into()),
    );
    let control_sha256 =
        sha256_bytes(&fs::read(&control).expect("read source replay control")).to_string();
    let observation = short_observe(&control, &control_sha256, &cwd, &arguments, &environment);
    assert!(
        observation.status.success(),
        "first-input source replay observation failed"
    );
    assert!(
        observation.stdout.is_empty(),
        "source replay observation wrote stdout"
    );
    assert!(
        observation.stderr.is_empty(),
        "source replay observation wrote stderr"
    );
    let retained = assert_first_input_selection(&output_root);

    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &cwd,
        relative_dir: Path::new(""),
        top_directory: &cwd,
        working_directory: &cwd,
    };
    let mut replay = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        retained.initial_stdout(),
        retained.invocation(),
        retained.environment_names(),
    )
    .expect("compose source reader from exact first-input packet and invocation");
    replay
        .read_file("selected.mk", source, &paths, &|_, _| Ok(None))
        .expect("parse source in retained first-input context");
    let replayed = replay
        .evaluate("$(CAPTURE)", &paths)
        .expect("evaluate source-dependent flags and input list");

    let ordinary = Command::new(&program)
        .args(arguments)
        .env_clear()
        .envs(&environment)
        .current_dir(&cwd)
        .output()
        .expect("run pinned GNU Make over the selected source");
    assert!(ordinary.status.success(), "ordinary pinned GNU Make failed");
    let actual = std::str::from_utf8(&ordinary.stdout)
        .expect("ordinary GNU Make query is UTF-8")
        .lines()
        .find_map(|line| line.strip_prefix("ACTUAL="))
        .expect("ordinary GNU Make emitted the source query");
    assert!(
        replayed.as_str().eq(actual),
        "first-input packet source replay differs from ordinary pinned GNU Make"
    );
}

#[test]
fn short_control_phase_schema_mismatches_refuse_before_launch() {
    let executable_temporary = tempfile::tempdir().expect("fake executable fixture");
    let (program, executable_sha256) = super::fake_make(&executable_temporary);

    for (version, phase) in [
        (2, None),
        (2, Some(Value::Null)),
        (1, Some("first-input-prelude".into())),
        (1, Some(Value::Null)),
        (2, Some("unknown-phase".into())),
    ] {
        let temporary = tempfile::tempdir().expect("phase mismatch fixture");
        let root = super::private_root(&temporary);
        let output_root = private_output_root(&root);
        let control = write_control(
            &root,
            &output_root,
            &program,
            executable_sha256.as_str(),
            version,
            phase,
        );
        let control_sha256 =
            sha256_bytes(&fs::read(&control).expect("read phase mismatch control")).to_string();
        let environment = super::launch_environment(&root);
        let output = short_observe(
            &control,
            &control_sha256,
            &root,
            &["-r", "-f", "never-read.mk", "FLAG=from-parent", "all"],
            &environment,
        );
        super::assert_refused(&output);
        assert!(
            !root.join("launched.marker").exists(),
            "phase mismatch launched the selected executable"
        );
        assert!(
            !output_root.join("observation-started.json").exists(),
            "phase mismatch started a capture"
        );
    }
}
