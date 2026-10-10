//! Bounded compiler-family evidence claims, independently versioned from v1.
//!
//! This pure contract binds claims to exact input/index bytes and a verifier
//! policy. It does not read reports or packages, execute compilers, verify
//! signatures, or admit publication/recovery. Report-byte read-back and external
//! cryptographic verification remain separate mandatory boundaries. The
//! attestation claim names the pre-attestation checksum manifest whose entries
//! selected the attested files; it does not claim that the manifest file itself
//! was an attestation subject.

use std::collections::{BTreeMap, BTreeSet};

use aros_common::{sha256_bytes, ArosCompilerIdentity, Sha256Digest};
use serde::{Deserialize, Serialize};

use crate::qualification_evidence::{
    canonical_https_url, canonical_repository, safe_segment, safe_signer, safe_workflow_path,
    validate_policy, EvidenceCoverage, EvidencePolicy,
};
use crate::recipe::GitObjectId;
use crate::release_index_v2::NativeReleaseIndexV2;
use crate::release_inputs::ReleaseInputs;
use crate::ContractError;

/// Closed compiler-family evidence schema; never accepted by the v1 parser.
pub const QUALIFICATION_EVIDENCE_V2_SCHEMA: &str = "aros-toolchain-qualification-evidence-v2";
const MAX_BYTES: usize = 16 * 1024 * 1024;
// Release inputs admit at most 32 groups, each with 128 profiles on three hosts.
const MAX_LANES: usize = 32 * 128 * 3;

/// One closed, bounded collection of qualification claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationEvidenceV2 {
    /// Exact v2 schema selector.
    pub schema: String,
    /// Nonzero evidence creation epoch.
    pub created_at: u64,
    /// Expiry epoch strictly later than creation.
    pub expires_at: u64,
    /// Immutable producer run and annotated tag claims.
    pub source_run: SourceRunIdentityV2,
    /// Exact release/input/index/checksum identities.
    pub release: ReleaseEvidenceV2,
    /// Claims supplied by a separate cryptographic verifier.
    pub attestation: AttestationClaimV2,
    /// Canonical asset-sorted report claims for input-derived lanes.
    pub lanes: Vec<QualificationLaneV2>,
    /// Diagnostic subsets never qualify a complete release candidate.
    pub coverage: EvidenceCoverage,
}

/// Run identity without a single global source commit: sources belong to groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRunIdentityV2 {
    /// Canonical credential-free HTTPS repository identity.
    pub repository: String,
    /// Repository-relative workflow file.
    pub workflow: String,
    /// Nonzero provider run identifier.
    pub run_id: u64,
    /// Exact nonzero provider attempt; a rerun is a different execution.
    pub run_attempt: u64,
    /// Exact producer revision selected by the run.
    pub producer_commit: GitObjectId,
    /// Immutable tag name; validated against observed Git state elsewhere.
    pub source_tag: String,
    /// Annotated tag object, not its peeled revision.
    pub tag_object: GitObjectId,
}

/// Release claims shared across the compiler-family input groups.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEvidenceV2 {
    /// Immutable candidate identifier.
    pub release_id: String,
    /// Canonical credential-free HTTPS download root.
    pub base_url: String,
    /// Digest of exact bound collection bytes.
    pub inputs_sha256: Sha256Digest,
    /// Digest of exact serialized index bytes, not reconstructed JSON.
    pub release_index_sha256: Sha256Digest,
    /// Digest of the pre-attestation checksum manifest used to select
    /// individual files for external attestation. The manifest itself is not
    /// thereby claimed to be an attestation subject.
    pub pre_attestation_checksums_sha256: Sha256Digest,
    /// Digest of the final checksum document supplied by the caller, including
    /// the provenance bundle when it is part of the final inventory.
    pub checksums_sha256: Sha256Digest,
    /// Digest of provenance document supplied by the caller.
    pub provenance_sha256: Sha256Digest,
    /// Shared producer Git revision.
    pub producer_commit: GitObjectId,
    /// Shared tools Git revision.
    pub tools_commit: GitObjectId,
}

/// Attestation identity claims returned by a separate cryptographic verifier.
///
/// The V2 claim identifies the pre-attestation manifest by its file digest.
/// That manifest selects individual attested files; the manifest itself is not
/// an attestation subject. The later final checksum document has a separate
/// identity. Parsing this claim does not authenticate a signature or verify
/// any subject bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationClaimV2 {
    /// Credential-free HTTPS repository accepted by the verifier.
    pub repository: String,
    /// Repository-relative workflow accepted by the verifier.
    pub workflow: String,
    /// Approved signer identity, for example the protected workflow principal.
    pub signer: String,
    /// File digest of the pre-attestation manifest selecting the attested files.
    /// The manifest itself is not claimed to be an attestation subject.
    pub subject_manifest_sha256: Sha256Digest,
}

/// Exact compiler/input/package claims for one asset and its four reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationLaneV2 {
    /// Input group supplying the source, recipe, lock and profiles.
    pub group_id: String,
    /// Canonical archive basename derived from the compiler and profile.
    pub asset: String,
    /// Input-derived native host.
    pub host: String,
    /// Producer-owned target profile, not a hard-coded board enumeration.
    pub target_profile: String,
    /// Exact target triple from the profile.
    pub target_triple: String,
    /// Source revision specific to this input group.
    pub source_commit: GitObjectId,
    /// Compiler versions and target ABI bound by the index.
    pub compiler: ArosCompilerIdentity,
    /// Recipe self-digest (distinct from the raw document reference digest).
    pub recipe_sha256: Sha256Digest,
    /// Exact source-lock document digest.
    pub source_lock_sha256: Sha256Digest,
    /// Exact profiles document digest.
    pub profiles_sha256: Sha256Digest,
    /// Archive content digest declared by the bound index.
    pub archive_sha256: Sha256Digest,
    /// Archive length declared by the bound index.
    pub archive_size: u64,
    /// Payload tree digest declared by the bound index.
    pub tree_sha256: Sha256Digest,
    /// Raw digest of A's complete portable finished-package measurement export.
    /// Its full build result and six-phase chain are acquired separately.
    pub build_a_report_sha256: Sha256Digest,
    /// Raw digest of B's complete portable finished-package measurement export.
    pub build_b_report_sha256: Sha256Digest,
    /// Byte-comparison report digest.
    pub comparison_report_sha256: Sha256Digest,
    /// Raw portable compatibility manifest digest, covering the exact receipt,
    /// all phase reports/logs and standalone ELFs, not a receipt self-digest.
    pub compatibility_report_sha256: Sha256Digest,
}

impl QualificationEvidenceV2 {
    /// Parse closed JSON and validate bounded structural claims.
    ///
    /// # Errors
    /// Returns AX0901 for malformed, duplicated, unknown or unsafe claims.
    /// Complete matrix and input identity require [`Self::validate_against_index`].
    pub fn parse(bytes: &[u8]) -> Result<Self, ContractError> {
        if bytes.len() > MAX_BYTES {
            return Err(error(
                "qualification evidence v2 exceeds its metadata limit",
            ));
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| error("qualification evidence is not a closed v2 JSON document"))?;
        value.validate_structure()?;
        Ok(value)
    }

    /// Bind exact index bytes, every selected lane and verifier policy.
    ///
    /// # Errors
    /// Returns AX0901 for expired/future evidence, changed bytes, invalid policy,
    /// cross-group/compiler/source/package swaps, or incomplete candidate lanes.
    /// Success validates claims only; it is not recovery/publication admission.
    pub fn validate_against_index(
        &self,
        index_bytes: &[u8],
        inputs: &ReleaseInputs,
        policy: &EvidencePolicy,
    ) -> Result<(), ContractError> {
        self.validate_structure()?;
        validate_policy(policy)?;
        if self.created_at > policy.now || self.expires_at <= policy.now {
            return Err(error(
                "qualification evidence v2 is not valid at the policy epoch",
            ));
        }
        if self.source_run.repository != policy.source_repository
            || self.source_run.workflow != policy.source_workflow
            || self.attestation.repository != policy.signer_repository
            || self.attestation.workflow != policy.signer_workflow
            || self.attestation.signer != policy.signer
        {
            return Err(error(
                "qualification evidence v2 differs from verifier policy",
            ));
        }
        if self.release.inputs_sha256 != *inputs.collection_sha256()
            || self.release.release_index_sha256 != sha256_bytes(index_bytes)
        {
            return Err(error(
                "qualification evidence v2 input or index bytes differ",
            ));
        }
        let index = NativeReleaseIndexV2::parse(index_bytes, inputs)
            .map_err(|_| error("qualification evidence v2 names an invalid bound index"))?;
        if self.release.release_id != index.release_id()
            || self.release.base_url != index.base_url()
            || self.release.producer_commit != *index.producer_commit()
            || self.release.tools_commit != *index.tools_commit()
        {
            return Err(error(
                "qualification evidence v2 differs from release identity",
            ));
        }
        if self.coverage == EvidenceCoverage::ReleaseCandidate
            && self.lanes.len() != index.artifacts().len()
        {
            return Err(error(
                "qualification evidence v2 lacks the complete input-derived matrix",
            ));
        }
        let artifacts = index
            .artifacts()
            .iter()
            .map(|a| (a.asset(), a))
            .collect::<BTreeMap<_, _>>();
        let groups = inputs
            .groups()
            .iter()
            .map(|g| (g.id(), g))
            .collect::<BTreeMap<_, _>>();
        for lane in &self.lanes {
            let artifact = artifacts
                .get(lane.asset.as_str())
                .ok_or_else(|| error("qualification evidence v2 names an absent asset"))?;
            let group = groups
                .get(lane.group_id.as_str())
                .ok_or_else(|| error("qualification evidence v2 names an absent input group"))?;
            if lane.group_id != artifact.group_id()
                || lane.host != artifact.host()
                || lane.target_profile != artifact.target_profile()
                || lane.target_triple != artifact.target_triple()
                || lane.source_commit != *artifact.source_commit()
                || lane.compiler != *artifact.compiler()
                || lane.recipe_sha256 != *group.recipe().sha256()
                || lane.source_lock_sha256 != *group.source_lock_reference().sha256()
                || lane.profiles_sha256 != *group.profiles_reference().sha256()
                || lane.archive_sha256 != *artifact.sha256()
                || lane.archive_size != artifact.size()
                || lane.tree_sha256 != *artifact.tree_sha256()
            {
                return Err(error(
                    "qualification evidence v2 lane differs from bound inputs or package",
                ));
            }
        }
        Ok(())
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        if self.schema != QUALIFICATION_EVIDENCE_V2_SCHEMA
            || self.created_at == 0
            || self.expires_at <= self.created_at
            || self.source_run.run_id == 0
            || self.source_run.run_attempt == 0
            || !safe_segment(&self.source_run.source_tag)
            || !safe_segment(&self.release.release_id)
            || !safe_workflow_path(&self.source_run.workflow)
            || !safe_workflow_path(&self.attestation.workflow)
            || !safe_signer(&self.attestation.signer)
            || canonical_repository(&self.source_run.repository)? != self.source_run.repository
            || canonical_repository(&self.attestation.repository)? != self.attestation.repository
            || canonical_https_url(&self.release.base_url)? != self.release.base_url
            || self.source_run.producer_commit != self.release.producer_commit
            || self.attestation.subject_manifest_sha256
                != self.release.pre_attestation_checksums_sha256
            || self.lanes.is_empty()
            || self.lanes.len() > MAX_LANES
        {
            return Err(error(
                "qualification evidence v2 has unsafe or inconsistent identities",
            ));
        }
        let mut previous: Option<&str> = None;
        let mut selectors = BTreeSet::new();
        let mut reports = BTreeSet::new();
        for lane in &self.lanes {
            if !safe_segment(&lane.group_id)
                || !safe_asset(&lane.asset)
                || !safe_segment(&lane.host)
                || !safe_segment(&lane.target_profile)
                || !safe_segment(&lane.target_triple)
                || lane.archive_size == 0
                || lane.archive_size > 32 * 1024 * 1024 * 1024
                || previous.is_some_and(|asset| asset >= lane.asset.as_str())
                || !selectors.insert((&lane.group_id, &lane.host, &lane.target_profile))
                || [
                    &lane.build_a_report_sha256,
                    &lane.build_b_report_sha256,
                    &lane.comparison_report_sha256,
                    &lane.compatibility_report_sha256,
                ]
                .into_iter()
                .any(|digest| !reports.insert(digest))
            {
                return Err(error(
                    "qualification evidence v2 lanes or report claims are noncanonical",
                ));
            }
            previous = Some(&lane.asset);
        }
        Ok(())
    }
}

fn error(message: &str) -> ContractError {
    ContractError::recovery(message)
}

fn safe_asset(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
