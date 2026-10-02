use crate::error::{ArosError, Result};
use serde::ser::{SerializeSeq, SerializeStruct};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path};
use url::Url;

pub const AROS_TOOLCHAIN_LOCK_SCHEMA: u32 = 1;
pub const AROS_TOOLCHAIN_LOCK_SCHEMA_V2: u32 = 2;
pub const AROS_TOOLCHAIN_MANIFEST_SCHEMA: u32 = 1;
pub const AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2: u32 = 2;
pub const AROS_TOOLCHAIN_MANIFEST_FILE: &str = "toolchain-manifest.json";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_VERSION_COMPONENT: u32 = 2_147_483_647;

/// Immutable release selection checked into the selected AROS source tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArosToolchainLock {
    pub schema: u32,
    pub release_id: String,
    pub base_url: Option<String>,
    pub artifacts: Vec<ArosToolchainArtifact>,
}

/// One host and target-profile-specific AROS cross-toolchain archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArosToolchainArtifact {
    pub host: String,
    pub target_profile: String,
    pub target_triple: String,
    pub asset: String,
    pub sha256: String,
    pub tree_sha256: String,
    pub llvm_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler: Option<ArosCompilerIdentity>,
    pub size: Option<u64>,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub strip_components: usize,
    pub required_paths: Vec<String>,
}

/// Manifest embedded at the root of every extracted toolchain asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArosToolchainManifest {
    pub schema: u32,
    pub release_id: String,
    pub host: String,
    pub target_profile: String,
    pub target_triple: String,
    pub tree_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llvm_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compiler: Option<ArosCompilerIdentity>,
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

/// Source-pinned compiler identity carried by schema-v2 toolchain metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "family", deny_unknown_fields)]
pub enum ArosCompilerIdentity {
    #[serde(rename = "llvm")]
    Llvm { version: String },
    #[serde(rename = "gnu")]
    Gnu {
        gcc_version: String,
        binutils_version: String,
        target: crate::elf::riscv::TargetContract,
    },
}

impl Serialize for ArosToolchainLock {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut record = serializer.serialize_struct("ArosToolchainLock", 4)?;
        record.serialize_field("schema", &self.schema)?;
        record.serialize_field("release_id", &self.release_id)?;
        record.serialize_field("base_url", &self.base_url)?;
        record.serialize_field(
            "artifacts",
            &SchemaArtifactList {
                schema: self.schema,
                artifacts: &self.artifacts,
            },
        )?;
        record.end()
    }
}

struct SchemaArtifactList<'a> {
    schema: u32,
    artifacts: &'a [ArosToolchainArtifact],
}

impl Serialize for SchemaArtifactList<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.artifacts.len()))?;
        for artifact in self.artifacts {
            if self.schema == AROS_TOOLCHAIN_LOCK_SCHEMA_V2 {
                sequence.serialize_element(&ArtifactRecordV2Ref::from(artifact))?;
            } else {
                sequence.serialize_element(&ArtifactRecordV1Ref::from(artifact))?;
            }
        }
        sequence.end()
    }
}

#[derive(Serialize)]
struct ArtifactRecordV1Ref<'a> {
    host: &'a str,
    target_profile: &'a str,
    target_triple: &'a str,
    asset: &'a str,
    sha256: &'a str,
    tree_sha256: &'a str,
    llvm_version: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiler: Option<&'a ArosCompilerIdentity>,
    size: Option<u64>,
    enabled: bool,
    disabled_reason: Option<&'a str>,
    strip_components: usize,
    required_paths: &'a [String],
}

impl<'a> From<&'a ArosToolchainArtifact> for ArtifactRecordV1Ref<'a> {
    fn from(artifact: &'a ArosToolchainArtifact) -> Self {
        Self {
            host: &artifact.host,
            target_profile: &artifact.target_profile,
            target_triple: &artifact.target_triple,
            asset: &artifact.asset,
            sha256: &artifact.sha256,
            tree_sha256: &artifact.tree_sha256,
            llvm_version: artifact.llvm_version.as_deref(),
            compiler: artifact.compiler.as_ref(),
            size: artifact.size,
            enabled: artifact.enabled,
            disabled_reason: artifact.disabled_reason.as_deref(),
            strip_components: artifact.strip_components,
            required_paths: &artifact.required_paths,
        }
    }
}

#[derive(Serialize)]
struct ArtifactRecordV2Ref<'a> {
    host: &'a str,
    target_profile: &'a str,
    target_triple: &'a str,
    asset: &'a str,
    sha256: &'a str,
    tree_sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiler: Option<&'a ArosCompilerIdentity>,
    size: Option<u64>,
    enabled: bool,
    disabled_reason: Option<&'a str>,
    strip_components: usize,
    required_paths: &'a [String],
}

impl<'a> From<&'a ArosToolchainArtifact> for ArtifactRecordV2Ref<'a> {
    fn from(artifact: &'a ArosToolchainArtifact) -> Self {
        Self {
            host: &artifact.host,
            target_profile: &artifact.target_profile,
            target_triple: &artifact.target_triple,
            asset: &artifact.asset,
            sha256: &artifact.sha256,
            tree_sha256: &artifact.tree_sha256,
            compiler: artifact.compiler.as_ref(),
            size: artifact.size,
            enabled: artifact.enabled,
            disabled_reason: artifact.disabled_reason.as_deref(),
            strip_components: artifact.strip_components,
            required_paths: &artifact.required_paths,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "family", deny_unknown_fields)]
enum ArosCompilerIdentityRecord {
    #[serde(rename = "llvm")]
    Llvm { version: String },
    #[serde(rename = "gnu")]
    Gnu {
        gcc_version: String,
        binutils_version: String,
        target: crate::elf::riscv::TargetContract,
    },
}

impl<'de> Deserialize<'de> for ArosCompilerIdentity {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let record = ArosCompilerIdentityRecord::deserialize(deserializer)?;
        let identity = match record {
            ArosCompilerIdentityRecord::Llvm { version } => Self::Llvm { version },
            ArosCompilerIdentityRecord::Gnu {
                gcc_version,
                binutils_version,
                target,
            } => Self::Gnu {
                gcc_version,
                binutils_version,
                target,
            },
        };
        identity
            .validate_versions()
            .map_err(serde::de::Error::custom)?;
        Ok(identity)
    }
}

impl ArosCompilerIdentity {
    /// Compiler family identifier used by the toolchain metadata contract.
    #[must_use]
    pub const fn family(&self) -> &'static str {
        match self {
            Self::Llvm { .. } => "llvm",
            Self::Gnu { .. } => "gnu",
        }
    }

    /// Validate version fields and their relation to the enclosing target triple.
    ///
    /// # Errors
    /// Rejects unsupported numeric version forms or, for GNU, a target triple
    /// whose RISC-V CPU width does not agree with the embedded target ABI.
    pub fn validate_for_target(&self, target_triple: &str) -> std::result::Result<(), String> {
        self.validate_versions()?;
        if let Self::Gnu { target, .. } = self {
            validate_gnu_target_triple(target_triple, target.abi())?;
        }
        Ok(())
    }

    fn validate_versions(&self) -> std::result::Result<(), String> {
        match self {
            Self::Llvm { version } => validate_numeric_version("LLVM version", version, 3, 3),
            Self::Gnu {
                gcc_version,
                binutils_version,
                ..
            } => {
                validate_numeric_version("GCC version", gcc_version, 3, 3)?;
                validate_numeric_version("binutils version", binutils_version, 2, 4)
            }
        }
    }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArosToolchainLockRecord {
    schema: u32,
    release_id: String,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    artifacts: Vec<ArosToolchainArtifactRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArosToolchainArtifactRecord {
    host: String,
    target_profile: String,
    target_triple: String,
    asset: String,
    sha256: String,
    tree_sha256: String,
    #[serde(default, deserialize_with = "deserialize_nullable_presence")]
    llvm_version: PresentField<String>,
    #[serde(default, deserialize_with = "deserialize_present_non_null")]
    compiler: Option<ArosCompilerIdentity>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    disabled_reason: Option<String>,
    #[serde(default = "default_strip_components")]
    strip_components: usize,
    #[serde(default)]
    required_paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArosToolchainManifestRecord {
    schema: u32,
    release_id: String,
    host: String,
    target_profile: String,
    target_triple: String,
    tree_sha256: String,
    #[serde(default, deserialize_with = "deserialize_nullable_presence")]
    llvm_version: PresentField<String>,
    #[serde(default, deserialize_with = "deserialize_present_non_null")]
    compiler: Option<ArosCompilerIdentity>,
    recipe_sha256: String,
    source_lock_sha256: String,
    profiles_sha256: String,
    source_commit: String,
    producer_commit: String,
    tools_commit: String,
    source_date_epoch: u64,
    capabilities: Vec<String>,
    build_environment: serde_json::Map<String, serde_json::Value>,
    files: Vec<ArosToolchainManifestEntry>,
}

impl<'de> Deserialize<'de> for ArosToolchainLock {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let record = ArosToolchainLockRecord::deserialize(deserializer)?;
        let artifacts = record
            .artifacts
            .into_iter()
            .map(|artifact| ArosToolchainArtifact::from_record(artifact, record.schema))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(serde::de::Error::custom)?;
        Ok(Self {
            schema: record.schema,
            release_id: record.release_id,
            base_url: record.base_url,
            artifacts,
        })
    }
}

impl<'de> Deserialize<'de> for ArosToolchainArtifact {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let record = ArosToolchainArtifactRecord::deserialize(deserializer)?;
        Self::from_record(record, 0).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ArosToolchainManifest {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let record = ArosToolchainManifestRecord::deserialize(deserializer)?;
        let (llvm_version, compiler) =
            compiler_fields_from_record(record.schema, record.llvm_version, record.compiler, true)
                .map_err(serde::de::Error::custom)?;
        Ok(Self {
            schema: record.schema,
            release_id: record.release_id,
            host: record.host,
            target_profile: record.target_profile,
            target_triple: record.target_triple,
            tree_sha256: record.tree_sha256,
            llvm_version,
            compiler,
            recipe_sha256: record.recipe_sha256,
            source_lock_sha256: record.source_lock_sha256,
            profiles_sha256: record.profiles_sha256,
            source_commit: record.source_commit,
            producer_commit: record.producer_commit,
            tools_commit: record.tools_commit,
            source_date_epoch: record.source_date_epoch,
            capabilities: record.capabilities,
            build_environment: record.build_environment,
            files: record.files,
        })
    }
}

impl ArosToolchainArtifact {
    fn from_record(
        record: ArosToolchainArtifactRecord,
        schema: u32,
    ) -> std::result::Result<Self, String> {
        let (llvm_version, compiler) =
            compiler_fields_from_record(schema, record.llvm_version, record.compiler, false)?;
        Ok(Self {
            host: record.host,
            target_profile: record.target_profile,
            target_triple: record.target_triple,
            asset: record.asset,
            sha256: record.sha256,
            tree_sha256: record.tree_sha256,
            llvm_version,
            compiler,
            size: record.size,
            enabled: record.enabled,
            disabled_reason: record.disabled_reason,
            strip_components: record.strip_components,
            required_paths: record.required_paths,
        })
    }
}

fn compiler_fields_from_record(
    schema: u32,
    llvm_version: PresentField<String>,
    compiler: Option<ArosCompilerIdentity>,
    require_legacy_llvm: bool,
) -> std::result::Result<(Option<String>, Option<ArosCompilerIdentity>), String> {
    if schema == AROS_TOOLCHAIN_LOCK_SCHEMA || schema == AROS_TOOLCHAIN_MANIFEST_SCHEMA {
        if compiler.is_some() {
            return Err("schema 1 metadata must not contain compiler".into());
        }
        if require_legacy_llvm && !matches!(&llvm_version, PresentField::Value(_)) {
            return Err("llvm_version must be present".into());
        }
        Ok((llvm_version.into_value(), None))
    } else if schema == AROS_TOOLCHAIN_LOCK_SCHEMA_V2 || schema == AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2
    {
        if !matches!(&llvm_version, PresentField::Absent) {
            return Err("schema 2 metadata must not contain llvm_version".into());
        }
        let compiler = compiler.ok_or_else(|| "schema 2 metadata requires compiler".to_string())?;
        Ok((None, Some(compiler)))
    } else {
        Ok((llvm_version.into_value(), compiler))
    }
}

fn deserialize_present_non_null<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Default)]
enum PresentField<T> {
    #[default]
    Absent,
    Null,
    Value(T),
}

impl<T> PresentField<T> {
    fn into_value(self) -> Option<T> {
        match self {
            Self::Absent | Self::Null => None,
            Self::Value(value) => Some(value),
        }
    }
}

fn deserialize_nullable_presence<'de, D, T>(
    deserializer: D,
) -> std::result::Result<PresentField<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?
        .map_or_else(|| PresentField::Null, PresentField::Value))
}

fn normalized_compiler_identity(
    llvm_version: Option<&str>,
    compiler: Option<&ArosCompilerIdentity>,
) -> std::result::Result<ArosCompilerIdentity, String> {
    match (llvm_version, compiler) {
        (Some(version), None) => Ok(ArosCompilerIdentity::Llvm {
            version: version.to_owned(),
        }),
        (None, Some(identity)) => Ok(identity.clone()),
        (Some(_), Some(_)) => Err("metadata mixes llvm_version and compiler".into()),
        (None, None) => Err("metadata does not declare a compiler identity".into()),
    }
}

fn validate_compiler_contract(
    schema: u32,
    llvm_version: Option<&str>,
    compiler: Option<&ArosCompilerIdentity>,
    target_triple: &str,
    require_legacy_llvm: bool,
) -> std::result::Result<(), String> {
    let legacy = schema == AROS_TOOLCHAIN_LOCK_SCHEMA || schema == AROS_TOOLCHAIN_MANIFEST_SCHEMA;
    let current =
        schema == AROS_TOOLCHAIN_LOCK_SCHEMA_V2 || schema == AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2;
    if legacy {
        if compiler.is_some() {
            return Err("schema 1 metadata must not contain compiler".into());
        }
        if require_legacy_llvm {
            let version = llvm_version.ok_or_else(|| "llvm_version must be present".to_string())?;
            validate_legacy_llvm_version(version)?;
        }
        Ok(())
    } else if current {
        if llvm_version.is_some() {
            return Err("schema 2 metadata must not contain llvm_version".into());
        }
        compiler
            .ok_or_else(|| "schema 2 metadata requires compiler".to_string())?
            .validate_for_target(target_triple)
    } else {
        Err(format!("unsupported toolchain metadata schema {schema}"))
    }
}

fn validate_legacy_llvm_version(version: &str) -> std::result::Result<(), String> {
    if version.split('.').count() != 3
        || version.split('.').any(|component| {
            component.is_empty() || !component.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err("llvm_version must contain three numeric components".into());
    }
    Ok(())
}

fn validate_numeric_version(
    field: &str,
    version: &str,
    min_components: usize,
    max_components: usize,
) -> std::result::Result<(), String> {
    let invalid_version = || {
        format!(
            "{field} must contain {min_components}..={max_components} bounded numeric components"
        )
    };
    if version.len() > max_components.saturating_mul(11) {
        return Err(invalid_version());
    }
    let mut count = 0;
    for component in version.split('.') {
        count += 1;
        if component.is_empty()
            || component.len() > 10
            || !component.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(invalid_version());
        }
        let value = component.parse::<u32>().map_err(|_| invalid_version())?;
        if value > MAX_VERSION_COMPONENT {
            return Err(invalid_version());
        }
        if count > max_components {
            return Err(invalid_version());
        }
    }
    if !(min_components..=max_components).contains(&count) {
        return Err(invalid_version());
    }
    Ok(())
}

fn validate_gnu_target_triple(target_triple: &str, abi: &str) -> std::result::Result<(), String> {
    let prefix = target_triple
        .strip_suffix("-aros")
        .ok_or_else(|| "GNU target_triple must end in -aros".to_string())?;
    if prefix.is_empty()
        || prefix.len() > 128
        || prefix.split('-').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
        })
    {
        return Err("GNU target_triple must contain safe identifier components".into());
    }
    let cpu = prefix.split('-').next().unwrap_or_default();
    let expected_cpu = if abi.starts_with("ilp32") {
        "riscv"
    } else if abi.starts_with("lp64") {
        "riscv64"
    } else {
        return Err("GNU target contract has an unsupported RISC-V ABI width".into());
    };
    if cpu != expected_cpu {
        return Err("GNU target_triple CPU differs from the target ABI width".into());
    }
    Ok(())
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
        if !matches!(
            self.schema,
            AROS_TOOLCHAIN_LOCK_SCHEMA | AROS_TOOLCHAIN_LOCK_SCHEMA_V2
        ) {
            return Err(format!(
                "unsupported AROS toolchain lock schema {}; expected {} or {}",
                self.schema, AROS_TOOLCHAIN_LOCK_SCHEMA, AROS_TOOLCHAIN_LOCK_SCHEMA_V2
            ));
        }
        validate_segment("release_id", &self.release_id)?;
        if let Some(base_url) = self.base_url.as_deref() {
            parse_credential_free_https_url(base_url)
                .map_err(|message| format!("invalid base_url: {message}"))?;
        }

        let mut selectors = HashSet::new();
        for artifact in &self.artifacts {
            artifact.validate(self.schema)?;
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
    /// Return a normalized compiler identity for either legacy LLVM or v2 metadata.
    ///
    /// # Errors
    /// Returns an error when this value mixes schema-family fields or does not
    /// carry a compiler declaration.
    pub fn compiler_identity(&self) -> std::result::Result<ArosCompilerIdentity, String> {
        normalized_compiler_identity(self.llvm_version.as_deref(), self.compiler.as_ref())
    }

    fn validate(&self, schema: u32) -> std::result::Result<(), String> {
        validate_segment("host", &self.host)?;
        validate_segment("target_profile", &self.target_profile)?;
        if self.target_triple.trim().is_empty() {
            return Err("target_triple must not be empty".into());
        }
        validate_compiler_contract(
            schema,
            self.llvm_version.as_deref(),
            self.compiler.as_ref(),
            &self.target_triple,
            false,
        )?;
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
        let file = crate::publication::open_regular_file_nofollow(&path)?;
        let mut content = String::new();
        file.take(MAX_MANIFEST_BYTES + 1)
            .read_to_string(&mut content)?;
        if content.len() > usize::try_from(MAX_MANIFEST_BYTES).unwrap_or(usize::MAX) {
            return Err(ArosError::ToolchainManifest {
                file: path.display().to_string(),
                message: format!("manifest exceeds the {MAX_MANIFEST_BYTES}-byte limit"),
            });
        }
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

    /// Return a normalized LLVM or GNU compiler identity.
    ///
    /// # Errors
    /// Returns an error when schema-specific compiler fields are incomplete or
    /// mixed. Call [`Self::validate`] before treating the result as evidence.
    pub fn compiler_identity(&self) -> std::result::Result<ArosCompilerIdentity, String> {
        normalized_compiler_identity(self.llvm_version.as_deref(), self.compiler.as_ref())
    }

    /// Validate the fully decoded manifest against its schema-specific contract.
    ///
    /// This is public so native producers can validate the exact manifest
    /// bytes they are about to embed and publish using the same rules as
    /// consumers.
    ///
    /// # Errors
    ///
    /// Returns a stable explanatory message when any field, identity, or
    /// inventory entry violates the manifest contract.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if !matches!(
            self.schema,
            AROS_TOOLCHAIN_MANIFEST_SCHEMA | AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2
        ) {
            return Err(format!(
                "unsupported AROS toolchain manifest schema {}; expected {} or {}",
                self.schema, AROS_TOOLCHAIN_MANIFEST_SCHEMA, AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2
            ));
        }
        validate_segment("release_id", &self.release_id)?;
        validate_segment("host", &self.host)?;
        validate_segment("target_profile", &self.target_profile)?;
        if self.target_triple.trim().is_empty() {
            return Err("target_triple must not be empty".into());
        }
        validate_compiler_contract(
            self.schema,
            self.llvm_version.as_deref(),
            self.compiler.as_ref(),
            &self.target_triple,
            true,
        )?;
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
                    if entry.mode == "0777" && entry.sha256.is_none() && entry.size.is_none() =>
                {
                    let target = entry
                        .target
                        .as_deref()
                        .filter(|target| !target.is_empty())
                        .ok_or_else(|| {
                            "symlink inventory entry must contain a nonempty target".to_string()
                        })?;
                    validate_symlink_target(&entry.path, target)?;
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
            compiler: None,
            size: Some(42),
            enabled: true,
            disabled_reason: None,
            strip_components: 1,
            required_paths: vec!["bin/clang".into()],
        }
    }

    fn target_contract(abi: &str) -> crate::elf::riscv::TargetContract {
        let (isa, architecture) = if abi.starts_with("lp64") {
            ("rva22u64", "rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0")
        } else {
            ("rv32imac", "rv32i2p1_m2p0_a2p1_c2p0")
        };
        serde_json::from_value(serde_json::json!({
            "schema": "aros-riscv-target-v1",
            "isa": isa,
            "abi": abi,
            "code_model": "medany",
            "architecture": architecture,
            "unaligned_access": false,
            "atomic_abi": 0,
            "x3_reg_usage": 0
        }))
        .unwrap()
    }

    fn gnu_identity(abi: &str) -> ArosCompilerIdentity {
        ArosCompilerIdentity::Gnu {
            gcc_version: "16.2.0".into(),
            binutils_version: "2.47".into(),
            target: target_contract(abi),
        }
    }

    fn manifest_v1() -> ArosToolchainManifest {
        ArosToolchainManifest {
            schema: AROS_TOOLCHAIN_MANIFEST_SCHEMA,
            release_id: "release-v1".into(),
            host: "linux-x86_64".into(),
            target_profile: "pc-x86_64".into(),
            target_triple: "x86_64-unknown-aros".into(),
            tree_sha256: "1".repeat(64),
            llvm_version: Some("11.0.0".into()),
            compiler: None,
            recipe_sha256: "2".repeat(64),
            source_lock_sha256: "3".repeat(64),
            profiles_sha256: "4".repeat(64),
            source_commit: "5".repeat(40),
            producer_commit: "6".repeat(40),
            tools_commit: "7".repeat(40),
            source_date_epoch: 1,
            capabilities: vec!["collector".into()],
            build_environment: serde_json::Map::new(),
            files: vec![ArosToolchainManifestEntry {
                path: "bin/clang".into(),
                mode: "0755".into(),
                kind: "file".into(),
                sha256: Some("8".repeat(64)),
                size: Some(42),
                target: None,
            }],
        }
    }

    fn manifest_v2(target_triple: &str, compiler: ArosCompilerIdentity) -> ArosToolchainManifest {
        let mut manifest = manifest_v1();
        manifest.schema = AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2;
        manifest.target_profile = if target_triple.starts_with("riscv-") {
            "rv32-aros"
        } else {
            "rv64-aros"
        }
        .into();
        manifest.target_triple = target_triple.into();
        manifest.llvm_version = None;
        manifest.compiler = Some(compiler);
        manifest
    }

    fn artifact_v2(abi: &str) -> ArosToolchainArtifact {
        let mut item = artifact("linux-x86_64", "rv64-aros");
        item.target_triple = if abi.starts_with("lp64") {
            "riscv64-unknown-aros"
        } else {
            "riscv-unknown-aros"
        }
        .into();
        item.llvm_version = None;
        item.compiler = Some(gnu_identity(abi));
        item
    }

    fn lock_v2(abi: &str) -> ArosToolchainLock {
        ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA_V2,
            release_id: "release-v2".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![artifact_v2(abi)],
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

    #[cfg(unix)]
    #[test]
    fn installed_manifest_loader_rejects_a_symbolic_link() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = tempfile::NamedTempFile::new().unwrap();
        fs::write(target.path(), b"{not-json").unwrap();
        symlink(
            target.path(),
            directory.path().join(AROS_TOOLCHAIN_MANIFEST_FILE),
        )
        .unwrap();

        assert!(matches!(
            ArosToolchainManifest::load(directory.path()).unwrap_err(),
            ArosError::Io { .. }
        ));
    }

    #[test]
    fn installed_manifest_loader_bounds_a_regular_file_before_decoding() {
        let directory = tempfile::tempdir().unwrap();
        let oversized = vec![b' '; usize::try_from(MAX_MANIFEST_BYTES + 1).unwrap()];
        fs::write(
            directory.path().join(AROS_TOOLCHAIN_MANIFEST_FILE),
            oversized,
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

    #[test]
    fn schema_v1_manifest_and_lock_keep_legacy_llvm_roundtrips() {
        let manifest = manifest_v1();
        manifest.validate().unwrap();
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let expected_manifest = format!(
            concat!(
                "{{\"schema\":1,\"release_id\":\"release-v1\",\"host\":\"linux-x86_64\",",
                "\"target_profile\":\"pc-x86_64\",\"target_triple\":\"x86_64-unknown-aros\",",
                "\"tree_sha256\":\"{}\",\"llvm_version\":\"11.0.0\",",
                "\"recipe_sha256\":\"{}\",\"source_lock_sha256\":\"{}\",",
                "\"profiles_sha256\":\"{}\",\"source_commit\":\"{}\",",
                "\"producer_commit\":\"{}\",\"tools_commit\":\"{}\",",
                "\"source_date_epoch\":1,\"capabilities\":[\"collector\"],",
                "\"build_environment\":{{}},\"files\":[{{\"path\":\"bin/clang\",",
                "\"mode\":\"0755\",\"type\":\"file\",\"sha256\":\"{}\",\"size\":42}}]}}"
            ),
            "1".repeat(64),
            "2".repeat(64),
            "3".repeat(64),
            "4".repeat(64),
            "5".repeat(40),
            "6".repeat(40),
            "7".repeat(40),
            "8".repeat(64)
        );
        assert_eq!(manifest_bytes, expected_manifest.as_bytes());
        let parsed: ArosToolchainManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        parsed.validate().unwrap();
        assert_eq!(parsed, manifest);
        assert_eq!(
            parsed.compiler_identity().unwrap(),
            ArosCompilerIdentity::Llvm {
                version: "11.0.0".into()
            }
        );

        let mut legacy_artifact = artifact("linux-x86_64", "pc-x86_64");
        legacy_artifact.llvm_version = None;
        let lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "release-v1".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![legacy_artifact],
        };
        lock.validate().unwrap();
        let bytes = serde_json::to_vec(&lock).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("\"llvm_version\":null"));
        let parsed: ArosToolchainLock = serde_json::from_slice(&bytes).unwrap();
        parsed.validate().unwrap();
        assert_eq!(serde_json::to_vec(&parsed).unwrap(), bytes);
    }

    #[test]
    fn schema_v2_accepts_llvm_and_gnu_artifacts_and_manifests() {
        let llvm_identity = ArosCompilerIdentity::Llvm {
            version: "17.0.6".into(),
        };
        assert_eq!(llvm_identity.family(), "llvm");
        let mut llvm_artifact = artifact("linux-x86_64", "pc-x86_64");
        llvm_artifact.target_triple = "x86_64-unknown-aros".into();
        llvm_artifact.llvm_version = None;
        llvm_artifact.compiler = Some(llvm_identity.clone());
        let llvm_lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA_V2,
            release_id: "release-v2".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![llvm_artifact],
        };
        llvm_lock.validate().unwrap();
        let lock_value = serde_json::to_value(&llvm_lock).unwrap();
        assert_eq!(lock_value["artifacts"][0]["compiler"]["family"], "llvm");
        assert!(lock_value["artifacts"][0].get("llvm_version").is_none());
        let parsed_lock: ArosToolchainLock = serde_json::from_value(lock_value).unwrap();
        parsed_lock.validate().unwrap();
        assert_eq!(
            parsed_lock.artifacts[0].compiler_identity().unwrap(),
            llvm_identity
        );
        let llvm_manifest = manifest_v2(
            "x86_64-unknown-aros",
            ArosCompilerIdentity::Llvm {
                version: "17.0.6".into(),
            },
        );
        llvm_manifest.validate().unwrap();
        let parsed_llvm_manifest: ArosToolchainManifest =
            serde_json::from_slice(&serde_json::to_vec(&llvm_manifest).unwrap()).unwrap();
        parsed_llvm_manifest.validate().unwrap();
        assert_eq!(
            parsed_llvm_manifest.compiler_identity().unwrap().family(),
            "llvm"
        );

        for (abi, triple) in [
            ("ilp32", "riscv-unknown-aros"),
            ("lp64d", "riscv64-unknown-aros"),
        ] {
            let identity = gnu_identity(abi);
            assert_eq!(identity.family(), "gnu");
            identity.validate_for_target(triple).unwrap();

            let lock = lock_v2(abi);
            lock.validate().unwrap();
            let lock_value = serde_json::to_value(&lock).unwrap();
            assert_eq!(lock_value["artifacts"][0]["compiler"]["family"], "gnu");
            assert!(lock_value["artifacts"][0].get("llvm_version").is_none());
            let parsed_lock: ArosToolchainLock = serde_json::from_value(lock_value).unwrap();
            parsed_lock.validate().unwrap();
            assert_eq!(parsed_lock.artifacts[0], lock.artifacts[0]);

            let manifest = manifest_v2(triple, identity);
            manifest.validate().unwrap();
            let manifest_value = serde_json::to_value(&manifest).unwrap();
            assert_eq!(manifest_value["compiler"]["family"], "gnu");
            assert!(manifest_value.get("llvm_version").is_none());
            let parsed_manifest: ArosToolchainManifest =
                serde_json::from_value(manifest_value).unwrap();
            parsed_manifest.validate().unwrap();
            assert_eq!(parsed_manifest.compiler_identity().unwrap().family(), "gnu");
        }
    }

    #[test]
    fn schema_specific_fields_reject_null_missing_and_mixed_family_declarations() {
        let v1_manifest = serde_json::to_value(manifest_v1()).unwrap();
        let mut missing_v1_version = v1_manifest.clone();
        missing_v1_version
            .as_object_mut()
            .unwrap()
            .remove("llvm_version");
        assert!(serde_json::from_value::<ArosToolchainManifest>(missing_v1_version).is_err());
        let duplicate_manifest = serde_json::to_string(&manifest_v1()).unwrap().replacen(
            "\"schema\":1,",
            "\"schema\":1,\"schema\":1,",
            1,
        );
        assert!(serde_json::from_str::<ArosToolchainManifest>(&duplicate_manifest).is_err());
        for compiler in [
            serde_json::Value::Null,
            serde_json::json!({"family":"llvm","version":"17.0.6"}),
        ] {
            let mut value = v1_manifest.clone();
            value["compiler"] = compiler;
            assert!(serde_json::from_value::<ArosToolchainManifest>(value).is_err());
        }

        let v1_lock = serde_json::to_value(ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "release-v1".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![artifact("linux-x86_64", "pc-x86_64")],
        })
        .unwrap();
        let duplicate_lock = serde_json::to_string(&ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "release-v1".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![artifact("linux-x86_64", "pc-x86_64")],
        })
        .unwrap()
        .replacen("\"schema\":1,", "\"schema\":1,\"schema\":1,", 1);
        assert!(serde_json::from_str::<ArosToolchainLock>(&duplicate_lock).is_err());
        for compiler in [serde_json::Value::Null, serde_json::json!({"family":"gnu"})] {
            let mut value = v1_lock.clone();
            value["artifacts"][0]["compiler"] = compiler;
            assert!(serde_json::from_value::<ArosToolchainLock>(value).is_err());
        }

        let valid_gnu = manifest_v2("riscv64-unknown-aros", gnu_identity("lp64d"));
        let valid_gnu_value = serde_json::to_value(&valid_gnu).unwrap();
        let mut null_compiler = valid_gnu_value.clone();
        null_compiler["compiler"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<ArosToolchainManifest>(null_compiler).is_err());
        let mut missing_compiler = valid_gnu_value.clone();
        missing_compiler.as_object_mut().unwrap().remove("compiler");
        assert!(serde_json::from_value::<ArosToolchainManifest>(missing_compiler).is_err());
        for llvm_version in [serde_json::Value::Null, serde_json::json!("17.0.6")] {
            let mut value = valid_gnu_value.clone();
            value["llvm_version"] = llvm_version;
            assert!(serde_json::from_value::<ArosToolchainManifest>(value).is_err());
        }

        let valid_gnu_lock = serde_json::to_value(lock_v2("lp64d")).unwrap();
        let mut null_compiler = valid_gnu_lock.clone();
        null_compiler["artifacts"][0]["compiler"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<ArosToolchainLock>(null_compiler).is_err());
        let mut missing_compiler = valid_gnu_lock.clone();
        missing_compiler["artifacts"][0]
            .as_object_mut()
            .unwrap()
            .remove("compiler");
        assert!(serde_json::from_value::<ArosToolchainLock>(missing_compiler).is_err());
        for llvm_version in [serde_json::Value::Null, serde_json::json!("17.0.6")] {
            let mut value = valid_gnu_lock.clone();
            value["artifacts"][0]["llvm_version"] = llvm_version;
            assert!(serde_json::from_value::<ArosToolchainLock>(value).is_err());
        }

        let mut mixed_manifest = manifest_v1();
        mixed_manifest.compiler = Some(gnu_identity("lp64d"));
        assert!(mixed_manifest.validate().is_err());
        let mut mixed_v2_manifest = manifest_v2("riscv64-unknown-aros", gnu_identity("lp64d"));
        mixed_v2_manifest.llvm_version = Some("16.2.0".into());
        assert!(mixed_v2_manifest.validate().is_err());
        let mut mixed_lock = ArosToolchainLock {
            schema: AROS_TOOLCHAIN_LOCK_SCHEMA,
            release_id: "release-v1".into(),
            base_url: Some("https://example.invalid/release".into()),
            artifacts: vec![artifact("linux-x86_64", "pc-x86_64")],
        };
        mixed_lock.artifacts[0].compiler = Some(gnu_identity("lp64d"));
        assert!(mixed_lock.validate().is_err());
        let mut mixed_v2_lock = lock_v2("lp64d");
        mixed_v2_lock.artifacts[0].llvm_version = Some("16.2.0".into());
        assert!(mixed_v2_lock.validate().is_err());
    }

    #[test]
    fn compiler_identity_rejects_duplicate_unknown_bad_versions_and_target_mismatch() {
        let valid = serde_json::to_value(gnu_identity("lp64d")).unwrap();
        let mut bad_gcc = valid.clone();
        bad_gcc["gcc_version"] = serde_json::json!("16.x.0");
        assert!(serde_json::from_value::<ArosCompilerIdentity>(bad_gcc).is_err());
        let mut unbounded_gcc = valid.clone();
        unbounded_gcc["gcc_version"] = serde_json::json!("2147483648.0.0");
        assert!(serde_json::from_value::<ArosCompilerIdentity>(unbounded_gcc).is_err());
        let mut unbounded_binutils = valid.clone();
        unbounded_binutils["binutils_version"] = serde_json::json!("2.47.0.0.0");
        assert!(serde_json::from_value::<ArosCompilerIdentity>(unbounded_binutils).is_err());
        let mut four_part_binutils = valid.clone();
        four_part_binutils["binutils_version"] = serde_json::json!("2.47.1.0");
        assert!(serde_json::from_value::<ArosCompilerIdentity>(four_part_binutils).is_ok());
        let mut unknown = valid.clone();
        unknown["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ArosCompilerIdentity>(unknown).is_err());

        let duplicate_family = serde_json::to_string(&valid).unwrap().replacen(
            "\"family\":\"gnu\"",
            "\"family\":\"gnu\",\"family\":\"llvm\"",
            1,
        );
        assert!(serde_json::from_str::<ArosCompilerIdentity>(&duplicate_family).is_err());

        let identity = gnu_identity("lp64d");
        identity
            .validate_for_target("riscv64-unknown-aros")
            .unwrap();
        for wrong in [
            "riscv-unknown-aros",
            "x86_64-unknown-aros",
            "riscv64-unknown-linux-gnu",
            "riscv64--unknown-aros",
        ] {
            assert!(
                identity.validate_for_target(wrong).is_err(),
                "accepted {wrong}"
            );
        }
        gnu_identity("ilp32")
            .validate_for_target("riscv-unknown-aros")
            .unwrap();
    }
}
