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
    /// The locked AROS cross-toolchain profile. This is intentionally distinct
    /// from the CMake preset: a board-specific debug preset can share the
    /// audited `rpi-aarch64` target toolchain.
    pub toolchain_preset: String,
    pub target: Option<String>,
    pub jobs: Option<usize>,
    pub clean: bool,
    pub verbose: bool,
    pub offline: bool,
    pub toolchain_dir: Option<PathBuf>,
    pub cmake_definitions: Vec<CmakeDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmakeDefinition {
    pub key: String,
    pub value: String,
}

pub async fn run(repo_root: &Path, options: &BuildOptions) -> Result<()> {
    let build_dir = build_dir(repo_root, &options.preset)?;
    let profile = toolchain::target_profile(&options.toolchain_preset)
        .map_err(|error| miette::miette!("{error:#}"))?;
    let resolved = toolchain::resolve_for_build(
        &options.toolchain_preset,
        options.toolchain_dir.as_deref(),
        options.offline,
    )
    .await
    .map_err(|error| miette::miette!("{error:#}"))?;
    hosttools::ensure(repo_root)?;

    println!(
        "{ROCKET} {}Building AROS for target preset [{}]...",
        style("AROS-NG: ").cyan().bold(),
        style(&options.preset).yellow().bold()
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
    let cmake_toolchain = repo_root.join("cmake/toolchains/AROS.cmake");
    if !cmake_toolchain.is_file() {
        miette::bail!(
            "Required CMake toolchain file is missing: {}",
            cmake_toolchain.display()
        );
    }
    let mut configure = Command::new("cmake");
    configure
        .current_dir(repo_root)
        .args(["--preset", &options.preset]);
    configure.arg(format!(
        "-DCMAKE_TOOLCHAIN_FILE={}",
        cmake_toolchain.display()
    ));
    configure.arg(format!(
        "-DAROS_CROSS_TOOLCHAIN_ROOT={}",
        resolved.paths.root.display()
    ));
    configure.arg(format!("-DAROS_TARGET_CPU={}", profile.arch));
    configure.arg(format!("-DAROS_TARGET_PLATFORM={}", profile.platform));
    configure.arg(format!("-DAROS_TARGET_PROFILE={}", profile.name));
    configure.arg(format!("-DAROS_TARGET_TRIPLE={}", resolved.target_triple));
    if let Some(float_abi) = &profile.float_abi {
        configure.arg(format!("-DGCC_CONFIG_FLOAT_ABI={float_abi}"));
    }
    for definition in &options.cmake_definitions {
        validate_cmake_definition(definition)?;
        configure.arg(format!("-D{}={}", definition.key, definition.value));
    }
    if options.verbose {
        configure.arg("--log-level=VERBOSE");
    }
    crate::observability::run_command_at(
        &mut configure,
        &format!("CMake configure for preset '{}'", options.preset),
        crate::observability::ErrorBoundary {
            code: aros_common::DiagnosticCode::CliConfigure,
            stage: aros_common::DiagnosticStage::BuildConfiguration,
            hint: "inspect the bounded CMake output and repair the selected preset or configure contract",
        },
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
    crate::observability::run_command_at(
        &mut build,
        &format!("CMake build for preset '{}'", options.preset),
        crate::observability::ErrorBoundary {
            code: aros_common::DiagnosticCode::CliBuild,
            stage: aros_common::DiagnosticStage::BuildExecution,
            hint: "inspect the bounded CMake output and retry the exact reported build target",
        },
    )?;

    println!(
        "{CHECK} {}Build completed successfully in {:.2?}!",
        style("SUCCESS: ").green().bold(),
        start.elapsed()
    );
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
