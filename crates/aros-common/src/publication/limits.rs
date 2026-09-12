//! Explicit bounded-traversal and advisory-lock contracts.

#[cfg(not(unix))]
use super::unsupported_durability;
use super::{absolute_path, unix, validate_target_leaf};
use std::io::ErrorKind;
use std::path::Path;

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

/// An OS-held exclusive advisory lock for one no-follow regular file.
///
/// The guard owns the open descriptor. It is intentionally neither cloneable
/// nor serializable: a pathname or PID alone is never evidence that a lock is
/// still held.
#[derive(Debug)]
pub struct AdvisoryFileLock {
    #[cfg(unix)]
    file: std::fs::File,
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
        validate_target_leaf(path)?;
        #[cfg(unix)]
        {
            Ok(Self {
                file: unix::acquire_advisory_file_lock(&absolute_path(path)?)?,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            Err(unsupported_durability())
        }
    }

    /// Reassert that this process still holds the exclusive lock.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying lock can no longer be proven.
    pub fn revalidate(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            unix::revalidate_advisory_file_lock(&self.file)
        }
        #[cfg(not(unix))]
        {
            Err(unsupported_durability())
        }
    }
}
