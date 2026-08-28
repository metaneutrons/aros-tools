//! Validated CMake configure/build orchestration shared by all build commands.

use crate::{build_tools, toolchain};
use console::{style, Emoji};
use miette::Result;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static HAMMER: Emoji<'_, '_> = Emoji("🔨 ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");

/// Validated inputs for one configure-and-build transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOptions {
    /// CMake preset naming the build tree and configuration.
    pub preset: String,
    /// The locked AROS cross-toolchain profile. This is intentionally distinct
    /// from the CMake preset: a board-specific debug preset can share the
    /// audited `rpi-aarch64` target toolchain.
    pub toolchain_preset: String,
    /// Optional CMake target; absence builds the preset default graph.
    pub target: Option<String>,
    /// Optional parallel-job limit passed to the build tool.
    pub jobs: Option<usize>,
    /// Remove the validated preset build tree before configuring.
    pub clean: bool,
    /// Request verbose CMake configuration diagnostics.
    pub verbose: bool,
    /// Network and integrity policy applied to every build input.
    pub input_policy: BuildInputPolicy,
    /// Explicit local cross-toolchain override.
    pub toolchain_dir: Option<PathBuf>,
    /// Additional strictly named CMake cache definitions.
    pub cmake_definitions: Vec<CmakeDefinition>,
}

/// Acquisition and integrity policy shared by toolchains and port sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildInputPolicy {
    /// Prohibit network access and require installed, cached, or local inputs.
    pub offline: bool,
    /// Reject `%fetch` archives without source-declared SHA-256 values.
    pub require_fetch_checksums: bool,
}

/// One validated `-DKEY=VALUE` CMake cache definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmakeDefinition {
    /// CMake cache-variable name.
    pub key: String,
    /// Non-empty value passed without shell interpretation.
    pub value: String,
}

/// Resolve tools and execute one complete CMake configure/build transaction.
///
/// # Errors
///
/// Returns an error for invalid options, missing toolchains or build tools,
/// configuration failures, and compilation failures.
pub async fn run(repo_root: &Path, options: &BuildOptions) -> Result<()> {
    let build_dir = build_dir(repo_root, &options.preset)?;
    let profile = toolchain::target_profile(&options.toolchain_preset)?;
    let resolved = toolchain::resolve_for_build(
        &options.toolchain_preset,
        options.toolchain_dir.as_deref(),
        options.input_policy.offline,
    )
    .await?;
    build_tools::ensure(repo_root)?;

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

    let launcher = detected_compiler_cache().map_or("none", CompilerCache::program);
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
    configure.arg(format!(
        "-DAROS_FETCH_OFFLINE={}",
        if options.input_policy.offline {
            "ON"
        } else {
            "OFF"
        }
    ));
    configure.arg(format!(
        "-DAROS_FETCH_REQUIRE_CHECKSUMS={}",
        if options.input_policy.require_fetch_checksums {
            "ON"
        } else {
            "OFF"
        }
    ));
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

/// Return a preset's validated build directory below the checkout.
///
/// # Errors
///
/// Returns an error when `preset` could introduce a path component.
pub fn build_dir(repo_root: &Path, preset: &str) -> Result<PathBuf> {
    validate_preset(preset)?;
    Ok(repo_root.join("build").join(preset))
}

/// Require a portable preset identifier without path syntax.
///
/// # Errors
///
/// Returns an error for empty names or characters outside the allowed set.
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

/// Validate one CMake definition name and non-empty value.
///
/// # Errors
///
/// Returns an error when the key is not a CMake identifier or the value is
/// empty.
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

/// Supported compiler-cache implementation with its command-line contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompilerCache {
    /// Mozilla's distributed/local compiler cache, preferred when available.
    Sccache,
    /// Traditional local compiler cache fallback.
    Ccache,
}

impl CompilerCache {
    /// Executable name used both as compiler launcher and management command.
    pub const fn program(self) -> &'static str {
        match self {
            Self::Sccache => "sccache",
            Self::Ccache => "ccache",
        }
    }

    /// Argument which clears the selected cache.
    pub const fn clear_argument(self) -> &'static str {
        match self {
            Self::Sccache => "-z",
            Self::Ccache => "-C",
        }
    }

    /// Argument which prints cache statistics.
    pub const fn stats_argument() -> &'static str {
        "-s"
    }
}

/// Select the preferred available compiler cache once for all CLI commands.
pub fn detected_compiler_cache() -> Option<CompilerCache> {
    if which::which("sccache").is_ok() {
        Some(CompilerCache::Sccache)
    } else if which::which("ccache").is_ok() {
        Some(CompilerCache::Ccache)
    } else {
        None
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
