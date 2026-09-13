//! Typed, passive cache discovery and compiler-cache selection for AROS tools.
//!
//! This crate deliberately owns no cache mutation, downloader, producer, or
//! backend-process behaviour. It provides the stable observations and
//! selection vocabulary that cache-owning crates and the CLI share.

mod compiler;
mod roots;
mod status;

pub use compiler::{
    observe_compiler_backend, observe_compiler_backends, resolve_compiler_cache, CompilerBackend,
    CompilerBackendChoice, CompilerBackendObservation, CompilerBackendState,
    CompilerCacheSelection, CompilerConfigurationScope,
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
