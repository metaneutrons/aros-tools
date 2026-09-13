//! Explicit ownership and process environment for local compiler caches.
//!
//! Compiler-cache programs can otherwise inherit remote endpoints, foreign
//! storage roots, or daemon sockets from their caller. This module separates
//! passive discovery from an opt-in managed namespace: a build uses a cache
//! only after that namespace was prepared, ownership was revalidated, and a
//! shared lifecycle lease was acquired for its complete duration.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use aros_common::{
    directory_entry_names_nofollow_bounded, ensure_directory_nofollow,
    measure_regular_file_bounded, publish_atomic_file, sha256_bytes,
    validate_private_directory_nofollow, AdvisoryFileLock, AtomicFilePolicy, Sha256Digest,
};
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

/// Return the controlled local-only environment for a verified managed root.
///
/// # Errors
///
/// Returns an error if a root path cannot be represented in a child process
/// environment.
pub fn compiler_cache_environment(
    root: &CompilerCacheManagedRoot,
) -> Result<CompilerCacheEnvironment, CompilerCacheLifecycleError> {
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
    let executable = which::which(root.backend.program()).map_err(|_| {
        CompilerCacheLifecycleError::Resolution(CompilerCacheResolutionError::Unavailable {
            backend: root.backend.program(),
        })
    })?;
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
        compiler_cache_environment, load_managed_compiler_cache, prepare_managed_compiler_cache,
        resolve_managed_compiler_cache_for_build, CompilerCacheLifecycleError,
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
}
