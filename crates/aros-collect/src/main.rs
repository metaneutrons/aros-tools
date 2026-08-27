//! The part of `collect-aros` this build needs: symbol-set collection.
//!
//! For an AROS target the linker named by the compiler spec is not `ld` but
//! `collect-aros` (`config/elf-specs.in` `*linker:` -> `scripts/aros-ld.in:5`),
//! and `TARGET_LD` is the same wrapper (`configure:18209`, `use_ld_wrapper` is
//! unconditionally `yes`). It links twice: first `ld -r` over the inputs, then,
//! having read the section names out of that result, `ld -r -T <generated
//! script> <first result>` (`tools/collect-aros/collect-aros.c:650`). The
//! second pass is what turns the `.aros.set.*` sections into the arrays the
//! code reads.
//!
//! Which of its modes matters: `-r` and `-i` make it stop after the first pass,
//! `-Ur` makes it do both (`collect-aros.c:184` and `:188`). A module link uses
//! neither and gets both passes; a kickstart member and the kickstart itself
//! use `-Ur` and get both. Our link rule was a plain `ld.lld -r`
//! (`cmake/AROS.cmake:244`), which is exactly the one mode that skips the
//! collection, so every symbol set in this build was the empty weak stub.
//!
//! Two of collect-aros's jobs live here: the symbol sets (`sets`) and the
//! library-version markers (`libreq`). Both are emitted into one generated
//! script and one second pass, as `collect-aros.c:390` does.
//!
//! The released `collect-aros` aliases additionally implement `collect_extra`
//! (`backend-generic.c:117`). They obtain `static-cxx-cxa-pure-virtual.o` and
//! `libpthread.a` from an explicit Developer sysroot instead of embedding the
//! build machine's `OBJLIBDIR`.

mod driver;
mod extra;
mod libreq;
mod observability;
mod sets;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

use anyhow::{bail, Context, Result};
use aros_common::{Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticStage};
use clap::{error::ErrorKind, Parser};

use observability::{
    failure, requested_diagnostic_format, CollectorFailure, CollectorResult, LogLevel, Logger,
    RuntimeOptions,
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

fn run(ld: &Path, args: &[OsString]) -> Result<ExitStatus> {
    Command::new(ld)
        .args(args)
        .status()
        .with_context(|| format!("could not run {}", ld.display()))
}

fn write_report(path: Option<&PathBuf>, lines: &[String]) -> Result<()> {
    let Some(path) = path else { return Ok(()) };
    if lines.is_empty() {
        // Absent means clean, the same convention the transpiler's reports use.
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(path, body).with_context(|| format!("could not write {}", path.display()))
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
    let driver_mode = driver::is_driver_invocation(arguments.first().map(OsString::as_os_str));
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
        driver::run_entry(arguments, &logger, &mut diagnostics)
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
    collect(&cli, logger, diagnostics)
}

fn collect(cli: &Cli, logger: &Logger, diagnostics: &mut Vec<Diagnostic>) -> CollectorResult<()> {
    if cli.args.is_empty() {
        return Err(failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            "no linker command line was given",
            DiagnosticContext::default(),
        ));
    }
    let (index, joined, output) = output_argument(&cli.args).map_err(|error| {
        failure(
            DiagnosticCode::CollectorInvocation,
            DiagnosticStage::Invocation,
            format!("{error:#}"),
            DiagnosticContext::default(),
        )
    })?;
    let link_context = DiagnosticContext {
        tool: Some(cli.ld.display().to_string()),
        mode: Some("direct".into()),
        output: Some(output.display().to_string()),
        ..DiagnosticContext::default()
    };

    // The first pass writes beside the real output, so it lands on the same
    // filesystem and a failed link leaves the evidence next to the target it
    // was for.
    let mut staged = output.clone().into_os_string();
    staged.push(".collect-pre");
    let staged = PathBuf::from(staged);

    let mut first: Vec<OsString> = cli.args.clone();
    first[index] = if joined {
        let mut joined_argument = OsString::from("-o");
        joined_argument.push(&staged);
        joined_argument
    } else {
        staged.clone().into_os_string()
    };
    logger.event(
        LogLevel::Debug,
        "link.first.start",
        "starting first relocatable link",
        &link_context,
    )?;
    let status = run(&cli.ld, &first).map_err(|error| {
        failure(
            DiagnosticCode::CollectorFirstLink,
            DiagnosticStage::FirstLink,
            format!("{error:#}"),
            link_context.clone(),
        )
    })?;
    if !status.success() {
        return Err(process_failure(
            DiagnosticCode::CollectorFirstLink,
            DiagnosticStage::FirstLink,
            "the first relocatable link failed",
            status,
            link_context,
        ));
    }

    let object_context = DiagnosticContext {
        mode: Some("direct".into()),
        output: Some(staged.display().to_string()),
        ..DiagnosticContext::default()
    };
    let bytes = std::fs::read(&staged).map_err(|error| {
        failure(
            DiagnosticCode::CollectorObjectInspection,
            DiagnosticStage::ObjectInspection,
            format!("the linker wrote no readable {}: {error}", staged.display()),
            object_context.clone(),
        )
    })?;
    let object = aros_common::elf::read(&bytes).map_err(|error| {
        failure(
            DiagnosticCode::CollectorObjectInspection,
            DiagnosticStage::ObjectInspection,
            format!("could not inspect {}: {error:#}", staged.display()),
            object_context,
        )
    })?;
    let section_names = object.section_names();
    let (found, mut skipped) = sets::discover(&section_names);
    let (requirements, libreq_skipped) = libreq::discover(&object.symbols);
    skipped.extend(libreq_skipped);
    // Printed as well as written: a report file nobody aggregates is easy to
    // miss, and a section that looks like a set and is not laid out, or a
    // version requirement that is dropped, changes what the module does at
    // runtime.
    if !skipped.is_empty() {
        for line in &skipped {
            let diagnostic = Diagnostic::warning(
                DiagnosticCode::CollectorSetCollection,
                DiagnosticStage::SetCollection,
                line.clone(),
            )
            .with_context(DiagnosticContext {
                mode: Some("direct".into()),
                output: Some(output.display().to_string()),
                ..DiagnosticContext::default()
            });
            logger.diagnostic(&diagnostic)?;
            diagnostics.push(diagnostic);
        }
        logger.event(
            LogLevel::Warn,
            "collection.skipped",
            &format!(
                "{} set or library requirement entries were skipped",
                skipped.len()
            ),
            &DiagnosticContext {
                mode: Some("direct".into()),
                output: Some(output.display().to_string()),
                ..DiagnosticContext::default()
            },
        )?;
    }
    write_report(cli.report.as_ref(), &skipped).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSetCollection,
            DiagnosticStage::SetCollection,
            format!("{error:#}"),
            DiagnosticContext {
                output: cli.report.as_ref().map(|path| path.display().to_string()),
                ..DiagnosticContext::default()
            },
        )
    })?;

    if found.is_empty() && requirements.is_empty() {
        // Nothing to lay out, so the first pass is already the answer. The
        // reference runs its second pass regardless; skipping it here saves one
        // linker invocation on the majority of targets and cannot change the
        // result, because an empty script contributes nothing.
        std::fs::rename(&staged, &output).map_err(|error| {
            failure(
                DiagnosticCode::CollectorPublication,
                DiagnosticStage::Publication,
                format!(
                    "could not move {} to {}: {error}",
                    staged.display(),
                    output.display()
                ),
                DiagnosticContext {
                    mode: Some("direct".into()),
                    output: Some(output.display().to_string()),
                    ..DiagnosticContext::default()
                },
            )
        })?;
        return Ok(());
    }

    let script_path = cli.keep_script.clone().unwrap_or_else(|| {
        let mut path = output.clone().into_os_string();
        path.push(".collect-sets.ld");
        PathBuf::from(path)
    });
    let script = sets::script(&found, object.class, &libreq::script(&requirements));
    std::fs::write(&script_path, &script).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSetCollection,
            DiagnosticStage::SetCollection,
            format!("could not write {}: {error}", script_path.display()),
            DiagnosticContext {
                output: Some(script_path.display().to_string()),
                ..DiagnosticContext::default()
            },
        )
    })?;

    // `ld -r -o <output> <first pass> -T <script>`, as collect-aros.c:676
    // builds it. No other flag from the first pass is repeated -- the machine
    // comes from the input, and the inputs are already resolved into one
    // object.
    let second: Vec<OsString> = vec![
        OsString::from("-r"),
        OsString::from("-o"),
        output.into_os_string(),
        staged.clone().into_os_string(),
        OsString::from("-T"),
        script_path.clone().into_os_string(),
    ];
    logger.event(
        LogLevel::Debug,
        "link.second.start",
        "starting set-collection link",
        &link_context,
    )?;
    let status = run(&cli.ld, &second).map_err(|error| {
        failure(
            DiagnosticCode::CollectorSecondLink,
            DiagnosticStage::SecondLink,
            format!("{error:#}"),
            link_context.clone(),
        )
    })?;

    if cli.keep_script.is_none() {
        let _ = std::fs::remove_file(&script_path);
    }
    if status.success() {
        let _ = std::fs::remove_file(&staged);
        Ok(())
    } else {
        Err(process_failure(
            DiagnosticCode::CollectorSecondLink,
            DiagnosticStage::SecondLink,
            "the set-collection link failed",
            status,
            link_context,
        ))
    }
}

fn process_failure(
    code: DiagnosticCode,
    stage: DiagnosticStage,
    message: impl Into<String>,
    status: ExitStatus,
    mut context: DiagnosticContext,
) -> CollectorFailure {
    context.exit_code = status.code();
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        context.signal = status.signal();
    }
    failure(code, stage, message, context)
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
