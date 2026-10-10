//! Source acquisition under an explicit archive-byte representation policy.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use aros_common::{DiagnosticContext, LogLevel, Sha256Digest};

use crate::contract::{ArchivePayloadOptions, FetchRequest};
use crate::observability::Logger;
use crate::FetchResult;

use super::cache::{
    acquire_https_cache_payload_with_normalization, snapshot_verified_cache_payload,
    CachePayloadNormalization, VerifiedCachePayload,
};
use super::diagnostics::{cache_failure, integrity_failure, network_failure, FailureHint};
use super::locking::FetchLock;
use super::payload::PreparedPayload;
use super::{expand_origin, report_failed_source, verify, Source, MAX_DOWNLOAD_BYTES};

pub(super) async fn fetch_canonical_archive(
    request: &FetchRequest,
    options: ArchivePayloadOptions,
    logger: &mut Logger,
) -> FetchResult<(PreparedPayload, Option<VerifiedCachePayload>)> {
    let candidate = &request.archive_candidates[0];
    let expected_size = options
        .normalized_size
        .expect("canonical payload options validate a size");
    let expected_digest = request
        .checksums
        .get(candidate)
        .expect("canonical payload options validate a checksum");

    if let Some(verified) = snapshot_canonical_cache_entry(
        &request.location,
        candidate,
        expected_size,
        expected_digest,
    )? {
        let payload = payload_from_verified_cache(&verified, candidate)?;
        return Ok((payload, Some(verified)));
    }

    let normalized_cache = request.location.join(".aros-fetch-canonical-tar-gzip-v1");
    if let Some(verified) = snapshot_canonical_cache_entry(
        &normalized_cache,
        candidate,
        expected_size,
        expected_digest,
    )? {
        let payload = payload_from_verified_cache(&verified, candidate)?;
        return Ok((payload, Some(verified)));
    }

    let mut attempts = Vec::new();
    let mut integrity = None;
    for (origin_index, origin) in request.archive_origins.iter().enumerate() {
        for source in expand_origin(origin, candidate)? {
            if request.offline && !matches!(source, Source::Local(_)) {
                attempts.push(format!(
                    "offline mode forbids HTTPS acquisition from declared origin {}",
                    origin_index + 1
                ));
                continue;
            }
            aros_common::outputln!(
                "Trying     normalized {candidate} from declared origin {}...",
                origin_index + 1
            );
            let event_context = DiagnosticContext {
                mode: Some(source.kind().to_owned()),
                output: Some(candidate.clone()),
                ..DiagnosticContext::default()
            };
            logger.event(
                LogLevel::Debug,
                "transfer.attempt",
                "declared canonical archive source attempted",
                &event_context,
            )?;
            let selected = match source {
                Source::Local(path) => {
                    match fs::symlink_metadata(&path) {
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            attempts.push(format!(
                                "local canonical payload '{}' is unavailable",
                                path.display()
                            ));
                            continue;
                        }
                        Err(error) => {
                            return Err(cache_failure(format!(
                                "cannot inspect local canonical payload '{}': {error}",
                                path.display()
                            )))
                        }
                        Ok(_) => {}
                    }
                    let payload = PreparedPayload::import(&path, candidate, MAX_DOWNLOAD_BYTES)?;
                    verify_canonical_size(&payload, candidate, expected_size)?;
                    verify(candidate, &payload, Some(expected_digest), logger)?;
                    logger.event(
                        LogLevel::Info,
                        "transfer.complete",
                        "declared canonical local archive verified",
                        &event_context,
                    )?;
                    return Ok((payload, None));
                }
                Source::Http(url) => {
                    let cache_root = ensure_canonical_cache_directory(&request.location)?;
                    acquire_https_cache_payload_with_normalization(
                        &cache_root,
                        candidate,
                        &url,
                        expected_size,
                        expected_digest,
                        CachePayloadNormalization::CanonicalTarGzipV1,
                        request.offline,
                    )
                    .await
                }
                Source::Ftp(_) => unreachable!(
                    "canonical archive option validation rejects non-HTTPS remote origins"
                ),
            };
            match selected {
                Ok(verified) => {
                    logger.event(
                        LogLevel::Info,
                        "transfer.complete",
                        "declared canonical archive source verified",
                        &event_context,
                    )?;
                    let payload = payload_from_verified_cache(&verified, candidate)?;
                    return Ok((payload, Some(verified)));
                }
                Err(error) => {
                    let reason = error.diagnostic().message.clone();
                    if error.diagnostic().code == aros_common::DiagnosticCode::FetchIntegrity {
                        integrity.get_or_insert(error);
                    }
                    report_failed_source(logger, candidate, origin_index, &reason, &event_context)?;
                    attempts.push(reason);
                }
            }
        }
    }
    if let Some(error) = integrity {
        return Err(error);
    }
    let detail = attempts
        .last()
        .map_or("no declared source was usable", String::as_str);
    if request.offline {
        return Err(cache_failure(format!(
            "offline mode has no verified canonical cache/local payload for '{candidate}': {detail}"
        ))
        .with_hint("seed the canonical payload cache or rerun without --offline"));
    }
    Err(network_failure(format!(
        "could not acquire canonical archive '{candidate}' from declared origins: {}",
        attempts.join("; ")
    ))
    .with_hint("check the declared HTTPS origins and final canonical size/SHA-256 identity"))
}

fn snapshot_canonical_cache_entry(
    cache_root: &Path,
    candidate: &str,
    expected_size: u64,
    expected_digest: &Sha256Digest,
) -> FetchResult<Option<VerifiedCachePayload>> {
    let root_metadata = match fs::symlink_metadata(cache_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(cache_failure(format!(
                "cannot inspect canonical archive cache '{}': {error}",
                cache_root.display()
            )))
        }
    };
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(cache_failure(format!(
            "canonical archive cache '{}' is not a real directory",
            cache_root.display()
        )));
    }
    let path = cache_root.join(candidate);
    let lock = FetchLock::acquire_candidate(&path)?;
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            let verified = snapshot_verified_cache_payload(
                cache_root,
                candidate,
                expected_size,
                expected_digest,
            )?;
            lock.revalidate()?;
            Ok(Some(verified))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(cache_failure(format!(
            "cannot inspect canonical archive cache entry '{}': {error}",
            path.display()
        ))),
    }
}

fn ensure_canonical_cache_directory(location: &Path) -> FetchResult<PathBuf> {
    let path = location.join(".aros-fetch-canonical-tar-gzip-v1");
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(cache_failure(format!(
                "cannot create canonical archive cache '{}': {error}",
                path.display()
            )))
        }
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        cache_failure(format!(
            "cannot inspect canonical archive cache '{}': {error}",
            path.display()
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(cache_failure(format!(
            "canonical archive cache '{}' is not a real directory",
            path.display()
        )));
    }
    Ok(path)
}

fn payload_from_verified_cache(
    verified: &VerifiedCachePayload,
    candidate: &str,
) -> FetchResult<PreparedPayload> {
    let payload = PreparedPayload::import(verified.path(), candidate, MAX_DOWNLOAD_BYTES)?;
    if payload.digest != *verified.sha256() {
        return Err(integrity_failure(
            candidate,
            "private canonical cache snapshot changed while it was re-imported",
        ));
    }
    Ok(payload)
}

fn verify_canonical_size(
    payload: &PreparedPayload,
    candidate: &str,
    expected_size: u64,
) -> FetchResult<()> {
    let actual_size = fs::metadata(&payload.path)
        .map_err(|_| cache_failure("cannot measure private canonical archive payload"))?
        .len();
    if actual_size != expected_size {
        return Err(integrity_failure(
            candidate,
            format!(
                "canonical payload size differs from --normalized-size (expected {expected_size}, actual {actual_size})"
            ),
        ));
    }
    Ok(())
}
