//! Fixtures the crate's tests share.
//!
//! They lived in `parser.rs`'s test module, which is private, so a test could
//! not move out of that file without leaving them behind. Splitting the parser
//! means moving tests next to the code they cover, and that needs the fixtures
//! reachable from anywhere in the crate.

use crate::dirs::DirVars;
use crate::parser::TargetContext;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// A directory that removes itself, for tests that need a tree on disk.
pub struct TempTree(pub PathBuf);

impl TempTree {
    /// # Panics
    ///
    /// Panics when the process cannot create its uniquely named temporary
    /// test directory.
    #[must_use]
    pub fn new() -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "aros-parser-include-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Default for TempTree {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The repository root, from this crate's manifest.
#[must_use]
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// A concrete target profile, the way CMake derives one for a preset.
#[must_use]
pub fn target_context(cpu: &str, platform: &str, float_abi: &str) -> TargetContext {
    TargetContext {
        cpu: Some(cpu.to_owned()),
        platform: Some(platform.to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(if cpu == "x86_64" { "i386" } else { "" }.to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(float_abi.to_owned()),
    }
}

/// The directory variables of the real tree.
#[must_use]
pub fn dirs() -> DirVars {
    DirVars::load(&root())
}
