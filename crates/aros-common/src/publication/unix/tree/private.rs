//! Private-tree validation for confined destructive cleanup.

use super::{
    directory_entry_names_with_budget, identity_from_stat, rfs, AtFlags, Component, ErrorKind,
    Mode, OFlags, OwnedFd, Path, PathBuf, TreeMeasurementBudget, TreeTraversalLimits,
};

/// Prove that destructive cleanup is confined to a non-shared Unix namespace.
///
/// Descriptor-relative traversal prevents symlink escapes, while this check
/// prevents a group or world principal from modifying an otherwise verified
/// path through a writable ancestor or payload object. Sticky ancestors such
/// as `/tmp` are permitted because they protect entries owned by the store
/// owner; writable directories inside the managed tree are never permitted.
/// POSIX cannot express an identity-bound unlink; callers therefore use this
/// as the explicit trust boundary for the short revalidation-to-unlink
/// interval.
pub(super) fn validate_private_removal_boundary(
    target: &Path,
    directory: &OwnedFd,
    limits: TreeTraversalLimits,
) -> std::io::Result<()> {
    validate_private_ancestor_chain(target)?;
    let mut budget = TreeMeasurementBudget::bounded(limits);
    validate_private_tree(directory, target, &mut budget)
}

pub(in crate::publication::unix) fn validate_private_ancestor_chain(
    target: &Path,
) -> std::io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("tree removal target '{}' has no parent", target.display()),
        )
    })?;
    let mut directory = rfs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    validate_private_ancestor_directory(&rfs::fstat(&directory)?, Path::new("/"))?;
    let mut display = PathBuf::from("/");
    for component in parent.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let next = rfs::openat(
                    &directory,
                    Path::new(name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                display.push(name);
                validate_private_ancestor_directory(&rfs::fstat(&next)?, &display)?;
                directory = next;
            }
            Component::Prefix(_) | Component::CurDir | Component::ParentDir => {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "tree removal target '{}' is not absolute and normalized",
                        target.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

pub(in crate::publication) fn validate_private_directory_nofollow(
    path: &Path,
) -> std::io::Result<()> {
    validate_private_ancestor_chain(path)?;
    let directory = rfs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    validate_private_directory(&rfs::fstat(&directory)?, path)
}

pub(super) fn validate_private_tree(
    directory: &OwnedFd,
    display_path: &Path,
    budget: &mut TreeMeasurementBudget,
) -> std::io::Result<()> {
    let root = rfs::fstat(directory)?;
    validate_private_directory(&root, display_path)?;
    let root_identity = identity_from_stat(&root);
    let names = directory_entry_names_with_budget(directory, budget)?;
    for name in names {
        let path = display_path.join(&name);
        let stat = rfs::statat(directory, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW)?;
        let file_type = rfs::FileType::from_raw_mode(stat.st_mode);
        if file_type.is_file() {
            validate_private_regular_file(&stat, &path)?;
            continue;
        }
        if file_type.is_symlink() {
            continue;
        }
        if !file_type.is_dir() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "refusing to remove unsupported filesystem object '{}'",
                    path.display()
                ),
            ));
        }
        let child = rfs::openat(
            directory,
            Path::new(&name),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        if identity_from_stat(&rfs::fstat(&child)?) != identity_from_stat(&stat) {
            return Err(std::io::Error::other(format!(
                "directory '{}' changed while checking its removal trust boundary",
                path.display()
            )));
        }
        validate_private_tree(&child, &path, budget)?;
    }
    if identity_from_stat(&rfs::fstat(directory)?) != root_identity {
        return Err(std::io::Error::other(format!(
            "directory '{}' changed while checking its removal trust boundary",
            display_path.display()
        )));
    }
    Ok(())
}

pub(super) fn validate_private_directory(stat: &rfs::Stat, path: &Path) -> std::io::Result<()> {
    if !rfs::FileType::from_raw_mode(stat.st_mode).is_dir() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("removal boundary '{}' is not a directory", path.display()),
        ));
    }
    validate_not_group_or_world_writable(stat, path, "directory")
}

/// Validate an ancestor outside the managed tree.
///
/// A sticky directory can be writable without allowing another non-privileged
/// principal to rename or remove the store owner's entry. This admits standard
/// private temporary paths below `/tmp`, while `validate_private_tree` still
/// rejects every writable directory that is part of the managed envelope.
pub(super) fn validate_private_ancestor_directory(
    stat: &rfs::Stat,
    path: &Path,
) -> std::io::Result<()> {
    if !rfs::FileType::from_raw_mode(stat.st_mode).is_dir() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("removal ancestor '{}' is not a directory", path.display()),
        ));
    }
    #[allow(
        clippy::useless_conversion,
        reason = "rustix mode_t width differs between supported Unix targets"
    )]
    let mode = u32::from(stat.st_mode);
    if mode & 0o022 != 0 && mode & 0o1000 == 0 {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            format!(
                "refusing to remove through ancestor '{}' because it is group- or world-writable without the sticky bit",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(in crate::publication::unix) fn validate_private_regular_file(
    stat: &rfs::Stat,
    path: &Path,
) -> std::io::Result<()> {
    validate_not_group_or_world_writable(stat, path, "regular file")?;
    if stat.st_nlink != 1 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "refusing to remove multiply linked regular file '{}'; its content may be reachable outside the managed tree",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(super) fn validate_not_group_or_world_writable(
    stat: &rfs::Stat,
    path: &Path,
    kind: &str,
) -> std::io::Result<()> {
    #[allow(
        clippy::useless_conversion,
        reason = "rustix mode_t width differs between supported Unix targets"
    )]
    let mode = u32::from(stat.st_mode);
    if mode & 0o022 != 0 {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            format!(
                "refusing to remove {kind} '{}' because it is group- or world-writable",
                path.display()
            ),
        ));
    }
    Ok(())
}
