use crate::arch::Architecture;
use serde::{Deserialize, Serialize};

/// Target Profile representing a specific hardware board or configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetProfile {
    pub name: String,
    pub arch: Architecture,
    pub platform: String,
    pub bsp: String,
    pub features: Vec<String>,
}

impl TargetProfile {
    #[must_use]
    pub fn pc_x86_64() -> Self {
        Self {
            name: "pc-x86_64".into(),
            arch: Architecture::X86_64,
            platform: "pc".into(),
            bsp: "generic".into(),
            features: vec!["smp".into(), "acpi".into(), "hdaudio".into(), "ahci".into()],
        }
    }

    #[must_use]
    pub fn rpi_aarch64() -> Self {
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
        }
    }

    #[must_use]
    pub fn arm_raspi() -> Self {
        Self {
            name: "arm-raspi".into(),
            arch: Architecture::Arm,
            platform: "raspi".into(),
            bsp: "bcm2835".into(),
            features: vec!["pwm-audio".into(), "sdhost".into()],
        }
    }

    #[must_use]
    pub fn esp32p4_riscv32() -> Self {
        Self {
            name: "esp32p4-riscv32".into(),
            arch: Architecture::Riscv32,
            platform: "esp32p4".into(),
            bsp: "seeed-d1001".into(),
            features: vec!["smp".into(), "tcm-sram".into(), "ppa".into(), "sdio".into()],
        }
    }
}
