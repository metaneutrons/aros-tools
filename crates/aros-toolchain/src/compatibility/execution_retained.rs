//! Filesystem adapter for independently expected compiler-family receipts.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use aros_common::{
    publication_journal_lock_path, publication_journal_path, ArosCompilerIdentity,
    CancellationToken,
};

use super::{
    execute_native_compatibility, package_binding_required, receipt_ports_sources,
    revalidate_bound_roots, standalone_triples, validate_inputs, CompatibilityPhase,
    NativeCompatibilityCommandLogs, NativeCompatibilityExpectedPackage,
    NativeCompatibilityExpectedPortSource, NativeCompatibilityReceiptReadback,
    NativeCompatibilityReport, NativeCompatibilityRequest, StandaloneOutputRequest,
    StandaloneTargetArtifacts,
};
use crate::compatibility::{
    derive_native_compatibility_environment_identity, verify_standalone_outputs_with_compilers,
    CompatibilityHelperReport, NativeCompatibilityEnvironmentIdentity,
};
use crate::profiles::Profiles;
use crate::recipe::GitObjectId;
use crate::ContractError;

/// Execute compatibility and read back its retained evidence independently.
///
/// Before executing any phase, this adapter validates the recipe-bound profiles,
/// source contract, pristine upstream tree, measured engine/helpers and host
/// environments. After execution it reads the exact closed report/log inventory,
/// reparses standalone ELF bytes and joins them to those prior expectations.
/// New GNU and LLVM family-v2 packages use this boundary. Legacy LLVM v1 keeps
/// its existing execution and receipt format.
///
/// The caller must exclusively own all input and output roots. Remeasurement is
/// not an atomic filesystem snapshot or concurrent-writer lock. Prepared Python
/// import contents and the runtime's release identity still require independent
/// locked-input verification. Success does not authenticate a runtime, admit a
/// release, or provide recovery into newly reconstructed environment paths.
///
/// # Errors
/// Returns AX0703 for inconsistent inputs or retained evidence. It never retries,
/// deletes failed output roots, downloads, signs or publishes artifacts.
pub fn execute_native_compatibility_with_readback(
    request: &NativeCompatibilityRequest,
    profiles: &Profiles,
    cancellation: &CancellationToken,
) -> Result<NativeCompatibilityReport, ContractError> {
    if !package_binding_required(request) {
        return execute_native_compatibility(request, cancellation);
    }
    let expected = ExpectedRetainedEvidence::prepare(request, profiles)?;
    let report = execute_native_compatibility(request, cancellation)?;
    let readback = expected.readback(request, profiles)?;
    if readback.receipt_sha256 != report.receipt.sha256 {
        return Err(ContractError::compatibility(
            "retained compatibility receipt differs from its execution digest",
        ));
    }
    Ok(report)
}

/// Read back an existing compiler-family-v2 compatibility execution.
///
/// The caller independently selects the verified package, profiles, source,
/// prepared runtime and original execution roots. Retained reports cannot
/// select those expectations. This boundary revalidates them, reads the closed
/// report/log inventory and reparses the standalone outputs without executing
/// compatibility phases or creating or changing their output roots. Legacy
/// LLVM v1 is rejected rather than executed or used as a fallback.
///
/// Validation performs bounded offline Git/source inspection and a prepared
/// Python interpreter version probe. Source-declared host-generator inputs are
/// verified through temporary sealed copies, which are removed on return.
/// Thus this API is not a promise of no subprocesses or temporary filesystem
/// activity. The caller must exclusively own all supplied roots; it does not
/// acquire an atomic snapshot or reconstruct foreign runner environments.
///
/// Success establishes consistency with independently selected local inputs,
/// not authenticated command execution, runtime authenticity, A/B independence,
/// signature verification, release admission or permission to publish.
///
/// # Errors
/// Returns AX0703 for legacy packages or inconsistent inputs/evidence. It never
/// retries compatibility execution, downloads or modifies retained evidence.
pub fn readback_retained_native_compatibility(
    request: &NativeCompatibilityRequest,
    profiles: &Profiles,
) -> Result<NativeCompatibilityReceiptReadback, ContractError> {
    if !package_binding_required(request) {
        return Err(ContractError::compatibility(
            "retained native compatibility read-back requires a compiler-family-v2 package",
        ));
    }
    let expected = ExpectedRetainedEvidence::prepare(request, profiles)?;
    expected.readback(request, profiles)
}

/// Private pre-execution expectations; retained reports cannot select policy.
pub(super) struct ExpectedRetainedEvidence {
    compiler: ArosCompilerIdentity,
    upstream_tree: GitObjectId,
    environments: NativeCompatibilityEnvironmentIdentity,
    helpers: BTreeMap<String, CompatibilityHelperReport>,
    ports: Vec<NativeCompatibilityExpectedPortSource>,
    cmake_build_required: bool,
}

impl ExpectedRetainedEvidence {
    pub(super) fn portable_expectations<'a>(
        &'a self,
        request: &'a NativeCompatibilityRequest,
        profiles: &'a Profiles,
    ) -> Result<super::NativeCompatibilityReceiptExpectations<'a>, ContractError> {
        let verified = &request.relocation.second.verified;
        let source_commit = request.package_source_commit.as_ref().ok_or_else(|| {
            ContractError::compatibility("retained compatibility lost the package source commit")
        })?;
        Ok(super::NativeCompatibilityReceiptExpectations {
            package: NativeCompatibilityExpectedPackage {
                manifest: &verified.manifest,
                archive_sha256: &verified.archive_sha256,
                archive_size: verified.archive_size,
                compiler: &self.compiler,
                source_commit,
                host: &request.host,
            },
            profiles,
            profile: &request.profile,
            gnu_source_preset: request.source_preset.as_deref(),
            cmake_build_required: self.cmake_build_required,
            sdk_consumer_source_tree_sha256: &request.preparation.source_tree_sha256,
            engine_api_version: request.preparation.engine_api_version,
            engine_sha256: &request.preparation.engine_sha256,
            helpers: &self.helpers,
            host_tools: &self.environments.host_tools,
            sdk_environment_sha256: &self.environments.sdk_environment_sha256,
            standalone_environment_sha256: &self.environments.standalone_environment_sha256,
            upstream_source_commit: &request.upstream_source_commit,
            upstream_source_tree: &self.upstream_tree,
            ports_sources: &self.ports,
        })
    }

    pub(super) fn prepare(
        request: &NativeCompatibilityRequest,
        profiles: &Profiles,
    ) -> Result<Self, ContractError> {
        validate_profiles(request, profiles)?;
        super::super::validate_preparation(&request.preparation)?;
        let inputs = validate_inputs(request)?;
        let environments = derive_native_compatibility_environment_identity(
            &request.host_python,
            &request.host_tools,
            &request.host,
        )?;
        let helpers = request
            .preparation
            .helpers
            .iter()
            .map(|(name, identity)| {
                (
                    name.clone(),
                    CompatibilityHelperReport {
                        sha256: identity.sha256.clone(),
                        size: identity.size,
                    },
                )
            })
            .collect();
        let ports = receipt_ports_sources(&request.ports_sources)?
            .into_iter()
            .map(|source| NativeCompatibilityExpectedPortSource {
                id: source.id,
                cache_filename: source.cache_filename,
                relative_path: source.relative_path,
                fetch_marker: source.fetch_marker,
                sha256: source.sha256,
                size: source.size,
            })
            .collect();
        Ok(Self {
            compiler: inputs.compiler,
            upstream_tree: inputs.upstream_source_tree,
            environments,
            helpers,
            ports,
            cmake_build_required: inputs.native_consumer_contract.is_some(),
        })
    }

    pub(super) fn readback(
        &self,
        request: &NativeCompatibilityRequest,
        profiles: &Profiles,
    ) -> Result<NativeCompatibilityReceiptReadback, ContractError> {
        validate_profiles(request, profiles)?;
        super::super::validate_preparation(&request.preparation)?;
        revalidate_bound_roots(request)?;
        request.ports_sources.revalidate()?;
        let upstream_root = super::checked_directory(
            &request.upstream_source_root,
            "retained pristine upstream source",
        )?;
        let upstream_tree = super::verify_pristine_upstream_source(
            &upstream_root,
            &request.upstream_source_commit,
        )?;
        let environments = derive_native_compatibility_environment_identity(
            &request.host_python,
            &request.host_tools,
            &request.host,
        )?;
        if upstream_tree != self.upstream_tree || environments != self.environments {
            return Err(ContractError::compatibility(
                "native compatibility source or environment changed before retained read-back",
            ));
        }
        let standalone_request = standalone_output_request(request)?;
        let standalone_files = standalone_request
            .targets
            .values()
            .flat_map(|target| [&target.c, &target.cxx])
            .map(|path| relative_name(path))
            .collect::<Result<BTreeSet<_>, _>>()?;
        verify_directory_inventory(&standalone_request.output_root, &standalone_files)?;
        let compilers = standalone_request
            .targets
            .keys()
            .map(|triple| (triple.clone(), self.compiler.clone()))
            .collect();
        let standalone = verify_standalone_outputs_with_compilers(&standalone_request, &compilers)?;
        let reports_root = super::checked_directory(&request.reports_root, "retained reports")?;
        let mut paths = BTreeSet::from([PathBuf::from(super::COMPATIBILITY_RECEIPT_FILE)]);
        let mut phase_reports = BTreeMap::new();
        let mut logs = BTreeMap::new();
        for phase in super::super::REQUIRED_PROBE_PHASES {
            let commands = match phase {
                CompatibilityPhase::CmakeConsumer => 1 + usize::from(self.cmake_build_required),
                CompatibilityPhase::StandaloneC | CompatibilityPhase::StandaloneCxx => {
                    standalone_request.targets.len()
                }
                _ => 1,
            };
            let report_paths = super::super::ProbeReportPaths::new(&reports_root, phase, commands);
            paths.insert(relative_name(&report_paths.report)?);
            phase_reports.insert(
                phase,
                super::super::read_regular_bounded(
                    &report_paths.report,
                    "retained compatibility phase report",
                    crate::canonical::MAX_DOCUMENT_BYTES,
                )?,
            );
            let mut command_logs = Vec::with_capacity(commands);
            for command in report_paths.commands {
                paths.insert(relative_name(&command.stdout)?);
                paths.insert(relative_name(&command.stderr)?);
                command_logs.push(NativeCompatibilityCommandLogs {
                    stdout: read_log(&command.stdout)?,
                    stderr: read_log(&command.stderr)?,
                });
            }
            logs.insert(phase, command_logs);
        }
        verify_report_inventory(&reports_root, &paths)?;
        let receipt = super::super::read_regular_bounded(
            &reports_root.join(super::COMPATIBILITY_RECEIPT_FILE),
            "retained aggregate compatibility receipt",
            crate::canonical::MAX_DOCUMENT_BYTES,
        )?;
        self.portable_expectations(request, profiles)?
            .readback_documents(&receipt, &phase_reports, &logs, &standalone)
    }
}

fn validate_profiles(
    request: &NativeCompatibilityRequest,
    profiles: &Profiles,
) -> Result<(), ContractError> {
    let profile = profiles.select(request.profile.name()).map_err(|_| {
        ContractError::compatibility(
            "retained compatibility profile is absent from the independently selected inputs",
        )
    })?;
    if profile.document_sha256() != request.profile.document_sha256()
        || profiles.upstream_commit() != &request.upstream_source_commit
    {
        return Err(ContractError::compatibility(
            "retained compatibility profiles differ from the independently selected inputs",
        ));
    }
    Ok(())
}

fn standalone_output_request(
    request: &NativeCompatibilityRequest,
) -> Result<StandaloneOutputRequest, ContractError> {
    let output_root = super::checked_directory(
        &request.standalone_output_root,
        "retained standalone output root",
    )?;
    let targets = standalone_triples(&request.profile)?
        .into_iter()
        .map(|triple| {
            let cpu = triple
                .split_once('-')
                .map_or(triple.as_str(), |(cpu, _)| cpu);
            let artifacts = StandaloneTargetArtifacts {
                c: output_root.join(format!("c-{cpu}.o")),
                cxx: output_root.join(format!("cxx-{cpu}.o")),
            };
            (triple, artifacts)
        })
        .collect();
    Ok(StandaloneOutputRequest {
        output_root,
        targets,
    })
}

fn relative_name(path: &Path) -> Result<PathBuf, ContractError> {
    path.file_name()
        .map(PathBuf::from)
        .ok_or_else(|| ContractError::compatibility("retained compatibility path lacks a filename"))
}

fn read_log(path: &Path) -> Result<Vec<u8>, ContractError> {
    super::super::read_regular_bounded(
        path,
        "retained compatibility command log",
        super::super::MAX_RENDERED_LOG_BYTES,
    )
}

fn verify_report_inventory(root: &Path, expected: &BTreeSet<PathBuf>) -> Result<(), ContractError> {
    // The common durable publisher intentionally retains a persistent empty
    // advisory lock for each published file. Admit only names derived from
    // the exact data inventory, not arbitrary lock-shaped entries or journals.
    let mut expected_with_locks = expected.clone();
    for name in expected {
        let journal = publication_journal_path(&root.join(name), "file").map_err(|_| {
            ContractError::compatibility("cannot derive retained report publication journal")
        })?;
        let lock = publication_journal_lock_path(&journal).map_err(|_| {
            ContractError::compatibility("cannot derive retained report publication lock")
        })?;
        super::super::read_regular_bounded(&lock, "retained report publication lock", 0)?;
        expected_with_locks.insert(relative_name(&lock)?);
    }
    verify_directory_inventory(root, &expected_with_locks)
}

fn verify_directory_inventory(
    root: &Path,
    expected: &BTreeSet<PathBuf>,
) -> Result<(), ContractError> {
    let mut observed = BTreeSet::new();
    for entry in fs::read_dir(root)
        .map_err(|_| ContractError::compatibility("cannot enumerate retained evidence"))?
    {
        let entry = entry
            .map_err(|_| ContractError::compatibility("cannot inspect retained evidence entry"))?;
        if observed.len() >= expected.len() || !observed.insert(PathBuf::from(entry.file_name())) {
            return Err(ContractError::compatibility(
                "retained compatibility evidence directory has unexpected entries",
            ));
        }
    }
    if &observed != expected {
        return Err(ContractError::compatibility(
            "retained compatibility evidence directory differs from the exact inventory",
        ));
    }
    Ok(())
}
