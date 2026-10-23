//! Data-only projection of Mesa genmodule declarations in the full target scope.
//!
//! This uses declarations already present in the graph and Mesa 26's exact
//! compile contracts. It neither invents roots nor reads configuration files.

use super::configured_genmodule::{inventory_for_scope, MesaGenmoduleInventory};
use crate::arch_endpoint_effects::ArchEndpointEffectData;
use crate::ast::{
    GenmoduleLinklibs, InventoryTargetIdentity, ModuleMacroForm, ModuleType, TargetDefinition,
};
use crate::graph::supports_genmodule_client_archives;
use crate::{DependencyGraph, TargetContext};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// Project Mesa genmodule declarations in the unpruned ordinary-target graph.
///
/// Runtime and client lanes are derived from their source declaration forms.
/// Applicable architecture client effects are considered only when the graph
/// carries the exact matching source pair. This does not authenticate graph or
/// producer provenance.
///
/// # Errors
/// Rejects incomplete or unsupported target selectors and invalid, ambiguous,
/// or unsupported Mesa genmodule declarations and markers.
pub fn full_inventory(
    graph: &DependencyGraph,
    context: &TargetContext,
) -> Result<MesaGenmoduleInventory, String> {
    super::configured_genmodule::require_profile(context)?;

    let mut runtime_modules = BTreeSet::new();
    let mut client_archives = BTreeSet::new();
    let mut declarations = HashMap::<&str, FullDeclaration<'_>>::new();
    let concrete_owners: BTreeSet<_> = graph
        .targets
        .values()
        .map(|target| target.mmake_name.as_str())
        .collect();

    for target in graph.targets.values() {
        let declaration = FullDeclaration::from(target);
        add_declaration_scope(
            declaration,
            context,
            &mut runtime_modules,
            &mut client_archives,
        )?;
        if declarations
            .insert(target.mmake_name.as_str(), declaration)
            .is_some()
        {
            return Err(format!(
                "{}: duplicate concrete genmodule declaration",
                target.mmake_name
            ));
        }
    }
    for identity in &graph.inventory_targets {
        if concrete_owners.contains(identity.mmake_name.as_str()) {
            return Err(format!(
                "{}: concrete and pending genmodule declarations overlap",
                identity.mmake_name
            ));
        }
        let declaration = FullDeclaration::from(identity);
        add_declaration_scope(
            declaration,
            context,
            &mut runtime_modules,
            &mut client_archives,
        )?;
        if declarations
            .insert(identity.mmake_name.as_str(), declaration)
            .is_some()
        {
            return Err(format!(
                "{}: duplicate pending genmodule declaration",
                identity.mmake_name
            ));
        }
    }

    // Architecture client endpoints affect only their exact declaration and
    // source-pair lane, under the selected target context. Full scope already
    // includes declared client lanes; this validation prevents an applicable
    // source effect from broadening an unsupported or ambiguous declaration.
    for effect in graph
        .arch_endpoint_effects
        .iter()
        .filter(|effect| effect.applies_to(context))
    {
        match &effect.data {
            ArchEndpointEffectData::ArchModuleObjects { mainmmake, .. } => {
                let Some(declaration) = declarations.get(mainmmake.as_str()) else {
                    continue;
                };
                if super::mesa26::compile_contract(
                    declaration.directory,
                    declaration.mmake_name,
                    Some(context),
                )?
                .is_some()
                    && !runtime_modules.contains(mainmmake)
                {
                    return Err(format!(
                        "{mainmmake}: applicable architecture module-object lane has no supported runtime declaration"
                    ));
                }
            }
            ArchEndpointEffectData::ArchClientLinklibSources {
                mainmmake,
                tag,
                linklib_sources,
                directory,
                compiler,
                ..
            } => {
                if compiler != "target" {
                    continue;
                }
                let Some(declaration) = declarations.get(mainmmake.as_str()).copied() else {
                    continue;
                };
                if super::mesa26::compile_contract(
                    declaration.directory,
                    declaration.mmake_name,
                    Some(context),
                )?
                .is_none()
                {
                    continue;
                }
                let Some(linklibs) = declaration
                    .genmodule_linklibs
                    .filter(|linklibs| linklibs.enabled)
                else {
                    continue;
                };
                if !matches!(
                    declaration.module_macro,
                    Some(ModuleMacroForm::Full | ModuleMacroForm::RuntimeOnly)
                ) || !supports_genmodule_client_archives(declaration.module_type)
                {
                    return Err(format!(
                        "{mainmmake}: applicable architecture client lane has no supported full-module archive declaration"
                    ));
                }
                let source_pair = graph.arch_sources.get(mainmmake).is_some_and(|sources| {
                    sources.iter().any(|source| {
                        &source.tag == tag
                            && &source.dir == directory
                            && &source.linklib_files == linklib_sources
                            && !source.files.is_empty()
                    })
                });
                if !source_pair {
                    continue;
                }
                client_archives.insert(format!("{mainmmake}-linklib"));
                if linklibs.has_relative {
                    client_archives.insert(format!("{mainmmake}-linklib-rel"));
                }
            }
            _ => {}
        }
    }

    inventory_for_scope(graph, context, &runtime_modules, &client_archives)
}

#[derive(Clone, Copy)]
struct FullDeclaration<'a> {
    mmake_name: &'a str,
    module_type: &'a ModuleType,
    module_macro: Option<ModuleMacroForm>,
    directory: &'a Path,
    selection_headers_only: bool,
    genmodule_only: bool,
    linklib_name: Option<&'a str>,
    genmodule_linklibs: Option<&'a GenmoduleLinklibs>,
}

impl<'a> From<&'a TargetDefinition> for FullDeclaration<'a> {
    fn from(target: &'a TargetDefinition) -> Self {
        Self {
            mmake_name: &target.mmake_name,
            module_type: &target.module_type,
            module_macro: target.module_macro,
            directory: &target.dir_path,
            selection_headers_only: target.selection_headers_only,
            genmodule_only: target.genmodule_only,
            linklib_name: target.linklib_name.as_deref(),
            genmodule_linklibs: target.genmodule_linklibs.as_ref(),
        }
    }
}

impl<'a> From<&'a InventoryTargetIdentity> for FullDeclaration<'a> {
    fn from(identity: &'a InventoryTargetIdentity) -> Self {
        Self {
            mmake_name: &identity.mmake_name,
            module_type: &identity.module_type,
            module_macro: identity.module_macro,
            directory: &identity.dir_path,
            selection_headers_only: false,
            genmodule_only: identity.genmodule_only,
            linklib_name: identity.linklib_name.as_deref(),
            genmodule_linklibs: identity.genmodule_linklibs.as_ref(),
        }
    }
}

fn add_declaration_scope(
    declaration: FullDeclaration<'_>,
    context: &TargetContext,
    runtime_modules: &mut BTreeSet<String>,
    client_archives: &mut BTreeSet<String>,
) -> Result<(), String> {
    if super::mesa26::compile_contract(
        declaration.directory,
        declaration.mmake_name,
        Some(context),
    )?
    .is_none()
    {
        return Ok(());
    }

    let runtime_kind = matches!(
        declaration.module_type,
        ModuleType::Library
            | ModuleType::Device
            | ModuleType::Resource
            | ModuleType::Hidd
            | ModuleType::Datatype
            | ModuleType::Gadget
            | ModuleType::Mcc
    );
    if declaration.selection_headers_only || declaration.module_type == &ModuleType::ModuleHeaders {
        return Ok(());
    }
    if runtime_kind
        && !matches!(
            declaration.module_macro,
            Some(ModuleMacroForm::Full | ModuleMacroForm::RuntimeOnly)
        )
    {
        return Err(format!(
            "{}: full-target runtime has no supported module declaration",
            declaration.mmake_name
        ));
    }
    if declaration.module_type == &ModuleType::Abi
        && declaration.module_macro != Some(ModuleMacroForm::AbiOnly)
    {
        return Err(format!(
            "{}: full-target ABI client has no supported module declaration",
            declaration.mmake_name
        ));
    }
    if !runtime_kind
        && !matches!(
            declaration.module_type,
            ModuleType::Abi | ModuleType::LinkLib
        )
    {
        return Err(format!(
            "{}: full-target Mesa declaration has unsupported module type",
            declaration.mmake_name
        ));
    }
    if runtime_kind
        && !declaration.selection_headers_only
        && matches!(
            declaration.module_macro,
            Some(ModuleMacroForm::Full | ModuleMacroForm::RuntimeOnly)
        )
    {
        runtime_modules.insert(declaration.mmake_name.to_owned());
    }

    match (declaration.module_type, declaration.module_macro) {
        (ModuleType::Abi, Some(ModuleMacroForm::AbiOnly)) => {
            // %build_module_abi always creates one static client stub archive,
            // but not a relative archive or a runtime module.
            client_archives.insert(format!("{}-linklib", declaration.mmake_name));
        }
        (module_type, Some(ModuleMacroForm::Full | ModuleMacroForm::RuntimeOnly))
            if supports_genmodule_client_archives(module_type)
                && !declaration.selection_headers_only =>
        {
            let enabled = declaration.genmodule_only
                || declaration.linklib_name.is_some()
                || declaration
                    .genmodule_linklibs
                    .is_some_and(|linklibs| linklibs.enabled);
            if enabled {
                if !declaration.genmodule_only && declaration.genmodule_linklibs.is_none() {
                    return Err(format!(
                        "{}: client archive selection lacks parsed configuration metadata",
                        declaration.mmake_name
                    ));
                }
                client_archives.insert(format!("{}-linklib", declaration.mmake_name));
                if !declaration.genmodule_only
                    && declaration
                        .genmodule_linklibs
                        .is_some_and(|linklibs| linklibs.has_relative)
                {
                    client_archives.insert(format!("{}-linklib-rel", declaration.mmake_name));
                }
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
#[path = "configured_genmodule_full_tests.rs"]
mod tests;
