//! User-facing orchestration for AROS-NG builds, toolchains, tests, and boards.

#![warn(missing_docs)]

use aros_common::{
    render_diagnostics, requested_diagnostic_format, DiagnosticCode, DiagnosticContext,
    DiagnosticFormat, DiagnosticSet, DiagnosticStage, LogFormat, LogLevel, Logger,
};
use clap::{error::ErrorKind, Args, Parser, Subcommand};
use console::{style, Emoji};
use miette::Result;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

mod artifact;
mod boot;
mod build;
mod build_tools;
mod commands;
mod golden;
mod host_compiler;
mod observability;
mod pi;
mod repo;
mod toolchain;

static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static SPARKLES: Emoji<'_, '_> = Emoji("✨ ", "");

#[derive(Parser)]
#[command(
    name = "aros",
    author = "AROS Development Team & Fabian Schmieder (@metaneutrons)",
    version = "0.1.0",
    about = "AROS Tools v0.1: Next-Generation Build System & Tooling Engine",
    long_about = "Modern, ultra-fast, multi-platform build orchestrator and upstream sync pipeline for AROS.",
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
    /// Download and configure hermetic LLVM/LLD toolchain
    Setup {
        /// Force re-download even if already installed
        #[arg(short, long)]
        force: bool,

        /// Install the AROS cross-toolchain for this target preset
        #[arg(short, long)]
        preset: Option<String>,

        /// Install cross-toolchains for every configured target preset
        #[arg(long)]
        all: bool,

        /// Never access the network; use only verified cache/store content
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Use and verify an existing AROS-built prefix without copying it
        #[arg(long)]
        local: Option<PathBuf>,
    },

    /// Manage the host LLVM compiler used to bootstrap builds
    #[command(name = "host-compiler", visible_alias = "host-tools")]
    HostCompiler {
        #[command(subcommand)]
        command: HostCompilerCommands,
    },

    /// Build or inspect the local Rust helpers consumed by CMake
    #[command(name = "build-tools", visible_alias = "hosttools")]
    BuildTools {
        #[command(subcommand)]
        command: BuildToolsCommand,
    },

    /// Manage deterministic AROS cross-toolchain releases
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },

    /// Manage a locally configured Raspberry Pi development board
    Pi {
        #[command(subcommand)]
        command: PiCommand,
    },

    /// Build AROS for a specific target preset (pc-x86_64, rpi-aarch64, arm-raspi)
    Build {
        /// Target preset (e.g. pc-x86_64, rpi-aarch64, arm-raspi)
        #[arg(short, long, default_value = "pc-x86_64")]
        preset: String,

        /// Optional specific target to build (e.g. kernel-exec, workbench-c, boot-iso)
        #[arg(short, long)]
        target: Option<String>,

        /// Number of parallel jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Clean build directory before building
        #[arg(long)]
        clean: bool,

        /// Enable verbose build logs
        #[arg(short, long)]
        verbose: bool,

        /// Never access the network; use only a verified installed/cached toolchain
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

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

        /// Where to keep the run's evidence
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

    /// Sync upstream changes from `aros-development-team/AROS`
    Sync {
        /// Automatically regenerate `CMake` target graphs after sync
        #[arg(long, default_value_t = true)]
        transpile: bool,
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
        #[arg(short, long)]
        force: bool,
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,
    },
}

#[derive(Subcommand)]
enum ToolchainCommands {
    /// Install the exact host + target artifact selected by the lock file
    Install {
        #[arg(short, long)]
        preset: String,
        #[arg(short, long)]
        force: bool,
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,
        #[arg(long)]
        local: Option<PathBuf>,
    },
    /// List locked artifacts for the current host
    List,
    /// Verify an installed or explicitly local AROS toolchain
    Verify {
        #[arg(short, long)]
        preset: String,
        #[arg(long)]
        local: Option<PathBuf>,
    },
    /// Print the verified toolchain prefix for a target preset
    Path {
        #[arg(short, long)]
        preset: String,
        #[arg(long)]
        local: Option<PathBuf>,
    },
}

#[derive(Subcommand, Clone, Copy)]
enum BuildToolsCommand {
    /// Build the CMake configure-time Rust helpers in tools/aros-tools/target/release
    Build,
    /// Verify that all mandatory CMake configure-time Rust helpers are ready
    Check,
}

#[derive(Subcommand)]
enum PiCommand {
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

    /// Find USB CDC-ECM adapters that can be paired with an AROS Pi board
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
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Clean the board preset's build directory first
        #[arg(long)]
        clean: bool,

        /// Enable verbose CMake configure logs
        #[arg(short, long)]
        verbose: bool,

        /// Never access the network; use only a verified installed/cached toolchain
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Use an existing AROS-built cross-toolchain prefix
        #[arg(long)]
        toolchain_dir: Option<PathBuf>,

        /// Override the board profile's pinned Raspberry Pi 4 DTB for this build
        #[arg(long, value_name = "PATH")]
        dtb_path: Option<PathBuf>,

        /// Override the board profile's legacy Raspberry Pi 4 core KOBJ directory
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
        #[arg(long, value_enum, default_value_t = pi::console::ConsoleProgram::Auto)]
        program: pi::console::ConsoleProgram,

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

        /// Directory created by `aros pi sd image --apply`
        #[arg(long, value_name = "DIR")]
        artifact: PathBuf,

        /// Opaque whole-disk ID printed by `aros pi sd scan`
        #[arg(long, value_name = "SCAN_ID", value_parser = parse_opaque_scan_id)]
        device: String,

        /// Exact token printed by `aros pi sd scan --artifact ...`; without it this is a preview
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
            "expected an opaque scan ID printed by the corresponding `aros pi sd` scan command, not a device path"
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
        Commands::Pi { command } => match command {
            PiCommand::Build { board, .. } => (
                DiagnosticCode::CliBuild,
                DiagnosticStage::BuildExecution,
                "pi.build",
                Some(board.board.clone()),
                "inspect the board profile and the reported configure or build failure",
            ),
            PiCommand::Deploy { board, .. } => (
                DiagnosticCode::CliPublication,
                DiagnosticStage::Publication,
                "pi.deploy",
                Some(board.board.clone()),
                "validate the board profile, build artifact, and deployment destination before retrying",
            ),
            PiCommand::Sd { command } => {
                let target = match command {
                    SdCommand::Image { board, .. } | SdCommand::Write { board, .. } => {
                        Some(board.board.clone())
                    }
                    SdCommand::Scan { .. } | SdCommand::Unmount { .. } => None,
                };
                (
                    DiagnosticCode::CliMediaSafety,
                    DiagnosticStage::MediaSafety,
                    "pi.sd",
                    target,
                    "re-run the non-mutating scan or dry run and satisfy every reported media-safety check",
                )
            }
            PiCommand::Init { board, .. } => (
                DiagnosticCode::CliPi,
                DiagnosticStage::PiOperation,
                "pi.init",
                Some(board.clone()),
                "check the board name, configuration destination, and explicit apply mode",
            ),
            PiCommand::Doctor(selection)
            | PiCommand::Serve {
                board: selection, ..
            }
            | PiCommand::Console {
                board: selection, ..
            } => (
                DiagnosticCode::CliPi,
                DiagnosticStage::PiOperation,
                "pi",
                Some(selection.board.clone()),
                "inspect the board profile and the failed local prerequisite reported above",
            ),
            PiCommand::Scan => (
                DiagnosticCode::CliPi,
                DiagnosticStage::PiOperation,
                "pi.scan",
                None,
                "verify the local USB network interface and platform discovery tools",
            ),
        },
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
        Commands::Sync { .. } => (
            DiagnosticCode::CliNetwork,
            DiagnosticStage::NetworkTransfer,
            "sync",
            None,
            "inspect the upstream Git failure and verify repository and network state",
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
            print!("{error}");
            return ExitCode::SUCCESS;
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
    let logger = observability::install_runtime(logger, format);
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

    let repo_root = match repo::find_root().and_then(|repo_root| {
        std::env::set_current_dir(&repo_root).map_err(|error| {
            miette::miette!(
                "Could not enter AROS-NG checkout '{}': {error}",
                repo_root.display()
            )
        })?;
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

    let result = run(cli, repo_root).await;
    match result {
        Ok(()) => match logger.event(
            LogLevel::Info,
            "invocation.complete",
            "aros command completed",
            &context,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                render_diagnostics(
                    &DiagnosticSet::single(error.into_diagnostic()),
                    format,
                    observability::POLICY,
                );
                ExitCode::FAILURE
            }
        },
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

async fn run(cli: Cli, repo_root: PathBuf) -> Result<()> {
    commands::run(cli.command, &repo_root).await
}
