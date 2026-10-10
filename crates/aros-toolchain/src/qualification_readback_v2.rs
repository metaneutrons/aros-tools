//! Complete qualification-claim byte joins, not authenticated release admission.
//!
//! A valid V2 claim document alone is insufficient: every report digest must
//! match the complete, input-derived build/compatibility closure. This factory
//! performs that acquisition itself; it accepts neither a caller-created proof
//! nor a success-shaped JSON summary. Execution, owning-job/artifact origin,
//! signature verification and observed Git tag identity remain external gates.

use crate::native_candidate::{
    readback_release_compatibility_v2, ReleaseCompatibilityReadbackRequestV2,
    ReleaseCompatibilityReadbackV2,
};
use crate::qualification_evidence::{EvidenceCoverage, EvidencePolicy};
use crate::qualification_evidence_v2::QualificationEvidenceV2;
use crate::release_index_v2::NativeReleaseIndexV2;
use crate::ContractError;

/// Independently selected claim/index bytes and complete evidence selections.
///
/// The caller must externally select/authenticate these inputs and own all
/// files quiescently. This operation cannot turn unauthenticated claims into
/// trusted observations of executions or original runner environments.
#[derive(Debug)]
pub struct QualificationByteReadbackRequestV2<'a, 'e> {
    /// Exact bounded, closed V2 qualification claims.
    pub evidence_bytes: &'a [u8],
    /// Exact selected V2 index bytes, not reconstructed or discovered metadata.
    pub index_bytes: &'a [u8],
    /// Complete input-derived A/B, comparison and compatibility byte closure.
    pub complete: &'a ReleaseCompatibilityReadbackRequestV2<'e>,
    /// Independently selected repository/workflow/signer and current epoch.
    pub policy: &'a EvidencePolicy,
}

/// Opaque complete claim-to-byte join; not publication or recovery authority.
///
/// No public constructor or Deserialize implementation exists. Acquiring this
/// result requires every selected package/report/log/ELF and exact final file
/// inventory. Even authentic-looking, consistent bytes do not prove execution.
#[derive(Debug)]
pub struct QualificationByteReadbackV2 {
    evidence: QualificationEvidenceV2,
    complete: ReleaseCompatibilityReadbackV2,
}

impl QualificationByteReadbackV2 {
    /// Claims joined to all actually acquired report and release bytes.
    #[must_use]
    pub const fn evidence(&self) -> &QualificationEvidenceV2 {
        &self.evidence
    }

    /// Actual complete read-back. It grants no external authentication.
    #[must_use]
    pub const fn complete(&self) -> &ReleaseCompatibilityReadbackV2 {
        &self.complete
    }

    /// Consume the claim join and retain only its non-authenticating byte proof.
    #[must_use]
    pub fn into_complete(self) -> ReleaseCompatibilityReadbackV2 {
        self.complete
    }
}

/// Acquire a complete release and reject claim-only qualification substitutions.
///
/// First binds the exact claim/index/input documents to the independently
/// selected policy and complete release coverage. Then reads all A/B exports,
/// packages, comparisons, compatibility receipts/reports/logs and actual ELFs,
/// joining every claim to raw acquired bytes. Final checksums, provenance bytes
/// and the unchanged pre-attestation subject list must also match.
///
/// The operation writes nothing, runs no compiler and does not open foreign
/// original runner roots. Repeated observations reject observed changes, but
/// do not create an atomic snapshot or an ownership lock against other writers.
///
/// # Errors
/// Returns AX0901 for invalid policy/claims, diagnostic subsets or claim/byte
/// mismatches. Propagates exact lower-layer acquisition errors without masking
/// source, archive, build, comparison or compatibility failures. Success does
/// not verify signatures, execution separation, job/artifact origin or a tag.
pub fn readback_qualification_bytes_v2(
    request: &QualificationByteReadbackRequestV2<'_, '_>,
) -> Result<QualificationByteReadbackV2, ContractError> {
    let packages = &request.complete.builds.packages;
    let evidence = QualificationEvidenceV2::parse(request.evidence_bytes)?;
    evidence.validate_against_index(request.index_bytes, &packages.inputs, request.policy)?;
    if evidence.coverage != EvidenceCoverage::ReleaseCandidate {
        return Err(error(
            "qualification byte read-back requires complete release-candidate coverage",
        ));
    }
    let index = NativeReleaseIndexV2::parse(request.index_bytes, &packages.inputs)?;
    if index != packages.index || request.index_bytes != packages.index.to_json_bytes()? {
        return Err(error(
            "qualification index bytes differ from the complete selected release",
        ));
    }
    let complete = readback_release_compatibility_v2(request.complete)?;
    let builds = complete.builds();
    if evidence.release.checksums_sha256 != *builds.checksums_sha256()
        || evidence.release.pre_attestation_checksums_sha256 != *builds.subject_manifest_sha256()
        || evidence.release.provenance_sha256 != *builds.provenance_sha256()
    {
        return Err(error(
            "qualification release claims differ from measured checksums, subjects or provenance bytes",
        ));
    }
    if evidence.lanes.len() != builds.lanes().len()
        || evidence.lanes.len() != complete.lanes().len()
    {
        return Err(error(
            "qualification claims lack the complete measured lane set",
        ));
    }
    for ((lane, build), compatibility) in evidence
        .lanes
        .iter()
        .zip(builds.lanes())
        .zip(complete.lanes())
    {
        if lane.asset != build.asset()
            || lane.asset != compatibility.asset()
            || lane.build_a_report_sha256 != *build.measurement_sha256()[0]
            || lane.build_b_report_sha256 != *build.measurement_sha256()[1]
            || lane.comparison_report_sha256 != *build.comparison_sha256()
            || lane.compatibility_report_sha256 != *compatibility.compatibility().manifest_sha256()
        {
            return Err(error(
                "qualification report claims differ from the complete measured evidence bytes",
            ));
        }
    }
    Ok(QualificationByteReadbackV2 { evidence, complete })
}

fn error(message: &str) -> ContractError {
    ContractError::recovery(message)
}
