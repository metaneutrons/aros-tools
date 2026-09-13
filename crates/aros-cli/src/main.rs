//! User-facing orchestration for AROS builds, toolchains, tests, and boards.

#![warn(missing_docs)]

use aros_board::config::{BoardModel, Transport};
use aros_common::{
    effective_log_level, render_diagnostics, requested_diagnostic_format, Diagnostic,
    DiagnosticCode, DiagnosticContext, DiagnosticFormat, DiagnosticSet, DiagnosticStage, LogFormat,
    LogLevel, Logger,
};
use clap::{
    error::ErrorKind, parser::ValueSource, Args, CommandFactory, FromArgMatches, Parser,
    Subcommand, ValueEnum,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

mod artifact;
mod board;
mod boot;
mod build;
mod build_cache;
#[cfg(test)]
mod build_cache_tests;
mod build_tools;
mod cache;
/// Parser model for resource-oriented cache commands.
pub mod cache_command;
mod cli_contract;
/// Source-derived renderer for reviewed CLI-contract snapshots.
#[cfg(test)]
pub mod cli_contract_render;
#[cfg(test)]
mod cli_contract_sections;
mod commands;
mod completion_model;
mod golden;
mod host_compiler;
mod observability;
mod repo;
mod source;
mod toolchain;
mod toolchain_build;
mod toolchain_fetch_bridge;
mod toolchain_lifecycle;
mod toolchain_management;
mod toolchain_plan;
mod toolchain_producer;
mod toolchain_selection;

use build_cache::BuildCompilerCache;
use cache_command::{
    CacheCommand, CacheCompilerBackend, CacheCompilerCommand, CacheSourceSelector,
    CacheSourcesCommand,
};
use cli_contract::{
    parse_opaque_scan_id, parse_positive_usize, resolve_repository, BoardProfileSelection,
    GoldenAction,
};
use completion_model::CompletionShell;

#[derive(Parser)]
#[command(
    name = "aros",
    author = "AROS Development Team & Fabian Schmieder (@metaneutrons)",
    version,
    about = "Build, verify, and deploy AROS with explicit source and toolchain inputs",
    long_about = "Upstream-compatible host tooling for reproducible AROS and AROS-NX development workflows.",
    after_help = "OBSERVABILITY:\n  --diagnostic-format human|json\n  --log-level off|error|warn|info|debug|trace\n  --log-format human|jsonl\n  --log-file PATH\n\nThe same settings are available through AROS_DIAGNOSTIC_FORMAT, AROS_LOG_LEVEL,\nAROS_LOG_FORMAT, and AROS_LOG_FILE. Logging is off by default. A selected file\nwithout a selected level uses info; explicit off creates no sink, and a non-off\nlevel requires a local file."
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

#[derive(Subcommand)]
enum Commands {
    /// Install the declared host compiler or verified AROS cross-toolchains
    Setup {
        /// Re-download the archive cache; never overwrite an installed tree
        #[arg(short, long, conflicts_with_all = ["local", "offline"])]
        force: bool,

        /// Install the AROS cross-toolchain for this target preset
        #[arg(short, long, conflicts_with = "all")]
        preset: Option<String>,

        /// Install cross-toolchains for every configured target preset
        #[arg(long, conflicts_with_all = ["preset", "local"])]
        all: bool,

        /// Never access the network; use only verified cache/store content
        #[arg(long, env = "AROS_OFFLINE", conflicts_with = "force")]
        offline: bool,

        /// Use and verify an existing AROS-built prefix without copying it
        #[arg(long, requires = "preset", conflicts_with_all = ["all", "force"])]
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

        /// Compiler-cache policy; offline auto disables caching unless a later verified local policy is available
        #[arg(long, value_enum, default_value = "auto")]
        compiler_cache: BuildCompilerCache,

        /// Never access the network; use only verified installed/cached inputs
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Reject every third-party source archive without an explicit SHA-256
        #[arg(long, env = "AROS_FETCH_REQUIRE_CHECKSUMS")]
        require_fetch_checksums: bool,

        /// Use an existing AROS-built cross-toolchain prefix
        #[arg(long)]
        toolchain_dir: Option<PathBuf>,

        /// Build unoptimised with debug information
        #[arg(long)]
        debug: bool,

        /// Use the CMake engine in this directory instead of the embedded one
        ///
        /// Only ever honoured when given here. An engine that happens to sit in
        /// the checkout is not preferred on its own.
        #[arg(long, value_name = "DIR")]
        engine_dir: Option<PathBuf>,
    },

    /// Remove one explicitly selected build scope
    Clean {
        /// Target preset to clean
        #[arg(short, long, required_unless_present = "all", conflicts_with = "all")]
        preset: Option<String>,

        /// Remove the checkout's complete build directory
        #[arg(long, conflicts_with = "preset")]
        all: bool,

        /// Print the selected build directory without removing it
        #[arg(long)]
        dry_run: bool,
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
        #[arg(short, long, default_value_t = 20, value_parser = clap::value_parser!(u64).range(1..))]
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
        #[arg(long, default_value_t = 512, value_parser = clap::value_parser!(u32).range(1..))]
        memory: u32,
    },

    /// Inspect AROS-managed cache roots and compiler-cache backends
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },

    /// Query statistics through the legacy compiler-cache frontend
    Ccache,

    /// Capture or check a baseline of the transpiler's generated output
    Golden {
        #[command(subcommand)]
        action: GoldenAction,
    },

    /// Generate a shell completion script from the current public command model
    Completions {
        /// Shell syntax to generate
        #[arg(value_enum)]
        shell: CompletionShell,
    },

    /// Print observed system and toolchain information
    Info {
        /// Result representation on stdout, independent of diagnostic format
        #[arg(long, value_enum, default_value = "human")]
        format: toolchain_management::ResultFormat,
    },
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
        #[arg(long = "branch", value_name = "BRANCH", default_value = "master")]
        upstream_branch: String,

        /// Skip standalone-candidate target-graph validation
        #[arg(long = "no-transpile", action = clap::ArgAction::SetFalse)]
        transpile: bool,
    },
}

#[derive(Subcommand)]
enum HostCompilerCommands {
    /// Download and install the pinned host LLVM tools
    Install {
        /// Re-download the archive cache; never overwrite an installed tree
        #[arg(short, long, conflicts_with = "offline")]
        force: bool,

        /// Never access the network; use only verified cached content
        #[arg(long, env = "AROS_OFFLINE", conflicts_with = "force")]
        offline: bool,
    },
}

#[derive(Subcommand)]
enum ToolchainCommands {
    /// Inspect explicit producer inputs without building (experimental)
    Plan(toolchain_plan::PlanArgs),
    /// Build one local native toolchain candidate
    Build(toolchain_build::BuildArgs),
    /// Run explicit native producer stages without a legacy adapter
    Producer(toolchain_producer::ProducerArgs),
    /// Internal verified bridge used only by the native MetaMake lifecycle.
    #[command(name = "__metamake-fetch", hide = true)]
    MetaMakeFetch(toolchain_fetch_bridge::MetaMakeFetchArgs),
    /// Install the exact host + target artifact selected by the lock file
    Install {
        /// Target profile whose locked artifact should be installed
        #[arg(short, long)]
        preset: String,

        /// Re-download the archive cache; never overwrite an installed tree
        #[arg(short, long, conflicts_with_all = ["local", "offline"])]
        force: bool,

        /// Never access the network; use only verified cached content
        #[arg(long, env = "AROS_OFFLINE", conflicts_with = "force")]
        offline: bool,

        /// Verify and use an existing AROS-built prefix without copying it
        #[arg(long, conflicts_with = "force")]
        local: Option<PathBuf>,
    },
    /// List lock-selected artifacts for the current host without downloading them
    List {
        /// Result representation on stdout, independent of diagnostic format
        #[arg(long, value_enum, default_value = "human")]
        format: toolchain_management::ResultFormat,
    },
    /// Inspect installed cross-toolchain envelopes without downloading or executing them
    Inventory(toolchain_management::InventoryArgs),
    /// Preview or import one verified local toolchain into the managed store
    Import(toolchain_management::ImportArgs),
    /// Preview or register one verified external toolchain prefix without copying it
    Register(toolchain_management::RegisterArgs),
    /// Preview or atomically select one complete released lock for this checkout
    Select(toolchain_selection::SelectArgs),
    /// Preview or remove one exact owned local-toolchain import
    Remove(toolchain_lifecycle::RemoveArgs),
    /// Preview or reclaim unprotected owned local-toolchain imports
    Gc(toolchain_lifecycle::GcArgs),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum BoardInitModel {
    #[value(name = "rpi3")]
    Rpi3,
    #[value(name = "rpi4")]
    Rpi4,
    #[value(name = "rpi5")]
    Rpi5,
    #[value(name = "milk-v-titan")]
    MilkVTitan,
}

impl From<BoardInitModel> for BoardModel {
    fn from(value: BoardInitModel) -> Self {
        match value {
            BoardInitModel::Rpi3 => Self::Rpi3,
            BoardInitModel::Rpi4 => Self::Rpi4,
            BoardInitModel::Rpi5 => Self::Rpi5,
            BoardInitModel::MilkVTitan => Self::MilkVTitan,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum BoardInitTransport {
    #[value(name = "native-tftp")]
    NativeTftp,
    #[value(name = "uboot-usb-ecm")]
    UbootUsbEcm,
    #[value(name = "uefi-esp")]
    UefiEsp,
}

impl From<BoardInitTransport> for Transport {
    fn from(value: BoardInitTransport) -> Self {
        match value {
            BoardInitTransport::NativeTftp => Self::NativeTftp,
            BoardInitTransport::UbootUsbEcm => Self::UbootUsbEcm,
            BoardInitTransport::UefiEsp => Self::UefiEsp,
        }
    }
}

#[derive(Subcommand)]
enum BoardCommand {
    /// Print or explicitly create a typed local board-profile template
    Init {
        /// Local profile name to create; this does not select hardware
        #[arg(long)]
        profile: String,

        /// Required physical hardware model for the generated profile
        #[arg(long, value_enum)]
        model: BoardInitModel,

        /// Reviewed boot transport; defaults to the model's conservative transport
        #[arg(long, value_enum)]
        transport: Option<BoardInitTransport>,

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
    Doctor(BoardProfileSelection),

    /// Build using the board profile's CMake preset and locked toolchain profile
    Build {
        #[command(flatten)]
        board: BoardProfileSelection,

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

        /// Compiler-cache policy; offline auto disables caching unless a later verified local policy is available
        #[arg(long, value_enum, default_value = "auto")]
        compiler_cache: BuildCompilerCache,

        /// Never access the network; use only verified installed/cached inputs
        #[arg(long, env = "AROS_OFFLINE")]
        offline: bool,

        /// Reject every third-party source archive without an explicit SHA-256
        #[arg(long, env = "AROS_FETCH_REQUIRE_CHECKSUMS")]
        require_fetch_checksums: bool,

        /// Use an existing AROS-built cross-toolchain prefix
        #[arg(long)]
        toolchain_dir: Option<PathBuf>,

        /// Build unoptimised with debug information
        #[arg(long)]
        debug: bool,

        /// Use the CMake engine in this directory instead of the embedded one
        ///
        /// Only ever honoured when given here. An engine that happens to sit in
        /// the checkout is not preferred on its own.
        #[arg(long, value_name = "DIR")]
        engine_dir: Option<PathBuf>,

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
        board: BoardProfileSelection,

        /// Override the artifact directory for this deployment
        #[arg(long, value_name = "DIR")]
        artifact_dir: Option<PathBuf>,

        /// Publish the staged bundle. Without this flag deploy is a dry run.
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,

        /// Explicitly request dry-run output (the default unless --apply is given)
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,
    },

    /// Run restricted DHCP and read-only TFTP for one verified board profile
    Serve {
        #[command(flatten)]
        board: BoardProfileSelection,

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
        board: BoardProfileSelection,

        /// Serial terminal implementation to invoke
        #[arg(long, value_enum, default_value_t = board::console::ConsoleProgram::Auto)]
        program: board::console::ConsoleProgram,

        /// Override the configured serial device for this invocation
        #[arg(long, value_name = "PATH")]
        device: Option<PathBuf>,

        /// Override the configured serial baud rate for this invocation
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
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
        board: BoardProfileSelection,

        /// Directory containing boot-bundle.toml and all hash-pinned inputs
        #[arg(long, value_name = "DIR")]
        boot_bundle: PathBuf,

        /// New output artifact directory; an existing directory is refused
        #[arg(long, value_name = "DIR")]
        output: PathBuf,

        /// Create the image after validation. Without this flag it is a dry run.
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,

        /// Explicitly request dry-run output (the default unless --apply is given)
        #[arg(long, conflicts_with = "apply")]
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
        board: BoardProfileSelection,

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

fn command_boundary(command: &Commands) -> (observability::ErrorBoundary, DiagnosticContext) {
    let (code, stage, mode, target, hint) = match command {
        Commands::Setup { preset, .. } => (
            DiagnosticCode::CliToolchain,
            DiagnosticStage::ToolResolution,
            "setup",
            preset.clone(),
            "verify the selected profile, toolchain lock, network policy, and local cache",
        ),
        Commands::HostCompiler { command } => match command {
            HostCompilerCommands::Install { .. } => (
                DiagnosticCode::CliToolResolution,
                DiagnosticStage::ToolResolution,
                "host-compiler.install",
                None,
                "install the declared host compiler or verify the configured offline cache",
            ),
        },
        Commands::BuildTools { command } => match command {
            BuildToolsCommand::Build => (
                DiagnosticCode::CliToolResolution,
                DiagnosticStage::ToolResolution,
                "build-tools.build",
                None,
                "inspect the reported helper and Cargo failure, then rebuild the required build tools",
            ),
            BuildToolsCommand::Check => (
                DiagnosticCode::CliToolResolution,
                DiagnosticStage::ToolResolution,
                "build-tools.check",
                None,
                "inspect the named missing or unhealthy helper, then rebuild the required build tools",
            ),
        },
        Commands::Toolchain { command } => {
            match command {
                ToolchainCommands::Plan(args) => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    "toolchain.plan",
                    Some(args.preset().to_owned()),
                    "inspect the selected producer inputs and resolve the named readiness failure before building",
                ),
                ToolchainCommands::Build(toolchain_build::BuildArgs { preset, .. }) => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    "toolchain.build",
                    Some(preset.clone()),
                    "prepare the exact producer cache closure, then inspect the named build or compatibility failure",
                ),
                ToolchainCommands::Producer(args) => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    args.diagnostic_mode(),
                    None,
                    "inspect the named producer input, receipt, or retained output before retrying the exact stage",
                ),
                ToolchainCommands::MetaMakeFetch(_) => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    "toolchain.__metamake-fetch",
                    None,
                    "run the owning native MetaMake lifecycle; this bridge is not a user-facing recovery command",
                ),
                ToolchainCommands::Install { preset, .. } => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    "toolchain.install",
                    Some(preset.clone()),
                    "verify the selected lock artifact, local prefix, cache policy, and installation destination",
                ),
                ToolchainCommands::List { .. } => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    "toolchain.list",
                    None,
                    "verify the release lock and current-host artifact matrix",
                ),
                ToolchainCommands::Inventory(_) => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    "toolchain.inventory",
                    None,
                    "inspect the managed-store root and correct the named receipt or containment failure",
                ),
                ToolchainCommands::Import(_) => (
                    DiagnosticCode::CliPublication,
                    DiagnosticStage::Publication,
                    "toolchain.import",
                    None,
                    "re-run the preview, preserve any indeterminate receipt, and apply only its current token",
                ),
                ToolchainCommands::Register(_) => (
                    DiagnosticCode::CliPublication,
                    DiagnosticStage::Publication,
                    "toolchain.register",
                    None,
                    "re-run the preview and apply only the current registration token after fixing the named verification failure",
                ),
                ToolchainCommands::Select(_) => (
                    DiagnosticCode::CliPublication,
                    DiagnosticStage::Publication,
                    "toolchain.select",
                    None,
                    "re-run the preview and apply only the current selection token after checking the candidate lock",
                ),
                ToolchainCommands::Remove(_) => (
                    DiagnosticCode::CliPublication,
                    DiagnosticStage::Publication,
                    "toolchain.remove",
                    None,
                    "preserve an indeterminate removal journal; otherwise re-run the preview before applying its current token",
                ),
                ToolchainCommands::Gc(_) => (
                    DiagnosticCode::CliPublication,
                    DiagnosticStage::Publication,
                    "toolchain.gc",
                    None,
                    "inspect the listed blockers, then re-run the preview before applying its current token",
                ),
                ToolchainCommands::Verify { preset, .. } => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    "toolchain.verify",
                    Some(preset.clone()),
                    "repair or reinstall the exact named toolchain artifact, then verify it again",
                ),
                ToolchainCommands::Path { preset, .. } => (
                    DiagnosticCode::CliToolchain,
                    DiagnosticStage::ToolResolution,
                    "toolchain.path",
                    Some(preset.clone()),
                    "install or repair the exact named toolchain before requesting its verified path",
                ),
            }
        }
        Commands::Board { command } => match command {
            BoardCommand::Build { board, .. } => (
                DiagnosticCode::CliBuild,
                DiagnosticStage::BuildExecution,
                "board.build",
                Some(board.profile.clone()),
                "inspect the board profile and the reported configure or build failure",
            ),
            BoardCommand::Deploy { board, .. } => (
                DiagnosticCode::CliPublication,
                DiagnosticStage::Publication,
                "board.deploy",
                Some(board.profile.clone()),
                "validate the board profile, build artifact, and deployment destination before retrying",
            ),
            BoardCommand::Sd { command } => {
                match command {
                    SdCommand::Image { board, .. } => (
                        DiagnosticCode::CliMediaSafety,
                        DiagnosticStage::MediaSafety,
                        "board.sd.image",
                        Some(board.profile.clone()),
                        "re-run the dry run and satisfy every boot-bundle or media-safety check before creating an image",
                    ),
                    SdCommand::Scan { .. } => (
                        DiagnosticCode::CliMediaSafety,
                        DiagnosticStage::MediaSafety,
                        "board.sd.scan",
                        None,
                        "verify removable-media discovery prerequisites, then re-run the non-mutating scan",
                    ),
                    SdCommand::Unmount { .. } => (
                        DiagnosticCode::CliMediaSafety,
                        DiagnosticStage::MediaSafety,
                        "board.sd.unmount",
                        None,
                        "re-run the scan and use only its current opaque disk identifier",
                    ),
                    SdCommand::Write { board, .. } => (
                        DiagnosticCode::CliMediaSafety,
                        DiagnosticStage::MediaSafety,
                        "board.sd.write",
                        Some(board.profile.clone()),
                        "re-run the scan and dry run; use only the current opaque disk identifier and write token",
                    ),
                }
            }
            BoardCommand::Init { profile, .. } => (
                DiagnosticCode::CliBoard,
                DiagnosticStage::BoardOperation,
                "board.init",
                Some(profile.clone()),
                "check the profile name, configuration destination, and explicit apply mode",
            ),
            BoardCommand::Doctor(selection) => (
                DiagnosticCode::CliBoard,
                DiagnosticStage::BoardOperation,
                "board.doctor",
                Some(selection.profile.clone()),
                "inspect the board profile and the failed local prerequisite reported above",
            ),
            BoardCommand::Serve {
                board: selection, ..
            } => (
                DiagnosticCode::CliBoard,
                DiagnosticStage::BoardOperation,
                "board.serve",
                Some(selection.profile.clone()),
                "inspect the board profile, resolved deployment, and named local network prerequisite",
            ),
            BoardCommand::Console {
                board: selection, ..
            } => (
                DiagnosticCode::CliBoard,
                DiagnosticStage::BoardOperation,
                "board.console",
                Some(selection.profile.clone()),
                "inspect the board profile, serial device, and selected external terminal program",
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
            SourceCommand::Sync {
                upstream_branch, ..
            } => (
                DiagnosticCode::CliSourceState,
                DiagnosticStage::RepositoryDiscovery,
                "source.sync",
                Some(upstream_branch.clone()),
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
        Commands::Clean { preset, all, .. } => (
            DiagnosticCode::CliPublication,
            DiagnosticStage::Publication,
            "clean",
            if *all {
                Some("all".into())
            } else {
                preset.clone()
            },
            "verify the explicit clean scope and selected build directory before applying it",
        ),
        Commands::Test { preset, .. } => (
            DiagnosticCode::CliBoot,
            DiagnosticStage::BootValidation,
            "test",
            Some(preset.clone()),
            "inspect the retained boot evidence and the first reported serial or QEMU failure",
        ),
        Commands::Cache { command } => match command {
            CacheCommand::Status { .. } => (
                DiagnosticCode::CliConfiguration,
                DiagnosticStage::Configuration,
                "cache.status",
                None,
                "set AROS_HOME or AROS_CACHE_DIR to an absolute accessible path; cache status never creates or clears cache state",
            ),
            CacheCommand::Compiler {
                command: CacheCompilerCommand::Status { backend, dir, .. },
            } => {
                if dir.is_some() {
                    (
                        DiagnosticCode::CliConfiguration,
                        DiagnosticStage::Configuration,
                        "cache.compiler.status",
                        None,
                        "pass an absolute --dir path; compiler-cache status observes it only and never configures a backend",
                    )
                } else {
                    (
                        DiagnosticCode::CliToolResolution,
                        DiagnosticStage::ToolResolution,
                        "cache.compiler.status",
                        Some(
                            match backend {
                                CacheCompilerBackend::Auto => "auto",
                                CacheCompilerBackend::Sccache => "sccache",
                                CacheCompilerBackend::Ccache => "ccache",
                            }
                            .to_owned(),
                        ),
                        "inspect the passive availability report; this command does not start a compiler-cache backend or alter its storage",
                    )
                }
            }
            CacheCommand::Sources { command } => match command {
                CacheSourcesCommand::Status { .. } => (
                    DiagnosticCode::CliConfiguration,
                    DiagnosticStage::Configuration,
                    "cache.sources.status",
                    None,
                    "pass an absolute source-cache --dir; status observes root metadata only and never creates cache state",
                ),
                CacheSourcesCommand::List { .. } => (
                    DiagnosticCode::CliSourceInput,
                    DiagnosticStage::Configuration,
                    "cache.sources.list",
                    None,
                    "select one readable reviewed source lock or product plan and an existing real source-cache root; list never hashes or acquires payloads",
                ),
                CacheSourcesCommand::Fetch { offline, .. } => (
                    if *offline {
                        DiagnosticCode::CliSourceLock
                    } else {
                        DiagnosticCode::CliNetwork
                    },
                    DiagnosticStage::Configuration,
                    "cache.sources.fetch",
                    None,
                    "restore the selected reviewed closure and cache entries; offline fetch never accesses the network and online fetch never replaces an existing object",
                ),
                CacheSourcesCommand::Verify { .. } => (
                    DiagnosticCode::CliSourceLock,
                    DiagnosticStage::Configuration,
                    "cache.sources.verify",
                    None,
                    "restore the exact reviewed selector and cache objects; verify hashes payloads but never changes them",
                ),
            },
        },
        Commands::Ccache => (
            DiagnosticCode::CliToolResolution,
            DiagnosticStage::ToolResolution,
            "ccache",
            None,
            "install ccache or sccache before querying legacy statistics; use `aros cache compiler status` for a passive inspection that does not start a backend",
        ),
        Commands::Golden { action } => match action {
            GoldenAction::Capture { .. } => (
                DiagnosticCode::CliPublication,
                DiagnosticStage::Publication,
                "golden.capture",
                None,
                "inspect the named profile and generated product; capture only after reviewing an intentional change",
            ),
            GoldenAction::Verify { .. } => (
                DiagnosticCode::CliPublication,
                DiagnosticStage::Publication,
                "golden.verify",
                None,
                "inspect the named profile and generated product; update only after reviewing an intentional change",
            ),
        },
        Commands::Completions { .. } => (
            DiagnosticCode::CliConfiguration,
            DiagnosticStage::Configuration,
            "completions",
            None,
            "select bash, zsh, or fish and write the generated script to your shell completion directory",
        ),
        Commands::Info { .. } => (
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
    let matches = match Cli::command().try_get_matches_from(arguments) {
        Ok(matches) => matches,
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
    let log_level_was_explicit = matches
        .value_source("log_level")
        .is_some_and(|source| source != ValueSource::DefaultValue);
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
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
    if let Commands::Completions { shell } = cli.command {
        return completion_model::emit(shell, format);
    }
    let invocation_directory = match std::env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            render_diagnostics(
                &DiagnosticSet::single(
                    Diagnostic::error(
                        DiagnosticCode::CliConfiguration,
                        DiagnosticStage::Configuration,
                        format!("could not determine the invocation directory: {error}"),
                    )
                    .with_hint("run from an accessible directory and retry"),
                ),
                format,
                observability::POLICY,
            );
            return ExitCode::FAILURE;
        }
    };
    let logger = match Logger::open(
        effective_log_level(
            cli.observability.log_level,
            log_level_was_explicit,
            cli.observability.log_file.is_some(),
        ),
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
    observability::reset_recorded_mutation_state();
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
        match resolve_repository(&invocation_directory, cli.command.repository_requirement()) {
            Ok(repo_root) => repo_root,
            Err(error) => {
                let diagnostic = observability::report_diagnostic(
                    &error,
                    observability::ErrorBoundary::REPOSITORY,
                    context,
                );
                let mut diagnostics = observability::take_machine_subprocess_warnings();
                diagnostics.push(diagnostic);
                for diagnostic in diagnostics.clone() {
                    if let Err(log_error) = logger.diagnostic(&diagnostic) {
                        diagnostics.push(log_error.into_diagnostic());
                    }
                }
                render_diagnostics(
                    &observability::set(diagnostics),
                    format,
                    observability::POLICY,
                );
                return ExitCode::FAILURE;
            }
        };

    let result = commands::run(cli.command, repo_root.as_deref()).await;
    match result {
        Ok(()) => {
            if let Some(diagnostic) = aros_common::take_stdout_failure_diagnostic(
                DiagnosticCode::CliObservability,
                DiagnosticStage::Observability,
            ) {
                let mut diagnostics = vec![diagnostic.with_context(context.clone())];
                observability::attach_recorded_mutation_state(
                    diagnostics[0]
                        .context
                        .as_mut()
                        .expect("deferred stdout diagnostic received command context"),
                );
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
            for warning in observability::take_machine_subprocess_warnings() {
                if let Err(error) = logger.diagnostic(&warning) {
                    let mut diagnostic = error.into_diagnostic();
                    let mut reporting_context = context.clone();
                    if let Some(error_context) = diagnostic.context.take() {
                        reporting_context.log_path = error_context.log_path;
                    }
                    observability::attach_recorded_mutation_state(&mut reporting_context);
                    diagnostic.context = Some(reporting_context);
                    render_diagnostics(
                        &DiagnosticSet::single(diagnostic),
                        format,
                        observability::POLICY,
                    );
                    return ExitCode::FAILURE;
                }
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
                    let mut reporting_context = context.clone();
                    if let Some(error_context) = diagnostic.context.take() {
                        reporting_context.log_path = error_context.log_path;
                    }
                    observability::attach_recorded_mutation_state(&mut reporting_context);
                    diagnostic.context = Some(reporting_context);
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
            let mut diagnostic = observability::report_diagnostic(&error, boundary, context);
            if let Some(context) = diagnostic.context.as_mut() {
                observability::attach_recorded_mutation_state(context);
            }
            let mut diagnostics = observability::take_machine_subprocess_warnings();
            diagnostics.push(diagnostic);
            for diagnostic in diagnostics.clone() {
                if let Err(log_error) = logger.diagnostic(&diagnostic) {
                    diagnostics.push(log_error.into_diagnostic());
                }
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

#[cfg(test)]
mod tests {
    use super::{
        cli_contract::RepositoryRequirement,
        cli_contract_render::{
            public_contract_commands, rendered_cli_contract_index, rendered_cli_contract_section,
            table_cell,
        },
        cli_contract_sections::CLI_CONTRACT_SECTIONS,
        command_boundary, BoardCommand, BoardInitModel, BoardInitTransport, BoardModel, Cli,
        Commands, Parser,
    };
    use clap::{error::ErrorKind, CommandFactory};

    const CLI_CONTRACT_INDEX: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs-site/src/content/docs/reference/cli-contract.md"
    ));
    #[test]
    fn cli_contract_escapes_markdown_table_separators_once() {
        assert_eq!(table_cell("one|two"), "one\\|two");
    }

    #[test]
    fn generated_public_cli_contract_matches_the_clap_model() {
        let mut command = Cli::command();
        command.build();
        assert_eq!(
            CLI_CONTRACT_INDEX,
            rendered_cli_contract_index(&command),
            "a public CLI-model change requires an intentional reviewed update to docs-site/src/content/docs/reference/cli-contract.md"
        );
        let expected_sections = CLI_CONTRACT_SECTIONS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        let actual_sections = public_contract_commands(&command)
            .iter()
            .map(|child| child.get_name())
            .collect::<Vec<_>>();
        assert_eq!(
            expected_sections, actual_sections,
            "every visible top-level command needs exactly one generated contract section"
        );
        for (name, expected) in CLI_CONTRACT_SECTIONS {
            let section = command
                .find_subcommand(name)
                .expect("declared public contract section must resolve");
            assert_eq!(
                *expected,
                rendered_cli_contract_section(section),
                "a public CLI-model change requires an intentional reviewed update to docs-site/src/content/docs/reference/cli-contract/{name}.md"
            );
        }
    }

    #[test]
    fn diagnostic_context_identifies_the_exact_command_leaf() {
        let cases: &[(&[&str], &str)] = &[
            (
                &["aros", "host-compiler", "install"],
                "host-compiler.install",
            ),
            (&["aros", "build-tools", "check"], "build-tools.check"),
            (&["aros", "toolchain", "list"], "toolchain.list"),
            (
                &["aros", "toolchain", "path", "--preset", "pc-x86_64"],
                "toolchain.path",
            ),
            (
                &[
                    "aros",
                    "toolchain",
                    "producer",
                    "compatibility-host-tools",
                    "--host",
                    "linux-x86_64",
                ],
                "toolchain.producer.compatibility-host-tools",
            ),
            (&["aros", "board", "scan"], "board.scan"),
            (&["aros", "board", "sd", "scan"], "board.sd.scan"),
            (&["aros", "golden", "capture"], "golden.capture"),
            (&["aros", "source", "init", "/tmp/AROS"], "source.init"),
        ];
        for (arguments, expected_mode) in cases {
            let command = Cli::try_parse_from(*arguments)
                .expect("leaf diagnostic case must parse")
                .command;
            let (boundary, context) = command_boundary(&command);
            assert_eq!(context.mode.as_deref(), Some(*expected_mode));
            assert!(
                !boundary.hint.is_empty(),
                "every exact command leaf must retain actionable recovery guidance"
            );
        }
    }

    #[test]
    #[ignore = "developer aid for regenerating the reviewed CLI contract snapshots"]
    fn regenerate_public_cli_contract_snapshots() {
        let mut command = Cli::command();
        command.build();
        let documentation_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs-site/src/content/docs/reference");
        std::fs::write(
            documentation_root.join("cli-contract.md"),
            rendered_cli_contract_index(&command),
        )
        .expect("write generated contract index");
        for (name, _) in CLI_CONTRACT_SECTIONS {
            let section = command
                .find_subcommand(name)
                .expect("declared public contract section must resolve");
            std::fs::write(
                documentation_root
                    .join("cli-contract")
                    .join(format!("{name}.md")),
                rendered_cli_contract_section(section),
            )
            .expect("write generated contract section");
        }
    }

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
            requirement(&["aros", "toolchain", "inventory"]),
            RepositoryRequirement::Global
        );
        assert_eq!(
            requirement(&["aros", "info"]),
            RepositoryRequirement::Optional
        );
        assert_eq!(
            requirement(&["aros", "clean", "--preset", "pc-x86_64"]),
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
    fn installation_transport_modes_fail_before_repository_discovery() {
        for arguments in [
            &[
                "aros",
                "setup",
                "--preset",
                "pc-x86_64",
                "--local",
                "/opt/aros",
                "--force",
            ][..],
            &[
                "aros",
                "setup",
                "--preset",
                "pc-x86_64",
                "--force",
                "--offline",
            ][..],
            &["aros", "host-compiler", "install", "--force", "--offline"][..],
            &[
                "aros",
                "toolchain",
                "install",
                "--preset",
                "pc-x86_64",
                "--local",
                "/opt/aros",
                "--force",
            ][..],
            &[
                "aros",
                "toolchain",
                "install",
                "--preset",
                "pc-x86_64",
                "--force",
                "--offline",
            ][..],
        ] {
            assert_eq!(
                parse_error(arguments),
                ErrorKind::ArgumentConflict,
                "{arguments:?}"
            );
        }

        assert!(Cli::try_parse_from([
            "aros",
            "setup",
            "--preset",
            "pc-x86_64",
            "--local",
            "/opt/aros",
            "--offline",
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["aros", "host-compiler", "install", "--offline"]).is_ok());
        assert!(Cli::try_parse_from([
            "aros",
            "toolchain",
            "install",
            "--preset",
            "pc-x86_64",
            "--local",
            "/opt/aros",
            "--offline",
        ])
        .is_ok());
    }

    #[test]
    fn source_sync_uses_branch_while_source_init_retains_ref() {
        assert!(Cli::try_parse_from(["aros", "source", "sync", "--branch", "main"]).is_ok());
        assert_eq!(
            parse_error(&["aros", "source", "sync", "--ref", "main"]),
            ErrorKind::UnknownArgument
        );
        assert!(Cli::try_parse_from([
            "aros",
            "source",
            "init",
            "AROS",
            "--ref",
            "refs/heads/main",
        ])
        .is_ok());
    }

    #[test]
    fn clean_requires_one_explicit_scope_and_supports_preview() {
        assert_eq!(
            parse_error(&["aros", "clean"]),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            parse_error(&["aros", "clean", "--preset", "pc-x86_64", "--all"]),
            ErrorKind::ArgumentConflict
        );
        assert!(
            Cli::try_parse_from(["aros", "clean", "--preset", "pc-x86_64", "--dry-run",]).is_ok()
        );
        assert!(Cli::try_parse_from(["aros", "clean", "--all", "--dry-run"]).is_ok());
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

    #[test]
    fn resource_and_board_mutation_contracts_are_enforced_by_the_parser() {
        for arguments in [
            &["aros", "test", "--timeout", "0"][..],
            &["aros", "test", "--memory", "0"][..],
            &[
                "aros",
                "board",
                "console",
                "--profile",
                "rpi5",
                "--baud",
                "0",
            ][..],
        ] {
            assert_eq!(
                parse_error(arguments),
                ErrorKind::ValueValidation,
                "{arguments:?}"
            );
        }
        for arguments in [
            &[
                "aros",
                "board",
                "deploy",
                "--profile",
                "rpi5",
                "--apply",
                "--dry-run",
            ][..],
            &[
                "aros",
                "board",
                "sd",
                "image",
                "--profile",
                "rpi5",
                "--boot-bundle",
                "/bundle",
                "--output",
                "/output",
                "--apply",
                "--dry-run",
            ][..],
        ] {
            assert_eq!(
                parse_error(arguments),
                ErrorKind::ArgumentConflict,
                "{arguments:?}"
            );
        }
        assert!(Cli::try_parse_from(["aros", "test", "--timeout", "1", "--memory", "1",]).is_ok());
    }

    #[test]
    fn board_init_requires_a_typed_model_and_never_inferrs_one_from_the_profile_label() {
        assert_eq!(
            parse_error(&["aros", "board", "init", "--profile", "pi5-usb"]),
            ErrorKind::MissingRequiredArgument
        );
        assert_eq!(
            parse_error(&["aros", "board", "init", "--board", "pi5-usb", "--model", "rpi5",]),
            ErrorKind::UnknownArgument
        );

        let parsed = Cli::try_parse_from([
            "aros",
            "board",
            "init",
            "--profile",
            "pi5-usb",
            "--model",
            "rpi3",
            "--transport",
            "native-tftp",
        ])
        .expect("explicit model and transport parse");
        let Commands::Board {
            command:
                BoardCommand::Init {
                    profile,
                    model,
                    transport,
                    ..
                },
        } = parsed.command
        else {
            panic!("expected board init command");
        };
        assert_eq!(profile, "pi5-usb");
        assert_eq!(model, BoardInitModel::Rpi3);
        assert_eq!(transport, Some(BoardInitTransport::NativeTftp));
        assert!(Cli::try_parse_from([
            "aros",
            "board",
            "init",
            "--profile",
            "titan",
            "--model",
            "milk-v-titan",
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["aros", "board", "doctor", "--profile", "pi5-usb"]).is_ok());
    }

    #[test]
    fn board_init_model_contract_covers_each_reviewed_default_transport() {
        let defaults = [
            ("rpi3", BoardModel::Rpi3, "native-tftp"),
            ("rpi4", BoardModel::Rpi4, "native-tftp"),
            ("rpi5", BoardModel::Rpi5, "native-tftp"),
            ("milk-v-titan", BoardModel::MilkVTitan, "uefi-esp"),
        ];
        for (model_argument, expected_model, expected_transport) in defaults {
            let parsed = Cli::try_parse_from([
                "aros",
                "board",
                "init",
                "--profile",
                "deliberately-unrelated-label",
                "--model",
                model_argument,
            ])
            .expect("reviewed model must parse independently of the profile label");
            let Commands::Board {
                command:
                    BoardCommand::Init {
                        model, transport, ..
                    },
            } = parsed.command
            else {
                panic!("expected board init command");
            };
            let model: BoardModel = model.into();
            assert_eq!(model, expected_model);
            assert_eq!(transport, None);
            assert_eq!(model.default_transport().to_string(), expected_transport);
        }
    }

    #[test]
    fn toolchain_inventory_budget_is_bounded_at_the_cli_boundary() {
        assert_eq!(
            parse_error(&["aros", "toolchain", "inventory", "--max-entries", "0"]),
            ErrorKind::ValueValidation
        );
        assert_eq!(
            parse_error(&["aros", "toolchain", "inventory", "--max-entries", "100001"]),
            ErrorKind::ValueValidation
        );
        assert!(
            Cli::try_parse_from(["aros", "toolchain", "inventory", "--max-entries", "1"]).is_ok()
        );
    }

    #[test]
    fn native_producer_surface_has_one_prepared_input_mode_and_structural_package_output() {
        let native_roots = [
            "--preset",
            "pc-x86_64",
            "--recipe",
            "/recipe.json",
            "--source-dir",
            "/source",
            "--producer-dir",
            "/producer",
            "--tools-dir",
            "/tools",
        ];
        for command in ["plan", "build"] {
            let mut arguments = vec!["aros", "toolchain", command];
            arguments.extend(native_roots);
            arguments.push("--offline");
            assert_eq!(parse_error(&arguments), ErrorKind::UnknownArgument);
        }

        let package_context = [
            "--recipe",
            "/recipe.json",
            "--source-lock",
            "/producer/toolchains/lock.sources.json",
            "--profiles",
            "/producer/toolchains/profiles.json",
            "--preset",
            "pc-x86_64",
            "--release-id",
            "candidate-1",
            "--host",
            "linux-x86_64",
            "--build-environment",
            "/evidence/environment.json",
        ];
        let mut package = vec!["aros", "toolchain", "producer", "package"];
        package.extend(package_context);
        package.extend(["--input-dir", "/candidate/toolchain"]);
        assert_eq!(
            parse_error(&package),
            ErrorKind::MissingRequiredArgument,
            "package cannot silently choose an output root"
        );

        let mut verify = vec!["aros", "toolchain", "producer", "verify-package"];
        verify.extend(package_context);
        verify.extend([
            "--input-dir",
            "/candidate/toolchain",
            "--output-dir",
            "/packages/out",
        ]);
        assert_eq!(
            parse_error(&verify),
            ErrorKind::UnknownArgument,
            "verification must not accept a packaging output option"
        );
    }
}
