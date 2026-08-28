use std::ffi::OsString;
use std::process::ExitCode;

use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage};
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
            print!("{error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            render(
                &DiagnosticSet::single(Diagnostic::error(
                    DiagnosticCode::FetchInvocation,
                    DiagnosticStage::FetchInvocation,
                    error.to_string().trim().to_owned(),
                )),
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
    if let Err(error) = engine::run(&request, &mut logger).await {
        return render_logged_failure(error, &mut logger, cli.diagnostic_format);
    }
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.complete",
        "validated fetch invocation completed",
        &context,
    ) {
        return render_failure(error.into_diagnostic(), cli.diagnostic_format);
    }
    ExitCode::SUCCESS
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
    let mut diagnostics = vec![error.into_diagnostic()];
    if let Err(logging_error) = logger.diagnostic(&diagnostics[0]) {
        diagnostics.push(logging_error.into_diagnostic());
    }
    render(&DiagnosticSet::new(diagnostics), format);
    ExitCode::FAILURE
}
