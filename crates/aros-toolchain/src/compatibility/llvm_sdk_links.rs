//! Supervised ordinary LLVM application links and closed local readback.
//!
//! This is a local application-link proof, not a complete SDK generation,
//! package authentication, full scalar ABI or C++ execution proof. The owning
//! generator must bind its generated SDK and publish portable job evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aros_common::sdk_consumer_contract::{
    load_bound_sdk_consumer_contract, SDK_CONSUMER_CONTRACT_SCHEMA,
};
use aros_common::sdk_manifest::SdkIdentity;
use aros_common::{
    measure_tree_content_cas_bounded, sha256_bytes, ArosCompilerIdentity, CancellationToken,
    Sha256Digest, TreeContentCas, TreeTraversalLimits,
};
use serde::{Deserialize, Serialize};

use super::native_sdk_relocation::{encode_inventory, RelocatedSdk};
use super::{
    checked_directory, command_identity, llvm_sdk_application, llvm_sdk_link_paths,
    plan_llvm_sdk_application_links, read_regular_bounded, run_probe, validate_preparation,
    write_new_regular, CompatibilityEnvironment, CompatibilityPhase, CompatibilityPreparation,
    CompatibilityProbeReport, CompatibilityProbeRequest, LlvmSdkApplicationPlan,
    LlvmSdkApplicationPlanRequest, LlvmSdkDriverBinding, ProbeReportPaths,
};
use crate::sdk_source::VerifiedSdkSource;
use crate::{canonical, ContractError};

const RECEIPT: &str = "llvm-sdk-links.receipt.json";
const INVENTORY: &str = "sdk-inventory.json";
const MAX_ELF_BYTES: usize = 128 * 1024 * 1024;
const MAX_MAP_BYTES: usize = 16 * 1024 * 1024;
const PROOF_LIMITS: TreeTraversalLimits = TreeTraversalLimits {
    max_entries: 100_128,
    max_regular_file_bytes: 3 * 1024 * 1024 * 1024,
};

#[path = "llvm_sdk_links_readback.rs"]
mod readback;
pub use readback::readback_llvm_sdk_links;

/// Independently selected local inputs; no publication authority is conveyed.
#[derive(Debug, Clone)]
pub struct LlvmSdkLinkRequest {
    /// Clean committed source, sealed generic contract and probes.
    pub source: VerifiedSdkSource,
    /// Independently authenticated compiler's measured local driver binding.
    pub drivers: LlvmSdkDriverBinding,
    /// Exact source and tools-owned engine/helpers for the shared process runner.
    pub preparation: CompatibilityPreparation,
    /// Complete generated SDK; its generation is the owner's separate gate.
    pub sdk_root: PathBuf,
    /// Fresh, disjoint no-clobber destination; failed evidence remains here.
    pub output_root: PathBuf,
    /// Positive elapsed-time budget for all four links and their validation.
    /// Filesystem validation is not preemptible; no completed proof is returned
    /// after this budget, but it is not a hard wall-clock limit on filesystem I/O.
    pub timeout: Duration,
}

/// Opaque completed four-link proof, retaining its exact local input selection.
pub struct CompletedLlvmSdkLinks {
    request: LlvmSdkLinkRequest,
    plan: LlvmSdkApplicationPlan,
    relocated: RelocatedSdk,
    record: LinkRecord,
    receipt_sha256: Sha256Digest,
    snapshot: TreeContentCas,
}

impl std::fmt::Debug for CompletedLlvmSdkLinks {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompletedLlvmSdkLinks")
            .field("receipt_sha256", &self.receipt_sha256)
            .finish_non_exhaustive()
    }
}

impl CompletedLlvmSdkLinks {
    /// No-clobber local receipt. This is not an external job attestation.
    #[must_use]
    pub fn receipt_path(&self) -> PathBuf {
        self.request.output_root.join("receipt").join(RECEIPT)
    }

    /// Exact receipt byte digest.
    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.receipt_sha256
    }

    /// Complete original/relocated SDK inventory digest.
    #[must_use]
    pub const fn sdk_inventory_sha256(&self) -> &Sha256Digest {
        &self.record.sdk_inventory_sha256
    }

    /// Complete original/relocated SDK inventory entry count.
    #[must_use]
    pub const fn sdk_entries(&self) -> usize {
        self.record.sdk_entries
    }

    /// Recheck commands, source/compiler/SDK trees and retained evidence.
    ///
    /// No compiler is rerun and no output is repaired or adopted.
    ///
    /// # Errors
    /// Rejects cancellation, replaced roots, changed inputs/logs/reports/output,
    /// incomplete closure or a receipt disagreement.
    pub fn revalidate(&self, cancellation: &CancellationToken) -> Result<(), ContractError> {
        preflight(&self.request, cancellation)?;
        self.plan.revalidate_inputs(cancellation)?;
        self.relocated.revalidate()?;
        let current = proof_record(&self.request, &self.plan, &self.relocated, true)?;
        let bytes = read_regular_bounded(
            &self.receipt_path(),
            "LLVM SDK link receipt",
            canonical::MAX_DOCUMENT_BYTES,
        )?;
        let parsed: LinkRecord = serde_json::from_slice(&bytes)
            .map_err(|_| error("LLVM SDK link receipt is not closed JSON"))?;
        if current != self.record
            || parsed != current
            || bytes != encode(&current)?
            || sha256_bytes(&bytes) != self.receipt_sha256
            || proof_snapshot(&self.request.output_root)? != self.snapshot
        {
            return Err(error("LLVM SDK retained application evidence changed"));
        }
        self.plan.revalidate_inputs(cancellation)?;
        self.relocated.revalidate()?;
        if proof_snapshot(&self.request.output_root)? != self.snapshot {
            return Err(error("LLVM SDK evidence changed during readback"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    sha256: Sha256Digest,
    size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkRecord {
    schema: String,
    assurance: String,
    identity: SdkIdentity,
    compiler: ArosCompilerIdentity,
    compiler_tree_sha256: Sha256Digest,
    sdk_inventory_sha256: Sha256Digest,
    sdk_entries: usize,
    retained_tree_sha256: Sha256Digest,
    members: BTreeMap<String, Member>,
}

/// Run four normal source-selected LLVM links, then validate their local closure.
///
/// Source/compiler inputs are rechecked, the SDK is independently copied,
/// default startup/libraries remain enabled, children receive a closed poisoned
/// environment, and logs survive failed execution. Both language outputs must
/// be byte-identical after relocation and satisfy source-declared ELF facts.
/// Complete SDK generation and external evidence authentication are not inferred.
///
/// # Errors
/// Rejects selection disagreements, unsafe roots, failed/cancelled/timed-out
/// commands, foreign linker inputs, changed trees or incomplete retained bytes.
/// Never retries, overwrites, signs or publishes.
pub fn execute_llvm_sdk_links(
    request: &LlvmSdkLinkRequest,
    cancellation: &CancellationToken,
) -> Result<CompletedLlvmSdkLinks, ContractError> {
    let started = Instant::now();
    preflight(request, cancellation)?;
    let output = reserve_output(request)?;
    let mut request = request.clone();
    request.output_root = output;
    let relocated = RelocatedSdk::prepare(
        &request.sdk_root,
        &request.output_root.join("relocated-sdk"),
    )?;
    write_new_regular(
        &request.output_root.join(INVENTORY),
        &encode_inventory(&relocated.inventory)?,
        "LLVM SDK complete inventory",
    )?;
    let plan = plan_llvm_sdk_application_links(
        &LlvmSdkApplicationPlanRequest {
            source: request.source.clone(),
            drivers: request.drivers.clone(),
            original_sdk: relocated.original.clone(),
            relocated_sdk: relocated.relocated.clone(),
            output_root: request.output_root.join("links"),
        },
        cancellation,
    )?;
    let links = request.output_root.join("links");
    let reports = links.join("reports");
    let temporary = request.output_root.join("tmp");
    fs::create_dir(&links).map_err(|_| error("cannot reserve LLVM SDK link directory"))?;
    fs::create_dir(&reports).map_err(|_| error("cannot reserve LLVM SDK report directory"))?;
    fs::create_dir(&temporary).map_err(|_| error("cannot reserve LLVM SDK private temporary"))?;
    fs::create_dir(request.output_root.join("receipt"))
        .map_err(|_| error("cannot reserve LLVM SDK receipt namespace"))?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o700))
        .map_err(|_| error("cannot restrict LLVM SDK private temporary"))?;
    let environment = environment(&temporary)?;
    for (batch, phase) in [
        CompatibilityPhase::StandaloneC,
        CompatibilityPhase::StandaloneCxx,
    ]
    .into_iter()
    .enumerate()
    {
        preflight(&request, cancellation)?;
        plan.revalidate_inputs(cancellation)?;
        relocated.revalidate()?;
        let timeout = remaining(started, request.timeout)?;
        run_probe(
            &CompatibilityProbeRequest {
                phase,
                commands: plan.commands()[batch * 2..batch * 2 + 2].to_vec(),
                environment: environment.clone(),
                current_dir: links.clone(),
                reports_root: reports.clone(),
                timeout,
                preparation: request.preparation.clone(),
            },
            cancellation,
        )?;
    }
    preflight(&request, cancellation)?;
    plan.revalidate_inputs(cancellation)?;
    relocated.revalidate()?;
    let record = proof_record(&request, &plan, &relocated, false)?;
    remaining(started, request.timeout)?;
    let bytes = encode(&record)?;
    write_new_regular(
        &request.output_root.join("receipt").join(RECEIPT),
        &bytes,
        "LLVM SDK link receipt",
    )?;
    let snapshot = proof_snapshot(&request.output_root)?;
    let completed = CompletedLlvmSdkLinks {
        request,
        plan,
        relocated,
        record,
        receipt_sha256: sha256_bytes(&bytes),
        snapshot,
    };
    completed.revalidate(cancellation)?;
    remaining(started, completed.request.timeout)?;
    Ok(completed)
}

fn remaining(started: Instant, timeout: Duration) -> Result<Duration, ContractError> {
    timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| error("LLVM SDK application deadline expired"))
}

fn preflight(
    request: &LlvmSdkLinkRequest,
    cancellation: &CancellationToken,
) -> Result<(), ContractError> {
    if cancellation.is_cancelled() || request.timeout.is_zero() {
        return Err(error(
            "LLVM SDK links require a positive deadline and uncancelled request",
        ));
    }
    if request.source.consumer_contract_schema() != SDK_CONSUMER_CONTRACT_SCHEMA
        || request.drivers.target() != &request.source.identity().target
        || request.preparation.source_tree_sha256 != request.source.identity().source.content_sha256
        || checked_directory(&request.preparation.source_root, "LLVM SDK source")?
            != request.source.materialized_source_root()
    {
        return Err(error(
            "LLVM SDK source, drivers and preparation selection disagree",
        ));
    }
    llvm_sdk_application::validate_target(&request.source.identity().target)?;
    request.source.revalidate()?;
    request.drivers.revalidate(cancellation)?;
    validate_preparation(&request.preparation)?;
    Ok(())
}

fn reserve_output(request: &LlvmSdkLinkRequest) -> Result<PathBuf, ContractError> {
    let path = &request.output_root;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(error(
            "LLVM SDK output must be an absolute non-traversing leaf",
        ));
    }
    let parent = checked_directory(
        path.parent()
            .ok_or_else(|| error("LLVM SDK output parent is absent"))?,
        "LLVM SDK output parent",
    )?;
    let output = parent.join(
        path.file_name()
            .ok_or_else(|| error("LLVM SDK output leaf is absent"))?,
    );
    let compiler = compiler_root(&request.drivers)?;
    let sdk = checked_directory(&request.sdk_root, "LLVM SDK input")?;
    let preparation = validate_preparation(&request.preparation)?;
    for input in [
        &sdk,
        &compiler,
        &preparation.source,
        &preparation.engine,
        &preparation.helpers,
        request.source.original_source_root(),
    ] {
        if output.starts_with(input) || input.starts_with(&output) {
            return Err(error("LLVM SDK proof output overlaps an input"));
        }
    }
    fs::create_dir(&output)
        .map_err(|_| error("LLVM SDK proof output exists or cannot be reserved"))?;
    checked_directory(&output, "LLVM SDK reserved output")
}

fn proof_record(
    request: &LlvmSdkLinkRequest,
    plan: &LlvmSdkApplicationPlan,
    relocated: &RelocatedSdk,
    receipt_present: bool,
) -> Result<LinkRecord, ContractError> {
    let root = &request.output_root;
    let top = BTreeSet::from(["links", "relocated-sdk", "tmp", "receipt", INVENTORY]);
    require_names(root, &top, &[INVENTORY])?;
    if receipt_present {
        require_names(
            &root.join("receipt"),
            &BTreeSet::from([RECEIPT]),
            &[RECEIPT],
        )?;
    } else {
        require_names(&root.join("receipt"), &BTreeSet::new(), &[])?;
    }
    require_names(&root.join("tmp"), &BTreeSet::new(), &[])?;
    require_names(
        &root.join("links"),
        &BTreeSet::from([
            "original-c.elf",
            "relocated-c.elf",
            "original-cxx.elf",
            "relocated-cxx.elf",
            "original-c.map",
            "relocated-c.map",
            "original-cxx.map",
            "relocated-cxx.map",
            "reports",
        ]),
        &[],
    )?;
    let loaded = load_bound_sdk_consumer_contract(
        request.source.materialized_source_root(),
        Path::new(
            request
                .source
                .source_profile()
                .sdk_consumer_contract
                .as_deref()
                .ok_or_else(|| error("LLVM SDK contract is absent"))?,
        ),
        request.source.source_profile(),
    )
    .map_err(|_| error("LLVM SDK link contract changed"))?;
    let inventory_bytes =
        read_regular_bounded(&root.join(INVENTORY), "LLVM SDK inventory", MAX_MAP_BYTES)?;
    if inventory_bytes != encode_inventory(&relocated.inventory)? {
        return Err(error("LLVM SDK retained inventory changed"));
    }
    let mut members = BTreeMap::from([(INVENTORY.into(), member(&inventory_bytes))]);
    let mut report_names = BTreeSet::new();
    for (batch, (phase, language, probe)) in [
        (
            CompatibilityPhase::StandaloneC,
            "c",
            &loaded.contract.native_sdk_link_probes.c,
        ),
        (
            CompatibilityPhase::StandaloneCxx,
            "cxx",
            &loaded.contract.native_sdk_link_probes.cxx,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        verify_language(
            request,
            plan,
            relocated,
            (batch, phase, language),
            probe,
            &mut members,
            &mut report_names,
        )?;
    }
    let report_refs = report_names.iter().map(String::as_str).collect();
    let report_publications = report_names.iter().map(String::as_str).collect::<Vec<_>>();
    require_names(
        &root.join("links/reports"),
        &report_refs,
        &report_publications,
    )?;
    Ok(LinkRecord {
        schema: "aros-llvm-sdk-link-receipt-v1".into(),
        assurance: "local-links-not-generation-authentication-or-release-admission".into(),
        identity: request.source.identity().clone(),
        compiler: request.drivers.compiler().clone(),
        compiler_tree_sha256: request.drivers.compiler_tree_sha256().clone(),
        sdk_inventory_sha256: relocated.inventory_sha256.clone(),
        sdk_entries: relocated.inventory.len(),
        retained_tree_sha256: proof_snapshot(root)?.payload_digest_excluding(Some("receipt")),
        members,
    })
}

fn verify_language(
    request: &LlvmSdkLinkRequest,
    plan: &LlvmSdkApplicationPlan,
    relocated: &RelocatedSdk,
    selection: (usize, CompatibilityPhase, &str),
    probe: &aros_common::native_consumer_contract::NativeSdkLinkProbe,
    members: &mut BTreeMap<String, Member>,
    report_names: &mut BTreeSet<String>,
) -> Result<(), ContractError> {
    let (batch, phase, language) = selection;
    let root = &request.output_root;
    let paths = ProbeReportPaths::new(&root.join("links/reports"), phase, 2);
    let report_bytes = retain_member(root, &paths.report, canonical::MAX_DOCUMENT_BYTES, members)?;
    report_names.insert(file_name(&paths.report)?);
    let report = CompatibilityProbeReport::parse(&report_bytes)?;
    let resolved = super::environment::resolve(&environment(&root.join("tmp"))?)?;
    if report.phase != phase
        || report.commands.len() != 2
        || report.engine_api_version != request.preparation.engine_api_version
        || report.engine_sha256 != request.preparation.engine_sha256
        || report.source_tree_sha256 != request.preparation.source_tree_sha256
        || report.environment_sha256 != super::environment::identity(&resolved)?
        || !report.host_tools.is_empty()
        || report.helpers
            != request
                .preparation
                .helpers
                .iter()
                .map(|(name, h)| {
                    (
                        name.clone(),
                        super::CompatibilityHelperReport {
                            sha256: h.sha256.clone(),
                            size: h.size,
                        },
                    )
                })
                .collect()
    {
        return Err(error("LLVM SDK retained phase selection disagrees"));
    }
    let compiler = compiler_root(&request.drivers)?;
    let fixture = request
        .source
        .materialized_source_root()
        .join(&probe.source);
    let mut original = None;
    for (index, (label, sdk)) in [
        ("original", &relocated.original),
        ("relocated", &relocated.relocated),
    ]
    .into_iter()
    .enumerate()
    {
        let command = &plan.commands()[batch * 2 + index];
        let evidence = &report.commands[index];
        let driver = if batch == 0 {
            request.drivers.clang_identity()
        } else {
            request.drivers.clangxx_identity()
        };
        if evidence.program_sha256 != *driver.file_sha256()
            || evidence.command_sha256 != command_identity(&command.program, &command.arguments)?
        {
            return Err(error(
                "LLVM SDK retained invocation differs from its selected command",
            ));
        }
        let stdout = retain_member(
            root,
            &paths.commands[index].stdout,
            super::MAX_RENDERED_LOG_BYTES,
            members,
        )?;
        let stderr = retain_member(
            root,
            &paths.commands[index].stderr,
            super::MAX_RENDERED_LOG_BYTES,
            members,
        )?;
        report_names.insert(file_name(&paths.commands[index].stdout)?);
        report_names.insert(file_name(&paths.commands[index].stderr)?);
        if sha256_bytes(&stdout) != evidence.stdout_sha256
            || sha256_bytes(&stderr) != evidence.stderr_sha256
        {
            return Err(error(
                "LLVM SDK retained logs disagree with their phase report",
            ));
        }
        let name = format!("{label}-{language}");
        let bytes = retain_member(
            root,
            &root.join(format!("links/{name}.elf")),
            MAX_ELF_BYTES,
            members,
        )?;
        plan.verify_application_elf(&bytes)?;
        if let Some(before) = &original {
            if before != &bytes {
                return Err(error("LLVM SDK application differs after relocation"));
            }
        } else {
            original = Some(bytes);
        }
        let map = retain_member(
            root,
            &root.join(format!("links/{name}.map")),
            MAX_MAP_BYTES,
            members,
        )?;
        llvm_sdk_link_paths::validate_link_paths(
            &stdout,
            &map,
            sdk,
            &compiler,
            &root.join("tmp"),
            &relocated.original,
            (label == "relocated", &probe.libraries, &fixture),
        )?;
    }
    Ok(())
}

fn compiler_root(drivers: &LlvmSdkDriverBinding) -> Result<PathBuf, ContractError> {
    drivers
        .clang_path()
        .parent()
        .and_then(Path::parent)
        .map(Path::to_owned)
        .ok_or_else(|| error("LLVM SDK compiler root is absent"))
}

fn environment(temporary: &Path) -> Result<CompatibilityEnvironment, ContractError> {
    let tmp = temporary
        .to_str()
        .ok_or_else(|| error("LLVM SDK temporary root is not UTF-8"))?;
    Ok(CompatibilityEnvironment::Poisoned {
        variables: BTreeMap::from([
            ("PATH".into(), "/nonexistent".into()),
            ("LANG".into(), "C".into()),
            ("LC_ALL".into(), "C".into()),
            ("TMPDIR".into(), tmp.into()),
        ]),
    })
}

fn proof_snapshot(root: &Path) -> Result<TreeContentCas, ContractError> {
    measure_tree_content_cas_bounded(root, PROOF_LIMITS)
        .map_err(|_| error("LLVM SDK proof exceeds its bounded safe tree policy"))
}
fn require_names(
    root: &Path,
    expected: &BTreeSet<&str>,
    publications: &[&str],
) -> Result<(), ContractError> {
    checked_directory(root, "LLVM SDK proof directory")?;
    let mut expected = expected
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    for name in publications {
        let journal = aros_common::publication_journal_path(&root.join(name), "file")
            .map_err(|_| error("cannot derive LLVM SDK evidence publication journal"))?;
        let lock = aros_common::publication_journal_lock_path(&journal)
            .map_err(|_| error("cannot derive LLVM SDK evidence publication lock"))?;
        read_regular_bounded(&lock, "empty LLVM SDK evidence publication lock", 0)?;
        expected.insert(file_name(&lock)?);
    }
    let entries = aros_common::directory_entry_names_nofollow_bounded(root, expected.len() + 1)
        .map_err(|_| error("LLVM SDK proof directory cannot be bounded"))?;
    let observed = entries
        .iter()
        .map(|name| {
            name.to_str()
                .ok_or_else(|| error("LLVM SDK proof member is not UTF-8"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if observed != expected.iter().map(String::as_str).collect() {
        return Err(error(
            "LLVM SDK proof has missing or extra retained members",
        ));
    }
    Ok(())
}
fn retain_member(
    root: &Path,
    path: &Path,
    limit: usize,
    members: &mut BTreeMap<String, Member>,
) -> Result<Vec<u8>, ContractError> {
    let bytes = read_regular_bounded(path, "LLVM SDK retained member", limit)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| error("LLVM SDK evidence escaped its root"))?
        .to_str()
        .ok_or_else(|| error("LLVM SDK evidence path is not UTF-8"))?;
    if members.insert(relative.into(), member(&bytes)).is_some() {
        return Err(error("LLVM SDK retained member was selected twice"));
    }
    Ok(bytes)
}
fn file_name(path: &Path) -> Result<String, ContractError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| error("LLVM SDK retained log name is not UTF-8"))
}
fn member(bytes: &[u8]) -> Member {
    Member {
        sha256: sha256_bytes(bytes),
        size: bytes.len() as u64,
    }
}
fn encode(record: &LinkRecord) -> Result<Vec<u8>, ContractError> {
    canonical::bytes(
        &serde_json::to_value(record).map_err(|_| error("cannot encode LLVM SDK link receipt"))?,
    )
}
fn error(message: impl Into<String>) -> ContractError {
    ContractError::compatibility(message)
}
