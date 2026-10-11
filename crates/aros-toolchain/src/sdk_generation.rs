//! Source-bound SDK generation and packaging, without publication authority.
//!
//! Only the six-phase native executor can construct a completed generation.
//! Packaging cannot substitute another identity, SDK root or inventory. All
//! retained inputs are revalidated; this is local consistency, not authenticated
//! execution, a concurrent-writer snapshot or release admission.

use std::fs;
use std::path::{Path, PathBuf};

use aros_common::sdk_consumer_contract::SDK_CONSUMER_CONTRACT_SCHEMA;
use aros_common::sdk_manifest::SdkIdentity;
use aros_common::{
    publish_atomic_file, sha256_bytes, AtomicFilePolicy, CancellationToken, Sha256Digest,
};
use serde::{Deserialize, Serialize};

use crate::compatibility::{
    derive_native_compatibility_environment_identity_with_epoch,
    execute_native_compatibility_with_export, execute_native_compatibility_with_readback,
    readback_generated_sdk_root, NativeCompatibilityExecutionExport, NativeCompatibilityReport,
    NativeCompatibilityRequest,
};
use crate::profiles::Profiles;
use crate::sdk_package::{self, SdkPackageOutput, SdkPackageRequest, SdkVerificationRequest};
use crate::sdk_source::VerifiedSdkSource;
use crate::{canonical, inspection, ContractError};

/// Independent selection for executing and recording one source-owned SDK.
#[derive(Debug, Clone)]
pub struct SdkGenerationRequest {
    /// Previously verified committed source, sealed contract and complete ABI.
    pub source: VerifiedSdkSource,
    /// Independently selected native compiler/runtime and disjoint work roots.
    pub compatibility: NativeCompatibilityRequest,
    /// Recipe-bound compiler profiles, separate from the SDK source profile.
    pub profiles: Profiles,
    /// Absent regular record outside all source, input and execution roots.
    pub record_path: PathBuf,
}

/// Opaque completed generation; a descriptor alone cannot construct it.
#[derive(Debug, Clone)]
pub struct CompletedSdkGeneration {
    request: SdkGenerationRequest,
    report: NativeCompatibilityReport,
    candidate_root: PathBuf,
    record: GenerationRecord,
    record_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GnuGenerationRecordV1 {
    schema: String,
    assurance: String,
    identity: SdkIdentity,
    source_profile: String,
    environment_sha256: Sha256Digest,
    compatibility_receipt_sha256: Sha256Digest,
    sdk_link_receipt_sha256: Sha256Digest,
    sdk_inventory_sha256: Sha256Digest,
    sdk_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LlvmGenerationRecordV1 {
    schema: String,
    assurance: String,
    identity: SdkIdentity,
    source_profile: String,
    binding_sha256: Sha256Digest,
    environment_sha256: Sha256Digest,
    compatibility_receipt_sha256: Sha256Digest,
    sdk_link_receipt_sha256: Sha256Digest,
    sdk_inventory_sha256: Sha256Digest,
    sdk_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum GenerationRecord {
    GnuV1(GnuGenerationRecordV1),
    LlvmV1(LlvmGenerationRecordV1),
}

impl GenerationRecord {
    const fn identity(&self) -> &SdkIdentity {
        match self {
            Self::GnuV1(record) => &record.identity,
            Self::LlvmV1(record) => &record.identity,
        }
    }

    const fn sdk_inventory_sha256(&self) -> &Sha256Digest {
        match self {
            Self::GnuV1(record) => &record.sdk_inventory_sha256,
            Self::LlvmV1(record) => &record.sdk_inventory_sha256,
        }
    }
}

impl CompletedSdkGeneration {
    /// Source-owned identity that actually selected this generation.
    #[must_use]
    pub const fn identity(&self) -> &SdkIdentity {
        self.record.identity()
    }

    /// Complete inventory from original and independently relocated links.
    #[must_use]
    pub const fn tree_sha256(&self) -> &Sha256Digest {
        self.record.sdk_inventory_sha256()
    }

    /// Exact no-clobber generation record, not a release attestation.
    #[must_use]
    pub fn record_path(&self) -> &Path {
        &self.request.record_path
    }

    /// Digest of the closed generation record.
    #[must_use]
    pub const fn record_sha256(&self) -> &Sha256Digest {
        &self.record_sha256
    }

    /// Recheck source, environment, six phases, four links and record bytes.
    ///
    /// # Errors
    /// Rejects changed inputs or retained evidence; never reruns a build.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        validate_selection(&self.request)?;
        let root = readback_generated_sdk_root(
            &self.request.compatibility,
            &self.request.profiles,
            &self.report,
        )?;
        if root != self.candidate_root
            || generation_record(&self.request, &self.report)? != self.record
        {
            return Err(error(
                "completed SDK generation selection or evidence changed",
            ));
        }
        let bytes = inspection::read(&self.request.record_path)?;
        if sha256_bytes(&bytes) != self.record_sha256 || bytes != record_bytes(&self.record)? {
            return Err(error("completed SDK generation record changed"));
        }
        self.request.source.revalidate()
    }
}

/// Execute the complete native SDK proof with the explicit product epoch.
///
/// No source/compiler identity is inferred from another product. Generation
/// requires a supported source contract paired with its family-v2 compiler. Failed
/// execution roots remain diagnostic evidence; no completed record is emitted.
///
/// # Errors
/// Rejects mismatched selection before execution, incomplete/changed retained
/// evidence or an occupied record path. Never retries, signs or publishes.
pub fn generate(
    request: &SdkGenerationRequest,
    cancellation: &CancellationToken,
) -> Result<CompletedSdkGeneration, ContractError> {
    let request = prepare_generation_request(request)?;
    let report = execute_native_compatibility_with_readback(
        &request.compatibility,
        &request.profiles,
        cancellation,
    )?;
    finish_generation(request, report)
}

/// Execute one source-bound SDK generation and retain its portable compatibility bytes.
///
/// The returned export is the exact pre-execution input observation and portable
/// report/evidence closure produced by the same native execution used for the
/// completed generation. It is local consistency evidence only: this operation
/// does not publish, sign, authenticate an external job, or admit a release.
///
/// # Errors
/// Rejects the same source, compiler, root and record selections as [`generate`]
/// and propagates execution or retained-evidence failures. Outputs from failed
/// execution remain diagnostic evidence; no retry or publication occurs.
pub fn generate_with_export(
    request: &SdkGenerationRequest,
    cancellation: &CancellationToken,
) -> Result<(CompletedSdkGeneration, NativeCompatibilityExecutionExport), ContractError> {
    let request = prepare_generation_request(request)?;
    let execution = execute_native_compatibility_with_export(
        &request.compatibility,
        &request.profiles,
        cancellation,
    )?;
    let report = execution.report().clone();
    let completed = finish_generation(request, report)?;
    Ok((completed, execution))
}

fn prepare_generation_request(
    request: &SdkGenerationRequest,
) -> Result<SdkGenerationRequest, ContractError> {
    validate_selection(request)?;
    let mut request = request.clone();
    request.record_path = selected_destination(&request, &request.record_path)?;
    require_absent(&request.record_path)?;
    Ok(request)
}

fn finish_generation(
    request: SdkGenerationRequest,
    report: NativeCompatibilityReport,
) -> Result<CompletedSdkGeneration, ContractError> {
    request.source.revalidate()?;
    let candidate_root =
        readback_generated_sdk_root(&request.compatibility, &request.profiles, &report)?;
    let record = generation_record(&request, &report)?;
    let bytes = record_bytes(&record)?;
    let record_sha256 = sha256_bytes(&bytes);
    require_absent(&request.record_path)?;
    publish_atomic_file(&request.record_path, &bytes, AtomicFilePolicy::NoClobber)
        .map_err(|_| error("cannot persist complete SDK generation record"))?;
    let completed = CompletedSdkGeneration {
        request,
        report,
        candidate_root,
        record,
        record_sha256,
    };
    completed.revalidate()?;
    Ok(completed)
}

/// Package only the exact source-generated and ordinarily linked SDK.
///
/// The caller supplies an absent output and forbidden prefixes, never another
/// product identity, candidate or tree hash. Successful output is read back
/// against this completed generation; it is not signed or release-admitted.
///
/// # Errors
/// Rejects stale source/evidence, overlapping output, unsafe package bytes or
/// failed complete package read-back. Published local outputs are retained.
pub fn package_generated(
    completed: &CompletedSdkGeneration,
    output_dir: &Path,
    forbidden_prefixes: &[PathBuf],
) -> Result<SdkPackageOutput, ContractError> {
    completed.revalidate()?;
    let output_dir = inspection::destination(output_dir)?;
    if overlaps(&output_dir, &completed.request.record_path) {
        return Err(error("SDK package output overlaps its generation record"));
    }
    let output_dir = selected_destination(&completed.request, &output_dir)?;
    let output = sdk_package::package(&SdkPackageRequest {
        candidate_root: completed.candidate_root.clone(),
        output_dir,
        identity: completed.identity().clone(),
        expected_tree_sha256: completed.tree_sha256().clone(),
        forbidden_prefixes: forbidden_prefixes.to_vec(),
    })?;
    completed.revalidate()?;
    sdk_package::verify(&SdkVerificationRequest {
        package_dir: output.output_dir.clone(),
        identity: completed.identity().clone(),
        expected_tree_sha256: completed.tree_sha256().clone(),
        expected_archive_sha256: output.archive_sha256.clone(),
        expected_archive_size: output.archive_size,
        forbidden_prefixes: forbidden_prefixes.to_vec(),
    })?;
    completed.revalidate()?;
    Ok(output)
}

/// Require a source contract implemented by the current generation executor.
///
/// Call this before any compatibility preparation or source program execution.
///
/// # Errors
/// Rejects source contracts without a matching generation execution path.
pub fn validate_source_executor(source: &VerifiedSdkSource) -> Result<(), ContractError> {
    let family = source
        .source_profile()
        .transpiler
        .as_ref()
        .map(|context| context.toolchain.as_str());
    if !matches!(
        (source.consumer_contract_schema(), family),
        ("aros-native-consumer-contract-v2", Some("gnu"))
            | (SDK_CONSUMER_CONTRACT_SCHEMA, Some("llvm"))
    ) {
        return Err(error(
            "SDK generation executor does not support the selected source contract and compiler family",
        ));
    }
    Ok(())
}

fn validate_selection(request: &SdkGenerationRequest) -> Result<(), ContractError> {
    let identity = request.source.identity();
    let native = &request.compatibility;
    // Cheap explicit mismatches must not run source programs or reserve roots.
    if native.consumer_source_date_epoch != Some(identity.source_date_epoch) {
        return Err(error(
            "SDK generation epoch differs from its selected product identity",
        ));
    }
    if native.preparation.source_root != request.source.materialized_source_root()
        || native.preparation.source_tree_sha256 != identity.source.content_sha256
    {
        return Err(error(
            "SDK generation source differs from its verified committed projection",
        ));
    }
    validate_source_executor(&request.source)?;
    let first_manifest = &native.relocation.first.verified.manifest;
    let second_manifest = &native.relocation.second.verified.manifest;
    let (Some(compiler), Some(second_compiler)) = (
        first_manifest.compiler.as_ref(),
        second_manifest.compiler.as_ref(),
    ) else {
        return Err(error(
            "SDK generation requires a family-v2 compiler package",
        ));
    };
    if first_manifest.schema != 2
        || second_manifest.schema != 2
        || compiler.family() != second_compiler.family()
    {
        return Err(error(
            "SDK generation requires a family-v2 compiler package",
        ));
    }
    match (request.source.consumer_contract_schema(), compiler.family()) {
        ("aros-native-consumer-contract-v2", "gnu") => {
            if native.llvm_sdk_source.is_some() {
                return Err(error(
                    "GNU SDK generation requires its verified source preset and no generic source selection",
                ));
            }
            if native.source_preset.as_deref()
                != Some(request.source.source_profile().name.as_str())
            {
                return Err(error(
                    "SDK generation preset differs from its verified source profile",
                ));
            }
        }
        (SDK_CONSUMER_CONTRACT_SCHEMA, "llvm") => {
            let Some(selected_source) = native.llvm_sdk_source.as_ref() else {
                return Err(error(
                    "LLVM SDK generation requires the verified generic source selection",
                ));
            };
            if native.source_preset.is_some()
                || !same_source_selection(selected_source, &request.source)
            {
                return Err(error(
                    "LLVM SDK generation selection differs from its verified generic source",
                ));
            }
            let profile = request.source.source_profile();
            let context = profile
                .transpiler
                .as_ref()
                .ok_or_else(|| error("LLVM SDK source has no compiler profile mapping"))?;
            if request.source.identity().target.target_triple != native.profile.target_triple()
                || profile.toolchain_profile() != native.profile.name()
                || profile.arch.source_cpu() != native.profile.cpu()
                || profile.platform != native.profile.platform()
                || profile.float_abi.as_deref().unwrap_or_default() != native.profile.float_abi()
                || context.toolchain != "llvm"
            {
                return Err(error(
                    "LLVM SDK source profile differs from its selected compiler profile",
                ));
            }
        }
        _ => {
            return Err(error(
                "SDK source contract and family-v2 compiler selection do not match",
            ));
        }
    }
    request.source.revalidate()
}

fn same_source_selection(first: &VerifiedSdkSource, second: &VerifiedSdkSource) -> bool {
    first.identity() == second.identity()
        && first.source_profile().name == second.source_profile().name
        && first.consumer_contract_schema() == second.consumer_contract_schema()
        && first.original_source_root() == second.original_source_root()
        && first.materialized_source_root() == second.materialized_source_root()
}

fn generation_record(
    request: &SdkGenerationRequest,
    report: &NativeCompatibilityReport,
) -> Result<GenerationRecord, ContractError> {
    let environment = derive_native_compatibility_environment_identity_with_epoch(
        &request.compatibility.host_python,
        &request.compatibility.host_tools,
        &request.compatibility.host,
        request.compatibility.consumer_source_date_epoch,
    )?;
    let assurance = "local-generation-binding-not-release-admission".to_owned();
    let identity = request.source.identity().clone();
    let source_profile = request.source.source_profile().name.clone();
    let environment_sha256 = environment.sdk_environment_sha256;
    let compatibility_receipt_sha256 = report.receipt.sha256.clone();
    match request.source.consumer_contract_schema() {
        "aros-native-consumer-contract-v2" => {
            if report.llvm_sdk.is_some() {
                return Err(error("GNU SDK generation received LLVM link evidence"));
            }
            let sdk = report
                .native_sdk
                .as_ref()
                .ok_or_else(|| error("SDK generation requires four ordinary SDK links"))?;
            Ok(GenerationRecord::GnuV1(GnuGenerationRecordV1 {
                schema: "aros-sdk-generation-record-v1".into(),
                assurance,
                identity,
                source_profile,
                environment_sha256,
                compatibility_receipt_sha256,
                sdk_link_receipt_sha256: sdk.receipt_sha256.clone(),
                sdk_inventory_sha256: sdk.sdk_inventory_sha256.clone(),
                sdk_entries: sdk.sdk_entries,
            }))
        }
        SDK_CONSUMER_CONTRACT_SCHEMA => {
            if report.native_sdk.is_some() {
                return Err(error("LLVM SDK generation received GNU link evidence"));
            }
            let sdk = report
                .llvm_sdk
                .as_ref()
                .ok_or_else(|| error("LLVM SDK generation requires four ordinary SDK links"))?;
            if sdk.identity != identity {
                return Err(error(
                    "LLVM SDK link evidence differs from its verified source identity",
                ));
            }
            Ok(GenerationRecord::LlvmV1(LlvmGenerationRecordV1 {
                schema: "aros-llvm-sdk-generation-record-v1".into(),
                assurance,
                identity,
                source_profile,
                binding_sha256: sdk.binding_sha256.clone(),
                environment_sha256,
                compatibility_receipt_sha256,
                sdk_link_receipt_sha256: sdk.receipt_sha256.clone(),
                sdk_inventory_sha256: sdk.sdk_inventory_sha256.clone(),
                sdk_entries: sdk.sdk_entries,
            }))
        }
        _ => Err(error("SDK generation source contract is unsupported")),
    }
}

fn record_bytes(record: &GenerationRecord) -> Result<Vec<u8>, ContractError> {
    canonical::bytes(
        &serde_json::to_value(record).map_err(|_| error("cannot encode SDK generation record"))?,
    )
}

fn selected_destination(
    request: &SdkGenerationRequest,
    path: &Path,
) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(error("SDK generation output must be an absolute leaf"));
    }
    let path = inspection::destination(path)?;
    let native = &request.compatibility;
    let mut roots = vec![
        request.source.original_source_root(),
        request.source.materialized_source_root(),
        &native.preparation.engine_root,
        &native.cmake_build_root,
        &native.upstream_source_root,
        &native.upstream_build_root,
        &native.standalone_output_root,
        &native.reports_root,
        &native.relocation.first.root,
        &native.relocation.second.root,
        &native.host_tools.root,
        &native.ports_sources.root,
    ];
    roots.extend(
        native
            .host_python
            .import_roots()
            .iter()
            .map(PathBuf::as_path),
    );
    roots.extend(native.host_generator_cache_root.as_deref());
    // Programs are regular file leaves, not directories. Protect their parent
    // namespaces without asking directory resolution to accept a file.
    roots.extend(
        native
            .preparation
            .helpers
            .values()
            .filter_map(|helper| helper.path.parent()),
    );
    for root in roots {
        if overlaps(&path, &inspection::destination(root)?) {
            return Err(error(
                "SDK generation output overlaps protected inputs or execution roots",
            ));
        }
    }
    let mut files = vec![
        native.cmake_program.as_path(),
        native.ninja_program.as_path(),
        native.host_python.interpreter().path.as_path(),
        native.standalone_fixtures.c.as_path(),
        native.standalone_fixtures.cxx.as_path(),
    ];
    files.extend(
        native
            .host_tools
            .tools
            .values()
            .map(|tool| tool.program.as_path()),
    );
    for file in files {
        // These already selected inputs are remeasured by native read-back.
        // Resolve file leaves separately: `destination` accepts directories,
        // not existing executable files as a destination ancestor.
        let file = file
            .canonicalize()
            .map_err(|_| error("cannot resolve protected SDK generation file"))?;
        if !file.is_file() || overlaps(&path, &file) {
            return Err(error(
                "SDK generation output overlaps a protected input file",
            ));
        }
    }
    Ok(path)
}

fn overlaps(first: &Path, second: &Path) -> bool {
    first.starts_with(second) || second.starts_with(first)
}

fn require_absent(path: &Path) -> Result<(), ContractError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(error("SDK generation record must be absent")),
    }
}

fn error(message: &str) -> ContractError {
    ContractError::compatibility(message)
}
