//! Closed source projection for ordinary Make archives.
//!
//! This collector records an ordinary Make rule with one bounded `%mklib_q`
//! recipe and one direct ordinary `#MM` owner. It models source-declared
//! object lists as ordered identities and generated object wildcards as
//! deferred producer patterns. It does not create native build targets or
//! select an archiver; the archive command's `AR` and `RANLIB` roles remain
//! unresolved for a later, typed binding pass.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};
use crate::make_vars::{
    collect_vars_impl, strip_make_comment, variable_assignment, AssignmentKind, ConditionalTruth,
    VarScope,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

const MAX_SOURCE_BYTES: usize = 1_048_576;
const MAX_SOURCE_LINES: usize = 20_000;
const MAX_ARCHIVES: usize = 512;
const MAX_EXPLICIT_FROM_BYTES: usize = 4_096;
const MAX_EXPLICIT_FROM_VARIABLES: usize = 128;
const MAX_EXPLICIT_FROM_DEPTH: usize = 16;
const MAX_ARCHIVE_PROOF_BYTES: usize = 16 * 1_024 * 1_024;
const BUILD_ROOT: &str = "${AROS_BUILD_DIR}";

/// The source-evaluated inputs to one archive command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveMembers {
    /// Finite object paths in the order GNU Make's `$^` supplies them.
    Exact(Vec<String>),
    /// An unevaluated generated object inventory. The pattern remains data;
    /// it is never expanded against the configure-time filesystem.
    ProducerGlob { root: String, pattern: String },
}

/// One source-owned ordinary Make archive rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceArchiveDecl {
    /// Literal ordinary `#MM` owner that directly lists the archive output.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of the Make rule that produces the archive.
    pub line: usize,
    /// One-based line of the ordinary `#MM` owner rule.
    pub owner_line: usize,
    /// Full configured build-tree output path.
    pub output: String,
    /// Source-derived archive members, without filesystem glob expansion.
    pub members: ArchiveMembers,
}

/// A relevant source archive declaration that could not be represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceArchiveRejection {
    /// Best available ordinary `#MM` owner, or `<unknown>`.
    pub(crate) owner: String,
    /// Source-relative mmakefile path.
    pub(crate) file: String,
    /// One-based line of the closest archive or owner rule.
    pub(crate) line: usize,
    /// Why this source declaration was refused.
    pub(crate) reason: String,
}

#[derive(Debug, Clone)]
struct Recipe {
    text: String,
    state: ConditionalTruth,
    line: usize,
}

#[derive(Debug, Clone)]
struct MakeRule {
    target: String,
    prerequisites: String,
    line: usize,
    state: ConditionalTruth,
    recipes: Vec<Recipe>,
}

#[derive(Debug, Clone)]
struct OwnerRule {
    owner: String,
    prerequisites: String,
    state: ConditionalTruth,
    rule_index: usize,
}

#[derive(Debug, Clone)]
struct Candidate {
    rule_index: usize,
    owner_rule_index: usize,
    owner: String,
    output: String,
    member_shape: Result<ArchiveMembers, String>,
    failure: Option<String>,
}

#[derive(Clone, Copy)]
struct ArchiveRecipeContext<'a> {
    scope: &'a VarScope,
    dirs: &'a DirVars,
    source_root: &'a Path,
    relative_dir: &'a Path,
    configured_gen: &'a str,
    lines: &'a [&'a str],
    rules: &'a [MakeRule],
    states: &'a [ConditionalTruth],
    hidden: &'a [bool],
    source_bytes: usize,
}

/// Collects source-owned ordinary archive rules from a continuation-joined
/// Make snapshot. `scope` and `line_states` must describe this same snapshot.
/// Missing or out-of-range conditional state is treated as unknown.
///
/// A declaration proves only source ownership, output identity, and archive
/// members. The caller must separately bind the archive command to its chosen
/// generic archiver and ranlib roles before any build target can be qualified.
#[must_use]
pub(crate) fn collect_from_snapshot(
    content: &str,
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SourceArchiveDecl>, Vec<SourceArchiveRejection>) {
    let file = match source_file(source_root, relative_dir) {
        Ok(file) => file,
        Err(reason) => {
            return if looks_archive_relevant(content) {
                (
                    Vec::new(),
                    vec![rejection("<unknown>", relative_dir, 1, reason)],
                )
            } else {
                (Vec::new(), Vec::new())
            };
        }
    };
    if content.len() > MAX_SOURCE_BYTES || content.lines().count() > MAX_SOURCE_LINES {
        return if looks_archive_relevant(content) {
            (
                Vec::new(),
                vec![SourceArchiveRejection {
                    owner: "<unknown>".into(),
                    file,
                    line: 1,
                    reason: "source archive snapshot exceeds bounded parser limits".into(),
                }],
            )
        } else {
            (Vec::new(), Vec::new())
        };
    }
    let lines: Vec<_> = content.lines().collect();
    let fallback_states;
    let states = if let Some(states) = line_states {
        states
    } else {
        fallback_states = collect_vars_impl(content, None).1;
        &fallback_states
    };
    let hidden = hidden_make_lines(&lines);
    let rules = parse_make_rules(&lines, states, &hidden);
    let owners = collect_owners(&lines, states, &hidden, &rules);
    let Some(configured_lib) = dirs.expand("$(AROS_LIB)") else {
        return unresolved_candidates(
            &file,
            &rules,
            &owners,
            "configured `AROS_LIB` is unresolved",
        );
    };
    let Some(configured_gen) = dirs.expand("$(GENDIR)") else {
        return unresolved_candidates(&file, &rules, &owners, "configured `GENDIR` is unresolved");
    };
    if !safe_build_directory(&configured_lib) || !safe_build_directory(&configured_gen) {
        return unresolved_candidates(
            &file,
            &rules,
            &owners,
            "configured `AROS_LIB` or `GENDIR` is outside the supported build-tree roots",
        );
    }

    let controls = source_control_error(&lines, states, &hidden);
    let overrides = source_role_overrides(&lines, states, &hidden, dirs);
    let candidate_rule_indices = rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            (rule.state != ConditionalTruth::False
                && is_archive_candidate(
                    rule,
                    scope,
                    dirs,
                    source_root,
                    relative_dir,
                    &configured_lib,
                ))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    if candidate_rule_indices.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if candidate_rule_indices.len() > MAX_ARCHIVES {
        return (
            Vec::new(),
            vec![SourceArchiveRejection {
                owner: "<unknown>".into(),
                file,
                line: 1,
                reason: "source contains too many candidate archive rules".into(),
            }],
        );
    }

    let mut candidates = Vec::new();
    let mut rejections = Vec::new();
    let recipe_context = ArchiveRecipeContext {
        scope,
        dirs,
        source_root,
        relative_dir,
        configured_gen: &configured_gen,
        lines: &lines,
        rules: &rules,
        states,
        hidden: &hidden,
        source_bytes: content.len(),
    };
    let mut archive_proof_bytes = 0usize;
    let mut expression_contexts = Vec::with_capacity(rules.len());
    for rule in &rules {
        expression_contexts.push(MakeExprContext::new(
            scope,
            dirs,
            rule.line,
            source_root,
            relative_dir,
        ));
    }

    for rule_index in candidate_rule_indices {
        let rule = &rules[rule_index];
        let expr_context = &expression_contexts[rule_index];
        let raw_owner_path = evaluate_make_expr(&rule.target, expr_context);
        let output = raw_owner_path
            .as_ref()
            .ok()
            .and_then(|path| normalize_archive_output(path, &configured_lib));
        let owner_indices = output.as_ref().map_or_else(Vec::new, |output| {
            owners
                .iter()
                .enumerate()
                .filter_map(|(owner_index, owner)| {
                    let owner_context = &expression_contexts[owner.rule_index];
                    sole_word(&owner.prerequisites)
                        .and_then(|word| evaluate_make_expr(word, owner_context).ok())
                        .filter(|path| path == output)
                        .map(|_| owner_index)
                })
                .collect()
        });

        let mut owner = owner_indices
            .first()
            .and_then(|index| owners.get(*index))
            .map_or_else(|| "<unknown>".to_owned(), |item| item.owner.clone());
        let mut failure = (rule.state != ConditionalTruth::True)
            .then_some("archive rule is conditional or has unknown state".to_owned());
        if rule.recipes.len() != 1 || rule.recipes[0].state != ConditionalTruth::True {
            failure.get_or_insert_with(|| {
                "archive rule must have exactly one unconditional bounded `%mklib_q` recipe".into()
            });
        }
        if let Some(reason) = &controls {
            failure.get_or_insert_with(|| reason.clone());
        }
        if let Some(reason) = &overrides {
            failure.get_or_insert_with(|| reason.clone());
        }
        if raw_owner_path.is_err() || output.is_none() {
            failure.get_or_insert_with(|| {
                format!(
                    "archive target `{}` does not resolve to one direct `lib<name>.a` below configured `AROS_LIB`",
                    rule.target
                )
            });
        }
        let additional_target_error = output.as_deref().and_then(|output| {
            reject_additional_archive_targets(
                output,
                rule.line,
                &recipe_context,
                &mut archive_proof_bytes,
            )
            .err()
        });
        if let Some(reason) = additional_target_error {
            failure.get_or_insert(reason);
        }
        if owner_indices.len() != 1 {
            failure.get_or_insert_with(|| {
                if owner_indices.is_empty() {
                    "archive output has no unique direct ordinary `#MM` owner".into()
                } else {
                    "archive output has multiple ordinary `#MM` owners".into()
                }
            });
        }
        if let Some(owner_index) = owner_indices.first() {
            let owner_rule = &owners[*owner_index];
            owner.clone_from(&owner_rule.owner);
            let owner_make_rule = &rules[owner_rule.rule_index];
            if owner_rule.state != ConditionalTruth::True
                || owner_make_rule.state != ConditionalTruth::True
            {
                failure.get_or_insert_with(|| {
                    "ordinary `#MM` archive owner is conditional or has unknown state".into()
                });
            }
            if !valid_owner_rule(owner_make_rule) {
                failure.get_or_insert_with(|| {
                    "ordinary `#MM` owner must have no recipe or one exact `@$(NOP)` recipe".into()
                });
            }
            if effective_rule_count(
                &rules,
                owner_make_rule,
                scope,
                dirs,
                source_root,
                relative_dir,
            ) != 1
            {
                failure.get_or_insert_with(|| {
                    "ordinary `#MM` owner has multiple active Make rules".into()
                });
            }
        }

        let member_shape = archive_recipe_members(
            rule.recipes
                .first()
                .map_or("", |recipe| recipe.text.as_str()),
            &rule.prerequisites,
            rule.line,
            &recipe_context,
            &mut archive_proof_bytes,
        );
        if let Err(reason) = &member_shape {
            failure.get_or_insert_with(|| reason.clone());
        }
        if let (Some(output), Some(owner_index)) = (output, owner_indices.first()) {
            candidates.push(Candidate {
                rule_index,
                owner_rule_index: owners[*owner_index].rule_index,
                owner: owner.clone(),
                output,
                member_shape,
                failure,
            });
        } else {
            rejections.push(rejection(
                &owner,
                Path::new(&file),
                rule.line + 1,
                failure.unwrap_or_else(|| "archive ownership is unresolved".to_owned()),
            ));
        }
    }

    reject_case_collisions(&mut candidates, &owners);
    let mut declarations = Vec::new();
    for candidate in candidates {
        if let Some(reason) = candidate.failure {
            rejections.push(rejection(
                &candidate.owner,
                Path::new(&file),
                rules[candidate.rule_index].line + 1,
                reason,
            ));
            continue;
        }
        let Some(owner_rule) = owners
            .iter()
            .find(|owner| owner.rule_index == candidate.owner_rule_index)
        else {
            rejections.push(rejection(
                &candidate.owner,
                Path::new(&file),
                rules[candidate.rule_index].line + 1,
                "ordinary `#MM` owner disappeared during validation".into(),
            ));
            continue;
        };
        let Ok(members) = candidate.member_shape else {
            rejections.push(rejection(
                &candidate.owner,
                Path::new(&file),
                rules[candidate.rule_index].line + 1,
                "archive members failed source validation".into(),
            ));
            continue;
        };
        declarations.push(SourceArchiveDecl {
            owner: candidate.owner,
            file: file.clone(),
            line: rules[candidate.rule_index].line + 1,
            owner_line: rules[owner_rule.rule_index].line + 1,
            output: candidate.output,
            members,
        });
    }
    (declarations, rejections)
}

fn source_file(source_root: &Path, relative_dir: &Path) -> Result<String, String> {
    if relative_dir.is_absolute()
        || relative_dir.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err("archive mmakefile directory is not source-relative".into());
    }
    let root = source_root
        .canonicalize()
        .map_err(|_| "archive source root cannot be canonicalized".to_owned())?;
    if !root.is_dir() {
        return Err("archive source root is not a directory".into());
    }
    let path = root.join(relative_dir).join("mmakefile.src");
    let mut component_path = root.clone();
    for component in relative_dir.components() {
        let Component::Normal(part) = component else {
            return Err("archive mmakefile has an unsafe source-relative path".into());
        };
        component_path.push(part);
        if fs::symlink_metadata(&component_path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err("archive mmakefile source path contains a symlink".into());
        }
    }
    component_path.push("mmakefile.src");
    let metadata = fs::symlink_metadata(&component_path)
        .map_err(|_| "archive mmakefile does not exist".to_owned())?;
    if metadata.file_type().is_symlink() {
        return Err("archive mmakefile source path contains a symlink".into());
    }
    let resolved = path
        .canonicalize()
        .map_err(|_| "archive mmakefile cannot be canonicalized".to_owned())?;
    if !resolved.starts_with(&root) {
        return Err("archive mmakefile resolves outside the source root".into());
    }
    if !resolved.is_file() {
        return Err("archive mmakefile is not a regular file".into());
    }
    let relative = resolved
        .strip_prefix(&root)
        .map_err(|_| "archive mmakefile is outside the source root".to_owned())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err("archive mmakefile has an unsafe source-relative path".into());
        };
        let Some(part) = part.to_str() else {
            return Err("archive source-relative path is not UTF-8".into());
        };
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn parse_make_rules(lines: &[&str], states: &[ConditionalTruth], hidden: &[bool]) -> Vec<MakeRule> {
    let mut rules: Vec<MakeRule> = Vec::new();
    let mut current: Option<usize> = None;
    for (line, raw) in lines.iter().enumerate() {
        if hidden.get(line).copied().unwrap_or(true) {
            continue;
        }
        if let Some(recipe_text) = raw.strip_prefix('\t') {
            if let Some(index) = current {
                rules[index].recipes.push(Recipe {
                    text: recipe_text.to_owned(),
                    state: state_at(states, line),
                    line,
                });
            }
            continue;
        }
        let trimmed = strip_make_comment(raw).trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        current = None;
        // A bare one-stem Make target such as `%.a : input` is not a
        // GenMF directive. Keep it in the closure instead of silently hiding
        // it because its first byte is `%`. Ambiguous colon-bearing macro
        // text remains visible and is refused by target evaluation below.
        if is_assignment(trimmed)
            || (trimmed.starts_with('%') && !trimmed.contains(':'))
            || make_directive(trimmed)
        {
            continue;
        }
        let Some((target, prerequisites)) = trimmed.split_once(':') else {
            continue;
        };
        let target = target.trim();
        if target.is_empty() || target.contains(':') || target.contains(['\n', '\r']) {
            continue;
        }
        rules.push(MakeRule {
            target: target.to_owned(),
            prerequisites: prerequisites.trim().to_owned(),
            line,
            state: state_at(states, line),
            recipes: Vec::new(),
        });
        current = Some(rules.len() - 1);
    }
    rules
}

fn valid_owner_rule(rule: &MakeRule) -> bool {
    rule.recipes.is_empty()
        || (rule.recipes.len() == 1
            && rule.recipes[0].text == "@$(NOP)"
            && rule.recipes[0].state == ConditionalTruth::True)
}

fn effective_rule_count(
    rules: &[MakeRule],
    owner: &MakeRule,
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
) -> usize {
    let context = MakeExprContext::new(scope, dirs, owner.line, source_root, relative_dir);
    let Some(target) = evaluate_make_expr(&owner.target, &context).ok() else {
        return usize::MAX;
    };
    rules
        .iter()
        .filter(|rule| rule.state != ConditionalTruth::False)
        .filter(|rule| {
            let context = MakeExprContext::new(scope, dirs, rule.line, source_root, relative_dir);
            evaluate_make_expr(&rule.target, &context).is_ok_and(|other| other == target)
        })
        .count()
}

fn is_archive_candidate(
    rule: &MakeRule,
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
    configured_lib: &str,
) -> bool {
    let has_archive_macro = rule
        .recipes
        .iter()
        .any(|recipe| recipe.text.contains("%mklib_q"));
    if has_archive_macro {
        return true;
    }
    let context = MakeExprContext::new(scope, dirs, rule.line, source_root, relative_dir);
    evaluate_make_expr(&rule.target, &context)
        .is_ok_and(|target| normalize_archive_output(&target, configured_lib).is_some())
        || (rule.target.contains("AROS_LIB") && has_extension(&rule.target, "a"))
}

fn normalize_archive_output(path: &str, configured_lib: &str) -> Option<String> {
    if !safe_build_directory(configured_lib)
        || path.contains([';', '\\', '\n', '\r', '\'', '"', '`'])
    {
        return None;
    }
    let leaf = path.strip_prefix(configured_lib)?.strip_prefix('/')?;
    let name = leaf.strip_prefix("lib")?.strip_suffix(".a")?;
    if !safe_component(name) || leaf.contains('/') {
        return None;
    }
    Some(path.to_owned())
}

fn parse_members(
    raw: &str,
    line: usize,
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
    configured_gen: &str,
) -> Result<ArchiveMembers, String> {
    if raw.is_empty() || raw.contains([';', '\\', '\n', '\r', '\'', '"', '`']) {
        return Err("archive prerequisites are empty or contain unsupported control text".into());
    }
    if raw.split_whitespace().any(|word| matches!(word, "|" | ";")) {
        return Err("archive rule has unsupported order-only or command prerequisites".into());
    }
    let context = MakeExprContext::new(scope, dirs, line, source_root, relative_dir);
    match evaluate_make_list(raw, &context) {
        Ok(paths) => {
            if paths.is_empty() || paths.len() > MAX_ARCHIVES {
                return Err("archive member list is empty or exceeds the bounded limit".into());
            }
            let mut members = Vec::new();
            let mut seen = BTreeSet::new();
            let mut folded = BTreeSet::new();
            for path in paths {
                if !valid_object_path(&path, configured_gen) {
                    return Err(format!(
                        "archive member `{path}` is not one safe object below configured `GENDIR`"
                    ));
                }
                if !folded.insert(path.to_ascii_lowercase()) {
                    return Err("archive members have duplicate case-folded paths".into());
                }
                // `$^` de-duplicates repeated prerequisites while retaining
                // their first occurrence. Keep that precise order here.
                if seen.insert(path.clone()) {
                    members.push(path);
                }
            }
            if members.is_empty() {
                return Err("archive member list contains no unique objects".into());
            }
            Ok(ArchiveMembers::Exact(members))
        }
        Err(MakeExprError::DeferredWildcard { .. }) => {
            let Some(pattern_path) = deferred_wildcard_path(raw, scope, &context, line, 0) else {
                return Err(
                    "archive wildcard is not one explicit deferred AROS WILDCARD expression".into(),
                );
            };
            let Some((root, pattern)) = split_object_glob(&pattern_path, configured_gen) else {
                return Err(format!(
                    "deferred archive wildcard `{pattern_path}` is outside configured `GENDIR` or has an unsupported pattern"
                ));
            };
            Ok(ArchiveMembers::ProducerGlob { root, pattern })
        }
        Err(error) => Err(format!("cannot evaluate archive members: {error}")),
    }
}

fn archive_recipe_members(
    recipe: &str,
    prerequisites: &str,
    line: usize,
    context: &ArchiveRecipeContext<'_>,
    scan_bytes: &mut usize,
) -> Result<ArchiveMembers, String> {
    let ArchiveRecipeContext {
        scope,
        dirs,
        source_root,
        relative_dir,
        configured_gen,
        ..
    } = *context;
    if recipe == "%mklib_q from=$^" {
        return parse_members(
            prerequisites,
            line,
            scope,
            dirs,
            source_root,
            relative_dir,
            configured_gen,
        );
    }

    let Some(explicit_from) = recipe.strip_prefix("%mklib_q from=") else {
        return Err(
            "archive rule must use exactly %mklib_q from=$^ or one closed from= list".into(),
        );
    };
    if explicit_from.is_empty()
        || explicit_from.len() > MAX_EXPLICIT_FROM_BYTES
        || explicit_from.trim() != explicit_from
        || explicit_from.chars().any(char::is_whitespace)
        || explicit_from.contains([';', '\\', '\n', '\r', '\'', '"', '`'])
    {
        return Err(
            "explicit archive from= must be one bounded, unquoted Make list expression".into(),
        );
    }
    if explicit_from != prerequisites.trim() {
        return Err(
            "explicit archive from= must match the complete normal prerequisite expression".into(),
        );
    }

    let expression_context = MakeExprContext::new(scope, dirs, line, source_root, relative_dir);
    reject_unmodeled_archive_effects(context, line, scan_bytes)?;
    require_stable_archive_expression(
        explicit_from,
        line,
        context,
        &expression_context,
        scan_bytes,
    )?;

    let explicit_members = parse_members(
        explicit_from,
        line,
        scope,
        dirs,
        source_root,
        relative_dir,
        configured_gen,
    )?;
    // Raw expression identity plus stability through recipe execution means
    // the explicit from argument and this rule's normal prerequisites have
    // the same expansion context and ordered list. Evaluate once to avoid a
    // second independent expansion budget/allocation.
    match explicit_members {
        members @ ArchiveMembers::Exact(_) => Ok(members),
        ArchiveMembers::ProducerGlob { .. } => {
            Err("explicit archive from= must resolve to a finite object list".into())
        }
    }
}

fn reject_additional_archive_targets(
    output: &str,
    producer_line: usize,
    context: &ArchiveRecipeContext<'_>,
    proof_bytes: &mut usize,
) -> Result<(), String> {
    charge_archive_proof_bytes(context.source_bytes, proof_bytes)?;
    for rule in context.rules {
        if rule.state == ConditionalTruth::False {
            continue;
        }
        if rule.state != ConditionalTruth::True {
            return Err("archive prerequisite closure contains an unresolved rule".into());
        }
        let expression_context = MakeExprContext::new(
            context.scope,
            context.dirs,
            rule.line,
            context.source_root,
            context.relative_dir,
        );
        let targets = evaluate_make_list(&rule.target, &expression_context)
            .map_err(|_| "archive prerequisite closure contains an unresolved target".to_owned())?;
        let retained_bytes = targets.iter().try_fold(0usize, |total, target| {
            total
                .checked_add(target.len())
                .ok_or_else(|| "archive prerequisite target-identity budget overflowed".to_owned())
        })?;
        charge_archive_proof_bytes(retained_bytes, proof_bytes)?;
        if targets.is_empty()
            || targets.iter().any(|target| {
                target.contains(['\\', '*', '?', '[', ']', ';', '|', '&', '`', '"', '\''])
                    || target.chars().any(char::is_control)
                    || target.split('/').any(|part| matches!(part, "." | ".."))
                    || target.contains("//")
                    || !archive_target_has_closed_root(target)
            })
        {
            return Err(
                "archive prerequisite closure contains a nonliteral target identity".into(),
            );
        }
        for target in &targets {
            if target.contains('%') {
                // The target bytes were charged above. Charge the comparison
                // name too before folding or matching: many tiny patterns must
                // not multiply work against one long archive identity.
                charge_archive_proof_bytes(output.len(), proof_bytes)?;
                if archive_pattern_may_match(target, output)? {
                    return Err("archive output has a compatible active Make target pattern".into());
                }
            } else if rule.line != producer_line
                && (target.eq_ignore_ascii_case(output)
                    || (!target.starts_with(BUILD_ROOT)
                        && target.rsplit('/').next().is_some_and(|name| {
                            output
                                .rsplit('/')
                                .next()
                                .is_some_and(|archive| name.eq_ignore_ascii_case(archive))
                        })))
            {
                return Err(
                    "archive output has duplicate or case-colliding active Make targets".into(),
                );
            }
        }
    }
    Ok(())
}

fn archive_target_has_closed_root(target: &str) -> bool {
    if let Some(tail) = target.strip_prefix("${AROS_BUILD_DIR}/") {
        return !tail.is_empty() && !tail.contains('$');
    }
    // Physical absolute paths, home expansion, and other deferred roots
    // cannot be compared to the symbolic build root without its runtime
    // binding. Relative slash patterns also depend on Make's working
    // directory, which this source-only proof does not establish.
    !(target.starts_with(['/', '~'])
        || target.contains('$')
        || target.contains('%') && target.contains('/'))
}

/// Prove only disjointness, not implicit producer admission. GNU Make removes
/// directories before comparing a no-slash target pattern and restores them
/// in its stem. Consequently an empty basename stem is still potentially
/// compatible with a pathname archive. Accepting that possibility for slash
/// patterns as well is deliberately conservative.
fn archive_pattern_may_match(pattern: &str, output: &str) -> Result<bool, String> {
    let (prefix, suffix) = pattern
        .split_once('%')
        .ok_or_else(|| "archive target pattern has no stem marker".to_owned())?;
    if suffix.contains('%') {
        return Err("archive target pattern has multiple stem markers".into());
    }
    let compared = if pattern.contains('/') {
        output
    } else {
        output.rsplit('/').next().unwrap_or(output)
    };
    let compared = compared.to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    let suffix = suffix.to_ascii_lowercase();
    let fixed_bytes = prefix
        .len()
        .checked_add(suffix.len())
        .ok_or_else(|| "archive target pattern byte count overflowed".to_owned())?;
    Ok(compared.len() >= fixed_bytes
        && compared.starts_with(&prefix)
        && compared.ends_with(&suffix))
}

fn require_stable_archive_expression(
    expression: &str,
    line: usize,
    context: &ArchiveRecipeContext<'_>,
    expression_context: &MakeExprContext<'_>,
    scan_bytes: &mut usize,
) -> Result<(), String> {
    let ArchiveRecipeContext {
        scope,
        dirs,
        lines,
        states,
        hidden,
        source_bytes,
        ..
    } = *context;
    let mut pending = make_variable_references(expression)?
        .into_iter()
        .map(|name| (name, 0usize))
        .collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while let Some((name, depth)) = pending.pop() {
        if depth > MAX_EXPLICIT_FROM_DEPTH {
            return Err("explicit archive from= variable chain exceeds the bounded depth".into());
        }
        if !visited.insert(name.clone()) {
            continue;
        }
        if visited.len() > MAX_EXPLICIT_FROM_VARIABLES {
            return Err("explicit archive from= references too many Make variables".into());
        }
        reject_archive_variable_rebinding(
            &name,
            line,
            lines,
            states,
            hidden,
            source_bytes,
            scan_bytes,
        )?;

        // DirVars entries are the configured, immutable directory contract.
        // Prefer that closed binding for non-local names; recursively inspect
        // file-local and other source-scope values below.
        if !scope.is_known_local(&name) && dirs.expand(&format!("$({name})")).is_some() {
            continue;
        }
        if let Some(value) = scope.raw_at(&name, line) {
            for nested in make_variable_references(&value)? {
                pending.push((nested, depth + 1));
            }
            continue;
        }
        if evaluate_make_expr(&format!("$({name})"), expression_context).is_ok() {
            // Context-bound Make values such as CURDIR are fixed by this
            // declaring source location; source rebinding was checked above.
            continue;
        }
        return Err(format!(
            "explicit archive from= depends on unresolved Make variable {name}"
        ));
    }
    Ok(())
}

fn reject_unmodeled_archive_effects(
    context: &ArchiveRecipeContext<'_>,
    producer_line: usize,
    scan_bytes: &mut usize,
) -> Result<(), String> {
    let ArchiveRecipeContext {
        scope,
        dirs,
        source_root,
        relative_dir,
        lines,
        rules,
        states,
        hidden,
        source_bytes,
        ..
    } = *context;
    // VarScope marks opaque definition bodies inert for its own purpose;
    // that does not prove they cannot rebind recipe variables in GNU Make.
    if hidden.iter().any(|hidden| *hidden) {
        return Err("explicit archive from= cannot seal opaque Make definitions".into());
    }
    let admitted_recipe_line = rules
        .iter()
        .find(|rule| rule.line == producer_line)
        .filter(|rule| rule.recipes.len() == 1)
        .and_then(|rule| rule.recipes.first())
        .map(|recipe| recipe.line)
        .ok_or_else(|| "explicit archive from= has no unique bounded recipe line".to_owned())?;
    charge_archive_proof_bytes(source_bytes, scan_bytes)?;
    for (index, raw) in lines.iter().enumerate() {
        if raw.starts_with('#') {
            continue;
        }
        // GenMF runs before GNU Make interprets comments or conditional
        // branches. Inline/indented comments and unrelated tab recipes can
        // therefore inject source effects too. Exempt only this archive's
        // already bounded command, not every recipe containing a percent.
        if index != admitted_recipe_line && contains_genmf_reference(raw) {
            return Err(
                "source contains an unmodeled MetaMake directive affecting explicit archive from="
                    .into(),
            );
        }
        if state_at(states, index) == ConditionalTruth::False || raw.starts_with('\t') {
            continue;
        }
        let line = strip_make_comment(raw.trim());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if state_at(states, index) != ConditionalTruth::True {
            return Err("explicit archive from= has unresolved source effects".into());
        }
        if starts_include(line) {
            return Err("an include has unproved effects on explicit from= expansion".into());
        }
        // Unrelated immediate assignments can run indirect call/eval and
        // mutate recipe-time values. Only plain references are admitted in
        // this deliberately narrow, import-free source island.
        make_variable_references(line)?;
        let mut directive = line;
        while let Some((modifier, rest)) = directive.split_once(char::is_whitespace) {
            if !matches!(modifier, "override" | "export" | "private") {
                break;
            }
            directive = rest.trim_start();
        }
        if variable_assignment(directive)
            .is_some_and(|(_, _, kind)| kind == AssignmentKind::SetIfUnset)
        {
            // An environment-origin binding makes GNU Make skip this source
            // default. A source-only scope cannot prove which value is used.
            return Err(
                "explicit archive from= cannot seal environment-sensitive ?= bindings".into(),
            );
        }
        if directive
            .strip_prefix("undefine")
            .is_some_and(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
        {
            return Err("explicit archive from= source undefines Make variables".into());
        }
    }
    for rule in rules {
        if rule.state == ConditionalTruth::False {
            continue;
        }
        if rule.state != ConditionalTruth::True {
            return Err("explicit archive from= has an unresolved rule target".into());
        }
        charge_archive_proof_bytes(source_bytes, scan_bytes)?;
        let expression_context =
            MakeExprContext::new(scope, dirs, rule.line, source_root, relative_dir);
        let targets = evaluate_make_list(&rule.target, &expression_context)
            .map_err(|_| "explicit archive from= has an unresolved rule target".to_owned())?;
        if targets.len() != 1 {
            // A multi-target declaration can append prerequisites to the
            // archive without being recognized as an archive candidate.
            return Err("explicit archive from= requires one exact target per active rule".into());
        }
    }
    Ok(())
}

/// GenMF recognizes template references anywhere in a non-comment line,
/// not merely at its start. A percent in an ordinary Make pattern (for
/// example `%.o`) is not such a reference. Unknown template names are still
/// refused: this source-only proof must not rely on a template allowlist.
fn contains_genmf_reference(line: &str) -> bool {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'%' || !bytes.get(index + 1).is_some_and(u8::is_ascii_alphanumeric) {
            continue;
        }
        let mut end = index + 2;
        while bytes
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            end += 1;
        }
        // Python's Unicode `\s` also includes the four information
        // separators that Rust's White_Space predicate excludes.
        if line[end..]
            .chars()
            .next()
            .is_none_or(|next| next.is_whitespace() || matches!(next, '\u{1c}'..='\u{1f}'))
        {
            return true;
        }
    }
    false
}

fn charge_archive_proof_bytes(bytes: usize, proof_bytes: &mut usize) -> Result<(), String> {
    let next = proof_bytes
        .checked_add(bytes)
        .ok_or_else(|| "archive prerequisite proof-byte budget overflowed".to_owned())?;
    if next > MAX_ARCHIVE_PROOF_BYTES {
        return Err("archive prerequisite proof-byte budget was exceeded".into());
    }
    *proof_bytes = next;
    Ok(())
}

fn reject_archive_variable_rebinding(
    name: &str,
    rule_line: usize,
    lines: &[&str],
    states: &[ConditionalTruth],
    hidden: &[bool],
    source_bytes: usize,
    scan_bytes: &mut usize,
) -> Result<(), String> {
    charge_archive_proof_bytes(source_bytes, scan_bytes)?;
    for (index, raw) in lines.iter().enumerate() {
        if hidden.get(index).copied().unwrap_or(true)
            || state_at(states, index) == ConditionalTruth::False
        {
            continue;
        }
        let line = strip_make_comment(raw.trim());
        if line.is_empty() || line.starts_with('#') || raw.starts_with('\t') {
            continue;
        }
        if index > rule_line && (starts_include(line) || line.starts_with("%include_deps")) {
            return Err(
                "an include after the archive rule has unproved effects on explicit from= expansion"
                    .into(),
            );
        }
        let assigned_name = assignment_name(line);
        if index > rule_line && assigned_name == Some(name) {
            return Err(format!(
                "explicit archive from= variable {name} is rebound after its prerequisites"
            ));
        }
        if index > rule_line && dynamic_global_assignment_may_be_present(line) {
            return Err(
                "source contains an unrecognized global assignment after the archive rule".into(),
            );
        }
        if index > rule_line && line.starts_with("undefine ") {
            return Err(format!(
                "source contains an unproved undefine after the archive prerequisites ({name})"
            ));
        }
        // A global := assignment contains a colon but is not a target rule.
        // Its recipe-time rebinding was checked above.
        if assigned_name.is_some() {
            continue;
        }
        if let Some((_, target_specific)) = line.split_once(':') {
            if assignment_name(target_specific) == Some(name) {
                return Err(format!(
                    "explicit archive from= variable {name} has a target-specific override"
                ));
            }
            if target_specific_assignment_may_be_dynamic(target_specific) {
                return Err(
                    "source contains an unrecognized target-specific assignment that may affect explicit archive from="
                        .into(),
                );
            }
        }
    }
    Ok(())
}

fn target_specific_assignment_may_be_dynamic(value: &str) -> bool {
    let mut body = value.trim();
    while let Some((modifier, rest)) = body.split_once(char::is_whitespace) {
        if !matches!(modifier, "override" | "export" | "private") {
            break;
        }
        body = rest.trim_start();
    }
    ["::=", ":=", "+=", "?=", "="]
        .iter()
        .any(|operator| body.contains(operator))
}

fn dynamic_global_assignment_may_be_present(value: &str) -> bool {
    if assignment_name(value).is_some() || make_directive(value) {
        return false;
    }
    let assignment_at = ["::=", ":=", "+=", "?=", "="]
        .iter()
        .filter_map(|operator| value.find(operator))
        .min();
    assignment_at.is_some_and(|at| !value[..at].contains(':'))
}

fn make_variable_references(value: &str) -> Result<Vec<String>, String> {
    if value.len() > MAX_EXPLICIT_FROM_BYTES {
        return Err("explicit archive from= variable value exceeds the bounded size".into());
    }
    let bytes = value.as_bytes();
    let mut cursor = 0usize;
    let mut references = Vec::new();
    while cursor < bytes.len() {
        if bytes[cursor] != b'$' {
            cursor += 1;
            continue;
        }
        let Some(open) = bytes.get(cursor + 1).copied() else {
            return Err("explicit archive from= contains a dangling dollar sign".into());
        };
        let close = match open {
            b'(' => b')',
            b'{' => b'}',
            _ => {
                return Err(
                    "explicit archive from= contains an unsupported automatic or escaped variable"
                        .into(),
                );
            }
        };
        let start = cursor + 2;
        let Some(relative_end) = bytes[start..].iter().position(|byte| *byte == close) else {
            return Err(
                "explicit archive from= contains an unterminated variable reference".into(),
            );
        };
        let end = start + relative_end;
        let name = &value[start..end];
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(
                "explicit archive from= accepts only plain Make variable references".into(),
            );
        }
        references.push(name.to_owned());
        if references.len() > MAX_EXPLICIT_FROM_VARIABLES {
            return Err("explicit archive from= references too many Make variables".into());
        }
        cursor = end + 1;
    }
    Ok(references)
}

fn deferred_wildcard_path(
    raw: &str,
    scope: &VarScope,
    context: &MakeExprContext<'_>,
    line: usize,
    depth: usize,
) -> Option<String> {
    if depth > 16 {
        return None;
    }
    let text = raw.trim();
    if let Some(body) = wrapped_make_body(text) {
        let body = body.trim();
        if safe_make_variable(body) {
            let value = scope.raw_at(body, line)?;
            return deferred_wildcard_path(&value, scope, context, line, depth + 1);
        }
        if let Some(inner) = body.strip_prefix("strip ") {
            return deferred_wildcard_path(inner, scope, context, line, depth + 1);
        }
        if let Some(inner) = body.strip_prefix("call WILDCARD,") {
            let argument = inner.trim();
            if argument.is_empty() || argument.contains(',') {
                return None;
            }
            return evaluate_make_expr(argument, context).ok();
        }
        return None;
    }
    None
}

fn split_object_glob(path: &str, configured_gen: &str) -> Option<(String, String)> {
    if path.contains([';', '\\', '\n', '\r', '\'', '"', '`', '?', '[', ']'])
        || path.matches('*').count() != 1
    {
        return None;
    }
    let (root, pattern) = path.rsplit_once('/')?;
    if pattern != "*.o" || !safe_descendant(root, configured_gen) {
        return None;
    }
    Some((root.to_owned(), pattern.to_owned()))
}

fn valid_object_path(path: &str, configured_gen: &str) -> bool {
    has_extension(path, "o") && safe_descendant(path, configured_gen)
}

fn has_extension(path: &str, extension: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|actual| actual.to_string_lossy().eq_ignore_ascii_case(extension))
}

fn safe_descendant(path: &str, base: &str) -> bool {
    let Some(leaf) = path
        .strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };
    if leaf.is_empty() || !safe_build_directory(base) {
        return false;
    }
    leaf.split('/').all(safe_component)
}

fn safe_build_directory(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(BUILD_ROOT) else {
        return false;
    };
    rest.starts_with('/') && rest[1..].split('/').all(safe_component)
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

fn collect_owners(
    lines: &[&str],
    states: &[ConditionalTruth],
    hidden: &[bool],
    rules: &[MakeRule],
) -> Vec<OwnerRule> {
    let mut owners = Vec::new();
    for (index, raw) in lines.iter().enumerate() {
        if hidden.get(index).copied().unwrap_or(true)
            || raw.starts_with('\t')
            || raw.trim() != "#MM"
            || state_at(states, index) == ConditionalTruth::False
        {
            continue;
        }
        let Some(rule_index) = rules.iter().position(|rule| rule.line == index + 1) else {
            continue;
        };
        let rule = &rules[rule_index];
        if rule.state == ConditionalTruth::False
            || !safe_owner(&rule.target)
            || !one_explicit_prerequisite(&rule.prerequisites)
        {
            continue;
        }
        owners.push(OwnerRule {
            owner: rule.target.clone(),
            prerequisites: rule.prerequisites.clone(),
            state: state_at(states, index),
            rule_index,
        });
    }
    owners
}

fn one_explicit_prerequisite(prerequisites: &str) -> bool {
    !prerequisites.is_empty()
        && !prerequisites.contains(['|', ';', '\\', '\n', '\r'])
        && prerequisites.split_whitespace().count() == 1
}

fn sole_word(value: &str) -> Option<&str> {
    let mut words = value.split_whitespace();
    let word = words.next()?;
    words.next().is_none().then_some(word)
}

fn safe_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 160
        && owner.as_bytes()[0].is_ascii_alphanumeric()
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}

fn source_control_error(
    lines: &[&str],
    states: &[ConditionalTruth],
    hidden: &[bool],
) -> Option<String> {
    for (index, raw) in lines.iter().enumerate() {
        let state = state_at(states, index);
        if state == ConditionalTruth::False {
            continue;
        }
        if hidden.get(index).copied().unwrap_or(false) {
            return Some("source contains an opaque Make `define` body".into());
        }
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if [
            "$(eval", "${eval", "$(shell", "${shell", "$(file", "${file", "$(guile",
        ]
        .iter()
        .any(|effect| line.contains(effect))
        {
            return Some("source contains unsupported parse-time or shell side effects".into());
        }
        if starts_include(line) && line != "include $(SRCDIR)/config/aros.cfg" {
            return Some("source imports an unmodelled Make fragment".into());
        }
        if line.starts_with("%define") || line.starts_with("%end") {
            return Some("source contains unsupported MetaMake macro definitions".into());
        }
    }
    None
}

fn source_role_overrides(
    lines: &[&str],
    states: &[ConditionalTruth],
    hidden: &[bool],
    dirs: &DirVars,
) -> Option<String> {
    for (index, raw) in lines.iter().enumerate() {
        if hidden.get(index).copied().unwrap_or(true)
            || state_at(states, index) == ConditionalTruth::False
        {
            continue;
        }
        let line = strip_make_comment(raw.trim());
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let assignment = assignment_name(line);
        if assignment.is_some_and(is_archive_role) {
            if state_at(states, index) == ConditionalTruth::True
                && variable_assignment(line).is_some_and(|(name, value, kind)| {
                    kind == crate::make_vars::AssignmentKind::RecursiveSet
                        && match name {
                            "AR" => {
                                value.trim() == "$(NATIVE_TARGET_AR) cr"
                                    && dirs.expand("$(NATIVE_TARGET_AR)").as_deref()
                                        == Some("${CMAKE_AR}")
                            }
                            "RANLIB" => {
                                value.trim() == "$(NATIVE_TARGET_RANLIB)"
                                    && dirs.expand("$(NATIVE_TARGET_RANLIB)").as_deref()
                                        == Some("${CMAKE_RANLIB}")
                            }
                            _ => false,
                        }
                })
            {
                continue;
            }
            return Some(
                "source overrides archive command role `AR`/`RANLIB` or owner no-op `NOP`".into(),
            );
        }
        if line
            .strip_prefix("undefine ")
            .is_some_and(|name| is_archive_role(name.trim()))
        {
            return Some("source undefines archive command role `AR`/`RANLIB` or `NOP`".into());
        }
        if let Some((_, rhs)) = line.split_once(':') {
            if assignment_name(rhs).is_some_and(is_archive_role) {
                return Some(
                    "source contains a target-specific `AR`, `RANLIB`, or `NOP` override".into(),
                );
            }
        }
    }
    None
}

fn is_archive_role(name: &str) -> bool {
    matches!(
        name,
        "AR" | "RANLIB" | "NOP" | "NATIVE_TARGET_AR" | "NATIVE_TARGET_RANLIB"
    )
}

fn assignment_name(line: &str) -> Option<&str> {
    let mut body = line.trim();
    loop {
        if let Some((name, _, _)) = variable_assignment(body) {
            return Some(name);
        }
        let (modifier, rest) = body.split_once(char::is_whitespace)?;
        if !matches!(modifier, "override" | "export" | "private") {
            return None;
        }
        body = rest.trim_start();
    }
}

fn is_assignment(line: &str) -> bool {
    assignment_name(line).is_some()
}

fn make_directive(line: &str) -> bool {
    [
        "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "undefine",
    ]
    .iter()
    .any(|directive| {
        line.strip_prefix(directive).is_some_and(|tail| {
            tail.is_empty() || tail.starts_with(char::is_whitespace) || tail.starts_with('(')
        })
    })
}

fn starts_include(line: &str) -> bool {
    ["include", "-include", "sinclude"].iter().any(|directive| {
        line.strip_prefix(directive)
            .is_some_and(|tail| tail.starts_with(char::is_whitespace))
    })
}

fn hidden_make_lines(lines: &[&str]) -> Vec<bool> {
    let mut hidden = vec![false; lines.len()];
    let mut depth = 0usize;
    for (index, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if begins_define(line) {
            hidden[index] = true;
            depth = depth.saturating_add(1);
        } else if depth > 0 {
            hidden[index] = true;
            if line == "endef" {
                depth -= 1;
            }
        }
    }
    if depth > 0 {
        if let Some(first) = hidden.iter().position(|value| *value) {
            hidden[first..].fill(true);
        }
    }
    hidden
}

fn begins_define(line: &str) -> bool {
    let mut body = line;
    loop {
        if body == "define" || body.starts_with("define ") || body.starts_with("define\t") {
            return true;
        }
        let Some((modifier, rest)) = body.split_once(char::is_whitespace) else {
            return false;
        };
        if !matches!(modifier, "override" | "export" | "private") {
            return false;
        }
        body = rest.trim_start();
    }
}

fn state_at(states: &[ConditionalTruth], line: usize) -> ConditionalTruth {
    states
        .get(line)
        .copied()
        .unwrap_or(ConditionalTruth::Unknown)
}

fn wrapped_make_body(raw: &str) -> Option<&str> {
    let body = raw.strip_prefix("$(")?.strip_suffix(')')?;
    let mut depth = 0usize;
    let bytes = body.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'$' && bytes.get(index + 1) == Some(&b'(') {
            depth += 1;
            index += 2;
            continue;
        }
        if bytes[index] == b')' {
            if depth == 0 {
                return None;
            }
            depth -= 1;
        }
        index += 1;
    }
    (depth == 0).then_some(body)
}

fn safe_make_variable(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn reject_case_collisions(candidates: &mut [Candidate], owners: &[OwnerRule]) {
    let mut output_counts = BTreeMap::<String, usize>::new();
    let mut owner_counts = BTreeMap::<String, usize>::new();
    for candidate in candidates.iter() {
        *output_counts
            .entry(candidate.output.to_ascii_lowercase())
            .or_default() += 1;
        *owner_counts
            .entry(candidate.owner.to_ascii_lowercase())
            .or_default() += 1;
    }
    for candidate in candidates {
        if output_counts
            .get(&candidate.output.to_ascii_lowercase())
            .is_some_and(|count| *count > 1)
        {
            candidate.failure.get_or_insert_with(|| {
                "source declares duplicate or case-colliding archive outputs".into()
            });
        }
        if owner_counts
            .get(&candidate.owner.to_ascii_lowercase())
            .is_some_and(|count| *count > 1)
        {
            candidate.failure.get_or_insert_with(|| {
                "source declares duplicate or case-colliding archive owners".into()
            });
        }
        if owners.iter().any(|owner| {
            owner.owner.eq_ignore_ascii_case(&candidate.owner)
                && owner.rule_index != candidate.owner_rule_index
                && owner.state != ConditionalTruth::False
        }) {
            candidate.failure.get_or_insert_with(|| {
                "archive owner has duplicate or case-colliding ordinary `#MM` rules".into()
            });
        }
    }
}

fn unresolved_candidates(
    file: &str,
    rules: &[MakeRule],
    owners: &[OwnerRule],
    reason: &str,
) -> (Vec<SourceArchiveDecl>, Vec<SourceArchiveRejection>) {
    let mut rejections = Vec::new();
    for rule in rules.iter().filter(|rule| {
        rule.recipes
            .iter()
            .any(|recipe| recipe.text.contains("%mklib_q"))
    }) {
        let owner_matches = owners
            .iter()
            .filter(|item| {
                item.state != ConditionalTruth::False
                    && sole_word(&item.prerequisites) == Some(rule.target.as_str())
            })
            .collect::<Vec<_>>();
        let owner = if owner_matches.len() == 1 {
            owner_matches[0].owner.as_str()
        } else {
            "<unknown>"
        };
        rejections.push(SourceArchiveRejection {
            owner: owner.into(),
            file: file.into(),
            line: rule.line + 1,
            reason: reason.into(),
        });
    }
    (Vec::new(), rejections)
}

fn looks_archive_relevant(content: &str) -> bool {
    content.contains("%mklib_q")
        || content
            .lines()
            .any(|line| line.contains("AROS_LIB") && has_extension(line.trim_end(), "a"))
}

fn rejection(owner: &str, path: &Path, line: usize, reason: String) -> SourceArchiveRejection {
    SourceArchiveRejection {
        owner: owner.to_owned(),
        file: path.to_string_lossy().replace('\\', "/"),
        line,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_impl;
    use crate::parser::{join_continuations, TargetContext};
    use crate::testing::TempTree;
    use std::fs;

    const CONFIG: &str = "AROSDIR := $(TARGETDIR)/SYS\nAROS_DIR_DEVELOPER := Developer\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_DIR_LIB := lib\nAROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\nOBJDIR := $(GENDIR)/$(CURDIR)\n";

    fn fixture(source: &str) -> (TempTree, String, VarScope, DirVars, Vec<ConditionalTruth>) {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("config")).unwrap();
        fs::write(tree.0.join("config/make.cfg.in"), CONFIG).unwrap();
        fs::create_dir_all(tree.0.join("fixture")).unwrap();
        let joined = join_continuations(source);
        fs::write(tree.0.join("fixture/mmakefile.src"), &joined).unwrap();
        let context = TargetContext {
            make_variables: BTreeMap::from([(
                "OBJDIR".to_owned(),
                "$(GENDIR)/$(CURDIR)".to_owned(),
            )]),
            ..TargetContext::default()
        };
        let (scope, states) = collect_vars_impl(&joined, Some(&context));
        let dirs = DirVars::load(&tree.0);
        (tree, joined, scope, dirs, states)
    }

    fn collect(source: &str) -> (Vec<SourceArchiveDecl>, Vec<SourceArchiveRejection>) {
        let (tree, joined, scope, dirs, states) = fixture(source);
        collect_from_snapshot(
            &joined,
            &scope,
            &dirs,
            &tree.0,
            Path::new("fixture"),
            Some(&states),
        )
    }

    fn finite_source() -> &'static str {
        "FILES := alpha beta\nOBJS := $(foreach f,$(FILES),$(OBJDIR)/$(f).o)\nARCHIVE := $(AROS_LIB)/libunusual+name.a\n#MM\narchive-owner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from=$^\n"
    }

    #[test]
    fn finite_source_lists_keep_order_and_derive_archive_name() {
        let (declarations, rejections) = collect(finite_source());
        assert!(rejections.is_empty(), "{rejections:?}");
        assert_eq!(declarations.len(), 1);
        let declaration = &declarations[0];
        assert_eq!(declaration.owner, "archive-owner");
        assert_eq!(
            declaration.output,
            "${AROS_BUILD_DIR}/SYS/Developer/lib/libunusual+name.a"
        );
        assert!(matches!(
            &declaration.members,
            ArchiveMembers::Exact(members)
                if members == &[
                    "${AROS_BUILD_DIR}/gen/fixture/alpha.o",
                    "${AROS_BUILD_DIR}/gen/fixture/beta.o"
                ]
        ));
    }

    #[test]
    fn deferred_wildcard_is_preserved_as_data_without_expansion() {
        let source = "HIDD_LIB := $(AROS_LIB)/libhiddstubs.a\nHIDD_STUBS_OBJ := $(strip $(call WILDCARD, $(GENDIR)/lib/hidd/*.o))\n#MM\nlinklibs-hiddstubs: $(HIDD_LIB)\n$(HIDD_LIB) : $(HIDD_STUBS_OBJ)\n\t%mklib_q from=$^\n";
        let (declarations, rejections) = collect(source);
        assert!(rejections.is_empty(), "{rejections:?}");
        assert_eq!(declarations.len(), 1);
        assert_eq!(declarations[0].owner, "linklibs-hiddstubs");
        assert_eq!(
            declarations[0].members,
            ArchiveMembers::ProducerGlob {
                root: "${AROS_BUILD_DIR}/gen/lib/hidd".into(),
                pattern: "*.o".into(),
            }
        );
    }

    #[test]
    fn extra_or_modified_archive_recipes_are_refused() {
        for recipe in [
            "\t%mklib_q from=$^ extra=yes\n",
            "\t%mklib_q from=$^\n\t@echo extra\n",
            "\t@%mklib_q from=$^\n",
        ] {
            let source = finite_source().replace("\t%mklib_q from=$^\n", recipe);
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted recipe {recipe:?}");
            assert!(!rejections.is_empty(), "missed recipe {recipe:?}");
        }
    }

    fn explicit_from_source(recipe_from: &str, rule_members: &str, suffix: &str) -> String {
        format!(
            "OBJDIR := $(GENDIR)/fixture\nTOOL := gfxhiddtool\nOBJS := {rule_members}\nOTHER_OBJS := $(OBJDIR)/two.o $(OBJDIR)/one.o\nARCHIVE := $(AROS_LIB)/libgfxhiddtool.a\n#MM\narchive-owner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from={recipe_from}\n{suffix}"
        )
    }

    #[test]
    fn explicit_from_accepts_the_same_finite_nested_member_expression() {
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "");
        let (declarations, rejections) = collect(&source);
        assert!(rejections.is_empty(), "{rejections:?}");
        assert_eq!(declarations.len(), 1);
        assert_eq!(
            declarations[0].members,
            ArchiveMembers::Exact(vec![concat!(
                "$",
                "{AROS_BUILD_DIR}/gen/fixture/gfxhiddtool.o"
            )
            .into()])
        );
    }

    #[test]
    fn explicit_from_rejects_unequal_or_deferred_member_lists() {
        for source in [
            explicit_from_source("$(OBJDIR)/missing.o", "$(OBJDIR)/present.o", ""),
            explicit_from_source("$(OTHER_OBJS)", "$(OBJDIR)/one.o $(OBJDIR)/two.o", ""),
            explicit_from_source("$(OBJS)", "$(OBJDIR)/one.o $(OBJDIR)/one.o", ""),
            explicit_from_source(
                "$(OBJS)",
                "$(strip $(call WILDCARD,$(GENDIR)/lib/hidd/*.o))",
                "",
            ),
            explicit_from_source(
                "$(UNKNOWN_ARCHIVE_OBJECTS)",
                "$(UNKNOWN_ARCHIVE_OBJECTS)",
                "",
            ),
        ] {
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted {source:?}");
            assert!(!rejections.is_empty(), "missed {source:?}");
        }
    }

    #[test]
    fn explicit_from_rejects_extra_arguments_and_automatic_variables() {
        for recipe in [
            "\t%mklib_q from=$(OBJS) extra\n",
            "\t%mklib_q from=\"$(OBJS)\"\n",
            "\t%mklib_q from=$<\n",
            "\t%mklib_q from=$(call WILDCARD,$(GENDIR)/lib/hidd/*.o)\n",
        ] {
            let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "")
                .replace("\t%mklib_q from=$(OBJS)\n", recipe);
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted recipe {recipe:?}");
            assert!(!rejections.is_empty(), "missed recipe {recipe:?}");
        }
    }

    #[test]
    fn explicit_from_rejects_later_and_target_specific_rebindings() {
        for suffix in [
            "OBJDIR := $(GENDIR)/later\n",
            "OBJDIR = $(GENDIR)/later\n",
            "OBJDIR += later\n",
            "ARCHIVE: OBJDIR := $(GENDIR)/target-specific\n",
            "ARCHIVE: override OBJDIR = $(GENDIR)/target-specific\n",
            "ifeq ($(UNKNOWN_MODE),enabled)\nOBJDIR := $(GENDIR)/maybe\nendif\n",
        ] {
            let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", suffix);
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted suffix {suffix:?}");
            assert!(!rejections.is_empty(), "missed suffix {suffix:?}");
        }
    }

    #[test]
    fn explicit_from_rejects_nested_rebindings_and_late_includes() {
        let nested = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "TOOL := rebound\n")
            .replace("OBJS :=", "OBJS =");
        let include = explicit_from_source(
            "$(OBJS)",
            "$(OBJDIR)/$(TOOL).o",
            "include $(SRCDIR)/config/aros.cfg\n",
        );
        let define = explicit_from_source(
            "$(OBJS)",
            "$(OBJDIR)/$(TOOL).o",
            "define OBJS\n$(OBJDIR)/other.o\nendef\n",
        );
        for source in [nested, include, define] {
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted {source:?}");
            assert!(!rejections.is_empty(), "missed {source:?}");
        }
    }

    #[test]
    fn explicit_from_rejects_indirect_effects_and_modified_undefine() {
        for suffix in [
            "override undefine OBJS\n",
            "UNRELATED := $(call MUTATE,OBJS)\n",
            "FN := eval\nUNRELATED := $($(FN) OBJS := changed)\n",
            "UNRELATED := text %rule_makedirs dirs=extra\n",
            "%include_deps\n",
        ] {
            let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", suffix);
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted effects {suffix:?}");
            assert!(!rejections.is_empty(), "missed effects {suffix:?}");
        }
        let source = format!(
            "include $(SRCDIR)/config/aros.cfg\n{}",
            explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", ""),
        );
        assert!(collect(&source).0.is_empty());
    }

    #[test]
    fn explicit_from_preserves_immediate_assignment_freezing() {
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "TOOL := rebound\n");
        let (declarations, rejections) = collect(&source);
        assert!(rejections.is_empty(), "{rejections:?}");
        assert!(matches!(
            declarations[0].members,
            ArchiveMembers::Exact(ref members) if members[0].ends_with("/gfxhiddtool.o")
        ));
    }

    #[test]
    fn explicit_from_rejects_environment_owned_output_aliases() {
        for suffix in [
            "$(UNMODELED_OUTPUT) : external.o\n",
            "$(UNMODELED_OUTPUT) : external.o\n\t@echo replacement\n",
            "$(UNMODELED_OUTPUT) $(ARCHIVE) : external.o\n",
        ] {
            let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", suffix);
            let (declarations, rejections) = collect(&source);
            assert!(
                declarations.is_empty(),
                "accepted unknown output {suffix:?}"
            );
            assert!(!rejections.is_empty(), "missed unknown output {suffix:?}");
        }
    }

    #[test]
    fn explicit_from_rejects_environment_sensitive_source_defaults() {
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "");
        for (original, conditional) in [
            ("ARCHIVE :=", "ARCHIVE ?="),
            ("OBJS :=", "OBJS ?="),
            ("TOOL :=", "TOOL ?="),
            ("OBJDIR :=", "export OBJDIR ?="),
        ] {
            assert!(
                source.contains(original),
                "missing probe binding {original}"
            );
            let conditional_source = source.replace(original, conditional);
            let (declarations, rejections) = collect(&conditional_source);
            assert!(declarations.is_empty(), "accepted {conditional_source:?}");
            assert!(!rejections.is_empty(), "missed {conditional_source:?}");
        }
    }

    #[test]
    fn explicit_from_rejects_known_multi_target_prerequisite_overlays() {
        for overlay in [
            "$(ARCHIVE) $(OTHER_TARGET) : external.o\n",
            "$(OTHER_TARGET) $(ARCHIVE) : external.o\n",
            "$(ARCHIVE) $(ARCHIVE) : external.o\n",
        ] {
            let suffix = format!("OTHER_TARGET := $(OBJDIR)/other.o\n{overlay}");
            let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", &suffix);
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted {overlay:?}");
            assert!(!rejections.is_empty(), "missed {overlay:?}");
        }
    }

    #[test]
    fn automatic_from_rejects_known_and_unresolved_archive_overlays() {
        for suffix in [
            "OTHER_TARGET := $(OBJDIR)/other.o\n$(ARCHIVE) $(OTHER_TARGET) : external.o\n",
            "OTHER_TARGET := $(OBJDIR)/other.o\n$(OTHER_TARGET) $(ARCHIVE) : external.o\n",
            "$(UNMODELED_OUTPUT) : external.o\n",
            "$(AROS_LIB)/../lib/libgfxhiddtool.a : external.o\n",
        ] {
            let source = explicit_from_source("$^", "$(OBJDIR)/$(TOOL).o", suffix);
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted {suffix:?}");
            assert!(!rejections.is_empty(), "missed {suffix:?}");
        }
    }

    #[test]
    fn automatic_from_keeps_unrelated_known_multi_target_rules() {
        let source = explicit_from_source(
            "$^",
            "$(OBJDIR)/$(TOOL).o",
            "$(OBJDIR)/one.o $(OBJDIR)/two.o : input.c\n",
        );
        let (declarations, rejections) = collect(&source);
        assert_eq!(declarations.len(), 1, "{rejections:?}");
        assert!(rejections.is_empty(), "{rejections:?}");
    }

    #[test]
    fn archive_prerequisite_closure_keeps_provably_disjoint_patterns() {
        for from in ["$^", "$(OBJS)"] {
            for pattern in [
                "$(OBJDIR)/%.o",
                "%.o",
                "$(OBJDIR)/%/libgfxhiddtool.a",
                "libother%.a",
            ] {
                let source = explicit_from_source(
                    from,
                    "$(OBJDIR)/$(TOOL).o",
                    &format!("{pattern} : input.c\n"),
                );
                let (declarations, rejections) = collect(&source);
                assert_eq!(declarations.len(), 1, "{pattern}: {rejections:?}");
                assert!(rejections.is_empty(), "{pattern}: {rejections:?}");
            }
        }
    }

    #[test]
    fn genmf_reference_detection_does_not_hide_inline_or_unknown_templates() {
        for line in [
            "%compile_q",
            "OUTPUT := %unknown_template argument=value",
            "prefix%future_template\targument=value",
            "lib%unmodeled : input.o",
            "value := %1_name\u{2003}argument=value",
            "value := %1_name\u{1c}argument=value",
        ] {
            assert!(contains_genmf_reference(line), "missed {line:?}");
        }
        for line in ["%.o : %.c", "lib%other.a : input.o", "% : input.o", "x%/y"] {
            assert!(!contains_genmf_reference(line), "misclassified {line:?}");
        }
        for suffix in [
            "UNRELATED := prefix%unknown_template argument=value\n",
            "%future_template argument=value\n",
            "lib%unmodeled : input.o\n",
            "UNRELATED := value # %compile_q\n",
            "  # %compile_q\n",
            "other-target:\n\t%compile_q\n",
            "other-target:\n\t@echo value # %compile_q\n",
            "ifeq (no,yes)\n%future_template\nendif\n",
        ] {
            let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", suffix);
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted {suffix:?}");
            assert!(!rejections.is_empty(), "missed {suffix:?}");
        }
    }

    #[test]
    fn archive_prerequisite_closure_rejects_compatible_or_ambiguous_patterns() {
        for from in ["$^", "$(OBJS)"] {
            for pattern in [
                "$(AROS_LIB)/lib%.a",
                "lib%.a",
                "%.a",
                "%",
                "LIB%.A",
                "lib%gfxhiddtool.a",
                "$(TARGETDIR)/%/libgfxhiddtool.a",
                "lib%%.a",
                "$(AROS_LIB)/../lib/lib%.a",
                "$(AROS_LIB)//lib%.a",
                "lib*.a",
                "lib?.a",
                "lib[ab].a",
                "lib\\%.a",
            ] {
                let source = explicit_from_source(
                    from,
                    "$(OBJDIR)/$(TOOL).o",
                    &format!("{pattern} : external.o\n"),
                );
                let (declarations, rejections) = collect(&source);
                assert!(declarations.is_empty(), "accepted {from}: {pattern}");
                assert!(!rejections.is_empty(), "missed {from}: {pattern}");
            }
        }
    }

    #[test]
    fn archive_pattern_matching_preserves_directory_and_empty_basename_stems() {
        let output = "${AROS_BUILD_DIR}/SYS/Developer/lib/libopenurl.a";
        assert!(archive_pattern_may_match("lib%openurl.a", output).unwrap());
        assert!(archive_pattern_may_match("${AROS_BUILD_DIR}/%/libopenurl.a", output).unwrap());
        assert!(archive_pattern_may_match("LIB%OPENURL.A", output).unwrap());
        assert!(!archive_pattern_may_match("${AROS_BUILD_DIR}/gen/%.a", output).unwrap());
        assert!(!archive_pattern_may_match("%.o", output).unwrap());
        assert!(archive_pattern_may_match("lib%%.a", output).is_err());
    }

    #[test]
    fn archive_prerequisite_closure_rejects_unknown_pattern_conditions() {
        let source = explicit_from_source(
            "$^",
            "$(OBJDIR)/$(TOOL).o",
            "ifeq ($(UNKNOWN_ARCHIVE_MODE),enabled)\n$(OBJDIR)/%.o : input.c\nendif\n",
        );
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty());
        assert!(!rejections.is_empty());
    }

    #[test]
    fn archive_prerequisite_closure_rejects_unbound_root_and_working_directory_aliases() {
        for from in ["$^", "$(OBJS)"] {
            for target in [
                "/tmp/build/SYS/Developer/lib/libgfxhiddtool.a",
                "/tmp/build/SYS/Developer/lib/lib%.a",
                "~/build/libgfxhiddtool.a",
                "~other/build/lib%.a",
                "${AROS_SOURCE_DIR}/build/SYS/Developer/lib/libgfxhiddtool.a",
                "${AROS_SOURCE_DIR}/build/SYS/Developer/lib/%.a",
                "${AROS_BUILD_DIR}/${CMAKE_CONFIG}/libgfxhiddtool.a",
                "SYS/Developer/lib/%.a",
                "libgfxhiddtool.a",
                "SYS/Developer/lib/LIBGFXHIDDTOOL.A",
            ] {
                let source = explicit_from_source(
                    from,
                    "$(OBJDIR)/$(TOOL).o",
                    &format!("{target}: external.o\n"),
                );
                let (declarations, rejections) = collect(&source);
                assert!(declarations.is_empty(), "accepted {from}: {target}");
                assert!(!rejections.is_empty(), "missed {from}: {target}");
            }
        }
        for target in ["setup", "clean", "other.o", "relative/path/other.a"] {
            let source =
                explicit_from_source("$^", "$(OBJDIR)/$(TOOL).o", &format!("{target}: input.c\n"));
            let (declarations, rejections) = collect(&source);
            assert_eq!(declarations.len(), 1, "{target}: {rejections:?}");
            assert!(rejections.is_empty(), "{target}: {rejections:?}");
        }
    }

    #[test]
    fn archive_proof_bytes_are_shared_and_fail_without_reset_or_overflow() {
        let mut bytes = MAX_ARCHIVE_PROOF_BYTES - 4;
        charge_archive_proof_bytes(4, &mut bytes).unwrap();
        assert_eq!(bytes, MAX_ARCHIVE_PROOF_BYTES);
        assert!(charge_archive_proof_bytes(1, &mut bytes).is_err());
        assert_eq!(bytes, MAX_ARCHIVE_PROOF_BYTES);
        let mut overflowing = usize::MAX;
        assert!(charge_archive_proof_bytes(1, &mut overflowing).is_err());
        assert_eq!(overflowing, usize::MAX);
    }

    #[test]
    fn admitted_archive_role_defaults_are_explicit_and_not_ambient() {
        let source = format!(
            "AR = $(NATIVE_TARGET_AR) cr\nRANLIB = $(NATIVE_TARGET_RANLIB)\n{}",
            finite_source()
        );
        let (tree, joined, scope, mut dirs, states) = fixture(&source);
        let (_, rejected) = collect_from_snapshot(
            &joined,
            &scope,
            &dirs,
            &tree.0,
            Path::new("fixture"),
            Some(&states),
        );
        assert!(!rejected.is_empty());
        dirs.bind_native_target_tool_roles();
        let (decls, rejected) = collect_from_snapshot(
            &joined,
            &scope,
            &dirs,
            &tree.0,
            Path::new("fixture"),
            Some(&states),
        );
        assert_eq!(decls.len(), 1, "{rejected:?}");
        assert!(rejected.is_empty());
        let mutation = format!("{source}NATIVE_TARGET_AR := unsafe\n");
        let (tree, joined, scope, mut dirs, states) = fixture(&mutation);
        dirs.bind_native_target_tool_roles();
        let (decls, rejected) = collect_from_snapshot(
            &joined,
            &scope,
            &dirs,
            &tree.0,
            Path::new("fixture"),
            Some(&states),
        );
        assert!(decls.is_empty());
        assert!(!rejected.is_empty());
    }

    #[test]
    fn global_and_target_specific_archive_role_overrides_are_refused() {
        for prefix in [
            "AR := unsafe-ar\n",
            "override RANLIB = unsafe-ranlib\n",
            "$(AROS_LIB)/libunusual+name.a: AR = unsafe-ar\n",
            "archive-owner: RANLIB := unsafe-ranlib\n",
            "archive-owner: override RANLIB = unsafe-ranlib\n",
            "undefine AR\n",
        ] {
            let source = format!("{prefix}{}", finite_source());
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted override {prefix:?}");
            assert!(!rejections.is_empty(), "missed override {prefix:?}");
        }
    }

    #[test]
    fn duplicate_case_colliding_archive_outputs_are_refused() {
        let source = "FILES := one\nOBJS := $(OBJDIR)/$(FILES).o\nA := $(AROS_LIB)/libFoo.a\nB := $(AROS_LIB)/libfoo.a\n#MM\none-owner : $(A)\n$(A) : $(OBJS)\n\t%mklib_q from=$^\n#MM\ntwo-owner : $(B)\n$(B) : $(OBJS)\n\t%mklib_q from=$^\n";
        let (declarations, rejections) = collect(source);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("case-colliding")));
    }

    #[test]
    fn missing_or_ambiguous_owner_is_refused() {
        let missing =
            "OBJS := $(OBJDIR)/x.o\nA := $(AROS_LIB)/libx.a\n$(A) : $(OBJS)\n\t%mklib_q from=$^\n";
        let (declarations, rejections) = collect(missing);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("no unique")));

        let ambiguous = "OBJS := $(OBJDIR)/x.o\nA := $(AROS_LIB)/libx.a\n#MM\none-owner : $(A)\n#MM\ntwo-owner : $(A)\n$(A) : $(OBJS)\n\t%mklib_q from=$^\n";
        let (declarations, rejections) = collect(ambiguous);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("multiple ordinary")));
    }

    #[test]
    fn unknown_condition_and_opaque_make_control_are_refused() {
        let conditional =
            "ifeq ($(UNKNOWN_MODE),enabled)\n".to_owned() + finite_source() + "endif\n";
        let (declarations, rejections) = collect(&conditional);
        assert!(declarations.is_empty());
        assert!(!rejections.is_empty());

        for control in [
            "define HIDDEN\nAR := unsafe-ar\nendef\n",
            "$(eval AR := unsafe-ar)\n",
            "include $(UNMODELLED_CONFIG)\n",
        ] {
            let source = format!("{}{}", control, finite_source());
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted control {control:?}");
            assert!(!rejections.is_empty(), "missed control {control:?}");
        }
    }

    #[test]
    fn unsafe_members_and_outside_archive_root_are_refused() {
        for source in [
            "FILES := ../escape\nOBJS := $(addprefix $(OBJDIR)/,$(FILES))\nARCHIVE := $(AROS_LIB)/libescape.a\n#MM\nowner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from=$^\n",
            "FILES := object\nOBJS := $(OBJDIR)/$(FILES).o\nARCHIVE := /tmp/libforeign.a\n#MM\nowner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from=$^\n",
            "FILES := object\nOBJS := $(OBJDIR)/$(FILES).c\nARCHIVE := $(AROS_LIB)/libbad.a\n#MM\nowner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from=$^\n",
        ] {
            let (declarations, rejections) = collect(source);
            assert!(declarations.is_empty(), "accepted unsafe source {source:?}");
            assert!(!rejections.is_empty());
        }
    }

    #[test]
    #[ignore = "requires the isolated P4 checkout via AROS_P4_SOURCE_ROOT"]
    fn actual_p4_openurl_and_hidd_archive_rules_are_source_owned() {
        let source_root = std::env::var("AROS_P4_SOURCE_ROOT")
            .expect("explicit P4 archive source probe requires AROS_P4_SOURCE_ROOT");
        let source_root = Path::new(&source_root)
            .canonicalize()
            .expect("AROS_P4_SOURCE_ROOT must resolve");
        let dirs = DirVars::load(&source_root);
        let mut seen = BTreeSet::new();
        for (relative_dir, expected_owner, expected_suffix, glob) in [
            (
                Path::new("external/openurl/libopenurl"),
                "linklibs-openurl-quick",
                "libopenurl.a",
                false,
            ),
            (
                Path::new("compiler/libhiddstubs"),
                "linklibs-hiddstubs",
                "libhiddstubs.a",
                true,
            ),
        ] {
            let content = fs::read_to_string(source_root.join(relative_dir).join("mmakefile.src"))
                .expect("read P4 archive rule source");
            let joined = crate::parser::join_continuations(&content);
            let target_context = TargetContext {
                make_variables: BTreeMap::from([(
                    "OBJDIR".to_owned(),
                    "$(GENDIR)/$(CURDIR)".to_owned(),
                )]),
                ..TargetContext::default()
            };
            let (scope, states) = collect_vars_impl(&joined, Some(&target_context));
            let (declarations, rejections) = collect_from_snapshot(
                &joined,
                &scope,
                &dirs,
                &source_root,
                relative_dir,
                Some(&states),
            );
            assert!(rejections.is_empty(), "{relative_dir:?}: {rejections:?}");
            assert_eq!(declarations.len(), 1, "{relative_dir:?}");
            let declaration = &declarations[0];
            assert_eq!(declaration.owner, expected_owner);
            assert!(declaration.output.ends_with(expected_suffix));
            assert_eq!(
                matches!(declaration.members, ArchiveMembers::ProducerGlob { .. }),
                glob
            );
            seen.insert(declaration.output.clone());
        }
        assert_eq!(seen.len(), 2);
    }
}
