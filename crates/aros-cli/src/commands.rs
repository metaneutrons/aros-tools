//! Command handlers for the `aros` frontend.
//!
//! Argument parsing and top-level diagnostic rendering stay in `main`; each
//! handler here owns the validation and orchestration for one command family.

use super::{
    artifact, board, boot, build, cache, golden, host_compiler, observability, repo, source,
    toolchain, BoardCommand, BoardProfileSelection, BuildCompilerCache, BuildToolsCommand,
    CacheArchivesCommand, CacheCargoCommand, CacheCommand, CacheCompilerBackend,
    CacheCompilerCommand, CacheGenmfCommand, CacheSourcesCommand, Commands, GoldenAction,
    HostCompilerCommands, SdCommand, SourceCommand, ToolchainCommands,
};
use console::{style, Emoji};
use miette::Result;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static SPARKLES: Emoji<'_, '_> = Emoji("✨ ", "");

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
        Commands::Toolchain {
            command: ToolchainCommands::Plan(args),
        } => crate::toolchain_plan::run(args),
        Commands::Toolchain {
            command: ToolchainCommands::Build(args),
        } => crate::toolchain_build::run(args).await,
        Commands::Toolchain {
            command: ToolchainCommands::Producer(args),
        } => crate::toolchain_producer::run(args).await,
        Commands::Toolchain {
            command: ToolchainCommands::MetaMakeFetch(args),
        } => crate::toolchain_fetch_bridge::run(&args),
        Commands::Toolchain {
            command: ToolchainCommands::Inventory(args),
        } => crate::toolchain_management::inventory(args),
        Commands::Toolchain {
            command: ToolchainCommands::Import(args),
        } => crate::toolchain_management::import(args),
        Commands::Toolchain {
            command: ToolchainCommands::Register(args),
        } => crate::toolchain_management::register(args),
        Commands::Toolchain {
            command: ToolchainCommands::Remove(args),
        } => crate::toolchain_lifecycle::remove(args),
        Commands::Toolchain {
            command: ToolchainCommands::Gc(args),
        } => crate::toolchain_lifecycle::gc(args),
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
            compiler_cache,
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
                    compiler_cache: build_compiler_cache(compiler_cache),
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
        Commands::Clean {
            preset,
            all,
            dry_run,
        } => clean(required_repo(repo_root)?, preset, all, dry_run),
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
        Commands::Cache { command } => cache_command(command).await,
        Commands::Ccache => compiler_cache(),
        Commands::Golden { action } => golden_command(action, required_repo(repo_root)?),
        Commands::Completions { shell } => crate::completion_model::write(shell),
        Commands::Info { format } => info(repo_root, format),
    }
}

const fn build_compiler_cache(backend: BuildCompilerCache) -> aros_cache::CompilerBackendChoice {
    match backend {
        BuildCompilerCache::Auto => aros_cache::CompilerBackendChoice::Auto,
        BuildCompilerCache::Off => aros_cache::CompilerBackendChoice::Off,
        BuildCompilerCache::Sccache => aros_cache::CompilerBackendChoice::Sccache,
        BuildCompilerCache::Ccache => aros_cache::CompilerBackendChoice::Ccache,
    }
}

async fn cache_command(command: CacheCommand) -> Result<()> {
    match command {
        CacheCommand::Status { format } => cache::status(format),
        CacheCommand::Compiler {
            command:
                CacheCompilerCommand::Status {
                    backend,
                    dir,
                    format,
                },
        } => cache::compiler_status(cache_backend(backend), dir, format),
        CacheCommand::Sources {
            command: CacheSourcesCommand::Status { dir, format },
        } => cache::source_status(&dir, format),
        CacheCommand::Sources {
            command:
                CacheSourcesCommand::List {
                    selector,
                    dir,
                    format,
                },
        } => cache::source_list(selector, &dir, format),
        CacheCommand::Sources {
            command:
                CacheSourcesCommand::Fetch {
                    selector,
                    dir,
                    offline,
                    allow_unverified,
                    format,
                },
        } => cache::source_fetch(selector, &dir, offline, allow_unverified, format).await,
        CacheCommand::Sources {
            command:
                CacheSourcesCommand::Verify {
                    selector,
                    dir,
                    format,
                },
        } => cache::source_verify(selector, &dir, format),
        CacheCommand::Sources {
            command:
                CacheSourcesCommand::Keep {
                    selector,
                    dir,
                    name,
                    format,
                },
        } => cache::source_keep(selector, &dir, &name, format),
        CacheCommand::Sources {
            command:
                CacheSourcesCommand::Release {
                    dir,
                    name,
                    apply,
                    format,
                },
        } => cache::source_release(&dir, &name, apply.as_deref(), format),
        CacheCommand::Sources {
            command:
                CacheSourcesCommand::Remove {
                    selector,
                    dir,
                    role,
                    apply,
                    format,
                },
        } => cache::source_remove(selector, &dir, &role, apply.as_deref(), format),
        CacheCommand::Archives {
            command: CacheArchivesCommand::Status { format },
        } => cache::archive_status(format),
        CacheCommand::Archives {
            command: CacheArchivesCommand::List { selector, format },
        } => cache::archive_list(selector, format),
        CacheCommand::Archives {
            command:
                CacheArchivesCommand::Fetch {
                    selector,
                    offline,
                    refresh,
                    format,
                },
        } => cache::archive_fetch(selector, offline, refresh, format).await,
        CacheCommand::Archives {
            command: CacheArchivesCommand::Verify { selector, format },
        } => cache::archive_verify(selector, format),
        CacheCommand::Archives {
            command:
                CacheArchivesCommand::Keep {
                    selector,
                    name,
                    format,
                },
        } => cache::archive_keep(selector, &name, format),
        CacheCommand::Archives {
            command:
                CacheArchivesCommand::Release {
                    name,
                    apply,
                    format,
                },
        } => cache::archive_release(&name, apply.as_deref(), format),
        CacheCommand::Archives {
            command:
                CacheArchivesCommand::Remove {
                    selector,
                    apply,
                    format,
                },
        } => cache::archive_remove(selector, apply.as_deref(), format),
        CacheCommand::Cargo {
            command: CacheCargoCommand::Status { dir, format },
        } => cache::cargo_status(&dir, format),
        CacheCommand::Cargo {
            command: CacheCargoCommand::List { selector, format },
        } => cache::cargo_list(selector, format),
        CacheCommand::Cargo {
            command:
                CacheCargoCommand::Fetch {
                    selector,
                    offline,
                    format,
                },
        } => cache::cargo_fetch(selector, offline, format),
        CacheCommand::Cargo {
            command: CacheCargoCommand::Verify { selector, format },
        } => cache::cargo_verify(selector, format),
        CacheCommand::Cargo {
            command:
                CacheCargoCommand::Keep {
                    selector,
                    name,
                    format,
                },
        } => cache::cargo_keep(selector, &name, format),
        CacheCommand::Cargo {
            command:
                CacheCargoCommand::Release {
                    dir,
                    name,
                    apply,
                    format,
                },
        } => cache::cargo_release(&dir, &name, apply.as_deref(), format),
        CacheCommand::Cargo {
            command:
                CacheCargoCommand::Remove {
                    selector,
                    apply,
                    format,
                },
        } => cache::cargo_remove(selector, apply.as_deref(), format),
        CacheCommand::Genmf {
            command: CacheGenmfCommand::Status { dir, format },
        } => cache::genmf_status(&dir, format),
        CacheCommand::Genmf {
            command: CacheGenmfCommand::List { selector, format },
        } => cache::genmf_list(selector, format),
        CacheCommand::Genmf {
            command: CacheGenmfCommand::Verify { selector, format },
        } => cache::genmf_verify(selector, format),
        CacheCommand::Genmf {
            command: CacheGenmfCommand::Refresh { selector, format },
        } => cache::genmf_refresh(selector, format).await,
        CacheCommand::Genmf {
            command:
                CacheGenmfCommand::Keep {
                    selector,
                    name,
                    format,
                },
        } => cache::genmf_keep(selector, &name, format),
        CacheCommand::Genmf {
            command:
                CacheGenmfCommand::Release {
                    dir,
                    name,
                    apply,
                    format,
                },
        } => cache::genmf_release(&dir, &name, apply.as_deref(), format),
        CacheCommand::Genmf {
            command:
                CacheGenmfCommand::Remove {
                    selector,
                    source,
                    apply,
                    format,
                },
        } => cache::genmf_remove(selector, &source, apply.as_deref(), format),
    }
}

const fn cache_backend(backend: CacheCompilerBackend) -> aros_cache::CompilerBackendChoice {
    match backend {
        CacheCompilerBackend::Auto => aros_cache::CompilerBackendChoice::Auto,
        CacheCompilerBackend::Sccache => aros_cache::CompilerBackendChoice::Sccache,
        CacheCompilerBackend::Ccache => aros_cache::CompilerBackendChoice::Ccache,
    }
}

fn install_suite(source_bin: PathBuf, prefix: PathBuf) -> Result<()> {
    let args = aros_release::contract::InstallArgs { source_bin, prefix };
    match aros_release::install::install(&args) {
        Ok(_) => {
            observability::record_committed_mutation();
            Ok(())
        }
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
                let outcome =
                    toolchain::install(repo_root, &profile.name, offline, force, None).await?;
                record_toolchain_install(&outcome);
            }
        }
        (false, Some(preset), local) => {
            let outcome =
                toolchain::install(repo_root, &preset, offline, force, local.as_deref()).await?;
            record_toolchain_install(&outcome);
        }
        (false, None, Some(_)) => miette::bail!("--local requires --preset"),
        (false, None, None) => {
            let outcome = host_compiler::install(repo_root, force, offline).await?;
            record_host_compiler_install(outcome);
        }
    }
    Ok(())
}

async fn host_compiler_command(repo_root: &Path, command: HostCompilerCommands) -> Result<()> {
    match command {
        HostCompilerCommands::Install { force, offline } => {
            let outcome = crate::host_compiler::install(repo_root, force, offline).await?;
            record_host_compiler_install(outcome);
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
        ToolchainCommands::Plan(args) => return crate::toolchain_plan::run(args),
        ToolchainCommands::Build(args) => return crate::toolchain_build::run(args).await,
        ToolchainCommands::Producer(args) => return crate::toolchain_producer::run(args).await,
        ToolchainCommands::MetaMakeFetch(args) => return crate::toolchain_fetch_bridge::run(&args),
        ToolchainCommands::Inventory(args) => crate::toolchain_management::inventory(args)?,
        ToolchainCommands::Import(args) => crate::toolchain_management::import(args)?,
        ToolchainCommands::Register(args) => crate::toolchain_management::register(args)?,
        ToolchainCommands::Select(args) => crate::toolchain_selection::select(repo_root, args)?,
        ToolchainCommands::Remove(args) => crate::toolchain_lifecycle::remove(args)?,
        ToolchainCommands::Gc(args) => crate::toolchain_lifecycle::gc(args)?,
        ToolchainCommands::Install {
            preset,
            force,
            offline,
            local,
        } => {
            let outcome =
                crate::toolchain::install(repo_root, &preset, offline, force, local.as_deref())
                    .await?;
            record_toolchain_install(&outcome);
        }
        ToolchainCommands::List { format } => crate::toolchain::list(repo_root, format)?,
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

fn record_toolchain_install(outcome: &crate::toolchain::ToolchainInstallOutcome) {
    if outcome.publication_committed() {
        observability::record_committed_mutation();
    }
}

fn record_host_compiler_install(outcome: crate::host_compiler::HostCompilerInstallOutcome) {
    if outcome.publication_committed() {
        observability::record_committed_mutation();
    }
}

async fn board_command(command: BoardCommand, repo_root: Option<&Path>) -> Result<()> {
    match command {
        BoardCommand::Init {
            profile,
            model,
            transport,
            config,
            apply,
        } => crate::board::initialize_template(
            config.as_deref(),
            &profile,
            model.into(),
            transport.map(Into::into),
            apply,
        ),
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
            compiler_cache,
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
                    compiler_cache: build_compiler_cache(compiler_cache),
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
            dry_run: _,
        } => {
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
            dry_run: _,
        } => {
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
            upstream_branch,
            transpile,
        } => source::sync(
            required_repo(repo_root)?,
            &upstream,
            &upstream_branch,
            transpile,
        ),
    }
}

fn clean(repo_root: &Path, preset: Option<String>, all: bool, dry_run: bool) -> Result<()> {
    let target_dir = match (preset, all) {
        (Some(preset), false) => build::build_dir(repo_root, &preset)?,
        (None, true) => repo_root.join("build"),
        _ => miette::bail!("select exactly one cleanup scope with --preset NAME or --all"),
    };
    if dry_run {
        aros_common::outputln!("Dry run: would remove directory {}.", target_dir.display());
        return Ok(());
    }
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

fn compiler_cache() -> Result<()> {
    let cache = build::detected_compiler_cache()
        .ok_or_else(|| miette::miette!("neither sccache nor ccache is available on PATH"))?;
    observability::run_command(
        Command::new(cache.program()).arg(build::CompilerCache::stats_argument()),
        "compiler cache statistics query",
    )?;
    Ok(())
}

fn golden_command(action: GoldenAction, repo_root: &Path) -> Result<()> {
    let tools = crate::build_tools::ensure(repo_root)?;
    let transpiler = tools.bin_dir.join(if cfg!(windows) {
        "aros-transpiler.exe"
    } else {
        "aros-transpiler"
    });
    // Golden baselines are an explicit checkout-owned contract, unlike paths
    // supplied by the caller. Keep this root stable when a user invokes the
    // command from a nested checkout directory.
    let build_root = repo_root.join("build");
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

fn info(repo_root: Option<&Path>, format: crate::toolchain_management::ResultFormat) -> Result<()> {
    let report = inspect_info(repo_root)?;
    match format {
        crate::toolchain_management::ResultFormat::Human => print_info_human(&report),
        crate::toolchain_management::ResultFormat::Json => {
            let document = serde_json::to_string_pretty(&report).map_err(|error| {
                miette::miette!("could not serialize environment information: {error}")
            })?;
            aros_common::outputln!("{document}");
        }
    }
    Ok(())
}

fn inspect_info(repo_root: Option<&Path>) -> Result<InfoReport> {
    let checkout = repo_root
        .map(|repo_root| {
            let targets_path = repo::targets_file(repo_root);
            let profiles_from_builtin = !path_entry_exists(&targets_path)?;
            let target_profiles = repo::load_target_profiles(repo_root)?
                .into_iter()
                .map(|target| target.name)
                .collect();
            let lock_path = toolchain::lock_file_path(repo_root);
            let toolchain_lock = if path_entry_exists(&lock_path)? {
                let lock = toolchain::load_lock(repo_root)?;
                Some(ToolchainLockSummary {
                    release_id: lock.release_id,
                    artifact_count: lock.artifacts.len(),
                })
            } else {
                None
            };
            Ok::<_, miette::Report>(InfoCheckout {
                state: CheckoutState::Available,
                root: Some(repo_root.display().to_string()),
                target_profiles,
                target_profile_source: Some(if profiles_from_builtin {
                    TargetProfileSource::BuiltIn
                } else {
                    TargetProfileSource::CheckoutOverride
                }),
                toolchain_lock,
            })
        })
        .transpose()?
        .unwrap_or_else(|| InfoCheckout {
            state: CheckoutState::Unavailable,
            root: None,
            target_profiles: Vec::new(),
            target_profile_source: None,
            toolchain_lock: None,
        });

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
    let (summary, status, path) = if managed_host_entry_exists {
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
                Some(host_paths.clang.display().to_string()),
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
                Some(host_paths.clang.display().to_string()),
            )
        } else {
            (
                format!(
                    "Unverified managed LLVM; no checkout pin available ({})",
                    host_paths.clang.display()
                ),
                InfoStatus::Unverified,
                Some(host_paths.clang.display().to_string()),
            )
        }
    } else if let Ok(clang) = which::which("clang") {
        (
            format!("Unmanaged system LLVM ({})", clang.display()),
            InfoStatus::Unverified,
            Some(clang.display().to_string()),
        )
    } else {
        (
            "Not found (run `aros host-compiler install`)".to_string(),
            InfoStatus::Invalid,
            None,
        )
    };
    let compiler_cache = build::detected_compiler_cache().map(|cache| {
        let path = which::which(cache.program())
            .ok()
            .map(|path| path.display().to_string());
        CompilerCacheInfo {
            program: cache.program(),
            path,
        }
    });
    Ok(InfoReport {
        schema: "aros-info-v1",
        tool_version: env!("CARGO_PKG_VERSION"),
        build_frontend: "CMake + Ninja with explicit target profiles",
        host_compiler: HostCompilerInfo {
            status,
            summary,
            path,
            managed: managed_host_entry_exists,
        },
        state: InfoState {
            root: state_home.display().to_string(),
            archive_cache: archive_cache.display().to_string(),
            cross_toolchain_store: cross_store.display().to_string(),
        },
        cmake_engine: CmakeEngineInfo {
            digest: aros_cmake_engine::digest().to_string(),
            file_count: aros_cmake_engine::file_count(),
            api_version: aros_cmake_engine::api_version(),
        },
        compiler_cache,
        checkout,
    })
}

fn print_info_human(report: &InfoReport) {
    aros_common::outputln!(
        "{SPARKLES} {}",
        style(format!(
            "AROS tools {}: environment information",
            report.tool_version
        ))
        .cyan()
        .bold()
    );
    aros_common::outputln!("  • Build frontend:         {}", report.build_frontend);
    match report.host_compiler.status {
        InfoStatus::Verified => aros_common::outputln!(
            "  • Host C/C++ compiler:    {}",
            style(&report.host_compiler.summary).green().bold()
        ),
        InfoStatus::Unverified => aros_common::outputln!(
            "  • Host C/C++ compiler:    {}",
            style(&report.host_compiler.summary).yellow().bold()
        ),
        InfoStatus::Invalid => {
            aros_common::outputln!(
                "  • Host C/C++ compiler:    {}",
                style(&report.host_compiler.summary).red().bold()
            );
        }
    }
    aros_common::outputln!("  • AROS state root:        {}", report.state.root);
    aros_common::outputln!("  • Archive cache:          {}", report.state.archive_cache);
    aros_common::outputln!(
        "  • Cross-toolchain store:  {}",
        report.state.cross_toolchain_store
    );
    // Which engine a build will use, and its identity. A reader debugging a
    // configure failure needs this before anything else: the modules are not in
    // the checkout any more, so there is nowhere else to look them up.
    aros_common::outputln!(
        "  • CMake engine:           embedded {} ({} files, api {})",
        &report.cmake_engine.digest[..12],
        report.cmake_engine.file_count,
        report.cmake_engine.api_version
    );
    aros_common::outputln!(
        "  • C/C++ Compiler Launcher: {}",
        report.compiler_cache.as_ref().map_or_else(
            || "none".to_string(),
            |cache| cache
                .path
                .clone()
                .unwrap_or_else(|| cache.program.to_string()),
        )
    );
    if let Some(root) = report.checkout.root.as_deref() {
        aros_common::outputln!("  • Source checkout:        {root}");
        let target_source =
            if report.checkout.target_profile_source == Some(TargetProfileSource::BuiltIn) {
                " (built into aros-tools; pristine upstream checkout)"
            } else {
                " (checkout override)"
            };
        aros_common::outputln!(
            "  • Configured targets:     {}{}",
            report.checkout.target_profiles.join(", "),
            target_source
        );
        match &report.checkout.toolchain_lock {
            Some(lock) => aros_common::outputln!(
                "  • AROS toolchain lock:    {} ({} assets)",
                lock.release_id,
                lock.artifact_count
            ),
            None => aros_common::outputln!("  • AROS toolchain lock:    not configured"),
        }
    } else {
        aros_common::outputln!("  • Source checkout:        none discovered");
        aros_common::outputln!(
            "    Create one with `aros source init PATH`, or run inside an existing checkout."
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum InfoStatus {
    Verified,
    Unverified,
    Invalid,
}

#[derive(Serialize)]
struct InfoReport {
    schema: &'static str,
    tool_version: &'static str,
    build_frontend: &'static str,
    host_compiler: HostCompilerInfo,
    state: InfoState,
    cmake_engine: CmakeEngineInfo,
    compiler_cache: Option<CompilerCacheInfo>,
    checkout: InfoCheckout,
}

#[derive(Serialize)]
struct HostCompilerInfo {
    status: InfoStatus,
    summary: String,
    path: Option<String>,
    managed: bool,
}

#[derive(Serialize)]
struct InfoState {
    root: String,
    archive_cache: String,
    cross_toolchain_store: String,
}

#[derive(Serialize)]
struct CmakeEngineInfo {
    digest: String,
    file_count: usize,
    api_version: u32,
}

#[derive(Serialize)]
struct CompilerCacheInfo {
    program: &'static str,
    path: Option<String>,
}

#[derive(Serialize)]
struct InfoCheckout {
    state: CheckoutState,
    root: Option<String>,
    target_profiles: Vec<String>,
    target_profile_source: Option<TargetProfileSource>,
    toolchain_lock: Option<ToolchainLockSummary>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CheckoutState {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum TargetProfileSource {
    BuiltIn,
    CheckoutOverride,
}

#[derive(Serialize)]
struct ToolchainLockSummary {
    release_id: String,
    artifact_count: usize,
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

fn load_board(selection: &BoardProfileSelection) -> Result<board::config::Board> {
    board::config::load_board(selection.config.as_deref(), &selection.profile)
}

#[cfg(test)]
mod tests {
    use super::test;

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
