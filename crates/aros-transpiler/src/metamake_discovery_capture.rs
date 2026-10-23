//! Bounded filesystem capture for MetaMake source discovery.
//!
//! This records filesystem names, kinds, and symlink target bytes only. It is
//! not a content snapshot, source identity proof, or authorization to follow
//! links outside the measured source tree.

use super::{charge, inventory_relative_path, ConfiguredProject, InventoryEntry};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};
use walkdir::WalkDir;

/// Captured logical MetaMake inputs and their physical in-tree referents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedProjectDiscovery {
    inputs: BTreeMap<String, String>,
}

impl CapturedProjectDiscovery {
    /// Capture a bounded no-follow metadata inventory and resolve it with the
    /// same algorithm used by the existing source discovery projection.
    ///
    /// Excluded names are exact root-relative namespaces. Their contents are
    /// omitted from the inventory; any link that refers into them is refused
    /// by the resolver as unmeasured.
    ///
    /// # Errors
    /// Refuses unsupported paths, changed/nonregular entries, invalid links,
    /// incomplete inventory and bounded resource excess. No content is read.
    pub fn capture(
        root: &Path,
        project: &ConfiguredProject,
        excluded: &BTreeSet<String>,
    ) -> Result<Self, String> {
        #[cfg(unix)]
        {
            capture_unix(root, project, excluded)
        }
        #[cfg(not(unix))]
        {
            let _ = (root, project, excluded);
            Err("MetaMake discovery capture requires Unix no-follow link metadata".into())
        }
    }

    /// Borrow the logical source input -> original physical in-tree path map.
    #[must_use]
    pub const fn inputs(&self) -> &BTreeMap<String, String> {
        &self.inputs
    }

    /// Consume the capture and return its logical source input map.
    #[must_use]
    pub fn into_inputs(self) -> BTreeMap<String, String> {
        self.inputs
    }
}

#[cfg(unix)]
fn capture_unix(
    root: &Path,
    project: &ConfiguredProject,
    excluded: &BTreeSet<String>,
) -> Result<CapturedProjectDiscovery, String> {
    use std::os::unix::ffi::OsStrExt;

    if excluded.len() > 200_000 {
        return Err("MetaMake discovery exclusion count exceeds limit".into());
    }
    super::single_component(&project.default_makefile)?;

    let mut retained = 0usize;
    let mut exclusions = ExcludedNamespaces::default();
    for name in excluded {
        inventory_relative_path(name)?;
        exclusions.insert(name, &mut retained)?;
    }

    // Canonicalizing only the root gives a stable lexical base for the walk;
    // all descendants are still inspected with no-follow metadata.
    let root = fs::canonicalize(root)
        .map_err(|error| format!("cannot canonicalize MetaMake source root: {error}"))?;
    let root_metadata = fs::symlink_metadata(&root)
        .map_err(|error| format!("cannot inspect MetaMake source root: {error}"))?;
    if !root_metadata.file_type().is_dir() {
        return Err("MetaMake source root is not a directory".into());
    }

    let walker = WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || entry
                    .path()
                    .strip_prefix(&root)
                    .is_ok_and(|relative| !exclusions.contains(relative))
        });

    let mut inventory = BTreeMap::new();
    let mut targets = BTreeMap::new();
    let mut entries = 0usize;

    for entry in walker {
        let entry = entry.map_err(|error| format!("cannot capture MetaMake discovery: {error}"))?;
        if entry.depth() == 0 {
            continue;
        }
        entries = entries
            .checked_add(1)
            .filter(|count| *count <= 200_000)
            .ok_or("MetaMake discovery inventory exceeds limit")?;

        let relative = entry
            .path()
            .strip_prefix(&root)
            .map_err(|_| "MetaMake discovery entry escaped its source root")?;
        let name = relative_name(relative)?;

        let before = fs::symlink_metadata(entry.path())
            .map_err(|error| format!("cannot inspect MetaMake discovery entry {name}: {error}"))?;
        let kind = classify(&before).ok_or_else(|| {
            format!("unsupported special file in MetaMake discovery inventory: {name}")
        })?;
        if !same_kind(entry.file_type(), before.file_type()) {
            return Err(format!(
                "MetaMake discovery entry changed during capture: {name}"
            ));
        }

        match kind {
            InventoryEntry::Link => {
                // On Unix lstat reports a symlink's target byte length. Check
                // before read_link so oversized targets are rejected before
                // allocating their contents where the platform permits it.
                if before.len() > super::MAX_VALUE_BYTES as u64 {
                    return Err("MetaMake discovery link target exceeds limit".into());
                }
                charge(&mut retained, name.len(), super::MAX_RETAINED_BYTES)?;
                charge(&mut retained, name.len(), super::MAX_RETAINED_BYTES)?;
                charge(
                    &mut retained,
                    before.len() as usize,
                    super::MAX_RETAINED_BYTES,
                )?;

                let target = fs::read_link(entry.path()).map_err(|error| {
                    format!("cannot capture MetaMake link target {name}: {error}")
                })?;
                let target_bytes = target.as_os_str().as_bytes();
                if target_bytes.len() > super::MAX_VALUE_BYTES {
                    return Err("MetaMake discovery link target exceeds limit".into());
                }
                let after = fs::symlink_metadata(entry.path()).map_err(|error| {
                    format!("cannot recheck MetaMake link during capture {name}: {error}")
                })?;
                if !same_symlink(&before, &after) || target_bytes.len() as u64 != before.len() {
                    return Err(format!(
                        "MetaMake link changed during discovery capture: {name}"
                    ));
                }

                let captured = target_bytes.to_vec();
                if inventory
                    .insert(name.to_owned(), InventoryEntry::Link)
                    .is_some()
                {
                    return Err("duplicate MetaMake discovery path".into());
                }
                targets.insert(name.to_owned(), captured);
            }
            InventoryEntry::Directory | InventoryEntry::RegularFile => {
                charge(&mut retained, name.len(), super::MAX_RETAINED_BYTES)?;
                if inventory.insert(name.to_owned(), kind).is_some() {
                    return Err("duplicate MetaMake discovery path".into());
                }
            }
            InventoryEntry::Other => unreachable!("classify excludes special files"),
        }
    }

    let resolved = super::discovery_links::resolve_source_discovery_excluding(
        project, &inventory, &targets, excluded,
    )?;
    for path in resolved.inventory.keys() {
        charge(&mut retained, path.len(), super::MAX_RETAINED_BYTES)?;
    }
    let inputs = resolved.original_inputs;
    for (logical, physical) in &inputs {
        charge(&mut retained, logical.len(), super::MAX_RETAINED_BYTES)?;
        charge(&mut retained, physical.len(), super::MAX_RETAINED_BYTES)?;
    }
    Ok(CapturedProjectDiscovery { inputs })
}

#[cfg(unix)]
fn relative_name(relative: &Path) -> Result<&str, String> {
    if relative.as_os_str().len() > super::MAX_VALUE_BYTES {
        return Err("MetaMake discovery path exceeds limit".into());
    }
    let name = relative
        .to_str()
        .ok_or("non-UTF-8 MetaMake discovery path")?;
    inventory_relative_path(name)?;
    Ok(name)
}

#[cfg(unix)]
fn classify(metadata: &fs::Metadata) -> Option<InventoryEntry> {
    let kind = metadata.file_type();
    if kind.is_dir() {
        Some(InventoryEntry::Directory)
    } else if kind.is_file() {
        Some(InventoryEntry::RegularFile)
    } else if kind.is_symlink() {
        Some(InventoryEntry::Link)
    } else {
        None
    }
}

#[cfg(unix)]
fn same_kind(walked: fs::FileType, measured: fs::FileType) -> bool {
    walked.is_dir() == measured.is_dir()
        && walked.is_file() == measured.is_file()
        && walked.is_symlink() == measured.is_symlink()
}

#[cfg(unix)]
fn same_symlink(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    let before_kind = before.file_type();
    let after_kind = after.file_type();
    before_kind.is_symlink()
        && after_kind.is_symlink()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
}

#[derive(Default)]
struct ExcludedNamespaces {
    root: ExcludedNode,
}

#[derive(Default)]
struct ExcludedNode {
    terminal: bool,
    children: BTreeMap<String, Self>,
}

impl ExcludedNamespaces {
    fn insert(&mut self, name: &str, retained: &mut usize) -> Result<(), String> {
        let mut node = &mut self.root;
        for component in name.split('/') {
            if !node.children.contains_key(component) {
                charge(retained, component.len(), super::MAX_RETAINED_BYTES)?;
                node.children
                    .insert(component.to_owned(), ExcludedNode::default());
            }
            node = node.children.get_mut(component).expect("inserted above");
        }
        node.terminal = true;
        Ok(())
    }

    fn contains(&self, path: &Path) -> bool {
        let mut node = &self.root;
        for component in path.components() {
            let Component::Normal(name) = component else {
                return false;
            };
            let Some(name) = name.to_str() else {
                return false;
            };
            let Some(next) = node.children.get(name) else {
                return false;
            };
            node = next;
            if node.terminal {
                return true;
            }
        }
        node.terminal
    }
}

#[cfg(test)]
#[path = "metamake_discovery_capture_tests.rs"]
mod tests;
