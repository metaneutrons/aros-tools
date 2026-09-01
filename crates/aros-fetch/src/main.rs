use std::ffi::OsString;
use std::process::ExitCode;

use aros_common::{
    CommitState, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage,
};
use aros_fetch::contract::{normalize_legacy_arguments, Cli, FetchRequest};
use aros_fetch::engine;
use aros_fetch::observability::{
    render, requested_diagnostic_format, DiagnosticFormat, LogLevel, Logger,
};
use aros_fetch::FetchFailure;
use clap::{error::ErrorKind, Parser};

#[tokio::main]
async fn main() -> ExitCode {
    let original: Vec<OsString> = std::env::args_os().collect();
    let requested_format = requested_diagnostic_format(&original);
    let arguments = normalize_legacy_arguments(original);
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
                                DiagnosticCode::FetchObservability,
                                DiagnosticStage::FetchObservability,
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
                        DiagnosticCode::FetchInvocation,
                        DiagnosticStage::FetchInvocation,
                        error.to_string().trim().to_owned(),
                    )
                    .with_hint(
                        "run 'aros-fetch --help' and supply the required --archive contract",
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
            return render_failure(error.into_diagnostic(), cli.diagnostic_format);
        }
    };
    let request = match FetchRequest::from_cli(&cli) {
        Ok(request) => request,
        Err(error) => return render_logged_failure(error, &mut logger, cli.diagnostic_format),
    };
    let context = DiagnosticContext {
        mode: Some(if request.offline { "offline" } else { "online" }.into()),
        target: Some(request.destination.display().to_string()),
        output: Some(request.archive.clone()),
        ..DiagnosticContext::default()
    };
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.start",
        "validated fetch invocation started",
        &context,
    ) {
        return render_failure(error.into_diagnostic(), cli.diagnostic_format);
    }
    let outcome = match engine::run(&request, &mut logger).await {
        Ok(outcome) => outcome,
        Err(error) => return render_logged_failure(error, &mut logger, cli.diagnostic_format),
    };
    if let Some(diagnostic) = aros_common::take_stdout_failure_diagnostic(
        DiagnosticCode::FetchObservability,
        DiagnosticStage::FetchObservability,
    ) {
        return render_logged_failure(
            FetchFailure::new(with_commit_state(
                diagnostic,
                if outcome.committed {
                    CommitState::Committed
                } else {
                    CommitState::RolledBack
                },
            )),
            &mut logger,
            cli.diagnostic_format,
        );
    }
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.complete",
        "validated fetch invocation completed",
        &context,
    ) {
        return render_failure(
            with_commit_state(
                error.into_diagnostic(),
                if outcome.committed {
                    CommitState::Committed
                } else {
                    CommitState::RolledBack
                },
            ),
            cli.diagnostic_format,
        );
    }
    ExitCode::SUCCESS
}

fn with_commit_state(mut diagnostic: Diagnostic, state: CommitState) -> Diagnostic {
    diagnostic
        .context
        .get_or_insert_with(DiagnosticContext::default)
        .commit_state = Some(state);
    diagnostic
}

fn render_failure(diagnostic: Diagnostic, format: DiagnosticFormat) -> ExitCode {
    render(&DiagnosticSet::single(diagnostic), format);
    ExitCode::FAILURE
}

fn render_logged_failure(
    error: FetchFailure,
    logger: &mut Logger,
    format: DiagnosticFormat,
) -> ExitCode {
    let commit_state = error
        .diagnostic()
        .context
        .as_ref()
        .and_then(|context| context.commit_state);
    let mut diagnostics = vec![error.into_diagnostic()];
    if let Err(logging_error) = logger.diagnostic(&diagnostics[0]) {
        let mut diagnostic = logging_error.into_diagnostic();
        if let Some(state) = commit_state {
            diagnostic = with_commit_state(diagnostic, state);
        }
        diagnostics.push(diagnostic);
    }
    render(&DiagnosticSet::new(diagnostics), format);
    ExitCode::FAILURE
}
