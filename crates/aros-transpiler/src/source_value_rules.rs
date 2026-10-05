//! Closed scanner for source-local rules which extract one literal value.
//!
//! The scanner recognizes a deliberately narrow, side-effect-free shape: a
//! tagged ordinary Make owner, one directory-creation command, and a two-stage
//! `sed` pipeline with literal scripts. It models the pipeline's documented
//! string operations; it never evaluates shell or invokes `sed`.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{strip_make_comment, ConditionalTruth, VarScope};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_RULES: usize = 16_384;
const MAX_REJECTIONS: usize = 4096;
const SOURCE_ALIAS: &str = "${AROS_SOURCE_DIR}";
const BUILD_PREFS: &str = "${AROS_BUILD_DIR}/SYS/Prefs";
const SECOND_SED_SCRIPT: &str = "s/^ *//;s/[ */*].*//p";

/// A named source-local rule producing one value through a closed `sed` model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceValueRuleDecl {
    pub owner: String,
    /// Source-relative joined mmakefile path.
    pub file: String,
    /// One-based source line containing the owner rule.
    pub line: usize,
    /// Existing input below `${AROS_SOURCE_DIR}`.
    pub input: String,
    /// Generated output below `${AROS_BUILD_DIR}/SYS/Prefs`.
    pub output: String,
    /// Literal first-stage `sed` marker, removed at its first occurrence.
    pub marker: String,
    /// Filled by parser integration from the original source snapshot.
    pub file_sha256: String,
}

/// A source-value rule that could not be represented by the closed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceValueRuleRejection {
    pub owner: String,
    pub file: String,
    pub line: usize,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct LogicalLine {
    text: String,
    source_line: usize,
    continued: bool,
}

#[derive(Debug, Clone)]
struct Recipe {
    text: String,
    continued: bool,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct Rule {
    owner: String,
    owner_line: usize,
    state: ConditionalTruth,
    marker_state: ConditionalTruth,
    issues: Vec<RuleIssue>,
    recipes: Vec<Recipe>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleIssue {
    Prerequisites,
    DoubleColon,
    TargetSpecific,
    InvalidOwner,
    InlineRecipe,
    ConditionalSyntax,
}

/// Collects only tagged source-value rules with the source/build roots proven
/// by this translation context. Recipe paths use the final Make variable
/// scope, because Make expands ordinary recipe references when the recipe is
/// run, after the complete makefile has been read. Conditional line states
/// still refer to zero-based source-line positions.
#[must_use]
pub(crate) fn collect_source_value_rules_with_context(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SourceValueRuleDecl>, Vec<SourceValueRuleRejection>) {
    let file = match source_file_name(rel_dir) {
        Ok(file) => file,
        Err(reason) => {
            return (
                Vec::new(),
                vec![rejection(
                    "<unknown>",
                    "<unknown>",
                    1,
                    format!("source-value directory: {reason}"),
                )],
            );
        }
    };
    if content.len() > MAX_SNAPSHOT_BYTES {
        return (
            Vec::new(),
            vec![rejection(
                "<unknown>",
                &file,
                1,
                "joined mmakefile exceeds the source-value scan limit",
            )],
        );
    }

    let source_root = match source_root.canonicalize() {
        Ok(root) if root.is_dir() => root,
        _ => {
            return (
                Vec::new(),
                vec![rejection(
                    "<unknown>",
                    &file,
                    1,
                    "source root is not an existing canonicalizable directory",
                )],
            );
        }
    };

    let (rules, target_specific_owners) = parse_rules(content, line_states);
    if rules.len() > MAX_RULES {
        return (
            Vec::new(),
            vec![rejection(
                "<unknown>",
                &file,
                1,
                "joined mmakefile exceeds the source-value rule limit",
            )],
        );
    }

    let candidates = rules
        .into_iter()
        .filter(is_source_value_candidate)
        .collect::<Vec<_>>();
    let mut owner_counts = BTreeMap::<String, usize>::new();
    for rule in &candidates {
        *owner_counts.entry(rule.owner.clone()).or_default() += 1;
    }

    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    for rule in candidates {
        let owner = if rule.owner.is_empty() {
            "<unknown>"
        } else {
            &rule.owner
        };
        let line = rule.owner_line + 1;
        let fail = |reason: String| rejection(owner, &file, line, reason);

        if rule.issues.contains(&RuleIssue::InvalidOwner) {
            rejections.push(fail(
                "owner is not one safe literal ordinary Make target".into(),
            ));
            continue;
        }
        if owner_counts.get(&rule.owner).copied().unwrap_or(0) != 1 {
            rejections.push(fail("owner has duplicate source-value rule blocks".into()));
            continue;
        }
        if rule.state == ConditionalTruth::False || rule.marker_state == ConditionalTruth::False {
            continue;
        }
        if rule.state == ConditionalTruth::Unknown || rule.marker_state == ConditionalTruth::Unknown
        {
            rejections.push(fail(
                "owner or #MM marker is in an undecided Make conditional".into(),
            ));
            continue;
        }
        if rule.issues.contains(&RuleIssue::ConditionalSyntax) {
            rejections.push(fail(
                "conditional syntax affects the owner or recipe".into(),
            ));
            continue;
        }
        if rule.issues.contains(&RuleIssue::DoubleColon) {
            rejections.push(fail(
                "source-value owner must use an ordinary single-colon rule".into(),
            ));
            continue;
        }
        if rule.issues.contains(&RuleIssue::Prerequisites) {
            rejections.push(fail(
                "source-value owner must not have prerequisites".into(),
            ));
            continue;
        }
        if rule.issues.contains(&RuleIssue::TargetSpecific)
            || target_specific_owners.contains(&rule.owner)
        {
            rejections.push(fail(
                "target-specific variable scope is not represented".into(),
            ));
            continue;
        }
        if rule.issues.contains(&RuleIssue::InlineRecipe) || rule.recipes.len() != 2 {
            rejections.push(fail(format!(
                "source-value owner has {} recipe lines; expected exactly two plain lines",
                rule.recipes.len()
            )));
            continue;
        }
        if rule.recipes.iter().any(|recipe| recipe.continued) {
            rejections.push(fail(
                "continued or multiline recipe commands are not represented".into(),
            ));
            continue;
        }
        if rule
            .recipes
            .iter()
            .any(|recipe| recipe.state == ConditionalTruth::Unknown)
        {
            rejections.push(fail(
                "recipe line is in an undecided Make conditional".into(),
            ));
            continue;
        }
        if rule
            .recipes
            .iter()
            .any(|recipe| recipe.state == ConditionalTruth::False)
        {
            rejections.push(fail(
                "recipe is only partially active under a Make conditional".into(),
            ));
            continue;
        }

        let (raw_mkdir, raw_input, raw_output, marker) = match parse_recipe_pair(&rule.recipes) {
            Ok(parsed) => parsed,
            Err(reason) => {
                rejections.push(fail(reason));
                continue;
            }
        };

        if ["AROS_PREFS", "SRCDIR", "CURDIR"].iter().any(|name| {
            scope.path_raw_at(name, usize::MAX).is_some()
                || scope.conditionally_assigned_before(name, usize::MAX)
        }) {
            rejections.push(fail(
                "source-local `AROS_PREFS`, `SRCDIR`, or `CURDIR` override is not allowed".into(),
            ));
            continue;
        }
        if dirs.expand("$(AROS_DIR_AROS)").as_deref() != Some("SYS")
            || dirs.expand("$(AROS_DIR_PREFS)").as_deref() != Some("Prefs")
            || dirs.expand("$(AROS_PREFS)").as_deref() != Some(BUILD_PREFS)
        {
            rejections.push(fail(
                "configured `AROS_PREFS` is not the canonical source-owned SYS/Prefs root".into(),
            ));
            continue;
        }

        let expression_context =
            MakeExprContext::new(scope, dirs, usize::MAX, &source_root, rel_dir);
        let output = match evaluate_make_expr(raw_output, &expression_context) {
            Ok(path) => path,
            Err(error) => {
                rejections.push(fail(format!("cannot resolve source-value output: {error}")));
                continue;
            }
        };
        if !safe_cmake_path(&output) || !output.starts_with(&format!("{BUILD_PREFS}/")) {
            rejections.push(fail(
                "output is not a safe path below canonical `AROS_PREFS`".into(),
            ));
            continue;
        }
        let output_parent = output
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or_default();
        let mkdir = match evaluate_make_expr(raw_mkdir, &expression_context) {
            Ok(path) => path,
            Err(error) => {
                rejections.push(fail(format!(
                    "cannot resolve source-value directory: {error}"
                )));
                continue;
            }
        };
        if !safe_cmake_path(&mkdir) || mkdir != output_parent {
            rejections.push(fail(
                "directory command does not name exactly the output parent".into(),
            ));
            continue;
        }

        let input = match evaluate_make_expr(raw_input, &expression_context) {
            Ok(path) => path,
            Err(error) => {
                rejections.push(fail(format!("cannot resolve source-value input: {error}")));
                continue;
            }
        };
        let relative_input = match input.strip_prefix(&format!("{SOURCE_ALIAS}/")) {
            Some(path) if safe_relative_path(path) => path,
            _ => {
                rejections.push(fail(
                    "input is not a safe literal path below `$(SRCDIR)`".into(),
                ));
                continue;
            }
        };
        let relative_dir_text = path_text(rel_dir);
        let expected_prefix = if relative_dir_text.is_empty() || relative_dir_text == "." {
            String::new()
        } else {
            format!("{relative_dir_text}/")
        };
        if !relative_input.starts_with(&expected_prefix) {
            rejections.push(fail(
                "input is not local to the mmakefile directory under `$(SRCDIR)`".into(),
            ));
            continue;
        }
        if let Err(reason) = validate_existing_source_input(&source_root, relative_input) {
            rejections.push(fail(reason));
            continue;
        }
        if let Err(reason) = validate_marker(&marker) {
            rejections.push(fail(reason));
            continue;
        }

        declarations.push(SourceValueRuleDecl {
            owner: rule.owner,
            file: file.clone(),
            line,
            input,
            output,
            marker,
            file_sha256: String::new(),
        });
    }

    if rejections.len() > MAX_REJECTIONS {
        rejections.truncate(MAX_REJECTIONS);
        rejections.push(rejection(
            "<unknown>",
            &file,
            1,
            "source-value rejection limit reached",
        ));
    }
    (declarations, rejections)
}

fn parse_rules(
    content: &str,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<Rule>, BTreeSet<String>) {
    let mut rules = Vec::new();
    let mut target_specific_owners = BTreeSet::new();
    let mut current: Option<Rule> = None;
    let mut pending_bare_mm = false;
    let mut pending_marker_state = ConditionalTruth::True;
    let mut conditional_depth = 0usize;
    let mut conditional_else_seen = Vec::<bool>::new();
    let mut conditional_malformed = false;
    let mut gnu_define_depth = 0usize;
    let mut metamake_define_depth = 0usize;

    for logical in logical_lines(content) {
        let state = line_state(line_states, logical.source_line);
        let trimmed = logical.text.trim_start();

        if gnu_define_depth > 0 {
            if let Some((directive, _)) = gnu_define_directive(trimmed) {
                if directive == "define" {
                    gnu_define_depth = gnu_define_depth.saturating_add(1);
                } else {
                    gnu_define_depth = gnu_define_depth.saturating_sub(1);
                }
            }
            continue;
        }
        if metamake_define_depth > 0 {
            let uncommented = strip_make_comment(trimmed).trim();
            if is_metamake_define(uncommented) {
                metamake_define_depth = metamake_define_depth.saturating_add(1);
            } else if uncommented == "%end" {
                metamake_define_depth = metamake_define_depth.saturating_sub(1);
            }
            continue;
        }

        if !logical.text.starts_with('\t') {
            if let Some((directive, _)) = gnu_define_directive(trimmed) {
                if let Some(rule) = current.take() {
                    rules.push(rule);
                }
                pending_bare_mm = false;
                if directive == "define" {
                    gnu_define_depth = 1;
                }
                continue;
            }
            let uncommented = strip_make_comment(trimmed).trim();
            if is_metamake_define(uncommented) {
                if let Some(rule) = current.take() {
                    rules.push(rule);
                }
                pending_bare_mm = false;
                metamake_define_depth = 1;
                continue;
            }
        }

        if logical.text.starts_with('\t') {
            if let Some(rule) = current.as_mut() {
                if rule.state == ConditionalTruth::False || state == ConditionalTruth::False {
                    continue;
                }
                if state == ConditionalTruth::Unknown {
                    add_issue(rule, RuleIssue::ConditionalSyntax);
                }
                if line_states.is_none() && conditional_depth > 0 {
                    add_issue(rule, RuleIssue::ConditionalSyntax);
                }
                rule.recipes.push(Recipe {
                    text: logical.text.trim().to_owned(),
                    continued: logical.continued,
                    state,
                });
            }
            continue;
        }

        let raw_header = trimmed.trim();
        if raw_header == "#MM" {
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
            pending_bare_mm = true;
            pending_marker_state = state;
            continue;
        }
        if raw_header.starts_with("#MM-") {
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
            pending_bare_mm = false;
            continue;
        }

        let uncommented = strip_make_comment(trimmed).trim();
        if uncommented.is_empty() {
            continue;
        }

        if let Some(kind) = conditional_directive(uncommented) {
            if let Some(rule) = current.as_mut() {
                if line_states.is_none() {
                    add_issue(rule, RuleIssue::ConditionalSyntax);
                }
                if state == ConditionalTruth::Unknown {
                    add_issue(rule, RuleIssue::ConditionalSyntax);
                }
            }
            match kind {
                ConditionalDirective::Open => {
                    conditional_depth += 1;
                    conditional_else_seen.push(false);
                    if !valid_conditional_open(uncommented) {
                        conditional_malformed = true;
                    }
                }
                ConditionalDirective::Branch => {
                    if uncommented != "else" {
                        conditional_malformed = true;
                    }
                    if let Some(seen) = conditional_else_seen.last_mut() {
                        if *seen {
                            conditional_malformed = true;
                        }
                        *seen = true;
                    } else {
                        conditional_malformed = true;
                    }
                }
                ConditionalDirective::Close => {
                    if conditional_depth == 0 {
                        conditional_malformed = true;
                    } else {
                        conditional_depth -= 1;
                        conditional_else_seen.pop();
                        if uncommented != "endif" {
                            conditional_malformed = true;
                        }
                    }
                }
            }
            continue;
        }

        if let Some(rule) = current.take() {
            rules.push(rule);
        }

        if let Some(owner) = target_specific_owner(uncommented) {
            target_specific_owners.insert(owner);
        }

        if !pending_bare_mm {
            continue;
        }
        pending_bare_mm = false;
        let Some(mut rule) = parse_rule_header(
            uncommented,
            logical.source_line,
            state,
            pending_marker_state,
        ) else {
            continue;
        };
        if line_states.is_none() && conditional_depth > 0 {
            add_issue(&mut rule, RuleIssue::ConditionalSyntax);
        }
        if conditional_malformed {
            add_issue(&mut rule, RuleIssue::ConditionalSyntax);
        }
        current = Some(rule);
    }

    if conditional_depth != 0 {
        conditional_malformed = true;
    }
    if let Some(rule) = current {
        rules.push(rule);
    }
    if conditional_malformed {
        for rule in &mut rules {
            add_issue(rule, RuleIssue::ConditionalSyntax);
        }
    }
    (rules, target_specific_owners)
}

fn parse_rule_header(
    header: &str,
    owner_line: usize,
    state: ConditionalTruth,
    marker_state: ConditionalTruth,
) -> Option<Rule> {
    let (target, after_colon) = header.split_once(':')?;
    let double_colon = after_colon.starts_with(':');
    let after_colon = if double_colon {
        &after_colon[1..]
    } else {
        after_colon
    };
    let target_words = target.split_whitespace().collect::<Vec<_>>();
    let owner = target_words.first().copied().unwrap_or_default().to_owned();
    let target_specific = is_make_assignment(after_colon.trim());
    let mut prerequisites = after_colon.trim();
    let mut inline_recipe = false;
    let mut recipes = Vec::new();
    if let Some((before_recipe, recipe)) = prerequisites.split_once(';') {
        prerequisites = before_recipe.trim();
        inline_recipe = true;
        if !recipe.trim().is_empty() {
            recipes.push(Recipe {
                text: recipe.trim().to_owned(),
                continued: false,
                state,
            });
        }
    }
    Some(Rule {
        issues: {
            let mut issues = Vec::new();
            if target_words.len() != 1 || !safe_target_name(&owner) {
                issues.push(RuleIssue::InvalidOwner);
            }
            if !prerequisites.is_empty() && !target_specific {
                issues.push(RuleIssue::Prerequisites);
            }
            if double_colon {
                issues.push(RuleIssue::DoubleColon);
            }
            if target_specific {
                issues.push(RuleIssue::TargetSpecific);
            }
            if inline_recipe {
                issues.push(RuleIssue::InlineRecipe);
            }
            issues
        },
        owner,
        owner_line,
        state,
        marker_state,
        recipes,
    })
}

fn is_source_value_candidate(rule: &Rule) -> bool {
    rule.recipes.iter().any(|recipe| {
        recipe.text.contains("$(SED)")
            && recipe.text.contains('>')
            && (recipe.text.contains("$(SRCDIR)") || recipe.text.contains("AROS_PREFS"))
    })
}

fn add_issue(rule: &mut Rule, issue: RuleIssue) {
    if !rule.issues.contains(&issue) {
        rule.issues.push(issue);
    }
}

fn parse_recipe_pair(recipes: &[Recipe]) -> Result<(&str, &str, &str, String), String> {
    let mkdir = strip_recipe_prefix(&recipes[0].text)?;
    let mkdir_path = mkdir
        .strip_prefix("$(MKDIR) ")
        .filter(|path| !path.is_empty() && !path.starts_with(char::is_whitespace))
        .ok_or_else(|| {
            "first recipe line must be one literal `$(MKDIR) <path>` command".to_owned()
        })?;
    if contains_shell_syntax(mkdir_path) {
        return Err("directory command contains unsupported shell syntax".into());
    }

    let pipeline = strip_recipe_prefix(&recipes[1].text)?;
    let prefix = "$(SED) -n 's/";
    let rest = pipeline.strip_prefix(prefix).ok_or_else(|| {
        "second recipe line must begin with a literal first-stage `$(SED)`".to_owned()
    })?;
    let (marker, rest) = rest.split_once("// p' < ").ok_or_else(|| {
        "first-stage `sed` script is outside the closed substitution form".to_owned()
    })?;
    let (input, output) = rest
        .split_once(&format!(" | $(SED) -n '{SECOND_SED_SCRIPT}' > "))
        .ok_or_else(|| "recipe is not the exact supported two-stage `sed` pipeline".to_owned())?;
    if input.is_empty()
        || output.is_empty()
        || contains_shell_syntax(input)
        || contains_shell_syntax(output)
    {
        return Err("source-value input or output contains unsupported shell syntax".into());
    }
    Ok((mkdir_path, input, output, marker.to_owned()))
}

fn strip_recipe_prefix(recipe: &str) -> Result<&str, String> {
    let command = recipe.strip_prefix('@').unwrap_or(recipe);
    if command.is_empty() || command.starts_with('@') {
        return Err("recipe command has unsupported command modifiers".into());
    }
    Ok(command)
}

fn validate_marker(marker: &str) -> Result<(), String> {
    if marker.is_empty() || marker.len() > 128 || !marker.is_ascii() {
        return Err("first-stage marker must contain 1–128 ASCII bytes".into());
    }
    if marker.bytes().any(|byte| {
        matches!(
            byte,
            b'.' | b'['
                | b']'
                | b'*'
                | b'^'
                | b'$'
                | b'\\'
                | b'+'
                | b'?'
                | b'('
                | b')'
                | b'{'
                | b'}'
                | b'|'
                | b'/'
                | b'\''
                | b'"'
                | b'`'
                | b';'
                | b'\n'
                | b'\r'
                | b'<'
                | b'>'
                | b'&'
        )
    }) {
        return Err(
            "first-stage marker contains a BRE metacharacter, escape, or shell delimiter".into(),
        );
    }
    Ok(())
}

fn validate_existing_source_input(source_root: &Path, relative_input: &str) -> Result<(), String> {
    if !safe_relative_path(relative_input) {
        return Err("input path contains traversal or unsafe source components".into());
    }
    let mut path = source_root.to_path_buf();
    for (index, component) in Path::new(relative_input).components().enumerate() {
        let Component::Normal(component) = component else {
            return Err("input path contains a non-normal source component".into());
        };
        path.push(component);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| "source-value input does not exist below the source root".to_owned())?;
        if metadata.file_type().is_symlink() {
            return Err("source-value input has a symlink path component".into());
        }
        if index + 1 < Path::new(relative_input).components().count() && !metadata.is_dir() {
            return Err("source-value input has a non-directory path component".into());
        }
        if index + 1 == Path::new(relative_input).components().count() && !metadata.is_file() {
            return Err("source-value input is not a regular file".into());
        }
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "source-value input could not be canonicalized".to_owned())?;
    if !canonical.starts_with(source_root) {
        return Err("source-value input resolves outside the canonical source root".into());
    }
    Ok(())
}

fn safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
        && !value.contains(['\\', '$', ';', '"', '\'', '`', '|', '&', '<', '>'])
        && Path::new(value).components().all(|component| {
            matches!(component, Component::Normal(name)
            if !name.is_empty()
                && name.to_string_lossy().bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.')
                }))
        })
}

fn safe_cmake_path(value: &str) -> bool {
    if value.is_empty()
        || value.chars().any(char::is_whitespace)
        || !value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
        || value.contains([';', '\\', '\n', '\r', '"', '\'', '`', '|', '&', '<', '>'])
        || value.contains("$(")
        || !allowed_cmake_dollars(value)
    {
        return false;
    }
    Path::new(value)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn allowed_cmake_dollars(value: &str) -> bool {
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        rest = &rest[at + 1..];
        let Some(tail) = rest.strip_prefix('{') else {
            return false;
        };
        let Some(end) = tail.find('}') else {
            return false;
        };
        if &tail[..end] != "AROS_BUILD_DIR" {
            return false;
        }
        rest = &tail[end + 1..];
    }
    true
}

fn target_specific_owner(header: &str) -> Option<String> {
    let (target, after_colon) = header.split_once(':')?;
    let targets = target.split_whitespace().collect::<Vec<_>>();
    if targets.len() != 1 || !is_make_assignment(after_colon.trim()) {
        return None;
    }
    Some(targets[0].to_owned())
}

fn is_make_assignment(value: &str) -> bool {
    ["::=", ":=", "?=", "+=", "!=", "="].iter().any(|operator| {
        value
            .split_once(operator)
            .is_some_and(|(name, _)| !name.trim().is_empty() && !name.trim().contains(':'))
    })
}

fn conditional_directive(line: &str) -> Option<ConditionalDirective> {
    let word = line.split_whitespace().next()?;
    match word {
        "ifeq" | "ifneq" | "ifdef" | "ifndef" => Some(ConditionalDirective::Open),
        "else" => Some(ConditionalDirective::Branch),
        "endif" => Some(ConditionalDirective::Close),
        _ => None,
    }
}

fn valid_conditional_open(line: &str) -> bool {
    let mut words = line.split_whitespace();
    let directive = words.next().unwrap_or_default();
    let arguments = words.collect::<Vec<_>>();
    match directive {
        "ifeq" | "ifneq" => !arguments.is_empty(),
        "ifdef" | "ifndef" => arguments.len() == 1,
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum ConditionalDirective {
    Open,
    Branch,
    Close,
}

fn line_state(line_states: Option<&[ConditionalTruth]>, source_line: usize) -> ConditionalTruth {
    line_states
        .and_then(|states| states.get(source_line).copied())
        .unwrap_or_else(|| {
            if line_states.is_some() {
                ConditionalTruth::Unknown
            } else {
                ConditionalTruth::True
            }
        })
}

fn logical_lines(content: &str) -> Vec<LogicalLine> {
    let mut result = Vec::new();
    let mut pending: Option<(String, usize, bool)> = None;
    for (source_line, physical) in content.lines().enumerate() {
        let (mut line, first_source_line, was_continued) =
            if let Some((previous, first, continued)) = pending.take() {
                (previous, first, continued)
            } else {
                (String::new(), source_line, false)
            };
        let continuation = physical.trim_end().ends_with('\\');
        if line.is_empty() && first_source_line == source_line {
            line.push_str(physical);
        } else {
            line.push_str(physical.trim_start());
        }
        if continuation {
            let without_slash = line.trim_end().trim_end_matches('\\').trim_end();
            pending = Some((format!("{without_slash} "), first_source_line, true));
        } else {
            result.push(LogicalLine {
                text: line,
                source_line: first_source_line,
                continued: was_continued,
            });
        }
    }
    if let Some((text, source_line, _)) = pending {
        result.push(LogicalLine {
            text,
            source_line,
            continued: true,
        });
    }
    result
}

fn gnu_define_directive(line: &str) -> Option<(&'static str, &str)> {
    let mut words = line.splitn(2, char::is_whitespace);
    let word = words.next()?;
    let rest = words.next().unwrap_or_default().trim();
    match word {
        "define" => Some(("define", rest)),
        "endef" if rest.is_empty() => Some(("endef", rest)),
        _ => None,
    }
}

fn is_metamake_define(line: &str) -> bool {
    line.split_whitespace().next() == Some("%define")
}

fn source_file_name(rel_dir: &Path) -> Result<String, String> {
    let relative = path_text(rel_dir);
    if rel_dir.is_absolute()
        || (!relative.is_empty() && relative != "." && !safe_relative_path(&relative))
    {
        return Err("relative directory contains an unsafe or non-normal component".into());
    }
    Ok(if relative.is_empty() || relative == "." {
        "mmakefile.src".into()
    } else {
        format!("{relative}/mmakefile.src")
    })
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn safe_target_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-'))
}

fn contains_shell_syntax(value: &str) -> bool {
    value.contains([';', '\n', '\r', '`', '|', '&', '<', '>', '"', '\'', '\\'])
}

fn rejection(
    owner: &str,
    file: &str,
    line: usize,
    reason: impl Into<String>,
) -> SourceValueRuleRejection {
    SourceValueRuleRejection {
        owner: owner.to_owned(),
        file: file.to_owned(),
        line,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_source_value_rules_with_context, SourceValueRuleDecl, SECOND_SED_SCRIPT};
    use crate::make_vars::{collect_vars, ConditionalTruth};
    use crate::testing::TempTree;
    use std::fs;
    use std::path::Path;

    const MARKER: &str = "#define MODEL_VERSION-MAJOR:";

    fn setup_tree() -> TempTree {
        let tree = TempTree::new();
        fs::create_dir_all(tree.0.join("config")).unwrap();
        fs::create_dir_all(tree.0.join("rom/aros")).unwrap();
        fs::write(
            tree.0.join("config/make.cfg.in"),
            "AROS_DIR_AROS := SYS\n\
             AROS_DIR_PREFS := Prefs\n\
             AROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\n\
             AROS_PREFS := $(AROSDIR)/$(AROS_DIR_PREFS)\n",
        )
        .unwrap();
        fs::write(
            tree.0.join("rom/aros/value.in"),
            format!("{MARKER} 3 /metadata\n"),
        )
        .unwrap();
        tree
    }

    fn valid_source(owner: &str, marker: &str) -> String {
        format!(
            "#MM\n\
             {owner} :\n\
             \t@$(MKDIR) $(AROS_PREFS)/Fixture\n\
             \t@$(SED) -n 's/{marker}// p' < $(SRCDIR)/$(CURDIR)/value.in | $(SED) -n '{SECOND_SED_SCRIPT}' > $(AROS_PREFS)/Fixture/VERSION\n"
        )
    }

    fn scan(
        root: &Path,
        content: &str,
        line_states: Option<&[ConditionalTruth]>,
    ) -> (
        Vec<SourceValueRuleDecl>,
        Vec<super::SourceValueRuleRejection>,
    ) {
        let dirs = crate::dirs::DirVars::load(root);
        let scope = collect_vars(content);
        collect_source_value_rules_with_context(
            content,
            root,
            Path::new("rom/aros"),
            &scope,
            &dirs,
            line_states,
        )
    }

    #[test]
    fn recognizes_arbitrary_owner_marker_and_output_without_interpreting_value() {
        let tree = setup_tree();
        let (declarations, rejected) = scan(&tree.0, &valid_source("read-version", MARKER), None);

        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(declarations.len(), 1);
        assert_eq!(
            declarations[0],
            SourceValueRuleDecl {
                owner: "read-version".into(),
                file: "rom/aros/mmakefile.src".into(),
                line: 2,
                input: "${AROS_SOURCE_DIR}/rom/aros/value.in".into(),
                output: "${AROS_BUILD_DIR}/SYS/Prefs/Fixture/VERSION".into(),
                marker: MARKER.into(),
                file_sha256: String::new(),
            }
        );
    }

    #[test]
    fn recipe_expansion_uses_final_file_scope_for_late_assignments() {
        let tree = setup_tree();
        let content = format!(
            "OUTPUT_PART := Early\n{}OUTPUT_PART := Final\n",
            valid_source("late-scope-owner", MARKER)
                .replace("$(AROS_PREFS)/Fixture", "$(AROS_PREFS)/$(OUTPUT_PART)")
        );
        let (declarations, rejected) = scan(&tree.0, &content, None);

        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(declarations.len(), 1);
        assert_eq!(
            declarations[0].output,
            "${AROS_BUILD_DIR}/SYS/Prefs/Final/VERSION"
        );
    }

    #[test]
    fn prerequisite_inline_and_multiline_variants_are_rejected() {
        let tree = setup_tree();
        let with_prerequisite = valid_source("has-prerequisite", MARKER)
            .replace("has-prerequisite :", "has-prerequisite : input.in");
        let (declarations, rejected) = scan(&tree.0, &with_prerequisite, None);
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("prerequisites")));

        let continued = valid_source("continued-owner", MARKER).replace(
            " $(AROS_PREFS)/Fixture/VERSION\n",
            " $(AROS_PREFS)/Fixture/VERSION \\\n+             \t# continued command\n",
        );
        let (declarations, rejected) = scan(&tree.0, &continued, None);
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("multiline")));

        let inline = valid_source("inline-owner", MARKER)
            .replace("inline-owner :\n\t@$(MKDIR)", "inline-owner : ; @$(MKDIR)");
        let (declarations, rejected) = scan(&tree.0, &inline, None);
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("recipe lines")));
    }

    #[test]
    fn missing_rule_is_absent_and_duplicate_owners_are_rejected() {
        let tree = setup_tree();
        let (declarations, rejected) = scan(&tree.0, "#MM\nnot-a-rule\n", None);
        assert!(declarations.is_empty());
        assert!(rejected.is_empty());

        let source = valid_source("duplicate-owner", MARKER);
        let (declarations, rejected) = scan(&tree.0, &(source.clone() + &source), None);
        assert!(declarations.is_empty());
        assert_eq!(rejected.len(), 2);
        assert!(rejected
            .iter()
            .all(|item| item.reason.contains("duplicate")));
    }

    #[test]
    fn unsafe_output_marker_and_input_traversal_are_rejected() {
        let tree = setup_tree();
        let output_escape = valid_source("output-escape", MARKER)
            .replace("$(AROS_PREFS)/Fixture", "$(TARGETDIR)/Outside")
            .replace(
                "$(AROS_PREFS)/Fixture/VERSION",
                "$(TARGETDIR)/Outside/VERSION",
            );
        let (declarations, rejected) = scan(&tree.0, &output_escape, None);
        assert!(declarations.is_empty());
        assert!(rejected.iter().any(|item| item.reason.contains("output")));

        let bad_marker = valid_source("bad-marker", "#define MODEL.VERSION:");
        let (declarations, rejected) = scan(&tree.0, &bad_marker, None);
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("BRE metacharacter")));

        let traversal = valid_source("traversal-input", MARKER)
            .replace("$(SRCDIR)/$(CURDIR)/value.in", "$(SRCDIR)/../value.in");
        let (declarations, rejected) = scan(&tree.0, &traversal, None);
        assert!(declarations.is_empty());
        assert!(!rejected.is_empty());

        let config_path = tree.0.join("config/make.cfg.in");
        let config = fs::read_to_string(&config_path).unwrap();
        fs::write(
            &config_path,
            config.replace(
                "AROS_PREFS := $(AROSDIR)/$(AROS_DIR_PREFS)",
                "AROS_PREFS := $(TARGETDIR)/Outside",
            ),
        )
        .unwrap();
        let (declarations, rejected) =
            scan(&tree.0, &valid_source("bad-configured-root", MARKER), None);
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("configured `AROS_PREFS`")));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_input_components_are_rejected() {
        use std::os::unix::fs::symlink;

        let tree = setup_tree();
        fs::remove_file(tree.0.join("rom/aros/value.in")).unwrap();
        symlink("/etc/hosts", tree.0.join("rom/aros/value.in")).unwrap();
        let (declarations, rejected) = scan(&tree.0, &valid_source("symlink-owner", MARKER), None);
        assert!(declarations.is_empty());
        assert!(rejected.iter().any(|item| item.reason.contains("symlink")));
    }

    #[test]
    fn unknown_and_false_conditional_states_are_fail_closed_and_defines_are_inert() {
        let tree = setup_tree();
        let source = valid_source("conditional-owner", MARKER);
        let mut unknown = vec![ConditionalTruth::True; source.lines().count()];
        unknown[1] = ConditionalTruth::Unknown;
        unknown[2] = ConditionalTruth::Unknown;
        let (declarations, rejected) = scan(&tree.0, &source, Some(&unknown));
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("undecided")));

        let false_states = vec![ConditionalTruth::False; source.lines().count()];
        let (declarations, rejected) = scan(&tree.0, &source, Some(&false_states));
        assert!(declarations.is_empty());
        assert!(rejected.is_empty());

        let define_body = format!(
            "%define inert\n{}%end\n",
            valid_source("hidden-owner", MARKER)
        );
        let (declarations, rejected) = scan(&tree.0, &define_body, None);
        assert!(declarations.is_empty());
        assert!(rejected.is_empty());

        let define_body = format!(
            "define inert\n{}endef\n",
            valid_source("hidden-gnu-owner", MARKER)
        );
        let (declarations, rejected) = scan(&tree.0, &define_body, None);
        assert!(declarations.is_empty());
        assert!(rejected.is_empty());
    }

    #[test]
    fn local_root_overrides_and_target_specific_scopes_are_rejected() {
        let tree = setup_tree();
        let local_override = format!(
            "AROS_PREFS := $(TARGETDIR)/Elsewhere\n{}",
            valid_source("local-root", MARKER)
        );
        let (declarations, rejected) = scan(&tree.0, &local_override, None);
        assert!(declarations.is_empty());
        assert!(rejected.iter().any(|item| item.reason.contains("override")));

        let scoped = format!(
            "{}\nscoped-owner: PRIVATE_VALUE = unsafe\n",
            valid_source("scoped-owner", MARKER)
        );
        let (declarations, rejected) = scan(&tree.0, &scoped, None);
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("target-specific")));
    }

    #[test]
    fn malformed_and_unanalyzed_conditional_syntax_is_rejected() {
        let tree = setup_tree();
        let source = format!("else\n{}", valid_source("malformed-condition", MARKER));
        let (declarations, rejected) = scan(&tree.0, &source, None);
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("conditional")));

        let source = format!(
            "ifeq ($(UNKNOWN),yes)\n{}endif\n",
            valid_source("unknown-condition", MARKER)
        );
        let (declarations, rejected) = scan(&tree.0, &source, None);
        assert!(declarations.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("conditional")));
    }
}
