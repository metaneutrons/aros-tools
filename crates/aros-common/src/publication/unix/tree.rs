//! Durable no-clobber tree staging, recovery, and prepared-tree syncing.

use super::*;
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
    let mut names = BTreeSet::new();
    for entry in rfs::Dir::read_from(directory)? {
        let entry = entry?;
        if matches!(entry.file_name().to_bytes(), b"." | b"..") {
            continue;
        }
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
    if !directory_entry_names(&directory)?.is_empty() {
        return Err(std::io::Error::other(format!(
            "refusing to remove non-empty directory '{}'",
            name.to_string_lossy()
        )));
    }
    drop(directory);
    if directory_identity_at(parent, name)? != Some(expected_identity) {
        return Err(std::io::Error::other(format!(
            "directory '{}' changed before removal",
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

pub(super) fn measure_tree_content_at(
    directory: &OwnedFd,
    display_path: &Path,
    prefix: &[u8],
) -> std::io::Result<BTreeMap<Vec<u8>, TreeContentEntry>> {
    let directory_before = rfs::fstat(directory)?;
    let directory_identity = identity_from_stat(&directory_before);
    let names = directory_entry_names(directory)?;
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
        let snapshot = TreeNodeSnapshot {
            identity: prepared.identity,
            kind: match prepared.kind {
                PreparedNodeKind::File => 1,
                PreparedNodeKind::Directory => 2,
                PreparedNodeKind::Symlink => 3,
            },
            mode: u32::from(stat_before.st_mode),
            size: prepared.size,
            mtime: prepared.mtime,
            mtime_nsec: prepared.mtime_nsec,
            ctime: prepared.ctime,
            ctime_nsec: prepared.ctime_nsec,
        };
        let content = match prepared.kind {
            PreparedNodeKind::File => {
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
                let children = measure_tree_content_at(&fd, &child_display, &relative)?;
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
        || directory_entry_names(directory)? != directory_entry_names_from_keys(&entries, prefix)
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
    let first = measure_tree_content_at(directory, display_path, &[])?;
    test_pause_point("tree-content-cas-between-passes");
    let second = measure_tree_content_at(directory, display_path, &[])?;
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
