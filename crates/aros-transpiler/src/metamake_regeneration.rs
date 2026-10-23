//! GenMF output names derived before execution from captured Source inventory.
//!
//! This is not a general recipe-write planner or native execution authority.

use super::{plan_source_inputs, ConfiguredProject, InventoryEntry, OwnerInput};
use aros_common::{
    mesa_source::MesaSourceSnapshot, sha256_bytes, Sha256Digest, TreeContentEntryKind,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Immutable source-owned discovery plan, retaining its original Source owner.
///
/// All generated names come from the existing generic project discovery model,
/// not from a completed build's files or a target-specific directory table.
/// Direct makefiles remain inputs; only `.src` owners are GenMF output names.
/// The configured project bytes are explicit selected data. Their relationship
/// to actual configure execution and generator/runtime identity must still be
/// bound by the supervising producer.
pub struct MetaMakeRegenerationPlan<'a> {
    source: &'a MesaSourceSnapshot,
    configuration_sha256: Sha256Digest,
    project: String,
    owners: Vec<OwnerInput>,
    original_inputs: BTreeMap<String, String>,
    generated_files: Vec<String>,
}

impl<'a> MetaMakeRegenerationPlan<'a> {
    /// Derive complete candidate owner/output coordinates from captured inputs.
    ///
    /// No process is launched or build output/ambient environment consulted.
    /// Inventory and link targets come only from retained snapshot data; this
    /// performs no live Source I/O or verification. It is discovery over the
    /// captured selection, not proof that the live Source still matches. The
    /// caller must revalidate the full Source selection before and after any
    /// execution. The caller supplies the exact configured bytes and
    /// independent expected digest; missing/unsupported configuration or
    /// inventory refuses.
    ///
    /// # Errors
    /// Refuses changed configured bytes, unsupported names/kinds/project
    /// discovery, incomplete directory provenance or resource exhaustion.
    pub fn derive(
        source: &'a MesaSourceSnapshot,
        configured: &[u8],
        expected_configuration_sha256: &Sha256Digest,
        project: &str,
    ) -> Result<Self, String> {
        if sha256_bytes(configured) != *expected_configuration_sha256 {
            return Err("MetaMake regeneration configuration bytes changed".into());
        }
        let text = std::str::from_utf8(configured)
            .map_err(|_| "MetaMake regeneration configuration is not UTF-8")?;
        let selected = ConfiguredProject::parse(text, project)?;
        // A measured input exclusion must also be excluded by the actual
        // selected native scan. It is not proof that the path is absent.
        if (source.excludes_git_metadata() && !selected.ignored_directories.contains(".git"))
            || source
                .excluded_generated_subtree()
                .is_some_and(|subtree| !selected.ignored_directories.contains(subtree))
        {
            return Err(
                "MetaMake native discovery does not exclude the omitted Source namespaces".into(),
            );
        }
        let mut inventory = BTreeMap::new();
        let mut inventory_bytes = 0usize;
        for (name, kind) in source.entry_kinds() {
            inventory_bytes = inventory_bytes
                .checked_add(name.len())
                .filter(|bytes| *bytes <= super::MAX_RETAINED_BYTES)
                .ok_or("MetaMake regeneration source inventory byte limit exceeded")?;
            if inventory.len() >= 200_000 || name.len() > super::MAX_VALUE_BYTES {
                return Err("MetaMake regeneration source inventory exceeds limit".into());
            }
            let name = std::str::from_utf8(name)
                .map_err(|_| "MetaMake regeneration source inventory is not UTF-8")?;
            let kind = match kind {
                TreeContentEntryKind::RegularFile => InventoryEntry::RegularFile,
                TreeContentEntryKind::Directory => InventoryEntry::Directory,
                TreeContentEntryKind::SymbolicLink => InventoryEntry::Link,
            };
            inventory.insert(name.to_owned(), kind);
        }
        let links = source.captured_symbolic_link_targets()?;
        let mut excluded = BTreeSet::new();
        if source.excludes_git_metadata() {
            excluded.insert(".git".to_owned());
        }
        if let Some(path) = source.excluded_generated_subtree() {
            excluded.insert(path.to_owned());
        }
        let resolved = super::discovery_links::resolve_source_discovery_excluding(
            &selected, &inventory, &links, &excluded,
        )?;
        let owners = plan_source_inputs(&selected, &resolved.inventory)?;
        let original_inputs = resolved.original_inputs;
        for owner in &owners {
            let original = original_inputs
                .get(&owner.input)
                .ok_or("MetaMake owner lost original Source input")?;
            if source.regular_input_digest(original).is_none() {
                return Err("MetaMake owner input lacks original regular Source bytes".into());
            }
        }
        for original in original_inputs.values() {
            if source.regular_input_digest(original).is_none() {
                return Err("MetaMake discovered input lacks original regular Source bytes".into());
            }
        }
        let generated_files = owners
            .iter()
            .filter(|owner| owner.generated)
            .map(|owner| owner.owner.clone())
            .collect();
        Ok(Self {
            source,
            configuration_sha256: expected_configuration_sha256.clone(),
            project: project.to_owned(),
            owners,
            original_inputs,
            generated_files,
        })
    }

    /// Selected project name, not an inferred architecture/profile name.
    #[must_use]
    pub fn project(&self) -> &str {
        &self.project
    }

    /// Digest of the exact configured project bytes used for discovery.
    #[must_use]
    pub const fn configuration_sha256(&self) -> &Sha256Digest {
        &self.configuration_sha256
    }

    /// All Source owners, including direct Make inputs and generated outputs.
    #[must_use]
    pub fn owners(&self) -> &[OwnerInput] {
        &self.owners
    }

    /// Original measured regular input for a logical native discovery input.
    /// Alias coordinates are retained in `owners`; their referent coordinates
    /// here bind bytes without pretending the symbolic alias is a regular file.
    #[must_use]
    pub fn original_input(&self, input: &str) -> Option<&str> {
        self.original_inputs.get(input).map(String::as_str)
    }

    /// All discovered logical inputs and original regular Source referents.
    /// Includes a generated sibling superseded by its `.src` input in `owners`.
    #[must_use]
    pub const fn discovered_inputs(&self) -> &BTreeMap<String, String> {
        &self.original_inputs
    }

    /// Exact generated file coordinates relative to the independently selected TOP.
    /// No TOP directory is guessed or joined here.
    #[must_use]
    pub fn generated_files(&self) -> &[String] {
        &self.generated_files
    }

    /// Recheck the same Source owner and configured bytes before/after execution.
    /// This does not authenticate execution or admit arbitrary recipe outputs.
    ///
    /// # Errors
    /// Refuses changed Source identity/bytes or changed configured project bytes.
    pub fn revalidate(&self, source_root: &Path, configured: &[u8]) -> Result<(), String> {
        self.source.verify(source_root)?;
        if sha256_bytes(configured) != self.configuration_sha256 {
            return Err("MetaMake regeneration configuration bytes changed".into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "metamake_regeneration_tests.rs"]
mod tests;
