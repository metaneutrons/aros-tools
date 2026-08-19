use crate::arch::Architecture;
use crate::error::{ArosError, Result};
use std::path::PathBuf;
use tracing::info;

/// Toolchain detection and management.
#[derive(Debug, Clone)]
pub struct Toolchain {
    pub arch: Architecture,
    pub cc_path: PathBuf,
    pub cxx_path: PathBuf,
    pub ar_path: PathBuf,
    pub ranlib_path: PathBuf,
    pub ld_path: PathBuf,
    pub objcopy_path: PathBuf,
}

impl Toolchain {
    /// Detect or verify a cross-compiler for the given target architecture.
    pub fn detect(arch: Architecture) -> Result<Self> {
        let prefix = arch.triple_prefix();
        let cc_name = format!("{prefix}-gcc");
        let cxx_name = format!("{prefix}-g++");
        let ar_name = format!("{prefix}-ar");
        let ranlib_name = format!("{prefix}-ranlib");
        let ld_name = format!("{prefix}-ld");
        let objcopy_name = format!("{prefix}-objcopy");

        let cc_path = which::which(&cc_name).or_else(|_| {
            let fallback = PathBuf::from(format!("/opt/aros-toolchains/bin/{cc_name}"));
            if fallback.exists() {
                Ok(fallback)
            } else {
                Err(ArosError::ToolchainNotFound {
                    binary: cc_name.clone(),
                })
            }
        })?;

        info!(arch = %arch, cc = ?cc_path, "Toolchain detected");

        Ok(Self {
            arch,
            cc_path: cc_path.clone(),
            cxx_path: which::which(&cxx_name).unwrap_or_else(|_| cc_path.with_file_name(cxx_name)),
            ar_path: which::which(&ar_name).unwrap_or_else(|_| cc_path.with_file_name(ar_name)),
            ranlib_path: which::which(&ranlib_name)
                .unwrap_or_else(|_| cc_path.with_file_name(ranlib_name)),
            ld_path: which::which(&ld_name).unwrap_or_else(|_| cc_path.with_file_name(ld_name)),
            objcopy_path: which::which(&objcopy_name)
                .unwrap_or_else(|_| cc_path.with_file_name(objcopy_name)),
        })
    }
}
