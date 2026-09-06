//! Owner-scoped flock lifetime, independent of inherited/duplicated references.

use std::fs::File;
use std::io;

use aros_common::DiagnosticCode;
use rustix::fs::{flock, FlockOperation};
use rustix::process::{getpid, Pid};

/// Created immediately after acquisition, before any fallible marker work.
#[derive(Debug)]
pub(super) struct DirectoryLock {
    file: File,
    held: bool,
    owner: Pid,
}

impl DirectoryLock {
    pub(super) fn acquire(file: File) -> io::Result<Self> {
        flock(&file, FlockOperation::NonBlockingLockExclusive)?;
        Ok(Self {
            file,
            held: true,
            owner: getpid(),
        })
    }

    pub(super) const fn file(&self) -> &File {
        &self.file
    }

    pub(super) fn release(&mut self) -> io::Result<()> {
        self.release_with(|file| flock(file, FlockOperation::Unlock).map_err(Into::into))
    }

    fn require_owner(&self) -> io::Result<()> {
        if self.owner != getpid() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                "directory lock belongs to a different process; refusing ownership of its shared description"));
        }
        Ok(())
    }

    pub(super) fn revalidate(&self) -> io::Result<()> {
        self.require_owner()?;
        if !self.held {
            return Err(io::Error::other("directory lock was already released"));
        }
        flock(&self.file, FlockOperation::NonBlockingLockExclusive)?;
        Ok(())
    }

    fn release_with(&mut self, unlock: impl FnOnce(&File) -> io::Result<()>) -> io::Result<()> {
        self.require_owner()?;
        if self.held {
            unlock(&self.file)?;
            self.held = false;
        }
        Ok(())
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        // An inherited guard is not the owner. A child must only close its
        // reference, never unlock the still-active parent's shared description.
        if self.owner != getpid() {
            return;
        }
        // Closing just this fd is insufficient: dup/fork references share the
        // lock, and CLOEXEC does not end the pre-exec fork window. Unlock the
        // owner's description explicitly, then File closes our descriptor.
        // Drop cannot report success/failure to a caller; normal operation
        // completion must use the fallible RunDirectories::release boundary.
        if let Err(error) = self.release() {
            tracing::error!(
                code = %DiagnosticCode::ProducerState,
                operation = "release-directory-lock",
                error = %error,
                "Fallback directory unlock failed; descriptor close cannot prove inherited references released the lock"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_non_owner_guard_cannot_unlock_the_parent_description() {
        let root = tempfile::tempdir().unwrap();
        let mut lock = DirectoryLock::acquire(File::open(root.path()).unwrap()).unwrap();
        let duplicate = lock.file().try_clone().unwrap();
        // Model the identity mismatch after fork without unsafe fork inside
        // Rust's multi-threaded test harness. The actual OFD stays shared.
        lock.owner = Pid::from_raw(if getpid() == Pid::INIT { 2 } else { 1 }).unwrap();
        assert_eq!(
            lock.revalidate().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            lock.release().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        drop(lock);
        let observer = File::open(root.path()).unwrap();
        assert_eq!(
            flock(&observer, FlockOperation::NonBlockingLockExclusive).unwrap_err(),
            rustix::io::Errno::WOULDBLOCK
        );
        flock(&duplicate, FlockOperation::Unlock).unwrap();
        flock(&observer, FlockOperation::NonBlockingLockExclusive).unwrap();
    }

    #[test]
    fn failed_unlock_is_reported_and_does_not_mark_the_lock_released() {
        let root = tempfile::tempdir().unwrap();
        let mut lock = DirectoryLock::acquire(File::open(root.path()).unwrap()).unwrap();
        let failure = lock.release_with(|_| Err(io::Error::other("synthetic unlock failure")));
        assert_eq!(failure.unwrap_err().to_string(), "synthetic unlock failure");
        let observer = File::open(root.path()).unwrap();
        assert_eq!(
            flock(&observer, FlockOperation::NonBlockingLockExclusive).unwrap_err(),
            rustix::io::Errno::WOULDBLOCK,
        );
        lock.release().unwrap();
        assert!(lock.revalidate().is_err());
        flock(&observer, FlockOperation::NonBlockingLockExclusive).unwrap();
        lock.release_with(|_| panic!("a released owner must not unlock twice"))
            .unwrap();
    }

    #[test]
    fn early_failure_keeps_drop_cleanup_before_marker_construction() {
        // A retained dup models the reference held by an in-flight fork.
        let root = tempfile::tempdir().unwrap();
        let file = File::open(root.path()).unwrap();
        let duplicate = file.try_clone().unwrap();
        let attempt = || -> io::Result<()> {
            let _lock = DirectoryLock::acquire(file)?;
            Err(io::Error::other("synthetic marker persistence failure"))
        };
        assert!(attempt().is_err());
        let observer = File::open(root.path()).unwrap();
        flock(&observer, FlockOperation::NonBlockingLockExclusive).unwrap();
        duplicate.metadata().unwrap();
    }
}
