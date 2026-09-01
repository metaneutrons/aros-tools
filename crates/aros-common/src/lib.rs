//! Common abstractions, architectures, and data structures for AROS tooling.

/// Emit formatted user-facing output through the shared non-panicking stdout
/// contract.
#[macro_export]
macro_rules! output {
    ($($argument:tt)*) => {{
        $crate::emit_stdout(format_args!($($argument)*), false);
    }};
}

/// Emit one user-facing output line through the shared non-panicking stdout
/// contract.
#[macro_export]
macro_rules! outputln {
    () => {{
        $crate::emit_stdout(format_args!(""), true);
    }};
    ($($argument:tt)*) => {{
        $crate::emit_stdout(format_args!($($argument)*), true);
    }};
}

pub mod arch;
pub mod diagnostic;
pub mod digest;
pub mod elf;
pub mod error;
pub mod observability;
pub mod pins;
pub mod process;
pub mod publication;
pub mod target;
pub mod text;
pub mod toolchain;
pub mod toolchain_manifest;

pub use arch::Architecture;
pub use diagnostic::{
    CommitState, Diagnostic, DiagnosticCode, DiagnosticContext, DiagnosticSet, DiagnosticSeverity,
    DiagnosticStage, SourceLocation,
};
pub use digest::{
    finish_sha256, sha256_bytes, sha256_file, sha256_reader, Sha256Digest, Sha256Result,
};
pub use error::{ArosError, Result};
pub use observability::{
    emit_stdout, render_diagnostics, requested_diagnostic_format, take_stdout_failure_diagnostic,
    write_stdout, DiagnosticFailure, DiagnosticFormat, LogFormat, LogLevel, Logger,
    ObservabilityPolicy,
};
pub use process::{
    bounded_output_detail, exit_signal, run_output, run_output_with_input, run_output_with_limit,
    run_output_with_timeout, run_status, run_status_with_timeout, CapturedStream, ProcessOutput,
    ProcessStatus, TimedProcessStatus, DEFAULT_CAPTURE_LIMIT,
};
pub use publication::{
    canonical_source_file, casefold_path_key, exchange_prepared_tree,
    exchange_prepared_tree_if_unchanged, is_rollback_incomplete, measure_regular_file,
    measure_tree_content_cas, publication_failure_class, publication_journal_path,
    publish_atomic_file, publish_flat_tree_noclobber, publish_prepared_source_tree_noclobber,
    publish_prepared_tree_noclobber, AtomicFilePolicy, DurableFileSet, FileIdentity,
    PortableOutputName, PublicationError, PublicationFailureClass, PublicationReceipt,
    RecoveryOutcome, TreeContentCas,
};
pub use target::{TargetProfile, TranspilerProfile};
pub use text::read_source;
pub use toolchain::Toolchain;
pub use toolchain_manifest::{
    parse_credential_free_https_url, ArosToolchainArtifact, ArosToolchainLock,
    ArosToolchainManifest, ArosToolchainManifestEntry,
};
