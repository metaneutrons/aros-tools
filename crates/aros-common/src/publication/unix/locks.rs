//! No-follow advisory locks and directory creation.

use super::{open_parent, rfs, Component, ErrorKind, FlockOperation, Mode, OFlags, Path};

pub(in crate::publication) fn acquire_advisory_file_lock(
    path: &Path,
) -> std::io::Result<std::fs::File> {
    let parent = open_parent(path, true)?;
    let fd = rfs::openat(
        &parent.fd,
        Path::new(&parent.leaf),
        OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    if !rfs::FileType::from_raw_mode(rfs::fstat(&fd)?.st_mode).is_file() {
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
) -> std::io::Result<()> {
    rfs::flock(file, FlockOperation::NonBlockingLockExclusive).map_err(Into::into)
}

pub(in crate::publication) fn ensure_directory_nofollow(path: &Path) -> std::io::Result<()> {
    let absolute = path.to_path_buf();
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
                        rfs::mkdirat(&directory, Path::new(name), Mode::from_raw_mode(0o755))?;
                        rfs::fsync(&directory)?;
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
