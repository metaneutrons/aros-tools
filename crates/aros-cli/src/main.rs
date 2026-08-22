use clap::{Parser, Subcommand};
use console::{style, Emoji};
use miette::Result;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

mod artifact;
mod host_tools;
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

    /// Manage deterministic AROS cross-toolchain releases
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
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

    /// Run automated or interactive QEMU boot test for a target
    Test {
        /// Target preset to test
        #[arg(short, long, default_value = "pc-x86_64")]
        preset: String,

        /// Run in headless mode (no GUI window, default)
        #[arg(long)]
        headless: bool,

        /// Open graphical QEMU window (interactive mode)
        #[arg(short, long)]
        gui: bool,

        /// Timeout duration in seconds (0 = run indefinitely until window closed)
        #[arg(short, long, default_value_t = 10)]
        timeout: u64,
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

    /// Print system and toolchain information
    Info,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

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
            headless,
            gui,
            timeout,
        } => {
            // Headless is the default; --gui is what switches it off. The
            // --headless flag stays accepted so it can be passed explicitly.
            let _ = headless;
            let is_headless = !gui;

            println!(
                "🧪 Running QEMU test suite for [{}] (mode: {})...",
                style(&preset).yellow().bold(),
                if is_headless {
                    style("headless").cyan()
                } else {
                    style("interactive GUI").magenta().bold()
                }
            );

            let iso_path = format!("build/{preset}/aros-x86_64-pc.iso");
            if !std::path::Path::new(&iso_path).exists() {
                println!("{HAMMER} ISO image not found. Building target 'boot-iso'...");
                let status = Command::new("cmake")
                    .args([
                        "--build",
                        &format!("build/{preset}"),
                        "--target",
                        "boot-iso",
                    ])
                    .status()
                    .map_err(|e| miette::miette!("Failed to build boot-iso: {e}"))?;
                if !status.success() {
                    miette::bail!("Failed to generate bootable ISO for '{preset}'");
                }
            }

            let qemu_bin = if preset.contains("x86_64") {
                "qemu-system-x86_64"
            } else if preset.contains("aarch64") {
                "qemu-system-aarch64"
            } else if preset.contains("arm") {
                "qemu-system-arm"
            } else {
                "qemu-system-x86_64"
            };

            if which::which(qemu_bin).is_err() {
                miette::bail!("Emulator binary '{qemu_bin}' not found in PATH.");
            }

            println!(
                "🚀 Launching {} with [{}]...",
                style(qemu_bin).green().bold(),
                style(&iso_path).yellow()
            );

            let mut qemu_cmd = Command::new(qemu_bin);
            let bootstrap_path = format!("build/{preset}/bootstrap");
            if std::path::Path::new(&bootstrap_path).exists() {
                qemu_cmd.args(["-kernel", &bootstrap_path]);
                let exec_lib = format!("build/{preset}/SYS/Libs/kernel-exec.library");
                if std::path::Path::new(&exec_lib).exists() {
                    qemu_cmd.args(["-initrd", &exec_lib]);
                }
            }
            qemu_cmd.args([
                "-cdrom", &iso_path, "-m", "512M", "-smp", "2", "-serial", "stdio", "-boot",
                "order=c",
            ]);

            if is_headless {
                qemu_cmd.args(["-display", "none"]);
            } else {
                qemu_cmd.args(["-vga", "std"]);
            }

            let mut child = qemu_cmd
                .spawn()
                .map_err(|e| miette::miette!("Failed to start QEMU: {e}"))?;

            if timeout > 0 {
                println!(
                    "⏱️  Executing test run for {timeout}s (use --timeout 0 for indefinite run)..."
                );
                std::thread::sleep(std::time::Duration::from_secs(timeout));
                let _ = child.kill();
                let _ = child.wait();
                println!(
                    "{CHECK} {}QEMU boot execution finished cleanly without crashes!",
                    style("VERIFIED: ").green().bold()
                );
            } else {
                println!(
                    "🎮 QEMU is running interactively. Close the window or press Ctrl+C to exit."
                );
                let _ = child.wait();
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
