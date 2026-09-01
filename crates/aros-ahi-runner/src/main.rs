use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use aros_ahi_runner::contract::Contract;
use aros_ahi_runner::engine;
use aros_ahi_runner::observability::{
    render, requested_diagnostic_format, DiagnosticFormat, LogFormat, LogLevel, Logger,
};
use aros_common::{
    CommitState, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage,
};
use clap::{error::ErrorKind, Parser};

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Validate and execute the closed AROS AHI build contract",
    long_about = "Validate one generated, declarative AROS AHI build contract and either audit its complete input/filesystem closure or execute its fixed native build stages without evaluating arbitrary CMake or shell code.",
    after_help = "CLOSED CONTRACT:\n  The contract must be a generated regular file with the exact supported schema,\n  contained source/build paths, declared source and product manifests, and measured SHA-256 identities.\n  --validate-only performs parsing, identity, filesystem, and input validation without build execution.\n\nOBSERVABILITY:\n  Diagnostics are written to stderr; --diagnostic-format=json selects the stable JSON contract.\n  Logging is off by default. A non-off --log-level requires an explicit --log-file.\n  --log-format selects human or jsonl.\n  Environment: AROS_AHI_DIAGNOSTIC_FORMAT, AROS_AHI_LOG_LEVEL,\n  AROS_AHI_LOG_FORMAT, AROS_AHI_LOG_FILE."
)]
struct Cli {
    /// Generated closed AHI contract file to validate and execute.
    #[arg(long)]
    contract: PathBuf,

    /// Validate the complete contract and input closure without executing a build.
    #[arg(long)]
    validate_only: bool,

    /// Failure renderer: human text or one stable JSON diagnostic document.
    #[arg(long, value_enum, default_value_t = DiagnosticFormat::Human, env = "AROS_AHI_DIAGNOSTIC_FORMAT")]
    diagnostic_format: DiagnosticFormat,

    /// Minimum level for the explicit local log; non-off requires --log-file.
    #[arg(long, value_enum, default_value_t = LogLevel::Off, env = "AROS_AHI_LOG_LEVEL")]
    log_level: LogLevel,

    /// Explicit local log encoding.
    #[arg(long, value_enum, default_value_t = LogFormat::Human, env = "AROS_AHI_LOG_FORMAT")]
    log_format: LogFormat,

    /// Explicit local log file; no log is written while --log-level is off.
    #[arg(long, env = "AROS_AHI_LOG_FILE")]
    log_file: Option<PathBuf>,
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    let requested_format = requested_diagnostic_format(&arguments);
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return match aros_common::write_stdout(&error.to_string()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(output_error) => {
                    render(
                        &DiagnosticSet::single(
                            Diagnostic::error(
                                DiagnosticCode::AhiObservability,
                                DiagnosticStage::AhiObservability,
                                format!("could not write command help: {output_error}"),
                            )
                            .with_hint("check the stdout destination and retry"),
                        ),
                        requested_format,
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Err(error) => {
            render(
                &DiagnosticSet::single(
                    Diagnostic::error(
                        DiagnosticCode::AhiInvocation,
                        DiagnosticStage::AhiInvocation,
                        error.to_string().trim().to_owned(),
                    )
                    .with_hint(
                        "run 'aros-ahi-runner --help' and provide one generated --contract file",
                    ),
                ),
                requested_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let mut logger = match Logger::open(cli.log_level, cli.log_format, cli.log_file.clone()) {
        Ok(logger) => logger,
        Err(error) => {
            render(
                &DiagnosticSet::single(error.into_diagnostic()),
                cli.diagnostic_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let invocation = DiagnosticContext {
        output: Some(cli.contract.display().to_string()),
        ..DiagnosticContext::default()
    };
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.start",
        "AHI runner invocation started",
        &invocation,
    ) {
        return render_failure(error.into_diagnostic(), cli.diagnostic_format);
    }
    let contract = match Contract::load(&cli.contract) {
        Ok(contract) => contract,
        Err(error) => return render_logged_failure(error.into_diagnostic(), &mut logger, &cli),
    };
    let context = DiagnosticContext {
        mode: Some(contract.mode.as_str().into()),
        target: Some(contract.target_triple.clone()),
        output: Some(cli.contract.display().to_string()),
        ..DiagnosticContext::default()
    };
    if let Err(error) = logger.event(
        LogLevel::Info,
        "contract.validated",
        "typed AHI contract validated",
        &context,
    ) {
        return render_failure(error.into_diagnostic(), cli.diagnostic_format);
    }
    if let Err(error) = contract.validate_filesystem(&cli.contract) {
        return render_logged_failure(error.into_diagnostic(), &mut logger, &cli);
    }
    if let Err(error) = logger.event(
        LogLevel::Info,
        "inputs.validated",
        "AHI filesystem and input audit completed",
        &context,
    ) {
        return render_failure(error.into_diagnostic(), cli.diagnostic_format);
    }
    let mut committed = false;
    if !cli.validate_only {
        if let Err(error) = logger.event(
            LogLevel::Info,
            "execution.start",
            "native AHI execution engine started",
            &context,
        ) {
            return render_failure(error.into_diagnostic(), cli.diagnostic_format);
        }
        if let Err(error) = engine::run(&contract, &mut logger) {
            return render_logged_failure(error.into_diagnostic(), &mut logger, &cli);
        }
        committed = true;
        if let Err(error) = logger.event(
            LogLevel::Info,
            "execution.complete",
            "native AHI execution engine completed",
            &context,
        ) {
            return render_failure(
                committed_observability_failure(error.into_diagnostic()),
                cli.diagnostic_format,
            );
        }
    }
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.complete",
        "AHI runner invocation completed",
        &context,
    ) {
        let diagnostic = if committed {
            committed_observability_failure(error.into_diagnostic())
        } else {
            error.into_diagnostic()
        };
        return render_failure(diagnostic, cli.diagnostic_format);
    }
    ExitCode::SUCCESS
}

fn committed_observability_failure(mut diagnostic: Diagnostic) -> Diagnostic {
    let original = diagnostic.message;
    diagnostic.message = format!(
        "AHI products were committed successfully, but completion logging failed: {original}"
    );
    diagnostic.hint = Some(
        "the installed AHI product set is authoritative; repair or disable the explicit log destination before the next build"
            .to_owned(),
    );
    let mut context = diagnostic.context.unwrap_or_default();
    context.commit_state = Some(CommitState::Committed);
    diagnostic.context = Some(context);
    diagnostic
}

fn render_failure(diagnostic: Diagnostic, format: DiagnosticFormat) -> ExitCode {
    render(&DiagnosticSet::single(diagnostic), format);
    ExitCode::FAILURE
}

fn render_logged_failure(diagnostic: Diagnostic, logger: &mut Logger, cli: &Cli) -> ExitCode {
    let mut diagnostics = vec![diagnostic];
    if let Err(error) = logger.diagnostic(&diagnostics[0]) {
        diagnostics.push(error.into_diagnostic());
    }
    render(&DiagnosticSet::new(diagnostics), cli.diagnostic_format);
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_commit_log_failure_is_typed_as_committed() {
        let diagnostic = Diagnostic::error(
            DiagnosticCode::AhiObservability,
            DiagnosticStage::AhiObservability,
            "disk full",
        );

        let committed = committed_observability_failure(diagnostic);

        assert_eq!(
            committed
                .context
                .as_ref()
                .and_then(|context| context.commit_state),
            Some(CommitState::Committed)
        );
        assert!(committed.message.contains("committed successfully"));
        assert!(committed.message.contains("disk full"));
        assert!(committed
            .hint
            .as_deref()
            .is_some_and(|hint| hint.contains("authoritative")));
    }
}
