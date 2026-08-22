use crate::arch::Architecture;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Host compiler asset declaration per host platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct HostCompilerConfig {
    pub llvm_version: String,
    pub base_url: String,
    #[serde(default)]
    pub hosts: HashMap<String, HostCompilerAssetConfig>,
}

impl Default for HostCompilerConfig {
    fn default() -> Self {
        let mut hosts = HashMap::new();
        hosts.insert(
            "macos-aarch64".to_string(),
            HostCompilerAssetConfig {
                asset: "clang+llvm-{version}-arm64-apple-macos11.tar.xz".to_string(),
                sha256: None,
            },
        );
        hosts.insert(
            "macos-x86_64".to_string(),
            HostCompilerAssetConfig {
                asset: "clang+llvm-{version}-x86_64-apple-darwin.tar.xz".to_string(),
                sha256: None,
            },
        );
        hosts.insert(
            "linux-x86_64".to_string(),
            HostCompilerAssetConfig {
                asset: "clang+llvm-{version}-x86_64-linux-gnu-ubuntu-18.04.tar.xz".to_string(),
                sha256: None,
            },
        );
        hosts.insert(
            "linux-aarch64".to_string(),
            HostCompilerAssetConfig {
                asset: "clang+llvm-{version}-aarch64-linux-gnu.tar.xz".to_string(),
                sha256: None,
            },
        );

        Self {
            llvm_version: "18.1.8".to_string(),
            base_url: "https://github.com/llvm/llvm-project/releases/download/llvmorg-{version}"
                .to_string(),
            hosts,
        }
    }
}

/// Root structure of `aros-targets.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArosConfig {
    #[serde(default, alias = "toolchain")]
    pub host_compiler: Option<HostCompilerConfig>,
    #[serde(default)]
    pub targets: Vec<TargetProfile>,
}

/// Target Profile representing a specific hardware board or configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Load targets from configuration file.
    pub fn load_from_file(path: &Path) -> Result<Vec<Self>> {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let config: ArosConfig = toml::from_str(&content).map_err(|e| {
                crate::error::ArosError::TranspilerSyntax {
                    file: path.display().to_string(),
                    message: e.to_string(),
                }
            })?;
            if config.targets.is_empty() {
                Ok(Self::default_profiles())
            } else {
                Ok(config.targets)
            }
        } else {
            Ok(Self::default_profiles())
        }
    }

    /// Load full ArosConfig from file.
    pub fn load_config(path: &Path) -> Result<ArosConfig> {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let config: ArosConfig = toml::from_str(&content).map_err(|e| {
                crate::error::ArosError::TranspilerSyntax {
                    file: path.display().to_string(),
                    message: e.to_string(),
                }
            })?;
            Ok(config)
        } else {
            Ok(ArosConfig {
                host_compiler: Some(HostCompilerConfig::default()),
                targets: Self::default_profiles(),
            })
        }
    }

    /// Default baseline target profiles (pc-x86_64, rpi-aarch64, arm-raspi).
    #[must_use]
    pub fn default_profiles() -> Vec<Self> {
        vec![
            Self {
                name: "pc-x86_64".into(),
                arch: Architecture::X86_64,
                platform: "pc".into(),
                bsp: "generic".into(),
                features: vec!["smp".into(), "acpi".into(), "hdaudio".into(), "ahci".into()],
                float_abi: None,
            },
            Self {
                name: "rpi-aarch64".into(),
                arch: Architecture::AArch64,
                platform: "raspi".into(),
                bsp: "bcm2711-bcm2712".into(),
                features: vec![
                    "smp".into(),
                    "genet".into(),
                    "rp1".into(),
                    "i2s".into(),
                    "hdmi-audio".into(),
                ],
                float_abi: None,
            },
            Self {
                name: "arm-raspi".into(),
                arch: Architecture::Arm,
                platform: "raspi".into(),
                bsp: "bcm2835".into(),
                features: vec!["pwm-audio".into(), "sdhost".into()],
                float_abi: Some("hard".into()),
            },
        ]
    }
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
}
