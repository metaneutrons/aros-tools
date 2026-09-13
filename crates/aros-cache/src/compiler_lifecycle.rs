//! Explicit ownership and process environment for local compiler caches.
//!
//! Compiler-cache programs can otherwise inherit remote endpoints, foreign
//! storage roots, or daemon sockets from their caller. This module separates
//! passive discovery from an opt-in managed namespace: a build uses a cache
//! only after that namespace was prepared, ownership was revalidated, and a
//! shared lifecycle lease was acquired for its complete duration.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aros_common::{
    directory_entry_names_nofollow_bounded, ensure_directory_nofollow,
    is_publication_journal_lock_name, measure_regular_file_bounded,
    measure_tree_content_cas_bounded, publish_atomic_file, remove_tree_from_snapshot_nofollow,
    sha256_bytes, validate_private_directory_nofollow, AdvisoryFileLock, AtomicFilePolicy,
    FileIdentity, Sha256Digest, TreeContentCas, TreeTraversalLimits,
};
use rustix::fs::{self as rfs, AtFlags, Mode, OFlags};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    resolve_default_compiler_cache_root, resolve_explicit_root, CacheEnvironment, CompilerBackend,
    CompilerBackendChoice, CompilerCacheResolutionError, CompilerCacheSelection,
    RootResolutionError,
};

const OWNERSHIP_SCHEMA: &str = "aros-managed-compiler-cache-v1";
const CONTROL_DIRECTORY: &str = ".aros-compiler-cache";
const CONTROL_VERSION: &str = "v1";
const OWNERSHIP_FILE: &str = "ownership.json";
const LOCK_DIRECTORY: &str = "locks";
const BUILD_LOCK: &str = "build.lock";
const CONFIGURATION_FILE: &str = "configuration.toml";
const MAX_OWNERSHIP_BYTES: u64 = 16 * 1024;
const MAX_ROOT_ENTRIES_DURING_PREPARE: usize = 1;
// control directory, generated configuration, data directory, one durable
// publication lock, and the optional sccache Unix-domain socket.
const MAX_MANAGED_ROOT_ENTRIES: usize = 5;
const MUTATION_TOKEN_SCHEMA: &str = "aros-compiler-cache-mutation-v1";
const PREVIEW_LIFETIME_SECONDS: u64 = 5 * 60;
const MAX_COMPILER_CACHE_ENTRIES: usize = 200_000;
const MAX_COMPILER_CACHE_BYTES: u64 = 6 * 1024 * 1024 * 1024;
const SCCACHE_SERVER_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const SCCACHE_SERVER_STOP_POLL_INTERVAL: Duration = Duration::from_millis(25);
// Darwin's sockaddr_un reserves only 104 bytes for sun_path, including the
// trailing NUL. Linux permits a few more, but one cross-host managed layout
// must reject paths that cannot work on the stricter supported host.
const MAX_SCCACHE_SOCKET_PATH_BYTES: usize = 103;

/// Failure while preparing, validating, or leasing a managed compiler cache.
#[derive(Debug, Error)]
pub enum CompilerCacheLifecycleError {
    /// A caller supplied a relative or otherwise invalid cache root.
    #[error("invalid compiler-cache root: {0}")]
    Root(#[from] RootResolutionError),
    /// The requested managed root does not exist yet.
    #[error(
        "compiler-cache root '{path}' is not prepared for '{backend}'; run `aros cache compiler prepare --backend {backend} --dir {path}` first"
    )]
    MissingPreparedRoot {
        /// Selected backend.
        backend: &'static str,
        /// Absolute root requiring preparation.
        path: PathBuf,
    },
    /// A root contains state that has not been explicitly claimed by AROS.
    #[error(
        "refusing to claim non-empty compiler-cache root '{path}'; choose an empty private directory so AROS never adopts foreign cache state"
    )]
    NonEmptyUnownedRoot {
        /// Root containing foreign or interrupted state.
        path: PathBuf,
    },
    /// An ownership marker is absent, malformed, or bound to another root.
    #[error(
        "compiler-cache root '{path}' is not a valid AROS-managed '{backend}' namespace: {reason}"
    )]
    InvalidOwnership {
        /// Expected backend.
        backend: &'static str,
        /// Root being validated.
        path: PathBuf,
        /// Exact validation boundary.
        reason: String,
    },
    /// An I/O primitive rejected a no-follow ownership operation.
    #[error("cannot {action} at '{path}': {source}")]
    Io {
        /// Operation boundary.
        action: &'static str,
        /// Exact path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A persisted document could not be decoded or encoded safely.
    #[error("compiler-cache ownership metadata is invalid: {0}")]
    Metadata(String),
    /// The requested launcher was not available.
    #[error(transparent)]
    Resolution(#[from] CompilerCacheResolutionError),
    /// The request cannot unambiguously select one managed backend namespace.
    #[error("invalid managed compiler-cache selection: {0}")]
    Selection(String),
    /// A preview token is malformed, expired, or does not match current state.
    #[error("compiler-cache mutation preview token is invalid: {0}")]
    Token(String),
    /// The system clock could not establish a preview expiry boundary.
    #[error("cannot establish compiler-cache preview expiry: {0}")]
    Clock(String),
}

impl CompilerCacheLifecycleError {
    fn io(action: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }

    fn ownership(backend: CompilerBackend, path: &Path, reason: impl Into<String>) -> Self {
        Self::InvalidOwnership {
            backend: backend.program(),
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }
}

/// Durable report emitted after a root is safely prepared or revalidated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompilerCachePreparation {
    /// Versioned result schema.
    pub schema: &'static str,
    /// Operation performed by the caller.
    pub operation: &'static str,
    /// Selected local backend.
    pub backend: CompilerBackend,
    /// Exact absolute managed root.
    pub cache_root: PathBuf,
    /// Exact data directory owned by the selected backend.
    pub data_root: PathBuf,
    /// Whether this call claimed an empty root.
    pub created: bool,
    /// Ownership guarantee established by this operation.
    pub ownership: &'static str,
}

/// A verified AROS-managed namespace for exactly one compiler-cache backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerCacheManagedRoot {
    backend: CompilerBackend,
    root: PathBuf,
    data_root: PathBuf,
    configuration_path: PathBuf,
}

impl CompilerCacheManagedRoot {
    /// Backend bound to this namespace.
    #[must_use]
    pub const fn backend(&self) -> CompilerBackend {
        self.backend
    }

    /// Absolute namespace root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute local data root configured for the selected backend.
    #[must_use]
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// Absolute generated configuration path.
    #[must_use]
    pub fn configuration_path(&self) -> &Path {
        &self.configuration_path
    }

    /// Resolve the exact launcher executable after ownership was validated.
    ///
    /// # Errors
    ///
    /// Returns an actionable error when the selected backend executable is not
    /// currently available on `PATH`.
    pub fn executable(&self) -> Result<PathBuf, CompilerCacheLifecycleError> {
        which::which(self.backend.program()).map_err(|_| {
            CompilerCacheLifecycleError::Resolution(CompilerCacheResolutionError::Unavailable {
                backend: self.backend.program(),
            })
        })
    }

    fn build_lock_path(&self) -> PathBuf {
        control_root(&self.root)
            .join(LOCK_DIRECTORY)
            .join(BUILD_LOCK)
    }
}

/// A controlled environment passed to CMake and every compiler it starts.
///
/// It removes every recognized user compiler-cache variable first, then adds
/// the small backend-specific local-only configuration below an owned root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerCacheEnvironment {
    values: BTreeMap<&'static str, String>,
}

impl CompilerCacheEnvironment {
    /// Apply the controlled cache environment to a child command.
    pub fn apply_to(&self, command: &mut std::process::Command) {
        // A fixed `env_remove` list is unsafe: a later backend release could
        // add an unlisted remote-setting variable. Preserve every unrelated
        // host value, then add only the generated compiler-cache settings.
        command.env_clear();
        command.envs(std::env::vars_os().filter(|(name, _)| {
            let name = name.to_string_lossy();
            !name.starts_with("SCCACHE_") && !name.starts_with("CCACHE_")
        }));
        command.envs(&self.values);
    }

    /// Stable values, intended for diagnostics and integration tests only.
    #[must_use]
    pub const fn values(&self) -> &BTreeMap<&'static str, String> {
        &self.values
    }
}

/// Shared build lease for a verified compiler-cache namespace.
#[derive(Debug)]
pub struct CompilerCacheBuildLease {
    _lease: AdvisoryFileLock,
}

/// Explicit operation selected for one managed compiler-cache mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompilerCacheMutationOperation {
    /// Reset backend counters without deleting cached compiler outputs.
    ResetStats,
    /// Clear cached compiler outputs from one owned local namespace.
    Clear,
}

impl CompilerCacheMutationOperation {
    const fn operation_name(self) -> &'static str {
        match self {
            Self::ResetStats => "compiler.reset_stats",
            Self::Clear => "compiler.clear",
        }
    }

    const fn token_name(self) -> &'static str {
        match self {
            Self::ResetStats => "reset_stats",
            Self::Clear => "clear",
        }
    }
}

/// One exact managed namespace selected for a compiler-cache mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerCacheMutationRequest {
    /// Backend that owns the namespace.
    pub backend: CompilerBackend,
    /// Exact absolute AROS-managed namespace root.
    pub cache_root: PathBuf,
}

/// Public bounded scope of a measured compiler-cache data tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompilerCacheDataScope {
    /// Number of non-root entries below the managed data root.
    pub entry_count: usize,
    /// Total regular-file bytes below the managed data root.
    pub regular_file_bytes: u64,
    /// Stable payload digest of the measured data tree.
    pub payload_sha256: Sha256Digest,
    /// Snapshot digest binding identities and timestamps for preview/apply.
    pub snapshot_sha256: Sha256Digest,
    /// Explicit maximum entry count applied during measurement.
    pub max_entries: usize,
    /// Explicit maximum regular-file bytes applied during measurement.
    pub max_regular_file_bytes: u64,
}

/// Non-mutating preview for one token-confirmed compiler-cache mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompilerCacheMutationPreview {
    /// Versioned result schema.
    pub schema: &'static str,
    /// Planned operation.
    pub operation: &'static str,
    /// Backend bound to the managed namespace.
    pub backend: CompilerBackend,
    /// Exact absolute managed root.
    pub cache_root: PathBuf,
    /// Exact local data root affected by clear operations.
    pub data_root: PathBuf,
    /// Measured data scope for `clear`; counter reset intentionally omits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_scope: Option<CompilerCacheDataScope>,
    /// Unix timestamp after which the token cannot be applied.
    pub expires_unix_seconds: u64,
    /// Exact short-lived token required by the apply operation.
    pub apply_token: String,
    /// Explicit recovery boundary for this operation.
    pub recovery: &'static str,
}

/// Successful completion of a token-confirmed compiler-cache mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompilerCacheMutationResult {
    /// Versioned result schema.
    pub schema: &'static str,
    /// Completed operation.
    pub operation: &'static str,
    /// Backend that performed the operation.
    pub backend: CompilerBackend,
    /// Exact managed root that was locked for the operation.
    pub cache_root: PathBuf,
    /// Exact local data root that was selected.
    pub data_root: PathBuf,
    /// Deterministic outcome label.
    pub outcome: &'static str,
}

/// An exclusive token-confirmed compiler-cache mutation guard.
///
/// Hold this guard while invoking the backend. It serializes with every AROS
/// build using the same managed root. For sccache `clear`, call
/// [`Self::clear_sccache_data`] only after stopping the private server through
/// the controlled environment.
#[derive(Debug)]
pub struct CompilerCacheMutation {
    root: CompilerCacheManagedRoot,
    operation: CompilerCacheMutationOperation,
    data_snapshot: Option<TreeContentCas>,
    data_limits: TreeTraversalLimits,
    lease: AdvisoryFileLock,
}

impl CompilerCacheMutation {
    /// Managed root locked exclusively for the operation.
    #[must_use]
    pub const fn root(&self) -> &CompilerCacheManagedRoot {
        &self.root
    }

    /// Controlled local-only environment for the selected backend command.
    ///
    /// # Errors
    ///
    /// Returns an error if the root cannot be represented safely in the child
    /// environment.
    pub fn environment(&self) -> Result<CompilerCacheEnvironment, CompilerCacheLifecycleError> {
        compiler_cache_environment(&self.root)
    }

    /// Descriptor-remove and recreate the owned sccache data tree.
    ///
    /// This is deliberately unavailable for ccache: ccache clearing must use
    /// its own bounded command under this same exclusive guard. The snapshot
    /// from the preview is revalidated by the removal primitive, so any changed
    /// object, root swap, traversal, special file, or budget overrun fails
    /// closed without recursive fallback deletion.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation is not sccache clear, the preview did
    /// not bind a data tree, or the tree no longer matches its snapshot.
    pub fn clear_sccache_data(&mut self) -> Result<(), CompilerCacheLifecycleError> {
        if self.operation != CompilerCacheMutationOperation::Clear
            || self.root.backend != CompilerBackend::Sccache
        {
            return Err(CompilerCacheLifecycleError::Selection(
                "descriptor-based data removal is only valid for an applied sccache clear"
                    .to_owned(),
            ));
        }
        let snapshot = self.data_snapshot.take().ok_or_else(|| {
            CompilerCacheLifecycleError::Selection(
                "sccache clear requires a data-tree preview".to_owned(),
            )
        })?;
        remove_tree_from_snapshot_nofollow(self.root.data_root(), &snapshot, self.data_limits)
            .map_err(|error| {
                CompilerCacheLifecycleError::io(
                    "remove managed sccache data tree",
                    self.root.data_root(),
                    error,
                )
            })?;
        ensure_directory_nofollow(self.root.data_root()).map_err(|error| {
            CompilerCacheLifecycleError::io(
                "recreate managed sccache data directory",
                self.root.data_root(),
                error,
            )
        })?;
        validate_private_directory_nofollow(self.root.data_root()).map_err(|error| {
            CompilerCacheLifecycleError::io(
                "validate recreated managed sccache data directory",
                self.root.data_root(),
                error,
            )
        })?;
        Ok(())
    }

    /// Unlink a stopped private sccache socket through the root descriptor.
    ///
    /// sccache 0.17 can leave a stale Unix-domain socket name after its server
    /// has acknowledged `--stop-server`. This method first proves that no
    /// process accepts a connection on the exact socket, then removes only
    /// that leaf through an open no-follow namespace descriptor. It never
    /// follows the pathname or probes an ambient server.
    ///
    /// # Errors
    ///
    /// Returns an error if the operation is not an applied sccache clear, the
    /// server still accepts connections after the bounded stop interval, or
    /// the selected leaf is unsafe or cannot be unlinked durably.
    pub fn remove_stopped_sccache_socket(&mut self) -> Result<(), CompilerCacheLifecycleError> {
        if self.operation != CompilerCacheMutationOperation::Clear
            || self.root.backend != CompilerBackend::Sccache
        {
            return Err(CompilerCacheLifecycleError::Selection(
                "socket removal is only valid for an applied sccache clear".to_owned(),
            ));
        }
        let socket = self.root.root().join("server.sock");
        let deadline = Instant::now() + SCCACHE_SERVER_STOP_TIMEOUT;
        loop {
            match UnixStream::connect(&socket) {
                Ok(stream) => {
                    drop(stream);
                    if Instant::now() >= deadline {
                        return Err(CompilerCacheLifecycleError::Selection(format!(
                            "managed sccache server still accepts connections at '{}' after {} ms",
                            socket.display(),
                            SCCACHE_SERVER_STOP_TIMEOUT.as_millis()
                        )));
                    }
                    std::thread::sleep(SCCACHE_SERVER_STOP_POLL_INTERVAL);
                }
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                Err(error) if error.kind() == ErrorKind::ConnectionRefused => break,
                Err(error) => {
                    return Err(CompilerCacheLifecycleError::io(
                        "probe managed sccache server socket",
                        &socket,
                        error,
                    ));
                }
            }
        }
        let metadata = std::fs::symlink_metadata(&socket).map_err(|error| {
            CompilerCacheLifecycleError::io(
                "inspect stopped managed sccache server socket",
                &socket,
                error,
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
            return Err(CompilerCacheLifecycleError::ownership(
                CompilerBackend::Sccache,
                self.root.root(),
                "stopped managed sccache server path is not a real Unix-domain socket",
            ));
        }
        let directory = rfs::open(
            self.root.root(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            CompilerCacheLifecycleError::io(
                "open managed sccache root for socket removal",
                self.root.root(),
                error.into(),
            )
        })?;
        rfs::unlinkat(&directory, Path::new("server.sock"), AtFlags::empty()).map_err(|error| {
            CompilerCacheLifecycleError::io(
                "unlink stopped managed sccache server socket",
                &socket,
                error.into(),
            )
        })?;
        rfs::fsync(&directory).map_err(|error| {
            CompilerCacheLifecycleError::io(
                "sync managed sccache root after socket removal",
                self.root.root(),
                error.into(),
            )
        })?;
        Ok(())
    }

    /// Complete a successful backend operation after revalidating root control.
    ///
    /// # Errors
    ///
    /// Returns an error if ownership or generated configuration changed while
    /// the exclusive guard was held.
    pub fn finish(self) -> Result<CompilerCacheMutationResult, CompilerCacheLifecycleError> {
        let lock_path = self.root.build_lock_path();
        self.lease.revalidate().map_err(|error| {
            CompilerCacheLifecycleError::io(
                "revalidate compiler-cache mutation lease",
                &lock_path,
                error,
            )
        })?;
        let _ = load_managed_compiler_cache(self.root.backend, self.root.root.clone())?;
        let outcome = match self.operation {
            CompilerCacheMutationOperation::ResetStats => "statistics_reset",
            CompilerCacheMutationOperation::Clear => "managed_cache_cleared",
        };
        Ok(CompilerCacheMutationResult {
            schema: MUTATION_TOKEN_SCHEMA,
            operation: self.operation.operation_name(),
            backend: self.root.backend,
            cache_root: self.root.root,
            data_root: self.root.data_root,
            outcome,
        })
    }
}

/// A fully resolved launcher, local-only environment, and held build lease.
#[derive(Debug)]
pub struct CompilerCacheBuild {
    selection: CompilerCacheSelection,
    root: CompilerCacheManagedRoot,
    environment: CompilerCacheEnvironment,
    _lease: CompilerCacheBuildLease,
}

impl CompilerCacheBuild {
    /// Exact launcher selection passed to CMake.
    #[must_use]
    pub const fn selection(&self) -> &CompilerCacheSelection {
        &self.selection
    }

    /// Validated owned namespace used by this build.
    #[must_use]
    pub const fn root(&self) -> &CompilerCacheManagedRoot {
        &self.root
    }

    /// Controlled process environment for configure and build children.
    #[must_use]
    pub const fn environment(&self) -> &CompilerCacheEnvironment {
        &self.environment
    }
}

/// Compiler-cache result for one build transaction.
#[derive(Debug)]
pub enum CompilerCacheBuildSelection {
    /// No prepared local backend was selected, so no cache launcher is used.
    Off,
    /// A prepared local backend is selected and read-locked for the build.
    Managed(CompilerCacheBuild),
}

impl CompilerCacheBuildSelection {
    /// CMake-visible selection for this build.
    #[must_use]
    pub fn selection(&self) -> CompilerCacheSelection {
        match self {
            Self::Off => CompilerCacheSelection::Off,
            Self::Managed(build) => build.selection.clone(),
        }
    }

    /// Apply the managed environment when caching is enabled.
    pub fn apply_to(&self, command: &mut std::process::Command) {
        if let Self::Managed(build) = self {
            build.environment.apply_to(command);
        }
    }
}

/// Prepare an empty private root for exactly one selected compiler-cache backend.
///
/// The root is never adopted when it contains any prior state. Configuration
/// and the ownership marker are generated by this function; later builds
/// validate both before spawning a launcher.
///
/// # Errors
///
/// Returns an error if the root is relative, unsafe, non-private, non-empty,
/// already owned by another backend, or cannot be no-clobber initialized.
pub fn prepare_managed_compiler_cache(
    backend: CompilerBackend,
    cache_root: PathBuf,
) -> Result<CompilerCachePreparation, CompilerCacheLifecycleError> {
    let root = resolve_explicit_root(cache_root)?.path;
    validate_backend_path_constraints(backend, &root)?;
    match load_managed_compiler_cache(backend, root.clone()) {
        Ok(managed) => {
            return Ok(CompilerCachePreparation {
                schema: OWNERSHIP_SCHEMA,
                operation: "compiler.prepare",
                backend,
                cache_root: managed.root,
                data_root: managed.data_root,
                created: false,
                ownership: "existing_validated",
            });
        }
        Err(CompilerCacheLifecycleError::MissingPreparedRoot { .. }) => {}
        Err(error) => return Err(error),
    }

    ensure_directory_nofollow(&root).map_err(|error| {
        CompilerCacheLifecycleError::io("create compiler-cache root", &root, error)
    })?;
    validate_private_directory_nofollow(&root).map_err(|error| {
        CompilerCacheLifecycleError::io("validate private compiler-cache root", &root, error)
    })?;
    let entries = directory_entry_names_nofollow_bounded(&root, MAX_ROOT_ENTRIES_DURING_PREPARE)
        .map_err(|error| {
            CompilerCacheLifecycleError::io("inspect empty compiler-cache root", &root, error)
        })?;
    if !entries.is_empty() {
        return Err(CompilerCacheLifecycleError::NonEmptyUnownedRoot { path: root });
    }

    let control = control_root(&root);
    ensure_directory_nofollow(&control).map_err(|error| {
        CompilerCacheLifecycleError::io("create compiler-cache control directory", &control, error)
    })?;
    validate_private_directory_nofollow(&control).map_err(|error| {
        CompilerCacheLifecycleError::io(
            "validate compiler-cache control directory",
            &control,
            error,
        )
    })?;
    let prepare_lock_path = control.join("prepare.lock");
    let prepare_lock = AdvisoryFileLock::acquire(&prepare_lock_path).map_err(|error| {
        CompilerCacheLifecycleError::io(
            "acquire compiler-cache preparation lock",
            &prepare_lock_path,
            error,
        )
    })?;
    prepare_lock.revalidate().map_err(|error| {
        CompilerCacheLifecycleError::io(
            "revalidate compiler-cache preparation lock",
            &prepare_lock_path,
            error,
        )
    })?;

    let root_entries = directory_entry_names_nofollow_bounded(&root, 2).map_err(|error| {
        CompilerCacheLifecycleError::io(
            "recheck compiler-cache root ownership boundary",
            &root,
            error,
        )
    })?;
    if root_entries.len() != 1
        || root_entries
            .first()
            .is_none_or(|entry| entry != CONTROL_DIRECTORY)
    {
        return Err(CompilerCacheLifecycleError::NonEmptyUnownedRoot { path: root });
    }

    let data_root = root.join(CompilerBackend::data_directory());
    ensure_directory_nofollow(&data_root).map_err(|error| {
        CompilerCacheLifecycleError::io("create compiler-cache data directory", &data_root, error)
    })?;
    validate_private_directory_nofollow(&data_root).map_err(|error| {
        CompilerCacheLifecycleError::io("validate compiler-cache data directory", &data_root, error)
    })?;
    let configuration_path = root.join(CONFIGURATION_FILE);
    let configuration = configuration_contents(backend, &data_root)?;
    publish_atomic_file(
        &configuration_path,
        configuration.as_bytes(),
        AtomicFilePolicy::NoClobber,
    )
    .map_err(|error| {
        CompilerCacheLifecycleError::io(
            "publish compiler-cache configuration",
            &configuration_path,
            error,
        )
    })?;
    let ownership = OwnershipRecord {
        schema: OWNERSHIP_SCHEMA.to_owned(),
        backend,
        cache_root: root.clone(),
        data_directory: CompilerBackend::data_directory().to_owned(),
        configuration_sha256: sha256_bytes(configuration.as_bytes()),
    };
    let ownership_path = control.join(OWNERSHIP_FILE);
    let bytes = serde_json::to_vec(&ownership)
        .map_err(|error| CompilerCacheLifecycleError::Metadata(error.to_string()))?;
    publish_atomic_file(&ownership_path, &bytes, AtomicFilePolicy::NoClobber).map_err(|error| {
        CompilerCacheLifecycleError::io(
            "publish compiler-cache ownership marker",
            &ownership_path,
            error,
        )
    })?;
    prepare_lock.revalidate().map_err(|error| {
        CompilerCacheLifecycleError::io(
            "revalidate compiler-cache preparation lock",
            &prepare_lock_path,
            error,
        )
    })?;
    Ok(CompilerCachePreparation {
        schema: OWNERSHIP_SCHEMA,
        operation: "compiler.prepare",
        backend,
        cache_root: root,
        data_root,
        created: true,
        ownership: "empty_private_root_claimed",
    })
}

/// Load and verify one prepared, local-only compiler-cache namespace.
///
/// This validation operation never creates a directory, backend process, lock,
/// or configuration file.
///
/// # Errors
///
/// Returns a specific error if the root is missing, unowned, unsafe, malformed
/// or no longer matches the local-only generated configuration.
pub fn load_managed_compiler_cache(
    backend: CompilerBackend,
    cache_root: PathBuf,
) -> Result<CompilerCacheManagedRoot, CompilerCacheLifecycleError> {
    let root = resolve_explicit_root(cache_root)?.path;
    validate_backend_path_constraints(backend, &root)?;
    let metadata = match std::fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(CompilerCacheLifecycleError::MissingPreparedRoot {
                backend: backend.program(),
                path: root,
            });
        }
        Err(error) => {
            return Err(CompilerCacheLifecycleError::io(
                "inspect compiler-cache root",
                &root,
                error,
            ));
        }
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CompilerCacheLifecycleError::ownership(
            backend,
            &root,
            "root is not a real directory",
        ));
    }
    validate_private_directory_nofollow(&root).map_err(|error| {
        CompilerCacheLifecycleError::io("validate private compiler-cache root", &root, error)
    })?;
    let ownership_path = control_root(&root).join(OWNERSHIP_FILE);
    let (_, bytes) = measure_regular_file_bounded(&ownership_path, MAX_OWNERSHIP_BYTES)
        .map_err(|error| {
            CompilerCacheLifecycleError::io(
                "read compiler-cache ownership marker",
                &ownership_path,
                error,
            )
        })?
        .ok_or_else(|| CompilerCacheLifecycleError::MissingPreparedRoot {
            backend: backend.program(),
            path: root.clone(),
        })?;
    let ownership = decode_ownership(&bytes, backend, &root)?;
    validate_managed_root_layout(backend, &root)?;
    let configuration_path = root.join(CONFIGURATION_FILE);
    validate_configuration(backend, &root, &configuration_path, &ownership)?;
    let data_root = root.join(&ownership.data_directory);
    validate_private_directory_nofollow(&data_root).map_err(|error| {
        CompilerCacheLifecycleError::io("validate compiler-cache data directory", &data_root, error)
    })?;
    Ok(CompilerCacheManagedRoot {
        backend,
        root,
        data_root,
        configuration_path,
    })
}

fn validate_managed_root_layout(
    backend: CompilerBackend,
    root: &Path,
) -> Result<(), CompilerCacheLifecycleError> {
    let entries = directory_entry_names_nofollow_bounded(root, MAX_MANAGED_ROOT_ENTRIES).map_err(
        |error| {
            CompilerCacheLifecycleError::ownership(
                backend,
                root,
                format!("managed root layout cannot be enumerated safely: {error}"),
            )
        },
    )?;
    for entry in entries {
        if entry == CONTROL_DIRECTORY || entry == CONFIGURATION_FILE || entry == "data" {
            continue;
        }
        if entry.to_str().is_some_and(is_publication_journal_lock_name) {
            let lock = root.join(&entry);
            let metadata = std::fs::symlink_metadata(&lock).map_err(|error| {
                CompilerCacheLifecycleError::io("inspect managed publication lock", &lock, error)
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(CompilerCacheLifecycleError::ownership(
                    backend,
                    root,
                    "managed publication lock is not a real regular file",
                ));
            }
            continue;
        }
        if entry == "server.sock" && backend == CompilerBackend::Sccache {
            let socket = root.join(&entry);
            let metadata = std::fs::symlink_metadata(&socket).map_err(|error| {
                CompilerCacheLifecycleError::io(
                    "inspect managed sccache server socket",
                    &socket,
                    error,
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
                return Err(CompilerCacheLifecycleError::ownership(
                    backend,
                    root,
                    "managed sccache server path is not a real Unix-domain socket",
                ));
            }
            continue;
        }
        return Err(CompilerCacheLifecycleError::ownership(
            backend,
            root,
            format!("unexpected root entry '{}'", entry.to_string_lossy()),
        ));
    }
    Ok(())
}

fn validate_backend_path_constraints(
    backend: CompilerBackend,
    root: &Path,
) -> Result<(), CompilerCacheLifecycleError> {
    if backend != CompilerBackend::Sccache {
        return Ok(());
    }
    let socket = root.join("server.sock");
    let length = socket.as_os_str().as_bytes().len();
    if length > MAX_SCCACHE_SOCKET_PATH_BYTES {
        return Err(CompilerCacheLifecycleError::ownership(
            backend,
            root,
            format!(
                "private sccache Unix-domain socket path '{}' is {length} bytes; the cross-host limit is {MAX_SCCACHE_SOCKET_PATH_BYTES} bytes, so choose a shorter AROS_HOME or --dir",
                socket.display()
            ),
        ));
    }
    Ok(())
}

/// Return the controlled local-only environment for a verified managed root.
///
/// # Errors
///
/// Returns an error if a root path cannot be represented in a child process
/// environment.
pub fn compiler_cache_environment(
    root: &CompilerCacheManagedRoot,
) -> Result<CompilerCacheEnvironment, CompilerCacheLifecycleError> {
    validate_backend_path_constraints(root.backend, root.root())?;
    let display = |path: &Path| {
        path.to_str().map(str::to_owned).ok_or_else(|| {
            CompilerCacheLifecycleError::ownership(
                root.backend,
                root.root(),
                "managed path is not valid UTF-8 for backend configuration",
            )
        })
    };
    let mut values = BTreeMap::new();
    match root.backend {
        CompilerBackend::Ccache => {
            values.insert("CCACHE_DIR", display(root.data_root())?);
            values.insert("CCACHE_CONFIGPATH", display(root.configuration_path())?);
        }
        CompilerBackend::Sccache => {
            values.insert("SCCACHE_DIR", display(root.data_root())?);
            values.insert("SCCACHE_CONF", display(root.configuration_path())?);
            values.insert(
                "SCCACHE_SERVER_UDS",
                display(&root.root().join("server.sock"))?,
            );
            values.insert("SCCACHE_IDLE_TIMEOUT", "0".to_owned());
        }
    }
    Ok(CompilerCacheEnvironment { values })
}

/// Acquire a shared lease for a complete configure/build transaction.
///
/// Lifecycle mutation commands must acquire the same lock exclusively before
/// they clear entries or reset backend counters.
///
/// # Errors
///
/// Returns an error if the root control path is unsafe or a lifecycle mutation
/// is already in progress.
pub fn acquire_compiler_cache_build(
    root: &CompilerCacheManagedRoot,
) -> Result<CompilerCacheBuildLease, CompilerCacheLifecycleError> {
    let lock_path = root.build_lock_path();
    let lock_parent = lock_path.parent().ok_or_else(|| {
        CompilerCacheLifecycleError::ownership(
            root.backend,
            root.root(),
            "managed build lock has no parent",
        )
    })?;
    ensure_directory_nofollow(lock_parent).map_err(|error| {
        CompilerCacheLifecycleError::io(
            "create compiler-cache lifecycle lock directory",
            lock_parent,
            error,
        )
    })?;
    validate_private_directory_nofollow(lock_parent).map_err(|error| {
        CompilerCacheLifecycleError::io(
            "validate compiler-cache lifecycle lock directory",
            lock_parent,
            error,
        )
    })?;
    let lease = AdvisoryFileLock::acquire_shared(&lock_path).map_err(|error| {
        CompilerCacheLifecycleError::io("acquire compiler-cache build lease", &lock_path, error)
    })?;
    lease.revalidate().map_err(|error| {
        CompilerCacheLifecycleError::io("revalidate compiler-cache build lease", &lock_path, error)
    })?;
    Ok(CompilerCacheBuildLease { _lease: lease })
}

/// Resolve one build's compiler-cache request against managed local roots only.
///
/// `auto` selects the first available *prepared* AROS-owned backend; a missing
/// prepared root simply means no cache launcher. Explicit backends fail closed
/// if their namespace has not first been prepared. Managed roots are local by
/// construction, so offline builds may safely use them.
///
/// # Errors
///
/// Returns an error for invalid explicit-root combinations, unsafe ownership,
/// unavailable explicitly requested launchers, or lease contention.
pub fn resolve_managed_compiler_cache_for_build(
    choice: CompilerBackendChoice,
    explicit_root: Option<&Path>,
) -> Result<CompilerCacheBuildSelection, CompilerCacheLifecycleError> {
    if matches!(choice, CompilerBackendChoice::Off) {
        if explicit_root.is_some() {
            return Err(CompilerCacheLifecycleError::Selection(
                "--compiler-cache-dir requires --compiler-cache sccache or ccache".to_owned(),
            ));
        }
        return Ok(CompilerCacheBuildSelection::Off);
    }
    if explicit_root.is_some() && matches!(choice, CompilerBackendChoice::Auto) {
        return Err(CompilerCacheLifecycleError::Selection(
            "--compiler-cache-dir requires an explicit backend; use --compiler-cache sccache or --compiler-cache ccache"
                .to_owned(),
        ));
    }
    let candidates: &[CompilerBackend] = match choice {
        CompilerBackendChoice::Auto => &[CompilerBackend::Sccache, CompilerBackend::Ccache],
        CompilerBackendChoice::Sccache => &[CompilerBackend::Sccache],
        CompilerBackendChoice::Ccache => &[CompilerBackend::Ccache],
        CompilerBackendChoice::Off => unreachable!("returned above"),
    };
    for backend in candidates {
        let root = match &explicit_root {
            Some(root) => root.to_path_buf(),
            None => {
                resolve_default_compiler_cache_root(&CacheEnvironment::current(), *backend)?.path
            }
        };
        match load_managed_compiler_cache(*backend, root) {
            Ok(root) => match build_selection(root) {
                Ok(selection) => return Ok(CompilerCacheBuildSelection::Managed(selection)),
                Err(CompilerCacheLifecycleError::Resolution(
                    CompilerCacheResolutionError::Unavailable { .. },
                )) if matches!(choice, CompilerBackendChoice::Auto) => {}
                Err(error) => return Err(error),
            },
            Err(CompilerCacheLifecycleError::MissingPreparedRoot { .. })
                if matches!(choice, CompilerBackendChoice::Auto) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(CompilerCacheBuildSelection::Off)
}

fn build_selection(
    root: CompilerCacheManagedRoot,
) -> Result<CompilerCacheBuild, CompilerCacheLifecycleError> {
    let executable = root.executable()?;
    let environment = compiler_cache_environment(&root)?;
    let lease = acquire_compiler_cache_build(&root)?;
    Ok(CompilerCacheBuild {
        selection: CompilerCacheSelection::Backend {
            backend: root.backend,
            executable,
        },
        root,
        environment,
        _lease: lease,
    })
}

/// Preview one counter reset without starting a backend or taking a lock.
///
/// # Errors
///
/// Returns an error if the selected root is not a valid managed namespace.
pub fn preview_compiler_cache_reset_stats(
    request: &CompilerCacheMutationRequest,
) -> Result<CompilerCacheMutationPreview, CompilerCacheLifecycleError> {
    preview_compiler_cache_mutation(
        request,
        CompilerCacheMutationOperation::ResetStats,
        current_unix_seconds()?.saturating_add(PREVIEW_LIFETIME_SECONDS),
    )
}

/// Preview one managed local compiler-cache clear without mutating it.
///
/// The data tree is measured with explicit entry and byte bounds, and its
/// snapshot is bound into the resulting short-lived token.
///
/// # Errors
///
/// Returns an error for missing, unsafe, special, oversized, or changed data.
pub fn preview_compiler_cache_clear(
    request: &CompilerCacheMutationRequest,
) -> Result<CompilerCacheMutationPreview, CompilerCacheLifecycleError> {
    preview_compiler_cache_mutation(
        request,
        CompilerCacheMutationOperation::Clear,
        current_unix_seconds()?.saturating_add(PREVIEW_LIFETIME_SECONDS),
    )
}

/// Start one token-confirmed exclusive compiler-cache mutation.
///
/// The returned guard must be held across the exact backend command. It holds
/// the same lifecycle lock that managed AROS builds hold in shared mode. The
/// token is rebuilt after lock acquisition, rejecting stale configuration,
/// ownership, root, or data-tree state before any backend is started.
///
/// # Errors
///
/// Returns an error for malformed, expired, stale, or mismatched tokens; an
/// unsafe root; or active AROS build readers.
pub fn begin_compiler_cache_mutation(
    request: &CompilerCacheMutationRequest,
    operation: CompilerCacheMutationOperation,
    apply_token: &str,
) -> Result<CompilerCacheMutation, CompilerCacheLifecycleError> {
    let expires = parse_mutation_token_expiry(apply_token)?;
    let now = current_unix_seconds()?;
    if now > expires {
        return Err(CompilerCacheLifecycleError::Token(
            "the preview expired; run the preview command again".to_owned(),
        ));
    }
    if expires.saturating_sub(now) > PREVIEW_LIFETIME_SECONDS {
        return Err(CompilerCacheLifecycleError::Token(
            "the preview expiry is outside the permitted lifetime".to_owned(),
        ));
    }
    let initial = load_managed_compiler_cache(request.backend, request.cache_root.clone())?;
    let lock_path = initial.build_lock_path();
    let lock_parent = lock_path.parent().ok_or_else(|| {
        CompilerCacheLifecycleError::ownership(
            request.backend,
            initial.root(),
            "managed lifecycle lock has no parent",
        )
    })?;
    ensure_directory_nofollow(lock_parent).map_err(|error| {
        CompilerCacheLifecycleError::io(
            "create compiler-cache lifecycle lock directory",
            lock_parent,
            error,
        )
    })?;
    validate_private_directory_nofollow(lock_parent).map_err(|error| {
        CompilerCacheLifecycleError::io(
            "validate compiler-cache lifecycle lock directory",
            lock_parent,
            error,
        )
    })?;
    let lease = AdvisoryFileLock::acquire(&lock_path).map_err(|error| {
        CompilerCacheLifecycleError::io("acquire compiler-cache mutation lease", &lock_path, error)
    })?;
    lease.revalidate().map_err(|error| {
        CompilerCacheLifecycleError::io(
            "revalidate compiler-cache mutation lease",
            &lock_path,
            error,
        )
    })?;
    let prepared = prepare_compiler_cache_mutation(request, operation, expires)?;
    if prepared.preview.apply_token != apply_token {
        return Err(CompilerCacheLifecycleError::Token(
            "the managed root, ownership marker, configuration, operation, or data snapshot changed; run the preview command again"
                .to_owned(),
        ));
    }
    lease.revalidate().map_err(|error| {
        CompilerCacheLifecycleError::io(
            "revalidate compiler-cache mutation lease",
            &lock_path,
            error,
        )
    })?;
    Ok(CompilerCacheMutation {
        root: prepared.root,
        operation,
        data_snapshot: prepared.data_snapshot,
        data_limits: compiler_cache_tree_limits()?,
        lease,
    })
}

fn preview_compiler_cache_mutation(
    request: &CompilerCacheMutationRequest,
    operation: CompilerCacheMutationOperation,
    expires_unix_seconds: u64,
) -> Result<CompilerCacheMutationPreview, CompilerCacheLifecycleError> {
    Ok(prepare_compiler_cache_mutation(request, operation, expires_unix_seconds)?.preview)
}

struct PreparedCompilerCacheMutation {
    root: CompilerCacheManagedRoot,
    preview: CompilerCacheMutationPreview,
    data_snapshot: Option<TreeContentCas>,
}

fn prepare_compiler_cache_mutation(
    request: &CompilerCacheMutationRequest,
    operation: CompilerCacheMutationOperation,
    expires_unix_seconds: u64,
) -> Result<PreparedCompilerCacheMutation, CompilerCacheLifecycleError> {
    let root = load_managed_compiler_cache(request.backend, request.cache_root.clone())?;
    let ownership = measured_file_proof(
        &control_root(root.root()).join(OWNERSHIP_FILE),
        "read compiler-cache ownership marker",
    )?;
    let configuration = measured_file_proof(
        root.configuration_path(),
        "read compiler-cache configuration",
    )?;
    let (data_scope, data_snapshot) = match operation {
        CompilerCacheMutationOperation::ResetStats => (None, None),
        CompilerCacheMutationOperation::Clear => {
            let limits = compiler_cache_tree_limits()?;
            let snapshot =
                measure_tree_content_cas_bounded(root.data_root(), limits).map_err(|error| {
                    CompilerCacheLifecycleError::io(
                        "measure managed compiler-cache data tree",
                        root.data_root(),
                        error,
                    )
                })?;
            let regular_file_bytes = snapshot.regular_file_bytes().ok_or_else(|| {
                CompilerCacheLifecycleError::ownership(
                    root.backend,
                    root.data_root(),
                    "data-tree byte total is invalid",
                )
            })?;
            let scope = CompilerCacheDataScope {
                entry_count: snapshot.entry_count(),
                regular_file_bytes,
                payload_sha256: snapshot.payload_digest_excluding(None),
                snapshot_sha256: snapshot.snapshot_digest(),
                max_entries: limits.max_entries,
                max_regular_file_bytes: limits.max_regular_file_bytes,
            };
            (Some(scope), Some(snapshot))
        }
    };
    let apply_token = compiler_cache_mutation_token(
        request,
        operation,
        &root,
        &ownership,
        &configuration,
        data_scope.as_ref(),
        expires_unix_seconds,
    )?;
    Ok(PreparedCompilerCacheMutation {
        root: root.clone(),
        preview: CompilerCacheMutationPreview {
            schema: MUTATION_TOKEN_SCHEMA,
            operation: operation.operation_name(),
            backend: root.backend,
            cache_root: root.root.clone(),
            data_root: root.data_root,
            data_scope,
            expires_unix_seconds,
            apply_token,
            recovery: match operation {
                CompilerCacheMutationOperation::ResetStats => {
                    "apply acquires an exclusive local lease and requests only backend counter reset; cached compiler outputs are not selected for deletion"
                }
                CompilerCacheMutationOperation::Clear => {
                    "apply acquires an exclusive local lease and clears only this measured AROS-owned local backend namespace; external, remote, foreign, and unprepared roots are refused"
                }
            },
        },
        data_snapshot,
    })
}

fn compiler_cache_tree_limits() -> Result<TreeTraversalLimits, CompilerCacheLifecycleError> {
    TreeTraversalLimits::new(MAX_COMPILER_CACHE_ENTRIES, MAX_COMPILER_CACHE_BYTES).map_err(
        |error| {
            CompilerCacheLifecycleError::Metadata(format!(
                "invalid managed compiler-cache traversal policy: {error}"
            ))
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CompilerCacheFileProof {
    identity: FileIdentity,
    sha256: Sha256Digest,
    size: u64,
}

fn measured_file_proof(
    path: &Path,
    action: &'static str,
) -> Result<CompilerCacheFileProof, CompilerCacheLifecycleError> {
    let (identity, bytes) = measure_regular_file_bounded(path, MAX_OWNERSHIP_BYTES)
        .map_err(|error| CompilerCacheLifecycleError::io(action, path, error))?
        .ok_or_else(|| {
            CompilerCacheLifecycleError::io(
                action,
                path,
                std::io::Error::new(ErrorKind::NotFound, "expected regular file is missing"),
            )
        })?;
    let size = u64::try_from(bytes.len())
        .map_err(|error| CompilerCacheLifecycleError::Metadata(error.to_string()))?;
    Ok(CompilerCacheFileProof {
        identity,
        sha256: sha256_bytes(&bytes),
        size,
    })
}

#[derive(Debug, Serialize)]
struct CompilerCacheMutationTokenBinding<'a> {
    schema: &'static str,
    operation: &'static str,
    backend: CompilerBackend,
    cache_root: &'a Path,
    data_root: &'a Path,
    ownership: &'a CompilerCacheFileProof,
    configuration: &'a CompilerCacheFileProof,
    data_scope: Option<&'a CompilerCacheDataScope>,
    expires_unix_seconds: u64,
}

fn compiler_cache_mutation_token(
    request: &CompilerCacheMutationRequest,
    operation: CompilerCacheMutationOperation,
    root: &CompilerCacheManagedRoot,
    ownership: &CompilerCacheFileProof,
    configuration: &CompilerCacheFileProof,
    data_scope: Option<&CompilerCacheDataScope>,
    expires_unix_seconds: u64,
) -> Result<String, CompilerCacheLifecycleError> {
    let binding = CompilerCacheMutationTokenBinding {
        schema: MUTATION_TOKEN_SCHEMA,
        operation: operation.token_name(),
        backend: request.backend,
        cache_root: root.root(),
        data_root: root.data_root(),
        ownership,
        configuration,
        data_scope,
        expires_unix_seconds,
    };
    let bytes = serde_json::to_vec(&binding)
        .map_err(|error| CompilerCacheLifecycleError::Metadata(error.to_string()))?;
    Ok(format!(
        "{MUTATION_TOKEN_SCHEMA}:{expires_unix_seconds}:{}",
        sha256_bytes(&bytes)
    ))
}

fn parse_mutation_token_expiry(token: &str) -> Result<u64, CompilerCacheLifecycleError> {
    let mut parts = token.split(':');
    let schema = parts.next();
    let expiry = parts.next();
    let digest = parts.next();
    if schema != Some(MUTATION_TOKEN_SCHEMA) || parts.next().is_some() {
        return Err(CompilerCacheLifecycleError::Token(
            "expected a versioned token emitted by the corresponding preview command".to_owned(),
        ));
    }
    let expiry = expiry
        .ok_or_else(|| CompilerCacheLifecycleError::Token("token has no expiry value".to_owned()))?
        .parse::<u64>()
        .map_err(|_| {
            CompilerCacheLifecycleError::Token(
                "token expiry is not an unsigned timestamp".to_owned(),
            )
        })?;
    Sha256Digest::parse(digest.ok_or_else(|| {
        CompilerCacheLifecycleError::Token("token has no binding digest".to_owned())
    })?)
    .map_err(|_| {
        CompilerCacheLifecycleError::Token("token binding digest is malformed".to_owned())
    })?;
    Ok(expiry)
}

fn current_unix_seconds() -> Result<u64, CompilerCacheLifecycleError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| CompilerCacheLifecycleError::Clock(error.to_string()))
}

fn control_root(root: &Path) -> PathBuf {
    root.join(CONTROL_DIRECTORY).join(CONTROL_VERSION)
}

fn configuration_contents(
    backend: CompilerBackend,
    data_root: &Path,
) -> Result<String, CompilerCacheLifecycleError> {
    let data_root = data_root.to_str().ok_or_else(|| {
        CompilerCacheLifecycleError::ownership(
            backend,
            data_root,
            "managed data root is not valid UTF-8 for backend configuration",
        )
    })?;
    Ok(match backend {
        CompilerBackend::Ccache => {
            "# Generated by aros-tools. Do not add remote or secondary storage here.\nmax_size = 5.0G\n".to_owned()
        }
        CompilerBackend::Sccache => format!(
            "# Generated by aros-tools. This namespace has local disk storage only.\n[cache.disk]\ndir = \"{data_root}\"\nsize = 5368709120\n"
        ),
    })
}

fn decode_ownership(
    bytes: &[u8],
    backend: CompilerBackend,
    root: &Path,
) -> Result<OwnershipRecord, CompilerCacheLifecycleError> {
    let ownership = serde_json::from_slice::<OwnershipRecord>(bytes).map_err(|error| {
        CompilerCacheLifecycleError::ownership(
            backend,
            root,
            format!("cannot decode ownership marker: {error}"),
        )
    })?;
    if ownership.schema != OWNERSHIP_SCHEMA {
        return Err(CompilerCacheLifecycleError::ownership(
            backend,
            root,
            "ownership schema is unsupported",
        ));
    }
    if ownership.backend != backend {
        return Err(CompilerCacheLifecycleError::ownership(
            backend,
            root,
            format!("marker is bound to '{}'", ownership.backend.program()),
        ));
    }
    if ownership.cache_root != root {
        return Err(CompilerCacheLifecycleError::ownership(
            backend,
            root,
            "marker cache root does not match the requested absolute root",
        ));
    }
    if ownership.data_directory != CompilerBackend::data_directory() {
        return Err(CompilerCacheLifecycleError::ownership(
            backend,
            root,
            "marker data directory is not the fixed managed location",
        ));
    }
    Ok(ownership)
}

fn validate_configuration(
    backend: CompilerBackend,
    root: &Path,
    path: &Path,
    ownership: &OwnershipRecord,
) -> Result<(), CompilerCacheLifecycleError> {
    let (_, bytes) = measure_regular_file_bounded(path, MAX_OWNERSHIP_BYTES)
        .map_err(|error| {
            CompilerCacheLifecycleError::io("read compiler-cache configuration", path, error)
        })?
        .ok_or_else(|| {
            CompilerCacheLifecycleError::ownership(backend, root, "configuration file is missing")
        })?;
    let expected = configuration_contents(backend, &root.join(CompilerBackend::data_directory()))?;
    let actual = sha256_bytes(&bytes);
    if actual != ownership.configuration_sha256 || bytes != expected.as_bytes() {
        return Err(CompilerCacheLifecycleError::ownership(
            backend,
            root,
            "configuration changed; managed cache configuration must remain generated local-only",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnershipRecord {
    schema: String,
    backend: CompilerBackend,
    cache_root: PathBuf,
    data_directory: String,
    configuration_sha256: Sha256Digest,
}

#[cfg(test)]
mod tests {
    use super::{
        acquire_compiler_cache_build, begin_compiler_cache_mutation, compiler_cache_environment,
        load_managed_compiler_cache, prepare_managed_compiler_cache, preview_compiler_cache_clear,
        preview_compiler_cache_reset_stats, resolve_managed_compiler_cache_for_build,
        CompilerCacheLifecycleError, CompilerCacheMutationOperation, CompilerCacheMutationRequest,
    };
    use crate::{CompilerBackend, CompilerBackendChoice};

    #[test]
    #[cfg(unix)]
    fn preparation_claims_only_an_empty_private_root_and_generates_local_sccache_configuration() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("sccache");
        let prepared =
            prepare_managed_compiler_cache(CompilerBackend::Sccache, root.clone()).unwrap();
        assert!(prepared.created);
        let managed = load_managed_compiler_cache(CompilerBackend::Sccache, root).unwrap();
        let environment = compiler_cache_environment(&managed).unwrap();
        assert_eq!(
            environment.values().get("SCCACHE_DIR"),
            Some(&managed.data_root().display().to_string())
        );
        assert_eq!(
            environment.values().get("SCCACHE_CONF"),
            Some(&managed.configuration_path().display().to_string())
        );
        assert!(std::fs::read_to_string(managed.configuration_path())
            .unwrap()
            .contains("[cache.disk]"));
    }

    #[test]
    #[cfg(unix)]
    fn preparation_never_adopts_preexisting_files() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("foreign");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("foreign-entry"), "no").unwrap();
        assert!(matches!(
            prepare_managed_compiler_cache(CompilerBackend::Ccache, root),
            Err(CompilerCacheLifecycleError::NonEmptyUnownedRoot { .. })
        ));
    }

    #[test]
    #[cfg(unix)]
    fn sccache_preparation_rejects_a_socket_path_beyond_the_cross_host_limit() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("x".repeat(128));
        assert!(matches!(
            prepare_managed_compiler_cache(CompilerBackend::Sccache, root),
            Err(CompilerCacheLifecycleError::InvalidOwnership { .. })
        ));
    }

    #[test]
    #[cfg(unix)]
    fn tampered_configuration_invalidates_managed_ownership() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("ccache");
        prepare_managed_compiler_cache(CompilerBackend::Ccache, root.clone()).unwrap();
        std::fs::write(
            root.join("configuration.toml"),
            "remote_storage = redis://example.invalid\n",
        )
        .unwrap();
        assert!(matches!(
            load_managed_compiler_cache(CompilerBackend::Ccache, root),
            Err(CompilerCacheLifecycleError::InvalidOwnership { .. })
        ));
    }

    #[test]
    #[cfg(unix)]
    fn managed_root_rejects_unregistered_top_level_entries() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("ccache");
        prepare_managed_compiler_cache(CompilerBackend::Ccache, root.clone()).unwrap();
        std::fs::write(root.join("unexpected"), "not managed").unwrap();
        assert!(matches!(
            load_managed_compiler_cache(CompilerBackend::Ccache, root),
            Err(CompilerCacheLifecycleError::InvalidOwnership { .. })
        ));
    }

    #[test]
    #[cfg(unix)]
    fn a_custom_build_root_requires_one_explicit_backend_before_it_can_be_read() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("ccache");
        assert!(matches!(
            resolve_managed_compiler_cache_for_build(
                CompilerBackendChoice::Auto,
                Some(root.as_path())
            ),
            Err(CompilerCacheLifecycleError::Selection(_))
        ));
        assert!(matches!(
            resolve_managed_compiler_cache_for_build(
                CompilerBackendChoice::Off,
                Some(root.as_path())
            ),
            Err(CompilerCacheLifecycleError::Selection(_))
        ));
    }

    #[test]
    #[cfg(unix)]
    fn clear_preview_binds_the_exact_data_tree_and_rejects_a_changed_file() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("sccache");
        prepare_managed_compiler_cache(CompilerBackend::Sccache, root.clone()).unwrap();
        let data = root.join("data").join("nested");
        std::fs::create_dir(&data).unwrap();
        let entry = data.join("object");
        std::fs::write(&entry, "first").unwrap();
        let request = CompilerCacheMutationRequest {
            backend: CompilerBackend::Sccache,
            cache_root: root,
        };
        let preview = preview_compiler_cache_clear(&request).unwrap();
        assert_eq!(preview.data_scope.as_ref().unwrap().entry_count, 2);
        assert!(entry.is_file(), "preview must not delete cache data");
        std::fs::write(&entry, "changed").unwrap();
        assert!(matches!(
            begin_compiler_cache_mutation(
                &request,
                CompilerCacheMutationOperation::Clear,
                &preview.apply_token,
            ),
            Err(CompilerCacheLifecycleError::Token(_))
        ));
        assert!(entry.is_file(), "stale apply must not delete cache data");
    }

    #[test]
    #[cfg(unix)]
    fn sccache_clear_recreates_only_its_owned_data_root_after_token_apply() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("sccache");
        prepare_managed_compiler_cache(CompilerBackend::Sccache, root.clone()).unwrap();
        std::fs::write(root.join("data").join("object"), "cache bytes").unwrap();
        let request = CompilerCacheMutationRequest {
            backend: CompilerBackend::Sccache,
            cache_root: root.clone(),
        };
        let preview = preview_compiler_cache_clear(&request).unwrap();
        let mut mutation = begin_compiler_cache_mutation(
            &request,
            CompilerCacheMutationOperation::Clear,
            &preview.apply_token,
        )
        .unwrap();
        mutation.clear_sccache_data().unwrap();
        let result = mutation.finish().unwrap();
        assert_eq!(result.outcome, "managed_cache_cleared");
        assert!(root.join("data").is_dir());
        assert!(std::fs::read_dir(root.join("data"))
            .unwrap()
            .next()
            .is_none());
        assert!(root.join("configuration.toml").is_file());
    }

    #[test]
    #[cfg(unix)]
    fn stopped_private_sccache_socket_is_descriptor_unlinked_before_clear() {
        use std::os::unix::net::UnixListener;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("sccache");
        prepare_managed_compiler_cache(CompilerBackend::Sccache, root.clone()).unwrap();
        let socket = root.join("server.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        drop(listener);
        assert!(
            socket.exists(),
            "Unix-domain socket pathname must remain stale"
        );
        let request = CompilerCacheMutationRequest {
            backend: CompilerBackend::Sccache,
            cache_root: root,
        };
        let preview = preview_compiler_cache_clear(&request).unwrap();
        let mut mutation = begin_compiler_cache_mutation(
            &request,
            CompilerCacheMutationOperation::Clear,
            &preview.apply_token,
        )
        .unwrap();
        mutation.remove_stopped_sccache_socket().unwrap();
        assert!(
            !socket.exists(),
            "only the stale private socket is unlinked"
        );
        mutation.clear_sccache_data().unwrap();
        mutation.finish().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn active_build_reader_excludes_a_token_confirmed_counter_reset() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("ccache");
        prepare_managed_compiler_cache(CompilerBackend::Ccache, root.clone()).unwrap();
        let managed = load_managed_compiler_cache(CompilerBackend::Ccache, root.clone()).unwrap();
        let reader = acquire_compiler_cache_build(&managed).unwrap();
        let request = CompilerCacheMutationRequest {
            backend: CompilerBackend::Ccache,
            cache_root: root,
        };
        let preview = preview_compiler_cache_reset_stats(&request).unwrap();
        assert!(matches!(
            begin_compiler_cache_mutation(
                &request,
                CompilerCacheMutationOperation::ResetStats,
                &preview.apply_token,
            ),
            Err(CompilerCacheLifecycleError::Io { .. })
        ));
        drop(reader);
    }
}
