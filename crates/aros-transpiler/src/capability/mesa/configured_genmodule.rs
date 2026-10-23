//! Data-only projection of selected Mesa genmodule producers and consumers.
//!
//! This inventories the graph's already-parsed metadata. It does not read
//! source files, parse genmodule configuration files, prove that a producer
//! ran, or authenticate the source graph.

use crate::ast::{
    GenmoduleLinklibs, InventoryTargetIdentity, ModuleMacroForm, ModuleType, TargetDefinition,
};
use crate::genmodule_header_rules::GenmoduleType;
use crate::{DependencyGraph, TargetContext};
pub use aros_common::mesa_genmodule_inventory::{
    MesaGenmoduleInventory, MesaGenmoduleInventoryRecord,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const INVENTORY_SCHEMA: &str =
    aros_common::mesa_genmodule_inventory::MESA_GENMODULE_INVENTORY_SCHEMA;
const SOURCE_ROOT: &str = "${AROS_SOURCE_DIR}/";
const MARKER_TAG: &str = "@AROS_GENMODULE";
const COMPONENT_ORDER: [&str; 4] = ["stackstubs", "regcallstubs", "autoinit", "getlibbase"];

/// Project selected Mesa 26 runtime declarations and generated-client markers.
///
/// The result is a candidate inventory only. In particular, it does not attest
/// to the source bytes, a genmodule executable, or successful producer output.
///
/// # Errors
/// Rejects incomplete or unsupported selectors, unsafe declaration paths,
/// ambiguous producers, conflicting owner tuples and malformed client markers.
pub fn inventory(
    graph: &DependencyGraph,
    target: &TargetContext,
) -> Result<MesaGenmoduleInventory, String> {
    require_profile(target)?;
    if graph.native_selected_runtime_modules.is_none()
        || graph.native_selected_client_archives.is_none()
    {
        return Err("Mesa genmodule inventory requires a source-selected native graph".into());
    }

    let runtime_modules = graph
        .native_selected_runtime_modules
        .as_ref()
        .ok_or_else(|| "Mesa genmodule inventory requires selected runtime modules".to_owned())?;
    let client_archives = graph
        .native_selected_client_archives
        .as_ref()
        .ok_or_else(|| "Mesa genmodule inventory requires selected client archives".to_owned())?;

    inventory_for_scope(graph, target, runtime_modules, client_archives)
}

/// Shared projection over caller-selected runtime and client scopes.
///
/// The caller owns scope derivation. This function only projects the exact
/// declarations and marker consumers present in `graph` and never mutates it.
pub(super) fn inventory_for_scope(
    graph: &DependencyGraph,
    target: &TargetContext,
    runtime_modules: &BTreeSet<String>,
    client_archives: &BTreeSet<String>,
) -> Result<MesaGenmoduleInventory, String> {
    require_profile(target)?;

    let mut records = Vec::new();
    let mut producers = BTreeMap::<String, ProducerTuple>::new();
    let mut lanes = BTreeSet::<(String, String)>::new();

    let mut declarations: Vec<_> = graph.targets.values().collect();
    declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
    let concrete_owners: BTreeSet<_> = declarations
        .iter()
        .map(|declaration| declaration.mmake_name.as_str())
        .collect();
    for declaration in declarations {
        let declaration = DeclarationMetadata::from(declaration);
        if super::mesa26::compile_contract(
            declaration.dir_path,
            declaration.mmake_name,
            Some(target),
        )?
        .is_none()
        {
            continue;
        }
        for record in module_records(&declaration, runtime_modules, client_archives)? {
            register_record(&mut records, &mut producers, &mut lanes, record)?;
        }
    }

    // A pending identity can carry the same declaration metadata as a
    // concrete target, but never its source or producer state. Use it only
    // when the selected graph has no concrete declaration for that owner.
    let mut pending_declarations: Vec<_> = graph
        .inventory_targets
        .iter()
        .filter(|identity| !concrete_owners.contains(identity.mmake_name.as_str()))
        .collect();
    pending_declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
    for declaration in pending_declarations {
        let declaration = DeclarationMetadata::from(declaration);
        if super::mesa26::compile_contract(
            declaration.dir_path,
            declaration.mmake_name,
            Some(target),
        )?
        .is_none()
        {
            continue;
        }
        for record in module_records(&declaration, runtime_modules, client_archives)? {
            register_record(&mut records, &mut producers, &mut lanes, record)?;
        }
    }

    // Pending identities have no retained source-list projection. Only
    // concrete LinkLib targets can carry source-owned client markers.
    let mut consumers: Vec<_> = graph.targets.values().collect();
    consumers.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
    for consumer in consumers {
        // Native selection already retained this concrete LinkLib target.
        // native_selected_client_archives instead names synthetic archive
        // endpoints of module builders, not these standalone LinkLib owners.
        let markers = marker_tokens(consumer)?;
        if markers.is_empty() {
            continue;
        }
        if consumer.module_type != ModuleType::LinkLib {
            return Err(format!(
                "{}: genmodule marker is attached to a non-linklib target",
                consumer.mmake_name
            ));
        }
        let consumer_dir = normalized_relative_path(
            consumer.dir_path.to_str().ok_or_else(|| {
                format!(
                    "{}: declaration directory is not UTF-8",
                    consumer.mmake_name
                )
            })?,
            "declaration directory",
        )?;
        for marker in markers {
            let record = marker_record(graph, consumer, &consumer_dir, marker)?;
            register_record(&mut records, &mut producers, &mut lanes, record)?;
        }
    }

    records.sort_by(|left, right| {
        (&left.owner, &left.consumer_path, &left.module).cmp(&(
            &right.owner,
            &right.consumer_path,
            &right.module,
        ))
    });
    let inventory = MesaGenmoduleInventory {
        schema: INVENTORY_SCHEMA.to_owned(),
        records,
    };
    inventory.validate()?;
    Ok(inventory)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProducerTuple {
    declaration_directory: String,
    module: String,
    module_type: String,
    base_configuration: String,
    override_configuration: Option<String>,
}

impl From<&MesaGenmoduleInventoryRecord> for ProducerTuple {
    fn from(record: &MesaGenmoduleInventoryRecord) -> Self {
        Self {
            declaration_directory: record.declaration_directory.clone(),
            module: record.module.clone(),
            module_type: record.module_type.clone(),
            base_configuration: record.base_configuration.clone(),
            override_configuration: record.override_configuration.clone(),
        }
    }
}

pub(super) fn require_profile(target: &TargetContext) -> Result<(), String> {
    for (name, value) in [
        ("cpu", target.cpu.as_ref()),
        ("platform", target.platform.as_ref()),
        ("toolchain", target.toolchain.as_ref()),
        ("cpu32", target.cpu32.as_ref()),
        ("use_mmu", target.use_mmu.as_ref()),
        ("float_abi", target.float_abi.as_ref()),
    ] {
        if value.is_none() {
            return Err(format!(
                "Mesa genmodule inventory requires explicit {name} selector"
            ));
        }
    }
    if target.mesa_version.as_deref() != Some("26.0.0") {
        return Err("Mesa genmodule inventory requires explicit Mesa 26.0.0 selector".into());
    }
    super::mesa26::profile(Some(target))?.ok_or_else(|| {
        "Mesa genmodule inventory requires a supported explicit Mesa 26.0.0 profile".to_owned()
    })?;
    Ok(())
}

#[derive(Clone, Copy)]
struct DeclarationMetadata<'a> {
    mmake_name: &'a str,
    target_name: &'a str,
    module_type: &'a ModuleType,
    module_macro: Option<ModuleMacroForm>,
    declared_mod_type: Option<&'a str>,
    dir_path: &'a Path,
    config_file: Option<&'a str>,
    config_override_file: Option<&'a str>,
    genmodule_linklibs: Option<&'a GenmoduleLinklibs>,
    selection_headers_only: bool,
}

impl<'a> From<&'a TargetDefinition> for DeclarationMetadata<'a> {
    fn from(target: &'a TargetDefinition) -> Self {
        Self {
            mmake_name: &target.mmake_name,
            target_name: &target.target_name,
            module_type: &target.module_type,
            module_macro: target.module_macro,
            declared_mod_type: target.declared_mod_type.as_deref(),
            dir_path: &target.dir_path,
            config_file: target.config_file.as_deref(),
            config_override_file: target.config_override_file.as_deref(),
            genmodule_linklibs: target.genmodule_linklibs.as_ref(),
            selection_headers_only: target.selection_headers_only,
        }
    }
}

impl<'a> From<&'a InventoryTargetIdentity> for DeclarationMetadata<'a> {
    fn from(identity: &'a InventoryTargetIdentity) -> Self {
        Self {
            mmake_name: &identity.mmake_name,
            target_name: &identity.target_name,
            module_type: &identity.module_type,
            module_macro: identity.module_macro,
            declared_mod_type: identity.declared_mod_type.as_deref(),
            dir_path: &identity.dir_path,
            config_file: identity.config_file.as_deref(),
            config_override_file: identity.config_override_file.as_deref(),
            genmodule_linklibs: identity.genmodule_linklibs.as_ref(),
            // Inventory identities are declaration-only and cannot claim a
            // concrete headers-only projection.
            selection_headers_only: false,
        }
    }
}

fn module_records(
    declaration: &DeclarationMetadata<'_>,
    runtime_modules: &BTreeSet<String>,
    client_archives: &BTreeSet<String>,
) -> Result<Vec<MesaGenmoduleInventoryRecord>, String> {
    if declaration.module_type == &ModuleType::LinkLib {
        return Ok(Vec::new());
    }
    let runtime = runtime_modules.contains(declaration.mmake_name);
    let clients: Vec<_> = [("normal", "-linklib"), ("rel", "-linklib-rel")]
        .into_iter()
        .filter_map(|(lane, suffix)| {
            client_archives
                .contains(&format!("{}{suffix}", declaration.mmake_name))
                .then_some(lane)
        })
        .collect();
    if !runtime && clients.is_empty() {
        return Ok(Vec::new());
    }
    if !clients.is_empty()
        && declaration.module_type != &ModuleType::Abi
        && !crate::graph::supports_genmodule_client_archives(declaration.module_type)
    {
        return Err(format!(
            "{}: selected module type has no client archive builder",
            declaration.mmake_name
        ));
    }
    if declaration.module_type == &ModuleType::ModuleHeaders || declaration.selection_headers_only {
        return Err(format!(
            "{}: header-only declaration cannot supply selected compile lanes",
            declaration.mmake_name
        ));
    }
    if (runtime
        && !matches!(
            declaration.module_macro,
            Some(ModuleMacroForm::Full | ModuleMacroForm::RuntimeOnly)
        ))
        || (!clients.is_empty()
            && !matches!(
                declaration.module_macro,
                Some(
                    ModuleMacroForm::Full | ModuleMacroForm::RuntimeOnly | ModuleMacroForm::AbiOnly
                )
            ))
    {
        return Err(format!(
            "{}: selected Mesa compile lane has no supported module declaration",
            declaration.mmake_name
        ));
    }
    let record = runtime_record(declaration)?;
    let mut records = Vec::new();
    if runtime {
        records.push(record.clone());
    }
    for lane in clients {
        if lane == "rel"
            && !declaration
                .genmodule_linklibs
                .is_some_and(|facts| facts.has_relative)
        {
            return Err(format!(
                "{}: selected relative client lane is not declared by the module",
                declaration.mmake_name
            ));
        }
        let mut client = record.clone();
        client.consumer_path = format!("module-client/{}/{lane}", declaration.mmake_name);
        records.push(client);
    }
    Ok(records)
}

fn runtime_record(
    target: &DeclarationMetadata<'_>,
) -> Result<MesaGenmoduleInventoryRecord, String> {
    let owner = safe_atom(target.mmake_name, "runtime owner")?;
    let module = safe_atom(target.target_name, "module name")?;
    let directory = normalized_relative_path(
        target
            .dir_path
            .to_str()
            .ok_or_else(|| format!("{owner}: declaration directory is not UTF-8"))?,
        "declaration directory",
    )?;
    let module_type = module_type_name(target)?;
    let base_configuration = match target.config_file {
        Some(config) => source_relative_path(config, "base configuration")?,
        None => normalized_relative_path(
            &format!("{directory}/{module}.conf"),
            "default base configuration",
        )?,
    };
    let override_configuration = target
        .config_override_file
        .map(|config| source_relative_path(config, "override configuration"))
        .transpose()?;
    Ok(MesaGenmoduleInventoryRecord {
        owner: owner.clone(),
        declaration_directory: directory,
        module,
        module_type,
        base_configuration,
        override_configuration,
        consumer_path: format!("runtime/{owner}"),
        components: Vec::new(),
    })
}

fn module_type_name(target: &DeclarationMetadata<'_>) -> Result<String, String> {
    let type_name = match target.module_type {
        ModuleType::Library => "library",
        ModuleType::Device => "device",
        ModuleType::Resource => "resource",
        ModuleType::Hidd => "hidd",
        ModuleType::Datatype => "datatype",
        ModuleType::Gadget => "gadget",
        ModuleType::Mcc => "mcc",
        ModuleType::Custom | ModuleType::Abi => target
            .declared_mod_type
            .ok_or_else(|| format!("{}: custom genmodule type is missing", target.mmake_name))?,
        _ => {
            return Err(format!(
                "{}: runtime declaration has unsupported module type {:?}",
                target.mmake_name, target.module_type
            ));
        }
    };
    GenmoduleType::parse(type_name)
        .map(|kind| kind.as_str().to_owned())
        .ok_or_else(|| {
            format!(
                "{}: unknown genmodule type `{type_name}`",
                target.mmake_name
            )
        })
}

fn marker_tokens(target: &TargetDefinition) -> Result<Vec<&str>, String> {
    let mut markers = Vec::new();
    for (lane, sources) in [
        ("C++", target.cxx_source_files.as_slice()),
        ("Objective-C", target.objc_source_files.as_slice()),
        ("assembler", target.asm_source_files.as_slice()),
    ] {
        if sources.iter().any(|source| source.contains(MARKER_TAG)) {
            return Err(format!(
                "{}: genmodule marker appears in unsupported {lane} source lane",
                target.mmake_name
            ));
        }
    }
    for source in &target.source_files {
        if source.contains(MARKER_TAG) {
            if !source.starts_with(MARKER_TAG) {
                return Err(format!(
                    "{}: malformed genmodule marker token `{source}`",
                    target.mmake_name
                ));
            }
            markers.push(source.as_str());
        }
    }
    Ok(markers)
}

fn marker_record(
    graph: &DependencyGraph,
    consumer: &TargetDefinition,
    consumer_dir: &str,
    raw: &str,
) -> Result<MesaGenmoduleInventoryRecord, String> {
    let fields: Vec<_> = raw.split('|').collect();
    if fields.len() != 6 || fields[0] != MARKER_TAG {
        return Err(format!(
            "{}: malformed genmodule marker `{raw}`",
            consumer.mmake_name
        ));
    }
    let flavor = match fields[1] {
        "normal" | "rel" => fields[1],
        other => {
            return Err(format!(
                "{}: unknown genmodule marker flavor `{other}`",
                consumer.mmake_name
            ));
        }
    };
    let components = parse_components(fields[2], &consumer.mmake_name)?;
    let module = safe_atom(fields[3], "marker module name")?;
    let module_type = GenmoduleType::parse(fields[4])
        .map(|kind| kind.as_str().to_owned())
        .ok_or_else(|| {
            format!(
                "{}: unknown genmodule marker type `{}`",
                consumer.mmake_name, fields[4]
            )
        })?;
    let base_configuration = marker_configuration(fields[5], consumer_dir)?;

    let mut candidates = Vec::new();
    for rule in &graph.genmodule_writefiles_rules {
        let rule_dir = normalized_relative_path(&rule.declaring_dir, "writefiles directory")?;
        let rule_config = normalized_relative_path(&rule.config, "writefiles configuration")?;
        if rule.module == module
            && rule.modtype.as_str() == module_type
            && rule_dir == consumer_dir
            && rule_config == base_configuration
        {
            candidates.push(rule);
        }
    }
    let [rule] = candidates.as_slice() else {
        return Err(format!(
            "{}: genmodule marker `{raw}` maps to {} writefiles producers",
            consumer.mmake_name,
            candidates.len()
        ));
    };
    let owner = safe_atom(&rule.owner, "writefiles owner")?;
    let declaration_directory =
        normalized_relative_path(&rule.declaring_dir, "writefiles directory")?;
    let consumer_owner = safe_atom(&consumer.mmake_name, "client owner")?;
    Ok(MesaGenmoduleInventoryRecord {
        owner,
        declaration_directory,
        module,
        module_type,
        base_configuration,
        override_configuration: None,
        consumer_path: format!("marker/{consumer_owner}/{flavor}"),
        components,
    })
}

fn parse_components(raw: &str, owner: &str) -> Result<Vec<String>, String> {
    if raw.is_empty() {
        return Err(format!("{owner}: genmodule marker has no components"));
    }
    let components: Vec<_> = raw.split(',').collect();
    let mut previous = None;
    for component in &components {
        let Some(index) = COMPONENT_ORDER.iter().position(|known| known == component) else {
            return Err(format!(
                "{owner}: unknown genmodule component `{component}`"
            ));
        };
        if previous.is_some_and(|last| index <= last) {
            return Err(format!(
                "{owner}: genmodule components are duplicated or out of order"
            ));
        }
        previous = Some(index);
    }
    Ok(components.into_iter().map(str::to_owned).collect())
}

fn marker_configuration(raw: &str, consumer_dir: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("genmodule marker configuration is empty".into());
    }
    if raw.starts_with(SOURCE_ROOT) {
        source_relative_path(raw, "marker configuration")
    } else if raw.contains('/') {
        normalized_relative_path(raw, "marker configuration")
    } else {
        normalized_relative_path(&format!("{consumer_dir}/{raw}"), "marker configuration")
    }
}

fn source_relative_path(raw: &str, what: &str) -> Result<String, String> {
    let relative = raw.strip_prefix(SOURCE_ROOT).unwrap_or(raw);
    normalized_relative_path(relative, what)
}

fn normalized_relative_path(raw: &str, what: &str) -> Result<String, String> {
    if raw.is_empty()
        || raw.starts_with('/')
        || raw.contains(['\\', '$', ';', '|', '*', '?', '"', '\'', '<', '>'])
        || raw
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!(
            "{what} is not a safe source-relative path: `{raw}`"
        ));
    }
    let mut segments = Vec::new();
    for segment in raw.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
        {
            return Err(format!("{what} is not lexically normalized: `{raw}`"));
        }
        segments.push(segment);
    }
    Ok(segments.join("/"))
}

fn safe_atom(raw: &str, what: &str) -> Result<String, String> {
    if raw.is_empty()
        || raw == "."
        || raw == ".."
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
    {
        return Err(format!("{what} is not a safe literal atom: `{raw}`"));
    }
    Ok(raw.to_owned())
}

fn register_record(
    records: &mut Vec<MesaGenmoduleInventoryRecord>,
    producers: &mut BTreeMap<String, ProducerTuple>,
    lanes: &mut BTreeSet<(String, String)>,
    record: MesaGenmoduleInventoryRecord,
) -> Result<(), String> {
    let tuple = ProducerTuple::from(&record);
    if let Some(previous) = producers.get(&record.owner) {
        if previous != &tuple {
            return Err(format!(
                "{}: genmodule owner has conflicting declaration tuples",
                record.owner
            ));
        }
    } else {
        producers.insert(record.owner.clone(), tuple);
    }
    let lane = (record.owner.clone(), record.consumer_path.clone());
    if !lanes.insert(lane) {
        return Err(format!(
            "{}: duplicate genmodule owner lane `{}`",
            record.owner, record.consumer_path
        ));
    }
    records.push(record);
    Ok(())
}

#[cfg(test)]
#[path = "configured_genmodule_tests.rs"]
mod tests;
