//! Closed scanner for narrowly defined Make rules which copy source headers
//! verbatim into configured include roots.
//!
//! It models finite named owners, recognized include-root static patterns,
//! and two literal ordinary-rule forms. It does not interpret arbitrary shell
//! recipes or infer output aliases.

use crate::copy_includes::HeaderTransformDecl;
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{strip_make_comment, variable_assignment, ConditionalTruth, VarScope};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path};

const MAX_OUTPUTS: usize = 4096;
const MAX_MAKEFILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_HEADER_RELATIVE_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub owner: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct LogicalLine {
    text: String,
    source_line: usize,
    state: ConditionalTruth,
    conditional_syntax: bool,
}

#[derive(Debug, Clone)]
struct Recipe {
    text: String,
    state: ConditionalTruth,
    conditional_syntax: bool,
}

#[derive(Debug, Clone)]
struct StaticPattern {
    targets: String,
    pattern: String,
}

#[derive(Debug, Clone)]
struct Rule {
    target: String,
    prerequisites: String,
    source_line: usize,
    state: ConditionalTruth,
    conditional_syntax: bool,
    static_pattern: Option<StaticPattern>,
    recipes: Vec<Recipe>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrdinaryCandidateKind {
    Explicit,
    Implicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncludeRoot {
    Sdk,
    Generated,
}

#[derive(Debug, Clone)]
struct CandidateResult {
    owner: Option<String>,
    outputs: Vec<String>,
    declarations: Vec<HeaderTransformDecl>,
    rejection: Option<String>,
}

#[derive(Clone, Copy)]
struct CandidateScanContext<'a> {
    rules: &'a [Rule],
    scope: &'a VarScope,
    dirs: &'a DirVars,
    source_root: &'a Path,
    relative_dir: &'a Path,
    relative_directory: &'a str,
    expression_root: &'a Path,
}

/// Finds finite source-local header copies with a complete, named Make owner.
///
/// `line_states` uses zero-based line positions in the continuation-joined
/// source. A conditional candidate without a proven line state is rejected;
/// false branches are ignored.
#[must_use]
pub fn collect(
    content: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    relative_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<HeaderTransformDecl>, Vec<Rejection>) {
    if content.len() > MAX_MAKEFILE_BYTES {
        return (
            Vec::new(),
            vec![Rejection {
                owner: None,
                reason: "static header-copy Makefile exceeds the 4 MiB scan limit".into(),
            }],
        );
    }

    let lines = logical_lines(content, line_states);
    let rules = parse_rules(&lines);
    let mut candidates = Vec::new();
    let mut ordinary_candidates = Vec::new();
    for rule in &rules {
        if rule.static_pattern.is_none() {
            if rule.state != ConditionalTruth::False && ordinary_candidate_kind(rule).is_some() {
                ordinary_candidates.push(rule);
            }
            continue;
        }
        let Some(pattern) = &rule.static_pattern else {
            continue;
        };
        if rule.state == ConditionalTruth::False {
            continue;
        }
        if is_include_root_expression(&pattern.pattern) {
            candidates.push(rule);
            continue;
        }
        let expression_context =
            MakeExprContext::new(scope, dirs, rule.source_line, root, relative_dir);
        if expression_targets_known_include(&pattern.targets, &expression_context)
            || source_local_header_candidate(rule, pattern, &expression_context)
        {
            candidates.push(rule);
        }
    }
    if candidates.is_empty() && ordinary_candidates.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let relative_directory = match safe_relative_directory(relative_dir) {
        Ok(directory) => directory,
        Err(reason) => {
            return (
                Vec::new(),
                candidates
                    .iter()
                    .map(|rule| Rejection {
                        owner: owner_hint(rule, &rules),
                        reason: format!("static header-copy source directory: {reason}"),
                    })
                    .chain(ordinary_candidates.iter().map(|rule| Rejection {
                        owner: ordinary_owner_hint(rule, &rules),
                        reason: format!("header-copy source directory: {reason}"),
                    }))
                    .collect(),
            );
        }
    };
    let source_root = match root.canonicalize() {
        Ok(path) if path.is_dir() => path,
        Ok(_) => {
            return rejected_all_candidates(
                &candidates,
                &ordinary_candidates,
                &rules,
                "selected source root is not a directory",
            );
        }
        Err(error) => {
            let reason = format!("selected source root cannot be canonicalized: {error}");
            return rejected_all_candidates(&candidates, &ordinary_candidates, &rules, &reason);
        }
    };

    let scan_context = CandidateScanContext {
        rules: &rules,
        scope,
        dirs,
        source_root: &source_root,
        relative_dir,
        relative_directory: &relative_directory,
        expression_root: root,
    };
    let mut results = Vec::new();
    for rule in candidates {
        results.push(scan_candidate(rule, &scan_context));
    }
    results.extend(scan_ordinary_candidates(
        &ordinary_candidates,
        &scan_context,
    ));

    let mut output_owners: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, result) in results.iter().enumerate() {
        for output in &result.outputs {
            output_owners
                .entry(output.to_ascii_lowercase())
                .or_default()
                .push(index);
        }
    }
    for (output, owners) in output_owners {
        let unique = owners.iter().copied().collect::<HashSet<_>>();
        if unique.len() < 2 {
            continue;
        }
        for index in unique {
            let reason = format!("duplicate static header-copy output `{output}`");
            let result = &mut results[index];
            if result.rejection.is_none() {
                result.rejection = Some(reason);
            }
            result.declarations.clear();
        }
    }

    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    for mut result in results {
        if let Some(reason) = result.rejection.take() {
            rejections.push(Rejection {
                owner: result.owner,
                reason,
            });
        } else {
            declarations.extend(result.declarations);
        }
    }
    (declarations, rejections)
}

fn scan_candidate(rule: &Rule, scan: &CandidateScanContext<'_>) -> CandidateResult {
    let CandidateScanContext {
        rules,
        scope,
        dirs,
        source_root,
        relative_dir,
        relative_directory,
        expression_root,
    } = *scan;
    let Some(static_pattern) = &rule.static_pattern else {
        return rejected(None, "candidate is not a static-pattern rule");
    };
    let hint = owner_hint(rule, rules);
    let expression_context =
        MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
    if simple_variable_reference(&static_pattern.targets).is_none() {
        return rejected(
            hint,
            "static header-copy target list must be one simple Make variable reference",
        );
    }
    let outputs = match evaluate_make_list(&static_pattern.targets, &expression_context) {
        Ok(outputs) => outputs,
        Err(error) => {
            return rejected(
                hint,
                format!("cannot resolve static header-copy target list: {error}"),
            );
        }
    };
    if outputs.is_empty() {
        return rejected(hint, "static header-copy target list is empty");
    }
    if outputs.len() > MAX_OUTPUTS {
        return rejected(
            hint,
            format!("static header-copy target list exceeds {MAX_OUTPUTS} files"),
        );
    }

    if source_shadows_copy_command(scope, rules) {
        return rejected_with_outputs(
            hint,
            "source reassigns or target-specifies `CP`; the static copy command is not proven",
            outputs,
        );
    }

    if rule.state != ConditionalTruth::True
        || rule.conditional_syntax
        || rule
            .recipes
            .iter()
            .any(|recipe| recipe.state != ConditionalTruth::True || recipe.conditional_syntax)
    {
        return rejected_with_outputs(
            hint,
            "static header-copy rule or recipe is guarded by an unresolved Make conditional",
            outputs,
        );
    }

    let (_include_root, output_root, cmake_root) =
        match static_include_root(&static_pattern.pattern, &expression_context, dirs) {
            Ok(value) => value,
            Err(reason) => return rejected_with_outputs(hint, reason, outputs),
        };
    let source_prefix = match source_pattern_prefix(&rule.prerequisites, &expression_context) {
        Ok(prefix) => prefix,
        Err(reason) => return rejected_with_outputs(hint, reason, outputs),
    };
    let expected_source_prefix = if relative_directory.is_empty() {
        "${AROS_SOURCE_DIR}".to_owned()
    } else {
        format!("${{AROS_SOURCE_DIR}}/{relative_directory}")
    };
    let normalized_source_prefix = if relative_directory.is_empty()
        && matches!(
            source_prefix.as_str(),
            "${AROS_SOURCE_DIR}/" | "${AROS_SOURCE_DIR}/."
        ) {
        "${AROS_SOURCE_DIR}"
    } else {
        &source_prefix
    };
    if normalized_source_prefix != expected_source_prefix {
        return rejected_with_outputs(
            hint,
            format!(
                "static header-copy source prefix `{source_prefix}` is not the declaring source directory"
            ),
            outputs,
        );
    }

    let mut headers = Vec::with_capacity(outputs.len());
    let mut output_names = HashSet::new();
    for output in &outputs {
        let Some(tail) = output
            .strip_prefix(&output_root)
            .and_then(|tail| tail.strip_prefix('/'))
        else {
            return rejected_with_outputs(
                hint,
                format!("static header-copy output `{output}` is outside its include root"),
                outputs.clone(),
            );
        };
        if !safe_header_relative(tail) {
            return rejected_with_outputs(
                hint,
                format!("static header-copy output path `{tail}` is not a safe relative .h path"),
                outputs.clone(),
            );
        }
        let identity = tail.to_ascii_lowercase();
        if !output_names.insert(identity) {
            return rejected_with_outputs(
                hint,
                format!("static header-copy output `{tail}` is duplicated"),
                outputs.clone(),
            );
        }
        headers.push(tail.to_owned());
    }

    let owner = match find_owner(
        rule,
        rules,
        scope,
        dirs,
        expression_root,
        relative_dir,
        &outputs,
    ) {
        Ok(owner) => owner,
        Err((owner, reason)) => {
            return rejected_with_outputs(owner.or(hint), reason, outputs);
        }
    };

    let mut declarations = Vec::with_capacity(headers.len());
    for header in headers {
        if let Err(reason) = validate_regular_source(source_root, relative_dir, &header) {
            return CandidateResult {
                owner: Some(owner),
                outputs,
                declarations: Vec::new(),
                rejection: Some(reason),
            };
        }
        let input = if relative_directory.is_empty() {
            format!("${{AROS_SOURCE_DIR}}/{header}")
        } else {
            format!("${{AROS_SOURCE_DIR}}/{relative_directory}/{header}")
        };
        declarations.push(HeaderTransformDecl {
            name: owner.clone(),
            file: if relative_directory.is_empty() {
                "mmakefile.src".to_owned()
            } else {
                format!("{relative_directory}/mmakefile.src")
            },
            line: rule.source_line + 1,
            input,
            output: format!("{cmake_root}/{header}"),
            match_text: String::new(),
            replacement: String::new(),
            copy_only: true,
            replace_whole_line_containing: false,
            substitutions: Vec::new(),
            dependencies: Vec::new(),
            consumers: Vec::new(),
            generated_input_owner: None,
        });
    }

    CandidateResult {
        owner: Some(owner),
        outputs,
        declarations,
        rejection: None,
    }
}

fn ordinary_candidate_kind(rule: &Rule) -> Option<OrdinaryCandidateKind> {
    let target = rule.target.trim();
    let include_target = ["$(AROS_INCLUDES)/", "$(GENINCDIR)/"]
        .iter()
        .any(|prefix| target.starts_with(prefix));
    if !include_target || !has_exact_h_extension(target) {
        return None;
    }
    if target.contains('%') {
        (implicit_source_header_shape(rule.prerequisites.trim()) && has_cp_source_recipe(rule))
            .then_some(OrdinaryCandidateKind::Implicit)
    } else {
        (literal_source_header_shape(rule.prerequisites.trim()) && has_cp_source_recipe(rule))
            .then_some(OrdinaryCandidateKind::Explicit)
    }
}

fn literal_source_header_shape(prerequisites: &str) -> bool {
    let ordinary = prerequisites.split('|').next().unwrap_or_default();
    let Some(source) = ordinary.split_whitespace().next() else {
        return false;
    };
    !source.contains('$') && !Path::new(source).is_absolute() && has_exact_h_extension(source)
}

fn has_exact_h_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension == "h")
}

fn implicit_source_header_shape(prerequisites: &str) -> bool {
    let ordinary = prerequisites.split('|').next().unwrap_or_default();
    ordinary.split_whitespace().next() == Some("%.h")
}

fn has_cp_source_recipe(rule: &Rule) -> bool {
    rule.recipes.iter().any(|recipe| {
        let words = recipe.text.split_whitespace().collect::<Vec<_>>();
        words.len() >= 3 && matches!(words[0], "$(CP)" | "@$(CP)") && words[1] == "$<"
    })
}

fn source_shadows_copy_command(scope: &VarScope, rules: &[Rule]) -> bool {
    scope.is_known_local("CP")
        || scope.conditionally_assigned_before("CP", usize::MAX)
        || rules
            .iter()
            .filter(|rule| rule.state != ConditionalTruth::False)
            .any(target_specific_cp_assignment)
}

fn target_specific_cp_assignment(rule: &Rule) -> bool {
    target_specific_assignment_name(&rule.prerequisites) == Some("CP")
}

fn target_specific_assignment_name(value: &str) -> Option<&str> {
    let mut assignment = value.trim();
    for _ in 0..3 {
        let mut words = assignment.splitn(2, char::is_whitespace);
        let modifier = words.next().unwrap_or_default();
        if !matches!(modifier, "override" | "export" | "private") {
            break;
        }
        assignment = words.next().unwrap_or_default().trim();
    }
    variable_assignment(assignment).map(|(name, _, _)| name)
}

fn scan_ordinary_candidates(
    candidates: &[&Rule],
    scan: &CandidateScanContext<'_>,
) -> Vec<CandidateResult> {
    candidates
        .iter()
        .map(|rule| match ordinary_candidate_kind(rule) {
            Some(OrdinaryCandidateKind::Explicit) => scan_explicit_candidate(rule, scan),
            Some(OrdinaryCandidateKind::Implicit) => scan_implicit_candidate(rule, scan),
            None => rejected(
                None,
                "candidate is not a supported ordinary header-copy rule",
            ),
        })
        .collect()
}

fn scan_explicit_candidate(rule: &Rule, scan: &CandidateScanContext<'_>) -> CandidateResult {
    let CandidateScanContext {
        rules,
        scope,
        dirs,
        source_root,
        relative_dir,
        relative_directory,
        expression_root,
    } = *scan;
    let context =
        MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
    let candidate_value = evaluate_make_expr(rule.target.trim(), &context).ok();
    let outputs = candidate_value.iter().cloned().collect::<Vec<_>>();
    let hint = ordinary_owner_hint(rule, rules);
    let (output, _include_root, _output_root, _cmake_root, _header) =
        match explicit_target(rule, &context, dirs) {
            Ok(value) => value,
            Err(reason) => return rejected_with_outputs(hint, reason, outputs),
        };
    let output_vec = vec![output.clone()];
    if source_shadows_copy_command(scope, rules) {
        return rejected_with_outputs(
            hint,
            "source reassigns or target-specifies `CP`; the explicit copy command is not proven",
            output_vec,
        );
    }
    let owners = rules
        .iter()
        .filter(|candidate| {
            named_owner_mentions(
                candidate,
                &output,
                scope,
                dirs,
                expression_root,
                relative_dir,
            )
        })
        .collect::<Vec<_>>();
    if owners.len() != 1 {
        let reason = if owners.is_empty() {
            "orphan explicit header-copy rule has no named owner"
        } else {
            "explicit header-copy output has multiple named owners"
        };
        return rejected_with_outputs(hint, reason, output_vec);
    }
    let owner_rule = owners[0];
    let owner = owner_rule.target.trim().to_owned();
    let same_name_count = rules
        .iter()
        .filter(|candidate| {
            candidate.state != ConditionalTruth::False && candidate.target.trim() == owner
        })
        .count();
    if same_name_count != 1 {
        return rejected_with_outputs(
            Some(owner),
            "explicit header-copy owner has multiple active Make rule blocks",
            output_vec,
        );
    }
    if !ordinary_rule_is_unconditional(owner_rule)
        || !owner_rule.recipes.is_empty()
        || owner_rule.prerequisites.contains('|')
    {
        return rejected_with_outputs(
            Some(owner),
            "explicit header-copy owner must be unconditional, dependency-only, and have no order-only prerequisites",
            output_vec,
        );
    }
    let owner_context = MakeExprContext::new(
        scope,
        dirs,
        owner_rule.source_line,
        expression_root,
        relative_dir,
    );
    let owner_outputs = match evaluate_make_list(&owner_rule.prerequisites, &owner_context) {
        Ok(outputs) if !outputs.is_empty() => outputs,
        Ok(_) => {
            return rejected_with_outputs(
                Some(owner),
                "explicit header-copy owner output list is empty",
                output_vec,
            );
        }
        Err(error) => {
            return rejected_with_outputs(
                Some(owner),
                format!("cannot resolve explicit header-copy owner outputs: {error}"),
                output_vec,
            );
        }
    };
    if owner_outputs.len() > MAX_OUTPUTS {
        return rejected_with_outputs(
            Some(owner),
            format!("explicit header-copy owner output list exceeds {MAX_OUTPUTS} files"),
            output_vec,
        );
    }
    if let Err(reason) = validate_literal_owner_outputs(
        &owner_rule.prerequisites,
        &owner_outputs,
        &owner_context,
        dirs,
        false,
    ) {
        return rejected_with_outputs(Some(owner), reason, output_vec);
    }
    let mut output_names = HashSet::new();
    for owner_output in &owner_outputs {
        if !output_names.insert(owner_output.to_ascii_lowercase()) {
            return rejected_with_outputs(
                Some(owner),
                format!("explicit header-copy owner output `{owner_output}` is duplicated"),
                output_vec,
            );
        }
    }
    let aliases = rules
        .iter()
        .filter(|candidate| {
            owner_outputs.iter().any(|owner_output| {
                named_owner_mentions(
                    candidate,
                    owner_output,
                    scope,
                    dirs,
                    expression_root,
                    relative_dir,
                )
            })
        })
        .collect::<Vec<_>>();
    if aliases.len() != 1 || aliases[0].target.trim() != owner {
        return rejected_with_outputs(
            Some(owner),
            "explicit header-copy output list has an ambiguous named owner",
            output_vec,
        );
    }
    let mut selected_decl = None;
    for owner_output in &owner_outputs {
        let producers = rules
            .iter()
            .filter(|candidate| {
                candidate.state != ConditionalTruth::False
                    && evaluate_make_expr(
                        candidate.target.trim(),
                        &MakeExprContext::new(
                            scope,
                            dirs,
                            candidate.source_line,
                            expression_root,
                            relative_dir,
                        ),
                    )
                    .is_ok_and(|target| target == *owner_output)
            })
            .collect::<Vec<_>>();
        if producers.len() != 1 {
            return rejected_with_outputs(
                Some(owner),
                format!(
                    "explicit header-copy output `{owner_output}` has {} active target rules",
                    producers.len()
                ),
                output_vec,
            );
        }
        let producer = producers[0];
        let producer_context = MakeExprContext::new(
            scope,
            dirs,
            producer.source_line,
            expression_root,
            relative_dir,
        );
        let Ok((resolved_output, include_root, root_path, cmake_path, output_header)) =
            explicit_target(producer, &producer_context, dirs)
        else {
            return rejected_with_outputs(
                Some(owner),
                format!(
                    "explicit header-copy output `{owner_output}` has an unsupported target rule"
                ),
                output_vec,
            );
        };
        if resolved_output != *owner_output {
            return rejected_with_outputs(
                Some(owner),
                format!(
                    "explicit header-copy target does not resolve to owner output `{owner_output}`"
                ),
                output_vec,
            );
        }
        if !ordinary_rule_is_unconditional(producer) || producer.prerequisites.contains('|') {
            return rejected_with_outputs(
                Some(owner),
                "explicit header-copy rule or recipe is guarded, conditional, or has order-only prerequisites",
                output_vec,
            );
        }
        let source_relative = producer.prerequisites.trim();
        if source_relative.split_whitespace().count() != 1
            || source_relative.contains('$')
            || !safe_header_relative(source_relative)
        {
            return rejected_with_outputs(
                Some(owner),
                format!(
                    "explicit header-copy source `{source_relative}` is not one safe source-relative header"
                ),
                output_vec,
            );
        }
        if producer.recipes.len() != 1 {
            return rejected_with_outputs(
                Some(owner),
                "explicit header-copy rule must have exactly one copy recipe command",
                output_vec,
            );
        }
        if !explicit_copy_recipe(
            &producer.recipes[0].text,
            &producer.target,
            &producer_context,
            &resolved_output,
        ) {
            return rejected_with_outputs(
                Some(owner),
                "explicit header-copy recipe must be exactly `$(CP) $< <same target>`",
                output_vec,
            );
        }
        if let Err(reason) = validate_regular_source(source_root, relative_dir, source_relative) {
            return rejected_with_outputs(Some(owner), reason, output_vec);
        }
        if resolved_output == output {
            let input = if relative_directory.is_empty() {
                format!("${{AROS_SOURCE_DIR}}/{source_relative}")
            } else {
                format!("${{AROS_SOURCE_DIR}}/{relative_directory}/{source_relative}")
            };
            selected_decl = Some(HeaderTransformDecl {
                name: owner.clone(),
                file: if relative_directory.is_empty() {
                    "mmakefile.src".to_owned()
                } else {
                    format!("{relative_directory}/mmakefile.src")
                },
                line: producer.source_line + 1,
                input,
                output: format!("{cmake_path}/{output_header}"),
                match_text: String::new(),
                replacement: String::new(),
                copy_only: true,
                replace_whole_line_containing: false,
                substitutions: Vec::new(),
                dependencies: Vec::new(),
                consumers: Vec::new(),
                generated_input_owner: None,
            });
        }
        let _ = (include_root, root_path);
    }
    match selected_decl {
        Some(declaration) => CandidateResult {
            owner: Some(owner),
            outputs: output_vec,
            declarations: vec![declaration],
            rejection: None,
        },
        None => rejected_with_outputs(
            Some(owner),
            "explicit header-copy candidate is absent from its named owner outputs",
            output_vec,
        ),
    }
}

fn scan_implicit_candidate(rule: &Rule, scan: &CandidateScanContext<'_>) -> CandidateResult {
    let CandidateScanContext {
        rules,
        scope,
        dirs,
        source_root,
        relative_dir,
        relative_directory,
        expression_root,
    } = *scan;
    let hint = ordinary_owner_hint(rule, rules);
    let context =
        MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
    if source_shadows_copy_command(scope, rules) {
        return rejected(
            hint,
            "source reassigns or target-specifies `CP`; the implicit copy command is not proven",
        );
    }
    if rule.target.trim() != "$(AROS_INCLUDES)/%.h" || rule.prerequisites.trim() != "%.h" {
        return rejected_with_outputs(
            hint,
            "ordinary implicit header-copy rule must be exactly `$(AROS_INCLUDES)/%.h : %.h`",
            Vec::new(),
        );
    }
    let (_, output_root, cmake_root) =
        match static_include_root("$(AROS_INCLUDES)/%", &context, dirs) {
            Ok(value) => value,
            Err(reason) => return rejected(hint, reason),
        };
    let duplicate_patterns = rules
        .iter()
        .filter(|candidate| {
            candidate.state != ConditionalTruth::False
                && candidate.target.trim() == rule.target.trim()
        })
        .count();
    if duplicate_patterns != 1 {
        return rejected(
            hint,
            "ordinary implicit header-copy pattern has duplicate target rule blocks",
        );
    }
    if !ordinary_rule_is_unconditional(rule) || rule.prerequisites.contains('|') {
        return rejected(
            hint,
            "ordinary implicit header-copy rule or recipe is guarded, conditional, or has order-only prerequisites",
        );
    }
    if rule.recipes.len() != 1
        || !implicit_copy_recipe(&rule.recipes[0].text, &context, &output_root)
    {
        return rejected(
            hint,
            "ordinary implicit header-copy recipe must be exactly `$(CP) $< $(AROS_INCLUDES)`",
        );
    }
    let owner_rules = rules
        .iter()
        .filter(|candidate| {
            candidate.state != ConditionalTruth::False
                && safe_owner_name(candidate.target.trim())
                && owner_mentions_include_root(
                    candidate,
                    scope,
                    dirs,
                    expression_root,
                    relative_dir,
                )
        })
        .collect::<Vec<_>>();
    if owner_rules.len() != 1 {
        let reason = if owner_rules.is_empty() {
            "orphan ordinary implicit header-copy pattern has no finite named owner"
        } else {
            "ordinary implicit header-copy pattern has multiple named owners"
        };
        return rejected(hint, reason);
    }
    let owner_rule = owner_rules[0];
    let owner = owner_rule.target.trim().to_owned();
    let same_name_count = rules
        .iter()
        .filter(|candidate| {
            candidate.state != ConditionalTruth::False && candidate.target.trim() == owner
        })
        .count();
    if same_name_count != 1 {
        return rejected(
            Some(owner),
            "ordinary implicit header-copy owner has multiple active Make rule blocks",
        );
    }
    if !ordinary_rule_is_unconditional(owner_rule)
        || !owner_rule.recipes.is_empty()
        || owner_rule.prerequisites.contains('|')
    {
        return rejected(
            Some(owner),
            "ordinary implicit header-copy owner must be unconditional, dependency-only, and have no order-only prerequisites",
        );
    }
    let owner_context = MakeExprContext::new(
        scope,
        dirs,
        owner_rule.source_line,
        expression_root,
        relative_dir,
    );
    let outputs = match evaluate_make_list(&owner_rule.prerequisites, &owner_context) {
        Ok(outputs) if !outputs.is_empty() => outputs,
        Ok(_) => {
            return rejected(
                Some(owner),
                "ordinary implicit header-copy owner list is empty",
            );
        }
        Err(error) => {
            return rejected(
                Some(owner),
                format!("cannot resolve ordinary implicit header-copy owner outputs: {error}"),
            );
        }
    };
    if outputs.len() > MAX_OUTPUTS {
        return rejected(
            Some(owner),
            format!("ordinary implicit header-copy owner list exceeds {MAX_OUTPUTS} files"),
        );
    }
    if let Err(reason) = validate_literal_owner_outputs(
        &owner_rule.prerequisites,
        &outputs,
        &owner_context,
        dirs,
        true,
    ) {
        return rejected(Some(owner), reason);
    }
    let aliases = rules
        .iter()
        .filter(|candidate| {
            outputs.iter().any(|output| {
                named_owner_mentions(
                    candidate,
                    output,
                    scope,
                    dirs,
                    expression_root,
                    relative_dir,
                )
            })
        })
        .collect::<Vec<_>>();
    if aliases.len() != 1 || aliases[0].target.trim() != owner {
        return rejected(
            Some(owner),
            "ordinary implicit header-copy output list has an ambiguous named owner",
        );
    }
    let mut headers = Vec::with_capacity(outputs.len());
    let mut seen = HashSet::new();
    for output in &outputs {
        let Some(header) = output
            .strip_prefix(&output_root)
            .and_then(|tail| tail.strip_prefix('/'))
        else {
            return rejected_with_outputs(
                Some(owner),
                format!(
                    "ordinary implicit header-copy output `{output}` is outside the SDK include root"
                ),
                outputs.clone(),
            );
        };
        if !safe_header_relative(header) || header.contains('/') {
            return rejected_with_outputs(
                Some(owner),
                format!(
                    "ordinary implicit header-copy output `{output}` is not a safe flat .h basename"
                ),
                outputs.clone(),
            );
        }
        if !seen.insert(header.to_ascii_lowercase()) {
            return rejected_with_outputs(
                Some(owner),
                format!("ordinary implicit header-copy output `{output}` is duplicated"),
                outputs.clone(),
            );
        }
        headers.push(header.to_owned());
    }
    let mut declarations = Vec::with_capacity(headers.len());
    for header in headers {
        if let Err(reason) = validate_regular_source(source_root, relative_dir, &header) {
            return rejected_with_outputs(Some(owner), reason, outputs.clone());
        }
        let input = if relative_directory.is_empty() {
            format!("${{AROS_SOURCE_DIR}}/{header}")
        } else {
            format!("${{AROS_SOURCE_DIR}}/{relative_directory}/{header}")
        };
        declarations.push(HeaderTransformDecl {
            name: owner.clone(),
            file: if relative_directory.is_empty() {
                "mmakefile.src".to_owned()
            } else {
                format!("{relative_directory}/mmakefile.src")
            },
            line: rule.source_line + 1,
            input,
            output: format!("{cmake_root}/{header}"),
            match_text: String::new(),
            replacement: String::new(),
            copy_only: true,
            replace_whole_line_containing: false,
            substitutions: Vec::new(),
            dependencies: Vec::new(),
            consumers: Vec::new(),
            generated_input_owner: None,
        });
    }
    CandidateResult {
        owner: Some(owner),
        outputs,
        declarations,
        rejection: None,
    }
}

fn explicit_target(
    rule: &Rule,
    context: &MakeExprContext<'_>,
    dirs: &DirVars,
) -> Result<(String, IncludeRoot, String, &'static str, String), String> {
    literal_header_output(rule.target.trim(), context, dirs)
}

fn literal_header_output(
    raw: &str,
    context: &MakeExprContext<'_>,
    dirs: &DirVars,
) -> Result<(String, IncludeRoot, String, &'static str, String), String> {
    let (root_expression, include_root, tail) =
        if let Some(tail) = raw.strip_prefix("$(AROS_INCLUDES)/") {
            ("$(AROS_INCLUDES)", IncludeRoot::Sdk, tail)
        } else if let Some(tail) = raw.strip_prefix("$(GENINCDIR)/") {
            ("$(GENINCDIR)", IncludeRoot::Generated, tail)
        } else {
            return Err(format!(
                "include-header path `{raw}` must begin with a literal configured include root"
            ));
        };
    if raw.split_whitespace().count() != 1 || !safe_header_relative(tail) {
        return Err(format!(
            "include-header path `{raw}` is not one safe relative .h path"
        ));
    }
    let (_, output_root, cmake_root) =
        static_include_root(&format!("{root_expression}/%"), context, dirs)?;
    let resolved = evaluate_make_expr(raw, context)
        .map_err(|error| format!("cannot resolve literal include-header path: {error}"))?;
    let expected = format!("{output_root}/{tail}");
    if resolved != expected {
        return Err(
            "literal include-header path differs from its configured include-root mapping".into(),
        );
    }
    Ok((
        resolved,
        include_root,
        output_root,
        cmake_root,
        tail.to_owned(),
    ))
}

fn validate_literal_owner_outputs(
    raw: &str,
    outputs: &[String],
    context: &MakeExprContext<'_>,
    dirs: &DirVars,
    sdk_only: bool,
) -> Result<(), String> {
    let raw_outputs = raw.split_whitespace().collect::<Vec<_>>();
    if raw_outputs.len() != outputs.len() || raw_outputs.is_empty() {
        return Err(
            "header-copy owner must enumerate a finite list of literal output paths".into(),
        );
    }
    for (raw_output, output) in raw_outputs.iter().zip(outputs) {
        let (resolved, include_root, _, _, _) = literal_header_output(raw_output, context, dirs)
            .map_err(|reason| {
                format!("header-copy owner output is not a literal include path: {reason}")
            })?;
        if resolved != *output || (sdk_only && include_root != IncludeRoot::Sdk) {
            return Err(
                "header-copy owner output differs from its literal path or required SDK root"
                    .into(),
            );
        }
    }
    Ok(())
}

fn ordinary_rule_is_unconditional(rule: &Rule) -> bool {
    rule.state == ConditionalTruth::True
        && !rule.conditional_syntax
        && rule
            .recipes
            .iter()
            .all(|recipe| recipe.state == ConditionalTruth::True && !recipe.conditional_syntax)
}

fn named_owner_mentions(
    rule: &Rule,
    output: &str,
    scope: &VarScope,
    dirs: &DirVars,
    expression_root: &Path,
    relative_dir: &Path,
) -> bool {
    if rule.state == ConditionalTruth::False || !safe_owner_name(rule.target.trim()) {
        return false;
    }
    let context =
        MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
    evaluate_make_list(&rule.prerequisites, &context)
        .is_ok_and(|outputs| outputs.iter().any(|candidate| candidate == output))
        || rule
            .prerequisites
            .split_whitespace()
            .any(|candidate| candidate == output)
}

fn owner_mentions_include_root(
    rule: &Rule,
    scope: &VarScope,
    dirs: &DirVars,
    expression_root: &Path,
    relative_dir: &Path,
) -> bool {
    if !safe_owner_name(rule.target.trim()) {
        return false;
    }
    if rule.prerequisites.contains("$(AROS_INCLUDES)/") {
        return true;
    }
    let context =
        MakeExprContext::new(scope, dirs, rule.source_line, expression_root, relative_dir);
    let Ok(outputs) = evaluate_make_list(&rule.prerequisites, &context) else {
        return false;
    };
    evaluate_make_expr("$(AROS_INCLUDES)", &context).is_ok_and(|root| {
        outputs
            .iter()
            .any(|output| output.starts_with(&format!("{root}/")))
    })
}

fn explicit_copy_recipe(
    raw: &str,
    target: &str,
    context: &MakeExprContext<'_>,
    output: &str,
) -> bool {
    let words = raw.split_whitespace().collect::<Vec<_>>();
    if words.len() != 3 || !matches!(words[0], "$(CP)" | "@$(CP)") || words[1] != "$<" {
        return false;
    }
    if words[2] != target {
        return false;
    }
    evaluate_make_expr(words[2], context).is_ok_and(|destination| destination == output)
}

fn implicit_copy_recipe(raw: &str, context: &MakeExprContext<'_>, output_root: &str) -> bool {
    let words = raw.split_whitespace().collect::<Vec<_>>();
    words.len() == 3
        && matches!(words[0], "$(CP)" | "@$(CP)")
        && words[1] == "$<"
        && words[2] == "$(AROS_INCLUDES)"
        && evaluate_make_expr(words[2], context).is_ok_and(|destination| destination == output_root)
}

fn static_include_root(
    pattern: &str,
    context: &MakeExprContext<'_>,
    dirs: &DirVars,
) -> Result<(IncludeRoot, String, &'static str), String> {
    let (root, expression, cmake_root, expected_expression) = match pattern.trim() {
        "$(AROS_INCLUDES)/%" => (
            IncludeRoot::Sdk,
            "$(AROS_INCLUDES)",
            "${AROS_SDK_INCLUDE_DIR}",
            "$(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)",
        ),
        "$(GENINCDIR)/%" => (
            IncludeRoot::Generated,
            "$(GENINCDIR)",
            "${AROS_GENINC_DIR}",
            "$(GENDIR)/include",
        ),
        _ => {
            return Err(format!(
                "static header-copy pattern `{pattern}` must be exactly one known include root followed by `/%`"
            ));
        }
    };
    let output_root = evaluate_make_expr(expression, context)
        .map_err(|error| format!("cannot resolve static include root: {error}"))?;
    let root_components: &[&str] = match root {
        IncludeRoot::Sdk => &["AROS_INCLUDES", "AROS_DEVELOPER", "AROS_DIR_INCLUDE"],
        IncludeRoot::Generated => &["GENINCDIR", "GENDIR"],
    };
    for name in root_components {
        let reference = format!("$({name})");
        let configured = dirs
            .expand(&reference)
            .ok_or_else(|| format!("cannot resolve configured include component {name}"))?;
        let active = evaluate_make_expr(&reference, context)
            .map_err(|error| format!("cannot resolve active include component {name}: {error}"))?;
        if active != configured {
            return Err(format!(
                "static include root component {name} differs from its configured mapping"
            ));
        }
    }
    let configured_root = dirs
        .expand(expression)
        .ok_or_else(|| "cannot resolve source-configured include root".to_owned())?;
    let expected_root = dirs
        .expand(expected_expression)
        .ok_or_else(|| "cannot resolve configured include-root mapping".to_owned())?;
    if output_root != configured_root || configured_root != expected_root {
        return Err(format!(
            "static include root `{expression}` differs from its configured mapping"
        ));
    }
    Ok((root, output_root, cmake_root))
}

fn source_pattern_prefix(raw: &str, context: &MakeExprContext<'_>) -> Result<String, String> {
    const EXPECTED: &str = "$(SRCDIR)/$(CURDIR)/%";
    if raw.trim() != EXPECTED {
        return Err(format!(
            "static header-copy source prerequisite `{raw}` is not `{EXPECTED}`"
        ));
    }
    let prefix = raw
        .trim()
        .strip_suffix("/%")
        .expect("expected suffix checked");
    evaluate_make_expr(prefix, context)
        .map_err(|error| format!("cannot resolve source-local static pattern: {error}"))
}

fn find_owner(
    candidate: &Rule,
    rules: &[Rule],
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
    outputs: &[String],
) -> Result<String, (Option<String>, String)> {
    let static_pattern = candidate
        .static_pattern
        .as_ref()
        .expect("static candidate has a static pattern");
    let target_expression = static_pattern.targets.trim();
    let exact = rules
        .iter()
        .filter(|rule| {
            rule.state != ConditionalTruth::False
                && safe_owner_name(rule.target.trim())
                && rule.prerequisites.trim() == target_expression
        })
        .collect::<Vec<_>>();
    let nearby = rules
        .iter()
        .filter(|rule| {
            rule.state != ConditionalTruth::False
                && safe_owner_name(rule.target.trim())
                && rule
                    .prerequisites
                    .split_whitespace()
                    .any(|word| word == target_expression)
        })
        .collect::<Vec<_>>();
    let owners = exact
        .iter()
        .map(|rule| rule.target.trim().to_owned())
        .collect::<HashSet<_>>();
    if exact.len() != 1 || owners.len() != 1 {
        let owner_hint = match nearby.as_slice() {
            [rule, ..] => Some(rule.target.trim().to_owned()),
            [] => None,
        };
        let reason = if exact.len() > 1 || owners.len() > 1 {
            "static header-copy list has multiple named owners".to_owned()
        } else if !nearby.is_empty() {
            "static header-copy owner has extra prerequisites; its complete prerequisite expression must be the static list expression".to_owned()
        } else {
            "orphan static header-copy candidate has no named owner for its complete output list"
                .to_owned()
        };
        return Err((owner_hint, reason));
    }
    let owner_rule = exact[0];
    let owner = owner_rule.target.trim().to_owned();
    let same_name_count = rules
        .iter()
        .filter(|rule| rule.state != ConditionalTruth::False && rule.target.trim() == owner)
        .count();
    if same_name_count != 1 {
        return Err((
            Some(owner),
            "static header-copy owner has multiple active Make rule blocks".into(),
        ));
    }
    if owner_rule.state != ConditionalTruth::True
        || owner_rule.conditional_syntax
        || owner_rule
            .recipes
            .iter()
            .any(|recipe| recipe.state != ConditionalTruth::True || recipe.conditional_syntax)
    {
        return Err((
            Some(owner),
            "static header-copy owner is guarded by an unresolved Make conditional".into(),
        ));
    }
    if !owner_rule.recipes.is_empty() {
        return Err((
            Some(owner),
            "static header-copy owner alias must not have recipe commands".into(),
        ));
    }
    if nearby.len() != 1 || nearby[0].target.trim() != owner {
        return Err((
            Some(owner),
            "static header-copy output list has an ambiguous alias owner".into(),
        ));
    }
    let owner_context = MakeExprContext::new(
        scope,
        dirs,
        owner_rule.source_line,
        source_root,
        relative_dir,
    );
    let owner_outputs =
        evaluate_make_list(&owner_rule.prerequisites, &owner_context).map_err(|error| {
            (
                Some(owner.clone()),
                format!("cannot resolve static header-copy owner prerequisites: {error}"),
            )
        })?;
    if owner_outputs != outputs {
        return Err((
            Some(owner),
            "static header-copy owner prerequisites do not resolve to the complete static output list".into(),
        ));
    }
    if candidate.recipes.len() != 1 {
        return Err((
            Some(owner),
            "static header-copy pattern must have exactly one copy recipe command".into(),
        ));
    }
    let recipe_words = candidate.recipes[0]
        .text
        .split_whitespace()
        .collect::<Vec<_>>();
    if recipe_words.as_slice() != ["@$(CP)", "$<", "$@"] {
        return Err((
            Some(owner),
            "static header-copy recipe must be exactly `@$(CP) $< $@`".into(),
        ));
    }
    Ok(owner)
}

fn expression_targets_known_include(expression: &str, context: &MakeExprContext<'_>) -> bool {
    let Ok(outputs) = evaluate_make_list(expression, context) else {
        return false;
    };
    let roots = ["$(AROS_INCLUDES)", "$(GENINCDIR)"]
        .into_iter()
        .filter_map(|root| evaluate_make_expr(root, context).ok())
        .collect::<Vec<_>>();
    outputs.iter().any(|output| {
        roots.iter().any(|root| {
            output
                .strip_prefix(root)
                .is_some_and(|tail| tail.starts_with('/'))
        })
    })
}

fn source_local_header_candidate(
    rule: &Rule,
    pattern: &StaticPattern,
    context: &MakeExprContext<'_>,
) -> bool {
    if rule.prerequisites.trim() != "$(SRCDIR)/$(CURDIR)/%" {
        return false;
    }
    // If a source-local static rule has an unresolved finite target-list
    // variable, do not silently omit what may be a header-copy rule.
    evaluate_make_list(&pattern.targets, context).map_or_else(
        |_| simple_variable_reference(&pattern.targets).is_some(),
        |outputs| {
            outputs.iter().any(|output| {
                Path::new(output)
                    .extension()
                    .is_some_and(|extension| extension.to_str() == Some("h"))
            })
        },
    )
}

fn is_include_root_expression(pattern: &str) -> bool {
    pattern.contains("$(AROS_INCLUDES)") || pattern.contains("$(GENINCDIR)")
}

fn simple_variable_reference(expression: &str) -> Option<&str> {
    let value = expression.trim();
    let body = value
        .strip_prefix("$(")
        .and_then(|value| value.strip_suffix(')'))?;
    if body.is_empty()
        || !body
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return None;
    }
    Some(body)
}

fn owner_hint(candidate: &Rule, rules: &[Rule]) -> Option<String> {
    let expression = candidate.static_pattern.as_ref()?.targets.trim();
    let owners = rules
        .iter()
        .filter(|rule| {
            rule.state != ConditionalTruth::False
                && safe_owner_name(rule.target.trim())
                && rule
                    .prerequisites
                    .split_whitespace()
                    .any(|word| word == expression)
        })
        .map(|rule| rule.target.trim().to_owned())
        .collect::<HashSet<_>>();
    if owners.len() == 1 {
        owners.into_iter().next()
    } else {
        None
    }
}

fn safe_owner_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-'))
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn safe_header_relative(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_HEADER_RELATIVE_BYTES
        || value.starts_with('/')
        || value.contains([
            '\\', ';', '$', '`', '*', '?', '[', ']', '#', '\n', '\r', '\t',
        ])
        || Path::new(value).is_absolute()
        || Path::new(value)
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("h")
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

fn validate_regular_source(
    source_root: &Path,
    relative_dir: &Path,
    header: &str,
) -> Result<(), String> {
    let mut path = source_root.to_path_buf();
    let mut components = Vec::new();
    components.extend(relative_dir.components());
    components.extend(Path::new(header).components());
    for component in components {
        let Component::Normal(component) = component else {
            return Err("static header-copy input escapes the selected source tree".into());
        };
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "static header-copy source {} is missing: {error}",
                path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "static header-copy source crosses a symlink: {}",
                path.display()
            ));
        }
        if path != source_root.join(relative_dir).join(header) && !metadata.is_dir() {
            return Err(format!(
                "static header-copy source parent is not a directory: {}",
                path.display()
            ));
        }
        if path == source_root.join(relative_dir).join(header) && !metadata.is_file() {
            return Err(format!(
                "static header-copy source is not a regular file: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn rejected(owner: Option<String>, reason: impl Into<String>) -> CandidateResult {
    CandidateResult {
        owner,
        outputs: Vec::new(),
        declarations: Vec::new(),
        rejection: Some(reason.into()),
    }
}

fn rejected_with_outputs(
    owner: Option<String>,
    reason: impl Into<String>,
    outputs: Vec<String>,
) -> CandidateResult {
    CandidateResult {
        owner,
        outputs,
        declarations: Vec::new(),
        rejection: Some(reason.into()),
    }
}

fn rejected_all_candidates(
    static_candidates: &[&Rule],
    ordinary_candidates: &[&Rule],
    rules: &[Rule],
    reason: &str,
) -> (Vec<HeaderTransformDecl>, Vec<Rejection>) {
    let mut rejections = static_candidates
        .iter()
        .map(|rule| Rejection {
            owner: owner_hint(rule, rules),
            reason: reason.to_owned(),
        })
        .collect::<Vec<_>>();
    rejections.extend(ordinary_candidates.iter().map(|rule| Rejection {
        owner: ordinary_owner_hint(rule, rules),
        reason: reason.to_owned(),
    }));
    (Vec::new(), rejections)
}

fn ordinary_owner_hint(candidate: &Rule, rules: &[Rule]) -> Option<String> {
    let output = if ordinary_candidate_kind(candidate) == Some(OrdinaryCandidateKind::Explicit) {
        // A lightweight expansion is enough for diagnostics; validation still
        // happens in the full ordinary-candidate scanner.
        Some(candidate.target.trim().to_owned())
    } else {
        None
    };
    let mentions = rules
        .iter()
        .filter(|rule| {
            safe_owner_name(rule.target.trim())
                && (output.as_ref().is_some_and(|output| {
                    rule.prerequisites
                        .split_whitespace()
                        .any(|word| word == output)
                }) || (matches!(
                    ordinary_candidate_kind(candidate),
                    Some(OrdinaryCandidateKind::Implicit)
                ) && rule.prerequisites.contains("$(AROS_INCLUDES)/")))
        })
        .map(|rule| rule.target.trim().to_owned())
        .collect::<HashSet<_>>();
    if mentions.len() == 1 {
        mentions.into_iter().next()
    } else {
        None
    }
}

fn logical_lines(content: &str, states: Option<&[ConditionalTruth]>) -> Vec<LogicalLine> {
    let physical = content.lines().collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut conditional_depth = 0usize;
    let mut define_depth = 0usize;
    let mut at = 0usize;
    while at < physical.len() {
        let first = physical[at];
        if !first.starts_with('\t') {
            match make_define_boundary(first) {
                Some(true) => {
                    define_depth = define_depth.saturating_add(1);
                    at += 1;
                    continue;
                }
                Some(false) if define_depth > 0 => {
                    define_depth -= 1;
                    at += 1;
                    continue;
                }
                Some(false) => {
                    at += 1;
                    continue;
                }
                None => {}
            }
        }
        if define_depth > 0 {
            at += 1;
            continue;
        }
        let first_trimmed = first.trim();
        if is_conditional_open(first_trimmed) {
            conditional_depth += 1;
        }
        let mut text = first.to_owned();
        let source_line = at;
        let mut state = line_state(states, at);
        let mut conditional_syntax = states.is_none() && conditional_depth > 0;
        while has_unescaped_trailing_backslash(&text) && at + 1 < physical.len() {
            let trimmed = text.trim_end();
            text.truncate(trimmed.len() - 1);
            at += 1;
            state = combine_state(state, line_state(states, at));
            conditional_syntax |= states.is_none() && conditional_depth > 0;
            text.push(' ');
            text.push_str(physical[at].trim());
        }
        output.push(LogicalLine {
            text,
            source_line,
            state,
            conditional_syntax,
        });
        if is_conditional_close(first_trimmed) {
            conditional_depth = conditional_depth.saturating_sub(1);
        }
        at += 1;
    }
    output
}

/// Returns `Some(true)` for an opening GNU Make define and `Some(false)` for a
/// syntactically complete `endef`. The same modifier forms as the variable
/// scanner are recognized; a fourth modifier fails closed as an open body.
fn make_define_boundary(line: &str) -> Option<bool> {
    if line.starts_with('\t') {
        return None;
    }
    let statement = strip_make_comment(line.trim_start()).trim_start();
    let mut words = statement.splitn(2, char::is_whitespace);
    let mut word = words.next().unwrap_or_default();
    let mut rest = words.next().unwrap_or_default().trim();
    let mut modifiers = 0;
    while matches!(word, "override" | "export" | "private") && modifiers < 3 {
        modifiers += 1;
        let mut modifier_words = rest.splitn(2, char::is_whitespace);
        word = modifier_words.next().unwrap_or_default();
        rest = modifier_words.next().unwrap_or_default().trim();
    }
    if word == "define" {
        Some(true)
    } else if word == "endef" {
        rest.is_empty().then_some(false)
    } else if modifiers == 3 && matches!(word, "override" | "export" | "private") {
        Some(true)
    } else {
        None
    }
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
                        state: line.state,
                        conditional_syntax: line.conditional_syntax,
                    });
                    rule.conditional_syntax |= line.conditional_syntax;
                }
            }
            continue;
        }
        let statement = strip_make_comment(line.text.trim_end_matches('\r')).trim();
        if statement.is_empty() {
            continue;
        }
        if is_conditional_directive(statement) {
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
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
        let Some((target, tail)) = statement.split_once(':') else {
            continue;
        };
        // The second colon in `output.h: CP := ...` belongs to the variable
        // assignment operator, not a static-pattern separator. Keep this as
        // a rule so callers can reject target-specific CP scope, but don't
        // manufacture a file-producing static-pattern candidate from it.
        if target_specific_assignment_name(tail).is_some() {
            current = Some(Rule {
                target: target.trim().to_owned(),
                prerequisites: tail.trim().to_owned(),
                source_line: line.source_line,
                state: line.state,
                conditional_syntax: line.conditional_syntax,
                static_pattern: None,
                recipes: Vec::new(),
            });
            continue;
        }
        let (static_pattern, prerequisites) =
            if let Some((pattern, prerequisites)) = tail.split_once(':') {
                (
                    Some(StaticPattern {
                        targets: target.trim().to_owned(),
                        pattern: pattern.trim().to_owned(),
                    }),
                    prerequisites.trim().to_owned(),
                )
            } else {
                (None, tail.trim().to_owned())
            };
        if target.trim().is_empty() || target.contains('=') {
            continue;
        }
        current = Some(Rule {
            target: target.trim().to_owned(),
            prerequisites,
            source_line: line.source_line,
            state: line.state,
            conditional_syntax: line.conditional_syntax,
            static_pattern,
            recipes: Vec::new(),
        });
    }
    if let Some(rule) = current {
        rules.push(rule);
    }
    rules
}

fn line_state(states: Option<&[ConditionalTruth]>, index: usize) -> ConditionalTruth {
    states.map_or(ConditionalTruth::True, |states| {
        states
            .get(index)
            .copied()
            .unwrap_or(ConditionalTruth::Unknown)
    })
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

fn has_unescaped_trailing_backslash(value: &str) -> bool {
    let tail = value.trim_end();
    tail.chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count()
        % 2
        == 1
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

#[cfg(test)]
#[path = "static_header_copies_tests.rs"]
mod tests;
