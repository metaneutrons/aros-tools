//! Conservative path overlap observations for fresh local output preflights.

use std::path::Path;

/// Compare existing directory identities and prospective path suffixes.
///
/// Existing case, Unicode and symlink aliases are compared by device/inode.
/// Absent suffixes are conservatively compared without ASCII case. This is a
/// read-only preflight, not a filesystem lock or no-follow write primitive;
/// callers must own quiescent paths and separately validate write destinations.
///
/// # Errors
/// Rejects relative or non-normalized paths, inaccessible ancestry, and hosts
/// without Unix directory identities. Missing suffixes need not exist.
pub fn filesystem_paths_overlap(left: &Path, right: &Path) -> std::io::Result<bool> {
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "path overlap requires Unix directory identities",
        ))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::path::{Component, PathBuf};

        let ancestors = |path: &Path| -> std::io::Result<Vec<(PathBuf, u64, u64)>> {
            if !path.is_absolute()
                || path
                    .components()
                    .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "path overlap requires normalized absolute paths",
                ));
            }
            let mut existing = Vec::new();
            for ancestor in path.ancestors() {
                match std::fs::metadata(ancestor) {
                    Ok(metadata) if metadata.is_dir() => {
                        existing.push((ancestor.to_path_buf(), metadata.dev(), metadata.ino()));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(existing)
        };
        let left_ancestors = ancestors(left)?;
        let right_ancestors = ancestors(right)?;
        for (left_parent, device, inode) in left_ancestors {
            if let Some((right_parent, _, _)) =
                right_ancestors
                    .iter()
                    .find(|(_, other_device, other_inode)| {
                        *other_device == device && *other_inode == inode
                    })
            {
                let left_suffix = left
                    .strip_prefix(left_parent)
                    .map_err(std::io::Error::other)?;
                let right_suffix = right
                    .strip_prefix(right_parent)
                    .map_err(std::io::Error::other)?;
                let left_parts = left_suffix.components().collect::<Vec<_>>();
                let right_parts = right_suffix.components().collect::<Vec<_>>();
                return Ok(left_parts.iter().zip(&right_parts).all(|(left, right)| {
                    left.as_os_str()
                        .as_encoded_bytes()
                        .eq_ignore_ascii_case(right.as_os_str().as_encoded_bytes())
                }));
            }
        }
        Err(std::io::Error::other("cannot establish path ancestry"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::filesystem_paths_overlap;
    use std::os::unix::fs::symlink;

    #[test]
    fn compares_existing_aliases_and_absent_suffixes_symmetrically() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let existing = root.join("build");
        std::fs::create_dir(&existing).unwrap();
        let alias = root.join("build-alias");
        symlink(&existing, &alias).unwrap();
        for (left, right, expected) in [
            (existing.join("new-output"), existing.clone(), true),
            (alias.join("new-output"), existing.clone(), true),
            (existing.join("new-output"), alias.clone(), true),
            (root.join("missing/New"), root.join("missing/new/sub"), true),
            (root.join("missing/New"), root.join("missing/other"), false),
            (existing.clone(), root.join("other-build"), false),
        ] {
            assert_eq!(filesystem_paths_overlap(&left, &right).unwrap(), expected);
            assert_eq!(filesystem_paths_overlap(&right, &left).unwrap(), expected);
        }
    }

    #[test]
    fn observes_native_case_and_unicode_directory_aliases_by_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().canonicalize().unwrap();
        for (original, alias) in [
            ("SelectedRoot", "selectedroot"),
            ("Caf\u{e9}Root", "Cafe\u{301}Root"),
        ] {
            let original = parent.join(original);
            std::fs::create_dir(&original).unwrap();
            let alias = parent.join(alias);
            if !alias.is_dir() {
                continue;
            }
            assert!(filesystem_paths_overlap(&alias.join("new-output"), &original).unwrap());
            assert!(filesystem_paths_overlap(&original, &alias.join("new-output")).unwrap());
            assert!(!filesystem_paths_overlap(
                &alias.join("new-output"),
                &parent.join("separate-root")
            )
            .unwrap());
        }
    }

    #[test]
    fn refuses_relative_and_traversing_observations_without_writes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        for path in [std::path::PathBuf::from("relative"), root.join("../escape")] {
            assert_eq!(
                filesystem_paths_overlap(&root, &path).unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    }
}
