//! Shared exact source snapshot for the Mesa producer and its local consumer.
//!
//! This measures bytes and local Git selection, not authenticated execution.

use crate::local_source::LocalSourceIdentity;
use crate::{measure_tree_content_cas, Sha256Digest, TreeContentCas, TreeContentEntryKind};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

const MAX_SYMBOLIC_LINKS: usize = 200_000;

/// A checked source selection. Local builds omit only their selected generated
/// subtree; complete-tree users retain their historical measurement semantics.
#[derive(Debug, Clone)]
pub struct MesaSourceSnapshot {
    root: PathBuf,
    measurement: Measurement,
}

#[derive(Debug, Clone)]
enum Measurement {
    Complete(TreeContentCas),
    LocalBuild {
        identity: LocalSourceIdentity,
        inputs: TreeContentCas,
    },
}

impl MesaSourceSnapshot {
    /// Capture the actual selected bytes, checking an optional local build
    /// identity instead of accepting arbitrary caller-supplied ignore paths.
    ///
    /// # Errors
    /// Rejects mismatched bytes, malformed local identity, tracked generated
    /// source, unsafe paths or an unmeasurable/changing source selection.
    pub fn capture(
        root: &Path,
        expected: &Sha256Digest,
        local: Option<&LocalSourceIdentity>,
    ) -> Result<Self, String> {
        let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
        let measurement = if let Some(local) = local {
            let preset = selected_preset(local)?;
            let inputs = local.verified_tree(root, preset)?;
            if &local.content_sha256 != expected {
                return Err("local Mesa source digest differs from selected inputs".into());
            }
            Measurement::LocalBuild {
                identity: local.clone(),
                inputs,
            }
        } else {
            let tree = measure_tree_content_cas(root).map_err(|error| error.to_string())?;
            if &tree.payload_digest_excluding(None) != expected {
                return Err("Mesa source bytes differ from selected inputs".into());
            }
            Measurement::Complete(tree)
        };
        if root.canonicalize().map_err(|error| error.to_string())? != canonical_root {
            return Err("Mesa source root changed during capture".into());
        }
        Ok(Self {
            root: canonical_root,
            measurement,
        })
    }

    /// Recheck using the same exact selection as the original capture.
    ///
    /// # Errors
    /// Rejects source/Git drift or an unsafe/unmeasurable tree.
    pub fn verify(&self, root: &Path) -> Result<(), String> {
        if root.canonicalize().map_err(|error| error.to_string())? != self.root {
            return Err("Mesa source root differs from its captured selection".into());
        }
        match &self.measurement {
            Measurement::Complete(tree) => {
                let actual = measure_tree_content_cas(root).map_err(|error| error.to_string())?;
                if &actual != tree {
                    return Err("complete Mesa source changed".into());
                }
                Ok(())
            }
            Measurement::LocalBuild { identity, .. } => {
                identity.verify(root, selected_preset(identity)?)
            }
        }
    }

    /// Verify the original snapshot against the exact caller selection.
    ///
    /// This replaces capturing the same source again before verifying its held
    /// snapshot. It does not adopt current bytes or renew the original owner.
    /// Complete-tree and local-build digest domains cannot be interchanged.
    ///
    /// # Errors
    /// Refuses a different digest, local selection, root or changed source.
    pub fn verify_selection(
        &self,
        root: &Path,
        expected: &Sha256Digest,
        local: Option<&LocalSourceIdentity>,
    ) -> Result<(), String> {
        let matches = match (&self.measurement, local) {
            (Measurement::Complete(tree), None) => tree.payload_digest_excluding(None) == *expected,
            (Measurement::LocalBuild { identity, .. }, Some(selected)) => {
                identity == selected && &identity.content_sha256 == expected
            }
            _ => false,
        };
        if !matches {
            return Err("Mesa source snapshot differs from selected inputs".into());
        }
        self.verify(root)
    }

    /// Whether a canonical generated path can be written without modifying
    /// measured source. External outputs remain allowed; in-source output is
    /// allowed only below the one validated local generated subtree.
    #[must_use]
    pub fn permits_generated_path(&self, root: &Path, path: &Path) -> bool {
        if root != self.root
            || !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
            || root.starts_with(path)
        {
            return false;
        }
        if !path.starts_with(root) {
            return true;
        }
        match &self.measurement {
            Measurement::Complete(_) => false,
            Measurement::LocalBuild { identity, .. } => {
                path.starts_with(root.join(&identity.generated_subtree))
            }
        }
    }

    /// Digest of one exact regular input in the original measured selection.
    ///
    /// No filesystem I/O or path normalization occurs. Local Git metadata and
    /// the selected generated subtree are absent by the measurement's own
    /// exclusion rules, rather than a second caller-maintained ignore list.
    #[must_use]
    pub fn regular_input_digest(&self, relative: &str) -> Option<&Sha256Digest> {
        match &self.measurement {
            Measurement::Complete(tree) | Measurement::LocalBuild { inputs: tree, .. } => {
                tree.regular_file_digest(relative)
            }
        }
    }

    /// Return every name and kind in the original measured source selection.
    ///
    /// Names remain raw bytes and are ordered by their stored byte keys. A
    /// complete snapshot exposes its complete measured tree; a local-build
    /// snapshot exposes exactly its verified input tree, retaining the `.git`
    /// and selected generated-subtree exclusions used during capture. This
    /// reads no filesystem state and applies no second ignore list.
    pub fn entry_kinds(&self) -> impl Iterator<Item = (&[u8], TreeContentEntryKind)> + '_ {
        let tree = match &self.measurement {
            Measurement::Complete(tree) | Measurement::LocalBuild { inputs: tree, .. } => tree,
        };
        tree.entry_kinds()
    }

    /// Whether the selected local input measurement excludes Git metadata.
    /// This is measurement policy, not an instruction to a native scanner.
    #[must_use]
    pub const fn excludes_git_metadata(&self) -> bool {
        matches!(self.measurement, Measurement::LocalBuild { .. })
    }

    /// The exact generated subtree omitted by a local input measurement.
    /// A consumer must not silently treat its absence as physical nonexistence.
    #[must_use]
    pub fn excluded_generated_subtree(&self) -> Option<&str> {
        match &self.measurement {
            Measurement::Complete(_) => None,
            Measurement::LocalBuild { identity, .. } => Some(&identity.generated_subtree),
        }
    }

    /// Return retained symbolic-link targets from the original measured source.
    ///
    /// This reads only the captured tree data: it performs no filesystem I/O,
    /// source verification, link resolution, or target inspection. Only names
    /// recorded as symbolic links in that tree are considered, and each target
    /// must have been retained during measurement. Names must be UTF-8 for the
    /// returned map. This is not evidence that the live source still matches.
    ///
    /// # Errors
    /// Refuses unsupported names, missing retained targets, or target
    /// count/length/aggregate resource excess.
    pub fn captured_symbolic_link_targets(&self) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let tree = match &self.measurement {
            Measurement::Complete(tree) | Measurement::LocalBuild { inputs: tree, .. } => tree,
        };
        let link_count = tree
            .entry_kinds()
            .filter(|(_, kind)| *kind == TreeContentEntryKind::SymbolicLink)
            .count();
        if link_count > MAX_SYMBOLIC_LINKS {
            return Err("Mesa source contains too many symbolic links".to_owned());
        }

        let mut total_target_bytes = 0usize;
        let mut targets = BTreeMap::new();
        for (name, kind) in tree.entry_kinds() {
            if kind != TreeContentEntryKind::SymbolicLink {
                continue;
            }
            let name = std::str::from_utf8(name)
                .map_err(|_| "Mesa symbolic-link name is not UTF-8".to_owned())?;
            let target = tree
                .symbolic_link_target(name)
                .ok_or_else(|| format!("Mesa symbolic link has no captured target: {name}"))?;
            if target.len() > TreeContentCas::MAX_RETAINED_SYMBOLIC_LINK_TARGET_BYTES {
                return Err(format!("Mesa symbolic-link target is too long: {name}"));
            }
            total_target_bytes = total_target_bytes
                .checked_add(target.len())
                .ok_or_else(|| "Mesa symbolic-link target bytes exceed limit".to_owned())?;
            if total_target_bytes > TreeContentCas::MAX_RETAINED_SYMBOLIC_LINK_TARGET_BYTES_TOTAL {
                return Err("Mesa symbolic-link target bytes exceed limit".into());
            }
            targets.insert(name.to_owned(), target.to_vec());
        }
        Ok(targets)
    }

    /// Read the original measured symbolic-link targets after checking that
    /// the live source still matches the captured selection.
    ///
    /// The original tree is verified both before and after extracting the
    /// retained target bytes. Only names recorded as symbolic links in that
    /// tree are considered; extraction itself does not resolve links or inspect
    /// their targets. Names must be UTF-8 for the returned map.
    ///
    /// # Errors
    /// Refuses changed source selection, unsupported names, or target
    /// count/length/aggregate resource excess.
    pub fn symbolic_link_targets(&self) -> Result<BTreeMap<String, Vec<u8>>, String> {
        self.verify(&self.root).map_err(|error| {
            format!("Mesa source changed before symbolic-link target access: {error}")
        })?;
        let result = self.captured_symbolic_link_targets();

        self.verify(&self.root).map_err(|error| {
            format!("Mesa source changed during symbolic-link target access: {error}")
        })?;
        result
    }
}

fn selected_preset(local: &LocalSourceIdentity) -> Result<&str, String> {
    local
        .generated_subtree
        .strip_prefix("build/")
        .ok_or_else(|| "local Mesa source must select build/{preset}".into())
}

#[cfg(test)]
#[path = "mesa_source_tests.rs"]
mod tests;
