//! Descriptor-stable reads for regular publication control-plane files.

use super::{
    identity_from_stat, open_parent, rfs, test_pause_point,
    tree::{validate_private_ancestor_chain, validate_private_regular_file},
    ErrorKind, FileIdentity, Mode, OFlags, Path,
};
use crate::digest::{sha256_bytes, Sha256Digest};

pub(in crate::publication) fn read_regular_bounded(
    path: &Path,
    max_bytes: u64,
) -> std::io::Result<Option<(FileIdentity, Vec<u8>)>> {
    read_regular_with_mode_bounded(path, max_bytes)
        .map(|snapshot| snapshot.map(|(identity, bytes, _mode)| (identity, bytes)))
}

pub(in crate::publication) fn read_regular(
    path: &Path,
) -> std::io::Result<Option<(FileIdentity, Vec<u8>)>> {
    read_regular_bounded(path, u64::MAX)
}

pub(in crate::publication) fn open_regular_file_nofollow(
    path: &Path,
) -> std::io::Result<std::fs::File> {
    let parent = open_parent(path, false)?;
    let fd = rfs::openat(
        &parent.fd,
        Path::new(&parent.leaf),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let stat = rfs::fstat(&fd)?;
    if !rfs::FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "publication target '{}' is not a regular file",
                path.display()
            ),
        ));
    }
    Ok(std::fs::File::from(fd))
}

/// Remove one regular file only when its identity and bytes still match a
/// caller-measured snapshot.
///
/// The final unlink is descriptor-relative and preceded by a second no-follow
/// pathname revalidation. As with tree removal, the target must be below a
/// private Unix namespace because POSIX cannot bind unlink to an open inode.
pub(in crate::publication) fn remove_regular_file_from_snapshot_nofollow(
    path: &Path,
    expected_identity: FileIdentity,
    expected_sha256: &Sha256Digest,
    expected_size: u64,
    max_bytes: u64,
) -> std::io::Result<()> {
    if expected_size > max_bytes {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "regular-file removal snapshot exceeds its explicit byte limit",
        ));
    }
    let parent = open_parent(path, false)?;
    let fd = rfs::openat(
        &parent.fd,
        Path::new(&parent.leaf),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let before = rfs::fstat(&fd)?;
    if !rfs::FileType::from_raw_mode(before.st_mode).is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("removal target '{}' is not a regular file", path.display()),
        ));
    }
    if identity_from_stat(&before) != expected_identity
        || before.st_size < 0
        || u64::try_from(before.st_size).ok() != Some(expected_size)
    {
        return Err(std::io::Error::other(format!(
            "refusing to remove identity-mismatched regular file '{}'",
            path.display()
        )));
    }
    validate_private_ancestor_chain(path)?;
    validate_private_regular_file(&before, path)?;

    let mut file = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    let read_limit = max_bytes.saturating_add(1);
    let mut input = std::io::Read::take(&mut file, read_limit);
    std::io::Read::read_to_end(&mut input, &mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_size
        || sha256_bytes(&bytes) != *expected_sha256
    {
        return Err(std::io::Error::other(format!(
            "refusing to remove changed regular file '{}'",
            path.display()
        )));
    }
    test_pause_point("regular-remove-before-final-stat");
    let after = rfs::fstat(&file)?;
    if !same_regular_snapshot(&before, &after, bytes.len()) {
        return Err(std::io::Error::other(format!(
            "regular file '{}' changed while its removal snapshot was read",
            path.display()
        )));
    }
    let current = rfs::statat(
        &parent.fd,
        Path::new(&parent.leaf),
        rfs::AtFlags::SYMLINK_NOFOLLOW,
    )?;
    if !rfs::FileType::from_raw_mode(current.st_mode).is_file()
        || !same_regular_snapshot(&before, &current, bytes.len())
    {
        return Err(std::io::Error::other(format!(
            "regular file '{}' changed before final removal",
            path.display()
        )));
    }
    // POSIX cannot bind unlink to the open descriptor. Exercise the final
    // pathname revalidation directly in the adversarial test suite.
    #[cfg(test)]
    crate::publication::publication_tests::run_boundary("regular-remove-before-final-unlink", path);
    let current = rfs::statat(
        &parent.fd,
        Path::new(&parent.leaf),
        rfs::AtFlags::SYMLINK_NOFOLLOW,
    )?;
    if !rfs::FileType::from_raw_mode(current.st_mode).is_file()
        || !same_regular_snapshot(&before, &current, bytes.len())
    {
        return Err(std::io::Error::other(format!(
            "regular file '{}' changed immediately before final removal",
            path.display()
        )));
    }
    rfs::unlinkat(&parent.fd, Path::new(&parent.leaf), rfs::AtFlags::empty())?;
    rfs::fsync(&parent.fd)?;
    Ok(())
}

pub(in crate::publication) fn read_regular_with_mode(
    path: &Path,
) -> std::io::Result<Option<(FileIdentity, Vec<u8>, u16)>> {
    read_regular_with_mode_bounded(path, u64::MAX)
}

fn read_regular_with_mode_bounded(
    path: &Path,
    max_bytes: u64,
) -> std::io::Result<Option<(FileIdentity, Vec<u8>, u16)>> {
    let parent = match open_parent(path, false) {
        Ok(parent) => parent,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let fd = match rfs::openat(
        &parent.fd,
        Path::new(&parent.leaf),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let stat = rfs::fstat(&fd)?;
    if !rfs::FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "publication target '{}' is not a regular file",
                path.display()
            ),
        ));
    }
    let identity = identity_from_stat(&stat);
    let mode = permission_mode_from_stat(&stat);
    let mut file = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    let read_limit = max_bytes.saturating_add(1);
    let mut input = std::io::Read::take(&mut file, read_limit);
    std::io::Read::read_to_end(&mut input, &mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "publication target '{}' exceeds the {max_bytes}-byte read limit",
                path.display()
            ),
        ));
    }
    test_pause_point("read-before-final-stat");
    let final_stat = rfs::fstat(&file)?;
    if !same_regular_snapshot(&stat, &final_stat, bytes.len()) {
        return Err(std::io::Error::other(format!(
            "publication target changed or was written concurrently while reading: '{}'",
            path.display()
        )));
    }
    Ok(Some((identity, bytes, mode)))
}

#[allow(clippy::cast_sign_loss)]
pub(super) fn same_regular_snapshot(before: &rfs::Stat, after: &rfs::Stat, bytes: usize) -> bool {
    identity_from_stat(before) == identity_from_stat(after)
        && before.st_size >= 0
        && usize::try_from(before.st_size).ok() == Some(bytes)
        && before.st_size == after.st_size
        && before.st_mtime == after.st_mtime
        && before.st_mtime_nsec == after.st_mtime_nsec
        && before.st_ctime == after.st_ctime
        && before.st_ctime_nsec == after.st_ctime_nsec
        && before.st_mode == after.st_mode
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::unnecessary_cast,
    reason = "rustix mode_t width differs between supported Unix targets"
)]
const fn permission_mode_from_stat(stat: &rfs::Stat) -> u16 {
    (stat.st_mode as u32 & 0o7777) as u16
}
