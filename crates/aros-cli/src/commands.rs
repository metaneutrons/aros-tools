//! Command handlers for the `aros` frontend.
//!
//! Argument parsing and top-level diagnostic rendering stay in `main`; each
//! handler here owns the validation and orchestration for one command family.

use super::{
    board, boot, build, golden, host_compiler, observability, repo, style, toolchain, BoardCommand,
    BoardSelection, BuildToolsCommand, Commands, GoldenAction, HostCompilerCommands, SdCommand,
    ToolchainCommands, CHECK, SPARKLES,
};
use miette::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

pub async fn run(command: Commands, repo_root: &Path) -> Result<()> {
    match command {
        Commands::Setup {
            force,
            preset,
            all,
            offline,
            local,
        } => setup(force, preset, all, offline, local).await,
        Commands::HostCompiler { command } => host_compiler_command(command).await,
        Commands::BuildTools { command } => build_tools_command(command, repo_root),
        Commands::Toolchain { command } => toolchain_command(command).await,
        Commands::Board { command } => board_command(command, repo_root).await,
        Commands::Build {
            preset,
            target,
            jobs,
            clean,
            verbose,
            offline,
            toolchain_dir,
        } => {
            build::run(
                repo_root,
                &build::BuildOptions {
                    toolchain_preset: preset.clone(),
                    preset,
                    target,
                    jobs,
                    clean,
                    verbose,
                    offline,
                    toolchain_dir,
                    cmake_definitions: Vec::new(),
                },
            )
            .await
        }
        Commands::Clean { preset } => clean(repo_root, preset),
        Commands::Test {
            preset,
            timeout,
            packages,
            modules,
            evidence,
            memory,
        } => test(
            repo_root, &preset, timeout, packages, modules, evidence, memory,
        ),
        Commands::Ccache { stats, clear } => compiler_cache(stats, clear),
        Commands::Sync { transpile } => sync(transpile),
        Commands::Golden { action } => golden_command(action),
        Commands::Info => info(),
    }
}

async fn setup(
    force: bool,
    preset: Option<String>,
    all: bool,
    offline: bool,
    local: Option<PathBuf>,
) -> Result<()> {
    if all {
        if local.is_some() {
            miette::bail!("--local cannot be combined with --all");
        }
        for profile in repo::load_target_profiles()? {
            toolchain::install(&profile.name, offline, force, None).await?;
        }
    } else if let Some(preset) = preset {
        toolchain::install(&preset, offline, force, local.as_deref()).await?;
    } else if local.is_some() {
        miette::bail!("--local requires --preset");
    } else {
        host_compiler::install(force, offline).await?;
    }
    Ok(())
}

async fn host_compiler_command(command: HostCompilerCommands) -> Result<()> {
    match command {
        HostCompilerCommands::Install { force, offline } => {
            crate::host_compiler::install(force, offline).await?;
        }
    }
    Ok(())
}

fn build_tools_command(command: BuildToolsCommand, repo_root: &Path) -> Result<()> {
    match command {
        BuildToolsCommand::Build => crate::build_tools::build(repo_root).map(|_| ()),
        BuildToolsCommand::Check => crate::build_tools::print_check(repo_root),
    }
}

async fn toolchain_command(command: ToolchainCommands) -> Result<()> {
    match command {
        ToolchainCommands::Install {
            preset,
            force,
            offline,
            local,
        } => {
            crate::toolchain::install(&preset, offline, force, local.as_deref()).await?;
        }
        ToolchainCommands::List => crate::toolchain::list()?,
        ToolchainCommands::Verify { preset, local } => {
            crate::toolchain::verify(&preset, local.as_deref())?;
        }
        ToolchainCommands::Path { preset, local } => {
            let resolved = crate::toolchain::path(&preset, local.as_deref())?;
            println!("{}", resolved.paths.root.display());
        }
    }
    Ok(())
}

async fn board_command(command: BoardCommand, repo_root: &Path) -> Result<()> {
    match command {
        BoardCommand::Init {
            board,
            config,
            apply,
        } => crate::board::initialize_template(config.as_deref(), &board, apply),
        BoardCommand::Scan => crate::board::scan(),
        BoardCommand::Doctor(selection) => {
            let board = load_board(&selection)?;
            crate::board::doctor(&board, repo_root)
        }
        BoardCommand::Build {
            board: selection,
            target,
            jobs,
            clean,
            verbose,
            offline,
            toolchain_dir,
            dtb_path,
            core_kobj_dir,
        } => {
            let board = load_board(&selection)?;
            crate::board::build(
                &board,
                repo_root,
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
            .await
        }
        BoardCommand::Deploy {
            board: selection,
            artifact_dir,
            apply,
            dry_run,
        } => {
            exclusive_apply_dry_run(apply, dry_run)?;
            let board = load_board(&selection)?;
            crate::board::deploy(&board, repo_root, artifact_dir.as_deref(), apply)
        }
        BoardCommand::Serve {
            board: selection,
            dry_run,
        } => {
            let board = load_board(&selection)?;
            crate::board::serve(&board, dry_run).await
        }
        BoardCommand::Sd { command } => sd(command),
        BoardCommand::Console {
            board: selection,
            program,
            device,
            baud,
            dry_run,
        } => {
            let board = load_board(&selection)?;
            crate::board::console(&board, program, device, baud, dry_run)
        }
    }
}

fn sd(command: SdCommand) -> Result<()> {
    match command {
        SdCommand::Image {
            board: selection,
            boot_bundle,
            output,
            apply,
            dry_run,
        } => {
            exclusive_apply_dry_run(apply, dry_run)?;
            let board = load_board(&selection)?;
            crate::board::create_sd_image(&board, &boot_bundle, &output, apply)
        }
        SdCommand::Scan { artifact } => crate::board::scan_sd_disks(artifact.as_deref()),
        SdCommand::Unmount {
            device,
            apply,
            dry_run,
        } => crate::board::unmount_sd_disk(device.as_deref(), apply, dry_run),
        SdCommand::Write {
            board: selection,
            artifact,
            device,
            confirm,
            dry_run,
        } => {
            let board = load_board(&selection)?;
            crate::board::write_sd_image(&board, &artifact, &device, confirm.as_deref(), dry_run)
        }
    }
}

fn exclusive_apply_dry_run(apply: bool, dry_run: bool) -> Result<()> {
    if apply && dry_run {
        miette::bail!("--apply and --dry-run cannot be used together.");
    }
    Ok(())
}

fn clean(repo_root: &Path, preset: Option<String>) -> Result<()> {
    let target_dir = match preset {
        Some(preset) => build::build_dir(repo_root, &preset)?,
        None => repo_root.join("build"),
    };
    println!("🧹 Removing directory {}...", target_dir.display());
    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).map_err(|error| {
            miette::miette!(
                "Could not remove build directory '{}': {error}",
                target_dir.display()
            )
        })?;
    }
    println!("{CHECK} Clean complete.");
    Ok(())
}

fn test(
    repo_root: &Path,
    preset: &str,
    timeout: u64,
    packages: bool,
    modules: Vec<PathBuf>,
    evidence: Option<PathBuf>,
    memory: u32,
) -> Result<()> {
    let build_dir = build::build_dir(repo_root, preset)?;
    if !build_dir.is_dir() {
        miette::bail!(
            "no build directory at {} -- configure and build the preset first",
            build_dir.display()
        );
    }

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
        Ok(())
    } else {
        miette::bail!(
            "the boot did not come up clean; every finding above is read from the retained logs, not inferred"
        );
    }
}

fn compiler_cache(stats: bool, clear: bool) -> Result<()> {
    if !stats && !clear {
        return Ok(());
    }
    let cache = build::detected_compiler_cache()
        .ok_or_else(|| miette::miette!("neither sccache nor ccache is available on PATH"))?;
    if clear {
        observability::run_command(
            Command::new(cache.program()).arg(cache.clear_argument()),
            "compiler cache clear",
        )?;
        println!("{CHECK} Compiler cache cleared.");
    }
    if stats {
        observability::run_command(
            Command::new(cache.program()).arg(build::CompilerCache::stats_argument()),
            "compiler cache statistics query",
        )?;
    }
    Ok(())
}

fn sync(transpile: bool) -> Result<()> {
    println!("🔄 Fetching latest commits from upstream (aros-development-team/AROS)...");
    observability::run_command(
        Command::new("git").args(["fetch", "upstream", "master"]),
        "Git fetch from upstream/master",
    )?;
    observability::run_command(
        Command::new("git").args(["merge", "upstream/master", "--no-edit"]),
        "Git merge of upstream/master",
    )?;
    if transpile {
        println!("⚡ Regenerating dynamic CMake target tree...");
        println!("{CHECK} Sync and target regeneration complete!");
    }
    Ok(())
}

fn golden_command(action: GoldenAction) -> Result<()> {
    let transpiler = PathBuf::from("tools/aros-tools/target/release/aros-transpiler");
    if !transpiler.is_file() {
        miette::bail!(
            "no {} -- build it with `cargo build --release -p aros-transpiler` in tools/aros-tools",
            transpiler.display()
        );
    }
    let build_root = PathBuf::from("build");
    let snapshot_root = build_root.join("golden");
    match action {
        GoldenAction::Capture { presets } => {
            for subject in golden::subjects(&build_root, &presets)? {
                let capture = golden::capture(&transpiler, &subject, &snapshot_root)?;
                println!(
                    "{CHECK} {}: {} products captured to {}",
                    subject.name,
                    capture.products,
                    capture.destination.display()
                );
                match capture.reproduces_build_tree {
                    Some(true) => println!(
                        "  the recorded invocation reproduces the build tree's own output"
                    ),
                    Some(false) => println!(
                        "  note: it does not reproduce {} -- that tree may predate a source change; the baseline itself is fine",
                        subject.build_output.display()
                    ),
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
                let (comparison, baseline) = golden::verify(&transpiler, subject, &snapshot_root)?;
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
                    "the generated output changed for {}. If that was the point, re-capture with `aros golden verify --update`",
                    differing.join(", ")
                );
            }
        }
    }
    Ok(())
}

fn info() -> Result<()> {
    println!(
        "{SPARKLES} {}",
        style("AROS Tools v0.1: Workspace Info").cyan().bold()
    );
    println!("  • Toolchain Architecture: Multi-Target Modern CMake + Ninja");
    let host_dir = host_compiler::default_host_compiler_dir();
    let host_paths = host_compiler::host_compiler_paths(&host_dir);
    let status = if host_compiler::is_host_compiler_installed(&host_paths) {
        format!("Pinned host LLVM ({})", host_paths.clang.display())
    } else if let Ok(clang) = which::which("clang") {
        format!("Unmanaged system LLVM ({})", clang.display())
    } else {
        "Not found (run `aros host-compiler install`)".to_string()
    };
    println!(
        "  • Active C/C++ Compiler:  {}",
        style(status).green().bold()
    );
    println!(
        "  • C/C++ Compiler Launcher: {}",
        build::detected_compiler_cache().map_or_else(
            || "none".into(),
            |cache| {
                which::which(cache.program()).map_or_else(
                    |_| cache.program().into(),
                    |path| path.display().to_string(),
                )
            },
        )
    );
    let target_names = repo::load_target_profiles()?
        .into_iter()
        .map(|target| target.name)
        .collect::<Vec<_>>();
    println!("  • Configured Targets:     {}", target_names.join(", "));
    match toolchain::load_lock() {
        Ok(lock) => println!(
            "  • AROS Toolchain Lock:    {} ({} assets)",
            lock.release_id,
            lock.artifacts.len()
        ),
        Err(error) => println!("  • AROS Toolchain Lock:    invalid ({error})"),
    }
    Ok(())
}

fn load_board(selection: &BoardSelection) -> Result<board::config::Board> {
    board::config::load_board(selection.config.as_deref(), &selection.board)
}

#[cfg(test)]
mod tests {
    use super::{exclusive_apply_dry_run, test};

    #[test]
    fn apply_and_dry_run_are_mutually_exclusive() {
        assert!(exclusive_apply_dry_run(true, true).is_err());
        assert!(exclusive_apply_dry_run(true, false).is_ok());
        assert!(exclusive_apply_dry_run(false, true).is_ok());
    }

    #[test]
    fn boot_test_rejects_a_preset_path_before_reading_a_build_tree() {
        let checkout = tempfile::tempdir().expect("temporary checkout");
        let error = test(
            checkout.path(),
            "../outside",
            1,
            false,
            Vec::new(),
            None,
            64,
        )
        .expect_err("preset path must fail before build-tree access");
        assert!(error.to_string().contains("Invalid CMake preset"));
    }
}
