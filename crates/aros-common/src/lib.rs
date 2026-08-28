//! Common abstractions, architectures, and data structures for AROS-NG tooling.

pub mod arch;
pub mod diagnostic;
pub mod digest;
pub mod elf;
pub mod error;
pub mod observability;
pub mod pins;
pub mod process;
pub mod target;
pub mod text;
pub mod toolchain;
pub mod toolchain_manifest;

pub use arch::Architecture;
pub use diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticSeverity,
    DiagnosticStage, SourceLocation,
};
pub use digest::{
    finish_sha256, sha256_bytes, sha256_file, sha256_reader, Sha256Digest, Sha256Result,
};
pub use error::{ArosError, Result};
pub use observability::{
    render_diagnostics, requested_diagnostic_format, DiagnosticFailure, DiagnosticFormat,
    LogFormat, LogLevel, Logger, ObservabilityPolicy,
};
pub use process::{
    bounded_output_detail, exit_signal, run_output, run_status, ProcessOutput, ProcessStatus,
};
pub use target::TargetProfile;
pub use text::read_source;
pub use toolchain::Toolchain;
pub use toolchain_manifest::{
    ArosToolchainArtifact, ArosToolchainLock, ArosToolchainManifest, ArosToolchainManifestEntry,
};
