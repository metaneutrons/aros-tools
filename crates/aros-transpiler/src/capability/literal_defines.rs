//! Literal `-D` fragments one declaration provides for another.
//!
//! One family in the tree writes its HAL configuration as a Make fragment of
//! bare `-D` options and hands it to a second declaration, which compiles a
//! generated header from it. That is a provider/consumer pair expressed in Make
//! text, and it survives transpilation only if both halves are recognised
//! together: the provider's variable manifest, its product closure, and the
//! consumer's marked header.
//!
//! Named for the mechanism rather than for its one current user. The checked
//! variable list is Atheros HAL's, but nothing here is about wireless.
//!
//! This was the last family to move, and the reason is worth keeping: it runs
//! its own pass over a whole mmakefile, so it needed the Make-variable layer,
//! the conditional evaluation and the expression-dependency walk to be modules
//! of their own first. Extracting it earlier would have meant making twenty-two
//! parser internals crate-visible to move one family.

use crate::ast::DefineHeaderDecl;
use crate::local_make_includes::LocalMakeIncludeScan;
use crate::make_deps::{
    make_conditional_dependencies, make_expression_dependencies, make_semantic_lines,
    make_variable_reference_count, references_any_make_variable,
};
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{
    collect_vars_impl, directive_tail, evaluate_conditional, strip_make_comment,
    variable_assignment, ConditionalFrame, ConditionalTruth, VarScope,
};
use crate::parser::{
    is_concrete_build_invocation, join_continuations, macro_arg, sanitize_ident,
    select_target_invocations, TargetContext,
};
use crate::sources::evaluate_macro_sources;
use aros_common::read_source;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

pub(crate) const ATHEROS_HAL_LITERAL_DEFINE_VARIABLES: &[&str] = &[
    "AH_ASSERT",
    "AH_DEBUG",
    "AH_DEBUG_ALQ",
    "AH_DEBUG_COUNTRY",
    "AH_DISABLE_WME",
    "AH_EEPROM_V1",
    "AH_EEPROM_V14",
    "AH_EEPROM_V3",
    "AH_EEPROM_V4K",
    "AH_ENABLE_AP_SUPPORT",
    "AH_ENABLE_FORCEBIAS",
    "AH_NEED_DESC_SWAP",
    "AH_PRIVATE_DIAG",
    "AH_REGOPS_FUNC",
    "AH_SUPPORT_2133",
    "AH_SUPPORT_2316",
    "AH_SUPPORT_2317",
    "AH_SUPPORT_2413",
    "AH_SUPPORT_2417",
    "AH_SUPPORT_2425",
    "AH_SUPPORT_5111",
    "AH_SUPPORT_5112",
    "AH_SUPPORT_5413",
    "AH_SUPPORT_AR5210",
    "AH_SUPPORT_AR5211",
    "AH_SUPPORT_AR5212",
    "AH_SUPPORT_AR5312",
    "AH_SUPPORT_AR5416",
    "AH_WRITE_EEPROM",
    "AH_WRITE_REGDOMAIN",
    "HAL_OBJS",
    "OPT_AH_PATH",
];

/// Grants one reviewed recipe-bearing fragment the exact Make capabilities it
/// needs. A syntactic or prefix-based policy cannot prove that an assignment
/// is declaration-local: names such as AROS_LIB, TARGET_CC, ECHO and MKDEPEND
/// are read implicitly by MetaMake recipes even when the declaring mmakefile
/// never references them. New fragments therefore require a deliberate exact
/// manifest entry instead of widening an ambient-variable denylist.
pub(crate) fn literal_define_fragment_has_capability(
    fragment: &Path,
    provider: &str,
    owner: &str,
    provider_files: &str,
    raw_output: &str,
    owner_prerequisite: &str,
    assigned_variables: &[String],
) -> bool {
    if fragment != Path::new("workbench/devs/networks/atheros5000/hal/Makefile.inc")
        || provider != "workbench-devs-networks-atheros5000-hal"
        || owner != "workbench-devs-networks-atheros5000-hal-opts"
        || !assigned_variables
            .iter()
            .map(String::as_str)
            .eq(ATHEROS_HAL_LITERAL_DEFINE_VARIABLES.iter().copied())
    {
        return false;
    }

    let assigned: HashSet<&str> = assigned_variables.iter().map(String::as_str).collect();
    let source_roots = make_expression_dependencies(provider_files)
        .map(|names| {
            names
                .into_iter()
                .filter(|name| assigned.contains(name.as_str()))
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let output_roots = make_expression_dependencies(raw_output)
        .and_then(|mut names| {
            names.extend(make_expression_dependencies(owner_prerequisite)?);
            Some(
                names
                    .into_iter()
                    .filter(|name| assigned.contains(name.as_str()))
                    .collect::<HashSet<_>>(),
            )
        })
        .unwrap_or_default();
    source_roots == HashSet::from(["HAL_OBJS".to_owned()])
        && output_roots == HashSet::from(["OPT_AH_PATH".to_owned()])
}

/// Computes the complete fragment-local assignment closure which can affect
/// the provider's source list or the generated header.
///
/// Every assignment records both its right-hand-side references and the
/// condition variables which decide whether it runs. Header recipe conditions
/// are roots alongside the source expression, rule target and marked owner
/// prerequisite. Callers require this closure to equal the fragment's full
/// assignment set, so an otherwise harmless-looking unused assignment cannot
/// leak a Make file-scope build property into the declaration.
pub(crate) fn literal_define_fragment_product_closure(
    content: &str,
    provider_files: &str,
    raw_output: &str,
    owner_prerequisite: &str,
    assigned_variables: &[String],
) -> Option<HashSet<String>> {
    let assigned: HashSet<String> = assigned_variables.iter().cloned().collect();
    let mut assignment_dependencies: HashMap<String, HashSet<String>> = assigned
        .iter()
        .map(|name| (name.clone(), HashSet::new()))
        .collect();
    let mut condition_stack: Vec<HashSet<String>> = Vec::new();
    let mut header_condition_roots = HashSet::new();
    let mut rule_seen = false;

    for raw_line in join_continuations(content).lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
            .into_iter()
            .find_map(|word| directive_tail(trimmed, word).map(|args| (word, args)))
        {
            condition_stack.push(make_conditional_dependencies(directive, args)?);
            continue;
        }
        if trimmed == "endif" {
            condition_stack.pop()?;
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            let current = condition_stack.last_mut()?;
            let tail = trimmed.strip_prefix("else").unwrap().trim();
            if !tail.is_empty() {
                let (directive, args) = ["ifeq", "ifneq", "ifdef", "ifndef"]
                    .into_iter()
                    .find_map(|word| directive_tail(tail, word).map(|args| (word, args)))?;
                current.extend(make_conditional_dependencies(directive, args)?);
            }
            continue;
        }
        if raw_line.starts_with('\t') {
            if !rule_seen {
                return None;
            }
            for conditions in &condition_stack {
                header_condition_roots.extend(conditions.iter().cloned());
            }
            continue;
        }
        let semantic = strip_make_comment(trimmed).trim();
        if semantic.is_empty() {
            continue;
        }
        if let Some((name, value, _)) = variable_assignment(semantic) {
            let dependencies = assignment_dependencies.get_mut(name)?;
            dependencies.extend(make_expression_dependencies(value)?);
            for conditions in &condition_stack {
                dependencies.extend(conditions.iter().cloned());
            }
            continue;
        }
        if semantic
            .split_once(':')
            .is_some_and(|(target, prerequisites)| {
                !target.trim().is_empty() && prerequisites.trim().is_empty()
            })
        {
            if rule_seen || !condition_stack.is_empty() {
                return None;
            }
            rule_seen = true;
            continue;
        }
        return None;
    }
    if !condition_stack.is_empty() || !rule_seen {
        return None;
    }

    let mut pending = make_expression_dependencies(provider_files)?;
    pending.extend(make_expression_dependencies(raw_output)?);
    pending.extend(make_expression_dependencies(owner_prerequisite)?);
    pending.extend(header_condition_roots);
    let mut closure = HashSet::new();
    while let Some(variable) = pending.iter().next().cloned() {
        pending.remove(&variable);
        if !assigned.contains(&variable) || !closure.insert(variable.clone()) {
            continue;
        }
        pending.extend(assignment_dependencies.get(&variable)?.iter().cloned());
    }
    Some(closure)
}

pub(crate) fn normalize_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

pub(crate) fn in_tree_c_source_exists(root: &Path, relative_dir: &Path, source: &str) -> bool {
    if source.is_empty() || source.contains('$') || source.contains(';') || source.contains('\\') {
        return false;
    }
    let source_relative = if let Some(relative) = source.strip_prefix("${CMAKE_SOURCE_DIR}/") {
        PathBuf::from(relative)
    } else {
        let path = Path::new(source);
        if path.is_absolute() {
            return false;
        }
        relative_dir.join(path)
    };
    let Some(source_relative) = normalize_relative_path(&source_relative) else {
        return false;
    };
    let Some(owner) = normalize_relative_path(relative_dir) else {
        return false;
    };
    if !source_relative.starts_with(&owner) {
        return false;
    }
    let source_path = root.join(&source_relative);
    source_path.is_file() || PathBuf::from(format!("{}.c", source_path.display())).is_file()
}

pub(crate) fn safe_define_header_output(output: &str) -> bool {
    let Some(relative) = output.strip_prefix("${AROS_BUILD_DIR}/") else {
        return false;
    };
    if relative.is_empty()
        || relative.contains('$')
        || relative.contains(';')
        || relative.contains('\\')
        || Path::new(relative).extension() != Some(std::ffi::OsStr::new("h"))
    {
        return false;
    }
    normalize_relative_path(Path::new(relative)).is_some_and(|normalized| {
        normalized == Path::new(relative)
            && normalized.file_name().is_some_and(|name| !name.is_empty())
    })
}

pub(crate) fn safe_build_tree_output_directory(output: &str) -> bool {
    let Some(relative) = output.strip_prefix("${AROS_BUILD_DIR}/") else {
        return false;
    };
    if relative.is_empty()
        || relative.contains('$')
        || relative.contains(';')
        || relative.contains('\\')
    {
        return false;
    }
    normalize_relative_path(Path::new(relative)).is_some_and(|normalized| {
        normalized == Path::new(relative) && !normalized.as_os_str().is_empty()
    })
}

pub(crate) fn parse_literal_define_recipe_line(line: &str) -> Option<(&str, &str)> {
    let quoted = line.trim().strip_prefix("echo \"")?;
    let close = quoted.rfind('"')?;
    let definition = quoted[..close].strip_prefix("#define ")?.trim();
    let redirect = quoted[close + 1..].trim();
    let destination = redirect
        .strip_prefix(">>")
        .or_else(|| redirect.strip_prefix('>'))?
        .trim();
    (!definition.is_empty() && !destination.is_empty()).then_some((definition, destination))
}

pub(crate) fn collect_active_literal_defines(
    content: &str,
    scope: &VarScope,
    target: &TargetContext,
) -> std::result::Result<(usize, String, String, Vec<String>), String> {
    let mut rule: Option<(usize, String)> = None;
    let mut destination: Option<String> = None;
    let mut definitions = Vec::new();
    let mut definition_names = HashSet::new();
    let mut stack: Vec<ConditionalFrame> = Vec::new();

    for (index, raw_line) in content.lines().enumerate() {
        let trimmed = raw_line.trim();
        if rule.is_none() {
            if raw_line.starts_with('\t') || trimmed.starts_with('#') || trimmed.is_empty() {
                continue;
            }
            let uncommented = strip_make_comment(trimmed).trim();
            if let Some((output, prerequisites)) = uncommented.split_once(':') {
                if prerequisites.trim().is_empty() {
                    rule = Some((index + 1, output.trim().to_owned()));
                }
            }
            continue;
        }

        if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
            .into_iter()
            .find_map(|word| directive_tail(trimmed, word).map(|tail| (word, tail)))
        {
            let parent = stack
                .last()
                .map_or(ConditionalTruth::True, |frame| frame.current);
            let condition = evaluate_conditional(directive, args, scope, target);
            stack.push(ConditionalFrame::new(parent, condition));
            continue;
        }
        if trimmed == "endif" {
            stack.pop();
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            let Some(frame) = stack.last_mut() else {
                return Err(format!("line {} has an unmatched else", index + 1));
            };
            let tail = trimmed.strip_prefix("else").unwrap().trim();
            if tail.is_empty() {
                frame.otherwise();
            } else if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
                .into_iter()
                .find_map(|word| directive_tail(tail, word).map(|args| (word, args)))
            {
                frame.else_if(evaluate_conditional(directive, args, scope, target));
            } else {
                return Err(format!(
                    "line {} has an unsupported else directive",
                    index + 1
                ));
            }
            continue;
        }
        if !raw_line.starts_with('\t') {
            continue;
        }
        let state = stack
            .last()
            .map_or(ConditionalTruth::True, |frame| frame.current);
        if state == ConditionalTruth::Unknown {
            return Err(format!(
                "line {} is guarded by an unresolved Make conditional",
                index + 1
            ));
        }
        let Some((definition, recipe_destination)) = parse_literal_define_recipe_line(trimmed)
        else {
            return Err(format!("line {} is not a literal define recipe", index + 1));
        };
        if let Some(previous) = &destination {
            if previous != recipe_destination {
                return Err(format!(
                    "line {} redirects to a different header",
                    index + 1
                ));
            }
        } else {
            destination = Some(recipe_destination.to_owned());
        }
        if state == ConditionalTruth::True {
            let Some(name) = definition.split_whitespace().next() else {
                return Err(format!("line {} has an empty literal define", index + 1));
            };
            if !definition_names.insert(name.to_owned()) {
                return Err(format!("line {} repeats literal define {name}", index + 1));
            }
            definitions.push(definition.to_owned());
        }
    }

    let Some((line, output)) = rule else {
        return Err("the fragment has no literal header output rule".to_owned());
    };
    let Some(destination) = destination else {
        return Err("the fragment has no literal define recipes".to_owned());
    };
    if definitions.is_empty() {
        return Err("the selected profile produces an empty define header".to_owned());
    }
    Ok((line, output, destination, definitions))
}

pub(crate) fn marked_header_owner(
    content: &str,
    output: &str,
    expression_context: &MakeExprContext<'_>,
) -> Option<(String, String)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut owners = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#MM" {
            continue;
        }
        let Some(rule) = lines[index + 1..]
            .iter()
            .map(|candidate| candidate.trim())
            .find(|candidate| !candidate.is_empty() && !candidate.starts_with('#'))
        else {
            continue;
        };
        let Some((owner, prerequisites)) = rule.split_once(':') else {
            continue;
        };
        let owner = owner.trim();
        let words: Vec<&str> = prerequisites.split_whitespace().collect();
        if owner.is_empty()
            || owner.contains('$')
            || !owner.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
            })
            || words.len() != 1
        {
            continue;
        }
        if evaluate_make_expr(words[0], expression_context)
            .ok()
            .as_deref()
            == Some(output)
        {
            owners.push((owner.to_owned(), words[0].to_owned()));
        }
    }
    owners.sort();
    owners.dedup();
    (owners.len() == 1).then(|| owners.remove(0))
}

/// Adopts a recipe-bearing local fragment only when one concrete declaration
/// proves ownership of both its in-tree sources and its complete literal
/// generated header.
// Each argument is an independent ownership boundary: the two competing
// scans, active target, directory-variable table, source root, declaring file
// and its directory/content. Grouping those paths would make it easier to mix
// a fragment's source-relative identity with the declaring mmakefile identity.
#[allow(clippy::too_many_arguments)]
pub(crate) fn owned_scope(
    plain: &LocalMakeIncludeScan,
    candidate: &LocalMakeIncludeScan,
    target: Option<&TargetContext>,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    relative_path: &Path,
    relative_dir: &Path,
    original_content: &str,
) -> Option<Vec<DefineHeaderDecl>> {
    let target = target?;
    if candidate.expanded == plain.expanded
        || !candidate.issues.is_empty()
        || candidate.fragments.len() != 1
        || !candidate.fragments[0].literal_define_header
    {
        return None;
    }
    let fragment = &candidate.fragments[0];
    if fragment.included_from != relative_path || fragment.assigned_variables.is_empty() {
        return None;
    }

    let plain_joined = join_continuations(&plain.expanded);
    let candidate_joined = join_continuations(&candidate.expanded);
    let (plain_scope, plain_states) = collect_vars_impl(&plain_joined, Some(target));
    let (candidate_scope, candidate_states) = collect_vars_impl(&candidate_joined, Some(target));
    let mut ignored = Vec::new();
    let plain_invocations = select_target_invocations(
        &plain_joined,
        Some(&plain_states),
        relative_dir,
        &mut ignored,
    );
    ignored.clear();
    let candidate_invocations = select_target_invocations(
        &candidate_joined,
        Some(&candidate_states),
        relative_dir,
        &mut ignored,
    );
    if candidate_invocations.len() != plain_invocations.len()
        || candidate_invocations
            .iter()
            .zip(&plain_invocations)
            .any(|(candidate, plain)| candidate.name != plain.name || candidate.args != plain.args)
    {
        return None;
    }

    let mut provider: Option<(String, usize, String)> = None;
    if candidate_invocations
        .iter()
        .filter(|invocation| is_concrete_build_invocation(&invocation.name))
        .count()
        != 1
    {
        // Fragment assignments are file scope in Make. With more than one
        // concrete declaration they could alter a target which does not own
        // either the source inventory or the generated header.
        return None;
    }
    for (candidate_invocation, plain_invocation) in
        candidate_invocations.iter().zip(&plain_invocations)
    {
        if !is_concrete_build_invocation(&candidate_invocation.name) {
            continue;
        }
        let Some(files) = macro_arg(&candidate_invocation.args, "files") else {
            continue;
        };
        if !references_any_make_variable(&files, &fragment.assigned_variables) {
            continue;
        }
        let vars = candidate_scope.snapshot(candidate_invocation.line);
        let context = MakeExprContext::new(
            &candidate_scope,
            dirs,
            candidate_invocation.line,
            root,
            relative_dir,
        );
        let Ok(sources) = evaluate_macro_sources(&candidate_invocation.args, &vars, &context)
        else {
            return None;
        };
        if !sources.declared
            || sources.c.is_empty()
            || !sources.cxx.is_empty()
            || !sources.objc.is_empty()
            || !sources.asm.is_empty()
            || !sources.diagnostics.is_empty()
            || !sources
                .c
                .iter()
                .all(|source| in_tree_c_source_exists(root, relative_dir, source))
        {
            return None;
        }
        let plain_vars = plain_scope.snapshot(plain_invocation.line);
        let plain_context = MakeExprContext::new(
            &plain_scope,
            dirs,
            plain_invocation.line,
            root,
            relative_dir,
        );
        if evaluate_macro_sources(&plain_invocation.args, &plain_vars, &plain_context)
            .is_ok_and(|plain_sources| !plain_sources.is_empty())
        {
            return None;
        }
        let mmake = sanitize_ident(&macro_arg(&candidate_invocation.args, "mmake")?);
        if provider
            .replace((mmake, candidate_invocation.line, files))
            .is_some()
        {
            return None;
        }
    }
    let (provider, declaration_line, provider_files) = provider?;

    let fragment_content = read_source(&root.join(&fragment.path)).ok()?;
    let (rule_line, raw_output, recipe_destination, definitions) =
        collect_active_literal_defines(&fragment_content, &candidate_scope, target).ok()?;
    let context =
        MakeExprContext::new(&candidate_scope, dirs, declaration_line, root, relative_dir);
    let output = evaluate_make_expr(&raw_output, &context).ok()?;
    if !safe_define_header_output(&output)
        || output.rsplit('/').next() != Some(recipe_destination.as_str())
    {
        return None;
    }
    let (owner, owner_prerequisite) = marked_header_owner(original_content, &output, &context)?;

    if !literal_define_fragment_has_capability(
        &fragment.path,
        &provider,
        &owner,
        &provider_files,
        &raw_output,
        &owner_prerequisite,
        &fragment.assigned_variables,
    ) {
        return None;
    }

    // Work backwards from the only two products this mode may introduce: the
    // provider's source inventory and the one literal generated header. Every
    // fragment assignment must be in their transitive data/control closure;
    // merely being unused is not safe in Make because templates also consume
    // ambient file-scope properties without spelling them in this mmakefile.
    let product_closure = literal_define_fragment_product_closure(
        &fragment_content,
        &provider_files,
        &raw_output,
        &owner_prerequisite,
        &fragment.assigned_variables,
    )?;
    if product_closure.len() != fragment.assigned_variables.len()
        || fragment
            .assigned_variables
            .iter()
            .any(|variable| !product_closure.contains(variable))
    {
        return None;
    }

    // A reference elsewhere in the declaring file could alter a macro name,
    // output directory, link input, flags, or another rule. Count every
    // semantic reference and admit only the source expression and proven
    // owner's prerequisite. The exact capability manifest above separately
    // excludes provider-private and ambient template variables, even when one
    // has been made part of the product closure.
    let semantic_original = make_semantic_lines(original_content);
    for variable in &fragment.assigned_variables {
        let total = make_variable_reference_count(&semantic_original, variable);
        let allowed = make_variable_reference_count(&provider_files, variable)
            + make_variable_reference_count(&owner_prerequisite, variable);
        if total != allowed {
            return None;
        }
    }
    let mut dependencies = vec![
        format!("${{CMAKE_SOURCE_DIR}}/{}", fragment.path.display()),
        format!("${{CMAKE_SOURCE_DIR}}/{}", relative_path.display()),
    ];
    dependencies.sort();
    dependencies.dedup();

    Some(vec![DefineHeaderDecl {
        owner,
        file: fragment.path.display().to_string(),
        line: rule_line,
        output,
        definitions,
        dependencies,
        provider,
        consumers: Vec::new(),
    }])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn literal_define_capability_is_an_exact_path_provider_and_variable_manifest() {
        let fragment = Path::new("workbench/devs/networks/atheros5000/hal/Makefile.inc");
        let provider = "workbench-devs-networks-atheros5000-hal";
        let owner = "workbench-devs-networks-atheros5000-hal-opts";
        let provider_files = "$(basename $(HAL_OBJS))";
        let output = "${OPT_AH_PATH}";
        let owner_prerequisite = "$(TOP)/$(CURDIR)/opt_ah.h";
        let variables = ATHEROS_HAL_LITERAL_DEFINE_VARIABLES
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();

        assert!(literal_define_fragment_has_capability(
            fragment,
            provider,
            owner,
            provider_files,
            output,
            owner_prerequisite,
            &variables,
        ));

        for ambient in [
            "AROS_LIB",
            "Q",
            "ECHO",
            "IF",
            "TEST",
            "NOP",
            "MKDIR",
            "MKDEPEND",
            "TARGET_CC",
            "USER_CFLAGS",
        ] {
            let mut broadened = variables.clone();
            broadened.push(ambient.to_owned());
            broadened.sort();
            assert!(
                !literal_define_fragment_has_capability(
                    fragment,
                    provider,
                    owner,
                    provider_files,
                    output,
                    owner_prerequisite,
                    &broadened,
                ),
                "ambient Make capability {ambient} was accepted"
            );
        }
        assert!(!literal_define_fragment_has_capability(
            Path::new("elsewhere/Makefile.inc"),
            provider,
            owner,
            provider_files,
            output,
            owner_prerequisite,
            &variables,
        ));
        assert!(!literal_define_fragment_has_capability(
            fragment,
            "different-provider",
            owner,
            provider_files,
            output,
            owner_prerequisite,
            &variables,
        ));
        assert!(!literal_define_fragment_has_capability(
            fragment,
            provider,
            owner,
            "$(basename $(OPT_AH_PATH))",
            output,
            owner_prerequisite,
            &variables,
        ));
    }
}
