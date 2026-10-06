//! A contained TAR hard link becomes an independently budgeted regular copy.
//! No inode aliasing, symlink traversal or archive-order-dependent chain follows.

use std::fs::{self, FileTimes, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::{budget::ExtractionBudget, extraction_failure, FetchResult};

fn real_path(root: &Path, relative: &Path, archive: &str) -> FetchResult<PathBuf> {
    let mut path = root.to_path_buf();
    for component in relative.components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            extraction_failure(
                archive,
                format!(
                    "TAR hard link target '{}' is missing: {error}",
                    relative.display()
                ),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(extraction_failure(
                archive,
                "TAR hard link target traverses a symlink",
            ));
        }
    }
    Ok(path)
}

pub(super) fn materialize(
    root: &Path,
    links: &[(PathBuf, PathBuf)],
    budget: &mut ExtractionBudget,
    archive: &str,
) -> FetchResult<()> {
    // Resolve all targets before copying any links: a hardlink-to-hardlink
    // chain is not an independently present regular source and is rejected.
    let mut copies = Vec::new();
    for (relative, target) in links {
        let source = real_path(root, target, archive)?;
        let metadata = fs::symlink_metadata(&source).map_err(|error| {
            extraction_failure(
                archive,
                format!("cannot inspect TAR hard link target: {error}"),
            )
        })?;
        if !metadata.is_file() {
            return Err(extraction_failure(
                archive,
                "TAR hard link target is not a regular file",
            ));
        }
        let output = root.join(relative);
        if fs::symlink_metadata(&output).is_ok() {
            return Err(extraction_failure(
                archive,
                "TAR hard link collides with an extracted entry",
            ));
        }
        let parent = relative
            .parent()
            .ok_or_else(|| extraction_failure(archive, "TAR hard link has no parent"))?;
        let mut directory = root.to_path_buf();
        for component in parent.components() {
            directory.push(component);
            match fs::symlink_metadata(&directory) {
                Ok(value) if value.is_dir() && !value.file_type().is_symlink() => {}
                Ok(_) => {
                    return Err(extraction_failure(
                        archive,
                        "TAR hard link parent is not a real directory",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&directory).map_err(|error| {
                        extraction_failure(
                            archive,
                            format!("cannot create TAR hard link parent: {error}"),
                        )
                    })?;
                }
                Err(error) => {
                    return Err(extraction_failure(
                        archive,
                        format!("cannot inspect TAR hard link parent: {error}"),
                    ));
                }
            }
        }
        // The header was counted once by unpack_tar; add logical bytes
        // without counting a second archive entry.
        budget.account_expanded_size(metadata.len(), archive, relative)?;
        copies.push((source, output, metadata));
    }
    for (source, output, metadata) in copies {
        with_readable_source(&source, &metadata.permissions(), archive, || {
            let mut input = fs::File::open(&source).map_err(|error| {
                extraction_failure(
                    archive,
                    format!("cannot read TAR hard link payload: {error}"),
                )
            })?;
            let mut destination = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|error| {
                    extraction_failure(
                        archive,
                        format!("cannot create TAR hard link copy: {error}"),
                    )
                })?;
            let transferred = io::copy(
                &mut input.by_ref().take(metadata.len().saturating_add(1)),
                &mut destination,
            )
            .map_err(|error| {
                extraction_failure(
                    archive,
                    format!("cannot copy TAR hard link payload: {error}"),
                )
            })?;
            if transferred != metadata.len() {
                return Err(extraction_failure(
                    archive,
                    "TAR hard link payload size changed",
                ));
            }
            let modified = metadata.modified().map_err(|error| {
                extraction_failure(
                    archive,
                    format!("cannot read TAR hard link target timestamp: {error}"),
                )
            })?;
            destination
                .set_times(FileTimes::new().set_modified(modified))
                .map_err(|error| {
                    extraction_failure(
                        archive,
                        format!("cannot preserve TAR hard link timestamp: {error}"),
                    )
                })?;
            destination
                .set_permissions(metadata.permissions())
                .map_err(|error| {
                    extraction_failure(
                        archive,
                        format!("cannot set TAR hard link permissions: {error}"),
                    )
                })?;
            Ok(())
        })?;
    }
    Ok(())
}

fn with_readable_source<T>(
    source: &Path,
    original_permissions: &fs::Permissions,
    archive: &str,
    operation: impl FnOnce() -> FetchResult<T>,
) -> FetchResult<T> {
    let mut guard = ReadabilityGuard::make(source, original_permissions).map_err(|error| {
        extraction_failure(
            archive,
            format!("cannot temporarily make TAR hard link target readable: {error}"),
        )
    })?;
    let result = operation();
    match guard.restore() {
        Ok(()) => result,
        Err(restore_error) => Err(extraction_failure(
            archive,
            match result {
                Ok(_) => {
                    format!("cannot restore TAR hard link target permissions: {restore_error}")
                }
                Err(operation_error) => format!(
                    "cannot restore TAR hard link target permissions after copy failed ({operation_error}): {restore_error}"
                ),
            },
        )),
    }
}

struct ReadabilityGuard {
    path: PathBuf,
    original_permissions: Option<fs::Permissions>,
}

impl ReadabilityGuard {
    fn make(path: &Path, original_permissions: &fs::Permissions) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = original_permissions.mode();
            if mode & 0o400 == 0 {
                let guard = Self {
                    path: path.to_path_buf(),
                    original_permissions: Some(original_permissions.clone()),
                };
                fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o400))?;
                return Ok(guard);
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
            original_permissions: None,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        if let Some(original) = &self.original_permissions {
            fs::set_permissions(&self.path, original.clone())?;
            self.original_permissions = None;
        }
        Ok(())
    }
}

impl Drop for ReadabilityGuard {
    fn drop(&mut self) {
        if let Some(original) = self.original_permissions.take() {
            let _ = fs::set_permissions(&self.path, original);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_link_logical_expansion_is_budgeted_before_copy() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("value"), b"four").unwrap();
        let mut budget = ExtractionBudget {
            entries: 2,
            expanded_bytes: super::super::budget::MAX_ARCHIVE_EXPANDED_BYTES - 2,
        };
        assert!(materialize(
            root.path(),
            &[("alias".into(), "value".into())],
            &mut budget,
            "fixture.tar"
        )
        .is_err());
        assert!(!root.path().join("alias").exists());
    }

    #[cfg(unix)]
    #[test]
    fn restrictive_source_permissions_are_restored_after_copy_failure() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("value");
        fs::write(&source, b"payload").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o111)).unwrap();
        let original = fs::symlink_metadata(&source).unwrap().permissions();

        let result: FetchResult<()> =
            with_readable_source(&source, &original, "fixture.tar", || {
                assert_eq!(
                    fs::metadata(&source).unwrap().permissions().mode() & 0o777,
                    0o511
                );
                Err(extraction_failure("fixture.tar", "injected copy failure"))
            });

        assert!(result.is_err());
        assert_eq!(
            fs::symlink_metadata(&source).unwrap().permissions().mode() & 0o777,
            0o111
        );
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_copy_preserves_restrictive_mode_and_target_mtime() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::time::{Duration, UNIX_EPOCH};

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("value");
        fs::write(&source, b"payload").unwrap();
        let modified = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(FileTimes::new().set_modified(modified))
            .unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o111)).unwrap();
        let mut budget = ExtractionBudget::default();

        materialize(
            root.path(),
            &[("alias".into(), "value".into())],
            &mut budget,
            "fixture.tar",
        )
        .unwrap();

        let source_metadata = fs::symlink_metadata(&source).unwrap();
        let alias_metadata = fs::symlink_metadata(root.path().join("alias")).unwrap();
        assert_eq!(source_metadata.permissions().mode() & 0o777, 0o111);
        assert_eq!(alias_metadata.permissions().mode() & 0o777, 0o111);
        assert_eq!(source_metadata.mtime(), 1_600_000_000);
        assert_eq!(alias_metadata.mtime(), source_metadata.mtime());
        assert_ne!(source_metadata.ino(), alias_metadata.ino());
    }
}
