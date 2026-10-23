//! Positioned import of already-bound original genmodule Make data.
//!
//! This preserves the original include's place in the GenMF expansion. It
//! neither executes Make nor authenticates a producer. Recipe/GCA evaluation
//! must consume the resulting program at their respective original positions;
//! importing these assignments alone is not complete compiler projection.
//! This is an internal test prototype, not an active production API.

use crate::genmf_projection::{ExpandedLineProvenance, ExpandedText};
use aros_common::mesa_genmodule_collection::{
    mesa_genmodule_output_relative_path, LoadedMesaGenmoduleCollection,
};
use aros_common::mesa_genmodule_inventory::MesaGenmoduleProducer;
use aros_common::mesa_genmodule_make::MesaGenmoduleMakefile;
use std::collections::BTreeMap;
use std::path::{Component, Path};

const MAX_PROGRAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESOLVER_PREFIX_BYTES: usize = 64 * 1024 * 1024;

/// One span's origin, explicitly distinguishing imported data from templates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MakeProgramOrigin {
    /// Unchanged text from the original GenMF expansion.
    Genmf(ExpandedLineProvenance),
    /// Canonical literal data at its original template include location.
    Genmodule {
        owner: String,
        module: String,
        include: ExpandedLineProvenance,
    },
}

/// A half-open byte range in the positioned program, not in the input text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakeProgramSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub origin: MakeProgramOrigin,
}

/// Source-ordered text and complete, nonoverlapping origin coverage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionedMesaMakeProgram {
    pub text: String,
    pub spans: Vec<MakeProgramSpan>,
}

/// Replace every selected module's original generated-Make include in place.
///
/// Selection is by Source declaration owner, never module prefix or listing
/// order. Two owners may legitimately share a module prefix. The raw output
/// is selected internally by owner from the validated collection, not supplied
/// as an independently pairable Makefile reference. This data-only operation
/// additionally checks the original expansion frame and declaration location.
/// `resolve` must evaluate Source expressions at the supplied original include
/// position, independently of the expected producer. It must reject unknown
/// values; substituting values from `selected` would defeat the binding.
///
/// # Errors
/// Rejects incomplete provenance, foreign/missing/duplicate selected owners, a changed
/// include form, mismatched module/type/location and oversized output. Unknown
/// source declarations must not be repaired by appending flags elsewhere.
#[cfg(test)]
fn project(
    source_root: &Path,
    expanded: &ExpandedText,
    collection: &LoadedMesaGenmoduleCollection,
    owners: &[&str],
    resolve: &impl Fn(&str, &ExpandedLineProvenance) -> Result<String, String>,
) -> Result<PositionedMesaMakeProgram, String> {
    project_with_prefix(
        source_root,
        expanded,
        collection,
        owners,
        &|_, _| Ok(Some(true)),
        &|expression, origin, _| resolve(expression, origin),
    )
}

/// Resolve bound module imports against their already-emitted program prefix.
///
/// Earlier imports are present. No future statement
/// or the current import is present in `prefix`. `active` independently evaluates
/// the include's branch from that prefix. False sites remain unchanged; unknown
/// selection refuses before producer lookup. This does not execute include paths.
/// The cumulative prefix bytes supplied to callbacks are bounded independently
/// of output size. Callbacks must reject unaccounted includes/effects; a prefix
/// string alone does not prove that its complete Make scope was interpreted.
///
/// # Errors
/// Returns provenance, owner, size, work and binding failures,
/// and every positional resolver failure.
pub fn project_with_prefix(
    source_root: &Path,
    expanded: &ExpandedText,
    collection: &LoadedMesaGenmoduleCollection,
    owners: &[&str],
    active: &impl Fn(&ExpandedLineProvenance, &str) -> Result<Option<bool>, String>,
    resolve: &impl Fn(&str, &ExpandedLineProvenance, &str) -> Result<String, String>,
) -> Result<PositionedMesaMakeProgram, String> {
    let mut selected = Vec::with_capacity(owners.len());
    let producers = collection.selection().producers();
    for owner in owners {
        let index = producers
            .binary_search_by(|producer| producer.owner.as_str().cmp(owner))
            .map_err(|_| {
                format!("Mesa include owner is absent from validated collection: {owner}")
            })?;
        let makefile = collection
            .outputs()
            .get(*owner)
            .ok_or("Mesa validated collection lacks selected owner output")?;
        selected.push((&producers[index], makefile));
    }
    project_selected_with_prefix(source_root, expanded, &selected, active, resolve)
}

// Private projection core: synthetic unit tests may exercise inconsistent
// tuples, but productive callers cannot supply owner-swapped raw Make data.
#[cfg(test)]
fn project_selected(
    source_root: &Path,
    expanded: &ExpandedText,
    selected: &[(&MesaGenmoduleProducer, &MesaGenmoduleMakefile)],
    resolve: &impl Fn(&str, &ExpandedLineProvenance) -> Result<String, String>,
) -> Result<PositionedMesaMakeProgram, String> {
    project_selected_with_prefix(
        source_root,
        expanded,
        selected,
        &|_, _| Ok(Some(true)),
        &|expression, origin, _| resolve(expression, origin),
    )
}

fn project_selected_with_prefix(
    source_root: &Path,
    expanded: &ExpandedText,
    selected: &[(&MesaGenmoduleProducer, &MesaGenmoduleMakefile)],
    active: &impl Fn(&ExpandedLineProvenance, &str) -> Result<Option<bool>, String>,
    resolve: &impl Fn(&str, &ExpandedLineProvenance, &str) -> Result<String, String>,
) -> Result<PositionedMesaMakeProgram, String> {
    if !source_root.is_absolute() || expanded.text.len() > MAX_PROGRAM_BYTES {
        return Err("invalid Mesa positioned-program root or size".into());
    }
    let mut owners = BTreeMap::new();
    for &(producer, makefile) in selected {
        mesa_genmodule_output_relative_path(&producer.owner)?;
        let directory = Path::new(&producer.declaration_directory);
        if directory.as_os_str().is_empty()
            || directory
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || producer.module != makefile.module()
            || owners
                .insert(producer.owner.as_str(), (producer, makefile, 0usize))
                .is_some()
        {
            return Err("invalid or duplicate selected Mesa genmodule producer".into());
        }
    }
    let mut result = PositionedMesaMakeProgram {
        text: String::new(),
        spans: Vec::new(),
    };
    let mut input_end = 0;
    let prefix_work = std::cell::Cell::new(0usize);
    let charge_prefix = |length: usize| {
        let next = prefix_work
            .get()
            .checked_add(length)
            .filter(|value| *value <= MAX_RESOLVER_PREFIX_BYTES)
            .ok_or("Mesa include resolver exceeds cumulative prefix work limit")?;
        prefix_work.set(next);
        Ok::<(), String>(())
    };
    for origin in &expanded.provenance {
        if origin.output_start_byte != input_end || origin.output_end_byte <= input_end {
            return Err("Mesa GenMF provenance has a gap, overlap or empty span".into());
        }
        let line = expanded
            .text
            .get(origin.output_start_byte..origin.output_end_byte)
            .ok_or("Mesa GenMF provenance exceeds its UTF-8 text")?;
        input_end = origin.output_end_byte;
        let include = module_include(origin, line)?;
        if let Some((owner, frame)) = include {
            charge_prefix(result.text.len())?;
            match active(origin, &result.text)? {
                Some(false) => {
                    append(&mut result, line, MakeProgramOrigin::Genmf(origin.clone()))?;
                    continue;
                }
                None => {
                    return Err(
                        "Mesa generated-Make include has unresolved branch selection".into(),
                    )
                }
                Some(true) => {}
            }
            let (producer, makefile, seen) = owners.get_mut(owner).ok_or_else(|| {
                format!("original Mesa include has no selected producer: {owner}")
            })?;
            let declaration = source_root
                .join(&producer.declaration_directory)
                .join("mmakefile.src");
            if origin
                .top_level_source_invocation
                .as_ref()
                .map(|site| &site.path)
                != Some(&declaration)
                || frame.resolved_arguments.get("modtype") != Some(&producer.module_type)
            {
                return Err(
                    "Mesa original include differs from its selected Source declaration".into(),
                );
            }
            let source_frame = origin
                .macro_stack
                .first()
                .ok_or("Mesa include has no original Source macro frame")?;
            let at_include = |expression: &str, site: &ExpandedLineProvenance| {
                charge_prefix(result.text.len())?;
                resolve(expression, site, &result.text)
            };
            verify_source_identity(source_root, producer, source_frame, origin, &at_include)?;
            verify_source_identity(source_root, producer, frame, origin, &at_include)?;
            *seen += 1;
            // Preserve repeated source include sites and their surrounding
            // conditionals. Deduplicating them would change Make's += semantics.
            append(
                &mut result,
                &makefile.canonical_make_fragment(),
                MakeProgramOrigin::Genmodule {
                    owner: producer.owner.clone(),
                    module: producer.module.clone(),
                    include: origin.clone(),
                },
            )?;
        } else {
            append(&mut result, line, MakeProgramOrigin::Genmf(origin.clone()))?;
        }
    }
    if input_end != expanded.text.len() || owners.values().any(|(_, _, count)| *count == 0) {
        return Err(
            "Mesa positioned program does not cover every selected producer and input byte".into(),
        );
    }
    Ok(result)
}

fn module_include<'a>(
    origin: &'a ExpandedLineProvenance,
    line: &str,
) -> Result<Option<(&'a str, &'a crate::genmf_projection::MacroExpansionFrame)>, String> {
    let mut result = None;
    for frame in &origin.macro_stack {
        if frame.name != "build_module_core" {
            continue;
        }
        let argument = |name: &str| {
            frame
                .resolved_arguments
                .get(name)
                .map(String::as_str)
                .ok_or_else(|| format!("original Mesa module frame lacks {name}"))
        };
        let owner = argument("mmake")?;
        let flavour = argument("flavour")?;
        let expected = format!("-include $({owner}{flavour}-makefile)");
        if !line.trim_start().starts_with("-include ") {
            continue;
        }
        // Dependency-file includes are not generated module-Make includes.
        if !line.contains("-makefile)") {
            continue;
        }
        if line.trim() != expected || result.is_some() {
            return Err("original Mesa generated-Make include form is ambiguous or changed".into());
        }
        result = Some((owner, frame));
    }
    Ok(result)
}

fn verify_source_identity(
    source_root: &Path,
    producer: &MesaGenmoduleProducer,
    frame: &crate::genmf_projection::MacroExpansionFrame,
    origin: &ExpandedLineProvenance,
    resolve: &impl Fn(&str, &ExpandedLineProvenance) -> Result<String, String>,
) -> Result<(), String> {
    let argument = |name: &str| {
        frame
            .resolved_arguments
            .get(name)
            .ok_or_else(|| format!("original Mesa module frame lacks {name}"))
            .and_then(|expression| resolve(expression, origin))
    };
    if argument("mmake")? != producer.owner || argument("modtype")? != producer.module_type {
        return Err(
            "Mesa Source macro owner or type differs from selected genmodule producer".into(),
        );
    }
    let module = argument("modname")?;
    mesa_genmodule_output_relative_path(&module)?;
    if module != producer.module {
        return Err("Mesa Source module name differs from selected genmodule producer".into());
    }
    let base = argument("conffile")?;
    let base = if base.is_empty() {
        format!("{module}.conf")
    } else {
        base
    };
    let base = source_configuration_path(source_root, producer, &base)?;
    let override_config = argument("confoverride")?;
    let explicit_override = origin
        .macro_stack
        .first()
        .and_then(|frame| frame.arguments.get("confoverride"))
        .is_some_and(Option::is_some);
    if explicit_override && override_config.is_empty() {
        return Err("Mesa Source explicitly supplied override resolves empty".into());
    }
    let override_config = if override_config.is_empty() {
        None
    } else {
        Some(source_configuration_path(
            source_root,
            producer,
            &override_config,
        )?)
    };
    if base != producer.base_configuration || override_config != producer.override_configuration {
        return Err("Mesa Source configuration differs from selected genmodule producer".into());
    }
    Ok(())
}

fn source_configuration_path(
    root: &Path,
    producer: &MesaGenmoduleProducer,
    value: &str,
) -> Result<String, String> {
    if value
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "$;|*?\"'<>\\".contains(c))
    {
        return Err("Mesa Source configuration is not a resolved inert path".into());
    }
    let path = Path::new(value);
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .map_err(|_| "Mesa Source configuration leaves selected source root")?
            .to_path_buf()
    } else {
        Path::new(&producer.declaration_directory).join(path)
    };
    // As in the existing declaration parser, a relative config is relative
    // to its mmakefile. Do not normalize away parent traversal or aliasing.
    let mut parts = Vec::new();
    for part in relative.components() {
        match part {
            Component::Normal(part) => parts.push(part.to_str().ok_or("non-UTF8 Mesa config")?),
            Component::CurDir => {}
            _ => return Err("Mesa Source configuration is not source-relative".into()),
        }
    }
    if parts.is_empty() {
        return Err("Mesa Source configuration has no file name".into());
    }
    Ok(parts.join("/"))
}

fn append(
    result: &mut PositionedMesaMakeProgram,
    text: &str,
    origin: MakeProgramOrigin,
) -> Result<(), String> {
    let end = result
        .text
        .len()
        .checked_add(text.len())
        .filter(|end| *end <= MAX_PROGRAM_BYTES)
        .ok_or("Mesa positioned program exceeds its byte limit")?;
    result.spans.push(MakeProgramSpan {
        start_byte: result.text.len(),
        end_byte: end,
        origin,
    });
    result.text.push_str(text);
    Ok(())
}

#[cfg(test)]
#[path = "configured_make_program_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "configured_make_program_actual_tests.rs"]
mod actual_tests;
