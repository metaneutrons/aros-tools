//! Descriptor-relative, no-follow filesystem and CAS primitives.

use super::*;
pub(super) fn write_new_file(path: &Path, contents: &[u8]) -> std::io::Result<FileIdentity> {
    write_new_file_mode(path, contents, 0o644)
}

pub(super) fn write_new_file_mode(
    path: &Path,
    contents: &[u8],
    mode: u16,
) -> std::io::Result<FileIdentity> {
    // `rustix::fs::RawMode` is `u16` on Darwin and `u32` on Linux. The
    // conversion is intentionally target-dependent even though Clippy sees an
    // identity conversion on the current host.
    #[allow(
        clippy::useless_conversion,
        reason = "rustix RawMode width differs between supported Unix targets"
    )]
    let raw_mode = mode.into();
    let parent = open_parent(path, true)?;
    let fd = rfs::openat(
        &parent.fd,
        Path::new(&parent.leaf),
        OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(raw_mode),
    )?;
    let identity = identity_from_stat(&rfs::fstat(&fd)?);
    let mut file = std::fs::File::from(fd);
    let write = (|| {
        file.set_permissions(std::fs::Permissions::from_mode(u32::from(mode)))?;
        file.write_all(contents)?;
        file.flush()?;
        file.sync_all()?;
        if identity_from_stat(&rfs::fstat(&file)?) != identity {
            return Err(std::io::Error::other(
                "staged file identity changed while writing",
            ));
        }
        rfs::fsync(&parent.fd)?;
        let measured = read_regular_with_mode(path)?;
        if !measured
            .as_ref()
            .is_some_and(|(current, bytes, current_mode)| {
                *current == identity
                    && sha256_bytes(bytes) == sha256_bytes(contents)
                    && *current_mode == mode
            })
        {
            return Err(std::io::Error::other(format!(
                "staged file '{}' failed identity/digest/mode readback",
                path.display()
            )));
        }
        Ok::<(), std::io::Error>(())
    })();
    drop(file);
    if let Err(error) = write {
        let cleanup = remove_at(&parent, Some(identity))
            .err()
            .map_or_else(Vec::new, |cleanup| vec![cleanup.to_string()]);
        return Err(combine_primary_and_cleanup(error, &cleanup, true));
    }
    Ok(identity)
}

pub(super) fn remove_operation_aux_exact(
    operation: &JournalOperation,
    path: &Path,
    expected: FileIdentity,
    expected_digest: &Sha256Digest,
    expected_mode: Option<u16>,
) -> std::io::Result<()> {
    let parent = open_parent(path, false)?;
    if identity_from_stat(&rfs::fstat(&parent.fd)?) != operation.parent_identity {
        return Err(std::io::Error::other(format!(
            "publication parent identity changed for auxiliary '{}'",
            path.display()
        )));
    }
    remove_at_exact_mode(&parent, expected, expected_digest, expected_mode)
}

pub(super) fn remove_operation_aux_unidentified(
    operation: &JournalOperation,
    path: &Path,
) -> std::io::Result<()> {
    let parent = open_parent(path, false)?;
    if identity_from_stat(&rfs::fstat(&parent.fd)?) != operation.parent_identity {
        return Err(std::io::Error::other(format!(
            "publication parent identity changed for auxiliary '{}'",
            path.display()
        )));
    }
    let Some((identity, bytes)) = read_regular(path)? else {
        return Ok(());
    };
    remove_at_exact(&parent, identity, &sha256_bytes(&bytes))
}

fn remove_at(parent: &ParentHandle, expected: Option<FileIdentity>) -> std::io::Result<()> {
    let current = identity_at(parent, &parent.leaf)?;
    if current.is_none() {
        return Ok(());
    }
    if expected.is_some() && current != expected {
        return Err(std::io::Error::other(format!(
            "refusing to remove identity-mismatched file '{}'",
            parent.path.join(&parent.leaf).display()
        )));
    }
    rfs::unlinkat(&parent.fd, &parent.leaf, AtFlags::empty())?;
    rfs::fsync(&parent.fd)?;
    Ok(())
}

pub(super) fn remove_regular_exact(
    path: &Path,
    expected: FileIdentity,
    expected_digest: &Sha256Digest,
) -> std::io::Result<()> {
    let parent = open_parent(path, false)?;
    remove_at_exact(&parent, expected, expected_digest)
}

pub(super) fn remove_at_exact(
    parent: &ParentHandle,
    expected: FileIdentity,
    expected_digest: &Sha256Digest,
) -> std::io::Result<()> {
    remove_at_exact_mode(parent, expected, expected_digest, None)
}

pub(super) fn remove_at_exact_mode(
    parent: &ParentHandle,
    expected: FileIdentity,
    expected_digest: &Sha256Digest,
    expected_mode: Option<u16>,
) -> std::io::Result<()> {
    let path = parent.path.join(&parent.leaf);
    let current = read_regular_with_mode(&path)?;
    let Some((identity, bytes, mode)) = current else {
        return Ok(());
    };
    if identity != expected
        || &sha256_bytes(&bytes) != expected_digest
        || expected_mode.is_some_and(|expected| expected != mode)
    {
        return Err(std::io::Error::other(format!(
            "refusing to remove identity/digest/mode-mismatched file '{}'",
            path.display()
        )));
    }
    rfs::unlinkat(&parent.fd, &parent.leaf, AtFlags::empty())?;
    rfs::fsync(&parent.fd)?;
    Ok(())
}

pub(super) fn identity_at(
    parent: &ParentHandle,
    leaf: &OsStr,
) -> std::io::Result<Option<FileIdentity>> {
    identity_at_fd(&parent.fd, leaf).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "cannot identify publication target '{}': {error}",
                parent.path.join(leaf).display()
            ),
        )
    })
}

pub(super) fn identity_at_fd(
    parent: &impl std::os::fd::AsFd,
    leaf: &OsStr,
) -> std::io::Result<Option<FileIdentity>> {
    match rfs::statat(parent, Path::new(leaf), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if rfs::FileType::from_raw_mode(stat.st_mode).is_file() => {
            Ok(Some(identity_from_stat(&stat)))
        }
        Ok(_) => Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "publication target is not a regular file",
        )),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[allow(clippy::cast_sign_loss, clippy::unnecessary_cast)]
pub(super) const fn identity_from_stat(stat: &rfs::Stat) -> FileIdentity {
    FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
    }
}

pub(super) fn rename_noclobber(
    source_parent: &ParentHandle,
    source: &OsStr,
    target_parent: &ParentHandle,
    target: &OsStr,
) -> std::io::Result<()> {
    rfs::renameat_with(
        &source_parent.fd,
        Path::new(source),
        &target_parent.fd,
        Path::new(target),
        RenameFlags::NOREPLACE,
    )?;
    Ok(())
}

pub(super) fn open_parent(path: &Path, create: bool) -> std::io::Result<ParentHandle> {
    let absolute = platform_absolute_path(path)?;
    let leaf = absolute.file_name().ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("'{}' has no file name", absolute.display()),
        )
    })?;
    let parent_path = absolute.parent().ok_or_else(|| {
        std::io::Error::new(ErrorKind::InvalidInput, "publication path has no parent")
    })?;
    let mut fd = rfs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut traversed = PathBuf::from("/");
    for component in parent_path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let next = match rfs::openat(
                    &fd,
                    Path::new(name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(next) => next,
                    Err(rustix::io::Errno::NOENT) if create => {
                        rfs::mkdirat(&fd, Path::new(name), Mode::from_raw_mode(0o755))?;
                        rfs::fsync(&fd)?;
                        rfs::openat(
                            &fd,
                            Path::new(name),
                            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                        )?
                    }
                    Err(error) => return Err(error.into()),
                };
                traversed.push(name);
                fd = next;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "publication path '{}' is not normalized",
                        absolute.display()
                    ),
                ));
            }
        }
    }
    let path_stat = rfs::fstat(&fd)?;
    let reopened = rfs::open(
        &traversed,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let reopened_stat = rfs::fstat(&reopened)?;
    if identity_from_stat(&path_stat) != identity_from_stat(&reopened_stat) {
        return Err(std::io::Error::other(format!(
            "publication parent changed during no-follow traversal: '{}'",
            parent_path.display()
        )));
    }
    Ok(ParentHandle {
        fd,
        leaf: leaf.to_os_string(),
        path: parent_path.to_path_buf(),
    })
}

fn platform_absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    absolute_path(path)
}

pub(super) fn sibling_name(leaf: &OsStr, purpose: &str, nonce: u64) -> OsString {
    let digest = sha256_bytes(leaf.as_bytes()).to_string();
    OsString::from(format!(".aros-{purpose}-{}-{nonce:016x}", &digest[..16]))
}
