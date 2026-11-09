//! Bounded local file access for caller-selected inputs and private outputs.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use aros_common::measure_regular_file_bounded;

use super::{fail, input, require, Result};

/// Absolute, normalized form without touching the file system (Python
/// `os.path.abspath`): relative paths join the working directory, `.` and
/// `..` components are folded lexically.
///
/// # Errors
///
/// Fails when the working directory is unavailable.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    let base = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().or_else(|_| input("working directory is unavailable"))?
    };
    let mut result = base;
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}

/// Read one bounded, singly-linked, non-symlink regular file as one stable
/// snapshot.
///
/// # Errors
///
/// Fails for unsafe, oversized, unreadable or concurrently changed files.
pub fn read_regular(path: &Path, label: &str, maximum: usize) -> Result<Vec<u8>> {
    match measure_regular_file_bounded(path, maximum as u64) {
        Ok(Some((_, bytes))) if bytes.len() <= maximum => Ok(bytes),
        _ => input(format!("{label} cannot be read as a bounded regular file")),
    }
}

/// Require an existing caller-owned mode-0700 directory as the parent.
///
/// # Errors
///
/// Fails when the parent is missing, not a directory, not owned by the caller
/// or not private.
#[cfg(unix)]
pub fn require_private_parent(path: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let Some(parent) = path.parent() else {
        return fail("output parent is unavailable");
    };
    let Ok(metadata) = std::fs::symlink_metadata(parent) else {
        return fail("output parent is unavailable");
    };
    let mine = rustix::process::getuid().as_raw();
    require(
        metadata.is_dir()
            && metadata.uid() == mine
            && metadata.permissions().mode() & 0o7777 == 0o700,
        "output parent must be an existing caller-owned mode-0700 directory",
    )
}

#[cfg(not(unix))]
pub fn require_private_parent(_: &Path) -> Result<()> {
    fail("private outputs require a Unix host")
}

/// Write `content` exclusively (never replacing) with mode 0600, then read it
/// back and require exact bytes, one link and the private mode.
///
/// # Errors
///
/// Fails when the destination exists or the readback differs.
#[cfg(unix)]
pub fn write_exclusive(path: &Path, content: &[u8], label: &str) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits().cast_signed())
        .open(path);
    let Ok(mut handle) = created else {
        return input(format!("{label} cannot be created exclusively"));
    };
    if handle
        .write_all(content)
        .and_then(|()| handle.sync_all())
        .is_err()
    {
        return input(format!("{label} cannot be written"));
    }
    drop(handle);
    let actual = read_regular(path, label, content.len().max(1))?;
    let metadata = std::fs::symlink_metadata(path).or_else(|_| input("readback unavailable"))?;
    require(
        actual == content
            && metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == rustix::process::getuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o600,
        &format!("{label} readback differs from the exact private output"),
    )
}

#[cfg(not(unix))]
pub fn write_exclusive(_: &Path, _: &[u8], _: &str) -> Result<()> {
    fail("private outputs require a Unix host")
}

/// Require that an output destination does not exist yet (and is no symlink).
///
/// # Errors
///
/// Fails when anything already occupies the path.
pub fn require_absent(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => fail("output destination already exists or is unavailable"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{symlink, PermissionsExt};

    use super::*;

    fn private_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }

    #[test]
    fn reads_are_bounded_and_refuse_links() {
        let directory = private_directory();
        let file = directory.path().join("a");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(read_regular(&file, "x", 3).unwrap(), b"abc");
        assert!(read_regular(&file, "x", 2).is_err());
        let link = directory.path().join("l");
        symlink(&file, &link).unwrap();
        assert!(read_regular(&link, "x", 8).is_err());
        let hard = directory.path().join("h");
        std::fs::hard_link(&file, &hard).unwrap();
        assert!(read_regular(&file, "x", 8).is_err() || read_regular(&hard, "x", 8).is_ok());
        assert!(read_regular(&directory.path().join("missing"), "x", 8).is_err());
        assert!(read_regular(directory.path(), "x", 8).is_err());
    }

    #[test]
    fn exclusive_writes_never_replace_and_need_a_private_parent() {
        let directory = private_directory();
        let target = directory.path().join("out");
        require_private_parent(&target).unwrap();
        require_absent(&target).unwrap();
        write_exclusive(&target, b"one", "out").unwrap();
        assert!(write_exclusive(&target, b"two", "out").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"one");
        assert!(require_absent(&target).is_err());
        let open = tempfile::tempdir().unwrap();
        std::fs::set_permissions(open.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(require_private_parent(&open.path().join("x")).is_err());
        assert!(require_private_parent(&directory.path().join("nodir/x")).is_err());
    }

    #[test]
    fn absolute_folds_dots_lexically() {
        assert_eq!(
            absolute(Path::new("/a/b/../c/./d")).unwrap(),
            PathBuf::from("/a/c/d")
        );
    }
}
