//! Bounded release metadata for selecting and identifying SDK artifacts.
//!
//! A release lock records an explicit release and the byte identities of its
//! SDK archive and manifest. It does not prove that the release was published
//! by an authorized party or authenticate either artifact.

use crate::sdk_manifest::{SdkIdentity, SDK_MAX_METADATA_BYTES};
use crate::{parse_credential_free_https_url, sha256_bytes, Sha256Digest};
use serde::{Deserialize, Serialize};

/// Schema identifier for closed SDK release lock documents.
pub const SDK_RELEASE_LOCK_SCHEMA: &str = "aros-sdk-lock-v2";

/// Maximum encoded SDK release lock document size.
pub const SDK_RELEASE_LOCK_MAX_BYTES: usize = 1024 * 1024;

/// SHA-256 and exact byte length for one release artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkArtifactDigest {
    /// SHA-256 digest of the artifact bytes.
    pub sha256: Sha256Digest,
    /// Exact artifact byte length.
    pub size: u64,
}

/// Explicit SDK release selection and artifact byte identities.
///
/// This descriptor records selection and expected bytes. It does not prove
/// publication, publisher identity, or authenticity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkReleaseLock {
    /// Closed release lock schema identifier.
    pub schema: String,
    /// Canonical `v`-prefixed SemVer release tag.
    pub release_tag: String,
    /// Canonical credential-free HTTPS base URL for this release.
    pub base_url: String,
    /// Source-selected SDK identity, independent of the release version.
    pub identity: SdkIdentity,
    /// Digest of the selected SDK source tree.
    pub tree_sha256: Sha256Digest,
    /// Expected SDK archive bytes.
    pub archive: SdkArtifactDigest,
    /// Expected SDK manifest bytes.
    pub manifest: SdkArtifactDigest,
}

impl SdkReleaseLock {
    /// Decode and validate one bounded, closed release lock document.
    ///
    /// # Errors
    /// Returns an error for oversized, malformed, duplicate-field, unknown-field
    /// or semantically invalid documents.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > SDK_RELEASE_LOCK_MAX_BYTES {
            return Err("SDK release lock exceeds its metadata limit".into());
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| "invalid closed SDK release lock document".to_string())?;
        value.validate()?;
        Ok(value)
    }

    /// Validate schema, release selection, URL, identity and artifact limits.
    ///
    /// # Errors
    /// Returns an error if any field is unsupported, noncanonical or outside
    /// the bounded SDK artifact policy.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SDK_RELEASE_LOCK_SCHEMA {
            return Err("unsupported SDK release lock schema".into());
        }
        validate_release_tag(&self.release_tag)?;
        self.identity.validate()?;
        validate_nonzero_digest(&self.tree_sha256)?;
        validate_artifact(&self.archive, 1, 4 * 1024 * 1024 * 1024, "SDK archive")?;
        validate_artifact(
            &self.manifest,
            1,
            SDK_MAX_METADATA_BYTES as u64,
            "SDK manifest",
        )?;
        validate_base_url(&self.base_url, &self.release_tag)?;
        Ok(())
    }

    /// Encode this lock as canonical pretty-printed JSON followed by a newline.
    ///
    /// # Errors
    /// Returns an error if the lock is invalid or the encoding exceeds the
    /// document limit.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)
            .map_err(|_| "cannot encode SDK release lock".to_string())?;
        bytes.push(b'\n');
        if bytes.len() > SDK_RELEASE_LOCK_MAX_BYTES {
            return Err("canonical SDK release lock exceeds its metadata limit".into());
        }
        Ok(bytes)
    }

    /// Return the SHA-256 of the canonical lock document bytes.
    ///
    /// # Errors
    /// Returns an error if canonical encoding fails.
    pub fn digest(&self) -> Result<Sha256Digest, String> {
        Ok(sha256_bytes(&self.canonical_bytes()?))
    }

    /// Return the selected archive URL.
    ///
    /// # Errors
    /// Returns an error if this lock is invalid or the SDK asset name cannot be
    /// derived from its identity.
    pub fn archive_url(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!("{}/{}", self.base_url, self.identity.asset_name()?))
    }

    /// Return the selected archive's JSON manifest URL.
    ///
    /// # Errors
    /// Returns an error if this lock is invalid or the SDK asset name cannot be
    /// derived from its identity.
    pub fn manifest_url(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!(
            "{}/{}.manifest.json",
            self.base_url,
            self.identity.asset_name()?
        ))
    }

    /// Return the selected archive's SHA-256 sidecar URL.
    ///
    /// # Errors
    /// Returns an error if this lock is invalid or the SDK asset name cannot be
    /// derived from its identity.
    pub fn checksum_url(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!(
            "{}/{}.sha256",
            self.base_url,
            self.identity.asset_name()?
        ))
    }
}

fn validate_release_tag(value: &str) -> Result<(), String> {
    let Some(version_text) = value.strip_prefix('v') else {
        return Err("SDK release tag must be v-prefixed canonical SemVer".into());
    };
    if value.len() > 128 {
        return Err("SDK release tag exceeds its portable length limit".into());
    }
    let version = semver::Version::parse(version_text)
        .map_err(|_| "SDK release tag must be v-prefixed canonical SemVer")?;
    if version.to_string() != version_text || !version.build.is_empty() {
        return Err("SDK release tag must be canonical SemVer without build metadata".into());
    }
    Ok(())
}

fn validate_base_url(value: &str, release_tag: &str) -> Result<(), String> {
    if value.len() > 4096 {
        return Err("SDK release base URL exceeds its byte limit".into());
    }
    let parsed = parse_credential_free_https_url(value)?;
    if parsed.as_str() != value {
        return Err("SDK release base URL must use its canonical exact form".into());
    }
    let path = parsed.path();
    if path.ends_with('/') || path.contains('%') || path.contains('\\') {
        return Err("SDK release base URL path is ambiguous or has a trailing slash".into());
    }
    let Some(path_without_leading_slash) = path.strip_prefix('/') else {
        return Err("SDK release base URL must end in its release tag".into());
    };
    let segments: Vec<&str> = path_without_leading_slash.split('/').collect();
    if segments.is_empty()
        || segments
            .iter()
            .any(|segment| segment.is_empty() || *segment == "." || *segment == "..")
        || segments.last().copied() != Some(release_tag)
    {
        return Err("SDK release base URL must end in its literal release tag".into());
    }
    Ok(())
}

fn validate_artifact(
    artifact: &SdkArtifactDigest,
    minimum: u64,
    maximum: u64,
    name: &str,
) -> Result<(), String> {
    validate_nonzero_digest(&artifact.sha256)?;
    if !(minimum..=maximum).contains(&artifact.size) {
        return Err(format!("{name} size is outside its allowed range"));
    }
    Ok(())
}

fn validate_nonzero_digest(value: &Sha256Digest) -> Result<(), String> {
    if value.as_str().bytes().all(|byte| byte == b'0') {
        return Err("SDK release lock digests must be nonzero SHA-256 values".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "sdk_release_lock_tests.rs"]
mod tests;
