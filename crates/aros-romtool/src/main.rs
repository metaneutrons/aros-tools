//! ROM and distribution packaging tool for AROS.
//!
//! Currently implements the kickstart package (`PKG`) container consumed by the
//! 32-bit bootstrap. See [`pkg`] for the format description.

mod pkg;
mod publication;

use anyhow::Context;
use aros_common::{
    render_diagnostics, requested_diagnostic_format, write_stdout, Diagnostic, DiagnosticCode,
    DiagnosticContext, DiagnosticFormat, DiagnosticSet, DiagnosticStage, LogFormat, LogLevel,
    Logger, ObservabilityPolicy, PublicationFailureClass, RecoveryOutcome, Sha256Digest,
    SourceLocation,
};
use clap::{error::ErrorKind, Parser, Subcommand};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const POLICY: ObservabilityPolicy = ObservabilityPolicy {
    log_schema: "aros-romtool-log-v1",
    component: "ROM tool",
    include_invocation: false,
    observability_code: DiagnosticCode::RomtoolObservability,
    observability_stage: DiagnosticStage::Observability,
    internal_code: DiagnosticCode::RomtoolInternal,
    internal_stage: DiagnosticStage::Internal,
    hint: "pass --log-file PATH or set AROS_ROMTOOL_LOG_FILE, or disable logging",
};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "ROM and distribution packaging tool for AROS",
    propagate_version = true
)]
struct Cli {
    /// Stable human or machine-readable diagnostics.
    #[arg(long, global = true, value_enum, default_value_t = DiagnosticFormat::Human, env = "AROS_ROMTOOL_DIAGNOSTIC_FORMAT")]
    diagnostic_format: DiagnosticFormat,

    /// Local logging threshold; logging is disabled by default.
    #[arg(long, global = true, value_enum, default_value_t = LogLevel::Off, env = "AROS_ROMTOOL_LOG_LEVEL")]
    log_level: LogLevel,

    /// Local log encoding.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Human, env = "AROS_ROMTOOL_LOG_FORMAT")]
    log_format: LogFormat,

    /// Explicit local log destination.
    #[arg(long, global = true, env = "AROS_ROMTOOL_LOG_FILE")]
    log_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Build, inspect, or unpack a kickstart package (PKG container).
    Pkg {
        #[command(subcommand)]
        action: PkgAction,
    },
}

#[derive(Subcommand, Debug)]
enum PkgAction {
    /// Pack ELF modules into a kickstart package.
    Create {
        /// Destination package file.
        #[arg(short, long)]
        output: PathBuf,

        /// Record only basenames instead of the paths as given. The bootstrap
        /// strips directories anyway, so this keeps packages reproducible
        /// across build directories.
        #[arg(long)]
        basename: bool,

        /// Do not fail when a member is not an ELF object. The bootstrap
        /// silently ignores such members.
        #[arg(long)]
        allow_non_elf: bool,

        /// Replace an existing regular file only if its current SHA-256 is
        /// exactly this value. Without this option creation is no-clobber.
        #[arg(long, value_name = "SHA256")]
        replace_if_sha256: Option<Sha256Digest>,

        /// Modules to pack, in load order.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// List the members of a package.
    List {
        /// Package file to inspect.
        package: PathBuf,
    },

    /// Unpack a package into a directory.
    Extract {
        /// Package file to unpack.
        package: PathBuf,

        /// Destination directory.
        #[arg(short = 'C', long)]
        directory: PathBuf,
    },
}

#[derive(Debug, Clone, Copy)]
enum FailureKind {
    Input,
    Validation,
    Publication,
    RollbackIncomplete,
}

#[derive(Debug)]
struct ToolFailure {
    kind: FailureKind,
    path: PathBuf,
    publication_class: Option<PublicationFailureClass>,
    source: anyhow::Error,
}

impl ToolFailure {
    fn new(kind: FailureKind, path: impl Into<PathBuf>, source: anyhow::Error) -> Self {
        Self {
            kind,
            path: path.into(),
            publication_class: None,
            source,
        }
    }

    fn publication(
        kind: FailureKind,
        path: impl Into<PathBuf>,
        class: PublicationFailureClass,
        source: anyhow::Error,
    ) -> Self {
        Self {
            kind,
            path: path.into(),
            publication_class: Some(class),
            source,
        }
    }

    fn from_create(error: pkg::CreateFailure) -> Self {
        let kind = match error.kind() {
            pkg::CreateFailureKind::Input => FailureKind::Input,
            pkg::CreateFailureKind::Validation => FailureKind::Validation,
            pkg::CreateFailureKind::Publication => FailureKind::Publication,
            pkg::CreateFailureKind::RollbackIncomplete => FailureKind::RollbackIncomplete,
        };
        let path = error.path().to_path_buf();
        let publication_class = error.publication_class();
        Self {
            kind,
            path,
            publication_class,
            source: error.into_source(),
        }
    }
}

type ToolResult<T> = std::result::Result<T, ToolFailure>;

#[derive(Clone, Copy, Debug, Default)]
struct OperationOutcome {
    recovery: RecoveryOutcome,
}

fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    let requested_format =
        requested_diagnostic_format(&arguments, "AROS_ROMTOOL_DIAGNOSTIC_FORMAT");
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
                Err(output_error) => {
                    render(
                        &DiagnosticSet::single(
                            Diagnostic::error(
                                DiagnosticCode::RomtoolObservability,
                                DiagnosticStage::Observability,
                                format!("cannot write help/version output: {output_error}"),
                            )
                            .with_hint("verify the stdout destination and retry"),
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
                        DiagnosticCode::RomtoolInvocation,
                        DiagnosticStage::Invocation,
                        error.to_string().trim().to_owned(),
                    )
                    .with_hint("run `aros-romtool --help` for the complete invocation contract"),
                ),
                requested_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let logger = match Logger::open(
        cli.log_level,
        cli.log_format,
        cli.log_file.clone(),
        "aros-romtool",
        POLICY,
    ) {
        Ok(logger) => logger,
        Err(error) => {
            render(
                &DiagnosticSet::single(error.into_diagnostic()),
                cli.diagnostic_format,
            );
            return ExitCode::FAILURE;
        }
    };
    let context = command_context(&cli.command);
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.start",
        "ROM tool invocation started",
        &context,
    ) {
        return render_failure(error.into_diagnostic(), cli.diagnostic_format);
    }

    let result = match &cli.command {
        Commands::Pkg { action } => match action {
            PkgAction::Create {
                output,
                basename,
                allow_non_elf,
                replace_if_sha256,
                files,
            } => create(
                output,
                files,
                *basename,
                *allow_non_elf,
                replace_if_sha256.as_ref(),
            ),
            PkgAction::List { package } => list(package),
            PkgAction::Extract { package, directory } => extract(package, directory),
        },
    };

    match result {
        Ok(outcome) => {
            if outcome.recovery.recovered() {
                if let Err(error) = logger.event(
                    LogLevel::Warn,
                    "publication.recovery",
                    &format!(
                        "ROM tool recovered predecessor publication state ({})",
                        outcome.recovery.as_str()
                    ),
                    &context,
                ) {
                    let mut stderr = std::io::stderr().lock();
                    let _ = writeln!(
                        stderr,
                        "warning: publication recovery completed ({}) but recovery logging failed: {error}",
                        outcome.recovery.as_str()
                    );
                }
            }
            if let Err(error) = logger.event(
                LogLevel::Info,
                "invocation.complete",
                "ROM tool invocation completed",
                &context,
            ) {
                let mut stderr = std::io::stderr().lock();
                let _ = writeln!(
                    stderr,
                    "warning: operation committed successfully, but completion logging failed: {error}"
                );
            }
            if let Some(diagnostic) = aros_common::take_stdout_failure_diagnostic(
                DiagnosticCode::RomtoolObservability,
                DiagnosticStage::Observability,
            ) {
                let mut diagnostics = vec![diagnostic];
                if let Err(log_error) = logger.diagnostic(&diagnostics[0]) {
                    diagnostics.push(log_error.into_diagnostic());
                }
                render(&DiagnosticSet::new(diagnostics), cli.diagnostic_format);
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            let diagnostic = operation_diagnostic(&cli.command, &error);
            let mut diagnostics = vec![diagnostic];
            if let Err(log_error) = logger.diagnostic(&diagnostics[0]) {
                diagnostics.push(log_error.into_diagnostic());
            }
            render(&DiagnosticSet::new(diagnostics), cli.diagnostic_format);
            ExitCode::FAILURE
        }
    }
}

fn command_context(command: &Commands) -> DiagnosticContext {
    match command {
        Commands::Pkg { action } => match action {
            PkgAction::Create { output, .. } => DiagnosticContext {
                mode: Some("pkg-create".into()),
                output: Some(output.display().to_string()),
                ..DiagnosticContext::default()
            },
            PkgAction::List { package } => DiagnosticContext {
                mode: Some("pkg-list".into()),
                output: Some(package.display().to_string()),
                ..DiagnosticContext::default()
            },
            PkgAction::Extract { directory, .. } => DiagnosticContext {
                mode: Some("pkg-extract".into()),
                output: Some(directory.display().to_string()),
                ..DiagnosticContext::default()
            },
        },
    }
}

fn operation_diagnostic(command: &Commands, error: &ToolFailure) -> Diagnostic {
    let publication_class = error.publication_class.or_else(|| {
        error
            .source
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>())
            .map(aros_common::publication_failure_class)
    });
    let (code, stage, hint) = match error.kind {
        FailureKind::Input => (
            DiagnosticCode::RomtoolInput,
            DiagnosticStage::ReleaseInput,
            "provide readable package inputs and retry",
        ),
        FailureKind::Validation => (
            DiagnosticCode::RomtoolValidation,
            DiagnosticStage::IntegrityValidation,
            "provide a complete, valid PKG version 1 container or valid ELF members",
        ),
        FailureKind::Publication => {
            let hint = match publication_class {
                Some(PublicationFailureClass::Conflict) => {
                    "the destination or CAS precondition changed; inspect it, stop concurrent writers, and retry intentionally"
                }
                Some(PublicationFailureClass::UnsafeTarget) => {
                    "use portable names below a real directory without symlink or special-file targets"
                }
                Some(PublicationFailureClass::Unsupported) => {
                    "run publication on a supported Unix filesystem with no-follow rename and directory-fsync support"
                }
                Some(PublicationFailureClass::RecoveryIncomplete) => {
                    "preserve the journal and auxiliary files, stop concurrent writers, then rerun the same command to recover"
                }
                Some(PublicationFailureClass::CommitStateUncertain) => {
                    "the complete destination was retained; do not delete it, and inspect it before retrying the identical command"
                }
                Some(PublicationFailureClass::Io) | None => {
                    "ensure the destination and its parent directory are writable, stable, and have free space"
                }
            };
            (
                DiagnosticCode::RomtoolPublication,
                DiagnosticStage::Publication,
                hint,
            )
        }
        FailureKind::RollbackIncomplete => (
            DiagnosticCode::RomtoolRollbackIncomplete,
            DiagnosticStage::Publication,
            "preserve the journal and backup files, stop concurrent writers, then rerun the same command to recover",
        ),
    };
    Diagnostic::error(code, stage, format!("{:#}", error.source))
        .with_location(SourceLocation::new(error.path.display().to_string()))
        .with_hint(hint)
        .with_context(command_context(command))
}

fn render(diagnostics: &DiagnosticSet, format: DiagnosticFormat) {
    render_diagnostics(diagnostics, format, POLICY);
}

fn render_failure(diagnostic: Diagnostic, format: DiagnosticFormat) -> ExitCode {
    render(&DiagnosticSet::single(diagnostic), format);
    ExitCode::FAILURE
}

fn create(
    output: &Path,
    files: &[PathBuf],
    basename: bool,
    allow_non_elf: bool,
    replace_if_sha256: Option<&Sha256Digest>,
) -> ToolResult<OperationOutcome> {
    let mode = if basename {
        pkg::PathMode::Basename
    } else {
        pkg::PathMode::Reference
    };

    let policy = replace_if_sha256.map_or(pkg::CreatePolicy::NoClobber, |digest| {
        pkg::CreatePolicy::ReplaceIfSha256(digest.clone())
    });
    let outcome = pkg::create(output, files, mode, allow_non_elf, &policy)
        .map_err(ToolFailure::from_create)?;
    let entries = &outcome.entries;

    let total: usize = entries.iter().map(|e| e.data.len()).sum();
    let mut report = String::new();
    let _ = writeln!(
        report,
        "📦 {} — {} module(s), {} bytes of payload",
        output.display(),
        entries.len(),
        total
    );
    for (i, entry) in entries.iter().enumerate() {
        let _ = writeln!(
            report,
            "   {:>3}. {:<28} {:>9} bytes{}",
            i + 1,
            entry.module_name(),
            entry.data.len(),
            if entry.is_elf() {
                ""
            } else {
                "  (not ELF, will be ignored)"
            }
        );
    }
    // Publication already committed. A closed pipe must not recast success as
    // an operation failure or panic through the standard print macros.
    aros_common::emit_stdout(format_args!("{report}"), false);

    Ok(OperationOutcome {
        recovery: outcome.publication.recovery(),
    })
}

fn list(package: &Path) -> ToolResult<OperationOutcome> {
    let bytes = fs::read(package)
        .with_context(|| format!("cannot read package '{}'", package.display()))
        .map_err(|error| ToolFailure::new(FailureKind::Input, package, error))?;
    let entries = pkg::parse(&bytes)
        .map_err(|error| ToolFailure::new(FailureKind::Validation, package, error))?;

    aros_common::outputln!("📦 {} — {} member(s)", package.display(), entries.len());
    for (i, entry) in entries.iter().enumerate() {
        aros_common::outputln!(
            "   {:>3}. {:<28} {:>9} bytes  {}",
            i + 1,
            entry.module_name(),
            entry.data.len(),
            if entry.is_elf() {
                "ELF"
            } else {
                "non-ELF (ignored at boot)"
            }
        );
        if entry.path != entry.module_name() {
            aros_common::outputln!("        path: {}", entry.path);
        }
    }

    Ok(OperationOutcome::default())
}

fn extract(package: &Path, directory: &Path) -> ToolResult<OperationOutcome> {
    let bytes = fs::read(package)
        .with_context(|| format!("cannot read package '{}'", package.display()))
        .map_err(|error| ToolFailure::new(FailureKind::Input, package, error))?;
    let entries = pkg::parse(&bytes)
        .map_err(|error| ToolFailure::new(FailureKind::Validation, package, error))?;

    let members: Vec<publication::NewMember<'_>> = entries
        .iter()
        .map(|entry| publication::NewMember {
            name: entry.module_name(),
            contents: &entry.data,
        })
        .collect();
    let publication = publication::publish_new_members(directory, &members).map_err(|error| {
        let class = aros_common::publication_failure_class(&error);
        let kind = if aros_common::is_rollback_incomplete(&error) {
            FailureKind::RollbackIncomplete
        } else {
            FailureKind::Publication
        };
        ToolFailure::publication(
            kind,
            directory,
            class,
            anyhow::Error::new(error).context(format!(
                "cannot atomically extract package into '{}'",
                directory.display()
            )),
        )
    })?;

    let mut report = String::new();
    for entry in &entries {
        let target = directory.join(entry.module_name());
        let _ = writeln!(
            report,
            "   {} ({} bytes)",
            target.display(),
            entry.data.len()
        );
    }
    let _ = writeln!(report, "📦 extracted {} member(s)", entries.len());
    aros_common::emit_stdout(format_args!("{report}"), false);
    Ok(OperationOutcome {
        recovery: publication.recovery(),
    })
}
