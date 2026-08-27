//! AROS linker collection for direct CMake links and compiler-driver aliases.
//!
//! Both front ends feed one collection engine. The direct `aros-collect --ld`
//! form preserves CMake's explicit relocatable-link contract and report files.
//! The released `collect-aros` and `collect-aros32` aliases additionally enable
//! sysroot-owned extras, undefined-symbol auditing, stripping, AROS ABI
//! marking, and executable permissions.

mod engine;
mod extra;
mod libreq;
mod observability;
mod sets;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage};
use clap::{error::ErrorKind, Parser};

use observability::{
    failure, requested_diagnostic_format, CollectorResult, LogLevel, Logger, RuntimeOptions,
};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Link an AROS relocatable object and collect its symbol sets",
    propagate_version = true,
    after_help = "OBSERVABILITY:\n  --diagnostic-format human|json\n  --log-level off|error|warn|info|debug|trace\n  --log-format human|jsonl\n  --log-file PATH\n\nThe same settings are available through AROS_COLLECT_DIAGNOSTIC_FORMAT,\nAROS_COLLECT_LOG_LEVEL, AROS_COLLECT_LOG_FORMAT, and AROS_COLLECT_LOG_FILE.\nLogging is off by default and is written only to an explicitly selected local file."
)]
struct Cli {
    /// The real linker to drive.
    #[arg(long)]
    ld: PathBuf,

    /// Keep the generated linker script at this path instead of removing it.
    #[arg(long)]
    keep_script: Option<PathBuf>,

    /// Report set sections that could not be laid out to this file. Written
    /// only when there is something to report, and removed when there is not.
    #[arg(long)]
    report: Option<PathBuf>,

    /// The linker command line, including its `-o <output>`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

/// Where `-o` names the output, and what it names.
///
/// Both spellings the reference accepts (`collect-aros.c:181`): `-o path` and
/// `-opath`.
fn output_argument(args: &[OsString]) -> Result<(usize, bool, PathBuf)> {
    for (index, argument) in args.iter().enumerate() {
        let text = argument.to_string_lossy();
        if text == "-o" {
            let value = args
                .get(index + 1)
                .context("the linker command line ends after -o")?;
            return Ok((index + 1, false, PathBuf::from(value)));
        }
        if let Some(rest) = text.strip_prefix("-o") {
            if !rest.is_empty() {
                return Ok((index, true, PathBuf::from(rest)));
            }
        }
    }
    bail!("the linker command line has no -o <output>");
}

fn main() -> ExitCode {
    let raw: Vec<OsString> = std::env::args_os().collect();
    let requested_format = requested_diagnostic_format(&raw);
    let (options, arguments) = match RuntimeOptions::extract(raw) {
        Ok(value) => value,
        Err(error) => {
            observability::render(
                &DiagnosticSet::single(error.into_diagnostic()),
                requested_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let driver_mode = engine::is_driver_invocation(arguments.first().map(OsString::as_os_str));
    let invocation = arguments
        .first()
        .and_then(|value| Path::new(value).file_stem())
        .and_then(|value| value.to_str())
        .unwrap_or("aros-collect");
    let logger = match Logger::open(&options, invocation) {
        Ok(logger) => logger,
        Err(error) => {
            observability::render(
                &DiagnosticSet::single(error.into_diagnostic()),
                options.diagnostic_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let invocation_context = DiagnosticContext {
        mode: Some(if driver_mode { "driver" } else { "direct" }.into()),
        ..DiagnosticContext::default()
    };
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.start",
        "collector invocation started",
        &invocation_context,
    ) {
        observability::render(
            &DiagnosticSet::single(error.into_diagnostic()),
            options.diagnostic_format,
        );
        return ExitCode::FAILURE;
    }

    let mut diagnostics = Vec::new();
    let result = if driver_mode {
        engine::run_entry(arguments, &logger, &mut diagnostics)
    } else {
        run_direct(arguments, &logger, &mut diagnostics)
    };
    match result {
        Ok(()) => match logger.event(
            LogLevel::Info,
            "invocation.complete",
            "collector invocation completed",
            &invocation_context,
        ) {
            Ok(()) => {
                if !diagnostics.is_empty() {
                    observability::render(
                        &DiagnosticSet::new(diagnostics),
                        options.diagnostic_format,
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => {
                diagnostics.push(error.into_diagnostic());
                observability::render(&DiagnosticSet::new(diagnostics), options.diagnostic_format);
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            let log_result = logger.diagnostic(error.diagnostic());
            diagnostics.push(error.into_diagnostic());
            if let Err(log_error) = log_result {
                diagnostics.push(log_error.into_diagnostic());
            }
            observability::render(&DiagnosticSet::new(diagnostics), options.diagnostic_format);
            ExitCode::FAILURE
        }
    }
}

fn run_direct(
    arguments: Vec<OsString>,
    logger: &Logger,
    diagnostics: &mut Vec<Diagnostic>,
) -> CollectorResult<()> {
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            print!("{error}");
            return Ok(());
        }
        Err(error) => {
            return Err(failure(
                DiagnosticCode::CollectorInvocation,
                DiagnosticStage::Invocation,
                error.to_string().trim().to_owned(),
                DiagnosticContext::default(),
            ));
        }
    };
    if cli.args.is_empty() {
        return Err(failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            "no linker command line was given",
            DiagnosticContext::default(),
        ));
    }
    let (_, _, output) = output_argument(&cli.args).map_err(|error| {
        failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            format!("{error:#}"),
            DiagnosticContext::default(),
        )
    })?;
    engine::run_direct(
        cli.ld,
        cli.args,
        output,
        cli.report,
        cli.keep_script,
        logger,
        diagnostics,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_separate_output_argument_is_found() {
        let (index, joined, path) = output_argument(&args(&["-r", "a.o", "-o", "out.o"])).unwrap();
        assert_eq!(index, 3);
        assert!(!joined);
        assert_eq!(path, PathBuf::from("out.o"));
    }

    #[test]
    fn the_joined_output_argument_is_found() {
        let (index, joined, path) = output_argument(&args(&["-r", "-oout.o", "a.o"])).unwrap();
        assert_eq!(index, 1);
        assert!(joined);
        assert_eq!(path, PathBuf::from("out.o"));
    }

    #[test]
    fn a_command_line_without_an_output_is_refused() {
        assert!(output_argument(&args(&["-r", "a.o"])).is_err());
    }
}
