use crate::arch::Architecture;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Target Profile representing a specific hardware board or configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetProfile {
    pub name: String,
    pub arch: Architecture,
    pub platform: String,
    pub bsp: String,
    #[serde(default)]
    pub features: Vec<String>,
}

impl TargetProfile {
    /// Load target profiles dynamically from a declarative configuration file (e.g. `aros-targets.toml`).
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or parsed.
    pub fn load_from_file(path: &Path) -> Result<Vec<Self>> {
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let targets: Vec<Self> = toml::from_str(&content).map_err(|e| {
                crate::error::ArosError::TranspilerSyntax {
                    file: path.display().to_string(),
                    message: e.to_string(),
                }
            })?;
            Ok(targets)
        } else {
            Ok(Self::default_profiles())
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
            },
            Self {
                name: "arm-raspi".into(),
                arch: Architecture::Arm,
                platform: "raspi".into(),
                bsp: "bcm2835".into(),
                features: vec!["pwm-audio".into(), "sdhost".into()],
            },
        ]
    }
}
