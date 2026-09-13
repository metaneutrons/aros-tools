//! Verification of the exact offline payload closure selected by a source lock.
//!
//! The cache is an input, never a source of truth.  A valid lock selects every
//! payload; `aros-fetch` snapshots and validates each direct cache entry before
//! this module reports it as usable.  Extra cache files are intentionally not a
//! failure: only actual source use can establish an undeclared-input violation.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::{fs, io::ErrorKind};

use aros_common::{DiagnosticCode, Sha256Digest};
use aros_fetch::engine::cache::{snapshot_measured_cache_payload, snapshot_verified_cache_payload};

#[cfg(unix)]
use aros_cache::{
    acquire_read_leases, acquire_write_leases, keep_many_validated, observe_root,
    resolve_explicit_root, CacheCapability, CacheFamily, CacheLifecycleError, CacheObjectKind,
    CacheObjectLeases, CacheObjectRequest, CacheRetentionRecord, CacheSideEffects, RootObservation,
};
#[cfg(unix)]
use serde::Serialize;

use crate::ContractError;
#[cfg(unix)]
use crate::{
    filesystem::open_directory,
    source_cache_request::{
        SourceCacheCandidate, SourceCacheEntry, SourceCacheIntegrity, SourceCacheRequest,
        SourceCacheRequestKind,
    },
};

/// Stable schema for an explicit source-cache root observation.
#[cfg(unix)]
pub const SOURCE_CACHE_STATUS_SCHEMA: &str = "aros-cache-sources-status-v1";

/// Stable schema for a source-cache selector metadata projection.
#[cfg(unix)]
pub const SOURCE_CACHE_LIST_SCHEMA: &str = "aros-cache-sources-list-v1";

/// Stable schema for a source-cache strict integrity verification.
#[cfg(unix)]
pub const SOURCE_CACHE_VERIFY_SCHEMA: &str = "aros-cache-sources-verify-v1";

/// Stable schema for source-cache population through a reviewed selector.
#[cfg(unix)]
pub const SOURCE_CACHE_FETCH_SCHEMA: &str = "aros-cache-sources-fetch-v1";

/// Stable schema for a named source-cache retention result.
#[cfg(unix)]
pub const SOURCE_CACHE_KEEP_SCHEMA: &str = "aros-cache-sources-keep-v1";

/// Shared lifecycle lease and verified closure for an active source consumer.
///
/// The non-cloneable lease set remains private and is retained with this value
/// for the entire consumer operation. It prevents a cooperating fetch, keep,
/// or removal operation from changing any selected source object after the
/// closure has been measured.
#[cfg(unix)]
#[derive(Debug)]
pub struct VerifiedSourceCacheLease {
    verification: SourceCacheVerification,
    leases: CacheObjectLeases,
}

#[cfg(unix)]
impl VerifiedSourceCacheLease {
    /// Exact source-cache verification bound to the held read leases.
    #[must_use]
    pub const fn verification(&self) -> &SourceCacheVerification {
        &self.verification
    }

    /// Reassert every selected lifecycle lock before a consumer crosses a
    /// phase boundary.
    ///
    /// # Errors
    ///
    /// Returns AX0301 when a held lifecycle lock can no longer be proven.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        self.leases
            .revalidate()
            .map_err(|error| source_lifecycle_error(&error))
    }
}

/// Passive observation of an explicit source-cache root.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheStatus {
    /// Versioned schema identifier.
    pub schema: &'static str,
    /// Stable operation name.
    pub operation: &'static str,
    /// This operation observes only root metadata.
    pub observation: &'static str,
    /// Hard side-effect boundary of this status operation.
    pub side_effects: CacheSideEffects,
    /// Public source-cache operations. Every operation except `status` needs
    /// an explicit reviewed selector in addition to this root.
    pub capabilities: [CacheCapability; 7],
    /// One non-recursive no-follow root observation.
    pub root: RootObservation,
}

/// Metadata state of one selector-declared source-cache entry.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCacheEntryState {
    /// No direct cache entry exists for the selected candidate.
    Missing,
    /// A regular file exists but this operation deliberately did not hash it.
    PresentUnverified,
    /// The direct cache entry is a symlink or another unsafe file type.
    Unsafe,
    /// Metadata could not be obtained and the entry is not considered missing.
    Inaccessible,
}

/// One selector-declared entry projected without content inspection.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheListEntry {
    /// Semantic declaration role, never inferred from a filename.
    pub role: String,
    /// Reviewed transport candidates in declaration order. They share one
    /// direct cache filename and differ only by origin, so a fallback cannot
    /// silently select a different cache object.
    pub candidates: Vec<SourceCacheCandidate>,
    /// Direct cache filename shared by every reviewed candidate.
    pub filename: String,
    /// Semantic archive/patch representation declared by the selector.
    pub representation: crate::source_cache_request::SourceCacheRepresentation,
    /// Declared cache representation policy.
    pub normalization: aros_fetch::engine::cache::CachePayloadNormalization,
    /// Declared integrity policy.
    pub integrity: SourceCacheIntegrity,
    /// Metadata-only state; `present_unverified` is never a hash claim.
    pub state: SourceCacheEntryState,
    /// Byte length from metadata when a regular file was observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_size: Option<u64>,
    /// Stable I/O category when state is inaccessible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<&'static str>,
}

/// Bounded metadata projection for one closed source-cache request.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheList {
    /// Versioned schema identifier.
    pub schema: &'static str,
    /// Stable operation name.
    pub operation: &'static str,
    /// Origin of the reviewed selector.
    pub request_kind: SourceCacheRequestKind,
    /// SHA-256 of the exact selector bytes.
    pub request_sha256: Sha256Digest,
    /// The hard side-effect boundary of list.
    pub side_effects: CacheSideEffects,
    /// One bounded entry for every selector-declared role.
    pub entries: Vec<SourceCacheListEntry>,
}

/// Integrity classification of one measured source-cache object.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCacheObjectIntegrity {
    /// A measured object matched a declared size and SHA-256 lock.
    Locked,
    /// A product plan explicitly permitted an unpinned object. Its displayed
    /// size and digest are local measurements, not upstream verification.
    MeasuredUnpinned,
}

#[cfg(unix)]
impl SourceCacheObjectIntegrity {
    /// Stable human-oriented integrity label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::MeasuredUnpinned => "measured_unpinned",
        }
    }
}

/// Exact object identity consumed by a source-cache verification.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservedSourceCacheEntry {
    /// Semantic declaration role.
    pub role: String,
    /// Filename from the reviewed direct-cache declaration.
    pub filename: String,
    /// Measured SHA-256 digest, equal to the lock declaration on success.
    pub sha256: Sha256Digest,
    /// Measured byte length, equal to the lock declaration on success.
    pub size: u64,
    /// Whether this measurement is lock-verified or deliberately unpinned.
    pub integrity: SourceCacheObjectIntegrity,
    /// Semantic archive/patch representation declared by the selector.
    pub representation: crate::source_cache_request::SourceCacheRepresentation,
    /// Declared representation policy.
    pub normalization: aros_fetch::engine::cache::CachePayloadNormalization,
}

/// Exact source-cache verification result.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheVerification {
    /// Versioned schema identifier.
    pub schema: &'static str,
    /// Stable operation name.
    pub operation: &'static str,
    /// Origin of the reviewed selector.
    pub request_kind: SourceCacheRequestKind,
    /// SHA-256 of the exact selector bytes.
    pub request_sha256: Sha256Digest,
    /// Verification hashes declared payloads but has no other side effect.
    pub side_effects: CacheSideEffects,
    /// Exact measured objects consumed by this verification.
    pub entries: Vec<ObservedSourceCacheEntry>,
}

/// Exact result of source-cache population through a reviewed selector.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheFetch {
    /// Versioned schema identifier.
    pub schema: &'static str,
    /// Stable operation name.
    pub operation: &'static str,
    /// Origin of the reviewed selector.
    pub request_kind: SourceCacheRequestKind,
    /// SHA-256 of the exact selector bytes.
    pub request_sha256: Sha256Digest,
    /// Explicit population boundary. `network` is false only for offline runs.
    pub side_effects: CacheSideEffects,
    /// Exact measured objects present after the operation completes.
    pub entries: Vec<ObservedSourceCacheEntry>,
}

#[cfg(unix)]
const PASSIVE_SIDE_EFFECTS: CacheSideEffects = CacheSideEffects {
    creates_state: false,
    mutates_state: false,
    network: false,
    backend_process: false,
    locks: false,
    hashes_payloads: false,
};

#[cfg(unix)]
const VERIFY_SIDE_EFFECTS: CacheSideEffects = CacheSideEffects {
    hashes_payloads: true,
    ..PASSIVE_SIDE_EFFECTS
};

#[cfg(unix)]
const FETCH_OFFLINE_SIDE_EFFECTS: CacheSideEffects = CacheSideEffects {
    creates_state: true,
    mutates_state: true,
    network: false,
    backend_process: false,
    locks: true,
    hashes_payloads: true,
};

#[cfg(unix)]
const FETCH_ONLINE_SIDE_EFFECTS: CacheSideEffects = CacheSideEffects {
    network: true,
    ..FETCH_OFFLINE_SIDE_EFFECTS
};

/// Observe an explicit source-cache root without creating or traversing it.
///
/// # Errors
///
/// Returns AX0101 when `cache_root` is relative. Missing, symlinked and
/// inaccessible absolute roots are successful truthful observations.
#[cfg(unix)]
pub fn status(cache_root: &Path) -> Result<SourceCacheStatus, ContractError> {
    let root = resolve_explicit_root(cache_root.to_owned())
        .map_err(|error| ContractError::invalid(error.to_string()))?;
    Ok(SourceCacheStatus {
        schema: SOURCE_CACHE_STATUS_SCHEMA,
        operation: "sources.status",
        observation: "passive",
        side_effects: PASSIVE_SIDE_EFFECTS,
        capabilities: [
            CacheCapability::Status,
            CacheCapability::List,
            CacheCapability::Fetch,
            CacheCapability::Verify,
            CacheCapability::Keep,
            CacheCapability::Release,
            CacheCapability::Remove,
        ],
        root: observe_root(root),
    })
}

/// List selector-declared cache entries without reading payload bytes.
///
/// # Errors
///
/// Returns AX0301 when the root is not an existing real absolute directory or
/// the request does not yet select one exact candidate per role.
#[cfg(unix)]
pub fn list(
    cache_root: &Path,
    request: &SourceCacheRequest,
) -> Result<SourceCacheList, ContractError> {
    checked_real_cache_root(cache_root)?;
    request.validate()?;
    let entries = request
        .entries
        .iter()
        .map(|entry| list_entry(cache_root, entry))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SourceCacheList {
        schema: SOURCE_CACHE_LIST_SCHEMA,
        operation: "sources.list",
        request_kind: request.kind,
        request_sha256: request.request_sha256.clone(),
        side_effects: PASSIVE_SIDE_EFFECTS,
        entries,
    })
}

/// Measure every selector entry and verify every strict declaration.
///
/// # Errors
///
/// Returns AX0301 when the cache root/object is unsafe, absent or changed.
/// Unverified product declarations are measured within their declared finite
/// limit and returned as `measured_unpinned`; the result never claims upstream
/// integrity for them.
#[cfg(unix)]
pub fn verify_request(
    cache_root: &Path,
    request: &SourceCacheRequest,
) -> Result<SourceCacheVerification, ContractError> {
    checked_real_cache_root(cache_root)?;
    request.validate()?;
    let mut entries = Vec::with_capacity(request.entries.len());
    for entry in &request.entries {
        let filename = cache_filename(entry)?;
        let (snapshot, integrity) = match &entry.integrity {
            SourceCacheIntegrity::Locked { sha256, size } => (
                snapshot_verified_cache_payload(cache_root, filename, *size, sha256)
                    .map_err(|error| map_fetch_failure(&error))?,
                SourceCacheObjectIntegrity::Locked,
            ),
            SourceCacheIntegrity::Unverified { max_size } => (
                snapshot_measured_cache_payload(cache_root, filename, *max_size)
                    .map_err(|error| map_fetch_failure(&error))?,
                SourceCacheObjectIntegrity::MeasuredUnpinned,
            ),
        };
        snapshot
            .revalidate()
            .map_err(|error| map_fetch_failure(&error))?;
        entries.push(ObservedSourceCacheEntry {
            role: entry.role.clone(),
            filename: filename.to_owned(),
            sha256: snapshot.sha256().clone(),
            size: snapshot.size(),
            integrity,
            representation: entry.representation,
            normalization: entry.normalization,
        });
    }
    entries.sort_by(|left, right| left.role.cmp(&right.role));
    Ok(SourceCacheVerification {
        schema: SOURCE_CACHE_VERIFY_SCHEMA,
        operation: "sources.verify",
        request_kind: request.kind,
        request_sha256: request.request_sha256.clone(),
        side_effects: VERIFY_SIDE_EFFECTS,
        entries,
    })
}

/// Open and verify the exact source closure for an active consumer.
///
/// The returned guard must remain alive for every read of the selected cache
/// entries. It holds one shared, no-follow lifecycle lease per object, then
/// verifies the closed request under those leases and revalidates the locks
/// before returning. It neither downloads nor creates source payloads.
///
/// # Errors
///
/// Returns AX0301 when the root/request/object is unsafe, a source object is
/// missing or changed, or a writer/removal lease is active.
#[cfg(unix)]
pub fn open_verified_request(
    cache_root: &Path,
    request: &SourceCacheRequest,
) -> Result<VerifiedSourceCacheLease, ContractError> {
    let objects = lifecycle_objects(cache_root, request)?;
    let leases = acquire_read_leases(&objects).map_err(|error| source_lifecycle_error(&error))?;
    let verification = verify_request(cache_root, request)?;
    leases
        .revalidate()
        .map_err(|error| source_lifecycle_error(&error))?;
    Ok(VerifiedSourceCacheLease {
        verification,
        leases,
    })
}

/// Retain a fully verified source-cache closure under one named reference.
///
/// Verification runs while every selected object has the same exclusive
/// lifecycle lease used to create the retention receipt. A producer lock,
/// compatibility lock, product archive and product patch therefore survive
/// until the final named reference is released.
///
/// # Errors
///
/// Returns AX0301 when selection, root or object verification fails, when a
/// reader/writer is active, or when the name is occupied. No failed operation
/// creates a partial retention receipt.
#[cfg(unix)]
pub fn retain_request(
    cache_root: &Path,
    request: &SourceCacheRequest,
    name: &str,
) -> Result<CacheRetentionRecord, ContractError> {
    let objects = lifecycle_objects(cache_root, request)?;
    keep_many_validated(&objects, name, || {
        verify_request(cache_root, request)
            .map(|_| ())
            .map_err(|error| CacheLifecycleError::validation(error.to_string()))
    })
    .map_err(|error| source_lifecycle_error(&error))
}

/// Select the one exact source-cache object named by a reviewed semantic role.
///
/// This intentionally does not infer selection from a filename. It validates
/// the full reviewed closure first, then returns the direct object associated
/// with the caller-provided role for preview/apply removal.
///
/// # Errors
///
/// Returns AX0301 when the request/root is unsafe or `role` is not part of the
/// reviewed closure.
#[cfg(unix)]
pub fn select_lifecycle_object(
    cache_root: &Path,
    request: &SourceCacheRequest,
    role: &str,
) -> Result<CacheObjectRequest, ContractError> {
    let objects = lifecycle_objects(cache_root, request)?;
    request
        .entries
        .iter()
        .position(|entry| entry.role == role)
        .and_then(|index| objects.get(index).cloned())
        .ok_or_else(|| {
            ContractError::sources(
                "the requested source-cache removal role is not part of the reviewed closure",
            )
        })
}

/// Acquire missing source-cache objects, then measure the complete closure.
///
/// Existing entries are snapshotted and verified before transport. They are
/// never refreshed, replaced or repaired. The caller must select an existing
/// real cache root; source-cache population creates only private transfer
/// staging and no-clobber declared payloads below that root.
///
/// # Errors
///
/// Returns AX0301 when the selected root/object is unsafe, a strict integrity
/// check fails, a transfer cannot complete, or offline mode encounters a miss.
/// An unverified product declaration is rejected unless the caller has made
/// the explicit `--allow-unverified` decision at its public boundary.
#[cfg(unix)]
pub async fn fetch_request(
    cache_root: &Path,
    request: &SourceCacheRequest,
    offline: bool,
    allow_unverified: bool,
) -> Result<SourceCacheFetch, ContractError> {
    checked_real_cache_root(cache_root)?;
    request.validate()?;
    validate_fetch_policy(request, allow_unverified)?;
    let objects = lifecycle_objects(cache_root, request)?;
    let leases = acquire_write_leases(&objects).map_err(|error| source_lifecycle_error(&error))?;
    preflight_existing_entries(cache_root, &request.entries)?;
    for entry in &request.entries {
        fetch_entry(cache_root, entry, offline, allow_unverified).await?;
    }
    let verified = verify_request(cache_root, request)?;
    leases
        .revalidate()
        .map_err(|error| source_lifecycle_error(&error))?;
    Ok(SourceCacheFetch {
        schema: SOURCE_CACHE_FETCH_SCHEMA,
        operation: "sources.fetch",
        request_kind: verified.request_kind,
        request_sha256: verified.request_sha256,
        side_effects: if offline {
            FETCH_OFFLINE_SIDE_EFFECTS
        } else {
            FETCH_ONLINE_SIDE_EFFECTS
        },
        entries: verified.entries,
    })
}

#[cfg(unix)]
fn lifecycle_objects(
    cache_root: &Path,
    request: &SourceCacheRequest,
) -> Result<Vec<CacheObjectRequest>, ContractError> {
    checked_real_cache_root(cache_root)?;
    request.validate()?;
    request
        .entries
        .iter()
        .map(|entry| {
            let filename = cache_filename(entry)?;
            let max_bytes = match entry.integrity {
                SourceCacheIntegrity::Locked { size, .. } => size,
                SourceCacheIntegrity::Unverified { max_size } => max_size,
            };
            Ok(CacheObjectRequest {
                family: CacheFamily::Sources,
                cache_root: cache_root.to_path_buf(),
                relative_path: PathBuf::from(filename),
                kind: CacheObjectKind::RegularFile { max_bytes },
            })
        })
        .collect()
}

#[cfg(unix)]
fn validate_fetch_policy(
    request: &SourceCacheRequest,
    allow_unverified: bool,
) -> Result<(), ContractError> {
    if !allow_unverified
        && request
            .entries
            .iter()
            .any(|entry| matches!(entry.integrity, SourceCacheIntegrity::Unverified { .. }))
    {
        return Err(ContractError::sources(
            "an explicitly unverified product source requires --allow-unverified; no cache object was changed",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn source_lifecycle_error(error: &CacheLifecycleError) -> ContractError {
    ContractError::sources(format!(
        "source-cache lifecycle operation was rejected safely: {error}"
    ))
}

#[cfg(unix)]
fn checked_real_cache_root(cache_root: &Path) -> Result<(), ContractError> {
    if !cache_root.is_absolute() || open_directory(cache_root).is_err() {
        return Err(ContractError::sources(
            "selected source cache must be an existing real absolute directory without symlink components",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn cache_filename(entry: &SourceCacheEntry) -> Result<&str, ContractError> {
    if entry.filename.is_empty() {
        return Err(ContractError::sources(
            "source-cache request has no reviewed direct cache filename for a declared role",
        ));
    }
    Ok(&entry.filename)
}

/// Reject every invalid existing object before acquisition can publish a
/// different missing object from the same closed request.
#[cfg(unix)]
fn preflight_existing_entries(
    cache_root: &Path,
    entries: &[SourceCacheEntry],
) -> Result<(), ContractError> {
    for entry in entries {
        let filename = cache_filename(entry)?;
        match fs::symlink_metadata(cache_root.join(filename)) {
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(_) => {
                return Err(ContractError::sources(
                    "cannot inspect a selected source-cache object before acquisition",
                ));
            }
            Ok(_) => {}
        }
        let snapshot = match &entry.integrity {
            SourceCacheIntegrity::Locked { sha256, size } => {
                snapshot_verified_cache_payload(cache_root, filename, *size, sha256)
            }
            SourceCacheIntegrity::Unverified { max_size } => {
                snapshot_measured_cache_payload(cache_root, filename, *max_size)
            }
        }
        .map_err(|error| map_fetch_failure(&error))?;
        snapshot
            .revalidate()
            .map_err(|error| map_fetch_failure(&error))?;
    }
    Ok(())
}

#[cfg(unix)]
async fn fetch_entry(
    cache_root: &Path,
    entry: &SourceCacheEntry,
    offline: bool,
    allow_unverified: bool,
) -> Result<(), ContractError> {
    let filename = cache_filename(entry)?;
    if matches!(entry.integrity, SourceCacheIntegrity::Unverified { .. }) && !allow_unverified {
        return Err(ContractError::sources(
            "an explicitly unverified product source requires --allow-unverified; no cache object was changed",
        ));
    }
    let mut last_failure = None;
    for candidate in &entry.candidates {
        let result = match &entry.integrity {
            SourceCacheIntegrity::Locked { sha256, size } => {
                aros_fetch::engine::cache::acquire_https_cache_payload_with_normalization(
                    cache_root,
                    filename,
                    &candidate.url,
                    *size,
                    sha256,
                    entry.normalization,
                    offline,
                )
                .await
            }
            SourceCacheIntegrity::Unverified { max_size } => {
                if entry.normalization
                    != aros_fetch::engine::cache::CachePayloadNormalization::ExactBytesV1
                {
                    return Err(ContractError::sources(
                        "an explicitly unverified product source cannot request archive normalization",
                    ));
                }
                aros_fetch::engine::cache::acquire_unverified_https_cache_payload(
                    cache_root,
                    filename,
                    &candidate.url,
                    *max_size,
                    offline,
                )
                .await
            }
        };
        match result {
            Ok(snapshot) => {
                snapshot
                    .revalidate()
                    .map_err(|error| map_fetch_failure(&error))?;
                return Ok(());
            }
            Err(error) => last_failure = Some(map_fetch_failure(&error)),
        }
    }
    Err(last_failure.unwrap_or_else(|| {
        ContractError::sources("source-cache role has no reviewed transport candidates")
    }))
}

#[cfg(unix)]
fn list_entry(
    cache_root: &Path,
    entry: &SourceCacheEntry,
) -> Result<SourceCacheListEntry, ContractError> {
    let filename = cache_filename(entry)?.to_owned();
    let path = cache_root.join(&filename);
    let (state, metadata_size, error_kind) = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            (SourceCacheEntryState::Unsafe, None, None)
        }
        Ok(metadata) => (
            SourceCacheEntryState::PresentUnverified,
            Some(metadata.len()),
            None,
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            (SourceCacheEntryState::Missing, None, None)
        }
        Err(error) => (
            SourceCacheEntryState::Inaccessible,
            None,
            Some(io_error_kind(error.kind())),
        ),
    };
    Ok(SourceCacheListEntry {
        role: entry.role.clone(),
        candidates: entry.candidates.clone(),
        filename,
        representation: entry.representation,
        normalization: entry.normalization,
        integrity: entry.integrity.clone(),
        state,
        metadata_size,
        error_kind,
    })
}

#[cfg(unix)]
const fn io_error_kind(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::PermissionDenied => "permission_denied",
        ErrorKind::NotADirectory => "not_a_directory",
        ErrorKind::TooManyLinks => "too_many_links",
        _ => "io_error",
    }
}

fn map_fetch_failure(error: &aros_fetch::FetchFailure) -> ContractError {
    match error.diagnostic().code {
        DiagnosticCode::FetchContract => ContractError::sources(
            "a selected source-cache declaration violates the reviewed cache transport contract",
        ),
        DiagnosticCode::FetchCache => ContractError::sources(
            "a selected source-cache payload is missing, unsafe or changed during verification",
        ),
        DiagnosticCode::FetchNetwork => ContractError::sources(
            "a reviewed source-cache transport failed; no existing cache object was replaced",
        ),
        DiagnosticCode::FetchIntegrity => ContractError::sources(
            "a selected source-cache payload does not match its locked size or SHA-256",
        ),
        _ => ContractError::sources("a selected source-cache operation failed safely"),
    }
}

#[cfg(all(test, unix))]
mod m2_tests {
    use std::fs;

    use aros_common::sha256_bytes;
    use aros_fetch::engine::cache::CachePayloadNormalization;

    use super::{
        fetch_request, list, open_verified_request, retain_request, select_lifecycle_object,
        status, verify_request, SourceCacheEntryState, SourceCacheFetch, SourceCacheIntegrity,
        SourceCacheList, SourceCacheObjectIntegrity, SourceCacheRequest, SourceCacheRequestKind,
        SourceCacheStatus, SourceCacheVerification, SOURCE_CACHE_FETCH_SCHEMA,
        SOURCE_CACHE_LIST_SCHEMA, SOURCE_CACHE_STATUS_SCHEMA, SOURCE_CACHE_VERIFY_SCHEMA,
    };
    use crate::source_cache_request::{
        SourceCacheCandidate, SourceCacheEntry, SourceCacheRepresentation,
    };

    fn real_tempdir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("aros-source-cache-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
    }

    fn strict_request(filename: &str, payload: &[u8]) -> SourceCacheRequest {
        SourceCacheRequest {
            kind: SourceCacheRequestKind::ProducerSourceLock,
            request_sha256: sha256_bytes(b"reviewed selector"),
            entries: vec![SourceCacheEntry {
                role: "producer:toolchain_component:llvm-project@20.1.7".to_owned(),
                filename: filename.to_owned(),
                candidates: vec![SourceCacheCandidate {
                    url: format!("https://example.invalid/{filename}"),
                }],
                representation: SourceCacheRepresentation::Archive,
                patch: None,
                normalization: CachePayloadNormalization::ExactBytesV1,
                integrity: SourceCacheIntegrity::Locked {
                    sha256: sha256_bytes(payload),
                    size: payload.len() as u64,
                },
            }],
        }
    }

    fn unverified_product_request(filename: &str, maximum_size: u64) -> SourceCacheRequest {
        SourceCacheRequest {
            kind: SourceCacheRequestKind::ProductSourceFetchPlan,
            request_sha256: sha256_bytes(b"reviewed product selector"),
            entries: vec![SourceCacheEntry {
                role: "product:grub@2.12".to_owned(),
                filename: filename.to_owned(),
                candidates: vec![SourceCacheCandidate {
                    url: format!("https://example.invalid/{filename}"),
                }],
                representation: SourceCacheRepresentation::Archive,
                patch: None,
                normalization: CachePayloadNormalization::ExactBytesV1,
                integrity: SourceCacheIntegrity::Unverified {
                    max_size: maximum_size,
                },
            }],
        }
    }

    #[test]
    fn status_observes_a_missing_explicit_root_without_creating_it() {
        let temporary = real_tempdir();
        let root = temporary.path().join("missing");
        let observation: SourceCacheStatus = status(&root).unwrap();
        assert_eq!(observation.schema, SOURCE_CACHE_STATUS_SCHEMA);
        assert_eq!(observation.operation, "sources.status");
        assert_eq!(observation.observation, "passive");
        assert_eq!(observation.root.state.as_str(), "missing");
        assert!(!root.exists());
        assert!(!observation.side_effects.creates_state);
        assert!(!observation.side_effects.hashes_payloads);
    }

    #[test]
    fn list_reports_metadata_without_upgrading_a_file_to_verified() {
        let temporary = real_tempdir();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let payload = b"exact locked payload";
        fs::write(cache.join("llvm.tar.xz"), payload).unwrap();
        let listed: SourceCacheList =
            list(&cache, &strict_request("llvm.tar.xz", payload)).unwrap();
        assert_eq!(listed.schema, SOURCE_CACHE_LIST_SCHEMA);
        assert_eq!(listed.operation, "sources.list");
        assert_eq!(listed.entries.len(), 1);
        assert_eq!(
            listed.entries[0].state,
            SourceCacheEntryState::PresentUnverified
        );
        assert_eq!(listed.entries[0].metadata_size, Some(payload.len() as u64));
        assert!(!listed.side_effects.hashes_payloads);
    }

    #[test]
    fn verify_remeasures_the_exact_locked_object_and_reports_its_identity() {
        let temporary = real_tempdir();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let payload = b"exact locked payload";
        fs::write(cache.join("llvm.tar.xz"), payload).unwrap();
        let verified: SourceCacheVerification =
            verify_request(&cache, &strict_request("llvm.tar.xz", payload)).unwrap();
        assert_eq!(verified.schema, SOURCE_CACHE_VERIFY_SCHEMA);
        assert_eq!(verified.operation, "sources.verify");
        assert_eq!(verified.entries[0].sha256, sha256_bytes(payload));
        assert_eq!(verified.entries[0].size, payload.len() as u64);
        assert!(verified.side_effects.hashes_payloads);
        assert!(!verified.side_effects.mutates_state);
    }

    #[test]
    fn verified_source_consumption_excludes_lifecycle_writers_and_retention() {
        let temporary = real_tempdir();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let payload = b"exact locked payload";
        let request = strict_request("llvm.tar.xz", payload);
        fs::write(cache.join("llvm.tar.xz"), payload).unwrap();

        let active = open_verified_request(&cache, &request).unwrap();
        let object = select_lifecycle_object(
            &cache,
            &request,
            "producer:toolchain_component:llvm-project@20.1.7",
        )
        .unwrap();
        assert!(aros_cache::acquire_write_lease(&object).is_err());
        assert!(retain_request(&cache, &request, "native-build").is_err());
        active.revalidate().unwrap();
        drop(active);

        let retention = retain_request(&cache, &request, "native-build").unwrap();
        assert_eq!(retention.objects.len(), 1);
        assert_eq!(retention.objects[0].relative_path, "llvm.tar.xz");
    }

    #[tokio::test]
    async fn offline_fetch_reuses_only_a_verified_existing_object() {
        let temporary = real_tempdir();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let payload = b"exact locked payload";
        fs::write(cache.join("llvm.tar.xz"), payload).unwrap();
        let fetched: SourceCacheFetch =
            fetch_request(&cache, &strict_request("llvm.tar.xz", payload), true, false)
                .await
                .unwrap();
        assert_eq!(fetched.schema, SOURCE_CACHE_FETCH_SCHEMA);
        assert_eq!(fetched.operation, "sources.fetch");
        assert!(!fetched.side_effects.network);
        assert!(fetched.side_effects.hashes_payloads);
        assert_eq!(fetched.entries[0].sha256, sha256_bytes(payload));

        let error = fetch_request(
            &cache,
            &strict_request("missing.tar.xz", payload),
            true,
            false,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("missing, unsafe or changed"));
        assert!(!cache.join("missing.tar.xz").exists());
    }

    #[tokio::test]
    async fn a_corrupt_existing_member_blocks_publication_of_every_missing_member() {
        let temporary = real_tempdir();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let expected = b"locked source payload";
        fs::write(cache.join("corrupt.tar.xz"), b"different bytes").unwrap();
        let mut request = strict_request("missing.tar.xz", expected);
        request.entries.push(SourceCacheEntry {
            role: "producer:target_build_dependency:corrupt@1".to_owned(),
            filename: "corrupt.tar.xz".to_owned(),
            candidates: vec![SourceCacheCandidate {
                url: "https://example.invalid/corrupt.tar.xz".to_owned(),
            }],
            representation: SourceCacheRepresentation::Archive,
            patch: None,
            normalization: CachePayloadNormalization::ExactBytesV1,
            integrity: SourceCacheIntegrity::Locked {
                sha256: sha256_bytes(expected),
                size: expected.len() as u64,
            },
        });
        request
            .entries
            .sort_by(|left, right| left.role.cmp(&right.role));

        let error = fetch_request(&cache, &request, false, false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("does not match"));
        assert!(!cache.join("missing.tar.xz").exists());
    }

    #[tokio::test]
    async fn unverified_product_entries_require_opt_in_and_remain_measured_unpinned() {
        let temporary = real_tempdir();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let payload = b"deliberately unpinned product source";
        fs::write(cache.join("grub.tar.xz"), payload).unwrap();
        let request = unverified_product_request("grub.tar.xz", payload.len() as u64);

        let refusal = fetch_request(&cache, &request, true, false)
            .await
            .unwrap_err();
        assert!(refusal.to_string().contains("--allow-unverified"));
        assert_eq!(fs::read(cache.join("grub.tar.xz")).unwrap(), payload);

        let verification = verify_request(&cache, &request).unwrap();
        assert_eq!(
            verification.entries[0].integrity,
            SourceCacheObjectIntegrity::MeasuredUnpinned
        );
        assert_eq!(verification.entries[0].sha256, sha256_bytes(payload));
        assert_eq!(verification.entries[0].size, payload.len() as u64);

        let fetched = fetch_request(&cache, &request, true, true).await.unwrap();
        assert_eq!(
            fetched.entries[0].integrity,
            SourceCacheObjectIntegrity::MeasuredUnpinned
        );
        assert!(!fetched.side_effects.network);
    }

    #[test]
    fn list_rejects_a_symlinked_cache_entry_without_following_it() {
        use std::os::unix::fs::symlink;

        let temporary = real_tempdir();
        let cache = temporary.path().join("cache");
        let outside = temporary.path().join("outside");
        fs::create_dir(&cache).unwrap();
        fs::write(&outside, b"outside cache object").unwrap();
        symlink(&outside, cache.join("llvm.tar.xz")).unwrap();
        let listed = list(
            &cache,
            &strict_request("llvm.tar.xz", b"outside cache object"),
        )
        .unwrap();
        assert_eq!(listed.entries[0].state, SourceCacheEntryState::Unsafe);
        assert_eq!(listed.entries[0].metadata_size, None);
    }
}
