//! Stable `aros` diagnostics and opt-in local logging.

use aros_common::{
    bounded_output_detail, exit_signal, run_output as execute_output, run_status as execute_status,
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticFormat, DiagnosticSet,
    DiagnosticStage, LogLevel, Logger, ObservabilityPolicy,
};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::OnceLock;

static LOGGER: OnceLock<Logger> = OnceLock::new();
static DIAGNOSTIC_FORMAT: OnceLock<DiagnosticFormat> = OnceLock::new();

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

pub fn install_runtime(logger: Logger, format: DiagnosticFormat) -> &'static Logger {
    assert!(LOGGER.set(logger).is_ok(), "aros logger installed twice");
    assert!(
        DIAGNOSTIC_FORMAT.set(format).is_ok(),
        "aros diagnostic format installed twice"
    );
    LOGGER.get().expect("aros logger was just installed")
}

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

#[derive(Debug, Clone, Copy)]
pub struct ErrorBoundary {
    pub code: DiagnosticCode,
    pub stage: DiagnosticStage,
    pub hint: &'static str,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("{message}")]
pub struct ProcessFailure {
    message: String,
    tool: String,
    exit_code: Option<i32>,
    signal: Option<i32>,
    boundary: Option<ErrorBoundary>,
}

impl ProcessFailure {
    fn start(description: &str, tool: String, error: &std::io::Error) -> Self {
        Self {
            message: format!("could not start {description}: {error}"),
            tool,
            exit_code: None,
            signal: None,
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
            boundary: None,
        }
    }

    fn apply_context(&self, context: &mut DiagnosticContext) {
        context.tool = Some(self.tool.clone());
        context.exit_code = self.exit_code;
        context.signal = self.signal;
    }

    const fn with_boundary(mut self, boundary: ErrorBoundary) -> Self {
        self.boundary = Some(boundary);
        self
    }
}

pub fn run_command(command: &mut Command, description: &str) -> Result<(), ProcessFailure> {
    if DIAGNOSTIC_FORMAT.get() == Some(&DiagnosticFormat::Json) {
        return run_captured_command(command, description, true);
    }
    run_interactive_command(command, description)
}

pub fn run_command_at(
    command: &mut Command,
    description: &str,
    boundary: ErrorBoundary,
) -> Result<(), ProcessFailure> {
    run_command(command, description).map_err(|error| error.with_boundary(boundary))
}

pub fn run_quiet_command(command: &mut Command, description: &str) -> Result<(), ProcessFailure> {
    if DIAGNOSTIC_FORMAT.get() == Some(&DiagnosticFormat::Json) {
        return run_captured_command(command, description, false);
    }
    run_interactive_command(command, description)
}

/// Run a command whose stdout is structured input for the caller.
///
/// Output is never replayed to the terminal.  A failed command is converted
/// into the same bounded, machine-attributed failure used by interactive CLI
/// subprocesses.
pub fn run_output_at(
    command: &mut Command,
    description: &str,
    boundary: ErrorBoundary,
) -> Result<Output, ProcessFailure> {
    run_output_checked(command, description).map_err(|error| error.with_boundary(boundary))
}

fn run_output_checked(command: &mut Command, description: &str) -> Result<Output, ProcessFailure> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let output = execute_output(command)
        .map_err(|error| ProcessFailure::start(description, tool, &error))?;
    let tool = output.tool;
    let output = output.output;
    if output.status.success() {
        return Ok(output);
    }
    let detail = bounded_output_detail(&output.stdout, &output.stderr, 64 * 1024);
    Err(ProcessFailure::exit(
        description,
        tool,
        output.status,
        &detail,
    ))
}

pub fn run_interactive_command(
    command: &mut Command,
    description: &str,
) -> Result<(), ProcessFailure> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let observed = execute_status(command)
        .map_err(|error| ProcessFailure::start(description, tool, &error))?;
    let tool = observed.tool;
    let status = observed.status;
    if !status.success() {
        return Err(ProcessFailure::exit(description, tool, status, ""));
    }
    Ok(())
}

fn run_captured_command(
    command: &mut Command,
    description: &str,
    replay_success: bool,
) -> Result<(), ProcessFailure> {
    let tool = command.get_program().to_string_lossy().into_owned();
    let mut stdout = tempfile::tempfile()
        .map_err(|error| ProcessFailure::start(description, tool.clone(), &error))?;
    let mut stderr = tempfile::tempfile()
        .map_err(|error| ProcessFailure::start(description, tool.clone(), &error))?;
    let child_stdout = stdout
        .try_clone()
        .map_err(|error| ProcessFailure::start(description, tool.clone(), &error))?;
    let child_stderr = stderr
        .try_clone()
        .map_err(|error| ProcessFailure::start(description, tool.clone(), &error))?;
    let observed = execute_status(
        command
            .stdout(Stdio::from(child_stdout))
            .stderr(Stdio::from(child_stderr)),
    )
    .map_err(|error| ProcessFailure::start(description, tool.clone(), &error))?;
    let status = observed.status;
    if status.success() {
        if replay_success {
            replay(&mut stdout, &mut std::io::stdout(), description, &tool)?;
            replay(&mut stderr, &mut std::io::stderr(), description, &tool)?;
        }
        return Ok(());
    }
    let detail = process_detail(&mut stdout, &mut stderr, description, &tool)?;
    Err(ProcessFailure::exit(description, tool, status, &detail))
}

fn replay(
    file: &mut File,
    destination: &mut impl Write,
    description: &str,
    tool: &str,
) -> Result<(), ProcessFailure> {
    file.seek(SeekFrom::Start(0))
        .and_then(|_| std::io::copy(file, destination).map(|_| ()))
        .and_then(|()| destination.flush())
        .map_err(|error| ProcessFailure::start(description, tool.to_owned(), &error))
}

fn process_detail(
    stdout: &mut File,
    stderr: &mut File,
    description: &str,
    tool: &str,
) -> Result<String, ProcessFailure> {
    const LIMIT: usize = 64 * 1024;

    fn part(
        file: &mut File,
        label: &str,
        description: &str,
        tool: &str,
    ) -> Result<String, ProcessFailure> {
        file.seek(SeekFrom::Start(0))
            .map_err(|error| ProcessFailure::start(description, tool.to_owned(), &error))?;
        let mut bytes = Vec::with_capacity(LIMIT + 1);
        file.take((LIMIT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| ProcessFailure::start(description, tool.to_owned(), &error))?;
        let truncated = bytes.len() > LIMIT;
        bytes.truncate(LIMIT);
        let mut text = String::from_utf8_lossy(&bytes).trim().to_owned();
        if truncated {
            text.push_str("\n[output truncated by aros]");
        }
        if text.is_empty() {
            Ok(String::new())
        } else {
            Ok(format!("{label}:\n{text}"))
        }
    }

    let stdout = part(stdout, "stdout", description, tool)?;
    let stderr = part(stderr, "stderr", description, tool)?;
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => Ok(String::new()),
        (false, true) => Ok(stdout),
        (true, false) => Ok(stderr),
        (false, false) => Ok(format!("{stdout}\n{stderr}")),
    }
}

impl ErrorBoundary {
    pub const INVOCATION: Self = Self {
        code: DiagnosticCode::CliInvocation,
        stage: DiagnosticStage::Invocation,
        hint: "run `aros --help` or `aros <command> --help` to inspect the accepted command line",
    };

    pub const REPOSITORY: Self = Self {
        code: DiagnosticCode::CliRepository,
        stage: DiagnosticStage::RepositoryDiscovery,
        hint: "run the command inside an AROS-NG checkout containing aros-targets.toml",
    };

    pub const PI: Self = Self {
        code: DiagnosticCode::CliPi,
        stage: DiagnosticStage::PiOperation,
        hint: "inspect the reported host tool and Raspberry Pi identity data before retrying",
    };

    pub const MEDIA_SAFETY: Self = Self {
        code: DiagnosticCode::CliMediaSafety,
        stage: DiagnosticStage::MediaSafety,
        hint: "re-scan the removable device and resolve every reported identity or mount ambiguity before retrying",
    };
}

#[must_use]
pub fn clap_diagnostic(error: &clap::Error) -> Diagnostic {
    Diagnostic::error(
        ErrorBoundary::INVOCATION.code,
        ErrorBoundary::INVOCATION.stage,
        error.to_string().trim().to_owned(),
    )
    .with_hint(ErrorBoundary::INVOCATION.hint)
}

#[must_use]
pub fn report_diagnostic(
    error: &miette::Report,
    mut boundary: ErrorBoundary,
    mut context: DiagnosticContext,
) -> Diagnostic {
    if let Some(process) = error.downcast_ref::<ProcessFailure>() {
        process.apply_context(&mut context);
        if let Some(process_boundary) = process.boundary {
            boundary = process_boundary;
        }
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
    fn structured_output_failure_keeps_bounded_stdout_and_stderr() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf structured; printf problem >&2; exit 7"]);
        let error = run_output_at(&mut command, "fixture query", ErrorBoundary::PI)
            .expect_err("command must fail");
        assert_eq!(error.exit_code, Some(7));
        assert_eq!(
            error.boundary.map(|boundary| boundary.code),
            Some(DiagnosticCode::CliPi)
        );
        assert!(error.message.contains("stdout:\nstructured"));
        assert!(error.message.contains("stderr:\nproblem"));
    }
}
