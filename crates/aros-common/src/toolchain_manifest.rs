use crate::error::{ArosError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path};
use url::Url;

pub const AROS_TOOLCHAIN_LOCK_SCHEMA: u32 = 1;
pub const AROS_TOOLCHAIN_MANIFEST_SCHEMA: u32 = 1;
pub const AROS_TOOLCHAIN_MANIFEST_FILE: &str = "toolchain-manifest.json";

/// Immutable release selection checked into the selected AROS source tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct ArosToolchainManifest {
    pub schema: u32,
    pub release_id: String,
    pub host: String,
    pub target_profile: String,
    pub target_triple: String,
    pub tree_sha256: String,
    #[serde(default)]
    pub llvm_version: Option<String>,
    pub recipe_sha256: String,
    pub source_lock_sha256: String,
    pub profiles_sha256: String,
    pub source_commit: String,
    pub producer_commit: String,
    pub tools_commit: String,
    pub source_date_epoch: u64,
    pub capabilities: Vec<String>,
    pub build_environment: serde_json::Map<String, serde_json::Value>,
    pub files: Vec<ArosToolchainManifestEntry>,
}

/// One canonical entry in the payload inventory and tree digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Load and validate a JSON or TOML release lock.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, decoded, or validated.
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let lock: Self = if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            serde_json::from_str(&content).map_err(|error| ArosError::Configuration {
                file: path.display().to_string(),
                message: error.to_string(),
            })?
        } else {
            toml::from_str(&content).map_err(|error| ArosError::Configuration {
                file: path.display().to_string(),
                message: error.to_string(),
            })?
        };
        lock.validate()
            .map_err(|message| ArosError::Configuration {
                file: path.display().to_string(),
                message,
            })?;
        Ok(lock)
    }

    /// Validate schema, selectors, paths, URLs, and digest invariants.
    ///
    /// # Errors
    ///
    /// Returns a description of the first violated release-lock invariant.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.schema != AROS_TOOLCHAIN_LOCK_SCHEMA {
            return Err(format!(
                "unsupported AROS toolchain lock schema {}; expected {}",
                self.schema, AROS_TOOLCHAIN_LOCK_SCHEMA
            ));
        }
        validate_segment("release_id", &self.release_id)?;
        if let Some(base_url) = self.base_url.as_deref() {
            parse_credential_free_https_url(base_url)
                .map_err(|message| format!("invalid base_url: {message}"))?;
        }

        let mut selectors = HashSet::new();
        for artifact in &self.artifacts {
            artifact.validate()?;
            if artifact.enabled {
                self.asset_url(artifact)?;
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

    /// Resolve an artifact's absolute or lock-relative download URL.
    ///
    /// # Errors
    ///
    /// Returns an error when a relative asset has no lock-level base URL.
    pub fn asset_url(
        &self,
        artifact: &ArosToolchainArtifact,
    ) -> std::result::Result<String, String> {
        if Url::parse(&artifact.asset).is_ok() || artifact.asset.contains("://") {
            parse_credential_free_https_url(&artifact.asset)
                .map_err(|message| format!("invalid artifact URL: {message}"))?;
            Ok(artifact.asset.clone())
        } else {
            validate_relative_asset(&artifact.asset)?;
            let base_url = self
                .base_url
                .as_deref()
                .ok_or_else(|| "enabled artifact has no download base_url".to_string())?;
            let resolved = format!(
                "{}/{}",
                base_url.trim_end_matches('/'),
                artifact.asset.trim_start_matches('/')
            );
            parse_credential_free_https_url(&resolved)
                .map_err(|message| format!("invalid resolved artifact URL: {message}"))?;
            Ok(resolved)
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
        if !self.asset.is_empty() {
            if Url::parse(&self.asset).is_ok() || self.asset.contains("://") {
                parse_credential_free_https_url(&self.asset)
                    .map_err(|message| format!("invalid artifact URL: {message}"))?;
            } else {
                validate_relative_asset(&self.asset)?;
            }
        }
        Ok(())
    }
}

/// Parse one credential-free HTTPS download URL used by a locked artifact.
///
/// # Errors
///
/// Returns a stable description when the value is not an absolute HTTPS URL,
/// does not name a host, or contains credentials, a query, or a fragment.
pub fn parse_credential_free_https_url(value: &str) -> std::result::Result<Url, String> {
    let parsed = Url::parse(value).map_err(|error| format!("URL is invalid: {error}"))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err("URL must use HTTPS and name a host".into());
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("URL must not contain credentials, a query, or a fragment".into());
    }
    Ok(parsed)
}

fn validate_relative_asset(value: &str) -> std::result::Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains(['\\', '?', '#', ':'])
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir
                    | std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "artifact '{value}' is not a safe relative asset path"
        ));
    }
    Ok(())
}

fn is_null_sha256(value: &str) -> bool {
    value.bytes().all(|byte| byte == b'0')
}

impl ArosToolchainManifest {
    /// Load the installed toolchain manifest below `root`.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest cannot be read or decoded, or uses
    /// an unsupported schema version.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(AROS_TOOLCHAIN_MANIFEST_FILE);
        let content = fs::read_to_string(&path)?;
        let manifest: Self =
            serde_json::from_str(&content).map_err(|error| ArosError::ToolchainManifest {
                file: path.display().to_string(),
                message: error.to_string(),
            })?;
        manifest
            .validate()
            .map_err(|message| ArosError::ToolchainManifest {
                file: path.display().to_string(),
                message,
            })?;
        Ok(manifest)
    }

    fn validate(&self) -> std::result::Result<(), String> {
        if self.schema != AROS_TOOLCHAIN_MANIFEST_SCHEMA {
            return Err(format!(
                "unsupported AROS toolchain manifest schema {}; expected {}",
                self.schema, AROS_TOOLCHAIN_MANIFEST_SCHEMA
            ));
        }
        validate_segment("release_id", &self.release_id)?;
        validate_segment("host", &self.host)?;
        validate_segment("target_profile", &self.target_profile)?;
        if self.target_triple.trim().is_empty() {
            return Err("target_triple must not be empty".into());
        }
        let llvm_version = self
            .llvm_version
            .as_deref()
            .ok_or_else(|| "llvm_version must be present".to_string())?;
        if llvm_version.split('.').count() != 3
            || llvm_version.split('.').any(|component| {
                component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return Err("llvm_version must contain three numeric components".into());
        }
        for (field, digest) in [
            ("tree_sha256", self.tree_sha256.as_str()),
            ("recipe_sha256", self.recipe_sha256.as_str()),
            ("source_lock_sha256", self.source_lock_sha256.as_str()),
            ("profiles_sha256", self.profiles_sha256.as_str()),
        ] {
            validate_lower_sha256(field, digest)?;
        }
        for (field, commit) in [
            ("source_commit", self.source_commit.as_str()),
            ("producer_commit", self.producer_commit.as_str()),
            ("tools_commit", self.tools_commit.as_str()),
        ] {
            validate_git_commit(field, commit)?;
        }
        if self.capabilities.is_empty() {
            return Err("capabilities must not be empty".into());
        }
        let mut capabilities = HashSet::new();
        for capability in &self.capabilities {
            if capability.trim().is_empty() || !capabilities.insert(capability) {
                return Err("capabilities must be non-empty and unique".into());
            }
        }
        if self.files.is_empty() {
            return Err("toolchain file inventory must not be empty".into());
        }
        let mut previous: Option<&str> = None;
        for entry in &self.files {
            validate_manifest_path(&entry.path)?;
            if entry.path == AROS_TOOLCHAIN_MANIFEST_FILE {
                return Err("toolchain manifest must not inventory itself".into());
            }
            if previous.is_some_and(|prior| prior >= entry.path.as_str()) {
                return Err("toolchain file inventory must be strictly path-sorted".into());
            }
            previous = Some(&entry.path);
            match entry.kind.as_str() {
                "directory"
                    if entry.mode == "0755"
                        && entry.sha256.is_none()
                        && entry.size.is_none()
                        && entry.target.is_none() => {}
                "file"
                    if matches!(entry.mode.as_str(), "0644" | "0755")
                        && entry.size.is_some()
                        && entry.target.is_none() =>
                {
                    validate_lower_sha256(
                        "file inventory sha256",
                        entry.sha256.as_deref().ok_or_else(|| {
                            "file inventory entry must contain sha256".to_string()
                        })?,
                    )?;
                }
                "symlink"
                    if entry.mode == "0777"
                        && entry.sha256.is_none()
                        && entry.size.is_none()
                        && entry
                            .target
                            .as_deref()
                            .is_some_and(|target| !target.is_empty()) =>
                {
                    validate_symlink_target(
                        &entry.path,
                        entry.target.as_deref().expect("guarded symlink target"),
                    )?;
                }
                _ => {
                    return Err(format!(
                        "invalid type, mode, or fields for toolchain inventory entry '{}'",
                        entry.path
                    ));
                }
            }
        }
        Ok(())
    }
}

fn validate_lower_sha256(field: &str, value: &str) -> std::result::Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{field} must contain exactly 64 lowercase hexadecimal characters"
        ));
    }
    Ok(())
}

fn validate_git_commit(field: &str, value: &str) -> std::result::Result<(), String> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{field} must contain exactly 40 lowercase hexadecimal characters"
        ));
    }
    Ok(())
}

fn validate_manifest_path(value: &str) -> std::result::Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "inventory path '{value}' is not a safe relative path"
        ));
    }
    Ok(())
}

fn validate_symlink_target(path: &str, target: &str) -> std::result::Result<(), String> {
    if target.contains('\\') || Path::new(target).is_absolute() {
        return Err(format!("symlink '{path}' has an unsafe target '{target}'"));
    }
    let mut depth = Path::new(path).parent().map_or(0, |parent| {
        parent
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .count()
    });
    for component in Path::new(target).components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("symlink '{path}' escapes the toolchain root"));
            }
        }
    }
    Ok(())
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

    #[test]
    fn malformed_lock_is_reported_as_configuration_not_transpiler_syntax() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("aros-toolchains.lock.toml");
        fs::write(&path, "schema = [not-valid").unwrap();
        assert!(matches!(
            ArosToolchainLock::load(&path).unwrap_err(),
            ArosError::Configuration { .. }
        ));
    }

    #[test]
    fn malformed_installed_manifest_has_its_own_error_category() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(AROS_TOOLCHAIN_MANIFEST_FILE),
            b"{not-json",
        )
        .unwrap();
        assert!(matches!(
            ArosToolchainManifest::load(directory.path()).unwrap_err(),
            ArosError::ToolchainManifest { .. }
        ));
    }

    #[test]
    fn current_producer_manifest_contract_is_exact_and_required() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(AROS_TOOLCHAIN_MANIFEST_FILE);
        let mut manifest = serde_json::json!({
            "schema": AROS_TOOLCHAIN_MANIFEST_SCHEMA,
            "release_id": "toolchain-v1-test",
            "host": "macos-aarch64",
            "target_profile": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros",
            "tree_sha256": "1".repeat(64),
            "llvm_version": "11.0.0",
            "recipe_sha256": "2".repeat(64),
            "source_lock_sha256": "3".repeat(64),
            "profiles_sha256": "4".repeat(64),
            "source_commit": "5".repeat(40),
            "producer_commit": "6".repeat(40),
            "tools_commit": "7".repeat(40),
            "source_date_epoch": 1,
            "capabilities": ["collector"],
            "build_environment": {"runner": "macos-15"},
            "files": [{
                "path": "bin/clang",
                "mode": "0755",
                "type": "file",
                "sha256": "8".repeat(64),
                "size": 42
            }]
        });
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let loaded = ArosToolchainManifest::load(directory.path()).unwrap();
        assert_eq!(
            loaded.build_environment.get("runner"),
            Some(&serde_json::json!("macos-15"))
        );

        manifest
            .as_object_mut()
            .unwrap()
            .remove("build_environment");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(matches!(
            ArosToolchainManifest::load(directory.path()).unwrap_err(),
            ArosError::ToolchainManifest { .. }
        ));
    }

    #[test]
    fn rejects_unknown_lock_and_artifact_fields() {
        let root_unknown = r#"
            schema = 1
            release_id = "release-v1"
            unexpected_policy = true
        "#;
        assert!(toml::from_str::<ArosToolchainLock>(root_unknown).is_err());

        let artifact_unknown = format!(
            r#"
                schema = 1
                release_id = "release-v1"
                base_url = "https://example.invalid/release"

                [[artifacts]]
                host = "linux-x86_64"
                target_profile = "pc-x86_64"
                target_triple = "x86_64-unknown-aros"
                asset = "toolchain.tar.xz"
                sha256 = "{}"
                tree_sha256 = "{}"
                enabled_typo = false
            "#,
            "1".repeat(64),
            "2".repeat(64)
        );
        assert!(toml::from_str::<ArosToolchainLock>(&artifact_unknown).is_err());
    }

    #[test]
    fn rejects_unknown_manifest_and_entry_fields() {
        let manifest_unknown = serde_json::json!({
            "schema": AROS_TOOLCHAIN_MANIFEST_SCHEMA,
            "release_id": "release-v1",
            "host": "linux-x86_64",
            "target_profile": "pc-x86_64",
            "target_triple": "x86_64-unknown-aros",
            "tree_sha256": "2".repeat(64),
            "files": [],
            "unexpected_policy": true
        });
        assert!(serde_json::from_value::<ArosToolchainManifest>(manifest_unknown).is_err());

        let entry_unknown = serde_json::json!({
            "path": "bin/clang",
            "mode": "0755",
            "type": "file",
            "sha256": "1".repeat(64),
            "size": 42,
            "unexpected_policy": true
        });
        assert!(serde_json::from_value::<ArosToolchainManifestEntry>(entry_unknown).is_err());
    }

    #[test]
    fn download_urls_are_credential_free_https_origins() {
        assert!(parse_credential_free_https_url(
            "https://example.invalid/releases/toolchain.tar.xz"
        )
        .is_ok());
        for invalid in [
            "http://example.invalid/toolchain.tar.xz",
            "https://user@example.invalid/toolchain.tar.xz",
            "https://example.invalid/toolchain.tar.xz?token=secret",
            "https://example.invalid/toolchain.tar.xz#fragment",
            "file:///tmp/toolchain.tar.xz",
            "not-a-url",
        ] {
            assert!(
                parse_credential_free_https_url(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn lock_validation_rejects_insecure_and_unsafe_asset_locations() {
        let mut insecure_base = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "release-v1".into(),
            base_url: Some("http://example.invalid/release".into()),
            artifacts: vec![artifact("linux-x86_64", "pc-x86_64")],
        };
        assert!(insecure_base.validate().is_err());

        insecure_base.base_url = None;
        insecure_base.artifacts[0].asset = "http://example.invalid/toolchain.tar.xz".into();
        assert!(insecure_base.validate().is_err());

        insecure_base.base_url = Some("https://example.invalid/release".into());
        insecure_base.artifacts[0].asset = "../toolchain.tar.xz".into();
        assert!(insecure_base.validate().is_err());
    }
}
