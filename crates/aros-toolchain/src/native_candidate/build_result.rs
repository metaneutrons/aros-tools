//! Bounded adapter from the local CLI build result to a finished-candidate proof.
//!
//! The selected raw result digest identifies the exact CLI document supplied
//! by the caller. It is not authentication or execution provenance.

use std::path::{Path, PathBuf};

use aros_common::{sha256_bytes, Sha256Digest};
use serde::Deserialize;

use super::{
    read_document, readback_finished_candidate, FinishedCandidateReadback,
    FinishedCandidateRequest, PhaseOutput, PhaseReceipt,
};
use crate::plan::{Executor, Identity};
use crate::profiles::Profile;
use crate::source_lock::SourceLock;
use crate::{ContractError, Recipe};

const RESULT_SCHEMA: &str = "aros-toolchain-result-v1";
const OPERATION: &str = "build";
const QUALIFICATION: &str = "local-only";
const COMMIT_STATE: &str = "committed";
const PRODUCER_CONTRACT_ID: &str = "aros-toolchain-producer-v1";
const PHASES_AND_FINISHED: [&str; 7] = [
    "preflight",
    "environment",
    "configure",
    "compiler",
    "collector",
    "publish",
    "finished-candidate",
];

/// Inputs selected independently of one serialized CLI build result.
///
/// `build_result_sha256` must be retained outside the result document. It
/// selects bytes only; it does not establish who ran or authenticated a build.
#[derive(Debug)]
pub struct FinishedBuildResultRequest<'a> {
    /// Original native work root, not its `native-lifecycle` subdirectory.
    pub work_dir: &'a Path,
    /// Original output root containing the finished `toolchain` directory.
    pub output_dir: &'a Path,
    /// Validated recipe selected independently of the result document.
    pub recipe: &'a Recipe,
    /// Exact recipe-bound compiler-family lock.
    pub source_lock: &'a SourceLock,
    /// Exact recipe-bound selected profile.
    pub profile: &'a Profile,
    /// Independently expected native host name.
    pub host: &'a str,
    /// Exact bounded CLI result document to retain through package commit.
    pub build_result: &'a Path,
    /// Externally selected digest of the exact raw result-document bytes.
    pub build_result_sha256: &'a Sha256Digest,
}

/// Read the CLI result and its complete retained lifecycle chain.
///
/// This is a local consistency adapter. It does not authenticate the CLI,
/// establish that commands executed, or grant release authority. The result
/// file bytes are retained in the returned proof and rechecked before package
/// commit.
///
/// # Errors
/// Rejects a changed/unsafe result file, any closed-schema or evidence mismatch,
/// mismatched selected roots/host, or an inconsistent finished-candidate chain.
pub fn readback_finished_build_result(
    request: &FinishedBuildResultRequest<'_>,
) -> Result<FinishedCandidateReadback, ContractError> {
    let bytes = read_document(request.build_result)?;
    if sha256_bytes(&bytes) != *request.build_result_sha256 {
        return Err(ContractError::state(
            "selected native build result bytes differ from their external digest",
        ));
    }

    let selected = select_result(&bytes, request.output_dir, request.host)?;

    let mut proof = readback_finished_candidate(&FinishedCandidateRequest {
        work_dir: request.work_dir,
        output_dir: request.output_dir,
        recipe: request.recipe,
        source_lock: request.source_lock,
        profile: request.profile,
        identity: &selected.identity,
        phase_receipt_digests: &selected.phase_receipt_digests,
        candidate_receipt_digest: &selected.receipt_sha256,
    })?;

    let publish: PhaseReceipt = serde_json::from_slice(&proof.receipts[5].1)
        .map_err(|_| ContractError::state("validated publish receipt cannot be decoded"))?;
    if selected.outputs != publish.outputs {
        return Err(ContractError::state(
            "native build result outputs differ from the selected publish receipt",
        ));
    }

    proof
        .receipts
        .push((PathBuf::from(request.build_result), bytes));
    proof.revalidate()?;
    Ok(proof)
}

pub(super) struct SelectedResult {
    pub(super) identity: Identity,
    pub(super) phase_receipt_digests: [Sha256Digest; 6],
    pub(super) receipt_sha256: Sha256Digest,
    pub(super) outputs: Vec<PhaseOutput>,
}

/// Pure byte/schema join; it neither reads the original roots nor authenticates execution.
pub(super) fn select_result(
    bytes: &[u8],
    output_dir: &Path,
    host: &str,
) -> Result<SelectedResult, ContractError> {
    if bytes.is_empty() || bytes.len() > crate::canonical::MAX_DOCUMENT_BYTES {
        return Err(ContractError::state(
            "native build result exceeds its byte bound",
        ));
    }
    let result: BuildResultDocument = serde_json::from_slice(bytes).map_err(|_| {
        ContractError::state("native build result violates its closed result-v1 schema")
    })?;
    if result.schema != RESULT_SCHEMA
        || result.operation != OPERATION
        || result.output_root.as_path() != output_dir
        || result.identity.host != host
        || result.qualification != QUALIFICATION
        || result.commit_state != COMMIT_STATE
    {
        return Err(ContractError::state(
            "native build result differs from the selected operation, roots, host or local state",
        ));
    }
    let phases = selected_phase_digests(&result.evidence)?;
    let finished = result.evidence[6]
        .report_sha256
        .clone()
        .ok_or_else(|| ContractError::state("native build result omits its finished digest"))?;
    Ok(SelectedResult {
        identity: selected_identity(result.identity, host)?,
        phase_receipt_digests: phases,
        receipt_sha256: finished,
        outputs: result.outputs,
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildResultDocument {
    schema: String,
    operation: String,
    identity: ResultIdentity,
    output_root: PathBuf,
    outputs: Vec<PhaseOutput>,
    evidence: Vec<BuildEvidence>,
    qualification: String,
    commit_state: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultIdentity {
    recipe_sha256: Sha256Digest,
    source_commit: crate::recipe::GitObjectId,
    producer_commit: crate::recipe::GitObjectId,
    tools_commit: crate::recipe::GitObjectId,
    host: String,
    target_profile: String,
    executor: ResultExecutor,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultExecutor {
    #[serde(deserialize_with = "required_option")]
    contract_id: Option<String>,
    #[serde(deserialize_with = "required_option")]
    contract_sha256: Option<Sha256Digest>,
    #[serde(deserialize_with = "required_option")]
    tools_commit: Option<crate::recipe::GitObjectId>,
    binary_sha256: Sha256Digest,
    #[serde(deserialize_with = "required_option")]
    origin_evidence_sha256: Option<Sha256Digest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildEvidence {
    check: String,
    status: String,
    #[serde(deserialize_with = "required_option")]
    report_sha256: Option<Sha256Digest>,
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn selected_phase_digests(evidence: &[BuildEvidence]) -> Result<[Sha256Digest; 6], ContractError> {
    if evidence.len() != 8 {
        return Err(ContractError::state(
            "native build result must contain the complete ordered evidence set",
        ));
    }
    let mut selected = Vec::with_capacity(6);
    for (index, expected_check) in PHASES_AND_FINISHED.iter().enumerate() {
        let entry = &evidence[index];
        if entry.check != *expected_check || entry.status != "passed" {
            return Err(ContractError::state(
                "native build result has missing, reordered or incomplete lifecycle evidence",
            ));
        }
        let digest = entry.report_sha256.as_ref().ok_or_else(|| {
            ContractError::state("native build result lifecycle evidence omits its report digest")
        })?;
        if index < 6 {
            selected.push(digest.clone());
        }
    }
    let origin = &evidence[7];
    if origin.check != "origin" || origin.status != "not-run" || origin.report_sha256.is_some() {
        return Err(ContractError::state(
            "native build result must leave executor-origin evidence unclaimed",
        ));
    }
    selected.try_into().map_err(|_| {
        ContractError::state("native build result does not select six phase receipt digests")
    })
}

fn selected_identity(
    identity: ResultIdentity,
    expected_host: &str,
) -> Result<Identity, ContractError> {
    if identity.host != expected_host
        || identity.executor.origin_evidence_sha256.is_some()
        || identity.executor.contract_id.as_deref() != Some(PRODUCER_CONTRACT_ID)
        || identity.executor.contract_sha256.is_none()
        || identity.executor.tools_commit.as_ref() != Some(&identity.tools_commit)
    {
        return Err(ContractError::state(
            "native build result identity claims an invalid host, contract or origin",
        ));
    }
    let host = match expected_host {
        "linux-x86_64" => "linux-x86_64",
        "linux-aarch64" => "linux-aarch64",
        "macos-aarch64" => "macos-aarch64",
        "macos-x86_64" => "macos-x86_64",
        _ => return Err(ContractError::state("unsupported native build result host")),
    };
    let executor = identity.executor;
    Ok(Identity {
        recipe_sha256: identity.recipe_sha256,
        source_commit: identity.source_commit,
        producer_commit: identity.producer_commit,
        tools_commit: identity.tools_commit,
        host,
        target_profile: identity.target_profile,
        executor: Executor {
            contract_id: Some(PRODUCER_CONTRACT_ID),
            contract_sha256: executor.contract_sha256,
            tools_commit: executor.tools_commit,
            binary_sha256: executor.binary_sha256,
            origin_evidence_sha256: None,
        },
    })
}

#[cfg(test)]
#[path = "build_result_tests.rs"]
mod tests;
