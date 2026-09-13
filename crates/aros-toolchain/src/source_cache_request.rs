//! Typed, lock-derived source-cache requests.
//!
//! CACHE-M2 uses this module as the one conversion boundary between reviewed
//! input declarations and cache operations.  It deliberately does not open a
//! cache root, contact an origin, create a source tree, or invoke a build.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

use aros_common::{open_regular_file_nofollow, sha256_bytes, Sha256Digest};
use aros_fetch::engine::cache::CachePayloadNormalization;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::compatibility_ports::CompatibilityPortsLock;
use crate::source_lock::{SourceLock, SourcePurpose};
use crate::ContractError;

const SOURCE_FETCH_PLAN_SCHEMA: &str = "aros-cache-source-fetch-plan-v1";
const MAX_SOURCE_FETCH_PLAN_ENTRIES: usize = 256;
const MAX_SOURCE_FETCH_PLAN_CANDIDATES: usize = 8;
const MAX_SOURCE_FETCH_PLAN_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_SELECTOR_BYTES: u64 = 1024 * 1024;

/// Origin of a reviewed source-cache selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCacheRequestKind {
    /// The native toolchain producer source-lock v2 closure.
    ProducerSourceLock,
    /// The native compatibility ports-lock v2 closure.
    CompatibilityPortsLock,
    /// A validated product source-fetch plan.
    ProductSourceFetchPlan,
}

impl SourceCacheRequestKind {
    /// Stable label for human-oriented result rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProducerSourceLock => "producer_source_lock",
            Self::CompatibilityPortsLock => "compatibility_ports_lock",
            Self::ProductSourceFetchPlan => "product_source_fetch_plan",
        }
    }
}

/// Integrity policy selected by a reviewed source declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceCacheIntegrity {
    /// The declaration binds both the byte count and SHA-256 digest.
    Locked {
        /// Required SHA-256 digest.
        sha256: Sha256Digest,
        /// Required byte length.
        size: u64,
    },
    /// A reviewed product declaration permits acquisition without an upstream
    /// byte identity. The explicit byte limit bounds the transfer and cache
    /// snapshot, but it must never be represented as a lock.
    Unverified {
        /// Maximum byte count accepted for this deliberately unpinned object.
        max_size: u64,
    },
}

/// Declared semantic representation of one cached product input.
///
/// A cache filename is never sufficient to infer whether downstream code must
/// treat its bytes as an archive or as a direct patch. Product plans make the
/// distinction explicit; native lock adapters retain their archive contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCacheRepresentation {
    /// A source archive or archive-shaped immutable source object.
    Archive,
    /// A direct source patch consumed without archive extraction.
    Patch,
}

impl SourceCacheRepresentation {
    /// Stable label for human-oriented result rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Patch => "patch",
        }
    }
}

/// One ordered transport candidate for a source-cache entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheCandidate {
    /// Credential-free HTTPS origin selected by the reviewed request.
    pub url: String,
}

/// Direct patch application metadata bound to one product source declaration.
///
/// The source-cache operation stores and measures patch bytes only. The
/// consumer that owns a target tree applies those bytes using this exact
/// metadata, so a changed target subdirectory or option sequence changes the
/// reviewed declaration identity rather than silently reusing a patch claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCachePatchApplication {
    /// Relative target-tree subdirectory, or the target-tree root when absent.
    pub subdirectory: Option<String>,
    /// Closed patch invocation options in reviewed order.
    pub options: Vec<String>,
}

/// One role-labelled source object requested from a cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheEntry {
    /// Stable semantic role; it is never inferred from a payload filename.
    pub role: String,
    /// Direct portable cache filename selected independently of transport.
    pub filename: String,
    /// Candidate transports in reviewed preference order. Each candidate feeds
    /// this entry's one declared cache object, even when its origin path has a
    /// different basename.
    pub candidates: Vec<SourceCacheCandidate>,
    /// Semantic representation declared for the selected object.
    pub representation: SourceCacheRepresentation,
    /// Direct patch application metadata when `representation` is `patch`.
    pub patch: Option<SourceCachePatchApplication>,
    /// Representation policy for the bytes stored in the cache.
    pub normalization: CachePayloadNormalization,
    /// Integrity policy that applies to the selected object.
    pub integrity: SourceCacheIntegrity,
}

/// A complete, typed and lock-bound source-cache request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheRequest {
    /// Request family that selected this closed input set.
    pub kind: SourceCacheRequestKind,
    /// SHA-256 of the exact regular selector bytes supplied by the caller.
    pub request_sha256: Sha256Digest,
    /// Role-labelled inputs in deterministic role order.
    pub entries: Vec<SourceCacheEntry>,
}

impl SourceCacheRequest {
    /// Validate the request invariants required by the cache reader and writer.
    ///
    /// The fields remain public so callers can record a completed request, but
    /// operations always reapply this closure check before using paths or
    /// origins. This prevents a hand-constructed request from bypassing the
    /// parser's direct-child and bounded-identity rules.
    ///
    /// # Errors
    ///
    /// Returns AX0101 if the request has an invalid role, candidate, cache
    /// identity, normalization or size limit.
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.entries.is_empty() || self.entries.len() > MAX_SOURCE_FETCH_PLAN_ENTRIES {
            return Err(ContractError::invalid(
                "a source-cache request must select between one and 256 source objects",
            ));
        }
        if self.kind != SourceCacheRequestKind::ProductSourceFetchPlan
            && self.entries.iter().any(|entry| {
                matches!(entry.integrity, SourceCacheIntegrity::Unverified { .. })
                    || entry.representation == SourceCacheRepresentation::Patch
            })
        {
            return Err(ContractError::invalid(
                "only a product source-fetch plan may declare an unverified object or direct patch",
            ));
        }
        let mut entries = self.entries.clone();
        close_entries(&mut entries)?;
        if entries != self.entries {
            return Err(ContractError::invalid(
                "source-cache request entries must be in deterministic role order",
            ));
        }
        let total = self.entries.iter().try_fold(0_u64, |total, entry| {
            total
                .checked_add(integrity_limit(&entry.integrity))
                .ok_or_else(|| {
                    ContractError::invalid(
                        "source-cache request payload limits overflow the supported bound",
                    )
                })
        })?;
        if total > MAX_SOURCE_FETCH_PLAN_BYTES {
            return Err(ContractError::invalid(
                "source-cache request payload closure exceeds the 8 GiB safety limit",
            ));
        }
        Ok(())
    }

    /// Parse a native producer source-lock v2 document into a cache request.
    ///
    /// # Errors
    ///
    /// Returns the source-lock parser error when the declaration is malformed
    /// or ambiguous. No filesystem or network access occurs.
    pub fn from_source_lock(bytes: &[u8]) -> Result<Self, ContractError> {
        let lock = SourceLock::parse(bytes)?;
        let mut entries = lock
            .source_components()
            .map(|component| {
                let payload = component.payload();
                let purpose = match component.purpose() {
                    SourcePurpose::ToolchainComponent => "toolchain_component",
                    SourcePurpose::TargetBuildDependency => "target_build_dependency",
                };
                locked_entry(
                    format!(
                        "producer:{purpose}:{}@{}",
                        component.component(),
                        component.version()
                    ),
                    payload.filename(),
                    payload.url(),
                    payload.sha256().clone(),
                    payload.size(),
                    CachePayloadNormalization::ExactBytesV1,
                )
            })
            .chain(lock.host_python_packages().iter().map(|package| {
                let payload = package.payload();
                locked_entry(
                    format!("host_python:{}@{}", package.name(), package.version()),
                    payload.filename(),
                    payload.url(),
                    payload.sha256().clone(),
                    payload.size(),
                    CachePayloadNormalization::ExactBytesV1,
                )
            }))
            .collect::<Vec<_>>();
        close_entries(&mut entries)?;
        Ok(Self {
            kind: SourceCacheRequestKind::ProducerSourceLock,
            request_sha256: sha256_bytes(bytes),
            entries,
        })
    }

    /// Parse a compatibility ports-lock v2 document into a cache request.
    ///
    /// # Errors
    ///
    /// Returns the ports-lock parser error when the declaration is malformed
    /// or ambiguous. No filesystem or network access occurs.
    pub fn from_compatibility_ports_lock(bytes: &[u8]) -> Result<Self, ContractError> {
        let lock = CompatibilityPortsLock::parse(bytes)?;
        let mut entries = lock
            .payloads()
            .into_iter()
            .map(|payload| {
                locked_entry(
                    format!("compatibility_ports:{}", payload.id),
                    &payload.cache_filename,
                    &payload.url,
                    payload.sha256,
                    payload.size,
                    payload.normalization,
                )
            })
            .collect::<Vec<_>>();
        close_entries(&mut entries)?;
        Ok(Self {
            kind: SourceCacheRequestKind::CompatibilityPortsLock,
            request_sha256: sha256_bytes(bytes),
            entries,
        })
    }

    /// Parse a reviewed product source-fetch-plan v1 document into a cache
    /// request.
    ///
    /// Product plans are intentionally capable of selecting an explicitly
    /// unverified object, but only with a declared finite maximum size. The
    /// request preserves that fact instead of synthesising a checksum.
    ///
    /// # Errors
    ///
    /// Returns AX0101 when the document, its selector bounds or an origin is
    /// malformed, ambiguous or unsafe. No filesystem or network access occurs.
    pub fn from_product_source_fetch_plan(bytes: &[u8]) -> Result<Self, ContractError> {
        if bytes.len() > crate::canonical::MAX_DOCUMENT_BYTES {
            return Err(ContractError::invalid("source fetch plan exceeds 1 MiB"));
        }
        let record: SourceFetchPlanRecord = serde_json::from_slice(bytes)
            .map_err(|_| ContractError::invalid("invalid source-fetch-plan-v1 document"))?;
        validate_source_fetch_plan(&record)?;
        let mut entries = record
            .entries
            .into_iter()
            .map(|entry| SourceCacheEntry {
                role: entry.role,
                filename: entry.filename,
                candidates: entry
                    .candidates
                    .into_iter()
                    .map(|candidate| SourceCacheCandidate { url: candidate.url })
                    .collect(),
                representation: entry.representation,
                patch: entry.patch,
                normalization: entry.normalization,
                integrity: match entry.integrity {
                    SourceFetchPlanIntegrity::Locked { sha256, size } => {
                        SourceCacheIntegrity::Locked { sha256, size }
                    }
                    SourceFetchPlanIntegrity::Unverified { max_size } => {
                        SourceCacheIntegrity::Unverified { max_size }
                    }
                },
            })
            .collect::<Vec<_>>();
        close_entries(&mut entries)?;
        Ok(Self {
            kind: SourceCacheRequestKind::ProductSourceFetchPlan,
            request_sha256: sha256_bytes(bytes),
            entries,
        })
    }
}

/// Read one bounded regular selector without following its final symlink.
///
/// This is the common frontend boundary for all source-cache selectors. It
/// returns only exact bytes that were read from one opened descriptor; callers
/// derive the request digest from those bytes and never infer a selector from a
/// cache filename or checkout state.
///
/// # Errors
///
/// Returns AX0101 if the file is not a readable regular file, exceeds one MiB,
/// or changes its byte length while it is read. It does not access a cache root
/// or network.
pub fn read_selector(path: &Path, label: &'static str) -> Result<Vec<u8>, ContractError> {
    let mut file = open_regular_file_nofollow(path)
        .map_err(|_| ContractError::invalid(format!("source cache {label} is unavailable")))?;
    let metadata = file
        .metadata()
        .map_err(|_| ContractError::invalid(format!("cannot inspect source cache {label}")))?;
    if !metadata.is_file() || metadata.len() > MAX_SELECTOR_BYTES {
        return Err(ContractError::invalid(format!(
            "source cache {label} must be a regular file no larger than 1 MiB"
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut limited = file.by_ref().take(MAX_SELECTOR_BYTES.saturating_add(1));
    limited
        .read_to_end(&mut bytes)
        .map_err(|_| ContractError::invalid(format!("cannot read source cache {label}")))?;
    if bytes.len() as u64 > MAX_SELECTOR_BYTES {
        return Err(ContractError::invalid(format!(
            "source cache {label} exceeded the 1 MiB read limit"
        )));
    }
    let final_metadata = file
        .metadata()
        .map_err(|_| ContractError::invalid(format!("cannot recheck source cache {label}")))?;
    if bytes.len() as u64 != metadata.len() || final_metadata.len() != metadata.len() {
        return Err(ContractError::invalid(format!(
            "source cache {label} changed while it was read"
        )));
    }
    Ok(bytes)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFetchPlanRecord {
    schema: String,
    entries: Vec<SourceFetchPlanEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFetchPlanEntry {
    role: String,
    filename: String,
    candidates: Vec<SourceFetchPlanCandidate>,
    representation: SourceCacheRepresentation,
    patch: Option<SourceCachePatchApplication>,
    normalization: CachePayloadNormalization,
    integrity: SourceFetchPlanIntegrity,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceFetchPlanCandidate {
    url: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SourceFetchPlanIntegrity {
    Locked {
        #[serde(deserialize_with = "digest")]
        sha256: Sha256Digest,
        size: u64,
    },
    Unverified {
        max_size: u64,
    },
}

fn locked_entry(
    role: String,
    filename: &str,
    url: &str,
    sha256: Sha256Digest,
    size: u64,
    normalization: CachePayloadNormalization,
) -> SourceCacheEntry {
    SourceCacheEntry {
        role,
        filename: filename.to_owned(),
        candidates: vec![SourceCacheCandidate {
            url: url.to_owned(),
        }],
        representation: SourceCacheRepresentation::Archive,
        patch: None,
        normalization,
        integrity: SourceCacheIntegrity::Locked { sha256, size },
    }
}

fn close_entries(entries: &mut Vec<SourceCacheEntry>) -> Result<(), ContractError> {
    entries.sort_by(|left, right| left.role.cmp(&right.role));
    let mut roles = BTreeSet::new();
    let mut filenames = BTreeSet::new();
    for entry in entries {
        if !role(&entry.role)
            || !roles.insert(&entry.role)
            || !portable_filename(&entry.filename)
            || entry.candidates.is_empty()
            || entry.candidates.len() > MAX_SOURCE_FETCH_PLAN_CANDIDATES
            || !entry
                .candidates
                .iter()
                .all(|candidate| https_url(&candidate.url))
            || !filenames.insert(&entry.filename)
            || !integrity_is_bounded(&entry.integrity)
            || (matches!(entry.integrity, SourceCacheIntegrity::Unverified { .. })
                && entry.normalization != CachePayloadNormalization::ExactBytesV1)
            || !patch_declaration_is_valid(entry)
            || (entry.normalization == CachePayloadNormalization::CanonicalTarGzipV1
                && !canonical_tar_gzip_filename(&entry.filename))
        {
            return Err(ContractError::invalid(
                "source-cache request has duplicate or incomplete role/candidate identity",
            ));
        }
        let mut urls = BTreeSet::new();
        if entry
            .candidates
            .iter()
            .any(|candidate| !urls.insert(candidate.url.as_str()))
        {
            return Err(ContractError::invalid(
                "source-cache request has duplicate candidate origins",
            ));
        }
    }
    Ok(())
}

fn validate_source_fetch_plan(record: &SourceFetchPlanRecord) -> Result<(), ContractError> {
    if record.schema != SOURCE_FETCH_PLAN_SCHEMA
        || record.entries.is_empty()
        || record.entries.len() > MAX_SOURCE_FETCH_PLAN_ENTRIES
    {
        return Err(ContractError::invalid(
            "source fetch plan has an unsupported schema or invalid closure size",
        ));
    }
    let mut entries = record
        .entries
        .iter()
        .map(|entry| SourceCacheEntry {
            role: entry.role.clone(),
            filename: entry.filename.clone(),
            candidates: entry
                .candidates
                .iter()
                .map(|candidate| SourceCacheCandidate {
                    url: candidate.url.clone(),
                })
                .collect(),
            representation: entry.representation,
            patch: entry.patch.clone(),
            normalization: entry.normalization,
            integrity: match &entry.integrity {
                SourceFetchPlanIntegrity::Locked { sha256, size } => SourceCacheIntegrity::Locked {
                    sha256: sha256.clone(),
                    size: *size,
                },
                SourceFetchPlanIntegrity::Unverified { max_size } => {
                    SourceCacheIntegrity::Unverified {
                        max_size: *max_size,
                    }
                }
            },
        })
        .collect::<Vec<_>>();
    close_entries(&mut entries)?;
    let total = entries.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(integrity_limit(&entry.integrity))
            .ok_or_else(|| {
                ContractError::invalid(
                    "source fetch plan payload limits overflow the supported bound",
                )
            })
    })?;
    if total > MAX_SOURCE_FETCH_PLAN_BYTES {
        return Err(ContractError::invalid(
            "source fetch plan payload closure exceeds the 8 GiB safety limit",
        ));
    }
    Ok(())
}

const fn integrity_limit(integrity: &SourceCacheIntegrity) -> u64 {
    match integrity {
        SourceCacheIntegrity::Locked { size, .. } => *size,
        SourceCacheIntegrity::Unverified { max_size } => *max_size,
    }
}

const fn integrity_is_bounded(integrity: &SourceCacheIntegrity) -> bool {
    matches!(integrity_limit(integrity), 1..=MAX_SOURCE_FETCH_PLAN_BYTES)
}

fn patch_declaration_is_valid(entry: &SourceCacheEntry) -> bool {
    match (entry.representation, &entry.patch) {
        (SourceCacheRepresentation::Archive, None) => true,
        (SourceCacheRepresentation::Archive, Some(_))
        | (SourceCacheRepresentation::Patch, None) => false,
        (SourceCacheRepresentation::Patch, Some(patch)) => {
            entry.normalization == CachePayloadNormalization::ExactBytesV1
                && patch
                    .subdirectory
                    .as_deref()
                    .is_none_or(relative_patch_subdirectory)
                && patch.options.iter().all(|option| {
                    matches!(option.as_str(), "-f" | "-N" | "--forward")
                        || option.strip_prefix("-p").is_some_and(|level| {
                            level.len() == 1 && level.bytes().all(|byte| byte.is_ascii_digit())
                        })
                })
        }
    }
}

fn relative_patch_subdirectory(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1_024
        && value.split('/').all(|component| {
            !component.is_empty()
                && component != "."
                && component != ".."
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
        })
}

fn role(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:@+-".contains(&byte)
        })
}

fn portable_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
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

fn canonical_tar_gzip_filename(value: &str) -> bool {
    value
        .get(value.len().saturating_sub(".tar.gz".len())..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".tar.gz"))
        || value
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("tgz"))
}

fn digest<'de, D>(deserializer: D) -> Result<Sha256Digest, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(serde::de::Error::custom(
            "expected a lowercase SHA-256 digest",
        ));
    }
    Sha256Digest::parse(&value).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use aros_common::sha256_bytes;
    use serde_json::json;

    use super::{
        read_selector, SourceCacheIntegrity, SourceCacheRepresentation, SourceCacheRequest,
        SourceCacheRequestKind, MAX_SELECTOR_BYTES,
    };

    fn source_lock() -> Vec<u8> {
        let llvm = b"llvm payload";
        let mako = b"mako payload";
        serde_json::to_vec(&json!({
            "schema": "aros-toolchain-source-lock-v2",
            "family": "llvm",
            "version": "20.1.7",
            "sources": [{
                "component": "llvm-project",
                "version": "20.1.7",
                "purpose": "toolchain-component",
                "filename": "llvm-project.tar.xz",
                "url": "https://example.invalid/llvm-project.tar.xz",
                "sha256": sha256_bytes(llvm),
                "size": llvm.len(),
            }],
            "host_python_packages": [{
                "name": "mako",
                "version": "1.3.10",
                "filename": "mako.tar.gz",
                "url": "https://example.invalid/mako.tar.gz",
                "sha256": sha256_bytes(mako),
                "size": mako.len(),
                "source_root": "Mako-1.3.10",
                "python_path": "src",
            }],
        }))
        .unwrap()
    }

    #[test]
    fn producer_lock_becomes_a_role_labelled_exact_request() {
        let bytes = source_lock();
        let request = SourceCacheRequest::from_source_lock(&bytes).unwrap();
        assert_eq!(request.kind, SourceCacheRequestKind::ProducerSourceLock);
        assert_eq!(request.request_sha256, sha256_bytes(&bytes));
        assert_eq!(request.entries.len(), 2);
        assert_eq!(request.entries[0].role, "host_python:mako@1.3.10");
        assert_eq!(
            request.entries[1].role,
            "producer:toolchain_component:llvm-project@20.1.7"
        );
        assert!(matches!(
            request.entries[0].integrity,
            SourceCacheIntegrity::Locked { .. }
        ));
    }

    #[test]
    fn compatibility_lock_keeps_its_normalization_and_role() {
        let payload = b"compatibility payload";
        let bytes = serde_json::to_vec(&json!({
            "schema": "aros-toolchain-compatibility-ports-v2",
            "upstream_commit": "a".repeat(40),
            "inputs": [{
                "id": "mesa-glu",
                "cache_filename": "glu.tar.gz",
                "relative_path": "glu.tar.gz",
                "fetch_marker": "",
                "normalization": "canonical-tar-gzip-v1",
                "url": "https://example.invalid/glu.tar.gz",
                "sha256": sha256_bytes(payload),
                "size": payload.len(),
            }],
            "profiles": [{
                "name": "pc-x86_64",
                "inputs": ["mesa-glu"],
            }],
        }))
        .unwrap();
        let request = SourceCacheRequest::from_compatibility_ports_lock(&bytes).unwrap();
        assert_eq!(request.kind, SourceCacheRequestKind::CompatibilityPortsLock);
        assert_eq!(request.request_sha256, sha256_bytes(&bytes));
        assert_eq!(request.entries[0].role, "compatibility_ports:mesa-glu");
        assert_eq!(
            serde_json::to_value(request.entries[0].normalization).unwrap(),
            "canonical-tar-gzip-v1"
        );
    }

    #[test]
    fn product_plan_preserves_ordered_fallbacks_and_an_explicit_unpinned_limit() {
        let bytes = serde_json::to_vec(&json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [{
                "role": "product:grub@2.12",
                "filename": "grub-2.12.tar.xz",
                "candidates": [
                    {
                        "url": "https://mirror-a.example.invalid/grub-2.12.tar.xz",
                    },
                    {
                        "url": "https://mirror-b.example.invalid/grub-2.12.tar.xz",
                    }
                ],
                "representation": "archive",
                "normalization": "exact-bytes-v1",
                "integrity": {
                    "kind": "unverified",
                    "max_size": 1_048_576,
                }
            }]
        }))
        .unwrap();
        let request = SourceCacheRequest::from_product_source_fetch_plan(&bytes).unwrap();
        assert_eq!(request.kind, SourceCacheRequestKind::ProductSourceFetchPlan);
        assert_eq!(request.request_sha256, sha256_bytes(&bytes));
        assert_eq!(request.entries[0].candidates.len(), 2);
        assert_eq!(
            request.entries[0].integrity,
            SourceCacheIntegrity::Unverified {
                max_size: 1_048_576
            }
        );
    }

    #[test]
    fn product_plan_allows_fallback_origins_with_distinct_transport_basenames() {
        let bytes = serde_json::to_vec(&json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [{
                "role": "product:grub@2.12",
                "filename": "grub-2.12.source",
                "candidates": [
                    {
                        "url": "https://mirror-a.example.invalid/grub-2.12.tar.xz",
                    },
                    {
                        "url": "https://mirror-b.example.invalid/other.tar.xz",
                    }
                ],
                "representation": "archive",
                "normalization": "exact-bytes-v1",
                "integrity": {
                    "kind": "unverified",
                    "max_size": 1_048_576,
                }
            }]
        }))
        .unwrap();
        let request = SourceCacheRequest::from_product_source_fetch_plan(&bytes).unwrap();
        assert_eq!(request.entries[0].filename, "grub-2.12.source");
    }

    #[test]
    fn product_plan_rejects_unpinned_normalization_before_any_cache_access() {
        let bytes = serde_json::to_vec(&json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [{
                "role": "product:grub@2.12",
                "filename": "grub-2.12.tar.gz",
                "candidates": [{
                    "url": "https://example.invalid/grub-2.12.tar.gz",
                }],
                "representation": "archive",
                "normalization": "canonical-tar-gzip-v1",
                "integrity": {
                    "kind": "unverified",
                    "max_size": 1_048_576,
                }
            }]
        }))
        .unwrap();
        assert!(SourceCacheRequest::from_product_source_fetch_plan(&bytes).is_err());
    }

    #[test]
    fn product_plan_keeps_a_direct_patch_representation_explicit() {
        let payload = b"reviewed direct patch";
        let bytes = serde_json::to_vec(&json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [{
                "role": "product:grub-patch@2.12",
                "filename": "grub-2.12.patch",
                "candidates": [{
                    "url": "https://example.invalid/grub-2.12.patch",
                }],
                "representation": "patch",
                "patch": {
                    "subdirectory": "grub-core",
                    "options": ["-p1", "--forward"],
                },
                "normalization": "exact-bytes-v1",
                "integrity": {
                    "kind": "locked",
                    "sha256": sha256_bytes(payload),
                    "size": payload.len(),
                }
            }]
        }))
        .unwrap();
        let request = SourceCacheRequest::from_product_source_fetch_plan(&bytes).unwrap();
        assert_eq!(
            request.entries[0].representation,
            SourceCacheRepresentation::Patch
        );
        assert_eq!(
            request.entries[0]
                .patch
                .as_ref()
                .unwrap()
                .subdirectory
                .as_deref(),
            Some("grub-core")
        );

        let changed = String::from_utf8(bytes)
            .unwrap()
            .replace("-p1", "-p2")
            .into_bytes();
        let changed = SourceCacheRequest::from_product_source_fetch_plan(&changed).unwrap();
        assert_ne!(request.request_sha256, changed.request_sha256);
    }

    #[test]
    fn product_plan_rejects_incomplete_patch_metadata_and_non_https_origins() {
        let payload = b"reviewed direct patch";
        let incomplete_patch = serde_json::to_vec(&json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [{
                "role": "product:grub-patch@2.12",
                "filename": "grub-2.12.patch",
                "candidates": [{
                    "url": "https://example.invalid/grub-2.12.patch",
                }],
                "representation": "patch",
                "normalization": "exact-bytes-v1",
                "integrity": {
                    "kind": "locked",
                    "sha256": sha256_bytes(payload),
                    "size": payload.len(),
                }
            }]
        }))
        .unwrap();
        assert!(SourceCacheRequest::from_product_source_fetch_plan(&incomplete_patch).is_err());

        let non_https = serde_json::to_vec(&json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [{
                "role": "product:grub@2.12",
                "filename": "grub-2.12.tar.xz",
                "candidates": [{
                    "url": "http://example.invalid/grub-2.12.tar.xz",
                }],
                "representation": "archive",
                "normalization": "exact-bytes-v1",
                "integrity": {
                    "kind": "locked",
                    "sha256": sha256_bytes(payload),
                    "size": payload.len(),
                }
            }]
        }))
        .unwrap();
        assert!(SourceCacheRequest::from_product_source_fetch_plan(&non_https).is_err());
    }

    #[test]
    fn product_plan_rejects_distinct_roles_that_reuse_one_cache_filename() {
        let first = b"first source identity";
        let second = b"second source identity";
        let bytes = serde_json::to_vec(&json!({
            "schema": "aros-cache-source-fetch-plan-v1",
            "entries": [
                {
                    "role": "product:first@1",
                    "filename": "shared.tar.xz",
                    "candidates": [{"url": "https://example.invalid/first.tar.xz"}],
                    "representation": "archive",
                    "normalization": "exact-bytes-v1",
                    "integrity": {
                        "kind": "locked",
                        "sha256": sha256_bytes(first),
                        "size": first.len(),
                    }
                },
                {
                    "role": "product:second@1",
                    "filename": "shared.tar.xz",
                    "candidates": [{"url": "https://example.invalid/second.tar.xz"}],
                    "representation": "archive",
                    "normalization": "exact-bytes-v1",
                    "integrity": {
                        "kind": "locked",
                        "sha256": sha256_bytes(second),
                        "size": second.len(),
                    }
                }
            ]
        }))
        .unwrap();
        assert!(SourceCacheRequest::from_product_source_fetch_plan(&bytes).is_err());
    }

    #[test]
    fn request_validation_rejects_a_direct_patch_outside_a_product_plan() {
        let mut request = SourceCacheRequest::from_source_lock(&source_lock()).unwrap();
        request.entries[0].representation = SourceCacheRepresentation::Patch;
        assert!(request.validate().is_err());
    }

    #[test]
    fn selector_reader_rejects_an_oversized_regular_file() {
        let temporary = tempfile::Builder::new()
            .prefix("aros-selector-reader-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let selector = temporary.path().join("source-lock.json");
        fs::write(&selector, vec![b'x'; MAX_SELECTOR_BYTES as usize + 1]).unwrap();
        assert!(read_selector(&selector, "source lock").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn selector_reader_rejects_a_final_symlink() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::Builder::new()
            .prefix("aros-selector-reader-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let outside = temporary.path().join("outside.json");
        let selector = temporary.path().join("source-lock.json");
        fs::write(&outside, b"{}\n").unwrap();
        symlink(&outside, &selector).unwrap();
        assert!(read_selector(&selector, "source lock").is_err());
    }

    #[test]
    fn source_lock_patch_declaration_changes_the_bound_request_identity() {
        let without_patch = source_lock();
        let mut with_patch: serde_json::Value = serde_json::from_slice(&without_patch).unwrap();
        with_patch["sources"][0]["patch"] = json!("tools/crosstools/llvm/llvm-aros.diff");
        let with_patch = serde_json::to_vec(&with_patch).unwrap();

        let first = SourceCacheRequest::from_source_lock(&without_patch).unwrap();
        let second = SourceCacheRequest::from_source_lock(&with_patch).unwrap();
        assert_eq!(first.entries, second.entries);
        assert_ne!(first.request_sha256, second.request_sha256);
    }
}
