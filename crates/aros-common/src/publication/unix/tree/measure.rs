//! Bounded content measurement and source-tree link resolution.

use super::*;

pub(super) const SOURCE_TREE_MAX_DEPTH: usize = 128;

pub(super) fn measure_tree_content_at(
    directory: &OwnedFd,
    display_path: &Path,
    prefix: &[u8],
    budget: &mut TreeMeasurementBudget,
) -> std::io::Result<BTreeMap<Vec<u8>, TreeContentEntry>> {
    let depth = if prefix.is_empty() {
        0
    } else {
        prefix.split(|byte| *byte == b'/').count()
    };
    if depth > SOURCE_TREE_MAX_DEPTH {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("tree exceeds the {SOURCE_TREE_MAX_DEPTH}-component depth limit"),
        ));
    }
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
        let entry_depth = relative.split(|byte| *byte == b'/').count();
        if entry_depth > SOURCE_TREE_MAX_DEPTH {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("tree exceeds the {SOURCE_TREE_MAX_DEPTH}-component depth limit"),
            ));
        }
        let child_display = display_path.join(&name);
        let stat_before = rfs::statat(directory, Path::new(&name), AtFlags::SYMLINK_NOFOLLOW)?;
        let prepared = prepared_snapshot(&stat_before)?;
        let snapshot = tree_node_snapshot(&stat_before)?;
        let content = match prepared.kind {
            PreparedNodeKind::File => {
                budget.reserve_regular_file_bytes(prepared.size, &child_display)?;
                let expected_size = u64::try_from(snapshot.size).map_err(|_| {
                    std::io::Error::new(
                        ErrorKind::InvalidInput,
                        format!(
                            "tree file '{}' has a negative size",
                            child_display.display()
                        ),
                    )
                })?;
                let read_limit = expected_size
                    .checked_add(1)
                    .ok_or_else(|| std::io::Error::other("tree file read limit overflowed"))?;
                test_pause_point("tree-content-cas-before-file-open");
                let fd = rfs::openat(
                    directory,
                    Path::new(&name),
                    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                if tree_node_snapshot(&rfs::fstat(&fd)?)? != snapshot {
                    return Err(std::io::Error::other(format!(
                        "tree file '{}' changed before hashing",
                        child_display.display()
                    )));
                }
                let mut file = std::fs::File::from(fd);
                let measured =
                    sha256_reader(&mut std::io::Read::by_ref(&mut file).take(read_limit))?;
                if measured.size != expected_size
                    || tree_node_snapshot(&rfs::fstat(&file)?)? != snapshot
                    || tree_node_snapshot(&rfs::statat(
                        directory,
                        Path::new(&name),
                        AtFlags::SYMLINK_NOFOLLOW,
                    )?)? != snapshot
                {
                    return Err(std::io::Error::other(format!(
                        "tree file '{}' changed while hashing",
                        child_display.display()
                    )));
                }
                Some(measured.digest)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceTreeNode {
    snapshot: TreeNodeSnapshot,
    content: Option<Sha256Digest>,
    link_target: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceTreePass {
    root_snapshot: TreeNodeSnapshot,
    nodes: BTreeMap<Vec<u8>, SourceTreeNode>,
    excluded_roots: BTreeSet<Vec<u8>>,
}

pub(in crate::publication::unix) fn measure_source_tree_content_cas_bounded(
    path: &Path,
    generated_subtree: Option<&[Vec<u8>]>,
    limits: TreeTraversalLimits,
) -> std::io::Result<TreeContentCas> {
    let parent = open_parent(path, false)?;
    let directory = rfs::openat(
        &parent.fd,
        &parent.leaf,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let root = identity_from_stat(&rfs::fstat(&directory)?);
    let entries =
        stable_measure_source_tree_content_at_bounded(&directory, path, generated_subtree, limits)?;
    if identity_from_stat(&rfs::fstat(&directory)?) != root
        || directory_identity_at(&parent.fd, &parent.leaf)? != Some(root)
    {
        return Err(std::io::Error::other(format!(
            "source tree '{}' changed while it was measured",
            path.display()
        )));
    }
    Ok(TreeContentCas { root, entries })
}

pub(super) fn stable_measure_source_tree_content_at_bounded(
    directory: &OwnedFd,
    display_path: &Path,
    generated_subtree: Option<&[Vec<u8>]>,
    limits: TreeTraversalLimits,
) -> std::io::Result<BTreeMap<Vec<u8>, TreeContentEntry>> {
    let first = measure_source_tree_pass(directory, display_path, generated_subtree, limits)?;
    test_pause_point("source-tree-content-cas-between-passes");
    let second = measure_source_tree_pass(directory, display_path, generated_subtree, limits)?;
    if first != second {
        return Err(std::io::Error::other(format!(
            "source tree '{}' changed between complete filtered measurement passes",
            display_path.display()
        )));
    }
    finalize_source_tree_pass(second, generated_subtree)
}

pub(super) fn measure_source_tree_pass(
    directory: &OwnedFd,
    display_path: &Path,
    generated_subtree: Option<&[Vec<u8>]>,
    limits: TreeTraversalLimits,
) -> std::io::Result<SourceTreePass> {
    let root_snapshot = tree_node_snapshot(&rfs::fstat(directory)?)?;
    if root_snapshot.kind != 2 {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "source tree '{}' is not a real directory",
                display_path.display()
            ),
        ));
    }
    let mut budget = TreeMeasurementBudget::bounded(limits);
    let mut nodes = BTreeMap::new();
    let mut excluded_roots = generated_subtree
        .map(|components| BTreeSet::from([source_tree_path_key(components)]))
        .unwrap_or_default();
    measure_source_tree_contents_at(
        directory,
        display_path,
        &[],
        generated_subtree,
        &mut budget,
        &mut nodes,
        &mut excluded_roots,
    )?;
    if tree_node_snapshot(&rfs::fstat(directory)?)? != root_snapshot {
        return Err(std::io::Error::other(format!(
            "source tree root '{}' changed while measuring",
            display_path.display()
        )));
    }
    Ok(SourceTreePass {
        root_snapshot,
        nodes,
        excluded_roots,
    })
}

pub(super) fn measure_source_tree_contents_at(
    directory: &OwnedFd,
    display_path: &Path,
    prefix: &[Vec<u8>],
    generated_subtree: Option<&[Vec<u8>]>,
    budget: &mut TreeMeasurementBudget,
    nodes: &mut BTreeMap<Vec<u8>, SourceTreeNode>,
    excluded_roots: &mut BTreeSet<Vec<u8>>,
) -> std::io::Result<()> {
    let directory_snapshot = tree_node_snapshot(&rfs::fstat(directory)?)?;
    let names = directory_entry_names_with_budget(directory, budget)?;
    let mut collision_keys = BTreeSet::new();
    for name in &names {
        let name_bytes = name.as_bytes();
        if name_bytes.is_empty() || name_bytes.contains(&b'/') {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "source tree contains an invalid filesystem component",
            ));
        }
        let collision_key = preserved_source_name_collision_key(name)?;
        if !collision_keys.insert(collision_key) {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "source tree directory '{}' contains colliding names",
                    display_path.display()
                ),
            ));
        }
    }

    for name in &names {
        let name_bytes = name.as_bytes();
        if name_bytes == b".git" {
            let mut excluded_components = prefix.to_vec();
            excluded_components.push(name_bytes.to_vec());
            excluded_roots.insert(source_tree_path_key(&excluded_components));
            continue;
        }
        if generated_subtree_matches_child(prefix, name_bytes, generated_subtree) {
            let mut excluded_components = prefix.to_vec();
            excluded_components.push(name_bytes.to_vec());
            excluded_roots.insert(source_tree_path_key(&excluded_components));
            continue;
        }
        let mut components = prefix.to_vec();
        components.push(name_bytes.to_vec());
        if components.len() > SOURCE_TREE_MAX_DEPTH {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("source tree exceeds the {SOURCE_TREE_MAX_DEPTH}-component depth limit"),
            ));
        }
        let relative = source_tree_path_key(&components);
        let child_display = display_path.join(name);
        let stat_before = rfs::statat(directory, Path::new(name), AtFlags::SYMLINK_NOFOLLOW)?;
        let snapshot = tree_node_snapshot(&stat_before)?;
        let generated_path_crosses_non_directory = generated_subtree.is_some_and(|generated| {
            components.len() < generated.len()
                && components
                    .iter()
                    .zip(generated)
                    .all(|(current, excluded)| current == excluded)
                && snapshot.kind != 2
        });
        if generated_path_crosses_non_directory {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "generated subtree path crosses a non-directory at '{}'",
                    child_display.display()
                ),
            ));
        }
        let content = match snapshot.kind {
            1 => {
                budget.reserve_regular_file_bytes(snapshot.size, &child_display)?;
                let fd = rfs::openat(
                    directory,
                    Path::new(name),
                    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                if tree_node_snapshot(&rfs::fstat(&fd)?)? != snapshot {
                    return Err(std::io::Error::other(format!(
                        "source file '{}' changed before hashing",
                        child_display.display()
                    )));
                }
                let mut file = std::fs::File::from(fd);
                let expected_size = u64::try_from(snapshot.size).map_err(|_| {
                    std::io::Error::new(
                        ErrorKind::InvalidInput,
                        format!(
                            "source file '{}' has a negative size",
                            child_display.display()
                        ),
                    )
                })?;
                let measured =
                    sha256_reader(&mut std::io::Read::by_ref(&mut file).take(expected_size))?;
                if measured.size != expected_size
                    || tree_node_snapshot(&rfs::fstat(&file)?)? != snapshot
                    || tree_node_snapshot(&rfs::statat(
                        directory,
                        Path::new(name),
                        AtFlags::SYMLINK_NOFOLLOW,
                    )?)? != snapshot
                {
                    return Err(std::io::Error::other(format!(
                        "source file '{}' changed while hashing",
                        child_display.display()
                    )));
                }
                Some(measured.digest)
            }
            2 => {
                let fd = rfs::openat(
                    directory,
                    Path::new(name),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
                if tree_node_snapshot(&rfs::fstat(&fd)?)? != snapshot {
                    return Err(std::io::Error::other(format!(
                        "source directory '{}' changed before traversal",
                        child_display.display()
                    )));
                }
                measure_source_tree_contents_at(
                    &fd,
                    &child_display,
                    &components,
                    generated_subtree,
                    budget,
                    nodes,
                    excluded_roots,
                )?;
                if tree_node_snapshot(&rfs::fstat(&fd)?)? != snapshot
                    || tree_node_snapshot(&rfs::statat(
                        directory,
                        Path::new(name),
                        AtFlags::SYMLINK_NOFOLLOW,
                    )?)? != snapshot
                {
                    return Err(std::io::Error::other(format!(
                        "source directory '{}' changed while traversing",
                        child_display.display()
                    )));
                }
                None
            }
            3 => {
                let target = rfs::readlinkat(directory, Path::new(name), Vec::new())?;
                if tree_node_snapshot(&rfs::statat(
                    directory,
                    Path::new(name),
                    AtFlags::SYMLINK_NOFOLLOW,
                )?)? != snapshot
                {
                    return Err(std::io::Error::other(format!(
                        "source symlink '{}' changed while measuring",
                        child_display.display()
                    )));
                }
                nodes.insert(
                    relative,
                    SourceTreeNode {
                        snapshot,
                        content: None,
                        link_target: Some(target.as_bytes().to_vec()),
                    },
                );
                continue;
            }
            _ => {
                return Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    format!(
                        "source tree contains an unsupported filesystem object at '{}'",
                        child_display.display()
                    ),
                ))
            }
        };
        if nodes
            .insert(
                relative,
                SourceTreeNode {
                    snapshot,
                    content,
                    link_target: None,
                },
            )
            .is_some()
        {
            return Err(std::io::Error::other("duplicate source tree entry"));
        }
    }

    if tree_node_snapshot(&rfs::fstat(directory)?)? != directory_snapshot
        || directory_entry_names_capped(directory, budget.limits.map(|value| value.max_entries))?
            != names
    {
        return Err(std::io::Error::other(format!(
            "source tree directory '{}' changed while measuring",
            display_path.display()
        )));
    }
    Ok(())
}

pub(super) fn generated_subtree_matches_child(
    prefix: &[Vec<u8>],
    child: &[u8],
    generated_subtree: Option<&[Vec<u8>]>,
) -> bool {
    generated_subtree.is_some_and(|generated| {
        prefix.len() + 1 == generated.len()
            && prefix
                .iter()
                .zip(generated)
                .all(|(current, excluded)| current == excluded)
            && child == generated[prefix.len()]
    })
}

pub(super) fn source_tree_path_key(components: &[Vec<u8>]) -> Vec<u8> {
    let size = components
        .iter()
        .map(Vec::len)
        .sum::<usize>()
        .saturating_add(components.len().saturating_sub(1));
    let mut path = Vec::with_capacity(size);
    for (index, component) in components.iter().enumerate() {
        if index != 0 {
            path.push(b'/');
        }
        path.extend_from_slice(component);
    }
    path
}

pub(super) fn source_tree_path_components(path: &[u8]) -> Vec<Vec<u8>> {
    if path.is_empty() {
        Vec::new()
    } else {
        path.split(|byte| *byte == b'/')
            .map(<[u8]>::to_vec)
            .collect()
    }
}

pub(super) fn source_tree_path_is_excluded(
    components: &[Vec<u8>],
    generated_subtree: Option<&[Vec<u8>]>,
) -> bool {
    components.iter().any(|component| component == b".git")
        || generated_subtree.is_some_and(|generated| {
            components.len() >= generated.len()
                && components
                    .iter()
                    .zip(generated)
                    .all(|(current, excluded)| current == excluded)
        })
}

pub(super) fn validate_source_link_target(path: &[u8], target: &[u8]) -> std::io::Result<()> {
    let unsafe_bytes = target.is_empty()
        || target.starts_with(b"/")
        || target
            .iter()
            .any(|byte| byte.is_ascii_control() || *byte == b'\\');
    let unsafe_unicode =
        std::str::from_utf8(target).is_ok_and(|value| value.chars().any(char::is_control));
    if unsafe_bytes || unsafe_unicode {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "source symlink '{}' has an absolute or unsafe target",
                String::from_utf8_lossy(path)
            ),
        ));
    }
    Ok(())
}

pub(super) fn source_tree_link_target(
    link_path: &[u8],
    nodes: &BTreeMap<Vec<u8>, SourceTreeNode>,
    generated_subtree: Option<&[Vec<u8>]>,
    active_links: &mut BTreeSet<Vec<u8>>,
) -> std::io::Result<Vec<u8>> {
    if active_links.contains(link_path) {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "source symlink cycle includes '{}'",
                String::from_utf8_lossy(link_path)
            ),
        ));
    }
    if active_links.len() >= SOURCE_TREE_MAX_DEPTH {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "source symlink chain exceeds the {SOURCE_TREE_MAX_DEPTH}-link resolution limit"
            ),
        ));
    }
    let node = nodes.get(link_path).ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "source symlink '{}' is missing from the measured inventory",
                String::from_utf8_lossy(link_path)
            ),
        )
    })?;
    let target = node.link_target.as_deref().ok_or_else(|| {
        std::io::Error::new(ErrorKind::InvalidData, "source link has no captured target")
    })?;
    validate_source_link_target(link_path, target)?;
    active_links.insert(link_path.to_vec());
    let mut base = source_tree_path_components(link_path);
    base.pop();
    let resolved = resolve_source_link_target(
        link_path,
        &base,
        target,
        nodes,
        generated_subtree,
        active_links,
    );
    active_links.remove(link_path);
    resolved.map(|components| source_tree_path_key(&components))
}

pub(super) fn resolve_source_link_target(
    link_path: &[u8],
    base: &[Vec<u8>],
    target: &[u8],
    nodes: &BTreeMap<Vec<u8>, SourceTreeNode>,
    generated_subtree: Option<&[Vec<u8>]>,
    active_links: &mut BTreeSet<Vec<u8>>,
) -> std::io::Result<Vec<Vec<u8>>> {
    validate_source_link_target(link_path, target)?;
    let mut current = base.to_vec();
    ensure_source_directory(&current, nodes, link_path)?;
    for component in target.split(|byte| *byte == b'/') {
        ensure_source_directory(&current, nodes, link_path)?;
        if component.is_empty() || component == b"." {
            continue;
        }
        if component == b".." {
            if current.pop().is_none() {
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    format!(
                        "source symlink '{}' escapes the measured root",
                        String::from_utf8_lossy(link_path)
                    ),
                ));
            }
            ensure_source_directory(&current, nodes, link_path)?;
            continue;
        }
        let name = OsStr::from_bytes(component);
        preserved_source_name_collision_key(name)?;
        let mut candidate = current.clone();
        candidate.push(component.to_vec());
        if source_tree_path_is_excluded(&candidate, generated_subtree) {
            return Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                format!(
                    "source symlink '{}' targets an excluded path",
                    String::from_utf8_lossy(link_path)
                ),
            ));
        }
        let candidate_key = source_tree_path_key(&candidate);
        let node = nodes.get(&candidate_key).ok_or_else(|| {
            std::io::Error::new(
                ErrorKind::NotFound,
                format!(
                    "source symlink '{}' has a missing target",
                    String::from_utf8_lossy(link_path)
                ),
            )
        })?;
        if node.snapshot.kind == 3 {
            let resolved =
                source_tree_link_target(&candidate_key, nodes, generated_subtree, active_links)?;
            current = source_tree_path_components(&resolved);
        } else {
            current = candidate;
        }
    }
    ensure_source_inventory_node(&current, nodes, link_path)?;
    Ok(current)
}

pub(super) fn ensure_source_directory(
    path: &[Vec<u8>],
    nodes: &BTreeMap<Vec<u8>, SourceTreeNode>,
    link_path: &[u8],
) -> std::io::Result<()> {
    if path.is_empty()
        || nodes
            .get(&source_tree_path_key(path))
            .is_some_and(|node| node.snapshot.kind == 2)
    {
        return Ok(());
    }
    Err(std::io::Error::new(
        ErrorKind::NotADirectory,
        format!(
            "source symlink '{}' traverses a regular file or missing directory",
            String::from_utf8_lossy(link_path)
        ),
    ))
}

pub(super) fn ensure_source_inventory_node(
    path: &[Vec<u8>],
    nodes: &BTreeMap<Vec<u8>, SourceTreeNode>,
    link_path: &[u8],
) -> std::io::Result<()> {
    if path.is_empty() || nodes.contains_key(&source_tree_path_key(path)) {
        return Ok(());
    }
    Err(std::io::Error::new(
        ErrorKind::NotFound,
        format!(
            "source symlink '{}' has a missing target",
            String::from_utf8_lossy(link_path)
        ),
    ))
}

pub(super) fn finalize_source_tree_pass(
    pass: SourceTreePass,
    generated_subtree: Option<&[Vec<u8>]>,
) -> std::io::Result<BTreeMap<Vec<u8>, TreeContentEntry>> {
    let mut resolved_links = BTreeMap::new();
    for (path, node) in &pass.nodes {
        if node.snapshot.kind == 3 {
            let mut active_links = BTreeSet::new();
            let target =
                source_tree_link_target(path, &pass.nodes, generated_subtree, &mut active_links)?;
            resolved_links.insert(path.clone(), target);
        }
    }
    for (link_path, target_path) in &resolved_links {
        let target_is_directory = target_path.is_empty()
            || pass
                .nodes
                .get(target_path)
                .is_some_and(|node| node.snapshot.kind == 2);
        if target_is_directory
            && pass
                .excluded_roots
                .iter()
                .any(|excluded| source_path_is_ancestor(target_path, excluded))
        {
            return Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                format!(
                    "source symlink '{}' exposes an excluded subtree through a directory target",
                    String::from_utf8_lossy(link_path)
                ),
            ));
        }
    }

    let mut children: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for path in pass.nodes.keys() {
        let separator = path.iter().rposition(|byte| *byte == b'/');
        let (parent, _) = separator.map_or((&[][..], path.as_slice()), |index| {
            (&path[..index], &path[index + 1..])
        });
        children
            .entry(parent.to_vec())
            .or_default()
            .push(path.clone());
    }
    for child_paths in children.values_mut() {
        child_paths.sort_by(|left, right| {
            left.rsplit(|byte| *byte == b'/')
                .next()
                .cmp(&right.rsplit(|byte| *byte == b'/').next())
        });
    }

    let mut digests = BTreeMap::new();
    let mut active_nodes = BTreeSet::new();
    for (path, node) in &pass.nodes {
        if node.snapshot.kind == 3 {
            source_tree_node_digest(
                path,
                &pass.nodes,
                &children,
                &resolved_links,
                &mut active_nodes,
                &mut digests,
            )?;
        }
    }

    let mut entries = BTreeMap::new();
    for (path, node) in pass.nodes {
        let content = if node.snapshot.kind == 3 {
            digests.get(&path).cloned()
        } else {
            node.content
        };
        if entries
            .insert(
                path,
                TreeContentEntry {
                    snapshot: node.snapshot,
                    content,
                },
            )
            .is_some()
        {
            return Err(std::io::Error::other("duplicate filtered source entry"));
        }
    }
    Ok(entries)
}

pub(super) fn source_path_is_ancestor(ancestor: &[u8], descendant: &[u8]) -> bool {
    ancestor.is_empty()
        || ancestor == descendant
        || descendant
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with(b"/"))
}

pub(super) fn source_tree_node_digest(
    path: &[u8],
    nodes: &BTreeMap<Vec<u8>, SourceTreeNode>,
    children: &BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
    resolved_links: &BTreeMap<Vec<u8>, Vec<u8>>,
    active: &mut BTreeSet<Vec<u8>>,
    cached: &mut BTreeMap<Vec<u8>, Sha256Digest>,
) -> std::io::Result<Sha256Digest> {
    if let Some(digest) = cached.get(path) {
        return Ok(digest.clone());
    }
    if !active.insert(path.to_vec()) {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!(
                "source symlink closure contains a cycle at '{}'",
                String::from_utf8_lossy(path)
            ),
        ));
    }
    let result = (|| {
        if path.is_empty() || nodes.get(path).is_some_and(|node| node.snapshot.kind == 2) {
            let mut bytes = b"aros-source-directory-v1\0".to_vec();
            for child_path in children.get(path).into_iter().flatten() {
                let child = nodes.get(child_path).ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidData, "source child is missing")
                })?;
                let name = child_path
                    .rsplit(|byte| *byte == b'/')
                    .next()
                    .unwrap_or_default();
                bytes.extend_from_slice(
                    &u64::try_from(name.len())
                        .map_err(std::io::Error::other)?
                        .to_be_bytes(),
                );
                bytes.extend_from_slice(name);
                bytes.push(child.snapshot.kind);
                let child_digest = source_tree_node_digest(
                    child_path,
                    nodes,
                    children,
                    resolved_links,
                    active,
                    cached,
                )?;
                bytes.extend_from_slice(child_digest.to_string().as_bytes());
            }
            Ok(sha256_bytes(&bytes))
        } else {
            let node = nodes.get(path).ok_or_else(|| {
                std::io::Error::new(ErrorKind::NotFound, "source link target is missing")
            })?;
            match node.snapshot.kind {
                1 => node.content.clone().ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidData, "source file has no digest")
                }),
                3 => {
                    let raw_target = node.link_target.as_deref().ok_or_else(|| {
                        std::io::Error::new(ErrorKind::InvalidData, "source link has no target")
                    })?;
                    let resolved = resolved_links.get(path).ok_or_else(|| {
                        std::io::Error::new(
                            ErrorKind::InvalidData,
                            "source link has no resolved inventory target",
                        )
                    })?;
                    let target_digest = source_tree_node_digest(
                        resolved,
                        nodes,
                        children,
                        resolved_links,
                        active,
                        cached,
                    )?;
                    let mut bytes = b"aros-source-link-v1\0".to_vec();
                    bytes.extend_from_slice(
                        &u64::try_from(raw_target.len())
                            .map_err(std::io::Error::other)?
                            .to_be_bytes(),
                    );
                    bytes.extend_from_slice(raw_target);
                    bytes.extend_from_slice(target_digest.to_string().as_bytes());
                    Ok(sha256_bytes(&bytes))
                }
                _ => Err(std::io::Error::new(
                    ErrorKind::InvalidData,
                    "unsupported source tree node in link closure",
                )),
            }
        }
    })();
    active.remove(path);
    let digest = result?;
    cached.insert(path.to_vec(), digest.clone());
    Ok(digest)
}

pub(in crate::publication::unix) fn stable_measure_tree_content_at(
    directory: &OwnedFd,
    display_path: &Path,
) -> std::io::Result<BTreeMap<Vec<u8>, TreeContentEntry>> {
    stable_measure_tree_content_at_bounded(directory, display_path, None)
}

pub(in crate::publication::unix) fn stable_measure_tree_content_at_bounded(
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

pub(super) fn directory_entry_names_from_keys(
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
