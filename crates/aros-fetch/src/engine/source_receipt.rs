//! Read-only binding from a closed fetch declaration to its local source receipt.
//!
//! This API checks byte consistency among the caller's declaration, the
//! `.aros-fetch` receipt, and the current destination tree. Receipts are local,
//! editable files: a successful check does not authenticate an upstream origin
//! or prove who authored a receipt.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aros_common::{
    measure_regular_file_bounded, measure_tree_content_cas_bounded, sha256_bytes, Sha256Digest,
    TreeTraversalLimits,
};

use crate::contract::{Cli, FetchRequest};
use crate::{FetchFailure, FetchResult};

use super::{source_contract_id, PatchReceiptDeclaration, SourceReceipt, SourceReceiptDeclaration};

const RECEIPT_NAMESPACE: &str = ".aros-fetch";
const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;
const MAX_TREE_ENTRIES: usize = 300_000;
const MAX_TREE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// A read-only, revalidatable binding to one prepared source tree.
///
/// Construct this value with [`verify_prepared_source`]. The private request
/// and digest fields prevent callers from changing which declaration or local
/// bytes this value represents.
#[derive(Clone, Debug)]
pub struct VerifiedSourceReceipt {
    request: FetchRequest,
    destination: PathBuf,
    receipt_path: PathBuf,
    receipt_sha256: Sha256Digest,
    payload_tree_sha256: Sha256Digest,
    complete_tree_sha256: Sha256Digest,
}

impl VerifiedSourceReceipt {
    /// The closed request whose declaration was matched to the receipt.
    #[must_use]
    pub const fn request(&self) -> &FetchRequest {
        &self.request
    }

    /// The canonical live destination directory that was checked.
    #[must_use]
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// The no-follow receipt path that was checked.
    #[must_use]
    pub fn receipt_path(&self) -> &Path {
        &self.receipt_path
    }

    /// SHA-256 of the exact receipt bytes that were checked.
    #[must_use]
    pub const fn receipt_sha256(&self) -> &Sha256Digest {
        &self.receipt_sha256
    }

    /// Content digest of the checked destination tree, excluding `.aros-fetch`.
    #[must_use]
    pub const fn payload_tree_sha256(&self) -> &Sha256Digest {
        &self.payload_tree_sha256
    }

    /// Re-read the receipt and destination tree, rejecting any changed bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the closed request is invalid, the receipt is
    /// missing or unsafe, or either measured digest differs from this binding.
    pub fn revalidate(&self) -> FetchResult<()> {
        let current = inspect_prepared_source(&self.request)?;
        if current.destination != self.destination
            || current.receipt_path != self.receipt_path
            || current.receipt_sha256 != self.receipt_sha256
            || current.payload_tree_sha256 != self.payload_tree_sha256
            || current.complete_tree_sha256 != self.complete_tree_sha256
        {
            return Err(receipt_failure(
                "prepared source receipt or destination tree changed after verification",
            ));
        }
        Ok(())
    }
}

/// Verify one closed, single-archive/direct-patch declaration against its
/// existing local receipt and live destination tree.
///
/// The API performs no writes, network access, or subprocess execution. It
/// proves only local byte consistency: the receipt is not an authenticated
/// origin statement and this check does not prove that `aros-fetch` authored
/// the receipt.
///
/// # Errors
///
/// Returns an error unless the request is valid, has exactly one checksummed
/// archive candidate and checksums for direct patch filenames only, and its
/// bounded no-follow receipt and destination snapshot agree exactly.
pub fn verify_prepared_source(request: &FetchRequest) -> FetchResult<VerifiedSourceReceipt> {
    let inspected = inspect_prepared_source(request)?;
    Ok(VerifiedSourceReceipt {
        request: request.clone(),
        destination: inspected.destination,
        receipt_path: inspected.receipt_path,
        receipt_sha256: inspected.receipt_sha256,
        payload_tree_sha256: inspected.payload_tree_sha256,
        complete_tree_sha256: inspected.complete_tree_sha256,
    })
}

struct InspectedSource {
    destination: PathBuf,
    receipt_path: PathBuf,
    receipt_sha256: Sha256Digest,
    payload_tree_sha256: Sha256Digest,
    complete_tree_sha256: Sha256Digest,
}

fn inspect_prepared_source(request: &FetchRequest) -> FetchResult<InspectedSource> {
    validate_closed_request(request)?;

    // Resolve only after validating every typed request field. This matches
    // the canonical destination binding used by source publication.
    let destination = request.destination.canonicalize().map_err(|error| {
        receipt_failure(format!(
            "cannot resolve prepared source destination: {error}"
        ))
    })?;
    let archive_name = &request.archive_candidates[0];
    let declaration = SourceReceiptDeclaration {
        schema: "aros-fetch-source-receipt-v1".into(),
        destination_binding: sha256_bytes(destination.as_os_str().as_encoded_bytes()).to_string(),
        archive_name: archive_name.clone(),
        archive_sha256: request.checksums[archive_name].to_string(),
        patches: request
            .patches
            .iter()
            .map(|patch| PatchReceiptDeclaration {
                name: patch.name.clone(),
                selected_candidate: patch.name.clone(),
                sha256: request.checksums[&patch.name].to_string(),
                subdirectory: patch
                    .subdirectory
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
                options: patch.options.clone(),
            })
            .collect(),
    };
    let contract_id = source_contract_id(&declaration)?;
    let receipt_path = destination
        .join(RECEIPT_NAMESPACE)
        .join("receipts")
        .join(format!("{contract_id}.json"));

    let Some((_identity, receipt_bytes)) =
        measure_regular_file_bounded(&receipt_path, MAX_RECEIPT_BYTES).map_err(|error| {
            receipt_failure(format!("cannot read source receipt safely: {error}"))
        })?
    else {
        return Err(receipt_failure("matching source receipt does not exist"));
    };
    let receipt_sha256 = sha256_bytes(&receipt_bytes);
    let receipt: SourceReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|error| receipt_failure(format!("source receipt is malformed: {error}")))?;
    if receipt.declaration != declaration {
        return Err(receipt_failure(
            "source receipt declaration does not match the validated request",
        ));
    }

    let limits = TreeTraversalLimits::new(MAX_TREE_ENTRIES, MAX_TREE_BYTES)
        .map_err(|error| receipt_failure(format!("invalid source tree limits: {error}")))?;
    let snapshot = measure_tree_content_cas_bounded(&destination, limits)
        .map_err(|error| receipt_failure(format!("cannot measure source tree safely: {error}")))?;
    let payload_tree_sha256 = snapshot.payload_digest_excluding(Some(RECEIPT_NAMESPACE));
    // The publication payload digest deliberately omits its receipt namespace.
    // Revalidation must still bind every byte there, including any extra file.
    let complete_tree_sha256 = snapshot.payload_digest_excluding(None);
    if receipt.payload_tree_sha256 != payload_tree_sha256.to_string() {
        return Err(receipt_failure(
            "source receipt payload digest does not match the live destination tree",
        ));
    }

    Ok(InspectedSource {
        destination,
        receipt_path,
        receipt_sha256,
        payload_tree_sha256,
        complete_tree_sha256,
    })
}

fn validate_closed_request(request: &FetchRequest) -> FetchResult<()> {
    if request.archive_candidates.len() != 1 {
        return Err(contract_failure(
            "prepared source verification requires exactly one archive candidate",
        ));
    }
    let candidate = &request.archive_candidates[0];
    let suffixes = if candidate == &request.archive {
        String::new()
    } else {
        candidate
            .strip_prefix(&request.archive)
            .and_then(|rest| rest.strip_prefix('.'))
            .filter(|suffix| !suffix.is_empty())
            .ok_or_else(|| {
                contract_failure("archive candidate is not bound to the declared archive name")
            })?
            .to_owned()
    };

    let mut checksum_names = BTreeSet::from([candidate.clone()]);
    for patch in &request.patches {
        if patch.name == *candidate {
            return Err(contract_failure(
                "archive candidate and direct patch filename must be distinct",
            ));
        }
        checksum_names.insert(patch.name.clone());
    }
    let actual_checksum_names: BTreeSet<_> = request.checksums.keys().cloned().collect();
    if actual_checksum_names != checksum_names {
        return Err(contract_failure(
            "prepared source verification requires checksums for the archive and direct patch filenames only",
        ));
    }

    let patch_declarations = request
        .patches
        .iter()
        .map(|patch| {
            let mut declaration = patch.name.clone();
            if patch.subdirectory.is_some() || !patch.options.is_empty() {
                declaration.push(':');
                if let Some(subdirectory) = &patch.subdirectory {
                    declaration.push_str(subdirectory.to_str().ok_or_else(|| {
                        contract_failure("patch subdirectory must be valid UTF-8")
                    })?);
                }
                declaration.push(':');
                declaration.push_str(&patch.options.join(","));
            }
            Ok(declaration)
        })
        .collect::<FetchResult<Vec<_>>>()?
        .join(" ");
    let checksums = request
        .checksums
        .iter()
        .map(|(name, digest)| format!("{name}=sha256:{digest}"))
        .collect::<Vec<_>>()
        .join(" ");
    let cli = Cli {
        archive_origins: request.archive_origins.join(" "),
        archive: request.archive.clone(),
        suffixes,
        destination: request.destination.clone(),
        patch_origins: request.patch_origins.join(" "),
        patches: patch_declarations,
        base: Some(request.base.clone()),
        location: request.location.clone(),
        rename_directory: None,
        checksums,
        force: request.force,
        offline: request.offline,
        require_checksums: true,
        diagnostic_format: aros_common::DiagnosticFormat::Human,
        log_level: aros_common::LogLevel::Off,
        log_format: aros_common::LogFormat::Human,
        log_file: None,
    };
    let validated = FetchRequest::from_cli(&cli)?;
    if !same_request(request, &validated) {
        return Err(contract_failure(
            "prepared source request does not round-trip through the closed fetch contract",
        ));
    }
    Ok(())
}

fn same_request(left: &FetchRequest, right: &FetchRequest) -> bool {
    left.archive == right.archive
        && left.archive_candidates == right.archive_candidates
        && left.archive_origins == right.archive_origins
        && left.destination == right.destination
        && left.location == right.location
        && left.base == right.base
        && left.patches == right.patches
        && left.patch_origins == right.patch_origins
        && left.checksums == right.checksums
        && left.force == right.force
        && left.offline == right.offline
}

fn contract_failure(message: impl Into<String>) -> FetchFailure {
    FetchFailure::new(aros_common::Diagnostic::error(
        aros_common::DiagnosticCode::FetchContract,
        aros_common::DiagnosticStage::FetchContract,
        message,
    ))
}

fn receipt_failure(message: impl Into<String>) -> FetchFailure {
    FetchFailure::new(aros_common::Diagnostic::error(
        aros_common::DiagnosticCode::FetchPublication,
        aros_common::DiagnosticStage::Publication,
        message,
    ))
}
