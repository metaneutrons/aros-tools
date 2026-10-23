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
use std::time::{Duration, Instant};

use aros_common::make_initial_values::{
    ValidatedMakeInitialValues, MAKE_INITIAL_VALUES_METADATA_KEY,
};
use aros_common::make_namespace::{ValidatedMakeNamespace, MAKE_NAMESPACE_METADATA_KEY};
use aros_common::mesa_configuration::{
    MesaConfigurationIdentity, ValidatedMesaConfiguration, MESA_BASELINE_VARIABLES,
    MESA_COMPILER_OPTION_VARIABLES, MESA_UNDEFINED_SCOPE_VARIABLES,
};
use aros_common::mesa_configuration_input::{MesaConfigurationInputDocument, INPUT_FILE};
use aros_common::{
    measure_regular_file_bounded, measure_tree_content_cas, publish_atomic_file,
    run_output_with_input_and_control, sha256_bytes, ArosCompilerIdentity, AtomicFilePolicy,
    CancellationToken, Sha256Digest, TreeContentCas,
};
use serde::Serialize;

use super::mesa_preflight_llvm::{
    from_measurements as llvm_configuration, LLVM_MEASUREMENT_ASSIGNMENTS, LLVM_MEASUREMENT_KEYS,
};
use super::mesa_preflight_measurement::{parse_values, wrapper};
use super::mesa_preflight_profile::MesaPreflightProfile;
use super::mesa_preflight_profile::SourceMesaPreflightProfile;
use super::mesa_preflight_recipe::{
    from_measurements as recipe_configuration, RECIPE_MEASUREMENT_ASSIGNMENTS,
    RECIPE_MEASUREMENT_KEYS,
};
use super::{environment, CompatibilityEnvironment, LlvmSdkDriverBinding};
use crate::ContractError;
use aros_common::mesa_genmodule_inventory::MesaGenmoduleInventoryScope;

#[path = "mesa_preflight_genmodule.rs"]
mod genmodule;
#[path = "mesa_preflight_invocation.rs"]
pub(super) mod invocation;
#[path = "mesa_preflight_native_collection_observer.rs"]
mod native_collection_observer;
#[path = "mesa_preflight_native_root.rs"]
mod native_root;
#[path = "mesa_preflight_preparation.rs"]
mod preparation;
pub use native_root::{
    GeneratedNativeMakeInclude, NativeMakeIncludeExecution,
    build_mesa_native_host, collect_mesa_native_root, join_mesa_native_source,
    observe_mesa_native_dispatch, observe_mesa_native_launch_inputs, observe_mesa_native_root,
    prepare_mesa_native_direct_collection, BuiltMesaNativeHost, CollectedMesaNativeDirect,
    CollectedMesaNativeRoot, ExecutedMesa26OriginalRecipe, Mesa26GeneratedPathListSelection,
    Mesa26OriginalRecipeExecution, MesaNativeCollectionSelection, MesaNativeDirectOrigin,
    MesaNativeDispatchSelection, MesaNativeHostBuildSelection, MesaNativeRootQuerySelection,
    MesaNativeSourceSelection, ObservedFencedMesaNativeDispatch, ObservedMesaNativeDispatch,
    ObservedMesaNativeLaunchInputs, ObservedMesaNativeRoot, PreparedMesaNativeDirectCollection,
    RecordedMesaNativeDispatch, SourceJoinedMesaNativeRoot,
};
#[path = "mesa_preflight_scope.rs"]
mod scope;

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
    /// Exact local CLI snapshot; None preserves complete-tree source semantics.
    pub local_source: Option<&'a aros_common::local_source::LocalSourceIdentity>,
    pub profile: MesaPreflightProfile<'a>,
    /// Explicit source graph scope and measured helper. None retains the
    /// historical singleton path, not complete multi-module qualification.
    pub genmodule_selection: Option<MesaGenmodulePreflightSelection<'a>>,
    /// Existing measured drivers; package authentication remains the caller's duty.
    pub drivers: &'a LlvmSdkDriverBinding,
    pub environment: &'a CompatibilityEnvironment,
    /// Fresh leaf outside source/compiler/host-tool inputs. Failures are retained.
    pub output_root: &'a Path,
    /// Whole preflight deadline (at most 20 minutes), never renewed per subprocess.
    pub timeout: Duration,
}

/// Caller-selected source profile/scope and exact inventory helper executable.
#[derive(Clone, Copy)]
pub struct MesaGenmodulePreflightSelection<'a> {
    pub source_profile: &'a SourceMesaPreflightProfile,
    pub scope: MesaGenmoduleInventoryScope,
    pub transpiler: &'a Path,
    pub transpiler_sha256: &'a Sha256Digest,
}

/// A new inventory stage policy, not replacement Source or invocation data.
pub struct NativeGenmoduleInventorySelection<'a> {
    pub helper: &'a Path,
    pub helper_sha256: &'a Sha256Digest,
    pub scope: MesaGenmoduleInventoryScope,
    pub evidence_root: &'a Path,
}

/// Source-helper inventory joined to the original live collection's call slots.
/// No generator output, effective-owner scope or execution authority is added.
pub struct SelectedNativeGenmoduleInventory {
    inventory: aros_common::mesa_genmodule_inventory::ValidatedMesaGenmoduleSelection,
    owners: Vec<aros_common::mesa_native_make::MesaNativeOwnerCall>,
    directory: aros_common::make_native_invocation::PrivateNativeCaptureDirectory,
    evidence: aros_common::TreeContentCas,
    owner_sha256: Sha256Digest,
    stage: Stage,
}

/// Explicit original child recipe and private output for one fixed data operation.
/// This does not select or execute a project recipe or discharge its stamp.
pub struct NativeGenmoduleMakeSelection<'a> {
    pub owner: &'a str,
    pub recipe_index: usize,
    /// Original recipe's independently selected nominal writefiles destination.
    pub nominal_destination: &'a Path,
    /// Fresh external directory, disjoint from every retained input/evidence root.
    pub evidence_root: &'a Path,
}

/// One original-owner-bound Make data output, not project execution or SDK proof.
pub struct NativeGenmoduleMakeData {
    directory: aros_common::make_native_invocation::PrivateNativeCaptureDirectory,
    evidence: TreeContentCas,
    stage: Stage,
    source_input: PathBuf,
    makefile: aros_common::mesa_genmodule_make::MesaGenmoduleMakefile,
    output: aros_common::mesa_genmodule_collection::MesaGenmoduleOutputBinding,
    owner_sha256: Sha256Digest,
    inventory_sha256: Sha256Digest,
}

/// Completed local producer; private fields prevent construction from a data file.
pub struct CompletedMesaPreflight {
    // Live runner state only. A portable receipt cannot reconstruct this map
    // or authorize a subsequent native observation from its digest alone.
    configured_invocation: invocation::InvocationContext,
    configuration: ValidatedMesaConfiguration,
    configuration_path: PathBuf,
    receipt_sha256: Sha256Digest,
    input_sha256: Sha256Digest,
    make_values: BTreeMap<String, String>,
    source_root: PathBuf,
    local_source: Option<aros_common::local_source::LocalSourceIdentity>,
    compiler_root: PathBuf,
    host_root: PathBuf,
    profile_sha256: Sha256Digest,
    environment_sha256: Sha256Digest,
    genmodule_selection_sha256: Option<Sha256Digest>,
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

    /// Build the data-channel arguments only after rechecking the actual producer.
    ///
    /// These arguments transport values, not producer authority. The execution
    /// owner must retain this completed result, revalidate after graph execution,
    /// and bind its receipt into its own execution/readback evidence.
    ///
    /// # Errors
    /// Rejects changed producer inputs/outputs or a non-UTF-8 input path.
    pub fn cmake_configuration_arguments(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<String>, ContractError> {
        self.revalidate(request, cancellation)?;
        let input = request.output_root.join(INPUT_FILE);
        let input = input
            .to_str()
            .ok_or_else(|| failure("Mesa configuration input path is not UTF-8"))?;
        Ok(vec![
            "-DAROS_MESA_CONFIGURATION_MODE=bound".into(),
            format!("-DAROS_MESA_CONFIGURATION_INPUT={input}"),
            format!(
                "-DAROS_MESA_CONFIGURATION_INPUT_SHA256={}",
                self.input_sha256
            ),
        ])
    }

    /// Snapshot the fixed semantic evidence only while retaining the actual
    /// completed producer. The rootless reader cannot recreate this authority.
    pub(super) fn data_evidence(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<
        (
            super::mesa_preflight_evidence::MesaPreflightDataProof,
            BTreeMap<String, Vec<u8>>,
        ),
        ContractError,
    > {
        self.revalidate(request, cancellation)?;
        let receipt = read_bytes(&request.output_root.join("receipt.json"))?;
        let members =
            super::mesa_preflight_evidence::expected_members(request.output_root, &receipt)?;
        let files = members
            .into_iter()
            .map(|(name, (path, limit))| {
                let (_, bytes) = measure_regular_file_bounded(&path, limit as u64)
                    .map_err(|_| failure("cannot snapshot regular Mesa evidence member"))?
                    .ok_or_else(|| failure("required Mesa evidence member is missing"))?;
                Ok((name, bytes))
            })
            .collect::<Result<BTreeMap<_, _>, ContractError>>()?;
        let proof = super::mesa_preflight_evidence::readback(
            &files,
            &super::mesa_preflight_evidence::MesaPreflightEvidenceExpectations {
                profile_name: request.profile.name(),
                target_triple: request.profile.target_triple(),
                profile_document_sha256: request.profile.document_sha256(),
                compiler: request.drivers.compiler(),
                compiler_tree_sha256: request.drivers.compiler_tree_sha256(),
                compiler_root: Some(request.drivers.compiler_root()),
                source_tree_sha256: request.source_tree_sha256,
                cpu: request.profile.cpu(),
                platform: request.profile.platform(),
                clang_path: request.drivers.clang_path(),
                clangxx_path: request.drivers.clangxx_path(),
                output_root: request.output_root,
                genmodule_selection: request.genmodule_selection.map(|selection| {
                    selection
                        .source_profile
                        .genmodule_inventory_binding(selection.scope)
                }),
                genmodule_helper_sha256: request
                    .genmodule_selection
                    .map(|selection| selection.transpiler_sha256.clone()),
            },
        )?;
        if proof.receipt_sha256() != self.receipt_sha256()
            || proof.input_sha256() != &self.input_sha256
            || proof.configuration_sha256() != &sha256_bytes(&read_bytes(&self.configuration_path)?)
            || proof.configuration_identity() != self.configuration.identity()
        {
            return Err(failure(
                "Mesa evidence differs from its retained private producer",
            ));
        }
        self.revalidate(request, cancellation)?;
        Ok((proof, files))
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
        self.configured_invocation.revalidate()?;
        validate_selection(request, cancellation)?;
        let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment
        else {
            return Err(failure(
                "Mesa preflight revalidation lost sealed host tools",
            ));
        };
        if request.source_root != self.source_root
            || request.local_source != self.local_source.as_ref()
            || request.source_tree_sha256 != &self.configuration.identity().source_inputs_sha256
            || request.drivers.compiler() != &self.configuration.identity().compiler
            || request.drivers.compiler_tree_sha256()
                != &self.configuration.identity().compiler_tree_sha256
            || request.drivers.compiler_root() != self.compiler_root.as_path()
            || host_tools.root != self.host_root
            || request.profile.document_sha256() != &self.profile_sha256
            || genmodule_request_identity(request)? != self.genmodule_selection_sha256
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
    // Only the private live owner can supply this context. Portable receipts
    // retain the existing digest contract, never raw environment/argv values.
    #[serde(skip)]
    invocation_context: invocation::InvocationContext,
    #[serde(skip)]
    name: String,
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
    prepare_mesa_preflight(request, cancellation)?.complete(request, cancellation)
}

// Kept internal: preparing source configuration is not a completed receipt or
// authority to execute an arbitrary native goal. Additional native preparation
// must acquire its own closed stage binding before it may reach completion.
fn prepare_mesa_preflight(
    request: &MesaPreflightRequest<'_>,
    cancellation: &CancellationToken,
) -> Result<preparation::PreparedMesaPreflight, ContractError> {
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
    let compiler_root = request.drivers.compiler_root();
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
    if let Some(local) = request.local_source {
        arguments.push(format!(
            "--with-mmake-ignore-directory={}",
            local.generated_subtree
        ));
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
    keys.extend(LLVM_MEASUREMENT_KEYS.iter().copied());
    keys.extend(RECIPE_MEASUREMENT_KEYS.iter().copied());
    keys.push(MAKE_NAMESPACE_METADATA_KEY);
    keys.push(MAKE_INITIAL_VALUES_METADATA_KEY);
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
    for (key, expected) in [
        ("MESA_MEASURE_TOP", path_text(&build)?),
        ("MESA_MEASURE_SRCDIR", path_text(&source)?),
        ("MESA_MEASURE_PORTSDIR", values["PORTSDIR"].clone()),
        ("MESA_MEASURE_GENDIR", path_text(&generated)?),
    ] {
        if measured[key] != expected {
            return Err(failure(
                "final Mesa Make coordinates differ from selected configure inputs",
            ));
        }
    }
    if measured["TARGET_CC"] != path_text(request.drivers.clang_path())?
        || measured["AROS_CXX"] != path_text(request.drivers.clangxx_path())?
    {
        return Err(failure("final Mesa recipe drivers changed after features"));
    }
    validate_final_sysroot(
        &measured["TARGET_SYSROOT"],
        &values["TARGET_SYSROOT"],
        &values["AROS_DEVELOPER"],
        &build,
    )?;
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
    // Admit the complete measured option data before starting a host build.
    // The final identity below also binds the two original-generator stages.
    let bytes = configuration_bytes(&identity, &measured)?;
    let initial_configuration = ValidatedMesaConfiguration::parse_bound(&bytes, &identity)
        .map_err(|_| failure("source produced invalid Mesa configuration values"))?;
    preparation::PreparedMesaPreflight::retain(
        request,
        cancellation,
        preparation::ConfiguredMesaSource {
            started,
            source,
            initial_configuration,
            measured,
            values,
            compiler_cfg,
            make,
            final_path,
            stages,
        },
    )
}

fn complete_mesa_preflight(
    request: &MesaPreflightRequest<'_>,
    cancellation: &CancellationToken,
    prepared: preparation::PreparedMesaPreflight,
) -> Result<CompletedMesaPreflight, ContractError> {
    // Consume the live preparation once. Serialized data cannot recreate it;
    // intervening output/input/selection changes refuse before a new child.
    prepared.revalidate(request, cancellation)?;
    let preparation::ConfiguredMesaSource {
        started,
        source,
        initial_configuration,
        measured,
        values,
        compiler_cfg,
        make,
        final_path,
        mut stages,
    } = prepared.into_configuration();
    let mut identity = initial_configuration.identity().clone();
    let compiler_root = request.drivers.compiler_root();
    let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment else {
        return Err(failure(
            "Mesa completion lost its sealed host-tool environment",
        ));
    };
    let configured_stdout_sha256 = stages[3].stdout.clone();
    let (genmodule, genmodule_collection) = if request.genmodule_selection.is_some() {
        (
            None,
            Some(genmodule::produce_collection(
                (request, cancellation, started),
                &make,
                &final_path,
                &values["PORTSDIR"],
                &initial_configuration,
                &configured_stdout_sha256,
                &mut stages,
            )?),
        )
    } else {
        (
            Some(genmodule::produce(
                (request, cancellation, started),
                &make,
                &final_path,
                &mut stages,
            )?),
            None,
        )
    };
    let version = match (
        request.local_source.is_some(),
        genmodule_collection.is_some(),
    ) {
        (false, false) => 3,
        (true, false) => 4,
        (false, true) => 7,
        (true, true) => 8,
    };
    identity.configure_invocation_sha256 = digest_json(
        &serde_json::json!({"stages":stages,"profile":request.profile.document_sha256()}),
    )?;
    let bytes = configuration_bytes(&identity, &measured)?;
    let configuration = ValidatedMesaConfiguration::parse_bound(&bytes, &identity)
        .map_err(|_| failure("source produced invalid Mesa configuration values"))?;
    let mut make_values = measured;
    make_values.remove(MAKE_NAMESPACE_METADATA_KEY);
    make_values.remove(MAKE_INITIAL_VALUES_METADATA_KEY);
    for key in LLVM_MEASUREMENT_KEYS {
        make_values.remove(*key);
    }
    for key in RECIPE_MEASUREMENT_KEYS
        .iter()
        .filter(|key| key.starts_with("MESA_MEASURE_"))
    {
        make_values.remove(*key);
    }
    if let Some(safety) = configuration.make_variables().get("SAFETY_CFLAGS") {
        make_values.insert("SAFETY_CFLAGS".into(), safety.clone());
    }
    make_values.extend(configuration.llvm_configuration().defined_variables());
    if make_values != configuration.make_variables() {
        return Err(failure(
            "measured values differ from validated Mesa Make projection",
        ));
    }
    validate_selection(request, cancellation)?;
    if started.elapsed() >= request.timeout {
        return Err(failure(
            "Mesa preflight deadline expired before output publication",
        ));
    }
    let configuration_path = request.output_root.join("mesa-configuration.json");
    write(&configuration_path, &bytes)?;
    let mut receipt_document = serde_json::json!({
        "schema":format!("aros-mesa-preflight-v{version}"),
        "qualification":"local-configure-only",
        "identity":identity,"configuration_sha256":sha256_bytes(&bytes),
        "compiler_cfg_sha256":sha256_bytes(compiler_cfg.as_bytes()),
        "compiler_programs":request.drivers.program_selection("target-c", "target-cxx"),
        "make_values":make_values,"configured_paths":values,"stages":stages,
    });
    // Absent optional bindings are omitted, matching the canonical reader and
    // preserving historical receipts without a collection field.
    if let Some(binding) = &genmodule {
        receipt_document["genmodule"] = serde_json::to_value(binding)
            .map_err(|_| failure("cannot encode Mesa genmodule binding"))?;
    }
    if let Some(binding) = &genmodule_collection {
        receipt_document["genmodule_collection"] = serde_json::to_value(binding)
            .map_err(|_| failure("cannot encode Mesa genmodule collection"))?;
    }
    let receipt = crate::canonical::bytes(&receipt_document)
        .map_err(|_| failure("cannot encode Mesa preflight receipt"))?;
    write(&request.output_root.join("receipt.json"), &receipt)?;
    // The child receives a data-only projection. Its consistency checks do not
    // replace this private completed producer and the execution owner's proof.
    let input = serde_json::to_value(MesaConfigurationInputDocument {
        version,
        identity,
        configuration_sha256: sha256_bytes(&bytes),
        receipt_sha256: sha256_bytes(&receipt),
        local_source: request.local_source.cloned(),
        genmodule,
        genmodule_collection,
    })
    .map_err(|_| failure("cannot serialize Mesa transpiler input"))?;
    let input = crate::canonical::bytes(&input)
        .map_err(|_| failure("cannot encode Mesa transpiler input"))?;
    write(&request.output_root.join(INPUT_FILE), &input)?;
    if started.elapsed() >= request.timeout {
        return Err(failure(
            "Mesa preflight deadline expired during output publication",
        ));
    }
    let configured_invocation = stages
        .get(3)
        .filter(|stage| stage.name == "configured-values")
        .ok_or_else(|| failure("completed Mesa preflight lost its configured invocation"))?
        .invocation_context
        .clone();
    let completed = CompletedMesaPreflight {
        configured_invocation,
        configuration,
        configuration_path,
        receipt_sha256: sha256_bytes(&receipt),
        input_sha256: sha256_bytes(&input),
        make_values,
        source_root: source,
        local_source: request.local_source.cloned(),
        compiler_root: compiler_root.to_owned(),
        host_root: host_tools.root.clone(),
        profile_sha256: request.profile.document_sha256().clone(),
        environment_sha256: environment::identity(&environment::resolve(request.environment)?)?,
        genmodule_selection_sha256: genmodule_request_identity(request)?,
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
    validate_selection_with_source(request, cancellation, None)
}

fn validate_selection_with_source(
    request: &MesaPreflightRequest<'_>,
    cancellation: &CancellationToken,
    source: Option<&aros_common::mesa_source::MesaSourceSnapshot>,
) -> Result<(), ContractError> {
    if cancellation.is_cancelled()
        || request.timeout.is_zero()
        || request.timeout > Duration::from_mins(60)
        || request.drivers.target().target_triple != request.profile.target_triple()
        || request
            .profile
            .source_root()
            .is_some_and(|root| root != request.source_root)
        || request
            .profile
            .source_target()
            .is_some_and(|target| target != request.drivers.target())
        || !matches!(
            request.drivers.compiler(),
            ArosCompilerIdentity::Llvm { .. }
        )
    {
        return Err(failure("invalid or cancelled Mesa preflight selection"));
    }
    request.drivers.revalidate(cancellation)?;
    if let Some(selection) = request.genmodule_selection {
        selection.source_profile.revalidate()?;
        if selection.source_profile.source_root() != request.source_root
            || selection.source_profile.target() != request.drivers.target()
        {
            return Err(failure(
                "genmodule inventory source/profile differs from the selected compiler",
            ));
        }
        let executable = super::checked_executable(selection.transpiler)?;
        if executable != selection.transpiler
            || &super::measure_executable(&executable)?.0 != selection.transpiler_sha256
        {
            return Err(failure("selected genmodule inventory helper changed"));
        }
    }
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
    if let Some(source) = source {
        source
            .verify_selection(
                request.source_root,
                request.source_tree_sha256,
                request.local_source,
            )
            .map_err(|_| failure("Mesa preflight source changed"))?;
    } else {
        aros_common::mesa_source::MesaSourceSnapshot::capture(
            request.source_root,
            request.source_tree_sha256,
            request.local_source,
        )
        .map_err(|_| failure("Mesa preflight source changed"))?;
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
    let (request, _, _) = control;
    let mut env = environment::resolve(request.environment)?;
    env.variables.insert("LC_ALL".into(), "C".into());
    env.variables.extend(additions.clone());
    run_prepared(
        control,
        stage,
        invocation::InvocationContext::new(program, arguments, cwd, env)?,
    )
}

fn run_inheriting(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    stage: &str,
    program: &Path,
    arguments: &[String],
    cwd: &Path,
    parent: &Stage,
) -> Result<Stage, ContractError> {
    parent.invocation_context.revalidate()?;
    let context = invocation::InvocationContext::new(
        program,
        arguments,
        cwd,
        parent.invocation_context.environment().clone(),
    )?;
    run_prepared(control, stage, context)
}

fn run_prepared(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    stage: &str,
    invocation_context: invocation::InvocationContext,
) -> Result<Stage, ContractError> {
    run_prepared_outcome(control, stage, invocation_context, 0).map(|(stage, _)| stage)
}

/// Observe exactly one selected process outcome under the original owner clock.
/// A nonzero expected outcome is not phase success: its caller must additionally
/// validate the phase-specific protocol before admitting any preparation result.
fn run_prepared_outcome(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    stage: &str,
    invocation_context: invocation::InvocationContext,
    expected_exit: i32,
) -> Result<(Stage, i32), ContractError> {
    run_prepared_observe(
        control,
        stage,
        invocation_context,
        expected_exit,
        |stdout, stderr| {
            write(
                &control.0.output_root.join(format!("{stage}.stdout")),
                stdout,
            )?;
            write(
                &control.0.output_root.join(format!("{stage}.stderr")),
                stderr,
            )
        },
    )
}

fn run_prepared_observe(
    control: (&MesaPreflightRequest<'_>, &CancellationToken, Instant),
    stage: &str,
    invocation_context: invocation::InvocationContext,
    expected_exit: i32,
    retain: impl FnOnce(&[u8], &[u8]) -> Result<(), ContractError>,
) -> Result<(Stage, i32), ContractError> {
    if !(0..=255).contains(&expected_exit) {
        return Err(failure(
            "Mesa preflight selected an invalid process outcome",
        ));
    }
    let (request, cancellation, started) = control;
    validate_selection(request, cancellation)?;
    invocation_context.revalidate()?;
    let mut command = invocation_context.command();
    let remaining = request
        .timeout
        .checked_sub(started.elapsed())
        .filter(|value| !value.is_zero())
        .ok_or_else(|| failure("Mesa preflight deadline expired"))?;
    let output =
        run_output_with_input_and_control(&mut command, &[], CAPTURE, remaining, cancellation)
            .map_err(|_| failure("cannot supervise Mesa preflight child"))?;
    retain(
        &output.stdout.exact_bytes().map_or_else(
            || output.stdout.rendered_lossy().into_bytes(),
            <[u8]>::to_vec,
        ),
        &output.stderr.exact_bytes().map_or_else(
            || output.stderr.rendered_lossy().into_bytes(),
            <[u8]>::to_vec,
        ),
    )?;
    let stdout = output
        .stdout
        .exact_bytes()
        .ok_or_else(|| failure("Mesa preflight stdout exceeded capture bound"))?;
    let stderr = output
        .stderr
        .exact_bytes()
        .ok_or_else(|| failure("Mesa preflight stderr exceeded capture bound"))?;
    if output.timed_out || output.cancelled || output.status.code() != Some(expected_exit) {
        return Err(failure(
            "Mesa preflight child failed; inspect retained stage logs",
        ));
    }
    validate_selection(request, cancellation)?;
    invocation_context.revalidate()?;
    let observed_exit = output
        .status
        .code()
        .ok_or_else(|| failure("Mesa preflight child terminated without a process exit code"))?;
    Ok((
        Stage {
            program: invocation_context.program_sha256().clone(),
            invocation: invocation_context.invocation_sha256().clone(),
            environment: invocation_context.environment_sha256().clone(),
            stdout: sha256_bytes(stdout),
            stderr: sha256_bytes(stderr),
            invocation_context,
            name: stage.to_owned(),
        },
        observed_exit,
    ))
}

fn genmodule_request_identity(
    request: &MesaPreflightRequest<'_>,
) -> Result<Option<Sha256Digest>, ContractError> {
    request
        .genmodule_selection
        .map(|selection| {
            digest_json(&serde_json::json!({
                "selection": selection.source_profile.genmodule_inventory_binding(selection.scope),
                "helper": selection.transpiler,
                "helper_sha256": selection.transpiler_sha256,
            }))
        })
        .transpose()
}

fn configuration_bytes(
    identity: &MesaConfigurationIdentity,
    values: &BTreeMap<String, String>,
) -> Result<Vec<u8>, ContractError> {
    let namespace = ValidatedMakeNamespace::from_measurement(
        values
            .get(MAKE_NAMESPACE_METADATA_KEY)
            .ok_or_else(|| failure("missing configured Make namespace"))?,
    )
    .map_err(|_| failure("source produced invalid Make namespace"))?;
    let initial_values = ValidatedMakeInitialValues::from_measurement(
        values
            .get(MAKE_INITIAL_VALUES_METADATA_KEY)
            .ok_or_else(|| failure("missing configured Make initial values"))?,
        &namespace,
    )
    .map_err(|_| failure("source produced invalid Make initial values"))?;
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
    crate::canonical::bytes(&serde_json::json!({"version":3,"identity":identity,
        "initial_namespace":namespace.to_observation(),
        "initial_values":initial_values.to_observation(),
        "target_isa_flags":words("TARGET_ISA_CFLAGS")?,"builtin_selectors":selectors,
        "compiler_options":compiler_options,"baseline_variables":baseline_variables,
        "mesa_compile_options":words("MESA_COMPILEOPTIONS")?,
        "undefined_variables":MESA_UNDEFINED_SCOPE_VARIABLES,
        "recipe_configuration":recipe_configuration(values)?,
        "llvm_configuration":llvm_configuration(values)?}))
    .map_err(|_| failure("cannot serialize Mesa measured configuration"))
}
pub(super) fn validate_compiler_assignments(
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
        "-f".into(),
        path_text(path)?,
        "aros-mesa-preflight-values".into(),
    ])
}
// Retain ordinary file/default values as raw data, not a second expansion.
// The namespace is captured before introducing either private helper. Special
// dot variables and non-file/default origins keep their existing unresolved
// semantics. The matching data parser requires the exact eligible name set.
fn initial_values_fragment() -> String {
    aros_common::make_observer::initial_values_fragment()
}

fn measurement_wrapper(
    makefile: &str,
    keys: &[&str],
    include_features: bool,
) -> Result<String, ContractError> {
    if keys
        .iter()
        .filter(|key| **key == MAKE_NAMESPACE_METADATA_KEY)
        .count()
        != usize::from(include_features)
    {
        return Err(failure(
            "Make namespace metadata must occur exactly once in final measurement",
        ));
    }
    if keys
        .iter()
        .filter(|key| **key == MAKE_INITIAL_VALUES_METADATA_KEY)
        .count()
        != usize::from(include_features)
    {
        return Err(failure(
            "Make initial values metadata must occur exactly once in final measurement",
        ));
    }
    let value_keys = keys
        .iter()
        .copied()
        .filter(|key| {
            ![
                MAKE_NAMESPACE_METADATA_KEY,
                MAKE_INITIAL_VALUES_METADATA_KEY,
            ]
            .contains(key)
        })
        .collect::<Vec<_>>();
    let mut result = wrapper(makefile, &value_keys)?;
    if include_features {
        // make.cfg establishes roots and baseline flags, but does not load the
        // compiler feature results. Require the freshly generated file before
        // expanding any Mesa options; a missing file must remain a hard error.
        result = result.replacen(
            "include $(TOP)/config/make.cfg\n",
            "include $(TOP)/config/make.cfg\ninclude $(GENDIR)/config/compiler.cfg\n",
            1,
        );
        result = result.replacen(
            "include $(GENDIR)/config/compiler.cfg\n",
            &format!("include $(GENDIR)/config/compiler.cfg\n\
$(if $(filter undefined,$(origin __AROS_NAMESPACE_ITEM)),,$(error Make namespace iterator collision))\n\
$(info {MAKE_NAMESPACE_METADATA_KEY}=v1:$(words $(sort $(.VARIABLES)));$(foreach __AROS_NAMESPACE_ITEM,$(sort $(.VARIABLES)),$(__AROS_NAMESPACE_ITEM)|$(origin $(__AROS_NAMESPACE_ITEM));)end)\n\
{}{LLVM_MEASUREMENT_ASSIGNMENTS}{RECIPE_MEASUREMENT_ASSIGNMENTS}", initial_values_fragment()),
            1,
        );
        for name in MESA_UNDEFINED_SCOPE_VARIABLES {
            writeln!(result, "$(if $(filter undefined,$(origin {name})),,$(error configured Mesa source-local variable {name} must be undefined before source evaluation))")
                .map_err(|_| failure("cannot encode Mesa undefined-variable observation"))?;
        }
    }
    for key in value_keys
        .iter()
        .filter(|name| **name != "MESA_COMPILEOPTIONS")
    {
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

fn validate_final_sysroot(
    final_value: &str,
    feature_value: &str,
    developer: &str,
    build: &Path,
) -> Result<(), ContractError> {
    if final_value == feature_value {
        return Ok(());
    }
    let path = final_value
        .strip_prefix("--sysroot=")
        .or_else(|| final_value.strip_prefix("--sysroot "))
        .ok_or_else(|| failure("unsupported final Mesa recipe sysroot syntax"))?;
    let path = Path::new(path);
    validate_make_path(path)?;
    let canonical = super::checked_directory(path, "final Mesa recipe sysroot")?;
    if canonical != path
        || path != Path::new(developer)
        || !path.starts_with(build)
        || path == build
    {
        return Err(failure(
            "final Mesa recipe sysroot is not its selected private developer root",
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

#[cfg(test)]
#[path = "mesa_preflight_root_capture_tests.rs"]
mod root_capture_tests;
