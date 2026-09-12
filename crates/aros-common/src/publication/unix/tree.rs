//! Durable no-clobber tree staging, recovery, and prepared-tree syncing.

use super::*;

pub(in crate::publication) fn publish_prepared_tree_noclobber(
    staging: &Path,
    destination: &Path,
    name_policy: PreparedTreeNamePolicy,
) -> std::io::Result<PublicationReceipt> {
    let stage_parent = open_parent(staging, false)?;
    let destination_parent = open_parent(destination, true)?;
    let stage_parent_identity = identity_from_stat(&rfs::fstat(&stage_parent.fd)?);
    let destination_parent_identity = identity_from_stat(&rfs::fstat(&destination_parent.fd)?);
    if stage_parent_identity != destination_parent_identity {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "prepared tree '{}' and destination '{}' must have the same parent directory",
                staging.display(),
                destination.display()
            ),
        ));
    }
    let journal = transaction_journal_path(destination, "prepared-tree")?;
    let lock = lock_for_journal(&journal)?;

    let stage_parent = open_parent(staging, false)?;
    let destination_parent = open_parent(destination, false)?;
    if identity_from_stat(&rfs::fstat(&stage_parent.fd)?) != stage_parent_identity
        || identity_from_stat(&rfs::fstat(&destination_parent.fd)?) != stage_parent_identity
    {
        return Err(std::io::Error::other(
            "prepared-tree parent changed while acquiring its publication lock",
        ));
    }
    match rfs::statat(
        &destination_parent.fd,
        &destination_parent.leaf,
        AtFlags::SYMLINK_NOFOLLOW,
    ) {
        Ok(_) => {
            return Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                format!(
                    "refusing to replace existing prepared-tree destination '{}'",
                    destination.display()
                ),
            ))
        }
        Err(rustix::io::Errno::NOENT) => {}
        Err(error) => return Err(error.into()),
    }
    let stage_fd = rfs::openat(
        &stage_parent.fd,
        &stage_parent.leaf,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let stage_identity = identity_from_stat(&rfs::fstat(&stage_fd)?);
    sync_prepared_tree(&stage_fd, staging, name_policy)?;
    #[cfg(test)]
    crate::publication::publication_tests::run_fault_point(
        "prepared-tree-after-sync-before-rename",
        staging,
    )?;
    if directory_identity_at(&stage_parent.fd, &stage_parent.leaf)? != Some(stage_identity) {
        return Err(std::io::Error::other(format!(
            "prepared tree '{}' changed while it was being synced",
            staging.display()
        )));
    }
    rfs::renameat_with(
        &stage_parent.fd,
        &stage_parent.leaf,
        &destination_parent.fd,
        &destination_parent.leaf,
        RenameFlags::NOREPLACE,
    )?;
    if let Err(error) = filesystem::sync_prepared_parent(&destination_parent, destination) {
        drop(lock);
        return Err(io_failure(PublicationError::uncertain(format!(
            "prepared-tree rename completed but durable parent sync could not be proven: {error}; complete destination retained"
        ))));
    }
    if directory_identity_at(&destination_parent.fd, &destination_parent.leaf)?
        != Some(stage_identity)
    {
        drop(lock);
        return Err(io_failure(PublicationError::uncertain(
            "prepared-tree destination identity changed after rename",
        )));
    }
    drop(lock);
    Ok(PublicationReceipt::default())
}

/// Copy a previously measured source tree into an empty staging tree using
/// descriptor-relative no-follow operations only.
pub(in crate::publication) fn copy_tree_from_snapshot_nofollow(
    source: &Path,
    destination: &Path,
    expected: &TreeContentCas,
    limits: TreeTraversalLimits,
) -> std::io::Result<TreeContentCas> {
    let source_parent = open_parent(source, false)?;
    let source_directory = rfs::openat(
        &source_parent.fd,
        &source_parent.leaf,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let source_identity = identity_from_stat(&rfs::fstat(&source_directory)?);
    let destination_parent = open_parent(destination, false)?;
    let destination_directory = rfs::openat(
        &destination_parent.fd,
        &destination_parent.leaf,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let destination_identity = identity_from_stat(&rfs::fstat(&destination_directory)?);
    if !directory_entry_names_capped(&destination_directory, Some(limits.max_entries))?.is_empty() {
        return Err(std::io::Error::new(
            ErrorKind::AlreadyExists,
            format!(
                "destination staging tree '{}' is not empty",
                destination.display()
            ),
        ));
    }

    let current = TreeContentCas {
        root: source_identity,
        entries: stable_measure_tree_content_at_bounded(&source_directory, source, Some(limits))?,
    };
    if &current != expected {
        return Err(std::io::Error::other(format!(
            "source tree '{}' changed after its import preview",
            source.display()
        )));
    }

    let mut copy_budget = TreeMeasurementBudget::bounded(limits);
    let mut portable_paths = BTreeSet::new();
    copy_tree_contents(
        &source_directory,
        &destination_directory,
        source,
        destination,
        Path::new(""),
        &mut copy_budget,
        &mut portable_paths,
    )?;
    rfs::fsync(&destination_directory)?;

    let source_after = TreeContentCas {
        root: source_identity,
        entries: stable_measure_tree_content_at_bounded(&source_directory, source, Some(limits))?,
    };
    if &source_after != expected
        || identity_from_stat(&rfs::fstat(&source_directory)?) != source_identity
        || directory_identity_at(&source_parent.fd, &source_parent.leaf)? != Some(source_identity)
    {
        return Err(std::io::Error::other(format!(
            "source tree '{}' changed while it was imported",
            source.display()
        )));
    }

    let copied = TreeContentCas {
        root: destination_identity,
        entries: stable_measure_tree_content_at_bounded(
            &destination_directory,
            destination,
            Some(limits),
        )?,
    };
    if copied.payload_digest_excluding(None) != expected.payload_digest_excluding(None)
        || identity_from_stat(&rfs::fstat(&destination_directory)?) != destination_identity
        || directory_identity_at(&destination_parent.fd, &destination_parent.leaf)?
            != Some(destination_identity)
    {
        return Err(std::io::Error::other(format!(
            "staged copy '{}' does not exactly match its source snapshot",
            destination.display()
        )));
    }
    Ok(copied)
}

/// Remove a complete tree only when it still equals the measured snapshot.
///
/// This is deliberately descriptor-relative and bounded. It is suitable for a
/// higher-level lifecycle operation that has already recorded a durable
/// recovery receipt; it may leave a partially removed tree when the operating
/// system reports a later error, rather than guessing that a changed target is
/// still safe to remove.
pub(in crate::publication) fn remove_tree_from_snapshot_nofollow(
    target: &Path,
    expected: &TreeContentCas,
    limits: TreeTraversalLimits,
) -> std::io::Result<()> {
    let parent = open_parent(target, false)?;
    let directory = rfs::openat(
        &parent.fd,
        &parent.leaf,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let root = identity_from_stat(&rfs::fstat(&directory)?);
    if root != expected.root {
        return Err(std::io::Error::other(format!(
            "refusing to remove identity-mismatched tree '{}'",
            target.display()
        )));
    }
    // POSIX has no `unlinkat` variant that binds deletion to an already-open
    // descriptor or an expected inode.  The identity checks below therefore
    // need a namespace in which an uncooperative principal cannot replace a
    // path between check and mutation.  Reject group/world-writable ancestors,
    // directories, and regular files before treating the caller's advisory
    // locks as that cooperative-writer boundary.  A same-UID process is part
    // of the store owner's trust domain; it has the authority to replace the
    // entire store and cannot be distinguished portably from its owner.
    validate_private_removal_boundary(target, &directory, limits)?;
    let current = TreeContentCas {
        root,
        entries: stable_measure_tree_content_at_bounded(&directory, target, Some(limits))?,
    };
    if &current != expected {
        return Err(std::io::Error::other(format!(
            "refusing to remove changed tree '{}'",
            target.display()
        )));
    }

    let mut budget = TreeMeasurementBudget::bounded(limits);
    remove_tree_contents_from_snapshot(&directory, target, &expected.entries, &[], &mut budget)?;
    if !directory_entry_names_capped(&directory, Some(limits.max_entries))?.is_empty() {
        return Err(std::io::Error::other(format!(
            "refusing to remove tree '{}' because entries appeared during cleanup",
            target.display()
        )));
    }
    // Keep the verified root descriptor open through the final pathname
    // operation. Revalidate both the descriptor and no-follow parent entry
    // immediately before `unlinkat`; a substituted root must fail closed
    // rather than being treated as the approved snapshot.
    if identity_from_stat(&rfs::fstat(&directory)?) != root
        || directory_identity_at(&parent.fd, &parent.leaf)? != Some(root)
    {
        return Err(std::io::Error::other(format!(
            "tree '{}' changed before final removal",
            target.display()
        )));
    }
    rfs::unlinkat(&parent.fd, Path::new(&parent.leaf), AtFlags::REMOVEDIR)?;
    rfs::fsync(&parent.fd)?;
    Ok(())
}

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
fn validate_private_removal_boundary(
    target: &Path,
    directory: &OwnedFd,
    limits: TreeTraversalLimits,
) -> std::io::Result<()> {
    validate_private_ancestor_chain(target)?;
    let mut budget = TreeMeasurementBudget::bounded(limits);
    validate_private_tree(directory, target, &mut budget)
}

fn validate_private_ancestor_chain(target: &Path) -> std::io::Result<()> {
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

fn validate_private_tree(
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

fn validate_private_directory(stat: &rfs::Stat, path: &Path) -> std::io::Result<()> {
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
fn validate_private_ancestor_directory(stat: &rfs::Stat, path: &Path) -> std::io::Result<()> {
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

fn validate_private_regular_file(stat: &rfs::Stat, path: &Path) -> std::io::Result<()> {
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

fn validate_not_group_or_world_writable(
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

/// Return a stable immediate-name snapshot of one real directory without
/// following any component. The second read detects ordinary concurrent
/// additions/removals; callers must still bind each resulting entry before a
/// mutation.
pub(in crate::publication) fn directory_entry_names_nofollow_bounded(
    path: &Path,
    max_entries: usize,
) -> std::io::Result<Vec<OsString>> {
    let parent = open_parent(path, false)?;
    let directory = rfs::openat(
        &parent.fd,
        &parent.leaf,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let identity = identity_from_stat(&rfs::fstat(&directory)?);
    let first = directory_entry_names_capped(&directory, Some(max_entries))?;
    let second = directory_entry_names_capped(&directory, Some(max_entries))?;
    if first != second
        || identity_from_stat(&rfs::fstat(&directory)?) != identity
        || directory_identity_at(&parent.fd, &parent.leaf)? != Some(identity)
    {
        return Err(std::io::Error::other(format!(
            "directory '{}' changed while its entries were listed",
            path.display()
        )));
    }
    Ok(second.into_iter().collect())
}

fn remove_tree_contents_from_snapshot(
    directory: &OwnedFd,
    display_path: &Path,
    entries: &BTreeMap<Vec<u8>, TreeContentEntry>,
    prefix: &[u8],
    budget: &mut TreeMeasurementBudget,
) -> std::io::Result<()> {
    let expected_names = directory_entry_names_from_keys(entries, prefix);
    let actual_names = directory_entry_names_with_budget(directory, budget)?;
    if actual_names != expected_names {
        return Err(std::io::Error::other(format!(
            "tree directory '{}' changed before owned cleanup",
            display_path.display()
        )));
    }

    for name in actual_names {
        let relative = child_relative_path(prefix, &name);
        let expected_entry = entries.get(&relative).ok_or_else(|| {
            std::io::Error::other(format!(
                "tree directory '{}' has an unrecorded cleanup entry '{}'",
                display_path.display(),
                name.to_string_lossy()
            ))
        })?;
        let child_display = display_path.join(&name);
        let stat = rfs::statat(directory, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW)?;
        if tree_node_snapshot(&stat)? != expected_entry.snapshot {
            return Err(std::io::Error::other(format!(
                "tree entry '{}' changed before owned cleanup",
                child_display.display()
            )));
        }

        match expected_entry.snapshot.kind {
            1 => remove_snapshot_regular(directory, &name, &child_display, expected_entry, budget)?,
            2 => remove_snapshot_directory(
                directory,
                &name,
                &child_display,
                entries,
                &relative,
                expected_entry,
                budget,
            )?,
            3 => remove_snapshot_symlink(directory, &name, &child_display, expected_entry)?,
            kind => {
                return Err(std::io::Error::other(format!(
                    "tree entry '{}' has unsupported snapshot kind {kind}",
                    child_display.display()
                )));
            }
        }
    }
    Ok(())
}

fn child_relative_path(prefix: &[u8], name: &OsStr) -> Vec<u8> {
    let mut relative = prefix.to_owned();
    if !relative.is_empty() {
        relative.push(b'/');
    }
    relative.extend_from_slice(name.as_bytes());
    relative
}

fn remove_snapshot_regular(
    parent: &OwnedFd,
    name: &OsStr,
    display_path: &Path,
    expected: &TreeContentEntry,
    budget: &mut TreeMeasurementBudget,
) -> std::io::Result<()> {
    budget.reserve_regular_file_bytes(expected.snapshot.size, display_path)?;
    let fd = rfs::openat(
        parent,
        Path::new(name),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if tree_node_snapshot(&rfs::fstat(&fd)?)? != expected.snapshot {
        return Err(std::io::Error::other(format!(
            "tree file '{}' changed before owned cleanup",
            display_path.display()
        )));
    }
    let mut file = std::fs::File::from(fd);
    let digest = sha256_reader(&mut file)?.digest;
    if tree_node_snapshot(&rfs::fstat(&file)?)? != expected.snapshot
        || expected.content.as_ref() != Some(&digest)
    {
        return Err(std::io::Error::other(format!(
            "tree file '{}' changed while verifying owned cleanup",
            display_path.display()
        )));
    }
    // Retain the verified descriptor and recheck the parent entry immediately
    // before unlink. POSIX cannot make that final name-based unlink
    // identity-atomic, so the enclosing private-namespace check is also a
    // required part of this mutation contract.
    #[cfg(test)]
    crate::publication::publication_tests::run_boundary(
        "snapshot-removal-before-regular-unlink",
        display_path,
    );
    if tree_node_snapshot(&rfs::statat(
        parent,
        Path::new(name),
        AtFlags::SYMLINK_NOFOLLOW,
    )?)? != expected.snapshot
    {
        return Err(std::io::Error::other(format!(
            "tree file '{}' changed immediately before owned cleanup",
            display_path.display()
        )));
    }
    rfs::unlinkat(parent, Path::new(name), AtFlags::empty())?;
    rfs::fsync(parent)?;
    Ok(())
}

fn remove_snapshot_symlink(
    parent: &OwnedFd,
    name: &OsStr,
    display_path: &Path,
    expected: &TreeContentEntry,
) -> std::io::Result<()> {
    let target = rfs::readlinkat(parent, Path::new(name), Vec::new())?;
    if tree_node_snapshot(&rfs::statat(
        parent,
        Path::new(name),
        AtFlags::SYMLINK_NOFOLLOW,
    )?)? != expected.snapshot
        || expected.content.as_ref() != Some(&sha256_bytes(target.as_bytes()))
    {
        return Err(std::io::Error::other(format!(
            "tree link '{}' changed while verifying owned cleanup",
            display_path.display()
        )));
    }
    if tree_node_snapshot(&rfs::statat(
        parent,
        Path::new(name),
        AtFlags::SYMLINK_NOFOLLOW,
    )?)? != expected.snapshot
    {
        return Err(std::io::Error::other(format!(
            "tree link '{}' changed immediately before owned cleanup",
            display_path.display()
        )));
    }
    rfs::unlinkat(parent, Path::new(name), AtFlags::empty())?;
    rfs::fsync(parent)?;
    Ok(())
}

fn remove_snapshot_directory(
    parent: &OwnedFd,
    name: &OsStr,
    display_path: &Path,
    entries: &BTreeMap<Vec<u8>, TreeContentEntry>,
    relative: &[u8],
    expected: &TreeContentEntry,
    budget: &mut TreeMeasurementBudget,
) -> std::io::Result<()> {
    let directory = rfs::openat(
        parent,
        Path::new(name),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if tree_node_snapshot(&rfs::fstat(&directory)?)? != expected.snapshot {
        return Err(std::io::Error::other(format!(
            "tree directory '{}' changed before owned cleanup",
            display_path.display()
        )));
    }
    remove_tree_contents_from_snapshot(&directory, display_path, entries, relative, budget)?;
    if !directory_entry_names_capped(&directory, budget.limits.map(|limits| limits.max_entries))?
        .is_empty()
    {
        return Err(std::io::Error::other(format!(
            "tree directory '{}' received entries during owned cleanup",
            display_path.display()
        )));
    }
    // Do not drop the verified child descriptor before the directory-entry
    // comparison and removal below.
    remove_open_empty_directory_at_exact(parent, name, &directory, expected.snapshot.identity)
}

#[allow(clippy::too_many_arguments)]
fn copy_tree_contents(
    source: &OwnedFd,
    destination: &OwnedFd,
    source_display: &Path,
    destination_display: &Path,
    relative: &Path,
    budget: &mut TreeMeasurementBudget,
    portable_paths: &mut BTreeSet<String>,
) -> std::io::Result<()> {
    let source_identity = identity_from_stat(&rfs::fstat(source)?);
    let destination_identity = identity_from_stat(&rfs::fstat(destination)?);
    let names = directory_entry_names_with_budget(source, budget)?;
    for name in names {
        let child_relative = relative.join(&name);
        let collision_key = payload_casefold_path_key(&child_relative)?;
        if !portable_paths.insert(collision_key) {
            return Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                format!(
                    "source tree '{}' has a case-folded path collision at '{}'",
                    source_display.display(),
                    child_relative.display()
                ),
            ));
        }
        let source_path = source_display.join(&name);
        let destination_path = destination_display.join(&name);
        let before = rfs::statat(source, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW)?;
        let snapshot = prepared_snapshot(&before)?;
        match snapshot.kind {
            PreparedNodeKind::File => copy_regular_entry(
                source,
                destination,
                &name,
                &source_path,
                &before,
                snapshot,
                budget,
            )?,
            PreparedNodeKind::Directory => {
                rfs::mkdirat(destination, Path::new(&name), Mode::from_raw_mode(0o700))?;
                let source_child = rfs::openat(
                    source,
                    Path::new(&name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                if prepared_snapshot(&rfs::fstat(&source_child)?)? != snapshot {
                    return Err(std::io::Error::other(format!(
                        "source directory '{}' changed before it was copied",
                        source_path.display()
                    )));
                }
                let destination_child = rfs::openat(
                    destination,
                    Path::new(&name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                copy_tree_contents(
                    &source_child,
                    &destination_child,
                    &source_path,
                    &destination_path,
                    &child_relative,
                    budget,
                    portable_paths,
                )?;
                set_mode(&destination_child, before.st_mode)?;
                rfs::fsync(&destination_child)?;
                if prepared_snapshot(&rfs::fstat(&source_child)?)? != snapshot {
                    return Err(std::io::Error::other(format!(
                        "source directory '{}' changed while it was copied",
                        source_path.display()
                    )));
                }
            }
            PreparedNodeKind::Symlink => {
                let target = rfs::readlinkat(source, Path::new(&name), Vec::new())?;
                if prepared_snapshot(&rfs::statat(
                    source,
                    Path::new(&name),
                    AtFlags::SYMLINK_NOFOLLOW,
                )?)? != snapshot
                {
                    return Err(std::io::Error::other(format!(
                        "source symlink '{}' changed before it was copied",
                        source_path.display()
                    )));
                }
                rfs::symlinkat(
                    OsStr::from_bytes(target.as_bytes()),
                    destination,
                    Path::new(&name),
                )?;
            }
        }
    }
    rfs::fsync(destination)?;
    if identity_from_stat(&rfs::fstat(source)?) != source_identity
        || identity_from_stat(&rfs::fstat(destination)?) != destination_identity
    {
        return Err(std::io::Error::other(format!(
            "source directory '{}' changed while it was copied",
            source_display.display()
        )));
    }
    Ok(())
}

fn copy_regular_entry(
    source: &OwnedFd,
    destination: &OwnedFd,
    name: &OsStr,
    source_display: &Path,
    before: &rfs::Stat,
    snapshot: PreparedNodeSnapshot,
    budget: &mut TreeMeasurementBudget,
) -> std::io::Result<()> {
    budget.reserve_regular_file_bytes(snapshot.size, source_display)?;
    let size = u64::try_from(snapshot.size).map_err(|_| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "source file '{}' has a negative size",
                source_display.display()
            ),
        )
    })?;
    let source_fd = rfs::openat(
        source,
        Path::new(name),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if prepared_snapshot(&rfs::fstat(&source_fd)?)? != snapshot {
        return Err(std::io::Error::other(format!(
            "source file '{}' changed before it was copied",
            source_display.display()
        )));
    }
    let destination_fd = rfs::openat(
        destination,
        Path::new(name),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    let mut input = std::fs::File::from(source_fd);
    let mut output = std::fs::File::from(destination_fd);
    let copied = std::io::copy(
        &mut std::io::Read::take(&mut input, size.saturating_add(1)),
        &mut output,
    )?;
    if copied != size {
        return Err(std::io::Error::other(format!(
            "source file '{}' changed while it was copied",
            source_display.display()
        )));
    }
    set_mode(&output, before.st_mode)?;
    output.flush()?;
    output.sync_all()?;
    if prepared_snapshot(&rfs::fstat(&input)?)? != snapshot {
        return Err(std::io::Error::other(format!(
            "source file '{}' changed while it was copied",
            source_display.display()
        )));
    }
    Ok(())
}

fn set_mode(file: &impl std::os::fd::AsFd, mode: rustix::fs::RawMode) -> std::io::Result<()> {
    rfs::fchmod(file, Mode::from_raw_mode(mode & 0o7777)).map_err(Into::into)
}

pub(super) fn tree_stage_name(leaf: &OsStr) -> OsString {
    let folded = leaf.to_string_lossy().to_ascii_lowercase();
    let digest = sha256_bytes(folded.as_bytes()).to_string();
    OsString::from(format!(".aros-tree-stage-{}", &digest[..32]))
}

pub(super) fn recover_flat_tree_stage(
    parent: &ParentHandle,
    stage_name: &OsStr,
    destination_name: &OsStr,
) -> std::io::Result<RecoveryOutcome> {
    let stage_fd = match rfs::openat(
        &parent.fd,
        Path::new(stage_name),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(RecoveryOutcome::None),
        Err(error) => return Err(error.into()),
    };
    let stage_identity = identity_from_stat(&rfs::fstat(&stage_fd)?);
    let entries = directory_entry_names(&stage_fd)?;
    if !entries.contains(OsStr::new("owner.json")) {
        if entries.is_empty() {
            drop(stage_fd);
            remove_empty_directory_exact(parent, stage_name, stage_identity)?;
            return Ok(RecoveryOutcome::RemovedTreeStage);
        }
        return Err(io_failure(PublicationError::rollback(format!(
            "refusing to remove unowned interrupted tree stage '{}': owner.json is absent; stage retained for inspection",
            parent.path.join(stage_name).display()
        ))));
    }

    let marker_path = parent.path.join(stage_name).join("owner.json");
    let (marker_identity, marker_bytes) = read_regular(&marker_path)?.ok_or_else(|| {
        io_failure(PublicationError::rollback(format!(
            "tree stage owner marker '{}' disappeared; stage retained",
            marker_path.display()
        )))
    })?;
    let marker = match parse_tree_stage_marker(&marker_bytes) {
        Ok(marker) => marker,
        Err(_error)
            if entries == BTreeSet::from([OsString::from("owner.json")])
                && (marker_bytes.is_empty()
                    || TREE_STAGE_MAGIC.starts_with(&marker_bytes)
                    || marker_bytes.starts_with(TREE_STAGE_MAGIC)) =>
        {
            remove_regular_at_exact(
                &stage_fd,
                OsStr::new("owner.json"),
                marker_identity,
                &sha256_bytes(&marker_bytes),
            )?;
            rfs::fsync(&stage_fd)?;
            drop(stage_fd);
            remove_empty_directory_exact(parent, stage_name, stage_identity)?;
            return Ok(RecoveryOutcome::RemovedTreeStage);
        }
        Err(error) => {
            return Err(io_failure(PublicationError::rollback(format!(
                "cannot parse tree stage owner marker '{}': {error}; stage retained",
                marker_path.display()
            ))));
        }
    };
    validate_tree_stage_marker(
        &marker,
        parent,
        stage_name,
        destination_name,
        stage_identity,
    )?;

    let unexpected = entries
        .iter()
        .filter(|entry| *entry != OsStr::new("owner.json") && *entry != OsStr::new("payload"))
        .map(|entry| entry.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if !unexpected.is_empty() {
        return Err(io_failure(PublicationError::rollback(format!(
            "interrupted tree stage contains unowned entries: {}; stage retained",
            unexpected.join(", ")
        ))));
    }

    if entries.contains(OsStr::new("payload")) {
        match directory_identity_at(&parent.fd, destination_name)? {
            Some(_) => {
                return Err(io_failure(PublicationError::rollback(format!(
                    "both interrupted payload and destination '{}' exist; refusing ambiguous recovery",
                    parent.path.join(destination_name).display()
                ))));
            }
            None => remove_owned_flat_payload(&stage_fd, &marker.members)?,
        }
    } else if directory_identity_at(&parent.fd, destination_name)?.is_some() {
        let destination_fd = rfs::openat(
            &parent.fd,
            Path::new(destination_name),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        verify_flat_tree_members(&destination_fd, &marker.members).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "tree destination exists but does not match retained owner marker: {error}; marker retained"
            )))
        })?;
        remove_regular_at_exact(
            &stage_fd,
            OsStr::new("owner.json"),
            marker_identity,
            &sha256_bytes(&marker_bytes),
        )?;
        rfs::fsync(&stage_fd)?;
        drop(stage_fd);
        remove_empty_directory_exact(parent, stage_name, stage_identity)?;
        return Ok(RecoveryOutcome::CompletedCleanup);
    }

    remove_regular_at_exact(
        &stage_fd,
        OsStr::new("owner.json"),
        marker_identity,
        &sha256_bytes(&marker_bytes),
    )?;
    rfs::fsync(&stage_fd)?;
    drop(stage_fd);
    remove_empty_directory_exact(parent, stage_name, stage_identity)?;
    Ok(RecoveryOutcome::RemovedTreeStage)
}

fn validate_tree_stage_marker(
    marker: &TreeStageMarker,
    parent: &ParentHandle,
    stage_name: &OsStr,
    destination_name: &OsStr,
    stage_identity: FileIdentity,
) -> std::io::Result<()> {
    let expected_destination = destination_name
        .to_str()
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "non-UTF-8 destination"))?
        .to_ascii_lowercase();
    if marker.schema != "aros-flat-tree-stage-v1"
        || marker.destination != expected_destination
        || marker.parent_identity != identity_from_stat(&rfs::fstat(&parent.fd)?)
        || marker.stage_identity != stage_identity
    {
        return Err(io_failure(PublicationError::rollback(format!(
            "tree stage owner marker does not match '{}' and its current parent; stage retained",
            parent.path.join(stage_name).display()
        ))));
    }
    let mut folded = BTreeSet::new();
    for name in marker.members.keys() {
        let portable = PortableOutputName::new(name).map_err(|error| {
            io_failure(PublicationError::rollback(format!(
                "tree stage marker contains a non-portable member: {error}"
            )))
        })?;
        if !folded.insert(portable.as_str().to_ascii_lowercase()) {
            return Err(io_failure(PublicationError::rollback(
                "tree stage marker contains a case-folded member collision",
            )));
        }
    }
    Ok(())
}

pub(super) fn cleanup_empty_tree_root(
    parent: &ParentHandle,
    stage_name: &OsStr,
    stage_identity: FileIdentity,
) -> std::io::Result<()> {
    let stage_fd = rfs::openat(
        &parent.fd,
        Path::new(stage_name),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if identity_from_stat(&rfs::fstat(&stage_fd)?) != stage_identity
        || !directory_entry_names(&stage_fd)?.is_empty()
    {
        return Err(std::io::Error::other(
            "refusing to remove a changed or non-empty tree staging root",
        ));
    }
    drop(stage_fd);
    remove_empty_directory_exact(parent, stage_name, stage_identity)
}

pub(super) fn cleanup_completed_tree_root(
    parent: &ParentHandle,
    stage_name: &OsStr,
    expected_marker: &TreeStageMarker,
) -> std::io::Result<()> {
    let stage_fd = rfs::openat(
        &parent.fd,
        Path::new(stage_name),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if identity_from_stat(&rfs::fstat(&stage_fd)?) != expected_marker.stage_identity {
        return Err(std::io::Error::other(
            "tree staging root identity changed after publication",
        ));
    }
    let entries = directory_entry_names(&stage_fd)?;
    if entries != BTreeSet::from([OsString::from("owner.json")]) {
        return Err(std::io::Error::other(
            "tree staging root contains unexpected post-publication entries",
        ));
    }
    let marker_path = parent.path.join(stage_name).join("owner.json");
    let (identity, bytes) = read_regular(&marker_path)?
        .ok_or_else(|| std::io::Error::new(ErrorKind::NotFound, "tree owner marker disappeared"))?;
    let marker = parse_tree_stage_marker(&bytes)?;
    if &marker != expected_marker {
        return Err(std::io::Error::other(
            "tree owner marker changed after publication",
        ));
    }
    remove_regular_at_exact(
        &stage_fd,
        OsStr::new("owner.json"),
        identity,
        &sha256_bytes(&bytes),
    )?;
    rfs::fsync(&stage_fd)?;
    drop(stage_fd);
    remove_empty_directory_exact(parent, stage_name, expected_marker.stage_identity)
}

fn remove_owned_flat_payload(
    stage_root_fd: &OwnedFd,
    expected: &BTreeMap<String, Sha256Digest>,
) -> std::io::Result<()> {
    let payload_fd = rfs::openat(
        stage_root_fd,
        Path::new("payload"),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let payload_identity = identity_from_stat(&rfs::fstat(&payload_fd)?);
    let names = directory_entry_names(&payload_fd)?;
    for name in &names {
        let Some(name_str) = name.to_str() else {
            return Err(io_failure(PublicationError::rollback(
                "interrupted payload contains a non-UTF-8 entry; stage retained",
            )));
        };
        if !expected.contains_key(name_str) {
            return Err(io_failure(PublicationError::rollback(format!(
                "interrupted payload contains unowned member '{name_str}'; stage retained"
            ))));
        }
        let (identity, bytes) = read_regular_at(&payload_fd, name, "interrupted payload")?;
        remove_regular_at_exact(&payload_fd, name, identity, &sha256_bytes(&bytes))?;
    }
    rfs::fsync(&payload_fd)?;
    drop(payload_fd);
    remove_empty_directory_at_exact(stage_root_fd, OsStr::new("payload"), payload_identity)
}

pub(super) fn encode_tree_stage_marker(marker: &TreeStageMarker) -> std::io::Result<Vec<u8>> {
    let json = serde_json::to_vec(marker).map_err(std::io::Error::other)?;
    let mut bytes = Vec::with_capacity(TREE_STAGE_MAGIC.len() + json.len());
    bytes.extend_from_slice(TREE_STAGE_MAGIC);
    bytes.extend_from_slice(&json);
    Ok(bytes)
}

fn parse_tree_stage_marker(bytes: &[u8]) -> std::io::Result<TreeStageMarker> {
    let json = bytes.strip_prefix(TREE_STAGE_MAGIC).ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            "tree stage owner marker has no owned schema prefix",
        )
    })?;
    serde_json::from_slice(json).map_err(std::io::Error::other)
}

pub(super) fn verify_flat_tree_members(
    directory: &OwnedFd,
    expected: &BTreeMap<String, Sha256Digest>,
) -> std::io::Result<()> {
    let names = directory_entry_names(directory)?;
    if names.len() != expected.len() {
        return Err(std::io::Error::other(format!(
            "tree member count mismatch: expected {}, found {}",
            expected.len(),
            names.len()
        )));
    }
    for name in names {
        let name_str = name.to_str().ok_or_else(|| {
            std::io::Error::new(ErrorKind::InvalidInput, "tree contains non-UTF-8 member")
        })?;
        PortableOutputName::new(name_str)?;
        let expected_digest = expected
            .get(name_str)
            .ok_or_else(|| std::io::Error::other(format!("unexpected tree member '{name_str}'")))?;
        let (_, bytes) = read_regular_at(directory, &name, "tree member")?;
        if &sha256_bytes(&bytes) != expected_digest {
            return Err(std::io::Error::other(format!(
                "tree member digest mismatch for '{name_str}'"
            )));
        }
    }
    Ok(())
}

fn directory_entry_names(directory: &OwnedFd) -> std::io::Result<BTreeSet<OsString>> {
    directory_entry_names_capped(directory, None)
}

fn directory_entry_names_capped(
    directory: &OwnedFd,
    limit: Option<usize>,
) -> std::io::Result<BTreeSet<OsString>> {
    let mut names = BTreeSet::new();
    for entry in rfs::Dir::read_from(directory)? {
        let entry = entry?;
        if matches!(entry.file_name().to_bytes(), b"." | b"..") {
            continue;
        }
        if let Some(limit) = limit {
            if names.len() == limit {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("tree directory exceeds the {limit}-entry traversal limit"),
                ));
            }
        }
        names.insert(OsStr::from_bytes(entry.file_name().to_bytes()).to_os_string());
    }
    Ok(names)
}

#[derive(Clone, Copy)]
struct TreeMeasurementBudget {
    limits: Option<TreeTraversalLimits>,
    entries: usize,
    regular_file_bytes: u64,
}

impl TreeMeasurementBudget {
    const fn unrestricted() -> Self {
        Self {
            limits: None,
            entries: 0,
            regular_file_bytes: 0,
        }
    }

    const fn bounded(limits: TreeTraversalLimits) -> Self {
        Self {
            limits: Some(limits),
            entries: 0,
            regular_file_bytes: 0,
        }
    }

    fn consume_entry(&mut self) -> std::io::Result<()> {
        if let Some(limits) = self.limits {
            if self.entries == limits.max_entries {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "tree exceeds the {}-entry traversal limit",
                        limits.max_entries
                    ),
                ));
            }
        }
        self.entries = self
            .entries
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("tree entry count overflowed"))?;
        Ok(())
    }

    fn reserve_regular_file_bytes(&mut self, size: i64, path: &Path) -> std::io::Result<()> {
        let size = u64::try_from(size).map_err(|_| {
            std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("tree file '{}' has a negative size", path.display()),
            )
        })?;
        let next = self
            .regular_file_bytes
            .checked_add(size)
            .ok_or_else(|| std::io::Error::other("tree regular-file byte count overflowed"))?;
        if let Some(limits) = self.limits {
            if next > limits.max_regular_file_bytes {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "tree exceeds the {}-byte regular-file traversal limit",
                        limits.max_regular_file_bytes
                    ),
                ));
            }
        }
        self.regular_file_bytes = next;
        Ok(())
    }
}

fn directory_entry_names_with_budget(
    directory: &OwnedFd,
    budget: &mut TreeMeasurementBudget,
) -> std::io::Result<BTreeSet<OsString>> {
    let mut names = BTreeSet::new();
    for entry in rfs::Dir::read_from(directory)? {
        let entry = entry?;
        if matches!(entry.file_name().to_bytes(), b"." | b"..") {
            continue;
        }
        budget.consume_entry()?;
        names.insert(OsStr::from_bytes(entry.file_name().to_bytes()).to_os_string());
    }
    Ok(names)
}

fn read_regular_at(
    parent: &OwnedFd,
    name: &OsStr,
    context: &str,
) -> std::io::Result<(FileIdentity, Vec<u8>)> {
    let fd = rfs::openat(
        parent,
        Path::new(name),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let before = rfs::fstat(&fd)?;
    if !rfs::FileType::from_raw_mode(before.st_mode).is_file() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "{context} '{}' is not a regular file",
                name.to_string_lossy()
            ),
        ));
    }
    let mut file = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)?;
    let after = rfs::fstat(&file)?;
    if !same_regular_snapshot(&before, &after, bytes.len()) {
        return Err(std::io::Error::other(format!(
            "{context} '{}' changed while reading",
            name.to_string_lossy()
        )));
    }
    Ok((identity_from_stat(&before), bytes))
}

fn remove_regular_at_exact(
    parent: &OwnedFd,
    name: &OsStr,
    expected_identity: FileIdentity,
    expected_digest: &Sha256Digest,
) -> std::io::Result<()> {
    let (identity, bytes) = read_regular_at(parent, name, "owned cleanup entry")?;
    if identity != expected_identity || &sha256_bytes(&bytes) != expected_digest {
        return Err(std::io::Error::other(format!(
            "refusing to remove changed owned entry '{}'",
            name.to_string_lossy()
        )));
    }
    rfs::unlinkat(parent, Path::new(name), AtFlags::empty())?;
    rfs::fsync(parent)?;
    Ok(())
}

fn remove_empty_directory_exact(
    parent: &ParentHandle,
    name: &OsStr,
    expected_identity: FileIdentity,
) -> std::io::Result<()> {
    remove_empty_directory_at_exact(&parent.fd, name, expected_identity)?;
    rfs::fsync(&parent.fd)?;
    Ok(())
}

fn remove_empty_directory_at_exact(
    parent: &OwnedFd,
    name: &OsStr,
    expected_identity: FileIdentity,
) -> std::io::Result<()> {
    if directory_identity_at(parent, name)? != Some(expected_identity) {
        return Err(std::io::Error::other(format!(
            "refusing to remove identity-mismatched directory '{}'",
            name.to_string_lossy()
        )));
    }
    let directory = rfs::openat(
        parent,
        Path::new(name),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    remove_open_empty_directory_at_exact(parent, name, &directory, expected_identity)
}

fn remove_open_empty_directory_at_exact(
    parent: &OwnedFd,
    name: &OsStr,
    directory: &OwnedFd,
    expected_identity: FileIdentity,
) -> std::io::Result<()> {
    if identity_from_stat(&rfs::fstat(directory)?) != expected_identity
        || !directory_entry_names(directory)?.is_empty()
    {
        return Err(std::io::Error::other(format!(
            "refusing to remove changed or non-empty directory '{}'",
            name.to_string_lossy()
        )));
    }
    if directory_identity_at(parent, name)? != Some(expected_identity) {
        return Err(std::io::Error::other(format!(
            "directory '{}' changed immediately before removal",
            name.to_string_lossy()
        )));
    }
    rfs::unlinkat(parent, Path::new(name), AtFlags::REMOVEDIR)?;
    rfs::fsync(parent)?;
    Ok(())
}

pub(super) fn directory_identity_at(
    parent: &impl std::os::fd::AsFd,
    leaf: &OsStr,
) -> std::io::Result<Option<FileIdentity>> {
    match rfs::statat(parent, Path::new(leaf), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) if rfs::FileType::from_raw_mode(stat.st_mode).is_dir() => {
            Ok(Some(identity_from_stat(&stat)))
        }
        Ok(_) => Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("'{}' is not a directory", leaf.to_string_lossy()),
        )),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreparedNodeKind {
    File,
    Directory,
    Symlink,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PreparedNodeSnapshot {
    identity: FileIdentity,
    kind: PreparedNodeKind,
    size: i64,
    mtime: i64,
    mtime_nsec: i64,
    ctime: i64,
    ctime_nsec: i64,
}

pub(super) fn sync_prepared_tree(
    directory: &OwnedFd,
    display_path: &Path,
    name_policy: PreparedTreeNamePolicy,
) -> std::io::Result<()> {
    let directory_identity = identity_from_stat(&rfs::fstat(directory)?);
    let before = snapshot_prepared_directory(directory, display_path, name_policy)?;
    for (name, snapshot) in &before {
        let child_display = display_path.join(name);
        match snapshot.kind {
            PreparedNodeKind::File => {
                let fd = rfs::openat(
                    directory,
                    Path::new(name),
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                let opened = prepared_snapshot(&rfs::fstat(&fd)?)?;
                if opened != *snapshot {
                    return Err(std::io::Error::other(format!(
                        "prepared file '{}' changed before sync",
                        child_display.display()
                    )));
                }
                rfs::fsync(&fd)?;
                if prepared_snapshot(&rfs::fstat(&fd)?)? != *snapshot {
                    return Err(std::io::Error::other(format!(
                        "prepared file '{}' changed while syncing",
                        child_display.display()
                    )));
                }
            }
            PreparedNodeKind::Directory => {
                let fd = rfs::openat(
                    directory,
                    Path::new(name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                if prepared_snapshot(&rfs::fstat(&fd)?)? != *snapshot {
                    return Err(std::io::Error::other(format!(
                        "prepared directory '{}' changed before traversal",
                        child_display.display()
                    )));
                }
                sync_prepared_tree(&fd, &child_display, name_policy)?;
                if prepared_snapshot(&rfs::fstat(&fd)?)? != *snapshot {
                    return Err(std::io::Error::other(format!(
                        "prepared directory '{}' changed while syncing",
                        child_display.display()
                    )));
                }
            }
            PreparedNodeKind::Symlink => {
                // Symlinks have no portable fsync operation. Their link
                // objects are made durable by syncing the containing
                // directory below; they are never followed.
            }
        }
    }
    rfs::fsync(directory)?;
    if identity_from_stat(&rfs::fstat(directory)?) != directory_identity
        || snapshot_prepared_directory(directory, display_path, name_policy)? != before
    {
        return Err(std::io::Error::other(format!(
            "prepared directory '{}' changed while syncing",
            display_path.display()
        )));
    }
    Ok(())
}

fn snapshot_prepared_directory(
    directory: &OwnedFd,
    display_path: &Path,
    name_policy: PreparedTreeNamePolicy,
) -> std::io::Result<BTreeMap<OsString, PreparedNodeSnapshot>> {
    let mut entries = BTreeMap::new();
    let mut folded = BTreeSet::new();
    for entry in rfs::Dir::read_from(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let name_os = OsStr::from_bytes(name.to_bytes());
        let collision_key = prepared_tree_name_collision_key(name_os, name_policy)?;
        if !folded.insert(collision_key) {
            return Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                format!(
                    "prepared tree '{}' contains a case-folded name collision",
                    display_path.display()
                ),
            ));
        }
        let stat = rfs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let snapshot = prepared_snapshot(&stat)?;
        if entries.insert(name_os.to_os_string(), snapshot).is_some() {
            return Err(std::io::Error::other("duplicate prepared-tree entry"));
        }
    }
    Ok(entries)
}

fn prepared_tree_name_collision_key(
    name: &OsStr,
    policy: PreparedTreeNamePolicy,
) -> std::io::Result<Vec<u8>> {
    match policy {
        PreparedTreeNamePolicy::PortableGeneratedOutput => {
            let name = name.to_str().ok_or_else(|| {
                std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "generated prepared tree contains a non-UTF-8 name",
                )
            })?;
            Ok(PortableOutputName::new(name)?
                .as_str()
                .to_ascii_lowercase()
                .into_bytes())
        }
        PreparedTreeNamePolicy::PreservedSource => preserved_source_name_collision_key(name),
    }
}

fn preserved_source_name_collision_key(name: &OsStr) -> std::io::Result<Vec<u8>> {
    let bytes = name.as_bytes();
    let unsafe_bytes = bytes.is_empty()
        || matches!(bytes, b"." | b"..")
        || bytes
            .iter()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b'/' | b'\\'));
    let unsafe_unicode = name
        .to_str()
        .is_some_and(|value| value.chars().any(char::is_control));
    if unsafe_bytes || unsafe_unicode {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "preserved source tree contains unsafe path component '{}'",
                name.to_string_lossy()
            ),
        ));
    }

    if let Some(name) = name.to_str() {
        return Ok(name
            .chars()
            .flat_map(char::to_lowercase)
            .collect::<String>()
            .into_bytes());
    }
    Ok(bytes.iter().map(u8::to_ascii_lowercase).collect())
}

fn prepared_snapshot(stat: &rfs::Stat) -> std::io::Result<PreparedNodeSnapshot> {
    let file_type = rfs::FileType::from_raw_mode(stat.st_mode);
    let kind = if file_type.is_file() {
        PreparedNodeKind::File
    } else if file_type.is_dir() {
        PreparedNodeKind::Directory
    } else if file_type.is_symlink() {
        PreparedNodeKind::Symlink
    } else {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "prepared tree contains an unsupported special filesystem object",
        ));
    };
    // Darwin exposes signed nanosecond fields while Linux exposes unsigned
    // fields through rustix. Keep the persisted snapshot signed and reject a
    // theoretical Linux value that cannot be represented.
    #[allow(
        clippy::useless_conversion,
        reason = "rustix timestamp signedness differs between supported Unix targets"
    )]
    let mtime_nsec = i64::try_from(stat.st_mtime_nsec)
        .map_err(|_| std::io::Error::new(ErrorKind::InvalidData, "mtime nanoseconds exceed i64"))?;
    #[allow(
        clippy::useless_conversion,
        reason = "rustix timestamp signedness differs between supported Unix targets"
    )]
    let ctime_nsec = i64::try_from(stat.st_ctime_nsec)
        .map_err(|_| std::io::Error::new(ErrorKind::InvalidData, "ctime nanoseconds exceed i64"))?;
    Ok(PreparedNodeSnapshot {
        identity: identity_from_stat(stat),
        kind,
        size: stat.st_size,
        mtime: stat.st_mtime,
        mtime_nsec,
        ctime: stat.st_ctime,
        ctime_nsec,
    })
}

fn tree_node_snapshot(stat: &rfs::Stat) -> std::io::Result<TreeNodeSnapshot> {
    let prepared = prepared_snapshot(stat)?;
    #[allow(
        clippy::useless_conversion,
        reason = "rustix mode_t width differs between supported Unix targets"
    )]
    let mode = u32::from(stat.st_mode);
    Ok(TreeNodeSnapshot {
        identity: prepared.identity,
        kind: match prepared.kind {
            PreparedNodeKind::File => 1,
            PreparedNodeKind::Directory => 2,
            PreparedNodeKind::Symlink => 3,
        },
        mode,
        size: prepared.size,
        mtime: prepared.mtime,
        mtime_nsec: prepared.mtime_nsec,
        ctime: prepared.ctime,
        ctime_nsec: prepared.ctime_nsec,
    })
}

fn measure_tree_content_at(
    directory: &OwnedFd,
    display_path: &Path,
    prefix: &[u8],
    budget: &mut TreeMeasurementBudget,
) -> std::io::Result<BTreeMap<Vec<u8>, TreeContentEntry>> {
    let directory_before = rfs::fstat(directory)?;
    let directory_identity = identity_from_stat(&directory_before);
    let names = directory_entry_names_with_budget(directory, budget)?;
    let mut entries = BTreeMap::new();
    for name in names {
        let name_bytes = name.as_bytes();
        if name_bytes.contains(&b'/') || name_bytes.is_empty() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "tree contains an invalid filesystem component",
            ));
        }
        let mut relative = prefix.to_owned();
        if !relative.is_empty() {
            relative.push(b'/');
        }
        relative.extend_from_slice(name_bytes);
        let child_display = display_path.join(&name);
        let stat_before = rfs::statat(directory, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW)?;
        let prepared = prepared_snapshot(&stat_before)?;
        let snapshot = tree_node_snapshot(&stat_before)?;
        let content = match prepared.kind {
            PreparedNodeKind::File => {
                budget.reserve_regular_file_bytes(prepared.size, &child_display)?;
                let fd = rfs::openat(
                    directory,
                    Path::new(&name),
                    OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                if prepared_snapshot(&rfs::fstat(&fd)?)? != prepared {
                    return Err(std::io::Error::other(format!(
                        "tree file '{}' changed before hashing",
                        child_display.display()
                    )));
                }
                let mut file = std::fs::File::from(fd);
                let digest = sha256_reader(&mut file)?.digest;
                if prepared_snapshot(&rfs::fstat(&file)?)? != prepared {
                    return Err(std::io::Error::other(format!(
                        "tree file '{}' changed while hashing",
                        child_display.display()
                    )));
                }
                Some(digest)
            }
            PreparedNodeKind::Symlink => {
                let target = rfs::readlinkat(directory, Path::new(&name), Vec::new())?;
                if prepared_snapshot(&rfs::statat(
                    directory,
                    Path::new(&name),
                    AtFlags::SYMLINK_NOFOLLOW,
                )?)? != prepared
                {
                    return Err(std::io::Error::other(format!(
                        "tree link '{}' changed while hashing",
                        child_display.display()
                    )));
                }
                Some(sha256_bytes(target.as_bytes()))
            }
            PreparedNodeKind::Directory => {
                let fd = rfs::openat(
                    directory,
                    Path::new(&name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                if prepared_snapshot(&rfs::fstat(&fd)?)? != prepared {
                    return Err(std::io::Error::other(format!(
                        "tree directory '{}' changed before traversal",
                        child_display.display()
                    )));
                }
                let children = measure_tree_content_at(&fd, &child_display, &relative, budget)?;
                if prepared_snapshot(&rfs::fstat(&fd)?)? != prepared {
                    return Err(std::io::Error::other(format!(
                        "tree directory '{}' changed while traversing",
                        child_display.display()
                    )));
                }
                for (path, child) in children {
                    if entries.insert(path, child).is_some() {
                        return Err(std::io::Error::other("duplicate tree entry"));
                    }
                }
                None
            }
        };
        if entries
            .insert(relative, TreeContentEntry { snapshot, content })
            .is_some()
        {
            return Err(std::io::Error::other("duplicate tree entry"));
        }
    }
    if identity_from_stat(&rfs::fstat(directory)?) != directory_identity
        || directory_entry_names_capped(directory, budget.limits.map(|limits| limits.max_entries))?
            != directory_entry_names_from_keys(&entries, prefix)
    {
        return Err(std::io::Error::other(format!(
            "tree directory '{}' changed while measuring content",
            display_path.display()
        )));
    }
    Ok(entries)
}

pub(super) fn stable_measure_tree_content_at(
    directory: &OwnedFd,
    display_path: &Path,
) -> std::io::Result<BTreeMap<Vec<u8>, TreeContentEntry>> {
    stable_measure_tree_content_at_bounded(directory, display_path, None)
}

pub(super) fn stable_measure_tree_content_at_bounded(
    directory: &OwnedFd,
    display_path: &Path,
    limits: Option<TreeTraversalLimits>,
) -> std::io::Result<BTreeMap<Vec<u8>, TreeContentEntry>> {
    let mut first_budget = limits.map_or_else(
        TreeMeasurementBudget::unrestricted,
        TreeMeasurementBudget::bounded,
    );
    let first = measure_tree_content_at(directory, display_path, &[], &mut first_budget)?;
    test_pause_point("tree-content-cas-between-passes");
    let mut second_budget = limits.map_or_else(
        TreeMeasurementBudget::unrestricted,
        TreeMeasurementBudget::bounded,
    );
    let second = measure_tree_content_at(directory, display_path, &[], &mut second_budget)?;
    if first != second {
        return Err(std::io::Error::other(format!(
            "tree '{}' changed between complete content measurement passes",
            display_path.display()
        )));
    }
    Ok(second)
}

fn directory_entry_names_from_keys(
    entries: &BTreeMap<Vec<u8>, TreeContentEntry>,
    prefix: &[u8],
) -> BTreeSet<OsString> {
    let mut names = BTreeSet::new();
    for path in entries.keys() {
        let remainder = if prefix.is_empty() {
            path.as_slice()
        } else {
            path.strip_prefix(prefix)
                .and_then(|value| value.strip_prefix(b"/"))
                .unwrap_or_default()
        };
        let name = remainder
            .split(|byte| *byte == b'/')
            .next()
            .unwrap_or_default();
        if !name.is_empty() {
            names.insert(OsStr::from_bytes(name).to_os_string());
        }
    }
    names
}
