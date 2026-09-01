use std::ffi::OsString;
use std::process::ExitCode;

use aros_common::{
    write_stdout, CommitState, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet,
    DiagnosticStage,
};
use aros_release::archive;
use aros_release::contract::{Cli, Command};
use aros_release::ecosystem;
use aros_release::install;
use aros_release::observability::{
    render, requested_diagnostic_format, DiagnosticFormat, LogLevel, Logger,
};
use aros_release::ReleaseFailure;
use clap::{error::ErrorKind, Parser, ValueEnum};

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
            return match write_stdout(&error.to_string()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(output_error) => render_failure(
                    ReleaseFailure::new(
                        Diagnostic::error(
                            DiagnosticCode::ReleaseObservability,
                            DiagnosticStage::Observability,
                            format!("cannot write help or version output: {output_error}"),
                        )
                        .with_hint("check the stdout destination and available filesystem or pipe resources"),
                    ),
                    requested_format,
                ),
            };
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
        Command::Generate(args) => {
            let Some(format) = args.format.to_possible_value() else {
                return render_failure(
                    ReleaseFailure::new(
                        Diagnostic::error(
                            DiagnosticCode::ReleaseInternal,
                            DiagnosticStage::Internal,
                            "validated package-manager format has no clap value identity",
                        )
                        .with_hint("stop publication and report this AP0999 diagnostic"),
                    ),
                    cli.diagnostic_format,
                );
            };
            DiagnosticContext {
                mode: Some("generate".into()),
                target: Some(format.get_name().into()),
                output: Some(args.output.display().to_string()),
                ..DiagnosticContext::default()
            }
        }
        Command::Install(args) => DiagnosticContext {
            mode: Some("install".into()),
            target: Some(args.source_bin.display().to_string()),
            output: Some(args.prefix.join("bin").display().to_string()),
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
        Command::Generate(args) => ecosystem::generate(args),
        Command::Install(args) => install::install(args).map(|_| ()),
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
        if matches!(&cli.command, Command::Install(_)) {
            let mut committed = context;
            committed.commit_state = Some(CommitState::Committed);
            let diagnostic = error.into_diagnostic().with_context(committed);
            render(&DiagnosticSet::single(diagnostic), cli.diagnostic_format);
            return ExitCode::FAILURE;
        }
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
