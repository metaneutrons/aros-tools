//! Complete compiler-family recovery checks, not authentication or execution.
//!
//! Recovery must acquire the original complete qualification bytes again. A
//! previous success summary, copied manifest environment, or V1 inventory is
//! insufficient. Provider/tag/signature observations are explicit caller
//! assertions; this local module checks their agreement but cannot authenticate
//! them. Protected callers must verify their origin before using the result.

use aros_common::{sha256_bytes, Sha256Digest};
use serde::{Deserialize, Serialize};

use crate::qualification_evidence::safe_segment;
use crate::qualification_evidence_v2::{AttestationClaimV2, SourceRunIdentityV2};
use crate::qualification_readback_v2::{
    readback_qualification_bytes_v2, QualificationByteReadbackRequestV2,
    QualificationByteReadbackV2,
};
use crate::recovery::{
    FailedStage, ObservedTag, RecoveryDecision, RecoveryHandoff, RecoveryOperation,
    ReleaseHandoffState,
};
use crate::ContractError;

/// Independently versioned closed request; historical V1 is never inferred.
pub const RECOVERY_V2_SCHEMA: &str = "aros-toolchain-recovery-request-v2";
const MAX_BYTES: usize = 1024 * 1024;

/// External observations and operation selection, not an execution capability.
///
/// Every observation must come from the protected verifier, not fields copied
/// from the qualification document. Parsing cannot establish that provenance.
/// Evidence, inventories and environments are deliberately not embedded here:
/// the checker acquires their exact complete byte closure afresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequestV2 {
    /// Exact V2 selector.
    pub schema: String,
    /// Independently selected raw qualification document digest.
    pub qualification_sha256: Sha256Digest,
    /// Explicit bounded recovery operation.
    pub operation: RecoveryOperation,
    /// Sole original failure stage, determined by the protected workflow.
    pub failed_stage: FailedStage,
    /// Fresh provider observation of repository/workflow/run/attempt/revision.
    pub observed_run: SourceRunIdentityV2,
    /// Fresh observation of the original annotated immutable tag.
    pub source_tag: ObservedTag,
    /// Exact identity returned by the external signature/attestation verifier.
    pub observed_attestation: Option<AttestationClaimV2>,
    /// Packaging only: independently observed fresh absent release identity.
    pub handoff: Option<RecoveryHandoff>,
}

impl RecoveryRequestV2 {
    /// Parse bounded closed JSON. It does not decide eligibility or authenticate.
    ///
    /// # Errors
    /// Returns AX0901 for oversized, malformed, unknown or non-V2 metadata.
    pub fn parse(bytes: &[u8]) -> Result<Self, ContractError> {
        if bytes.len() > MAX_BYTES {
            return Err(error("recovery request v2 exceeds its metadata limit"));
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| error("recovery request is not a closed v2 JSON document"))?;
        if value.schema != RECOVERY_V2_SCHEMA {
            return Err(error("recovery request does not select the v2 schema"));
        }
        Ok(value)
    }
}

/// Original complete bytes plus independently selected recovery observations.
#[derive(Debug)]
pub struct RecoveryByteReadbackRequestV2<'a, 'e> {
    /// Closed operation and provider/tag/verifier observations.
    pub recovery: &'a RecoveryRequestV2,
    /// Exact qualification/policy/index and complete original evidence closure.
    pub qualification: &'a QualificationByteReadbackRequestV2<'a, 'e>,
}

/// Opaque complete local recovery check. It grants no execution/publication.
#[derive(Debug)]
pub struct RecoveryByteReadbackV2 {
    decision: RecoveryDecision,
    qualification: QualificationByteReadbackV2,
}

impl RecoveryByteReadbackV2 {
    /// Conditional decision; external origin verification is still mandatory.
    #[must_use]
    pub const fn decision(&self) -> &RecoveryDecision {
        &self.decision
    }

    /// Complete reacquired original qualification bytes, not an auth token.
    #[must_use]
    pub const fn qualification(&self) -> &QualificationByteReadbackV2 {
        &self.qualification
    }

    /// Retain the complete original byte join and non-authorizing decision.
    #[must_use]
    pub fn into_parts(self) -> (RecoveryDecision, QualificationByteReadbackV2) {
        (self.decision, self.qualification)
    }
}

/// Reacquire every original byte and reject mismatched recovery observations.
///
/// Only packaging failures or failures of the compatibility *harness* are
/// eligible. Compilation, comparisons and actual compatibility failures cannot
/// be rescued. Packaging requires an absent, distinct annotated tag/release
/// peeling to the original producer revision; replay has no handoff authority.
/// No file is written, compiler executed, tag created or credential obtained.
///
/// # Errors
/// Returns AX0901 for changed qualification, operation or observations. Complete
/// acquisition propagates its original diagnostics rather than masking source,
/// compiler, package or compatibility errors. Caller-owned files must remain
/// quiescent; repeated read-back is not a concurrent-writer snapshot.
pub fn readback_recovery_bytes_v2(
    request: &RecoveryByteReadbackRequestV2<'_, '_>,
) -> Result<RecoveryByteReadbackV2, ContractError> {
    let recovery = request.recovery;
    if recovery.schema != RECOVERY_V2_SCHEMA
        || recovery.qualification_sha256 != sha256_bytes(request.qualification.evidence_bytes)
    {
        return Err(error(
            "recovery request v2 differs from selected qualification bytes",
        ));
    }
    if !matches!(
        (recovery.operation, recovery.failed_stage),
        (
            RecoveryOperation::CompatibilityReplay,
            FailedStage::CompatibilityHarness
        ) | (RecoveryOperation::PackagingRecovery, FailedStage::Packaging)
    ) {
        return Err(error(
            "recovery operation does not match the sole measured failed stage",
        ));
    }
    let qualification = readback_qualification_bytes_v2(request.qualification)?;
    let evidence = qualification.evidence();
    if recovery.observed_run != evidence.source_run {
        return Err(error(
            "recovery provider observation differs from the exact source run attempt",
        ));
    }
    if recovery.source_tag.name != evidence.source_run.source_tag
        || recovery.source_tag.tag_object != evidence.source_run.tag_object
        || recovery.source_tag.peeled_commit != evidence.source_run.producer_commit
    {
        return Err(error(
            "observed source tag does not match the immutable qualification identity",
        ));
    }
    if recovery.observed_attestation.as_ref() != Some(&evidence.attestation) {
        return Err(error(
            "recovery requires the exact external attestation observation",
        ));
    }
    let decision = match recovery.operation {
        RecoveryOperation::CompatibilityReplay => {
            if recovery.handoff.is_some() {
                return Err(error(
                    "compatibility replay has no release handoff authority",
                ));
            }
            RecoveryDecision::ReplayCompatibility
        }
        RecoveryOperation::PackagingRecovery => {
            let handoff = recovery.handoff.as_ref().ok_or_else(|| {
                error("packaging recovery requires an observed fresh absent handoff")
            })?;
            if handoff.state != ReleaseHandoffState::Absent
                || !safe_segment(&handoff.release_id)
                || handoff.release_id == evidence.release.release_id
                || !safe_segment(&handoff.tag.name)
                || handoff.tag.name == evidence.source_run.source_tag
                || handoff.tag.tag_object == evidence.source_run.tag_object
                || handoff.tag.peeled_commit != evidence.source_run.producer_commit
            {
                return Err(error(
                    "recovery handoff conflicts with the original identity or an existing release",
                ));
            }
            RecoveryDecision::Repackage {
                release_id: handoff.release_id.clone(),
                tag: handoff.tag.clone(),
            }
        }
    };
    Ok(RecoveryByteReadbackV2 {
        decision,
        qualification,
    })
}

fn error(message: &str) -> ContractError {
    ContractError::recovery(message)
}
