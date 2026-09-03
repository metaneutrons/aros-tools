//! Command handlers for the `aros` frontend.
//!
//! Argument parsing and top-level diagnostic rendering stay in `main`; each
//! handler here owns the validation and orchestration for one command family.

use super::{
    artifact, board, boot, build, golden, host_compiler, observability, repo, source, style,
    toolchain, BoardCommand, BoardSelection, BuildToolsCommand, Commands, GoldenAction,
    HostCompilerCommands, SdCommand, SourceCommand, ToolchainCommands, CHECK, SPARKLES,
};
use miette::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub async fn run(command: Commands, repo_root: Option<&Path>) -> Result<()> {
    match command {
        Commands::Setup {
            force,
            preset,
            all,
            offline,
            local,
        } => {
            setup(
                required_repo(repo_root)?,
                force,
                preset,
                all,
                offline,
                local,
            )
            .await
        }
        Commands::HostCompiler { command } => {
            host_compiler_command(required_repo(repo_root)?, command).await
        }
        Commands::BuildTools { command } => build_tools_command(command, repo_root),
        Commands::Toolchain { command } => {
            toolchain_command(required_repo(repo_root)?, command).await
        }
        Commands::Board { command } => board_command(command, repo_root).await,
        Commands::Source { command } => source_command(command, repo_root),
        Commands::Install { source_bin, prefix } => install_suite(source_bin, prefix),
        Commands::Build {
            preset,
            target,
            jobs,
            clean,
            verbose,
            offline,
            require_fetch_checksums,
            toolchain_dir,
            debug,
            engine_dir,
        } => {
            build::run(
                required_repo(repo_root)?,
                &build::BuildOptions {
                    toolchain_preset: preset.clone(),
                    preset,
                    target,
                    jobs,
                    clean,
                    verbose,
                    input_policy: build::BuildInputPolicy {
                        offline,
                        require_fetch_checksums,
                    },
                    toolchain_dir,
                    cmake_definitions: Vec::new(),
                    build_type: if debug {
                        build::BuildType::Debug
                    } else {
                        build::BuildType::Release
                    },
                    engine_dir: engine_dir.clone(),
                },
            )
            .await
        }
        Commands::Clean { preset } => clean(required_repo(repo_root)?, preset),
        Commands::Test {
            preset,
            timeout,
            packages,
            modules,
            evidence,
            memory,
        } => test(
            required_repo(repo_root)?,
            &preset,
            timeout,
            packages,
            modules,
            evidence,
            memory,
        ),
        Commands::Ccache { stats, clear } => compiler_cache(stats, clear),
        Commands::Golden { action } => golden_command(action, required_repo(repo_root)?),
        Commands::Info => info(repo_root),
    }
}

fn install_suite(source_bin: PathBuf, prefix: PathBuf) -> Result<()> {
    let args = aros_release::contract::InstallArgs { source_bin, prefix };
    match aros_release::install::install(&args) {
        Ok(_) => Ok(()),
        Err(error) => {
            let state = error
                .diagnostic()
                .context
                .as_ref()
                .and_then(|context| context.commit_state);
            let result = Err(miette::miette!("{error}"));
            match state {
                Some(state) => {
                    observability::commit_state(result, state, "native suite publication state")
                }
                None => result,
            }
        }
    }
}

fn required_repo(repo_root: Option<&Path>) -> Result<&Path> {
    repo_root.ok_or_else(|| {
        miette::miette!(
            "This command requires an AROS source checkout, but repository discovery returned no checkout."
        )
    })
}

async fn setup(
    repo_root: &Path,
    force: bool,
    preset: Option<String>,
    all: bool,
    offline: bool,
    local: Option<PathBuf>,
) -> Result<()> {
    match (all, preset, local) {
        (true, Some(_), _) => miette::bail!("--all cannot be combined with --preset"),
        (true, None, Some(_)) => miette::bail!("--all cannot be combined with --local"),
        (true, None, None) => {
            for profile in repo::load_target_profiles(repo_root)? {
                toolchain::install(repo_root, &profile.name, offline, force, None).await?;
            }
        }
        (false, Some(preset), local) => {
            toolchain::install(repo_root, &preset, offline, force, local.as_deref()).await?;
        }
        (false, None, Some(_)) => miette::bail!("--local requires --preset"),
        (false, None, None) => {
            host_compiler::install(repo_root, force, offline).await?;
        }
    }
    Ok(())
}

async fn host_compiler_command(repo_root: &Path, command: HostCompilerCommands) -> Result<()> {
    match command {
        HostCompilerCommands::Install { force, offline } => {
            crate::host_compiler::install(repo_root, force, offline).await?;
        }
    }
    Ok(())
}

fn build_tools_command(command: BuildToolsCommand, repo_root: Option<&Path>) -> Result<()> {
    match command {
        BuildToolsCommand::Build => crate::build_tools::build(repo_root).map(|_| ()),
        BuildToolsCommand::Check => crate::build_tools::print_check(repo_root),
    }
}

async fn toolchain_command(repo_root: &Path, command: ToolchainCommands) -> Result<()> {
    match command {
        ToolchainCommands::Install {
            preset,
            force,
            offline,
            local,
        } => {
            crate::toolchain::install(repo_root, &preset, offline, force, local.as_deref()).await?;
        }
        ToolchainCommands::List => crate::toolchain::list(repo_root)?,
        ToolchainCommands::Verify { preset, local } => {
            crate::toolchain::verify(repo_root, &preset, local.as_deref())?;
        }
        ToolchainCommands::Path { preset, local } => {
            let resolved = crate::toolchain::path(repo_root, &preset, local.as_deref())?;
            aros_common::outputln!("{}", resolved.paths.root.display());
        }
    }
    Ok(())
}

async fn board_command(command: BoardCommand, repo_root: Option<&Path>) -> Result<()> {
    match command {
        BoardCommand::Init {
            board,
            config,
            apply,
        } => crate::board::initialize_template(config.as_deref(), &board, apply),
        BoardCommand::Scan => crate::board::scan(),
        BoardCommand::Doctor(selection) => {
            let board = load_board(&selection)?;
            crate::board::doctor(&board, required_repo(repo_root)?)
        }
        BoardCommand::Build {
            board: selection,
            target,
            jobs,
            clean,
            verbose,
            offline,
            require_fetch_checksums,
            toolchain_dir,
            dtb_path,
            core_kobj_dir,
            debug,
            engine_dir,
        } => {
            let board = load_board(&selection)?;
            crate::board::build(
                &board,
                required_repo(repo_root)?,
                build::BuildOptions {
                    preset: board.config.preset.clone(),
                    toolchain_preset: board.config.toolchain_preset.clone(),
                    target: target.or_else(|| Some(board.config.build_target.clone())),
                    jobs,
                    clean,
                    verbose,
                    input_policy: build::BuildInputPolicy {
                        offline,
                        require_fetch_checksums,
                    },
                    toolchain_dir,
                    cmake_definitions: Vec::new(),
                    build_type: if debug {
                        build::BuildType::Debug
                    } else {
                        build::BuildType::Release
                    },
                    engine_dir: engine_dir.clone(),
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
            crate::board::deploy(
                &board,
                required_repo(repo_root)?,
                artifact_dir.as_deref(),
                apply,
            )
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

fn source_command(command: SourceCommand, repo_root: Option<&Path>) -> Result<()> {
    match command {
        SourceCommand::Init {
            path,
            upstream,
            fork,
            source_ref,
        } => source::initialize(&source::InitOptions {
            destination: path,
            upstream_url: upstream,
            origin_url: fork,
            source_ref,
        }),
        SourceCommand::Sync {
            upstream,
            upstream_ref,
            transpile,
        } => source::sync(
            required_repo(repo_root)?,
            &upstream,
            &upstream_ref,
            transpile,
        ),
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
    aros_common::outputln!("🧹 Removing directory {}...", target_dir.display());
    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).map_err(|error| {
            miette::miette!(
                "Could not remove build directory '{}': {error}",
                target_dir.display()
            )
        })?;
    }
    aros_common::outputln!("{CHECK} Clean complete.");
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
            let package_directory = build_dir.join(dir);
            let entries = match std::fs::read_dir(&package_directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing_packages.push(dir.to_owned());
                    continue;
                }
                Err(error) => {
                    return Err(miette::miette!(
                        "cannot enumerate boot packages in {}: {error}",
                        package_directory.display()
                    ));
                }
            };
            let mut found = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|error| {
                    miette::miette!(
                        "cannot enumerate an entry in {}: {error}",
                        package_directory.display()
                    )
                })?;
                let path = entry.path();
                if path.extension().is_some_and(|extension| extension == "pkg") {
                    found.push(path);
                }
            }
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
    aros_common::outputln!(
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
    aros_common::output!("{}", boot::render(&report));
    if report.is_success() {
        aros_common::outputln!(
            "{CHECK} {}the boot reached a positive milestone without a failure or exception.",
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
        aros_common::outputln!("{CHECK} Compiler cache cleared.");
    }
    if stats {
        observability::run_command(
            Command::new(cache.program()).arg(build::CompilerCache::stats_argument()),
            "compiler cache statistics query",
        )?;
    }
    Ok(())
}

fn golden_command(action: GoldenAction, repo_root: &Path) -> Result<()> {
    let tools = crate::build_tools::ensure(repo_root)?;
    let transpiler = tools.bin_dir.join(if cfg!(windows) {
        "aros-transpiler.exe"
    } else {
        "aros-transpiler"
    });
    let build_root = PathBuf::from("build");
    let snapshot_root = build_root.join("golden");
    match action {
        GoldenAction::Capture { presets } => {
            for subject in golden::subjects(&build_root, &presets)? {
                let capture = golden::capture(&transpiler, &subject, &snapshot_root)?;
                aros_common::outputln!(
                    "{CHECK} {}: {} products captured to {}",
                    subject.name,
                    capture.products,
                    capture.destination.display()
                );
                match capture.reproduces_build_tree {
                    Some(true) => aros_common::outputln!(
                        "  the recorded invocation reproduces the build tree's own output"
                    ),
                    Some(false) => aros_common::outputln!(
                        "  note: it does not reproduce {} -- that tree may predate a source change; the baseline itself is fine",
                        subject.build_output.display()
                    ),
                    None => aros_common::outputln!(
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
                    aros_common::outputln!(
                        "{CHECK} {}: baseline replaced, {} products",
                        subject.name,
                        capture.products
                    );
                    continue;
                }
                let (comparison, baseline) = golden::verify(&transpiler, subject, &snapshot_root)?;
                if comparison.is_clean() {
                    aros_common::outputln!(
                        "{CHECK} {}: identical to {} ({} products)",
                        subject.name,
                        baseline.display(),
                        comparison.identical
                    );
                } else {
                    aros_common::outputln!(
                        "❌ {}: differs from {}",
                        subject.name,
                        baseline.display()
                    );
                    aros_common::output!("{}", golden::render(&comparison));
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

fn info(repo_root: Option<&Path>) -> Result<()> {
    let repository_configuration = repo_root
        .map(|repo_root| {
            let targets_path = repo::targets_file(repo_root);
            let profiles_from_builtin = !path_entry_exists(&targets_path)?;
            let profiles = repo::load_target_profiles(repo_root)?;
            let lock_path = toolchain::lock_file_path(repo_root);
            let lock = if path_entry_exists(&lock_path)? {
                Some(toolchain::load_lock(repo_root)?)
            } else {
                None
            };
            Ok::<_, miette::Report>((profiles, profiles_from_builtin, lock))
        })
        .transpose()?;

    let state_home = artifact::aros_home()?;
    let archive_cache = artifact::archive_cache_root()?;
    let cross_store = toolchain::default_store_root()?;
    let host_dir = host_compiler::default_host_compiler_dir()?;
    let host_paths = host_compiler::host_compiler_paths(&host_dir);
    let expected_host = repo_root.and_then(|root| {
        host_compiler::load_host_compiler_config(root)
            .ok()
            .and_then(|config| host_compiler::select_host_compiler(&config).ok())
    });
    let managed_host_entry_exists = path_entry_exists(&host_dir)?;
    let (status, status_kind) = if managed_host_entry_exists {
        if expected_host.as_ref().is_some_and(|selection| {
            selection.sha256.as_deref().is_some_and(|digest| {
                host_compiler::verify_host_compiler_install(&host_dir, digest, &selection.version)
                    .is_ok()
            })
        }) {
            (
                format!(
                    "Verified pinned host LLVM inventory and version ({})",
                    host_paths.clang.display()
                ),
                InfoStatus::Verified,
            )
        } else if expected_host
            .as_ref()
            .is_some_and(|selection| selection.sha256.is_some())
        {
            (
                format!(
                    "Invalid managed LLVM inventory, identity, or version ({})",
                    host_paths.clang.display()
                ),
                InfoStatus::Invalid,
            )
        } else {
            (
                format!(
                    "Unverified managed LLVM; no checkout pin available ({})",
                    host_paths.clang.display()
                ),
                InfoStatus::Unverified,
            )
        }
    } else if let Ok(clang) = which::which("clang") {
        (
            format!("Unmanaged system LLVM ({})", clang.display()),
            InfoStatus::Unverified,
        )
    } else {
        (
            "Not found (run `aros host-compiler install`)".to_string(),
            InfoStatus::Invalid,
        )
    };
    aros_common::outputln!(
        "{SPARKLES} {}",
        style(format!(
            "AROS tools {}: environment information",
            env!("CARGO_PKG_VERSION")
        ))
        .cyan()
        .bold()
    );
    aros_common::outputln!(
        "  • Build frontend:         CMake + Ninja with explicit target profiles"
    );
    match status_kind {
        InfoStatus::Verified => aros_common::outputln!(
            "  • Host C/C++ compiler:    {}",
            style(status).green().bold()
        ),
        InfoStatus::Unverified => aros_common::outputln!(
            "  • Host C/C++ compiler:    {}",
            style(status).yellow().bold()
        ),
        InfoStatus::Invalid => {
            aros_common::outputln!("  • Host C/C++ compiler:    {}", style(status).red().bold());
        }
    }
    aros_common::outputln!("  • AROS state root:        {}", state_home.display());
    aros_common::outputln!("  • Archive cache:          {}", archive_cache.display());
    aros_common::outputln!("  • Cross-toolchain store:  {}", cross_store.display());
    // Which engine a build will use, and its identity. A reader debugging a
    // configure failure needs this before anything else: the modules are not in
    // the checkout any more, so there is nowhere else to look them up.
    aros_common::outputln!(
        "  • CMake engine:           embedded {} ({} files, api {})",
        &aros_cmake_engine::digest()[..12],
        aros_cmake_engine::file_count(),
        aros_cmake_engine::api_version()
    );
    aros_common::outputln!(
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
    if let Some((repo_root, (profiles, profiles_from_builtin, lock))) =
        repo_root.zip(repository_configuration)
    {
        aros_common::outputln!("  • Source checkout:        {}", repo_root.display());
        let target_names = profiles
            .into_iter()
            .map(|target| target.name)
            .collect::<Vec<_>>();
        let target_source = if profiles_from_builtin {
            " (built into aros-tools; pristine upstream checkout)"
        } else {
            " (checkout override)"
        };
        aros_common::outputln!(
            "  • Configured targets:     {}{}",
            target_names.join(", "),
            target_source
        );
        match lock {
            Some(lock) => aros_common::outputln!(
                "  • AROS toolchain lock:    {} ({} assets)",
                lock.release_id,
                lock.artifacts.len()
            ),
            None => aros_common::outputln!("  • AROS toolchain lock:    not configured"),
        }
    } else {
        aros_common::outputln!("  • Source checkout:        none discovered");
        aros_common::outputln!(
            "    Create one with `aros source init PATH`, or run inside an existing checkout."
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InfoStatus {
    Verified,
    Unverified,
    Invalid,
}

fn path_entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(miette::miette!(
            "Could not inspect configuration path '{}': {error}",
            path.display()
        )),
    }
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
