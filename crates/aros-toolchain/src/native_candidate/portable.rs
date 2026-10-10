//! Portable byte joins, not a recreation of owning-host proof or authenticated execution.

use std::path::{Path, PathBuf};

use aros_common::{measure_regular_file_bounded, sha256_bytes, Sha256Digest};
use serde::{Deserialize, Serialize};

use super::{
    build_result, expected_record, owned_identity, payload_limits, validate_phase_receipt,
    verify_self_digest, FinishedCandidatePackage, FinishedCandidateReadback,
    FinishedCandidateRequest, FinishedRecord, PHASES, RECEIPT_FILE,
};
use crate::package::PackageFormat;
use crate::package_verify::{verify_with_format, PackageVerificationRequest, VerifiedPackage};
use crate::plan::Identity;
use crate::release_index::{measure_single_package_set, PackageSetComparison};
use crate::{canonical, ContractError};

const SCHEMA: &str = "aros-toolchain-finished-package-measurement-v2";
const MAX_BYTES: usize = 16 * 1024 * 1024;
const RESULT_FILE: &str = "build-result.json";

/// Exact owning-host export bytes, constructed only from live local proofs.
///
/// The export includes the raw build result, six phase receipts, finished
/// record and four package measurements. Hashing these bytes is not provenance.
/// Original absolute paths occur in retained receipts; keep this transport in
/// protected evidence artifacts, not in public package metadata.
#[derive(Debug)]
pub struct FinishedPackageMeasurement {
    bytes: Vec<u8>,
    sha256: Sha256Digest,
}

impl FinishedPackageMeasurement {
    /// Bounded serialized transport bytes; no mutable record is exposed.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Digest of exact transport bytes, distinct from every receipt self-digest.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }
}

/// Independently selected collector expectations, never derived from the export.
#[derive(Debug)]
pub struct PortableFinishedPackageRequest<'a> {
    /// Bounded regular exported document, possibly downloaded on another host.
    pub measurement: &'a Path,
    /// Raw document digest selected by a separate artifact/origin verifier.
    /// This API checks bytes, but does not verify that external authority.
    pub measurement_sha256: &'a Sha256Digest,
    /// Exact selected lane and executor observation, including binary/contract digests.
    pub identity: &'a Identity,
    /// Independent inputs, environment and downloaded package directory.
    /// Do not copy expectations from the export or package manifest.
    pub package: &'a PackageVerificationRequest,
}

/// Opaque collector byte read-back; not local tree proof or release admission.
///
/// All retained documents are parsed and joined to independently selected lane
/// inputs and freshly verified package members. The collector does not reread
/// the original host's paths, measure its raw tree or prove commands executed.
#[derive(Debug)]
pub struct PortableFinishedPackageReadback {
    measurement_sha256: Sha256Digest,
    build_result_sha256: Sha256Digest,
    finished_receipt_sha256: Sha256Digest,
    raw_payload_sha256: Sha256Digest,
    package: VerifiedPackage,
    members: PackageSetComparison,
}

impl PortableFinishedPackageReadback {
    /// Exact externally selected transport bytes.
    #[must_use]
    pub const fn measurement_sha256(&self) -> &Sha256Digest {
        &self.measurement_sha256
    }
    /// Exact retained build-result bytes, not a receipt self-digest.
    #[must_use]
    pub const fn build_result_sha256(&self) -> &Sha256Digest {
        &self.build_result_sha256
    }
    /// Canonical finished record self-digest, joined to the build result.
    #[must_use]
    pub const fn finished_receipt_sha256(&self) -> &Sha256Digest {
        &self.finished_receipt_sha256
    }
    /// Owning-host raw payload claim; not remeasured on the collector.
    #[must_use]
    pub const fn raw_payload_sha256(&self) -> &Sha256Digest {
        &self.raw_payload_sha256
    }
    /// Freshly measured normalized package bytes and tree.
    #[must_use]
    pub const fn verified_package(&self) -> &VerifiedPackage {
        &self.package
    }
    /// Four measured members of this single package; not an A/B comparison.
    #[must_use]
    pub const fn package_members(&self) -> &PackageSetComparison {
        &self.members
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeasurementDocument {
    schema: String,
    original_work_root: PathBuf,
    original_output_root: PathBuf,
    documents: Vec<RetainedDocument>,
    package_members: PackageSetComparison,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedDocument {
    name: String,
    // Strings preserve exact UTF-8 source bytes; a JSON Value would discard duplicates.
    content: String,
}

/// Export one guarded package and its complete retained local build evidence.
///
/// No files are written. Both local proofs are revalidated before export; the
/// serialized record cannot grant execution, A/B, signer or release authority.
///
/// # Errors
/// Rejects changed candidate/package bytes, mismatched proofs, absent CLI build
/// result, malformed documents or an oversized transport.
pub fn export_finished_package_measurement(
    candidate: &FinishedCandidateReadback,
    package: &FinishedCandidatePackage,
) -> Result<FinishedPackageMeasurement, ContractError> {
    super::comparison::verify_side(candidate, package)?;
    if candidate.receipts.len() != 8 {
        return Err(error(
            "portable package export requires the retained CLI build result",
        ));
    }
    let names = PHASES
        .iter()
        .map(|phase| format!("{phase}.json"))
        .chain([RECEIPT_FILE.to_owned(), RESULT_FILE.to_owned()]);
    let documents = names
        .zip(&candidate.receipts)
        .map(|(name, (_, bytes))| {
            Ok(RetainedDocument {
                name,
                content: String::from_utf8(bytes.clone())
                    .map_err(|_| error("portable package document is not UTF-8"))?,
            })
        })
        .collect::<Result<Vec<_>, ContractError>>()?;
    let record = MeasurementDocument {
        schema: SCHEMA.into(),
        original_work_root: candidate.work_dir.clone(),
        original_output_root: candidate
            .record
            .candidate_root
            .parent()
            .ok_or_else(|| error("finished candidate has no original output root"))?
            .to_path_buf(),
        documents,
        package_members: measure_single_package_set(&package.output.output_dir)?,
    };
    let mut bytes = serde_json::to_vec(&record)
        .map_err(|_| error("cannot serialize portable package measurement"))?;
    bytes.push(b'\n');
    let identity = build_result::select_result(
        record.documents[7].content.as_bytes(),
        &record.original_output_root,
        &package.request.host,
    )?
    .identity;
    let request = PackageVerificationRequest {
        package_dir: package.output.output_dir.clone(),
        release_id: package.request.release_id.clone(),
        host: package.request.host.clone(),
        recipe: package.request.recipe.clone(),
        source_lock: package.request.source_lock.clone(),
        profile: package.request.profile.clone(),
        build_environment: package.request.build_environment.clone(),
        forbidden_prefixes: package.request.forbidden_prefixes.clone(),
    };
    validate_bytes(&bytes, &identity, &request)?;
    candidate.revalidate()?;
    Ok(FinishedPackageMeasurement {
        sha256: sha256_bytes(&bytes),
        bytes,
    })
}

/// Acquire an export and join its complete documents to a downloaded V2 package.
///
/// Original roots are checked lexically but never opened. External origin and
/// signature verification remain mandatory outside this API. A rehashed
/// attacker-created record alone cannot establish authenticated execution.
///
/// # Errors
/// Rejects unsafe/oversized/changing files, changed selected bytes, duplicate or
/// unknown fields, incomplete chains, wrong lane/executor/environment or packages.
pub fn readback_finished_package_measurement(
    request: &PortableFinishedPackageRequest<'_>,
) -> Result<PortableFinishedPackageReadback, ContractError> {
    let (_, bytes) = measure_regular_file_bounded(request.measurement, MAX_BYTES as u64)
        .map_err(|_| error("portable package measurement is unsafe or exceeds its byte bound"))?
        .ok_or_else(|| error("portable package measurement changed while read"))?;
    if sha256_bytes(&bytes) != *request.measurement_sha256 {
        return Err(error(
            "portable package measurement differs from selected raw bytes",
        ));
    }
    validate_bytes(&bytes, request.identity, request.package)
}

fn validate_bytes(
    bytes: &[u8],
    identity: &Identity,
    package_request: &PackageVerificationRequest,
) -> Result<PortableFinishedPackageReadback, ContractError> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err(error("portable package measurement exceeds its byte bound"));
    }
    let document: MeasurementDocument = serde_json::from_slice(bytes)
        .map_err(|_| error("portable package measurement violates its closed schema"))?;
    if document.schema != SCHEMA || document.documents.len() != 8 {
        return Err(error(
            "portable package measurement lacks its complete document set",
        ));
    }
    let names = PHASES
        .iter()
        .map(|phase| format!("{phase}.json"))
        .chain([RECEIPT_FILE.to_owned(), RESULT_FILE.to_owned()]);
    for (expected, retained) in names.zip(&document.documents) {
        if retained.name != expected
            || retained.content.is_empty()
            || retained.content.len() > canonical::MAX_DOCUMENT_BYTES
        {
            return Err(error(
                "portable package documents are missing, reordered or oversized",
            ));
        }
    }
    let result_bytes = document.documents[7].content.as_bytes();
    let selected =
        build_result::select_result(result_bytes, &document.original_output_root, identity.host)?;
    let expected_identity = owned_identity(identity)?;
    if owned_identity(&selected.identity)? != expected_identity {
        return Err(error(
            "portable package build result differs from selected executor or lane",
        ));
    }
    let expected = expected_record(&FinishedCandidateRequest {
        work_dir: &document.original_work_root,
        output_dir: &document.original_output_root,
        recipe: &package_request.recipe,
        source_lock: &package_request.source_lock,
        profile: &package_request.profile,
        identity,
        phase_receipt_digests: &selected.phase_receipt_digests,
        candidate_receipt_digest: &selected.receipt_sha256,
    })?;
    if identity.host != package_request.host {
        return Err(error(
            "portable package expectations select different hosts",
        ));
    }
    let mut collectors = Vec::new();
    for index in 0..6 {
        let phase = validate_phase_receipt(
            document.documents[index].content.as_bytes(),
            &expected_identity,
            &selected.phase_receipt_digests,
            index,
            &document.original_work_root,
            &document.original_output_root,
            &package_request.profile,
        )?;
        if index == 5 && phase.outputs != selected.outputs {
            return Err(error(
                "portable build result outputs differ from publish receipt",
            ));
        }
        if index == 5 {
            collectors = phase.outputs;
        }
    }
    let finished_bytes = document.documents[6].content.as_bytes();
    let finished: FinishedRecord = serde_json::from_slice(finished_bytes)
        .map_err(|_| error("portable finished record violates its closed schema"))?;
    verify_self_digest(&finished, &finished.receipt_sha256, finished_bytes)?;
    if finished.schema != expected.schema
        || finished.identity != expected.identity
        || finished.compiler != expected.compiler
        || finished.target_triple != expected.target_triple
        || finished.source_lock_sha256 != expected.source_lock_sha256
        || finished.profiles_sha256 != expected.profiles_sha256
        || finished.candidate_root != expected.candidate_root
        || finished.phase_receipt_digests != selected.phase_receipt_digests
        || finished.receipt_sha256 != selected.receipt_sha256
        || finished.entry_count == 0
        || finished.entry_count > payload_limits().max_entries as u64
        || finished.regular_file_bytes > payload_limits().max_regular_file_bytes
    {
        return Err(error(
            "portable finished record differs from selected inputs or complete chain",
        ));
    }
    let package = verify_with_format(package_request, PackageFormat::CompilerFamilyV2)?;
    for collector in collectors {
        let path = collector
            .path
            .strip_prefix("toolchain/")
            .ok_or_else(|| error("portable collector is outside the selected package"))?;
        if !package.manifest.files.iter().any(|file| {
            file.path == path
                && file.kind == "file"
                && file.sha256.as_deref() == Some(collector.sha256.as_str())
                && file.size == Some(collector.size)
        }) {
            return Err(error(
                "portable publish collector differs from verified package payload",
            ));
        }
    }
    let members = measure_single_package_set(&package_request.package_dir)?;
    if members != document.package_members {
        return Err(error(
            "portable measurement differs from downloaded package members",
        ));
    }
    Ok(PortableFinishedPackageReadback {
        measurement_sha256: sha256_bytes(bytes),
        build_result_sha256: sha256_bytes(result_bytes),
        finished_receipt_sha256: finished.receipt_sha256,
        raw_payload_sha256: finished.payload_sha256,
        package,
        members,
    })
}

fn error(message: &str) -> ContractError {
    ContractError::state(message)
}
