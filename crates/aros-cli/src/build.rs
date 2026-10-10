//! Validated CMake configure/build orchestration shared by all build commands.

use crate::{build_tools, toolchain};
use aros_common::local_toolchain::{LocalToolchainDescriptor, LOCAL_TOOLCHAIN_DESCRIPTOR_FILE};
use aros_common::native_build_contract::validate_native_build_compiler;
use aros_common::native_consumer_contract::validate_native_consumer_compiler;
use console::{style, Emoji};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

const MAX_LOCAL_TOOLCHAIN_DESCRIPTOR_BYTES: u64 = 16 * 1024 * 1024;

mod host_inputs;
mod local_identity;
mod native_contract;

use native_contract::NativeContractSelection;

/// Forward only the exact validated source bytes. CMake revalidates the hash
/// and inputs before translating their graph, independently of this frontend.
fn native_contract_variables(
    binding: Option<&NativeContractSelection>,
    resolved: &toolchain::ResolvedToolchain,
    profile: &aros_common::TargetProfile,
) -> Result<Vec<(String, String)>> {
    let (local_compiler, local_variables) = if resolved.source
        == toolchain::ToolchainSource::LocalCompilerOnly
    {
        let descriptor = LocalToolchainDescriptor::load(&resolved.paths.root).map_err(|error| {
            miette::miette!("cannot reload local compiler-only descriptor: {error}")
        })?;
        descriptor.verify(&resolved.paths.root).map_err(|error| {
            miette::miette!("local compiler-only descriptor is no longer verified: {error}")
        })?;
        let expected_host = crate::host_compiler::host_platform_key()?;
        let expected_profile = profile.toolchain_profile();
        if descriptor.host != expected_host
            || descriptor.target_profile != expected_profile
            || descriptor.target_triple != resolved.target_triple
        {
            miette::bail!(
                "local compiler descriptor is for {}/{}/{}; expected {}/{}/{}",
                descriptor.host,
                descriptor.target_profile,
                descriptor.target_triple,
                expected_host,
                expected_profile,
                resolved.target_triple
            );
        }
        toolchain::validate_profile_compiler(
            profile,
            &descriptor.compiler,
            &descriptor.target_triple,
        )?;

        // Hash the exact descriptor bytes CMake will independently read.
        // Reparse and compare after the bounded no-follow read to detect a
        // replacement between `load`, verification, and hashing.
        let descriptor_path = resolved.paths.root.join(LOCAL_TOOLCHAIN_DESCRIPTOR_FILE);
        let (_, bytes) = aros_common::measure_regular_file_bounded(
            &descriptor_path,
            MAX_LOCAL_TOOLCHAIN_DESCRIPTOR_BYTES,
        )
        .into_diagnostic()
        .wrap_err("cannot safely reread local compiler-only descriptor")?
        .ok_or_else(|| miette::miette!("local compiler-only descriptor disappeared"))?;
        let reread = LocalToolchainDescriptor::parse(&bytes)
            .map_err(|error| miette::miette!("invalid reread local descriptor: {error}"))?;
        if reread != descriptor {
            miette::bail!("local compiler-only descriptor changed while selecting its SHA-256");
        }
        (
            Some(descriptor.compiler.clone()),
            vec![
                (
                    "AROS_CROSS_TOOLCHAIN_QUALIFICATION".into(),
                    descriptor.qualification,
                ),
                (
                    "AROS_CROSS_TOOLCHAIN_LOCAL_SHA256".into(),
                    aros_common::sha256_bytes(&bytes).to_string(),
                ),
            ],
        )
    } else {
        (
            None,
            vec![
                ("AROS_CROSS_TOOLCHAIN_QUALIFICATION".into(), String::new()),
                ("AROS_CROSS_TOOLCHAIN_LOCAL_SHA256".into(), String::new()),
            ],
        )
    };

    let mut variables = if let Some(binding) = binding {
        let compiler = if let Some(compiler) = local_compiler {
            compiler
        } else {
            let manifest = aros_common::ArosToolchainManifest::load(&resolved.paths.root)
                .into_diagnostic()
                .wrap_err("native source contracts require a verified compiler manifest")?;
            manifest
                .compiler_identity()
                .map_err(|error| miette::miette!(error))?
        };
        match binding {
            NativeContractSelection::Build(loaded) => {
                validate_native_build_compiler(
                    &loaded.contract,
                    &compiler,
                    &resolved.target_triple,
                )
                .into_diagnostic()?;
                if resolved
                    .paths
                    .executable_roles
                    .iter()
                    .filter(|(role, _)| *role == "objdump")
                    .count()
                    != 1
                {
                    miette::bail!(
                        "native source build contracts require one verified objdump role (toolchain-tools v3); no host fallback is permitted"
                    );
                }
            }
            NativeContractSelection::Consumer(loaded) => {
                validate_native_consumer_compiler(
                    &loaded.contract,
                    &compiler,
                    &resolved.target_triple,
                )
                .into_diagnostic()?;
            }
        }
        binding.cmake_variables()?
    } else {
        // Clear both contract kinds so a reused CMake cache cannot retain an
        // inactive source binding.
        NativeContractSelection::empty_cmake_variables()
    };
    variables.extend(local_variables);
    Ok(variables)
}

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
    /// Explicit compiler-cache policy shared with the CMake engine.
    pub compiler_cache: aros_cache::CompilerBackendChoice,
    /// Optional prepared AROS-owned local compiler-cache namespace.
    pub compiler_cache_dir: Option<PathBuf>,
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
/// profile's architecture, the compiler family is the profile's declared
/// selector, and the bootloader follows the platform. Compiler executables
/// come separately from the verified payload roles. Deriving these is what
/// removes the last reason for a checkout to carry `CMakePresets.json`, and it
/// is what lets a tree without one be built from built-in profiles.
fn profile_cache_variables(
    profile: &aros_common::TargetProfile,
    build_type: BuildType,
) -> Vec<(String, String)> {
    let mut variables = vec![
        ("CMAKE_SYSTEM_NAME".to_owned(), "Generic".to_owned()),
        (
            "CMAKE_SYSTEM_PROCESSOR".to_owned(),
            profile.arch.to_string(),
        ),
        (
            "AROS_TOOLCHAIN".to_owned(),
            profile
                .transpiler
                .as_ref()
                .map_or("llvm", |selectors| selectors.toolchain.as_str())
                .to_owned(),
        ),
        (
            "AROS_TARGET_BOOTLOADER".to_owned(),
            profile.bootloader().to_owned(),
        ),
        (
            "CMAKE_BUILD_TYPE".to_owned(),
            build_type.cmake_value().to_owned(),
        ),
        ("CMAKE_EXPORT_COMPILE_COMMANDS".to_owned(), "ON".to_owned()),
    ];
    if let Some(context) = &profile.transpiler {
        variables.extend([
            ("AROS_TARGET_FAMILY".to_owned(), context.family.clone()),
            ("AROS_TARGET_VARIANT".to_owned(), context.variant.clone()),
            ("AROS_TARGET_CPU32".to_owned(), context.cpu32.clone()),
            (
                "AROS_ENABLE_MMU".to_owned(),
                if context.use_mmu { "ON" } else { "OFF" }.to_owned(),
            ),
        ]);
    }
    if let Some(version) = profile
        .transpiler
        .as_ref()
        .and_then(|context| context.mesa_version.as_ref())
    {
        variables.push(("AROS_MESA_VERSION".to_owned(), version.clone()));
    }
    if let Some(abi) = &profile.bootstrap_abi {
        variables.extend([
            ("AROS_ABI_FLAVOUR".to_owned(), abi.flavour.clone()),
            (
                "AROS_ABI_PLATFORM_SMP".to_owned(),
                if abi.platform_smp { "ON" } else { "OFF" }.to_owned(),
            ),
        ]);
    }
    variables
}

fn configure_cache_variables(
    profile: &aros_common::TargetProfile,
    build_type: BuildType,
    build_tool_variables: Vec<(String, String)>,
    compiler_variables: Vec<(String, String)>,
    native_variables: Vec<(String, String)>,
) -> Vec<(String, String)> {
    profile_cache_variables(profile, build_type)
        .into_iter()
        .chain(build_tool_variables)
        .chain(compiler_variables)
        .chain(native_variables)
        .collect()
}

/// Forward only executable roles already verified by toolchain resolution.
/// GNU tools are never guessed from a triple or discovered on the host PATH.
fn compiler_cache_variables(
    profile: &aros_common::TargetProfile,
    paths: &toolchain::ToolchainPaths,
) -> Result<Vec<(String, String)>> {
    let family = profile
        .transpiler
        .as_ref()
        .map_or("llvm", |selectors| selectors.toolchain.as_str());
    let roles: &[(&str, &str)] = match family {
        "llvm" => &[
            ("CMAKE_C_COMPILER", "clang"),
            ("CMAKE_CXX_COMPILER", "clang++"),
            ("CMAKE_ASM_COMPILER", "clang"),
            ("CMAKE_AR", "llvm-ar"),
            ("AROS_LLD_BIN", "ld.lld"),
        ],
        "gnu" => &[
            ("CMAKE_C_COMPILER", "c"),
            ("CMAKE_CXX_COMPILER", "cxx"),
            ("CMAKE_ASM_COMPILER", "c"),
            ("AROS_AS_BIN", "assembler"),
            ("CMAKE_AR", "archive"),
            ("CMAKE_RANLIB", "ranlib"),
            ("CMAKE_NM", "nm"),
            ("CMAKE_STRIP", "strip"),
            ("CMAKE_OBJCOPY", "objcopy"),
            ("AROS_LINKER_BIN", "linker"),
            ("AROS_COLLECT_BIN", "collector"),
        ],
        _ => {
            miette::bail!("unsupported native compiler family '{family}'");
        }
    };
    let mut roles = roles.to_vec();
    if family == "gnu"
        && paths
            .executable_roles
            .iter()
            .any(|(role, _)| *role == "objdump")
    {
        roles.push(("CMAKE_OBJDUMP", "objdump"));
    }
    roles
        .iter()
        .map(|(variable, required)| {
            let matches: Vec<_> = paths
                .executable_roles
                .iter()
                .filter(|(role, _)| role == required)
                .collect();
            let [(_, path)] = matches.as_slice() else {
                miette::bail!(
                    "native {family} build requires exactly one verified '{required}' tool role"
                );
            };
            if !path.is_absolute()
                || !path.starts_with(&paths.root)
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                miette::bail!("verified '{required}' tool is outside its payload root");
            }
            Ok(((*variable).to_owned(), path.display().to_string()))
        })
        .collect()
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
    let native_contract = NativeContractSelection::load(repo_root, &profile)?;
    if let Some(contract) = &native_contract {
        local_identity::validate_source_namespace(&profile, contract)?;
    }
    for definition in &options.cmake_definitions {
        validate_cmake_definition(definition)?;
    }
    let compiler_cache = aros_cache::resolve_managed_compiler_cache_for_build(
        options.compiler_cache,
        options.compiler_cache_dir.as_deref(),
    )
    .map_err(|error| miette::miette!(error))?;
    let compiler_cache_selection = compiler_cache.selection();
    let resolved = toolchain::resolve_for_build(
        repo_root,
        &options.toolchain_preset,
        options.toolchain_dir.as_deref(),
        options.input_policy.offline,
    )
    .await?;
    let lease = crate::toolchain_lifecycle::acquire_for_build(repo_root, &resolved)?;
    let native_variables =
        native_contract_variables(native_contract.as_ref(), &resolved, &profile)?;
    if resolved.source == toolchain::ToolchainSource::LocalCompilerOnly && native_contract.is_some()
    {
        aros_common::local_source::LocalSourceIdentity::validate_build_namespace(
            repo_root,
            &options.preset,
        )
        .map_err(|error| miette::miette!(error))?;
    }
    let compiler_variables = compiler_cache_variables(&profile, &resolved.paths)?;
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

    print_compiler_cache_selection(&compiler_cache_selection, options.input_policy.offline);

    aros_common::outputln!("{HAMMER} Configuring CMake build tree...");
    let engine = place_engine(&build_dir, options.engine_dir.as_deref())?;
    let cmake_toolchain = engine.join("toolchains/AROS.cmake");
    if !cmake_toolchain.is_file() {
        miette::bail!(
            "Required CMake toolchain file is missing: {}",
            cmake_toolchain.display()
        );
    }
    let is_local_native = resolved.source == toolchain::ToolchainSource::LocalCompilerOnly
        && native_contract.is_some();
    let cmake_executor = if is_local_native {
        which::which("cmake")
            .into_diagnostic()?
            .canonicalize()
            .into_diagnostic()?
    } else {
        PathBuf::from("cmake")
    };
    let mut configure = Command::new(&cmake_executor);
    let host_inputs =
        host_inputs::prepare(native_contract.as_ref(), options.input_policy.offline).await?;
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
    compiler_cache.apply_to(&mut configure);
    // General definitions precede every source, compiler and ABI identity.
    // Board callers may add build options but cannot replace verified inputs.
    for definition in &options.cmake_definitions {
        configure.arg(format!("-D{}={}", definition.key, definition.value));
    }
    configure.arg(format!(
        "-DAROS_NATIVE_HOST_INPUT_DIRECTORY={}",
        host_inputs
            .as_ref()
            .map_or_else(String::new, |root| root.display().to_string())
    ));
    configure.arg(format!("-DAROS_SOURCE_DIR={}", repo_root.display()));
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
    let media_cli = std::env::current_exe()
        .into_diagnostic()
        .wrap_err("cannot locate the current aros executable for media receipts")?;
    configure.arg(format!("-DAROS_MEDIA_CLI_BIN={}", media_cli.display()));
    configure.arg(format!("-DAROS_TARGET_CPU={}", profile.arch.source_cpu()));
    configure.arg(format!("-DAROS_TARGET_PLATFORM={}", profile.platform));
    configure.arg(format!("-DAROS_TARGET_PROFILE={}", profile.name));
    configure.arg(format!(
        "-DAROS_CROSS_TOOLCHAIN_PROFILE={}",
        profile.toolchain_profile()
    ));
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
    // General CMake definitions cannot change source/compiler/ABI contracts
    // already validated by this frontend.
    for (key, value) in configure_cache_variables(
        &profile,
        options.build_type,
        build_tools.cmake_variables(),
        compiler_variables,
        native_variables,
    ) {
        configure.arg(format!("-D{key}={value}"));
    }
    for definition in compiler_cache_cmake_definitions(&compiler_cache_selection)? {
        configure.arg(format!("-D{}={}", definition.key, definition.value));
    }
    if options.verbose {
        configure.arg("--log-level=VERBOSE");
    }
    if is_local_native {
        let ninja = which::which("ninja")
            .into_diagnostic()?
            .canonicalize()
            .into_diagnostic()?;
        configure.arg(format!("-DCMAKE_MAKE_PROGRAM={}", ninja.display()));
    }
    // Local source identity is not a clean Git/released-toolchain receipt.
    // Capture before execution, refuse adoption of historical output trees,
    // and remeasure before declaring a successful native build.
    let local_native = if resolved.source == toolchain::ToolchainSource::LocalCompilerOnly
        && native_contract.is_some()
    {
        let selected_contract = native_contract
            .as_ref()
            .ok_or_else(|| miette::miette!("local native build has no source contract"))?;
        let identity =
            local_identity::LocalNativeInputs::capture(&local_identity::LocalNativeCapture {
                root: repo_root,
                preset: &options.preset,
                profile: &profile,
                toolchain_root: &resolved.paths.root,
                engine: &engine,
                tools: &build_tools.bin_dir,
                configure: &configure,
                selected_contract,
            })?;
        identity.bind_build_tree(&build_dir)?;
        local_identity::verify_configured_tree(&build_dir, false)?;
        Some(identity)
    } else {
        None
    };
    crate::observability::run_command_at(
        &mut configure,
        &format!("CMake configure for preset '{}'", options.preset),
        crate::observability::ErrorBoundary {
            code: aros_common::DiagnosticCode::CliConfigure,
            stage: aros_common::DiagnosticStage::BuildConfiguration,
            hint: "inspect the bounded CMake output and repair the selected preset or configure contract",
        },
    )?;

    if let Some(expected) = &local_native {
        let selected_contract = native_contract
            .as_ref()
            .ok_or_else(|| miette::miette!("local native configure lost its source contract"))?;
        let actual =
            local_identity::LocalNativeInputs::capture(&local_identity::LocalNativeCapture {
                root: repo_root,
                preset: &options.preset,
                profile: &profile,
                toolchain_root: &resolved.paths.root,
                engine: &engine,
                tools: &build_tools.bin_dir,
                configure: &configure,
                selected_contract,
            })?;
        expected.require_unchanged(&actual)?;
        expected.bind_build_tree(&build_dir)?;
        local_identity::verify_configured_tree(&build_dir, true)?;
    }

    aros_common::outputln!("{HAMMER} Compiling AROS modules with Ninja...");
    let mut build = Command::new(&cmake_executor);
    build.current_dir(repo_root).args(["--build"]);
    compiler_cache.apply_to(&mut build);
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

    if let Some(expected) = &local_native {
        let selected_contract = native_contract
            .as_ref()
            .ok_or_else(|| miette::miette!("local native build lost its source contract"))?;
        let actual =
            local_identity::LocalNativeInputs::capture(&local_identity::LocalNativeCapture {
                root: repo_root,
                preset: &options.preset,
                profile: &profile,
                toolchain_root: &resolved.paths.root,
                engine: &engine,
                tools: &build_tools.bin_dir,
                configure: &configure,
                selected_contract,
            })?;
        expected.require_unchanged(&actual)?;
        expected.bind_build_tree(&build_dir)?;
        local_identity::verify_configured_tree(&build_dir, false)?;
    }

    aros_common::outputln!(
        "{CHECK} {}Build completed successfully in {:.2?}!",
        style("SUCCESS: ").green().bold(),
        start.elapsed()
    );
    drop(lease);
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

fn print_compiler_cache_selection(selection: &aros_cache::CompilerCacheSelection, offline: bool) {
    match selection {
        aros_cache::CompilerCacheSelection::Off if offline => aros_common::outputln!(
            "⚡ Compiler cache launcher: {} (no prepared AROS-owned local backend is available)",
            style("none").green().bold()
        ),
        aros_cache::CompilerCacheSelection::Off => {
            aros_common::outputln!(
                "⚡ Compiler cache launcher: {}",
                style("none").green().bold()
            );
        }
        aros_cache::CompilerCacheSelection::Backend {
            backend,
            executable,
        } => aros_common::outputln!(
            "⚡ Compiler cache launcher: {} ({})",
            style(backend.program()).green().bold(),
            executable.display()
        ),
    }
}

/// Translate the single Rust-side selection into non-overridable CMake input.
///
/// These definitions are appended after all general CMake definitions. That
/// preserves the frontend's resolved selection as the sole source of truth and
/// prevents stale CMake cache values or generic definitions from re-enabling a
/// launcher after `off`.
fn compiler_cache_cmake_definitions(
    selection: &aros_cache::CompilerCacheSelection,
) -> Result<Vec<CmakeDefinition>> {
    match selection {
        aros_cache::CompilerCacheSelection::Off => Ok(vec![CmakeDefinition {
            key: "AROS_COMPILER_CACHE_MODE".to_owned(),
            value: "off".to_owned(),
        }]),
        aros_cache::CompilerCacheSelection::Backend {
            backend,
            executable,
        } => {
            if !executable.is_absolute() {
                miette::bail!(
                    "selected compiler-cache executable '{}' is not absolute",
                    executable.display()
                );
            }
            let executable = executable.to_str().ok_or_else(|| {
                miette::miette!(
                    "selected compiler-cache executable path is not valid UTF-8 and cannot be passed safely to CMake"
                )
            })?;
            if executable.contains(';') {
                miette::bail!(
                    "selected compiler-cache executable path contains ';', which CMake treats as a list separator"
                );
            }
            Ok(vec![
                CmakeDefinition {
                    key: "AROS_COMPILER_CACHE_MODE".to_owned(),
                    value: backend.program().to_owned(),
                },
                CmakeDefinition {
                    key: "AROS_COMPILER_CACHE_EXECUTABLE".to_owned(),
                    value: executable.to_owned(),
                },
            ])
        }
    }
}

/// Backwards-compatible local name for the shared compiler-cache backend
/// contract. New callers should resolve it through `aros-cache` so frontend
/// and future CMake integration share one selection vocabulary.
pub use aros_cache::CompilerBackend as CompilerCache;

/// Select the preferred available compiler cache once for all CLI commands.
pub fn detected_compiler_cache() -> Option<CompilerCache> {
    match aros_cache::resolve_compiler_cache(aros_cache::CompilerBackendChoice::Auto)
        .expect("automatic compiler-cache discovery does not fail")
    {
        aros_cache::CompilerCacheSelection::Off => None,
        aros_cache::CompilerCacheSelection::Backend { backend, .. } => Some(backend),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_dir, compiler_cache_cmake_definitions, compiler_cache_variables,
        configure_cache_variables, native_contract_variables, profile_cache_variables, run,
        validate_cmake_definition, validate_preset, BuildInputPolicy, BuildOptions, BuildType,
        CmakeDefinition, NativeContractSelection,
    };

    pub(super) fn native_consumer_source_fixture() -> (
        tempfile::TempDir,
        aros_common::TargetProfile,
        serde_json::Value,
    ) {
        use serde_json::json;
        use std::fs;

        let root = tempfile::tempdir().unwrap();
        let profile_text = "[[targets]]\nname='fixture-target'\narch='riscv32'\nplatform='fixture'\nbsp='fixture'\nfloat_abi='ilp32f'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=false\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=false\n";
        fs::write(root.path().join("aros-targets.toml"), profile_text).unwrap();
        fs::write(root.path().join("policy.json"), b"{}").unwrap();
        let profile = aros_common::TargetProfile::parse_config(profile_text, "fixture")
            .unwrap()
            .targets
            .remove(0);
        let contract = json!({
            "schema": "aros-native-consumer-contract-v1",
            "profile": "fixture-target",
            "source_baseline": "0123456789abcdef0123456789abcdef01234567",
            "roots": ["includes", "linklibs"],
            "metamake_projection": "policy.json",
            "inputs": [
                {"path": "aros-targets.toml", "sha256": aros_common::sha256_bytes(profile_text.as_bytes())},
                {"path": "policy.json", "sha256": aros_common::sha256_bytes(b"{}")}
            ],
            "abi": {
                "source_cpu": "riscv",
                "target_triple": "riscv-aros",
                "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
                "abi": "ilp32f",
                "code_model": "medany",
                "flavour": "native",
                "platform_smp": false,
                "use_mmu": false
            }
        });
        fs::write(
            root.path().join("consumer.json"),
            serde_json::to_vec(&contract).unwrap(),
        )
        .unwrap();
        (root, profile, contract)
    }

    #[cfg(unix)]
    pub(super) fn local_compiler_variables_fixture() -> (
        tempfile::TempDir,
        aros_common::TargetProfile,
        crate::toolchain::ResolvedToolchain,
    ) {
        use serde_json::json;
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().unwrap();
        let host = crate::host_compiler::host_platform_key().unwrap();
        assert!(
            matches!(host, "linux-x86_64" | "linux-aarch64" | "macos-aarch64"),
            "local compiler fixture host is unsupported: {host}"
        );
        let profile = aros_common::TargetProfile {
            name: "fixture-target".into(),
            toolchain_profile: None,
            arch: aros_common::Architecture::Riscv32,
            platform: "fixture".into(),
            bsp: "fixture".into(),
            features: Vec::new(),
            float_abi: Some("ilp32f".into()),
            bootloader: None,
            transpiler: Some(aros_common::TranspilerProfile {
                family: String::new(),
                variant: String::new(),
                toolchain: "gnu".into(),
                cpu32: String::new(),
                use_mmu: false,
                mesa_version: None,
            }),
            bootstrap_abi: None,
            native_build_contract: None,
            native_consumer_contract: None,
        };
        let compiler = json!({
            "family": "gnu", "gcc_version": "16.2.0", "binutils_version": "2.47",
            "target": {
                "schema": "aros-riscv-target-v1",
                "isa": "rv32imafc_zicsr_zifencei_zaamo_zalrsc",
                "abi": "ilp32f", "code_model": "medany",
                "architecture": "rv32i2p1_m2p0_a2p1_f2p2_c2p0_zicsr2p0_zifencei2p0_zaamo1p0_zalrsc1p0",
                "unaligned_access": false, "atomic_abi": 0, "x3_reg_usage": 0
            }
        });
        let tools = json!({
            "c": "bin/gcc", "cxx": "bin/g++", "assembler": "bin/as",
            "linker": "bin/ld", "archive": "bin/ar", "ranlib": "bin/ranlib",
            "strip": "bin/strip", "collector": "bin/collect-aros", "nm": "bin/nm",
            "objcopy": "bin/objcopy", "objdump": "bin/objdump"
        });
        let layout = json!({
            "schema": "aros-toolchain-tools-v3", "compiler": compiler,
            "target_triple": "riscv-aros", "tools": tools
        });
        fs::create_dir_all(root.path().join("bin")).unwrap();
        let script = "#!/bin/sh\ncase \"$1\" in\n--version) printf '%s\\n' fixture;;\n-dumpmachine) printf '%s\\n' 'riscv-aros';;\n-dumpfullversion) printf '%s\\n' '16.2.0';;\n*) exit 0;;\nesac\n";
        for path in [
            "bin/gcc",
            "bin/g++",
            "bin/as",
            "bin/ld",
            "bin/ar",
            "bin/ranlib",
            "bin/strip",
            "bin/collect-aros",
            "bin/nm",
            "bin/objcopy",
            "bin/objdump",
        ] {
            let path = root.path().join(path);
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            root.path()
                .join(aros_common::toolchain_layout::TOOLCHAIN_TOOLS_FILE),
            serde_json::to_vec(&layout).unwrap(),
        )
        .unwrap();
        let identity = serde_json::from_value(layout["compiler"].clone()).unwrap();
        let descriptor = aros_common::local_toolchain::LocalToolchainDescriptor::capture(
            root.path(),
            host,
            profile.toolchain_profile(),
            "riscv-aros",
            identity,
        )
        .unwrap();
        fs::write(
            root.path()
                .join(aros_common::local_toolchain::LOCAL_TOOLCHAIN_DESCRIPTOR_FILE),
            serde_json::to_vec(&descriptor).unwrap(),
        )
        .unwrap();
        let resolved = crate::toolchain::ResolvedToolchain {
            paths: crate::toolchain::ToolchainPaths {
                root: root.path().to_path_buf(),
                executable_roles: Vec::new(),
            },
            target_triple: "riscv-aros".into(),
            release_id: None,
            source: crate::toolchain::ToolchainSource::LocalCompilerOnly,
        };
        (root, profile, resolved)
    }

    #[test]
    fn built_in_profiles_pin_llvm_family_and_platform_bootloader() {
        let absent_override = tempfile::tempdir().unwrap();
        let profiles = aros_common::TargetProfile::load_config_or_builtin(
            &absent_override.path().join("aros-targets.toml"),
        )
        .unwrap()
        .targets;
        assert!(!profiles.is_empty());

        for profile in profiles {
            let values: std::collections::HashMap<_, _> =
                profile_cache_variables(&profile, BuildType::Release)
                    .into_iter()
                    .collect();
            for (key, expected) in [("AROS_TOOLCHAIN", "llvm"), ("CMAKE_SYSTEM_NAME", "Generic")] {
                assert_eq!(
                    values.get(key).map(String::as_str),
                    Some(expected),
                    "{}: {key}",
                    profile.name
                );
            }
            let expected_bootloader = if profile.platform == "pc" {
                "grub2gfx"
            } else {
                ""
            };
            assert_eq!(
                values.get("AROS_TARGET_BOOTLOADER").map(String::as_str),
                Some(expected_bootloader),
                "{}: bootloader",
                profile.name
            );
            assert!(!values.contains_key("AROS_MESA_VERSION"));
            assert!(!values.contains_key("CMAKE_C_COMPILER"));
        }
    }

    #[test]
    fn verified_gnu_collector_overrides_the_selected_suite_collector() {
        let absent_override = tempfile::tempdir().unwrap();
        let mut profile = aros_common::TargetProfile::load_config_or_builtin(
            &absent_override.path().join("aros-targets.toml"),
        )
        .unwrap()
        .targets
        .remove(0);
        profile.transpiler.as_mut().unwrap().toolchain = "gnu".into();

        let variables = configure_cache_variables(
            &profile,
            BuildType::Release,
            vec![("AROS_COLLECT_BIN".into(), "/suite/aros-collect".into())],
            vec![(
                "AROS_COLLECT_BIN".into(),
                "/verified/bin/collect-aros".into(),
            )],
            Vec::new(),
        );
        let collectors = variables
            .iter()
            .filter(|(key, _)| key == "AROS_COLLECT_BIN")
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            collectors,
            ["/suite/aros-collect", "/verified/bin/collect-aros"]
        );
        assert_eq!(collectors.last(), Some(&"/verified/bin/collect-aros"));
    }

    #[cfg(unix)]
    #[test]
    fn local_compiler_cmake_variables_bind_raw_descriptor_even_without_native_contract() {
        let (root, profile, resolved) = local_compiler_variables_fixture();
        let values: std::collections::HashMap<_, _> =
            native_contract_variables(None, &resolved, &profile)
                .unwrap()
                .into_iter()
                .collect();
        let descriptor = root
            .path()
            .join(aros_common::local_toolchain::LOCAL_TOOLCHAIN_DESCRIPTOR_FILE);
        let bytes = std::fs::read(&descriptor).unwrap();
        assert_eq!(
            values["AROS_CROSS_TOOLCHAIN_QUALIFICATION"],
            "local-byte-verified"
        );
        assert_eq!(
            values["AROS_CROSS_TOOLCHAIN_LOCAL_SHA256"],
            aros_common::sha256_bytes(&bytes).to_string()
        );
        assert_eq!(values["AROS_NATIVE_BUILD_CONTRACT"], "");
        assert_eq!(values["AROS_NATIVE_BUILD_CONTRACT_SHA256"], "");
        assert_eq!(values["AROS_NATIVE_CONSUMER_CONTRACT"], "");
        assert_eq!(values["AROS_NATIVE_CONSUMER_CONTRACT_SHA256"], "");
    }

    #[test]
    fn nonlocal_source_clears_stale_local_cmake_variables() {
        let root = tempfile::tempdir().unwrap();
        let profile = aros_common::TargetProfile::load_config_or_builtin(
            &std::path::Path::new("/absent-config").join("aros-targets.toml"),
        )
        .unwrap()
        .targets
        .remove(0);
        let resolved = crate::toolchain::ResolvedToolchain {
            paths: crate::toolchain::ToolchainPaths {
                root: root.path().to_path_buf(),
                executable_roles: Vec::new(),
            },
            target_triple: "x86_64-unknown-aros".into(),
            release_id: Some("test-release".into()),
            source: crate::toolchain::ToolchainSource::LockedRelease,
        };
        let values: std::collections::HashMap<_, _> =
            native_contract_variables(None, &resolved, &profile)
                .unwrap()
                .into_iter()
                .collect();
        for key in [
            "AROS_CROSS_TOOLCHAIN_QUALIFICATION",
            "AROS_CROSS_TOOLCHAIN_LOCAL_SHA256",
            "AROS_NATIVE_BUILD_CONTRACT",
            "AROS_NATIVE_BUILD_CONTRACT_SHA256",
            "AROS_NATIVE_CONSUMER_CONTRACT",
            "AROS_NATIVE_CONSUMER_CONTRACT_SHA256",
        ] {
            assert_eq!(values[key], "", "{key}");
        }
    }

    #[test]
    fn native_consumer_selection_forwards_exact_binding_and_clears_build_binding() {
        let (root, profile, _) = native_consumer_source_fixture();
        let selection = NativeContractSelection::load(root.path(), &profile)
            .unwrap()
            .unwrap();
        let values: std::collections::HashMap<_, _> =
            selection.cmake_variables().unwrap().into_iter().collect();
        assert_eq!(
            values["AROS_NATIVE_CONSUMER_CONTRACT"],
            root.path()
                .canonicalize()
                .unwrap()
                .join("consumer.json")
                .display()
                .to_string()
        );
        assert_eq!(
            values["AROS_NATIVE_CONSUMER_CONTRACT_SHA256"],
            aros_common::sha256_file(&root.path().join("consumer.json"))
                .unwrap()
                .digest
                .to_string()
        );
        assert_eq!(values["AROS_NATIVE_BUILD_CONTRACT"], "");
        assert_eq!(values["AROS_NATIVE_BUILD_CONTRACT_SHA256"], "");
    }

    #[test]
    fn native_contract_selection_rejects_invalid_inputs_profile_drift_and_ambiguity() {
        let (root, profile, mut contract) = native_consumer_source_fixture();
        contract["roots"] = serde_json::json!([]);
        std::fs::write(
            root.path().join("consumer.json"),
            serde_json::to_vec(&contract).unwrap(),
        )
        .unwrap();
        assert!(NativeContractSelection::load(root.path(), &profile).is_err());

        let (root, profile, _) = native_consumer_source_fixture();
        std::fs::write(root.path().join("policy.json"), b"changed").unwrap();
        assert!(NativeContractSelection::load(root.path(), &profile).is_err());

        let (root, mut profile, _) = native_consumer_source_fixture();
        profile.features.push("different-source-profile".into());
        assert!(NativeContractSelection::load(root.path(), &profile).is_err());

        let (root, mut profile, _) = native_consumer_source_fixture();
        profile.native_build_contract = Some("missing-build.json".into());
        let error = NativeContractSelection::load(root.path(), &profile).unwrap_err();
        assert!(error.to_string().contains("both"));
    }

    #[test]
    fn native_consumer_cannot_bind_inputs_from_generated_build_namespace() {
        let (root, profile, _) = native_consumer_source_fixture();
        let mut selection = NativeContractSelection::load(root.path(), &profile)
            .unwrap()
            .unwrap();
        assert!(super::local_identity::validate_source_namespace(&profile, &selection).is_ok());
        let NativeContractSelection::Consumer(binding) = &mut selection else {
            panic!("fixture must select a consumer contract");
        };
        binding.contract.inputs[1].path = "build/policy.json".into();
        assert!(super::local_identity::validate_source_namespace(&profile, &selection).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn local_consumer_binding_checks_the_verified_descriptor_compiler_identity() {
        let (toolchain_root, _, resolved) = local_compiler_variables_fixture();
        let (source_root, profile, mut contract) = native_consumer_source_fixture();
        contract["abi"]["isa"] = serde_json::json!("rv32imafc");
        std::fs::write(
            source_root.path().join("consumer.json"),
            serde_json::to_vec(&contract).unwrap(),
        )
        .unwrap();
        let selection = NativeContractSelection::load(source_root.path(), &profile)
            .unwrap()
            .unwrap();
        let error = native_contract_variables(Some(&selection), &resolved, &profile).unwrap_err();
        assert!(error
            .to_string()
            .contains("verified triple/ISA/ABI/code model differs"));
        // Keep the verified toolchain fixture alive while the binding is checked.
        assert!(toolchain_root
            .path()
            .join(aros_common::local_toolchain::LOCAL_TOOLCHAIN_DESCRIPTOR_FILE)
            .is_file());
    }

    #[test]
    fn native_compiler_selection_uses_verified_roles_and_rejects_missing_or_ambiguous_tools() {
        let mut profile = aros_common::TargetProfile::load_config_or_builtin(
            &std::path::Path::new("/absent-config").join("aros-targets.toml"),
        )
        .unwrap()
        .targets
        .remove(0);
        let root = std::path::PathBuf::from("/verified/payload");
        let llvm = crate::toolchain::get_toolchain_paths(&root);
        let llvm_values: std::collections::HashMap<_, _> =
            compiler_cache_variables(&profile, &llvm)
                .unwrap()
                .into_iter()
                .collect();
        assert_eq!(
            llvm_values["CMAKE_C_COMPILER"],
            "/verified/payload/bin/clang"
        );
        profile.transpiler.as_mut().unwrap().toolchain = "gnu".into();
        profile.arch = aros_common::Architecture::Riscv32;
        profile.transpiler.as_mut().unwrap().use_mmu = false;
        profile.transpiler.as_mut().unwrap().cpu32.clear();
        profile.transpiler.as_mut().unwrap().variant = "experimental".into();
        profile.bootstrap_abi = Some(aros_common::BootstrapAbiProfile {
            flavour: "standalone".into(),
            platform_smp: false,
        });
        let profile_values: std::collections::HashMap<_, _> =
            profile_cache_variables(&profile, BuildType::Release)
                .into_iter()
                .collect();
        assert_eq!(profile_values["AROS_TOOLCHAIN"], "gnu");
        assert_eq!(profile_values["AROS_ENABLE_MMU"], "OFF");
        assert_eq!(profile_values["AROS_TARGET_VARIANT"], "experimental");
        assert_eq!(profile_values["AROS_TARGET_CPU32"], "");
        assert_eq!(profile_values["AROS_ABI_FLAVOUR"], "standalone");
        assert_eq!(profile_values["AROS_ABI_PLATFORM_SMP"], "OFF");
        assert_eq!(profile.arch.source_cpu(), "riscv");
        assert!(compiler_cache_variables(&profile, &llvm).is_err());
        let mut gnu = crate::toolchain::ToolchainPaths {
            root: root.clone(),
            executable_roles: [
                "c",
                "cxx",
                "assembler",
                "archive",
                "ranlib",
                "nm",
                "strip",
                "objcopy",
                "linker",
                "collector",
            ]
            .into_iter()
            .map(|role| (role, root.join("bin").join(format!("declared-{role}"))))
            .collect(),
        };
        let values: std::collections::HashMap<_, _> = compiler_cache_variables(&profile, &gnu)
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(
            values["CMAKE_C_COMPILER"],
            "/verified/payload/bin/declared-c"
        );
        assert_eq!(
            values["AROS_LINKER_BIN"],
            "/verified/payload/bin/declared-linker"
        );
        assert!(!values.contains_key("AROS_LLD_BIN"));
        assert_eq!(values["CMAKE_ASM_COMPILER"], values["CMAKE_C_COMPILER"]);
        assert_eq!(
            values["AROS_AS_BIN"],
            "/verified/payload/bin/declared-assembler"
        );
        gnu.executable_roles.push(gnu.executable_roles[0].clone());
        assert!(compiler_cache_variables(&profile, &gnu).is_err());
        gnu.executable_roles.pop();
        gnu.executable_roles[0].1 = "/host/bin/gcc".into();
        assert!(compiler_cache_variables(&profile, &gnu).is_err());
        profile.transpiler.as_mut().unwrap().toolchain = "unknown".into();
        assert!(compiler_cache_variables(&profile, &gnu).is_err());
    }

    #[test]
    fn explicit_mesa_selector_reaches_cmake_without_changing_legacy_profiles() {
        let absent_override = tempfile::tempdir().unwrap();
        let mut profile = aros_common::TargetProfile::load_config_or_builtin(
            &absent_override.path().join("aros-targets.toml"),
        )
        .unwrap()
        .targets
        .remove(0);
        profile.transpiler.as_mut().unwrap().mesa_version = Some("26.0.0".into());
        let values: std::collections::HashMap<_, _> =
            profile_cache_variables(&profile, BuildType::Release)
                .into_iter()
                .collect();
        assert_eq!(
            values.get("AROS_MESA_VERSION").map(String::as_str),
            Some("26.0.0")
        );
    }

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

    #[test]
    fn compiler_cache_cmake_definitions_carry_one_resolved_selection() {
        let selected = aros_cache::CompilerCacheSelection::Backend {
            backend: aros_cache::CompilerBackend::Sccache,
            executable: std::path::PathBuf::from("/tools/sccache"),
        };
        assert_eq!(
            compiler_cache_cmake_definitions(&selected).unwrap(),
            vec![
                CmakeDefinition {
                    key: "AROS_COMPILER_CACHE_MODE".to_owned(),
                    value: "sccache".to_owned(),
                },
                CmakeDefinition {
                    key: "AROS_COMPILER_CACHE_EXECUTABLE".to_owned(),
                    value: "/tools/sccache".to_owned(),
                },
            ]
        );
        assert_eq!(
            compiler_cache_cmake_definitions(&aros_cache::CompilerCacheSelection::Off).unwrap(),
            vec![CmakeDefinition {
                key: "AROS_COMPILER_CACHE_MODE".to_owned(),
                value: "off".to_owned(),
            }]
        );
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
            compiler_cache: aros_cache::CompilerBackendChoice::Auto,
            compiler_cache_dir: None,
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

    #[tokio::test]
    async fn ambiguous_source_contract_selection_stops_before_toolchain_resolution() {
        let checkout = tempfile::tempdir().unwrap();
        std::fs::write(
            checkout.path().join("aros-targets.toml"),
            "[[targets]]\nname='fixture-target'\narch='riscv32'\nplatform='fixture'\nbsp='fixture'\nfloat_abi='ilp32f'\nnative_build_contract='build.json'\nnative_consumer_contract='consumer.json'\n[targets.transpiler]\nfamily=''\nvariant=''\ntoolchain='gnu'\ncpu32=''\nuse_mmu=false\n[targets.bootstrap_abi]\nflavour='native'\nplatform_smp=false\n",
        )
        .unwrap();
        let options = BuildOptions {
            preset: "fixture".into(),
            toolchain_preset: "fixture-target".into(),
            target: None,
            jobs: None,
            clean: false,
            verbose: false,
            compiler_cache: aros_cache::CompilerBackendChoice::Off,
            compiler_cache_dir: None,
            input_policy: BuildInputPolicy {
                offline: true,
                require_fetch_checksums: true,
            },
            toolchain_dir: None,
            cmake_definitions: Vec::new(),
            build_type: BuildType::Release,
            engine_dir: None,
        };
        let error = run(checkout.path(), &options).await.unwrap_err();
        assert!(error.to_string().contains("both"));
    }
}
