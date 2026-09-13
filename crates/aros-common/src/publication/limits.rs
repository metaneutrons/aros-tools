//! Explicit bounded-traversal and advisory-lock contracts.

#[cfg(not(unix))]
use super::unsupported_durability;
use super::{absolute_path, unix, validate_target_leaf, FileIdentity};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Resource ceiling for a descriptor-validated tree snapshot or copy.
///
/// The limit is deliberately explicit at mutation boundaries. It prevents a
/// caller from turning an inspection or import preview into an unbounded walk
/// of an attacker-controlled tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeTraversalLimits {
    /// Maximum number of non-root filesystem entries examined.
    pub max_entries: usize,
    /// Maximum total regular-file bytes examined.
    pub max_regular_file_bytes: u64,
}

impl TreeTraversalLimits {
    /// Construct a non-zero bounded traversal policy.
    ///
    /// # Errors
    ///
    /// Returns an invalid-input error when either ceiling is zero.
    pub fn new(max_entries: usize, max_regular_file_bytes: u64) -> std::io::Result<Self> {
        if max_entries == 0 || max_regular_file_bytes == 0 {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "tree traversal limits must be greater than zero",
            ));
        }
        Ok(Self {
            max_entries,
            max_regular_file_bytes,
        })
    }
}

/// Lock mode for a no-follow advisory file lock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvisoryLockMode {
    /// Several cooperating readers may hold the lock simultaneously.
    Shared,
    /// Exactly one cooperating writer or lifecycle mutation may hold the lock.
    Exclusive,
}

/// An OS-held advisory lock for one no-follow regular file.
///
/// The guard owns the open descriptor. It is intentionally neither cloneable
/// nor serializable: a pathname or PID alone is never evidence that a lock is
/// still held.
#[derive(Debug)]
pub struct AdvisoryFileLock {
    #[cfg(unix)]
    file: std::fs::File,
    #[cfg(unix)]
    path: PathBuf,
    #[cfg(unix)]
    identity: FileIdentity,
    #[cfg(unix)]
    mode: AdvisoryLockMode,
}

/// Observed state of one advisory lock without creating its path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvisoryLockState {
    /// No regular lock file exists at the requested path.
    Absent,
    /// A regular lock file exists, but no process currently holds it.
    Unheld,
    /// Another process currently holds the exclusive lock.
    Held,
}

/// Descriptor-measured observation of an advisory-lock pathname.
///
/// A pathname may not be substituted between a durable receipt and a later
/// lifecycle scan. Consumers therefore bind both this identity and state; a
/// state alone is not authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvisoryLockObservation {
    /// Identity of the regular lock file when it exists.
    pub identity: Option<FileIdentity>,
    /// Whether another process currently holds that exact file lock.
    pub state: AdvisoryLockState,
}

impl AdvisoryFileLock {
    /// Acquire an exclusive no-follow advisory lock, creating its parent
    /// namespace and lock file safely when needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is unsafe, another process owns the
    /// lock, or durable Unix locking is unavailable.
    pub fn acquire(path: &Path) -> std::io::Result<Self> {
        Self::acquire_with_mode(path, AdvisoryLockMode::Exclusive)
    }

    /// Acquire a shared no-follow advisory lock.
    ///
    /// Several readers may coexist, while an exclusive lifecycle mutation is
    /// refused until every reader releases its descriptor. The caller must
    /// retain this guard for the complete duration of its read.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is unsafe, an exclusive holder owns the
    /// lock, or durable Unix locking is unavailable.
    pub fn acquire_shared(path: &Path) -> std::io::Result<Self> {
        Self::acquire_with_mode(path, AdvisoryLockMode::Shared)
    }

    fn acquire_with_mode(path: &Path, mode: AdvisoryLockMode) -> std::io::Result<Self> {
        validate_target_leaf(path)?;
        #[cfg(unix)]
        {
            let path = absolute_path(path)?;
            let file = unix::acquire_advisory_file_lock(&path, mode)?;
            let identity = unix::advisory_file_lock_identity(&file)?;
            Ok(Self {
                file,
                path,
                identity,
                mode,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (path, mode);
            Err(unsupported_durability())
        }
    }

    /// Reassert that this process still holds its selected lock mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying lock can no longer be proven.
    pub fn revalidate(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            unix::revalidate_advisory_file_lock(&self.file, &self.path, self.identity, self.mode)
        }
        #[cfg(not(unix))]
        {
            Err(unsupported_durability())
        }
    }

    /// Return the descriptor-measured identity of this lock file.
    ///
    /// Lifecycle receipts use this value to reject a substituted lock
    /// pathname rather than treating a different unheld inode as stale.
    ///
    /// # Errors
    ///
    /// Returns an error if the descriptor no longer owns its lock or the
    /// lock pathname was replaced.
    pub fn identity(&self) -> std::io::Result<FileIdentity> {
        #[cfg(unix)]
        {
            self.revalidate()?;
            Ok(self.identity)
        }
        #[cfg(not(unix))]
        {
            Err(unsupported_durability())
        }
    }
}

/// Probe one advisory lock without creating a file or changing lock state.
///
/// This is suitable for a read-only lifecycle preview. A persisted receipt is
/// not authority by itself: consumers must bind the returned identity and
/// [`AdvisoryLockState::Held`] before treating a process as protecting the
/// associated resource.
///
/// # Errors
///
/// Returns an error when the path is unsafe, points to a non-regular file, or
/// cannot be inspected through the no-follow descriptor path.
pub fn probe_advisory_file_lock(path: &Path) -> std::io::Result<AdvisoryLockObservation> {
    validate_target_leaf(path)?;
    #[cfg(unix)]
    {
        unix::probe_advisory_file_lock(&absolute_path(path)?)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(unsupported_durability())
    }
}
