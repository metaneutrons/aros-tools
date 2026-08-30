use std::ffi::OsString;
use std::process::ExitCode;

use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage};
use aros_release::archive;
use aros_release::contract::{Cli, Command};
use aros_release::observability::{
    render, requested_diagnostic_format, DiagnosticFormat, LogLevel, Logger,
};
use aros_release::ReleaseFailure;
use clap::{error::ErrorKind, Parser};

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
            print!("{error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            render(
                &DiagnosticSet::single(Diagnostic::error(
                    DiagnosticCode::ReleaseInvocation,
                    DiagnosticStage::Invocation,
                    error.to_string().trim().to_owned(),
                )),
                requested_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let mut logger = match Logger::open(cli.log_level, cli.log_format, cli.log_file) {
        Ok(logger) => logger,
        Err(error) => return render_failure(error, cli.diagnostic_format),
    };
    let context = match &cli.command {
        Command::Package(args) => DiagnosticContext {
            mode: Some("package".into()),
            target: Some(args.target.clone()),
            output: Some(args.output_dir.display().to_string()),
            ..DiagnosticContext::default()
        },
        Command::Verify(args) => DiagnosticContext {
            mode: Some("verify".into()),
            target: Some(args.archive.display().to_string()),
            ..DiagnosticContext::default()
        },
    };
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.start",
        "validated release operation started",
        &context,
    ) {
        return render_failure(error, cli.diagnostic_format);
    }
    let operation = match &cli.command {
        Command::Package(args) => archive::package(args).map(|_| ()),
        Command::Verify(args) => archive::verify(args).map(|_| ()),
    };
    if let Err(error) = operation {
        return render_logged_failure(error, &mut logger, cli.diagnostic_format);
    }
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.complete",
        "validated release operation completed",
        &context,
    ) {
        return render_failure(error, cli.diagnostic_format);
    }
    ExitCode::SUCCESS
}

fn render_failure(error: ReleaseFailure, format: DiagnosticFormat) -> ExitCode {
    render(&DiagnosticSet::single(error.into_diagnostic()), format);
    ExitCode::FAILURE
}

fn render_logged_failure(
    error: ReleaseFailure,
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
