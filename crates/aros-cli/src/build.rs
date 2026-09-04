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
    /// `Debug` or `Release`; the presets carried this per build tree.
    pub build_type: BuildType,
    /// An explicitly nominated CMake engine, replacing the embedded one.
    ///
    /// Only ever set from an explicit request. An engine found lying in the
    /// checkout is never preferred on its own: a stale copy silently outranking
    /// the current one is a failure this project has already paid for once.
    pub engine_dir: Option<PathBuf>,
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

/// Optimisation and assertion policy for one build tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuildType {
    /// Optimised, assertions off. What every product build uses.
    #[default]
    Release,
    /// Unoptimised with debug information, for board bring-up.
    Debug,
}

impl BuildType {
    /// The `CMAKE_BUILD_TYPE` value.
    #[must_use]
    pub const fn cmake_value(self) -> &'static str {
        match self {
            Self::Release => "Release",
            Self::Debug => "Debug",
        }
    }
}

/// The cache variables a CMake preset used to carry.
///
/// They are derived rather than named because none of them is a free choice:
/// the system name is fixed for a bare-metal target, the processor is the
/// profile's architecture, the compilers are the LLVM path the target graph
/// assumes, and the bootloader follows the platform. Deriving them is what
/// removes the last reason for a checkout to carry `CMakePresets.json`, and it
/// is what lets a tree without one be built from built-in profiles.
fn profile_cache_variables(
    profile: &aros_common::TargetProfile,
    options: &BuildOptions,
) -> Vec<(String, String)> {
    vec![
        ("CMAKE_SYSTEM_NAME".to_owned(), "Generic".to_owned()),
        (
            "CMAKE_SYSTEM_PROCESSOR".to_owned(),
            profile.arch.to_string(),
        ),
        ("CMAKE_C_COMPILER".to_owned(), "clang".to_owned()),
        ("CMAKE_CXX_COMPILER".to_owned(), "clang++".to_owned()),
        ("CMAKE_ASM_COMPILER".to_owned(), "clang".to_owned()),
        ("AROS_TOOLCHAIN".to_owned(), "llvm".to_owned()),
        (
            "AROS_TARGET_BOOTLOADER".to_owned(),
            profile.bootloader().to_owned(),
        ),
        (
            "CMAKE_BUILD_TYPE".to_owned(),
            options.build_type.cmake_value().to_owned(),
        ),
        ("CMAKE_EXPORT_COMPILE_COMMANDS".to_owned(), "ON".to_owned()),
    ]
}

/// Puts the CMake engine where this build will read it from.
///
/// The embedded engine is placed inside the build tree, so nothing is written
/// into the checkout and a pristine upstream tree stays pristine. An explicit
/// override replaces it wholesale and is reported, because a build running
/// against modules other than the ones this binary was built with is exactly
/// the thing a reader needs told.
///
/// # Errors
///
/// When the engine cannot be written, or a nominated directory does not hold
/// one.
fn place_engine(build_dir: &Path, override_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(directory) = override_dir {
        let root = directory.canonicalize().map_err(|error| {
            miette::miette!(
                "Could not resolve --engine-dir '{}': {error}",
                directory.display()
            )
        })?;
        if !root.join("AROS.cmake").is_file() {
            miette::bail!(
                "No CMake engine at '{}': AROS.cmake is missing.",
                root.display()
            );
        }
        aros_common::outputln!("🔧 CMake engine: {} (explicit override)", root.display());
        return Ok(root);
    }

    let root = build_dir.join(ENGINE_SUBDIRECTORY);
    let placement = aros_cmake_engine::materialize(&root).map_err(|error| {
        miette::miette!(
            "Could not place the CMake engine in '{}': {error}",
            root.display()
        )
    })?;
    aros_common::outputln!(
        "🔧 CMake engine: embedded {} (api {})",
        &placement.digest[..12],
        aros_cmake_engine::api_version()
    );
    Ok(placement.root)
}

/// Directory inside a build tree that holds the placed engine.
const ENGINE_SUBDIRECTORY: &str = "cmake-engine";

/// Resolve tools and execute one complete CMake configure/build transaction.
///
/// # Errors
///
/// Returns an error for invalid options, missing toolchains or build tools,
/// configuration failures, and compilation failures.
pub async fn run(repo_root: &Path, options: &BuildOptions) -> Result<()> {
    if options.jobs == Some(0) {
        miette::bail!("parallel job count must be greater than zero");
    }
    let build_dir = build_dir(repo_root, &options.preset)?;
    let profile = toolchain::target_profile(repo_root, &options.toolchain_preset)?;
    let resolved = toolchain::resolve_for_build(
        repo_root,
        &options.toolchain_preset,
        options.toolchain_dir.as_deref(),
        options.input_policy.offline,
    )
    .await?;
    let build_tools = build_tools::ensure(repo_root)?;

    aros_common::outputln!(
        "{ROCKET} {}Building AROS for target preset [{}]...",
        style("AROS: ").cyan().bold(),
        style(&options.preset).yellow().bold()
    );
    aros_common::outputln!(
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
        aros_common::outputln!("🧹 Cleaning build directory for {}...", options.preset);
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
    aros_common::outputln!(
        "⚡ Compiler cache launcher: {}",
        style(launcher).green().bold()
    );

    aros_common::outputln!("{HAMMER} Configuring CMake build tree...");
    let engine = place_engine(&build_dir, options.engine_dir.as_deref())?;
    let cmake_toolchain = engine.join("toolchains/AROS.cmake");
    if !cmake_toolchain.is_file() {
        miette::bail!(
            "Required CMake toolchain file is missing: {}",
            cmake_toolchain.display()
        );
    }
    let mut configure = Command::new("cmake");
    // The engine is the project and the checkout is an input, which is what
    // lets a tree that does not carry a build system be built at all. A preset
    // cannot express this: it fixes the binary directory relative to its own
    // source directory and refuses an explicit -B.
    configure
        .current_dir(repo_root)
        .arg("-S")
        .arg(&engine)
        .arg("-B")
        .arg(&build_dir)
        .args(["-G", "Ninja"]);
    configure.arg(format!("-DAROS_SOURCE_DIR={}", repo_root.display()));
    for (key, value) in profile_cache_variables(&profile, options) {
        configure.arg(format!("-D{key}={value}"));
    }
    configure.arg(format!(
        "-DCMAKE_TOOLCHAIN_FILE={}",
        cmake_toolchain.display()
    ));
    configure.arg(format!(
        "-DAROS_CROSS_TOOLCHAIN_ROOT={}",
        resolved.paths.root.display()
    ));
    configure.arg(format!(
        "-DAROS_RUST_TOOLS_DIR={}",
        build_tools.bin_dir.display()
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

    aros_common::outputln!("{HAMMER} Compiling AROS modules with Ninja...");
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

    aros_common::outputln!(
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
    use super::{
        build_dir, run, validate_cmake_definition, validate_preset, BuildInputPolicy, BuildOptions,
        CmakeDefinition,
    };

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

    #[tokio::test]
    async fn runtime_contract_rejects_zero_jobs_before_repository_access() {
        let options = BuildOptions {
            preset: "pc-x86_64".into(),
            toolchain_preset: "pc-x86_64".into(),
            target: None,
            jobs: Some(0),
            clean: false,
            verbose: false,
            input_policy: BuildInputPolicy {
                offline: true,
                require_fetch_checksums: true,
            },
            toolchain_dir: None,
            cmake_definitions: Vec::new(),
            build_type: super::BuildType::Release,
            engine_dir: None,
        };
        let checkout = tempfile::tempdir().unwrap();
        assert!(run(checkout.path(), &options)
            .await
            .unwrap_err()
            .to_string()
            .contains("greater than zero"));
    }
}
