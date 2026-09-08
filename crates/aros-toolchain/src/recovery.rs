//! Closed M5 replay and packaging-recovery eligibility policy.
//!
//! This module deliberately has no transport, credential, tag, release, or
//! archive-writing authority. A protected workflow supplies measured bytes,
//! externally verified attestation claims, and its observed remote handoff
//! state. The policy admits only a complete, unexpired qualified candidate;
//! it never turns a partial build or a changed draft into an eligible input.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::path::Path;

use aros_common::{open_regular_file_nofollow, sha256_bytes, sha256_reader, Sha256Digest};
use serde::{Deserialize, Serialize};

use crate::qualification_evidence::{
    AttestationClaim, EvidenceCoverage, EvidencePolicy, QualificationEvidence,
};
use crate::recipe::GitObjectId;
use crate::release_index::NativeReleaseIndex;
use crate::ContractError;

const CHECKSUMS_NAME: &str = "SHA256SUMS";
const INDEX_NAME: &str = "toolchain-index-v1.json";
const PROVENANCE_NAME: &str = "toolchain-provenance.sigstore.json";
const V1_FINAL_ASSET_COUNT: usize = 56;
const V1_CHECKSUM_SUBJECT_COUNT: usize = V1_FINAL_ASSET_COUNT - 1;
const MAX_CHECKSUM_BYTES: usize = 2 * 1024 * 1024;

/// The only recovery actions that may reuse a qualified candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryOperation {
    /// Re-run only a repaired compatibility harness against immutable packages.
    CompatibilityReplay,
    /// Produce fresh release metadata after a packaging-only handoff failure.
    PackagingRecovery,
}

/// The measured stage that failed in the original attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailedStage {
    /// A tools-owned compatibility harness defect occurred after qualification.
    CompatibilityHarness,
    /// Only the packaging/release handoff failed after qualification.
    Packaging,
    /// Upstream configuration failed.
    Configure,
    /// A compiler build failed.
    Compiler,
    /// The independent byte comparison failed.
    Comparison,
    /// A compatibility consumer failed.
    Compatibility,
    /// Provenance or attestation verification failed.
    Attestation,
    /// The failure could not be classified from durable evidence.
    Unknown,
}

/// The observed filesystem type of a downloaded release member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseAssetKind {
    /// A regular file measured by the isolated downloader.
    Regular,
    /// A directory is never a release asset.
    Directory,
    /// A symlink is never a release asset.
    Symlink,
    /// Any other non-regular filesystem entry.
    Other,
}

/// One measured isolated-download release asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseAsset {
    /// UTF-8 asset basename.
    pub name: String,
    /// Observed outer asset type.
    pub kind: ReleaseAssetKind,
    /// SHA-256 of the regular-file bytes.
    pub sha256: Sha256Digest,
    /// Exact regular-file length.
    pub size: u64,
}

/// One observed immutable annotated tag identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedTag {
    /// Tag name.
    pub name: String,
    /// Git tag-object identity, not merely the peeled commit.
    pub tag_object: GitObjectId,
    /// Commit to which that annotated tag peels.
    pub peeled_commit: GitObjectId,
}

/// The observed state of a prospective recovered release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseHandoffState {
    /// No release object exists for the prospective identity.
    Absent,
    /// A draft release exists; this is an interrupted handoff and is rejected.
    Draft,
    /// A published release exists and must never be altered.
    Published,
    /// The caller could not establish the remote state.
    Unknown,
}

/// Explicit fresh handoff identity for a packaging-only recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryHandoff {
    /// New immutable release identity; it must differ from the source candidate.
    pub release_id: String,
    /// Pre-created annotated tag observed immediately before the handoff.
    pub tag: ObservedTag,
    /// Existing remote state for `release_id`.
    pub state: ReleaseHandoffState,
}

/// Complete, already-isolated input to one policy decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    /// Requested bounded recovery operation.
    pub operation: RecoveryOperation,
    /// Measured original failure classification.
    pub failed_stage: FailedStage,
    /// Parsed qualification evidence from the completed producer run.
    pub evidence: QualificationEvidence,
    /// Exact downloaded v1 index bytes.
    pub release_index_bytes: Vec<u8>,
    /// Exact downloaded final checksum document bytes.
    pub checksums_bytes: Vec<u8>,
    /// Every measured outer release asset from the isolated download.
    pub assets: Vec<ReleaseAsset>,
    /// Attestation claim returned by a trusted external verifier.
    pub verified_attestation: Option<AttestationClaim>,
    /// Explicit expected origin/signer policy and validation time.
    pub evidence_policy: EvidencePolicy,
    /// Re-observed immutable tag for the source qualification run.
    pub source_tag: ObservedTag,
    /// Required only for a packaging recovery; replay has no release authority.
    pub handoff: Option<RecoveryHandoff>,
}

impl RecoveryRequest {
    /// Parse one closed recovery request supplied to a local native boundary.
    ///
    /// Parsing intentionally performs no eligibility decision: callers must
    /// invoke [`evaluate_recovery`] immediately before consuming any retained
    /// package input, so expiry, source-tag and isolated-inventory claims are
    /// checked at the execution boundary rather than at file-read time.
    ///
    /// # Errors
    ///
    /// Returns AX0901 when the input is oversized, malformed, or contains an
    /// unknown field. Eligibility failures are reported by
    /// [`evaluate_recovery`].
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::recovery(
                "recovery request exceeds the configured document limit",
            ));
        }
        serde_json::from_slice(input).map_err(|_| {
            ContractError::recovery("recovery request is not a closed v1 JSON document")
        })
    }
}

/// An eligibility result; executing it requires a separate protected boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryDecision {
    /// Immutable candidate packages may be used for the one requested replay.
    ReplayCompatibility,
    /// Immutable payloads may be re-packaged under a fresh release identity.
    Repackage {
        /// The fresh release identity admitted by this decision.
        release_id: String,
        /// The checked immutable tag to which the later protected handoff binds.
        tag: ObservedTag,
    },
}

/// Measured result of validating one isolated complete recovery inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryInventoryValidation {
    /// Recovery operation admitted immediately before the inventory was read.
    pub decision: RecoveryDecision,
    /// Number of directly measured regular release assets.
    pub asset_count: usize,
}

/// A complete, locally measured final v1 release inventory.
///
/// This is deliberately a read-only observation.  It records no release
/// authority and performs no network, extraction, package, tag, or publication
/// operation.  Both qualification recording and recovery execution use the
/// same measurement boundary, so a recovery request cannot describe a weaker
/// filesystem view than the one that was qualified.
#[derive(Debug, Clone)]
pub struct MeasuredReleaseInventory {
    /// Exact bytes of the final native release index.
    pub release_index_bytes: Vec<u8>,
    /// Exact bytes of the final checksum document.
    pub checksums_bytes: Vec<u8>,
    /// Every direct, regular release asset measured through a no-follow handle.
    pub assets: Vec<ReleaseAsset>,
}

/// Evaluate whether a candidate is eligible for M5 replay or recovery.
///
/// The result is deliberately not an execution capability. The caller must
/// retain and revalidate all named material at the protected execution
/// boundary. Any mismatch is an AX0901 recovery failure; the caller must use
/// a fresh qualification rather than modify evidence or reuse a partial run.
///
/// # Errors
///
/// Returns AX0901 unless the failure class, complete qualified evidence,
/// externally verified attestation, isolated asset inventory, source tag, and
/// any requested fresh packaging handoff remain exact and recovery-eligible.
pub fn evaluate_recovery(request: &RecoveryRequest) -> Result<RecoveryDecision, ContractError> {
    validate_operation(request)?;
    validate_source_tag(&request.source_tag, &request.evidence)?;
    validate_evidence_and_assets(request)?;

    match request.operation {
        RecoveryOperation::CompatibilityReplay => Ok(RecoveryDecision::ReplayCompatibility),
        RecoveryOperation::PackagingRecovery => {
            let handoff = request.handoff.as_ref().ok_or_else(|| {
                ContractError::recovery(
                    "packaging recovery requires one explicit fresh immutable handoff",
                )
            })?;
            validate_handoff(handoff, &request.evidence)?;
            Ok(RecoveryDecision::Repackage {
                release_id: handoff.release_id.clone(),
                tag: handoff.tag.clone(),
            })
        }
    }
}

/// Re-evaluate recovery eligibility and measure every isolated release asset.
///
/// The release directory must contain exactly the regular direct children
/// recorded by the closed recovery request. Each is opened through a
/// no-follow descriptor and compared with the request's measured size and
/// SHA-256 before its package payload can be reused. The final checksum and
/// index files are also required to be byte-identical to the evidence fields,
/// not merely hash-equivalent.
///
/// # Errors
///
/// Returns AX0901 if recovery is not eligible, the directory is unsafe, or a
/// measured asset differs from the isolated inventory. This function has no
/// extraction, package, network, tag, credential, or publication authority.
pub fn validate_recovery_inventory(
    request: &RecoveryRequest,
    release_root: &Path,
) -> Result<RecoveryInventoryValidation, ContractError> {
    let decision = evaluate_recovery(request)?;
    let inventory = measure_complete_release_inventory(release_root)?;
    let expected = measured_assets(&request.assets)?;
    let actual = measured_assets(&inventory.assets)?;
    if actual != expected
        || inventory.release_index_bytes != request.release_index_bytes
        || inventory.checksums_bytes != request.checksums_bytes
    {
        return Err(ContractError::recovery(
            "isolated recovery inventory differs from the measured recovery request",
        ));
    }
    Ok(RecoveryInventoryValidation {
        decision,
        asset_count: actual.len(),
    })
}

/// Measure and validate the complete final v1 inventory at an isolated path.
///
/// The root must be an absolute real directory and must contain exactly the 56
/// direct regular files that are named by its final checksum document: the 55
/// checksummed subjects plus `SHA256SUMS` itself.  Archive identity, every
/// required sidecar, the final index and the provenance bundle are bound before
/// a caller receives the observation.
///
/// # Errors
///
/// Returns AX0901 when the root is unsafe, incomplete, non-final, or changes
/// while it is measured.  It does not attest origin or authorize recovery.
pub fn measure_complete_release_inventory(
    release_root: &Path,
) -> Result<MeasuredReleaseInventory, ContractError> {
    let paths = enumerate_release_assets(release_root)?;
    let checksums_path = paths.get(CHECKSUMS_NAME).ok_or_else(|| {
        ContractError::recovery("isolated recovery release inventory is missing SHA256SUMS")
    })?;
    let checksums_bytes = read_bounded_release_asset(checksums_path)?;
    let expected_checksums = parse_checksums(&checksums_bytes)?;
    let expected_names = expected_checksums
        .keys()
        .cloned()
        .chain(std::iter::once(CHECKSUMS_NAME.into()))
        .collect::<BTreeSet<_>>();
    if expected_checksums.len() != V1_CHECKSUM_SUBJECT_COUNT
        || paths.len() != V1_FINAL_ASSET_COUNT
        || paths.keys().cloned().collect::<BTreeSet<_>>() != expected_names
    {
        return Err(ContractError::recovery(
            "isolated recovery release inventory is not the exact complete v1 regular-file asset set",
        ));
    }

    let mut assets = Vec::with_capacity(paths.len());
    for (name, path) in &paths {
        let (size, sha256) = measure_release_asset(path)?;
        if let Some(expected) = expected_checksums.get(name) {
            if sha256 != *expected {
                return Err(ContractError::recovery(
                    "isolated recovery release asset differs from the final checksum document",
                ));
            }
        }
        assets.push(ReleaseAsset {
            name: name.clone(),
            kind: ReleaseAssetKind::Regular,
            sha256,
            size,
        });
    }
    let release_index_path = paths.get(INDEX_NAME).ok_or_else(|| {
        ContractError::recovery(
            "isolated recovery release inventory is missing the native release index",
        )
    })?;
    let release_index_bytes = read_bounded_release_asset(release_index_path)?;
    let index = NativeReleaseIndex::parse(&release_index_bytes).map_err(|_| {
        ContractError::recovery("isolated recovery release has an invalid native release index")
    })?;
    validate_index_inventory(&index, &measured_assets(&assets)?)?;
    Ok(MeasuredReleaseInventory {
        release_index_bytes,
        checksums_bytes,
        assets,
    })
}

fn enumerate_release_assets(
    root: &Path,
) -> Result<BTreeMap<String, std::path::PathBuf>, ContractError> {
    if !root.is_absolute() {
        return Err(ContractError::recovery(
            "isolated recovery release directory must be absolute",
        ));
    }
    let metadata = fs::symlink_metadata(root).map_err(|_| {
        ContractError::recovery("isolated recovery release directory is unavailable")
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ContractError::recovery(
            "isolated recovery release directory must be a real directory",
        ));
    }
    let mut actual = BTreeMap::new();
    for entry in fs::read_dir(root)
        .map_err(|_| ContractError::recovery("cannot enumerate isolated recovery release assets"))?
    {
        let entry = entry
            .map_err(|_| ContractError::recovery("cannot read isolated recovery release asset"))?;
        let name = entry.file_name().into_string().map_err(|_| {
            ContractError::recovery("isolated recovery release asset name is not UTF-8")
        })?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
            ContractError::recovery("cannot inspect isolated recovery release asset")
        })?;
        if !safe_asset_name(&name)
            || !metadata.is_file()
            || metadata.file_type().is_symlink()
            || actual.insert(name, entry.path()).is_some()
        {
            return Err(ContractError::recovery(
                "isolated recovery release contains a duplicate or non-regular asset",
            ));
        }
    }
    Ok(actual)
}

fn measure_release_asset(path: &Path) -> Result<(u64, Sha256Digest), ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::recovery("cannot safely open isolated recovery asset"))?;
    let before = file
        .metadata()
        .map_err(|_| ContractError::recovery("cannot inspect isolated recovery asset"))?
        .len();
    let measured = sha256_reader(&mut file)
        .map_err(|_| ContractError::recovery("cannot measure isolated recovery asset"))?;
    let after = file
        .metadata()
        .map_err(|_| ContractError::recovery("cannot re-inspect isolated recovery asset"))?
        .len();
    if before != after || after != measured.size {
        return Err(ContractError::recovery(
            "isolated recovery asset changed while it was measured",
        ));
    }
    Ok((measured.size, measured.digest))
}

fn read_bounded_release_asset(path: &Path) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::recovery("cannot safely open recovery metadata asset"))?;
    let size = file
        .metadata()
        .map_err(|_| ContractError::recovery("cannot inspect recovery metadata asset"))?
        .len();
    if size > MAX_CHECKSUM_BYTES as u64 {
        return Err(ContractError::recovery(
            "recovery metadata asset exceeds the configured document limit",
        ));
    }
    let capacity = usize::try_from(size).map_err(|_| {
        ContractError::recovery("recovery metadata asset exceeds addressable memory")
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)
        .map_err(|_| ContractError::recovery("cannot read recovery metadata asset"))?;
    if u64::try_from(bytes.len()).ok() != Some(size) {
        return Err(ContractError::recovery(
            "recovery metadata asset changed while it was read",
        ));
    }
    Ok(bytes)
}

fn validate_operation(request: &RecoveryRequest) -> Result<(), ContractError> {
    let eligible = matches!(
        (request.operation, request.failed_stage),
        (
            RecoveryOperation::CompatibilityReplay,
            FailedStage::CompatibilityHarness
        ) | (RecoveryOperation::PackagingRecovery, FailedStage::Packaging)
    );
    if !eligible {
        return Err(ContractError::recovery(
            "recovery operation does not match the sole measured failed stage",
        ));
    }
    if request.operation == RecoveryOperation::CompatibilityReplay && request.handoff.is_some() {
        return Err(ContractError::recovery(
            "compatibility replay has no release handoff authority",
        ));
    }
    Ok(())
}

fn validate_source_tag(
    observed: &ObservedTag,
    evidence: &QualificationEvidence,
) -> Result<(), ContractError> {
    if !safe_segment(&observed.name)
        || observed.name != evidence.source_run.source_tag
        || observed.tag_object != evidence.source_run.tag_object
        || observed.peeled_commit != evidence.source_run.producer_commit
    {
        return Err(ContractError::recovery(
            "observed source tag does not match the immutable qualification identity",
        ));
    }
    Ok(())
}

fn validate_evidence_and_assets(request: &RecoveryRequest) -> Result<(), ContractError> {
    if request.evidence.coverage != EvidenceCoverage::ReleaseCandidate {
        return Err(ContractError::recovery(
            "only complete release-candidate qualification evidence is recovery-eligible",
        ));
    }
    request
        .evidence
        .validate_against_index(&request.release_index_bytes, &request.evidence_policy)?;
    if request.verified_attestation.as_ref() != Some(&request.evidence.attestation) {
        return Err(ContractError::recovery(
            "recovery requires the exact externally verified attestation claim",
        ));
    }
    let index = NativeReleaseIndex::parse(&request.release_index_bytes).map_err(|_| {
        ContractError::recovery("recovery input has an invalid measured native release index")
    })?;
    let assets = measured_assets(&request.assets)?;
    if sha256_bytes(&request.release_index_bytes) != request.evidence.release.release_index_sha256
        || sha256_bytes(&request.checksums_bytes) != request.evidence.release.checksums_sha256
    {
        return Err(ContractError::recovery(
            "recovery index or checksum bytes differ from qualification evidence",
        ));
    }
    let checksums_asset = assets.get(CHECKSUMS_NAME).ok_or_else(|| {
        ContractError::recovery("recovery inventory is missing the final checksum document")
    })?;
    if checksums_asset.sha256 != request.evidence.release.checksums_sha256 {
        return Err(ContractError::recovery(
            "recovery checksum asset differs from qualification evidence",
        ));
    }
    let expected_checksums = parse_checksums(&request.checksums_bytes)?;
    let expected_names = expected_checksums
        .keys()
        .cloned()
        .chain(std::iter::once(CHECKSUMS_NAME.into()))
        .collect::<BTreeSet<_>>();
    if expected_checksums.len() != V1_CHECKSUM_SUBJECT_COUNT
        || assets.len() != V1_FINAL_ASSET_COUNT
        || assets.keys().cloned().collect::<BTreeSet<_>>() != expected_names
    {
        return Err(ContractError::recovery(
            "recovery inventory is not the exact complete v1 regular-file asset set",
        ));
    }
    for (name, expected) in &expected_checksums {
        let actual = assets.get(name).ok_or_else(|| {
            ContractError::recovery("recovery inventory lost a checksummed release asset")
        })?;
        if actual.sha256 != *expected {
            return Err(ContractError::recovery(
                "recovery release asset differs from the final checksum document",
            ));
        }
    }
    let index_asset = assets.get(INDEX_NAME).ok_or_else(|| {
        ContractError::recovery("recovery inventory is missing the native release index")
    })?;
    let provenance_asset = assets.get(PROVENANCE_NAME).ok_or_else(|| {
        ContractError::recovery("recovery inventory is missing the provenance bundle")
    })?;
    if index_asset.sha256 != request.evidence.release.release_index_sha256
        || provenance_asset.sha256 != request.evidence.release.provenance_sha256
    {
        return Err(ContractError::recovery(
            "recovery index or provenance asset differs from qualification evidence",
        ));
    }
    validate_index_inventory(&index, &assets)
}

fn validate_index_inventory(
    index: &NativeReleaseIndex,
    assets: &BTreeMap<String, &ReleaseAsset>,
) -> Result<(), ContractError> {
    let index_asset = assets.get(INDEX_NAME).ok_or_else(|| {
        ContractError::recovery("recovery inventory is missing the native release index")
    })?;
    let provenance_asset = assets.get(PROVENANCE_NAME).ok_or_else(|| {
        ContractError::recovery("recovery inventory is missing the provenance bundle")
    })?;
    if index_asset.kind != ReleaseAssetKind::Regular
        || provenance_asset.kind != ReleaseAssetKind::Regular
        || provenance_asset.size == 0
    {
        return Err(ContractError::recovery(
            "recovery inventory contains an invalid index or provenance asset",
        ));
    }
    for artifact in &index.artifacts {
        let archive = assets.get(&artifact.asset).ok_or_else(|| {
            ContractError::recovery("recovery inventory is missing an indexed package archive")
        })?;
        let expected_archive = Sha256Digest::parse(&artifact.sha256).map_err(|_| {
            ContractError::recovery("recovery index has an invalid indexed package digest")
        })?;
        if archive.sha256 != expected_archive || archive.size != artifact.size {
            return Err(ContractError::recovery(
                "recovery package archive differs from its measured release index identity",
            ));
        }
        for suffix in [".manifest.json", ".sha256", ".spdx.json"] {
            if !assets.contains_key(&format!("{}{suffix}", artifact.asset)) {
                return Err(ContractError::recovery(
                    "recovery inventory is missing a required indexed package sidecar",
                ));
            }
        }
    }
    Ok(())
}

fn measured_assets(
    assets: &[ReleaseAsset],
) -> Result<BTreeMap<String, &ReleaseAsset>, ContractError> {
    let mut measured = BTreeMap::new();
    for asset in assets {
        if !safe_asset_name(&asset.name)
            || asset.kind != ReleaseAssetKind::Regular
            || asset.size == 0
            || measured.insert(asset.name.clone(), asset).is_some()
        {
            return Err(ContractError::recovery(
                "recovery download contains a duplicate, unsafe, non-regular, or empty release asset",
            ));
        }
    }
    Ok(measured)
}

fn parse_checksums(input: &[u8]) -> Result<BTreeMap<String, Sha256Digest>, ContractError> {
    if input.is_empty() || input.len() > MAX_CHECKSUM_BYTES || !input.ends_with(b"\n") {
        return Err(ContractError::recovery(
            "recovery checksum document is empty, oversized, or not newline-terminated",
        ));
    }
    let input = std::str::from_utf8(input)
        .map_err(|_| ContractError::recovery("recovery checksum document is not valid UTF-8"))?;
    let mut checksums = BTreeMap::new();
    for line in input.lines() {
        let Some((digest, name)) = line.split_once("  ") else {
            return Err(ContractError::recovery(
                "recovery checksum document has a noncanonical record",
            ));
        };
        if line.matches("  ").count() != 1 || !safe_asset_name(name) || name == CHECKSUMS_NAME {
            return Err(ContractError::recovery(
                "recovery checksum document contains an unsafe or duplicate asset name",
            ));
        }
        let digest = Sha256Digest::parse(digest).map_err(|_| {
            ContractError::recovery("recovery checksum document contains an invalid SHA-256")
        })?;
        if checksums.insert(name.into(), digest).is_some() {
            return Err(ContractError::recovery(
                "recovery checksum document contains a duplicate asset name",
            ));
        }
    }
    Ok(checksums)
}

fn validate_handoff(
    handoff: &RecoveryHandoff,
    evidence: &QualificationEvidence,
) -> Result<(), ContractError> {
    if handoff.state != ReleaseHandoffState::Absent
        || !safe_segment(&handoff.release_id)
        || handoff.release_id == evidence.release.release_id
        || !safe_segment(&handoff.tag.name)
        || handoff.tag.name == evidence.source_run.source_tag
        || handoff.tag.tag_object == evidence.source_run.tag_object
        || handoff.tag.peeled_commit != evidence.source_run.producer_commit
    {
        return Err(ContractError::recovery(
            "recovery handoff would conflict with an existing draft, release, or immutable source tag",
        ));
    }
    Ok(())
}

fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn safe_asset_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.contains(['/', '\\', '\0'])
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use aros_common::{sha256_bytes, DiagnosticCode, Sha256Digest};
    use serde_json::{json, Map};

    use super::{
        evaluate_recovery, AttestationClaim, EvidenceCoverage, EvidencePolicy, FailedStage,
        GitObjectId, NativeReleaseIndex, ObservedTag, QualificationEvidence, RecoveryHandoff,
        RecoveryOperation, RecoveryRequest, ReleaseAsset, ReleaseAssetKind, ReleaseHandoffState,
        CHECKSUMS_NAME, INDEX_NAME, PROVENANCE_NAME,
    };
    use crate::package::{package, PackageRequest};
    use crate::package_verify::{verify, PackageVerificationRequest};
    use crate::profiles::{Profile, Profiles};
    use crate::qualification_evidence::{QualificationLane, QUALIFICATION_EVIDENCE_SCHEMA};
    use crate::qualification_evidence::{ReleaseEvidence, SourceRunIdentity};
    use crate::release_index::{NativeReleaseArtifact, V1_HOSTS, V1_PROFILES};
    use crate::source_lock::SourceLock;
    use crate::Recipe;

    fn digest(value: u64) -> Sha256Digest {
        Sha256Digest::parse(&format!("{value:064x}")).unwrap()
    }

    fn git(value: u8) -> GitObjectId {
        GitObjectId::try_from(format!("{value:x}").repeat(40)).unwrap()
    }

    fn index() -> NativeReleaseIndex {
        let mut artifacts = V1_HOSTS
            .iter()
            .flat_map(|host| {
                V1_PROFILES
                    .iter()
                    .map(move |profile| NativeReleaseArtifact {
                        asset: crate::package::canonical_asset_name("11.0.0", host, profile)
                            .unwrap(),
                        sha256: digest(
                            1 + V1_HOSTS
                                .iter()
                                .position(|candidate| candidate == host)
                                .unwrap() as u64
                                * 3
                                + V1_PROFILES
                                    .iter()
                                    .position(|candidate| candidate == profile)
                                    .unwrap() as u64,
                        )
                        .to_string(),
                        size: 10,
                        host: (*host).into(),
                        target_profile: (*profile).into(),
                        target_triple: match *profile {
                            "pc-x86_64" => "x86_64-unknown-aros",
                            "arm-raspi" => "arm-unknown-aros",
                            "rpi-aarch64" => "aarch64-unknown-aros",
                            _ => unreachable!(),
                        }
                        .into(),
                        tree_sha256: digest(30).to_string(),
                        llvm_version: "11.0.0".into(),
                        enabled: true,
                        strip_components: 1,
                        required_paths: vec!["bin/clang".into()],
                    })
            })
            .collect::<Vec<_>>();
        artifacts.sort_by(|left, right| left.asset.cmp(&right.asset));
        NativeReleaseIndex {
            schema: 1,
            release_id: "toolchain-v1-source".into(),
            base_url: "https://example.invalid/toolchains/toolchain-v1-source".into(),
            source_commit: git(1).into(),
            producer_commit: git(2).into(),
            tools_commit: git(3).into(),
            artifacts,
        }
    }

    fn evidence(
        index_bytes: &[u8],
        checksums: Sha256Digest,
        provenance: Sha256Digest,
    ) -> QualificationEvidence {
        let index = index();
        QualificationEvidence {
            schema: QUALIFICATION_EVIDENCE_SCHEMA.into(),
            created_at: 100,
            expires_at: 200,
            source_run: SourceRunIdentity {
                repository: "https://github.com/metaneutrons/aros-toolchains".into(),
                workflow: ".github/workflows/toolchain-release.yml".into(),
                run_id: 42,
                producer_commit: git(2),
                source_commit: git(1),
                source_tag: "toolchain-v1-source".into(),
                tag_object: git(4),
            },
            release: ReleaseEvidence {
                release_id: index.release_id.clone(),
                base_url: index.base_url.clone(),
                release_index_sha256: sha256_bytes(index_bytes),
                checksums_sha256: checksums.clone(),
                provenance_sha256: provenance,
                recipe_sha256: digest(60),
                source_lock_sha256: digest(61),
                profiles_sha256: digest(62),
                source_commit: git(1),
                producer_commit: git(2),
                tools_commit: git(3),
            },
            attestation: AttestationClaim {
                repository: "https://github.com/metaneutrons/aros-toolchains".into(),
                workflow: ".github/workflows/toolchain-release.yml".into(),
                signer: "github-actions".into(),
                subject_sha256: checksums,
            },
            lanes: index
                .artifacts
                .iter()
                .enumerate()
                .map(|(offset, artifact)| QualificationLane {
                    host: artifact.host.clone(),
                    target_profile: artifact.target_profile.clone(),
                    target_triple: artifact.target_triple.clone(),
                    build_a_report_sha256: digest((offset * 4 + 1) as u64 + 100),
                    build_b_report_sha256: digest((offset * 4 + 2) as u64 + 100),
                    comparison_report_sha256: digest((offset * 4 + 3) as u64 + 100),
                    compatibility_report_sha256: digest((offset * 4 + 4) as u64 + 100),
                })
                .collect(),
            coverage: EvidenceCoverage::ReleaseCandidate,
        }
    }

    fn policy(now: u64) -> EvidencePolicy {
        EvidencePolicy {
            source_repository: "https://github.com/metaneutrons/aros-toolchains".into(),
            source_workflow: ".github/workflows/toolchain-release.yml".into(),
            signer_repository: "https://github.com/metaneutrons/aros-toolchains".into(),
            signer_workflow: ".github/workflows/toolchain-release.yml".into(),
            signer: "github-actions".into(),
            now,
        }
    }

    fn source_tag() -> ObservedTag {
        ObservedTag {
            name: "toolchain-v1-source".into(),
            tag_object: git(4),
            peeled_commit: git(2),
        }
    }

    fn handoff(state: ReleaseHandoffState) -> RecoveryHandoff {
        RecoveryHandoff {
            release_id: "toolchain-v1-recovered".into(),
            tag: ObservedTag {
                name: "toolchain-v1-recovered".into(),
                tag_object: git(5),
                peeled_commit: git(2),
            },
            state,
        }
    }

    fn request(operation: RecoveryOperation, failed_stage: FailedStage) -> RecoveryRequest {
        request_for_index(&index(), operation, failed_stage)
    }

    fn request_for_index(
        index: &NativeReleaseIndex,
        operation: RecoveryOperation,
        failed_stage: FailedStage,
    ) -> RecoveryRequest {
        let index_bytes = serde_json::to_vec(&index).unwrap();
        let mut files = index
            .artifacts
            .iter()
            .flat_map(|artifact| {
                let archive_digest = Sha256Digest::parse(&artifact.sha256).unwrap();
                [
                    (artifact.asset.clone(), archive_digest, artifact.size),
                    (format!("{}.manifest.json", artifact.asset), digest(200), 11),
                    (format!("{}.sha256", artifact.asset), digest(201), 12),
                    (format!("{}.spdx.json", artifact.asset), digest(202), 13),
                ]
            })
            .collect::<Vec<_>>();
        files.extend([
            ("profiles-v1.json".into(), digest(203), 14),
            ("source-lock-v1.json".into(), digest(204), 15),
            ("toolchain-manifest-v1.schema.json".into(), digest(205), 16),
            ("toolchain-recipe-v2.json".into(), digest(206), 17),
            ("tree-digest-v1.fixture.json".into(), digest(207), 18),
        ]);
        assert_eq!(files.len(), 53);
        let index_digest = sha256_bytes(&index_bytes);
        files.push((INDEX_NAME.into(), index_digest, index_bytes.len() as u64));
        let provenance = digest(208);
        files.push((PROVENANCE_NAME.into(), provenance.clone(), 19));
        assert_eq!(files.len(), 55);
        files.sort_by(|left, right| left.0.cmp(&right.0));
        let checksums_bytes = files
            .iter()
            .fold(String::new(), |mut output, (name, digest, _)| {
                output.push_str(digest.as_str());
                output.push_str("  ");
                output.push_str(name);
                output.push('\n');
                output
            })
            .into_bytes();
        let checksums_digest = sha256_bytes(&checksums_bytes);
        let mut assets = files
            .into_iter()
            .map(|(name, sha256, size)| ReleaseAsset {
                name,
                kind: ReleaseAssetKind::Regular,
                sha256,
                size,
            })
            .collect::<Vec<_>>();
        assets.push(ReleaseAsset {
            name: CHECKSUMS_NAME.into(),
            kind: ReleaseAssetKind::Regular,
            sha256: checksums_digest.clone(),
            size: checksums_bytes.len() as u64,
        });
        let evidence = evidence(&index_bytes, checksums_digest, provenance);
        RecoveryRequest {
            operation,
            failed_stage,
            release_index_bytes: index_bytes,
            checksums_bytes,
            verified_attestation: Some(evidence.attestation.clone()),
            evidence,
            assets,
            evidence_policy: policy(150),
            source_tag: source_tag(),
            handoff: (operation == RecoveryOperation::PackagingRecovery)
                .then(|| handoff(ReleaseHandoffState::Absent)),
        }
    }

    #[test]
    fn admits_only_the_two_bound_recovery_actions() {
        let replay = request(
            RecoveryOperation::CompatibilityReplay,
            FailedStage::CompatibilityHarness,
        );
        assert!(matches!(
            evaluate_recovery(&replay).unwrap(),
            super::RecoveryDecision::ReplayCompatibility
        ));
        let repackage = request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        assert!(matches!(
            evaluate_recovery(&repackage).unwrap(),
            super::RecoveryDecision::Repackage { .. }
        ));
    }

    #[test]
    fn recovery_request_parser_rejects_unknown_fields_before_execution() {
        let original = request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        let bytes = serde_json::to_vec(&original).unwrap();
        assert_eq!(
            RecoveryRequest::parse(&bytes).unwrap().operation,
            original.operation
        );
        let mut changed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        changed["unexpected"] = json!(true);
        assert_recovery(
            &RecoveryRequest::parse(&serde_json::to_vec(&changed).unwrap()).unwrap_err(),
        );
    }

    #[test]
    fn recovery_inventory_validation_refuses_an_incomplete_isolated_release() {
        let temporary = tempfile::tempdir().unwrap();
        let request = request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        assert_recovery(
            &super::validate_recovery_inventory(&request, temporary.path()).unwrap_err(),
        );
    }

    #[test]
    fn complete_isolated_inventory_is_measured_before_recovery_reuses_it() {
        let temporary = tempfile::tempdir().unwrap();
        let index = write_complete_release_fixture(temporary.path());
        let inventory = super::measure_complete_release_inventory(temporary.path()).unwrap();
        assert_eq!(inventory.assets.len(), super::V1_FINAL_ASSET_COUNT);
        let provenance = inventory
            .assets
            .iter()
            .find(|asset| asset.name == PROVENANCE_NAME)
            .unwrap()
            .sha256
            .clone();
        let mut request = request_for_index(
            &index,
            RecoveryOperation::PackagingRecovery,
            FailedStage::Packaging,
        );
        request.release_index_bytes = inventory.release_index_bytes.clone();
        request.checksums_bytes = inventory.checksums_bytes.clone();
        request.assets = inventory.assets;
        request.evidence = evidence(
            &request.release_index_bytes,
            sha256_bytes(&request.checksums_bytes),
            provenance,
        );
        request.verified_attestation = Some(request.evidence.attestation.clone());
        let validation = super::validate_recovery_inventory(&request, temporary.path()).unwrap();
        assert_eq!(validation.asset_count, super::V1_FINAL_ASSET_COUNT);
    }

    fn write_complete_release_fixture(root: &std::path::Path) -> NativeReleaseIndex {
        let mut index = index();
        let mut files = BTreeMap::new();
        for (ordinal, artifact) in index.artifacts.iter_mut().enumerate() {
            let archive = format!("archive-{ordinal}\n").into_bytes();
            artifact.sha256 = sha256_bytes(&archive).to_string();
            artifact.size = archive.len() as u64;
            files.insert(artifact.asset.clone(), archive);
            for (suffix, contents) in [
                (".manifest.json", b"manifest\n".as_slice()),
                (".sha256", b"checksum\n".as_slice()),
                (".spdx.json", b"sbom\n".as_slice()),
            ] {
                files.insert(format!("{}{suffix}", artifact.asset), contents.to_vec());
            }
        }
        for (name, contents) in [
            ("profiles-v1.json", b"profiles\n".as_slice()),
            ("source-lock-v1.json", b"source lock\n".as_slice()),
            ("toolchain-manifest-v1.schema.json", b"schema\n".as_slice()),
            ("toolchain-recipe-v2.json", b"recipe\n".as_slice()),
            ("tree-digest-v1.fixture.json", b"tree\n".as_slice()),
        ] {
            files.insert(name.into(), contents.to_vec());
        }
        files.insert(INDEX_NAME.into(), serde_json::to_vec(&index).unwrap());
        files.insert(PROVENANCE_NAME.into(), b"provenance\n".to_vec());
        assert_eq!(files.len(), super::V1_CHECKSUM_SUBJECT_COUNT);
        let checksums = files
            .iter()
            .fold(String::new(), |mut output, (name, bytes)| {
                output.push_str(&sha256_bytes(bytes).to_string());
                output.push_str("  ");
                output.push_str(name);
                output.push('\n');
                output
            });
        files.insert(CHECKSUMS_NAME.into(), checksums.into_bytes());
        assert_eq!(files.len(), super::V1_FINAL_ASSET_COUNT);
        for (name, bytes) in files {
            std::fs::write(root.join(name), bytes).unwrap();
        }
        index
    }

    #[test]
    fn rejects_non_packaging_failures_and_release_authority_for_replay() {
        let invalid = request(RecoveryOperation::PackagingRecovery, FailedStage::Compiler);
        assert_recovery(&evaluate_recovery(&invalid).unwrap_err());
        let mut replay = request(
            RecoveryOperation::CompatibilityReplay,
            FailedStage::CompatibilityHarness,
        );
        replay.handoff = Some(handoff(ReleaseHandoffState::Absent));
        assert_recovery(&evaluate_recovery(&replay).unwrap_err());
    }

    #[test]
    fn rejects_expired_missing_or_incorrect_attestation_and_source_claims() {
        let mut expired = request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        expired.evidence_policy.now = 200;
        assert_recovery(&evaluate_recovery(&expired).unwrap_err());
        let mut missing = request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        missing.verified_attestation = None;
        assert_recovery(&evaluate_recovery(&missing).unwrap_err());
        let mut wrong_signer =
            request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        wrong_signer.verified_attestation.as_mut().unwrap().signer = "other".into();
        assert_recovery(&evaluate_recovery(&wrong_signer).unwrap_err());
        let mut changed_source =
            request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        changed_source.source_tag.peeled_commit = git(9);
        assert_recovery(&evaluate_recovery(&changed_source).unwrap_err());
    }

    #[test]
    fn rejects_changed_nonregular_or_incomplete_release_assets() {
        let mut changed_checksum =
            request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        changed_checksum.checksums_bytes[0] = b'f';
        assert_recovery(&evaluate_recovery(&changed_checksum).unwrap_err());
        let mut changed = request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        changed.assets[0].sha256 = digest(999);
        assert_recovery(&evaluate_recovery(&changed).unwrap_err());
        let mut nonregular = request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        nonregular.assets[0].kind = ReleaseAssetKind::Symlink;
        assert_recovery(&evaluate_recovery(&nonregular).unwrap_err());
        let mut incomplete = request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        incomplete.assets.pop();
        assert_recovery(&evaluate_recovery(&incomplete).unwrap_err());
    }

    #[test]
    fn rejects_tag_conflicts_and_interrupted_or_existing_handoffs() {
        for state in [
            ReleaseHandoffState::Draft,
            ReleaseHandoffState::Published,
            ReleaseHandoffState::Unknown,
        ] {
            let mut interrupted =
                request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
            interrupted.handoff.as_mut().unwrap().state = state;
            assert_recovery(&evaluate_recovery(&interrupted).unwrap_err());
        }
        let mut tag_conflict =
            request(RecoveryOperation::PackagingRecovery, FailedStage::Packaging);
        tag_conflict.handoff.as_mut().unwrap().tag.peeled_commit = git(9);
        assert_recovery(&evaluate_recovery(&tag_conflict).unwrap_err());
    }

    fn package_inputs() -> (Recipe, SourceLock, Profile) {
        let lock_bytes = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2", "family": "llvm", "version": "11.0.0",
            "sources": [{"component": "llvm", "version": "11.0.0", "purpose": "toolchain-component", "filename": "llvm.tar.xz", "url": "https://example.invalid/llvm.tar.xz", "sha256": "c".repeat(64), "size": 1}],
            "host_python_packages": [{"name": "mako", "version": "1.3.10", "filename": "mako.tar.gz", "url": "https://example.invalid/mako.tar.gz", "sha256": "d".repeat(64), "size": 1, "source_root": "mako", "python_path": "."}]
        }))
        .unwrap();
        let profiles_bytes = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-profiles-v1", "upstream_commit": "1".repeat(40),
            "profiles": [{"name": "pc-x86_64", "configure_target": "pc-x86_64", "upstream_output_target": "pc-x86_64", "target_triple": "x86_64-unknown-aros", "cpu": "x86_64", "platform": "pc", "float_abi": "", "capabilities": ["c"]}]
        }))
        .unwrap();
        let mut value = json!({
            "schema": "aros-toolchain-recipe-v2", "source_commit": "1".repeat(40), "source_tree": "2".repeat(40),
            "producer_commit": "2".repeat(40), "producer_tree": "3".repeat(40), "tools_commit": "3".repeat(40), "tools_tree": "4".repeat(40),
            "source_date_epoch": 946_684_800_u64, "source_lock_sha256": sha256_bytes(&lock_bytes).to_string(), "profiles_sha256": sha256_bytes(&profiles_bytes).to_string(), "patches": []
        });
        value["recipe_sha256"] =
            json!(sha256_bytes(&crate::canonical::bytes(&value).unwrap()).to_string());
        let recipe = Recipe::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
        let lock = SourceLock::parse(&lock_bytes).unwrap();
        let profile = Profiles::parse(&profiles_bytes)
            .unwrap()
            .select("pc-x86_64")
            .unwrap()
            .clone();
        (recipe, lock, profile)
    }

    fn write_candidate(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/clang"), b"recovery fixture\n").unwrap();
    }

    #[test]
    fn repackage_requires_two_independent_verified_outputs_before_success() {
        let temporary = tempfile::tempdir().unwrap();
        let (recipe, source_lock, profile) = package_inputs();
        let original_candidate = temporary.path().join("original-candidate");
        std::fs::create_dir(&original_candidate).unwrap();
        write_candidate(&original_candidate);
        let original = package(&PackageRequest {
            candidate_root: original_candidate,
            output_dir: temporary.path().join("original-package"),
            release_id: "toolchain-v1-source".into(),
            host: "linux-x86_64".into(),
            recipe: recipe.clone(),
            source_lock: source_lock.clone(),
            profile: profile.clone(),
            build_environment: Map::new(),
            forbidden_prefixes: vec![],
        })
        .unwrap();
        let original_verified = verify(&PackageVerificationRequest {
            package_dir: original.output_dir.clone(),
            release_id: "toolchain-v1-source".into(),
            host: "linux-x86_64".into(),
            recipe: recipe.clone(),
            source_lock: source_lock.clone(),
            profile: profile.clone(),
            build_environment: Map::new(),
            forbidden_prefixes: vec![],
        })
        .unwrap();
        let mut qualified = index();
        let lane = qualified
            .artifacts
            .iter_mut()
            .find(|artifact| {
                artifact.host == "linux-x86_64" && artifact.target_profile == "pc-x86_64"
            })
            .unwrap();
        lane.sha256 = original.archive_sha256.to_string();
        lane.size = original.archive_size;
        lane.tree_sha256 = original_verified.manifest.tree_sha256;
        let expected_tree = lane.tree_sha256.clone();
        let recovery = request_for_index(
            &qualified,
            RecoveryOperation::PackagingRecovery,
            FailedStage::Packaging,
        );
        let source_request = PackageVerificationRequest {
            package_dir: original.output_dir,
            release_id: "toolchain-v1-source".into(),
            host: "linux-x86_64".into(),
            recipe: recipe.clone(),
            source_lock: source_lock.clone(),
            profile: profile.clone(),
            build_environment: Map::new(),
            forbidden_prefixes: vec![],
        };
        let from_package = crate::repackage::repackage_verified_package(
            &crate::repackage::VerifiedPackageRepackageRequest {
                recovery: recovery.clone(),
                source: source_request,
                first_extraction_root: temporary.path().join("first-extracted"),
                second_extraction_root: temporary.path().join("second-extracted"),
                first_output_dir: temporary.path().join("first-from-package"),
                second_output_dir: temporary.path().join("second-from-package"),
            },
        )
        .unwrap();
        assert_eq!(
            from_package.repackaged.first_verified.manifest.tree_sha256,
            expected_tree
        );
        assert_eq!(
            from_package.repackaged.first.archive_sha256,
            from_package.repackaged.second.archive_sha256
        );
        let first_candidate = temporary.path().join("first-candidate");
        let second_candidate = temporary.path().join("second-candidate");
        std::fs::create_dir(&first_candidate).unwrap();
        std::fs::create_dir(&second_candidate).unwrap();
        write_candidate(&first_candidate);
        write_candidate(&second_candidate);
        let request = crate::repackage::RepackageRequest {
            recovery,
            first: PackageRequest {
                candidate_root: first_candidate,
                output_dir: temporary.path().join("first-package"),
                release_id: "toolchain-v1-recovered".into(),
                host: "linux-x86_64".into(),
                recipe: recipe.clone(),
                source_lock: source_lock.clone(),
                profile: profile.clone(),
                build_environment: Map::new(),
                forbidden_prefixes: vec![],
            },
            second: PackageRequest {
                candidate_root: second_candidate,
                output_dir: temporary.path().join("second-package"),
                release_id: "toolchain-v1-recovered".into(),
                host: "linux-x86_64".into(),
                recipe,
                source_lock,
                profile,
                build_environment: Map::new(),
                forbidden_prefixes: vec![],
            },
        };
        let output = crate::repackage::repackage(&request).unwrap();
        assert_eq!(
            output.first_verified.manifest.tree_sha256,
            output.second_verified.manifest.tree_sha256
        );
        assert_eq!(output.first_verified.manifest.tree_sha256, expected_tree);
        assert!(output.first.output_dir.exists() && output.second.output_dir.exists());
        let first_archive = std::fs::read(&output.first.archive).unwrap();
        let second_archive = std::fs::read(&output.second.archive).unwrap();
        assert!(crate::repackage::repackage(&request).is_err());
        assert_eq!(std::fs::read(&output.first.archive).unwrap(), first_archive);
        assert_eq!(
            std::fs::read(&output.second.archive).unwrap(),
            second_archive
        );
    }

    fn assert_recovery(error: &crate::ContractError) {
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerRecovery
        );
    }
}
