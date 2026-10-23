//! Atomic productive graph projection from already-bound Mesa configure data.
//!
//! The owning caller must retain/revalidate its execution evidence. This
//! module derives compilation metadata, never producer authentication.

use aros_common::mesa_configuration::ValidatedMesaConfiguration;
use std::collections::BTreeMap;
use std::path::Path;

use crate::{dirs::DirVars, DependencyGraph, TargetContext};

/// A complete configured selection published into the graph in one operation.
/// This metadata does not authenticate its configure producer.
#[derive(Debug)]
pub struct ConfiguredMesaGraph {
    pub driver: super::configured_driver::MesaConfiguredDriver,
    pub targets: BTreeMap<String, ConfiguredMesaTarget>,
}

/// Options and nominal source context for the same concrete target.
#[derive(Debug)]
pub struct ConfiguredMesaTarget {
    pub options: super::ordered_contract::OrderedMesaCompilerOptions,
    pub source_context: super::configured_source_context::MesaSourceCompileContext,
}

/// Whether this graph contains any reviewed Mesa 26 compile declaration.
///
/// # Errors
/// Rejects unsupported concrete target profiles reported by the capability.
pub fn required(graph: &DependencyGraph, target: &TargetContext) -> Result<bool, String> {
    for declaration in graph.targets.values() {
        if super::mesa26::compile_contract(
            &declaration.dir_path,
            &declaration.mmake_name,
            Some(target),
        )?
        .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Replace the Mesa graph's compile metadata as one complete transaction.
///
/// No partial map is retained when one selected declaration cannot be proved.
/// The old static metadata remains available for recipe/capability validation,
/// but must not be emitted as additional compilation flags by the generator.
///
/// # Errors
/// Rejects reselection, unsupported or unresolved source options, and unsafe
/// build/input locations. The caller must still revalidate its bound inputs
/// immediately before publishing the complete generated graph.
pub fn project(
    graph: &mut DependencyGraph,
    source: &Path,
    dirs: &DirVars,
    target: &TargetContext,
    configuration: &ValidatedMesaConfiguration,
    build: &Path,
    ports: &Path,
) -> Result<Vec<super::configured_inventory::InventoryObservation>, String> {
    if graph.configured_mesa.is_some() {
        return Err("configured Mesa graph must not be reselected".into());
    }
    if !build.is_absolute() || !ports.is_absolute() {
        return Err("configured Mesa requires explicit absolute build and ports roots".into());
    }
    let configured_driver = super::configured_driver::MesaConfiguredDriver::project(configuration)?;
    let coordinates = &configuration.recipe_configuration().make_coordinates;
    let selected_source = source
        .canonicalize()
        .map_err(|_| "cannot canonicalize selected Mesa source")?;
    if Path::new(&coordinates.source_root) != selected_source {
        return Err("measured Mesa Make source root differs from selected source".into());
    }
    let mut targets = BTreeMap::new();
    let mut inventories = Vec::new();
    let mut declarations: Vec<_> = graph.targets.values().collect();
    declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
    let mut selected = Vec::new();
    for declaration in declarations {
        if super::mesa26::compile_contract(
            &declaration.dir_path,
            &declaration.mmake_name,
            Some(target),
        )?
        .is_some()
        {
            selected.push(declaration);
        }
    }
    if !selected.is_empty() {
        super::configured_source_context::verify_templates(source)?;
    }
    for declaration in selected {
        // Ordinary assemble_q uses AFLAGS, not CFLAGS. Never activate the new
        // C/C++/GCA path for a mixed target while its assembler view is still
        // unmeasured. This is an unfinished boundary, not a scope exemption.
        if !declaration.asm_source_files.is_empty() {
            return Err(format!(
                "{}: configured Mesa assembler-option projection is not yet complete",
                declaration.mmake_name
            ));
        }
        if declaration.variant_32bit {
            return Err(format!(
                "{}: configured Mesa companion-compiler option projection is not yet complete",
                declaration.mmake_name
            ));
        }
        if declaration.dir_path == Path::new("workbench/libs/mesa/libmesa") {
            super::configured_generators::require_core_owner(&graph.python_outputs)?;
        }
        let projection = super::configured_scope::project(
            source,
            &declaration.dir_path,
            &declaration.mmake_name,
            dirs,
            target,
            configuration,
            (build, ports),
        )
        .map_err(|message| {
            format!(
                "{} ({}): {message}",
                declaration.mmake_name,
                declaration.dir_path.display()
            )
        })?;
        let source_context = super::configured_source_context::project(
            source,
            &declaration.dir_path,
            &declaration.mmake_name,
            dirs,
            coordinates,
            &projection.context_input,
        )
        .map_err(|message| format!("{}: {message}", declaration.mmake_name))?;
        inventories.extend(projection.inventories);
        let projection = projection.options;
        if projection.c.is_empty() || projection.cxx.is_empty() {
            return Err(format!(
                "{}: configured Mesa language projection is empty",
                declaration.mmake_name
            ));
        }
        targets.insert(
            declaration.mmake_name.clone(),
            ConfiguredMesaTarget {
                options: projection,
                source_context,
            },
        );
    }
    for inventory in &inventories {
        inventory.verify()?;
    }
    graph.configured_mesa = Some(ConfiguredMesaGraph {
        driver: configured_driver,
        targets,
    });
    Ok(inventories)
}
