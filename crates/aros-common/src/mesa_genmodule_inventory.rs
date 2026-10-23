//! Bounded, data-only handoff for a selected Mesa genmodule graph inventory.
//!
//! The reader validates the transported inventory's shape and binds its
//! selection fields to caller-supplied values. It does not authenticate the
//! producer, prove the source graph, or establish that a genmodule command ran.

use crate::Sha256Digest;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// Schema identifier for a selected Mesa genmodule inventory.
pub const MESA_GENMODULE_INVENTORY_SCHEMA: &str = "aros-mesa-genmodule-inventory-v1";
/// Schema identifier for the complete selection document emitted by the transpiler.
pub const MESA_GENMODULE_SELECTION_SCHEMA: &str = "aros-mesa-genmodule-selection-v1";
/// Maximum encoded selection document size.
pub const MESA_GENMODULE_SELECTION_MAX_BYTES: usize = 1024 * 1024;
/// Maximum number of consumer rows in one selected inventory.
pub const MESA_GENMODULE_INVENTORY_MAX_ROWS: usize = 2048;

const COMPONENT_ORDER: [&str; 4] = ["stackstubs", "regcallstubs", "autoinit", "getlibbase"];

/// A deterministic, source-relative inventory projected from a selected graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaGenmoduleInventory {
    pub schema: String,
    pub records: Vec<MesaGenmoduleInventoryRecord>,
}

/// One Mesa runtime module or generated-client marker row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaGenmoduleInventoryRecord {
    /// Graph owner of the runtime declaration or `writefiles` producer.
    pub owner: String,
    pub declaration_directory: String,
    pub module: String,
    pub module_type: String,
    pub base_configuration: String,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub override_configuration: Option<String>,
    /// `runtime/<owner>`, `module-client/<owner>/<normal|rel>`, or
    /// `marker/<client-owner>/<normal|rel>`.
    pub consumer_path: String,
    /// Empty for runtime/module-client records; marker declaration order otherwise.
    pub components: Vec<String>,
}

/// Selects the breadth represented by a Mesa genmodule inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MesaGenmoduleInventoryScope {
    ContractRoots,
    FullTarget,
}

/// Selection metadata binding an inventory to the caller's selected profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaGenmoduleSelectionBinding {
    pub profile: String,
    pub kind: String,
    pub scope: MesaGenmoduleInventoryScope,
    pub contract_sha256: Sha256Digest,
}

/// Complete data-only selection document emitted by the transpiler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaGenmoduleSelectionDocument {
    pub schema: String,
    pub selection: MesaGenmoduleSelectionBinding,
    pub inventory: MesaGenmoduleInventory,
}

/// One genmodule producer deduplicated from its separate consumer rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MesaGenmoduleProducer {
    pub owner: String,
    pub declaration_directory: String,
    pub module: String,
    pub module_type: String,
    pub base_configuration: String,
    #[serde(deserialize_with = "deserialize_present_option")]
    pub override_configuration: Option<String>,
}

impl From<&MesaGenmoduleInventoryRecord> for MesaGenmoduleProducer {
    fn from(record: &MesaGenmoduleInventoryRecord) -> Self {
        Self {
            owner: record.owner.clone(),
            declaration_directory: record.declaration_directory.clone(),
            module: record.module.clone(),
            module_type: record.module_type.clone(),
            base_configuration: record.base_configuration.clone(),
            override_configuration: record.override_configuration.clone(),
        }
    }
}

impl MesaGenmoduleInventory {
    /// Validate the closed inventory schema and all producer/consumer rows.
    ///
    /// # Errors
    /// Rejects an unsupported schema, excessive rows, unsafe identifiers or
    /// paths, malformed consumer roles, duplicate lanes, unknown module types,
    /// invalid components, and conflicting records for one producer owner.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != MESA_GENMODULE_INVENTORY_SCHEMA {
            return Err("unsupported Mesa genmodule inventory schema".into());
        }
        if self.records.len() > MESA_GENMODULE_INVENTORY_MAX_ROWS {
            return Err(format!(
                "Mesa genmodule inventory exceeds the {MESA_GENMODULE_INVENTORY_MAX_ROWS}-row limit"
            ));
        }

        let mut producers = BTreeMap::<String, MesaGenmoduleProducer>::new();
        let mut consumer_paths = std::collections::BTreeSet::<(String, String)>::new();
        for record in &self.records {
            validate_atom(&record.owner, "producer owner")?;
            validate_atom(&record.module, "module name")?;
            validate_module_type(&record.module_type)?;
            validate_source_relative_path(&record.declaration_directory, "declaration directory")?;
            validate_source_relative_path(&record.base_configuration, "base configuration")?;
            if let Some(path) = &record.override_configuration {
                validate_source_relative_path(path, "override configuration")?;
            }
            validate_components(&record.components)?;
            validate_consumer_path(record)?;

            if !consumer_paths.insert((record.owner.clone(), record.consumer_path.clone())) {
                return Err(format!(
                    "{}: duplicate Mesa genmodule consumer path `{}`",
                    record.owner, record.consumer_path
                ));
            }
            let producer = MesaGenmoduleProducer::from(record);
            if let Some(previous) = producers.get(&record.owner) {
                if previous != &producer {
                    return Err(format!(
                        "{}: Mesa genmodule owner has conflicting producer tuples",
                        record.owner
                    ));
                }
            } else {
                producers.insert(record.owner.clone(), producer);
            }
        }
        Ok(())
    }

    /// Deduplicate rows into producers, requiring producer-tuple agreement.
    ///
    /// # Errors
    /// Returns the same validation error as [`Self::validate`].
    pub fn producers(&self) -> Result<Vec<MesaGenmoduleProducer>, String> {
        self.validate()?;
        let mut producers = BTreeMap::<String, MesaGenmoduleProducer>::new();
        for record in &self.records {
            producers
                .entry(record.owner.clone())
                .or_insert_with(|| MesaGenmoduleProducer::from(record));
        }
        Ok(producers.into_values().collect())
    }
}

/// A successfully parsed, caller-bound genmodule selection.
///
/// Fields are private so downstream code can only use this view after the
/// schema, selection binding, and producer/consumer rows have been validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMesaGenmoduleSelection {
    binding: MesaGenmoduleSelectionBinding,
    inventory: MesaGenmoduleInventory,
    producers: Vec<MesaGenmoduleProducer>,
}

impl ValidatedMesaGenmoduleSelection {
    /// Parse a bounded selection document and compare its binding exactly.
    ///
    /// This validates transported data only. It does not authenticate a
    /// producer or establish source/build-system parity.
    ///
    /// # Errors
    /// Rejects oversized or malformed JSON, unknown fields, schema/binding
    /// mismatch, and any invalid inventory row.
    pub fn parse_bound(
        bytes: &[u8],
        expected: &MesaGenmoduleSelectionBinding,
    ) -> Result<Self, String> {
        if bytes.len() > MESA_GENMODULE_SELECTION_MAX_BYTES {
            return Err(format!(
                "Mesa genmodule selection exceeds the {MESA_GENMODULE_SELECTION_MAX_BYTES}-byte limit"
            ));
        }
        validate_binding(expected)?;
        let document: MesaGenmoduleSelectionDocument =
            serde_json::from_slice(bytes).map_err(|error| {
                format!("invalid closed Mesa genmodule selection document: {error}")
            })?;
        if document.schema != MESA_GENMODULE_SELECTION_SCHEMA {
            return Err("unsupported Mesa genmodule selection schema".into());
        }
        validate_binding(&document.selection)?;
        if document.selection != *expected {
            return Err("Mesa genmodule selection does not match the expected binding".into());
        }
        let producers = document.inventory.producers()?;
        Ok(Self {
            binding: document.selection,
            inventory: document.inventory,
            producers,
        })
    }

    /// Return the selection binding checked against the caller's expectation.
    #[must_use]
    pub const fn binding(&self) -> &MesaGenmoduleSelectionBinding {
        &self.binding
    }

    /// Return the validated consumer rows in their transported order.
    #[must_use]
    pub const fn inventory(&self) -> &MesaGenmoduleInventory {
        &self.inventory
    }

    /// Return one producer per owner, sorted by owner.
    #[must_use]
    pub fn producers(&self) -> &[MesaGenmoduleProducer] {
        &self.producers
    }
}

fn validate_binding(binding: &MesaGenmoduleSelectionBinding) -> Result<(), String> {
    validate_atom(&binding.profile, "selection profile")?;
    if !matches!(binding.kind.as_str(), "build" | "consumer" | "sdk-consumer") {
        return Err("Mesa genmodule selection kind is not supported".into());
    }
    Ok(())
}

fn validate_atom(value: &str, what: &str) -> Result<(), String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
    {
        return Err(format!(
            "{what} is not a safe literal identifier: `{value}`"
        ));
    }
    Ok(())
}

fn validate_source_relative_path(value: &str, what: &str) -> Result<(), String> {
    if value.is_empty() || value.starts_with('/') {
        return Err(format!("{what} is not a source-relative path: `{value}`"));
    }
    for segment in value.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
        {
            return Err(format!(
                "{what} is not a normalized source-relative path: `{value}`"
            ));
        }
    }
    Ok(())
}

fn validate_module_type(value: &str) -> Result<(), String> {
    if matches!(
        value,
        "class"
            | "library"
            | "mcc"
            | "mui"
            | "mcp"
            | "device"
            | "resource"
            | "gadget"
            | "image"
            | "datatype"
            | "usbclass"
            | "btclass"
            | "hidd"
            | "handler"
            | "hook"
    ) {
        Ok(())
    } else {
        Err(format!("unknown Mesa genmodule type `{value}`"))
    }
}

fn validate_components(components: &[String]) -> Result<(), String> {
    let mut previous = None;
    for component in components {
        let Some(index) = COMPONENT_ORDER.iter().position(|known| known == component) else {
            return Err(format!("unknown Mesa genmodule component `{component}`"));
        };
        if previous.is_some_and(|last| index <= last) {
            return Err(
                "Mesa genmodule components are duplicated or out of canonical order".into(),
            );
        }
        previous = Some(index);
    }
    Ok(())
}

fn validate_consumer_path(record: &MesaGenmoduleInventoryRecord) -> Result<(), String> {
    let mut parts = record.consumer_path.split('/');
    let role = parts.next();
    let first = parts.next();
    let second = parts.next();
    if parts.next().is_some() {
        return Err(format!(
            "{}: invalid Mesa genmodule consumer role `{}`",
            record.owner, record.consumer_path
        ));
    }
    match (role, first, second) {
        (Some("runtime"), Some(owner), None) if owner == record.owner => {
            if !record.components.is_empty() {
                return Err("runtime Mesa genmodule rows cannot declare components".into());
            }
        }
        (Some("module-client"), Some(owner), Some(lane))
            if owner == record.owner && valid_lane(lane) =>
        {
            if !record.components.is_empty() {
                return Err("module-client Mesa genmodule rows cannot declare components".into());
            }
        }
        (Some("marker"), Some(client_owner), Some(lane)) if valid_lane(lane) => {
            validate_atom(client_owner, "marker client owner")?;
            if record.components.is_empty() {
                return Err("marker Mesa genmodule rows require components".into());
            }
        }
        _ => {
            return Err(format!(
                "{}: invalid Mesa genmodule consumer role `{}`",
                record.owner, record.consumer_path
            ));
        }
    }
    Ok(())
}

fn valid_lane(value: &str) -> bool {
    matches!(value, "normal" | "rel")
}

fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[cfg(test)]
#[path = "mesa_genmodule_inventory_tests.rs"]
mod tests;
