//! Typed, lock-derived source-cache requests.
//!
//! CACHE-M2 uses this module as the one conversion boundary between reviewed
//! input declarations and cache operations.  It deliberately does not open a
//! cache root, contact an origin, create a source tree, or invoke a build.

use std::collections::BTreeSet;

use aros_common::{sha256_bytes, Sha256Digest};
use aros_fetch::engine::cache::CachePayloadNormalization;
use serde::Serialize;

use crate::compatibility_ports::CompatibilityPortsLock;
use crate::source_lock::{SourceLock, SourcePurpose};
use crate::ContractError;

/// Origin of a reviewed source-cache selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCacheRequestKind {
    /// The native toolchain producer source-lock v2 closure.
    ProducerSourceLock,
    /// The native compatibility ports-lock v2 closure.
    CompatibilityPortsLock,
    /// A later validated product source-fetch plan.
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
    /// byte identity. It must never be represented as a lock.
    Unverified,
}

/// One ordered transport candidate for a source-cache entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheCandidate {
    /// Portable payload filename selected by this candidate.
    pub filename: String,
    /// Credential-free HTTPS origin selected by the reviewed request.
    pub url: String,
}

/// One role-labelled source object requested from a cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceCacheEntry {
    /// Stable semantic role; it is never inferred from a payload filename.
    pub role: String,
    /// Candidate transports in reviewed preference order.
    pub candidates: Vec<SourceCacheCandidate>,
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
        candidates: vec![SourceCacheCandidate {
            filename: filename.to_owned(),
            url: url.to_owned(),
        }],
        normalization,
        integrity: SourceCacheIntegrity::Locked { sha256, size },
    }
}

fn close_entries(entries: &mut Vec<SourceCacheEntry>) -> Result<(), ContractError> {
    entries.sort_by(|left, right| left.role.cmp(&right.role));
    let mut roles = BTreeSet::new();
    let mut candidates = BTreeSet::new();
    for entry in entries {
        if entry.role.is_empty()
            || !roles.insert(&entry.role)
            || entry.candidates.len() != 1
            || !candidates.insert(&entry.candidates[0].filename)
        {
            return Err(ContractError::invalid(
                "source-cache request has duplicate or incomplete role/candidate identity",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use aros_common::sha256_bytes;
    use serde_json::json;

    use super::{SourceCacheIntegrity, SourceCacheRequest, SourceCacheRequestKind};

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
}
