//! Descriptor-relative Unix implementation; no recursive removal or adoption.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use aros_common::Sha256Digest;
use rustix::fs::{self as fs, AtFlags, Mode, OFlags};

use super::{directory_lock::DirectoryLock, state_io};
use crate::filesystem::{open_directory, DIRECTORY};
use crate::ContractError;

const MARKER: &str = ".aros-toolchain-owner-v1.json";

#[derive(Debug)]
pub(super) struct Parent {
    path: PathBuf,
    leaf: OsString,
    file: File,
    identity: (u64, u64),
}

impl Parent {
    pub(super) fn inspect(root: &Path) -> Result<Self, ContractError> {
        let path = root
            .parent()
            .ok_or_else(|| ContractError::state("work root has no parent"))?;
        let leaf = root
            .file_name()
            .ok_or_else(|| ContractError::state("work root has no leaf"))?;
        let file =
            open_directory(path).map_err(|error| state_io("open existing parent", path, &error))?;
        let identity = identity(&file).map_err(|error| state_io("inspect parent", path, &error))?;
        let result = Self {
            path: path.to_owned(),
            leaf: leaf.to_owned(),
            file,
            identity,
        };
        result.require_absent()?;
        Ok(result)
    }

    fn require_absent(&self) -> Result<(), ContractError> {
        match fs::statat(&self.file, &self.leaf, AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => Ok(()),
            Ok(_) => Err(ContractError::state(format!(
                "refusing to adopt existing work/output root '{}'",
                self.path.join(&self.leaf).display()
            ))),
            Err(error) => Err(state_io(
                "inspect proposed root",
                &self.path.join(&self.leaf),
                &error.into(),
            )),
        }
    }

    fn revalidate(&self) -> Result<(), ContractError> {
        let current = open_directory(&self.path)
            .map_err(|error| state_io("reopen parent", &self.path, &error))?;
        if identity(&current)
            .map_err(|error| state_io("inspect reopened parent", &self.path, &error))?
            != self.identity
        {
            return Err(ContractError::state("work/output parent identity changed"));
        }
        Ok(())
    }

    pub(super) fn reserve(
        self,
        owner: &Sha256Digest,
        role: &str,
    ) -> Result<OwnedDirectory, ContractError> {
        self.revalidate()?;
        self.require_absent()?;
        let path = self.path.join(&self.leaf);
        fs::mkdirat(&self.file, &self.leaf, Mode::RUSR | Mode::WUSR | Mode::XUSR)
            .map_err(|error| state_io("reserve fresh directory", &path, &error.into()))?;
        // From this point failures retain the reserved root for inspection.
        let file = File::from(
            fs::openat(&self.file, &self.leaf, DIRECTORY, Mode::empty())
                .map_err(|error| state_io("open reserved directory", &path, &error.into()))?,
        );
        let lock = DirectoryLock::acquire(file)
            .map_err(|error| state_io("lock reserved directory", &path, &error))?;
        let file = lock.file();
        let root_identity = identity(file)
            .map_err(|error| state_io("inspect reserved directory", &path, &error))?;
        let record = format!("{{\"schema\":\"aros-toolchain-owner-v1\",\"role\":\"{role}\",\"owner_sha256\":\"{owner}\"}}\n").into_bytes();
        let mut marker = File::from(
            fs::openat(
                file,
                MARKER,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map_err(|error| state_io("create ownership marker", &path, &error.into()))?,
        );
        let marker_identity = identity(&marker)
            .map_err(|error| state_io("inspect ownership marker", &path, &error))?;
        marker
            .write_all(&record)
            .and_then(|()| marker.sync_all())
            .map_err(|error| state_io("persist ownership marker", &path, &error))?;
        file.sync_all()
            .and_then(|()| self.file.sync_all())
            .map_err(|error| state_io("persist reserved directory", &path, &error))?;
        let reservation = OwnedDirectory {
            parent: self,
            lock,
            identity: root_identity,
            marker_identity,
            record,
        };
        reservation.revalidate()?;
        Ok(reservation)
    }
}

#[derive(Debug)]
pub(super) struct OwnedDirectory {
    parent: Parent,
    lock: DirectoryLock,
    identity: (u64, u64),
    marker_identity: (u64, u64),
    record: Vec<u8>,
}

impl OwnedDirectory {
    pub(super) const fn file(&self) -> &File {
        self.lock.file()
    }

    pub(super) fn release(&mut self) -> io::Result<()> {
        self.lock.release()
    }

    pub(super) fn revalidate(&self) -> Result<(), ContractError> {
        self.parent.revalidate()?;
        let path = self.parent.path.join(&self.parent.leaf);
        let current = File::from(
            fs::openat(
                &self.parent.file,
                &self.parent.leaf,
                DIRECTORY,
                Mode::empty(),
            )
            .map_err(|error| state_io("reopen reserved root", &path, &error.into()))?,
        );
        if identity(&current).map_err(|error| state_io("inspect reserved root", &path, &error))?
            != self.identity
        {
            return Err(ContractError::state("reserved directory identity changed"));
        }
        if current
            .metadata()
            .map_err(|error| state_io("inspect root permissions", &path, &error))?
            .mode()
            & 0o077
            != 0
        {
            return Err(ContractError::state(
                "reserved directory is no longer private",
            ));
        }
        self.lock
            .revalidate()
            .map_err(|error| state_io("revalidate directory lock", &path, &error))?;
        let marker = File::from(
            fs::openat(
                self.lock.file(),
                MARKER,
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| state_io("reopen ownership marker", &path, &error.into()))?,
        );
        let metadata = marker
            .metadata()
            .map_err(|error| state_io("inspect ownership marker", &path, &error))?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || (metadata.dev(), metadata.ino()) != self.marker_identity
        {
            return Err(ContractError::state(
                "ownership marker type/permissions/link count/identity changed",
            ));
        }
        let mut bytes = Vec::new();
        marker
            .take(self.record.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| state_io("read ownership marker", &path, &error))?;
        if bytes != self.record {
            return Err(ContractError::state("ownership marker bytes changed"));
        }
        Ok(())
    }
}

fn identity(file: &File) -> io::Result<(u64, u64)> {
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::FlockOperation;

    #[test]
    fn dropping_owner_unlocks_even_while_a_duplicated_description_survives() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("work");
        let guard = Parent::inspect(&path)
            .unwrap()
            .reserve(&aros_common::sha256_bytes(b"fixture"), "work")
            .unwrap();
        // dup and fork share the same open file description. CLOEXEC only
        // closes inherited descriptors at exec, not during the fork window.
        let duplicate = guard.lock.file().try_clone().unwrap();
        assert!(rustix::io::fcntl_getfd(&duplicate)
            .unwrap()
            .contains(rustix::io::FdFlags::CLOEXEC));
        let independent = File::open(&path).unwrap();
        assert_eq!(
            fs::flock(&independent, FlockOperation::NonBlockingLockExclusive).unwrap_err(),
            rustix::io::Errno::WOULDBLOCK,
        );
        drop(guard);
        fs::flock(&independent, FlockOperation::NonBlockingLockExclusive).unwrap();
        // The duplicated reference is intentionally still alive here.
        duplicate.metadata().unwrap();
    }

    #[test]
    fn parent_replaced_between_inspection_and_reservation_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let parent = root.join("parent");
        std::fs::create_dir(&parent).unwrap();
        let candidate = Parent::inspect(&parent.join("work")).unwrap();
        std::fs::rename(&parent, root.join("retained")).unwrap();
        std::fs::create_dir(&parent).unwrap();
        let result = candidate.reserve(&aros_common::sha256_bytes(b"fixture"), "work");
        assert!(result.is_err());
        assert!(!parent.join("work").exists());
        assert!(!root.join("retained/work").exists());
    }

    #[test]
    fn second_root_race_retains_the_first_reservation_and_foreign_data() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let work = Parent::inspect(&root.join("work")).unwrap();
        let output = Parent::inspect(&root.join("output")).unwrap();
        let binding = aros_common::sha256_bytes(b"fixture");
        let guard = work.reserve(&binding, "work").unwrap();
        std::fs::create_dir(root.join("output")).unwrap();
        std::fs::write(root.join("output/foreign"), b"keep").unwrap();
        assert!(output.reserve(&binding, "output").is_err());
        drop(guard);
        assert!(root.join("work").join(MARKER).is_file());
        assert_eq!(std::fs::read(root.join("output/foreign")).unwrap(), b"keep");
    }
}
