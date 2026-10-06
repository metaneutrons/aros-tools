//! Closed projection for source-owned C-to-assembly generated headers.
//!
//! The admitted shape compiles one regular source file to a build-tree `.s`
//! file, then extracts quoted assembler strings into a build-tree `.h` file
//! with the bounded `grep | cut | sed` recipe. A finite `#MM` provider and a
//! finite aggregate declaration must own that header. This module reads no
//! generated files, does not execute Make or shell commands, and does not
//! infer compiler flags.

mod compiler_arguments;
mod native_snapshot;
mod snapshot_rules;

use compiler_arguments::{compiler_arguments, parse_compiler_arguments};
pub(crate) use native_snapshot::native_configuration_snapshot;
use native_snapshot::prepare_native_scope;
use snapshot_rules::{
    has_extension, is_build_directory, is_build_output, is_build_path, is_generated_output,
    line_state, parse_snapshot, safe_owner_name, source_relative_path, split_target_colon,
    valid_relative_tail,
};

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{
    collect_vars_impl, strip_make_comment, undefine_directive, variable_assignment, AssignmentKind,
    ConditionalTruth, VarScope,
};
use crate::parser::TargetContext;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

const SOURCE_ALIAS: &str = "${AROS_SOURCE_DIR}";
const BUILD_ALIAS: &str = "${AROS_BUILD_DIR}";
const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_RULES: usize = 16_384;
const MAX_REJECTIONS: usize = 256;

/// One closed C-to-assembly-to-header producer and its owning Make targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyHeaderDecl {
    /// Source-local `#MM` provider target for the generated header.
    pub owner: String,
    /// Source-local aggregate target that exposes the provider.
    pub aggregate_owner: String,
    /// Expanded ordered prerequisites of the aggregate declaration.
    pub aggregate_dependencies: Vec<String>,
    /// Source-relative mmakefile path.
    pub file: String,
    /// 1-based physical source line of the provider target rule.
    pub line: usize,
    /// Regular C source under `${AROS_SOURCE_DIR}`.
    pub source: String,
    /// Assembly output under `${AROS_BUILD_DIR}`.
    pub assembly_output: String,
    /// Header output under `${AROS_BUILD_DIR}`.
    pub header_output: String,
    /// Resolved source-configured Make `GENINCDIR` root for this header.
    pub header_root: String,
    /// Ordered compiler arguments, excluding the compiler, source, `-S`, and
    /// output pair. Every argument comes from the explicit Make scope.
    pub arguments: Vec<String>,
    /// The admitted grep selector, `.asciz` or `.ascii`.
    pub token: String,
}

/// A relevant assembly-header producer that could not be proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyHeaderRejection {
    /// Best-known local owner, or `<unknown>` when ownership is ambiguous.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// 1-based physical source line before Make continuation joining.
    pub line: usize,
    /// Why the producer is outside this closed capability.
    pub reason: String,
}

#[derive(Debug, Clone)]
struct Recipe {
    text: String,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct Rule {
    target: String,
    prerequisites: String,
    order_only: Option<String>,
    line: usize,
    state: ConditionalTruth,
    bare_meta_line: Option<usize>,
    inline_recipe: Option<String>,
    recipes: Vec<Recipe>,
}

#[derive(Debug, Clone)]
struct MetaRule {
    target: String,
    prerequisites: String,
    line: usize,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct SnapshotRules {
    rules: Vec<Rule>,
    meta_rules: Vec<MetaRule>,
}

struct CandidateContext<'a> {
    scope: &'a VarScope,
    line_states: &'a [ConditionalTruth],
    arch_definitions: &'a [String],
    dirs: &'a DirVars,
    root: &'a Path,
    relative_dir: &'a Path,
    file: &'a str,
}

#[derive(Debug)]
struct CandidateError {
    owner: String,
    line: usize,
    reason: String,
}

/// Collects the bounded assembly-header pipeline from a physical mmakefile snapshot.
///
/// Make continuations are joined internally while original line anchors are
/// retained. Make variables and conditional states are
/// reconstructed from this snapshot plus `target`; reached Make expressions
/// run with filesystem enumeration disabled. Source file reads only verify
/// that the mmakefile and its C input are regular files below `root`.
#[must_use]
pub fn collect_from_snapshot(
    snapshot: &str,
    target: &TargetContext,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> (Vec<AssemblyHeaderDecl>, Vec<AssemblyHeaderRejection>) {
    let file_hint = relative_mmakefile(relative_dir);
    if snapshot.len() > MAX_SNAPSHOT_BYTES {
        return (
            Vec::new(),
            with_file(
                vec![rejection(
                    "<unknown>",
                    1,
                    "assembly-header mmakefile exceeds the 2 MiB scan limit",
                )],
                &file_hint,
            ),
        );
    }

    let (source_joined, source_physical_lines) = join_continuations_with_origins(snapshot);
    let (source_scope, source_states) = collect_vars_impl(&source_joined, Some(target));
    let source_parsed = parse_snapshot(&source_joined, &source_states);
    let source_candidate_indices = source_parsed
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            looks_like_header_pipeline(rule, &source_scope, dirs, root, relative_dir)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if source_candidate_indices.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let file = match safe_relative_file(root, relative_dir, "mmakefile.src") {
        Ok(path) => path,
        Err(reason) => {
            return (
                Vec::new(),
                finalize_rejections(
                    source_candidate_indices
                        .iter()
                        .map(|index| {
                            rejection("<unknown>", source_parsed.rules[*index].line + 1, &reason)
                        })
                        .take(MAX_REJECTIONS)
                        .collect(),
                    &file_hint,
                    &source_physical_lines,
                ),
            );
        }
    };
    let canonical_root = match root.canonicalize() {
        Ok(path) if path.is_dir() => path,
        _ => {
            return (
                Vec::new(),
                finalize_rejections(
                    vec![rejection(
                        "<unknown>",
                        source_parsed.rules[source_candidate_indices[0]].line + 1,
                        "selected source root is unavailable or not a directory",
                    )],
                    &file_hint,
                    &source_physical_lines,
                ),
            );
        }
    };
    let observed_snapshot = match read_bounded_snapshot(&file) {
        Ok(text) => text,
        Err(reason) => {
            return (
                Vec::new(),
                finalize_rejections(
                    vec![rejection(
                        "<unknown>",
                        source_parsed.rules[source_candidate_indices[0]].line + 1,
                        &reason,
                    )],
                    &file_hint,
                    &source_physical_lines,
                ),
            );
        }
    };
    if observed_snapshot != snapshot {
        return (
            Vec::new(),
            finalize_rejections(
                vec![rejection(
                    "<unknown>",
                    source_parsed.rules[source_candidate_indices[0]].line + 1,
                    "provided physical snapshot differs from the current source mmakefile",
                )],
                &file_hint,
                &source_physical_lines,
            ),
        );
    }
    let file = match file.strip_prefix(&canonical_root) {
        Ok(path) => path.to_string_lossy().replace('\\', "/"),
        Err(_) => {
            return (
                Vec::new(),
                finalize_rejections(
                    vec![rejection(
                        "<unknown>",
                        source_parsed.rules[source_candidate_indices[0]].line + 1,
                        "physical mmakefile is outside the selected source root",
                    )],
                    &file_hint,
                    &source_physical_lines,
                ),
            );
        }
    };

    let relative_recipe = relative_dir.join("mmakefile.src");
    let prepared =
        match prepare_native_scope(snapshot, target, dirs, &canonical_root, &relative_recipe) {
            Ok(prepared) => prepared,
            Err(reason) => {
                return (
                    Vec::new(),
                    finalize_rejections(
                        source_candidate_indices
                            .iter()
                            .map(|index| {
                                rejection(
                                    "<unknown>",
                                    source_parsed.rules[*index].line + 1,
                                    &reason,
                                )
                            })
                            .take(MAX_REJECTIONS)
                            .collect(),
                        &file,
                        &source_physical_lines,
                    ),
                );
            }
        };
    let parsed = parse_snapshot(&prepared.rule_view, &prepared.line_states);
    let candidate_indices = parsed
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            looks_like_header_pipeline(rule, &prepared.scope, dirs, &canonical_root, relative_dir)
                .then_some(index)
        })
        .collect::<Vec<_>>();
    if candidate_indices.is_empty() {
        return (Vec::new(), Vec::new());
    }

    if let Some((line, reason)) = role_mutation_issue(&prepared.joined, &prepared.line_states) {
        return (
            Vec::new(),
            finalize_rejections(
                candidate_indices
                    .iter()
                    .map(|index| {
                        rejection(
                            "<unknown>",
                            parsed.rules[*index].line + 1,
                            &format!(
                                "{reason} (source line {})",
                                physical_line(&prepared.physical_lines, line + 1)
                            ),
                        )
                    })
                    .collect(),
                &file,
                &prepared.physical_lines,
            ),
        );
    }

    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    let context = CandidateContext {
        scope: &prepared.scope,
        line_states: &prepared.line_states,
        arch_definitions: &prepared.arch_definitions,
        dirs,
        root: &canonical_root,
        relative_dir,
        file: &file,
    };
    for index in candidate_indices {
        match collect_one(&parsed, index, &context) {
            Ok(declaration) => declarations.push(declaration),
            Err(error) => rejections.push(rejection(&error.owner, error.line, &error.reason)),
        }
        if rejections.len() >= MAX_REJECTIONS {
            break;
        }
    }

    declarations.sort_by(|left, right| {
        (&left.owner, &left.header_output, left.line).cmp(&(
            &right.owner,
            &right.header_output,
            right.line,
        ))
    });
    for declaration in &mut declarations {
        declaration.line = physical_line(&prepared.physical_lines, declaration.line);
    }

    let mut claimed_outputs = BTreeMap::<String, usize>::new();
    for declaration in &declarations {
        *claimed_outputs
            .entry(declaration.header_output.clone())
            .or_default() += 1;
    }
    if claimed_outputs.values().any(|count| *count > 1) {
        let duplicate_outputs = claimed_outputs
            .into_iter()
            .filter_map(|(output, count)| (count > 1).then_some(output))
            .collect::<BTreeSet<_>>();
        declarations.retain(|declaration| !duplicate_outputs.contains(&declaration.header_output));
        rejections.extend(duplicate_outputs.into_iter().map(|output| {
            rejection(
                "<unknown>",
                1,
                &format!("multiple assembly-header declarations claim `{output}`"),
            )
        }));
    }
    rejections.sort_by(|left, right| {
        (&left.owner, left.line, &left.reason).cmp(&(&right.owner, right.line, &right.reason))
    });
    rejections.dedup();
    (
        declarations,
        finalize_rejections(rejections, &file, &prepared.physical_lines),
    )
}

fn collect_one(
    parsed: &SnapshotRules,
    header_index: usize,
    context: &CandidateContext<'_>,
) -> Result<AssemblyHeaderDecl, CandidateError> {
    let CandidateContext {
        scope,
        line_states,
        arch_definitions,
        dirs,
        root: canonical_root,
        relative_dir,
        file,
    } = context;
    let header_rule = &parsed.rules[header_index];
    let fallback_line = header_rule.line + 1;
    require_active_rule(header_rule, "<unknown>", "header", line_states)?;
    let header_path = eval_single_path(
        &header_rule.target,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        header_rule.line,
    )
    .map_err(|reason| candidate_error("<unknown>", fallback_line, reason))?;
    if !is_build_output(&header_path, ".h") {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            format!(
                "header rule target `{header_path}` is not a `.h` output below the configured build root"
            ),
        ));
    }
    let header_root = eval_single_path(
        "$(GENINCDIR)",
        scope,
        dirs,
        canonical_root,
        relative_dir,
        header_rule.line,
    )
    .map_err(|reason| candidate_error("<unknown>", fallback_line, reason))?;
    if !is_build_directory(&header_root)
        || !header_path
            .strip_prefix(&format!("{header_root}/"))
            .is_some_and(valid_relative_tail)
    {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "header output is not below the source-configured GENINCDIR build root",
        ));
    }
    reject_duplicate_rule_target(parsed, &header_path, header_index, context, fallback_line)?;

    let prerequisites = eval_words(
        &header_rule.prerequisites,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        header_rule.line,
    )
    .map_err(|reason| candidate_error("<unknown>", fallback_line, reason))?;
    let [assembly_path] = prerequisites.as_slice() else {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "header rule must have exactly one normal prerequisite",
        ));
    };
    if !is_generated_output(assembly_path, ".s") {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "header rule prerequisite is not a `.s` output below the configured generated root",
        ));
    }
    require_one_build_directory(
        header_rule,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        fallback_line,
    )?;
    require_active_recipes(header_rule, fallback_line, "header")?;
    if header_rule.inline_recipe.is_some() {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "inline recipes are outside the assembly-header grammar",
        ));
    }
    let header_recipes = active_recipe_texts(header_rule, fallback_line, "header")?;
    if header_recipes.as_slice()
        != [
            "@$(ECHO) Generating $@...",
            "@grep $(GREPTOKEN) $< | cut -d'\"' -f2 | sed 's/\\$$//g' >$@",
        ]
    {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "header recipe differs from the closed grep/cut/sed pipeline",
        ));
    }

    let providers = find_providers(
        parsed,
        &header_path,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        fallback_line,
    )?;
    let [(owner, provider_index)] = providers.as_slice() else {
        return Err(candidate_error(
            "<unknown>",
            fallback_line,
            "generated header must have exactly one bare-`#MM` provider target",
        ));
    };
    let provider = &parsed.rules[*provider_index];
    require_active_rule(provider, owner.as_str(), "provider", line_states)?;
    if provider.inline_recipe.is_some()
        || !active_recipe_texts(provider, provider.line + 1, "provider")?.is_empty()
    {
        return Err(candidate_error(
            owner,
            provider.line + 1,
            "provider target must be a recipe-free Make alias",
        ));
    }
    let provider_prerequisites = eval_words(
        &provider.prerequisites,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        provider.line,
    )
    .map_err(|reason| candidate_error(owner, provider.line + 1, reason))?;
    if provider_prerequisites != [header_path.clone()] || provider.order_only.is_some() {
        return Err(candidate_error(
            owner,
            provider.line + 1,
            "provider must depend on only the generated header",
        ));
    }

    let (aggregate_owner, aggregate_dependencies, aggregate_line, aggregate_rule_index) =
        find_aggregate(
            parsed,
            owner,
            provider.line + 1,
            scope,
            dirs,
            canonical_root,
            relative_dir,
        )?;
    let aggregate_rule = &parsed.rules[aggregate_rule_index];
    require_active_rule(aggregate_rule, &aggregate_owner, "aggregate", line_states)?;
    if aggregate_rule.inline_recipe.is_some()
        || !aggregate_rule.prerequisites.trim().is_empty()
        || aggregate_rule
            .order_only
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
        || active_recipe_texts(aggregate_rule, aggregate_rule.line + 1, "aggregate")? != ["@$(NOP)"]
    {
        return Err(candidate_error(
            &aggregate_owner,
            aggregate_line,
            "aggregate target must be a no-prerequisite `@$(NOP)` rule",
        ));
    }

    let assembly_rules = matching_rules(
        parsed,
        assembly_path,
        scope,
        dirs,
        canonical_root,
        relative_dir,
    );
    let [assembly_rule_index] = assembly_rules.as_slice() else {
        return Err(candidate_error(
            owner,
            provider.line + 1,
            "assembly output must have exactly one source-local compile rule",
        ));
    };
    let assembly_rule = &parsed.rules[*assembly_rule_index];
    require_active_rule(assembly_rule, owner, "assembly compile", line_states)?;
    if assembly_rule.inline_recipe.is_some() {
        return Err(candidate_error(
            owner,
            assembly_rule.line + 1,
            "inline compile recipes are outside the assembly-header grammar",
        ));
    }
    require_one_build_directory(
        assembly_rule,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        assembly_rule.line + 1,
    )
    .map_err(|mut error| {
        error.owner.clone_from(owner);
        error
    })?;
    let assembly_recipes =
        active_recipe_texts(assembly_rule, assembly_rule.line + 1, "assembly compile")?;
    let compile_echo_matches = matches!(
        assembly_recipes.first().copied(),
        Some("@$(ECHO) \"Compiling $<...\"" | "@$(ECHO) \"Compiling  $<...\"")
    );
    if assembly_recipes.len() != 2
        || !compile_echo_matches
        || assembly_recipes.get(1)
            != Some(&"@$(TARGET_CC) $(TARGET_SYSROOT) $(CFLAGS) $(PRIV_EXEC_INCLUDES) -S $< -o $@")
    {
        return Err(candidate_error(
            owner,
            assembly_rule.line + 1,
            "assembly recipe differs from the closed target-C compile-to-assembly command",
        ));
    }

    let compile_prerequisites = eval_words(
        &assembly_rule.prerequisites,
        scope,
        dirs,
        canonical_root,
        relative_dir,
        assembly_rule.line,
    )
    .map_err(|reason| candidate_error(owner, assembly_rule.line + 1, reason))?;
    let [source] = compile_prerequisites.as_slice() else {
        return Err(candidate_error(
            owner,
            assembly_rule.line + 1,
            "assembly compile rule must have exactly one normal prerequisite",
        ));
    };
    let source_relative = source_relative_path(source).ok_or_else(|| {
        candidate_error(
            owner,
            assembly_rule.line + 1,
            "assembly compiler input is not a source-root-relative path",
        )
    })?;
    if !has_extension(&source_relative, "c") {
        return Err(candidate_error(
            owner,
            assembly_rule.line + 1,
            "assembly compile input is not a C source file",
        ));
    }
    safe_relative_file(canonical_root, Path::new(&source_relative), "")
        .map_err(|reason| candidate_error(owner, assembly_rule.line + 1, reason))?;

    let arguments = compiler_arguments(scope, arch_definitions, dirs, canonical_root, relative_dir)
        .map_err(|reason| candidate_error(owner, assembly_rule.line + 1, reason))?;
    let token = evaluate_make_expr(
        "$(GREPTOKEN)",
        &expression_context(scope, dirs, canonical_root, relative_dir, usize::MAX),
    )
    .map_err(|error| {
        candidate_error(
            owner,
            fallback_line,
            format!("cannot resolve GREPTOKEN: {error}"),
        )
    })?;
    if let Some(reason) = scope.flavor_uncertainty_reason_at("GREPTOKEN", usize::MAX) {
        return Err(candidate_error(
            owner,
            fallback_line,
            format!("cannot safely resolve GREPTOKEN: {reason}"),
        ));
    }
    let token = unquote_grep_token(&token).ok_or_else(|| {
        candidate_error(
            owner,
            fallback_line,
            "GREPTOKEN must resolve to exactly `.asciz` or `.ascii`",
        )
    })?;

    if !safe_owner_name(owner) || !safe_owner_name(&aggregate_owner) {
        return Err(candidate_error(
            owner,
            provider.line + 1,
            "provider or aggregate owner is not a finite Make target name",
        ));
    }
    if aggregate_dependencies.iter().collect::<BTreeSet<_>>().len() != aggregate_dependencies.len()
    {
        return Err(candidate_error(
            owner,
            aggregate_line,
            "aggregate declaration contains duplicate prerequisites",
        ));
    }

    Ok(AssemblyHeaderDecl {
        owner: owner.clone(),
        aggregate_owner,
        aggregate_dependencies,
        file: (*file).to_owned(),
        line: provider.line + 1,
        source: source.clone(),
        assembly_output: assembly_path.clone(),
        header_output: header_path,
        header_root,
        arguments,
        token,
    })
}

fn find_providers(
    parsed: &SnapshotRules,
    header_path: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line: usize,
) -> Result<Vec<(String, usize)>, CandidateError> {
    let mut matches = Vec::new();
    for (index, rule) in parsed.rules.iter().enumerate() {
        if rule.state == ConditionalTruth::False || rule.bare_meta_line.is_none() {
            continue;
        }
        let target = eval_single_path(&rule.target, scope, dirs, root, relative_dir, rule.line);
        let prerequisites = eval_words(
            &rule.prerequisites,
            scope,
            dirs,
            root,
            relative_dir,
            rule.line,
        );
        if let (Ok(owner), Ok(values)) = (target, prerequisites) {
            if values == [header_path.to_owned()] {
                matches.push((owner, index));
            }
        }
    }
    if matches.len() > 1 {
        return Err(candidate_error(
            "<unknown>",
            line,
            "multiple bare-`#MM` provider targets claim the generated header",
        ));
    }
    Ok(matches)
}

fn find_aggregate(
    parsed: &SnapshotRules,
    provider_owner: &str,
    line: usize,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> Result<(String, Vec<String>, usize, usize), CandidateError> {
    let mut matches = Vec::new();
    for meta in &parsed.meta_rules {
        if meta.state == ConditionalTruth::False {
            continue;
        }
        let context = expression_context(scope, dirs, root, relative_dir, meta.line);
        let owner = evaluate_make_expr(&meta.target, &context);
        let dependencies = evaluate_make_list(&meta.prerequisites, &context);
        match (owner, dependencies) {
            (Ok(owner), Ok(dependencies))
                if dependencies.iter().any(|dep| dep == provider_owner) =>
            {
                if meta.state == ConditionalTruth::Unknown {
                    return Err(candidate_error(
                        provider_owner,
                        meta.line + 1,
                        "aggregate declaration is inside an unresolved Make conditional",
                    ));
                }
                matches.push((owner, dependencies, meta.line));
            }
            _ => {}
        }
    }
    if matches.len() != 1 {
        return Err(candidate_error(
            provider_owner,
            line,
            if matches.is_empty() {
                "provider has no finite `#MM` aggregate declaration"
            } else {
                "provider appears in multiple `#MM` aggregate declarations"
            },
        ));
    }
    let (aggregate_owner, dependencies, aggregate_line) = matches.pop().expect("one match");
    if !safe_owner_name(&aggregate_owner)
        || dependencies.is_empty()
        || dependencies
            .iter()
            .any(|dependency| !safe_owner_name(dependency))
    {
        return Err(candidate_error(
            provider_owner,
            aggregate_line + 1,
            "aggregate declaration is not a finite list of Make target names",
        ));
    }
    if aggregate_owner == provider_owner {
        return Err(candidate_error(
            provider_owner,
            aggregate_line + 1,
            "aggregate owner cannot be its own provider",
        ));
    }

    let mut aggregate_rules = Vec::new();
    for (index, rule) in parsed.rules.iter().enumerate() {
        if rule.state == ConditionalTruth::False || rule.bare_meta_line.is_none() {
            continue;
        }
        if eval_single_path(&rule.target, scope, dirs, root, relative_dir, rule.line)
            .is_ok_and(|name| name == aggregate_owner)
        {
            aggregate_rules.push(index);
        }
    }
    let [aggregate_rule_index] = aggregate_rules.as_slice() else {
        return Err(candidate_error(
            &aggregate_owner,
            aggregate_line + 1,
            "aggregate declaration must have exactly one matching bare-`#MM` Make rule",
        ));
    };
    Ok((
        aggregate_owner,
        dependencies,
        aggregate_line + 1,
        *aggregate_rule_index,
    ))
}

fn unquote_grep_token(value: &str) -> Option<String> {
    let token = match value {
        "\".asciz\"" | ".asciz" => ".asciz",
        "\".ascii\"" | ".ascii" => ".ascii",
        _ => return None,
    };
    Some(token.to_owned())
}

fn looks_like_header_pipeline(
    rule: &Rule,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> bool {
    let has_extraction_signal = rule
        .recipes
        .iter()
        .map(|recipe| recipe.text.as_str())
        .chain(rule.inline_recipe.iter().map(String::as_str))
        .any(|recipe| {
            recipe.contains("$(GREPTOKEN)")
                || recipe.contains(".asciz")
                || recipe.contains(".ascii")
        });
    if has_extraction_signal {
        return true;
    }
    let target_looks_header = has_extension(&rule.target, "h")
        || eval_single_path(&rule.target, scope, dirs, root, relative_dir, rule.line)
            .is_ok_and(|path| has_extension(&path, "h"));
    let prereq_looks_assembly = eval_words(
        &rule.prerequisites,
        scope,
        dirs,
        root,
        relative_dir,
        rule.line,
    )
    .is_ok_and(|words| words.iter().any(|word| has_extension(word, "s")));
    target_looks_header && prereq_looks_assembly
}

fn reject_duplicate_rule_target(
    parsed: &SnapshotRules,
    output: &str,
    selected: usize,
    context: &CandidateContext<'_>,
    line: usize,
) -> Result<(), CandidateError> {
    let matches = matching_rules(
        parsed,
        output,
        context.scope,
        context.dirs,
        context.root,
        context.relative_dir,
    );
    if matches.len() != 1 || matches[0] != selected {
        return Err(candidate_error(
            "<unknown>",
            line,
            "multiple Make rules claim the generated header output",
        ));
    }
    Ok(())
}

fn matching_rules(
    parsed: &SnapshotRules,
    output: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> Vec<usize> {
    parsed
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            (rule.state != ConditionalTruth::False
                && eval_single_path(&rule.target, scope, dirs, root, relative_dir, rule.line)
                    .is_ok_and(|target| target == output))
            .then_some(index)
        })
        .collect()
}

fn require_one_build_directory(
    rule: &Rule,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line: usize,
) -> Result<(), CandidateError> {
    let Some(raw) = rule.order_only.as_deref() else {
        return Err(candidate_error(
            "<unknown>",
            line,
            "producer must declare one order-only build directory",
        ));
    };
    let words = eval_words(raw, scope, dirs, root, relative_dir, rule.line)
        .map_err(|reason| candidate_error("<unknown>", line, reason))?;
    if words.len() != 1 || !is_build_directory(&words[0]) {
        return Err(candidate_error(
            "<unknown>",
            line,
            "producer order-only prerequisite must be one directory below the build root",
        ));
    }
    Ok(())
}

fn require_active_rule(
    rule: &Rule,
    owner: &str,
    description: &str,
    line_states: &[ConditionalTruth],
) -> Result<(), CandidateError> {
    if rule.state != ConditionalTruth::True
        || rule
            .bare_meta_line
            .is_some_and(|line| line_state(line_states, line) != ConditionalTruth::True)
    {
        return Err(candidate_error(
            owner,
            rule.line + 1,
            format!("{description} rule is inside an unresolved Make conditional"),
        ));
    }
    Ok(())
}

fn require_active_recipes(
    rule: &Rule,
    line: usize,
    description: &str,
) -> Result<(), CandidateError> {
    if rule
        .recipes
        .iter()
        .any(|recipe| recipe.state == ConditionalTruth::Unknown)
    {
        return Err(candidate_error(
            "<unknown>",
            line,
            format!("{description} recipe is inside an unresolved Make conditional"),
        ));
    }
    Ok(())
}

fn active_recipe_texts<'a>(
    rule: &'a Rule,
    line: usize,
    description: &str,
) -> Result<Vec<&'a str>, CandidateError> {
    require_active_recipes(rule, line, description)?;
    Ok(rule
        .recipes
        .iter()
        .filter(|recipe| recipe.state == ConditionalTruth::True)
        .map(|recipe| recipe.text.trim())
        .collect())
}

fn eval_single_path(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line: usize,
) -> Result<String, String> {
    let values = eval_words(raw, scope, dirs, root, relative_dir, line)?;
    match values.as_slice() {
        [value] => Ok(value.clone()),
        _ => Err("Make path expression must resolve to exactly one word".into()),
    }
}

fn eval_words(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line: usize,
) -> Result<Vec<String>, String> {
    evaluate_make_list(
        raw,
        &expression_context(scope, dirs, root, relative_dir, line),
    )
    .map_err(|error| format!("cannot resolve Make expression `{raw}`: {error}"))
}

const fn expression_context<'a>(
    scope: &'a VarScope,
    dirs: &'a DirVars,
    root: &'a Path,
    relative_dir: &'a Path,
    line: usize,
) -> MakeExprContext<'a> {
    MakeExprContext::new(scope, dirs, line, root, relative_dir).without_filesystem()
}

fn safe_relative_file(root: &Path, relative: &Path, file_name: &str) -> Result<PathBuf, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("selected source root cannot be canonicalized: {error}"))?;
    if !canonical_root.is_dir() {
        return Err("selected source root is not a directory".into());
    }
    let mut candidate = relative.to_path_buf();
    if !file_name.is_empty() {
        candidate.push(file_name);
    }
    if candidate.as_os_str().is_empty()
        || candidate.is_absolute()
        || candidate
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("source path is not a normal source-root-relative path".into());
    }
    let mut current = canonical_root.clone();
    let components = candidate.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err("source path contains a non-normal component".into());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "source path `{}` is unavailable: {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "source path `{}` is a symlink and is not admitted",
                current.display()
            ));
        }
        let last = index + 1 == components.len();
        if last && !metadata.is_file() {
            return Err(format!(
                "source path `{}` is not a regular file",
                current.display()
            ));
        }
        if !last && !metadata.is_dir() {
            return Err(format!(
                "source path parent `{}` is not a directory",
                current.display()
            ));
        }
    }
    if !current.starts_with(&canonical_root) {
        return Err("source path resolves outside the selected source root".into());
    }
    Ok(current)
}

fn read_bounded_snapshot(path: &Path) -> Result<String, String> {
    let measured = aros_common::measure_regular_file_bounded(path, MAX_SNAPSHOT_BYTES as u64)
        .map_err(|error| format!("physical mmakefile cannot be read: {error}"))?;
    let Some((_, bytes)) = measured else {
        return Err("physical mmakefile is missing".into());
    };
    String::from_utf8(bytes).map_err(|_| "physical mmakefile is not valid UTF-8".into())
}

fn role_mutation_issue(
    snapshot: &str,
    line_states: &[ConditionalTruth],
) -> Option<(usize, String)> {
    let mut target_cc_count = 0usize;
    for (line, raw) in snapshot.lines().enumerate() {
        if raw.starts_with('\t') || line_state(line_states, line) == ConditionalTruth::False {
            continue;
        }
        let clean = strip_make_comment(raw).trim();
        if clean.contains("$(eval") || clean.contains("${eval") {
            return Some((line, "Make `eval` could mutate compiler arguments".into()));
        }
        if let Some((name, value, kind)) = variable_assignment(clean) {
            match name {
                "NATIVE_TARGET_CC" => {
                    return Some((
                        line,
                        "source rebinds the admitted native compiler alias".into(),
                    ));
                }
                "TARGET_CC" => {
                    target_cc_count += 1;
                    if value.trim() != "$(NATIVE_TARGET_CC)"
                        || kind != AssignmentKind::RecursiveSet
                        || target_cc_count > 1
                    {
                        return Some((
                            line,
                            "source rebinds TARGET_CC outside its admitted native alias".into(),
                        ));
                    }
                }
                "TARGET_SYSROOT" if value.trim() != "--sysroot=$(AROS_DEVELOPER)" => {
                    return Some((
                        line,
                        "source rebinds TARGET_SYSROOT outside the configured Developer root"
                            .into(),
                    ));
                }
                _ => {}
            }
        }
        if let Some((_, remainder)) = split_target_colon(clean) {
            if variable_assignment(remainder).is_some_and(|(name, _, _)| {
                matches!(name, "TARGET_CC" | "NATIVE_TARGET_CC" | "TARGET_SYSROOT")
            }) {
                return Some((
                    line,
                    "source uses a target-specific compiler-role assignment".into(),
                ));
            }
        }
        if let Ok(Some(name)) = undefine_directive(clean) {
            if matches!(name, "TARGET_CC" | "NATIVE_TARGET_CC" | "TARGET_SYSROOT") {
                return Some((line, format!("source undefines compiler role `{name}`")));
            }
        }
        if clean.starts_with("define TARGET_CC")
            || clean.starts_with("define NATIVE_TARGET_CC")
            || clean.starts_with("define TARGET_SYSROOT")
        {
            return Some((line, "source defines a compiler role opaquely".into()));
        }
    }
    None
}

fn rejection(owner: &str, line: usize, reason: &str) -> AssemblyHeaderRejection {
    AssemblyHeaderRejection {
        owner: owner.to_owned(),
        file: String::new(),
        line,
        reason: reason.to_owned(),
    }
}

fn relative_mmakefile(relative_dir: &Path) -> String {
    let path = relative_dir.join("mmakefile.src");
    path.to_string_lossy().replace('\\', "/")
}

fn with_file(
    mut rejections: Vec<AssemblyHeaderRejection>,
    file: &str,
) -> Vec<AssemblyHeaderRejection> {
    for rejection in &mut rejections {
        file.clone_into(&mut rejection.file);
    }
    rejections
}

fn finalize_rejections(
    mut rejections: Vec<AssemblyHeaderRejection>,
    file: &str,
    physical_lines: &[usize],
) -> Vec<AssemblyHeaderRejection> {
    for rejection in &mut rejections {
        file.clone_into(&mut rejection.file);
        rejection.line = physical_line(physical_lines, rejection.line);
    }
    rejections
}

fn physical_line(physical_lines: &[usize], joined_line: usize) -> usize {
    physical_lines
        .get(joined_line.saturating_sub(1))
        .map_or(joined_line, |line| line + 1)
}

fn join_continuations_with_origins(source: &str) -> (String, Vec<usize>) {
    crate::parser::join_continuations_with_origins(source)
}

fn candidate_error(owner: &str, line: usize, reason: impl Into<String>) -> CandidateError {
    CandidateError {
        owner: owner.to_owned(),
        line,
        reason: reason.into(),
    }
}

#[cfg(test)]
#[path = "assembly_headers_tests.rs"]
mod tests;
