//! ZIP links are deferred so no archive write can traverse a symbolic link.

use std::fs;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use super::{extraction_failure, validate_link_target, FetchResult};

const MAX_LINK_BYTES: u64 = 4096;

pub(super) fn read_target<R: Read + Seek>(
    entry: &mut zip::read::ZipFile<'_, R>,
    path: &Path,
    archive: &str,
) -> FetchResult<PathBuf> {
    let declared = entry.size();
    if declared == 0 || declared > MAX_LINK_BYTES {
        return Err(extraction_failure(
            archive,
            "ZIP link target has invalid size",
        ));
    }
    let mut bytes = Vec::new();
    entry
        .take(declared + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            extraction_failure(archive, format!("cannot read ZIP link target: {error}"))
        })?;
    if bytes.len() as u64 != declared || bytes.contains(&0) {
        return Err(extraction_failure(
            archive,
            "ZIP link target has invalid payload",
        ));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| extraction_failure(archive, "ZIP link target is not UTF-8"))?;
    let target = PathBuf::from(text);
    validate_link_target(path, &target, archive)?;
    Ok(target)
}

pub(super) fn materialize(
    root: &Path,
    links: &[(PathBuf, PathBuf)],
    archive: &str,
) -> FetchResult<()> {
    // Reserve every real parent before creating any links. A ZIP that also
    // writes below a link path has already created a directory there and fails
    // this collision check, independently of entry order.
    for (relative, _) in links {
        let path = root.join(relative);
        if fs::symlink_metadata(&path).is_ok() {
            return Err(extraction_failure(
                archive,
                format!(
                    "ZIP link '{}' collides with an extracted entry",
                    relative.display()
                ),
            ));
        }
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| extraction_failure(archive, "ZIP link has no parent"))?,
        )
        .map_err(|error| {
            extraction_failure(archive, format!("cannot prepare ZIP link parent: {error}"))
        })?;
    }
    // A later parent reservation can have occupied an earlier link's path.
    for (relative, _) in links {
        if fs::symlink_metadata(root.join(relative)).is_ok() {
            return Err(extraction_failure(
                archive,
                "ZIP link is an ancestor of another entry",
            ));
        }
    }
    for (relative, target) in links {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, root.join(relative)).map_err(|error| {
            extraction_failure(archive, format!("cannot create ZIP link: {error}"))
        })?;
        #[cfg(not(unix))]
        {
            let _ = target;
            return Err(extraction_failure(
                archive,
                "safe ZIP links are unsupported on this host",
            ));
        }
    }
    let canonical_root = root.canonicalize().map_err(|error| {
        extraction_failure(
            archive,
            format!("cannot resolve ZIP extraction root: {error}"),
        )
    })?;
    for (relative, _) in links {
        // Reject dangling links, cycles and indirect escapes, not only lexical
        // `..` escapes. The verified tree is still private and unpublished.
        let resolved = root.join(relative).canonicalize().map_err(|error| {
            extraction_failure(
                archive,
                format!("ZIP link '{}' cannot resolve: {error}", relative.display()),
            )
        })?;
        if !resolved.starts_with(&canonical_root) {
            return Err(extraction_failure(
                archive,
                format!(
                    "ZIP link '{}' resolves outside extraction root",
                    relative.display()
                ),
            ));
        }
    }
    Ok(())
}
