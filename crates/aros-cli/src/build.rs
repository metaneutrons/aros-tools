use crate::{hosttools, toolchain};
use console::{style, Emoji};
use miette::Result;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static HAMMER: Emoji<'_, '_> = Emoji("🔨 ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOptions {
    pub preset: String,
    pub target: Option<String>,
    pub jobs: Option<usize>,
    pub clean: bool,
    pub verbose: bool,
    pub cmake_definitions: Vec<CmakeDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmakeDefinition {
    pub key: String,
    pub value: String,
}

pub async fn run(repo_root: &Path, options: &BuildOptions) -> Result<()> {
    let build_dir = build_dir(repo_root, &options.preset)?;
    ensure_toolchain(repo_root).await?;
    hosttools::ensure(repo_root)?;

    println!(
        "{ROCKET} {}Building AROS for target preset [{}]...",
        style("AROS-NG: ").cyan().bold(),
        style(&options.preset).yellow().bold()
    );
    let start = Instant::now();

    if options.clean {
        println!("🧹 Cleaning build directory for {}...", options.preset);
        if build_dir.exists() {
            std::fs::remove_dir_all(&build_dir).map_err(|error| {
                miette::miette!(
                    "Could not remove build directory '{}': {error}",
                    build_dir.display()
                )
            })?;
        }
    }

    let launcher = compiler_cache_launcher();
    println!(
        "⚡ Compiler cache launcher: {}",
        style(launcher).green().bold()
    );

    println!("{HAMMER} Configuring CMake build tree...");
    let mut configure = Command::new("cmake");
    configure
        .current_dir(repo_root)
        .args(["--preset", &options.preset]);
    for definition in &options.cmake_definitions {
        validate_cmake_definition(definition)?;
        configure.arg(format!("-D{}={}", definition.key, definition.value));
    }
    if options.verbose {
        configure.arg("--log-level=VERBOSE");
    }
    run_command(
        &mut configure,
        &format!("CMake configure for preset '{}'", options.preset),
    )?;

    println!("{HAMMER} Compiling AROS modules with Ninja...");
    let mut build = Command::new("cmake");
    build.current_dir(repo_root).args(["--build"]);
    build.arg(&build_dir);
    if let Some(target) = &options.target {
        build.args(["--target", target]);
    }
    if let Some(jobs) = options.jobs {
        build.args(["-j", &jobs.to_string()]);
    }
    run_command(
        &mut build,
        &format!("CMake build for preset '{}'", options.preset),
    )?;

    println!(
        "{CHECK} {}Build completed successfully in {:.2?}!",
        style("SUCCESS: ").green().bold(),
        start.elapsed()
    );
    Ok(())
}

pub async fn ensure_toolchain(repo_root: &Path) -> Result<()> {
    let paths = toolchain::get_toolchain_paths(&toolchain::default_toolchain_dir());
    if !toolchain::is_toolchain_installed(&paths) && which::which("clang").is_err() {
        println!("ℹ️ Hermetic toolchain not found. Initializing automatic setup...");
        toolchain::setup_toolchain_at(&repo_root.join("aros-targets.toml"), false)
            .await
            .map_err(|error| miette::miette!("{error}"))?;
    }
    Ok(())
}

pub fn build_dir(repo_root: &Path, preset: &str) -> Result<PathBuf> {
    validate_preset(preset)?;
    Ok(repo_root.join("build").join(preset))
}

pub fn validate_preset(preset: &str) -> Result<()> {
    let valid = !preset.is_empty()
        && preset
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if !valid {
        miette::bail!(
            "Invalid CMake preset '{preset}'. Preset names may contain only ASCII letters, digits, '-' and '_'."
        );
    }
    Ok(())
}

pub fn validate_cmake_definition(definition: &CmakeDefinition) -> Result<()> {
    let mut characters = definition.key.chars();
    let Some(first) = characters.next() else {
        miette::bail!("CMake definition names must not be empty.");
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        miette::bail!(
            "Invalid CMake definition name '{}'. Names may contain only ASCII letters, digits and '_' and cannot start with a digit.",
            definition.key
        );
    }
    if definition.value.is_empty() {
        miette::bail!(
            "CMake definition '{}' must not have an empty value.",
            definition.key
        );
    }
    Ok(())
}

fn compiler_cache_launcher() -> &'static str {
    if which::which("sccache").is_ok() {
        "sccache"
    } else if which::which("ccache").is_ok() {
        "ccache"
    } else {
        "none"
    }
}

fn run_command(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .map_err(|error| miette::miette!("Could not start {description}: {error}"))?;
    if !status.success() {
        miette::bail!("{description} failed.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_dir, validate_cmake_definition, validate_preset, CmakeDefinition};

    #[test]
    fn build_directory_stays_inside_the_checkout() {
        let root = std::path::Path::new("/checkout");
        assert_eq!(
            build_dir(root, "rpi-aarch64").expect("valid preset"),
            root.join("build/rpi-aarch64")
        );
    }

    #[test]
    fn preset_rejects_path_components() {
        assert!(validate_preset("../other").is_err());
        assert!(validate_preset("rpi/aarch64").is_err());
        assert!(validate_preset("").is_err());
    }

    #[test]
    fn cmake_definition_rejects_an_unsafe_name() {
        let definition = CmakeDefinition {
            key: "AROS_RPI4_DTB;OTHER".to_string(),
            value: "/tmp/board.dtb".to_string(),
        };
        assert!(validate_cmake_definition(&definition).is_err());
    }
}
