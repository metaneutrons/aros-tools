use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use aros_ahi_runner::contract::Contract;
use aros_ahi_runner::engine;
use aros_ahi_runner::observability::{
    render, requested_diagnostic_format, DiagnosticFormat, LogFormat, LogLevel, Logger,
};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage};
use clap::{error::ErrorKind, Parser};

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "Validate and execute the closed AROS AHI build contract",
    after_help = "OBSERVABILITY:\n  --diagnostic-format human|json\n  --log-level off|error|warn|info|debug|trace\n  --log-format human|jsonl\n  --log-file PATH\n\nLogging is off by default and is written only to an explicitly selected local file."
)]
struct Cli {
    #[arg(long)]
    contract: PathBuf,

    #[arg(long)]
    validate_only: bool,

    #[arg(long, value_enum, default_value_t = DiagnosticFormat::Human, env = "AROS_AHI_DIAGNOSTIC_FORMAT")]
    diagnostic_format: DiagnosticFormat,

    #[arg(long, value_enum, default_value_t = LogLevel::Off, env = "AROS_AHI_LOG_LEVEL")]
    log_level: LogLevel,

    #[arg(long, value_enum, default_value_t = LogFormat::Human, env = "AROS_AHI_LOG_FORMAT")]
    log_format: LogFormat,

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
            print!("{error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            render(
                &DiagnosticSet::single(Diagnostic::error(
                    DiagnosticCode::AhiInvocation,
                    DiagnosticStage::AhiInvocation,
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
        if let Err(error) = logger.event(
            LogLevel::Info,
            "execution.complete",
            "native AHI execution engine completed",
            &context,
        ) {
            return render_failure(error.into_diagnostic(), cli.diagnostic_format);
        }
    }
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.complete",
        "AHI runner invocation completed",
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

fn render_logged_failure(diagnostic: Diagnostic, logger: &mut Logger, cli: &Cli) -> ExitCode {
    let mut diagnostics = vec![diagnostic];
    if let Err(error) = logger.diagnostic(&diagnostics[0]) {
        diagnostics.push(error.into_diagnostic());
    }
    render(&DiagnosticSet::new(diagnostics), cli.diagnostic_format);
    ExitCode::FAILURE
}
