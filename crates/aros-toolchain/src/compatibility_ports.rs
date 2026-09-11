//! Exact offline Unicode inputs for the upstream compatibility `includes` phase.
//!
//! The pinned upstream Makefile otherwise downloads the mutable Unicode
//! `latest` files itself.  A release qualification must never permit that
//! implicit network input.  This module therefore owns a deliberately small
//! lock format, verifies or acquires its two direct cache payloads, and
//! materializes a fresh read-only ports-source directory for `configure`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use aros_common::{open_regular_file_nofollow, sha256_reader, Sha256Digest, Sha256Result};
use aros_fetch::engine::cache::{
    acquire_https_cache_payload, snapshot_verified_cache_payload, VerifiedCachePayload,
};
use serde::Deserialize;

use crate::filesystem::open_directory;
use crate::ContractError;

const SCHEMA: &str = "aros-toolchain-compatibility-ports-v1";
const MAX_DOCUMENT_BYTES: usize = 64 * 1024;
const MAX_PAYLOAD_BYTES: u64 = 8 * 1024 * 1024;
const REQUIRED_FILENAMES: [&str; 2] = ["SpecialCasing.txt", "UnicodeData.txt"];

/// Exact declared Unicode input closure for the upstream `includes` phase.
#[derive(Debug, Clone)]
pub struct CompatibilityPortsLock(Record);

/// One measured direct Unicode input selected by a ports lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPortsPayload {
    /// Portable filename consumed by upstream Make rules.
    pub filename: String,
    /// Official immutable Unicode HTTPS location.
    pub url: String,
    /// Complete SHA-256 identity.
    pub sha256: Sha256Digest,
    /// Exact byte size.
    pub size: u64,
}

/// Complete cache observation for one ports lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPortsCache {
    /// Inputs in stable filename order.
    pub payloads: Vec<CompatibilityPortsPayload>,
}

/// Fresh, private and revalidatable `--with-portssources` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityPortsSources {
    /// Canonical private directory passed to upstream configure.
    pub root: PathBuf,
    payloads: BTreeMap<String, CompatibilityPortsPayload>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: String,
    unicode_version: String,
    inputs: Vec<Input>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    filename: String,
    url: String,
    #[serde(deserialize_with = "digest")]
    sha256: Sha256Digest,
    size: u64,
}

impl CompatibilityPortsLock {
    /// Parse and close the small Unicode-input contract without I/O.
    ///
    /// The only accepted files are the two files called by the selected
    /// upstream Makefiles, from an explicitly versioned Unicode release.
    ///
    /// # Errors
    ///
    /// Returns AX0101 when the document is malformed, non-canonical, mutable,
    /// or does not declare the exact two-file Unicode input closure.
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

    /// Inputs in stable filename order.
    #[must_use]
    pub fn payloads(&self) -> Vec<CompatibilityPortsPayload> {
        let mut payloads = self
            .0
            .inputs
            .iter()
            .map(|input| CompatibilityPortsPayload {
                filename: input.filename.clone(),
                url: input.url.clone(),
                sha256: input.sha256.clone(),
                size: input.size,
            })
            .collect::<Vec<_>>();
        payloads.sort_by(|left, right| left.filename.cmp(&right.filename));
        payloads
    }
}

/// Verify that every selected Unicode payload is already present in `cache`.
///
/// # Errors
///
/// Returns AX0301 when the cache or one selected payload is absent, unsafe, or
/// differs from its measured lock identity.
pub fn verify_cache(
    cache: &Path,
    lock: &CompatibilityPortsLock,
) -> Result<CompatibilityPortsCache, ContractError> {
    let snapshots = snapshots(cache, lock)?;
    let payloads = snapshots
        .iter()
        .map(|(_, payload)| payload.clone())
        .collect();
    Ok(CompatibilityPortsCache { payloads })
}

/// Acquire only missing lock-selected Unicode payloads, then verify all of them.
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
        let snapshot = acquire_https_cache_payload(
            &cache,
            &payload.filename,
            &payload.url,
            payload.size,
            &payload.sha256,
            offline,
        )
        .await
        .map_err(|_| {
            ContractError::sources(
                "a compatibility ports input is missing, unsafe, changed, or could not be acquired",
            )
        })?;
        snapshot.revalidate().map_err(|_| {
            ContractError::sources("a compatibility ports input changed after acquisition")
        })?;
    }
    verify_cache(&cache, lock)
}

/// Materialize a fresh, read-only `--with-portssources` directory from exact
/// verified cache snapshots.  The selected cache is never passed to upstream.
///
/// # Errors
///
/// Returns AX0301 for an unsafe cache input and AX0703 when the output root or
/// any copied file cannot be created, sealed, or revalidated exactly.
pub fn materialize(
    cache: &Path,
    lock: &CompatibilityPortsLock,
    output_root: &Path,
) -> Result<CompatibilityPortsSources, ContractError> {
    let output_root =
        checked_absent_directory(output_root, "compatibility ports output directory")?;
    let snapshots = snapshots(cache, lock)?;
    fs::create_dir(&output_root).map_err(|_| {
        ContractError::compatibility("cannot create fresh compatibility ports output directory")
    })?;
    fs::set_permissions(&output_root, fs::Permissions::from_mode(0o700)).map_err(|_| {
        ContractError::compatibility("cannot restrict compatibility ports output directory")
    })?;

    let mut payloads = BTreeMap::new();
    for (snapshot, payload) in snapshots {
        let destination = output_root.join(&payload.filename);
        copy_snapshot(&snapshot, &destination, &payload)?;
        payloads.insert(payload.filename.clone(), payload);
    }
    fs::set_permissions(&output_root, fs::Permissions::from_mode(0o500)).map_err(|_| {
        ContractError::compatibility("cannot seal compatibility ports output directory")
    })?;
    let sources = CompatibilityPortsSources {
        root: checked_directory(&output_root, "compatibility ports output directory")?,
        payloads,
    };
    sources.revalidate()?;
    Ok(sources)
}

impl CompatibilityPortsSources {
    /// Materialized inputs in stable filename order, suitable for durable
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
        if root != self.root || self.payloads.len() != REQUIRED_FILENAMES.len() {
            return Err(ContractError::compatibility(
                "compatibility ports source directory changed after materialization",
            ));
        }
        let entries = fs::read_dir(&root)
            .map_err(|_| {
                ContractError::compatibility(
                    "cannot enumerate compatibility ports source directory",
                )
            })?
            .map(|entry| {
                entry
                    .map_err(|_| {
                        ContractError::compatibility(
                            "cannot inspect a compatibility ports source entry",
                        )
                    })?
                    .file_name()
                    .into_string()
                    .map_err(|_| {
                        ContractError::compatibility(
                            "compatibility ports source entry name is not UTF-8",
                        )
                    })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if entries != self.payloads.keys().cloned().collect() {
            return Err(ContractError::compatibility(
                "compatibility ports source directory contains unmeasured or missing entries",
            ));
        }
        for (filename, expected) in &self.payloads {
            let path = root.join(filename);
            let metadata = fs::symlink_metadata(&path).map_err(|_| {
                ContractError::compatibility("cannot inspect a compatibility ports source payload")
            })?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() != expected.size
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
}

fn snapshots(
    cache: &Path,
    lock: &CompatibilityPortsLock,
) -> Result<Vec<(VerifiedCachePayload, CompatibilityPortsPayload)>, ContractError> {
    let cache = checked_cache(cache)?;
    lock.payloads()
        .into_iter()
        .map(|payload| {
            let snapshot = snapshot_verified_cache_payload(
                &cache,
                &payload.filename,
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
    if record.schema != SCHEMA || !unicode_version(&record.unicode_version) {
        return Err(ContractError::invalid(
            "compatibility ports lock has an unsupported schema or Unicode version",
        ));
    }
    if record.inputs.len() != REQUIRED_FILENAMES.len() {
        return Err(ContractError::invalid(
            "compatibility ports lock must declare exactly UnicodeData.txt and SpecialCasing.txt",
        ));
    }
    let mut names = BTreeSet::new();
    for input in &record.inputs {
        if !REQUIRED_FILENAMES.contains(&input.filename.as_str())
            || !names.insert(input.filename.as_str())
            || input.size == 0
            || input.size > MAX_PAYLOAD_BYTES
        {
            return Err(ContractError::invalid(
                "compatibility ports lock contains an invalid or duplicate Unicode input",
            ));
        }
        let expected_url = format!(
            "https://www.unicode.org/Public/{}/ucd/{}",
            record.unicode_version, input.filename
        );
        if input.url != expected_url {
            return Err(ContractError::invalid(
                "compatibility ports lock input is not an official versioned Unicode URL",
            ));
        }
    }
    if names != BTreeSet::from(REQUIRED_FILENAMES) {
        return Err(ContractError::invalid(
            "compatibility ports lock does not declare the required Unicode input set",
        ));
    }
    Ok(())
}

fn matches_payload(measured: &Sha256Result, expected: &CompatibilityPortsPayload) -> bool {
    (measured.size, &measured.digest) == (expected.size, &expected.sha256)
}

fn unicode_version(value: &str) -> bool {
    let fields = value.split('.').collect::<Vec<_>>();
    fields.len() == 3
        && fields
            .iter()
            .all(|field| !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit()))
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

    use aros_common::sha256_bytes;
    use serde_json::json;

    use super::{materialize, verify_cache, CompatibilityPortsLock};

    fn lock(unicode: &[u8], special: &[u8]) -> CompatibilityPortsLock {
        CompatibilityPortsLock::parse(
            serde_json::to_vec(&json!({
                "schema": "aros-toolchain-compatibility-ports-v1",
                "unicode_version": "16.0.0",
                "inputs": [
                    {
                        "filename": "UnicodeData.txt",
                        "url": "https://www.unicode.org/Public/16.0.0/ucd/UnicodeData.txt",
                        "sha256": sha256_bytes(unicode), "size": unicode.len()
                    },
                    {
                        "filename": "SpecialCasing.txt",
                        "url": "https://www.unicode.org/Public/16.0.0/ucd/SpecialCasing.txt",
                        "sha256": sha256_bytes(special), "size": special.len()
                    }
                ]
            }))
            .unwrap()
            .as_slice(),
        )
        .unwrap()
    }

    #[test]
    fn materializes_exact_read_only_unicode_inputs_from_verified_cache() {
        let temporary = tempfile::tempdir().unwrap();
        let cache = temporary.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let unicode = b"0000;<control>;Cc;0;BN;;;;;N;NULL;;;;\n";
        let special = b"# SpecialCasing-16.0.0.txt\n";
        fs::write(cache.join("UnicodeData.txt"), unicode).unwrap();
        fs::write(cache.join("SpecialCasing.txt"), special).unwrap();
        let lock = lock(unicode, special);

        let sources = materialize(&cache, &lock, &temporary.path().join("ports")).unwrap();
        assert_eq!(
            fs::read(sources.root.join("UnicodeData.txt")).unwrap(),
            unicode
        );
        assert_eq!(
            fs::read(sources.root.join("SpecialCasing.txt")).unwrap(),
            special
        );
        sources.revalidate().unwrap();
        assert_eq!(verify_cache(&cache, &lock).unwrap().payloads.len(), 2);
    }

    #[test]
    fn rejects_a_mutable_or_wrong_unicode_origin() {
        let document = json!({
            "schema": "aros-toolchain-compatibility-ports-v1",
            "unicode_version": "16.0.0",
            "inputs": [
                {
                    "filename": "UnicodeData.txt",
                    "url": "https://www.unicode.org/Public/UCD/latest/ucd/UnicodeData.txt",
                    "sha256": "a".repeat(64), "size": 1
                },
                {
                    "filename": "SpecialCasing.txt",
                    "url": "https://www.unicode.org/Public/16.0.0/ucd/SpecialCasing.txt",
                    "sha256": "b".repeat(64), "size": 1
                }
            ]
        });
        assert!(CompatibilityPortsLock::parse(&serde_json::to_vec(&document).unwrap()).is_err());
    }
}
