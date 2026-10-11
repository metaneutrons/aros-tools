//! Host-independent Developer SDK identity and closed payload metadata.
//!
//! These records establish byte consistency against an explicit selection.
//! They do not authenticate a build, prove ABI compatibility or authorize a
//! release. Compiler packages and their host identities remain separate.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{payload_casefold_path_key, toolchain_inventory_sha256, ArosToolchainManifestEntry};
use crate::{sha256_bytes, Sha256Digest};

pub const SDK_MANIFEST_SCHEMA: &str = "aros-sdk-manifest-v2";
pub const SDK_MANIFEST_FILE: &str = "sdk-manifest.json";
pub const SDK_MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;
pub const SDK_MAX_ENTRIES: usize = 100_000;
pub const SDK_MAX_REGULAR_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Largest nonnegative timestamp representable in the portable SDK tar header.
pub const SDK_MAX_SOURCE_DATE_EPOCH: u64 = 0o77_777_777_777;

/// Scalar source-owned ABI facts. No executable extensions or board enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SdkAbiValue {
    Text(String),
    Boolean(bool),
    Unsigned(u64),
}

/// ABI descriptor selected from the source, not inferred from the build host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkTarget {
    pub target_triple: String,
    #[serde(deserialize_with = "unique_abi_facts")]
    pub abi: BTreeMap<String, SdkAbiValue>,
}

/// Exact selected source and source-owned SDK contract identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkSourceIdentity {
    pub commit: String,
    pub git_tree: String,
    /// Normalized raw-Git source projection with top-level `cmake/` omitted;
    /// commit/tree above identify the complete original committed checkout.
    pub content_sha256: Sha256Digest,
    pub contract_sha256: Sha256Digest,
}

/// SDK product identity. Version is independent of compiler/tool versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkIdentity {
    pub sdk_id: String,
    pub version: String,
    pub target: SdkTarget,
    pub source: SdkSourceIdentity,
    pub source_date_epoch: u64,
}

impl SdkIdentity {
    /// Check portable identity fields without assuming a supported target.
    ///
    /// # Errors
    /// Rejects unsafe identifiers, noncanonical SemVer, invalid Git identities,
    /// empty/unbounded ABI facts and unsupported archive timestamps.
    pub fn validate(&self) -> Result<(), String> {
        token("SDK ID", &self.sdk_id, 96)?;
        if self.sdk_id.bytes().any(|b| b.is_ascii_uppercase()) {
            return Err("SDK ID must be lowercase".into());
        }
        if self.version.len() > 96 {
            return Err("SDK version exceeds its portable filename budget".into());
        }
        let version = semver::Version::parse(&self.version)
            .map_err(|_| "SDK version must be canonical SemVer")?;
        if version.to_string() != self.version || !version.build.is_empty() {
            return Err("SDK version must be canonical SemVer without build metadata".into());
        }
        self.target.validate()?;
        for value in [&self.source.commit, &self.source.git_tree] {
            if value.len() != 40
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || value.bytes().all(|b| b == b'0')
            {
                return Err("SDK source Git identities require nonzero lowercase SHA-1".into());
            }
        }
        for value in [&self.source.content_sha256, &self.source.contract_sha256] {
            nonzero_digest(value)?;
        }
        if self.source_date_epoch > SDK_MAX_SOURCE_DATE_EPOCH {
            return Err("SDK source-date epoch exceeds portable tar timestamp range".into());
        }
        Ok(())
    }

    /// Host-independent archive name; no compiler or build-host selector.
    ///
    /// # Errors
    /// Rejects an invalid identity before deriving an output filename.
    pub fn asset_name(&self) -> Result<String, String> {
        self.validate()?;
        Ok(format!("aros-sdk-{}-{}.tar.xz", self.sdk_id, self.version))
    }
}

impl SdkTarget {
    /// Validate a bounded, data-only ABI descriptor.
    ///
    /// # Errors
    /// Rejects host/board selectors, unsafe tokens and a contradictory triple.
    pub fn validate(&self) -> Result<(), String> {
        token("SDK target triple", &self.target_triple, 128)?;
        if self.abi.is_empty() || self.abi.len() > 32 {
            return Err("SDK ABI requires between one and 32 source-owned facts".into());
        }
        for (key, value) in &self.abi {
            token("SDK ABI key", key, 64)?;
            if matches!(key.as_str(), "host" | "board" | "compiler") {
                return Err("SDK ABI must not contain host, board or compiler identity".into());
            }
            if let SdkAbiValue::Text(text) = value {
                token("SDK ABI value", text, 256)?;
            }
        }
        if self
            .abi
            .get("target_triple")
            .is_some_and(|value| value != &SdkAbiValue::Text(self.target_triple.clone()))
        {
            return Err("SDK ABI target triple contradicts the selected target".into());
        }
        Ok(())
    }

    /// Canonical digest of the complete target/ABI descriptor.
    ///
    /// # Errors
    /// Rejects invalid descriptors or serialization failure.
    pub fn sha256(&self) -> Result<Sha256Digest, String> {
        self.validate()?;
        serde_json::to_vec(self)
            .map(|bytes| sha256_bytes(&bytes))
            .map_err(|_| "cannot encode SDK target identity".into())
    }
}

/// Self-describing SDK payload. The manifest does not inventory itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkManifest {
    pub schema: String,
    pub identity: SdkIdentity,
    pub tree_sha256: Sha256Digest,
    pub files: Vec<ArosToolchainManifestEntry>,
}

impl SdkManifest {
    /// Decode exactly one bounded, closed metadata document and validate it.
    ///
    /// # Errors
    /// Rejects unknown/duplicate fields, invalid identity or payload inventory.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > SDK_MAX_METADATA_BYTES {
            return Err("SDK manifest exceeds its metadata limit".into());
        }
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|_| "invalid closed SDK manifest document".to_string())?;
        value.validate()?;
        Ok(value)
    }

    /// Validate identity, complete portable inventory and canonical tree hash.
    ///
    /// # Errors
    /// Rejects malformed entries, aliases, unsafe/dangling/cyclic links,
    /// unbounded payloads and a digest that does not describe the inventory.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SDK_MANIFEST_SCHEMA {
            return Err("unsupported SDK manifest schema".into());
        }
        self.identity.validate()?;
        nonzero_digest(&self.tree_sha256)?;
        validate_inventory(&self.files)?;
        if toolchain_inventory_sha256(&self.files).map_err(|_| "cannot hash SDK inventory")?
            != self.tree_sha256.as_str()
        {
            return Err("SDK inventory differs from the selected tree digest".into());
        }
        Ok(())
    }
}

/// Validate shared inventory entries under the SDK's stricter bounded policy.
///
/// # Errors
/// Rejects noncanonical paths/modes/fields, unsafe links and resource excess.
pub fn validate_inventory(entries: &[ArosToolchainManifestEntry]) -> Result<(), String> {
    if entries.is_empty() || entries.len() > SDK_MAX_ENTRIES {
        return Err("SDK inventory is empty or exceeds its entry limit".into());
    }
    let mut previous: Option<&str> = None;
    let mut folded = BTreeSet::new();
    let mut directories = BTreeSet::from([String::new()]);
    let mut nodes = BTreeMap::new();
    let mut size = 0_u64;
    let mut regular_count = 0;
    for entry in entries {
        relative(&entry.path)?;
        if entry.path == SDK_MANIFEST_FILE || previous.is_some_and(|p| p >= entry.path.as_str()) {
            return Err(
                "SDK inventory must be strictly sorted and must not inventory its manifest".into(),
            );
        }
        previous = Some(&entry.path);
        let key = payload_casefold_path_key(Path::new(&entry.path))
            .map_err(|_| "SDK inventory has a nonportable path")?;
        if !folded.insert(key) {
            return Err("SDK inventory has a case-folding path collision".into());
        }
        let parent = Path::new(&entry.path)
            .parent()
            .and_then(Path::to_str)
            .unwrap_or("");
        if !directories.contains(parent) {
            return Err(
                "SDK inventory requires an explicitly declared real parent directory".into(),
            );
        }
        match entry.kind.as_str() {
            "directory"
                if entry.mode == "0755"
                    && entry.sha256.is_none()
                    && entry.size.is_none()
                    && entry.target.is_none() =>
            {
                directories.insert(entry.path.clone());
            }
            "file" if matches!(entry.mode.as_str(), "0644" | "0755") && entry.target.is_none() => {
                let digest = Sha256Digest::parse(entry.sha256.as_deref().unwrap_or(""))
                    .map_err(|_| "SDK file requires a valid SHA-256")?;
                if entry.sha256.as_deref() != Some(digest.as_str()) {
                    return Err("SDK file digest must be lowercase".into());
                }
                size = size
                    .checked_add(entry.size.ok_or("SDK file requires a byte size")?)
                    .filter(|n| *n <= SDK_MAX_REGULAR_BYTES)
                    .ok_or("SDK payload exceeds its regular-byte limit")?;
                regular_count += 1;
            }
            "symlink" if entry.mode == "0777" && entry.sha256.is_none() && entry.size.is_none() => {
                link_components(entry.target.as_deref().unwrap_or(""))?;
            }
            _ => return Err("SDK inventory has invalid type, mode or entry fields".into()),
        }
        nodes.insert(entry.path.as_str(), entry);
    }
    if regular_count == 0 {
        return Err("SDK payload requires at least one regular file".into());
    }
    for entry in entries.iter().filter(|e| e.kind == "symlink") {
        resolve_inventory_link(entry, &nodes)?;
    }
    Ok(())
}

fn relative(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 32 * 1024
        || value.contains(['\\', '\0'])
        || value
            .split('/')
            .any(|p| p.is_empty() || matches!(p, "." | ".."))
        || Path::new(value).components().count() > 128
        || Path::new(value)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("SDK inventory path must be a bounded canonical relative path".into());
    }
    Ok(())
}

fn link_components(target: &str) -> Result<VecDeque<String>, String> {
    if target.is_empty()
        || target.len() > 32 * 1024
        || target.contains(['\\', '\0'])
        || Path::new(target).is_absolute()
        || target.split('/').filter(|s| !s.is_empty()).count() > 128
    {
        return Err("SDK symbolic link requires a bounded relative target".into());
    }
    let mut components = target
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect::<VecDeque<_>>();
    // A trailing slash requires a directory under POSIX path traversal.
    if target.ends_with('/') {
        components.push_back(".".into());
    }
    Ok(components)
}

fn resolve_inventory_link(
    entry: &ArosToolchainManifestEntry,
    nodes: &BTreeMap<&str, &ArosToolchainManifestEntry>,
) -> Result<(), String> {
    let mut current = PathBuf::from(&entry.path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf();
    let mut pending = link_components(entry.target.as_deref().unwrap_or(""))?;
    let mut hops = 0_usize;
    let mut steps = 0_usize;
    while let Some(component) = pending.pop_front() {
        steps += 1;
        if steps > 16_384 || pending.len() > 16_384 {
            return Err("SDK symbolic link exceeds its traversal limit".into());
        }
        match component.as_str() {
            "." => {}
            ".." => {
                if !current.pop() {
                    return Err("SDK symbolic link escapes its payload".into());
                }
            }
            name => {
                let next = current.join(name);
                let next_name = next.to_str().ok_or("SDK link path is not UTF-8")?;
                let node = nodes
                    .get(next_name)
                    .ok_or("SDK symbolic link has a dangling target")?;
                if node.kind == "symlink" {
                    hops += 1;
                    if hops > 128 {
                        return Err("SDK symbolic link is cyclic or exceeds its hop limit".into());
                    }
                    let mut expansion = link_components(node.target.as_deref().unwrap_or(""))?;
                    expansion.append(&mut pending);
                    pending = expansion;
                } else {
                    if !pending.is_empty() && node.kind != "directory" {
                        return Err("SDK symbolic link traverses a non-directory".into());
                    }
                    current = next;
                    if current.components().count() > 128 {
                        return Err("SDK symbolic link exceeds its path depth".into());
                    }
                }
            }
        }
    }
    Ok(())
}

fn token(label: &str, value: &str, limit: usize) -> Result<(), String> {
    if value.is_empty()
        || value.len() > limit
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'))
        || matches!(value, "." | "..")
    {
        return Err(format!("{label} must be a bounded portable token"));
    }
    Ok(())
}

fn nonzero_digest(value: &Sha256Digest) -> Result<(), String> {
    if value.as_str().bytes().all(|b| b == b'0') {
        return Err("SDK identity requires a nonzero SHA-256".into());
    }
    Ok(())
}

fn unique_abi_facts<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, SdkAbiValue>, D::Error> {
    struct Facts;
    impl<'de> Visitor<'de> for Facts {
        type Value = BTreeMap<String, SdkAbiValue>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded map of unique source-owned ABI facts")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut facts = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, SdkAbiValue>()? {
                if facts.len() >= 32 || facts.insert(key, value).is_some() {
                    return Err(de::Error::custom(
                        "SDK ABI facts are excessive or duplicated",
                    ));
                }
            }
            Ok(facts)
        }
    }
    deserializer.deserialize_map(Facts)
}

#[cfg(test)]
#[path = "sdk_manifest_tests.rs"]
mod tests;
