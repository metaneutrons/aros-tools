//! Verification of the exact offline payload closure selected by a source lock.
//!
//! The cache is an input, never a source of truth.  A valid lock selects every
//! payload; `aros-fetch` snapshots and validates each direct cache entry before
//! this module reports it as usable.  Extra cache files are intentionally not a
//! failure: only actual source use can establish an undeclared-input violation.

use std::path::Path;

#[cfg(unix)]
use std::{fs, io::ErrorKind};

use aros_common::{DiagnosticCode, Sha256Digest};
use aros_fetch::engine::cache::snapshot_verified_cache_payload;

#[cfg(unix)]
use aros_cache::{observe_root, resolve_explicit_root, CacheSideEffects, RootObservation};
#[cfg(unix)]
use serde::Serialize;

use crate::source_lock::SourceLock;
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
    /// The sole currently selected candidate.
    pub candidate: SourceCacheCandidate,
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

/// Exact object identity consumed by a successful strict source-cache verify.
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifiedSourceCacheEntry {
    /// Semantic declaration role.
    pub role: String,
    /// Filename from the reviewed direct-cache declaration.
    pub filename: String,
    /// Measured SHA-256 digest, equal to the lock declaration on success.
    pub sha256: Sha256Digest,
    /// Measured byte length, equal to the lock declaration on success.
    pub size: u64,
    /// Declared representation policy.
    pub normalization: aros_fetch::engine::cache::CachePayloadNormalization,
}

/// Exact strict source-cache verification result.
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
    /// Exact locked objects consumed by this verification.
    pub entries: Vec<VerifiedSourceCacheEntry>,
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
    /// Exact locked objects present after the operation completes.
    pub entries: Vec<VerifiedSourceCacheEntry>,
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

/// Verify every strict selector entry against its declared cache identity.
///
/// # Errors
///
/// Returns AX0301 when the cache root/object is unsafe, absent or changed.
/// Unverified product declarations require their dedicated M2 acquisition and
/// observation path; this strict verifier never upgrades them to a lock.
#[cfg(unix)]
pub fn verify_request(
    cache_root: &Path,
    request: &SourceCacheRequest,
) -> Result<SourceCacheVerification, ContractError> {
    checked_real_cache_root(cache_root)?;
    let mut entries = Vec::with_capacity(request.entries.len());
    for entry in &request.entries {
        let candidate = exact_candidate(entry)?;
        let SourceCacheIntegrity::Locked { sha256, size } = &entry.integrity else {
            return Err(ContractError::sources(
                "unverified product source declarations cannot be reported as verified",
            ));
        };
        let snapshot =
            snapshot_verified_cache_payload(cache_root, &candidate.filename, *size, sha256)
                .map_err(|error| map_fetch_failure(&error))?;
        snapshot
            .revalidate()
            .map_err(|error| map_fetch_failure(&error))?;
        entries.push(VerifiedSourceCacheEntry {
            role: entry.role.clone(),
            filename: candidate.filename.clone(),
            sha256: snapshot.sha256().clone(),
            size: snapshot.size(),
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

/// Acquire missing strict source-cache objects, then verify the full closure.
///
/// Existing entries are snapshotted and verified before transport. They are
/// never refreshed, replaced or repaired. The caller must select an existing
/// real cache root; source-cache population creates only private transfer
/// staging and no-clobber declared payloads below that root.
///
/// # Errors
///
/// Returns AX0301 when the selected root/object is unsafe, an integrity check
/// fails, a transfer cannot complete, or offline mode encounters a miss.
/// Unverified product declarations require the separate explicit M2 path and
/// cannot enter a strict producer/compatibility cache closure.
#[cfg(unix)]
pub async fn fetch_request(
    cache_root: &Path,
    request: &SourceCacheRequest,
    offline: bool,
) -> Result<SourceCacheFetch, ContractError> {
    checked_real_cache_root(cache_root)?;
    for entry in &request.entries {
        let candidate = exact_candidate(entry)?;
        let SourceCacheIntegrity::Locked { sha256, size } = &entry.integrity else {
            return Err(ContractError::sources(
                "unverified product source declarations require --allow-unverified and cannot enter a strict source cache closure",
            ));
        };
        let snapshot = aros_fetch::engine::cache::acquire_https_cache_payload_with_normalization(
            cache_root,
            &candidate.filename,
            &candidate.url,
            *size,
            sha256,
            entry.normalization,
            offline,
        )
        .await
        .map_err(|error| map_fetch_failure(&error))?;
        snapshot
            .revalidate()
            .map_err(|error| map_fetch_failure(&error))?;
    }
    let verified = verify_request(cache_root, request)?;
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
fn checked_real_cache_root(cache_root: &Path) -> Result<(), ContractError> {
    if !cache_root.is_absolute() || open_directory(cache_root).is_err() {
        return Err(ContractError::sources(
            "selected source cache must be an existing real absolute directory without symlink components",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn exact_candidate(entry: &SourceCacheEntry) -> Result<&SourceCacheCandidate, ContractError> {
    let [candidate] = entry.candidates.as_slice() else {
        return Err(ContractError::sources(
            "this source-cache operation requires one exact candidate for each declared role",
        ));
    };
    if candidate.filename.is_empty()
        || Path::new(&candidate.filename).parent() != Some(Path::new(""))
    {
        return Err(ContractError::sources(
            "source-cache request has an unsafe direct-cache candidate filename",
        ));
    }
    Ok(candidate)
}

#[cfg(unix)]
fn list_entry(
    cache_root: &Path,
    entry: &SourceCacheEntry,
) -> Result<SourceCacheListEntry, ContractError> {
    let candidate = exact_candidate(entry)?.clone();
    let path = cache_root.join(&candidate.filename);
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
        candidate,
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

/// One verified payload observation, safe to retain in an M2 plan/report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadObservation {
    /// Lock-selected portable basename.
    pub filename: String,
    /// Measured digest, equal to the lock on success.
    pub sha256: Sha256Digest,
    /// Measured byte size, equal to the lock on success.
    pub size: u64,
}

/// Complete verified cache closure for one selected source lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheObservation {
    /// Payloads in stable filename order.
    pub payloads: Vec<PayloadObservation>,
}

/// Verify every source-lock payload as an immutable direct cache child.
///
/// No network operation, cache insertion, extraction or source execution is
/// permitted.  `aros-fetch` creates private temporary snapshots only; they are
/// dropped after their source and snapshot CAS checks pass.
///
/// # Errors
///
/// Returns AX0301 when a cache root/payload is missing, changing, unsafe or has
/// a different measured identity.  It deliberately does not echo cache paths.
pub fn verify(cache_root: &Path, lock: &SourceLock) -> Result<CacheObservation, ContractError> {
    if !cache_root.is_absolute() || !cache_root.is_dir() {
        return Err(ContractError::sources(
            "selected verified source cache is not an existing directory",
        ));
    }
    let mut payloads = Vec::new();
    for expected in lock.payloads() {
        let snapshot = snapshot_verified_cache_payload(
            cache_root,
            expected.filename(),
            expected.size(),
            expected.sha256(),
        )
        .map_err(|error| map_fetch_failure(&error))?;
        snapshot
            .revalidate()
            .map_err(|error| map_fetch_failure(&error))?;
        payloads.push(PayloadObservation {
            filename: expected.filename().to_owned(),
            sha256: snapshot.sha256().clone(),
            size: snapshot.size(),
        });
    }
    payloads.sort_by(|left, right| left.filename.cmp(&right.filename));
    Ok(CacheObservation { payloads })
}

/// Acquire only lock-selected missing payloads, then return their exact cache observation.
///
/// Existing cache entries are never refreshed: any stale or poisoned object
/// fails closed before a compiler process could start.
///
/// # Errors
///
/// Returns AX0301 for every source/cache/transport failure. `offline` permits
/// no network transfer and therefore turns a cache miss into the same typed
/// source-preflight failure.
pub async fn acquire(
    cache_root: &Path,
    lock: &SourceLock,
    offline: bool,
) -> Result<CacheObservation, ContractError> {
    if !cache_root.is_absolute() || !cache_root.is_dir() {
        return Err(ContractError::sources(
            "selected verified source cache is not an existing directory",
        ));
    }
    for expected in lock.payloads() {
        let snapshot = aros_fetch::engine::cache::acquire_https_cache_payload(
            cache_root,
            expected.filename(),
            expected.url(),
            expected.size(),
            expected.sha256(),
            offline,
        )
        .await
        .map_err(|error| map_fetch_failure(&error))?;
        snapshot
            .revalidate()
            .map_err(|error| map_fetch_failure(&error))?;
    }
    verify(cache_root, lock)
}

fn map_fetch_failure(error: &aros_fetch::FetchFailure) -> ContractError {
    match error.diagnostic().code {
        DiagnosticCode::FetchIntegrity => ContractError::sources(
            "a selected source-cache payload does not match its locked size or SHA-256",
        ),
        _ => ContractError::sources(
            "a selected source-cache payload is missing, unsafe or changed during verification",
        ),
    }
}

#[cfg(all(test, unix))]
mod m2_tests {
    use std::fs;

    use aros_common::sha256_bytes;
    use aros_fetch::engine::cache::CachePayloadNormalization;

    use super::{
        fetch_request, list, status, verify_request, SourceCacheEntryState, SourceCacheFetch,
        SourceCacheIntegrity, SourceCacheList, SourceCacheRequest, SourceCacheRequestKind,
        SourceCacheStatus, SourceCacheVerification, SOURCE_CACHE_FETCH_SCHEMA,
        SOURCE_CACHE_LIST_SCHEMA, SOURCE_CACHE_STATUS_SCHEMA, SOURCE_CACHE_VERIFY_SCHEMA,
    };
    use crate::source_cache_request::{SourceCacheCandidate, SourceCacheEntry};

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
                candidates: vec![SourceCacheCandidate {
                    filename: filename.to_owned(),
                    url: format!("https://example.invalid/{filename}"),
                }],
                normalization: CachePayloadNormalization::ExactBytesV1,
                integrity: SourceCacheIntegrity::Locked {
                    sha256: sha256_bytes(payload),
                    size: payload.len() as u64,
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

    #[tokio::test]
    async fn offline_fetch_reuses_only_a_verified_existing_object() {
        let temporary = real_tempdir();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let payload = b"exact locked payload";
        fs::write(cache.join("llvm.tar.xz"), payload).unwrap();
        let fetched: SourceCacheFetch =
            fetch_request(&cache, &strict_request("llvm.tar.xz", payload), true)
                .await
                .unwrap();
        assert_eq!(fetched.schema, SOURCE_CACHE_FETCH_SCHEMA);
        assert_eq!(fetched.operation, "sources.fetch");
        assert!(!fetched.side_effects.network);
        assert!(fetched.side_effects.hashes_payloads);
        assert_eq!(fetched.entries[0].sha256, sha256_bytes(payload));

        let error = fetch_request(&cache, &strict_request("missing.tar.xz", payload), true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing, unsafe or changed"));
        assert!(!cache.join("missing.tar.xz").exists());
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
