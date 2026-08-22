use crate::error::{ArosError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub const AROS_TOOLCHAIN_LOCK_SCHEMA: u32 = 1;
pub const AROS_TOOLCHAIN_MANIFEST_SCHEMA: u32 = 1;
pub const AROS_TOOLCHAIN_MANIFEST_FILE: &str = "toolchain-manifest.json";

/// Immutable release selection checked into the AROS-NG source tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArosToolchainLock {
    pub schema: u32,
    pub release_id: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub artifacts: Vec<ArosToolchainArtifact>,
}

/// One host and target-profile-specific AROS cross-toolchain archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArosToolchainArtifact {
    pub host: String,
    pub target_profile: String,
    pub target_triple: String,
    pub asset: String,
    pub sha256: String,
    pub tree_sha256: String,
    #[serde(default)]
    pub llvm_version: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub disabled_reason: Option<String>,
    #[serde(default = "default_strip_components")]
    pub strip_components: usize,
    #[serde(default)]
    pub required_paths: Vec<String>,
}

/// Manifest embedded at the root of every extracted toolchain asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArosToolchainManifest {
    pub schema: u32,
    pub release_id: String,
    pub host: String,
    pub target_profile: String,
    pub target_triple: String,
    pub tree_sha256: String,
    #[serde(default)]
    pub llvm_version: Option<String>,
    pub files: Vec<ArosToolchainManifestEntry>,
}

/// One canonical entry in the payload inventory and tree digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArosToolchainManifestEntry {
    pub path: String,
    pub mode: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

const fn default_enabled() -> bool {
    true
}

const fn default_strip_components() -> usize {
    1
}

impl ArosToolchainLock {
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let lock: Self = if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            serde_json::from_str(&content).map_err(|error| ArosError::TranspilerSyntax {
                file: path.display().to_string(),
                message: error.to_string(),
            })?
        } else {
            toml::from_str(&content).map_err(|error| ArosError::TranspilerSyntax {
                file: path.display().to_string(),
                message: error.to_string(),
            })?
        };
        lock.validate()
            .map_err(|message| ArosError::TranspilerSyntax {
                file: path.display().to_string(),
                message,
            })?;
        Ok(lock)
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.schema != AROS_TOOLCHAIN_LOCK_SCHEMA {
            return Err(format!(
                "unsupported AROS toolchain lock schema {}; expected {}",
                self.schema, AROS_TOOLCHAIN_LOCK_SCHEMA
            ));
        }
        validate_segment("release_id", &self.release_id)?;
        if self
            .base_url
            .as_deref()
            .is_some_and(|base_url| base_url.trim().is_empty())
        {
            return Err("base_url must be absent or non-empty".into());
        }

        let mut selectors = HashSet::new();
        for artifact in &self.artifacts {
            artifact.validate()?;
            if artifact.enabled
                && !artifact.asset.starts_with("https://")
                && !artifact.asset.starts_with("http://")
                && self.base_url.is_none()
            {
                return Err(format!(
                    "enabled artifact {}/{} has a relative asset but no base_url",
                    artifact.host, artifact.target_profile
                ));
            }
            if !selectors.insert((&artifact.host, &artifact.target_profile)) {
                return Err(format!(
                    "duplicate artifact selector host='{}', target_profile='{}'",
                    artifact.host, artifact.target_profile
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn resolve(&self, host: &str, target_profile: &str) -> Option<&ArosToolchainArtifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.host == host && artifact.target_profile == target_profile)
    }

    pub fn asset_url(
        &self,
        artifact: &ArosToolchainArtifact,
    ) -> std::result::Result<String, String> {
        if artifact.asset.starts_with("https://") || artifact.asset.starts_with("http://") {
            Ok(artifact.asset.clone())
        } else {
            let base_url = self
                .base_url
                .as_deref()
                .ok_or_else(|| "enabled artifact has no download base_url".to_string())?;
            Ok(format!(
                "{}/{}",
                base_url.trim_end_matches('/'),
                artifact.asset.trim_start_matches('/')
            ))
        }
    }
}

impl ArosToolchainArtifact {
    fn validate(&self) -> std::result::Result<(), String> {
        validate_segment("host", &self.host)?;
        validate_segment("target_profile", &self.target_profile)?;
        if self.target_triple.trim().is_empty() {
            return Err("target_triple must not be empty".into());
        }
        validate_sha256("sha256", &self.sha256)?;
        validate_sha256("tree_sha256", &self.tree_sha256)?;
        if self.strip_components > 8 {
            return Err("strip_components must not exceed 8".into());
        }
        for path in &self.required_paths {
            validate_relative_path(path)?;
        }
        if self.enabled {
            if self.asset.trim().is_empty() {
                return Err("enabled artifact must name an asset".into());
            }
            if is_null_sha256(&self.sha256) || is_null_sha256(&self.tree_sha256) {
                return Err("enabled artifact must use non-null SHA256 digests".into());
            }
        } else if self.disabled_reason.as_deref().is_none_or(str::is_empty) {
            return Err(format!(
                "disabled artifact {}/{} needs disabled_reason",
                self.host, self.target_profile
            ));
        }
        Ok(())
    }
}

fn is_null_sha256(value: &str) -> bool {
    value.bytes().all(|byte| byte == b'0')
}

impl ArosToolchainManifest {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
        let content = fs::read_to_string(&path)?;
        let manifest: Self =
            serde_json::from_str(&content).map_err(|error| ArosError::TranspilerSyntax {
                file: path.display().to_string(),
                message: error.to_string(),
            })?;
        if manifest.schema != AROS_TOOLCHAIN_MANIFEST_SCHEMA {
            return Err(ArosError::TranspilerSyntax {
                file: path.display().to_string(),
                message: format!(
                    "unsupported AROS toolchain manifest schema {}; expected {}",
                    manifest.schema, AROS_TOOLCHAIN_MANIFEST_SCHEMA
                ),
            });
        }
        Ok(manifest)
    }
}

fn validate_sha256(field: &str, value: &str) -> std::result::Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!(
            "{field} must contain exactly 64 hexadecimal characters"
        ));
    }
    Ok(())
}

fn validate_segment(field: &str, value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(format!("{field} must be one safe path segment"));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> std::result::Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "required path '{value}' is not a safe relative path"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(host: &str, profile: &str) -> ArosToolchainArtifact {
        ArosToolchainArtifact {
            host: host.into(),
            target_profile: profile.into(),
            target_triple: "x86_64-unknown-aros".into(),
            asset: "toolchain.tar.xz".into(),
            sha256: "1".repeat(64),
            tree_sha256: "2".repeat(64),
            llvm_version: Some("11.0.0".into()),
            size: Some(42),
            enabled: true,
            disabled_reason: None,
            strip_components: 1,
            required_paths: vec!["bin/clang".into()],
        }
    }

    #[test]
    fn resolves_exact_host_and_profile() {
        let lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "test-release".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![artifact("linux-x86_64", "pc-x86_64")],
        };
        lock.validate().unwrap();
        assert!(lock.resolve("linux-x86_64", "pc-x86_64").is_some());
        assert!(lock.resolve("linux-aarch64", "pc-x86_64").is_none());
        assert!(lock.resolve("linux-x86_64", "arm-raspi").is_none());
    }

    #[test]
    fn rejects_duplicate_selector_and_unsafe_path() {
        let mut duplicate = artifact("linux-x86_64", "pc-x86_64");
        duplicate.required_paths = vec!["../escape".into()];
        let lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "test-release".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![duplicate],
        };
        assert!(lock.validate().is_err());

        let duplicate = artifact("linux-x86_64", "pc-x86_64");
        let lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "test-release".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![artifact("linux-x86_64", "pc-x86_64"), duplicate],
        };
        assert!(lock.validate().is_err());
    }

    #[test]
    fn disabled_sentinel_needs_no_url_but_enabled_sentinel_is_rejected() {
        let mut disabled = artifact("linux-x86_64", "pc-x86_64");
        disabled.enabled = false;
        disabled.disabled_reason = Some("not published".into());
        disabled.sha256 = "0".repeat(64);
        disabled.tree_sha256 = "0".repeat(64);
        let lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "unpublished-v1".into(),
            base_url: None,
            artifacts: vec![disabled.clone()],
        };
        lock.validate().unwrap();

        disabled.enabled = true;
        disabled.disabled_reason = None;
        let lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "bad-release".into(),
            base_url: None,
            artifacts: vec![disabled],
        };
        assert!(lock.validate().is_err());
    }

    #[test]
    fn loads_release_index_json_with_the_same_schema_as_toml() {
        let lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "release-v1".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![artifact("linux-x86_64", "pc-x86_64")],
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("toolchain-index-v1.json");
        fs::write(&path, serde_json::to_vec(&lock).unwrap()).unwrap();

        assert_eq!(ArosToolchainLock::load(&path).unwrap(), lock);
    }
}
