//! Closed model for directory-only MetaMake setup targets.
//!
//! A setup target may be represented when each line in its complete recipe is
//! a `%mkdirs_q` invocation, or when it comes from the closed
//! `%rule_makedirs` template call, and every directory maps to a known
//! generated or include root. The scanner does not execute or preserve
//! arbitrary shell syntax; unsupported candidates are returned with their
//! owning target.

use std::path::Path;

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{ConditionalTruth, VarScope};

/// A named MetaMake target that only creates declared output directories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectorySetupDecl {
    /// MetaMake target that owns the directory preparation.
    pub owner: String,
    /// Absolute CMake paths, rooted under the configured build/include roots.
    pub directories: Vec<String>,
}

/// A directory setup candidate that is outside the closed representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectorySetupRejection {
    /// Best-effort owner text from the Make rule header.
    pub owner: String,
    /// Why the rule could not be represented safely.
    pub reason: String,
}

#[derive(Debug)]
struct Rule {
    owner: String,
    owner_state: ConditionalTruth,
    issues: Vec<RuleIssue>,
    recipes: Vec<Recipe>,
}

#[derive(Debug)]
struct Recipe {
    text: String,
    source_line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleIssue {
    UnsupportedPrerequisites,
    ConditionalSyntax,
    UnknownConditional,
}

#[derive(Debug)]
struct LogicalLine {
    text: String,
    source_line: usize,
}

/// Collects only rules whose recipe mentions `%mkdirs_q`.
///
/// The returned declarations and rejections are separate so callers can make
/// unsupported selected targets fail closed without rejecting unrelated
/// recipes in the same makefile. `rel_dir` is the source-relative directory
/// represented by `$(CURDIR)`.
#[must_use]
pub fn collect_directory_setups(
    content: &str,
    rel_dir: &Path,
) -> (Vec<DirectorySetupDecl>, Vec<DirectorySetupRejection>) {
    collect_directory_setups_internal(content, rel_dir, None, None)
}

/// Line-state-aware compatibility wrapper used by the existing unit tests.
/// `line_states` uses the same zero-based line positions as the joined source
/// text. False rule and recipe lines are omitted; an active candidate that
/// touches an unknown owner or recipe line is rejected.
#[cfg(test)]
pub(crate) fn collect_directory_setups_with_line_states(
    content: &str,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<DirectorySetupDecl>, Vec<DirectorySetupRejection>) {
    collect_directory_setups_internal(content, rel_dir, line_states, None)
}

/// Context-aware form for finite directory expressions in recipes.
///
/// Recipe arguments are expanded positionally at the recipe's source line.
/// Only roots whose configured source values match the native CMake layout are
/// mapped to output paths; an mmakefile-local root override is rejected.
#[must_use]
pub(crate) fn collect_directory_setups_with_context(
    content: &str,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<DirectorySetupDecl>, Vec<DirectorySetupRejection>) {
    collect_directory_setups_internal(content, rel_dir, line_states, Some((scope, dirs, root)))
}

fn collect_directory_setups_internal(
    content: &str,
    rel_dir_path: &Path,
    line_states: Option<&[ConditionalTruth]>,
    expression_inputs: Option<(&VarScope, &DirVars, &Path)>,
) -> (Vec<DirectorySetupDecl>, Vec<DirectorySetupRejection>) {
    let rel_dir = source_relative_directory(rel_dir_path);
    let mut rules = Vec::new();
    let mut rejections = Vec::new();
    let mut current: Option<Rule> = None;
    let mut conditional_depth = 0usize;
    let mut define_depth = 0usize;
    let mut metamake_define_depth = 0usize;
    let malformed_conditional_controls = has_malformed_conditional_controls(content);

    for logical in logical_lines(content) {
        let line_state = line_states.map(|states| {
            states
                .get(logical.source_line)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
        });
        // GNU define bodies and MetaMake macro bodies are inert until an
        // explicit eval/expansion. In particular, Make-looking rules,
        // conditionals, and nested macro calls inside them are data rather
        // than source-level declarations.
        let define_directive = (!logical.text.starts_with('\t'))
            .then(|| gnu_define_directive(&logical.text))
            .flatten();
        if define_depth > 0 {
            match define_directive {
                Some(("define", _)) => define_depth = define_depth.saturating_add(1),
                Some(("endef", "")) => define_depth -= 1,
                _ => {}
            }
            continue;
        }
        if let Some(("define", _)) = define_directive {
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
            define_depth = 1;
            continue;
        }

        let uncommented = crate::make_vars::strip_make_comment(logical.text.trim_start()).trim();
        if metamake_define_depth > 0 {
            if is_metamake_define(uncommented) {
                metamake_define_depth = metamake_define_depth.saturating_add(1);
            } else if uncommented == "%end" {
                metamake_define_depth -= 1;
            }
            continue;
        }
        if is_metamake_define(uncommented) {
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
            metamake_define_depth = 1;
            continue;
        }

        if logical.text.starts_with('\t') {
            if let Some(rule) = current.as_mut() {
                if rule.owner_state == ConditionalTruth::False
                    || line_state == Some(ConditionalTruth::False)
                {
                    continue;
                }
                if line_states.is_none() && conditional_depth > 0 {
                    add_issue(rule, RuleIssue::ConditionalSyntax);
                }
                if line_state == Some(ConditionalTruth::Unknown) {
                    add_issue(rule, RuleIssue::UnknownConditional);
                }
                rule.recipes.push(Recipe {
                    text: logical.text.trim().to_owned(),
                    source_line: logical.source_line,
                });
            }
            continue;
        }

        let header = uncommented;
        if header.is_empty() || header.starts_with('#') {
            // GNU make ignores blank lines and comments between a rule and
            // its recipe, so keep the current owner until another statement.
            continue;
        }
        if let Some(directive) = conditional_directive(header) {
            if line_states.is_none() {
                if let Some(rule) = current.as_mut() {
                    add_issue(rule, RuleIssue::ConditionalSyntax);
                }
            }
            match directive {
                ConditionalDirective::Open => conditional_depth += 1,
                ConditionalDirective::Branch => {}
                ConditionalDirective::Close => {
                    conditional_depth = conditional_depth.saturating_sub(1);
                }
            }
            continue;
        }

        if let Some(rule) = current.take() {
            rules.push(rule);
        }
        if let Some(arguments) = rule_makedirs_arguments(header) {
            let state = line_state.unwrap_or(ConditionalTruth::True);
            if state == ConditionalTruth::False {
                continue;
            }
            let (owner, directories) = match parse_rule_makedirs_arguments(arguments) {
                Ok(parsed) => parsed,
                Err(reason) => {
                    rejections.push(DirectorySetupRejection {
                        owner: best_effort_makedirs_owner(arguments),
                        reason,
                    });
                    continue;
                }
            };
            let mut issues = Vec::new();
            if state == ConditionalTruth::Unknown {
                issues.push(RuleIssue::UnknownConditional);
            }
            if malformed_conditional_controls || (line_states.is_none() && conditional_depth > 0) {
                issues.push(RuleIssue::ConditionalSyntax);
            }
            rules.push(Rule {
                owner,
                owner_state: state,
                issues,
                recipes: vec![Recipe {
                    text: format!("%mkdirs_q {directories}"),
                    source_line: logical.source_line,
                }],
            });
            continue;
        }
        let Some((raw_owner, prerequisites)) = header.split_once(':') else {
            continue;
        };
        let raw_owner = raw_owner.trim();
        // Assignments and double-colon rules are not simple named targets.
        if raw_owner.is_empty()
            || prerequisites.starts_with([':', '='])
            || raw_owner.contains(['$', '/', '\\'])
        {
            continue;
        }
        let state = line_state.unwrap_or(ConditionalTruth::True);
        let mut issues = Vec::new();
        if !prerequisites.trim().is_empty() {
            issues.push(RuleIssue::UnsupportedPrerequisites);
        }
        if line_states.is_none() && conditional_depth > 0 {
            issues.push(RuleIssue::ConditionalSyntax);
        }
        if state == ConditionalTruth::Unknown {
            issues.push(RuleIssue::UnknownConditional);
        }
        current = Some(Rule {
            owner: raw_owner.to_owned(),
            owner_state: state,
            issues,
            recipes: Vec::new(),
        });
    }
    if let Some(rule) = current {
        rules.push(rule);
    }

    let mut recipe_blocks_by_owner = std::collections::BTreeMap::<String, usize>::new();
    for rule in &rules {
        if rule.owner_state != ConditionalTruth::False && !rule.recipes.is_empty() {
            *recipe_blocks_by_owner
                .entry(rule.owner.clone())
                .or_default() += 1;
        }
    }

    let mut candidates = Vec::new();
    for rule in rules {
        if rule.owner_state != ConditionalTruth::False
            && rule
                .recipes
                .iter()
                .any(|recipe| recipe.text.contains("%mkdirs_q"))
        {
            candidates.push(rule);
        }
    }

    let mut declarations = Vec::new();
    for rule in candidates {
        let reject = |reason: String| DirectorySetupRejection {
            owner: rule.owner.clone(),
            reason,
        };

        if !safe_target_name(&rule.owner) {
            rejections.push(reject("rule does not have one safe named target".into()));
            continue;
        }
        if recipe_blocks_by_owner
            .get(&rule.owner)
            .copied()
            .unwrap_or(0)
            > 1
        {
            rejections.push(reject(
                "target has multiple recipe-bearing rule blocks".into(),
            ));
            continue;
        }
        if rule.issues.contains(&RuleIssue::UnsupportedPrerequisites) {
            rejections.push(reject(
                "directory setup rule has prerequisites this model does not represent".into(),
            ));
            continue;
        }
        if rule.issues.contains(&RuleIssue::UnknownConditional) {
            rejections.push(reject(
                "owner or recipe line is guarded by an unresolved Make conditional".into(),
            ));
            continue;
        }
        if rule.issues.contains(&RuleIssue::ConditionalSyntax) {
            rejections.push(reject(
                "conditional syntax affects the directory setup rule".into(),
            ));
            continue;
        }
        let mut raw_directories = Vec::new();
        let mut recipe_failure = None;
        for recipe_line in &rule.recipes {
            let recipe = recipe_line
                .text
                .strip_prefix('@')
                .unwrap_or(&recipe_line.text);
            let Some(arguments) = recipe.strip_prefix("%mkdirs_q") else {
                recipe_failure = Some("%mkdirs_q must be the sole recipe command".to_owned());
                break;
            };
            if !arguments.is_empty() && !arguments.chars().next().is_some_and(char::is_whitespace) {
                recipe_failure = Some("%mkdirs_q must be the sole recipe command".to_owned());
                break;
            }
            let raw_arguments = arguments.trim();
            if raw_arguments.is_empty() {
                recipe_failure = Some("%mkdirs_q has no directory arguments".to_owned());
                break;
            }
            if recipe.contains([';', '&', '|', '<', '>', '`']) {
                recipe_failure = Some("%mkdirs_q must be the sole recipe command".to_owned());
                break;
            }

            let line_directories = if let Some((scope, dirs, root)) = expression_inputs {
                let expression_context =
                    MakeExprContext::new(scope, dirs, recipe_line.source_line, root, rel_dir_path);
                if let Err(reason) = validate_active_directory_roots(&expression_context, dirs) {
                    recipe_failure = Some(reason);
                    break;
                }
                let line_directories = match evaluate_make_list(raw_arguments, &expression_context)
                {
                    Ok(directories) => directories,
                    Err(error) => {
                        recipe_failure =
                            Some(format!("cannot resolve %mkdirs_q directory list: {error}"));
                        break;
                    }
                };
                if let Err(reason) =
                    validate_active_developer_lib_root(&expression_context, dirs, &line_directories)
                {
                    recipe_failure = Some(reason);
                    break;
                }
                line_directories
            } else {
                raw_arguments
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect()
            };

            if raw_directories.len().saturating_add(line_directories.len()) > 4096 {
                recipe_failure = Some("%mkdirs_q directory list exceeds 4096 entries".to_owned());
                break;
            }
            raw_directories.extend(line_directories);
        }
        if let Some(reason) = recipe_failure {
            rejections.push(reject(reason));
            continue;
        }

        let mut directories = Vec::new();
        let mut reason = None;
        for raw in raw_directories {
            let mapped = if let Some((_, dirs, _)) = expression_inputs {
                map_configured_directory(&raw, dirs, &rel_dir)
            } else {
                map_directory(&raw, &rel_dir)
            };
            match mapped {
                Ok(directory) => {
                    if !directories.contains(&directory) {
                        directories.push(directory);
                    }
                }
                Err(error) => {
                    reason = Some(error);
                    break;
                }
            }
        }
        if let Some(reason) = reason {
            rejections.push(reject(reason));
        } else if directories.is_empty() {
            rejections.push(reject("%mkdirs_q has no usable directories".into()));
        } else {
            declarations.push(DirectorySetupDecl {
                owner: rule.owner,
                directories,
            });
        }
    }

    (declarations, rejections)
}

#[derive(Clone, Copy)]
enum ConditionalDirective {
    Open,
    Branch,
    Close,
}

fn conditional_directive(line: &str) -> Option<ConditionalDirective> {
    let directive = line.split_whitespace().next()?;
    match directive {
        "ifeq" | "ifneq" | "ifdef" | "ifndef" => Some(ConditionalDirective::Open),
        "else" => Some(ConditionalDirective::Branch),
        "endif" => Some(ConditionalDirective::Close),
        _ => None,
    }
}

fn has_malformed_conditional_controls(content: &str) -> bool {
    let mut conditionals = Vec::<bool>::new();
    let mut define_depth = 0usize;
    let mut metamake_define_depth = 0usize;
    let mut malformed = false;

    for logical in logical_lines(content) {
        let define_directive = (!logical.text.starts_with('\t'))
            .then(|| gnu_define_directive(&logical.text))
            .flatten();
        if define_depth > 0 {
            match define_directive {
                Some(("define", _)) => define_depth = define_depth.saturating_add(1),
                Some(("endef", "")) => define_depth -= 1,
                _ => {}
            }
            continue;
        }
        if matches!(define_directive, Some(("define", _))) {
            define_depth = 1;
            continue;
        }

        let line = crate::make_vars::strip_make_comment(logical.text.trim_start()).trim();
        if metamake_define_depth > 0 {
            if is_metamake_define(line) {
                metamake_define_depth = metamake_define_depth.saturating_add(1);
            } else if line == "%end" {
                metamake_define_depth -= 1;
            }
            continue;
        }
        if is_metamake_define(line) {
            metamake_define_depth = 1;
            continue;
        }

        let Some(directive) = conditional_directive(line) else {
            continue;
        };
        let word = line.split_whitespace().next().unwrap_or_default();
        match directive {
            ConditionalDirective::Open => conditionals.push(false),
            ConditionalDirective::Branch => {
                if word != "else" || line != "else" {
                    malformed = true;
                }
                if let Some(seen_else) = conditionals.last_mut() {
                    if *seen_else {
                        malformed = true;
                    }
                    *seen_else = true;
                } else {
                    malformed = true;
                }
            }
            ConditionalDirective::Close => {
                if word != "endif" || line != "endif" || conditionals.pop().is_none() {
                    malformed = true;
                }
            }
        }
    }
    malformed || !conditionals.is_empty()
}

fn gnu_define_directive(line: &str) -> Option<(&'static str, &str)> {
    let line = crate::make_vars::strip_make_comment(line.trim_start()).trim_start();
    let mut words = line.splitn(2, char::is_whitespace);
    let mut word = words.next().unwrap_or_default();
    let mut rest = words.next().unwrap_or_default().trim();
    let mut modifiers = 0usize;
    while matches!(word, "override" | "export" | "private") && modifiers < 3 {
        modifiers += 1;
        let mut modifier_words = rest.splitn(2, char::is_whitespace);
        word = modifier_words.next().unwrap_or_default();
        rest = modifier_words.next().unwrap_or_default().trim();
    }
    if matches!(word, "define" | "endef") {
        let directive = if word == "define" { "define" } else { "endef" };
        Some((directive, rest))
    } else if modifiers == 3 && matches!(word, "override" | "export" | "private") {
        // An unsupported modifier sequence must not expose assignments or
        // rules that could actually be data in a define body.
        Some(("define", ""))
    } else {
        None
    }
}

fn is_metamake_define(line: &str) -> bool {
    let mut words = line.splitn(2, char::is_whitespace);
    words.next() == Some("%define")
}

fn rule_makedirs_arguments(line: &str) -> Option<&str> {
    let tail = line.strip_prefix("%rule_makedirs")?;
    if tail.is_empty() || tail.starts_with(char::is_whitespace) {
        Some(tail.trim())
    } else {
        None
    }
}

fn parse_rule_makedirs_arguments(raw: &str) -> Result<(String, String), String> {
    if raw.len() > 16 * 1024 {
        return Err("%rule_makedirs arguments exceed 16384 bytes".into());
    }
    let mut arguments = std::collections::BTreeMap::new();
    for word in split_macro_argument_words(raw)? {
        let Some((name, raw_value)) = word.split_once('=') else {
            return Err("%rule_makedirs contains an unnamed or malformed argument".into());
        };
        if !matches!(name, "dirs" | "setuptarget") {
            return Err(format!("%rule_makedirs has unsupported argument `{name}`"));
        }
        if arguments.contains_key(name) {
            return Err(format!("%rule_makedirs repeats argument `{name}`"));
        }
        let value = if raw_value.starts_with('"') || raw_value.starts_with('\'') {
            let quote = raw_value.as_bytes()[0] as char;
            if raw_value.len() < 2
                || !raw_value.ends_with(quote)
                || raw_value[1..raw_value.len() - 1].contains(quote)
            {
                return Err("%rule_makedirs has an unsupported quoted argument".into());
            }
            raw_value[1..raw_value.len() - 1].to_owned()
        } else {
            if raw_value.contains(['"', '\'']) {
                return Err("%rule_makedirs has an unsupported argument quote".into());
            }
            raw_value.to_owned()
        };
        if value.is_empty() {
            return Err(format!("%rule_makedirs {name}= must not be empty"));
        }
        arguments.insert(name.to_owned(), value);
    }
    if arguments.len() != 2 {
        return Err("%rule_makedirs requires exactly dirs= and setuptarget= arguments".into());
    }
    let owner = arguments
        .remove("setuptarget")
        .ok_or_else(|| "%rule_makedirs requires setuptarget=".to_owned())?;
    if !safe_target_name(&owner) {
        return Err("%rule_makedirs setuptarget= must be one literal safe target name".into());
    }
    let directories = arguments
        .remove("dirs")
        .ok_or_else(|| "%rule_makedirs requires dirs=".to_owned())?;
    Ok((owner, directories))
}

fn split_macro_argument_words(raw: &str) -> Result<Vec<String>, String> {
    let bytes = raw.as_bytes();
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut closing = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        let byte = bytes[at];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            at += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            start.get_or_insert(at);
            quote = Some(byte);
            at += 1;
            continue;
        }
        if byte == b'$' && matches!(bytes.get(at + 1), Some(b'(' | b'{')) {
            start.get_or_insert(at);
            closing.push(if bytes[at + 1] == b'(' { b')' } else { b'}' });
            at += 2;
            continue;
        }
        if closing.last() == Some(&byte) {
            closing.pop();
            at += 1;
            continue;
        }
        if byte.is_ascii_whitespace() && closing.is_empty() {
            if let Some(begin) = start.take() {
                words.push(raw[begin..at].to_owned());
                if words.len() > 8 {
                    return Err("%rule_makedirs has too many arguments".into());
                }
            }
            at += 1;
            continue;
        }
        start.get_or_insert(at);
        at += 1;
    }
    if quote.is_some() || !closing.is_empty() {
        return Err("%rule_makedirs has an unterminated quote or Make reference".into());
    }
    if let Some(begin) = start {
        words.push(raw[begin..].to_owned());
    }
    if words.len() > 8 {
        return Err("%rule_makedirs has too many arguments".into());
    }
    Ok(words)
}

fn best_effort_makedirs_owner(raw: &str) -> String {
    split_macro_argument_words(raw)
        .ok()
        .into_iter()
        .flatten()
        .find_map(|word| {
            let (name, value) = word.split_once('=')?;
            if name != "setuptarget" {
                return None;
            }
            let value = value.trim_matches(['"', '\'']);
            safe_target_name(value).then(|| value.to_owned())
        })
        .unwrap_or_else(|| "rule_makedirs".to_owned())
}

fn add_issue(rule: &mut Rule, issue: RuleIssue) {
    if !rule.issues.contains(&issue) {
        rule.issues.push(issue);
    }
}

fn logical_lines(content: &str) -> Vec<LogicalLine> {
    let mut result = Vec::new();
    let mut pending: Option<(String, usize)> = None;
    for (source_line, physical) in content.lines().enumerate() {
        let (line, first_source_line) =
            if let Some((mut previous, first_source_line)) = pending.take() {
                previous.push_str(physical.trim_start());
                (previous, first_source_line)
            } else {
                (physical.to_owned(), source_line)
            };
        let trimmed = line.trim_end();
        if let Some(without_slash) = trimmed.strip_suffix('\\') {
            pending = Some((format!("{} ", without_slash.trim_end()), first_source_line));
        } else {
            result.push(LogicalLine {
                text: line,
                source_line: first_source_line,
            });
        }
    }
    if let Some((text, source_line)) = pending {
        result.push(LogicalLine { text, source_line });
    }
    result
}

fn safe_target_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
}

fn source_relative_directory(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    if value == "." || value.is_empty() {
        return String::new();
    }
    value
}

fn map_directory(raw: &str, rel_dir: &str) -> Result<String, String> {
    let (source_root, cmake_root) = if raw == "$(AROS_LIB)" {
        ("", "${AROS_DEVELOPER_LIB_DIR}")
    } else if let Some(tail) = raw.strip_prefix("$(AROS_LIB)/") {
        (tail, "${AROS_DEVELOPER_LIB_DIR}")
    } else if let Some(tail) = raw.strip_prefix("$(GENDIR)/") {
        (tail, "${AROS_BUILD_DIR}/gen")
    } else if let Some(tail) = raw.strip_prefix("$(GENINCDIR)/") {
        (tail, "${AROS_GENINC_DIR}")
    } else if let Some(tail) = raw.strip_prefix("$(AROS_INCLUDES)/") {
        (tail, "${AROS_DEVELOPER_INCLUDE_DIR}")
    } else {
        return Err(format!(
            "directory `{raw}` is not rooted at GENDIR, GENINCDIR, or AROS_INCLUDES"
        ));
    };

    if source_root.is_empty() {
        return Ok(cmake_root.to_owned());
    }

    let tail = source_root.strip_prefix("$(CURDIR)/").map_or_else(
        || source_root.to_owned(),
        |tail| {
            if rel_dir.is_empty() {
                tail.to_owned()
            } else {
                format!("{rel_dir}/{tail}")
            }
        },
    );
    validate_relative_path(&tail)?;
    Ok(format!("{cmake_root}/{tail}"))
}

fn map_configured_directory(
    expanded: &str,
    dirs: &DirVars,
    rel_dir: &str,
) -> Result<String, String> {
    let gendir = dirs
        .expand("$(GENDIR)")
        .ok_or_else(|| "configured GENDIR root cannot be resolved".to_owned())?;
    if gendir != "${AROS_BUILD_DIR}/gen" {
        return Err(format!(
            "configured GENDIR root `{gendir}` differs from the native generated root"
        ));
    }

    let geninc = dirs
        .expand("$(GENINCDIR)")
        .ok_or_else(|| "configured GENINCDIR root cannot be resolved".to_owned())?;
    let expected_geninc = dirs
        .expand("$(GENDIR)/include")
        .ok_or_else(|| "source-derived GENINCDIR mapping cannot be resolved".to_owned())?;
    if geninc != expected_geninc {
        return Err(format!(
            "configured GENINCDIR root `{geninc}` differs from source mapping `{expected_geninc}`"
        ));
    }

    let sdk_include = dirs
        .expand("$(AROS_INCLUDES)")
        .ok_or_else(|| "configured AROS_INCLUDES root cannot be resolved".to_owned())?;
    let expected_sdk_include = dirs
        .expand("$(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)")
        .ok_or_else(|| "source-derived AROS_INCLUDES mapping cannot be resolved".to_owned())?;
    if sdk_include != expected_sdk_include {
        return Err(format!(
            "configured AROS_INCLUDES root `{sdk_include}` differs from source mapping `{expected_sdk_include}`"
        ));
    }

    let developer_lib = dirs.expand("$(AROS_DEVELOPER)/$(AROS_DIR_LIB)");
    let mut roots = vec![
        (sdk_include, "${AROS_SDK_INCLUDE_DIR}"),
        (geninc, "${AROS_GENINC_DIR}"),
        (gendir, "${AROS_BUILD_DIR}/gen"),
    ];
    if let Some(developer_lib) = developer_lib {
        roots.push((developer_lib, "${AROS_DEVELOPER_LIB_DIR}"));
    }
    for (root, _) in &roots {
        if root.is_empty() || root.ends_with('/') {
            return Err(format!(
                "configured directory root `{root}` is not canonical"
            ));
        }
    }
    for (index, (left, _)) in roots.iter().enumerate() {
        if roots[index + 1..].iter().any(|(right, _)| left == right) {
            return Err(format!(
                "configured directory roots contain an ambiguous mapping for `{left}`"
            ));
        }
    }

    let mut matches = roots
        .iter()
        .filter_map(|(root, cmake_root)| {
            if expanded == root {
                Some((root.len(), *cmake_root, ""))
            } else {
                expanded
                    .strip_prefix(root)
                    .and_then(|tail| tail.strip_prefix('/'))
                    .map(|tail| (root.len(), *cmake_root, tail))
            }
        })
        .collect::<Vec<_>>();
    let Some(longest) = matches.iter().map(|(length, _, _)| *length).max() else {
        return Err(format!(
            "directory `{expanded}` is not rooted at a configured GENDIR, GENINCDIR, or AROS_INCLUDES source value"
        ));
    };
    matches.retain(|(length, _, _)| *length == longest);
    if matches.len() != 1 {
        return Err(format!(
            "directory `{expanded}` has an ambiguous configured-root mapping"
        ));
    }
    let (_, cmake_root, tail) = matches[0];
    // Make renders CURDIR as `.` for a root-level declaring makefile. Only
    // normalize that leading current-directory component in the root case.
    let tail = if rel_dir.is_empty() {
        tail.strip_prefix("./").unwrap_or(tail)
    } else {
        tail
    };
    let tail = tail.to_owned();
    if tail.is_empty() {
        return Ok(cmake_root.to_owned());
    }
    validate_relative_path(&tail)?;
    Ok(format!("{cmake_root}/{tail}"))
}

fn validate_active_directory_roots(
    context: &MakeExprContext<'_>,
    dirs: &DirVars,
) -> Result<(), String> {
    for name in ["GENDIR", "GENINCDIR", "AROS_INCLUDES"] {
        let expression = format!("$({name})");
        let expected = dirs
            .expand(&expression)
            .ok_or_else(|| format!("source-configured {name} root cannot be resolved"))?;
        let actual = evaluate_make_expr(&expression, context)
            .map_err(|error| format!("cannot validate active {name} root: {error}"))?;
        if actual != expected {
            return Err(format!(
                "active {name} root `{actual}` differs from source-configured root `{expected}`"
            ));
        }
    }
    Ok(())
}

fn validate_active_developer_lib_root(
    context: &MakeExprContext<'_>,
    dirs: &DirVars,
    directories: &[String],
) -> Result<(), String> {
    let Ok(actual) = evaluate_make_expr("$(AROS_LIB)", context) else {
        return Ok(());
    };
    if !directories
        .iter()
        .any(|directory| path_is_within_root(directory, &actual))
    {
        return Ok(());
    }
    let expected = dirs.expand("$(AROS_DEVELOPER)/$(AROS_DIR_LIB)");
    if expected.as_deref() != Some(actual.as_str()) {
        let Some(expected) = expected else {
            return Err("source-derived AROS_LIB mapping cannot be resolved".to_owned());
        };
        return Err(format!(
            "active AROS_LIB root `{actual}` differs from source-configured root `{expected}`"
        ));
    }
    Ok(())
}

fn path_is_within_root(path: &str, root: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/') && !root.ends_with('/'))
}

fn validate_relative_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("directory path is empty".into());
    }
    if path.contains([
        '$', '\\', ';', '"', '\'', '`', '|', '&', '<', '>', '*', '?', '[', ']',
    ]) {
        return Err(format!(
            "directory path `{path}` contains unsupported syntax"
        ));
    }
    let components: Vec<&str> = path.split('/').collect();
    if components.iter().any(|component| {
        component.is_empty()
            || *component == "."
            || *component == ".."
            || !component
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
    }) {
        return Err(format!(
            "directory path `{path}` is not a safe relative path"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars;
    use crate::parser::TargetContext;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn collects_arbitrary_setup_targets_and_resolves_curdir() {
        let source = "#MM\nkernel-clocksource-gen-setup:\n\t%mkdirs_q $(GENDIR)/$(CURDIR)/include/clib $(GENDIR)/$(CURDIR)/include/defines $(GENDIR)/$(CURDIR)/include/inline $(GENDIR)/$(CURDIR)/include/proto\nrenamed-setup:\n\t@%mkdirs_q $(GENINCDIR)/headers\n";
        let (decls, rejected) = collect_directory_setups(source, Path::new("rom/kernel"));

        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].owner, "kernel-clocksource-gen-setup");
        assert_eq!(
            decls[0].directories,
            [
                "${AROS_BUILD_DIR}/gen/rom/kernel/include/clib",
                "${AROS_BUILD_DIR}/gen/rom/kernel/include/defines",
                "${AROS_BUILD_DIR}/gen/rom/kernel/include/inline",
                "${AROS_BUILD_DIR}/gen/rom/kernel/include/proto",
            ]
        );
        assert_eq!(decls[1].owner, "renamed-setup");
        assert_eq!(decls[1].directories, ["${AROS_GENINC_DIR}/headers"]);
    }

    #[test]
    fn collects_multiple_pure_mkdirs_lines_with_line_local_values() {
        let source = "OBJDIR := $(GENDIR)/$(CURDIR)\nsetup:\n\t%mkdirs_q $(OBJDIR)\n\t%mkdirs_q $(AROS_LIB)/libopenurl\n";
        let (decls, rejected) =
            collect_with_context(source, Path::new("external/openurl/libopenurl"), None);

        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].owner, "setup");
        assert_eq!(
            decls[0].directories,
            [
                "${AROS_BUILD_DIR}/gen/external/openurl/libopenurl",
                "${AROS_DEVELOPER_LIB_DIR}/libopenurl",
            ]
        );
    }

    #[test]
    fn rejects_non_closed_directory_recipes_with_their_owner() {
        let cases = [
            (
                "extra-command:\n\t%mkdirs_q $(GENDIR)/safe && touch marker\n",
                "sole recipe command",
            ),
            (
                "extra-line:\n\t%mkdirs_q $(GENDIR)/safe\n\techo unexpected\n",
                "sole recipe command",
            ),
            (
                "traversal:\n\t%mkdirs_q $(GENDIR)/../../outside\n",
                "safe relative path",
            ),
            (
                "shell-syntax:\n\t%mkdirs_q $(GENINCDIR)/safe;echo\n",
                "sole recipe command",
            ),
            ("empty:\n\t%mkdirs_q\n", "no directory arguments"),
            (
                "unknown-root:\n\t%mkdirs_q $(OTHER_ROOT)/safe\n",
                "not rooted",
            ),
            (
                "has-prerequisite: input\n\t%mkdirs_q $(GENDIR)/safe\n",
                "prerequisites",
            ),
            (
                "duplicated-owner:\n\t%mkdirs_q $(GENDIR)/safe\nduplicated-owner:\n\techo unexpected\n",
                "multiple recipe-bearing",
            ),
            (
                "conditional:\nifeq ($(MODE),yes)\n\t%mkdirs_q $(GENDIR)/safe\nendif\n",
                "conditional syntax",
            ),
        ];

        for (source, expected_reason) in cases {
            let (decls, rejected) = collect_directory_setups(source, Path::new("rom/kernel"));
            assert!(decls.is_empty(), "unexpected declaration for {source:?}");
            assert_eq!(rejected.len(), 1, "{source:?}: {rejected:?}");
            assert!(rejected[0].reason.contains(expected_reason), "{rejected:?}");
            assert!(!rejected[0].owner.is_empty(), "{rejected:?}");
        }
    }

    #[test]
    fn ignores_file_targets_and_rejects_invalid_named_owners() {
        let file_target = "$(GENDIR)/generated/.stamp:\n\t%mkdirs_q $(GENDIR)/generated\n";
        let (decls, rejected) = collect_directory_setups(file_target, Path::new("rom/kernel"));
        assert!(decls.is_empty());
        assert!(
            rejected.is_empty(),
            "file-producing rule leaked into setup scan"
        );

        let invalid_owner = "not a target name:\n\t%mkdirs_q $(GENDIR)/safe\n";
        let (decls, rejected) = collect_directory_setups(invalid_owner, Path::new("rom/kernel"));
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert_eq!(rejected[0].owner, "not a target name");
    }

    #[test]
    fn comments_and_blank_lines_do_not_detach_make_recipes() {
        let source = "setup:\n# a make comment\n\n\t%mkdirs_q $(GENDIR)/safe\n";
        let (decls, rejected) = collect_directory_setups(source, Path::new("rom/kernel"));
        assert!(rejected.is_empty(), "{rejected:?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].owner, "setup");
        assert_eq!(decls[0].directories, ["${AROS_BUILD_DIR}/gen/safe"]);
    }

    #[test]
    fn line_states_omit_false_rules_and_reject_unknown_candidate_lines() {
        let source = "inactive:\n\t%mkdirs_q $(GENDIR)/ignored\nactive:\n\t%mkdirs_q $(GENINCDIR)/safe\nunknown-owner:\n\t%mkdirs_q $(AROS_INCLUDES)/uncertain\n";
        let states = [
            ConditionalTruth::False,
            ConditionalTruth::False,
            ConditionalTruth::True,
            ConditionalTruth::True,
            ConditionalTruth::Unknown,
            ConditionalTruth::True,
        ];
        let (decls, rejected) = collect_directory_setups_with_line_states(
            source,
            Path::new("rom/kernel"),
            Some(&states),
        );
        assert_eq!(decls.len(), 1, "{decls:?}");
        assert_eq!(decls[0].owner, "active");
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert_eq!(rejected[0].owner, "unknown-owner");
        assert!(rejected[0].reason.contains("unresolved"), "{rejected:?}");

        let source = "known-owner:\n\t%mkdirs_q $(GENDIR)/safe\n\techo maybe\n";
        let states = [
            ConditionalTruth::True,
            ConditionalTruth::True,
            ConditionalTruth::Unknown,
        ];
        let (decls, rejected) = collect_directory_setups_with_line_states(
            source,
            Path::new("rom/kernel"),
            Some(&states),
        );
        assert!(decls.is_empty(), "unknown command was dropped: {decls:?}");
        assert_eq!(rejected.len(), 1, "{rejected:?}");
        assert_eq!(rejected[0].owner, "known-owner");
        assert!(rejected[0].reason.contains("unresolved"), "{rejected:?}");
    }

    #[test]
    fn context_expands_finite_directory_lists_including_bare_roots() {
        let source = "DIRS := $(AROS_INCLUDES) $(AROS_INCLUDES)/arpa $(GENINCDIR) $(GENINCDIR)/net $(GENDIR) $(GENDIR)/drivers\nsetup:\n\t%mkdirs_q $(DIRS)\n";
        let (decls, rejected) = collect_with_context(source, Path::new("workbench/example"), None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(
            decls[0].directories,
            [
                "${AROS_SDK_INCLUDE_DIR}",
                "${AROS_SDK_INCLUDE_DIR}/arpa",
                "${AROS_GENINC_DIR}",
                "${AROS_GENINC_DIR}/net",
                "${AROS_BUILD_DIR}/gen",
                "${AROS_BUILD_DIR}/gen/drivers",
            ]
        );
    }

    #[test]
    fn context_directory_variables_are_evaluated_at_recipe_line() {
        let source = "DIRS := $(AROS_INCLUDES)/before\nsetup:\n\t%mkdirs_q $(DIRS)\nDIRS := $(GENINCDIR)/after\n";
        let (decls, rejected) = collect_with_context(source, Path::new("workbench/example"), None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].directories, ["${AROS_SDK_INCLUDE_DIR}/before"]);
    }

    #[test]
    fn source_shaped_rule_makedirs_resolves_directory_snapshot_and_developer_lib() {
        let source = "STARTUP_DIRS := $(GENDIR)/$(CURDIR) $(GENDIR)/$(CURDIR)/nix $(GENDIR)/$(CURDIR)/cxx $(AROS_LIB)\n%rule_makedirs dirs=\"$(STARTUP_DIRS)\" setuptarget=linklibs-startup-setup\n";
        let (decls, rejected) = collect_with_context(source, Path::new("compiler/startup"), None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].owner, "linklibs-startup-setup");
        assert_eq!(
            decls[0].directories,
            [
                "${AROS_BUILD_DIR}/gen/compiler/startup",
                "${AROS_BUILD_DIR}/gen/compiler/startup/nix",
                "${AROS_BUILD_DIR}/gen/compiler/startup/cxx",
                "${AROS_DEVELOPER_LIB_DIR}",
            ]
        );
    }

    #[test]
    fn rule_makedirs_uses_target_line_states_and_ignores_inactive_define_bodies() {
        let context = TargetContext {
            cpu32: Some(String::new()),
            ..TargetContext::default()
        };
        let source = "ifneq ($(AROS_TARGET_CPU32),)\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=cpu32-only\nelse\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=selected-startup\nendif\ndefine OUTER\nifeq (1,0)\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=hidden-outer\noverride export private define INNER\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=hidden-inner\nendef\nendif\nendef\n";
        let (decls, rejected) =
            collect_with_target_context(source, Path::new("compiler/startup"), &context);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1, "{decls:#?}");
        assert_eq!(decls[0].owner, "selected-startup");

        let source = "ifeq ($(AROS_TARGET_CPU32),)\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=selected\nelse\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=inactive\nendif\n";
        let (decls, rejected) =
            collect_with_target_context(source, Path::new("compiler/startup"), &context);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].owner, "selected");
    }

    #[test]
    fn rule_makedirs_rejects_unknown_conditionals_opaque_values_and_bad_arguments() {
        let unknown = "ifeq ($(UNKNOWN_MODE),enabled)\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=maybe\nendif\n";
        let (decls, rejected) = collect_with_target_context(
            unknown,
            Path::new("compiler/startup"),
            &TargetContext::default(),
        );
        assert!(decls.is_empty(), "{decls:#?}; rejected={rejected:#?}");
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(rejected[0].reason.contains("unresolved Make conditional"));

        let unresolved = "%rule_makedirs dirs=$(MISSING_DIRS) setuptarget=missing\n";
        let (decls, rejected) =
            collect_with_context(unresolved, Path::new("compiler/startup"), None);
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(rejected[0].reason.contains("cannot resolve %mkdirs_q"));

        let opaque = "define STARTUP_DIRS\n$(AROS_LIB)\nendef\n%rule_makedirs dirs=\"$(STARTUP_DIRS)\" setuptarget=opaque\n";
        let (decls, rejected) = collect_with_context(opaque, Path::new("compiler/startup"), None);
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(rejected[0].reason.contains("cannot resolve %mkdirs_q"));

        let frozen_alias = "STARTUP_DIRS := $(AROS_LIB)\ndefine AROS_LIB\nunsafe\nendef\n%rule_makedirs dirs=\"$(STARTUP_DIRS)\" setuptarget=frozen-alias\n";
        let (decls, rejected) =
            collect_with_context(frozen_alias, Path::new("compiler/startup"), None);
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(rejected[0]
            .reason
            .contains("unsafe Make variable `AROS_LIB`"));

        let reset = "define STARTUP_DIRS\nunsafe\nendef\nSTARTUP_DIRS := $(AROS_LIB)\n%rule_makedirs dirs=\"$(STARTUP_DIRS)\" setuptarget=reset-startup\n";
        let (decls, rejected) = collect_with_context(reset, Path::new("compiler/startup"), None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].owner, "reset-startup");
        assert_eq!(decls[0].directories, ["${AROS_DEVELOPER_LIB_DIR}"]);

        let redirected_library = "AROS_LIB := $(GENDIR)/foreign\nSTARTUP_DIRS := $(AROS_LIB)\n%rule_makedirs dirs=\"$(STARTUP_DIRS)\" setuptarget=redirected-library\n";
        let (decls, rejected) =
            collect_with_context(redirected_library, Path::new("compiler/startup"), None);
        assert!(decls.is_empty(), "{decls:#?}; rejected={rejected:#?}");
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(
            rejected[0]
                .reason
                .contains("differs from source-configured root"),
            "{rejected:#?}"
        );

        for (source, expected_reason) in [
            (
                "%rule_makedirs dirs=$(AROS_LIB)\n",
                "requires exactly dirs= and setuptarget=",
            ),
            (
                "%rule_makedirs dirs=$(AROS_LIB) setuptarget=setup extra=value\n",
                "unsupported argument `extra`",
            ),
            (
                "%rule_makedirs dirs=$(AROS_LIB) setuptarget=$(TARGET)\n",
                "literal safe target name",
            ),
            (
                "%rule_makedirs dirs=\"$(AROS_LIB) setuptarget=setup\n",
                "unterminated quote",
            ),
            (
                "%rule_makedirs dirs=$(AROS_LIB) setuptarget=before-open\nifeq (1,1)\n",
                "conditional syntax affects",
            ),
            (
                "ifeq (1,1)\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=inside-open\n",
                "conditional syntax affects",
            ),
            (
                "else\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=after-stray-else\n",
                "conditional syntax affects",
            ),
            (
                "endif\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=after-stray-endif\n",
                "conditional syntax affects",
            ),
            (
                "ifeq (1,1)\nelse\nelse\nendif\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=after-duplicate-else\n",
                "conditional syntax affects",
            ),
        ] {
            let (decls, rejected) =
                collect_with_context(source, Path::new("compiler/startup"), None);
            assert!(decls.is_empty(), "{source:?}: {decls:#?}");
            assert_eq!(rejected.len(), 1, "{source:?}: {rejected:#?}");
            assert!(
                rejected[0].reason.contains(expected_reason),
                "{source:?}: {rejected:#?}"
            );
        }
    }

    #[test]
    fn context_free_rule_makedirs_accepts_only_the_explicit_developer_lib_root() {
        let (decls, rejected) = collect_directory_setups(
            "%rule_makedirs dirs=$(AROS_LIB) setuptarget=library-setup\n",
            Path::new("compiler/startup"),
        );
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].owner, "library-setup");
        assert_eq!(decls[0].directories, ["${AROS_DEVELOPER_LIB_DIR}"]);

        let (decls, rejected) = collect_directory_setups(
            "%rule_makedirs dirs=$(UNCONFIGURED_LIB) setuptarget=unknown-library\n",
            Path::new("compiler/startup"),
        );
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].reason.contains("not rooted"));

        let (decls, rejected) = collect_directory_setups(
            "%define SETUP_BODY\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=hidden\n%end\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=visible\n",
            Path::new("compiler/startup"),
        );
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].owner, "visible");

        for malformed_body in [
            "define OPEN\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=hidden\n",
            "define BAD_END\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=hidden\nendef extra\n%rule_makedirs dirs=$(AROS_LIB) setuptarget=also-hidden\n",
        ] {
            let (decls, rejected) =
                collect_directory_setups(malformed_body, Path::new("compiler/startup"));
            assert!(decls.is_empty(), "{malformed_body:?}: {decls:#?}");
            assert!(rejected.is_empty(), "{malformed_body:?}: {rejected:#?}");
        }
    }

    #[test]
    fn context_rule_makedirs_does_not_require_unused_developer_lib_configuration() {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/make.cfg.in"),
            "AROS_DIR_DEVELOPER := Developer\nAROS_DIR_INCLUDE := include\nAROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\nGENINCDIR := $(GENDIR)/include\n",
        )
        .unwrap();
        let source = "%rule_makedirs dirs=$(GENDIR)/$(CURDIR)/objects setuptarget=generated-only\n";
        let scope = collect_vars(source);
        let dirs = DirVars::load(&root);
        let (decls, rejected) = collect_directory_setups_with_context(
            source,
            Path::new("compiler/startup"),
            &scope,
            &dirs,
            &root,
            None,
        );
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(
            decls[0].directories,
            ["${AROS_BUILD_DIR}/gen/compiler/startup/objects"]
        );

        let uses_unconfigured_library =
            "%rule_makedirs dirs=$(AROS_LIB) setuptarget=missing-library-root\n";
        let scope = collect_vars(uses_unconfigured_library);
        let (decls, rejected) = collect_directory_setups_with_context(
            uses_unconfigured_library,
            Path::new("compiler/startup"),
            &scope,
            &dirs,
            &root,
            None,
        );
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(
            rejected[0]
                .reason
                .contains("cannot resolve %mkdirs_q directory list"),
            "{rejected:#?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn context_rejects_missing_unsafe_and_conditionally_assigned_lists() {
        let cases = [
            (
                "setup:\n\t%mkdirs_q $(MISSING_DIRS)\n",
                "cannot resolve %mkdirs_q directory list",
            ),
            (
                "DIRS := $(AROS_INCLUDES)/../outside\nsetup:\n\t%mkdirs_q $(DIRS)\n",
                "safe relative path",
            ),
            (
                "ifeq ($(UNKNOWN_MODE),1)\nDIRS := $(AROS_INCLUDES)/optional\nendif\nsetup:\n\t%mkdirs_q $(DIRS)\n",
                "cannot resolve %mkdirs_q directory list",
            ),
        ];
        for (source, expected_reason) in cases {
            let (decls, rejected) =
                collect_with_context(source, Path::new("workbench/example"), None);
            assert!(decls.is_empty(), "unexpected declarations for {source:?}");
            assert_eq!(rejected.len(), 1, "{source:?}: {rejected:#?}");
            assert!(
                rejected[0].reason.contains(expected_reason),
                "{rejected:#?}"
            );
        }
    }

    #[test]
    fn context_rejects_source_root_overrides_and_oversized_lists() {
        let overridden = "AROS_INCLUDES := $(GENDIR)/foreign\nDIRS := $(AROS_INCLUDES)/headers\nsetup:\n\t%mkdirs_q $(DIRS)\n";
        let (decls, rejected) =
            collect_with_context(overridden, Path::new("workbench/example"), None);
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(rejected[0]
            .reason
            .contains("differs from source-configured"));

        // Exercise this collector's entry cap independently of the shared
        // expression evaluator's earlier expansion/scan limits.
        let directories = std::iter::repeat_n("headers", 4097)
            .collect::<Vec<_>>()
            .join(" ");
        let oversized = format!("setup:\n\t%mkdirs_q {directories}\n");
        let (decls, rejected) =
            collect_with_context(&oversized, Path::new("workbench/example"), None);
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(rejected[0].reason.contains("exceeds 4096"));
    }

    #[test]
    fn context_rejects_a_local_root_override_used_only_by_the_second_recipe_line() {
        let source = "AROS_LIB := $(GENDIR)/foreign\nsetup:\n\t%mkdirs_q $(GENDIR)/$(CURDIR)/objects\n\t%mkdirs_q $(AROS_LIB)/archives\n";
        let (decls, rejected) = collect_with_context(source, Path::new("external/openurl"), None);

        assert!(decls.is_empty(), "{decls:#?}");
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(
            rejected[0]
                .reason
                .contains("differs from source-configured root"),
            "{rejected:#?}"
        );
    }

    #[test]
    fn context_bounds_directory_count_across_recipe_lines() {
        let first_line = std::iter::repeat_n("$(AROS_INCLUDES)/headers", 2048)
            .collect::<Vec<_>>()
            .join(" ");
        let second_line = std::iter::repeat_n("$(AROS_INCLUDES)/headers", 2049)
            .collect::<Vec<_>>()
            .join(" ");
        let source = format!("setup:\n\t%mkdirs_q {first_line}\n\t%mkdirs_q {second_line}\n");
        let (decls, rejected) = collect_with_context(&source, Path::new("workbench/example"), None);

        assert!(decls.is_empty(), "{decls:#?}");
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert!(rejected[0].reason.contains("exceeds 4096"), "{rejected:#?}");
    }

    fn collect_with_context(
        source: &str,
        rel_dir: &Path,
        line_states: Option<&[ConditionalTruth]>,
    ) -> (Vec<DirectorySetupDecl>, Vec<DirectorySetupRejection>) {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/make.cfg.in"),
            "AROS_DIR_DEVELOPER := Developer\nAROS_DIR_INCLUDE := include\nAROS_DIR_LIB := lib\nAROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\nAROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\nGENINCDIR := $(GENDIR)/include\n",
        )
        .unwrap();
        let scope = collect_vars(source);
        let dirs = DirVars::load(&root);
        let result = collect_directory_setups_with_context(
            source,
            rel_dir,
            &scope,
            &dirs,
            &root,
            line_states,
        );
        fs::remove_dir_all(root).unwrap();
        result
    }

    fn collect_with_target_context(
        source: &str,
        rel_dir: &Path,
        target_context: &TargetContext,
    ) -> (Vec<DirectorySetupDecl>, Vec<DirectorySetupRejection>) {
        let root = unique_temp_dir();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/make.cfg.in"),
            "AROS_DIR_DEVELOPER := Developer\nAROS_DIR_INCLUDE := include\nAROS_DIR_LIB := lib\nAROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\nAROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\nGENINCDIR := $(GENDIR)/include\n",
        )
        .unwrap();
        let (scope, line_states) =
            crate::make_vars::collect_vars_impl(source, Some(target_context));
        let dirs = DirVars::load(&root);
        let result = collect_directory_setups_with_context(
            source,
            rel_dir,
            &scope,
            &dirs,
            &root,
            Some(&line_states),
        );
        fs::remove_dir_all(root).unwrap();
        result
    }

    #[test]
    fn cmake_helper_materializes_only_contained_directories() {
        if Command::new("cmake").arg("--version").output().is_err() {
            eprintln!("cmake is unavailable; skipping CMake fixture");
            return;
        }

        let fixture = unique_temp_dir();
        fs::create_dir_all(&fixture).unwrap();
        let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../aros-cmake-engine/engine/DirectorySetup.cmake");
        let cmakelists = format!(
            "cmake_minimum_required(VERSION 3.24)\nproject(directory_setup NONE)\ninclude(\"{}\")\nset(AROS_BUILD_DIR \"${{CMAKE_BINARY_DIR}}/build-root\")\nset(AROS_GENINC_DIR \"${{CMAKE_BINARY_DIR}}/geninc\")\nset(AROS_DEVELOPER_INCLUDE_DIR \"${{CMAKE_BINARY_DIR}}/developer/include\")\nset(AROS_DEVELOPER_LIB_DIR \"${{CMAKE_BINARY_DIR}}/developer/lib\")\naros_prepare_directories(NAME renamed-setup DIRECTORIES \"${{AROS_BUILD_DIR}}/gen/rom/kernel/include/proto\" \"${{AROS_GENINC_DIR}}/headers\" \"${{AROS_DEVELOPER_INCLUDE_DIR}}/headers\" \"${{AROS_DEVELOPER_LIB_DIR}}/archives\")\n",
            helper.display()
        );
        fs::write(fixture.join("CMakeLists.txt"), cmakelists).unwrap();

        let configure = Command::new("cmake")
            .arg("-S")
            .arg(&fixture)
            .arg("-B")
            .arg(fixture.join("build"))
            .output()
            .unwrap();
        assert!(
            configure.status.success(),
            "cmake configure failed:\n{}",
            String::from_utf8_lossy(&configure.stderr)
        );
        let build = Command::new("cmake")
            .arg("--build")
            .arg(fixture.join("build"))
            .arg("--target")
            .arg("renamed-setup")
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "cmake build failed:\n{}",
            String::from_utf8_lossy(&build.stderr)
        );
        for directory in [
            "build-root/gen/rom/kernel/include/proto",
            "geninc/headers",
            "developer/include/headers",
            "developer/lib/archives",
        ] {
            assert!(
                fixture.join("build").join(directory).is_dir(),
                "{directory}"
            );
        }

        let unsafe_fixture = fixture.join("unsafe");
        fs::create_dir_all(&unsafe_fixture).unwrap();
        fs::write(
            unsafe_fixture.join("CMakeLists.txt"),
            format!(
                "cmake_minimum_required(VERSION 3.24)\nproject(directory_setup_unsafe NONE)\ninclude(\"{}\")\nset(AROS_BUILD_DIR \"${{CMAKE_BINARY_DIR}}/build-root\")\nset(AROS_GENINC_DIR \"${{CMAKE_BINARY_DIR}}/geninc\")\nset(AROS_DEVELOPER_INCLUDE_DIR \"${{CMAKE_BINARY_DIR}}/developer/include\")\naros_prepare_directories(NAME unsafe-setup DIRECTORIES \"${{AROS_BUILD_DIR}}/../outside\")\n",
                helper.display()
            ),
        )
        .unwrap();
        let unsafe_configure = Command::new("cmake")
            .arg("-S")
            .arg(&unsafe_fixture)
            .arg("-B")
            .arg(unsafe_fixture.join("build"))
            .output()
            .unwrap();
        assert!(!unsafe_configure.status.success());
        let diagnostics = String::from_utf8_lossy(&unsafe_configure.stderr);
        assert!(
            diagnostics.contains("escapes configured roots"),
            "{diagnostics}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let symlink_fixture = fixture.join("symlink");
            let symlink_build = symlink_fixture.join("build");
            let linked_root = symlink_build.join("build-root");
            fs::create_dir_all(&symlink_fixture).unwrap();
            fs::create_dir_all(&symlink_build).unwrap();
            fs::create_dir_all(symlink_fixture.join("outside")).unwrap();
            fs::create_dir_all(&linked_root).unwrap();
            symlink(symlink_fixture.join("outside"), linked_root.join("link")).unwrap();
            fs::write(
                symlink_fixture.join("CMakeLists.txt"),
                format!(
                    "cmake_minimum_required(VERSION 3.24)\nproject(directory_setup_symlink NONE)\ninclude(\"{}\")\nset(AROS_BUILD_DIR \"${{CMAKE_BINARY_DIR}}/build-root\")\nset(AROS_GENINC_DIR \"${{CMAKE_BINARY_DIR}}/geninc\")\nset(AROS_DEVELOPER_INCLUDE_DIR \"${{CMAKE_BINARY_DIR}}/developer/include\")\naros_prepare_directories(NAME symlink-setup DIRECTORIES \"${{AROS_BUILD_DIR}}/link/escaped\")\n",
                    helper.display()
                ),
            )
            .unwrap();
            let symlink_configure = Command::new("cmake")
                .arg("-S")
                .arg(&symlink_fixture)
                .arg("-B")
                .arg(&symlink_build)
                .output()
                .unwrap();
            assert!(!symlink_configure.status.success());
            let diagnostics = String::from_utf8_lossy(&symlink_configure.stderr);
            assert!(diagnostics.contains("crosses a symlink"), "{diagnostics}");
        }

        fs::remove_dir_all(&fixture).unwrap();
    }

    fn unique_temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_nanos();
        let unique_id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "aros-directory-setup-{}-{nanos}-{unique_id}",
            std::process::id()
        ))
    }
}
