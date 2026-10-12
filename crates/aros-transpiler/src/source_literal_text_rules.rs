//! Closed scanner for source-owned Make recipes that write literal text files.
//!
//! This capability models one output below the declaring source directory's
//! generated build-tree directory. It accepts only an exact `#MM` aggregate,
//! the output's own source `mmakefile.src` prerequisite, one directory command,
//! and bounded `$(ECHO)` writes. Make is never executed.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{variable_assignment, ConditionalTruth, VarScope};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_RULES: usize = 16_384;
const MAX_REJECTIONS: usize = 512;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_LINES: usize = 256;
const BUILD_ALIAS: &str = "${AROS_BUILD_DIR}";
const SOURCE_ALIAS: &str = "${AROS_SOURCE_DIR}";

/// One source-owned file rendered from ordered literal echo lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLiteralTextRuleDecl {
    /// Named, nonvirtual Make provider owning the generated output.
    pub owner: String,
    /// Source-relative joined mmakefile path.
    pub file: String,
    /// One-based source line of the bare `#MM` marker.
    pub line: usize,
    /// Output path below `${AROS_BUILD_DIR}/gen/<declaring-directory>`.
    pub output: String,
    /// Exact bytes written by the echo recipe, including each final newline.
    pub text: String,
    /// Filled by parser integration from the original source snapshot.
    pub file_sha256: String,
}

/// A literal text producer outside the closed source-owned contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLiteralTextRuleRejection {
    pub owner: String,
    pub file: String,
    pub line: usize,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct Line {
    text: String,
    position: usize,
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
struct Rule {
    target: String,
    prerequisites: String,
    position: usize,
    state: ConditionalTruth,
    conditional_syntax: bool,
    recipes: Vec<Recipe>,
}

#[derive(Debug)]
struct Candidate {
    owner: String,
    line: usize,
    aggregate: Rule,
    output_expression: String,
}

/// Collects the narrowly bounded literal text rule from one joined Make
/// snapshot. Conditional states, when supplied, use zero-based joined-line
/// positions. The source digest is filled by the parser after collection.
#[must_use]
pub(crate) fn collect_with_context(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    line_states: Option<&[ConditionalTruth]>,
) -> (
    Vec<SourceLiteralTextRuleDecl>,
    Vec<SourceLiteralTextRuleRejection>,
) {
    let file = match source_relative_file(rel_dir) {
        Ok(file) => file,
        Err(reason) => {
            return (
                Vec::new(),
                vec![rejection("<unknown>", "<unknown>", 1, reason)],
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
                "joined mmakefile exceeds the literal text scan limit",
            )],
        );
    }
    if source_root.canonicalize().is_err() {
        return (
            Vec::new(),
            vec![rejection(
                "<unknown>",
                &file,
                1,
                "source root cannot be canonicalized",
            )],
        );
    }

    let lines = logical_lines(content, line_states);
    let rules = parse_rules(&lines);
    if rules.len() > MAX_RULES {
        return (
            Vec::new(),
            vec![rejection(
                "<unknown>",
                &file,
                1,
                "joined mmakefile exceeds the literal text rule limit",
            )],
        );
    }

    let mut candidates = Vec::new();
    let mut rejections = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.text != "#MM" {
            // An inline metadata edge that points into the generated tree is
            // a plausible producer claim, but it is not the closed ordinary
            // aggregate shape this capability represents.
            if let Some((owner, prerequisites)) = parse_inline_meta_edge(&line.text) {
                if prerequisites.iter().any(|word| {
                    literal_candidate_output(
                        word,
                        line.position,
                        &rules,
                        source_root,
                        rel_dir,
                        scope,
                        dirs,
                        &file,
                    )
                }) {
                    rejections.push(rejection(
                        &owner,
                        &file,
                        line.position + 1,
                        "literal text aggregate requires a bare `#MM` marker before an ordinary rule",
                    ));
                }
            }
            continue;
        }
        let Some(aggregate_line) = lines.get(index + 1) else {
            continue;
        };
        let Some(aggregate) = rules
            .iter()
            .find(|rule| rule.position == aggregate_line.position)
            .cloned()
        else {
            continue;
        };
        let owner = aggregate.target.trim();
        if !safe_target_name(owner) {
            continue;
        }
        let raw_prerequisites = aggregate
            .prerequisites
            .split_whitespace()
            .collect::<Vec<_>>();
        if !raw_prerequisites.iter().any(|word| {
            literal_candidate_output(
                word,
                aggregate.position,
                &rules,
                source_root,
                rel_dir,
                scope,
                dirs,
                &file,
            )
        }) {
            continue;
        }

        let fail = |reason: &str| rejection(owner, &file, line.position + 1, reason.to_owned());
        if line.state == ConditionalTruth::False || aggregate.state == ConditionalTruth::False {
            continue;
        }
        if line.state == ConditionalTruth::Unknown || aggregate.state == ConditionalTruth::Unknown {
            rejections.push(fail("owner aggregate is in an undecided Make conditional"));
            continue;
        }
        if line.conditional_syntax || aggregate.conditional_syntax {
            rejections.push(fail(
                "owner aggregate is conditional but line-state analysis is unavailable",
            ));
            continue;
        }
        if !aggregate.recipes.is_empty() {
            rejections.push(fail(
                "literal text aggregate must have no ordinary Make recipe",
            ));
            continue;
        }
        if raw_prerequisites.len() != 1 || raw_prerequisites[0].contains("$$") {
            rejections.push(fail(
                "literal text aggregate must name exactly one unescaped generated output",
            ));
            continue;
        }
        let output_expression = raw_prerequisites[0].to_owned();
        candidates.push(Candidate {
            owner: owner.to_owned(),
            line: line.position + 1,
            aggregate,
            output_expression,
        });
    }

    let mut declarations = Vec::new();
    for candidate in candidates {
        if rules.iter().any(|rule| {
            rule.state != ConditionalTruth::False
                && target_specific_echo_assignment(&rule.prerequisites)
        }) {
            rejections.push(rejection(
                &candidate.owner,
                &file,
                candidate.line,
                "target-specific ECHO command semantics are not proved",
            ));
            continue;
        }
        match validate_candidate(candidate, &rules, source_root, rel_dir, scope, dirs, &file) {
            Ok(declaration) => declarations.push(declaration),
            Err(rejection) => rejections.push(rejection),
        }
    }

    reject_duplicate_claims(&mut declarations, &mut rejections, &file);
    if rejections.len() > MAX_REJECTIONS {
        rejections.truncate(MAX_REJECTIONS);
        rejections.push(rejection(
            "<unknown>",
            &file,
            1,
            "literal text rejection limit reached",
        ));
    }
    (declarations, rejections)
}

fn target_specific_echo_assignment(value: &str) -> bool {
    let mut assignment = value.trim();
    for _ in 0..3 {
        let Some((modifier, tail)) = assignment.split_once(char::is_whitespace) else {
            break;
        };
        if !matches!(modifier, "override" | "export" | "private") {
            break;
        }
        assignment = tail.trim_start();
    }
    variable_assignment(assignment).is_some_and(|(name, _, _)| name == "ECHO")
}

fn validate_candidate(
    candidate: Candidate,
    rules: &[Rule],
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    file: &str,
) -> Result<SourceLiteralTextRuleDecl, SourceLiteralTextRuleRejection> {
    let fail = |reason: String| rejection(&candidate.owner, file, candidate.line, reason);
    let raw_directory = source_relative_directory(rel_dir).map_err(fail)?;
    let aggregate_context = MakeExprContext::new(
        scope,
        dirs,
        candidate.aggregate.position,
        source_root,
        rel_dir,
    );
    let output = evaluate_make_expr(&candidate.output_expression, &aggregate_context)
        .map_err(|error| fail(format!("cannot resolve generated output: {error}")))?;
    let output_parent = evaluate_make_expr("$(GENDIR)/$(CURDIR)", &aggregate_context)
        .map_err(|error| fail(format!("cannot resolve generated output root: {error}")))?;
    if !safe_literal_output_path(&output, &output_parent, &raw_directory) {
        return Err(fail(format!(
            "generated output `{output}` must be one safe direct file below `{output_parent}`"
        )));
    }

    let producer_rules = rules
        .iter()
        .filter(|rule| {
            let context = MakeExprContext::new(scope, dirs, rule.position, source_root, rel_dir);
            evaluate_make_expr(rule.target.trim(), &context).is_ok_and(|target| target == output)
        })
        .collect::<Vec<_>>();
    let [producer] = producer_rules.as_slice() else {
        return Err(fail(if producer_rules.is_empty() {
            format!("generated output `{output}` has no ordinary file rule")
        } else {
            format!("generated output `{output}` has duplicate ordinary file rules")
        }));
    };
    if producer.state == ConditionalTruth::False {
        return Err(fail(format!(
            "generated output `{output}` is inactive while its owner is active"
        )));
    }
    if producer.state == ConditionalTruth::Unknown
        || producer
            .recipes
            .iter()
            .any(|recipe| recipe.state == ConditionalTruth::Unknown)
    {
        return Err(fail(
            "generated output or recipe is in an undecided Make conditional".into(),
        ));
    }
    if producer.conditional_syntax
        || producer
            .recipes
            .iter()
            .any(|recipe| recipe.conditional_syntax)
    {
        return Err(fail(
            "generated output or recipe is conditional but line-state analysis is unavailable"
                .into(),
        ));
    }
    if producer
        .recipes
        .iter()
        .any(|recipe| has_unescaped_trailing_backslash(&recipe.text))
    {
        return Err(fail(
            "continued or multiline literal text recipe commands are not represented".into(),
        ));
    }

    let raw_inputs = producer
        .prerequisites
        .split_whitespace()
        .collect::<Vec<_>>();
    let [raw_input] = raw_inputs.as_slice() else {
        return Err(fail(format!(
            "generated output `{output}` must have exactly one source prerequisite"
        )));
    };
    if raw_input.contains("$$") {
        return Err(fail(
            "generated output source prerequisite contains an escaped dollar".into(),
        ));
    }
    let producer_context =
        MakeExprContext::new(scope, dirs, producer.position, source_root, rel_dir);
    let source_recipe = evaluate_make_expr("$(SRCDIR)/$(CURDIR)/mmakefile.src", &producer_context)
        .map_err(|error| fail(format!("cannot resolve source mmakefile path: {error}")))?;
    let expected_source_recipe = if raw_directory.is_empty() {
        format!("{SOURCE_ALIAS}/mmakefile.src")
    } else {
        format!("{SOURCE_ALIAS}/{raw_directory}/mmakefile.src")
    };
    if source_recipe != expected_source_recipe {
        return Err(fail(format!(
            "source mmakefile expression resolves to `{source_recipe}`, expected `{expected_source_recipe}`"
        )));
    }
    let input = evaluate_make_expr(raw_input, &producer_context)
        .map_err(|error| fail(format!("cannot resolve source prerequisite: {error}")))?;
    if input != source_recipe {
        return Err(fail(format!(
            "generated output prerequisite `{input}` is not its source mmakefile `{source_recipe}`"
        )));
    }

    let text = parse_literal_recipe(
        producer,
        &output,
        &output_parent,
        scope,
        dirs,
        source_root,
        rel_dir,
    )
    .map_err(|reason| fail(format!("literal text recipe: {reason}")))?;

    Ok(SourceLiteralTextRuleDecl {
        owner: candidate.owner,
        file: file.to_owned(),
        line: candidate.line,
        output,
        text,
        file_sha256: String::new(),
    })
}

fn parse_literal_recipe(
    rule: &Rule,
    output: &str,
    output_parent: &str,
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    rel_dir: &Path,
) -> Result<String, String> {
    let command_context = MakeExprContext::new(scope, dirs, usize::MAX, source_root, rel_dir);
    if scope.conditionally_assigned_before("ECHO", usize::MAX)
        || !evaluate_make_expr("$(ECHO)", &command_context).is_ok_and(|command| command == "echo")
    {
        return Err(
            "ECHO command is overridden or its ordinary echo semantics are unproved".into(),
        );
    }
    if rule.recipes.is_empty() {
        return Err("file rule has no recipe".into());
    }
    if rule.recipes.len() > MAX_OUTPUT_LINES + 2 {
        return Err("recipe exceeds the literal text line limit".into());
    }
    if rule
        .recipes
        .iter()
        .any(|recipe| recipe.state != ConditionalTruth::True)
    {
        return Err("recipe is not unconditionally active".into());
    }

    let mut at = 0;
    if rule
        .recipes
        .first()
        .is_some_and(|recipe| is_status_echo(&recipe.text))
    {
        let status = parse_echo(&rule.recipes[0].text, false)
            .map_err(|reason| format!("status echo {reason}"))?;
        if status.redirect.is_some() || status.raw_data.contains('$') {
            return Err("status echo must be a safe static literal".into());
        }
        validate_echo_data(&status.raw_data).map_err(|reason| format!("status echo {reason}"))?;
        at += 1;
    }

    let Some(mkdir) = rule.recipes.get(at) else {
        return Err("missing exact `%mkdir_q` command".into());
    };
    let Some(raw_directory) = mkdir.text.trim().strip_prefix("%mkdir_q dir=") else {
        return Err("expected exact `%mkdir_q dir=output-parent`".into());
    };
    if raw_directory.is_empty() || raw_directory.contains(char::is_whitespace) {
        return Err("`%mkdir_q` must name exactly one directory".into());
    }
    let mkdir_context = MakeExprContext::new(scope, dirs, usize::MAX, source_root, rel_dir);
    let directory = evaluate_make_expr(raw_directory, &mkdir_context)
        .map_err(|error| format!("cannot resolve `%mkdir_q` directory: {error}"))?;
    if directory != output_parent
        || output.rsplit_once('/').map(|(parent, _)| parent) != Some(output_parent)
    {
        return Err("`%mkdir_q` directory must equal the generated output parent".into());
    }
    at += 1;

    let mut text = String::new();
    let mut output_lines = 0usize;
    for recipe in &rule.recipes[at..] {
        let echo = parse_echo(&recipe.text, true)?;
        let expected_redirect = if output_lines == 0 {
            Redirect::Truncate
        } else {
            Redirect::Append
        };
        if echo.redirect != Some(expected_redirect) {
            return Err(if output_lines == 0 {
                "first text echo must truncate with `> $@`".into()
            } else {
                "subsequent text echoes must append with `>> $@`".into()
            });
        }
        validate_echo_data(&echo.raw_data)?;
        // Recipes expand after the complete makefile has been read; ordinary
        // targets/prerequisites above remain declaration-time expressions.
        let context = MakeExprContext::new(scope, dirs, usize::MAX, source_root, rel_dir);
        let expanded = evaluate_make_expr(&echo.raw_data, &context)
            .map_err(|error| format!("cannot resolve echo data: {error}"))?;
        validate_expanded_data(&expanded)?;
        if expanded.starts_with('-') {
            return Err("echo data beginning with `-` has shell-dependent option semantics".into());
        }
        output_lines += 1;
        if output_lines > MAX_OUTPUT_LINES {
            return Err("recipe exceeds the literal text line limit".into());
        }
        text.push_str(&expanded);
        text.push('\n');
        if text.len() > MAX_OUTPUT_BYTES {
            return Err("generated text exceeds the literal text byte limit".into());
        }
    }
    if output_lines == 0 {
        return Err("recipe has no literal output echo".into());
    }
    Ok(text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redirect {
    Truncate,
    Append,
}

#[derive(Debug)]
struct Echo {
    raw_data: String,
    redirect: Option<Redirect>,
}

fn is_status_echo(command: &str) -> bool {
    parse_echo(command, false).is_ok_and(|echo| echo.redirect.is_none())
}

fn parse_echo(command: &str, require_redirect: bool) -> Result<Echo, String> {
    let command = command.trim();
    let body = command
        .strip_prefix("@$(ECHO) ")
        .ok_or_else(|| "must use the exact `@$(ECHO)` command".to_owned())?;
    let quoted = body
        .strip_prefix('"')
        .ok_or_else(|| "data must be one double-quoted argument".to_owned())?;
    let closing_quote = quoted
        .find('"')
        .ok_or_else(|| "data argument has no closing quote".to_owned())?;
    let raw_data = &quoted[..closing_quote];
    let suffix = quoted[closing_quote + 1..].trim();
    let redirect = match suffix {
        "" if !require_redirect => None,
        "> $@" => Some(Redirect::Truncate),
        ">> $@" => Some(Redirect::Append),
        "" => return Err("output echo must redirect exactly to `$@`".into()),
        _ => return Err("echo must redirect exactly to `$@`".into()),
    };
    validate_echo_data(raw_data)?;
    Ok(Echo {
        raw_data: raw_data.to_owned(),
        redirect,
    })
}

fn validate_echo_data(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("empty echo data is not represented".into());
    }
    if value.len() > MAX_OUTPUT_BYTES {
        return Err("echo data exceeds the literal text byte limit".into());
    }
    if value.contains(['"', '\\', '`', '\n', '\r', '\0']) || value.chars().any(char::is_control) {
        return Err(
            "quotes, backslashes, backticks, and control characters are not safe echo data".into(),
        );
    }
    let mut remaining = value;
    while let Some(dollar) = remaining.find('$') {
        remaining = &remaining[dollar..];
        if remaining.starts_with("$$") {
            return Err("escaped dollars are not represented".into());
        }
        let Some(expression) = remaining.strip_prefix("$(") else {
            return Err("only ordinary `$(VARIABLE)` Make references are represented".into());
        };
        let end = expression
            .find(')')
            .ok_or_else(|| "unterminated Make variable reference".to_owned())?;
        let name = &expression[..end];
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err("Make functions and nested expressions are not represented".into());
        }
        remaining = &expression[end + 1..];
    }
    Ok(())
}

fn validate_expanded_data(value: &str) -> Result<(), String> {
    if value.len() > MAX_OUTPUT_BYTES
        || value.is_empty()
        || value.contains(['"', '\\', '`', '\n', '\r', '\0'])
        || value.chars().any(char::is_control)
    {
        return Err("expanded echo data contains unsafe or oversized text".into());
    }
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        rest = &rest[at..];
        let Some(tail) = rest.strip_prefix("${") else {
            return Err("expanded echo data contains an unresolved Make variable".into());
        };
        let end = tail
            .find('}')
            .ok_or_else(|| "expanded echo data has an unterminated deferred alias".to_owned())?;
        let name = &tail[..end];
        if !matches!(name, "AROS_SOURCE_DIR" | "AROS_BUILD_DIR") {
            return Err(format!("deferred alias `${{{name}}}` is not reserved here"));
        }
        rest = &tail[end + 1..];
    }
    Ok(())
}

fn parse_inline_meta_edge(line: &str) -> Option<(String, Vec<String>)> {
    let body = line
        .strip_prefix("#MM ")
        .or_else(|| line.strip_prefix("#MM- "))?;
    let (owner, prerequisites) = body.split_once(':')?;
    let owner = owner.trim();
    safe_target_name(owner).then(|| {
        (
            owner.to_owned(),
            prerequisites
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
        )
    })
}

fn parse_rules(lines: &[Line]) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut current: Option<Rule> = None;
    for line in lines {
        if line.text.starts_with('\t') {
            if let Some(rule) = current.as_mut() {
                rule.recipes.push(Recipe {
                    text: line.text.trim_start_matches('\t').to_owned(),
                    state: line.state,
                    conditional_syntax: line.conditional_syntax,
                });
            }
            continue;
        }
        if let Some((target, prerequisites)) = parse_rule_fields(&line.text) {
            if let Some(rule) = current.take() {
                rules.push(rule);
            }
            current = Some(Rule {
                target,
                prerequisites,
                position: line.position,
                state: line.state,
                conditional_syntax: line.conditional_syntax,
                recipes: Vec::new(),
            });
        }
    }
    if let Some(rule) = current {
        rules.push(rule);
    }
    rules
}

fn parse_rule_fields(line: &str) -> Option<(String, String)> {
    if line.starts_with([' ', '\t']) || line.trim_start().starts_with('#') {
        return None;
    }
    if has_unescaped_trailing_backslash(line) {
        return None;
    }
    let colon = line.find(':')?;
    let target = line[..colon].trim();
    let tail = line[colon + 1..].trim();
    if target.is_empty()
        || line[colon..].starts_with("::")
        || tail.starts_with('=')
        || target.contains(['=', ';', '`', '\\'])
        || tail.contains([';', '`'])
    {
        return None;
    }
    Some((target.to_owned(), tail.to_owned()))
}

fn logical_lines(content: &str, states: Option<&[ConditionalTruth]>) -> Vec<Line> {
    let physical = content.lines().collect::<Vec<_>>();
    let mut output = Vec::with_capacity(physical.len());
    let mut conditional_depth = 0usize;
    for (position, physical_line) in physical.iter().enumerate() {
        let text = physical_line.trim_end_matches('\r').to_owned();
        let trimmed = text.trim();
        if is_conditional_open(trimmed) {
            conditional_depth += 1;
        }
        let state = states.map_or(ConditionalTruth::True, |states| {
            states
                .get(position)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
        });
        let closing = trimmed == "endif";
        output.push(Line {
            text,
            position,
            state,
            conditional_syntax: states.is_none() && conditional_depth > 0,
        });
        if closing {
            conditional_depth = conditional_depth.saturating_sub(1);
        }
    }
    output
}

fn is_conditional_open(line: &str) -> bool {
    ["ifeq", "ifneq", "ifdef", "ifndef"]
        .iter()
        .any(|directive| line == *directive || line.starts_with(&format!("{directive} ")))
}

fn safe_literal_output_path(output: &str, output_parent: &str, relative_dir: &str) -> bool {
    if !output.starts_with(&format!("{BUILD_ALIAS}/gen/"))
        || !output.starts_with(&format!("{output_parent}/"))
    {
        return false;
    }
    let Some(filename) = output.strip_prefix(&format!("{output_parent}/")) else {
        return false;
    };
    if !safe_filename(filename) {
        return false;
    }
    let expected_parent = if relative_dir.is_empty() {
        format!("{BUILD_ALIAS}/gen")
    } else {
        format!("{BUILD_ALIAS}/gen/{relative_dir}")
    };
    output_parent == expected_parent
        && Path::new(output)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[allow(clippy::too_many_arguments)]
fn literal_candidate_output(
    raw: &str,
    line: usize,
    rules: &[Rule],
    root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    file: &str,
) -> bool {
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let output = evaluate_make_expr(raw, &context).ok();
    let generated = raw.contains("$(GENDIR)")
        || output
            .as_deref()
            .is_some_and(|output| output.starts_with("${AROS_BUILD_DIR}/gen/"));
    if !generated {
        return false;
    }
    rules.iter().any(|rule| {
        let rule_context = MakeExprContext::new(scope, dirs, rule.position, root, rel_dir);
        let matching = rule.target == raw
            || output.as_ref().is_some_and(|output| {
                evaluate_make_expr(&rule.target, &rule_context)
                    .is_ok_and(|target| &target == output)
            });
        matching
            && (rule.prerequisites.split_whitespace().count() == 1
                && rule.prerequisites.split_whitespace().any(|input| {
                    input == "$(SRCDIR)/$(CURDIR)/mmakefile.src"
                        || evaluate_make_expr(input, &rule_context)
                            .is_ok_and(|input| input == format!("{SOURCE_ALIAS}/{file}"))
                })
                || rule.recipes.iter().any(|recipe| {
                    let command = recipe.text.trim().trim_start_matches('@');
                    (command.starts_with("$(ECHO) ")
                        || command.starts_with("echo ")
                        || command.starts_with("printf "))
                        && command.contains('>')
                        && command.contains("$@")
                }))
    })
}

fn safe_filename(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && !value.contains('/')
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
}

fn safe_target_name(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
}

fn source_relative_file(rel_dir: &Path) -> Result<String, String> {
    let directory = source_relative_directory(rel_dir)?;
    Ok(if directory.is_empty() {
        "mmakefile.src".to_owned()
    } else {
        format!("{directory}/mmakefile.src")
    })
}

fn source_relative_directory(rel_dir: &Path) -> Result<String, String> {
    if rel_dir.is_absolute() {
        return Err("literal text mmakefile directory must be source-relative".into());
    }
    let mut components = Vec::new();
    for component in rel_dir.components() {
        let Component::Normal(part) = component else {
            if matches!(component, Component::CurDir) {
                continue;
            }
            return Err("literal text mmakefile directory escapes the source root".into());
        };
        let part = part
            .to_str()
            .ok_or_else(|| "literal text mmakefile directory is not UTF-8".to_owned())?;
        if part.is_empty()
            || !part
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
        {
            return Err("literal text mmakefile directory has an unsafe component".into());
        }
        components.push(part);
    }
    Ok(components.join("/"))
}

fn has_unescaped_trailing_backslash(value: &str) -> bool {
    let tail = value.trim_end();
    let count = tail
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count();
    count % 2 == 1
}

fn reject_duplicate_claims(
    declarations: &mut Vec<SourceLiteralTextRuleDecl>,
    rejections: &mut Vec<SourceLiteralTextRuleRejection>,
    file: &str,
) {
    let mut owner_counts = BTreeMap::<String, usize>::new();
    let mut output_counts = BTreeMap::<String, usize>::new();
    for declaration in declarations.iter() {
        *owner_counts.entry(declaration.owner.clone()).or_default() += 1;
        *output_counts.entry(declaration.output.clone()).or_default() += 1;
    }
    let duplicate_owners = owner_counts
        .into_iter()
        .filter_map(|(owner, count)| (count > 1).then_some(owner))
        .collect::<BTreeSet<_>>();
    let duplicate_outputs = output_counts
        .into_iter()
        .filter_map(|(output, count)| (count > 1).then_some(output))
        .collect::<BTreeSet<_>>();
    declarations.retain(|declaration| {
        let duplicate_owner = duplicate_owners.contains(&declaration.owner);
        let duplicate_output = duplicate_outputs.contains(&declaration.output);
        if duplicate_owner || duplicate_output {
            rejections.push(rejection(
                &declaration.owner,
                file,
                declaration.line,
                if duplicate_owner {
                    "literal text owner has duplicate active aggregates"
                } else {
                    "literal text output has duplicate active owners"
                },
            ));
            false
        } else {
            true
        }
    });
}

fn rejection(
    owner: &str,
    file: &str,
    line: usize,
    reason: impl Into<String>,
) -> SourceLiteralTextRuleRejection {
    SourceLiteralTextRuleRejection {
        owner: owner.to_owned(),
        file: file.to_owned(),
        line,
        reason: reason.into(),
    }
}

#[cfg(test)]
#[path = "source_literal_text_rules_tests.rs"]
mod tests;
