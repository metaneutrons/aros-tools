//! Pure, bounded projection of native discovery through captured in-tree links.

use super::{
    charge, inventory_relative_path, is_ignored_directory_path, ConfiguredProject, InventoryEntry,
    MAX_RETAINED_BYTES, MAX_VALUE_BYTES,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub(super) struct ResolvedDiscovery {
    pub inventory: BTreeMap<String, InventoryEntry>,
    pub original_inputs: BTreeMap<String, String>,
}

/// The input inventory and link bytes must come from the same retained snapshot.
/// No filesystem state is read, ignored subtree invented, or target followed
/// outside that inventory. Logical alias coordinates remain distinct owners.
#[cfg(test)]
pub(super) fn resolve_source_discovery(
    project: &ConfiguredProject,
    inventory: &BTreeMap<String, InventoryEntry>,
    targets: &BTreeMap<String, Vec<u8>>,
) -> Result<ResolvedDiscovery, String> {
    resolve_source_discovery_excluding(project, inventory, targets, &BTreeSet::new())
}

pub(super) fn resolve_source_discovery_excluding(
    project: &ConfiguredProject,
    inventory: &BTreeMap<String, InventoryEntry>,
    targets: &BTreeMap<String, Vec<u8>>,
    excluded: &BTreeSet<String>,
) -> Result<ResolvedDiscovery, String> {
    let mut children = BTreeMap::<&str, Vec<(&str, InventoryEntry)>>::new();
    let mut bytes = 0;
    if excluded.len() > 200_000 {
        return Err("MetaMake discovery exclusion count exceeds limit".into());
    }
    for path in excluded {
        inventory_relative_path(path)?;
        charge(&mut bytes, path.len(), MAX_RETAINED_BYTES)?;
    }
    if inventory.len() > 200_000 {
        return Err("MetaMake discovery inventory exceeds limit".into());
    }
    for (path, kind) in inventory {
        inventory_relative_path(path)?;
        charge(&mut bytes, path.len(), MAX_RETAINED_BYTES)?;
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        if !parent.is_empty() && inventory.get(parent) != Some(&InventoryEntry::Directory) {
            return Err(format!(
                "MetaMake discovery missing directory provenance: {parent}"
            ));
        }
        if (*kind == InventoryEntry::Link) != targets.contains_key(path) {
            return Err(format!(
                "MetaMake discovery link target inventory differs: {path}"
            ));
        }
        children.entry(parent).or_default().push((name, *kind));
    }
    if targets.len()
        != inventory
            .values()
            .filter(|kind| **kind == InventoryEntry::Link)
            .count()
    {
        return Err("MetaMake discovery has undeclared link target data".into());
    }
    let mut projection = Projection {
        project,
        source_basename: format!("{}.src", project.default_makefile),
        source: inventory,
        targets,
        excluded,
        children,
        result: ResolvedDiscovery {
            inventory: BTreeMap::new(),
            original_inputs: BTreeMap::new(),
        },
        bytes: 0,
        work: 0,
    };
    projection.visit("", "", &mut Vec::new())?;
    Ok(projection.result)
}

struct Projection<'a> {
    project: &'a ConfiguredProject,
    source_basename: String,
    source: &'a BTreeMap<String, InventoryEntry>,
    targets: &'a BTreeMap<String, Vec<u8>>,
    excluded: &'a BTreeSet<String>,
    children: BTreeMap<&'a str, Vec<(&'a str, InventoryEntry)>>,
    result: ResolvedDiscovery,
    bytes: usize,
    work: usize,
}

impl Projection<'_> {
    fn visit(
        &mut self,
        logical: &str,
        physical: &str,
        ancestors: &mut Vec<String>,
    ) -> Result<(), String> {
        // A physical exclusion is not an empty directory. An alias to an
        // ancestor must not turn omitted children into an apparently complete
        // logical inventory, even when it contains no selected Makefile.
        if logical != physical {
            let prefix = format!("{physical}/");
            if self.excluded.contains(physical)
                || self
                    .excluded
                    .range(prefix.clone()..)
                    .next()
                    .is_some_and(|path| path.starts_with(&prefix))
            {
                return Err(format!(
                    "MetaMake directory alias reaches an unmeasured namespace: {logical}"
                ));
            }
        }
        if ancestors.len() >= 256 || ancestors.iter().any(|path| path == physical) {
            return Err(format!(
                "MetaMake discovery directory cycle/depth limit: {logical}"
            ));
        }
        ancestors.push(physical.to_owned());
        // Clone only the immediate-child list; the work budget bounds repeated
        // aliases before expanding them, including trees without Make owners.
        let children = self.children.get(physical).cloned().unwrap_or_default();
        for (name, kind) in children {
            charge(&mut self.work, 1, 200_000)?;
            let selected = name == self.project.default_makefile || name == self.source_basename;
            let alias = join(logical, name);
            if !selected
                && matches!(kind, InventoryEntry::Directory | InventoryEntry::Link)
                && is_ignored_directory_path(&self.project.ignored_directories, &alias, kind)
            {
                continue;
            }
            let original = join(physical, name);
            let resolved = if kind == InventoryEntry::Link {
                resolve(&original, self.source, self.targets, &mut self.work)?
            } else {
                Some((original, kind))
            };
            let Some((original, kind)) = resolved else {
                if selected {
                    return Err(format!(
                        "MetaMake selected input is a dangling link: {alias}"
                    ));
                }
                continue;
            };
            if selected && kind != InventoryEntry::RegularFile {
                return Err(format!(
                    "MetaMake selected input is not a regular file: {alias}"
                ));
            }
            inventory_relative_path(&alias)?;
            charge(&mut self.bytes, alias.len(), MAX_RETAINED_BYTES)?;
            if self.result.inventory.insert(alias.clone(), kind).is_some() {
                return Err("MetaMake discovery duplicate logical coordinate".into());
            }
            match kind {
                InventoryEntry::Directory => self.visit(&alias, &original, ancestors)?,
                InventoryEntry::RegularFile => {
                    if selected {
                        self.result.original_inputs.insert(alias, original);
                    }
                }
                _ => return Err(format!("unsupported MetaMake discovery entry: {alias}")),
            }
        }
        ancestors.pop();
        Ok(())
    }
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.into()
    } else {
        format!("{parent}/{name}")
    }
}

fn resolve(
    path: &str,
    inventory: &BTreeMap<String, InventoryEntry>,
    targets: &BTreeMap<String, Vec<u8>>,
    work: &mut usize,
) -> Result<Option<(String, InventoryEntry)>, String> {
    let mut pending: VecDeque<String> = path.split('/').map(str::to_owned).collect();
    let mut current = Vec::<String>::new();
    let mut hops = 0;
    while let Some(part) = pending.pop_front() {
        charge(work, 1, 200_000)?;
        match part.as_str() {
            "" | "." => continue,
            ".." => {
                if current.pop().is_none() {
                    return Err(format!("MetaMake discovery link escapes source: {path}"));
                }
                continue;
            }
            _ => {}
        }
        let candidate = join(&current.join("/"), &part);
        if candidate.len() > MAX_VALUE_BYTES {
            return Err("MetaMake discovery resolved path exceeds limit".into());
        }
        let Some(kind) = inventory.get(&candidate).copied() else {
            // Missing selected inventory keys are not physical ENOENT proof:
            // a local-source snapshot may exclude its generated namespace.
            return Err(format!(
                "MetaMake discovery link target is unmeasured: {candidate}"
            ));
        };
        match kind {
            InventoryEntry::Link => {
                hops += 1;
                if hops > 40 {
                    return Err(format!("MetaMake discovery link cycle/hop limit: {path}"));
                }
                let bytes = targets
                    .get(&candidate)
                    .ok_or("missing captured link target")?;
                let target =
                    std::str::from_utf8(bytes).map_err(|_| "non-UTF-8 MetaMake link target")?;
                if target.is_empty()
                    || target.len() > MAX_VALUE_BYTES
                    || target.starts_with('/')
                    || target.chars().any(char::is_control)
                {
                    return Err(format!("unsupported MetaMake link target: {candidate}"));
                }
                for component in target.split('/').rev() {
                    pending.push_front(component.to_owned());
                }
            }
            InventoryEntry::Directory => current.push(part),
            InventoryEntry::RegularFile if pending.is_empty() => {
                return Ok(Some((candidate, kind)));
            }
            InventoryEntry::RegularFile => return Ok(None),
            InventoryEntry::Other => {
                return Err(format!("unsupported MetaMake link referent: {candidate}"));
            }
        }
    }
    let physical = current.join("/");
    Ok(Some((physical, InventoryEntry::Directory)))
}

#[cfg(test)]
#[path = "metamake_discovery_links_tests.rs"]
mod tests;
