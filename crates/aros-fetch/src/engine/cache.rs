//! Public, no-follow cache payload snapshots for higher-level source contracts.
//!
//! This is intentionally narrower than the fetch CLI: the caller supplies an
//! already selected portable filename and expected identity, and receives an
//! immutable private copy.  It neither contacts a remote origin nor publishes
//! anything into a cache.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use aros_common::Sha256Digest;
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use tempfile::Builder;

use super::diagnostics::{cache_failure, contract_failure, integrity_failure, network_failure};
use super::locking::FetchLock;
use super::payload::PreparedPayload;
use crate::FetchResult;

const DIRECT_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const DIRECT_TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(15);
const DIRECT_RETRIES: usize = 3;

/// A verified private snapshot of one cache payload.
pub struct VerifiedCachePayload {
    payload: PreparedPayload,
    size: u64,
}

/// Acquire a direct HTTPS payload only when its exact cache entry is absent.
///
/// An existing payload is always snapshotted and verified first. A stale,
/// poisoned or unsafe existing entry therefore fails instead of being silently
/// replaced. Network transfer is permitted only when `offline` is false and
/// writes a no-clobber cache entry after exact byte-count and SHA-256 checks.
///
/// # Errors
///
/// Returns a normal `aros-fetch` contract/cache/network/integrity diagnostic.
/// Higher-level producer code maps that envelope to its AX0301 source boundary.
pub async fn acquire_https_cache_payload(
    cache_root: &Path,
    filename: &str,
    url: &str,
    expected_size: u64,
    expected_sha256: &Sha256Digest,
    offline: bool,
) -> FetchResult<VerifiedCachePayload> {
    if !cache_root.is_dir() {
        return Err(cache_failure(
            "verified cache root is not an existing directory",
        ));
    }
    if expected_size == 0 || !portable_basename(filename) {
        return Err(cache_failure(
            "verified cache payload declaration has an unsafe name or zero size",
        ));
    }
    let origin = validate_https_origin(url)?;
    let destination = direct_child(cache_root, filename)?;
    let _lock = FetchLock::acquire_candidate(&destination)?;
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            return snapshot_verified_cache_payload(
                cache_root,
                filename,
                expected_size,
                expected_sha256,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(cache_failure(
                "cannot inspect selected direct cache payload before acquisition",
            ))
        }
    }
    if offline {
        return Err(cache_failure(
            "offline mode forbids acquisition of a missing selected source payload",
        ));
    }
    let client = reqwest::Client::builder()
        .connect_timeout(DIRECT_CONNECT_TIMEOUT)
        .timeout(DIRECT_TRANSFER_TIMEOUT)
        .redirect(Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .user_agent(concat!("aros-fetch/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| network_failure("cannot initialize locked HTTPS transport"))?;
    download_exact_https(
        &client,
        &origin,
        cache_root,
        &destination,
        filename,
        expected_size,
        expected_sha256,
    )
    .await?;
    snapshot_verified_cache_payload(cache_root, filename, expected_size, expected_sha256)
}

impl VerifiedCachePayload {
    /// Path to the private immutable snapshot, not the mutable cache object.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.payload.path
    }

    /// Measured SHA-256 digest of the snapshot.
    #[must_use]
    pub const fn sha256(&self) -> &Sha256Digest {
        &self.payload.digest
    }

    /// Measured byte size of the snapshot.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Recheck the original cache object and private snapshot before a later
    /// source-consuming phase commits its work.
    ///
    /// # Errors
    ///
    /// Returns a stable cache failure if either object has changed.
    pub fn revalidate(&self) -> FetchResult<()> {
        self.payload.revalidate()
    }
}

/// Snapshot one direct, regular cache entry and verify its complete identity.
///
/// The cache root must already exist.  The function refuses symlinks through
/// the existing no-follow `aros-fetch` primitive, retains no cache mutation,
/// and never attempts transport.  The returned value owns a temporary private
/// copy so callers can safely extract or inspect it after cache verification.
///
/// # Errors
///
/// Returns a contract/cache/integrity diagnostic if the root/name is unsafe,
/// the payload is absent/non-regular/changing, or its measured size/digest does
/// not match the selected lock declaration.
pub fn snapshot_verified_cache_payload(
    cache_root: &Path,
    filename: &str,
    expected_size: u64,
    expected_sha256: &Sha256Digest,
) -> FetchResult<VerifiedCachePayload> {
    if !cache_root.is_dir() {
        return Err(cache_failure(
            "verified cache root is not an existing directory",
        ));
    }
    if expected_size == 0 || !portable_basename(filename) {
        return Err(cache_failure(
            "verified cache payload declaration has an unsafe name or zero size",
        ));
    }
    let path = direct_child(cache_root, filename)?;
    let payload = PreparedPayload::import(&path, filename, expected_size)?;
    if payload.digest != *expected_sha256 {
        return Err(integrity_failure(
            filename,
            "cached payload SHA-256 differs from the selected source lock",
        ));
    }
    let size = std::fs::metadata(&payload.path)
        .map_err(|_| cache_failure("cannot remeasure private cache payload snapshot"))?
        .len();
    if size != expected_size {
        return Err(integrity_failure(
            filename,
            "cached payload size differs from the selected source lock",
        ));
    }
    Ok(VerifiedCachePayload { payload, size })
}

fn direct_child(root: &Path, filename: &str) -> FetchResult<PathBuf> {
    let path = root.join(filename);
    if path.parent() != Some(root) {
        return Err(cache_failure(
            "cache payload must be a direct cache-root child",
        ));
    }
    Ok(path)
}

fn validate_https_origin(value: &str) -> FetchResult<reqwest::Url> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| contract_failure("selected direct source URL is invalid"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(contract_failure(
            "selected direct source URL must be credential-free HTTPS without a query or fragment",
        ));
    }
    Ok(url)
}

async fn download_exact_https(
    client: &reqwest::Client,
    origin: &reqwest::Url,
    cache_root: &Path,
    destination: &Path,
    filename: &str,
    expected_size: u64,
    expected_sha256: &Sha256Digest,
) -> FetchResult<()> {
    let mut last_failure = None;
    for _ in 0..DIRECT_RETRIES {
        let response = match client.get(origin.clone()).send().await {
            Ok(response)
                if response.status().is_success() && response.url().scheme() == "https" =>
            {
                response
            }
            Ok(response) if response.url().scheme() != "https" => {
                last_failure = Some("redirect did not preserve HTTPS".to_owned());
                continue;
            }
            Ok(response) => {
                last_failure = Some(format!("HTTP server returned status {}", response.status()));
                continue;
            }
            Err(error) => {
                last_failure = Some(http_failure_summary(&error));
                continue;
            }
        };
        if response
            .content_length()
            .is_some_and(|size| size != expected_size)
        {
            return Err(integrity_failure(
                filename,
                "HTTPS response content length differs from the selected source lock",
            ));
        }
        let mut staged = Builder::new()
            .prefix(".aros-fetch-direct-")
            .tempfile_in(cache_root)
            .map_err(|_| cache_failure("cannot create direct source transfer staging file"))?;
        let mut written = 0_u64;
        let mut stream = response.bytes_stream();
        let mut failed = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    written = written.checked_add(chunk.len() as u64).ok_or_else(|| {
                        integrity_failure(filename, "direct source transfer byte count overflowed")
                    })?;
                    if written > expected_size {
                        return Err(integrity_failure(
                            filename,
                            "direct source transfer exceeds its selected byte length",
                        ));
                    }
                    if let Err(error) = staged.write_all(&chunk) {
                        failed = Some(format!("cannot write transfer staging file: {error}"));
                        break;
                    }
                }
                Err(error) => {
                    failed = Some(http_failure_summary(&error));
                    break;
                }
            }
        }
        if let Some(failure) = failed {
            last_failure = Some(failure);
            continue;
        }
        if written != expected_size {
            return Err(integrity_failure(
                filename,
                "direct source transfer ended before its selected byte length",
            ));
        }
        staged
            .as_file_mut()
            .sync_all()
            .map_err(|_| cache_failure("cannot sync direct source transfer staging file"))?;
        let prepared = PreparedPayload::import(staged.path(), filename, expected_size)?;
        if prepared.digest != *expected_sha256 {
            return Err(integrity_failure(
                filename,
                "direct source transfer SHA-256 differs from the selected source lock",
            ));
        }
        prepared.revalidate()?;
        drop(prepared);
        super::payload::publish_download_noclobber(staged.path(), destination, filename)?;
        return Ok(());
    }
    Err(network_failure(format!(
        "locked HTTPS source transfer failed after {DIRECT_RETRIES} attempts: {}",
        last_failure.unwrap_or_else(|| "unknown transport failure".to_owned())
    )))
}

fn http_failure_summary(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "HTTPS request timed out".to_owned()
    } else if error.is_connect() {
        "HTTPS connection failed".to_owned()
    } else if error.is_redirect() {
        "HTTPS redirect policy rejected the response".to_owned()
    } else if error.is_decode() {
        "HTTPS response decoding failed".to_owned()
    } else {
        "HTTPS transport failed".to_owned()
    }
}

fn portable_basename(value: &str) -> bool {
    let path = Path::new(value);
    path.components().count() == 1
        && matches!(path.components().next(), Some(Component::Normal(_)))
        && !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use aros_common::sha256_bytes;

    use super::snapshot_verified_cache_payload;

    #[tokio::test]
    async fn offline_acquisition_uses_only_an_exact_existing_cache_entry() {
        let temporary = tempfile::tempdir().unwrap();
        let bytes = b"locked payload\n";
        fs::write(temporary.path().join("payload.tar.gz"), bytes).unwrap();
        let payload = super::acquire_https_cache_payload(
            temporary.path(),
            "payload.tar.gz",
            "https://example.invalid/payload.tar.gz",
            u64::try_from(bytes.len()).unwrap(),
            &sha256_bytes(bytes),
            true,
        )
        .await
        .unwrap();
        assert_eq!(fs::read(payload.path()).unwrap(), bytes);
        assert!(super::acquire_https_cache_payload(
            temporary.path(),
            "missing.tar.gz",
            "https://example.invalid/missing.tar.gz",
            1,
            &sha256_bytes(b"x"),
            true,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn direct_acquisition_rejects_an_ambiguous_origin_before_transport() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(super::acquire_https_cache_payload(
            temporary.path(),
            "payload.tar.gz",
            "https://example.invalid/payload.tar.gz?unreviewed=true",
            1,
            &sha256_bytes(b"x"),
            false,
        )
        .await
        .is_err());
    }

    #[test]
    fn private_snapshot_keeps_the_verified_cached_bytes() {
        let temporary = tempfile::tempdir().unwrap();
        let bytes = b"locked payload\n";
        fs::write(temporary.path().join("payload.tar.gz"), bytes).unwrap();
        let payload = snapshot_verified_cache_payload(
            temporary.path(),
            "payload.tar.gz",
            u64::try_from(bytes.len()).unwrap(),
            &sha256_bytes(bytes),
        )
        .unwrap();
        assert_eq!(fs::read(payload.path()).unwrap(), bytes);
        assert_eq!(payload.size(), u64::try_from(bytes.len()).unwrap());
        payload.revalidate().unwrap();
    }

    #[test]
    fn wrong_identity_never_returns_a_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        fs::write(temporary.path().join("payload.tar.gz"), b"wrong\n").unwrap();
        assert!(snapshot_verified_cache_payload(
            temporary.path(),
            "payload.tar.gz",
            6,
            &sha256_bytes(b"other\n"),
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_cache_entries_are_rejected() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        fs::write(temporary.path().join("real"), b"locked\n").unwrap();
        symlink("real", temporary.path().join("payload.tar.gz")).unwrap();
        assert!(snapshot_verified_cache_payload(
            temporary.path(),
            "payload.tar.gz",
            7,
            &sha256_bytes(b"locked\n"),
        )
        .is_err());
    }
}
