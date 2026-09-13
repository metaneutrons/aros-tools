//! Typed cache discovery, lifecycle and compiler-cache selection for AROS tools.
//!
//! This crate deliberately owns no cache mutation, downloader, producer, or
//! backend-process behaviour. It provides the stable observations and
//! selection vocabulary that cache-owning crates and the CLI share.

mod compiler;
mod compiler_lifecycle;
mod lifecycle;
mod roots;
mod status;

pub use compiler::{
    observe_compiler_backend, observe_compiler_backends, resolve_compiler_cache,
    resolve_compiler_cache_for_build, CompilerBackend, CompilerBackendChoice,
    CompilerBackendObservation, CompilerBackendState, CompilerCacheResolutionError,
    CompilerCacheSelection, CompilerConfigurationScope,
};
pub use compiler_lifecycle::{
    acquire_compiler_cache_build, begin_compiler_cache_mutation, compiler_cache_environment,
    load_managed_compiler_cache, prepare_managed_compiler_cache, preview_compiler_cache_clear,
    preview_compiler_cache_reset_stats, resolve_managed_compiler_cache_for_build,
    CompilerCacheBuild, CompilerCacheBuildLease, CompilerCacheBuildSelection,
    CompilerCacheDataScope, CompilerCacheEnvironment, CompilerCacheLifecycleError,
    CompilerCacheManagedRoot, CompilerCacheMutation, CompilerCacheMutationOperation,
    CompilerCacheMutationPreview, CompilerCacheMutationRequest, CompilerCacheMutationResult,
    CompilerCachePreparation,
};
pub use lifecycle::{
    acquire_read_lease, acquire_read_leases, acquire_write_lease, acquire_write_leases,
    apply_removal, apply_retention_release, keep, keep_many, keep_many_validated, keep_validated,
    preview_removal, preview_retention_release, CacheLifecycleError, CacheObjectKind,
    CacheObjectLease, CacheObjectLeases, CacheObjectProof, CacheObjectRequest, CacheRemovalBlocker,
    CacheRemovalPreview, CacheRemovalResult, CacheRetainedObject, CacheRetentionRecord,
    CacheRetentionRelease, CacheRetentionReleasePreview,
};
pub use roots::{
    archive_cache_root, aros_home, observe_root, resolve_archive_cache_root, resolve_aros_home,
    resolve_default_compiler_cache_root, resolve_explicit_root, CacheEnvironment, CacheRoot,
    RootObservation, RootOrigin, RootResolutionError, RootState,
};
pub use status::{
    cache_status, compiler_cache_status, CacheCapability, CacheFamily, CacheFamilyStatus,
    CacheFamilyStatusKind, CacheSideEffects, CacheStatus, CompilerCacheStatus, CACHE_STATUS_SCHEMA,
    COMPILER_CACHE_STATUS_SCHEMA,
};
