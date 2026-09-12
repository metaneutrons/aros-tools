//! Exact offline compatibility inputs for upstream `includes` and `linklibs`.
//!
//! The pinned upstream Makefiles otherwise fetch missing port archives and the
//! mutable Unicode `latest` files themselves. A release qualification must
//! never permit those implicit network inputs. This module therefore owns a
//! deliberately small, profile-bound lock format, verifies or acquires every
//! direct cache payload, and
//! materializes a fresh private ports-source directory for `configure`. The
//! payload files are read-only; the owner-only directory remains writable
//! solely because upstream `fetch.sh` creates and removes a transient
//! `.fetch` lock and records a declared empty `.fetched` marker beside an
//! already verified archive. The executor removes only the declared markers
//! before it revalidates the exact source tree for its receipt.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use aros_common::{open_regular_file_nofollow, sha256_reader, Sha256Digest, Sha256Result};
use aros_fetch::engine::cache::{
    acquire_https_cache_payload_with_normalization, snapshot_verified_cache_payload,
    CachePayloadNormalization, VerifiedCachePayload,
};
use serde::Deserialize;
use url::Url;

use crate::filesystem::open_directory;
use crate::recipe::GitObjectId;
use crate::ContractError;

const SCHEMA: &str = "aros-toolchain-compatibility-ports-v2";
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;
const MAX_INPUTS: usize = 128;
const MAX_PROFILES: usize = 32;

/// Exact declared source-input closure for upstream `includes` and `linklibs`.
#[derive(Debug, Clone)]
pub struct CompatibilityPortsLock(Record);

/// One measured direct source input selected by a ports lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPortsPayload {
    /// Stable lock-local identifier selected by a named target profile.
    pub id: String,
    /// Portable cache filename; it never controls the materialized path.
    pub cache_filename: String,
    /// Safe relative path below the private upstream source directory.
    pub relative_path: String,
    /// Exact safe relative path of the empty marker which upstream records
    /// after unpacking, if any.
    pub fetch_marker: String,
    /// Explicit transport representation policy for the measured cache object.
    pub normalization: CachePayloadNormalization,
    /// Official immutable HTTPS location.
    pub url: String,
    /// Complete SHA-256 identity.
    pub sha256: Sha256Digest,
    /// Exact byte size.
    pub size: u64,
}

/// Complete cache observation for one ports lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPortsCache {
    /// Inputs in stable cache-filename order.
    pub payloads: Vec<CompatibilityPortsPayload>,
}

/// Fresh, private and revalidatable `--with-portssources` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPortsSources {
    /// Canonical private directory passed to upstream configure.
    pub root: PathBuf,
    payloads: BTreeMap<String, CompatibilityPortsPayload>,
    fetch_markers: BTreeSet<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: String,
    upstream_commit: GitObjectId,
    inputs: Vec<Input>,
    profiles: Vec<ProfileInputs>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    id: String,
    cache_filename: String,
    relative_path: String,
    fetch_marker: String,
    #[serde(default)]
    normalization: CachePayloadNormalization,
    url: String,
    #[serde(deserialize_with = "digest")]
    sha256: Sha256Digest,
    size: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileInputs {
    name: String,
    inputs: Vec<String>,
}

impl CompatibilityPortsLock {
    /// Parse and close the small source-input contract without I/O.
    ///
    /// Every input names its cache identity and independent materialized path.
    /// The lock binds those inputs to one immutable upstream revision and to
    /// explicit target-profile selections; a new upstream fetch must therefore
    /// be reviewed as a lock update rather than reaching the network silently.
    ///
    /// # Errors
    ///
    /// Returns AX0101 when the document is malformed, non-canonical, mutable,
    /// or does not declare the exact upstream source-input closure.
    pub fn parse(bytes: &[u8]) -> Result<Self, ContractError> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid(
                "compatibility ports lock exceeds 64 KiB",
            ));
        }
        let record: Record = serde_json::from_slice(bytes)
            .map_err(|_| ContractError::invalid("compatibility ports lock is not valid JSON"))?;
        validate(&record)?;
        Ok(Self(record))
    }

    /// Immutable upstream revision for which this closure was derived.
    #[must_use]
    pub const fn upstream_commit(&self) -> &GitObjectId {
        &self.0.upstream_commit
    }

    /// All lock inputs in stable cache-filename order.
    #[must_use]
    pub fn payloads(&self) -> Vec<CompatibilityPortsPayload> {
        let mut payloads = self
            .0
            .inputs
            .iter()
            .map(|input| CompatibilityPortsPayload {
                id: input.id.clone(),
                cache_filename: input.cache_filename.clone(),
                relative_path: input.relative_path.clone(),
                fetch_marker: input.fetch_marker.clone(),
                normalization: input.normalization,
                url: input.url.clone(),
                sha256: input.sha256.clone(),
                size: input.size,
            })
            .collect::<Vec<_>>();
        payloads.sort_by(|left, right| left.cache_filename.cmp(&right.cache_filename));
        payloads
    }

    /// Select the full declared closure for one profile and pinned upstream
    /// revision.
    ///
    /// # Errors
    ///
    /// Returns AX0101 when the caller's source identity or profile does not
    /// match the reviewed lock contract.
    pub fn select(
        &self,
        upstream_commit: &GitObjectId,
        profile: &str,
    ) -> Result<Vec<CompatibilityPortsPayload>, ContractError> {
        if &self.0.upstream_commit != upstream_commit {
            return Err(ContractError::invalid(
                "compatibility ports lock upstream revision does not match the selected profile",
            ));
        }
        let selection = self
            .0
            .profiles
            .iter()
            .find(|selection| selection.name == profile)
            .ok_or_else(|| {
                ContractError::invalid(
                    "compatibility ports lock does not declare the selected profile",
                )
            })?;
        let inputs = self
            .0
            .inputs
            .iter()
            .map(|input| (input.id.as_str(), input))
            .collect::<BTreeMap<_, _>>();
        let mut selected = selection
            .inputs
            .iter()
            .map(|id| {
                let input = inputs.get(id.as_str()).ok_or_else(|| {
                    ContractError::invalid(
                        "compatibility ports profile selects an undeclared input",
                    )
                })?;
                Ok(CompatibilityPortsPayload {
                    id: input.id.clone(),
                    cache_filename: input.cache_filename.clone(),
                    relative_path: input.relative_path.clone(),
                    fetch_marker: input.fetch_marker.clone(),
                    normalization: input.normalization,
                    url: input.url.clone(),
                    sha256: input.sha256.clone(),
                    size: input.size,
                })
            })
            .collect::<Result<Vec<_>, ContractError>>()?;
        selected.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(selected)
    }
}

/// Verify that every selected compatibility payload is already present in `cache`.
///
/// # Errors
///
/// Returns AX0301 when the cache or one selected payload is absent, unsafe, or
/// differs from its measured lock identity.
pub fn verify_cache(
    cache: &Path,
    lock: &CompatibilityPortsLock,
) -> Result<CompatibilityPortsCache, ContractError> {
    let payloads = lock.payloads();
    let snapshots = snapshots(cache, &payloads)?;
    let payloads = snapshots
        .iter()
        .map(|(_, payload)| payload.clone())
        .collect();
    Ok(CompatibilityPortsCache { payloads })
}

/// Acquire only missing lock-selected compatibility payloads, then verify all of them.
///
/// # Errors
///
/// Returns AX0301 when the cache is unsafe, a transport fails, offline mode
/// encounters a cache miss, or any payload differs from its lock identity.
pub async fn acquire_cache(
    cache: &Path,
    lock: &CompatibilityPortsLock,
    offline: bool,
) -> Result<CompatibilityPortsCache, ContractError> {
    let cache = checked_cache(cache)?;
    for payload in lock.payloads() {
        let snapshot = acquire_https_cache_payload_with_normalization(
            &cache,
            &payload.cache_filename,
            &payload.url,
            payload.size,
            &payload.sha256,
            payload.normalization,
            offline,
        )
        .await
        .map_err(|error| {
            ContractError::sources(format!(
                "compatibility ports input '{}' could not be acquired or verified: {error}",
                payload.id
            ))
        })?;
        snapshot.revalidate().map_err(|_| {
            ContractError::sources(format!(
                "compatibility ports input '{}' changed after acquisition",
                payload.id
            ))
        })?;
    }
    verify_cache(&cache, lock)
}

/// Materialize a fresh private `--with-portssources` directory from verified
/// cache snapshots. The selected cache is never passed to upstream.
///
/// Payloads are immutable and revalidated before and after the upstream phase.
/// The private directory is writable only for the selected upstream fetch-lock
/// protocol.
///
/// # Errors
///
/// Returns AX0301 for an unsafe cache input and AX0703 when the output root or
/// any copied file cannot be created, sealed, or revalidated exactly.
pub fn materialize(
    cache: &Path,
    lock: &CompatibilityPortsLock,
    upstream_commit: &GitObjectId,
    profile: &str,
    output_root: &Path,
) -> Result<CompatibilityPortsSources, ContractError> {
    let output_root =
        checked_absent_directory(output_root, "compatibility ports output directory")?;
    let snapshots = snapshots(cache, &lock.select(upstream_commit, profile)?)?;
    fs::create_dir(&output_root).map_err(|_| {
        ContractError::compatibility("cannot create fresh compatibility ports output directory")
    })?;
    fs::set_permissions(&output_root, fs::Permissions::from_mode(0o700)).map_err(|_| {
        ContractError::compatibility("cannot restrict compatibility ports output directory")
    })?;

    let mut payloads = BTreeMap::new();
    let mut fetch_markers = BTreeSet::new();
    for (snapshot, payload) in snapshots {
        let destination = output_root.join(&payload.relative_path);
        let parent = destination.parent().ok_or_else(|| {
            ContractError::compatibility(
                "compatibility ports source payload has no parent directory",
            )
        })?;
        create_private_parents(&output_root, parent)?;
        copy_snapshot(&snapshot, &destination, &payload)?;
        if !payload.fetch_marker.is_empty() {
            fetch_markers.insert(payload.fetch_marker.clone());
        }
        payloads.insert(payload.relative_path.clone(), payload);
    }
    let sources = CompatibilityPortsSources {
        root: checked_directory(&output_root, "compatibility ports output directory")?,
        payloads,
        fetch_markers,
    };
    sources.revalidate()?;
    Ok(sources)
}

impl CompatibilityPortsSources {
    /// Materialized inputs in stable relative-path order, suitable for durable
    /// compatibility evidence without exposing a runner-local directory.
    #[must_use]
    pub fn payloads(&self) -> Vec<CompatibilityPortsPayload> {
        self.payloads.values().cloned().collect()
    }

    /// Recheck the private materialization before it is supplied to upstream.
    ///
    /// # Errors
    ///
    /// Returns AX0703 when the owned directory layout, file types, sizes, or
    /// SHA-256 identities differ from the materialized lock closure.
    pub fn revalidate(&self) -> Result<(), ContractError> {
        let root = checked_directory(&self.root, "compatibility ports source directory")?;
        if root != self.root || self.payloads.is_empty() {
            return Err(ContractError::compatibility(
                "compatibility ports source directory changed after materialization",
            ));
        }
        let root_metadata = fs::symlink_metadata(&root).map_err(|_| {
            ContractError::compatibility("cannot inspect compatibility ports source directory")
        })?;
        if !root_metadata.is_dir()
            || root_metadata.file_type().is_symlink()
            || root_metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(ContractError::compatibility(
                "compatibility ports source directory is not owner-private",
            ));
        }
        let expected_paths = self
            .payloads
            .keys()
            .map(PathBuf::from)
            .collect::<BTreeSet<_>>();
        let actual_paths = collect_materialized_paths(&root)?;
        if actual_paths != expected_paths {
            return Err(ContractError::compatibility(
                "compatibility ports source directory contains unmeasured or missing entries",
            ));
        }
        for (relative_path, expected) in &self.payloads {
            let path = root.join(relative_path);
            let metadata = fs::symlink_metadata(&path).map_err(|_| {
                ContractError::compatibility("cannot inspect a compatibility ports source payload")
            })?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() != expected.size
                || metadata.permissions().mode() & 0o777 != 0o400
            {
                return Err(ContractError::compatibility(
                    "compatibility ports source payload changed after materialization",
                ));
            }
            let mut file = open_regular_file_nofollow(&path).map_err(|_| {
                ContractError::compatibility(
                    "cannot safely open a compatibility ports source payload",
                )
            })?;
            let measured = sha256_reader(&mut file).map_err(|_| {
                ContractError::compatibility("cannot measure a compatibility ports source payload")
            })?;
            if !matches_payload(&measured, expected) {
                return Err(ContractError::compatibility(
                    "compatibility ports source payload identity changed after materialization",
                ));
            }
        }
        Ok(())
    }

    /// Remove only the reviewed empty markers left by successful upstream port
    /// fetches, then leave every other unexpected entry for `revalidate` to
    /// reject.
    ///
    /// # Errors
    ///
    /// Returns AX0703 if a declared marker is unsafe, non-empty, or cannot be
    /// removed durably. An absent declared marker is valid for a phase which
    /// did not need that port.
    pub fn clear_upstream_fetch_markers(&self) -> Result<(), ContractError> {
        let root = checked_directory(&self.root, "compatibility ports source directory")?;
        if root != self.root {
            return Err(ContractError::compatibility(
                "compatibility ports source directory changed before marker cleanup",
            ));
        }
        for marker in &self.fetch_markers {
            let path = root.join(marker);
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !metadata.is_file()
                        || metadata.file_type().is_symlink()
                        || metadata.len() != 0
                    {
                        return Err(ContractError::compatibility(
                            "upstream compatibility fetch marker is not an empty regular file",
                        ));
                    }
                    fs::remove_file(&path).map_err(|_| {
                        ContractError::compatibility(
                            "cannot remove an upstream compatibility fetch marker",
                        )
                    })?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(ContractError::compatibility(
                        "cannot inspect an upstream compatibility fetch marker",
                    ));
                }
            }
        }
        open_directory(&root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| {
                ContractError::compatibility(
                    "cannot durably clear upstream compatibility fetch markers",
                )
            })
    }
}

fn snapshots(
    cache: &Path,
    payloads: &[CompatibilityPortsPayload],
) -> Result<Vec<(VerifiedCachePayload, CompatibilityPortsPayload)>, ContractError> {
    let cache = checked_cache(cache)?;
    payloads
        .iter()
        .cloned()
        .map(|payload| {
            let snapshot = snapshot_verified_cache_payload(
                &cache,
                &payload.cache_filename,
                payload.size,
                &payload.sha256,
            )
            .map_err(|_| {
                ContractError::sources(
                    "a compatibility ports input is missing, unsafe, or differs from its lock",
                )
            })?;
            snapshot.revalidate().map_err(|_| {
                ContractError::sources("a compatibility ports input changed during verification")
            })?;
            Ok((snapshot, payload))
        })
        .collect()
}

fn copy_snapshot(
    snapshot: &VerifiedCachePayload,
    destination: &Path,
    payload: &CompatibilityPortsPayload,
) -> Result<(), ContractError> {
    let mut source = open_regular_file_nofollow(snapshot.path()).map_err(|_| {
        ContractError::compatibility("cannot safely open a verified compatibility ports snapshot")
    })?;
    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| {
            ContractError::compatibility("cannot create a compatibility ports source payload")
        })?;
    let mut remaining = payload.size;
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let limit = usize::try_from(remaining.min(buffer.len() as u64)).map_err(|_| {
            ContractError::compatibility(
                "compatibility ports source payload size is not representable",
            )
        })?;
        let read = source.read(&mut buffer[..limit]).map_err(|_| {
            ContractError::compatibility("cannot read a verified compatibility ports snapshot")
        })?;
        if read == 0 {
            return Err(ContractError::compatibility(
                "verified compatibility ports snapshot ended before its locked size",
            ));
        }
        destination_file.write_all(&buffer[..read]).map_err(|_| {
            ContractError::compatibility("cannot write a compatibility ports source payload")
        })?;
        remaining = remaining.saturating_sub(read as u64);
    }
    let mut trailing = [0_u8; 1];
    if source.read(&mut trailing).map_err(|_| {
        ContractError::compatibility("cannot finish reading a compatibility ports snapshot")
    })? != 0
    {
        return Err(ContractError::compatibility(
            "verified compatibility ports snapshot exceeds its locked size",
        ));
    }
    destination_file.sync_all().map_err(|_| {
        ContractError::compatibility("cannot durably write a compatibility ports source payload")
    })?;
    drop(destination_file);
    fs::set_permissions(destination, fs::Permissions::from_mode(0o400)).map_err(|_| {
        ContractError::compatibility("cannot seal a compatibility ports source payload")
    })?;
    snapshot.revalidate().map_err(|_| {
        ContractError::compatibility(
            "compatibility ports cache input changed during materialization",
        )
    })?;
    let mut copied = open_regular_file_nofollow(destination).map_err(|_| {
        ContractError::compatibility("cannot reopen a compatibility ports source payload")
    })?;
    let measured = sha256_reader(&mut copied).map_err(|_| {
        ContractError::compatibility("cannot measure a compatibility ports source payload")
    })?;
    if !matches_payload(&measured, payload) {
        return Err(ContractError::compatibility(
            "materialized compatibility ports source payload differs from its lock",
        ));
    }
    Ok(())
}

fn validate(record: &Record) -> Result<(), ContractError> {
    if record.schema != SCHEMA
        || record.inputs.is_empty()
        || record.inputs.len() > MAX_INPUTS
        || record.profiles.is_empty()
        || record.profiles.len() > MAX_PROFILES
    {
        return Err(ContractError::invalid(
            "compatibility ports lock has an unsupported schema or invalid closure size",
        ));
    }
    let mut identifiers = BTreeSet::new();
    let mut cache_filenames = BTreeSet::new();
    let mut relative_paths = BTreeSet::new();
    let mut fetch_markers = BTreeSet::new();
    for input in &record.inputs {
        if !identifier(&input.id)
            || !portable_filename(&input.cache_filename)
            || !safe_relative_path(&input.relative_path)
            || (input.normalization == CachePayloadNormalization::CanonicalTarGzipV1
                && !canonical_tar_gzip_filename(&input.cache_filename))
            || (!input.fetch_marker.is_empty()
                && (!safe_fetch_marker_path(&input.fetch_marker)
                    || !fetch_markers.insert(input.fetch_marker.as_str())))
            || !https_url(&input.url)
            || !identifiers.insert(input.id.as_str())
            || !cache_filenames.insert(input.cache_filename.as_str())
            || !relative_paths.insert(input.relative_path.as_str())
            || input.size == 0
            || input.size > MAX_PAYLOAD_BYTES
        {
            return Err(ContractError::invalid(
                "compatibility ports lock contains an invalid or duplicate source input",
            ));
        }
    }
    if fetch_markers
        .iter()
        .any(|marker| relative_paths.contains(marker))
    {
        return Err(ContractError::invalid(
            "compatibility ports lock reuses a source path as an upstream fetch marker",
        ));
    }
    let mut profiles = BTreeSet::new();
    for profile in &record.profiles {
        if !profile_identifier(&profile.name)
            || !profiles.insert(profile.name.as_str())
            || profile.inputs.is_empty()
            || profile.inputs.len() > record.inputs.len()
        {
            return Err(ContractError::invalid(
                "compatibility ports lock contains an invalid or duplicate profile selection",
            ));
        }
        let mut selected = BTreeSet::new();
        if profile
            .inputs
            .iter()
            .any(|id| !identifiers.contains(id.as_str()) || !selected.insert(id.as_str()))
        {
            return Err(ContractError::invalid(
                "compatibility ports profile selects a missing or duplicate source input",
            ));
        }
    }
    Ok(())
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn profile_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

fn portable_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains('/')
        && !value.contains('\\')
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+._-".contains(&byte))
}

fn canonical_tar_gzip_filename(value: &str) -> bool {
    value
        .get(value.len().saturating_sub(".tar.gz".len())..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".tar.gz"))
        || Path::new(value)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tgz"))
}

pub(crate) fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|part| portable_filename(part) && part != "." && part != "..")
}

/// Whether `value` is a safe relative path for an upstream fetch marker.
///
/// Fetch markers may deliberately reside beside a nested upstream source, for
/// example `codesets/.6.22-fetched`; callers outside this crate use this
/// predicate when they verify persisted compatibility evidence.
#[must_use]
pub fn safe_fetch_marker_path(value: &str) -> bool {
    let basename = value.rsplit('/').next().unwrap_or_default();
    safe_relative_path(value) && basename.starts_with('.') && basename.ends_with("-fetched")
}

fn https_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.has_host()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn single_segment(value: &str) -> bool {
    portable_filename(value) && !value.contains('/')
}

fn matches_payload(measured: &Sha256Result, expected: &CompatibilityPortsPayload) -> bool {
    (measured.size, &measured.digest) == (expected.size, &expected.sha256)
}

fn create_private_parents(root: &Path, parent: &Path) -> Result<(), ContractError> {
    let relative = parent.strip_prefix(root).map_err(|_| {
        ContractError::compatibility("compatibility ports source parent escapes its private root")
    })?;
    let mut current = root.to_owned();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(ContractError::compatibility(
                "compatibility ports source parent has an unsafe path component",
            ));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if !metadata.is_dir()
                    || metadata.file_type().is_symlink()
                    || metadata.permissions().mode() & 0o777 != 0o700
                {
                    return Err(ContractError::compatibility(
                        "compatibility ports source parent is not a private real directory",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|_| {
                    ContractError::compatibility(
                        "cannot create a compatibility ports source parent directory",
                    )
                })?;
                fs::set_permissions(&current, fs::Permissions::from_mode(0o700)).map_err(|_| {
                    ContractError::compatibility(
                        "cannot restrict a compatibility ports source parent directory",
                    )
                })?;
            }
            Err(_) => {
                return Err(ContractError::compatibility(
                    "cannot inspect a compatibility ports source parent directory",
                ));
            }
        }
    }
    Ok(())
}

fn collect_materialized_paths(root: &Path) -> Result<BTreeSet<PathBuf>, ContractError> {
    let mut paths = BTreeSet::new();
    collect_materialized_paths_at(root, Path::new(""), &mut paths)?;
    Ok(paths)
}

fn collect_materialized_paths_at(
    directory: &Path,
    relative: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), ContractError> {
    for entry in fs::read_dir(directory)
        .map_err(|_| ContractError::compatibility("cannot enumerate compatibility ports sources"))?
    {
        let entry = entry.map_err(|_| {
            ContractError::compatibility("cannot inspect a compatibility ports source entry")
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            ContractError::compatibility("compatibility ports source entry name is not UTF-8")
        })?;
        if !single_segment(name) {
            return Err(ContractError::compatibility(
                "compatibility ports source entry name is unsafe",
            ));
        }
        let child = entry.path();
        let child_relative = relative.join(name);
        let metadata = fs::symlink_metadata(&child).map_err(|_| {
            ContractError::compatibility("cannot inspect a compatibility ports source entry")
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ContractError::compatibility(
                "compatibility ports source entries must not be symlinks",
            ));
        }
        if metadata.is_dir() {
            if metadata.permissions().mode() & 0o777 != 0o700 {
                return Err(ContractError::compatibility(
                    "compatibility ports source directory is not owner-private",
                ));
            }
            collect_materialized_paths_at(&child, &child_relative, paths)?;
        } else if metadata.is_file() {
            paths.insert(child_relative);
        } else {
            return Err(ContractError::compatibility(
                "compatibility ports source entries must be regular files or directories",
            ));
        }
    }
    Ok(())
}

fn checked_cache(path: &Path) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::sources(
            "compatibility ports cache must be an existing real absolute directory",
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ContractError::sources("compatibility ports cache cannot be canonicalized"))?;
    if open_directory(&canonical).is_err() {
        return Err(ContractError::sources(
            "compatibility ports cache must be an existing real absolute directory",
        ));
    }
    Ok(canonical)
}

fn checked_absent_directory(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() || path.file_name().is_none() || fs::symlink_metadata(path).is_ok() {
        return Err(ContractError::compatibility(format!(
            "{label} must be an absent absolute directory"
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ContractError::compatibility(format!("{label} has no parent directory")))?;
    checked_directory(parent, "compatibility ports output parent")?;
    Ok(path.to_owned())
}

fn checked_directory(path: &Path, label: &str) -> Result<PathBuf, ContractError> {
    if !path.is_absolute() {
        return Err(ContractError::compatibility(format!(
            "{label} must be an absolute directory"
        )));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ContractError::compatibility(format!("{label} cannot be canonicalized")))?;
    if open_directory(&canonical).is_err() {
        return Err(ContractError::compatibility(format!(
            "{label} does not resolve to a real directory without symlink ancestors"
        )));
    }
    Ok(canonical)
}

fn digest<'de, D>(deserializer: D) -> Result<Sha256Digest, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    Sha256Digest::parse(&text).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use aros_common::sha256_bytes;
    use serde_json::json;

    use super::{acquire_cache, materialize, verify_cache, CompatibilityPortsLock};
    use crate::recipe::GitObjectId;

    fn lock(payloads: &[(&str, &str, &str, &str, &[u8])]) -> CompatibilityPortsLock {
        let inputs = payloads
            .iter()
            .map(|(id, cache_filename, relative_path, fetch_marker, bytes)| {
                json!({
                    "id": id,
                    "cache_filename": cache_filename,
                    "relative_path": relative_path,
                    "fetch_marker": fetch_marker,
                    "url": format!("https://example.invalid/{cache_filename}"),
                    "sha256": sha256_bytes(bytes),
                    "size": bytes.len(),
                })
            })
            .collect::<Vec<_>>();
        let selections = ["pc-x86_64", "arm-raspi", "rpi-aarch64"]
            .into_iter()
            .map(|name| {
                json!({
                    "name": name,
                    "inputs": payloads.iter().map(|(id, _, _, _, _)| *id).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        CompatibilityPortsLock::parse(
            serde_json::to_vec(&json!({
                "schema": "aros-toolchain-compatibility-ports-v2",
                "upstream_commit": "a".repeat(40),
                "inputs": inputs,
                "profiles": selections,
            }))
            .unwrap()
            .as_slice(),
        )
        .unwrap()
    }

    #[test]
    fn materializes_exact_private_compatibility_inputs_from_verified_cache() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let unicode = b"0000;<control>;Cc;0;BN;;;;;N;NULL;;;;\n";
        let special = b"# SpecialCasing-16.0.0.txt\n";
        let bzip2 = b"bzip2 source archive";
        fs::write(cache.join("UnicodeData.txt"), unicode).unwrap();
        fs::write(cache.join("SpecialCasing.txt"), special).unwrap();
        fs::write(cache.join("bzip2-1.0.8.tar.gz"), bzip2).unwrap();
        let lock = lock(&[
            (
                "unicode-data",
                "UnicodeData.txt",
                "UnicodeData.txt",
                "",
                unicode,
            ),
            (
                "special-casing",
                "SpecialCasing.txt",
                "unicode/SpecialCasing.txt",
                "",
                special,
            ),
            (
                "bzip2",
                "bzip2-1.0.8.tar.gz",
                "ports/bzip2-1.0.8.tar.gz",
                "ports/.bzip2-1.0.8-fetched",
                bzip2,
            ),
        ]);
        let upstream_commit = lock.0.upstream_commit.clone();

        let sources = materialize(
            &cache,
            &lock,
            &upstream_commit,
            "pc-x86_64",
            &temporary.path().join("ports"),
        )
        .unwrap();
        assert_eq!(
            fs::read(sources.root.join("UnicodeData.txt")).unwrap(),
            unicode
        );
        assert_eq!(
            fs::read(sources.root.join("unicode/SpecialCasing.txt")).unwrap(),
            special
        );
        assert_eq!(
            fs::read(sources.root.join("ports/bzip2-1.0.8.tar.gz")).unwrap(),
            bzip2
        );
        assert_eq!(
            fs::metadata(&sources.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(sources.root.join("ports/bzip2-1.0.8.tar.gz"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        sources.revalidate().unwrap();
        fs::write(
            sources.root.join("ports/bzip2-1.0.8.tar.gz.fetch"),
            b"transient lock",
        )
        .unwrap();
        assert!(sources.revalidate().is_err());
        fs::remove_file(sources.root.join("ports/bzip2-1.0.8.tar.gz.fetch")).unwrap();
        fs::write(sources.root.join("ports/.bzip2-1.0.8-fetched"), b"").unwrap();
        assert!(sources.revalidate().is_err());
        sources.clear_upstream_fetch_markers().unwrap();
        assert!(!sources.root.join("ports/.bzip2-1.0.8-fetched").exists());
        sources.revalidate().unwrap();
        assert_eq!(verify_cache(&cache, &lock).unwrap().payloads.len(), 3);
    }

    #[test]
    fn rejects_unsafe_urls_paths_and_unselected_profiles() {
        let document = json!({
            "schema": "aros-toolchain-compatibility-ports-v2",
            "upstream_commit": "a".repeat(40),
            "inputs": [
                {
                    "id": "unicode-data",
                    "cache_filename": "UnicodeData.txt",
                    "relative_path": "../UnicodeData.txt",
                    "fetch_marker": "",
                    "url": "https://example.invalid/UnicodeData.txt?mutable=true",
                    "sha256": "a".repeat(64), "size": 1
                }
            ],
            "profiles": [{"name": "pc-x86_64", "inputs": ["unicode-data"]}],
        });
        assert!(CompatibilityPortsLock::parse(&serde_json::to_vec(&document).unwrap()).is_err());
    }

    #[test]
    fn canonical_normalization_requires_a_gzip_tar_cache_filename() {
        let document = json!({
            "schema": "aros-toolchain-compatibility-ports-v2",
            "upstream_commit": "a".repeat(40),
            "inputs": [
                {
                    "id": "unicode-data",
                    "cache_filename": "UnicodeData.txt",
                    "relative_path": "UnicodeData.txt",
                    "fetch_marker": "",
                    "normalization": "canonical-tar-gzip-v1",
                    "url": "https://example.invalid/UnicodeData.txt",
                    "sha256": "a".repeat(64), "size": 1
                }
            ],
            "profiles": [{"name": "pc-x86_64", "inputs": ["unicode-data"]}],
        });
        assert!(CompatibilityPortsLock::parse(&serde_json::to_vec(&document).unwrap()).is_err());
    }

    #[test]
    fn rejects_a_profile_that_selects_an_undeclared_input() {
        let document = json!({
            "schema": "aros-toolchain-compatibility-ports-v2",
            "upstream_commit": "a".repeat(40),
            "inputs": [
                {
                    "id": "unicode-data",
                    "cache_filename": "UnicodeData.txt",
                    "relative_path": "UnicodeData.txt",
                    "fetch_marker": "",
                    "url": "https://example.invalid/UnicodeData.txt",
                    "sha256": "a".repeat(64), "size": 1
                }
            ],
            "profiles": [{"name": "pc-x86_64", "inputs": ["missing"]}],
        });
        assert!(CompatibilityPortsLock::parse(&serde_json::to_vec(&document).unwrap()).is_err());
    }

    #[test]
    fn selection_requires_the_locked_upstream_revision_and_profile() {
        let lock = lock(&[(
            "unicode-data",
            "UnicodeData.txt",
            "UnicodeData.txt",
            "",
            b"unicode",
        )]);
        let other_commit = GitObjectId::try_from("b".repeat(40)).unwrap();
        assert!(lock.select(&other_commit, "pc-x86_64").is_err());
        assert!(lock.select(lock.upstream_commit(), "unselected").is_err());
        assert_eq!(
            lock.select(lock.upstream_commit(), "pc-x86_64")
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn acquisition_names_the_missing_locked_input() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let lock = lock(&[(
            "unicode-data",
            "UnicodeData.txt",
            "UnicodeData.txt",
            "",
            b"unicode",
        )]);

        let error = acquire_cache(&cache, &lock, true).await.unwrap_err();
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("unicode-data"));
        assert!(diagnostic.contains("offline mode forbids acquisition"));
    }
}
