//! Actual pre-transpilation source configuration using an existing LLVM compiler.
//!
//! This is a local measured producer, not a compatibility phase, compiler build,
//! authenticated owner, SDK or release. Callers still bind its receipt into the
//! complete execution identity before consuming its data in a qualified graph.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aros_common::mesa_configuration::{
    MesaConfigurationIdentity, ValidatedMesaConfiguration, MESA_BASELINE_VARIABLES,
    MESA_COMPILER_OPTION_VARIABLES,
};
use aros_common::{
    measure_regular_file_bounded, measure_tree_content_cas, publish_atomic_file,
    run_output_with_input_and_control, sha256_bytes, ArosCompilerIdentity, AtomicFilePolicy,
    CancellationToken, Sha256Digest, TreeContentCas,
};
use serde::Serialize;

use super::mesa_preflight_measurement::{parse_values, wrapper};
use super::{environment, CompatibilityEnvironment, LlvmSdkDriverBinding};
use crate::{profiles::Profile, ContractError};

const CAPTURE: usize = 256 * 1024;
const FILE_LIMIT: u64 = 1024 * 1024;
const ENVIRONMENT_KEYS: &[&str] = &[
    "PATH",
    "AROS_FETCH_OFFLINE",
    "ac_cv_prog_cc_c23",
    "PYTHON",
    "PYTHONPATH",
    "PYTHONDONTWRITEBYTECODE",
    "PYTHONHASHSEED",
    "PYTHONNOUSERSITE",
    "SOURCE_DATE_EPOCH",
    "LC_ALL",
];
const FEATURE_KEYS: &[&str] = &[
    "AROS_TARGET_CPU",
    "AROS_TARGET_ARCH",
    "TARGET_CC",
    "TARGET_SYSROOT",
    "TARGET_CFLAGS",
    "DEBUG_CFLAGS",
    "OPTIMIZATION_CFLAGS",
    "TARGET_CPP",
    "TARGET_CXX",
    "TARGET_CXXFLAGS",
    "AROS_DEVELOPER",
    "AROS_BUILD_HOST",
    "GENDIR",
    "PORTSDIR",
];
const BASELINE_KEYS: &[&str] = &[
    "TARGET_ISA_CFLAGS",
    "TARGET_ISA_CXXFLAGS",
    "CONFIG_CFLAGS",
    "CONFIG_CXXFLAGS",
    "CONFIG_CPPFLAGS",
    "TARGET_CFLAGS",
    "TARGET_CXXFLAGS",
    "TARGET_CPPFLAGS",
    "DEBUG_CFLAGS",
    "DEBUG_CXXFLAGS",
    "OPTIMIZATION_CFLAGS",
    "MESA_COMPILEOPTIONS",
    "LLVM_NOWARN",
];
const BUILTINS: &[(&str, &str)] = &[
    ("CFLAGS_NO_BUILTIN_FFS", "-fno-builtin-ffs"),
    ("CFLAGS_NO_BUILTIN_FFSLL", "-fno-builtin-ffsll"),
    ("CFLAGS_NO_BUILTIN_BSWAP32", "-fno-builtin-bswap32"),
    ("CFLAGS_NO_BUILTIN_BSWAP64", "-fno-builtin-bswap64"),
    ("CFLAGS_NO_BUILTIN_CLZ", "-fno-builtin-clz"),
    ("CFLAGS_NO_BUILTIN_CLZLL", "-fno-builtin-clzll"),
    ("CFLAGS_NO_BUILTIN_CTZ", "-fno-builtin-ctz"),
    ("CFLAGS_NO_BUILTIN_EXPECT", "-fno-builtin-expect"),
    ("CFLAGS_NO_BUILTIN_POPCOUNT", "-fno-builtin-popcount"),
    ("CFLAGS_NO_BUILTIN_POPCOUNTLL", "-fno-builtin-popcountll"),
    ("CFLAGS_NO_BUILTIN_UNREACHABLE", "-fno-builtin-unreachable"),
];

/// Explicit inputs already selected by a frontend, never inferred from a board name.
pub struct MesaPreflightRequest<'a> {
    pub source_root: &'a Path,
    /// Complete source-tree measurement independently selected by the caller.
    pub source_tree_sha256: &'a Sha256Digest,
    pub profile: &'a Profile,
    /// Existing measured drivers; package authentication remains the caller's duty.
    pub drivers: &'a LlvmSdkDriverBinding,
    pub environment: &'a CompatibilityEnvironment,
    /// Fresh leaf outside source/compiler/host-tool inputs. Failures are retained.
    pub output_root: &'a Path,
    /// Whole preflight deadline, not a fresh allowance for each subprocess.
    pub timeout: Duration,
}

/// Completed local producer; private fields prevent construction from a data file.
pub struct CompletedMesaPreflight {
    configuration: ValidatedMesaConfiguration,
    configuration_path: PathBuf,
    receipt_sha256: Sha256Digest,
    make_values: BTreeMap<String, String>,
    source_root: PathBuf,
    compiler_root: PathBuf,
    host_root: PathBuf,
    profile_sha256: Sha256Digest,
    environment_sha256: Sha256Digest,
    output_tree: TreeContentCas,
}

impl CompletedMesaPreflight {
    #[must_use]
    pub const fn configuration(&self) -> &ValidatedMesaConfiguration {
        &self.configuration
    }
    #[must_use]
    pub fn configuration_path(&self) -> &Path {
        &self.configuration_path
    }
    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.receipt_sha256
    }
    /// Includes configured global baseline values; these must not be lost when
    /// the USER/ISA source components are eventually projected into compilation.
    #[must_use]
    pub const fn make_values(&self) -> &BTreeMap<String, String> {
        &self.make_values
    }

    /// Revalidate the exact local input selection and every retained output.
    ///
    /// # Errors
    /// Rejects changed compiler/source/environment, root/profile reselection or
    /// any output mutation. A later consumer must call this before using data.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        validate_selection(request, cancellation)?;
        let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment
        else {
            return Err(failure(
                "Mesa preflight revalidation lost sealed host tools",
            ));
        };
        if request.source_root != self.source_root
            || request.source_tree_sha256 != &self.configuration.identity().source_inputs_sha256
            || request.drivers.compiler() != &self.configuration.identity().compiler
            || request.drivers.compiler_tree_sha256()
                != &self.configuration.identity().compiler_tree_sha256
            || request.drivers.clang_path().parent().and_then(Path::parent)
                != Some(self.compiler_root.as_path())
            || host_tools.root != self.host_root
            || request.profile.document_sha256() != &self.profile_sha256
            || request.profile.name() != self.configuration.identity().target_profile
            || request.profile.target_triple() != self.configuration.identity().target_triple
            || request.output_root.join("mesa-configuration.json") != self.configuration_path
            || environment::identity(&environment::resolve(request.environment)?)?
                != self.environment_sha256
        {
            return Err(failure(
                "Mesa preflight revalidation selected different inputs",
            ));
        }
        let current = measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot remeasure Mesa preflight output tree"))?;
        if current != self.output_tree {
            return Err(failure("Mesa preflight output changed"));
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct Stage {
    program: Sha256Digest,
    invocation: Sha256Digest,
    environment: Sha256Digest,
    stdout: Sha256Digest,
    stderr: Sha256Digest,
}

/// Run actual source configure/features before any graph translation.
///
/// # Errors
/// Rejects stale source/compiler/environment, unsafe or adopted output roots,
/// target disagreement, malformed measurements, failed children or deadline.
/// No compiler construction, fetch, retry, cleanup or publication is performed.
pub fn execute_mesa_preflight(
    request: &MesaPreflightRequest<'_>,
    cancellation: &CancellationToken,
) -> Result<CompletedMesaPreflight, ContractError> {
    let started = Instant::now();
    validate_selection(request, cancellation)?;
    let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment else {
        return Err(failure(
            "Mesa preflight requires the sealed host-tool environment",
        ));
    };
    let source = super::checked_directory(request.source_root, "Mesa preflight source")?;
    if source != request.source_root {
        return Err(failure(
            "Mesa preflight source must use its canonical absolute root",
        ));
    }
    let parent = request
        .output_root
        .parent()
        .ok_or_else(|| failure("missing output parent"))?;
    let parent = super::checked_directory(parent, "Mesa preflight output parent")?;
    if !request.output_root.is_absolute()
        || request.output_root.parent() != Some(parent.as_path())
        || request.output_root.file_name().is_none()
    {
        return Err(failure(
            "Mesa preflight requires a canonical absolute output leaf",
        ));
    }
    let compiler_root = request
        .drivers
        .clang_path()
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| failure("invalid measured compiler root"))?;
    for input in [source.as_path(), compiler_root, host_tools.root.as_path()] {
        validate_make_path(input)?;
        if request.output_root.starts_with(input) || input.starts_with(request.output_root) {
            return Err(failure("Mesa preflight output overlaps an input root"));
        }
    }
    validate_make_path(request.output_root)?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(request.output_root)
        .map_err(|_| failure("Mesa preflight output must be fresh"))?;
    let build = request.output_root.join("configure");
    fs::create_dir(&build).map_err(|_| failure("cannot create Mesa configure root"))?;
    let bash = host_tools.root.join("bash");
    let make = host_tools.root.join("make");
    let ArosCompilerIdentity::Llvm { version } = request.drivers.compiler() else {
        return Err(failure("Mesa source preflight requires LLVM"));
    };
    let mut arguments = vec![
        path_text(&source.join("configure"))?,
        format!("--target={}", request.profile.configure_target()),
        "--with-toolchain=llvm".into(),
        format!("--with-llvm-version={version}"),
        "--with-aros-toolchain=yes".into(),
        "--disable-crosstools".into(),
        format!(
            "--with-aros-toolchain-install={}",
            path_text(compiler_root)?
        ),
        "--with-mesagl=26.0.0".into(),
    ];
    if !request.profile.float_abi().is_empty() {
        arguments.push(format!("--with-float={}", request.profile.float_abi()));
    }
    let mut configure_env = BTreeMap::from([
        ("CONFIG_SHELL".into(), path_text(&bash)?),
        ("SHELL".into(), path_text(&bash)?),
        ("CONFIG_SITE".into(), "/dev/null".into()),
        ("CC".into(), path_text(&host_tools.root.join("cc"))?),
        ("CXX".into(), path_text(&host_tools.root.join("c++"))?),
    ]);
    let mut stages = vec![run(
        (request, cancellation, started),
        "configure",
        &bash,
        &arguments,
        &build,
        &configure_env,
    )?];
    let makefile = read_text(&build.join("Makefile"))?;
    validate_make_roots(&makefile, &build, &source)?;
    let feature_wrapper = measurement_wrapper(&makefile, FEATURE_KEYS, false)?;
    let feature_path = request.output_root.join("feature-inputs.mk");
    write(&feature_path, feature_wrapper.as_bytes())?;
    let args = make_arguments(&feature_path)?;
    configure_env.clear();
    stages.push(run(
        (request, cancellation, started),
        "feature-inputs",
        &make,
        &args,
        &build,
        &configure_env,
    )?);
    let values = parse_values(
        &read_bytes(&request.output_root.join("feature-inputs.stdout"))?,
        FEATURE_KEYS,
    )?;
    if values["AROS_TARGET_CPU"] != request.profile.cpu()
        || values["AROS_TARGET_ARCH"] != request.profile.platform()
    {
        return Err(failure(
            "configured Mesa CPU/platform differs from selected profile",
        ));
    }
    if values["TARGET_CC"] != path_text(request.drivers.clang_path())?
        || values["TARGET_CXX"] != path_text(request.drivers.clangxx_path())?
        || values["TARGET_CPP"] != format!("{} -E", path_text(request.drivers.clang_path())?)
    {
        return Err(failure(
            "configured Mesa feature drivers differ from measured compiler roles",
        ));
    }
    validate_sysroot(&values["TARGET_SYSROOT"], &source, compiler_root)?;
    let generated_path = PathBuf::from(&values["GENDIR"]);
    let generated = super::checked_directory(&generated_path, "Mesa generated configuration root")?;
    if generated != generated_path || !generated.starts_with(&build) {
        return Err(failure(
            "configured Mesa generated directory escaped its fresh build root",
        ));
    }
    let features_env = BTreeMap::from([
        ("CONFIG_SHELL".into(), path_text(&bash)?),
        ("SHELL".into(), path_text(&bash)?),
        ("CONFIG_SITE".into(), "/dev/null".into()),
        ("AROS_TARGET_CPU".into(), values["AROS_TARGET_CPU"].clone()),
        (
            "CC".into(),
            format!("{} {}", values["TARGET_CC"], values["TARGET_SYSROOT"]),
        ),
        (
            "CPP".into(),
            format!("{} {}", values["TARGET_CPP"], values["TARGET_SYSROOT"]),
        ),
        (
            "CXX".into(),
            format!("{} {}", values["TARGET_CXX"], values["TARGET_SYSROOT"]),
        ),
        (
            "CFLAGS".into(),
            format!(
                "{} {} {}",
                values["TARGET_CFLAGS"], values["DEBUG_CFLAGS"], values["OPTIMIZATION_CFLAGS"]
            ),
        ),
        // Match the source Makefile recipe, including its DEBUG_CFLAGS spelling.
        (
            "CXXFLAGS".into(),
            format!(
                " {} {} {}",
                values["TARGET_CXXFLAGS"], values["DEBUG_CFLAGS"], values["OPTIMIZATION_CFLAGS"]
            ),
        ),
        ("AROS_DEVELOPER".into(), values["AROS_DEVELOPER"].clone()),
        ("CONFIG_STATUS".into(), "features.status".into()),
    ]);
    let feature_args = vec![
        path_text(&source.join("config/features"))?,
        format!("--host={}", values["AROS_BUILD_HOST"]),
        format!("--with-config-dir={}/config", path_text(&generated)?),
    ];
    stages.push(run(
        (request, cancellation, started),
        "features",
        &bash,
        &feature_args,
        &build.join("config"),
        &features_env,
    )?);
    let mut keys = BASELINE_KEYS.to_vec();
    keys.extend(MESA_COMPILER_OPTION_VARIABLES.iter().copied());
    keys.extend(BUILTINS.iter().map(|(key, _)| *key));
    let final_wrapper = measurement_wrapper(&makefile, &keys, true)?;
    let final_path = request.output_root.join("configured-values.mk");
    write(&final_path, final_wrapper.as_bytes())?;
    stages.push(run(
        (request, cancellation, started),
        "configured-values",
        &make,
        &make_arguments(&final_path)?,
        &build,
        &BTreeMap::new(),
    )?);
    let measured = parse_values(
        &read_bytes(&request.output_root.join("configured-values.stdout"))?,
        &keys,
    )?;
    let compiler_cfg = read_text(&generated.join("config/compiler.cfg"))?;
    validate_compiler_assignments(&compiler_cfg, &measured)?;
    let invocation = digest_json(
        &serde_json::json!({"stages":stages,"profile":request.profile.document_sha256()}),
    )?;
    let identity = MesaConfigurationIdentity {
        target_profile: request.profile.name().into(),
        target_triple: request.profile.target_triple().into(),
        compiler: request.drivers.compiler().clone(),
        compiler_tree_sha256: request.drivers.compiler_tree_sha256().clone(),
        source_inputs_sha256: request.source_tree_sha256.clone(),
        configure_invocation_sha256: invocation,
    };
    let bytes = configuration_bytes(&identity, &measured)?;
    let configuration = ValidatedMesaConfiguration::parse_bound(&bytes, &identity)
        .map_err(|_| failure("source produced invalid Mesa configuration values"))?;
    validate_selection(request, cancellation)?;
    if started.elapsed() >= request.timeout {
        return Err(failure(
            "Mesa preflight deadline expired before output publication",
        ));
    }
    let configuration_path = request.output_root.join("mesa-configuration.json");
    write(&configuration_path, &bytes)?;
    let receipt = crate::canonical::bytes(&serde_json::json!({
        "schema":"aros-mesa-preflight-v1", "qualification":"local-configure-only",
        "identity":identity,"configuration_sha256":sha256_bytes(&bytes),
        "compiler_cfg_sha256":sha256_bytes(compiler_cfg.as_bytes()),
        "make_values":measured,"configured_paths":values,"stages":stages,
    }))
    .map_err(|_| failure("cannot encode Mesa preflight receipt"))?;
    write(&request.output_root.join("receipt.json"), &receipt)?;
    if started.elapsed() >= request.timeout {
        return Err(failure(
            "Mesa preflight deadline expired during output publication",
        ));
    }
    let completed = CompletedMesaPreflight {
        configuration,
        configuration_path,
        receipt_sha256: sha256_bytes(&receipt),
        make_values: measured,
        source_root: source,
        compiler_root: compiler_root.to_owned(),
        host_root: host_tools.root.clone(),
        profile_sha256: request.profile.document_sha256().clone(),
        environment_sha256: environment::identity(&environment::resolve(request.environment)?)?,
        output_tree: measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot retain Mesa preflight output identity"))?,
    };
    if started.elapsed() >= request.timeout {
        return Err(failure(
            "Mesa preflight deadline expired during final measurement",
        ));
    }
    Ok(completed)
}

fn validate_selection(
    request: &MesaPreflightRequest<'_>,
    cancellation: &CancellationToken,
) -> Result<(), ContractError> {
    if cancellation.is_cancelled()
        || request.timeout.is_zero()
        || request.timeout > Duration::from_secs(600)
        || request.drivers.target().target_triple != request.profile.target_triple()
        || !matches!(
            request.drivers.compiler(),
            ArosCompilerIdentity::Llvm { .. }
        )
    {
        return Err(failure("invalid or cancelled Mesa preflight selection"));
    }
    request.drivers.revalidate(cancellation)?;
    let environment = environment::resolve(request.environment)?;
    if environment
        .variables
        .keys()
        .any(|name| !ENVIRONMENT_KEYS.contains(&name.as_str()))
        || environment
            .variables
            .get("AROS_FETCH_OFFLINE")
            .map(String::as_str)
            != Some("1")
    {
        return Err(failure(
            "Mesa preflight environment contains undeclared inputs or permits fetching",
        ));
    }
    let measured = measure_tree_content_cas(request.source_root)
        .map_err(|_| failure("cannot measure Mesa preflight source"))?
        .payload_digest_excluding(None);
    if &measured != request.source_tree_sha256 {
        return Err(failure("Mesa preflight source changed"));
    }
    Ok(())
}

fn run(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    stage: &str,
    program: &Path,
    arguments: &[String],
    cwd: &Path,
    additions: &BTreeMap<String, String>,
) -> Result<Stage, ContractError> {
    let (request, cancellation, started) = control;
    validate_selection(request, cancellation)?;
    let mut env = environment::resolve(request.environment)?;
    env.variables.insert("LC_ALL".into(), "C".into());
    env.variables.extend(additions.clone());
    let env_sha = environment::identity(&env)?;
    let executable = super::checked_executable(program)?;
    let (program_sha256, _) = super::measure_executable(&executable)?;
    let working_directory =
        super::checked_directory(cwd, "Mesa preflight child working directory")?;
    if working_directory != cwd {
        return Err(failure("Mesa preflight child working directory changed"));
    }
    let mut command = Command::new(program);
    command
        .env_clear()
        .envs(&env.variables)
        .args(arguments)
        .current_dir(cwd);
    let remaining = request
        .timeout
        .checked_sub(started.elapsed())
        .filter(|value| !value.is_zero())
        .ok_or_else(|| failure("Mesa preflight deadline expired"))?;
    let output =
        run_output_with_input_and_control(&mut command, &[], CAPTURE, remaining, cancellation)
            .map_err(|_| failure("cannot supervise Mesa preflight child"))?;
    write(
        &request.output_root.join(format!("{stage}.stdout")),
        output.stdout.rendered_lossy().as_bytes(),
    )?;
    write(
        &request.output_root.join(format!("{stage}.stderr")),
        output.stderr.rendered_lossy().as_bytes(),
    )?;
    let stdout = output
        .stdout
        .exact_bytes()
        .ok_or_else(|| failure("Mesa preflight stdout exceeded capture bound"))?;
    let stderr = output
        .stderr
        .exact_bytes()
        .ok_or_else(|| failure("Mesa preflight stderr exceeded capture bound"))?;
    if output.timed_out || output.cancelled || !output.status.success() {
        return Err(failure(
            "Mesa preflight child failed; inspect retained stage logs",
        ));
    }
    validate_selection(request, cancellation)?;
    Ok(Stage {
        program: program_sha256,
        invocation: digest_json(&serde_json::json!({
            "command":super::command_identity(program, arguments)?,
            "working_directory":working_directory,
        }))?,
        environment: env_sha,
        stdout: sha256_bytes(stdout),
        stderr: sha256_bytes(stderr),
    })
}

fn configuration_bytes(
    identity: &MesaConfigurationIdentity,
    values: &BTreeMap<String, String>,
) -> Result<Vec<u8>, ContractError> {
    let mut selectors = BTreeMap::new();
    for (name, flag) in BUILTINS {
        let value = values
            .get(*name)
            .ok_or_else(|| failure("missing Mesa feature selector"))?;
        if !value.is_empty() && value != flag {
            return Err(failure("unexpected Mesa feature result"));
        }
        selectors.insert(*name, !value.is_empty());
    }
    let words = |name: &str| -> Result<Vec<&str>, ContractError> {
        Ok(values
            .get(name)
            .ok_or_else(|| failure("missing Mesa measured variable"))?
            .split_whitespace()
            .collect())
    };
    let mut compiler_options = BTreeMap::new();
    for name in MESA_COMPILER_OPTION_VARIABLES {
        compiler_options.insert(*name, words(name)?);
    }
    let mut baseline_variables = BTreeMap::new();
    for name in MESA_BASELINE_VARIABLES {
        baseline_variables.insert(*name, words(name)?);
    }
    crate::canonical::bytes(&serde_json::json!({"version":1,"identity":identity,
        "target_isa_flags":words("TARGET_ISA_CFLAGS")?,"builtin_selectors":selectors,
        "compiler_options":compiler_options,"baseline_variables":baseline_variables,
        "mesa_compile_options":words("MESA_COMPILEOPTIONS")?}))
    .map_err(|_| failure("cannot serialize Mesa measured configuration"))
}
fn validate_compiler_assignments(
    text: &str,
    measured: &BTreeMap<String, String>,
) -> Result<(), ContractError> {
    let mut assignments = BTreeMap::new();
    for line in text.lines() {
        if let Some((name, value)) = line.split_once(":=") {
            if assignments.insert(name.trim(), value.trim()).is_some() {
                return Err(failure("duplicate generated compiler feature assignment"));
            }
        }
    }
    for name in MESA_COMPILER_OPTION_VARIABLES
        .iter()
        .copied()
        .chain(BUILTINS.iter().map(|(name, _)| *name))
    {
        let value = assignments
            .get(name)
            .ok_or_else(|| failure("generated compiler feature assignment missing"))?;
        if measured.get(name).map(String::as_str) != Some(*value) {
            return Err(failure(
                "measured compiler feature differs from generated assignment",
            ));
        }
    }
    Ok(())
}
fn make_arguments(path: &Path) -> Result<Vec<String>, ContractError> {
    Ok(vec![
        "--silent".into(),
        "--no-builtin-rules".into(),
        "--no-builtin-variables".into(),
        "-f".into(),
        path_text(path)?,
        "aros-mesa-preflight-values".into(),
    ])
}
fn measurement_wrapper(
    makefile: &str,
    keys: &[&str],
    include_features: bool,
) -> Result<String, ContractError> {
    let mut result = wrapper(makefile, keys)?;
    if include_features {
        // make.cfg establishes roots and baseline flags, but does not load the
        // compiler feature results. Require the freshly generated file before
        // expanding any Mesa options; a missing file must remain a hard error.
        result = result.replacen(
            "include $(TOP)/config/make.cfg\n",
            "include $(TOP)/config/make.cfg\ninclude $(GENDIR)/config/compiler.cfg\n",
            1,
        );
    }
    for key in keys.iter().filter(|name| **name != "MESA_COMPILEOPTIONS") {
        writeln!(result, "$(if $(filter undefined,$(origin {key})),$(error missing configured Mesa variable {key}))")
            .map_err(|_| failure("cannot encode Mesa measurement guard"))?;
    }
    if result.len() > 64 * 1024 {
        return Err(failure(
            "guarded Mesa measurement wrapper exceeds its byte bound",
        ));
    }
    Ok(result)
}
fn validate_make_roots(makefile: &str, build: &Path, source: &Path) -> Result<(), ContractError> {
    let boundary = "include $(TOP)/config/make.cfg\n";
    let prefix = makefile
        .split_once(boundary)
        .ok_or_else(|| failure("missing Mesa config include"))?
        .0;
    for (name, path) in [("TOP", build), ("SRCDIR", source)] {
        validate_make_path(path)?;
        let mut selected = None;
        let mut conditional_depth = 0_usize;
        for line in prefix
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
        {
            let directive = line.split_whitespace().next().unwrap_or_default();
            if matches!(directive, "ifdef" | "ifndef" | "ifeq" | "ifneq") {
                conditional_depth += 1;
            } else if directive == "endif" {
                conditional_depth = conditional_depth.saturating_sub(1);
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some((left, right)) = line.split_once('=') {
                let variable = left
                    .trim_end()
                    .trim_end_matches([':', '?', '+', '!'])
                    .trim_end();
                if variable == name
                    && (conditional_depth != 0
                        || !left.trim_end().ends_with(':')
                        || selected.replace(right.trim()).is_some())
                {
                    return Err(failure("ambiguous generated Mesa configuration root"));
                }
            }
        }
        if selected != Some(path_text(path)?.as_str()) {
            return Err(failure(
                "generated Mesa configuration reads a different source/build root",
            ));
        }
    }
    Ok(())
}
fn validate_make_path(path: &Path) -> Result<(), ContractError> {
    let value = path_text(path)?;
    if value.chars().any(char::is_whitespace)
        || value.bytes().any(|byte| b"$#\\*?[]".contains(&byte))
    {
        return Err(failure(
            "Mesa preflight path is not literal GNU Make path text",
        ));
    }
    Ok(())
}
fn validate_sysroot(value: &str, source: &Path, compiler: &Path) -> Result<(), ContractError> {
    if value.is_empty() {
        return Ok(());
    }
    let path = value
        .strip_prefix("--sysroot=")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| failure("unsupported configured Mesa sysroot syntax"))?;
    let path = Path::new(path);
    validate_make_path(path)?;
    let canonical = super::checked_directory(path, "Mesa feature sysroot")?;
    if canonical != path || !(path.starts_with(source) || path.starts_with(compiler)) {
        return Err(failure(
            "Mesa feature sysroot is outside remeasured source/compiler inputs",
        ));
    }
    Ok(())
}
fn read_bytes(path: &Path) -> Result<Vec<u8>, ContractError> {
    measure_regular_file_bounded(path, FILE_LIMIT)
        .map_err(|_| failure("cannot safely read Mesa generated file"))?
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| failure("Mesa generated file is absent"))
}
fn read_text(path: &Path) -> Result<String, ContractError> {
    String::from_utf8(read_bytes(path)?).map_err(|_| failure("Mesa generated text is not UTF-8"))
}
fn write(path: &Path, bytes: &[u8]) -> Result<(), ContractError> {
    publish_atomic_file(path, bytes, AtomicFilePolicy::NoClobber)
        .map(|_| ())
        .map_err(|_| failure("cannot retain fresh Mesa preflight evidence"))
}
fn digest_json(value: &serde_json::Value) -> Result<Sha256Digest, ContractError> {
    crate::canonical::bytes(value)
        .map(|bytes| sha256_bytes(&bytes))
        .map_err(|_| failure("cannot encode Mesa identity"))
}
fn path_text(path: &Path) -> Result<String, ContractError> {
    path.to_str()
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_owned)
        .ok_or_else(|| failure("invalid Mesa preflight path"))
}
fn failure(message: &str) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
#[path = "mesa_preflight_tests.rs"]
mod tests;
