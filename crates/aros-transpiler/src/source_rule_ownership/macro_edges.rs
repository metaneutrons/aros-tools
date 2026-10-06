//! Edge construction for the four hash-verified build macros.

use super::native_macros::{
    effective_macro_arguments, has_inline_genmf_template_reference, native_template_names,
    verified_macro_contracts, MacroContract, MacroForm, VerifiedMacros,
};
use super::{
    add_make_consumer_edges, charge_identity_references, charge_macro_output_bytes, evaluate_one,
    evaluate_words, has_unproven_make_expansion_in_text, join_output, record_identity,
    safe_filename, safe_identity, safe_owner, state_at, unique_outputs, CompileMultiGroup,
    CompileMultiPair, SourceGraph, MAX_IDENTITIES, MAX_MACRO_ARGUMENTS, MAX_MACRO_ARGUMENT_BYTES,
    MAX_MACRO_OUTPUT_BYTES,
};
use crate::dirs::DirVars;
use crate::make_vars::{ConditionalTruth, VarScope};
use crate::parser::macro_invocations;
use std::collections::BTreeMap;
use std::path::Path;

pub(super) type ClosedMacroArguments = BTreeMap<String, String>;

/// Parses the `name=value` token grammar used by `tools/genmf/genmf.py`.
/// Only double quotes group whitespace; a backslash does not escape a quote,
/// single quotes are ordinary value characters, and a balanced Make reference
/// does not extend an unquoted token across whitespace.
pub(super) fn closed_macro_arguments(raw: &str) -> Option<ClosedMacroArguments> {
    if raw.len() > MAX_MACRO_ARGUMENT_BYTES
        || raw.contains(['\n', '\r'])
        || raw
            .chars()
            .any(|character| character.is_whitespace() && !character.is_ascii())
    {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    let mut arguments = BTreeMap::new();
    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }

        let key_start = cursor;
        if !bytes.get(cursor).is_some_and(u8::is_ascii_alphanumeric) {
            return None;
        }
        cursor += 1;
        while bytes
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            cursor += 1;
        }
        if cursor - key_start > 64 || bytes.get(cursor) != Some(&b'=') {
            return None;
        }
        let key = std::str::from_utf8(&bytes[key_start..cursor])
            .ok()?
            .to_owned();
        cursor += 1;
        let (value, end) = genmf_argument_value(raw, cursor)?;
        cursor = end;
        if arguments.len() >= MAX_MACRO_ARGUMENTS || arguments.insert(key, value).is_some() {
            return None;
        }
    }
    Some(arguments)
}

/// Mirrors GenMF's `([!\\s\"]+|\".*?\")?` value capture. An absent value
/// becomes the empty string in `template.write`; quote escapes are not
/// interpreted by GenMF, so the first double quote always closes the value.
pub(super) fn genmf_argument_value(raw: &str, start: usize) -> Option<(String, usize)> {
    let bytes = raw.as_bytes();
    if bytes.get(start) == Some(&b'"') {
        let value_start = start + 1;
        let end = raw.get(value_start..)?.find('"')? + value_start;
        return Some((raw.get(value_start..end)?.to_owned(), end + 1));
    }

    let mut end = start;
    while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b'"' {
        end += 1;
    }
    Some((raw.get(start..end)?.to_owned(), end))
}

pub(super) fn macro_value<'a>(arguments: &'a ClosedMacroArguments, key: &str) -> Option<&'a str> {
    arguments.get(key).map(String::as_str)
}

pub(super) fn add_verified_macro_edges(
    graph: &mut SourceGraph,
    lines: &[&str],
    scope: &VarScope,
    dirs: &DirVars,
    source_dirs: (&Path, &Path),
    states: &[ConditionalTruth],
    macros: VerifiedMacros,
) {
    let (root, rel_dir) = source_dirs;
    let invocations = macro_invocations(&lines.join("\n"));
    let template_names = native_template_names(root);
    for (line_no, raw) in lines.iter().enumerate() {
        if graph.definition_lines.contains(&line_no)
            || state_at(states, line_no) == ConditionalTruth::False
        {
            continue;
        }
        if has_inline_genmf_template_reference(raw, template_names.as_ref()) {
            // GenMF searches anywhere on a noncomment source line, while the
            // ownership collector models only full-line invocations.
            graph.uncertain = true;
        }
    }
    let contracts = verified_macro_contracts(root, macros);
    for invocation in &invocations {
        if graph.definition_lines.contains(&invocation.line) {
            continue;
        }
        match state_at(states, invocation.line) {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                graph.uncertain = true;
                continue;
            }
            ConditionalTruth::True => {}
        }
        graph.uncertain |= has_unproven_make_expansion_in_text(
            &invocation.args,
            scope,
            dirs,
            root,
            rel_dir,
            invocation.line,
        );
    }
    let object_dirs = if macros.allows(MacroForm::BuildProg) {
        build_prog_object_dirs(
            graph,
            &invocations,
            scope,
            dirs,
            source_dirs,
            states,
            &contracts,
        )
    } else {
        BTreeMap::new()
    };
    for invocation in invocations {
        if graph.definition_lines.contains(&invocation.line) {
            continue;
        }
        match state_at(states, invocation.line) {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                graph.uncertain = true;
                continue;
            }
            ConditionalTruth::True => {}
        }
        let form = match invocation.name.as_str() {
            "build_prog" => MacroForm::BuildProg,
            "rule_compile_multi" => MacroForm::CompileMulti,
            "rule_assemble_multi" => MacroForm::AssembleMulti,
            "rule_link_binary" => MacroForm::LinkBinary,
            // A macro outside this hash-verified subset may emit source rules,
            // owners, or hidden Make expansions. Visible arguments alone do
            // not prove the effects of its template body.
            _ => {
                graph.uncertain = true;
                continue;
            }
        };
        if !macros.allows(form) {
            graph.uncertain = true;
            continue;
        }
        let Some(contract) = contracts.get(form as usize).and_then(Option::as_ref) else {
            graph.uncertain = true;
            continue;
        };
        let Some(arguments) = effective_macro_arguments(
            &invocation.args,
            contract,
            scope,
            dirs,
            root,
            rel_dir,
            invocation.line,
        ) else {
            graph.uncertain = true;
            continue;
        };
        if matches!(form, MacroForm::LinkBinary) {
            if link_binary_edges(
                graph,
                &arguments,
                scope,
                dirs,
                source_dirs,
                invocation.line,
                &object_dirs,
            )
            .is_empty()
            {
                graph.uncertain = true;
            }
            continue;
        }
        let mut compile_multi_pairs = None;
        let outputs = match form {
            MacroForm::BuildProg => {
                // The verified template also reads USER_OBJS, namespace
                // overrides, arch/*.o and conditional generated depfiles.
                // Explicit files/objs are not a complete consumer projection.
                // Keep attribution uncertain until those inputs are sealed;
                // this does not disable the native program capability.
                graph.uncertain = true;
                continue;
            }
            MacroForm::CompileMulti | MacroForm::AssembleMulti => {
                // These macros use mmake solely as a variable namespace. Their
                // native defaults (`TMP`) are not MetaMake owners.
                let namespace = macro_value(&arguments, "mmake").unwrap_or("TMP");
                if evaluate_one(namespace, scope, dirs, root, rel_dir, invocation.line)
                    .and_then(|name| safe_owner(&name))
                    .is_none()
                {
                    graph.uncertain = true;
                    continue;
                }
                let outputs = if matches!(form, MacroForm::CompileMulti) {
                    let pairs = multi_compile_pairs(
                        &arguments,
                        scope,
                        dirs,
                        root,
                        rel_dir,
                        invocation.line,
                    );
                    let outputs = compile_multi_output_list(&pairs);
                    compile_multi_pairs = Some(pairs);
                    outputs
                } else {
                    multi_assemble_outputs(&arguments, scope, dirs, root, rel_dir, invocation.line)
                };
                if outputs.is_empty()
                    || !charge_identity_references(graph, outputs.len())
                    || !charge_macro_output_bytes(graph, &outputs)
                {
                    graph.uncertain |= outputs.is_empty();
                    continue;
                }
                let mut all_recorded = true;
                for output in &outputs {
                    if !record_identity(graph, output) {
                        all_recorded = false;
                        break;
                    }
                    if !graph.macro_outputs.insert(output.clone()) {
                        graph.ambiguous_macro_outputs.insert(output.clone());
                    }
                    graph.make_identities.insert(output.clone());
                }
                if !all_recorded {
                    continue;
                }
                if let Some(pairs) = compile_multi_pairs {
                    graph.compile_multi_groups.push(CompileMultiGroup {
                        invocation_line: invocation.line,
                        pairs,
                    });
                }
                outputs
            }
            MacroForm::LinkBinary => unreachable!("link binary is handled above"),
        };
        if outputs.is_empty() {
            graph.uncertain = true;
        }
    }
}

fn build_prog_object_dirs(
    graph: &mut SourceGraph,
    invocations: &[crate::parser::Invocation],
    scope: &VarScope,
    dirs: &DirVars,
    source_dirs: (&Path, &Path),
    states: &[ConditionalTruth],
    contracts: &[Option<MacroContract>; 4],
) -> BTreeMap<String, Option<String>> {
    let (root, rel_dir) = source_dirs;
    let mut object_dirs = BTreeMap::<String, Option<String>>::new();
    for invocation in invocations.iter().filter(|invocation| {
        invocation.name == "build_prog" && !graph.definition_lines.contains(&invocation.line)
    }) {
        if state_at(states, invocation.line) != ConditionalTruth::True {
            continue;
        }
        let Some(contract) = contracts[MacroForm::BuildProg as usize].as_ref() else {
            graph.uncertain = true;
            continue;
        };
        let Some(arguments) = effective_macro_arguments(
            &invocation.args,
            contract,
            scope,
            dirs,
            root,
            rel_dir,
            invocation.line,
        ) else {
            graph.uncertain = true;
            continue;
        };
        let owner = macro_value(&arguments, "mmake")
            .and_then(|raw| evaluate_one(raw, scope, dirs, root, rel_dir, invocation.line))
            .and_then(|owner| safe_owner(&owner));
        let Some(objdir_raw) = macro_value(&arguments, "objdir") else {
            graph.uncertain = true;
            continue;
        };
        let objdir = evaluate_one(objdir_raw, scope, dirs, root, rel_dir, invocation.line)
            .filter(|path| safe_identity(path));
        let (Some(owner), Some(objdir)) = (owner, objdir) else {
            graph.uncertain = true;
            continue;
        };
        object_dirs
            .entry(owner)
            .and_modify(|current| {
                if current.as_deref() != Some(objdir.as_str()) {
                    *current = None;
                }
            })
            .or_insert(Some(objdir));
    }
    object_dirs
}

pub(super) fn multi_compile_pairs(
    args: &ClosedMacroArguments,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Vec<CompileMultiPair> {
    let targetdir_raw = macro_value(args, "targetdir").unwrap_or_default();
    let basenames_raw = macro_value(args, "basenames");
    let Some(basenames_raw) = basenames_raw else {
        return Vec::new();
    };
    let targetdir = if targetdir_raw.trim().is_empty() {
        Some(String::new())
    } else {
        evaluate_one(targetdir_raw, scope, dirs, root, rel_dir, line)
    };
    let Some(targetdir) = targetdir else {
        return Vec::new();
    };
    let usetree = macro_value(args, "usetree").unwrap_or("no");
    if evaluate_one(usetree, scope, dirs, root, rel_dir, line).as_deref() != Some("no") {
        return Vec::new();
    }
    let Some(basenames) = evaluate_words(basenames_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    if basenames.len().saturating_mul(2) > MAX_IDENTITIES {
        return Vec::new();
    }
    let mut pairs = Vec::with_capacity(basenames.len());
    let mut output_bytes = 0usize;
    for basename in basenames {
        let base = if targetdir.is_empty() {
            if !safe_identity(&basename) {
                return Vec::new();
            }
            basename
        } else {
            let leaf = basename.rsplit('/').next().unwrap_or_default();
            if !safe_filename(leaf) {
                return Vec::new();
            }
            join_output(&targetdir, leaf)
        };
        let Some(pair_bytes) = base
            .len()
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(4))
        else {
            return Vec::new();
        };
        let Some(total_bytes) = output_bytes.checked_add(pair_bytes) else {
            return Vec::new();
        };
        if total_bytes > MAX_MACRO_OUTPUT_BYTES {
            return Vec::new();
        }
        output_bytes = total_bytes;
        pairs.push(CompileMultiPair {
            object: format!("{base}.o"),
            depfile: format!("{base}.d"),
        });
    }
    if compile_multi_output_list(&pairs).is_empty() {
        Vec::new()
    } else {
        pairs
    }
}

fn compile_multi_output_list(pairs: &[CompileMultiPair]) -> Vec<String> {
    unique_outputs(
        pairs
            .iter()
            .flat_map(|pair| [pair.object.clone(), pair.depfile.clone()])
            .collect(),
    )
}

fn multi_assemble_outputs(
    args: &ClosedMacroArguments,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Vec<String> {
    let targetdir_raw = macro_value(args, "targetdir").unwrap_or_default();
    let basenames_raw = macro_value(args, "basenames");
    let Some(basenames_raw) = basenames_raw else {
        return Vec::new();
    };
    let targetdir = if targetdir_raw.trim().is_empty() {
        Some(String::new())
    } else {
        evaluate_one(targetdir_raw, scope, dirs, root, rel_dir, line)
    };
    let Some(targetdir) = targetdir else {
        return Vec::new();
    };
    let Some(basenames) = evaluate_words(basenames_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    unique_outputs(
        basenames
            .into_iter()
            .map(|basename| {
                if targetdir.is_empty() {
                    safe_identity(&basename).then(|| format!("{basename}.o"))
                } else {
                    let basename = basename.rsplit('/').next().unwrap_or_default();
                    safe_filename(basename)
                        .then(|| join_output(&targetdir, &format!("{basename}.o")))
                }
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default(),
    )
}

fn link_binary_edges(
    graph: &mut SourceGraph,
    args: &ClosedMacroArguments,
    scope: &VarScope,
    dirs: &DirVars,
    source_dirs: (&Path, &Path),
    line: usize,
    object_dirs: &BTreeMap<String, Option<String>>,
) -> Vec<String> {
    let (root, rel_dir) = source_dirs;
    let Some(binary_raw) = macro_value(args, "file") else {
        return Vec::new();
    };
    let Some(binary_output) = evaluate_one(binary_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    let Some(name_raw) = macro_value(args, "name") else {
        return Vec::new();
    };
    let Some(name) = evaluate_one(name_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    if !safe_filename(&name) {
        return Vec::new();
    }
    let mmake_raw = macro_value(args, "mmake").unwrap_or("BD");
    let Some(mmake) = evaluate_one(mmake_raw, scope, dirs, root, rel_dir, line)
        .and_then(|mmake| safe_owner(&mmake))
    else {
        return Vec::new();
    };
    let objects_raw = macro_value(args, "objs").unwrap_or_default();
    let Some(objects) = evaluate_words(objects_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    let files_raw = macro_value(args, "files").unwrap_or_default();
    let Some(files) = evaluate_words(files_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    let asmfiles_raw = macro_value(args, "asmfiles").unwrap_or_default();
    let Some(asmfiles) = evaluate_words(asmfiles_raw, scope, dirs, root, rel_dir, line) else {
        return Vec::new();
    };
    let stem_count = files.len().saturating_add(asmfiles.len());
    if objects
        .len()
        .saturating_add(stem_count)
        .saturating_add(files.len())
        > MAX_IDENTITIES
    {
        return Vec::new();
    }
    let mut objects = objects;
    if stem_count > 0 {
        let Some(objdir) = object_dirs.get(&mmake).and_then(Option::as_deref) else {
            return Vec::new();
        };
        if objdir.is_empty() || !safe_identity(objdir) {
            return Vec::new();
        }
        for file_stem in files {
            let basename = file_stem.rsplit('/').next().unwrap_or_default();
            if !safe_filename(basename) {
                return Vec::new();
            }
            let object = join_output(objdir, &format!("{basename}.o"));
            objects.push(object.clone());
            objects.push(object.trim_end_matches(".o").to_owned() + ".d");
        }
        for asm_stem in asmfiles {
            let basename = asm_stem.rsplit('/').next().unwrap_or_default();
            if !safe_filename(basename) {
                return Vec::new();
            }
            objects.push(join_output(objdir, &format!("{basename}.o")));
        }
    }
    if objects.len() > MAX_IDENTITIES
        || objects.len() > MAX_IDENTITIES.saturating_sub(graph.edge_count)
        || !charge_identity_references(graph, objects.len().saturating_add(1))
    {
        return Vec::new();
    }
    add_make_consumer_edges(graph, &objects, &binary_output);
    vec![binary_output]
}
