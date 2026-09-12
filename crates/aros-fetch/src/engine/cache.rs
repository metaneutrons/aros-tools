//! Public, no-follow cache payload snapshots for higher-level source contracts.
//!
//! This is intentionally narrower than the fetch CLI: the caller supplies an
//! already selected portable filename and expected identity, and receives an
//! immutable private copy.  It neither contacts a remote origin nor publishes
//! anything into a cache.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};

use aros_common::{open_regular_file_nofollow, payload_casefold_path_key, Sha256Digest};
use flate2::read::MultiGzDecoder;
use flate2::{Compression, GzBuilder};
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use tempfile::{Builder, NamedTempFile};

use super::diagnostics::{cache_failure, contract_failure, integrity_failure, network_failure};
use super::locking::FetchLock;
use super::payload::PreparedPayload;
use crate::FetchResult;

const DIRECT_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
const DIRECT_TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(15);
const DIRECT_RETRIES: usize = 3;
const MAX_CANONICAL_TAR_TRANSPORT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CANONICAL_TAR_ENTRIES: u64 = 100_000;
const MAX_CANONICAL_TAR_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_CANONICAL_TAR_PATH_BYTES: usize = 1_024;
const MAX_CANONICAL_TAR_PATH_DEPTH: usize = 64;
const MAX_CANONICAL_TAR_END_PADDING_BYTES: u64 = 1_024;
// A TAR record occupies at least 512 bytes. This bounds headers, PAX/GNU
// metadata and trailing decoded data in addition to declared file contents.
const MAX_CANONICAL_TAR_DECODED_BYTES: u64 =
    MAX_CANONICAL_TAR_EXPANDED_BYTES + (MAX_CANONICAL_TAR_ENTRIES * 1_024);

/// Explicit representation policy for a directly acquired HTTPS cache object.
///
/// `CanonicalTarGzipV1` is reserved for an upstream that publishes an
/// immutable source tree in a transport archive with deliberately unstable
/// metadata. Its cached object is a deterministic gzip/tar representation of
/// the safely extracted tree, so the lock still binds exact bytes consumed by
/// the downstream build.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CachePayloadNormalization {
    /// The remote response bytes must equal the lock's size and SHA-256.
    #[default]
    ExactBytesV1,
    /// Safely rearchive a gzip/tar source tree with deterministic metadata.
    CanonicalTarGzipV1,
}

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
    acquire_https_cache_payload_with_normalization(
        cache_root,
        filename,
        url,
        expected_size,
        expected_sha256,
        CachePayloadNormalization::ExactBytesV1,
        offline,
    )
    .await
}

/// Acquire a direct HTTPS source after applying an explicit lock-selected
/// representation policy.
///
/// Existing cache entries are always verified as the final canonical payload;
/// source transport is used only when that entry is absent and offline mode is
/// disabled.
///
/// # Errors
///
/// Returns a structured fetch error when the lock identity is unsafe, an
/// existing cache object is absent or differs from its lock, the transport
/// cannot provide a permitted HTTPS response, or normalization cannot safely
/// construct the requested canonical payload.
pub async fn acquire_https_cache_payload_with_normalization(
    cache_root: &Path,
    filename: &str,
    url: &str,
    expected_size: u64,
    expected_sha256: &Sha256Digest,
    normalization: CachePayloadNormalization,
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
        // Lock identities describe the archive representation, not a
        // transparently decoded HTTP body. Keep response bytes raw so the
        // streaming length and SHA-256 checks below measure the exact object
        // received from the locked HTTPS origin.
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
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
    match normalization {
        CachePayloadNormalization::ExactBytesV1 => {
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
        }
        CachePayloadNormalization::CanonicalTarGzipV1 => {
            if !has_gzip_tar_extension(filename) {
                return Err(contract_failure(
                    "canonical tar-gzip normalization requires a .tar.gz or .tgz cache filename",
                ));
            }
            download_canonical_tar_gzip_https(
                &client,
                &origin,
                cache_root,
                &destination,
                filename,
                expected_size,
                expected_sha256,
            )
            .await?;
        }
    }
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
        // HTTP Content-Length describes the transferred representation. A
        // redirecting CDN may apply a transparent content encoding, so it is
        // not a stable assertion about the lock-owned payload bytes. The
        // streamed byte count and SHA-256 below are the authoritative,
        // end-to-end identity checks and retain the same bounded-write limit.
        let mut staged = Builder::new()
            .prefix(".aros-fetch-direct-")
            .tempfile_in(cache_root)
            .map_err(|_| cache_failure("cannot create direct source transfer staging file"))?;
        let response_url = response.url().as_str().to_owned();
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
                        failed = Some(format!(
                            "direct source transfer exceeded the selected byte length ({written} > {expected_size})"
                        ));
                        break;
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
            last_failure = Some(format!(
                "direct source transfer from {response_url} ended at {written} bytes; the selected source lock requires {expected_size} bytes"
            ));
            continue;
        }
        staged
            .as_file_mut()
            .sync_all()
            .map_err(|_| cache_failure("cannot sync direct source transfer staging file"))?;
        let prepared = PreparedPayload::import(staged.path(), filename, expected_size)?;
        if prepared.digest != *expected_sha256 {
            last_failure = Some(
                "direct source transfer SHA-256 differs from the selected source lock".to_owned(),
            );
            continue;
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

async fn download_canonical_tar_gzip_https(
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
        let response_url = response.url().as_str().to_owned();
        let mut raw = Builder::new()
            .prefix(".aros-fetch-normalize-source-")
            .tempfile_in(cache_root)
            .map_err(|_| cache_failure("cannot create source-normalization staging file"))?;
        let mut written = 0_u64;
        let mut stream = response.bytes_stream();
        let mut failed = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    written = written.checked_add(chunk.len() as u64).ok_or_else(|| {
                        integrity_failure(filename, "source-normalization byte count overflowed")
                    })?;
                    if written > MAX_CANONICAL_TAR_TRANSPORT_BYTES {
                        failed = Some(format!(
                            "source-normalization transport exceeds the {MAX_CANONICAL_TAR_TRANSPORT_BYTES}-byte limit"
                        ));
                        break;
                    }
                    if let Err(error) = raw.write_all(&chunk) {
                        failed = Some(format!(
                            "cannot write source-normalization staging file: {error}"
                        ));
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
        raw.as_file_mut()
            .sync_all()
            .map_err(|_| cache_failure("cannot sync source-normalization staging file"))?;
        let mut normalized = Builder::new()
            .prefix(".aros-fetch-canonical-")
            .tempfile_in(cache_root)
            .map_err(|_| cache_failure("cannot create canonical source staging file"))?;
        canonicalize_tar_gzip(raw.path(), filename, cache_root, &mut normalized)?;
        let prepared = PreparedPayload::import(normalized.path(), filename, expected_size)?;
        if prepared.digest != *expected_sha256 {
            last_failure = Some(format!(
                "canonical source from {response_url} SHA-256 differs from the selected source lock"
            ));
            continue;
        }
        prepared.revalidate()?;
        drop(prepared);
        super::payload::publish_download_noclobber(normalized.path(), destination, filename)?;
        return Ok(());
    }
    Err(network_failure(format!(
        "canonical locked HTTPS source transfer failed after {DIRECT_RETRIES} attempts: {}",
        last_failure.unwrap_or_else(|| "unknown transport failure".to_owned())
    )))
}

fn canonicalize_tar_gzip(
    source: &Path,
    filename: &str,
    cache_root: &Path,
    output: &mut NamedTempFile,
) -> FetchResult<()> {
    let extraction = Builder::new()
        .prefix(".aros-fetch-canonical-tree-")
        .tempdir_in(cache_root)
        .map_err(|_| cache_failure("cannot create canonical source extraction directory"))?;
    unpack_canonical_tar_gzip(source, filename, extraction.path())?;
    let entries = canonical_tree_entries(extraction.path(), filename)?;
    {
        let encoder = GzBuilder::new()
            .mtime(0)
            .operating_system(255)
            // Store deterministic TAR records instead of accepting a host
            // compression-library implementation as an input to the lock.
            .write(output.as_file_mut(), Compression::none());
        let mut archive = tar::Builder::new(encoder);
        for entry in entries {
            let mut header = tar::Header::new_gnu();
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            match entry.kind {
                CanonicalTreeEntryKind::Directory => {
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_cksum();
                    archive
                        .append_data(&mut header, &entry.path, std::io::empty())
                        .map_err(|_| {
                            integrity_failure(filename, "cannot write canonical source directory")
                        })?;
                }
                CanonicalTreeEntryKind::File { size, executable } => {
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_size(size);
                    header.set_mode(if executable { 0o755 } else { 0o644 });
                    header.set_cksum();
                    let mut file = open_regular_file_nofollow(&entry.absolute).map_err(|_| {
                        integrity_failure(filename, "cannot safely reopen canonical source file")
                    })?;
                    archive
                        .append_data(&mut header, &entry.path, &mut file)
                        .map_err(|_| {
                            integrity_failure(filename, "cannot write canonical source file")
                        })?;
                }
            }
        }
        archive
            .finish()
            .map_err(|_| integrity_failure(filename, "cannot finish canonical source TAR"))?;
        let mut encoder = archive
            .into_inner()
            .map_err(|_| integrity_failure(filename, "cannot finalize canonical source TAR"))?;
        encoder
            .try_finish()
            .map_err(|_| integrity_failure(filename, "cannot finish canonical source gzip"))?;
        encoder
            .get_mut()
            .sync_all()
            .map_err(|_| cache_failure("cannot sync canonical source staging file"))?;
    }
    Ok(())
}

fn unpack_canonical_tar_gzip(source: &Path, filename: &str, root: &Path) -> FetchResult<()> {
    let file = File::open(source)
        .map_err(|_| integrity_failure(filename, "cannot open source-normalization archive"))?;
    // MultiGzDecoder insists on a complete gzip stream and rejects trailing
    // non-gzip bytes. The bounded wrapper limits all decoded TAR records,
    // including data which appears after TAR's end marker.
    let decoder = MultiGzDecoder::new(file);
    let mut archive = tar::Archive::new(BoundedRead::new(decoder, MAX_CANONICAL_TAR_DECODED_BYTES));
    archive.set_preserve_mtime(false);
    archive.set_preserve_permissions(false);
    archive.set_preserve_ownerships(false);
    archive.set_unpack_xattrs(false);
    archive.set_overwrite(false);
    let mut nodes = BTreeMap::new();
    let mut collision_keys = BTreeSet::new();
    let mut count = 0_u64;
    let mut expanded = 0_u64;
    for entry in archive
        .entries()
        .map_err(|_| integrity_failure(filename, "cannot read source-normalization TAR index"))?
    {
        let mut entry = entry.map_err(|_| {
            integrity_failure(filename, "cannot read source-normalization TAR entry")
        })?;
        let path = entry
            .path()
            .map_err(|_| {
                integrity_failure(filename, "source-normalization TAR has an invalid path")
            })?
            .into_owned();
        let path = checked_canonical_input_path(&path, filename)?;
        let entry_type = entry.header().entry_type();
        if !entry_type.is_dir() && !entry_type.is_file() {
            return Err(integrity_failure(
                filename,
                "source-normalization TAR contains a non-regular entry",
            ));
        }
        count = count.checked_add(1).ok_or_else(|| {
            integrity_failure(filename, "source-normalization TAR entry count overflowed")
        })?;
        let size = entry.size();
        if entry_type.is_dir() && size != 0 {
            return Err(integrity_failure(
                filename,
                "source-normalization TAR directory has file content",
            ));
        }
        expanded = expanded.checked_add(size).ok_or_else(|| {
            integrity_failure(filename, "source-normalization TAR size overflowed")
        })?;
        if count > MAX_CANONICAL_TAR_ENTRIES || expanded > MAX_CANONICAL_TAR_EXPANDED_BYTES {
            return Err(integrity_failure(
                filename,
                "source-normalization TAR exceeds its reviewed extraction budget",
            ));
        }
        reserve_canonical_ancestors(&path, filename, &mut nodes, &mut collision_keys)?;
        if entry_type.is_dir() {
            reserve_canonical_directory(&path, true, filename, &mut nodes, &mut collision_keys)?;
            ensure_canonical_parents(root, &path, filename)?;
            ensure_canonical_directory(root, &path, filename)?;
        } else {
            reserve_canonical_file(&path, filename, &mut nodes, &mut collision_keys)?;
            write_canonical_file(root, &path, size, &mut entry, filename)?;
        }
    }
    if count == 0 {
        return Err(integrity_failure(
            filename,
            "source-normalization TAR contains no entries",
        ));
    }
    let mut decoder = archive.into_inner();
    let mut trailing = [0_u8; 8_192];
    let mut trailing_bytes = 0_u64;
    loop {
        let read = decoder.read(&mut trailing).map_err(|_| {
            integrity_failure(
                filename,
                "source-normalization gzip stream is truncated or invalid",
            )
        })?;
        if read == 0 {
            break;
        }
        trailing_bytes = trailing_bytes.checked_add(read as u64).ok_or_else(|| {
            integrity_failure(
                filename,
                "source-normalization TAR end padding byte count overflowed",
            )
        })?;
        if trailing_bytes > MAX_CANONICAL_TAR_END_PADDING_BYTES
            || trailing[..read].iter().any(|byte| *byte != 0)
        {
            return Err(integrity_failure(
                filename,
                "source-normalization TAR has data after its end marker",
            ));
        }
    }
    Ok(())
}

#[derive(Debug)]
enum CanonicalInputNode {
    Directory { explicit: bool },
    File,
}

struct BoundedRead<R> {
    inner: R,
    remaining: u64,
}

impl<R> BoundedRead<R> {
    const fn new(inner: R, remaining: u64) -> Self {
        Self { inner, remaining }
    }
}

impl<R: Read> Read for BoundedRead<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0_u8; 1];
            if self.inner.read(&mut probe)? == 0 {
                return Ok(0);
            }
            return Err(std::io::Error::other(
                "decoded source-normalization TAR exceeds its reviewed byte budget",
            ));
        }
        let requested = usize::try_from(self.remaining.min(buffer.len() as u64)).unwrap_or(0);
        let read = self.inner.read(&mut buffer[..requested])?;
        self.remaining = self.remaining.saturating_sub(read as u64);
        Ok(read)
    }
}

fn checked_canonical_input_path(path: &Path, filename: &str) -> FetchResult<PathBuf> {
    let mut normalized = PathBuf::new();
    let mut depth = 0_usize;
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(integrity_failure(
                filename,
                "source-normalization TAR contains a non-contained path",
            ));
        };
        depth = depth.checked_add(1).ok_or_else(|| {
            integrity_failure(filename, "source-normalization TAR path depth overflowed")
        })?;
        if depth > MAX_CANONICAL_TAR_PATH_DEPTH {
            return Err(integrity_failure(
                filename,
                "source-normalization TAR path exceeds its reviewed depth limit",
            ));
        }
        normalized.push(value);
    }
    if normalized.as_os_str().is_empty()
        || normalized.as_os_str().len() > MAX_CANONICAL_TAR_PATH_BYTES
    {
        return Err(integrity_failure(
            filename,
            "source-normalization TAR path exceeds its reviewed portability budget",
        ));
    }
    payload_casefold_path_key(&normalized).map_err(|_| {
        integrity_failure(
            filename,
            "source-normalization TAR path is not portable across supported hosts",
        )
    })?;
    Ok(normalized)
}

fn reserve_canonical_ancestors(
    path: &Path,
    filename: &str,
    nodes: &mut BTreeMap<PathBuf, CanonicalInputNode>,
    collision_keys: &mut BTreeSet<String>,
) -> FetchResult<()> {
    let mut ancestors = path
        .ancestors()
        .skip(1)
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    ancestors.reverse();
    for ancestor in ancestors {
        reserve_canonical_directory(&ancestor, false, filename, nodes, collision_keys)?;
    }
    Ok(())
}

fn reserve_canonical_directory(
    path: &Path,
    explicit: bool,
    filename: &str,
    nodes: &mut BTreeMap<PathBuf, CanonicalInputNode>,
    collision_keys: &mut BTreeSet<String>,
) -> FetchResult<()> {
    match nodes.get_mut(path) {
        Some(CanonicalInputNode::File) => Err(integrity_failure(
            filename,
            "source-normalization TAR reuses a regular-file path as a directory",
        )),
        Some(CanonicalInputNode::Directory {
            explicit: was_explicit,
        }) => {
            if explicit && *was_explicit {
                return Err(integrity_failure(
                    filename,
                    "source-normalization TAR contains a duplicate directory path",
                ));
            }
            *was_explicit |= explicit;
            Ok(())
        }
        None => {
            reserve_canonical_path_key(path, filename, collision_keys)?;
            reserve_canonical_node_budget(filename, nodes)?;
            nodes.insert(
                path.to_path_buf(),
                CanonicalInputNode::Directory { explicit },
            );
            Ok(())
        }
    }
}

fn reserve_canonical_file(
    path: &Path,
    filename: &str,
    nodes: &mut BTreeMap<PathBuf, CanonicalInputNode>,
    collision_keys: &mut BTreeSet<String>,
) -> FetchResult<()> {
    if nodes.contains_key(path) {
        return Err(integrity_failure(
            filename,
            "source-normalization TAR contains a duplicate or file-directory collision",
        ));
    }
    reserve_canonical_path_key(path, filename, collision_keys)?;
    reserve_canonical_node_budget(filename, nodes)?;
    nodes.insert(path.to_path_buf(), CanonicalInputNode::File);
    Ok(())
}

fn reserve_canonical_path_key(
    path: &Path,
    filename: &str,
    collision_keys: &mut BTreeSet<String>,
) -> FetchResult<()> {
    let key = payload_casefold_path_key(path).map_err(|_| {
        integrity_failure(
            filename,
            "source-normalization TAR path is not portable across supported hosts",
        )
    })?;
    if !collision_keys.insert(key) {
        return Err(integrity_failure(
            filename,
            "source-normalization TAR contains a duplicate or cross-host path collision",
        ));
    }
    Ok(())
}

fn reserve_canonical_node_budget(
    filename: &str,
    nodes: &BTreeMap<PathBuf, CanonicalInputNode>,
) -> FetchResult<()> {
    if nodes.len() >= usize::try_from(MAX_CANONICAL_TAR_ENTRIES).unwrap_or(usize::MAX) {
        return Err(integrity_failure(
            filename,
            "source-normalization TAR exceeds its reviewed node budget",
        ));
    }
    Ok(())
}

fn ensure_canonical_directory(root: &Path, relative: &Path, filename: &str) -> FetchResult<()> {
    let destination = root.join(relative);
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(integrity_failure(
            filename,
            "source-normalization TAR directory collides with an extracted entry",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&destination).map_err(|_| {
                integrity_failure(filename, "cannot create canonical source directory")
            })?;
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o700)).map_err(|_| {
                integrity_failure(filename, "cannot secure canonical source directory")
            })?;
            Ok(())
        }
        Err(_) => Err(integrity_failure(
            filename,
            "cannot inspect canonical source directory",
        )),
    }
}

fn ensure_canonical_parents(root: &Path, relative: &Path, filename: &str) -> FetchResult<()> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut accumulated = PathBuf::new();
    for component in parent.components() {
        let Component::Normal(value) = component else {
            return Err(integrity_failure(
                filename,
                "canonical source parent path escaped its extraction root",
            ));
        };
        accumulated.push(value);
        ensure_canonical_directory(root, &accumulated, filename)?;
    }
    Ok(())
}

fn write_canonical_file(
    root: &Path,
    relative: &Path,
    size: u64,
    entry: &mut tar::Entry<'_, impl Read>,
    filename: &str,
) -> FetchResult<()> {
    ensure_canonical_parents(root, relative, filename)?;
    let destination = root.join(relative);
    let executable =
        entry.header().mode().map_err(|_| {
            integrity_failure(filename, "source-normalization TAR has an invalid mode")
        })? & 0o111
            != 0;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&destination)
        .map_err(|_| integrity_failure(filename, "cannot create canonical source file"))?;
    let copied = std::io::copy(entry, &mut output)
        .map_err(|_| integrity_failure(filename, "cannot extract canonical source file"))?;
    if copied != size {
        return Err(integrity_failure(
            filename,
            "source-normalization TAR file ended before its declared length",
        ));
    }
    output
        .sync_all()
        .map_err(|_| integrity_failure(filename, "cannot sync canonical source file"))?;
    fs::set_permissions(
        &destination,
        fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
    )
    .map_err(|_| integrity_failure(filename, "cannot secure canonical source file"))?;
    Ok(())
}

#[derive(Debug)]
struct CanonicalTreeEntry {
    path: PathBuf,
    absolute: PathBuf,
    kind: CanonicalTreeEntryKind,
}

#[derive(Debug)]
enum CanonicalTreeEntryKind {
    Directory,
    File { size: u64, executable: bool },
}

fn canonical_tree_entries(root: &Path, filename: &str) -> FetchResult<Vec<CanonicalTreeEntry>> {
    let mut entries = Vec::new();
    collect_canonical_tree_entries(root, root, filename, &mut entries)?;
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn collect_canonical_tree_entries(
    root: &Path,
    current: &Path,
    filename: &str,
    entries: &mut Vec<CanonicalTreeEntry>,
) -> FetchResult<()> {
    let directory = fs::read_dir(current)
        .map_err(|_| integrity_failure(filename, "cannot read canonical source directory"))?;
    for child in directory {
        let child = child.map_err(|_| {
            integrity_failure(filename, "cannot read canonical source directory entry")
        })?;
        let absolute = child.path();
        let metadata = fs::symlink_metadata(&absolute).map_err(|_| {
            integrity_failure(filename, "cannot inspect canonical source directory entry")
        })?;
        let relative = absolute
            .strip_prefix(root)
            .map_err(|_| {
                integrity_failure(
                    filename,
                    "canonical source entry escaped its extraction root",
                )
            })?
            .to_path_buf();
        if metadata.file_type().is_symlink() {
            return Err(integrity_failure(
                filename,
                "canonical source tree contains a symbolic link",
            ));
        }
        if metadata.is_dir() {
            entries.push(CanonicalTreeEntry {
                path: relative,
                absolute: absolute.clone(),
                kind: CanonicalTreeEntryKind::Directory,
            });
            collect_canonical_tree_entries(root, &absolute, filename, entries)?;
        } else if metadata.is_file() {
            entries.push(CanonicalTreeEntry {
                path: relative,
                absolute,
                kind: CanonicalTreeEntryKind::File {
                    size: metadata.len(),
                    executable: metadata.permissions().mode() & 0o111 != 0,
                },
            });
        } else {
            return Err(integrity_failure(
                filename,
                "canonical source tree contains a non-regular entry",
            ));
        }
    }
    Ok(())
}

fn has_gzip_tar_extension(filename: &str) -> bool {
    filename
        .get(filename.len().saturating_sub(".tar.gz".len())..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".tar.gz"))
        || Path::new(filename)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tgz"))
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
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Write as _};
    use std::path::Path;

    use aros_common::sha256_bytes;
    use flate2::read::MultiGzDecoder;
    use flate2::{Compression, GzBuilder};

    use super::{canonicalize_tar_gzip, snapshot_verified_cache_payload};

    fn write_transport_variant(
        path: &Path,
        gzip_mtime: u32,
        tar_mtime: u64,
        reverse_entries: bool,
    ) {
        let file = File::create(path).unwrap();
        let encoder = GzBuilder::new()
            .mtime(gzip_mtime)
            .operating_system(3)
            .write(file, Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        let mut entries = vec![
            ("source", Vec::new(), 0o755, true),
            (
                "source/README",
                b"same source tree\n".to_vec(),
                0o644,
                false,
            ),
            (
                "source/configure",
                b"#!/bin/sh\nexit 0\n".to_vec(),
                0o755,
                false,
            ),
        ];
        if reverse_entries {
            entries.reverse();
        }
        for (name, contents, mode, directory) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_uid(1_234);
            header.set_gid(5_678);
            header.set_mtime(tar_mtime);
            header.set_mode(mode);
            if directory {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_cksum();
                archive.append(&header, io::empty()).unwrap();
            } else {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(u64::try_from(contents.len()).unwrap());
                header.set_cksum();
                archive.append(&header, contents.as_slice()).unwrap();
            }
        }
        let mut encoder = archive.into_inner().unwrap();
        encoder.try_finish().unwrap();
    }

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

    #[test]
    fn canonical_tar_gzip_normalization_removes_transport_metadata_and_order() {
        let temporary = tempfile::tempdir().unwrap();
        let first = temporary.path().join("first.tar.gz");
        let second = temporary.path().join("second.tar.gz");
        write_transport_variant(&first, 1, 123, false);
        write_transport_variant(&second, 2_000_000_000, 987_654, true);
        assert_ne!(fs::read(&first).unwrap(), fs::read(&second).unwrap());

        let mut first_normalized = tempfile::Builder::new()
            .tempfile_in(temporary.path())
            .unwrap();
        let mut second_normalized = tempfile::Builder::new()
            .tempfile_in(temporary.path())
            .unwrap();
        canonicalize_tar_gzip(
            &first,
            "fixture.tar.gz",
            temporary.path(),
            &mut first_normalized,
        )
        .unwrap();
        canonicalize_tar_gzip(
            &second,
            "fixture.tar.gz",
            temporary.path(),
            &mut second_normalized,
        )
        .unwrap();

        assert_eq!(
            fs::read(first_normalized.path()).unwrap(),
            fs::read(second_normalized.path()).unwrap()
        );
    }

    #[test]
    fn canonical_tar_gzip_rejects_symbolic_links() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("unsafe.tar.gz");
        let file = File::create(&source).unwrap();
        let encoder = GzBuilder::new().mtime(1).write(file, Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_path("source/link").unwrap();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_link_name("../../outside").unwrap();
        header.set_size(0);
        header.set_mode(0o777);
        header.set_cksum();
        archive.append(&header, io::empty()).unwrap();
        let mut encoder = archive.into_inner().unwrap();
        encoder.try_finish().unwrap();

        let mut normalized = tempfile::Builder::new()
            .tempfile_in(temporary.path())
            .unwrap();
        assert!(
            canonicalize_tar_gzip(&source, "unsafe.tar.gz", temporary.path(), &mut normalized,)
                .is_err()
        );
    }

    #[test]
    fn canonical_tar_gzip_rejects_duplicate_and_cross_host_colliding_paths() {
        let temporary = tempfile::tempdir().unwrap();
        for (name, paths) in [
            ("duplicate.tar.gz", ["source/same", "source/same"]),
            ("alias.tar.gz", ["source/same", "source/./same"]),
            ("casefold.tar.gz", ["source/Foo", "source/foo"]),
        ] {
            let source = temporary.path().join(name);
            let file = File::create(&source).unwrap();
            let encoder = GzBuilder::new().mtime(1).write(file, Compression::fast());
            let mut archive = tar::Builder::new(encoder);
            for path in paths {
                let mut header = tar::Header::new_gnu();
                header.set_path(path).unwrap();
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(1);
                header.set_mode(0o644);
                header.set_cksum();
                archive.append(&header, &b"x"[..]).unwrap();
            }
            let mut encoder = archive.into_inner().unwrap();
            encoder.try_finish().unwrap();

            let mut normalized = tempfile::Builder::new()
                .tempfile_in(temporary.path())
                .unwrap();
            assert!(
                canonicalize_tar_gzip(&source, name, temporary.path(), &mut normalized,).is_err()
            );
        }
    }

    #[test]
    fn canonical_tar_gzip_rejects_a_truncated_gzip_stream() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("truncated.tar.gz");
        write_transport_variant(&source, 1, 123, false);
        let mut bytes = fs::read(&source).unwrap();
        bytes.truncate(bytes.len() - 4);
        fs::write(&source, bytes).unwrap();

        let mut normalized = tempfile::Builder::new()
            .tempfile_in(temporary.path())
            .unwrap();
        assert!(canonicalize_tar_gzip(
            &source,
            "truncated.tar.gz",
            temporary.path(),
            &mut normalized,
        )
        .is_err());
    }

    #[test]
    fn canonical_tar_gzip_rejects_decoded_data_after_the_tar_end_marker() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("trailing-member.tar.gz");
        write_transport_variant(&source, 1, 123, false);
        let output = OpenOptions::new().append(true).open(&source).unwrap();
        let mut encoder = GzBuilder::new().mtime(2).write(output, Compression::fast());
        encoder.write_all(b"unexpected second member").unwrap();
        encoder.try_finish().unwrap();

        let mut normalized = tempfile::Builder::new()
            .tempfile_in(temporary.path())
            .unwrap();
        assert!(canonicalize_tar_gzip(
            &source,
            "trailing-member.tar.gz",
            temporary.path(),
            &mut normalized,
        )
        .is_err());
    }

    #[test]
    fn canonical_tar_gzip_preserves_a_gnu_long_path_deterministically() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("long-path.tar.gz");
        let long_path = format!("source/{}", "component-".repeat(16));
        assert!(long_path.len() > 100);
        let file = File::create(&source).unwrap();
        let encoder = GzBuilder::new().mtime(1).write(file, Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(4);
        header.set_mode(0o644);
        archive
            .append_data(&mut header, &long_path, &b"data"[..])
            .unwrap();
        let mut encoder = archive.into_inner().unwrap();
        encoder.try_finish().unwrap();

        let mut normalized = tempfile::Builder::new()
            .tempfile_in(temporary.path())
            .unwrap();
        canonicalize_tar_gzip(
            &source,
            "long-path.tar.gz",
            temporary.path(),
            &mut normalized,
        )
        .unwrap();
        let mut output =
            tar::Archive::new(MultiGzDecoder::new(File::open(normalized.path()).unwrap()));
        let paths = output
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().into_owned())
            .collect::<Vec<_>>();
        assert!(paths.contains(&std::path::PathBuf::from(long_path)));
    }
}
