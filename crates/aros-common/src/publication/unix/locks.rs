//! No-follow advisory locks and directory creation.

use super::{
    absolute_path, identity_from_stat, open_parent, rfs, AdvisoryLockObservation,
    AdvisoryLockState, Component, ErrorKind, FileIdentity, FlockOperation, Mode, OFlags, Ordering,
    Path, PathBuf, TRANSACTION_SEQUENCE,
};
use std::ffi::OsString;

pub(in crate::publication) fn acquire_advisory_file_lock(
    path: &Path,
) -> std::io::Result<std::fs::File> {
    let parent = open_parent(path, true).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "cannot prepare advisory-lock parent for '{}': {error}",
                path.display()
            ),
        )
    })?;
    let lock_flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let fd = match rfs::openat(
        &parent.fd,
        Path::new(&parent.leaf),
        lock_flags | OFlags::CREATE | OFlags::EXCL,
        Mode::from_raw_mode(0o600),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::EXIST) => rfs::openat(
            &parent.fd,
            Path::new(&parent.leaf),
            lock_flags,
            Mode::empty(),
        )?,
        Err(error) => return Err(error.into()),
    };
    let stat = rfs::fstat(&fd)?;
    if !rfs::FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("advisory lock '{}' is not a regular file", path.display()),
        ));
    }
    rfs::flock(&fd, FlockOperation::NonBlockingLockExclusive)?;
    Ok(std::fs::File::from(fd))
}

pub(in crate::publication) fn revalidate_advisory_file_lock(
    file: &std::fs::File,
    path: &Path,
    expected_identity: FileIdentity,
) -> std::io::Result<()> {
    if advisory_file_lock_identity(file)? != expected_identity {
        return Err(std::io::Error::other(
            "held advisory lock descriptor identity changed",
        ));
    }
    let parent = open_parent(path, false)?;
    let stat = rfs::statat(
        &parent.fd,
        Path::new(&parent.leaf),
        rfs::AtFlags::SYMLINK_NOFOLLOW,
    )?;
    if !rfs::FileType::from_raw_mode(stat.st_mode).is_file()
        || identity_from_stat(&stat) != expected_identity
    {
        return Err(std::io::Error::other(format!(
            "advisory lock '{}' was replaced while held",
            path.display()
        )));
    }
    rfs::flock(file, FlockOperation::NonBlockingLockExclusive).map_err(Into::into)
}

pub(in crate::publication) fn advisory_file_lock_identity(
    file: &std::fs::File,
) -> std::io::Result<FileIdentity> {
    let stat = rfs::fstat(file)?;
    if !rfs::FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "advisory lock descriptor is not a regular file",
        ));
    }
    Ok(identity_from_stat(&stat))
}

pub(in crate::publication) fn probe_advisory_file_lock(
    path: &Path,
) -> std::io::Result<AdvisoryLockObservation> {
    let parent = match open_parent(path, false) {
        Ok(parent) => parent,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(AdvisoryLockObservation {
                identity: None,
                state: AdvisoryLockState::Absent,
            });
        }
        Err(error) => return Err(error),
    };
    let fd = match rfs::openat(
        &parent.fd,
        Path::new(&parent.leaf),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(AdvisoryLockObservation {
                identity: None,
                state: AdvisoryLockState::Absent,
            });
        }
        Err(error) => return Err(error.into()),
    };
    let stat = rfs::fstat(&fd)?;
    if !rfs::FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("advisory lock '{}' is not a regular file", path.display()),
        ));
    }
    let identity = identity_from_stat(&stat);
    if identity_from_stat(&rfs::fstat(&fd)?) != identity {
        return Err(std::io::Error::other(format!(
            "advisory lock '{}' changed during inspection",
            path.display()
        )));
    }
    match rfs::flock(&fd, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(AdvisoryLockObservation {
            identity: Some(identity),
            state: AdvisoryLockState::Unheld,
        }),
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(AdvisoryLockObservation {
            identity: Some(identity),
            state: AdvisoryLockState::Held,
        }),
        Err(error) => Err(error.into()),
    }
}

pub(in crate::publication) fn ensure_directory_nofollow(path: &Path) -> std::io::Result<()> {
    let absolute = absolute_path(path)?;
    let mut directory = rfs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for component in absolute.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let child = match rfs::openat(
                    &directory,
                    Path::new(name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(child) => child,
                    Err(rustix::io::Errno::NOENT) => {
                        match rfs::mkdirat(&directory, Path::new(name), Mode::from_raw_mode(0o755))
                        {
                            Ok(()) => rfs::fsync(&directory)?,
                            // A cooperating creator won the race. Re-open
                            // below through the same no-follow parent fd.
                            Err(rustix::io::Errno::EXIST) => {}
                            Err(error) => return Err(error.into()),
                        }
                        rfs::openat(
                            &directory,
                            Path::new(name),
                            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                        )?
                    }
                    Err(error) => return Err(error.into()),
                };
                directory = child;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "directory path '{}' is not absolute and normalized",
                        absolute.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Validate the existing portion of a path without following a symlink.
///
/// This is the read-only counterpart of [`ensure_directory_nofollow`].  It
/// accepts missing trailing components so a preview can remain non-mutating.
pub(in crate::publication) fn validate_existing_directory_prefix_nofollow(
    path: &Path,
) -> std::io::Result<()> {
    let absolute = path.to_path_buf();
    let mut directory = rfs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for component in absolute.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => match rfs::openat(
                &directory,
                Path::new(name),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(child) => directory = child,
                Err(rustix::io::Errno::NOENT) => return Ok(()),
                Err(error) => return Err(error.into()),
            },
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "directory path '{}' is not absolute and normalized",
                        absolute.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Allocate one empty staging directory through a no-follow parent descriptor.
/// The directory is readable and traversable after an atomic publication, so
/// its mode is the portable public-directory mode `0755`. The caller must
/// continue with descriptor-relative operations on the returned path, because
/// an untrusted namespace can change after this function returns.
pub(in crate::publication) fn create_unique_directory_nofollow(
    parent: &Path,
    prefix: &str,
) -> std::io::Result<PathBuf> {
    let sentinel = parent.join(".aros-publication-parent");
    let parent = open_parent(&sentinel, true)?;
    let process = std::process::id();
    let sequence = TRANSACTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);

    for attempt in 0..100_u16 {
        let name = OsString::from(format!("{prefix}-{process}-{sequence}-{attempt}"));
        match rfs::mkdirat(&parent.fd, Path::new(&name), Mode::from_raw_mode(0o755)) {
            Ok(()) => {
                rfs::fsync(&parent.fd)?;
                let stage = rfs::openat(
                    &parent.fd,
                    Path::new(&name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                let _identity = identity_from_stat(&rfs::fstat(&stage)?);
                return Ok(parent.path.join(name));
            }
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(error.into()),
        }
    }

    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        format!(
            "could not allocate a unique directory with prefix '{prefix}' below '{}'",
            parent.path.display()
        ),
    ))
}
