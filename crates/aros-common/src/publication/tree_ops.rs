//! Public bounded no-follow tree operations.

#[cfg(not(unix))]
use super::unsupported_durability;
use super::{absolute_path, unix, validate_target_leaf, TreeContentCas, TreeTraversalLimits};
use std::ffi::OsString;
use std::path::Path;

/// List one directory through no-follow descriptor traversal with a stable,
/// bounded snapshot of its immediate entry names.
///
/// The directory is read twice through the same descriptor and must retain its
/// path binding and identity. Callers still need to validate every listed
/// entry before treating it as authority for a state-changing operation.
///
/// # Errors
///
/// Returns an I/O, unsafe-directory, concurrent-mutation, unsupported-host,
/// or resource-limit error.
pub fn directory_entry_names_nofollow_bounded(
    path: &Path,
    max_entries: usize,
) -> std::io::Result<Vec<OsString>> {
    if max_entries == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "directory entry limit must be greater than zero",
        ));
    }
    validate_target_leaf(path)?;
    #[cfg(unix)]
    {
        unix::directory_entry_names_nofollow_bounded_impl(&absolute_path(path)?, max_entries)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, max_entries);
        Err(unsupported_durability())
    }
}

/// Create a directory path through no-follow descriptor traversal, or verify
/// that the existing path is a real directory.
///
/// # Errors
///
/// Returns an error when a component is unsafe, a symlink, or not a directory.
pub fn ensure_directory_nofollow(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        unix::ensure_directory_nofollow(&absolute_path(path)?)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(unsupported_durability())
    }
}

/// Measure a complete tree through no-follow descriptors under explicit
/// resource limits.
///
/// This has the same identity and double-snapshot contract as
/// [`crate::publication::measure_tree_content_cas`], while rejecting a tree whose entry count or
/// regular-file content exceeds `limits`.
///
/// # Errors
///
/// Returns an I/O, unsafe-tree, mutation, or resource-limit error.
pub fn measure_tree_content_cas_bounded(
    path: &Path,
    limits: TreeTraversalLimits,
) -> std::io::Result<TreeContentCas> {
    validate_target_leaf(path)?;
    #[cfg(unix)]
    {
        unix::measure_tree_content_cas_bounded(&absolute_path(path)?, limits)
    }
    #[cfg(not(unix))]
    {
        let _ = (path, limits);
        Err(unsupported_durability())
    }
}

/// Copy one previously measured tree into an empty, caller-owned staging
/// directory without following source or destination symlinks.
///
/// The source must still equal `expected` when the copy begins and ends. The
/// copied staging tree is measured again and must have the same content
/// digest. Callers remain responsible for publishing the staging directory
/// atomically through [`crate::publication::publish_prepared_source_tree_noclobber`] or a stricter
/// envelope operation.
///
/// # Errors
///
/// Returns an I/O, unsafe-tree, mutation, content-mismatch, or resource-limit
/// error. It never replaces or removes either input tree.
pub fn copy_tree_from_snapshot_nofollow(
    source: &Path,
    destination: &Path,
    expected: &TreeContentCas,
    limits: TreeTraversalLimits,
) -> std::io::Result<TreeContentCas> {
    validate_target_leaf(source)?;
    validate_target_leaf(destination)?;
    #[cfg(unix)]
    {
        unix::copy_tree_from_snapshot_nofollow(
            &absolute_path(source)?,
            &absolute_path(destination)?,
            expected,
            limits,
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (source, destination, expected, limits);
        Err(unsupported_durability())
    }
}

/// Remove exactly one previously measured tree through no-follow descriptor
/// traversal.
///
/// The target must still match `expected` before mutation begins. Every child
/// is rechecked against that snapshot immediately before removal, and a new,
/// missing, replaced, symlinked, or changed entry aborts the operation. The
/// traversal remains bounded by `limits`; this function never follows a link
/// or recursively removes an arbitrary caller-selected parent.
///
/// On Unix the target must also be contained in a private, non-shared
/// namespace: no ancestor, directory, or regular payload file may be group-
/// or world-writable, and regular payload files may not have multiple links.
/// POSIX has no inode-bound unlink primitive, so arbitrary processes running
/// as the store owner remain within that owner's trust boundary.
///
/// # Errors
///
/// Returns an I/O, unsafe-tree, identity-race, content-mismatch, durability,
/// unsupported-host, or resource-limit error. A caller that needs recovery
/// after a partial deletion must retain its own durable operation record.
pub fn remove_tree_from_snapshot_nofollow(
    target: &Path,
    expected: &TreeContentCas,
    limits: TreeTraversalLimits,
) -> std::io::Result<()> {
    validate_target_leaf(target)?;
    #[cfg(unix)]
    {
        unix::remove_tree_from_snapshot_impl(&absolute_path(target)?, expected, limits)
    }
    #[cfg(not(unix))]
    {
        let _ = (target, expected, limits);
        Err(unsupported_durability())
    }
}
