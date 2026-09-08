//! Tools-owned compatibility-probe preparation and execution.
//!
//! [`prepare`] establishes the two identities a later runner may use: a fresh
//! materialization of the embedded CMake engine and one exact directory of
//! helpers built from the selected tools snapshot. [`execute_native_compatibility`]
//! then composes those checked inputs into the six bounded M5 process phases.
//! Neither boundary has source-fetch, cache, tag, publication, or release
//! authority; execution never selects a source-tree engine or shared Cargo
//! target directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use aros_common::{
    exit_signal, measure_tree_content_cas, open_regular_file_nofollow, publish_atomic_file,
    run_output_with_input_and_control, sha256_bytes, sha256_reader, AtomicFilePolicy,
    CancellationToken, DiagnosticContext, Sha256Digest,
};
use serde::{Deserialize, Serialize};

use crate::filesystem::open_directory;
use crate::package_extract::{
    checked_absent_root, verify_and_extract, ExtractedPackage, PackageExtractionRequest,
};
use crate::package_verify::PackageVerificationRequest;
use crate::ContractError;

mod environment;
mod execution;
mod host_tools;
mod standalone;

pub use environment::{CompatibilityEnvironment, CompatibilityHostToolReport};
pub use execution::{
    execute_native_compatibility, NativeCompatibilityReport, NativeCompatibilityRequest,
    StandaloneFixtures,
};
pub use host_tools::{
    prepare_host_tool_closure, CompatibilityHostTool, HostToolClosure, HostToolClosureRequest,
    HostToolIdentity,
};
pub use standalone::{
    verify_standalone_outputs, StandaloneArtifactIdentity, StandaloneOutputReport,
    StandaloneOutputRequest, StandaloneTargetArtifacts, StandaloneTargetReport,
};
#[cfg(test)]
use standalone::{CXX_COLLECTOR_SYMBOL, C_COLLECTOR_SYMBOL};

/// Helpers a compatibility probe must resolve from one exact fresh target root.
pub const REQUIRED_HELPERS: &[&str] = &[
    "aros-transpiler",
    "aros-genmodule",
    "aros-collect",
    "aros-ahi-runner",
    "aros-fetch",
];

const ENGINE_DIRECTORY: &str = "aros-cmake-engine";
const MAX_ENGINE_ENTRIES: usize = 4_096;
const MAX_ENGINE_DEPTH: usize = 32;
const MAX_ENGINE_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROBE_ARGUMENTS: usize = 256;
const MAX_PROBE_ARGUMENT_BYTES: usize = 64 * 1024;
const PROBE_CAPTURE_LIMIT: usize = 256 * 1024;
const MAX_RENDERED_LOG_BYTES: usize = PROBE_CAPTURE_LIMIT + 256;
const COMPATIBILITY_REPORT_SCHEMA: &str = "aros-toolchain-compatibility-report-v5";
const MAX_PROBE_COMMANDS: usize = 2;
const REQUIRED_PROBE_PHASES: [CompatibilityPhase; 6] = [
    CompatibilityPhase::CmakeConsumer,
    CompatibilityPhase::UpstreamConfigure,
    CompatibilityPhase::UpstreamIncludes,
    CompatibilityPhase::UpstreamLinklibs,
    CompatibilityPhase::StandaloneC,
    CompatibilityPhase::StandaloneCxx,
];

/// Explicit roots for preparing a tools-owned compatibility probe.
#[derive(Debug, Clone)]
pub struct CompatibilityPreparationRequest {
    /// Isolated source tree used by a later probe. It must not carry `cmake/`.
    pub source_root: PathBuf,
    /// Existing owned work root; this operation creates one fresh engine leaf.
    pub work_root: PathBuf,
    /// Fresh Cargo target's `release/` directory for the exact tools snapshot.
    pub helpers_root: PathBuf,
}

/// One measured helper selected from the exact preparation root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperIdentity {
    /// Absolute helper path below the selected helper root.
    pub path: PathBuf,
    /// Measured helper SHA-256.
    pub sha256: Sha256Digest,
    /// Measured helper byte length.
    pub size: u64,
}

/// Verified tools-owned engine and helper identities for a later probe runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPreparation {
    /// Engine-free source tree selected for the compatibility run.
    pub source_root: PathBuf,
    /// Content-only digest of the selected source tree before any probe starts.
    pub source_tree_sha256: Sha256Digest,
    /// Fresh materialized engine root, distinct from the source tree.
    pub engine_root: PathBuf,
    /// Embedded engine API version selected by this tools build.
    pub engine_api_version: u32,
    /// Digest of the embedded engine selected by this tools build.
    pub engine_sha256: Sha256Digest,
    /// Every required helper, keyed by its fixed executable name.
    pub helpers: BTreeMap<String, HelperIdentity>,
}

/// Inputs for the two independent roots required by a relocation probe.
///
/// Both roots read the same complete package contract independently. The
/// operation creates no work, source, engine, helper, tag or release material.
#[derive(Debug, Clone)]
pub struct TwoRootRelocationRequest {
    /// Exact complete package set that every root must reverify.
    pub verification: PackageVerificationRequest,
    /// First absent destination root.
    pub first_root: PathBuf,
    /// Second absent destination root.
    pub second_root: PathBuf,
}

/// Two independently reverified extracted payload roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoRootRelocation {
    /// First retained payload root.
    pub first: ExtractedPackage,
    /// Second retained payload root.
    pub second: ExtractedPackage,
}

/// Materialize two independent roots from one complete native package set.
///
/// The two destination identities are checked before either one is created.
/// Each extraction independently revalidates the complete four-member package
/// set, remeasures the selected archive and re-inventories its payload. The
/// operation refuses an overlap with each other or the package directory, an
/// existing destination, or a changed package. If either extraction begins and
/// later fails, all already-created roots remain available for diagnosis; it
/// never removes or adopts material.
///
/// # Errors
///
/// Returns AX0602 for an unsafe package or extraction root. It has no process,
/// network, cache, source-tree, tag or publication authority.
pub fn extract_two_roots(
    request: &TwoRootRelocationRequest,
) -> Result<TwoRootRelocation, ContractError> {
    let first_root = checked_absent_root(&request.first_root)?;
    let second_root = checked_absent_root(&request.second_root)?;
    if first_root == second_root {
        return Err(ContractError::verification(
            "compatibility relocation roots must have distinct output identities",
        ));
    }
    let package_directory = checked_verified_package_directory(&request.verification.package_dir)?;
    if first_root.starts_with(&package_directory) || second_root.starts_with(&package_directory) {
        return Err(ContractError::verification(
            "compatibility relocation root cannot be created inside the verified package directory",
        ));
    }
    let first = verify_and_extract(&PackageExtractionRequest {
        verification: request.verification.clone(),
        output_root: first_root,
    })?;
    let second = verify_and_extract(&PackageExtractionRequest {
        verification: request.verification.clone(),
        output_root: second_root,
    })?;
    if first.verified != second.verified {
        return Err(ContractError::verification(
            "compatibility relocation roots were not extracted from one measured package identity",
        ));
    }
    Ok(TwoRootRelocation { first, second })
}

fn checked_verified_package_directory(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::verification(
            "verified package directory must be an absolute directory",
        ));
    }
    let canonical = path.canonicalize().map_err(|_| {
        ContractError::verification("verified package directory cannot be canonicalized")
    })?;
    open_directory(&canonical).map_err(|_| {
        ContractError::verification(
            "verified package directory does not resolve to a real directory without symlink ancestors",
        )
    })?;
    Ok(canonical)
}

/// Prepare a fresh tools-owned engine and resolve every required helper.
///
/// The selected source tree must be an engine-free probe input: a top-level
/// `cmake/` file, directory or link is rejected. `work_root` and
/// `helpers_root` must be distinct absolute real directories. The fixed engine
/// destination `work_root/aros-cmake-engine` must be absent; it is never
/// adopted, repaired, or overwritten. Every embedded file is read back and
/// compared with the compiled-in resource after materialization, and every
/// helper must be a regular executable immediate child of `helpers_root`.
///
/// # Errors
///
/// Returns AX0703 for unsafe, mixed, stale or incomplete engine/helper inputs.
/// It performs no process, network, cache, source-tree, tag or publication
/// operation. Failures retain `work_root` and any newly materialized engine for
/// inspection; this function never removes caller data.
pub fn prepare(
    request: &CompatibilityPreparationRequest,
) -> Result<CompatibilityPreparation, ContractError> {
    let source_root = checked_directory(&request.source_root, "compatibility source root")?;
    let work_root = checked_directory(&request.work_root, "compatibility work root")?;
    let helpers_root = checked_directory(&request.helpers_root, "compatibility helpers root")?;
    if source_root == work_root || source_root == helpers_root || work_root == helpers_root {
        return Err(ContractError::compatibility(
            "compatibility source, work, and helper roots must be distinct",
        ));
    }
    reject_source_tree_engine(&source_root)?;
    let source_tree_sha256 = measure_tree_content_cas(&source_root)
        .map_err(|_| {
            ContractError::compatibility(
                "cannot measure the engine-free compatibility source tree without following links",
            )
        })?
        .payload_digest_excluding(None);

    let engine_root = work_root.join(ENGINE_DIRECTORY);
    if engine_root.exists() || fs::symlink_metadata(&engine_root).is_ok() {
        return Err(ContractError::compatibility(
            "compatibility engine destination already exists and cannot be adopted",
        ));
    }
    fs::create_dir(&engine_root).map_err(|_| {
        ContractError::compatibility("cannot create the fresh compatibility engine destination")
    })?;
    let placement = aros_cmake_engine::materialize(&engine_root).map_err(|_| {
        ContractError::compatibility("cannot materialize the embedded tools-owned CMake engine")
    })?;
    if placement.root != engine_root || placement.reused || placement.removed != 0 {
        return Err(ContractError::compatibility(
            "fresh compatibility engine materialization reported unexpected prior state",
        ));
    }
    let engine_sha256 = Sha256Digest::parse(aros_cmake_engine::digest()).map_err(|_| {
        ContractError::compatibility("embedded CMake engine exposes an invalid compiled digest")
    })?;
    verify_materialized_engine(&engine_root, &engine_sha256)?;

    let helpers = resolve_helpers(&helpers_root)?;
    Ok(CompatibilityPreparation {
        source_root,
        source_tree_sha256,
        engine_root,
        engine_api_version: aros_cmake_engine::api_version(),
        engine_sha256,
        helpers,
    })
}

/// Closed phases that may emit a compatibility report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompatibilityPhase {
    /// Configure a consumer through the embedded tools-owned CMake engine.
    CmakeConsumer,
    /// Configure the pristine upstream compatibility source tree.
    UpstreamConfigure,
    /// Build pristine upstream include material.
    UpstreamIncludes,
    /// Build pristine upstream link libraries.
    UpstreamLinklibs,
    /// Compile and link one standalone C consumer.
    StandaloneC,
    /// Compile and link one standalone C++ consumer.
    StandaloneCxx,
}

impl CompatibilityPhase {
    const fn file_stem(self) -> &'static str {
        match self {
            Self::CmakeConsumer => "cmake-consumer",
            Self::UpstreamConfigure => "upstream-configure",
            Self::UpstreamIncludes => "upstream-includes",
            Self::UpstreamLinklibs => "upstream-linklibs",
            Self::StandaloneC => "standalone-c",
            Self::StandaloneCxx => "standalone-cxx",
        }
    }
}

/// One explicit, bounded process invocation in a compatibility phase.
#[derive(Debug, Clone)]
pub struct CompatibilityCommand {
    /// Absolute regular executable selected by the later runner.
    pub program: PathBuf,
    /// Explicit UTF-8 arguments. Raw command strings and shell evaluation are unsupported.
    pub arguments: Vec<String>,
}

/// Explicit, bounded process batch for one compatibility phase.
#[derive(Debug, Clone)]
pub struct CompatibilityProbeRequest {
    /// Closed probe phase determining fresh report names.
    pub phase: CompatibilityPhase,
    /// One or two explicit commands, executed in declaration order.
    ///
    /// The PC profile uses two same-language commands for its x86-64 and i386
    /// collector probes. A phase report is written only once every command
    /// succeeds.
    pub commands: Vec<CompatibilityCommand>,
    /// Closed child-environment policy; inherited environment is forbidden.
    pub environment: CompatibilityEnvironment,
    /// Absolute real working directory owned by the compatibility operation.
    pub current_dir: PathBuf,
    /// Absolute real report directory. Per-phase outputs must not exist yet.
    pub reports_root: PathBuf,
    /// Positive phase deadline, bounded by the caller's whole-operation policy.
    pub timeout: Duration,
    /// Already checked engine/helper identities this probe is bound to.
    pub preparation: CompatibilityPreparation,
}

/// One closed, complete set of compatibility process requests.
///
/// Every M5.2 phase must appear exactly once. The set is a process harness
/// boundary, not a source, package, toolchain, release or publication API.
#[derive(Debug, Clone)]
pub struct CompatibilityProbeSetRequest {
    /// The six exact phase requests, in any caller-provided order.
    pub probes: Vec<CompatibilityProbeRequest>,
}

/// Reports from one completed closed compatibility probe set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityProbeSet {
    /// Exactly one successful report for every required M5.2 phase.
    pub reports: BTreeMap<CompatibilityPhase, CompatibilityProbeReport>,
}

/// Measured helper identity written into a compatibility report without paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityHelperReport {
    /// Measured executable SHA-256.
    pub sha256: Sha256Digest,
    /// Measured executable byte length.
    pub size: u64,
}

/// One measured command and its durable logs in a successful phase report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityCommandReport {
    /// SHA-256 of the validated executable file.
    pub program_sha256: Sha256Digest,
    /// SHA-256 of the exact executable/argument identity.
    pub command_sha256: Sha256Digest,
    /// SHA-256 of the durable rendered stdout log.
    pub stdout_sha256: Sha256Digest,
    /// SHA-256 of the durable rendered stderr log.
    pub stderr_sha256: Sha256Digest,
}

/// Closed, persisted success report for one compatibility process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityProbeReport {
    /// Closed schema name.
    pub schema: String,
    /// Executed compatibility phase.
    pub phase: CompatibilityPhase,
    /// Engine API version bound to this process.
    pub engine_api_version: u32,
    /// Embedded engine digest bound to this process.
    pub engine_sha256: Sha256Digest,
    /// Content-only digest of the engine-free source tree bound to this process.
    pub source_tree_sha256: Sha256Digest,
    /// Exact fixed helper identity set without workstation paths.
    pub helpers: BTreeMap<String, CompatibilityHelperReport>,
    /// Measured host tools admitted to PATH; empty for poisoned standalone phases.
    pub host_tools: BTreeMap<String, CompatibilityHostToolReport>,
    /// SHA-256 of the closed child environment without workstation paths.
    pub environment_sha256: Sha256Digest,
    /// One or two commands and their durable logs in declaration order.
    pub commands: Vec<CompatibilityCommandReport>,
}

impl CompatibilityProbeReport {
    /// Parse and validate one bounded, closed compatibility success report.
    ///
    /// # Errors
    ///
    /// Returns AX0703 for malformed, incomplete or mixed report material.
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::compatibility(
                "compatibility report exceeds the configured document limit",
            ));
        }
        let report: Self = serde_json::from_slice(input).map_err(|_| {
            ContractError::compatibility("compatibility report is not a closed v1 JSON document")
        })?;
        report.validate()?;
        Ok(report)
    }

    fn validate(&self) -> Result<(), ContractError> {
        if self.schema != COMPATIBILITY_REPORT_SCHEMA || self.engine_api_version == 0 {
            return Err(ContractError::compatibility(
                "compatibility report has an unsupported schema or engine API version",
            ));
        }
        if self.commands.is_empty() || self.commands.len() > MAX_PROBE_COMMANDS {
            return Err(ContractError::compatibility(
                "compatibility report has an invalid command batch size",
            ));
        }
        let expected = REQUIRED_HELPERS
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<BTreeSet<_>>();
        if self.helpers.keys().cloned().collect::<BTreeSet<_>>() != expected
            || self.helpers.values().any(|helper| helper.size == 0)
        {
            return Err(ContractError::compatibility(
                "compatibility report does not contain the exact measured helper set",
            ));
        }
        if self
            .host_tools
            .iter()
            .any(|(name, tool)| !host_tools::valid_tool_name(name) || tool.size == 0)
        {
            return Err(ContractError::compatibility(
                "compatibility report contains an invalid measured host-tool identity",
            ));
        }
        Ok(())
    }
}

/// Run one bounded compatibility phase and durably write its report and logs.
///
/// The phase receives only an absolute executable and an explicit argument
/// vector. It never invokes a shell or reads `PATH`. The closed report binds
/// the phase to the prepared embedded engine and helper digests. Logs and the
/// report are created once beneath `reports_root`; existing material is never
/// overwritten, adopted or deleted.
///
/// # Errors
///
/// Returns AX0703 when an input is unsafe, the process cannot be supervised,
/// cancellation/deadline/failure occurs, durable logging fails, or a report
/// cannot be written. Captured logs are retained for any process that starts.
/// No operation here has source, cache, network, tag or publication authority.
pub fn run_probe(
    request: &CompatibilityProbeRequest,
    cancellation: &CancellationToken,
) -> Result<CompatibilityProbeReport, ContractError> {
    validate_probe_request(request)?;
    let current_dir = checked_directory(&request.current_dir, "compatibility process directory")?;
    let reports_root = checked_directory(&request.reports_root, "compatibility report directory")?;
    let preparation_roots = validate_preparation(&request.preparation)?;
    if current_dir == reports_root
        || current_dir == preparation_roots.source
        || current_dir == preparation_roots.engine
        || current_dir == preparation_roots.helpers
        || reports_root == preparation_roots.source
        || reports_root == preparation_roots.engine
        || reports_root == preparation_roots.helpers
    {
        return Err(ContractError::compatibility(
            "compatibility process, report, engine, and helper directories must remain separate",
        ));
    }
    let environment = environment::resolve(&request.environment)?;
    let environment_sha256 = environment::identity(&environment)?;
    let paths = ProbeReportPaths::new(&reports_root, request.phase, request.commands.len());
    paths.require_absent()?;
    let phase_started = Instant::now();
    let mut commands = Vec::with_capacity(request.commands.len());
    for (index, command) in request.commands.iter().enumerate() {
        validate_preparation(&request.preparation)?;
        validate_probe_directories(request, &current_dir, &reports_root)?;
        let command_environment = environment::resolve(&request.environment)?;
        let program = checked_executable(&command.program)?;
        let (program_sha256, _) = measure_executable(&program)?;
        let command_sha256 = command_identity(&program, &command.arguments)?;
        let command_paths = paths.command(index).ok_or_else(|| {
            ContractError::compatibility("compatibility command batch lost a required log path")
        })?;
        let mut process = Command::new(&program);
        process
            .env_clear()
            .envs(&command_environment.variables)
            .current_dir(&current_dir)
            .args(&command.arguments);
        let remaining_timeout = remaining_phase_timeout(phase_started, request.timeout)?;
        let output = run_output_with_input_and_control(
            &mut process,
            &[],
            PROBE_CAPTURE_LIMIT,
            remaining_timeout,
            cancellation,
        )
        .map_err(|_| {
            ContractError::compatibility("cannot start or supervise compatibility process")
        })?;
        validate_probe_directories(request, &current_dir, &reports_root)?;
        environment::resolve(&request.environment)?;
        let stdout_sha256 = ProbeReportPaths::write_log(&command_paths.stdout, &output.stdout)?;
        let stderr_sha256 = ProbeReportPaths::write_log(&command_paths.stderr, &output.stderr)?;
        let tool = if request.commands.len() == 1 {
            request.phase.file_stem().to_owned()
        } else {
            format!("{}-{}", request.phase.file_stem(), index + 1)
        };
        if output.cancelled || cancellation.is_cancelled() {
            return Err(ContractError::compatibility(
                "compatibility process was cancelled; retained logs require inspection",
            )
            .context(DiagnosticContext {
                tool: Some(tool),
                ..DiagnosticContext::default()
            }));
        }
        if output.timed_out {
            return Err(ContractError::compatibility(
                "compatibility process exceeded its explicit deadline; retained logs require inspection",
            )
            .context(DiagnosticContext {
                tool: Some(tool),
                timed_out: Some(true),
                timeout_ms: Some(duration_millis(request.timeout)),
                ..DiagnosticContext::default()
            }));
        }
        if !output.status.success() {
            return Err(ContractError::compatibility(
                "compatibility process exited unsuccessfully; retained logs require inspection",
            )
            .context(DiagnosticContext {
                tool: Some(tool),
                exit_code: output.status.code(),
                signal: exit_signal(output.status),
                ..DiagnosticContext::default()
            }));
        }
        validate_preparation(&request.preparation)?;
        validate_probe_directories(request, &current_dir, &reports_root)?;
        commands.push(CompatibilityCommandReport {
            program_sha256,
            command_sha256,
            stdout_sha256,
            stderr_sha256,
        });
    }
    validate_probe_directories(request, &current_dir, &reports_root)?;
    let report = CompatibilityProbeReport {
        schema: COMPATIBILITY_REPORT_SCHEMA.into(),
        phase: request.phase,
        engine_api_version: request.preparation.engine_api_version,
        engine_sha256: request.preparation.engine_sha256.clone(),
        source_tree_sha256: request.preparation.source_tree_sha256.clone(),
        helpers: request
            .preparation
            .helpers
            .iter()
            .map(|(name, helper)| {
                (
                    name.clone(),
                    CompatibilityHelperReport {
                        sha256: helper.sha256.clone(),
                        size: helper.size,
                    },
                )
            })
            .collect(),
        host_tools: environment.host_tools,
        environment_sha256,
        commands,
    };
    report.validate()?;
    let encoded = crate::canonical::bytes(
        &serde_json::to_value(&report)
            .map_err(|_| ContractError::compatibility("cannot encode the compatibility report"))?,
    )
    .map_err(|_| {
        ContractError::compatibility("cannot canonically encode the compatibility report")
    })?;
    let persisted = paths.write_report(&encoded)?;
    let parsed = CompatibilityProbeReport::parse(&persisted)?;
    if parsed != report {
        return Err(ContractError::compatibility(
            "persisted compatibility report differs from its in-memory identity",
        ));
    }
    Ok(report)
}

/// Run the complete closed M5.2 compatibility probe set.
///
/// The request must name each consumer-CMake, upstream configure/includes/
/// linklibs and standalone C/C++ phase exactly once. All phases must bind the
/// same revalidated engine/helper preparation. Phases execute in their fixed
/// dependency order. A failure retains every already-written log/report and
/// starts no later phase; the function never skips, retries, overwrites or
/// adopts a report.
///
/// # Errors
///
/// Returns AX0703 for an incomplete, duplicate or mixed request, or when a
/// child phase fails. It has no source, cache, network, tag or publication
/// authority.
pub fn run_probe_set(
    request: &CompatibilityProbeSetRequest,
    cancellation: &CancellationToken,
) -> Result<CompatibilityProbeSet, ContractError> {
    let mut probes = BTreeMap::new();
    for probe in &request.probes {
        if probes.insert(probe.phase, probe).is_some() {
            return Err(ContractError::compatibility(
                "compatibility probe set repeats a closed phase",
            ));
        }
    }
    let required = REQUIRED_PROBE_PHASES.into_iter().collect::<BTreeSet<_>>();
    if probes.keys().copied().collect::<BTreeSet<_>>() != required {
        return Err(ContractError::compatibility(
            "compatibility probe set does not contain every required phase exactly once",
        ));
    }
    let expected_preparation = probes
        .get(&CompatibilityPhase::CmakeConsumer)
        .ok_or_else(|| {
            ContractError::compatibility("compatibility probe set has no consumer CMake phase")
        })?
        .preparation
        .clone();
    if probes
        .values()
        .any(|probe| probe.preparation != expected_preparation)
    {
        return Err(ContractError::compatibility(
            "compatibility probe set mixes engine or helper preparation identities",
        ));
    }
    let mut reports = BTreeMap::new();
    for phase in REQUIRED_PROBE_PHASES {
        let probe = probes.get(&phase).ok_or_else(|| {
            ContractError::compatibility("compatibility probe set lost a required phase")
        })?;
        reports.insert(phase, run_probe(probe, cancellation)?);
    }
    Ok(CompatibilityProbeSet { reports })
}

#[derive(Debug)]
struct CompatibilityPreparationRoots {
    source: PathBuf,
    engine: PathBuf,
    helpers: PathBuf,
}

fn validate_probe_request(request: &CompatibilityProbeRequest) -> Result<(), ContractError> {
    if request.timeout.is_zero() {
        return Err(ContractError::compatibility(
            "compatibility process deadline must be positive",
        ));
    }
    if request.commands.is_empty() || request.commands.len() > MAX_PROBE_COMMANDS {
        return Err(ContractError::compatibility(
            "compatibility phase must contain one or two explicit commands",
        ));
    }
    for command in &request.commands {
        if command.arguments.len() > MAX_PROBE_ARGUMENTS {
            return Err(ContractError::compatibility(
                "compatibility command has more arguments than the configured limit",
            ));
        }
        let argument_bytes = command
            .arguments
            .iter()
            .try_fold(0_usize, |total, argument| {
                if argument.chars().any(char::is_control) {
                    return Err(ContractError::compatibility(
                        "compatibility process arguments cannot contain control characters",
                    ));
                }
                total.checked_add(argument.len()).ok_or_else(|| {
                    ContractError::compatibility("compatibility process argument length overflowed")
                })
            })?;
        if argument_bytes > MAX_PROBE_ARGUMENT_BYTES {
            return Err(ContractError::compatibility(
                "compatibility process arguments exceed the configured byte limit",
            ));
        }
    }
    environment::resolve(&request.environment)?;
    Ok(())
}

fn validate_preparation(
    preparation: &CompatibilityPreparation,
) -> Result<CompatibilityPreparationRoots, ContractError> {
    if preparation.engine_api_version != aros_cmake_engine::api_version() {
        return Err(ContractError::compatibility(
            "compatibility preparation uses an engine API different from this tools build",
        ));
    }
    let compiled_digest = Sha256Digest::parse(aros_cmake_engine::digest()).map_err(|_| {
        ContractError::compatibility("embedded CMake engine exposes an invalid compiled digest")
    })?;
    if preparation.engine_sha256 != compiled_digest {
        return Err(ContractError::compatibility(
            "compatibility preparation uses an engine digest different from this tools build",
        ));
    }
    let source_root = checked_directory(&preparation.source_root, "compatibility source root")?;
    reject_source_tree_engine(&source_root)?;
    let measured_source_tree_sha256 = measure_tree_content_cas(&source_root)
        .map_err(|_| {
            ContractError::compatibility(
                "cannot remeasure the engine-free compatibility source tree without following links",
            )
        })?
        .payload_digest_excluding(None);
    if measured_source_tree_sha256 != preparation.source_tree_sha256 {
        return Err(ContractError::compatibility(
            "compatibility source tree changed after preparation",
        ));
    }

    let engine_root = checked_directory(&preparation.engine_root, "compatibility engine root")?;
    verify_materialized_engine(&engine_root, &preparation.engine_sha256)?;

    let expected = REQUIRED_HELPERS
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    if preparation.helpers.keys().cloned().collect::<BTreeSet<_>>() != expected {
        return Err(ContractError::compatibility(
            "compatibility preparation does not provide the exact helper set",
        ));
    }
    let mut helpers_root = None;
    for (name, helper) in &preparation.helpers {
        if helper.path.file_name().and_then(|value| value.to_str()) != Some(name) {
            return Err(ContractError::compatibility(
                "compatibility helper path does not match its fixed helper name",
            ));
        }
        let parent = helper.path.parent().ok_or_else(|| {
            ContractError::compatibility("compatibility helper path has no target-root parent")
        })?;
        let parent = checked_directory(parent, "compatibility helper root")?;
        if let Some(previous) = &helpers_root {
            if previous != &parent {
                return Err(ContractError::compatibility(
                    "compatibility helpers do not resolve from one exact target root",
                ));
            }
        } else {
            helpers_root = Some(parent);
        }
        let expected_root = helpers_root.as_deref().ok_or_else(|| {
            ContractError::compatibility("compatibility helper root was not initialized")
        })?;
        let path = checked_executable(&helper.path)?;
        if path.parent() != Some(expected_root) {
            return Err(ContractError::compatibility(
                "compatibility helper resolves outside its exact target root",
            ));
        }
        let measured = measure_executable(&path)?;
        if measured != (helper.sha256.clone(), helper.size) {
            return Err(ContractError::compatibility(
                "compatibility helper changed after preparation",
            ));
        }
    }
    let helpers_root = helpers_root.ok_or_else(|| {
        ContractError::compatibility("compatibility preparation does not provide a helper root")
    })?;
    Ok(CompatibilityPreparationRoots {
        source: source_root,
        engine: engine_root,
        helpers: helpers_root,
    })
}

fn checked_executable(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::compatibility(
            "compatibility process program must be an absolute executable path",
        ));
    }
    let canonical = path.canonicalize().map_err(|_| {
        ContractError::compatibility("compatibility process program cannot be canonicalized")
    })?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| {
        ContractError::compatibility("cannot inspect the compatibility process program")
    })?;
    let executable = metadata.permissions().mode() & 0o111 != 0;
    if !metadata.is_file() || metadata.file_type().is_symlink() || !executable {
        return Err(ContractError::compatibility(
            "compatibility process program is not a regular executable",
        ));
    }
    let file = open_regular_file_nofollow(&canonical).map_err(|_| {
        ContractError::compatibility("cannot safely open the compatibility process program")
    })?;
    let opened = file.metadata().map_err(|_| {
        ContractError::compatibility("cannot inspect the opened compatibility process program")
    })?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err(ContractError::compatibility(
            "compatibility process program changed while it was validated",
        ));
    }
    Ok(canonical)
}

fn measure_executable(path: &Path) -> Result<(Sha256Digest, u64), ContractError> {
    let mut file = open_regular_file_nofollow(path).map_err(|_| {
        ContractError::compatibility("cannot safely open the compatibility process program")
    })?;
    let metadata = file.metadata().map_err(|_| {
        ContractError::compatibility("cannot inspect the opened compatibility process program")
    })?;
    let measured = sha256_reader(&mut file.by_ref()).map_err(|_| {
        ContractError::compatibility("cannot measure the compatibility process program")
    })?;
    if !metadata.is_file() || measured.size == 0 || measured.size != metadata.len() {
        return Err(ContractError::compatibility(
            "compatibility process program changed while it was measured",
        ));
    }
    Ok((measured.digest, measured.size))
}

fn command_identity(program: &Path, arguments: &[String]) -> Result<Sha256Digest, ContractError> {
    let encoded = crate::canonical::bytes(&serde_json::json!({
        "arguments": arguments,
        "program": program.to_string_lossy(),
    }))
    .map_err(|_| {
        ContractError::compatibility("cannot canonically encode the compatibility command")
    })?;
    Ok(sha256_bytes(&encoded))
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn remaining_phase_timeout(started: Instant, timeout: Duration) -> Result<Duration, ContractError> {
    timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            ContractError::compatibility(
                "compatibility phase exhausted its explicit deadline before starting the next command",
            )
        })
}

fn validate_probe_directories(
    request: &CompatibilityProbeRequest,
    expected_current_dir: &Path,
    expected_reports_root: &Path,
) -> Result<(), ContractError> {
    let current_dir = checked_directory(&request.current_dir, "compatibility process directory")?;
    let reports_root = checked_directory(&request.reports_root, "compatibility report directory")?;
    if current_dir != expected_current_dir || reports_root != expected_reports_root {
        return Err(ContractError::compatibility(
            "compatibility process or report directory changed after probe validation",
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct ProbeReportPaths {
    commands: Vec<ProbeCommandPaths>,
    report: PathBuf,
    all_outputs: Vec<PathBuf>,
}

#[derive(Debug)]
struct ProbeCommandPaths {
    stdout: PathBuf,
    stderr: PathBuf,
}

impl ProbeReportPaths {
    fn new(root: &Path, phase: CompatibilityPhase, command_count: usize) -> Self {
        let stem = phase.file_stem();
        let report = root.join(format!("{stem}.report.json"));
        let mut all_outputs = vec![report.clone()];
        for name in std::iter::once(stem.to_owned())
            .chain((1..=MAX_PROBE_COMMANDS).map(|index| format!("{stem}.{index}")))
        {
            all_outputs.extend([
                root.join(format!("{name}.stdout.log")),
                root.join(format!("{name}.stderr.log")),
            ]);
        }
        Self {
            commands: (0..command_count)
                .map(|index| {
                    let name = if command_count == 1 {
                        stem.to_owned()
                    } else {
                        format!("{stem}.{}", index + 1)
                    };
                    ProbeCommandPaths {
                        stdout: root.join(format!("{name}.stdout.log")),
                        stderr: root.join(format!("{name}.stderr.log")),
                    }
                })
                .collect(),
            report,
            all_outputs,
        }
    }

    fn require_absent(&self) -> Result<(), ContractError> {
        for path in &self.all_outputs {
            match fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(ContractError::compatibility(
                        "compatibility report output already exists and cannot be adopted",
                    ))
                }
                Err(_) => {
                    return Err(ContractError::compatibility(
                        "cannot safely inspect a compatibility report output path",
                    ))
                }
            }
        }
        Ok(())
    }

    fn command(&self, index: usize) -> Option<&ProbeCommandPaths> {
        self.commands.get(index)
    }

    fn write_log(
        path: &Path,
        stream: &aros_common::CapturedStream,
    ) -> Result<Sha256Digest, ContractError> {
        let mut rendered = Vec::new();
        stream.write_rendered(&mut rendered).map_err(|_| {
            ContractError::compatibility("cannot render a compatibility process log")
        })?;
        write_new_regular(path, &rendered, "compatibility process log")?;
        let persisted = read_regular_bounded(
            path,
            "persisted compatibility process log",
            MAX_RENDERED_LOG_BYTES,
        )?;
        if persisted != rendered {
            return Err(ContractError::compatibility(
                "persisted compatibility process log bytes changed after publication",
            ));
        }
        Ok(sha256_bytes(&persisted))
    }

    fn write_report(&self, encoded: &[u8]) -> Result<Vec<u8>, ContractError> {
        write_new_regular(&self.report, encoded, "compatibility report")?;
        let persisted = read_regular_bounded(
            &self.report,
            "persisted compatibility report",
            crate::canonical::MAX_DOCUMENT_BYTES,
        )?;
        if persisted != encoded {
            return Err(ContractError::compatibility(
                "persisted compatibility report bytes changed after publication",
            ));
        }
        Ok(persisted)
    }
}

fn write_new_regular(path: &Path, contents: &[u8], label: &str) -> Result<(), ContractError> {
    publish_atomic_file(path, contents, AtomicFilePolicy::NoClobber)
        .map(|_| ())
        .map_err(|_| ContractError::compatibility(format!("cannot durably create {label}")))
}

fn read_regular_bounded(path: &Path, label: &str, limit: usize) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::compatibility(format!("cannot safely open {label}")))?;
    let metadata = file
        .metadata()
        .map_err(|_| ContractError::compatibility(format!("cannot inspect {label}")))?;
    let limit = u64::try_from(limit)
        .map_err(|_| ContractError::compatibility(format!("{label} limit is not representable")))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(ContractError::compatibility(format!(
            "{label} is not a regular file within its configured limit"
        )));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        ContractError::compatibility(format!(
            "{label} is too large for this process address space"
        ))
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ContractError::compatibility(format!("cannot read {label}")))?;
    if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
        return Err(ContractError::compatibility(format!(
            "{label} changed while it was read"
        )));
    }
    Ok(bytes)
}

fn checked_directory(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::compatibility(format!(
            "{label} must be an absolute directory"
        )));
    }
    let canonical = path.canonicalize().map_err(|_| {
        ContractError::compatibility(format!("{label} cannot be canonicalized after validation"))
    })?;
    if open_directory(&canonical).is_err() {
        return Err(ContractError::compatibility(format!(
            "{label} does not resolve to a real directory without symlink ancestors"
        )));
    }
    Ok(canonical)
}

fn reject_source_tree_engine(source_root: &Path) -> Result<(), ContractError> {
    let source_engine = source_root.join("cmake");
    if fs::symlink_metadata(&source_engine).is_ok() {
        return Err(ContractError::compatibility(
            "compatibility source tree must not contain a copied CMake engine",
        ));
    }
    Ok(())
}

fn verify_materialized_engine(
    engine_root: &Path,
    expected_digest: &Sha256Digest,
) -> Result<(), ContractError> {
    let actual_digest = Sha256Digest::parse(aros_cmake_engine::digest()).map_err(|_| {
        ContractError::compatibility("embedded CMake engine exposes an invalid compiled digest")
    })?;
    if &actual_digest != expected_digest {
        return Err(ContractError::compatibility(
            "embedded CMake engine digest changed during compatibility preparation",
        ));
    }
    let mut expected = BTreeSet::new();
    for relative in aros_cmake_engine::paths() {
        let path = engine_root.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            ContractError::compatibility("materialized CMake engine is missing an embedded file")
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ContractError::compatibility(
                "materialized CMake engine contains a non-regular embedded file",
            ));
        }
        let expected_contents = aros_cmake_engine::file(relative).ok_or_else(|| {
            ContractError::compatibility("embedded CMake engine file table changed unexpectedly")
        })?;
        let expected_len = u64::try_from(expected_contents.len()).map_err(|_| {
            ContractError::compatibility("embedded CMake engine file length is not representable")
        })?;
        if metadata.len() != expected_len {
            return Err(ContractError::compatibility(
                "materialized CMake engine file length differs from the embedded tools resource",
            ));
        }
        let contents = read_regular_bounded(
            &path,
            "materialized CMake engine file",
            MAX_ENGINE_FILE_BYTES,
        )?;
        if contents != expected_contents.as_bytes() {
            return Err(ContractError::compatibility(
                "materialized CMake engine file differs from the embedded tools resource",
            ));
        }
        expected.insert(relative.to_owned());
    }
    expected.insert(aros_cmake_engine::STAMP_FILE.to_owned());
    let actual = collect_regular_relative_paths(
        engine_root,
        Path::new(""),
        &mut EngineInventoryBudget::default(),
    )?;
    if actual != expected {
        return Err(ContractError::compatibility(
            "materialized CMake engine contains missing, foreign, or unsafe files",
        ));
    }
    let stamp = String::from_utf8(read_regular_bounded(
        &engine_root.join(aros_cmake_engine::STAMP_FILE),
        "materialized CMake engine stamp",
        MAX_ENGINE_FILE_BYTES,
    )?)
    .map_err(|_| ContractError::compatibility("materialized CMake engine stamp is not UTF-8"))?;
    if stamp != format!("{}\n", expected_digest.as_str()) {
        return Err(ContractError::compatibility(
            "materialized CMake engine stamp does not bind the embedded digest",
        ));
    }
    Ok(())
}

fn collect_regular_relative_paths(
    root: &Path,
    relative: &Path,
    budget: &mut EngineInventoryBudget,
) -> Result<BTreeSet<String>, ContractError> {
    if relative.components().count() > MAX_ENGINE_DEPTH {
        return Err(ContractError::compatibility(
            "materialized CMake engine exceeds the configured directory-depth limit",
        ));
    }
    let directory = root.join(relative);
    let mut paths = BTreeSet::new();
    for entry in fs::read_dir(&directory).map_err(|_| {
        ContractError::compatibility("cannot enumerate the materialized CMake engine")
    })? {
        let entry = entry.map_err(|_| {
            ContractError::compatibility("cannot read a materialized CMake engine directory entry")
        })?;
        budget.account()?;
        let name = entry.file_name().into_string().map_err(|_| {
            ContractError::compatibility("materialized CMake engine contains a non-UTF-8 path")
        })?;
        let next_relative = relative.join(&name);
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
            ContractError::compatibility("cannot inspect a materialized CMake engine entry")
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ContractError::compatibility(
                "materialized CMake engine contains a symbolic link",
            ));
        }
        if metadata.is_dir() {
            paths.extend(collect_regular_relative_paths(
                root,
                &next_relative,
                budget,
            )?);
        } else if metadata.is_file() {
            let relative = next_relative.to_str().ok_or_else(|| {
                ContractError::compatibility("materialized CMake engine path is not UTF-8")
            })?;
            paths.insert(relative.to_owned());
        } else {
            return Err(ContractError::compatibility(
                "materialized CMake engine contains a non-regular entry",
            ));
        }
    }
    Ok(paths)
}

#[derive(Default)]
struct EngineInventoryBudget {
    entries: usize,
}

impl EngineInventoryBudget {
    fn account(&mut self) -> Result<(), ContractError> {
        self.entries = self.entries.checked_add(1).ok_or_else(|| {
            ContractError::compatibility("materialized CMake engine entry count overflowed")
        })?;
        if self.entries > MAX_ENGINE_ENTRIES {
            return Err(ContractError::compatibility(
                "materialized CMake engine exceeds the configured entry limit",
            ));
        }
        Ok(())
    }
}

fn resolve_helpers(root: &Path) -> Result<BTreeMap<String, HelperIdentity>, ContractError> {
    let mut helpers = BTreeMap::new();
    for name in REQUIRED_HELPERS {
        let path = root.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|_| {
            ContractError::compatibility(
                "compatibility helper is missing from the exact target root",
            )
        })?;
        let executable = metadata.permissions().mode() & 0o111 != 0;
        if !metadata.is_file() || metadata.file_type().is_symlink() || !executable {
            return Err(ContractError::compatibility(
                "compatibility helper is not a regular executable from the exact target root",
            ));
        }
        let mut file = open_regular_file_nofollow(&path).map_err(|_| {
            ContractError::compatibility(
                "cannot safely open a compatibility helper from the target root",
            )
        })?;
        let opened_metadata = file.metadata().map_err(|_| {
            ContractError::compatibility("cannot inspect an opened compatibility helper")
        })?;
        if !opened_metadata.is_file() || opened_metadata.len() != metadata.len() {
            return Err(ContractError::compatibility(
                "compatibility helper changed while it was opened for measurement",
            ));
        }
        let measured = sha256_reader(&mut file.by_ref()).map_err(|_| {
            ContractError::compatibility(
                "cannot measure a compatibility helper from the target root",
            )
        })?;
        if measured.size != opened_metadata.len() {
            return Err(ContractError::compatibility(
                "compatibility helper changed while it was measured",
            ));
        }
        if measured.size == 0 {
            return Err(ContractError::compatibility(
                "compatibility helper from the exact target root is empty",
            ));
        }
        helpers.insert(
            (*name).to_owned(),
            HelperIdentity {
                path,
                sha256: measured.digest,
                size: measured.size,
            },
        );
    }
    Ok(helpers)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt as _};
    use std::time::Duration;

    use aros_common::{
        elf::{AROS_ABI_VERSION, OS_ABI_AROS},
        CancellationToken, DiagnosticCode,
    };

    use super::{
        prepare, prepare_host_tool_closure, run_probe, run_probe_set, verify_materialized_engine,
        verify_standalone_outputs, CompatibilityCommand, CompatibilityEnvironment,
        CompatibilityHostTool, CompatibilityPhase, CompatibilityPreparation,
        CompatibilityPreparationRequest, CompatibilityProbeReport, CompatibilityProbeRequest,
        CompatibilityProbeSetRequest, HostToolClosureRequest, StandaloneOutputRequest,
        StandaloneTargetArtifacts, CXX_COLLECTOR_SYMBOL, C_COLLECTOR_SYMBOL, REQUIRED_HELPERS,
    };

    fn request(root: &std::path::Path) -> CompatibilityPreparationRequest {
        let source_root = root.join("source");
        let work_root = root.join("work");
        let helpers_root = root.join("helpers");
        for directory in [&source_root, &work_root, &helpers_root] {
            fs::create_dir(directory).unwrap();
        }
        for helper in REQUIRED_HELPERS {
            let path = helpers_root.join(helper);
            fs::write(&path, b"fixture helper\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        CompatibilityPreparationRequest {
            source_root,
            work_root,
            helpers_root,
        }
    }

    #[test]
    fn standalone_outputs_require_aros_elf_identity_and_collector_symbols() {
        let temporary = tempfile::tempdir().unwrap();
        let output_root = temporary.path().join("standalone");
        fs::create_dir(&output_root).unwrap();
        let c = output_root.join("c-x86_64.o");
        let cxx = output_root.join("cxx-x86_64.o");
        fs::write(&c, fixture_elf64(C_COLLECTOR_SYMBOL, OS_ABI_AROS)).unwrap();
        fs::write(&cxx, fixture_elf64(CXX_COLLECTOR_SYMBOL, OS_ABI_AROS)).unwrap();

        let report = verify_standalone_outputs(&StandaloneOutputRequest {
            output_root: output_root.clone(),
            targets: BTreeMap::from([(
                "x86_64-unknown-aros".into(),
                StandaloneTargetArtifacts {
                    c: c.clone(),
                    cxx: cxx.clone(),
                },
            )]),
        })
        .unwrap();
        let target = &report.targets["x86_64-unknown-aros"];
        assert_eq!(target.c.class, aros_common::elf::Class::Elf64);
        assert_eq!(target.cxx.class, aros_common::elf::Class::Elf64);
        assert_ne!(target.c.sha256, target.cxx.sha256);

        let linked = output_root.join("c-linked.o");
        symlink(&c, &linked).unwrap();
        let linked_error = verify_standalone_outputs(&StandaloneOutputRequest {
            output_root: output_root.clone(),
            targets: BTreeMap::from([(
                "x86_64-unknown-aros".into(),
                StandaloneTargetArtifacts {
                    c: linked,
                    cxx: cxx.clone(),
                },
            )]),
        })
        .unwrap_err();
        assert_compatibility(&linked_error);

        let duplicate = verify_standalone_outputs(&StandaloneOutputRequest {
            output_root: output_root.clone(),
            targets: BTreeMap::from([(
                "x86_64-unknown-aros".into(),
                StandaloneTargetArtifacts {
                    c: c.clone(),
                    cxx: c,
                },
            )]),
        })
        .unwrap_err();
        assert_compatibility(&duplicate);

        let non_aros = output_root.join("c-non-aros.o");
        fs::write(&non_aros, fixture_elf64(C_COLLECTOR_SYMBOL, 0)).unwrap();
        let non_aros_error = verify_standalone_outputs(&StandaloneOutputRequest {
            output_root,
            targets: BTreeMap::from([(
                "x86_64-unknown-aros".into(),
                StandaloneTargetArtifacts { c: non_aros, cxx },
            )]),
        })
        .unwrap_err();
        assert_compatibility(&non_aros_error);
    }

    fn fixture_elf64(symbol: &str, os_abi: u8) -> Vec<u8> {
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
        object[7] = os_abi;
        object[8] = AROS_ABI_VERSION;
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

    fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
        buffer[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
        buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
        buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn preparation_materializes_only_the_embedded_engine_and_exact_helpers() {
        let temporary = tempfile::tempdir().unwrap();
        let request = request(temporary.path());
        let prepared = prepare(&request).unwrap();

        assert!(prepared.engine_root.join("CMakeLists.txt").is_file());
        assert!(prepared.engine_root.join("AROS.cmake").is_file());
        assert_eq!(prepared.helpers.len(), REQUIRED_HELPERS.len());
        let helpers_root = request.helpers_root.canonicalize().unwrap();
        assert!(prepared
            .helpers
            .values()
            .all(|helper| helper.path.starts_with(&helpers_root)));
        assert!(!request.source_root.join("cmake").exists());
    }

    #[test]
    fn preparation_rejects_source_engine_reused_destination_and_bad_helper() {
        let temporary = tempfile::tempdir().unwrap();
        let request = request(temporary.path());
        fs::create_dir(request.source_root.join("cmake")).unwrap();
        let source_engine_error = prepare(&request).unwrap_err();
        assert_compatibility(&source_engine_error);
        fs::remove_dir(request.source_root.join("cmake")).unwrap();

        fs::create_dir(request.work_root.join("aros-cmake-engine")).unwrap();
        let reused_engine_error = prepare(&request).unwrap_err();
        assert_compatibility(&reused_engine_error);
        fs::remove_dir(request.work_root.join("aros-cmake-engine")).unwrap();

        let helper = request.helpers_root.join("aros-fetch");
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o600)).unwrap();
        let error = prepare(&request).unwrap_err();
        assert_compatibility(&error);
    }

    #[test]
    fn materialized_engine_rejects_foreign_or_linked_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let engine = temporary.path().join("engine");
        fs::create_dir(&engine).unwrap();
        aros_cmake_engine::materialize(&engine).unwrap();
        let digest = aros_common::Sha256Digest::parse(aros_cmake_engine::digest()).unwrap();
        fs::write(engine.join("foreign.cmake"), b"unexpected\n").unwrap();
        let error = verify_materialized_engine(&engine, &digest).unwrap_err();
        assert_compatibility(&error);
    }

    #[test]
    fn probe_persists_a_canonical_report_bound_to_the_preparation() {
        let temporary = tempfile::tempdir().unwrap();
        let first_request = request(temporary.path());
        let preparation = prepare(&first_request).unwrap();
        let program = script(
            temporary.path(),
            "successful-probe",
            "[ \"$PATH\" = /nonexistent ] && [ -z \"${HOME+x}\" ] || exit 9; printf standard; printf error >&2",
        );
        let probe = probe_request(
            temporary.path(),
            preparation.clone(),
            CompatibilityPhase::CmakeConsumer,
            program,
            Duration::from_secs(1),
        );

        let report = run_probe(&probe, &CancellationToken::default()).unwrap();
        assert_eq!(report.engine_sha256, preparation.engine_sha256);
        assert_eq!(report.source_tree_sha256, preparation.source_tree_sha256);
        assert_eq!(report.helpers.len(), REQUIRED_HELPERS.len());
        let bytes = fs::read(probe.reports_root.join("cmake-consumer.report.json")).unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert_eq!(CompatibilityProbeReport::parse(&bytes).unwrap(), report);
        assert_eq!(
            fs::read(probe.reports_root.join("cmake-consumer.stdout.log")).unwrap(),
            b"standard"
        );
        assert_eq!(
            fs::read(probe.reports_root.join("cmake-consumer.stderr.log")).unwrap(),
            b"error"
        );

        let rerun = run_probe(&probe, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&rerun);
    }

    #[test]
    fn probe_executes_a_revalidated_sealed_host_tool_closure() {
        let temporary = tempfile::tempdir().unwrap();
        let preparation_request = request(temporary.path());
        let preparation = prepare(&preparation_request).unwrap();
        let selected_tool = script(temporary.path(), "selected-tool-source", "printf closure");
        let closure = prepare_host_tool_closure(&HostToolClosureRequest {
            output_root: temporary.path().join("host-tool-closure"),
            tools: vec![CompatibilityHostTool {
                name: "selected-tool".into(),
                program: selected_tool,
            }],
        })
        .unwrap();
        let mut probe = probe_request(
            temporary.path(),
            preparation,
            CompatibilityPhase::UpstreamConfigure,
            script(
                temporary.path(),
                "sealed-host-tool-probe",
                "[ \"$PATH\" = \"$1\" ] && [ -z \"${HOME+x}\" ] || exit 9; selected-tool",
            ),
            Duration::from_secs(5),
        );
        probe.commands[0]
            .arguments
            .push(closure.root.to_string_lossy().into_owned());
        probe.environment = CompatibilityEnvironment::SealedHostTools {
            variables: BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
            host_tools: closure.clone(),
        };

        let mut missing_marker = probe.clone();
        missing_marker.reports_root = temporary.path().join("missing-host-tool-marker-reports");
        fs::create_dir(&missing_marker.reports_root).unwrap();
        missing_marker.environment = CompatibilityEnvironment::SealedHostTools {
            variables: BTreeMap::new(),
            host_tools: closure.clone(),
        };
        assert_compatibility(
            &run_probe(&missing_marker, &CancellationToken::default()).unwrap_err(),
        );

        let report = run_probe(&probe, &CancellationToken::default()).unwrap();
        assert_eq!(
            fs::read(probe.reports_root.join("upstream-configure.stdout.log")).unwrap(),
            b"closure"
        );
        assert_eq!(report.host_tools.len(), 1);
        assert_eq!(
            report.host_tools["selected-tool"].sha256,
            closure.tools["selected-tool"].sha256
        );
    }

    #[test]
    fn probe_batch_binds_each_command_and_retains_failed_command_logs() {
        let temporary = tempfile::tempdir().unwrap();
        let preparation_request = request(temporary.path());
        let preparation = prepare(&preparation_request).unwrap();
        let mut successful = probe_request(
            temporary.path(),
            preparation,
            CompatibilityPhase::StandaloneC,
            script(temporary.path(), "batch-first", "printf first"),
            Duration::from_secs(5),
        );
        successful.commands.push(CompatibilityCommand {
            program: script(temporary.path(), "batch-second", "printf second >&2"),
            arguments: Vec::new(),
        });
        let report = run_probe(&successful, &CancellationToken::default()).unwrap();
        assert_eq!(report.commands.len(), 2);
        assert!(successful
            .reports_root
            .join("standalone-c.1.stdout.log")
            .is_file());
        assert!(successful
            .reports_root
            .join("standalone-c.2.stderr.log")
            .is_file());

        let failing_root = tempfile::tempdir().unwrap();
        let failing_request = request(failing_root.path());
        let mut failing = probe_request(
            failing_root.path(),
            prepare(&failing_request).unwrap(),
            CompatibilityPhase::StandaloneCxx,
            script(failing_root.path(), "batch-success", "printf first"),
            Duration::from_secs(1),
        );
        failing.commands.push(CompatibilityCommand {
            program: script(
                failing_root.path(),
                "batch-failure",
                "printf second >&2; exit 7",
            ),
            arguments: Vec::new(),
        });
        let error = run_probe(&failing, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);
        assert!(failing
            .reports_root
            .join("standalone-cxx.1.stdout.log")
            .is_file());
        assert!(failing
            .reports_root
            .join("standalone-cxx.2.stderr.log")
            .is_file());
        assert!(!failing
            .reports_root
            .join("standalone-cxx.report.json")
            .exists());
        let stale_retry = probe_request(
            failing_root.path(),
            failing.preparation.clone(),
            CompatibilityPhase::StandaloneCxx,
            script(failing_root.path(), "single-retry", "exit 0"),
            Duration::from_secs(1),
        );
        let error = run_probe(&stale_retry, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);

        let deadline_root = tempfile::tempdir().unwrap();
        let deadline_request = request(deadline_root.path());
        let mut deadline = probe_request(
            deadline_root.path(),
            prepare(&deadline_request).unwrap(),
            CompatibilityPhase::StandaloneC,
            script(
                deadline_root.path(),
                "deadline-first",
                "exec /bin/sleep 0.5",
            ),
            Duration::from_secs(2),
        );
        deadline.commands.push(CompatibilityCommand {
            program: script(deadline_root.path(), "deadline-second", "exec /bin/sleep 2"),
            arguments: Vec::new(),
        });
        let error = run_probe(&deadline, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);
        assert!(deadline
            .reports_root
            .join("standalone-c.2.stdout.log")
            .is_file());

        let changed_root = tempfile::tempdir().unwrap();
        let changed_request = request(changed_root.path());
        let changed_preparation = prepare(&changed_request).unwrap();
        let changed_helper = changed_preparation.helpers["aros-fetch"]
            .path
            .to_string_lossy()
            .into_owned();
        let mut changed = probe_request(
            changed_root.path(),
            changed_preparation,
            CompatibilityPhase::StandaloneC,
            script(
                changed_root.path(),
                "batch-mutator",
                "printf changed > \"$1\"",
            ),
            Duration::from_secs(1),
        );
        changed.commands[0].arguments.push(changed_helper);
        changed.commands.push(CompatibilityCommand {
            program: script(changed_root.path(), "batch-after-mutation", "exit 0"),
            arguments: Vec::new(),
        });
        let error = run_probe(&changed, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);
        assert!(changed
            .reports_root
            .join("standalone-c.1.stdout.log")
            .is_file());
        assert!(!changed
            .reports_root
            .join("standalone-c.2.stdout.log")
            .exists());
        assert!(!changed
            .reports_root
            .join("standalone-c.report.json")
            .exists());

        let moved_root = tempfile::tempdir().unwrap();
        let moved_request = request(moved_root.path());
        let mut moved = probe_request(
            moved_root.path(),
            prepare(&moved_request).unwrap(),
            CompatibilityPhase::StandaloneC,
            script(
                moved_root.path(),
                "move-working-directory",
                "exec /bin/mv \"$1\" \"$1-moved\"",
            ),
            Duration::from_secs(1),
        );
        moved.commands[0]
            .arguments
            .push(moved.current_dir.to_string_lossy().into_owned());
        moved.commands.push(CompatibilityCommand {
            program: script(moved_root.path(), "after-working-directory-move", "exit 0"),
            arguments: Vec::new(),
        });
        let error = run_probe(&moved, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);
        assert!(!moved
            .reports_root
            .join("standalone-c.2.stdout.log")
            .exists());
    }

    #[test]
    fn probe_preserves_diagnostics_for_exit_timeout_and_cancellation() {
        let temporary = tempfile::tempdir().unwrap();
        let request = request(temporary.path());
        let preparation = prepare(&request).unwrap();
        let failing = probe_request(
            temporary.path(),
            preparation.clone(),
            CompatibilityPhase::CmakeConsumer,
            script(
                temporary.path(),
                "failing-probe",
                "printf failure >&2; exit 7",
            ),
            Duration::from_secs(1),
        );
        let error = run_probe(&failing, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);
        assert!(failing
            .reports_root
            .join("cmake-consumer.stderr.log")
            .is_file());
        assert!(!failing
            .reports_root
            .join("cmake-consumer.report.json")
            .exists());

        let timed = probe_request(
            temporary.path(),
            preparation.clone(),
            CompatibilityPhase::UpstreamConfigure,
            script(
                temporary.path(),
                "timed-probe",
                "printf started; exec /bin/sleep 30",
            ),
            Duration::from_secs(5),
        );
        let error = run_probe(&timed, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);
        assert_eq!(
            fs::read(timed.reports_root.join("upstream-configure.stdout.log")).unwrap(),
            b"started"
        );
        assert!(!timed
            .reports_root
            .join("upstream-configure.report.json")
            .exists());

        let cancelled = probe_request(
            temporary.path(),
            preparation,
            CompatibilityPhase::StandaloneC,
            script(temporary.path(), "cancelled-probe", "exit 0"),
            Duration::from_secs(1),
        );
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let error = run_probe(&cancelled, &cancellation).unwrap_err();
        assert_compatibility(&error);
        assert!(!cancelled
            .reports_root
            .join("standalone-c.report.json")
            .exists());
    }

    #[test]
    fn probe_rejects_changed_helper_control_arguments_and_foreign_report_fields() {
        let temporary = tempfile::tempdir().unwrap();
        let first_request = request(temporary.path());
        let preparation = prepare(&first_request).unwrap();
        fs::write(
            preparation.helpers["aros-fetch"].path.clone(),
            b"changed helper\n",
        )
        .unwrap();
        let changed = probe_request(
            temporary.path(),
            preparation,
            CompatibilityPhase::CmakeConsumer,
            script(temporary.path(), "changed-helper-probe", "exit 0"),
            Duration::from_secs(1),
        );
        let error = run_probe(&changed, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);

        let source_changed_root = temporary.path().join("source-changed");
        fs::create_dir(&source_changed_root).unwrap();
        let source_changed_request = request(&source_changed_root);
        let source_changed_preparation = prepare(&source_changed_request).unwrap();
        fs::write(
            source_changed_preparation
                .source_root
                .join("unexpected-source-change"),
            b"changed source\n",
        )
        .unwrap();
        let changed_source = probe_request(
            &source_changed_root,
            source_changed_preparation,
            CompatibilityPhase::CmakeConsumer,
            script(temporary.path(), "changed-source-probe", "exit 0"),
            Duration::from_secs(1),
        );
        let error = run_probe(&changed_source, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);

        let separate = temporary.path().join("separate");
        fs::create_dir(&separate).unwrap();
        let second_request = request(&separate);
        let preparation = prepare(&second_request).unwrap();
        let mut invalid = probe_request(
            &separate,
            preparation,
            CompatibilityPhase::CmakeConsumer,
            script(temporary.path(), "invalid-argument-probe", "exit 0"),
            Duration::from_secs(1),
        );
        invalid.commands[0].arguments.push("line\nbreak".into());
        let error = run_probe(&invalid, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);

        let third = temporary.path().join("third");
        fs::create_dir(&third).unwrap();
        let third_request = request(&third);
        let mut invalid_environment = probe_request(
            &third,
            prepare(&third_request).unwrap(),
            CompatibilityPhase::StandaloneCxx,
            script(temporary.path(), "invalid-environment-probe", "exit 0"),
            Duration::from_secs(1),
        );
        let CompatibilityEnvironment::Poisoned { variables } = &mut invalid_environment.environment
        else {
            panic!("fixture must use a poisoned compatibility environment");
        };
        variables.insert("PATH".into(), "/bin".into());
        let error = run_probe(&invalid_environment, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);

        let malformed =
            br#"{\"schema\":\"aros-toolchain-compatibility-report-v3\",\"unexpected\":true}"#;
        let error = CompatibilityProbeReport::parse(malformed).unwrap_err();
        assert_compatibility(&error);
    }

    #[test]
    fn probe_set_requires_every_phase_once_and_stops_after_a_failure() {
        let temporary = tempfile::tempdir().unwrap();
        let preparation_request = request(temporary.path());
        let preparation = prepare(&preparation_request).unwrap();
        let successful = complete_probe_requests(temporary.path(), &preparation, None);
        let completed = run_probe_set(
            &CompatibilityProbeSetRequest {
                probes: successful.clone(),
            },
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(completed.reports.len(), 6);
        assert!(completed
            .reports
            .contains_key(&CompatibilityPhase::StandaloneCxx));

        let incomplete = CompatibilityProbeSetRequest {
            probes: successful[..5].to_vec(),
        };
        let error = run_probe_set(&incomplete, &CancellationToken::default()).unwrap_err();
        assert_compatibility(&error);

        let mut repeated = successful.clone();
        repeated.push(successful[0].clone());
        let error = run_probe_set(
            &CompatibilityProbeSetRequest { probes: repeated },
            &CancellationToken::default(),
        )
        .unwrap_err();
        assert_compatibility(&error);

        let failed_root = tempfile::tempdir().unwrap();
        let failed_request = request(failed_root.path());
        let failed = complete_probe_requests(
            failed_root.path(),
            &prepare(&failed_request).unwrap(),
            Some(CompatibilityPhase::UpstreamIncludes),
        );
        let error = run_probe_set(
            &CompatibilityProbeSetRequest { probes: failed },
            &CancellationToken::default(),
        )
        .unwrap_err();
        assert_compatibility(&error);
        let reports = failed_root.path().join("reports");
        assert!(reports.join("cmake-consumer.report.json").is_file());
        assert!(reports.join("upstream-configure.report.json").is_file());
        assert!(reports.join("upstream-includes.stderr.log").is_file());
        assert!(!reports.join("upstream-includes.report.json").exists());
        assert!(!reports.join("upstream-linklibs.report.json").exists());
    }

    fn complete_probe_requests(
        root: &std::path::Path,
        preparation: &CompatibilityPreparation,
        failing: Option<CompatibilityPhase>,
    ) -> Vec<CompatibilityProbeRequest> {
        [
            CompatibilityPhase::CmakeConsumer,
            CompatibilityPhase::UpstreamConfigure,
            CompatibilityPhase::UpstreamIncludes,
            CompatibilityPhase::UpstreamLinklibs,
            CompatibilityPhase::StandaloneC,
            CompatibilityPhase::StandaloneCxx,
        ]
        .into_iter()
        .map(|phase| {
            let body = if Some(phase) == failing {
                "printf failure >&2; exit 7"
            } else {
                "[ \"$PATH\" = /nonexistent ] || exit 9; printf success"
            };
            probe_request(
                root,
                preparation.clone(),
                phase,
                script(root, &format!("{phase:?}"), body),
                Duration::from_secs(5),
            )
        })
        .collect()
    }

    fn probe_request(
        root: &std::path::Path,
        preparation: CompatibilityPreparation,
        phase: CompatibilityPhase,
        program: std::path::PathBuf,
        timeout: Duration,
    ) -> CompatibilityProbeRequest {
        let current_dir = root.join("process");
        let reports_root = root.join("reports");
        fs::create_dir_all(&current_dir).unwrap();
        fs::create_dir_all(&reports_root).unwrap();
        CompatibilityProbeRequest {
            phase,
            commands: vec![CompatibilityCommand {
                program,
                arguments: Vec::new(),
            }],
            environment: CompatibilityEnvironment::Poisoned {
                variables: BTreeMap::from([("PATH".into(), "/nonexistent".into())]),
            },
            current_dir,
            reports_root,
            timeout,
            preparation,
        }
    }

    fn script(root: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = root.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn assert_compatibility(error: &crate::ContractError) {
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerCompatibility
        );
    }
}
