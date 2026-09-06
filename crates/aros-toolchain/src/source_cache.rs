//! Verification of the exact offline payload closure selected by a source lock.
//!
//! The cache is an input, never a source of truth.  A valid lock selects every
//! payload; `aros-fetch` snapshots and validates each direct cache entry before
//! this module reports it as usable.  Extra cache files are intentionally not a
//! failure: only actual source use can establish an undeclared-input violation.

use std::path::Path;

use aros_common::{DiagnosticCode, Sha256Digest};
use aros_fetch::engine::cache::snapshot_verified_cache_payload;

use crate::source_lock::SourceLock;
use crate::ContractError;

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
