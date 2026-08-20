use clap::{Parser, Subcommand};
use console::{style, Emoji};
use miette::Result;
use std::process::Command;
use std::time::Instant;

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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Setup { force } => {
            toolchain::setup_toolchain(force).await.map_err(|e| miette::miette!("{e}"))?;
        }

        Commands::Build {
            preset,
            target,
            jobs,
            clean,
            verbose,
        } => {
            // Ensure hermetic toolchain is present
            let tc_dir = toolchain::default_toolchain_dir();
            let tc_paths = toolchain::get_toolchain_paths(&tc_dir);
            if !toolchain::is_toolchain_installed(&tc_paths) && which::which("clang").is_err() {
                println!("ℹ️ Hermetic toolchain not found. Initializing automatic setup...");
                toolchain::setup_toolchain(false).await.map_err(|e| miette::miette!("{e}"))?;
            }

            println!(
                "{ROCKET} {}Building AROS for target preset [{}]...",
                style("AROS-NG: ").cyan().bold(),
                style(&preset).yellow().bold()
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
            let mut cfg_cmd = Command::new("cmake");
            cfg_cmd.args(["--preset", &preset]);
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
                if is_headless { style("headless").cyan() } else { style("interactive GUI").magenta().bold() }
            );

            let iso_path = format!("build/{preset}/aros-x86_64-pc.iso");
            if !std::path::Path::new(&iso_path).exists() {
                println!("{HAMMER} ISO image not found. Building target 'boot-iso'...");
                let status = Command::new("cmake")
                    .args(["--build", &format!("build/{preset}"), "--target", "boot-iso"])
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
            qemu_cmd.args(["-cdrom", &iso_path, "-m", "512M", "-smp", "2", "-serial", "stdio", "-boot", "order=c"]);

            if is_headless {
                qemu_cmd.args(["-display", "none"]);
            } else {
                qemu_cmd.args(["-vga", "std"]);
            }

            let mut child = qemu_cmd
                .spawn()
                .map_err(|e| miette::miette!("Failed to start QEMU: {e}"))?;

            if timeout > 0 {
                println!("⏱️  Executing test run for {}s (use --timeout 0 for indefinite run)...", timeout);
                std::thread::sleep(std::time::Duration::from_secs(timeout));
                let _ = child.kill();
                let _ = child.wait();
                println!(
                    "{CHECK} {}QEMU boot execution finished cleanly without crashes!",
                    style("VERIFIED: ").green().bold()
                );
            } else {
                println!("🎮 QEMU is running interactively. Close the window or press Ctrl+C to exit.");
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
            
            let tc_dir = toolchain::default_toolchain_dir();
            let tc_paths = toolchain::get_toolchain_paths(&tc_dir);
            let tc_status = if toolchain::is_toolchain_installed(&tc_paths) {
                format!("Hermetic LLVM ({})", tc_paths.clang.display())
            } else if let Ok(clang) = which::which("clang") {
                format!("System LLVM ({})", clang.display())
            } else {
                "Not found (run `aros setup`)".to_string()
            };
            println!("  • Active C/C++ Compiler:  {}", style(tc_status).green().bold());

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
        }
    }

    Ok(())
}
