//! Complete, no-follow regular-file inventory of a media build subtree.

use crate::media_profile::valid_destination;
use crate::{
    open_regular_file_nofollow, sha256_reader, validate_existing_directory_prefix_nofollow,
    Sha256Digest,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const MAX_ENTRIES: usize = 20_000;

/// One measured regular file under a declared source tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTreeFile {
    pub relative: String,
    pub source_path: PathBuf,
    pub sha256: Sha256Digest,
    pub size_bytes: u64,
}

/// Closed content observation, including otherwise empty directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaTreeInventory {
    pub sha256: Sha256Digest,
    pub directories: Vec<String>,
    pub files: Vec<MediaTreeFile>,
}

/// Walk one directory without following links or accepting special entries.
/// The digest is domain-separated and includes sorted relative directory and
/// regular-file paths, file sizes and file hashes.
///
/// # Errors
/// Refuses symlinks, special files, unsafe names, entry overflow or changed
/// files. Callers must remeasure before publication.
pub fn measure_media_tree(root: &Path) -> Result<MediaTreeInventory, std::io::Error> {
    validate_existing_directory_prefix_nofollow(root)?;
    let metadata = std::fs::symlink_metadata(root)?;
    if !metadata.file_type().is_dir() {
        return Err(invalid("media tree root is not a regular directory"));
    }
    let mut directories = Vec::new();
    let mut files = Vec::new();
    for entry in WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry.map_err(|error| invalid(&format!("cannot walk media tree: {error}")))?;
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| invalid("media tree path escaped its root"))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| invalid("media tree path is not UTF-8"))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if relative.len() > 1024 || !valid_destination(&relative) {
            return Err(invalid("media tree has an unsafe relative path"));
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_dir() {
            directories.push(relative);
        } else if metadata.file_type().is_file() {
            let mut file = open_regular_file_nofollow(entry.path())?;
            let measured = sha256_reader(&mut file)?;
            files.push(MediaTreeFile {
                relative,
                source_path: entry.path().to_path_buf(),
                sha256: measured.digest,
                size_bytes: measured.size,
            });
        } else {
            return Err(invalid("media tree contains a symlink or special entry"));
        }
        if directories.len() + files.len() > MAX_ENTRIES {
            return Err(invalid("media tree exceeds its entry limit"));
        }
    }
    directories.sort();
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    let mut hasher = Sha256::new();
    hasher.update(b"aros-media-tree-v1\0");
    for directory in &directories {
        hasher.update(b"D\0");
        hasher.update(directory.as_bytes());
        hasher.update(b"\n");
    }
    for file in &files {
        hasher.update(b"F\0");
        hasher.update(file.relative.as_bytes());
        hasher.update(b"\0");
        hasher.update(file.sha256.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(file.size_bytes.to_string().as_bytes());
        hasher.update(b"\n");
    }
    Ok(MediaTreeInventory {
        sha256: crate::finish_sha256(hasher),
        directories,
        files,
    })
}

fn invalid(message: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.to_string())
}

#[cfg(test)]
mod tests {
    use super::measure_media_tree;
    use std::fs;

    #[test]
    fn complete_tree_digest_detects_files_and_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("boot/empty")).unwrap();
        fs::write(temp.path().join("boot/kernel"), b"kernel").unwrap();
        let first = measure_media_tree(temp.path()).unwrap();
        assert_eq!(first.directories, ["boot", "boot/empty"]);
        assert_eq!(first.files.len(), 1);
        assert_eq!(first, measure_media_tree(temp.path()).unwrap());
        fs::remove_dir(temp.path().join("boot/empty")).unwrap();
        let without_empty = measure_media_tree(temp.path()).unwrap();
        assert_ne!(first.sha256, without_empty.sha256);
        fs::write(temp.path().join("boot/kernel"), b"changed").unwrap();
        assert_ne!(
            without_empty.sha256,
            measure_media_tree(temp.path()).unwrap().sha256
        );
    }

    #[cfg(unix)]
    #[test]
    fn tree_rejects_symlinked_files_and_root() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("real"), b"bytes").unwrap();
        std::os::unix::fs::symlink(temp.path().join("real"), temp.path().join("link")).unwrap();
        assert!(measure_media_tree(temp.path()).is_err());
        fs::remove_file(temp.path().join("link")).unwrap();
        let linked_root = temp.path().with_extension("link");
        std::os::unix::fs::symlink(temp.path(), &linked_root).unwrap();
        assert!(measure_media_tree(&linked_root).is_err());
        fs::remove_file(linked_root).unwrap();
    }
}
