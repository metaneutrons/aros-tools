//! Typed execution of the six native M5 compatibility phases.
//!
//! The generic probe runner records one or two explicit commands per phase.
//! This module is the higher-level, tools-owned adapter that derives those
//! commands from verified preparation, two independent package roots, a
//! selected profile, and a private Python environment. It does not download,
//! publish, create a tag, or grant a release authority.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aros_common::{sha256_bytes, CancellationToken, Sha256Digest};
use serde::{Deserialize, Serialize};

use super::{
    checked_executable, run_probe_set, verify_standalone_outputs, CompatibilityCommand,
    CompatibilityEnvironment, CompatibilityPhase, CompatibilityPreparation,
    CompatibilityProbeRequest, CompatibilityProbeSet, CompatibilityProbeSetRequest,
    HostToolClosure, StandaloneOutputReport, StandaloneOutputRequest, StandaloneTargetArtifacts,
    TwoRootRelocation,
};
use crate::profiles::Profile;
use crate::python_environment::PythonEnvironment;
use crate::recipe::GitObjectId;
use crate::source_audit::{self, Budget as SourceAuditBudget};
use crate::{canonical, inspection, ContractError};

const POISONED_PATH: &str = "/nonexistent";
const MAX_MAKE_JOBS: usize = 64;
const UPSTREAM_SOURCE_AUDIT_TIMEOUT: Duration = Duration::from_mins(5);
const COMPATIBILITY_RECEIPT_SCHEMA: &str = "aros-toolchain-native-compatibility-receipt-v1";
const COMPATIBILITY_RECEIPT_FILE: &str = "native-compatibility.receipt.json";

/// Explicit C and C++ fixture files compiled through the installed drivers.
#[derive(Debug, Clone)]
pub struct StandaloneFixtures {
    /// Regular C fixture source.
    pub c: PathBuf,
    /// Regular C++ fixture source.
    pub cxx: PathBuf,
}

/// Complete inputs for one native six-phase compatibility execution.
#[derive(Debug, Clone)]
pub struct NativeCompatibilityRequest {
    /// Tools-owned engine and exact helpers prepared for this operation.
    pub preparation: CompatibilityPreparation,
    /// Two independently verified extracted package roots.
    ///
    /// The first root is consumed by CMake; the second is consumed by the
    /// pristine-upstream and standalone probes, proving relocation rather than
    /// merely copying one installation path through every consumer.
    pub relocation: TwoRootRelocation,
    /// Producer-selected target profile.
    pub profile: Profile,
    /// Absolute CMake executable selected by the caller's host preflight.
    pub cmake_program: PathBuf,
    /// Absolute Ninja executable supplied to CMake without ambient PATH lookup.
    pub ninja_program: PathBuf,
    /// Engine-free, tools-owned CMake consumer build directory to create.
    pub cmake_build_root: PathBuf,
    /// Separate pristine upstream source tree containing the `configure` script.
    pub upstream_source_root: PathBuf,
    /// Exact upstream commit selected by the recipe-bound profiles matrix.
    pub upstream_source_commit: GitObjectId,
    /// Empty upstream build directory to create.
    pub upstream_build_root: PathBuf,
    /// Exact closed Python runtime for upstream configure and Make.
    pub host_python: PythonEnvironment,
    /// Fresh measured host command closure for upstream configure and Make.
    pub host_tools: HostToolClosure,
    /// Explicit bounded parallelism for the two upstream Make invocations.
    pub make_jobs: usize,
    /// Fixture sources for standalone C and C++ collector probes.
    pub standalone_fixtures: StandaloneFixtures,
    /// Empty standalone output directory to create.
    pub standalone_output_root: PathBuf,
    /// Empty directory to create for durable per-phase reports and logs.
    pub reports_root: PathBuf,
    /// Positive phase deadline applied independently to every phase.
    pub timeout: Duration,
}

/// Measured reports from the complete native compatibility execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeCompatibilityReport {
    /// Durable reports for the exact six process phases.
    pub probes: CompatibilityProbeSet,
    /// Post-process verification of C/C++ collector output identities.
    pub standalone: StandaloneOutputReport,
    /// Durable aggregate receipt binding every phase report and standalone
    /// collector result without workstation-local paths.
    pub receipt: CompatibilityReceipt,
}

/// Durable aggregate evidence emitted by one complete compatibility execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityReceipt {
    /// The one no-clobber receipt written below the operation's report root.
    pub path: PathBuf,
    /// SHA-256 of the exact canonical receipt bytes.
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityReceiptDocument {
    schema: String,
    operation: String,
    upstream_source_commit: String,
    upstream_source_tree: String,
    phase_reports: Vec<CompatibilityReceiptPhase>,
    standalone_targets: BTreeMap<String, CompatibilityReceiptTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityReceiptPhase {
    phase: CompatibilityPhase,
    report_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityReceiptTarget {
    c: CompatibilityReceiptArtifact,
    cxx: CompatibilityReceiptArtifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityReceiptArtifact {
    sha256: Sha256Digest,
    size: u64,
    class: String,
}

/// Execute the closed native compatibility harness.
///
/// CMake always uses the materialized engine rather than a source-tree CMake
/// directory. Configure, includes and linklibs use the second relocation root
/// and receive only a sealed host-command closure plus the verified private
/// Python environment. Standalone consumers use absolute prefix compilers and
/// `PATH=/nonexistent`; their outputs are parsed for AROS ELF and collector
/// evidence after the six process reports succeed.
///
/// # Errors
///
/// Returns AX0703 when any input root, phase environment, selected helper,
/// host command closure, process, report, or standalone output is unsafe or
/// fails validation. Created output roots and logs remain retained for
/// diagnosis; the function never retries, removes them, or publishes material.
pub fn execute_native_compatibility(
    request: &NativeCompatibilityRequest,
    cancellation: &CancellationToken,
) -> Result<NativeCompatibilityReport, ContractError> {
    if request.timeout.is_zero() || request.make_jobs == 0 || request.make_jobs > MAX_MAKE_JOBS {
        return Err(ContractError::compatibility(
            "native compatibility execution requires a positive deadline and one to 64 Make jobs",
        ));
    }
    let inputs = validate_inputs(request)?;
    let outputs = create_output_roots(request, &inputs)?;
    let mut upstream_environment = request.host_python.compatibility_environment()?;
    // Autoconf 2.73 can otherwise append a C23 dialect marker before the
    // pinned upstream snapshot captures its compiler base name. That produces
    // impossible LLVM helper names. This is an explicit, recorded upstream
    // compatibility input rather than a runner-specific inherited default.
    upstream_environment.insert("ac_cv_prog_cc_c23".into(), String::new());
    validate_upstream_environment(&upstream_environment, &request.host_tools)?;
    let sealed_environment = CompatibilityEnvironment::SealedHostTools {
        variables: upstream_environment,
        host_tools: request.host_tools.clone(),
    };
    let poisoned_environment = CompatibilityEnvironment::Poisoned {
        variables: BTreeMap::from([("PATH".into(), POISONED_PATH.into())]),
    };
    let helpers_root = helpers_root(&request.preparation)?;
    let cmake_command = cmake_command(request, &inputs, &outputs, &helpers_root)?;
    let upstream_commands = upstream_commands(request, &inputs, &outputs)?;
    let standalone = standalone_commands(request, &inputs, &outputs)?;

    let probes = CompatibilityProbeSetRequest {
        probes: vec![
            CompatibilityProbeRequest {
                phase: CompatibilityPhase::CmakeConsumer,
                commands: vec![cmake_command],
                environment: poisoned_environment.clone(),
                current_dir: outputs.cmake_build.clone(),
                reports_root: outputs.reports.clone(),
                timeout: request.timeout,
                preparation: request.preparation.clone(),
            },
            CompatibilityProbeRequest {
                phase: CompatibilityPhase::UpstreamConfigure,
                commands: vec![upstream_commands.configure],
                environment: sealed_environment.clone(),
                current_dir: outputs.upstream_build.clone(),
                reports_root: outputs.reports.clone(),
                timeout: request.timeout,
                preparation: request.preparation.clone(),
            },
            CompatibilityProbeRequest {
                phase: CompatibilityPhase::UpstreamIncludes,
                commands: vec![upstream_commands.includes],
                environment: sealed_environment.clone(),
                current_dir: outputs.upstream_build.clone(),
                reports_root: outputs.reports.clone(),
                timeout: request.timeout,
                preparation: request.preparation.clone(),
            },
            CompatibilityProbeRequest {
                phase: CompatibilityPhase::UpstreamLinklibs,
                commands: vec![upstream_commands.linklibs],
                environment: sealed_environment,
                current_dir: outputs.upstream_build.clone(),
                reports_root: outputs.reports.clone(),
                timeout: request.timeout,
                preparation: request.preparation.clone(),
            },
            CompatibilityProbeRequest {
                phase: CompatibilityPhase::StandaloneC,
                commands: standalone.c,
                environment: poisoned_environment.clone(),
                current_dir: outputs.standalone.clone(),
                reports_root: outputs.reports.clone(),
                timeout: request.timeout,
                preparation: request.preparation.clone(),
            },
            CompatibilityProbeRequest {
                phase: CompatibilityPhase::StandaloneCxx,
                commands: standalone.cxx,
                environment: poisoned_environment,
                current_dir: outputs.standalone.clone(),
                reports_root: outputs.reports.clone(),
                timeout: request.timeout,
                preparation: request.preparation.clone(),
            },
        ],
    };
    let probes = run_probe_set(&probes, cancellation)?;
    let standalone = verify_standalone_outputs(&StandaloneOutputRequest {
        output_root: outputs.standalone,
        targets: standalone.outputs,
    })?;
    let revalidated_tree =
        verify_pristine_upstream_source(&inputs.upstream_source, &inputs.upstream_source_commit)?;
    if revalidated_tree != inputs.upstream_source_tree {
        return Err(ContractError::compatibility(
            "pristine upstream source tree changed during native compatibility execution",
        ));
    }
    let receipt = write_compatibility_receipt(
        &outputs.reports,
        &probes,
        &standalone,
        &inputs.upstream_source_commit,
        &inputs.upstream_source_tree,
    )?;
    Ok(NativeCompatibilityReport {
        probes,
        standalone,
        receipt,
    })
}

fn write_compatibility_receipt(
    reports_root: &Path,
    probes: &CompatibilityProbeSet,
    standalone: &StandaloneOutputReport,
    upstream_source_commit: &GitObjectId,
    upstream_source_tree: &GitObjectId,
) -> Result<CompatibilityReceipt, ContractError> {
    let mut persisted_reports = BTreeMap::new();
    let mut phase_reports = Vec::with_capacity(super::REQUIRED_PROBE_PHASES.len());
    for phase in super::REQUIRED_PROBE_PHASES {
        let expected = probes.reports.get(&phase).ok_or_else(|| {
            ContractError::compatibility("native compatibility execution lost a required phase")
        })?;
        let report_path = reports_root.join(format!("{}.report.json", phase.file_stem()));
        let bytes = super::read_regular_bounded(
            &report_path,
            "persisted native compatibility phase report",
            crate::canonical::MAX_DOCUMENT_BYTES,
        )?;
        let parsed = super::CompatibilityProbeReport::parse(&bytes)?;
        if &parsed != expected {
            return Err(ContractError::compatibility(
                "persisted native compatibility phase report differs from its execution identity",
            ));
        }
        phase_reports.push(CompatibilityReceiptPhase {
            phase,
            report_sha256: sha256_bytes(&bytes),
        });
        persisted_reports.insert(phase, bytes);
    }
    let standalone_targets = standalone
        .targets
        .iter()
        .map(|(triple, target)| {
            (
                triple.clone(),
                CompatibilityReceiptTarget {
                    c: receipt_artifact(&target.c),
                    cxx: receipt_artifact(&target.cxx),
                },
            )
        })
        .collect();
    let document = CompatibilityReceiptDocument {
        schema: COMPATIBILITY_RECEIPT_SCHEMA.into(),
        operation: "native-compatibility".into(),
        upstream_source_commit: upstream_source_commit.as_str().into(),
        upstream_source_tree: upstream_source_tree.as_str().into(),
        phase_reports,
        standalone_targets,
    };
    document.validate()?;
    let encoded = canonical::bytes(
        &serde_json::to_value(&document)
            .map_err(|_| ContractError::compatibility("cannot encode compatibility receipt"))?,
    )
    .map_err(|_| ContractError::compatibility("cannot canonically encode compatibility receipt"))?;
    let path = reports_root.join(COMPATIBILITY_RECEIPT_FILE);
    super::write_new_regular(&path, &encoded, "native compatibility receipt")?;
    let persisted = super::read_regular_bounded(
        &path,
        "persisted native compatibility receipt",
        crate::canonical::MAX_DOCUMENT_BYTES,
    )?;
    if persisted != encoded {
        return Err(ContractError::compatibility(
            "persisted native compatibility receipt bytes changed after publication",
        ));
    }
    let parsed: CompatibilityReceiptDocument = serde_json::from_slice(&persisted)
        .map_err(|_| ContractError::compatibility("native compatibility receipt is not JSON"))?;
    parsed.validate()?;
    if parsed != document {
        return Err(ContractError::compatibility(
            "persisted native compatibility receipt differs from its execution identity",
        ));
    }
    for (phase, expected) in persisted_reports {
        let report_path = reports_root.join(format!("{}.report.json", phase.file_stem()));
        let observed = super::read_regular_bounded(
            &report_path,
            "revalidated native compatibility phase report",
            crate::canonical::MAX_DOCUMENT_BYTES,
        )?;
        if observed != expected {
            return Err(ContractError::compatibility(
                "native compatibility phase report changed while receipt was published",
            ));
        }
    }
    Ok(CompatibilityReceipt {
        path,
        sha256: sha256_bytes(&persisted),
    })
}

impl CompatibilityReceiptDocument {
    fn validate(&self) -> Result<(), ContractError> {
        if self.schema != COMPATIBILITY_RECEIPT_SCHEMA || self.operation != "native-compatibility" {
            return Err(ContractError::compatibility(
                "native compatibility receipt has an unsupported schema or operation",
            ));
        }
        for identity in [&self.upstream_source_commit, &self.upstream_source_tree] {
            if GitObjectId::try_from(identity.clone()).is_err() {
                return Err(ContractError::compatibility(
                    "native compatibility receipt contains an invalid upstream source identity",
                ));
            }
        }
        if self.phase_reports.len() != super::REQUIRED_PROBE_PHASES.len()
            || self
                .phase_reports
                .iter()
                .map(|entry| entry.phase)
                .ne(super::REQUIRED_PROBE_PHASES)
        {
            return Err(ContractError::compatibility(
                "native compatibility receipt does not contain the required ordered phase set",
            ));
        }
        if self.standalone_targets.is_empty() || self.standalone_targets.len() > 2 {
            return Err(ContractError::compatibility(
                "native compatibility receipt has an invalid standalone target set",
            ));
        }
        for (triple, target) in &self.standalone_targets {
            if !crate::profiles::identifier(triple)
                || !valid_receipt_artifact(&target.c)
                || !valid_receipt_artifact(&target.cxx)
            {
                return Err(ContractError::compatibility(
                    "native compatibility receipt contains invalid standalone evidence",
                ));
            }
        }
        Ok(())
    }
}

fn receipt_artifact(artifact: &super::StandaloneArtifactIdentity) -> CompatibilityReceiptArtifact {
    CompatibilityReceiptArtifact {
        sha256: artifact.sha256.clone(),
        size: artifact.size,
        class: match artifact.class {
            aros_common::elf::Class::Elf32 => "elf32",
            aros_common::elf::Class::Elf64 => "elf64",
        }
        .into(),
    }
}

fn valid_receipt_artifact(artifact: &CompatibilityReceiptArtifact) -> bool {
    artifact.size > 0 && matches!(artifact.class.as_str(), "elf32" | "elf64")
}

#[derive(Debug)]
struct Inputs {
    cmake_toolchain_root: PathBuf,
    upstream_toolchain_root: PathBuf,
    upstream_source: PathBuf,
    upstream_source_commit: GitObjectId,
    upstream_source_tree: GitObjectId,
    configure: PathBuf,
    cmake: PathBuf,
    ninja: PathBuf,
    standalone_c: PathBuf,
    standalone_cxx: PathBuf,
}

#[derive(Debug)]
struct OutputRoots {
    cmake_build: PathBuf,
    upstream_build: PathBuf,
    standalone: PathBuf,
    reports: PathBuf,
}

#[derive(Debug)]
struct UpstreamCommands {
    configure: CompatibilityCommand,
    includes: CompatibilityCommand,
    linklibs: CompatibilityCommand,
}

#[derive(Debug)]
struct StandaloneCommands {
    c: Vec<CompatibilityCommand>,
    cxx: Vec<CompatibilityCommand>,
    outputs: BTreeMap<String, StandaloneTargetArtifacts>,
}

fn validate_inputs(request: &NativeCompatibilityRequest) -> Result<Inputs, ContractError> {
    if request.relocation.first.verified != request.relocation.second.verified {
        return Err(ContractError::compatibility(
            "native compatibility execution requires two independently identical package roots",
        ));
    }
    let cmake_toolchain_root = checked_directory(
        &request.relocation.first.root,
        "first compatibility package root",
    )?;
    let upstream_toolchain_root = checked_directory(
        &request.relocation.second.root,
        "second compatibility package root",
    )?;
    if cmake_toolchain_root == upstream_toolchain_root {
        return Err(ContractError::compatibility(
            "native compatibility execution cannot reuse one relocation root",
        ));
    }
    let upstream_source = checked_directory(
        &request.upstream_source_root,
        "pristine upstream compatibility source root",
    )?;
    let upstream_source_tree =
        verify_pristine_upstream_source(&upstream_source, &request.upstream_source_commit)?;
    if upstream_source == request.preparation.source_root
        || upstream_source == request.preparation.engine_root
        || upstream_source == cmake_toolchain_root
        || upstream_source == upstream_toolchain_root
    {
        return Err(ContractError::compatibility(
            "pristine upstream source must be distinct from tools-owned and package roots",
        ));
    }
    let configure = upstream_source.join("configure");
    let metadata = fs::symlink_metadata(&configure).map_err(|_| {
        ContractError::compatibility("pristine upstream source does not contain configure")
    })?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(ContractError::compatibility(
            "pristine upstream configure is not a regular executable file",
        ));
    }
    let standalone_c =
        checked_regular_file(&request.standalone_fixtures.c, "standalone C fixture")?;
    let standalone_cxx =
        checked_regular_file(&request.standalone_fixtures.cxx, "standalone C++ fixture")?;
    if standalone_c == standalone_cxx {
        return Err(ContractError::compatibility(
            "standalone C and C++ compatibility fixtures must be distinct",
        ));
    }
    let cmake = checked_executable(&request.cmake_program)?;
    let ninja = checked_executable(&request.ninja_program)?;
    Ok(Inputs {
        cmake_toolchain_root,
        upstream_toolchain_root,
        upstream_source,
        upstream_source_commit: request.upstream_source_commit.clone(),
        upstream_source_tree,
        configure,
        cmake,
        ninja,
        standalone_c,
        standalone_cxx,
    })
}

fn verify_pristine_upstream_source(
    root: &Path,
    expected_commit: &GitObjectId,
) -> Result<GitObjectId, ContractError> {
    let deadline = Instant::now()
        .checked_add(UPSTREAM_SOURCE_AUDIT_TIMEOUT)
        .ok_or_else(|| {
            ContractError::preflight("upstream source audit deadline is not representable")
        })?;
    let cancellation = CancellationToken::default();
    let (commit, tree) = inspection::observed_identity(root, deadline, &cancellation)?;
    if &commit != expected_commit {
        return Err(ContractError::identity(
            "pristine upstream source commit differs from the recipe-bound profiles matrix",
        ));
    }
    let checkout =
        inspection::Checkout::inspect_controlled(root, (&commit, &tree), deadline, &cancellation)?;
    let mut budget = SourceAuditBudget::controlled(deadline, &cancellation);
    source_audit::verify(&checkout, &mut budget, 0)?;
    Ok(tree)
}

fn create_output_roots(
    request: &NativeCompatibilityRequest,
    inputs: &Inputs,
) -> Result<OutputRoots, ContractError> {
    let roots = [
        checked_absent_root(&request.cmake_build_root, "CMake compatibility build root")?,
        checked_absent_root(
            &request.upstream_build_root,
            "upstream compatibility build root",
        )?,
        checked_absent_root(
            &request.standalone_output_root,
            "standalone compatibility output root",
        )?,
        checked_absent_root(&request.reports_root, "compatibility report root")?,
    ];
    if roots.iter().collect::<BTreeSet<_>>().len() != roots.len() {
        return Err(ContractError::compatibility(
            "native compatibility execution output roots must be distinct",
        ));
    }
    let mut protected = vec![
        &request.preparation.source_root,
        &request.preparation.engine_root,
        &inputs.cmake_toolchain_root,
        &inputs.upstream_toolchain_root,
        &inputs.upstream_source,
        &inputs.standalone_c,
        &inputs.standalone_cxx,
        &request.host_tools.root,
    ];
    protected.extend(request.host_python.import_roots());
    if roots.iter().any(|root| {
        protected
            .iter()
            .any(|input| root.starts_with(input) || input.starts_with(root))
    }) {
        return Err(ContractError::compatibility(
            "native compatibility output root overlaps a source, engine, package, or fixture input",
        ));
    }
    for root in &roots {
        fs::create_dir(root).map_err(|_| {
            ContractError::compatibility(
                "cannot create a fresh native compatibility output directory",
            )
        })?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|_| {
            ContractError::compatibility(
                "cannot restrict a native compatibility output directory to its owner",
            )
        })?;
    }
    Ok(OutputRoots {
        cmake_build: roots[0].clone(),
        upstream_build: roots[1].clone(),
        standalone: roots[2].clone(),
        reports: roots[3].clone(),
    })
}

fn validate_upstream_environment(
    environment: &BTreeMap<String, String>,
    host_tools: &HostToolClosure,
) -> Result<(), ContractError> {
    let expected = BTreeSet::from([
        "ac_cv_prog_cc_c23",
        "PATH",
        "PYTHON",
        "PYTHONDONTWRITEBYTECODE",
        "PYTHONHASHSEED",
        "PYTHONNOUSERSITE",
        "PYTHONPATH",
    ]);
    if environment
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != expected
        || environment.get("PATH").map(String::as_str) != Some(POISONED_PATH)
        || environment
            .get("PYTHONDONTWRITEBYTECODE")
            .map(String::as_str)
            != Some("1")
        || environment.get("PYTHONHASHSEED").map(String::as_str) != Some("0")
        || environment.get("PYTHONNOUSERSITE").map(String::as_str) != Some("1")
        || environment.get("ac_cv_prog_cc_c23").map(String::as_str) != Some("")
        || environment.get("PYTHONPATH").is_none_or(String::is_empty)
    {
        return Err(ContractError::compatibility(
            "upstream compatibility phase does not have the exact closed Python environment",
        ));
    }
    host_tools.revalidate()?;
    let python = PathBuf::from(environment.get("PYTHON").ok_or_else(|| {
        ContractError::compatibility(
            "upstream compatibility Python environment lost its interpreter",
        )
    })?);
    let Some(host_python) = host_tools.tools.get("python3") else {
        return Err(ContractError::compatibility(
            "upstream compatibility host-tool closure does not expose the checked python3 interpreter",
        ));
    };
    if python != host_python.program || !host_tools.tools.contains_key("make") {
        return Err(ContractError::compatibility(
            "upstream compatibility host-tool closure does not bind Python and Make exactly",
        ));
    }
    Ok(())
}

fn helpers_root(preparation: &CompatibilityPreparation) -> Result<PathBuf, ContractError> {
    let root = preparation
        .helpers
        .get("aros-transpiler")
        .and_then(|helper| helper.path.parent())
        .ok_or_else(|| {
            ContractError::compatibility("compatibility preparation has no valid helper root")
        })?
        .to_owned();
    if preparation
        .helpers
        .values()
        .any(|helper| helper.path.parent() != Some(root.as_path()))
    {
        return Err(ContractError::compatibility(
            "compatibility preparation helpers do not share one exact root",
        ));
    }
    checked_directory(&root, "compatibility helper root")
}

fn cmake_command(
    request: &NativeCompatibilityRequest,
    inputs: &Inputs,
    outputs: &OutputRoots,
    helpers_root: &Path,
) -> Result<CompatibilityCommand, ContractError> {
    let engine = utf8_path(
        &request.preparation.engine_root,
        "materialized CMake engine",
    )?;
    let source = utf8_path(&request.preparation.source_root, "engine-free CMake source")?;
    let build = utf8_path(&outputs.cmake_build, "CMake compatibility build root")?;
    let toolchain = utf8_path(
        &inputs.cmake_toolchain_root,
        "first compatibility package root",
    )?;
    let helpers = utf8_path(helpers_root, "compatibility helper root")?;
    let ninja = utf8_path(&inputs.ninja, "Ninja executable")?;
    let toolchain_file = utf8_path(
        &request
            .preparation
            .engine_root
            .join("toolchains/AROS.cmake"),
        "materialized CMake toolchain file",
    )?;
    Ok(CompatibilityCommand {
        program: inputs.cmake.clone(),
        arguments: vec![
            "-S".into(),
            engine,
            "-B".into(),
            build,
            "-G".into(),
            "Ninja".into(),
            format!("-DCMAKE_MAKE_PROGRAM={ninja}"),
            format!("-DCMAKE_TOOLCHAIN_FILE={toolchain_file}"),
            format!("-DAROS_SOURCE_DIR={source}"),
            format!("-DAROS_CROSS_TOOLCHAIN_ROOT={toolchain}"),
            format!("-DAROS_TARGET_CPU={}", request.profile.cpu()),
            format!("-DAROS_TARGET_PLATFORM={}", request.profile.platform()),
            format!("-DGCC_CONFIG_FLOAT_ABI={}", request.profile.float_abi()),
            format!("-DAROS_RUST_TOOLS_DIR={helpers}"),
            "-DAROS_ENABLE_MMU=ON".into(),
            "-DCMAKE_BUILD_TYPE=Release".into(),
        ],
    })
}

fn upstream_commands(
    request: &NativeCompatibilityRequest,
    inputs: &Inputs,
    outputs: &OutputRoots,
) -> Result<UpstreamCommands, ContractError> {
    let toolchain = utf8_path(
        &inputs.upstream_toolchain_root,
        "second compatibility package root",
    )?;
    let build = utf8_path(&outputs.upstream_build, "upstream compatibility build root")?;
    let manifest = &request.relocation.second.verified.manifest;
    let llvm_version = manifest.llvm_version.as_deref().ok_or_else(|| {
        ContractError::compatibility(
            "second compatibility package manifest does not declare an LLVM version",
        )
    })?;
    let make = request
        .host_tools
        .tools
        .get("make")
        .ok_or_else(|| {
            ContractError::compatibility(
                "upstream compatibility host-tool closure has no make role",
            )
        })?
        .program
        .clone();
    let make_arguments = |target: &str| {
        vec![
            "-C".into(),
            build.clone(),
            format!("-j{}", request.make_jobs),
            target.into(),
        ]
    };
    Ok(UpstreamCommands {
        configure: CompatibilityCommand {
            program: inputs.configure.clone(),
            arguments: vec![
                format!("--target={}", request.profile.configure_target()),
                "--with-toolchain=llvm".into(),
                format!("--with-llvm-version={llvm_version}"),
                "--with-aros-toolchain=yes".into(),
                format!("--with-aros-toolchain-install={toolchain}"),
            ],
        },
        includes: CompatibilityCommand {
            program: make.clone(),
            arguments: make_arguments("includes"),
        },
        linklibs: CompatibilityCommand {
            program: make,
            arguments: make_arguments("linklibs"),
        },
    })
}

fn standalone_commands(
    request: &NativeCompatibilityRequest,
    inputs: &Inputs,
    outputs: &OutputRoots,
) -> Result<StandaloneCommands, ContractError> {
    let bin = inputs.upstream_toolchain_root.join("bin");
    let clang = bin.join("clang");
    let clangxx = bin.join("clang++");
    let developer = outputs
        .upstream_build
        .join("bin")
        .join(request.profile.upstream_output_target())
        .join("AROS/Developer");
    let developer = utf8_path(&developer, "upstream Developer sysroot")?;
    let c_fixture = utf8_path(&inputs.standalone_c, "standalone C fixture")?;
    let cxx_fixture = utf8_path(&inputs.standalone_cxx, "standalone C++ fixture")?;
    let mut c_commands = Vec::new();
    let mut cxx_commands = Vec::new();
    let mut targets = BTreeMap::new();
    for triple in standalone_triples(&request.profile)? {
        let cpu = triple
            .split_once('-')
            .map_or(triple.as_str(), |(cpu, _)| cpu);
        let c_output = outputs.standalone.join(format!("c-{cpu}.o"));
        let cxx_output = outputs.standalone.join(format!("cxx-{cpu}.o"));
        let c_output_arg = utf8_path(&c_output, "standalone C output")?;
        let cxx_output_arg = utf8_path(&cxx_output, "standalone C++ output")?;
        let mut c_arguments = vec![
            format!("--target={triple}"),
            format!("--sysroot={developer}"),
        ];
        let mut cxx_arguments = c_arguments.clone();
        if request.profile.float_abi() == "hard" && triple.starts_with("arm-") {
            c_arguments.push("-mfloat-abi=hard".into());
            cxx_arguments.push("-mfloat-abi=hard".into());
        }
        c_arguments.extend([
            "-ffreestanding".into(),
            "-fno-ident".into(),
            "-fno-unwind-tables".into(),
            "-fno-asynchronous-unwind-tables".into(),
            "-g0".into(),
            "-nostdlib".into(),
            "-nostartfiles".into(),
            c_fixture.clone(),
            "-o".into(),
            c_output_arg,
        ]);
        cxx_arguments.extend([
            "-ffreestanding".into(),
            "-fno-exceptions".into(),
            "-fno-ident".into(),
            "-fno-unwind-tables".into(),
            "-fno-asynchronous-unwind-tables".into(),
            "-g0".into(),
            "-nostdlib".into(),
            "-nostartfiles".into(),
            "-nostdinc++".into(),
            cxx_fixture.clone(),
            "-o".into(),
            cxx_output_arg,
        ]);
        c_commands.push(CompatibilityCommand {
            program: clang.clone(),
            arguments: c_arguments,
        });
        cxx_commands.push(CompatibilityCommand {
            program: clangxx.clone(),
            arguments: cxx_arguments,
        });
        targets.insert(
            triple,
            StandaloneTargetArtifacts {
                c: c_output,
                cxx: cxx_output,
            },
        );
    }
    Ok(StandaloneCommands {
        c: c_commands,
        cxx: cxx_commands,
        outputs: targets,
    })
}

fn standalone_triples(profile: &Profile) -> Result<Vec<String>, ContractError> {
    let mut triples = vec![profile.target_triple().to_owned()];
    if profile.name() == "pc-x86_64" {
        triples.push("i386-unknown-aros".into());
    }
    if triples.len() > 2
        || triples
            .iter()
            .any(|triple| !crate::profiles::identifier(triple))
    {
        return Err(ContractError::compatibility(
            "selected profile has an unsupported standalone compatibility target set",
        ));
    }
    Ok(triples)
}

fn checked_absent_root(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(ContractError::compatibility(format!(
            "{label} must be an absent absolute leaf directory"
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ContractError::compatibility(format!("{label} has no parent directory")))?;
    let parent = checked_directory(parent, label)?;
    let name = path.file_name().ok_or_else(|| {
        ContractError::compatibility(format!("{label} has no safe final path segment"))
    })?;
    let path = parent.join(name);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Ok(_) => Err(ContractError::compatibility(format!(
            "{label} already exists and cannot be adopted"
        ))),
        Err(_) => Err(ContractError::compatibility(format!(
            "cannot inspect {label} before creating it"
        ))),
    }
}

fn checked_regular_file(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::compatibility(format!(
            "{label} must be an absolute regular file"
        )));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::compatibility(format!("{label} cannot be inspected")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
        return Err(ContractError::compatibility(format!(
            "{label} is not a nonempty regular file"
        )));
    }
    path.canonicalize()
        .map_err(|_| ContractError::compatibility(format!("{label} cannot be canonicalized")))
}

fn checked_directory(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ContractError::compatibility(format!("{label} cannot be inspected")))?;
    if !path.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::compatibility(format!(
            "{label} must be an absolute real directory"
        )));
    }
    path.canonicalize()
        .map_err(|_| ContractError::compatibility(format!("{label} cannot be canonicalized")))
}

fn utf8_path(path: &Path, label: &str) -> Result<String, ContractError> {
    let path = path.to_str().ok_or_else(|| {
        ContractError::compatibility(format!("{label} is not UTF-8 representable"))
    })?;
    if path.chars().any(char::is_control) {
        return Err(ContractError::compatibility(format!(
            "{label} contains control characters"
        )));
    }
    Ok(path.into())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    use aros_common::{
        run_output, run_status, sha256_file, ArosToolchainManifest, CancellationToken,
    };
    use flate2::{write::GzEncoder, Compression};
    use serde_json::json;
    use tar::{Builder, Header};

    use super::{execute_native_compatibility, NativeCompatibilityRequest, StandaloneFixtures};
    use crate::compatibility::{
        prepare, prepare_host_tool_closure, CompatibilityHostTool, CompatibilityPreparationRequest,
        HostToolClosureRequest, TwoRootRelocation,
    };
    use crate::package_extract::ExtractedPackage;
    use crate::package_verify::VerifiedPackage;
    use crate::profiles::Profiles;
    use crate::python_environment::PythonEnvironment;
    use crate::source_lock::SourceLock;

    #[test]
    fn executes_every_phase_with_two_roots_and_closed_environments() {
        let temporary = tempfile::tempdir().unwrap();
        let (request, cmake_log, make_log) = request(temporary.path());
        let report = execute_native_compatibility(&request, &CancellationToken::default()).unwrap();

        assert_eq!(report.probes.reports.len(), 6);
        assert!(report
            .probes
            .reports
            .values()
            .all(|probe| !probe.commands.is_empty()));
        assert!(
            report.probes.reports[&crate::compatibility::CompatibilityPhase::CmakeConsumer]
                .host_tools
                .is_empty()
        );
        assert_eq!(
            report.probes.reports[&crate::compatibility::CompatibilityPhase::UpstreamConfigure]
                .host_tools
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["make", "python3"]
        );
        assert_eq!(report.standalone.targets.len(), 2);
        assert!(report
            .standalone
            .targets
            .contains_key("x86_64-unknown-aros"));
        assert!(report.standalone.targets.contains_key("i386-unknown-aros"));
        assert!(report.receipt.path.is_file());
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(&report.receipt.path).unwrap()).unwrap();
        assert_eq!(
            receipt["schema"],
            "aros-toolchain-native-compatibility-receipt-v1"
        );
        assert_eq!(receipt["phase_reports"].as_array().unwrap().len(), 6);
        assert_eq!(receipt["standalone_targets"].as_object().unwrap().len(), 2);
        assert_eq!(
            aros_common::sha256_file(&report.receipt.path)
                .unwrap()
                .digest,
            report.receipt.sha256
        );

        let cmake_arguments = fs::read_to_string(cmake_log).unwrap();
        assert!(cmake_arguments.contains("-S"));
        assert!(cmake_arguments.contains("aros-cmake-engine"));
        assert!(cmake_arguments.contains("AROS_SOURCE_DIR="));
        let make_arguments = fs::read_to_string(make_log).unwrap();
        assert!(make_arguments.contains("includes"));
        assert!(make_arguments.contains("linklibs"));
    }

    #[test]
    fn rejects_a_reused_native_output_root_before_any_child_starts() {
        let temporary = tempfile::tempdir().unwrap();
        let (request, _, _) = request(temporary.path());
        fs::create_dir(&request.reports_root).unwrap();
        let error =
            execute_native_compatibility(&request, &CancellationToken::default()).unwrap_err();
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            aros_common::DiagnosticCode::ProducerCompatibility
        );
    }

    #[test]
    fn rejects_a_dirty_or_mismatched_pristine_upstream_source_before_any_child_starts() {
        let temporary = tempfile::tempdir().unwrap();
        let (dirty, _, _) = request(temporary.path());
        fs::write(dirty.upstream_source_root.join("configure"), "exit 0\n").unwrap();
        assert!(execute_native_compatibility(&dirty, &CancellationToken::default()).is_err());
        assert!(!dirty.reports_root.exists());

        let temporary = tempfile::tempdir().unwrap();
        let (mut mismatched, _, _) = request(temporary.path());
        mismatched.upstream_source_commit =
            crate::recipe::GitObjectId::try_from("f".repeat(40)).unwrap();
        assert!(execute_native_compatibility(&mismatched, &CancellationToken::default()).is_err());
        assert!(!mismatched.reports_root.exists());
    }

    fn request(root: &Path) -> (NativeCompatibilityRequest, PathBuf, PathBuf) {
        let source = root.join("engine-free-source");
        let engine_work = root.join("engine-work");
        let helpers = root.join("helpers");
        for directory in [&source, &engine_work, &helpers] {
            fs::create_dir(directory).unwrap();
        }
        for helper in crate::compatibility::REQUIRED_HELPERS {
            script(&helpers.join(helper), "exit 0");
        }
        let preparation = prepare(&CompatibilityPreparationRequest {
            source_root: source,
            work_root: engine_work,
            helpers_root: helpers,
        })
        .unwrap();

        let first = root.join("first-toolchain");
        let second = root.join("second-toolchain");
        for toolchain in [&first, &second] {
            fs::create_dir(toolchain).unwrap();
            fs::create_dir(toolchain.join("bin")).unwrap();
        }
        let c_elf = root.join("c.elf");
        let cxx_elf = root.join("cxx.elf");
        let c_elf32 = root.join("c-i386.elf");
        let cxx_elf32 = root.join("cxx-i386.elf");
        fs::write(&c_elf, fixture_elf64("__TOOLCHAIN_LIST__$")).unwrap();
        fs::write(&cxx_elf, fixture_elf64("__INIT_ARRAY_LIST__$")).unwrap();
        fs::write(&c_elf32, fixture_elf32("__TOOLCHAIN_LIST__$")).unwrap();
        fs::write(&cxx_elf32, fixture_elf32("__INIT_ARRAY_LIST__$")).unwrap();
        for toolchain in [&first, &second] {
            script(
                &toolchain.join("bin/clang"),
                &format!(
                    "out=\ncase \" $* \" in *' --target=i386-unknown-aros '*) fixture='{}';; *) fixture='{}';; esac\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = -o ]; then shift; out=$1; fi\n  shift\ndone\n/bin/cp \"$fixture\" \"$out\"",
                    c_elf32.display(),
                    c_elf.display(),
                ),
            );
            script(
                &toolchain.join("bin/clang++"),
                &format!(
                    "out=\ncase \" $* \" in *' --target=i386-unknown-aros '*) fixture='{}';; *) fixture='{}';; esac\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = -o ]; then shift; out=$1; fi\n  shift\ndone\n/bin/cp \"$fixture\" \"$out\"",
                    cxx_elf32.display(),
                    cxx_elf.display(),
                ),
            );
        }
        let verified = VerifiedPackage {
            manifest: manifest(),
            archive_sha256: aros_common::Sha256Digest::parse(&"a".repeat(64)).unwrap(),
            archive_size: 1,
        };
        let relocation = TwoRootRelocation {
            first: ExtractedPackage {
                root: first,
                verified: verified.clone(),
            },
            second: ExtractedPackage {
                root: second,
                verified,
            },
        };

        let upstream = root.join("upstream-source");
        fs::create_dir(&upstream).unwrap();
        script(
            &upstream.join("configure"),
            "[ \"$PATH\" != /nonexistent ] || exit 20\n[ \"${ac_cv_prog_cc_c23+x}\" = x ] && [ -z \"$ac_cv_prog_cc_c23\" ] || exit 21\npython3 -S -P -c 'import mako, markupsafe'",
        );
        git(&upstream, &["init", "-q"]);
        git(&upstream, &["config", "user.email", "test@example.invalid"]);
        git(&upstream, &["config", "user.name", "AROS Tools Test"]);
        git(&upstream, &["add", "configure"]);
        git(
            &upstream,
            &["commit", "-qm", "test: pristine upstream source"],
        );
        let upstream_commit = git_output(&upstream, &["rev-parse", "HEAD"]);
        let cmake_log = root.join("cmake-arguments.log");
        let cmake = root.join("cmake");
        script(
            &cmake,
            &format!("printf '%s\\n' \"$@\" > '{}'", cmake_log.display()),
        );
        let ninja = root.join("ninja");
        script(&ninja, "exit 0");
        let make_log = root.join("make-arguments.log");
        let make = root.join("make");
        script(
            &make,
            &format!("printf '%s\\n' \"$@\" >> '{}'", make_log.display()),
        );
        let python = python_environment(root);
        let closure = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: root.join("host-tools"),
            tools: vec![
                CompatibilityHostTool {
                    name: "make".into(),
                    program: make,
                },
                CompatibilityHostTool {
                    name: "python3".into(),
                    program: python.interpreter().path.clone(),
                },
            ],
        })
        .unwrap();
        let c_fixture = root.join("smoke.c");
        let cxx_fixture = root.join("smoke.cpp");
        fs::write(&c_fixture, b"int main(void) { return 0; }\n").unwrap();
        fs::write(&cxx_fixture, b"int main() { return 0; }\n").unwrap();
        let profiles = Profiles::parse(
            serde_json::to_vec(&serde_json::json!({
                "schema": "aros-toolchain-profiles-v1",
                "upstream_commit": upstream_commit,
                "profiles": [{
                    "name": "pc-x86_64", "configure_target": "pc-x86_64",
                    "upstream_output_target": "pc-x86_64",
                    "target_triple": "x86_64-unknown-aros", "cpu": "x86_64",
                    "platform": "pc", "float_abi": "",
                    "capabilities": ["c", "cxx", "standalone-collector"]
                }]
            }))
            .unwrap()
            .as_slice(),
        )
        .unwrap();
        (
            NativeCompatibilityRequest {
                preparation,
                relocation,
                profile: profiles.select("pc-x86_64").unwrap().clone(),
                cmake_program: cmake,
                ninja_program: ninja,
                cmake_build_root: root.join("cmake-build"),
                upstream_source_root: upstream,
                upstream_source_commit: profiles.upstream_commit().clone(),
                upstream_build_root: root.join("upstream-build"),
                host_python: python,
                host_tools: closure,
                make_jobs: 2,
                standalone_fixtures: StandaloneFixtures {
                    c: c_fixture,
                    cxx: cxx_fixture,
                },
                standalone_output_root: root.join("standalone"),
                reports_root: root.join("reports"),
                timeout: Duration::from_secs(5),
            },
            cmake_log,
            make_log,
        )
    }

    fn git(root: &Path, arguments: &[&str]) {
        let mut command = Command::new("git");
        command.args(arguments).current_dir(root);
        let status = run_status(&mut command).unwrap();
        assert!(status.status.success());
    }

    fn git_output(root: &Path, arguments: &[&str]) -> String {
        let mut command = Command::new("git");
        command.args(arguments).current_dir(root);
        let output = run_output(&mut command).unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout.exact_bytes().unwrap().to_vec())
            .unwrap()
            .trim()
            .to_owned()
    }

    fn manifest() -> ArosToolchainManifest {
        ArosToolchainManifest {
            schema: 1,
            release_id: "toolchain-v1-test".into(),
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            tree_sha256: "b".repeat(64),
            llvm_version: Some("11.0.0".into()),
            recipe_sha256: "c".repeat(64),
            source_lock_sha256: "d".repeat(64),
            profiles_sha256: "e".repeat(64),
            source_commit: "1".repeat(40),
            producer_commit: "2".repeat(40),
            tools_commit: "3".repeat(40),
            source_date_epoch: 1,
            capabilities: vec!["c".into(), "cxx".into()],
            build_environment: serde_json::Map::default(),
            files: Vec::new(),
        }
    }

    fn python_environment(root: &Path) -> PythonEnvironment {
        let cache = root.join("python-cache");
        fs::create_dir(&cache).unwrap();
        archive(
            &cache.join("mako-1.3.10.tar.gz"),
            &[
                ("mako-1.3.10/mako/__init__.py", b"__version__ = '1.3.10'\n"),
                (
                    "mako-1.3.10/mako/template.py",
                    b"import markupsafe\nclass Template:\n def __init__(self, value): self.value = value\n def render(self): assert markupsafe.__version__ == '3.0.2'; return self.value\n",
                ),
            ],
        );
        archive(
            &cache.join("markupsafe-3.0.2.tar.gz"),
            &[(
                "markupsafe-3.0.2/src/markupsafe/__init__.py",
                b"__version__ = '3.0.2'\n",
            )],
        );
        let package =
            |name: &str, version: &str, filename: &str, source_root: &str, python_path: &str| {
                let measured = sha256_file(&cache.join(filename)).unwrap();
                json!({
                    "name": name, "version": version, "filename": filename,
                    "url": format!("https://example.invalid/{filename}"),
                    "sha256": measured.digest, "size": measured.size,
                    "source_root": source_root, "python_path": python_path
                })
            };
        let lock = SourceLock::parse(&serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{
                "component": "llvm", "version": "11.0.0", "purpose": "toolchain-component",
                "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz",
                "sha256": "a".repeat(64), "size": 1
            }],
            "host_python_packages": [
                package("mako", "1.3.10", "mako-1.3.10.tar.gz", "mako-1.3.10", "."),
                package("markupsafe", "3.0.2", "markupsafe-3.0.2.tar.gz", "markupsafe-3.0.2", "src")
            ]
        })).unwrap()).unwrap();
        PythonEnvironment::prepare(&lock, &cache, &root.join("python-environment")).unwrap()
    }

    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = Builder::new(encoder);
        for (name, contents) in entries {
            let mut header = Header::new_gnu();
            header.set_size(u64::try_from(contents.len()).unwrap());
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *contents).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    fn script(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn fixture_elf64(symbol: &str) -> Vec<u8> {
        let mut names = Vec::from([0_u8]);
        names.extend_from_slice(symbol.as_bytes());
        names.push(0);
        let section_offset = 64_usize;
        let section_size = 64_usize;
        let strtab_offset = section_offset + 3 * section_size;
        let symtab_offset = strtab_offset + names.len();
        let mut object = vec![0_u8; symtab_offset + 2 * 24];
        object[..4].copy_from_slice(b"\x7fELF");
        object[4] = 2;
        object[5] = 1;
        object[6] = 1;
        object[7] = aros_common::elf::OS_ABI_AROS;
        object[8] = aros_common::elf::AROS_ABI_VERSION;
        write_u64(&mut object, 0x28, section_offset as u64);
        write_u16(&mut object, 0x34, 64);
        write_u16(&mut object, 0x3a, section_size as u16);
        write_u16(&mut object, 0x3c, 3);
        let strtab = section_offset + section_size;
        write_u32(&mut object, strtab + 4, 3);
        write_u64(&mut object, strtab + 24, strtab_offset as u64);
        write_u64(&mut object, strtab + 32, names.len() as u64);
        write_u64(&mut object, strtab + 48, 1);
        let symtab = strtab + section_size;
        write_u32(&mut object, symtab + 4, 2);
        write_u64(&mut object, symtab + 24, symtab_offset as u64);
        write_u64(&mut object, symtab + 32, 48);
        write_u32(&mut object, symtab + 40, 1);
        write_u64(&mut object, symtab + 48, 8);
        write_u64(&mut object, symtab + 56, 24);
        object[strtab_offset..strtab_offset + names.len()].copy_from_slice(&names);
        let symbol_entry = symtab_offset + 24;
        write_u32(&mut object, symbol_entry, 1);
        object[symbol_entry + 4] = 0x10;
        write_u16(&mut object, symbol_entry + 6, 1);
        object
    }

    fn fixture_elf32(symbol: &str) -> Vec<u8> {
        let mut names = Vec::from([0_u8]);
        names.extend_from_slice(symbol.as_bytes());
        names.push(0);
        let section_offset = 52_usize;
        let section_size = 40_usize;
        let strtab_offset = section_offset + 3 * section_size;
        let symtab_offset = strtab_offset + names.len();
        let mut object = vec![0_u8; symtab_offset + 2 * 16];
        object[..4].copy_from_slice(b"\x7fELF");
        object[4] = 1;
        object[5] = 1;
        object[6] = 1;
        object[7] = aros_common::elf::OS_ABI_AROS;
        object[8] = aros_common::elf::AROS_ABI_VERSION;
        write_u32(&mut object, 0x20, section_offset as u32);
        write_u16(&mut object, 0x28, 52);
        write_u16(&mut object, 0x2e, section_size as u16);
        write_u16(&mut object, 0x30, 3);
        let strtab = section_offset + section_size;
        write_u32(&mut object, strtab + 4, 3);
        write_u32(&mut object, strtab + 16, strtab_offset as u32);
        write_u32(&mut object, strtab + 20, names.len() as u32);
        write_u32(&mut object, strtab + 32, 1);
        let symtab = strtab + section_size;
        write_u32(&mut object, symtab + 4, 2);
        write_u32(&mut object, symtab + 16, symtab_offset as u32);
        write_u32(&mut object, symtab + 20, 32);
        write_u32(&mut object, symtab + 24, 1);
        write_u32(&mut object, symtab + 36, 16);
        object[strtab_offset..strtab_offset + names.len()].copy_from_slice(&names);
        let symbol_entry = symtab_offset + 16;
        write_u32(&mut object, symbol_entry, 1);
        object[symbol_entry + 4] = 0x10;
        write_u16(&mut object, symbol_entry + 14, 1);
        object
    }

    fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
        buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
}
