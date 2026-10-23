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
pub mod board_registry;
pub mod captured_programs;
#[cfg(all(test, unix))]
mod captured_programs_tests;
pub mod diagnostic;
pub mod digest;
pub mod elf;
pub mod error;
pub mod esp_partition;
pub mod flash_plan;
pub mod local_source;
pub mod local_toolchain;
pub mod make_child_command;
pub mod make_environment_prefix;
pub mod make_initial_values;
#[cfg(test)]
mod make_input_output_transition_tests;
pub mod make_input_snapshot;
#[cfg(test)]
mod make_input_snapshot_tests;
pub mod make_namespace;
#[cfg(unix)]
pub mod make_native_collection;
pub mod make_native_invocation;
pub mod make_native_observation;
#[cfg(unix)]
pub mod make_native_sampler;
pub mod make_observer;
pub mod make_scope_snapshot;
pub mod make_startup_arguments;
pub mod media_input_lock;
pub mod media_plan;
pub mod media_profile;
pub mod media_receipt;
pub mod media_tree;
pub mod mesa_configuration;
pub mod mesa_configuration_input;
pub mod mesa_genmodule_collection;
pub mod mesa_genmodule_inventory;
pub mod mesa_genmodule_make;
pub mod mesa_native_make;
pub mod mesa_source;
pub mod native_build_contract;
pub mod native_consumer_contract;
pub(crate) mod native_consumer_graph;
pub mod native_host_generator;
pub mod native_make_template;
pub mod native_media;
pub mod observability;
pub mod pins;
pub mod process;
pub mod publication;
pub mod sdk_companion_role;
pub mod sdk_consumer_contract;
pub mod sdk_manifest;
pub mod sdk_release_lock;
pub mod target;
pub mod text;
pub mod toolchain;
pub mod toolchain_inventory;
pub mod toolchain_layout;
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
    effective_log_level, emit_stdout, render_diagnostics, requested_diagnostic_format,
    take_stdout_failure_diagnostic, write_stdout, DiagnosticFailure, DiagnosticFormat, LogFormat,
    LogLevel, Logger, ObservabilityPolicy,
};
pub use process::{
    bounded_output_detail, exit_signal, run_output, run_output_with_control, run_output_with_input,
    run_output_with_input_and_control, run_output_with_limit, run_output_with_timeout, run_status,
    run_status_with_timeout, CancellationToken, CapturedStream, ProcessOutput, ProcessStatus,
    TimedProcessStatus, DEFAULT_CAPTURE_LIMIT,
};
pub use publication::{
    canonical_source_file, casefold_path_key, copy_tree_from_snapshot_nofollow,
    create_unique_directory_nofollow, directory_entry_names_nofollow_bounded,
    ensure_directory_nofollow, exchange_prepared_source_tree_if_unchanged, exchange_prepared_tree,
    exchange_prepared_tree_if_unchanged, is_publication_journal_lock_name, is_rollback_incomplete,
    measure_regular_file, measure_regular_file_bounded, measure_regular_file_digest_bounded,
    measure_tree_content_cas, measure_tree_content_cas_bounded, open_regular_file_nofollow,
    payload_casefold_path_key, probe_advisory_file_lock, publication_failure_class,
    publication_journal_lock_path, publication_journal_path, publish_atomic_file,
    publish_flat_tree_noclobber, publish_prepared_source_tree_noclobber,
    publish_prepared_tree_noclobber, remove_regular_file_from_snapshot_nofollow,
    remove_tree_from_snapshot_nofollow, validate_existing_directory_prefix_nofollow,
    validate_private_directory_nofollow, AdvisoryFileLock, AdvisoryLockMode,
    AdvisoryLockObservation, AdvisoryLockState, AtomicFilePolicy, DurableFileSet, FileIdentity,
    PortableOutputName, PublicationError, PublicationFailureClass, PublicationReceipt,
    RecoveryOutcome, TreeContentCas, TreeContentEntryKind, TreeTraversalLimits,
};
pub use sdk_companion_role::{SdkCompanionRole, MAX_SDK_COMPANION_ROLES};
pub use target::{BootstrapAbiProfile, TargetProfile, TranspilerProfile};
pub use text::{decode_source_bytes, read_source};
pub use toolchain::Toolchain;
pub use toolchain_inventory::{
    normalized_toolchain_file_mode, toolchain_inventory_sha256, toolchain_tree_inventory,
    toolchain_tree_inventory_excluding, ToolchainInventoryError,
};
pub use toolchain_manifest::{
    parse_credential_free_https_url, validate_gnu_compiler_versions, ArosCompilerIdentity,
    ArosToolchainArtifact, ArosToolchainLock, ArosToolchainManifest, ArosToolchainManifestEntry,
    AROS_TOOLCHAIN_LOCK_SCHEMA, AROS_TOOLCHAIN_LOCK_SCHEMA_V2, AROS_TOOLCHAIN_MANIFEST_FILE,
    AROS_TOOLCHAIN_MANIFEST_SCHEMA, AROS_TOOLCHAIN_MANIFEST_SCHEMA_V2,
};
