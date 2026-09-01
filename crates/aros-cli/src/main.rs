//! User-facing orchestration for AROS builds, toolchains, tests, and boards.

#![warn(missing_docs)]

use aros_common::{
    render_diagnostics, requested_diagnostic_format, Diagnostic, DiagnosticCode, DiagnosticContext,
    DiagnosticFormat, DiagnosticSet, DiagnosticStage, LogFormat, LogLevel, Logger,
};
use clap::{error::ErrorKind, Args, Parser, Subcommand};
use console::{style, Emoji};
use miette::Result;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

mod artifact;
mod board;
mod boot;
mod build;
mod build_tools;
mod commands;
mod golden;
mod host_compiler;
mod observability;
mod repo;
mod source;
mod toolchain;

static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static SPARKLES: Emoji<'_, '_> = Emoji("✨ ", "");

#[derive(Parser)]
#[command(
    name = "aros",
    author = "AROS Development Team & Fabian Schmieder (@metaneutrons)",
    version,
    about = "Build, verify, and deploy AROS with explicit source and toolchain inputs",
    long_about = "Upstream-compatible host tooling for reproducible AROS and AROS-NX development workflows.",
    after_help = "OBSERVABILITY:\n  --diagnostic-format human|json\n  --log-level off|error|warn|info|debug|trace\n  --log-format human|jsonl\n  --log-file PATH\n\nThe same settings are available through AROS_DIAGNOSTIC_FORMAT, AROS_LOG_LEVEL,\nAROS_LOG_FORMAT, and AROS_LOG_FILE. Logging is off by default and is written\nonly to an explicitly selected local file."
)]
struct Cli {
    #[command(flatten)]
    observability: ObservabilityArgs,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Args, Clone)]
struct ObservabilityArgs {
    /// Stable diagnostic renderer used for errors
    #[arg(long, global = true, value_enum, default_value_t = DiagnosticFormat::Human, env = "AROS_DIAGNOSTIC_FORMAT")]
    diagnostic_format: DiagnosticFormat,

    /// Opt-in local log level; requires --log-file
    #[arg(long, global = true, value_enum, default_value_t = LogLevel::Off, env = "AROS_LOG_LEVEL")]
    log_level: LogLevel,

    /// Local log representation
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Human, env = "AROS_LOG_FORMAT")]
    log_format: LogFormat,

    /// Explicit local log destination
    #[arg(long, global = true, value_name = "PATH", env = "AROS_LOG_FILE")]
    log_file: Option<PathBuf>,
}

impl ObservabilityArgs {
    fn effective_log_level(&self) -> LogLevel {
        if self.log_file.is_some() && self.log_level == LogLevel::Off {
            LogLevel::Info
        } else {
            self.log_level
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Install the declared host compiler or verified AROS cross-toolchains
    Setup {
        /// Re-download the archive cache; never overwrite an installed tree
        #[arg(short, long)]
        force: bool,

        /// Install the AROS cross-toolchain for this target preset
        #[arg(short, long, conflicts_with = "all")]
        preset: Option<String>,

        /// Install cross-toolchains for every configured target preset
        #[arg(long, conflicts_with_all = ["preset", "local"])]
        all: bool,

        /// Never access the network; use only verified cache/store content
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Use and verify an existing AROS-built prefix without copying it
        #[arg(long, requires = "preset", conflicts_with = "all")]
        local: Option<PathBuf>,
    },

    /// Manage the host LLVM compiler used to bootstrap builds
    #[command(name = "host-compiler")]
    HostCompiler {
        #[command(subcommand)]
        command: HostCompilerCommands,
    },

    /// Build or inspect the local Rust helpers consumed by CMake
    #[command(name = "build-tools")]
    BuildTools {
        #[command(subcommand)]
        command: BuildToolsCommand,
    },

    /// Manage deterministic AROS cross-toolchain releases
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },

    /// Manage locally configured physical development boards
    Board {
        #[command(subcommand)]
        command: BoardCommand,
    },

    /// Create and configure an AROS source checkout
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },

    /// Install one verified extracted native aros-tools suite atomically
    Install {
        /// Extracted archive directory containing exactly the eight programs
        #[arg(long, value_name = "DIR")]
        source_bin: PathBuf,

        /// Existing absolute installation prefix; a missing bin leaf is created
        #[arg(long, value_name = "DIR")]
        prefix: PathBuf,
    },

    /// Build AROS for a target preset (pc-x86_64, rpi-aarch64, arm-raspi, opensbi-riscv64)
    Build {
        /// Target preset (e.g. pc-x86_64, rpi-aarch64, arm-raspi, opensbi-riscv64)
        #[arg(short, long, default_value = "pc-x86_64")]
        preset: String,

        /// Optional specific target to build (e.g. kernel-exec, workbench-c, boot-iso)
        #[arg(short, long)]
        target: Option<String>,

        /// Number of parallel jobs
        #[arg(short, long, value_parser = parse_positive_usize)]
        jobs: Option<usize>,

        /// Clean build directory before building
        #[arg(long)]
        clean: bool,

        /// Enable verbose build logs
        #[arg(short, long)]
        verbose: bool,

        /// Never access the network; use only verified installed/cached inputs
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Reject every third-party source archive without an explicit SHA-256
        #[arg(long, env = "AROS_FETCH_REQUIRE_CHECKSUMS")]
        require_fetch_checksums: bool,

        /// Use an existing AROS-built cross-toolchain prefix
        #[arg(long)]
        toolchain_dir: Option<PathBuf>,
    },

    /// Clean build directory
    Clean {
        /// Target preset to clean
        #[arg(short, long)]
        preset: Option<String>,
    },

    /// Boot the target in QEMU and report how far it got
    ///
    /// The verdict comes from the serial log and the QEMU exception trace, so
    /// this fails when the boot fails. There is no interactive mode: a run
    /// nobody reads cannot assert anything, and scripts/boot/qemu-pc-x86_64.sh
    /// is there for watching one by hand.
    Test {
        /// Target preset to test
        #[arg(short, long, default_value = "pc-x86_64")]
        preset: String,

        /// Seconds to let the guest run before stopping it
        #[arg(short, long, default_value_t = 20)]
        timeout: u64,

        /// Also pass every built package as a multiboot module
        #[arg(long)]
        packages: bool,

        /// Pass this file as a multiboot module; repeatable
        #[arg(long = "module")]
        modules: Vec<PathBuf>,

        /// Root below which each invocation keeps one private evidence directory
        #[arg(long)]
        evidence: Option<PathBuf>,

        /// Guest memory in MiB
        #[arg(long, default_value_t = 512)]
        memory: u32,
    },

    /// Manage and inspect compiler cache (`ccache` / `sccache`)
    Ccache {
        /// Show cache hit/miss statistics
        #[arg(long, default_value_t = true)]
        stats: bool,

        /// Clear compilation cache
        #[arg(long)]
        clear: bool,
    },

    /// Capture or check a baseline of the transpiler's generated output
    Golden {
        #[command(subcommand)]
        action: GoldenAction,
    },

    /// Print system and toolchain information
    Info,
}

#[derive(Subcommand)]
enum SourceCommand {
    /// Clone and configure a new AROS checkout atomically
    Init {
        /// New checkout path; an existing path is never reused or overwritten
        #[arg(value_name = "PATH")]
        path: PathBuf,

        /// Canonical upstream AROS repository URL
        #[arg(
            long,
            value_name = "URL",
            default_value = source::DEFAULT_UPSTREAM_URL
        )]
        upstream: String,

        /// Optional fork URL to configure as `origin`
        #[arg(long, value_name = "URL")]
        fork: Option<String>,

        /// Optional refs/heads/NAME, refs/tags/NAME, or exact commit OID
        #[arg(long = "ref", value_name = "REF")]
        source_ref: Option<String>,
    },

    /// Safely fast-forward a clean branch from a reviewed upstream remote
    Sync {
        /// Expected URL of the `upstream` remote
        #[arg(
            long,
            value_name = "URL",
            env = "AROS_UPSTREAM_URL",
            default_value = source::DEFAULT_UPSTREAM_URL
        )]
        upstream: String,

        /// Exact upstream branch name under refs/heads/
        #[arg(long = "ref", value_name = "BRANCH", default_value = "master")]
        upstream_ref: String,

        /// Skip standalone-candidate target-graph validation
        #[arg(long = "no-transpile", action = clap::ArgAction::SetFalse)]
        transpile: bool,
    },
}

#[derive(Subcommand)]
enum GoldenAction {
    /// Run the transpiler twice and store its output as the baseline
    Capture {
        /// Preset to capture; repeatable. Default: every configured preset
        #[arg(long = "preset")]
        presets: Vec<String>,
    },

    /// Run the transpiler and compare its output against the baseline
    Verify {
        /// Preset to check; repeatable. Default: every configured preset
        #[arg(long = "preset")]
        presets: Vec<String>,

        /// Replace the baseline with this run instead of reporting differences
        #[arg(long)]
        update: bool,
    },
}

#[derive(Subcommand)]
enum HostCompilerCommands {
    /// Download and install the pinned host LLVM tools
    Install {
        /// Re-download the archive cache; never overwrite an installed tree
        #[arg(short, long)]
        force: bool,

        /// Never access the network; use only verified cached content
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,
    },
}

#[derive(Subcommand)]
enum ToolchainCommands {
    /// Install the exact host + target artifact selected by the lock file
    Install {
        /// Target profile whose locked artifact should be installed
        #[arg(short, long)]
        preset: String,

        /// Re-download the archive cache; never overwrite an installed tree
        #[arg(short, long)]
        force: bool,

        /// Never access the network; use only verified cached content
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Verify and use an existing AROS-built prefix without copying it
        #[arg(long)]
        local: Option<PathBuf>,
    },
    /// List locked artifacts for the current host
    List,
    /// Verify an installed or explicitly local AROS toolchain
    Verify {
        /// Target profile whose locked contract should be verified
        #[arg(short, long)]
        preset: String,

        /// Verify this existing AROS-built prefix instead of the installed one
        #[arg(long)]
        local: Option<PathBuf>,
    },
    /// Print the verified toolchain prefix for a target preset
    Path {
        /// Target profile whose verified prefix should be printed
        #[arg(short, long)]
        preset: String,

        /// Print this verified AROS-built prefix instead of the installed one
        #[arg(long)]
        local: Option<PathBuf>,
    },
}

#[derive(Subcommand, Clone, Copy)]
enum BuildToolsCommand {
    /// Build the Rust helpers from the workspace selected by AROS_TOOLS_SOURCE_DIR
    Build,
    /// Verify that all mandatory CMake configure-time Rust helpers are ready
    Check,
}

#[derive(Subcommand)]
enum BoardCommand {
    /// Print or explicitly create a new local USB-ECM board-profile template
    Init {
        /// Local profile name to create
        #[arg(long)]
        board: String,

        /// Board configuration file; defaults to ~/.config/aros/boards.toml
        #[arg(long, value_name = "PATH", env = "AROS_BOARDS_FILE")]
        config: Option<PathBuf>,

        /// Create the new file. Without this flag the template is only shown.
        #[arg(long)]
        apply: bool,
    },

    /// Find USB CDC-ECM adapters that can be paired with a board profile
    Scan,

    /// Check a local board profile and its non-mutating prerequisites
    Doctor(BoardSelection),

    /// Build using the board profile's CMake preset and locked toolchain profile
    Build {
        #[command(flatten)]
        board: BoardSelection,

        /// Optional specific CMake target to build
        #[arg(short, long)]
        target: Option<String>,

        /// Number of parallel build jobs
        #[arg(short, long, value_parser = parse_positive_usize)]
        jobs: Option<usize>,

        /// Clean the board preset's build directory first
        #[arg(long)]
        clean: bool,

        /// Enable verbose CMake configure logs
        #[arg(short, long)]
        verbose: bool,

        /// Never access the network; use only verified installed/cached inputs
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Reject every third-party source archive without an explicit SHA-256
        #[arg(long, env = "AROS_FETCH_REQUIRE_CHECKSUMS")]
        require_fetch_checksums: bool,

        /// Use an existing AROS-built cross-toolchain prefix
        #[arg(long)]
        toolchain_dir: Option<PathBuf>,

        /// Override a Raspberry Pi board profile's exact model DTB for this build
        #[arg(long, value_name = "PATH")]
        dtb_path: Option<PathBuf>,

        /// Override the board profile's architecture-correct legacy core KOBJ directory
        #[arg(long, value_name = "DIR")]
        core_kobj_dir: Option<PathBuf>,
    },

    /// Stage the built boot bundle into a local TFTP root (dry-run by default)
    Deploy {
        #[command(flatten)]
        board: BoardSelection,

        /// Override the artifact directory for this deployment
        #[arg(long, value_name = "DIR")]
        artifact_dir: Option<PathBuf>,

        /// Publish the staged bundle. Without this flag deploy is a dry run.
        #[arg(long)]
        apply: bool,

        /// Explicitly request dry-run output (the default unless --apply is given)
        #[arg(long)]
        dry_run: bool,
    },

    /// Run restricted DHCP and read-only TFTP for one verified board profile
    Serve {
        #[command(flatten)]
        board: BoardSelection,

        /// Resolve identity, address and deployment without opening sockets
        #[arg(long)]
        dry_run: bool,
    },

    /// Create a verified SD-card image from an external, pinned boot bundle
    Sd {
        #[command(subcommand)]
        command: SdCommand,
    },

    /// Open an external serial terminal for the board; no UART driver is embedded
    Console {
        #[command(flatten)]
        board: BoardSelection,

        /// Serial terminal implementation to invoke
        #[arg(long, value_enum, default_value_t = board::console::ConsoleProgram::Auto)]
        program: board::console::ConsoleProgram,

        /// Override the configured serial device for this invocation
        #[arg(long, value_name = "PATH")]
        device: Option<PathBuf>,

        /// Override the configured serial baud rate for this invocation
        #[arg(long)]
        baud: Option<u32>,

        /// Print the external terminal command without starting it
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum SdCommand {
    /// Validate an external boot bundle and create a raw MBR/FAT32 image
    Image {
        #[command(flatten)]
        board: BoardSelection,

        /// Directory containing boot-bundle.toml and all hash-pinned inputs
        #[arg(long, value_name = "DIR")]
        boot_bundle: PathBuf,

        /// New output artifact directory; an existing directory is refused
        #[arg(long, value_name = "DIR")]
        output: PathBuf,

        /// Create the image after validation. Without this flag it is a dry run.
        #[arg(long)]
        apply: bool,

        /// Explicitly request dry-run output (the default unless --apply is given)
        #[arg(long)]
        dry_run: bool,
    },

    /// List only safe, unmounted removable SD-card targets
    Scan {
        /// Optionally verify an image artifact and print its write token for each target
        #[arg(long, value_name = "DIR")]
        artifact: Option<PathBuf>,
    },

    /// List or explicitly unmount one mounted removable whole-disk target
    Unmount {
        /// Opaque whole-disk ID printed by this command; raw device paths are rejected
        #[arg(long, value_name = "SCAN_ID", value_parser = parse_opaque_scan_id)]
        device: Option<String>,

        /// Unmount the explicitly selected disk; without this flag only show a preview
        #[arg(long, requires = "device", conflicts_with = "dry_run")]
        apply: bool,

        /// Explicitly request non-mutating preview output
        #[arg(long)]
        dry_run: bool,
    },

    /// Write one verified SD image after an explicit disk/token confirmation
    Write {
        #[command(flatten)]
        board: BoardSelection,

        /// Directory created by `aros board sd image --apply`
        #[arg(long, value_name = "DIR")]
        artifact: PathBuf,

        /// Opaque whole-disk ID printed by `aros board sd scan`
        #[arg(long, value_name = "SCAN_ID", value_parser = parse_opaque_scan_id)]
        device: String,

        /// Exact token printed by `aros board sd scan --artifact ...`; without it this is a preview
        #[arg(long, value_name = "TOKEN")]
        confirm: Option<String>,

        /// Validate the selected disk and token plan without writing it
        #[arg(long)]
        dry_run: bool,
    },
}

fn parse_opaque_scan_id(value: &str) -> std::result::Result<String, String> {
    if value.is_empty() || value.trim() != value || value.contains('/') || value.contains('\\') {
        return Err(
            "expected an opaque scan ID printed by the corresponding `aros board sd` scan command, not a device path"
                .to_string(),
        );
    }
    Ok(value.to_string())
}

#[derive(Args, Clone)]
struct BoardSelection {
    /// Local board profile name from ~/.config/aros/boards.toml
    #[arg(long)]
    board: String,

    /// Board configuration file; overrides AROS_BOARDS_FILE and the default path
    #[arg(long, value_name = "PATH", env = "AROS_BOARDS_FILE")]
    config: Option<PathBuf>,
}

/// Repository context needed before a command may run.
///
/// Keeping this policy beside the command model prevents a new global command
/// from accidentally inheriting checkout discovery merely because most build
/// commands need it.
fn parse_positive_usize(value: &str) -> std::result::Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("'{value}' is not a valid positive integer"))?;
    if parsed == 0 {
        return Err("parallel job count must be greater than zero".to_owned());
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepositoryRequirement {
    /// The command is independent of an AROS source checkout.
    Global,
    /// Use a checkout when one is discoverable, but remain useful without one.
    Optional,
    /// Refuse to run until an AROS source checkout has been discovered.
    Required,
}

impl Commands {
    const fn repository_requirement(&self) -> RepositoryRequirement {
        match self {
            Self::Source { command } => match command {
                SourceCommand::Init { .. } => RepositoryRequirement::Global,
                SourceCommand::Sync { .. } => RepositoryRequirement::Required,
            },
            Self::Ccache { .. } | Self::Install { .. } => RepositoryRequirement::Global,
            Self::Info | Self::BuildTools { .. } => RepositoryRequirement::Optional,
            Self::Board { command } => match command {
                BoardCommand::Init { .. }
                | BoardCommand::Scan
                | BoardCommand::Serve { .. }
                | BoardCommand::Sd { .. }
                | BoardCommand::Console { .. } => RepositoryRequirement::Global,
                BoardCommand::Doctor(_)
                | BoardCommand::Build { .. }
                | BoardCommand::Deploy { .. } => RepositoryRequirement::Required,
            },
            Self::Setup { .. }
            | Self::HostCompiler { .. }
            | Self::Toolchain { .. }
            | Self::Build { .. }
            | Self::Clean { .. }
            | Self::Test { .. }
            | Self::Golden { .. } => RepositoryRequirement::Required,
        }
    }

    const fn commits_on_success(&self) -> bool {
        matches!(self, Self::Install { .. })
    }
}

fn command_boundary(command: &Commands) -> (observability::ErrorBoundary, DiagnosticContext) {
    let (code, stage, mode, target, hint) = match command {
        Commands::Setup { preset, .. } => (
            DiagnosticCode::CliToolchain,
            DiagnosticStage::ToolResolution,
            "setup",
            preset.clone(),
            "verify the selected profile, toolchain lock, network policy, and local cache",
        ),
        Commands::HostCompiler { .. } => (
            DiagnosticCode::CliToolResolution,
            DiagnosticStage::ToolResolution,
            "host-compiler",
            None,
            "install the declared host compiler or verify the configured offline cache",
        ),
        Commands::BuildTools { .. } => (
            DiagnosticCode::CliToolResolution,
            DiagnosticStage::ToolResolution,
            "build-tools",
            None,
            "inspect the reported helper and Cargo failure, then rebuild the required build tools",
        ),
        Commands::Toolchain { command } => {
            let target = match command {
                ToolchainCommands::Install { preset, .. }
                | ToolchainCommands::Verify { preset, .. }
                | ToolchainCommands::Path { preset, .. } => Some(preset.clone()),
                ToolchainCommands::List => None,
            };
            (
                DiagnosticCode::CliToolchain,
                DiagnosticStage::ToolResolution,
                "toolchain",
                target,
                "verify the toolchain lock, selected host/profile artifact, cache, and installation prefix",
            )
        }
        Commands::Board { command } => match command {
            BoardCommand::Build { board, .. } => (
                DiagnosticCode::CliBuild,
                DiagnosticStage::BuildExecution,
                "board.build",
                Some(board.board.clone()),
                "inspect the board profile and the reported configure or build failure",
            ),
            BoardCommand::Deploy { board, .. } => (
                DiagnosticCode::CliPublication,
                DiagnosticStage::Publication,
                "board.deploy",
                Some(board.board.clone()),
                "validate the board profile, build artifact, and deployment destination before retrying",
            ),
            BoardCommand::Sd { command } => {
                let target = match command {
                    SdCommand::Image { board, .. } | SdCommand::Write { board, .. } => {
                        Some(board.board.clone())
                    }
                    SdCommand::Scan { .. } | SdCommand::Unmount { .. } => None,
                };
                (
                    DiagnosticCode::CliMediaSafety,
                    DiagnosticStage::MediaSafety,
                    "board.sd",
                    target,
                    "re-run the non-mutating scan or dry run and satisfy every reported media-safety check",
                )
            }
            BoardCommand::Init { board, .. } => (
                DiagnosticCode::CliBoard,
                DiagnosticStage::BoardOperation,
                "board.init",
                Some(board.clone()),
                "check the board name, configuration destination, and explicit apply mode",
            ),
            BoardCommand::Doctor(selection)
            | BoardCommand::Serve {
                board: selection, ..
            }
            | BoardCommand::Console {
                board: selection, ..
            } => (
                DiagnosticCode::CliBoard,
                DiagnosticStage::BoardOperation,
                "board",
                Some(selection.board.clone()),
                "inspect the board profile and the failed local prerequisite reported above",
            ),
            BoardCommand::Scan => (
                DiagnosticCode::CliBoard,
                DiagnosticStage::BoardOperation,
                "board.scan",
                None,
                "verify the local USB network interface and platform discovery tools",
            ),
        },
        Commands::Source { command } => match command {
            SourceCommand::Init { path, .. } => (
                DiagnosticCode::CliSourceInput,
                DiagnosticStage::Configuration,
                "source.init",
                Some(path.display().to_string()),
                "verify Git, the source URLs and ref, and select a new destination path",
            ),
            SourceCommand::Sync { upstream_ref, .. } => (
                DiagnosticCode::CliSourceState,
                DiagnosticStage::RepositoryDiscovery,
                "source.sync",
                Some(upstream_ref.clone()),
                "inspect the stable source diagnostic code, reviewed upstream, branch state, and candidate-validation failure",
            ),
        },
        Commands::Install { source_bin, prefix } => (
            DiagnosticCode::CliPublication,
            DiagnosticStage::Publication,
            "install",
            Some(format!("{} -> {}", source_bin.display(), prefix.display())),
            "preserve an indeterminate journal; otherwise remove an existing suite through the documented workflow and retry",
        ),
        Commands::Build { preset, .. } => (
            DiagnosticCode::CliBuild,
            DiagnosticStage::BuildExecution,
            "build",
            Some(preset.clone()),
            "inspect the preserved configure/build output and retry the exact reported target",
        ),
        Commands::Clean { preset } => (
            DiagnosticCode::CliPublication,
            DiagnosticStage::Publication,
            "clean",
            preset.clone(),
            "verify that the selected build directory belongs to the intended AROS preset",
        ),
        Commands::Test { preset, .. } => (
            DiagnosticCode::CliBoot,
            DiagnosticStage::BootValidation,
            "test",
            Some(preset.clone()),
            "inspect the retained boot evidence and the first reported serial or QEMU failure",
        ),
        Commands::Ccache { .. } => (
            DiagnosticCode::CliToolResolution,
            DiagnosticStage::ToolResolution,
            "ccache",
            None,
            "install ccache or sccache and verify that the selected executable can be started",
        ),
        Commands::Golden { .. } => (
            DiagnosticCode::CliPublication,
            DiagnosticStage::Publication,
            "golden",
            None,
            "inspect the named profile and generated product; update only after reviewing an intentional change",
        ),
        Commands::Info => (
            DiagnosticCode::CliConfiguration,
            DiagnosticStage::Configuration,
            "info",
            None,
            "repair the reported workspace or toolchain configuration",
        ),
    };
    (
        observability::ErrorBoundary { code, stage, hint },
        DiagnosticContext {
            mode: Some(mode.into()),
            target,
            ..DiagnosticContext::default()
        },
    )
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().collect();
    let requested_format = requested_diagnostic_format(&arguments, "AROS_DIAGNOSTIC_FORMAT");
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
                    render_diagnostics(
                        &DiagnosticSet::single(
                            Diagnostic::error(
                                DiagnosticCode::CliObservability,
                                DiagnosticStage::Observability,
                                format!("could not write command help: {output_error}"),
                            )
                            .with_hint("check the stdout destination and retry"),
                        ),
                        requested_format,
                        observability::POLICY,
                    );
                    ExitCode::FAILURE
                }
            };
        }
        Err(error) => {
            render_diagnostics(
                &DiagnosticSet::single(observability::clap_diagnostic(&error)),
                requested_format,
                observability::POLICY,
            );
            return ExitCode::FAILURE;
        }
    };
    let format = cli.observability.diagnostic_format;
    let logger = match Logger::open(
        cli.observability.effective_log_level(),
        cli.observability.log_format,
        cli.observability.log_file.clone(),
        "aros",
        observability::POLICY,
    ) {
        Ok(logger) => logger,
        Err(error) => {
            render_diagnostics(
                &DiagnosticSet::single(error.into_diagnostic()),
                format,
                observability::POLICY,
            );
            return ExitCode::FAILURE;
        }
    };
    let logger = match observability::install_runtime(logger, format) {
        Ok(logger) => logger,
        Err(error) => {
            render_diagnostics(
                &DiagnosticSet::single(
                    Diagnostic::error(
                        DiagnosticCode::CliInternal,
                        DiagnosticStage::Internal,
                        error,
                    )
                    .with_hint(
                        "restart the aros process; process-wide runtime state was inconsistent",
                    ),
                ),
                format,
                observability::POLICY,
            );
            return ExitCode::FAILURE;
        }
    };
    let (boundary, context) = command_boundary(&cli.command);
    if let Err(error) = logger.event(
        LogLevel::Info,
        "invocation.start",
        "aros command started",
        &context,
    ) {
        render_diagnostics(
            &DiagnosticSet::single(error.into_diagnostic()),
            format,
            observability::POLICY,
        );
        return ExitCode::FAILURE;
    }

    let repo_root =
        match resolve_repository(cli.command.repository_requirement()).and_then(|repo_root| {
            if let Some(path) = &repo_root {
                std::env::set_current_dir(path).map_err(|error| {
                    miette::miette!(
                        "Could not enter AROS checkout '{}': {error}",
                        path.display()
                    )
                })?;
            }
            Ok(repo_root)
        }) {
            Ok(repo_root) => repo_root,
            Err(error) => {
                let diagnostic = observability::report_diagnostic(
                    &error,
                    observability::ErrorBoundary::REPOSITORY,
                    context,
                );
                let mut diagnostics = vec![diagnostic.clone()];
                if let Err(log_error) = logger.diagnostic(&diagnostic) {
                    diagnostics.push(log_error.into_diagnostic());
                }
                render_diagnostics(
                    &observability::set(diagnostics),
                    format,
                    observability::POLICY,
                );
                return ExitCode::FAILURE;
            }
        };

    let commits_on_success = cli.command.commits_on_success();
    let result = run(cli, repo_root).await;
    match result {
        Ok(()) => {
            if let Some(diagnostic) = aros_common::take_stdout_failure_diagnostic(
                DiagnosticCode::CliObservability,
                DiagnosticStage::Observability,
            ) {
                let mut diagnostics = vec![diagnostic];
                if let Err(log_error) = logger.diagnostic(&diagnostics[0]) {
                    diagnostics.push(log_error.into_diagnostic());
                }
                render_diagnostics(
                    &DiagnosticSet::new(diagnostics),
                    format,
                    observability::POLICY,
                );
                return ExitCode::FAILURE;
            }
            match logger.event(
                LogLevel::Info,
                "invocation.complete",
                "aros command completed",
                &context,
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    let mut diagnostic = error.into_diagnostic();
                    if commits_on_success {
                        let mut committed = context.clone();
                        committed.commit_state = Some(aros_common::CommitState::Committed);
                        if let Some(error_context) = diagnostic.context.take() {
                            committed.log_path = error_context.log_path;
                        }
                        diagnostic.context = Some(committed);
                    }
                    render_diagnostics(
                        &DiagnosticSet::single(diagnostic),
                        format,
                        observability::POLICY,
                    );
                    ExitCode::FAILURE
                }
            }
        }
        Err(error) => {
            let diagnostic = observability::report_diagnostic(&error, boundary, context);
            let mut diagnostics = vec![diagnostic.clone()];
            if let Err(log_error) = logger.diagnostic(&diagnostic) {
                diagnostics.push(log_error.into_diagnostic());
            }
            render_diagnostics(
                &observability::set(diagnostics),
                format,
                observability::POLICY,
            );
            ExitCode::FAILURE
        }
    }
}

fn resolve_repository(requirement: RepositoryRequirement) -> Result<Option<PathBuf>> {
    match requirement {
        RepositoryRequirement::Global => Ok(None),
        RepositoryRequirement::Optional => repo::find_root_optional(),
        RepositoryRequirement::Required => repo::find_root().map(Some),
    }
}

async fn run(cli: Cli, repo_root: Option<PathBuf>) -> Result<()> {
    commands::run(cli.command, repo_root.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::{Cli, Parser, RepositoryRequirement};
    use clap::error::ErrorKind;

    fn requirement(arguments: &[&str]) -> RepositoryRequirement {
        Cli::try_parse_from(arguments)
            .expect("valid command line")
            .command
            .repository_requirement()
    }

    #[test]
    fn repository_policy_is_explicit_for_each_command_class() {
        assert_eq!(
            requirement(&["aros", "source", "init", "AROS"]),
            RepositoryRequirement::Global
        );
        assert_eq!(
            requirement(&["aros", "board", "scan"]),
            RepositoryRequirement::Global
        );
        assert_eq!(
            requirement(&["aros", "info"]),
            RepositoryRequirement::Optional
        );
        assert_eq!(
            requirement(&["aros", "clean"]),
            RepositoryRequirement::Required
        );
        assert_eq!(
            requirement(&["aros", "source", "sync"]),
            RepositoryRequirement::Required
        );
    }

    fn parse_error(arguments: &[&str]) -> ErrorKind {
        match Cli::try_parse_from(arguments) {
            Ok(_) => panic!("command line unexpectedly parsed: {arguments:?}"),
            Err(error) => error.kind(),
        }
    }

    #[test]
    fn setup_modes_are_mutually_exclusive_at_the_cli_boundary() {
        assert_eq!(
            parse_error(&["aros", "setup", "--all", "--preset", "pc-x86_64"]),
            ErrorKind::ArgumentConflict
        );
        assert_eq!(
            parse_error(&["aros", "setup", "--all", "--local", "/opt/aros"]),
            ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn setup_local_override_requires_one_preset() {
        assert_eq!(
            parse_error(&["aros", "setup", "--local", "/opt/aros"]),
            ErrorKind::MissingRequiredArgument
        );
        assert!(Cli::try_parse_from([
            "aros",
            "setup",
            "--preset",
            "pc-x86_64",
            "--local",
            "/opt/aros",
        ])
        .is_ok());
    }

    #[test]
    fn build_job_limits_reject_zero_at_the_cli_boundary() {
        assert_eq!(
            parse_error(&["aros", "build", "--jobs", "0"]),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse_error(&["aros", "board", "build", "--jobs", "0"]),
            ErrorKind::ValueValidation
        );
        assert!(Cli::try_parse_from(["aros", "build", "--jobs", "1"]).is_ok());
    }
}
