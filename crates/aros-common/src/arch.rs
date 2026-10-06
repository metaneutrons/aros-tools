use serde::{Deserialize, Serialize};
use std::fmt;

/// Supported AROS CPU architectures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    X86_64,
    AArch64,
    Arm,
    M68k,
    Riscv32,
    Riscv64,
    I386,
    Ppc,
}

impl Architecture {
    /// CPU token used by AROS source directories and MetaMake selectors.
    /// AROS names its 32-bit RISC-V CPU `riscv`; the public architecture
    /// identifier retains the width as `riscv32`.
    #[must_use]
    pub fn source_cpu(&self) -> String {
        if *self == Self::Riscv32 {
            "riscv".to_owned()
        } else {
            self.to_string()
        }
    }

    #[must_use]
    pub const fn triple_prefix(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-aros",
            Self::AArch64 => "aarch64-aros",
            Self::Arm => "arm-aros",
            Self::M68k => "m68k-aros",
            Self::Riscv32 => "riscv32-aros",
            Self::Riscv64 => "riscv64-aros",
            Self::I386 => "i386-aros",
            Self::Ppc => "ppc-aros",
        }
    }

    #[must_use]
    pub const fn pointer_width(&self) -> usize {
        match self {
            Self::X86_64 | Self::AArch64 | Self::Riscv64 => 64,
            Self::Arm | Self::M68k | Self::Riscv32 | Self::I386 | Self::Ppc => 32,
        }
    }
}

impl fmt::Display for Architecture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::X86_64 => write!(f, "x86_64"),
            Self::AArch64 => write!(f, "aarch64"),
            Self::Arm => write!(f, "arm"),
            Self::M68k => write!(f, "m68k"),
            Self::Riscv32 => write!(f, "riscv32"),
            Self::Riscv64 => write!(f, "riscv64"),
            Self::I386 => write!(f, "i386"),
            Self::Ppc => write!(f, "ppc"),
        }
    }
}
