//! Differential tests for serial recipe child-environment projection.

#![cfg(unix)]

use super::{MakeProgramPaths, MakeRecipeProjection, MakeVariableProgram};
use crate::dirs::DirVars;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_native_invocation::NativeMakeInvocation,
    make_native_sampler::{
        observe_first_input_make_with_recipe_controls, InitialMakeObservationRequest,
    },
    measure_tree_content_cas,
    process::CancellationToken,
    sha256_bytes,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use tempfile::TempDir;

struct NativeOracleFixture {
    _temporary: TempDir,
    make: PathBuf,
    through_shell: bool,
    recipe_command: String,
    invocation: NativeMakeInvocation,
    projection: MakeRecipeProjection,
    native_stdout: Vec<u8>,
    environment_names: std::collections::BTreeSet<String>,
}

fn gnu_make() -> PathBuf {
    // Match the existing host-test selection and ordinary PATH resolution.
    let name = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    for directory in std::env::split_paths(&std::env::var_os("PATH").expect("test PATH")) {
        let path = directory.join(name);
        let Ok(path) = path.canonicalize() else {
            continue;
        };
        let Ok(version) = Command::new(&path).arg("--version").output() else {
            continue;
        };
        if version.status.success() && version.stdout.starts_with(b"GNU Make ") {
            return path;
        }
    }
    panic!("serial recipe environment oracle requires GNU Make");
}

fn source_paths<'a>(dirs: &'a DirVars, root: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn input_snapshot(root: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        root,
        &[MakeInputRootSelection {
            root: root.to_owned(),
            content_sha256: measure_tree_content_cas(root)
                .expect("measure synthetic Source root")
                .payload_digest_excluding(None),
        }],
    )
    .expect("capture synthetic Source root")
    .with_include_directories(&[])
    .expect("capture empty include search path")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
    .expect("construct synthetic native invocation")
}

fn native_fixture(source_controls: &str, inherited: &[(&str, &str)]) -> NativeOracleFixture {
    native_fixture_with_options(source_controls, inherited, &[], &[])
}

fn native_fixture_with_options(
    source_controls: &str,
    inherited: &[(&str, &str)],
    options: &[&str],
    command_line_assignments: &[&str],
) -> NativeOracleFixture {
    native_fixture_selected(
        source_controls,
        inherited,
        options,
        command_line_assignments,
        false,
    )
}

fn native_fixture_selected(
    source_controls: &str,
    inherited: &[(&str, &str)],
    options: &[&str],
    command_line_assignments: &[&str],
    through_shell: bool,
) -> NativeOracleFixture {
    let temporary = tempfile::tempdir().expect("create private GNU Make oracle root");
    let root = temporary.path().join("source");
    fs::create_dir(&root).expect("create synthetic Source root");
    let root = root
        .canonicalize()
        .expect("canonicalize synthetic Source root");
    let env_program = PathBuf::from("/usr/bin/env")
        .canonicalize()
        .expect("canonicalize env executable");
    let env_command = shell_quote(&env_program.display().to_string());
    // Probe both GNU's direct-exec environment and the same environment after
    // the selected POSIX shell. A required builtin prevents Make's direct-exec
    // optimization; no shell-owned names are excluded from either comparison.
    let recipe_command = format!("{}{env_command} -0", if through_shell { ":; " } else { "" });
    let source = format!(
        "SHELL := /bin/sh\n.SHELLFLAGS := -c\n\
         export SERIAL_SOURCE := source value with spaces\n\
         export SERIAL_CLI_COPY := $(SERIAL_CLI)\n\
         unexport SERIAL_DROP\n\
         {source_controls}\n\
         probe.out:\n\t@{recipe_command}\n",
    );
    fs::write(root.join("Source.mk"), source).expect("write synthetic Source.mk");

    let make = gnu_make();
    let mut arguments = vec![
        "-r".to_owned(),
        "-R".to_owned(),
        "-s".to_owned(),
        "--no-print-directory".to_owned(),
    ];
    arguments.extend(options.iter().map(|value| (*value).to_owned()));
    arguments.extend(["-f".to_owned(), "Source.mk".to_owned()]);
    arguments.extend(
        std::iter::once("SERIAL_CLI=argument value with spaces")
            .chain(command_line_assignments.iter().copied())
            .map(str::to_owned),
    );
    arguments.push("probe.out".to_owned());
    let mut environment = BTreeMap::from([
        (
            "PATH".to_owned(),
            std::env::var("PATH").expect("test PATH is present"),
        ),
        ("LC_ALL".to_owned(), "C".to_owned()),
        ("MAKELEVEL".to_owned(), "2".to_owned()),
        ("SERIAL_INHERITED".to_owned(), "inherited value".to_owned()),
        ("SERIAL_DROP".to_owned(), "discard me".to_owned()),
    ]);
    for (name, value) in inherited {
        environment.insert((*name).to_owned(), (*value).to_owned());
    }
    let invocation = invocation(&arguments, &root, &environment);
    let executable_sha256 = sha256_bytes(&fs::read(&make).expect("read selected GNU Make"));
    let context_sha256 = sha256_bytes(b"synthetic serial recipe environment oracle");
    let request = InitialMakeObservationRequest {
        program: &make,
        executable_sha256: &executable_sha256,
        invocation: &invocation,
        context_sha256: &context_sha256,
        timeout: Duration::from_secs(20),
    };
    let observation = observe_first_input_make_with_recipe_controls(
        &request,
        &CancellationToken::default(),
        |_, _| Ok(()),
    )
    .unwrap_or_else(|error| panic!("GNU Make first-input observation failed: {error}"));

    let snapshot = input_snapshot(&root);
    let dirs = DirVars::default();
    let paths = source_paths(&dirs, &root);
    let (mut program, _file, _reads) = MakeVariableProgram::read_native_captured_inputs(
        &paths,
        observation.initial_stdout(),
        observation.invocation(),
        observation.environment_names(),
        &snapshot,
    )
    .unwrap_or_else(|error| panic!("read synthetic native Source inputs: {error}"));
    let automatic_variables = BTreeMap::from([("@".to_owned(), "probe.out".to_owned())]);
    let (projection, _reads) = program
        .evaluate_captured_target_recipe(
            &["probe.out"],
            ":",
            &automatic_variables,
            &paths,
            &snapshot,
        )
        .unwrap_or_else(|error| panic!("project synthetic Source target frame: {error}"));

    NativeOracleFixture {
        _temporary: temporary,
        make,
        through_shell,
        recipe_command,
        invocation,
        projection,
        native_stdout: observation.initial_stdout().to_vec(),
        environment_names: observation.environment_names().clone(),
    }
}

#[test]
fn missing_engine_packet_does_not_guess_cli_propagation() {
    let fixture = native_fixture("", &[]);
    assert_projection_matches_native(&fixture);
    let prefix = format!(
        "{}=",
        aros_common::make_initial_values::MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY
    );
    let historical = std::str::from_utf8(&fixture.native_stdout)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with(&prefix))
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(historical.as_bytes(), fixture.native_stdout);
    let root = fixture.invocation.working_directory();
    let snapshot = input_snapshot(root);
    let dirs = DirVars::default();
    let paths = source_paths(&dirs, root);
    let (mut program, _, _) = MakeVariableProgram::read_native_captured_inputs(
        &paths,
        historical.as_bytes(),
        &fixture.invocation,
        &fixture.environment_names,
        &snapshot,
    )
    .expect("historical data packet remains readable");
    let (projection, _) = program
        .evaluate_captured_target_recipe(
            &["probe.out"],
            ":",
            &BTreeMap::from([("@".into(), "probe.out".into())]),
            &paths,
            &snapshot,
        )
        .expect("data-only projection remains supported");
    let error = projection
        .serial_native_environment(&fixture.invocation)
        .expect_err("missing measured CLI propagation must refuse execution, not invent escaping");
    assert_eq!(
        error,
        "serial recipe environment lacks measured GNU CLI propagation"
    );
}

impl NativeOracleFixture {
    fn native_child_environment(&self) -> BTreeMap<Vec<u8>, Vec<u8>> {
        let output = Command::new(&self.make)
            .args(self.invocation.arguments())
            .current_dir(self.invocation.working_directory())
            .env_clear()
            .envs(self.invocation.environment())
            .stdin(Stdio::null())
            .output()
            .expect("execute original synthetic GNU Make recipe");
        assert!(
            output.status.success(),
            "synthetic native Make recipe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        environment_map(&output.stdout)
    }

    fn projected_environment(&self) -> BTreeMap<String, String> {
        self.projection
            .serial_native_environment(&self.invocation)
            .unwrap_or_else(|error| panic!("serial native environment projection refused: {error}"))
    }

    fn shell_child_environment(
        &self,
        environment: &BTreeMap<String, String>,
    ) -> BTreeMap<Vec<u8>, Vec<u8>> {
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(&self.recipe_command)
            .current_dir(self.invocation.working_directory())
            .env_clear()
            .envs(environment)
            .stdin(Stdio::null())
            .output()
            .expect("execute selected shell and env with projected environment");
        assert!(
            output.status.success(),
            "projected shell env command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        environment_map(&output.stdout)
    }

    fn candidate_environment(
        &self,
        values: &BTreeMap<String, String>,
    ) -> BTreeMap<Vec<u8>, Vec<u8>> {
        if self.through_shell {
            self.shell_child_environment(values)
        } else {
            values
                .iter()
                .map(|(name, value)| (name.as_bytes().to_vec(), value.as_bytes().to_vec()))
                .collect()
        }
    }
}

fn environment_map(bytes: &[u8]) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let split = entry
            .iter()
            .position(|byte| *byte == b'=')
            .expect("env -0 entry contains a name/value separator");
        let previous = result.insert(entry[..split].to_vec(), entry[split + 1..].to_vec());
        assert!(previous.is_none(), "env -0 emitted a duplicate name");
    }
    result
}

fn assert_projection_matches_native(fixture: &NativeOracleFixture) -> BTreeMap<Vec<u8>, Vec<u8>> {
    let actual = fixture.native_child_environment();
    let projected = fixture.projected_environment();
    assert_eq!(
        actual,
        fixture.candidate_environment(&projected),
        "complete native environment must match at the selected process boundary",
    );
    actual
}

#[test]
fn serial_projection_matches_gnu_make_with_flags_spaces_and_engine_level() {
    let fixture = native_fixture("", &[]);
    let actual = assert_projection_matches_native(&fixture);
    assert_eq!(actual.get(b"MAKELEVEL".as_slice()), Some(&b"3".to_vec()));
    assert!(actual.contains_key(b"MAKEFLAGS".as_slice()));
    assert_eq!(
        actual.get(b"SERIAL_CLI_COPY".as_slice()),
        Some(&b"argument value with spaces".to_vec())
    );
    assert_eq!(
        actual.get(b"SERIAL_SOURCE".as_slice()),
        Some(&b"source value with spaces".to_vec())
    );
    assert!(!actual.contains_key(b"SERIAL_DROP".as_slice()));
}

#[test]
fn complete_oracle_detects_wrong_level_and_missing_makeflags() {
    let fixture = native_fixture("", &[]);
    let actual = assert_projection_matches_native(&fixture);

    let mut wrong_level = fixture.projected_environment();
    assert!(wrong_level.contains_key("MAKELEVEL"));
    wrong_level.insert("MAKELEVEL".to_owned(), "2".to_owned());
    assert_ne!(
        actual,
        fixture.candidate_environment(&wrong_level),
        "the complete environment oracle must detect an unincremented level"
    );

    let mut missing_flags = fixture.projected_environment();
    assert!(missing_flags.remove("MAKEFLAGS").is_some());
    assert_ne!(
        actual,
        fixture.candidate_environment(&missing_flags),
        "the complete environment oracle must detect omitted engine flags"
    );
}

#[test]
fn original_shell_required_recipes_match_complete_native_environment() {
    for controls in [
        "",
        "unexport MAKEFLAGS\nunexport MFLAGS\nunexport MAKELEVEL\n",
        "export SHELL\nexport MAKEOVERRIDES\n",
    ] {
        let fixture = native_fixture_selected(controls, &[("SHELL", "/bin/zsh")], &[], &[], true);
        assert_projection_matches_native(&fixture);
    }
}

#[test]
fn shell_default_export_preserves_incoming_value_unless_source_controls_it() {
    let incoming = [("SHELL", "/bin/zsh")];

    let unchanged = native_fixture("", &incoming);
    let actual = assert_projection_matches_native(&unchanged);
    assert_eq!(actual.get(b"SHELL".as_slice()), Some(&b"/bin/zsh".to_vec()));

    let exported = native_fixture("export SHELL\n", &incoming);
    let actual = assert_projection_matches_native(&exported);
    assert_eq!(actual.get(b"SHELL".as_slice()), Some(&b"/bin/sh".to_vec()));

    let unexported = native_fixture("unexport SHELL\n", &incoming);
    let actual = assert_projection_matches_native(&unexported);
    assert_eq!(actual.get(b"SHELL".as_slice()), Some(&b"/bin/zsh".to_vec()));

    let unexported_absent =
        native_fixture_with_options("unexport SHELL\n", &[], &[], &["SHELL=/bin/sh"]);
    let actual = assert_projection_matches_native(&unexported_absent);
    assert!(!actual.contains_key(b"SHELL".as_slice()));

    let cli_with_incoming = native_fixture_with_options("", &incoming, &[], &["SHELL=/bin/sh"]);
    let actual = assert_projection_matches_native(&cli_with_incoming);
    assert_eq!(actual.get(b"SHELL".as_slice()), Some(&b"/bin/zsh".to_vec()));

    let cli_without_incoming = native_fixture_with_options("", &[], &[], &["SHELL=/bin/sh"]);
    let actual = assert_projection_matches_native(&cli_without_incoming);
    assert_eq!(actual.get(b"SHELL".as_slice()), Some(&b"/bin/sh".to_vec()));

    let cli_exported =
        native_fixture_with_options("export SHELL\n", &incoming, &[], &["SHELL=/bin/sh"]);
    let actual = assert_projection_matches_native(&cli_exported);
    assert_eq!(actual.get(b"SHELL".as_slice()), Some(&b"/bin/sh".to_vec()));

    let cli_level = native_fixture_with_options("", &[("MAKELEVEL", "2")], &[], &["MAKELEVEL=7"]);
    let actual = assert_projection_matches_native(&cli_level);
    assert_eq!(actual.get(b"MAKELEVEL".as_slice()), Some(&b"8".to_vec()));
}

#[test]
fn explicit_flag_unexport_is_allowed_and_makelevel_still_increments_and_exports() {
    let fixture = native_fixture("unexport MAKEFLAGS\nunexport MAKELEVEL\n", &[]);
    let actual = assert_projection_matches_native(&fixture);
    assert!(!actual.contains_key(b"MAKEFLAGS".as_slice()));
    assert_eq!(actual.get(b"MAKELEVEL".as_slice()), Some(&b"3".to_vec()));
}

#[test]
fn source_engine_mutations_are_refused_even_when_restored_or_scoped() {
    let engine_names = [
        "MAKEFLAGS",
        "MFLAGS",
        "MAKELEVEL",
        "MAKEOVERRIDES",
        "GNUMAKEFLAGS",
        "MAKEFILES",
    ];
    for name in engine_names {
        let write_and_restore = format!(
            "AROS_SAVED_{name} := $({name})\n{name} := temporary-value\n{name} := $(AROS_SAVED_{name})\n"
        );
        let fixture = native_fixture(&write_and_restore, &[]);
        assert!(
            fixture
                .projection
                .serial_native_environment(&fixture.invocation)
                .is_err(),
            "assignment to {name} must be refused even when it restores its value"
        );

        let undefine_and_restore = format!(
            "AROS_SAVED_{name} := $({name})\nundefine {name}\n{name} := $(AROS_SAVED_{name})\n"
        );
        let fixture = native_fixture(&undefine_and_restore, &[]);
        assert!(
            fixture
                .projection
                .serial_native_environment(&fixture.invocation)
                .is_err(),
            "undefining {name} must be refused even when it restores its value"
        );
    }

    for scoped_mutation in [
        "probe.out: MAKELEVEL := $(MAKELEVEL)\n",
        "%.out: MAKEOVERRIDES := $(MAKEOVERRIDES)\n",
    ] {
        let fixture = native_fixture(scoped_mutation, &[]);
        assert!(
            fixture
                .projection
                .serial_native_environment(&fixture.invocation)
                .is_err(),
            "target-specific and pattern engine-variable assignments must be refused"
        );
    }
}

fn with_arguments(original: &NativeMakeInvocation, arguments: Vec<String>) -> NativeMakeInvocation {
    let mut invocation = NativeMakeInvocation::new(
        arguments,
        original.working_directory().to_owned(),
        original
            .environment()
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    )
    .expect("construct alternate serial-startup invocation");
    if let Some(argument_zero) = original.argument_zero() {
        invocation = invocation
            .with_argument_zero(argument_zero.to_owned())
            .expect("preserve selected native argv[0]");
    }
    invocation
}

#[test]
fn sampled_dry_run_and_changed_parallel_startup_are_refused() {
    let sampled_dry_run = native_fixture_with_options("", &[], &["-n"], &[]);
    assert!(
        sampled_dry_run
            .projection
            .serial_native_environment(&sampled_dry_run.invocation)
            .is_err(),
        "an actually sampled dry-run invocation must be refused"
    );

    let fixture = native_fixture("", &[]);
    let mut dry_run_arguments = fixture.invocation.arguments().to_vec();
    dry_run_arguments.insert(0, "-n".to_owned());
    let dry_run = with_arguments(&fixture.invocation, dry_run_arguments);
    assert!(
        fixture
            .projection
            .serial_native_environment(&dry_run)
            .is_err(),
        "a changed invocation must not reuse the projection's bound observation"
    );

    let mut parallel_arguments = fixture.invocation.arguments().to_vec();
    parallel_arguments.insert(0, "-j2".to_owned());
    let parallel = with_arguments(&fixture.invocation, parallel_arguments);
    assert!(
        fixture
            .projection
            .serial_native_environment(&parallel)
            .is_err(),
        "parallel startup with jobserver state must be refused"
    );
}
