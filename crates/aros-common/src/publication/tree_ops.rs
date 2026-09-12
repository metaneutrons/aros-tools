//! Public bounded no-follow tree operations.

#[cfg(not(unix))]
use super::unsupported_durability;
use super::{absolute_path, unix, validate_target_leaf, TreeContentCas, TreeTraversalLimits};
use std::path::Path;

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
/// [`measure_tree_content_cas`], while rejecting a tree whose entry count or
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
/// atomically through [`publish_prepared_source_tree_noclobber`] or a stricter
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
