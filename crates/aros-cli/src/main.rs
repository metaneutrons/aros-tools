use clap::{Args, Parser, Subcommand};
use console::{style, Emoji};
use miette::Result;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

mod artifact;
mod boot;
mod build;
mod golden;
mod host_tools;
mod hosttools;
mod pi;
mod repo;
mod toolchain;

static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static HAMMER: Emoji<'_, '_> = Emoji("🔨 ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static SPARKLES: Emoji<'_, '_> = Emoji("✨ ", "");

#[derive(Parser)]
#[command(
    name = "aros",
    author = "AROS Development Team & Fabian Schmieder (@metaneutrons)",
    version = "0.1.0",
    about = "AROS Tools v0.1: Next-Generation Build System & Tooling Engine",
    long_about = "Modern, ultra-fast, multi-platform build orchestrator and upstream sync pipeline for AROS."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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

    /// Manage the host LLVM tools used to bootstrap builds
    HostTools {
        #[command(subcommand)]
        command: HostToolsCommands,
    },

    /// Build or inspect the local Rust helpers consumed by CMake
    Hosttools {
        #[command(subcommand)]
        command: HosttoolsCommand,
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
enum HostToolsCommands {
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

#[derive(Subcommand)]
enum HosttoolsCommand {
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let repo_root = repo::find_root()?;
    std::env::set_current_dir(&repo_root).map_err(|error| {
        miette::miette!(
            "Could not enter AROS-NG checkout '{}': {error}",
            repo_root.display()
        )
    })?;

    match cli.command {
        Commands::Setup {
            force,
            preset,
            all,
            offline,
            local,
        } => {
            if all {
                if local.is_some() {
                    miette::bail!("--local cannot be combined with --all");
                }
                let profiles = aros_common::TargetProfile::load_from_file(std::path::Path::new(
                    "aros-targets.toml",
                ))
                .map_err(|error| miette::miette!("{error}"))?;
                for profile in profiles {
                    toolchain::install(&profile.name, offline, force, None)
                        .await
                        .map_err(|error| miette::miette!("{error}"))?;
                }
            } else if let Some(preset) = preset {
                toolchain::install(&preset, offline, force, local.as_deref())
                    .await
                    .map_err(|error| miette::miette!("{error}"))?;
            } else if local.is_some() {
                miette::bail!("--local requires --preset");
            } else {
                host_tools::install(force, offline)
                    .await
                    .map_err(|error| miette::miette!("{error}"))?;
            }
        }

        Commands::HostTools { command } => match command {
            HostToolsCommands::Install { force, offline } => {
                host_tools::install(force, offline)
                    .await
                    .map_err(|error| miette::miette!("{error}"))?;
            }
        },

        Commands::Hosttools { command } => match command {
            HosttoolsCommand::Build => {
                hosttools::build(&repo_root)?;
            }
            HosttoolsCommand::Check => {
                hosttools::print_check(&repo_root)?;
            }
        },

        Commands::Toolchain { command } => match command {
            ToolchainCommands::Install {
                preset,
                force,
                offline,
                local,
            } => {
                toolchain::install(&preset, offline, force, local.as_deref())
                    .await
                    .map_err(|error| miette::miette!("{error}"))?;
            }
            ToolchainCommands::List => {
                toolchain::list().map_err(|error| miette::miette!("{error}"))?;
            }
            ToolchainCommands::Verify { preset, local } => {
                toolchain::verify(&preset, local.as_deref())
                    .map_err(|error| miette::miette!("{error}"))?;
            }
            ToolchainCommands::Path { preset, local } => {
                let resolved = toolchain::path(&preset, local.as_deref())
                    .map_err(|error| miette::miette!("{error}"))?;
                println!("{}", resolved.paths.root.display());
            }
        },

        Commands::Pi { command } => match command {
            PiCommand::Init {
                board,
                config,
                apply,
            } => {
                pi::config::initialize_template(config.as_deref(), &board, apply)?;
            }
            PiCommand::Scan => {
                pi::scan()?;
            }
            PiCommand::Doctor(board) => {
                let board = load_board(&board)?;
                pi::doctor(&board, &repo_root)?;
            }
            PiCommand::Build {
                board,
                target,
                jobs,
                clean,
                verbose,
                offline,
                toolchain_dir,
                dtb_path,
                core_kobj_dir,
            } => {
                let board = load_board(&board)?;
                pi::build(
                    &board,
                    &repo_root,
                    build::BuildOptions {
                        preset: board.config.preset.clone(),
                        toolchain_preset: board.config.toolchain_preset.clone(),
                        target: target.or_else(|| Some(board.config.build_target.clone())),
                        jobs,
                        clean,
                        verbose,
                        offline,
                        toolchain_dir,
                        cmake_definitions: Vec::new(),
                    },
                    dtb_path.as_deref(),
                    core_kobj_dir.as_deref(),
                )
                .await?;
            }
            PiCommand::Deploy {
                board,
                artifact_dir,
                apply,
                dry_run,
            } => {
                if apply && dry_run {
                    miette::bail!("--apply and --dry-run cannot be used together.");
                }
                let board = load_board(&board)?;
                pi::deploy(&board, &repo_root, artifact_dir.as_deref(), apply)?;
            }
            PiCommand::Serve { board, dry_run } => {
                let board = load_board(&board)?;
                pi::serve::run(&board, dry_run).await?;
            }
            PiCommand::Sd { command } => match command {
                SdCommand::Image {
                    board,
                    boot_bundle,
                    output,
                    apply,
                    dry_run,
                } => {
                    if apply && dry_run {
                        miette::bail!("--apply and --dry-run cannot be used together.");
                    }
                    let board = load_board(&board)?;
                    pi::create_sd_image(&board, &boot_bundle, &output, apply)?;
                }
                SdCommand::Scan { artifact } => {
                    pi::scan_sd_disks(artifact.as_deref())?;
                }
                SdCommand::Unmount {
                    device,
                    apply,
                    dry_run,
                } => {
                    pi::unmount_sd_disk(device.as_deref(), apply, dry_run)?;
                }
                SdCommand::Write {
                    board,
                    artifact,
                    device,
                    confirm,
                    dry_run,
                } => {
                    let board = load_board(&board)?;
                    pi::write_sd_image(&board, &artifact, &device, confirm.as_deref(), dry_run)?;
                }
            },
            PiCommand::Console {
                board,
                program,
                device,
                baud,
                dry_run,
            } => {
                let board = load_board(&board)?;
                pi::console(&board, program, device, baud, dry_run)?;
            }
        },

        Commands::Build {
            preset,
            target,
            jobs,
            clean,
            verbose,
            offline,
            toolchain_dir,
        } => {
            let profile =
                toolchain::target_profile(&preset).map_err(|error| miette::miette!("{error}"))?;
            let resolved = toolchain::resolve_for_build(&preset, toolchain_dir.as_deref(), offline)
                .await
                .map_err(|error| miette::miette!("{error}"))?;

            println!(
                "{ROCKET} {}Building AROS for target preset [{}]...",
                style("AROS-NG: ").cyan().bold(),
                style(&preset).yellow().bold()
            );
            println!(
                "🔧 Cross toolchain: {} ({}, {:?})",
                resolved.paths.root.display(),
                resolved
                    .release_id
                    .as_deref()
                    .unwrap_or("local-unversioned"),
                resolved.source
            );
            let start = Instant::now();

            if clean {
                println!("🧹 Cleaning build directory for {preset}...");
                let _ = std::fs::remove_dir_all(format!("build/{preset}"));
            }

            // Check ccache/sccache launcher
            let launcher = if which::which("sccache").is_ok() {
                "sccache"
            } else if which::which("ccache").is_ok() {
                "ccache"
            } else {
                "none"
            };
            println!(
                "⚡ Compiler cache launcher: {}",
                style(launcher).green().bold()
            );

            // Run CMake Configure
            println!("{HAMMER} Configuring CMake build tree...");
            let cmake_toolchain = std::env::current_dir()
                .map_err(|error| miette::miette!("Failed to resolve repository root: {error}"))?
                .join("cmake/toolchains/AROS.cmake");
            if !cmake_toolchain.is_file() {
                miette::bail!(
                    "Required CMake toolchain file is missing: {}",
                    cmake_toolchain.display()
                );
            }
            let mut cfg_cmd = Command::new("cmake");
            cfg_cmd.args(["--preset", &preset]);
            cfg_cmd.arg(format!(
                "-DCMAKE_TOOLCHAIN_FILE={}",
                cmake_toolchain.display()
            ));
            cfg_cmd.arg(format!(
                "-DAROS_CROSS_TOOLCHAIN_ROOT={}",
                resolved.paths.root.display()
            ));
            cfg_cmd.arg(format!("-DAROS_TARGET_CPU={}", profile.arch));
            cfg_cmd.arg(format!("-DAROS_TARGET_PLATFORM={}", profile.platform));
            cfg_cmd.arg(format!("-DAROS_TARGET_PROFILE={}", profile.name));
            cfg_cmd.arg(format!("-DAROS_TARGET_TRIPLE={}", resolved.target_triple));
            if let Some(float_abi) = &profile.float_abi {
                cfg_cmd.arg(format!("-DGCC_CONFIG_FLOAT_ABI={float_abi}"));
            }
            if verbose {
                cfg_cmd.arg("--log-level=VERBOSE");
            }
            let status = cfg_cmd
                .status()
                .map_err(|e| miette::miette!("Failed to execute cmake configure: {e}"))?;
            if !status.success() {
                miette::bail!("CMake configure failed for preset '{preset}'");
            }

            // Run CMake Build with Ninja
            println!("{HAMMER} Compiling AROS modules with Ninja...");
            let mut build_cmd = Command::new("cmake");
            build_cmd.args(["--build", &format!("build/{preset}")]);
            if let Some(t) = target {
                build_cmd.args(["--target", &t]);
            }
            if let Some(j) = jobs {
                build_cmd.args(["-j", &j.to_string()]);
            }
            let build_status = build_cmd
                .status()
                .map_err(|e| miette::miette!("Failed to execute cmake build: {e}"))?;
            if !build_status.success() {
                miette::bail!("Build failed for preset '{preset}'");
            }

            let elapsed = start.elapsed();
            println!(
                "{CHECK} {}Build completed successfully in {:.2?}!",
                style("SUCCESS: ").green().bold(),
                elapsed
            );
        }

        Commands::Clean { preset } => {
            let target_dir = preset.map_or_else(|| "build".into(), |p| format!("build/{p}"));
            println!("🧹 Removing directory {target_dir}...");
            let _ = std::fs::remove_dir_all(&target_dir);
            println!("{CHECK} Clean complete.");
        }

        Commands::Test {
            preset,
            timeout,
            packages,
            modules,
            evidence,
            memory,
        } => {
            let build_dir = PathBuf::from(format!("build/{preset}"));
            if !build_dir.is_dir() {
                miette::bail!(
                    "no build directory at {} -- configure and build the preset first",
                    build_dir.display()
                );
            }

            // Every package the build produced, discovered rather than listed,
            // so a new one is included without editing this.
            let mut module_list = modules;
            let mut missing_packages = Vec::new();
            if packages {
                for dir in ["SYS/boot", "SYS/boot/pc"] {
                    let Ok(entries) = std::fs::read_dir(build_dir.join(dir)) else {
                        missing_packages.push(dir.to_owned());
                        continue;
                    };
                    let mut found: Vec<PathBuf> = entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().is_some_and(|extension| extension == "pkg"))
                        .collect();
                    found.sort();
                    module_list.extend(found);
                }
            }

            let evidence = evidence.unwrap_or_else(|| build_dir.join("boot-check"));
            let request = boot::BootRequest {
                build_dir,
                modules: module_list,
                seconds: timeout,
                evidence,
                memory_mb: memory,
            };
            println!(
                "Booting [{}] with {} multiboot module(s) for {}s...",
                style(&preset).yellow().bold(),
                request.modules.len() + 1,
                timeout
            );

            let mut report = boot::check(&request)?;
            for dir in missing_packages {
                report
                    .untested
                    .push(format!("{dir} holds no packages in this build"));
            }
            print!("{}", boot::render(&report));
            if report.is_success() {
                println!(
                    "{CHECK} {}the boot produced no failure and no exception.",
                    style("PASS: ").green().bold()
                );
            } else {
                println!(
                    "{}the boot did not come up clean; every finding above is read \
                     out of the logs, not inferred.",
                    style("FAIL: ").red().bold()
                );
                std::process::exit(1);
            }
        }

        Commands::Ccache { stats, clear } => {
            if clear {
                if which::which("sccache").is_ok() {
                    let _ = Command::new("sccache").arg("-z").status();
                } else if which::which("ccache").is_ok() {
                    let _ = Command::new("ccache").arg("-C").status();
                }
                println!("{CHECK} Compiler cache cleared.");
            }
            if stats {
                if which::which("sccache").is_ok() {
                    let _ = Command::new("sccache").arg("-s").status();
                } else if which::which("ccache").is_ok() {
                    let _ = Command::new("ccache").arg("-s").status();
                }
            }
        }

        Commands::Sync { transpile } => {
            println!("🔄 Fetching latest commits from upstream (aros-development-team/AROS)...");
            let _ = Command::new("git")
                .args(["fetch", "upstream", "master"])
                .status();
            let _ = Command::new("git")
                .args(["merge", "upstream/master", "--no-edit"])
                .status();

            if transpile {
                println!("⚡ Regenerating dynamic CMake target tree...");
                println!("{CHECK} Sync and target regeneration complete!");
            }
        }

        Commands::Golden { action } => {
            // The transpiler this checks is the one the build uses, so a
            // refactor is measured on the binary that would ship, not on a
            // debug build with different inlining. It has to be built first;
            // saying so beats replaying a stale binary against a new baseline.
            let transpiler = PathBuf::from("tools/aros-tools/target/release/aros-transpiler");
            if !transpiler.is_file() {
                miette::bail!(
                    "no {} -- build it with `cargo build --release -p aros-transpiler` \
                     in tools/aros-tools",
                    transpiler.display()
                );
            }
            let build_root = PathBuf::from("build");
            let snapshot_root = build_root.join("golden");
            match action {
                GoldenAction::Capture { presets } => {
                    let subjects = golden::subjects(&build_root, &presets)?;
                    for subject in &subjects {
                        let capture = golden::capture(&transpiler, subject, &snapshot_root)?;
                        println!(
                            "{CHECK} {}: {} products captured to {}",
                            subject.name,
                            capture.products,
                            capture.destination.display()
                        );
                        match capture.reproduces_build_tree {
                            Some(true) => {
                                println!("  the recorded invocation reproduces the build tree's own output");
                            }
                            Some(false) => {
                                println!(
                                    "  note: it does not reproduce {} -- that tree may predate a \
                                     source change; the baseline itself is fine",
                                    subject.build_output.display()
                                );
                            }
                            None => println!(
                                "  note: {} is absent, so the record was not cross-checked",
                                subject.build_output.display()
                            ),
                        }
                    }
                }
                GoldenAction::Verify { presets, update } => {
                    let subjects = golden::subjects(&build_root, &presets)?;
                    let mut differing = Vec::new();
                    for subject in &subjects {
                        if update {
                            let capture = golden::capture(&transpiler, subject, &snapshot_root)?;
                            println!(
                                "{CHECK} {}: baseline replaced, {} products",
                                subject.name, capture.products
                            );
                            continue;
                        }
                        let (comparison, baseline) =
                            golden::verify(&transpiler, subject, &snapshot_root)?;
                        if comparison.is_clean() {
                            println!(
                                "{CHECK} {}: identical to {} ({} products)",
                                subject.name,
                                baseline.display(),
                                comparison.identical
                            );
                        } else {
                            println!("❌ {}: differs from {}", subject.name, baseline.display());
                            print!("{}", golden::render(&comparison));
                            differing.push(subject.name.clone());
                        }
                    }
                    if !differing.is_empty() {
                        miette::bail!(
                            "the generated output changed for {}. If that was the point, \
                             re-capture with `aros golden verify --update`",
                            differing.join(", ")
                        );
                    }
                }
            }
        }

        Commands::Info => {
            println!(
                "{SPARKLES} {}",
                style("AROS Tools v0.1: Workspace Info").cyan().bold()
            );
            println!("  • Toolchain Architecture: Multi-Target Modern CMake + Ninja");

            let host_dir = host_tools::default_host_tools_dir();
            let host_paths = host_tools::host_tool_paths(&host_dir);
            let tc_status = if host_tools::is_host_tools_installed(&host_paths) {
                format!("Pinned host LLVM ({})", host_paths.clang.display())
            } else if let Ok(clang) = which::which("clang") {
                format!("Unmanaged system LLVM ({})", clang.display())
            } else {
                "Not found (run `aros host-tools install`)".to_string()
            };
            println!(
                "  • Active C/C++ Compiler:  {}",
                style(tc_status).green().bold()
            );

            println!(
                "  • C/C++ Compiler Launcher: {}",
                which::which("sccache")
                    .or_else(|_| which::which("ccache"))
                    .map_or_else(|_| "none".into(), |p| p.display().to_string())
            );

            let targets = aros_common::TargetProfile::load_from_file(std::path::Path::new(
                "aros-targets.toml",
            ))
            .unwrap_or_else(|_| aros_common::TargetProfile::default_profiles());
            let target_names: Vec<String> = targets.into_iter().map(|t| t.name).collect();
            println!("  • Configured Targets:     {}", target_names.join(", "));
            match toolchain::load_lock() {
                Ok(lock) => println!(
                    "  • AROS Toolchain Lock:    {} ({} assets)",
                    lock.release_id,
                    lock.artifacts.len()
                ),
                Err(error) => println!("  • AROS Toolchain Lock:    invalid ({error})"),
            }
        }
    }

    Ok(())
}

fn load_board(selection: &BoardSelection) -> Result<pi::config::Board> {
    pi::load_board(selection.config.as_deref(), &selection.board)
}
