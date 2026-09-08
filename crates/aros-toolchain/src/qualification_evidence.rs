//! Closed, offline qualification-evidence binding for compatibility and recovery.
//!
//! A record in this module is an input to native compatibility or recovery
//! policy, never proof by itself that a GitHub attestation is cryptographically
//! valid. The protected workflow obtains and verifies that attestation; this
//! library checks that the verified claims, release index, matrix and reports
//! are complete and mutually consistent before a later M5 operation may use
//! them. It has no network, credential, tag, or publication authority.

use std::collections::BTreeSet;

use aros_common::{parse_credential_free_https_url, sha256_bytes, Sha256Digest};
use serde::{Deserialize, Serialize};

use crate::recipe::GitObjectId;
use crate::release_index::{NativeReleaseIndex, V1_HOSTS, V1_PROFILES};
use crate::{canonical, ContractError};

/// Closed schema name for M5 qualification-evidence records.
pub const QUALIFICATION_EVIDENCE_SCHEMA: &str = "aros-toolchain-qualification-evidence-v1";

/// One bounded qualification-evidence record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationEvidence {
    /// Closed schema name.
    pub schema: String,
    /// Measured evidence creation time as a Unix epoch in seconds.
    pub created_at: u64,
    /// Strictly later expiry time as a Unix epoch in seconds.
    pub expires_at: u64,
    /// Completed producer-run identity that supplied the immutable candidate.
    pub source_run: SourceRunIdentity,
    /// Bound release, index and exact source-input claims.
    pub release: ReleaseEvidence,
    /// Claims returned by a separately verified build attestation.
    pub attestation: AttestationClaim,
    /// Native build, comparison and compatibility reports by host/profile lane.
    pub lanes: Vec<QualificationLane>,
    /// Whether this record covers selected diagnostics or a full release matrix.
    pub coverage: EvidenceCoverage,
}

/// Completed producer-run identity used as an immutable evidence source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRunIdentity {
    /// Credential-free HTTPS producer repository identity.
    pub repository: String,
    /// Repository-relative workflow file that generated the candidate.
    pub workflow: String,
    /// Nonzero immutable provider run identifier.
    pub run_id: u64,
    /// Producer revision selected by this run.
    pub producer_commit: GitObjectId,
    /// AROS source revision selected by this run.
    pub source_commit: GitObjectId,
    /// Existing immutable source tag used for later replay/recovery checks.
    pub source_tag: String,
    /// Annotated Git tag object identity, not merely its peeled commit.
    pub tag_object: GitObjectId,
}

/// Release and immutable-index claims consumed by later M5 operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseEvidence {
    /// Immutable release identifier.
    pub release_id: String,
    /// Credential-free HTTPS archive download root.
    pub base_url: String,
    /// SHA-256 of the exact serialized v1 release index.
    pub release_index_sha256: Sha256Digest,
    /// SHA-256 of the final `SHA256SUMS` document.
    pub checksums_sha256: Sha256Digest,
    /// SHA-256 of the provenance bundle named by the v1 inventory.
    pub provenance_sha256: Sha256Digest,
    /// Producer recipe identity shared by every package manifest.
    pub recipe_sha256: Sha256Digest,
    /// Source-lock identity shared by every package manifest.
    pub source_lock_sha256: Sha256Digest,
    /// Profiles document identity shared by every package manifest.
    pub profiles_sha256: Sha256Digest,
    /// AROS source revision shared by every package manifest.
    pub source_commit: GitObjectId,
    /// Producer revision shared by every package manifest.
    pub producer_commit: GitObjectId,
    /// aros-tools revision shared by every package manifest.
    pub tools_commit: GitObjectId,
}

/// Attestation identity returned by an external cryptographic verifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestationClaim {
    /// Credential-free HTTPS repository accepted by the verifier.
    pub repository: String,
    /// Repository-relative workflow accepted by the verifier.
    pub workflow: String,
    /// Approved signer identity, for example the protected workflow principal.
    pub signer: String,
    /// Digest of the final checksum document accepted as the subject.
    pub subject_sha256: Sha256Digest,
}

/// One complete native qualification result for a host/profile package lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationLane {
    /// Closed v1 host selector.
    pub host: String,
    /// Closed v1 target-profile selector.
    pub target_profile: String,
    /// Target triple bound by the release index.
    pub target_triple: String,
    /// Report from the first independent compiler/package build.
    pub build_a_report_sha256: Sha256Digest,
    /// Report from the second independent compiler/package build.
    pub build_b_report_sha256: Sha256Digest,
    /// Report proving byte comparison of the independent packages.
    pub comparison_report_sha256: Sha256Digest,
    /// Report proving relocation and consumer compatibility.
    pub compatibility_report_sha256: Sha256Digest,
}

/// Evidence coverage that a caller must select explicitly for its operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceCoverage {
    /// One or more bounded diagnostic lanes; never recovery-eligible.
    Diagnostic,
    /// The exact complete v1 matrix; eligible for later policy evaluation only.
    ReleaseCandidate,
}

/// Immutable policy claims a caller expects from qualification evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePolicy {
    /// Expected producer repository.
    pub source_repository: String,
    /// Expected candidate-producing workflow path.
    pub source_workflow: String,
    /// Expected attesting repository.
    pub signer_repository: String,
    /// Expected attesting workflow path.
    pub signer_workflow: String,
    /// Expected verifier-reported signer identity.
    pub signer: String,
    /// Validation time as a Unix epoch in seconds.
    pub now: u64,
}

impl QualificationEvidence {
    /// Parse one bounded closed qualification-evidence document.
    ///
    /// # Errors
    ///
    /// Returns AX0901 for an unknown, malformed, incomplete, expired, or
    /// internally inconsistent record. It neither verifies an attestation nor
    /// reads release files; callers must use [`Self::validate_against_index`]
    /// with separately measured index bytes and an explicit policy.
    pub fn parse(input: &[u8]) -> Result<Self, ContractError> {
        if input.len() > canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::recovery(
                "qualification evidence exceeds the configured document limit",
            ));
        }
        let evidence: Self = serde_json::from_slice(input).map_err(|_| {
            ContractError::recovery("qualification evidence is not a closed v1 JSON document")
        })?;
        evidence.validate_structure()?;
        Ok(evidence)
    }

    /// Bind evidence to exact release-index bytes and an explicit verifier policy.
    ///
    /// The evidence hash is checked before the index is parsed. This rejects a
    /// changed index even if the altered JSON remains syntactically valid. The
    /// caller remains responsible for safely measuring the release's outer
    /// regular files and for obtaining the attestation claims cryptographically.
    ///
    /// # Errors
    ///
    /// Returns AX0901 when the evidence has expired, its policy claims differ,
    /// the index changes, or the index/reports do not name one consistent matrix.
    pub fn validate_against_index(
        &self,
        index_bytes: &[u8],
        policy: &EvidencePolicy,
    ) -> Result<(), ContractError> {
        self.validate_structure()?;
        validate_policy(policy)?;
        if self.expires_at <= policy.now {
            return Err(ContractError::recovery(
                "qualification evidence is expired at the requested validation time",
            ));
        }
        if self.source_run.repository != policy.source_repository
            || self.source_run.workflow != policy.source_workflow
            || self.attestation.repository != policy.signer_repository
            || self.attestation.workflow != policy.signer_workflow
            || self.attestation.signer != policy.signer
        {
            return Err(ContractError::recovery(
                "qualification evidence repository, workflow, or signer claim differs from policy",
            ));
        }
        if sha256_bytes(index_bytes) != self.release.release_index_sha256 {
            return Err(ContractError::recovery(
                "release index bytes differ from the qualification-evidence digest",
            ));
        }
        let index = NativeReleaseIndex::parse(index_bytes).map_err(|_| {
            ContractError::recovery("qualification evidence names an invalid native release index")
        })?;
        self.validate_index_binding(&index)
    }

    fn validate_structure(&self) -> Result<(), ContractError> {
        if self.schema != QUALIFICATION_EVIDENCE_SCHEMA
            || self.created_at == 0
            || self.expires_at <= self.created_at
            || self.source_run.run_id == 0
            || !safe_segment(&self.source_run.source_tag)
            || !safe_segment(&self.release.release_id)
            || !safe_workflow_path(&self.source_run.workflow)
            || !safe_workflow_path(&self.attestation.workflow)
            || !safe_signer(&self.attestation.signer)
        {
            return Err(ContractError::recovery(
                "qualification evidence has an unsupported schema or unsafe identity field",
            ));
        }
        let source_repository = canonical_repository(&self.source_run.repository)?;
        let attestation_repository = canonical_repository(&self.attestation.repository)?;
        let base_url = canonical_https_url(&self.release.base_url)?;
        if source_repository != self.source_run.repository
            || attestation_repository != self.attestation.repository
            || base_url != self.release.base_url.trim_end_matches('/')
            || self.source_run.source_commit != self.release.source_commit
            || self.source_run.producer_commit != self.release.producer_commit
            || self.attestation.subject_sha256 != self.release.checksums_sha256
        {
            return Err(ContractError::recovery(
                "qualification evidence has inconsistent source, signer, or release claims",
            ));
        }
        let expected = matrix_selectors();
        let mut actual = BTreeSet::new();
        let mut report_digests = BTreeSet::new();
        for lane in &self.lanes {
            if !expected.contains(&(lane.host.clone(), lane.target_profile.clone()))
                || lane.target_triple.is_empty()
                || lane.target_triple.len() > 128
                || lane
                    .target_triple
                    .bytes()
                    .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_'))
                || !actual.insert((lane.host.clone(), lane.target_profile.clone()))
                || !insert_report_digests(&mut report_digests, lane)
            {
                return Err(ContractError::recovery(
                    "qualification evidence lanes are duplicated, incomplete, or noncanonical",
                ));
            }
        }
        match self.coverage {
            EvidenceCoverage::Diagnostic if actual.is_empty() => Err(ContractError::recovery(
                "diagnostic qualification evidence must contain at least one lane",
            )),
            EvidenceCoverage::Diagnostic => Ok(()),
            EvidenceCoverage::ReleaseCandidate if actual == expected => Ok(()),
            EvidenceCoverage::ReleaseCandidate => Err(ContractError::recovery(
                "release-candidate evidence does not contain the complete v1 matrix",
            )),
        }
    }

    fn validate_index_binding(&self, index: &NativeReleaseIndex) -> Result<(), ContractError> {
        if index.release_id != self.release.release_id
            || index.base_url != self.release.base_url.trim_end_matches('/')
            || index.source_commit != self.release.source_commit.as_str()
            || index.producer_commit != self.release.producer_commit.as_str()
            || index.tools_commit != self.release.tools_commit.as_str()
        {
            return Err(ContractError::recovery(
                "qualification evidence differs from the measured release-index identity",
            ));
        }
        for lane in &self.lanes {
            let Some(artifact) = index.artifacts.iter().find(|artifact| {
                artifact.host == lane.host && artifact.target_profile == lane.target_profile
            }) else {
                return Err(ContractError::recovery(
                    "qualification evidence lane is absent from the measured release index",
                ));
            };
            if artifact.target_triple != lane.target_triple {
                return Err(ContractError::recovery(
                    "qualification evidence lane target differs from the measured release index",
                ));
            }
        }
        Ok(())
    }
}

fn validate_policy(policy: &EvidencePolicy) -> Result<(), ContractError> {
    let source_repository = canonical_repository(&policy.source_repository)?;
    let signer_repository = canonical_repository(&policy.signer_repository)?;
    if source_repository != policy.source_repository
        || signer_repository != policy.signer_repository
        || !safe_workflow_path(&policy.source_workflow)
        || !safe_workflow_path(&policy.signer_workflow)
        || !safe_signer(&policy.signer)
    {
        return Err(ContractError::recovery(
            "qualification-evidence policy has an unsafe repository, workflow, or signer claim",
        ));
    }
    Ok(())
}

fn canonical_repository(value: &str) -> Result<String, ContractError> {
    let url = parse_credential_free_https_url(value).map_err(|_| {
        ContractError::recovery("qualification evidence repository is not credential-free HTTPS")
    })?;
    if url.path() == "/" || url.path().ends_with('/') {
        return Err(ContractError::recovery(
            "qualification evidence repository must name a canonical repository path",
        ));
    }
    Ok(url.to_string())
}

fn canonical_https_url(value: &str) -> Result<String, ContractError> {
    let trimmed = value.trim_end_matches('/');
    let url = parse_credential_free_https_url(trimmed).map_err(|_| {
        ContractError::recovery("qualification evidence base URL is not credential-free HTTPS")
    })?;
    if url.path() == "/" {
        return Err(ContractError::recovery(
            "qualification evidence base URL must name a release path",
        ));
    }
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

fn safe_workflow_path(value: &str) -> bool {
    value.starts_with(".github/workflows/")
        && value
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension == "yml")
        && value.len() <= 512
        && !value.contains(['\\', '\0'])
        && value.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
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

fn safe_signer(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn matrix_selectors() -> BTreeSet<(String, String)> {
    V1_HOSTS
        .iter()
        .flat_map(|host| {
            V1_PROFILES
                .iter()
                .map(move |profile| ((*host).to_owned(), (*profile).to_owned()))
        })
        .collect()
}

fn insert_report_digests(digests: &mut BTreeSet<Sha256Digest>, lane: &QualificationLane) -> bool {
    [
        &lane.build_a_report_sha256,
        &lane.build_b_report_sha256,
        &lane.comparison_report_sha256,
        &lane.compatibility_report_sha256,
    ]
    .into_iter()
    .all(|digest| digests.insert(digest.clone()))
}

#[cfg(test)]
mod tests {
    use aros_common::{DiagnosticCode, Sha256Digest};

    use super::{
        AttestationClaim, EvidenceCoverage, EvidencePolicy, QualificationEvidence,
        QualificationLane, ReleaseEvidence, SourceRunIdentity, QUALIFICATION_EVIDENCE_SCHEMA,
    };
    use crate::recipe::GitObjectId;
    use crate::release_index::{NativeReleaseArtifact, NativeReleaseIndex, V1_HOSTS, V1_PROFILES};

    fn digest(value: u64) -> Sha256Digest {
        Sha256Digest::parse(&format!("{value:064x}")).unwrap()
    }

    fn git(value: u8) -> GitObjectId {
        GitObjectId::try_from(format!("{value:x}").repeat(40)).unwrap()
    }

    fn index() -> NativeReleaseIndex {
        let artifacts = V1_HOSTS
            .iter()
            .flat_map(|host| {
                V1_PROFILES
                    .iter()
                    .map(move |profile| NativeReleaseArtifact {
                        asset: format!("aros-toolchain-v1-llvm11.0.0-{host}-{profile}.tar.xz"),
                        sha256: "a".repeat(64),
                        size: 1,
                        host: (*host).into(),
                        target_profile: (*profile).into(),
                        target_triple: match *profile {
                            "pc-x86_64" => "x86_64-unknown-aros",
                            "arm-raspi" => "arm-unknown-aros",
                            "rpi-aarch64" => "aarch64-unknown-aros",
                            _ => unreachable!(),
                        }
                        .into(),
                        tree_sha256: "b".repeat(64),
                        llvm_version: "11.0.0".into(),
                        enabled: true,
                        strip_components: 1,
                        required_paths: vec!["bin/clang".into()],
                    })
            })
            .collect();
        NativeReleaseIndex {
            schema: 1,
            release_id: "toolchain-v1-test".into(),
            base_url: "https://example.invalid/toolchains/toolchain-v1-test".into(),
            source_commit: "1".repeat(40),
            producer_commit: "2".repeat(40),
            tools_commit: "3".repeat(40),
            artifacts,
        }
    }

    fn evidence(index_bytes: &[u8], coverage: EvidenceCoverage) -> QualificationEvidence {
        let index = index();
        let lanes = index
            .artifacts
            .iter()
            .enumerate()
            .map(|(offset, artifact)| QualificationLane {
                host: artifact.host.clone(),
                target_profile: artifact.target_profile.clone(),
                target_triple: artifact.target_triple.clone(),
                build_a_report_sha256: digest((offset * 4 + 1) as u64),
                build_b_report_sha256: digest((offset * 4 + 2) as u64),
                comparison_report_sha256: digest((offset * 4 + 3) as u64),
                compatibility_report_sha256: digest((offset * 4 + 4) as u64),
            })
            .collect();
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
                source_tag: "toolchain-v1-test".into(),
                tag_object: git(4),
            },
            release: ReleaseEvidence {
                release_id: "toolchain-v1-test".into(),
                base_url: "https://example.invalid/toolchains/toolchain-v1-test".into(),
                release_index_sha256: aros_common::sha256_bytes(index_bytes),
                checksums_sha256: digest(50),
                provenance_sha256: digest(51),
                recipe_sha256: digest(52),
                source_lock_sha256: digest(53),
                profiles_sha256: digest(54),
                source_commit: git(1),
                producer_commit: git(2),
                tools_commit: git(3),
            },
            attestation: AttestationClaim {
                repository: "https://github.com/metaneutrons/aros-toolchains".into(),
                workflow: ".github/workflows/toolchain-release.yml".into(),
                signer: "github-actions".into(),
                subject_sha256: digest(50),
            },
            lanes,
            coverage,
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

    #[test]
    fn complete_release_candidate_binds_exact_index_and_policy() {
        let index = index();
        let index_bytes = serde_json::to_vec(&index).unwrap();
        let original = evidence(&index_bytes, EvidenceCoverage::ReleaseCandidate);
        let parsed = QualificationEvidence::parse(&serde_json::to_vec(&original).unwrap()).unwrap();
        assert_eq!(parsed, original);
        parsed
            .validate_against_index(&index_bytes, &policy(150))
            .unwrap();
    }

    #[test]
    fn evidence_rejects_tampered_index_expiry_and_signer_claims() {
        let index_bytes = serde_json::to_vec(&index()).unwrap();
        let evidence = evidence(&index_bytes, EvidenceCoverage::ReleaseCandidate);
        let mut changed_index = index();
        changed_index.base_url = "https://example.invalid/toolchains/other-candidate".into();
        let changed_index = serde_json::to_vec(&changed_index).unwrap();
        assert!(crate::release_index::NativeReleaseIndex::parse(&changed_index).is_ok());
        let changed_index_error = evidence
            .validate_against_index(&changed_index, &policy(150))
            .unwrap_err();
        assert_recovery(&changed_index_error);
        let expired_error = evidence
            .validate_against_index(&index_bytes, &policy(200))
            .unwrap_err();
        assert_recovery(&expired_error);
        let mut wrong_signer = policy(150);
        wrong_signer.signer = "other-signer".into();
        let wrong_signer_error = evidence
            .validate_against_index(&index_bytes, &wrong_signer)
            .unwrap_err();
        assert_recovery(&wrong_signer_error);
    }

    #[test]
    fn parser_rejects_unknown_duplicate_and_incomplete_release_coverage() {
        let index_bytes = serde_json::to_vec(&index()).unwrap();
        let evidence = evidence(&index_bytes, EvidenceCoverage::ReleaseCandidate);
        let original = String::from_utf8(serde_json::to_vec(&evidence).unwrap()).unwrap();
        let trailing_error =
            QualificationEvidence::parse(format!("{original} {{}}").as_bytes()).unwrap_err();
        assert_recovery(&trailing_error);
        let duplicate_error = QualificationEvidence::parse(
            original
                .replacen('{', "{\"schema\":\"duplicate\",", 1)
                .as_bytes(),
        )
        .unwrap_err();
        assert_recovery(&duplicate_error);
        let mut incomplete = evidence;
        incomplete.lanes.pop();
        let incomplete_error =
            QualificationEvidence::parse(&serde_json::to_vec(&incomplete).unwrap()).unwrap_err();
        assert_recovery(&incomplete_error);
        incomplete.coverage = EvidenceCoverage::Diagnostic;
        assert!(QualificationEvidence::parse(&serde_json::to_vec(&incomplete).unwrap()).is_ok());
    }

    fn assert_recovery(error: &crate::ContractError) {
        assert_eq!(
            error.diagnostics().diagnostics[0].code,
            DiagnosticCode::ProducerRecovery
        );
    }
}
