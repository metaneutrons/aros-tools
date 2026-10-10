//! Complete qualification-claim recording from selected release bytes.
//!
//! This factory acquires the complete release closure itself and records
//! claims derived from its index, inputs and measured bytes. The source-run
//! identity and verifier policy remain external, unverified selections. The
//! result joins claims to bytes; it does not prove execution, authenticate a
//! producer, verify an attestation, or grant qualification/recovery authority.

use aros_common::sha256_bytes;

use crate::native_candidate::{
    readback_release_compatibility_v2, ReleaseCompatibilityReadbackRequestV2,
    ReleaseCompatibilityReadbackV2,
};
use crate::qualification_evidence::{EvidenceCoverage, EvidencePolicy};
use crate::qualification_evidence_v2::{
    AttestationClaimV2, QualificationEvidenceV2, QualificationLaneV2, ReleaseEvidenceV2,
    SourceRunIdentityV2, QUALIFICATION_EVIDENCE_V2_SCHEMA,
};
use crate::release_index_v2::NativeReleaseIndexV2;
use crate::ContractError;

/// External run and verifier selections plus the complete release read-back
/// inputs needed to record a V2 qualification claim.
///
/// The source-run identity and policy are claims selected by the caller. This
/// request does not authenticate the run, its producer, or the verifier.
#[derive(Debug)]
pub struct QualificationRecordingRequestV2<'a, 'e> {
    /// Exact selected canonical V2 index bytes.
    pub index_bytes: &'a [u8],
    /// Complete input-derived A/B, comparison and compatibility selections.
    pub complete: &'a ReleaseCompatibilityReadbackRequestV2<'e>,
    /// Externally selected source-run identity, not authenticated here.
    pub source_run: SourceRunIdentityV2,
    /// Explicit verifier-policy claims, not signature verification.
    pub policy: EvidencePolicy,
    /// Claim creation epoch in Unix seconds.
    pub created_at: u64,
    /// Claim expiry epoch in Unix seconds.
    pub expires_at: u64,
}

/// Opaque qualification claims joined to a complete byte read-back.
///
/// This value has no public constructor or deserializer. It is not execution
/// proof, signature verification, qualification admission, or recovery
/// authority.
#[derive(Debug)]
pub struct RecordedQualificationV2 {
    evidence: QualificationEvidenceV2,
    complete: ReleaseCompatibilityReadbackV2,
}

impl RecordedQualificationV2 {
    /// Generated qualification claims joined to the complete measured bytes.
    #[must_use]
    pub const fn evidence(&self) -> &QualificationEvidenceV2 {
        &self.evidence
    }

    /// Complete build and compatibility byte read-back.
    #[must_use]
    pub const fn complete(&self) -> &ReleaseCompatibilityReadbackV2 {
        &self.complete
    }

    /// Consume the result and return its claims and complete byte read-back.
    #[must_use]
    pub fn into_parts(self) -> (QualificationEvidenceV2, ReleaseCompatibilityReadbackV2) {
        (self.evidence, self.complete)
    }
}

/// Acquire a complete release and record byte-consistent V2 qualification claims.
///
/// Lane claims are derived in canonical index order from the selected input
/// groups, index artifacts, and the opaque complete read-back. Release claims
/// come from the read-back; attestation identity claims come from the supplied
/// policy. No caller-created proof or JSON summary is accepted.
///
/// The operation writes nothing, performs no network access, and runs no
/// compiler. Source-run identity and policy values remain unverified external
/// selections. Success establishes only that claims were joined to the
/// selected bytes; it does not establish execution, job/artifact origin,
/// original runner state, tag identity, or signature authenticity.
///
/// # Errors
/// Returns AX0901 for invalid time ordering, source-producer mismatches,
/// noncanonical selected index bytes, incomplete lane ordering, or invalid
/// generated claims. Propagates complete release acquisition and validation
/// failures without suppressing their diagnostics.
pub fn record_qualification_bytes_v2(
    request: &QualificationRecordingRequestV2<'_, '_>,
) -> Result<RecordedQualificationV2, ContractError> {
    if request.created_at == 0
        || request.expires_at <= request.created_at
        || request.created_at > request.policy.now
        || request.expires_at <= request.policy.now
    {
        return Err(error(
            "qualification recording has invalid creation, expiry or policy time",
        ));
    }

    let packages = &request.complete.builds.packages;
    if request.index_bytes != packages.index.to_json_bytes()? {
        return Err(error(
            "qualification recording index bytes are not the selected canonical index",
        ));
    }
    let index = NativeReleaseIndexV2::parse(request.index_bytes, &packages.inputs)?;
    if index != packages.index {
        return Err(error(
            "qualification recording index differs from the selected release",
        ));
    }
    if &request.source_run.producer_commit != index.producer_commit() {
        return Err(error(
            "qualification recording source producer differs from the selected index",
        ));
    }

    let complete = readback_release_compatibility_v2(request.complete)?;
    let builds = complete.builds();
    let artifacts = index.artifacts();
    if builds.lanes().len() != artifacts.len() || complete.lanes().len() != artifacts.len() {
        return Err(error(
            "qualification recording requires the complete measured lane set",
        ));
    }
    for ((artifact, build), compatibility) in
        artifacts.iter().zip(builds.lanes()).zip(complete.lanes())
    {
        if build.asset() != artifact.asset() || compatibility.asset() != artifact.asset() {
            return Err(error(
                "qualification recording measured lanes differ from canonical index order",
            ));
        }
    }

    let lanes = artifacts
        .iter()
        .zip(builds.lanes())
        .zip(complete.lanes())
        .map(|((artifact, build), compatibility)| {
            let group = packages
                .inputs
                .groups()
                .iter()
                .find(|group| group.id() == artifact.group_id())
                .ok_or_else(|| error("qualification recording lane has no input group"))?;
            let measurements = build.measurement_sha256();
            Ok(QualificationLaneV2 {
                group_id: artifact.group_id().to_owned(),
                asset: artifact.asset().to_owned(),
                host: artifact.host().to_owned(),
                target_profile: artifact.target_profile().to_owned(),
                target_triple: artifact.target_triple().to_owned(),
                source_commit: artifact.source_commit().clone(),
                compiler: artifact.compiler().clone(),
                recipe_sha256: group.recipe().sha256().clone(),
                source_lock_sha256: group.source_lock_reference().sha256().clone(),
                profiles_sha256: group.profiles_reference().sha256().clone(),
                archive_sha256: artifact.sha256().clone(),
                archive_size: artifact.size(),
                tree_sha256: artifact.tree_sha256().clone(),
                build_a_report_sha256: measurements[0].clone(),
                build_b_report_sha256: measurements[1].clone(),
                comparison_report_sha256: build.comparison_sha256().clone(),
                compatibility_report_sha256: compatibility
                    .compatibility()
                    .manifest_sha256()
                    .clone(),
            })
        })
        .collect::<Result<Vec<_>, ContractError>>()?;

    let subject_manifest_sha256 = builds.subject_manifest_sha256().clone();
    let evidence = QualificationEvidenceV2 {
        schema: QUALIFICATION_EVIDENCE_V2_SCHEMA.to_owned(),
        created_at: request.created_at,
        expires_at: request.expires_at,
        source_run: request.source_run.clone(),
        release: ReleaseEvidenceV2 {
            release_id: index.release_id().to_owned(),
            base_url: index.base_url().to_owned(),
            inputs_sha256: packages.inputs.collection_sha256().clone(),
            release_index_sha256: sha256_bytes(request.index_bytes),
            pre_attestation_checksums_sha256: subject_manifest_sha256.clone(),
            checksums_sha256: builds.checksums_sha256().clone(),
            provenance_sha256: builds.provenance_sha256().clone(),
            producer_commit: index.producer_commit().clone(),
            tools_commit: index.tools_commit().clone(),
        },
        attestation: AttestationClaimV2 {
            repository: request.policy.signer_repository.clone(),
            workflow: request.policy.signer_workflow.clone(),
            signer: request.policy.signer.clone(),
            subject_manifest_sha256,
        },
        lanes,
        coverage: EvidenceCoverage::ReleaseCandidate,
    };
    evidence.validate_against_index(request.index_bytes, &packages.inputs, &request.policy)?;

    Ok(RecordedQualificationV2 { evidence, complete })
}

fn error(message: &str) -> ContractError {
    ContractError::recovery(message)
}
