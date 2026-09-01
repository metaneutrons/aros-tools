//! Stable `aros` diagnostics and opt-in local logging.

use aros_common::{
    bounded_output_detail, exit_signal, run_output_with_limit, run_output_with_timeout,
    run_status as execute_status, run_status_with_timeout, CapturedStream, CommitState, Diagnostic,
    DiagnosticCode, DiagnosticContext, DiagnosticFormat, DiagnosticSet, DiagnosticStage, LogLevel,
    Logger, ObservabilityPolicy, ProcessOutput, TimedProcessStatus, DEFAULT_CAPTURE_LIMIT,
};
use std::io::Write;
use std::process::{Command, ExitStatus};
use std::sync::OnceLock;
use std::time::Duration;

static LOGGER: OnceLock<Logger> = OnceLock::new();
static DIAGNOSTIC_FORMAT: OnceLock<DiagnosticFormat> = OnceLock::new();

/// Whether human-only progress rendering may write to the diagnostic stream.
///
/// Machine mode reserves standard error for the single versioned diagnostic
/// document and therefore suppresses terminal progress bars.
#[must_use]
pub fn human_progress_enabled() -> bool {
    DIAGNOSTIC_FORMAT.get() != Some(&DiagnosticFormat::Json)
}

/// Stable logging and diagnostic policy for the `aros` frontend.
pub const POLICY: ObservabilityPolicy = ObservabilityPolicy {
    log_schema: "aros-cli-log-v1",
    component: "aros CLI",
    include_invocation: true,
    observability_code: DiagnosticCode::CliObservability,
    observability_stage: DiagnosticStage::Observability,
    internal_code: DiagnosticCode::CliInternal,
    internal_stage: DiagnosticStage::Internal,
    hint: "pass --log-file PATH or set AROS_LOG_FILE, or disable logging with --log-level off",
};

/// Install the process-wide logger and diagnostic format exactly once.
///
/// # Errors
///
/// Returns an internal-contract error if a caller attempts to install either
/// process-wide value more than once or installation cannot be observed.
pub fn install_runtime(
    logger: Logger,
    format: DiagnosticFormat,
) -> Result<&'static Logger, &'static str> {
    LOGGER
        .set(logger)
        .map_err(|_| "aros logger was installed more than once")?;
    DIAGNOSTIC_FORMAT
        .set(format)
        .map_err(|_| "aros diagnostic format was installed more than once")?;
    LOGGER
        .get()
        .ok_or("aros logger installation could not be observed")
}

/// Emit one structured event when local logging is enabled.
///
/// # Errors
///
/// Returns an error when the configured sink cannot persist the event.
pub fn log_event(
    level: LogLevel,
    event: &str,
    message: &str,
    context: &DiagnosticContext,
) -> miette::Result<()> {
    LOGGER.get().map_or(Ok(()), |logger| {
        logger
            .event(level, event, message, context)
            .map_err(|error| miette::miette!("{error}"))
    })
}

/// Stable diagnostic classification applied at one CLI error boundary.
#[derive(Debug, Clone, Copy)]
pub struct ErrorBoundary {
    /// Machine-readable diagnostic code.
    pub code: DiagnosticCode,
    /// Lifecycle stage in which the operation failed.
    pub stage: DiagnosticStage,
    /// Actionable recovery guidance shown to the user.
    pub hint: &'static str,
}

/// Typed context marker used to refine a command-level error boundary without
/// discarding the original error chain or subprocess metadata.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct ClassifiedFailure {
    message: &'static str,
    boundary: ErrorBoundary,
}

/// Typed publication-state marker carried through rich error chains.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct CommitStateFailure {
    message: &'static str,
    state: CommitState,
}

/// Attach a stable error boundary to an operation while retaining its cause.
///
/// # Errors
///
/// Returns the original success value or the classified error chain.
pub fn classify<T>(
    result: miette::Result<T>,
    boundary: ErrorBoundary,
    message: &'static str,
) -> miette::Result<T> {
    result.map_err(|error| error.wrap_err(ClassifiedFailure { message, boundary }))
}

/// Attach a machine-readable publication state without discarding the cause.
///
/// # Errors
///
/// Returns the original success value or the state-marked error chain.
pub fn commit_state<T>(
    result: miette::Result<T>,
    state: CommitState,
    message: &'static str,
) -> miette::Result<T> {
    result.map_err(|error| error.wrap_err(CommitStateFailure { message, state }))
}

/// Captured subprocess failure retaining tool, exit, and signal context.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("{message}")]
pub struct ProcessFailure {
    message: String,
    tool: String,
    exit_code: Option<i32>,
    signal: Option<i32>,
    timed_out: bool,
    timeout_ms: Option<u64>,
    boundary: Option<ErrorBoundary>,
}

impl ProcessFailure {
    fn execution(description: &str, tool: String, error: &std::io::Error) -> Self {
        Self {
            message: format!("could not execute {description}: {error}"),
            tool,
            exit_code: None,
            signal: None,
            timed_out: false,
            timeout_ms: None,
            boundary: None,
        }
    }

    fn timeout_io(
        description: &str,
        tool: String,
        timeout: Duration,
        error: &std::io::Error,
    ) -> Self {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        Self {
            message: format!(
                "{description} exceeded its {timeout_ms} ms deadline and process cleanup failed: {error}"
            ),
            tool,
            exit_code: None,
            signal: None,
            timed_out: true,
            timeout_ms: Some(timeout_ms),
            boundary: None,
        }
    }

    fn exit(description: &str, tool: String, status: ExitStatus, detail: &str) -> Self {
        let mut message = format!("{description} failed with {status}");
        if !detail.is_empty() {
            message.push_str(":\n");
            message.push_str(detail);
        }
        Self {
            message,
            tool,
            exit_code: status.code(),
            signal: exit_signal(status),
            timed_out: false,
            timeout_ms: None,
            boundary: None,
        }
    }

    fn timeout(
        description: &str,
        tool: String,
        status: ExitStatus,
        timeout: Duration,
        detail: &str,
    ) -> Self {
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let mut message = format!("{description} timed out after {timeout_ms} ms");
        if !detail.is_empty() {
            message.push_str(":\n");
            message.push_str(detail);
        }
        Self {
            message,
            tool,
            exit_code: status.code(),
            signal: exit_signal(status),
            timed_out: true,
            timeout_ms: Some(timeout_ms),
            boundary: None,
        }
    }

    fn apply_context(&self, context: &mut DiagnosticContext) {
        context.tool = Some(self.tool.clone());
        context.exit_code = self.exit_code;
        context.signal = self.signal;
        context.timed_out = self.timed_out.then_some(true);
        context.timeout_ms = self.timeout_ms;
    }

    const fn with_boundary(mut self, boundary: ErrorBoundary) -> Self {
        self.boundary = Some(boundary);
        self
    }
}

/// Run a command interactively, or captured when JSON diagnostics require it.
///
/// # Errors
///
/// Returns a structured failure when the process cannot start or exits
/// unsuccessfully.
pub fn run_command(command: &mut Command, description: &str) -> Result<(), ProcessFailure> {
    if DIAGNOSTIC_FORMAT.get() == Some(&DiagnosticFormat::Json) {
        return run_captured_command(command, description, true, None);
    }
    run_interactive_command(command, description)
}

/// Run a command and override its diagnostic classification on failure.
///
/// # Errors
///
/// Returns a structured failure when the process cannot start or exits
/// unsuccessfully.
pub fn run_command_at(
    command: &mut Command,
    description: &str,
    boundary: ErrorBoundary,
) -> Result<(), ProcessFailure> {
    run_command(command, description).map_err(|error| error.with_boundary(boundary))
}

/// Run a command without replaying successful captured output in JSON mode.
///
/// # Errors
///
/// Returns a structured failure when the process cannot start or exits
/// unsuccessfully.
pub fn run_quiet_command(command: &mut Command, description: &str) -> Result<(), ProcessFailure> {
    if DIAGNOSTIC_FORMAT.get() == Some(&DiagnosticFormat::Json) {
        return run_captured_command(command, description, false, None);
    }
    run_interactive_command(command, description)
}

/// Run a command with captured output and return its UTF-8-lossy standard
/// output. This is intended for small machine-readable probes such as Git
/// identities, not streaming build output.
///
/// # Errors
///
/// Returns a structured failure when the process cannot start, exits
/// unsuccessfully, or emits more than the bounded diagnostic limit.
#[cfg(test)]
pub fn capture_stdout(command: &mut Command, description: &str) -> Result<String, ProcessFailure> {
    capture_stdout_inner(command, description, None)
}

/// Capture one machine-readable stdout value with a process-group deadline.
///
/// # Errors
///
/// Returns a structured failure on timeout, unsuccessful exit, or truncation.
pub fn capture_stdout_with_timeout(
    command: &mut Command,
    description: &str,
    timeout: Duration,
) -> Result<String, ProcessFailure> {
    capture_stdout_inner(command, description, Some(timeout))
}

fn capture_stdout_inner(
    command: &mut Command,
    description: &str,
    timeout: Option<Duration>,
) -> Result<String, ProcessFailure> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let output = capture_process(command, description, &tool, timeout)?;
    if output.timed_out {
        let detail = bounded_output_detail(&output.stdout, &output.stderr);
        return Err(ProcessFailure::timeout(
            description,
            tool,
            output.status,
            configured_timeout(timeout, description)?,
            &detail,
        ));
    }
    if !output.status.success() {
        let detail = bounded_output_detail(&output.stdout, &output.stderr);
        return Err(ProcessFailure::exit(
            description,
            tool,
            output.status,
            &detail,
        ));
    }
    let Some(stdout) = output.stdout.exact_bytes() else {
        return Err(ProcessFailure::exit(
            description,
            tool,
            output.status,
            "stdout exceeded the 64 KiB machine-output limit",
        ));
    };
    Ok(String::from_utf8_lossy(stdout).trim_end().to_owned())
}

/// Capture an accepted exit code with a hard process-group deadline.
///
/// # Errors
///
/// Returns a structured failure on timeout, signal, or an unaccepted code.
pub fn capture_exit_code_with_timeout(
    command: &mut Command,
    description: &str,
    timeout: Duration,
    accepted: &[i32],
) -> Result<i32, ProcessFailure> {
    capture_exit_code_inner(command, description, accepted, Some(timeout))
}

fn capture_exit_code_inner(
    command: &mut Command,
    description: &str,
    accepted: &[i32],
    timeout: Option<Duration>,
) -> Result<i32, ProcessFailure> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let output = capture_process(command, description, &tool, timeout)?;
    if output.timed_out {
        let detail = bounded_output_detail(&output.stdout, &output.stderr);
        return Err(ProcessFailure::timeout(
            description,
            tool,
            output.status,
            configured_timeout(timeout, description)?,
            &detail,
        ));
    }
    if let Some(code) = output.status.code().filter(|code| accepted.contains(code)) {
        return Ok(code);
    }
    let detail = bounded_output_detail(&output.stdout, &output.stderr);
    Err(ProcessFailure::exit(
        description,
        tool,
        output.status,
        &detail,
    ))
}

fn capture_process(
    command: &mut Command,
    description: &str,
    tool: &str,
    timeout: Option<Duration>,
) -> Result<ProcessOutput, ProcessFailure> {
    let observed = match timeout {
        Some(timeout) => run_output_with_timeout(command, DEFAULT_CAPTURE_LIMIT, timeout),
        None => run_output_with_limit(command, DEFAULT_CAPTURE_LIMIT),
    };
    observed.map_err(|error| process_io_failure(description, tool.to_owned(), timeout, &error))
}

fn process_io_failure(
    description: &str,
    tool: String,
    timeout: Option<Duration>,
    error: &std::io::Error,
) -> ProcessFailure {
    if error.kind() == std::io::ErrorKind::TimedOut {
        if let Some(timeout) = timeout {
            return ProcessFailure::timeout_io(description, tool, timeout, error);
        }
    }
    ProcessFailure::execution(description, tool, error)
}

fn configured_timeout(
    timeout: Option<Duration>,
    description: &str,
) -> Result<Duration, ProcessFailure> {
    timeout.ok_or_else(|| {
        let error = std::io::Error::other(
            "process runner reported a timeout without a configured deadline",
        );
        ProcessFailure::execution(description, "internal process runner".into(), &error)
    })
}

/// Run a command with inherited streams and normalized exit metadata.
///
/// # Errors
///
/// Returns a structured failure when the process cannot start or exits
/// unsuccessfully.
pub fn run_interactive_command(
    command: &mut Command,
    description: &str,
) -> Result<(), ProcessFailure> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let observed = execute_status(command)
        .map_err(|error| ProcessFailure::execution(description, tool, &error))?;
    let tool = observed.tool;
    let status = observed.status;
    if !status.success() {
        return Err(ProcessFailure::exit(description, tool, status, ""));
    }
    Ok(())
}

/// Run and reap a process at a deadline while leaving exit interpretation to
/// the caller.
///
/// This is for evidence-producing tools such as QEMU where timeout and nonzero
/// exit are observations rather than the command's final success criterion.
///
/// # Errors
///
/// Returns a structured failure when spawn, polling, process-group termination,
/// or reaping fails.
pub fn observe_until_timeout(
    command: &mut Command,
    description: &str,
    timeout: Duration,
) -> Result<TimedProcessStatus, ProcessFailure> {
    let tool = command.get_program().to_string_lossy().into_owned();
    run_status_with_timeout(command, timeout)
        .map_err(|error| process_io_failure(description, tool, Some(timeout), &error))
}

/// Convert an evidence-producing process's unexplained early nonzero exit into
/// the same typed diagnostic context used by ordinary subprocess failures.
#[must_use]
pub fn unexpected_observed_exit(observed: TimedProcessStatus, description: &str) -> ProcessFailure {
    ProcessFailure::exit(description, observed.tool, observed.status, "")
}

fn run_captured_command(
    command: &mut Command,
    description: &str,
    replay_success: bool,
    timeout: Option<Duration>,
) -> Result<(), ProcessFailure> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let observed = capture_process(command, description, &tool, timeout)?;
    let status = observed.status;
    if observed.timed_out {
        let detail = bounded_output_detail(&observed.stdout, &observed.stderr);
        return Err(ProcessFailure::timeout(
            description,
            tool,
            status,
            configured_timeout(timeout, description)?,
            &detail,
        ));
    }
    if status.success() {
        if replay_success {
            replay(
                &observed.stdout,
                &mut std::io::stdout(),
                description,
                &tool,
                true,
            )?;
            replay(
                &observed.stderr,
                &mut std::io::stderr(),
                description,
                &tool,
                false,
            )?;
        }
        return Ok(());
    }
    let detail = bounded_output_detail(&observed.stdout, &observed.stderr);
    Err(ProcessFailure::exit(description, tool, status, &detail))
}

fn replay(
    stream: &CapturedStream,
    destination: &mut impl Write,
    description: &str,
    tool: &str,
    broken_pipe_is_success: bool,
) -> Result<(), ProcessFailure> {
    match stream
        .write_rendered(destination)
        .and_then(|()| destination.flush())
    {
        Ok(()) => Ok(()),
        Err(error) if broken_pipe_is_success && error.kind() == std::io::ErrorKind::BrokenPipe => {
            Ok(())
        }
        Err(error) => Err(ProcessFailure::execution(
            description,
            tool.to_owned(),
            &error,
        )),
    }
}

impl ErrorBoundary {
    /// Classification for invalid command-line input.
    pub const INVOCATION: Self = Self {
        code: DiagnosticCode::CliInvocation,
        stage: DiagnosticStage::Invocation,
        hint: "run `aros --help` or `aros <command> --help` to inspect the accepted command line",
    };

    /// Classification for checkout discovery and repository-state failures.
    pub const REPOSITORY: Self = Self {
        code: DiagnosticCode::CliRepository,
        stage: DiagnosticStage::RepositoryDiscovery,
        hint: "run inside an AROS checkout, or create one with `aros source init PATH`",
    };
}

#[must_use]
/// Convert a Clap parse failure into the shared diagnostic schema.
pub fn clap_diagnostic(error: &clap::Error) -> Diagnostic {
    Diagnostic::error(
        ErrorBoundary::INVOCATION.code,
        ErrorBoundary::INVOCATION.stage,
        error.to_string().trim().to_owned(),
    )
    .with_hint(ErrorBoundary::INVOCATION.hint)
}

#[must_use]
/// Convert a rich error report into one stable external diagnostic.
pub fn report_diagnostic(
    error: &miette::Report,
    mut boundary: ErrorBoundary,
    mut context: DiagnosticContext,
) -> Diagnostic {
    if let Some(classified) = error.downcast_ref::<ClassifiedFailure>() {
        boundary = classified.boundary;
    }
    if let Some(process) = error.downcast_ref::<ProcessFailure>() {
        process.apply_context(&mut context);
        if let Some(process_boundary) = process.boundary {
            boundary = process_boundary;
        }
    }
    if let Some(commit) = error.downcast_ref::<CommitStateFailure>() {
        context.commit_state = Some(commit.state);
    }
    Diagnostic::error(boundary.code, boundary.stage, render_error_chain(error))
        .with_hint(boundary.hint)
        .with_context(context)
}

fn render_error_chain(error: &miette::Report) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        if messages.last() != Some(&message) {
            messages.push(message);
        }
        source = cause.source();
    }
    messages.join(": ")
}

#[must_use]
/// Construct a diagnostic set for the process renderer.
pub fn set(diagnostics: Vec<Diagnostic>) -> DiagnosticSet {
    DiagnosticSet::new(diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_conversion_keeps_the_cause_chain_and_stable_boundary() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing fixture");
        let report = miette::miette!(io).wrap_err("cannot load configuration");
        let diagnostic = report_diagnostic(
            &report,
            ErrorBoundary::REPOSITORY,
            DiagnosticContext::default(),
        );
        assert_eq!(diagnostic.code, DiagnosticCode::CliRepository);
        assert!(diagnostic.message.contains("cannot load configuration"));
        assert!(diagnostic.message.contains("missing fixture"));
    }

    #[test]
    fn report_conversion_preserves_typed_commit_state() {
        let report = commit_state::<()>(
            Err(miette::miette!("publication failed")),
            CommitState::Indeterminate,
            "publication state could not be proven",
        )
        .unwrap_err();
        let diagnostic = report_diagnostic(
            &report,
            ErrorBoundary::REPOSITORY,
            DiagnosticContext::default(),
        );
        assert_eq!(
            diagnostic.context.unwrap().commit_state,
            Some(CommitState::Indeterminate)
        );
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_capture_drains_but_hard_limits_both_streams() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "yes stdout | head -c 131072; yes stderr | head -c 131072 >&2; exit 19",
        ]);
        let captured = capture_process(&mut command, "bounded fixture", "sh", None).unwrap();
        assert_eq!(captured.status.code(), Some(19));
        assert_eq!(captured.stdout.total_bytes(), 131_072);
        assert_eq!(captured.stderr.total_bytes(), 131_072);
        assert!(captured.stdout.is_truncated());
        assert!(captured.stderr.is_truncated());
        let detail = bounded_output_detail(&captured.stdout, &captured.stderr);
        assert!(detail.contains("stdout:"));
        assert!(detail.contains("stderr:"));
        assert_eq!(detail.matches("bytes omitted by aros").count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn machine_probe_rejects_oversized_success_output() {
        let mut command = Command::new("sh");
        command.args(["-c", "yes probe | head -c 131072"]);
        let error = capture_stdout(&mut command, "oversized fixture").unwrap_err();
        assert!(error
            .to_string()
            .contains("stdout exceeded the 64 KiB machine-output limit"));
    }

    #[cfg(unix)]
    #[test]
    fn timeout_is_structured_and_kills_the_process_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 10 & wait"]);
        let error =
            capture_stdout_with_timeout(&mut command, "timed fixture", Duration::from_millis(30))
                .unwrap_err();
        assert!(error.to_string().contains("timed out after 30 ms"));
        let report = miette::Report::new(error);
        let diagnostic = report_diagnostic(
            &report,
            ErrorBoundary::REPOSITORY,
            DiagnosticContext::default(),
        );
        let context = diagnostic.context.as_ref().unwrap();
        assert_eq!(context.timed_out, Some(true));
        assert_eq!(context.timeout_ms, Some(30));
    }
}
