//! Common abstractions, architectures, and data structures for AROS-NG tooling.

pub mod arch;
pub mod diagnostic;
pub mod elf;
pub mod error;
pub mod pins;
pub mod target;
pub mod text;
pub mod toolchain;
pub mod toolchain_manifest;

pub use arch::Architecture;
pub use diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticSet, DiagnosticSeverity, DiagnosticStage, SourceLocation,
};
pub use error::{ArosError, Result};
pub use target::TargetProfile;
pub use text::read_source;
pub use toolchain::Toolchain;
pub use toolchain_manifest::{
    ArosToolchainArtifact, ArosToolchainLock, ArosToolchainManifest, ArosToolchainManifestEntry,
};
