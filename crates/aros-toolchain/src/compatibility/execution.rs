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

use aros_common::toolchain_layout::ToolchainToolLayout;
use aros_common::{sha256_bytes, ArosCompilerIdentity, CancellationToken, Sha256Digest};
use serde::{Deserialize, Serialize};

use super::{
    checked_executable, run_probe_set, verify_standalone_outputs,
    verify_standalone_outputs_with_compilers, CompatibilityCommand, CompatibilityPhase,
    CompatibilityPreparation, CompatibilityProbeRequest, CompatibilityProbeSet,
    CompatibilityProbeSetRequest, HostToolClosure, StandaloneOutputReport, StandaloneOutputRequest,
    StandaloneTargetArtifacts, TwoRootRelocation,
};
use crate::compatibility_ports::{
    safe_fetch_marker_path, safe_relative_path, CompatibilityPortsPayload,
    CompatibilityPortsSources,
};
use crate::profiles::Profile;
use crate::python_environment::PythonEnvironment;
use crate::recipe::GitObjectId;
use crate::source_audit::{self, Budget as SourceAuditBudget};
use crate::{canonical, inspection, ContractError};

const MAX_MAKE_JOBS: usize = 64;
const UPSTREAM_SOURCE_AUDIT_TIMEOUT: Duration = Duration::from_mins(5);
const COMPATIBILITY_RECEIPT_SCHEMA: &str = "aros-toolchain-native-compatibility-receipt-v2";
const GNU_COMPATIBILITY_RECEIPT_SCHEMA: &str = "aros-toolchain-native-compatibility-receipt-v3";
const LLVM_V2_COMPATIBILITY_RECEIPT_SCHEMA: &str = "aros-toolchain-native-compatibility-receipt-v4";
const COMPATIBILITY_RECEIPT_FILE: &str = "native-compatibility.receipt.json";

#[path = "execution_receipt_readback.rs"]
mod receipt_readback;
pub use receipt_readback::{
    readback_native_compatibility_receipt, NativeCompatibilityCommandLogs,
    NativeCompatibilityExpectedPackage, NativeCompatibilityExpectedPortSource,
    NativeCompatibilityReceiptReadback, NativeCompatibilityReceiptReadbackRequest,
};

#[path = "execution_retained.rs"]
mod retained;
pub use retained::{
    execute_native_compatibility_with_readback, readback_retained_native_compatibility,
};

#[path = "execution_portable.rs"]
mod portable;
pub use portable::{
    export_retained_native_compatibility, readback_portable_native_compatibility,
    NativeCompatibilityReceiptExpectations, PortableNativeCompatibilityExport,
    PortableNativeCompatibilityReadback, PortableNativeCompatibilityRequest,
    PORTABLE_NATIVE_COMPATIBILITY_MANIFEST,
};

#[path = "execution_observation.rs"]
mod observation;
pub use observation::{
    execute_native_compatibility_with_export, NativeCompatibilityExecutionExport,
    NativeCompatibilityInputClaims,
};

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
    /// Package-build source commit from the independently verified recipe.
    /// Required for family-v2 packages. It is separate from the upstream
    /// consumer commit selected by the profiles document; neither replaces
    /// the other. This declaration alone is not source authentication.
    pub package_source_commit: Option<GitObjectId>,
    /// Explicit source-owned preset for GNU consumers, distinct from the
    /// compiler profile. LLVM's legacy adapter does not use this selector.
    pub source_preset: Option<String>,
    /// Explicit prepared cache for raw source-declared host-generator inputs.
    /// Required only when the bound native source contract declares them.
    /// Execution verifies every exact size/hash and never downloads a miss.
    pub host_generator_cache_root: Option<PathBuf>,
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
    /// Fresh measured host command closure for CMake host tools and upstream
    /// configure and Make.
    pub host_tools: HostToolClosure,
    /// Exact, private source closure supplied to pinned upstream `includes` and
    /// `linklibs` rules. It replaces their mutable network download paths.
    pub ports_sources: CompatibilityPortsSources,
    /// Closed v1 build-host selector used to derive the exact platform-specific
    /// command-role contract. The caller cannot weaken that contract.
    pub host: String,
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
    ports_sources: Vec<CompatibilityReceiptPortsSource>,
    phase_reports: Vec<CompatibilityReceiptPhase>,
    standalone_targets: BTreeMap<String, CompatibilityReceiptTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package: Option<CompatibilityReceiptPackage>,
}

/// V3 binds GNU packages and V4 binds LLVM family-v2 packages. Neither is a
/// signature or publication admission. Legacy LLVM V2 receipts retain their
/// existing byte shape, as do GNU V3 receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityReceiptPackage {
    compiler: ArosCompilerIdentity,
    host: String,
    target_profile: String,
    target_triple: String,
    archive_sha256: Sha256Digest,
    archive_size: u64,
    manifest_sha256: Sha256Digest,
    tree_sha256: Sha256Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_preset: Option<String>,
    source_tree_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityReceiptPhase {
    phase: CompatibilityPhase,
    report_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompatibilityReceiptPortsSource {
    id: String,
    cache_filename: String,
    relative_path: String,
    fetch_marker: String,
    sha256: Sha256Digest,
    size: u64,
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
/// directory and resolves its source-owned host tools through the sealed
/// measured closure. Configure, includes and linklibs use the second
/// relocation root and the same closure plus the verified private Python
/// environment. Standalone consumers use absolute prefix compilers and
/// `PATH=/nonexistent`; their outputs are parsed for AROS ELF and collector
/// evidence after the six process reports succeed.
///
/// GNU uses the inventoried executable-role document and the profile-bound
/// RISC-V contract. Family-v2 packages of either compiler family remeasure both
/// complete extracted trees before output creation and after the probes. The
/// caller must exclusively own these
/// roots while executing; these checks are not a filesystem snapshot or a
/// lock against concurrent writers. GNU receipts use v3 and LLVM family-v2
/// receipts use v4 to bind the canonical manifest identity. Neither grants
/// signature authentication or release admission. Legacy LLVM v1 remains v2.
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
    let cmake_source_cache = request
        .ports_sources
        .materialize_cmake_cache(&outputs.cmake_build)?;
    let environments = super::environment_plan::prepare_native_environments(
        &request.host_python,
        &request.host_tools,
        &request.host,
    )?;
    let sealed_environment = environments.sdk;
    let poisoned_environment = environments.standalone;
    let helpers_root = helpers_root(&request.preparation)?;
    let mut cmake_commands = vec![cmake_command(request, &inputs, &outputs, &helpers_root)?];
    if let Some(binding) = &inputs.native_consumer_contract {
        let mut arguments = vec![
            "--build".into(),
            utf8_path(&outputs.cmake_build, "native consumer CMake build root")?,
            "--parallel".into(),
            request.make_jobs.to_string(),
            "--target".into(),
        ];
        arguments.extend(binding.contract.roots.iter().cloned());
        cmake_commands.push(CompatibilityCommand {
            program: inputs.cmake.clone(),
            arguments,
        });
    }
    let upstream_commands = upstream_commands(request, &inputs, &outputs)?;
    let standalone = standalone_commands(request, &inputs, &outputs)?;

    let probes = CompatibilityProbeSetRequest {
        probes: vec![
            CompatibilityProbeRequest {
                phase: CompatibilityPhase::CmakeConsumer,
                commands: cmake_commands,
                // The AROS CMake engine builds host tools. Its selected host
                // compiler is an explicit measured closure entry, so this
                // phase must not inherit an ambient runner PATH.
                environment: sealed_environment.clone(),
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
    if let Some(host_inputs) = &inputs.host_generator_cache {
        host_inputs.revalidate()?;
    }
    cmake_source_cache.revalidate()?;
    request.ports_sources.clear_upstream_fetch_markers()?;
    request.ports_sources.revalidate()?;
    let standalone_request = StandaloneOutputRequest {
        output_root: outputs.standalone,
        targets: standalone.outputs,
    };
    let standalone = if package_binding_required(request) {
        let compilers = standalone_request
            .targets
            .keys()
            .map(|triple| (triple.clone(), inputs.compiler.clone()))
            .collect();
        let verified = verify_standalone_outputs_with_compilers(&standalone_request, &compilers)?;
        // A probe cannot alter a prefix and then turn its outputs into valid
        // compatibility evidence. Remeasure both complete inventories.
        revalidate_bound_roots(request)?;
        super::validate_preparation(&request.preparation)?;
        verified
    } else {
        verify_standalone_outputs(&standalone_request)?
    };
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
        &inputs.ports_sources,
        request,
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
    ports_sources: &CompatibilityPortsSources,
    request: &NativeCompatibilityRequest,
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
    let compiler = request
        .relocation
        .second
        .verified
        .manifest
        .compiler_identity()
        .map_err(|_| {
            ContractError::compatibility("compatibility package compiler identity is invalid")
        })?;
    let is_gnu = matches!(compiler, ArosCompilerIdentity::Gnu { .. });
    let package = if package_binding_required(request) {
        let verified = &request.relocation.second.verified;
        let manifest = &verified.manifest;
        let bytes = canonical::bytes(&serde_json::to_value(manifest).map_err(|_| {
            ContractError::compatibility("cannot encode compatibility package identity")
        })?)?;
        Some(CompatibilityReceiptPackage {
            compiler,
            host: manifest.host.clone(),
            target_profile: manifest.target_profile.clone(),
            target_triple: manifest.target_triple.clone(),
            archive_sha256: verified.archive_sha256.clone(),
            archive_size: verified.archive_size,
            manifest_sha256: sha256_bytes(&bytes),
            tree_sha256: Sha256Digest::parse(&manifest.tree_sha256).map_err(|_| {
                ContractError::compatibility("compatibility package tree identity is invalid")
            })?,
            source_preset: if is_gnu {
                Some(request.source_preset.clone().ok_or_else(|| {
                    ContractError::compatibility("GNU compatibility lost its source preset")
                })?)
            } else {
                None
            },
            source_tree_sha256: request.preparation.source_tree_sha256.clone(),
        })
    } else {
        None
    };
    let document = CompatibilityReceiptDocument {
        schema: if is_gnu {
            GNU_COMPATIBILITY_RECEIPT_SCHEMA
        } else if package.is_some() {
            LLVM_V2_COMPATIBILITY_RECEIPT_SCHEMA
        } else {
            COMPATIBILITY_RECEIPT_SCHEMA
        }
        .into(),
        operation: "native-compatibility".into(),
        upstream_source_commit: upstream_source_commit.as_str().into(),
        upstream_source_tree: upstream_source_tree.as_str().into(),
        ports_sources: receipt_ports_sources(ports_sources)?,
        phase_reports,
        standalone_targets,
        package,
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
        if self.operation != "native-compatibility"
            || match self.schema.as_str() {
                COMPATIBILITY_RECEIPT_SCHEMA => self.package.is_some(),
                GNU_COMPATIBILITY_RECEIPT_SCHEMA | LLVM_V2_COMPATIBILITY_RECEIPT_SCHEMA => {
                    self.package.is_none()
                }
                _ => true,
            }
        {
            return Err(ContractError::compatibility(
                "native compatibility receipt has an unsupported schema or operation",
            ));
        }
        if let Some(package) = &self.package {
            let valid_family = match (&package.compiler, self.schema.as_str()) {
                (ArosCompilerIdentity::Gnu { .. }, GNU_COMPATIBILITY_RECEIPT_SCHEMA) => package
                    .source_preset
                    .as_deref()
                    .is_some_and(crate::profiles::identifier),
                (ArosCompilerIdentity::Llvm { .. }, LLVM_V2_COMPATIBILITY_RECEIPT_SCHEMA) => {
                    package.source_preset.is_none()
                }
                _ => false,
            };
            let mut expected_targets = BTreeSet::from([package.target_triple.as_str()]);
            if self.schema == LLVM_V2_COMPATIBILITY_RECEIPT_SCHEMA
                && package.target_profile == "pc-x86_64"
            {
                expected_targets.insert("i386-unknown-aros");
            }
            if !valid_family
                || package
                    .compiler
                    .validate_for_target(&package.target_triple)
                    .is_err()
                || !crate::profiles::identifier(&package.host)
                || !crate::profiles::identifier(&package.target_profile)
                || package.archive_size == 0
                || self
                    .standalone_targets
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
                    != expected_targets
            {
                return Err(ContractError::compatibility(
                    "native compatibility receipt has an invalid package binding",
                ));
            }
        }
        let ports_ids = self
            .ports_sources
            .iter()
            .map(|source| source.id.as_str())
            .collect::<BTreeSet<_>>();
        let ports_paths = self
            .ports_sources
            .iter()
            .map(|source| source.relative_path.as_str())
            .collect::<BTreeSet<_>>();
        let declared_marker_count = self
            .ports_sources
            .iter()
            .filter(|source| !source.fetch_marker.is_empty())
            .count();
        let ports_markers = self
            .ports_sources
            .iter()
            .filter(|source| !source.fetch_marker.is_empty())
            .map(|source| source.fetch_marker.as_str())
            .collect::<BTreeSet<_>>();
        if self.ports_sources.is_empty()
            || self.ports_sources.len() > 128
            || ports_ids.len() != self.ports_sources.len()
            || ports_paths.len() != self.ports_sources.len()
            || ports_markers.len() != declared_marker_count
            || self.ports_sources.iter().any(|source| {
                !crate::profiles::identifier(&source.id)
                    || source.cache_filename.is_empty()
                    || !safe_relative_path(&source.relative_path)
                    || (!source.fetch_marker.is_empty()
                        && !safe_fetch_marker_path(&source.fetch_marker))
                    || source.size == 0
            })
        {
            return Err(ContractError::compatibility(
                "native compatibility receipt does not bind the exact upstream source-input closure",
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

fn receipt_ports_sources(
    ports_sources: &CompatibilityPortsSources,
) -> Result<Vec<CompatibilityReceiptPortsSource>, ContractError> {
    ports_sources.revalidate()?;
    let sources = ports_sources
        .payloads()
        .into_iter()
        .map(
            |source: CompatibilityPortsPayload| CompatibilityReceiptPortsSource {
                id: source.id,
                cache_filename: source.cache_filename,
                relative_path: source.relative_path,
                fetch_marker: source.fetch_marker,
                sha256: source.sha256,
                size: source.size,
            },
        )
        .collect::<Vec<_>>();
    Ok(sources)
}

fn valid_receipt_artifact(artifact: &CompatibilityReceiptArtifact) -> bool {
    artifact.size > 0 && matches!(artifact.class.as_str(), "elf32" | "elf64")
}

#[derive(Debug)]
struct Inputs {
    compiler: ArosCompilerIdentity,
    gnu_drivers: BTreeMap<String, PathBuf>,
    source_profile: Option<aros_common::TargetProfile>,
    native_contract: Option<aros_common::native_build_contract::LoadedNativeBuildContract>,
    native_consumer_contract:
        Option<aros_common::native_consumer_contract::LoadedNativeConsumerContract>,
    host_generator_cache: Option<super::host_generator_inputs::HostGeneratorInputs>,
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
    ports_sources: CompatibilityPortsSources,
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
    let manifest = &request.relocation.second.verified.manifest;
    let compiler = manifest.compiler_identity().map_err(|_| {
        ContractError::compatibility(
            "native compatibility package has an invalid compiler identity",
        )
    })?;
    let family = match compiler {
        ArosCompilerIdentity::Llvm { .. } => crate::source_lock::CompilerFamily::Llvm,
        ArosCompilerIdentity::Gnu { .. } => crate::source_lock::CompilerFamily::Gnu,
    };
    if family != request.profile.family()
        || manifest.target_profile != request.profile.name()
        || manifest.target_triple != request.profile.target_triple()
        || manifest.host != request.host
    {
        return Err(ContractError::compatibility(
            "native compatibility profile/host differs from the package identity",
        ));
    }
    if package_binding_required(request) {
        let package_source_commit = request.package_source_commit.as_ref().ok_or_else(|| {
            ContractError::compatibility(
                "family-v2 compatibility requires the recipe-bound package source commit",
            )
        })?;
        if manifest.profiles_sha256 != request.profile.document_sha256().as_str()
            || manifest.source_commit != package_source_commit.as_str()
        {
            return Err(ContractError::compatibility(
                "family-v2 compatibility source/profile contract differs from the package identity",
            ));
        }
        revalidate_bound_roots(request)?;
    }
    let gnu_drivers = if let ArosCompilerIdentity::Gnu { target, .. } = &compiler {
        if request.profile.target() != Some(target) {
            return Err(ContractError::compatibility(
                "GNU compatibility source/profile contract differs from the package identity",
            ));
        }
        let layout = ToolchainToolLayout::load(&upstream_toolchain_root).map_err(|_| {
            ContractError::compatibility("GNU compatibility executable layout is invalid")
        })?;
        layout
            .validate_binding(&compiler, request.profile.target_triple())
            .map_err(|_| {
                ContractError::compatibility(
                    "GNU compatibility executable layout is not compiler-bound",
                )
            })?;
        layout
            .resolve_tools(&upstream_toolchain_root)
            .map_err(|_| {
                ContractError::compatibility(
                    "GNU compatibility executable roles cannot be resolved",
                )
            })?
            .into_iter()
            .map(|(role, path)| (role.to_owned(), path))
            .collect()
    } else {
        BTreeMap::new()
    };
    let source_profile = if matches!(compiler, ArosCompilerIdentity::Gnu { .. }) {
        Some(validate_gnu_source_profile(request)?)
    } else {
        if request.source_preset.is_some() {
            return Err(ContractError::compatibility(
                "the legacy LLVM compatibility adapter does not accept a source preset",
            ));
        }
        None
    };
    let native_consumer_contract = source_profile
        .as_ref()
        .and_then(|profile| {
            profile
                .native_consumer_contract
                .as_ref()
                .map(|path| (profile, path))
        })
        .map(|(profile, path)| {
            let binding =
                aros_common::native_consumer_contract::load_bound_native_consumer_contract(
                    &request.preparation.source_root,
                    Path::new(path),
                    profile,
                )
                .map_err(|_| {
                    ContractError::compatibility("GNU source native consumer contract is invalid")
                })?;
            aros_common::native_consumer_contract::validate_native_consumer_compiler(
                &binding.contract,
                &compiler,
                request.profile.target_triple(),
            )
            .map_err(|_| {
                ContractError::compatibility(
                    "GNU source native consumer contract differs from the compiler",
                )
            })?;
            Ok(binding)
        })
        .transpose()?;
    // A source profile may describe both a full boot/media build and a bounded
    // compiler consumer. Compatibility qualification selects the consumer
    // contract explicitly when present and never falls back when that binding
    // fails validation.
    let native_contract = if native_consumer_contract.is_some() {
        None
    } else {
        source_profile
            .as_ref()
            .and_then(|profile| {
                profile
                    .native_build_contract
                    .as_ref()
                    .map(|path| (profile, path))
            })
            .map(|(profile, path)| {
                let binding = aros_common::native_build_contract::load_bound_native_build_contract(
                    &request.preparation.source_root,
                    Path::new(path),
                    profile,
                )
                .map_err(|_| {
                    ContractError::compatibility("GNU source native build contract is invalid")
                })?;
                aros_common::native_build_contract::validate_native_build_compiler(
                    &binding.contract,
                    &compiler,
                    request.profile.target_triple(),
                )
                .map_err(|_| {
                    ContractError::compatibility(
                        "GNU source native build contract differs from the compiler",
                    )
                })?;
                if !gnu_drivers.contains_key("objdump") {
                    return Err(ContractError::compatibility(
                        "GNU native build contracts require an inventoried objdump role",
                    ));
                }
                Ok(binding)
            })
            .transpose()?
    };
    let upstream_source = checked_directory(
        &request.upstream_source_root,
        "pristine upstream compatibility source root",
    )?;
    let upstream_source_tree =
        verify_pristine_upstream_source(&upstream_source, &request.upstream_source_commit)?;
    request.ports_sources.revalidate()?;
    let ports_sources = checked_directory(
        &request.ports_sources.root,
        "compatibility ports source directory",
    )?;
    if upstream_source == request.preparation.source_root
        || upstream_source == request.preparation.engine_root
        || upstream_source == cmake_toolchain_root
        || upstream_source == upstream_toolchain_root
        || upstream_source == ports_sources
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
    let generators = native_consumer_contract.as_ref().map_or_else(
        || {
            native_contract.as_ref().map_or(&[][..], |binding| {
                binding.contract.host_file_generators.as_slice()
            })
        },
        |binding| binding.contract.host_file_generators.as_slice(),
    );
    let host_generator_cache = super::host_generator_inputs::prepare(
        generators,
        request.host_generator_cache_root.as_deref(),
    )?;
    Ok(Inputs {
        compiler,
        gnu_drivers,
        source_profile,
        native_contract,
        native_consumer_contract,
        host_generator_cache,
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
        ports_sources: request.ports_sources.clone(),
    })
}

fn validate_gnu_source_profile(
    request: &NativeCompatibilityRequest,
) -> Result<aros_common::TargetProfile, ContractError> {
    super::validate_preparation(&request.preparation)?;
    let name = request.source_preset.as_deref().ok_or_else(|| {
        ContractError::compatibility(
            "GNU compatibility requires an explicit source preset from aros-targets.toml",
        )
    })?;
    let path = request.preparation.source_root.join("aros-targets.toml");
    let bytes = super::read_regular_bounded(
        &path,
        "source-owned target configuration",
        crate::canonical::MAX_DOCUMENT_BYTES,
    )?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        ContractError::compatibility("source-owned target configuration is not UTF-8")
    })?;
    let config =
        aros_common::TargetProfile::parse_config(text, "aros-targets.toml").map_err(|_| {
            ContractError::compatibility("source-owned target configuration is invalid")
        })?;
    let profile = config
        .targets
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| {
            ContractError::compatibility("GNU compatibility source preset is not declared")
        })?;
    let cpu = profile.arch.source_cpu();
    if profile.toolchain_profile() != request.profile.name()
        || cpu != request.profile.cpu()
        || profile.platform != request.profile.platform()
        || profile
            .float_abi
            .as_ref()
            .is_some_and(|abi| abi != request.profile.float_abi())
        || profile
            .transpiler
            .as_ref()
            .is_none_or(|context| context.toolchain != "gnu")
    {
        return Err(ContractError::compatibility(
            "GNU compatibility source preset differs from the compiler profile",
        ));
    }
    Ok(profile)
}

const fn package_binding_required(request: &NativeCompatibilityRequest) -> bool {
    request.relocation.second.verified.manifest.schema == 2
}

fn revalidate_bound_roots(request: &NativeCompatibilityRequest) -> Result<(), ContractError> {
    for package in [&request.relocation.first, &request.relocation.second] {
        crate::package_extract::verify_extracted_tree(&package.root, &package.verified.manifest)
            .map_err(|_| {
                ContractError::compatibility(
                    "family-v2 compatibility extracted package differs from its verified inventory",
                )
            })?;
    }
    Ok(())
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
    let mut protected: Vec<&Path> = vec![
        &request.preparation.source_root,
        &request.preparation.engine_root,
        &inputs.cmake_toolchain_root,
        &inputs.upstream_toolchain_root,
        &inputs.upstream_source,
        &inputs.standalone_c,
        &inputs.standalone_cxx,
        &request.host_tools.root,
        &inputs.ports_sources.root,
    ];
    protected.extend(
        request
            .host_python
            .import_roots()
            .iter()
            .map(PathBuf::as_path),
    );
    if let Some(host_inputs) = &inputs.host_generator_cache {
        protected.extend([host_inputs.cache_root(), host_inputs.root()]);
    }
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
    // This is a revalidated entry in the measured closure. Passing it
    // explicitly prevents the CMake engine's `cc` default from resolving an
    // ambient or cross compiler.
    let host_cc = utf8_path(
        &request.host_tools.root.join("cc"),
        "measured compatibility host C compiler",
    )?;
    let mut command = CompatibilityCommand {
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
            format!("-DAROS_HOST_CC={host_cc}"),
            // Compatibility qualification must not inherit a host compiler-cache
            // launcher. The embedded engine requires a frontend-selected policy,
            // and this deterministic producer phase deliberately selects off.
            "-DAROS_COMPILER_CACHE_MODE=off".into(),
            // Every configure-time source inventory comes from the private,
            // lock-verified cache materialized above. A missing input is a
            // closed qualification failure, never an implicit download.
            "-DAROS_FETCH_OFFLINE=ON".into(),
            "-DCMAKE_BUILD_TYPE=Release".into(),
        ],
    };
    if let Some(host_inputs) = &inputs.host_generator_cache {
        host_inputs.revalidate()?;
        command.arguments.push(format!(
            "-DAROS_NATIVE_HOST_INPUT_DIRECTORY={}",
            utf8_path(host_inputs.root(), "private verified host-generator inputs")?
        ));
    }
    if let Some(profile) = &inputs.source_profile {
        let context = profile.transpiler.as_ref().ok_or_else(|| {
            ContractError::compatibility("GNU source preset lost its transpiler context")
        })?;
        command.arguments.extend([
            "-DAROS_TOOLCHAIN=gnu".into(),
            format!("-DAROS_TARGET_PROFILE={}", profile.name),
            format!("-DAROS_CROSS_TOOLCHAIN_PROFILE={}", request.profile.name()),
            format!("-DAROS_TARGET_TRIPLE={}", request.profile.target_triple()),
            format!("-DAROS_TARGET_FAMILY={}", context.family),
            format!("-DAROS_TARGET_VARIANT={}", context.variant),
            format!("-DAROS_TARGET_CPU32={}", context.cpu32),
            format!(
                "-DAROS_ENABLE_MMU={}",
                if context.use_mmu { "ON" } else { "OFF" }
            ),
        ]);
        if let Some(version) = &context.mesa_version {
            command
                .arguments
                .push(format!("-DAROS_MESA_VERSION={version}"));
        }
        command
            .arguments
            .push(format!("-DAROS_TARGET_BOOTLOADER={}", profile.bootloader()));
        if let Some(binding) = &inputs.native_consumer_contract {
            command.arguments.extend([
                format!(
                    "-DAROS_NATIVE_CONSUMER_CONTRACT={}",
                    utf8_path(&binding.path, "native consumer contract")?
                ),
                format!("-DAROS_NATIVE_CONSUMER_CONTRACT_SHA256={}", binding.sha256),
                format!("-DAROS_ABI_FLAVOUR={}", binding.contract.abi.flavour),
                format!(
                    "-DAROS_ABI_PLATFORM_SMP={}",
                    if binding.contract.abi.platform_smp {
                        "ON"
                    } else {
                        "OFF"
                    }
                ),
            ]);
        } else if let Some(abi) = &profile.bootstrap_abi {
            command.arguments.extend([
                format!("-DAROS_ABI_FLAVOUR={}", abi.flavour),
                format!(
                    "-DAROS_ABI_PLATFORM_SMP={}",
                    if abi.platform_smp { "ON" } else { "OFF" }
                ),
            ]);
        }
        if let Some(binding) = &inputs.native_contract {
            command.arguments.extend([
                format!(
                    "-DAROS_NATIVE_BUILD_CONTRACT={}",
                    utf8_path(&binding.path, "native build contract")?
                ),
                format!("-DAROS_NATIVE_BUILD_CONTRACT_SHA256={}", binding.sha256),
            ]);
        }
    } else {
        command.arguments.push("-DAROS_ENABLE_MMU=ON".into());
    }
    Ok(command)
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
    let ports_sources = utf8_path(
        &inputs.ports_sources.root,
        "verified compatibility ports source directory",
    )?;
    let mut configure_arguments = vec![format!("--target={}", request.profile.configure_target())];
    match &inputs.compiler {
        ArosCompilerIdentity::Llvm { version } => configure_arguments.extend([
            "--with-toolchain=llvm".into(),
            format!("--with-llvm-version={version}"),
        ]),
        ArosCompilerIdentity::Gnu {
            gcc_version,
            binutils_version,
            ..
        } => configure_arguments.extend([
            "--with-toolchain=gnu".into(),
            format!("--with-gcc-version={gcc_version}"),
            format!("--with-binutils-version={binutils_version}"),
        ]),
    }
    configure_arguments.extend([
        "--with-aros-toolchain=yes".into(),
        format!("--with-portssources={ports_sources}"),
        format!("--with-aros-toolchain-install={toolchain}"),
    ]);
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
            arguments: configure_arguments,
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
    let (c_driver, cxx_driver) = if matches!(inputs.compiler, ArosCompilerIdentity::Gnu { .. }) {
        let driver = |role: &str| {
            inputs.gnu_drivers.get(role).cloned().ok_or_else(|| {
                ContractError::compatibility(
                    "GNU compatibility executable layout lacks a required compiler role",
                )
            })
        };
        (driver("c")?, driver("cxx")?)
    } else {
        (bin.join("clang"), bin.join("clang++"))
    };
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
        let mut c_arguments = match &inputs.compiler {
            ArosCompilerIdentity::Llvm { .. } => vec![format!("--target={triple}")],
            ArosCompilerIdentity::Gnu { target, .. } => vec![
                format!("-march={}", target.isa()),
                format!("-mabi={}", target.abi()),
                format!("-mcmodel={}", target.code_model()),
                if target.unaligned_access() {
                    "-mno-strict-align"
                } else {
                    "-mstrict-align"
                }
                .into(),
            ],
        };
        c_arguments.push(format!("--sysroot={developer}"));
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
            program: c_driver.clone(),
            arguments: c_arguments,
        });
        cxx_commands.push(CompatibilityCommand {
            program: cxx_driver.clone(),
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
    if profile.family() == crate::source_lock::CompilerFamily::Llvm && profile.name() == "pc-x86_64"
    {
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
#[path = "execution_gnu_tests.rs"]
mod gnu_tests;
#[cfg(test)]
pub use gnu_tests::riscv_elf as fixture_riscv_elf;

#[cfg(test)]
#[path = "execution_tests.rs"]
mod tests;
#[cfg(test)]
pub use tests::{fixture_elf32, fixture_elf64};

#[cfg(test)]
#[path = "execution_llvm_v2_tests.rs"]
mod llvm_v2_tests;

#[cfg(test)]
#[path = "execution_receipt_readback_tests.rs"]
pub mod receipt_readback_tests;

#[cfg(test)]
#[path = "execution_retained_tests.rs"]
mod retained_tests;

#[cfg(test)]
#[path = "execution_portable_tests.rs"]
mod portable_tests;

#[cfg(test)]
#[path = "execution_environment_tests.rs"]
mod environment_tests;
