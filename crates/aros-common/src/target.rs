use crate::arch::Architecture;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Host compiler asset declaration per host platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCompilerAssetConfig {
    pub asset: String,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Declarative host compiler configuration loaded from `aros-targets.toml`.
///
/// This is deliberately separate from the AROS cross-toolchain release lock.
/// The host compiler can bootstrap builds, but it does not contain the AROS
/// target runtimes or C++ standard library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCompilerConfig {
    pub llvm_version: String,
    pub base_url: String,
    #[serde(default)]
    pub hosts: HashMap<String, HostCompilerAssetConfig>,
}

/// Root structure of `aros-targets.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArosConfig {
    #[serde(default, alias = "toolchain")]
    pub host_compiler: Option<HostCompilerConfig>,
    #[serde(default)]
    pub targets: Vec<TargetProfile>,
}

/// Target Profile representing a specific hardware board or configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetProfile {
    pub name: String,
    pub arch: Architecture,
    pub platform: String,
    pub bsp: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub float_abi: Option<String>,
}

impl TargetProfile {
    /// Load targets from the authoritative configuration file.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, parsed, or validated.
    pub fn load_from_file(path: &Path) -> Result<Vec<Self>> {
        Ok(Self::load_config(path)?.targets)
    }

    /// Load the full authoritative configuration, failing on missing or empty data.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, parsed, or validated.
    pub fn load_config(path: &Path) -> Result<ArosConfig> {
        let content =
            fs::read_to_string(path).map_err(|error| crate::error::ArosError::Configuration {
                file: path.display().to_string(),
                message: format!("cannot read configuration: {error}"),
            })?;
        let config: ArosConfig =
            toml::from_str(&content).map_err(|error| crate::error::ArosError::Configuration {
                file: path.display().to_string(),
                message: error.to_string(),
            })?;
        validate_config(path, &config)?;
        Ok(config)
    }
}

fn validate_config(path: &Path, config: &ArosConfig) -> Result<()> {
    let invalid = |message: String| crate::error::ArosError::Configuration {
        file: path.display().to_string(),
        message,
    };
    if config.targets.is_empty() {
        return Err(invalid("the required `targets` array is empty".to_string()));
    }
    let mut names = HashSet::new();
    for (index, target) in config.targets.iter().enumerate() {
        if !safe_token(&target.name) {
            return Err(invalid(format!(
                "targets[{index}].name must be a non-empty portable token"
            )));
        }
        if !names.insert(target.name.as_str()) {
            return Err(invalid(format!(
                "target name {:?} is declared more than once",
                target.name
            )));
        }
        for (field, value) in [("platform", &target.platform), ("bsp", &target.bsp)] {
            if !safe_token(value) {
                return Err(invalid(format!(
                    "target {:?} has an invalid {field} token",
                    target.name
                )));
            }
        }
        let mut features = HashSet::new();
        for feature in &target.features {
            if !safe_token(feature) || !features.insert(feature.as_str()) {
                return Err(invalid(format!(
                    "target {:?} has an invalid or duplicate feature {feature:?}",
                    target.name
                )));
            }
        }
        if target
            .float_abi
            .as_deref()
            .is_some_and(|value| !safe_token(value))
        {
            return Err(invalid(format!(
                "target {:?} has an invalid float_abi token",
                target.name
            )));
        }
    }
    if let Some(host) = &config.host_compiler {
        if host.llvm_version.trim().is_empty() || host.base_url.trim().is_empty() {
            return Err(invalid(
                "host_compiler requires non-empty llvm_version and base_url".to_string(),
            ));
        }
        for (key, asset) in &host.hosts {
            if !safe_token(key) || asset.asset.trim().is_empty() {
                return Err(invalid(format!(
                    "host_compiler host {key:?} has an invalid key or empty asset"
                )));
            }
            if asset
                .sha256
                .as_deref()
                .is_some_and(|value| crate::Sha256Digest::parse(value).is_err())
            {
                return Err(invalid(format!(
                    "host_compiler host {key:?} has an invalid SHA-256 digest"
                )));
            }
        }
    }
    Ok(())
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_legacy_toolchain_table_as_host_compiler() {
        let config: ArosConfig = toml::from_str(
            r#"
                [toolchain]
                llvm_version = "18.1.8"
                base_url = "https://example.invalid/llvm"

                [toolchain.hosts.linux-x86_64]
                asset = "llvm.tar.xz"
            "#,
        )
        .unwrap();

        let host_compiler = config.host_compiler.unwrap();
        assert_eq!(host_compiler.llvm_version, "18.1.8");
        assert_eq!(host_compiler.hosts["linux-x86_64"].asset, "llvm.tar.xz");
    }

    #[test]
    fn missing_configuration_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let error =
            TargetProfile::load_from_file(&directory.path().join("missing.toml")).unwrap_err();
        assert!(matches!(error, crate::ArosError::Configuration { .. }));
    }

    #[test]
    fn empty_target_list_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("aros-targets.toml");
        fs::write(&path, "[host_compiler]\nllvm_version='18'\nbase_url='x'\n").unwrap();
        let error = TargetProfile::load_from_file(&path).unwrap_err();
        assert!(matches!(error, crate::ArosError::Configuration { .. }));
    }

    #[test]
    fn duplicate_targets_and_unknown_fields_fail_closed() {
        for content in [
            "[[targets]]\nname='same'\narch='arm'\nplatform='raspi'\nbsp='one'\n\
             [[targets]]\nname='same'\narch='arm'\nplatform='raspi'\nbsp='two'\n",
            "[[targets]]\nname='arm'\narch='arm'\nplatform='raspi'\nbsp='one'\ntypo=true\n",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("aros-targets.toml");
            fs::write(&path, content).unwrap();
            assert!(matches!(
                TargetProfile::load_from_file(&path),
                Err(crate::ArosError::Configuration { .. })
            ));
        }
    }
}
