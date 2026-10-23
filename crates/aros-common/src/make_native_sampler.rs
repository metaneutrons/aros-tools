//! Bounded GNU Make initial observations for one exact native invocation.
//!
//! The retained packet is data consistency only. It does not establish a
//! producer, parent, input closure, receipt, or authorization for ordinary
//! child execution.

use crate::{
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace, MAKE_NAMESPACE_METADATA_KEY},
    make_native_invocation::NativeMakeInvocation,
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    make_observer,
    make_startup_arguments::LiteralMakeStartupArguments,
    process::{run_output_with_input_and_control, CancellationToken, ProcessOutput},
    publication::{measure_regular_file_bounded, FileIdentity},
    sha256_bytes, Sha256Digest,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CAPTURE_BYTES: usize = 256 * 1024;
const STOP_MESSAGE: &str = "AROS_NATIVE_INITIAL_OBSERVATION_STOP.  Stop.";
const STOP_EXPRESSION: &str = "$(error AROS_NATIVE_INITIAL_OBSERVATION_STOP)\n";

/// Exact native invocation to observe. The program is selected independently
/// and must be an absolute path to the pinned executable.
pub struct InitialMakeObservationRequest<'a> {
    pub program: &'a Path,
    pub executable_sha256: &'a Sha256Digest,
    pub invocation: &'a NativeMakeInvocation,
    pub context_sha256: &'a Sha256Digest,
    pub timeout: Duration,
}

/// The two separately retained executions used to obtain an initial packet.
#[derive(Clone, Copy, Debug)]
pub enum NativeMakeObservationStage {
    Discovery,
    Initial,
}

/// Observe GNU Make before any selected makefile is parsed.
///
/// Both launches preserve the original argument order after the prepended
/// observer `--eval`, use only the invocation's explicit environment, and stop
/// through a fixed GNU Make error before source inputs or recipes can run.
/// If the invocation contains an explicitly selected `argv[0]`, both launches
/// pass that string unchanged while continuing to resolve and bind the
/// executable through `request.program`. The string is launch data, not process
/// authentication or evidence about the observer's own `argv[0]`.
/// File and working-directory identities are revalidated around each launch;
/// this is not an exec-descriptor or concurrent same-user race guarantee.
/// This is not a sandbox: the caller must select the trusted executable and
/// complete environment/runtime closure. Make startup admission does not
/// authenticate loader libraries, interpreters or an untrusted executable.
/// The retention callback must keep streams private, including failed/truncated
/// executions. A status2 sampler stop is not successful ordinary Make execution.
///
/// # Errors
/// Refuses unsupported startup data, changed executable/cwd identities, exhausted
/// deadlines, incomplete output, unexpected status/diagnostics and packet errors.
pub fn observe_initial_make(
    request: &InitialMakeObservationRequest<'_>,
    cancellation: &CancellationToken,
    retain: impl FnMut(NativeMakeObservationStage, &ProcessOutput) -> Result<(), String>,
) -> Result<RetainedMakeObservation, String> {
    observe_make(
        request,
        MakeObservationPhase::PreFirstFile,
        false,
        cancellation,
        retain,
    )
}

/// Observe through a target-free first explicit input, without `--eval`.
///
/// The observer is supplied on stdin before the unchanged original arguments.
/// Its normal MAKEFILE_LIST reset removes only that observer's input-list
/// contribution, respecting stronger command-line priority. Both executions
/// stop during this first input, before selected source parsing or makefile
/// remaking. No ordinary target execution or complete phase equivalence is
/// asserted. The same executable/environment/runtime trust obligations and
/// private retention requirements as [`observe_initial_make`] apply.
///
/// # Errors
/// Refuses the same unsupported startup, changed identities, budget,
/// cancellation, process and packet conditions as [`observe_initial_make`].
pub fn observe_first_input_make(
    request: &InitialMakeObservationRequest<'_>,
    cancellation: &CancellationToken,
    retain: impl FnMut(NativeMakeObservationStage, &ProcessOutput) -> Result<(), String>,
) -> Result<RetainedMakeObservation, String> {
    observe_make(
        request,
        MakeObservationPhase::FirstInputPrelude,
        false,
        cancellation,
        retain,
    )
}

/// First-input observation with separate opt-in recipe-control/engine packets.
///
/// Legacy initial and parser-control membership remains unchanged. The caller
/// still binds shell policy and final Source scope before any recipe launch.
///
/// # Errors
/// Refuses the same conditions as [`observe_first_input_make`] and malformed
/// or incomplete recipe-control metadata.
pub fn observe_first_input_make_with_recipe_controls(
    request: &InitialMakeObservationRequest<'_>,
    cancellation: &CancellationToken,
    retain: impl FnMut(NativeMakeObservationStage, &ProcessOutput) -> Result<(), String>,
) -> Result<RetainedMakeObservation, String> {
    observe_make(
        request,
        MakeObservationPhase::FirstInputPrelude,
        true,
        cancellation,
        retain,
    )
}

fn observe_make(
    request: &InitialMakeObservationRequest<'_>,
    phase: MakeObservationPhase,
    recipe_controls: bool,
    cancellation: &CancellationToken,
    mut retain: impl FnMut(NativeMakeObservationStage, &ProcessOutput) -> Result<(), String>,
) -> Result<RetainedMakeObservation, String> {
    if request.timeout.is_zero() || cancellation.is_cancelled() {
        return Err("native Make observation has no launch budget".into());
    }
    let deadline = Instant::now()
        .checked_add(request.timeout)
        .ok_or("native Make observation timeout is unsupported")?;

    let admitted = LiteralMakeStartupArguments::admit(request.invocation.arguments())
        .map_err(|_| "native Make startup arguments are unsupported")?;
    make_observer::validate_source_prelude_inputs(
        request.invocation.environment(),
        &BTreeMap::new(),
    )
    .map_err(|_| "native Make inherited input controls are unsupported")?;

    let executable = pin_executable(request.program, request.executable_sha256)?;
    let working_directory = pin_working_directory(request.invocation.working_directory())?;
    let control = StageControl {
        executable: &executable,
        working_directory: &working_directory,
        deadline,
        cancellation,
        phase,
    };

    let fragment = match phase {
        MakeObservationPhase::PreFirstFile => make_observer::pre_file_fragment,
        MakeObservationPhase::FirstInputPrelude => make_observer::source_prelude_fragment,
    };
    let observed_fragment = || {
        if recipe_controls {
            format!(
                "{}{}{}{}",
                make_observer::recipe_controls_guard(),
                fragment(),
                make_observer::recipe_controls_fragment(),
                make_observer::recipe_engine_values_fragment()
            )
        } else {
            fragment()
        }
    };
    let mut discovery_fragment = observed_fragment();
    discovery_fragment.push_str(
        &make_observer::command_line_values_fragment(admitted.command_line_names())
            .map_err(|_| "native Make command-line observation is unsupported")?,
    );
    discovery_fragment.push_str(STOP_EXPRESSION);
    let discovery = run_observation_stage(
        request,
        &control,
        NativeMakeObservationStage::Discovery,
        &discovery_fragment,
        &mut retain,
    )?;
    validate_observer_stdout(
        discovery
            .stdout
            .exact_bytes()
            .ok_or("native Make discovery output is incomplete")?,
        false,
        recipe_controls,
    )?;

    let namespace = ValidatedMakeNamespace::from_make_stdout(
        discovery
            .stdout
            .exact_bytes()
            .ok_or("native Make discovery output is incomplete")?,
    )
    .map_err(|_| "native Make discovery namespace is invalid")?;
    let environment_names = environment_origin_names(&namespace);
    let discovery_recipe_controls = recipe_controls
        .then(|| {
            crate::make_initial_values::ValidatedMakeRecipeControls::from_make_stdout(
                discovery
                    .stdout
                    .exact_bytes()
                    .ok_or("incomplete recipe-control discovery")?,
                &namespace,
            )
            .map_err(|_| "native recipe-control discovery is invalid")
        })
        .transpose()?;
    let discovery_recipe_engine = recipe_controls
        .then(|| {
            crate::make_initial_values::ValidatedMakeRecipeEngineValues::from_make_stdout(
                discovery
                    .stdout
                    .exact_bytes()
                    .ok_or("incomplete recipe-engine discovery")?,
                &namespace,
            )
            .map_err(|_| "native recipe-engine discovery is invalid")
        })
        .transpose()?;

    let mut initial_fragment = observed_fragment();
    initial_fragment.push_str(
        &make_observer::command_line_values_fragment(admitted.command_line_names())
            .map_err(|_| "native Make command-line observation is unsupported")?,
    );
    initial_fragment.push_str(
        &make_observer::environment_values_fragment(&environment_names)
            .map_err(|_| "native Make environment observation is unsupported")?,
    );
    initial_fragment.push_str(STOP_EXPRESSION);

    let initial = run_observation_stage(
        request,
        &control,
        NativeMakeObservationStage::Initial,
        &initial_fragment,
        &mut retain,
    )?;
    let stdout = initial
        .stdout
        .exact_bytes()
        .ok_or("native Make initial output is incomplete")?;
    validate_observer_stdout(stdout, true, recipe_controls)?;
    if let Some(expected) = discovery_recipe_controls {
        let initial_namespace = ValidatedMakeNamespace::from_make_stdout(stdout)
            .map_err(|_| "native recipe-control namespace is invalid")?;
        let actual = crate::make_initial_values::ValidatedMakeRecipeControls::from_make_stdout(
            stdout,
            &initial_namespace,
        )
        .map_err(|_| "native recipe-control initial packet is invalid")?;
        if actual != expected {
            return Err("native recipe controls changed across sampler stages".into());
        }
    }
    if let Some(expected) = discovery_recipe_engine {
        let initial_namespace = ValidatedMakeNamespace::from_make_stdout(stdout)
            .map_err(|_| "native recipe-engine namespace is invalid")?;
        let actual = crate::make_initial_values::ValidatedMakeRecipeEngineValues::from_make_stdout(
            stdout,
            &initial_namespace,
        )
        .map_err(|_| "native recipe-engine initial packet is invalid")?;
        if actual != expected {
            return Err("native recipe engine changed across sampler stages".into());
        }
    }

    let invocation_bytes = request
        .invocation
        .to_bytes()
        .map_err(|_| "native Make invocation cannot be retained")?;
    let binding = MakeObservationBinding {
        phase,
        context_sha256: request.context_sha256.clone(),
        invocation_sha256: sha256_bytes(&invocation_bytes),
        stdout_sha256: sha256_bytes(stdout),
    };
    RetainedMakeObservation::from_captured_bytes(
        &invocation_bytes,
        stdout,
        &environment_names,
        &binding,
    )
    .map_err(|_| "native Make initial observation is invalid".into())
}

struct ExecutablePin {
    resolved_path: PathBuf,
    identity: FileIdentity,
}

struct WorkingDirectoryPin {
    resolved_path: PathBuf,
    device: u64,
    inode: u64,
}

fn pin_executable(program: &Path, expected_sha256: &Sha256Digest) -> Result<ExecutablePin, String> {
    if !program.is_absolute() {
        return Err("native Make program path is not absolute".into());
    }
    let resolved_path =
        fs::canonicalize(program).map_err(|_| "native Make program path cannot be resolved")?;
    let identity = validate_executable(program, &resolved_path, None, expected_sha256)?;
    Ok(ExecutablePin {
        resolved_path,
        identity,
    })
}

fn validate_executable(
    selected_path: &Path,
    resolved_path: &Path,
    expected_identity: Option<FileIdentity>,
    expected_sha256: &Sha256Digest,
) -> Result<FileIdentity, String> {
    if fs::canonicalize(selected_path).ok().as_deref() != Some(resolved_path) {
        return Err("native Make program path changed".into());
    }
    let (identity, bytes) = measure_regular_file_bounded(resolved_path, MAX_EXECUTABLE_BYTES)
        .map_err(|_| "native Make executable cannot be measured")?
        .ok_or("native Make executable is unavailable")?;
    if expected_identity.is_some_and(|expected| expected != identity)
        || sha256_bytes(&bytes) != *expected_sha256
    {
        return Err("native Make executable differs from its selection".into());
    }
    let metadata = fs::metadata(resolved_path)
        .map_err(|_| "native Make executable metadata is unavailable")?;
    if !metadata.is_file()
        || metadata.dev() != identity.device()
        || metadata.ino() != identity.inode()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err("native Make executable is not a pinned executable file".into());
    }
    Ok(identity)
}

fn pin_working_directory(path: &Path) -> Result<WorkingDirectoryPin, String> {
    let resolved_path =
        fs::canonicalize(path).map_err(|_| "native Make working directory cannot be resolved")?;
    let metadata = fs::metadata(&resolved_path)
        .map_err(|_| "native Make working directory metadata is unavailable")?;
    if resolved_path.as_os_str() != path.as_os_str() || !metadata.is_dir() {
        return Err("native Make working directory is not a directory".into());
    }
    Ok(WorkingDirectoryPin {
        resolved_path,
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn validate_working_directory(
    selected_path: &Path,
    expected: &WorkingDirectoryPin,
) -> Result<(), String> {
    if fs::canonicalize(selected_path).ok().as_deref() != Some(expected.resolved_path.as_path()) {
        return Err("native Make working directory changed".into());
    }
    let metadata = fs::metadata(&expected.resolved_path)
        .map_err(|_| "native Make working directory metadata is unavailable")?;
    if !metadata.is_dir() || metadata.dev() != expected.device || metadata.ino() != expected.inode {
        return Err("native Make working directory identity changed".into());
    }
    Ok(())
}

fn revalidate_paths(
    request: &InitialMakeObservationRequest<'_>,
    executable: &ExecutablePin,
    working_directory: &WorkingDirectoryPin,
) -> Result<(), String> {
    let identity = validate_executable(
        request.program,
        &executable.resolved_path,
        Some(executable.identity),
        request.executable_sha256,
    )?;
    if identity != executable.identity {
        return Err("native Make executable identity changed".into());
    }
    validate_working_directory(request.invocation.working_directory(), working_directory)
}

struct StageControl<'a> {
    executable: &'a ExecutablePin,
    working_directory: &'a WorkingDirectoryPin,
    deadline: Instant,
    cancellation: &'a CancellationToken,
    phase: MakeObservationPhase,
}

fn run_observation_stage(
    request: &InitialMakeObservationRequest<'_>,
    control: &StageControl<'_>,
    stage: NativeMakeObservationStage,
    fragment: &str,
    retain: &mut impl FnMut(NativeMakeObservationStage, &ProcessOutput) -> Result<(), String>,
) -> Result<ProcessOutput, String> {
    if control.cancellation.is_cancelled() {
        return Err("native Make observation was cancelled before launch".into());
    }
    revalidate_paths(request, control.executable, control.working_directory)?;
    let timeout = control.deadline.saturating_duration_since(Instant::now());
    if timeout.is_zero() {
        return Err("native Make observation deadline expired before launch".into());
    }
    let mut command = Command::new(request.program);
    if let Some(argument_zero) = request.invocation.argument_zero() {
        command.arg0(argument_zero);
    }
    let input = match control.phase {
        MakeObservationPhase::PreFirstFile => {
            command.arg("--eval").arg(fragment);
            &[][..]
        }
        MakeObservationPhase::FirstInputPrelude => {
            command.args(["-f", "-"]);
            fragment.as_bytes()
        }
    };
    command
        .args(request.invocation.arguments())
        .env_clear()
        .envs(request.invocation.environment())
        .current_dir(request.invocation.working_directory());
    let output = run_output_with_input_and_control(
        &mut command,
        input,
        MAX_CAPTURE_BYTES,
        timeout,
        control.cancellation,
    )
    .map_err(|_| "native Make observation process failed")?;

    let retention = retain(stage, &output);
    let path_validation = revalidate_paths(request, control.executable, control.working_directory);
    retention.map_err(|_| "native Make observation retention failed")?;
    path_validation?;
    if control.cancellation.is_cancelled() {
        return Err("native Make observation was cancelled before completion".into());
    }
    if control.deadline <= Instant::now() {
        return Err("native Make observation deadline expired before completion".into());
    }
    validate_process_output(&output)?;
    Ok(output)
}

fn validate_process_output(output: &ProcessOutput) -> Result<(), String> {
    if output.timed_out {
        return Err("native Make observation timed out".into());
    }
    if output.cancelled {
        return Err("native Make observation was cancelled".into());
    }
    if output.status.code() != Some(2) {
        return Err("native Make observation did not stop at the fixed boundary".into());
    }
    if output.stdout.exact_bytes().is_none() || output.stderr.exact_bytes().is_none() {
        return Err("native Make observation output is incomplete".into());
    }
    let stderr = output
        .stderr
        .exact_bytes()
        .ok_or("native Make observation stderr is incomplete")?;
    let Some(line) = stderr.strip_suffix(b"\n") else {
        return Err("native Make observation diagnostic is unexpected".into());
    };
    if line.contains(&b'\n') || !line.ends_with(STOP_MESSAGE.as_bytes()) {
        return Err("native Make observation diagnostic is unexpected".into());
    }
    Ok(())
}

fn validate_observer_stdout(
    stdout: &[u8],
    includes_environment: bool,
    recipe_controls: bool,
) -> Result<(), String> {
    let text =
        std::str::from_utf8(stdout).map_err(|_| "native Make observer output is not UTF-8")?;
    if !stdout.ends_with(b"\n") {
        return Err("native Make observer output framing is incomplete".into());
    }
    let mut expected = vec![
        MAKE_NAMESPACE_METADATA_KEY,
        crate::make_initial_values::MAKE_INITIAL_VALUES_METADATA_KEY,
        crate::make_initial_values::MAKE_PARSER_CONTROLS_METADATA_KEY,
    ];
    if recipe_controls {
        expected.push(crate::make_initial_values::MAKE_RECIPE_CONTROLS_METADATA_KEY);
        expected.push(crate::make_initial_values::MAKE_RECIPE_ENGINE_VALUES_METADATA_KEY);
    }
    expected.push(crate::make_initial_values::MAKE_COMMAND_LINE_VALUES_METADATA_KEY);
    if includes_environment {
        expected.push(crate::make_initial_values::MAKE_ENVIRONMENT_VALUES_METADATA_KEY);
    }
    let lines: Vec<_> = text.lines().collect();
    if lines.len() != expected.len()
        || lines
            .iter()
            .zip(expected)
            .any(|(line, key)| !line.starts_with(&format!("{key}=")))
    {
        return Err("native Make observer output contains unexpected data".into());
    }
    Ok(())
}

fn environment_origin_names(namespace: &ValidatedMakeNamespace) -> BTreeSet<String> {
    namespace
        .to_observation()
        .entries
        .into_iter()
        .filter_map(|entry| (entry.origin == MakeVariableOrigin::Environment).then_some(entry.name))
        .collect()
}

#[cfg(test)]
#[path = "make_native_sampler_tests.rs"]
mod tests;
