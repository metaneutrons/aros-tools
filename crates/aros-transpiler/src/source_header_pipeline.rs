//! Closed projection for a source-owned generated-header copy pipeline.
//!
//! This scanner models one narrow ordinary-Make shape: a named `#MM` owner
//! depends on an SDK header; that header is copied from a generated include
//! header; and the generated header is copied from one regular source file,
//! optionally followed by ordered whole-line substitutions.  It does not
//! infer ambient generated files or interpret general shell/Make recipes.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{strip_make_comment, variable_assignment, ConditionalTruth, VarScope};
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path};

const MAX_MAKEFILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PIPELINES: usize = 128;
const MAX_REPLACEMENTS: usize = 32;
const MAX_PATH_BYTES: usize = 1024;

/// One operation in source order.
///
/// `ReplaceWholeLineInPlace` deliberately represents GNU Make's `sed -i`
/// semantics; it must not be converted to an
/// ordinary `HeaderTransformDecl`, whose backend requires distinct input and
/// output files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceHeaderPipelineStep {
    /// Exact `$<` to `$@` copy recipe.
    Copy {
        input: String,
        output: String,
        line: usize,
    },
    /// One bounded `s/.*TOKEN.*/REPLACEMENT/` in-place substitution.
    ReplaceWholeLineInPlace {
        path: String,
        token: String,
        replacement: String,
        line: usize,
    },
}

/// Source-evaluated closure for one SDK header output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceHeaderPipelineDecl {
    /// Named ordinary Make target following a bare `#MM`, or the owner of a
    /// direct `#MM owner : ...` declaration.
    pub owner: String,
    /// Declaring mmakefile relative to the selected source root.
    pub file: String,
    /// 1-based logical snapshot line of the owning Make declaration.
    pub owner_line: usize,
    /// 1-based logical snapshot line of the generated-header rule.
    pub line: usize,
    /// 1-based logical snapshot line of the SDK mirror rule, not its recipe.
    pub sdk_rule_line: usize,
    /// Independently reconstructed physical location, separate from the
    /// logical coordinates needed for ordered lowering.
    pub diagnostic_location: Option<aros_common::SourceLocation>,
    /// Physical source identity of the final SDK mirror rule, reconstructed
    /// independently of logical operation ordering. Without this proof a
    /// legacy projection of the same output cannot be reconciled.
    pub sdk_rule_location: Option<aros_common::SourceLocation>,
    /// Exact source prerequisite copied into `generated_output`.
    pub source_prerequisite: String,
    /// Expanded, ordered dependencies retained from the named owner rule.
    pub owner_prerequisites: Vec<String>,
    /// Header under the configured generated include root.
    pub generated_output: String,
    /// Header under the configured SDK include root.
    pub sdk_output: String,
    /// Copy, optional in-place substitutions, and final mirror in source order.
    pub steps: Vec<SourceHeaderPipelineStep>,
}

/// A relevant named pipeline that was not proved by the closed scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub owner: Option<String>,
    /// 1-based source line of the owner or the concrete generated rule.
    pub line: usize,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct LogicalLine {
    text: String,
    source_line: usize,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct Recipe {
    text: String,
    source_line: usize,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct Rule {
    target: String,
    prerequisites: String,
    order_only: Option<String>,
    source_line: usize,
    state: ConditionalTruth,
    inline_recipe: bool,
    double_colon: bool,
    target_specific_control: bool,
    recipes: Vec<Recipe>,
}

#[derive(Debug, Clone)]
struct OwnerMarker {
    raw_owner: String,
    raw_prerequisites: Option<String>,
    owner_rule_line: Option<usize>,
    owner_line: usize,
    source_line: usize,
    state: ConditionalTruth,
}

#[derive(Debug)]
struct Candidate {
    owner: Option<String>,
    raw_owner: String,
    raw_prerequisites: String,
    prerequisites_line: usize,
    owner_line: usize,
    marker_line: usize,
    owner_rule: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct SourceRoots<'a> {
    expression: &'a Path,
    canonical_source: &'a Path,
}

#[derive(Debug, Clone, Copy)]
struct SourceDirectories<'a> {
    relative: &'a Path,
    relative_string: &'a str,
}

#[derive(Debug)]
struct CandidateResult {
    owner: Option<String>,
    declarations: Vec<SourceHeaderPipelineDecl>,
    claimed_outputs: Vec<String>,
    claimed_inputs: Vec<String>,
    rejection_line: usize,
    rejection: Option<String>,
}

/// Collects bounded generated-header copy chains from one continuation-joined
/// Makefile snapshot. `line_states` uses zero-based snapshot line coordinates;
/// an explicit `False` recipe is omitted, while a missing or `Unknown` state
/// for a relevant recipe rejects the candidate.
#[must_use]
pub(crate) fn collect(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SourceHeaderPipelineDecl>, Vec<Rejection>) {
    if snapshot.len() > MAX_MAKEFILE_BYTES {
        return (
            Vec::new(),
            vec![Rejection {
                owner: None,
                line: 1,
                reason: "header pipeline Makefile exceeds the 4 MiB scan limit".into(),
            }],
        );
    }

    let lines = logical_lines(snapshot, line_states);
    let rules = parse_rules(&lines);
    let markers = parse_owner_markers(&lines, &rules);
    let candidates = candidates(&markers, &rules, scope, dirs, root, relative_dir);
    if candidates.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if candidates.len() > MAX_PIPELINES {
        return rejected_candidates(
            &candidates,
            &format!("header pipeline candidates exceed {MAX_PIPELINES}"),
        );
    }

    let relative_directory = match safe_relative_directory(relative_dir) {
        Ok(directory) => directory,
        Err(reason) => {
            return rejected_candidates(
                &candidates,
                &format!("header pipeline source directory: {reason}"),
            );
        }
    };
    let canonical_root = match root.canonicalize() {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            return rejected_candidates(&candidates, "selected source root is not a directory");
        }
        Err(error) => {
            return rejected_candidates(
                &candidates,
                &format!("selected source root cannot be canonicalized: {error}"),
            );
        }
    };

    let mut results = Vec::with_capacity(candidates.len());
    for candidate in &candidates {
        results.push(scan_candidate(
            candidate,
            &rules,
            &markers,
            scope,
            dirs,
            SourceRoots {
                expression: root,
                canonical_source: &canonical_root,
            },
            SourceDirectories {
                relative: relative_dir,
                relative_string: &relative_directory,
            },
        ));
    }

    reject_collisions(&mut results);

    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    for mut result in results {
        if let Some(reason) = result.rejection.take() {
            rejections.push(Rejection {
                owner: result.owner,
                line: result.rejection_line,
                reason,
            });
        } else {
            declarations.append(&mut result.declarations);
        }
    }
    (declarations, rejections)
}

fn candidates(
    markers: &[OwnerMarker],
    rules: &[Rule],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> Vec<Candidate> {
    let mut output = Vec::new();
    for marker in markers {
        if marker.state == ConditionalTruth::False {
            continue;
        }
        let (raw_prerequisites, owner_rule, prerequisites_line) =
            if let Some(prerequisites) = &marker.raw_prerequisites {
                (
                    prerequisites.clone(),
                    marker.owner_rule_line,
                    marker.source_line,
                )
            } else if let Some(line) = marker.owner_rule_line {
                let Some(rule) = rules.iter().find(|rule| rule.source_line == line) else {
                    continue;
                };
                (
                    rule.prerequisites.clone(),
                    Some(rule.source_line),
                    rule.source_line,
                )
            } else {
                let matching = rules
                    .iter()
                    .filter(|rule| {
                        rule.state != ConditionalTruth::False
                            && raw_target_matches_owner(&rule.target, &marker.raw_owner)
                    })
                    .collect::<Vec<_>>();
                if matching.len() != 1 {
                    continue;
                }
                (
                    matching[0].prerequisites.clone(),
                    Some(matching[0].source_line),
                    matching[0].source_line,
                )
            };
        if !has_source_copy_pipeline(
            &raw_prerequisites,
            prerequisites_line,
            rules,
            scope,
            dirs,
            root,
            relative_dir,
        ) {
            continue;
        }
        let owner = evaluate_owner(
            &marker.raw_owner,
            marker.owner_line,
            scope,
            dirs,
            root,
            relative_dir,
        );
        output.push(Candidate {
            owner,
            raw_owner: marker.raw_owner.clone(),
            raw_prerequisites,
            prerequisites_line,
            owner_line: marker.owner_line,
            marker_line: marker.source_line,
            owner_rule,
        });
    }
    output
}

fn scan_candidate(
    candidate: &Candidate,
    rules: &[Rule],
    markers: &[OwnerMarker],
    scope: &VarScope,
    dirs: &DirVars,
    roots: SourceRoots<'_>,
    directories: SourceDirectories<'_>,
) -> CandidateResult {
    let SourceRoots {
        expression: expression_root,
        canonical_source: source_root,
    } = roots;
    let SourceDirectories {
        relative: relative_dir,
        relative_string: relative_directory,
    } = directories;
    let mut result = CandidateResult {
        owner: candidate.owner.clone(),
        declarations: Vec::new(),
        claimed_outputs: Vec::new(),
        claimed_inputs: Vec::new(),
        rejection_line: candidate.owner_line + 1,
        rejection: None,
    };
    let rejection_line = Cell::new(result.rejection_line);
    let reject = |result: &mut CandidateResult, reason: String| {
        if result.rejection.is_none() {
            result.rejection_line = rejection_line.get();
            result.rejection = Some(reason);
        }
        result.declarations.clear();
    };

    let Some(owner) = candidate.owner.as_deref() else {
        reject(
            &mut result,
            "named #MM owner does not resolve to one safe target".into(),
        );
        return result;
    };
    if markers
        .iter()
        .filter(|marker| marker.state != ConditionalTruth::False)
        .filter_map(|marker| {
            evaluate_owner(
                &marker.raw_owner,
                marker.owner_line,
                scope,
                dirs,
                expression_root,
                relative_dir,
            )
        })
        .filter(|other| other == owner)
        .count()
        != 1
    {
        reject(
            &mut result,
            "named #MM owner is declared more than once".into(),
        );
        return result;
    }
    if marker_state(candidate, markers) != ConditionalTruth::True {
        reject(
            &mut result,
            "named #MM owner is guarded by an unresolved Make conditional".into(),
        );
        return result;
    }
    if !safe_owner(owner) {
        reject(
            &mut result,
            "named #MM owner is not a safe literal target".into(),
        );
        return result;
    }
    if source_rebinds_recipe_or_include_roots(scope) || rules.iter().any(target_specific_control) {
        reject(
            &mut result,
            "source-local or target-specific Make controls shadow a copy/include-root role".into(),
        );
        return result;
    }

    let owner_rule = candidate
        .owner_rule
        .and_then(|line| rules.iter().find(|rule| rule.source_line == line));
    let matching_owner_rules = rules
        .iter()
        .filter(|rule| rule.state != ConditionalTruth::False)
        .filter(|rule| {
            let context =
                MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
            evaluate_make_list(&rule.target, &context)
                .is_ok_and(|targets| targets.iter().any(|target| target == owner))
        })
        .collect::<Vec<_>>();
    if (owner_rule.is_some() && matching_owner_rules.len() != 1)
        || (owner_rule.is_none() && !matching_owner_rules.is_empty())
    {
        reject(
            &mut result,
            "named #MM owner has ambiguous ordinary Make rule ownership".into(),
        );
        return result;
    }
    if let Some(rule) = owner_rule {
        if rule.state != ConditionalTruth::True || rule.inline_recipe || !rule.recipes.is_empty() {
            reject(
                &mut result,
                "named #MM owner rule must be unconditionally dependency-only".into(),
            );
            return result;
        }
        if rule.order_only.is_some() {
            reject(
                &mut result,
                "named #MM owner rule has unsupported order-only prerequisites".into(),
            );
            return result;
        }
        let owner_context =
            MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
        match evaluate_make_list(&rule.target, &owner_context) {
            Ok(targets) if targets.len() == 1 && targets[0] == owner => {}
            Ok(_) => {
                reject(
                    &mut result,
                    "named #MM ordinary owner must resolve to exactly its named target".into(),
                );
                return result;
            }
            Err(error) => {
                reject(
                    &mut result,
                    format!("cannot resolve named #MM ordinary owner: {error}"),
                );
                return result;
            }
        }
    }

    let context = MakeExprContext::new(
        scope,
        dirs,
        candidate.prerequisites_line,
        expression_root,
        relative_dir,
    );
    let sdk_root = match validate_configured_root("$(AROS_INCLUDES)", &context, dirs) {
        Ok(value) => value,
        Err(reason) => {
            reject(&mut result, reason);
            return result;
        }
    };
    let generated_root = match validate_configured_root("$(GENINCDIR)", &context, dirs) {
        Ok(value) => value,
        Err(reason) => {
            reject(&mut result, reason);
            return result;
        }
    };
    let owner_prerequisites = match evaluate_make_list(&candidate.raw_prerequisites, &context) {
        Ok(words) if !words.is_empty() => words,
        Ok(_) => {
            reject(&mut result, "named #MM owner has no prerequisites".into());
            return result;
        }
        Err(error) => {
            reject(
                &mut result,
                format!("cannot resolve named #MM owner prerequisites: {error}"),
            );
            return result;
        }
    };
    if candidate.raw_prerequisites.contains('|') {
        reject(
            &mut result,
            "named #MM owner has unsupported order-only prerequisites".into(),
        );
        return result;
    }
    let mut sdk_headers = Vec::new();
    for prerequisite in &owner_prerequisites {
        if let Some(relative) = relative_under_root(prerequisite, &sdk_root) {
            if !safe_header(relative) {
                reject(
                    &mut result,
                    format!(
                        "named #MM owner has an unsafe SDK header prerequisite `{prerequisite}`"
                    ),
                );
                return result;
            }
            sdk_headers.push((prerequisite.clone(), relative.to_owned()));
        }
    }
    if sdk_headers.is_empty() {
        reject(
            &mut result,
            "named #MM owner no longer resolves to an SDK header prerequisite".into(),
        );
        return result;
    }
    if sdk_headers.len() > MAX_PIPELINES {
        reject(
            &mut result,
            format!("named #MM SDK headers exceed {MAX_PIPELINES}"),
        );
        return result;
    }
    let mut local_headers = HashSet::new();
    let mut local_inputs = HashSet::new();
    for (sdk_output, relative_header) in sdk_headers {
        rejection_line.set(candidate.owner_line + 1);
        if !local_headers.insert(sdk_output.to_ascii_lowercase()) {
            reject(
                &mut result,
                format!("named #MM owner repeats SDK output `{sdk_output}`"),
            );
            return result;
        }
        result.claimed_outputs.push(sdk_output.clone());
        let generated_output = format!(
            "{}/{}",
            generated_root.trim_end_matches('/'),
            relative_header
        );
        result.claimed_outputs.push(generated_output.clone());

        let generated_rules = matching_rules(
            rules,
            &generated_output,
            scope,
            dirs,
            expression_root,
            relative_dir,
        );
        rejection_line.set(
            generated_rules
                .first()
                .map_or(candidate.owner_line + 1, |rule| rule.source_line + 1),
        );
        if generated_rules.len() != 1 {
            reject(
                &mut result,
                if generated_rules.is_empty() {
                    format!("generated header `{generated_output}` has no explicit Make producer")
                } else {
                    format!("generated header `{generated_output}` has multiple Make producers")
                },
            );
            return result;
        }
        let generated_rule = generated_rules[0];
        if let Err(reason) = validate_output_rule(generated_rule) {
            reject(&mut result, format!("generated header rule: {reason}"));
            return result;
        }
        let generated_context = MakeExprContext::new(
            scope,
            dirs,
            generated_rule.source_line,
            expression_root,
            relative_dir,
        );
        validate_rule_output(
            generated_rule,
            &generated_context,
            &generated_root,
            &relative_header,
        )
        .unwrap_or_else(|reason| reject(&mut result, reason));
        if result.rejection.is_some() {
            return result;
        }
        let source_inputs =
            match evaluate_make_list(&generated_rule.prerequisites, &generated_context) {
                Ok(inputs) if inputs.len() == 1 => inputs,
                Ok(inputs) => {
                    reject(
                        &mut result,
                        format!(
                            "generated header must have exactly one source prerequisite, got {}",
                            inputs.len()
                        ),
                    );
                    return result;
                }
                Err(error) => {
                    reject(
                        &mut result,
                        format!("cannot resolve generated header source prerequisite: {error}"),
                    );
                    return result;
                }
            };
        let source_input = source_inputs[0].clone();
        if !local_inputs.insert(source_input.to_ascii_lowercase()) {
            reject(
                &mut result,
                format!(
                    "source prerequisite `{source_input}` is claimed more than once by `{owner}`"
                ),
            );
            return result;
        }
        if let Err(reason) = validate_regular_source_input(&source_input, source_root) {
            reject(&mut result, reason);
            return result;
        }
        result.claimed_inputs.push(source_input.clone());

        let mut steps = match parse_copy_rule(
            generated_rule,
            &generated_output,
            true,
            scope,
            dirs,
            expression_root,
            relative_dir,
        ) {
            Ok(steps) => steps,
            Err(reason) => {
                reject(&mut result, format!("generated header recipe: {reason}"));
                return result;
            }
        };
        if !matches!(steps.first(), Some(SourceHeaderPipelineStep::Copy { .. })) {
            reject(
                &mut result,
                "generated header pipeline must begin with the exact source copy".into(),
            );
            return result;
        }
        if let SourceHeaderPipelineStep::Copy { input, output, .. } = &mut steps[0] {
            if input != &source_input || output != &generated_output {
                reject(
                    &mut result,
                    "generated header copy endpoints differ from its Make rule".into(),
                );
                return result;
            }
        }

        let mirror_rules = matching_rules(
            rules,
            &sdk_output,
            scope,
            dirs,
            expression_root,
            relative_dir,
        );
        rejection_line.set(
            mirror_rules
                .first()
                .map_or(candidate.owner_line + 1, |rule| rule.source_line + 1),
        );
        if mirror_rules.len() != 1 {
            reject(
                &mut result,
                if mirror_rules.is_empty() {
                    format!("SDK header `{sdk_output}` has no explicit generated-header mirror")
                } else {
                    format!("SDK header `{sdk_output}` has multiple Make producers")
                },
            );
            return result;
        }
        let mirror_rule = mirror_rules[0];
        if let Err(reason) = validate_output_rule(mirror_rule) {
            reject(&mut result, format!("SDK mirror rule: {reason}"));
            return result;
        }
        let mirror_context = MakeExprContext::new(
            scope,
            dirs,
            mirror_rule.source_line,
            expression_root,
            relative_dir,
        );
        validate_rule_output(mirror_rule, &mirror_context, &sdk_root, &relative_header)
            .unwrap_or_else(|reason| reject(&mut result, reason));
        if result.rejection.is_some() {
            return result;
        }
        let mirror_inputs = match evaluate_make_list(&mirror_rule.prerequisites, &mirror_context) {
            Ok(inputs) if inputs.len() == 1 => inputs,
            Ok(inputs) => {
                reject(
                    &mut result,
                    format!(
                        "SDK mirror must have exactly one generated-header prerequisite, got {}",
                        inputs.len()
                    ),
                );
                return result;
            }
            Err(error) => {
                reject(
                    &mut result,
                    format!("cannot resolve SDK mirror prerequisite: {error}"),
                );
                return result;
            }
        };
        if mirror_inputs[0] != generated_output {
            reject(
                &mut result,
                format!(
                    "SDK mirror input `{}` is not generated header `{generated_output}`",
                    mirror_inputs[0]
                ),
            );
            return result;
        }
        match parse_copy_rule(
            mirror_rule,
            &sdk_output,
            false,
            scope,
            dirs,
            expression_root,
            relative_dir,
        ) {
            Ok(mut mirror_steps) if mirror_steps.len() == 1 => steps.append(&mut mirror_steps),
            Ok(_) => {
                reject(
                    &mut result,
                    "SDK mirror must contain exactly one copy operation".into(),
                );
                return result;
            }
            Err(reason) => {
                reject(&mut result, format!("SDK mirror recipe: {reason}"));
                return result;
            }
        }
        // Source directory expressions are proof inputs, not the engine's
        // physical layout. Publish only the configured include-role aliases,
        // exactly as the other typed header producers do.
        let include_alias = |path: &str| {
            relative_under_root(path, &sdk_root).map_or_else(
                || {
                    relative_under_root(path, &generated_root).map_or_else(
                        || path.to_owned(),
                        |relative| format!("${{AROS_GENINC_DIR}}/{relative}"),
                    )
                },
                |relative| format!("${{AROS_SDK_INCLUDE_DIR}}/{relative}"),
            )
        };
        for step in &mut steps {
            match step {
                SourceHeaderPipelineStep::Copy { input, output, .. } => {
                    *input = include_alias(input);
                    *output = include_alias(output);
                }
                SourceHeaderPipelineStep::ReplaceWholeLineInPlace { path, .. } => {
                    *path = include_alias(path);
                }
            }
        }
        result.declarations.push(SourceHeaderPipelineDecl {
            owner: owner.to_owned(),
            file: if relative_directory.is_empty() {
                "mmakefile.src".into()
            } else {
                format!("{relative_directory}/mmakefile.src")
            },
            owner_line: candidate.owner_line + 1,
            line: generated_rule.source_line + 1,
            sdk_rule_line: mirror_rule.source_line + 1,
            diagnostic_location: None,
            sdk_rule_location: None,
            source_prerequisite: source_input,
            owner_prerequisites: owner_prerequisites
                .iter()
                .map(|path| include_alias(path))
                .collect(),
            generated_output: include_alias(&generated_output),
            sdk_output: include_alias(&sdk_output),
            steps,
        });
    }
    result
}

fn parse_copy_rule(
    rule: &Rule,
    output: &str,
    allow_sed: bool,
    scope: &VarScope,
    dirs: &DirVars,
    expression_root: &Path,
    relative_dir: &Path,
) -> Result<Vec<SourceHeaderPipelineStep>, String> {
    if rule.state != ConditionalTruth::True {
        return Err("producer rule is in an unknown conditional state".into());
    }
    if rule.inline_recipe {
        return Err("inline recipes are unsupported".into());
    }
    if rule.double_colon {
        return Err("double-colon producer rules are unsupported".into());
    }
    if rule.order_only.is_some() {
        return Err("order-only prerequisites are unsupported".into());
    }
    let mut output_steps = Vec::new();
    let mut copy_line = None;
    let mut saw_echo = false;
    let mut saw_mkdir = false;
    let mut saw_sed = false;
    for recipe in &rule.recipes {
        match recipe.state {
            ConditionalTruth::False => continue,
            ConditionalTruth::Unknown => {
                return Err(format!(
                    "recipe at line {} has an unknown conditional state",
                    recipe.source_line + 1
                ));
            }
            ConditionalTruth::True => {}
        }
        let text = recipe.text.trim();
        if is_echo_recipe(text)? {
            if copy_line.is_some() || saw_sed {
                return Err("ECHO must precede copy/SED operations".into());
            }
            if saw_echo {
                return Err("more than one ECHO recipe is unsupported".into());
            }
            saw_echo = true;
        } else if let Some(directory) = text.strip_prefix("%mkdir_q dir=") {
            if saw_mkdir || copy_line.is_some() || saw_sed {
                return Err("mkdir_q must occur at most once before copy/SED operations".into());
            }
            let context =
                MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
            let resolved = evaluate_make_expr(directory.trim(), &context)
                .map_err(|error| format!("cannot resolve mkdir_q directory: {error}"))?;
            if resolved != output_parent(output)? {
                return Err(
                    "mkdir_q destination is not the generated output's exact parent".into(),
                );
            }
            saw_mkdir = true;
        } else if is_exact_copy_recipe(text) {
            if copy_line.replace(recipe.source_line).is_some() || saw_sed {
                return Err("copy recipe must occur exactly once before substitutions".into());
            }
            output_steps.push(SourceHeaderPipelineStep::Copy {
                input: String::new(),
                output: output.to_owned(),
                line: recipe.source_line + 1,
            });
        } else if text.trim_start_matches('@').starts_with("$(SED)") {
            if !allow_sed {
                return Err(
                    "SED is permitted only on the generated header before mirror copy".into(),
                );
            }
            if copy_line.is_none() {
                return Err("SED substitution precedes the generated source copy".into());
            }
            let substitutions = parse_sed_recipe(text)?;
            if substitutions.len()
                > MAX_REPLACEMENTS.saturating_sub(output_steps.len().saturating_sub(1))
            {
                return Err(format!("SED replacement count exceeds {MAX_REPLACEMENTS}"));
            }
            saw_sed = true;
            output_steps.extend(substitutions.into_iter().map(|(token, replacement)| {
                SourceHeaderPipelineStep::ReplaceWholeLineInPlace {
                    path: output.to_owned(),
                    token,
                    replacement,
                    line: recipe.source_line + 1,
                }
            }));
        } else {
            return Err(format!(
                "unsupported recipe command at line {}",
                recipe.source_line + 1
            ));
        }
    }
    if copy_line.is_none() {
        return Err("rule has no exact `$(CP) $< $@` recipe".into());
    }
    let context =
        MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
    let inputs = evaluate_make_list(&rule.prerequisites, &context)
        .map_err(|error| format!("cannot resolve copy input: {error}"))?;
    if inputs.len() != 1 {
        return Err(format!(
            "copy rule must have one prerequisite, got {}",
            inputs.len()
        ));
    }
    let SourceHeaderPipelineStep::Copy { input, .. } = &mut output_steps[0] else {
        unreachable!("the first operation was checked to be the copy")
    };
    input.clone_from(&inputs[0]);
    Ok(output_steps)
}

fn is_echo_recipe(text: &str) -> Result<bool, String> {
    let Some(command) = trim_recipe_prefix(text).strip_prefix("$(ECHO)") else {
        return Ok(false);
    };
    if !command.starts_with(char::is_whitespace) {
        return Err("ECHO command must separate its literal message".into());
    }
    let tokens = shell_tokens(command.trim()).ok_or_else(|| {
        "ECHO recipe has shell syntax outside the bounded literal form".to_owned()
    })?;
    if tokens.is_empty()
        || tokens.iter().any(|token| {
            token.value.is_empty()
                || token.value.contains(['`', '\\', '\n', '\r'])
                || !safe_echo_make_references(&token.value)
        })
    {
        return Err("ECHO recipe must contain only a bounded literal message".into());
    }
    Ok(true)
}

fn safe_echo_make_references(value: &str) -> bool {
    let mut rest = value;
    while let Some(dollar) = rest.find('$') {
        rest = &rest[dollar..];
        let Some(reference) = rest.strip_prefix("$(") else {
            return false;
        };
        let Some(close) = reference.find(')') else {
            return false;
        };
        if !matches!(&reference[..close], "GENINCDIR" | "AROS_INCLUDES") {
            return false;
        }
        rest = &reference[close + 1..];
    }
    true
}

fn is_exact_copy_recipe(text: &str) -> bool {
    let command = trim_recipe_prefix(text);
    command == "$(CP) $< $@"
}

fn parse_sed_recipe(text: &str) -> Result<Vec<(String, String)>, String> {
    let command = trim_recipe_prefix(text);
    let tokens = shell_tokens(command)
        .ok_or_else(|| "SED recipe has shell syntax outside the bounded literal form".to_owned())?;
    if tokens.len() < 5 || tokens[0].value != "$(SED)" || tokens[1].value != "-i" {
        return Err(
            "SED must use exact `$(SED) -i -e 's/.*TOKEN.*/REPLACEMENT/' ... $@` form".into(),
        );
    }
    if tokens.last().map(|token| token.value.as_str()) != Some("$@") {
        return Err("SED recipe must end exactly with `$@`".into());
    }
    let mut index = 2;
    let mut replacements = Vec::new();
    while index < tokens.len() - 1 {
        if tokens[index].value != "-e" || !tokens[index + 1].single_quoted {
            return Err("each SED expression must be a single-quoted literal `-e` argument".into());
        }
        replacements.push(parse_sed_expression(&tokens[index + 1].value)?);
        index += 2;
    }
    if replacements.is_empty() || replacements.len() > MAX_REPLACEMENTS {
        return Err(format!(
            "SED recipe must contain 1..={MAX_REPLACEMENTS} substitutions"
        ));
    }
    Ok(replacements)
}

fn parse_sed_expression(expression: &str) -> Result<(String, String), String> {
    let body = expression
        .strip_prefix("s/.*")
        .ok_or_else(|| "SED expression must start with `s/.*`".to_owned())?;
    let (token, replacement) = body.split_once(".*/").ok_or_else(|| {
        "SED expression must contain the whole-line `TOKEN.*/REPLACEMENT/` form".to_owned()
    })?;
    let replacement = replacement
        .strip_suffix('/')
        .ok_or_else(|| "SED expression must end after its literal replacement".to_owned())?;
    if !safe_sed_token(token) || !safe_sed_replacement(replacement) {
        return Err(
            "SED token/replacement contains unsupported regex or replacement syntax".into(),
        );
    }
    Ok((token.to_owned(), replacement.to_owned()))
}

#[derive(Debug)]
struct ShellToken {
    value: String,
    single_quoted: bool,
}

fn shell_tokens(input: &str) -> Option<Vec<ShellToken>> {
    let mut result = Vec::new();
    let mut value = String::new();
    let mut quote = None;
    let mut single_quoted = false;
    let mut started = false;
    for ch in input.chars() {
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else if ch == '\\' || ch == '\n' || ch == '\r' || ch == '`' {
                return None;
            } else {
                value.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                if started && value.is_empty() {
                    return None;
                }
                started = true;
                single_quoted = ch == '\'';
                quote = Some(ch);
            }
            ' ' | '\t' => {
                if started {
                    result.push(ShellToken {
                        value: std::mem::take(&mut value),
                        single_quoted,
                    });
                    started = false;
                    single_quoted = false;
                }
            }
            ';' | '<' | '>' | '|' | '&' | '\\' | '`' | '\n' | '\r' => return None,
            _ => {
                started = true;
                value.push(ch);
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if started {
        result.push(ShellToken {
            value,
            single_quoted,
        });
    }
    Some(result)
}

fn trim_recipe_prefix(text: &str) -> &str {
    text.strip_prefix('@').unwrap_or(text)
}

fn safe_sed_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 128
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | ';' | '-' | '+'))
}

fn safe_sed_replacement(replacement: &str) -> bool {
    !replacement.is_empty()
        && replacement.len() <= 512
        && replacement.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || ch.is_ascii_whitespace()
                || matches!(ch, '_' | ';' | '-' | '+' | '*' | ':' | '.')
        })
}

fn validate_output_rule(rule: &Rule) -> Result<(), String> {
    if rule.state != ConditionalTruth::True {
        return Err("output rule is guarded by an unresolved Make conditional".into());
    }
    if rule.double_colon {
        return Err("double-colon output rules are unsupported".into());
    }
    if rule.inline_recipe {
        return Err("inline output recipes are unsupported".into());
    }
    if rule.order_only.is_some() {
        return Err("order-only prerequisites are unsupported".into());
    }
    Ok(())
}

fn validate_rule_output(
    rule: &Rule,
    context: &MakeExprContext<'_>,
    configured_root: &str,
    expected_relative: &str,
) -> Result<(), String> {
    let targets = evaluate_make_list(&rule.target, context)
        .map_err(|error| format!("cannot resolve output target: {error}"))?;
    if targets.len() != 1 {
        return Err("output rule must have exactly one target".into());
    }
    let expected = format!(
        "{}/{}",
        configured_root.trim_end_matches('/'),
        expected_relative
    );
    if targets[0] != expected {
        return Err(format!(
            "output target `{}` differs from `{expected}`",
            targets[0]
        ));
    }
    Ok(())
}

fn matching_rules<'a>(
    rules: &'a [Rule],
    expected: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> Vec<&'a Rule> {
    rules
        .iter()
        .filter(|rule| rule.state != ConditionalTruth::False)
        .filter(|rule| {
            let context = MakeExprContext::new(scope, dirs, rule.source_line, root, relative_dir);
            evaluate_make_list(&rule.target, &context)
                .is_ok_and(|targets| targets.iter().any(|target| target == expected))
        })
        .collect()
}

/// Candidate discovery is narrower than recipe validation: an owner/header
/// relationship alone does not make this scanner responsible for arbitrary
/// generator routes. Require an explicit source-tree prerequisite copied by a
/// literal CP recipe to GENINCDIR, plus a CP recipe mirroring that output to
/// AROS_INCLUDES. Once that structural shape exists, `scan_candidate` validates
/// the full rule and rejects unknown states, extra commands, and malformed CP.
fn has_source_copy_pipeline(
    prerequisites: &str,
    line: usize,
    rules: &[Rule],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> bool {
    let context = MakeExprContext::new(scope, dirs, line, root, relative_dir);
    let (Ok(owner_prerequisites), Some(sdk_root), Some(generated_root)) = (
        evaluate_make_list(prerequisites, &context),
        dirs.expand("$(AROS_INCLUDES)"),
        dirs.expand("$(GENINCDIR)"),
    ) else {
        return false;
    };
    for sdk_output in owner_prerequisites {
        let Some(relative_header) =
            relative_under_root(&sdk_output, &sdk_root).filter(|relative| safe_header(relative))
        else {
            continue;
        };
        let generated_output = format!(
            "{}/{}",
            generated_root.trim_end_matches('/'),
            relative_header
        );
        let source_copy = matching_rules(rules, &generated_output, scope, dirs, root, relative_dir)
            .into_iter()
            .any(|rule| {
                if !has_cp_recipe(rule) {
                    return false;
                }
                let generated_context =
                    MakeExprContext::new(scope, dirs, rule.source_line, root, relative_dir);
                evaluate_make_list(&rule.prerequisites, &generated_context).is_ok_and(|inputs| {
                    inputs
                        .iter()
                        .any(|input| source_tree_relative(input).is_some())
                })
            });
        if !source_copy {
            continue;
        }
        let sdk_copy = matching_rules(rules, &sdk_output, scope, dirs, root, relative_dir)
            .into_iter()
            .any(|rule| {
                if !has_cp_recipe(rule) {
                    return false;
                }
                let mirror_context =
                    MakeExprContext::new(scope, dirs, rule.source_line, root, relative_dir);
                // Independent SOURCE -> SDK and SOURCE -> GEN copies are
                // handled by the static copier, not an ordered GEN -> SDK chain.
                // A wrong generated input remains a candidate and is refused by
                // the exact mirror proof, rather than silently ignored.
                evaluate_make_list(&rule.prerequisites, &mirror_context).map_or_else(
                    |_| {
                        rule.prerequisites.contains("$(GENINCDIR)")
                            || rule.prerequisites.contains("${AROS_GENINC_DIR}")
                    },
                    |inputs| {
                        inputs
                            .iter()
                            .any(|input| relative_under_root(input, &generated_root).is_some())
                    },
                )
            });
        if sdk_copy {
            return true;
        }
    }
    false
}

fn has_cp_recipe(rule: &Rule) -> bool {
    rule.recipes.iter().any(|recipe| {
        recipe.state != ConditionalTruth::False && is_cp_recipe_candidate(recipe.text.trim())
    })
}

fn is_cp_recipe_candidate(text: &str) -> bool {
    text.trim_start_matches(['@', '-', '+'])
        .starts_with("$(CP)")
}

fn source_tree_relative(path: &str) -> Option<&str> {
    let relative = path.strip_prefix("${AROS_SOURCE_DIR}/")?;
    safe_source_path(relative).then_some(relative)
}

fn evaluate_owner(
    raw: &str,
    line: usize,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
) -> Option<String> {
    let context = MakeExprContext::new(scope, dirs, line, root, relative_dir);
    let value = evaluate_make_expr(raw, &context).ok()?;
    safe_owner(&value).then_some(value)
}

fn raw_target_matches_owner(raw_target: &str, raw_owner: &str) -> bool {
    raw_target.trim() == raw_owner.trim()
}

fn marker_state(candidate: &Candidate, markers: &[OwnerMarker]) -> ConditionalTruth {
    markers
        .iter()
        .find(|marker| {
            (marker.source_line, &marker.raw_owner) == (candidate.marker_line, &candidate.raw_owner)
        })
        .map_or(ConditionalTruth::Unknown, |marker| marker.state)
}

fn validate_configured_root(
    expression: &str,
    context: &MakeExprContext<'_>,
    dirs: &DirVars,
) -> Result<String, String> {
    let active = evaluate_make_expr(expression, context)
        .map_err(|error| format!("cannot resolve header output root {expression}: {error}"))?;
    let configured = dirs
        .expand(expression)
        .ok_or_else(|| format!("cannot resolve configured header output root {expression}"))?;
    if active != configured || !safe_configured_root(&configured) {
        return Err(format!(
            "active header output root {expression} differs from its configured mapping"
        ));
    }
    Ok(configured)
}

fn safe_configured_root(root: &str) -> bool {
    let Some(relative) = root.strip_prefix("${AROS_BUILD_DIR}/") else {
        return false;
    };
    !relative.is_empty()
        && !relative.contains(['\\', ';', '$', '`', '\n', '\r'])
        && !relative.contains("//")
        && safe_components(relative)
}

fn validate_regular_source_input(input: &str, source_root: &Path) -> Result<(), String> {
    let relative = source_tree_relative(input).ok_or_else(|| {
        "generated-header input is not a safe path below the selected source tree".to_owned()
    })?;
    if !safe_source_path(relative) {
        return Err("generated-header source prerequisite is not a safe relative path".into());
    }
    let mut path = source_root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return Err(
                "generated-header source prerequisite contains a non-normal path component".into(),
            );
        };
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "cannot inspect source header prerequisite `{}`: {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "source header prerequisite crosses a symlink: {}",
                path.display()
            ));
        }
    }
    let metadata = fs::metadata(&path).map_err(|error| {
        format!(
            "cannot inspect source header prerequisite `{}`: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "source header prerequisite is not a regular file: {}",
            path.display()
        ));
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot canonicalize source header prerequisite: {error}"))?;
    if !canonical.starts_with(source_root) {
        return Err("source header prerequisite escapes the selected source root".into());
    }
    Ok(())
}

fn source_rebinds_recipe_or_include_roots(scope: &VarScope) -> bool {
    [
        "CP",
        "ECHO",
        "SED",
        "SHELL",
        "RECIPEPREFIX",
        "AROS_INCLUDES",
        "AROS_DEVELOPER",
        "AROS_DIR_INCLUDE",
        "GENDIR",
        "GENINCDIR",
        "SRCDIR",
        "CURDIR",
    ]
    .iter()
    .any(|name| scope.is_known_local(name) || scope.conditionally_assigned_before(name, usize::MAX))
}

const fn target_specific_control(rule: &Rule) -> bool {
    rule.target_specific_control
}

fn output_parent(output: &str) -> Result<String, String> {
    output
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_owned())
        .ok_or_else(|| "output header has no parent directory".into())
}

fn relative_under_root<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    path.strip_prefix(root)
        .and_then(|tail| tail.strip_prefix('/'))
}

fn safe_header(value: &str) -> bool {
    safe_relative_path(value, Some("h"))
}

fn safe_source_path(value: &str) -> bool {
    safe_relative_path(value, None)
}

fn safe_relative_path(value: &str, extension: Option<&str>) -> bool {
    if value.is_empty()
        || value.len() > MAX_PATH_BYTES
        || value.starts_with('/')
        || value.contains([
            '\\', ';', '$', '`', '*', '?', '[', ']', '#', '\n', '\r', '\t',
        ])
        || Path::new(value).is_absolute()
        || extension.is_some_and(|extension| {
            Path::new(value).extension().and_then(|part| part.to_str()) != Some(extension)
        })
    {
        return false;
    }
    let mut count = 0usize;
    for component in Path::new(value).components() {
        let Component::Normal(component) = component else {
            return false;
        };
        let Some(component) = component.to_str() else {
            return false;
        };
        if component.is_empty()
            || !component
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-'))
        {
            return false;
        }
        count += 1;
    }
    count > 0
}

fn safe_relative_directory(path: &Path) -> Result<String, String> {
    if path.is_absolute() {
        return Err("declaring directory must be source-root-relative".into());
    }
    let mut components = Vec::new();
    for component in path.components() {
        let Component::Normal(component) = component else {
            return Err("declaring directory contains a non-normal path component".into());
        };
        let component = component
            .to_str()
            .ok_or_else(|| "declaring directory is not valid UTF-8".to_owned())?;
        if component.contains(['\\', ';', '$', '`', '\n', '\r']) {
            return Err("declaring directory contains an unsafe path component".into());
        }
        components.push(component.to_owned());
    }
    Ok(components.join("/"))
}

fn safe_owner(owner: &str) -> bool {
    let mut chars = owner.chars();
    chars.next().is_some_and(|first| {
        (first.is_ascii_alphanumeric() || first == '_')
            && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-'))
    })
}

fn safe_components(value: &str) -> bool {
    Path::new(value).components().all(|component| {
        let Component::Normal(part) = component else {
            return false;
        };
        part.to_str().is_some_and(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-'))
        })
    })
}

fn reject_collisions(results: &mut [CandidateResult]) {
    let mut outputs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut inputs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, result) in results.iter().enumerate() {
        for output in &result.claimed_outputs {
            outputs
                .entry(output.to_ascii_lowercase())
                .or_default()
                .push(index);
        }
        for input in &result.claimed_inputs {
            inputs
                .entry(input.to_ascii_lowercase())
                .or_default()
                .push(index);
        }
    }
    for (path, owners) in outputs.into_iter().chain(inputs) {
        let unique = owners.into_iter().collect::<HashSet<_>>();
        if unique.len() < 2 {
            continue;
        }
        for index in unique {
            if results[index].rejection.is_none() {
                results[index].rejection = Some(format!(
                    "header pipeline path `{path}` is claimed more than once"
                ));
            }
            results[index].declarations.clear();
        }
    }
}

fn rejected_candidates(
    candidates: &[Candidate],
    reason: &str,
) -> (Vec<SourceHeaderPipelineDecl>, Vec<Rejection>) {
    (
        Vec::new(),
        candidates
            .iter()
            .map(|candidate| Rejection {
                owner: candidate.owner.clone(),
                line: candidate.owner_line + 1,
                reason: reason.to_owned(),
            })
            .collect(),
    )
}

fn logical_lines(content: &str, states: Option<&[ConditionalTruth]>) -> Vec<LogicalLine> {
    let physical = content.lines().collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut at = 0usize;
    while at < physical.len() {
        let first = physical[at];
        let first_trimmed = first.trim();
        let opens = is_conditional_open(first_trimmed);
        let mut text = first.to_owned();
        let start = at;
        let mut state = line_state(states, at, depth > 0);
        while has_unescaped_trailing_backslash(&text) && at + 1 < physical.len() {
            let trimmed = text.trim_end();
            text.truncate(trimmed.len() - 1);
            at += 1;
            state = combine_state(state, line_state(states, at, depth > 0));
            text.push(' ');
            text.push_str(physical[at].trim());
        }
        result.push(LogicalLine {
            text,
            source_line: start,
            state,
        });
        if opens {
            depth += 1;
        } else if is_conditional_close(first_trimmed) {
            depth = depth.saturating_sub(1);
        }
        at += 1;
    }
    result
}

fn parse_rules(lines: &[LogicalLine]) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut current: Option<Rule> = None;
    for line in lines {
        if line.text.starts_with('\t') {
            if let Some(rule) = current.as_mut() {
                if line.state != ConditionalTruth::False {
                    rule.recipes.push(Recipe {
                        text: line.text.trim().to_owned(),
                        source_line: line.source_line,
                        state: line.state,
                    });
                }
            }
            continue;
        }
        let statement = strip_make_comment(line.text.trim_end_matches('\r')).trim();
        if statement.is_empty() || statement.starts_with('#') || is_conditional_directive(statement)
        {
            continue;
        }
        if variable_assignment(statement).is_some() {
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
            continue;
        }
        if let Some(rule) = current.take() {
            rules.push(rule);
        }
        let Some((target, tail, double_colon)) = split_rule(statement) else {
            continue;
        };
        let (prerequisites_text, inline_recipe) = tail.split_once(';').map_or_else(
            || (tail.trim(), false),
            |(prerequisites, _)| (prerequisites.trim(), true),
        );
        let target_specific_control =
            variable_assignment(prerequisites_text).is_some_and(|(name, _, _)| {
                matches!(
                    name,
                    "CP" | "ECHO"
                        | "SED"
                        | "SHELL"
                        | "RECIPEPREFIX"
                        | "AROS_INCLUDES"
                        | "GENINCDIR"
                        | "GENDIR"
                        | "SRCDIR"
                        | "CURDIR"
                )
            });
        let (prerequisites, order_only) = match prerequisites_text.split_once('|') {
            Some((ordinary, order_only)) => (
                ordinary.trim().to_owned(),
                Some(order_only.trim().to_owned()),
            ),
            None => (prerequisites_text.to_owned(), None),
        };
        if target.trim().is_empty() || target.contains('=') {
            continue;
        }
        current = Some(Rule {
            target: target.trim().to_owned(),
            prerequisites,
            order_only,
            source_line: line.source_line,
            state: line.state,
            inline_recipe,
            double_colon,
            target_specific_control,
            recipes: Vec::new(),
        });
    }
    if let Some(rule) = current {
        rules.push(rule);
    }
    rules
}

fn parse_owner_markers(lines: &[LogicalLine], rules: &[Rule]) -> Vec<OwnerMarker> {
    let mut markers = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let statement = line.text.trim();
        let Some(body) = statement.strip_prefix("#MM") else {
            continue;
        };
        if body.starts_with('-') {
            continue;
        }
        let body = body.trim();
        if body.is_empty() {
            let next = lines[index + 1..].iter().find(|candidate| {
                let statement = candidate.text.trim();
                !statement.is_empty()
                    && !statement.starts_with('#')
                    && !is_conditional_directive(statement)
            });
            if let Some(next) = next {
                if let Some(rule) = rules
                    .iter()
                    .find(|rule| rule.source_line == next.source_line)
                {
                    let owner = rule
                        .target
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    markers.push(OwnerMarker {
                        raw_owner: owner,
                        raw_prerequisites: None,
                        owner_rule_line: Some(rule.source_line),
                        owner_line: rule.source_line,
                        source_line: line.source_line,
                        state: combine_state(line.state, rule.state),
                    });
                }
            }
            continue;
        }
        let Some((raw_owner, raw_prerequisites)) = body.split_once(':') else {
            markers.push(OwnerMarker {
                raw_owner: body.trim().to_owned(),
                raw_prerequisites: None,
                owner_rule_line: rules
                    .iter()
                    .find(|rule| raw_target_matches_owner(&rule.target, body.trim()))
                    .map(|rule| rule.source_line),
                owner_line: rules
                    .iter()
                    .find(|rule| raw_target_matches_owner(&rule.target, body.trim()))
                    .map_or(line.source_line, |rule| rule.source_line),
                source_line: line.source_line,
                state: line.state,
            });
            continue;
        };
        markers.push(OwnerMarker {
            raw_owner: raw_owner.trim().to_owned(),
            raw_prerequisites: Some(strip_make_comment(raw_prerequisites).trim().to_owned()),
            owner_rule_line: None,
            owner_line: line.source_line,
            source_line: line.source_line,
            state: line.state,
        });
    }
    markers
}

fn split_rule(statement: &str) -> Option<(&str, &str, bool)> {
    if statement.starts_with('%') || statement.contains(';') && statement.starts_with('#') {
        return None;
    }
    let colon = statement.find(':')?;
    let double_colon = statement.as_bytes().get(colon + 1) == Some(&b':');
    let colon_width = if double_colon { 2 } else { 1 };
    Some((
        &statement[..colon],
        &statement[colon + colon_width..],
        double_colon,
    ))
}

fn line_state(states: Option<&[ConditionalTruth]>, index: usize, nested: bool) -> ConditionalTruth {
    states.map_or_else(
        || {
            if nested {
                ConditionalTruth::Unknown
            } else {
                ConditionalTruth::True
            }
        },
        |states| {
            states
                .get(index)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
        },
    )
}

const fn combine_state(left: ConditionalTruth, right: ConditionalTruth) -> ConditionalTruth {
    match (left, right) {
        (ConditionalTruth::False, _) | (_, ConditionalTruth::False) => ConditionalTruth::False,
        (ConditionalTruth::Unknown, _) | (_, ConditionalTruth::Unknown) => {
            ConditionalTruth::Unknown
        }
        _ => ConditionalTruth::True,
    }
}

fn is_conditional_directive(line: &str) -> bool {
    is_conditional_open(line) || line == "else" || line == "endif"
}

fn is_conditional_open(line: &str) -> bool {
    ["ifeq", "ifneq", "ifdef", "ifndef"]
        .iter()
        .any(|directive| line == *directive || line.starts_with(&format!("{directive} ")))
}

fn is_conditional_close(line: &str) -> bool {
    line == "endif"
}

fn has_unescaped_trailing_backslash(value: &str) -> bool {
    value
        .trim_end()
        .chars()
        .rev()
        .take_while(|ch| *ch == '\\')
        .count()
        % 2
        == 1
}

#[cfg(test)]
#[path = "source_header_pipeline_tests.rs"]
mod tests;
