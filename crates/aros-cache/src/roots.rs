//! Passive resolution and observation of AROS-owned cache roots.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use serde::Serialize;
use thiserror::Error;

/// Environment inputs used to resolve AROS state without modifying it.
///
/// Tests and callers that need a deterministic observation can construct this
/// explicitly instead of mutating process-global environment variables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheEnvironment {
    /// Value of `AROS_HOME`, when configured.
    pub aros_home: Option<OsString>,
    /// Value of `AROS_CACHE_DIR`, when configured.
    pub archive_cache_dir: Option<OsString>,
    /// Value of the platform home directory, when available.
    pub home: Option<OsString>,
}

impl CacheEnvironment {
    /// Snapshot the process environment once for a passive cache operation.
    #[must_use]
    pub fn current() -> Self {
        Self {
            aros_home: env::var_os("AROS_HOME"),
            archive_cache_dir: env::var_os("AROS_CACHE_DIR"),
            home: env::var_os("HOME"),
        }
    }
}

/// Where a resolved root originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootOrigin {
    /// An explicit command-line path for a bounded status operation.
    Explicit,
    /// An explicit environment override.
    Environment,
    /// A deterministic default below the user's platform home directory.
    Default,
}

impl RootOrigin {
    /// Stable lowercase label used by human-oriented result rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Environment => "environment",
            Self::Default => "default",
        }
    }
}

/// One resolved AROS root before any filesystem interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheRoot {
    /// The absolute selected path.
    pub path: PathBuf,
    /// How the path was selected.
    pub origin: RootOrigin,
    /// Environment variable that supplied the path, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable: Option<&'static str>,
}

/// A filesystem state observed without following a root symlink or creating
/// directories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootState {
    /// The selected path does not exist.
    Missing,
    /// The selected path is an ordinary directory.
    Directory,
    /// The selected path is a symlink. It is not followed by passive status.
    Symlink,
    /// The selected path exists but is not a usable directory.
    Other,
    /// Metadata could not be read; the path is not treated as missing.
    Inaccessible,
}

impl RootState {
    /// Stable lowercase label used by human-oriented result rendering.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
            Self::Other => "other",
            Self::Inaccessible => "inaccessible",
        }
    }
}

/// A root and its one-path, non-creating observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RootObservation {
    /// The selected root and its provenance.
    #[serde(flatten)]
    pub root: CacheRoot,
    /// Observed state of the root itself.
    pub state: RootState,
    /// Scope of this observation; status reads only root metadata.
    pub coverage: &'static str,
    /// Number of child entries inspected by this observation.
    pub inspected_entries: u64,
    /// Configured entry limit, when the operation traversed entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<u64>,
    /// Metadata-derived byte size, when an operation collected it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_bytes: Option<u64>,
    /// Byte size measured by reading payload content, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_bytes: Option<u64>,
    /// Non-fatal facts that limit the observation.
    pub findings: Vec<&'static str>,
    /// Stable I/O category for an inaccessible root, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<&'static str>,
}

/// A root-resolution error that must be surfaced before a cache operation can
/// decide what it owns.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RootResolutionError {
    /// There is no platform home directory from which to derive the default.
    #[error("cannot resolve AROS state: HOME is unset; set AROS_HOME to an absolute path")]
    MissingHome,
    /// User-controlled state paths must never be relative to the caller.
    #[error("{variable} must be an absolute path, got '{path}'")]
    RelativePath {
        /// Name of the invalid environment variable.
        variable: &'static str,
        /// Rendered invalid value.
        path: String,
    },
}

/// Resolve the AROS state root without creating it.
///
/// # Errors
///
/// Returns an error when no absolute state root can be derived.
pub fn resolve_aros_home(environment: &CacheEnvironment) -> Result<CacheRoot, RootResolutionError> {
    if let Some(path) = &environment.aros_home {
        require_absolute("AROS_HOME", PathBuf::from(path), RootOrigin::Environment)
    } else {
        let home = environment
            .home
            .as_ref()
            .ok_or(RootResolutionError::MissingHome)?;
        require_absolute(
            "HOME",
            PathBuf::from(home).join(".aros"),
            RootOrigin::Default,
        )
    }
}

/// Resolve the shared host/cross archive cache root without creating it.
///
/// # Errors
///
/// Returns an error when an explicit path is relative or the default state
/// root cannot be derived.
pub fn resolve_archive_cache_root(
    environment: &CacheEnvironment,
) -> Result<CacheRoot, RootResolutionError> {
    if let Some(path) = &environment.archive_cache_dir {
        require_absolute(
            "AROS_CACHE_DIR",
            PathBuf::from(path),
            RootOrigin::Environment,
        )
    } else {
        let home = resolve_aros_home(environment)?;
        Ok(CacheRoot {
            path: home.path.join("cache"),
            origin: home.origin,
            variable: home.variable,
        })
    }
}

/// Resolve a caller-selected cache root without creating or inspecting it.
///
/// # Errors
///
/// Returns an error when the supplied path is relative.
pub fn resolve_explicit_root(path: PathBuf) -> Result<CacheRoot, RootResolutionError> {
    require_absolute("--dir", path, RootOrigin::Explicit)
}

/// Resolve the candidate AROS-owned compiler-cache namespace for one backend.
///
/// The function does not create the directory and does not claim that a
/// backend currently uses it. Build policy binds a backend to a namespace only
/// after the corresponding lifecycle contract is implemented.
///
/// # Errors
///
/// Returns an error when the AROS state root cannot be derived safely.
pub fn resolve_default_compiler_cache_root(
    environment: &CacheEnvironment,
    backend: crate::CompilerBackend,
) -> Result<CacheRoot, RootResolutionError> {
    let home = resolve_aros_home(environment)?;
    Ok(CacheRoot {
        path: home
            .path
            .join("cache")
            .join("compiler")
            .join("v1")
            .join(backend.program()),
        origin: home.origin,
        variable: home.variable,
    })
}

/// Resolve the AROS state root from the current process environment.
///
/// # Errors
///
/// Returns an error when no absolute root can be derived.
pub fn aros_home() -> Result<PathBuf, RootResolutionError> {
    Ok(resolve_aros_home(&CacheEnvironment::current())?.path)
}

/// Resolve the archive cache root from the current process environment.
///
/// # Errors
///
/// Returns an error when no absolute root can be derived.
pub fn archive_cache_root() -> Result<PathBuf, RootResolutionError> {
    Ok(resolve_archive_cache_root(&CacheEnvironment::current())?.path)
}

/// Observe one root without creating it, following it, recursing, hashing or
/// acquiring a lock.
#[must_use]
pub fn observe_root(root: CacheRoot) -> RootObservation {
    let (state, error_kind) = match fs::symlink_metadata(&root.path) {
        Ok(metadata) => classify_root_metadata(&metadata),
        Err(error) => classify_root_error(&error),
    };
    RootObservation {
        root,
        state,
        coverage: "root_metadata",
        inspected_entries: 0,
        max_entries: None,
        metadata_bytes: None,
        measured_bytes: None,
        findings: Vec::new(),
        error_kind,
    }
}

fn classify_root_metadata(metadata: &fs::Metadata) -> (RootState, Option<&'static str>) {
    if metadata.file_type().is_symlink() {
        (RootState::Symlink, None)
    } else if metadata.is_dir() {
        (RootState::Directory, None)
    } else {
        (RootState::Other, None)
    }
}

fn classify_root_error(error: &std::io::Error) -> (RootState, Option<&'static str>) {
    if error.kind() == ErrorKind::NotFound {
        (RootState::Missing, None)
    } else {
        (RootState::Inaccessible, Some(io_error_kind(error.kind())))
    }
}

fn require_absolute(
    variable: &'static str,
    path: PathBuf,
    origin: RootOrigin,
) -> Result<CacheRoot, RootResolutionError> {
    if !path.is_absolute() {
        return Err(RootResolutionError::RelativePath {
            variable,
            path: path.display().to_string(),
        });
    }
    Ok(CacheRoot {
        path,
        origin,
        variable: (origin == RootOrigin::Environment).then_some(variable),
    })
}

const fn io_error_kind(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::PermissionDenied => "permission_denied",
        ErrorKind::NotADirectory => "not_a_directory",
        ErrorKind::TooManyLinks => "too_many_links",
        _ => "io_error",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_root_error, observe_root, resolve_archive_cache_root, resolve_aros_home,
        resolve_explicit_root, CacheEnvironment, RootOrigin, RootResolutionError, RootState,
    };
    use std::ffi::OsString;
    use std::fs;
    use std::io::{Error, ErrorKind};

    #[test]
    fn defaults_stay_below_an_absolute_home_without_creating_state() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("missing-home");
        let environment = CacheEnvironment {
            home: Some(path.clone().into_os_string()),
            ..CacheEnvironment::default()
        };
        let home = resolve_aros_home(&environment).unwrap();
        let archive = resolve_archive_cache_root(&environment).unwrap();
        assert_eq!(home.path, path.join(".aros"));
        assert_eq!(home.origin, RootOrigin::Default);
        assert_eq!(archive.path, path.join(".aros/cache"));
        assert_eq!(observe_root(archive).state, RootState::Missing);
        assert!(!path.exists());
    }

    #[test]
    fn archive_override_has_its_own_environment_provenance() {
        let environment = CacheEnvironment {
            archive_cache_dir: Some(OsString::from("/work/archives")),
            ..CacheEnvironment::default()
        };
        let root = resolve_archive_cache_root(&environment).unwrap();
        assert_eq!(root.path, std::path::Path::new("/work/archives"));
        assert_eq!(root.origin, RootOrigin::Environment);
        assert_eq!(root.variable, Some("AROS_CACHE_DIR"));
    }

    #[test]
    fn archive_root_precedence_is_environment_then_aros_home_then_default_home() {
        let environment = CacheEnvironment {
            aros_home: Some(OsString::from("/work/aros-state")),
            archive_cache_dir: Some(OsString::from("/work/archive-cache")),
            home: Some(OsString::from("/users/fabian")),
        };
        let explicit_archive = resolve_archive_cache_root(&environment).unwrap();
        assert_eq!(
            explicit_archive.path,
            std::path::Path::new("/work/archive-cache")
        );
        assert_eq!(explicit_archive.origin, RootOrigin::Environment);
        assert_eq!(explicit_archive.variable, Some("AROS_CACHE_DIR"));

        let environment = CacheEnvironment {
            aros_home: Some(OsString::from("/work/aros-state")),
            home: Some(OsString::from("/users/fabian")),
            ..CacheEnvironment::default()
        };
        let aros_home_archive = resolve_archive_cache_root(&environment).unwrap();
        assert_eq!(
            aros_home_archive.path,
            std::path::Path::new("/work/aros-state/cache")
        );
        assert_eq!(aros_home_archive.origin, RootOrigin::Environment);
        assert_eq!(aros_home_archive.variable, Some("AROS_HOME"));

        let environment = CacheEnvironment {
            home: Some(OsString::from("/users/fabian")),
            ..CacheEnvironment::default()
        };
        let default_archive = resolve_archive_cache_root(&environment).unwrap();
        assert_eq!(
            default_archive.path,
            std::path::Path::new("/users/fabian/.aros/cache")
        );
        assert_eq!(default_archive.origin, RootOrigin::Default);
        assert_eq!(default_archive.variable, None);
    }

    #[cfg(unix)]
    #[test]
    fn root_observation_never_follows_symlinks_or_inspects_children() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("directory");
        let file = temporary.path().join("file");
        let link = temporary.path().join("link");
        fs::create_dir(&directory).unwrap();
        fs::write(&file, b"not a cache root").unwrap();
        symlink(&directory, &link).unwrap();

        for (path, expected) in [
            (directory, RootState::Directory),
            (file, RootState::Other),
            (link, RootState::Symlink),
        ] {
            let observation = observe_root(resolve_explicit_root(path).unwrap());
            assert_eq!(observation.state, expected);
            assert_eq!(observation.coverage, "root_metadata");
            assert_eq!(observation.inspected_entries, 0);
            assert_eq!(observation.max_entries, None);
            assert_eq!(observation.metadata_bytes, None);
            assert_eq!(observation.measured_bytes, None);
            assert!(observation.findings.is_empty());
        }
    }

    #[test]
    fn inaccessible_root_errors_are_not_misreported_as_missing() {
        for (kind, expected_kind) in [
            (ErrorKind::PermissionDenied, "permission_denied"),
            (ErrorKind::NotADirectory, "not_a_directory"),
            (ErrorKind::TooManyLinks, "too_many_links"),
            (ErrorKind::Interrupted, "io_error"),
        ] {
            assert_eq!(
                classify_root_error(&Error::from(kind)),
                (RootState::Inaccessible, Some(expected_kind))
            );
        }
        assert_eq!(
            classify_root_error(&Error::from(ErrorKind::NotFound)),
            (RootState::Missing, None)
        );
    }

    #[test]
    fn relative_user_roots_fail_before_status_can_touch_them() {
        let environment = CacheEnvironment {
            aros_home: Some(OsString::from("relative")),
            ..CacheEnvironment::default()
        };
        assert_eq!(
            resolve_aros_home(&environment),
            Err(RootResolutionError::RelativePath {
                variable: "AROS_HOME",
                path: "relative".to_owned(),
            })
        );
    }

    #[test]
    fn explicit_roots_are_absolute_and_retain_their_provenance() {
        let root = resolve_explicit_root(std::path::PathBuf::from("/work/cache")).unwrap();
        assert_eq!(root.origin, RootOrigin::Explicit);
        assert_eq!(root.variable, None);
        assert!(resolve_explicit_root(std::path::PathBuf::from("relative")).is_err());
    }
}
