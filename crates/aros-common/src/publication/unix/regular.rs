//! Descriptor-stable reads for regular publication control-plane files.

use super::{
    identity_from_stat, open_parent, rfs, test_pause_point, ErrorKind, FileIdentity, Mode, OFlags,
    Path,
};

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
