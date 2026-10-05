//! Closed model for source-declared, multi-output text products.
//!
//! This scanner recognizes one named aggregate whose products are generated
//! from fetched source templates by bounded literal `sed` operations. It does
//! not execute Make or preserve arbitrary shell commands.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{ConditionalTruth, VarScope};
use std::collections::BTreeSet;
use std::path::Component;
use std::path::Path;

/// A fetched input transformed into one declared text product.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTextOutput {
    pub input: String,
    pub output: String,
    pub operations: Vec<SourceTextOperation>,
    pub mode: Option<String>,
}

/// One bounded operation on source-derived text.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceTextOperation {
    ReplaceAll { token: String, replacement: String },
    ReplaceWholeLineContaining { token: String, replacement: String },
}

/// A named aggregate and all of its source-derived text products.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTextRuleDecl {
    pub owner: String,
    pub file: String,
    pub line: usize,
    pub fetch_owner: String,
    pub outputs: Vec<SourceTextOutput>,
}

/// A text producer that cannot be represented by the closed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTextRuleRejection {
    pub owner: String,
    pub file: String,
    pub line: usize,
    pub reason: String,
}

/// Collects named multi-output source text producers and their exact fetch
/// owner. Conditional line states use zero-based source line positions.
#[must_use]
pub(crate) fn collect_source_text_rules_with_context(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SourceTextRuleDecl>, Vec<SourceTextRuleRejection>) {
    let file = match source_relative_directory(rel_dir) {
        Ok(directory) if directory.is_empty() => "mmakefile.src".to_owned(),
        Ok(directory) => format!("{directory}/mmakefile.src"),
        Err(reason) => {
            return (
                Vec::new(),
                vec![SourceTextRuleRejection {
                    owner: owner_hint(content),
                    file: rel_dir.to_string_lossy().replace('\\', "/"),
                    line: 1,
                    reason,
                }],
            );
        }
    };
    if source_root.canonicalize().is_err() {
        return (
            Vec::new(),
            vec![SourceTextRuleRejection {
                owner: owner_hint(content),
                file,
                line: 1,
                reason: "source root cannot be canonicalized".into(),
            }],
        );
    }

    let lines = logical_lines(content, line_states);
    let rules = parse_rules(&lines);
    let edges = parse_meta_edges(&lines);
    let fetches = parse_fetch_rules(&lines, source_root, rel_dir, scope, dirs);
    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    let mut examined_owners = BTreeSet::new();

    for owner_rule in &rules {
        let owner = owner_rule.target.trim();
        if !safe_target_name(owner) || owner_rule.state == ConditionalTruth::False {
            continue;
        }
        let owner_context =
            MakeExprContext::new(scope, dirs, owner_rule.source_line, source_root, rel_dir);
        let Ok(roots) = output_roots(&owner_context) else {
            continue;
        };
        let raw_prerequisites = owner_rule
            .prerequisites
            .split_whitespace()
            .collect::<Vec<_>>();
        if raw_prerequisites.len() < 2 {
            continue;
        }
        let resolved_prerequisites = raw_prerequisites
            .iter()
            .map(|raw| evaluate_make_expr(raw, &owner_context).ok())
            .collect::<Vec<_>>();
        let has_include = resolved_prerequisites
            .iter()
            .flatten()
            .any(|path| output_root(path, &roots).is_some_and(|kind| kind != OutputKind::HostTool));
        let has_host_tool = resolved_prerequisites
            .iter()
            .flatten()
            .any(|path| output_root(path, &roots) == Some(OutputKind::HostTool));
        // The multi-output contract is deliberately narrower than generic
        // header transforms: at least one generated include and one host tool
        // must share the same source-owned aggregate.
        if !has_include || !has_host_tool {
            continue;
        }
        if !examined_owners.insert(owner.to_owned()) {
            continue;
        }

        let rejection = |reason: String| SourceTextRuleRejection {
            owner: owner.to_owned(),
            file: file.clone(),
            line: owner_rule.source_line + 1,
            reason,
        };
        if owner_rule.state == ConditionalTruth::Unknown || owner_rule.conditional_syntax {
            rejections.push(rejection(conditional_problem(
                owner_rule.state,
                owner_rule.conditional_syntax,
            )));
            continue;
        }
        if owner_rule_count(owner, &rules) != 1 {
            rejections.push(rejection(
                "source-text owner must have exactly one ordinary Make rule".into(),
            ));
            continue;
        }
        if owner_rule.recipes.len() != 1
            || owner_rule.recipes[0].text.trim() != "@$(NOP)"
            || owner_rule.recipes[0].state != ConditionalTruth::True
            || owner_rule.recipes[0].conditional_syntax
        {
            rejections.push(rejection(
                "source-text aggregate must have only the exact `@$(NOP)` recipe".into(),
            ));
            continue;
        }

        let mut outputs = Vec::with_capacity(raw_prerequisites.len());
        let mut seen_outputs = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        let mut resolution_error = None;
        for (raw, resolved) in raw_prerequisites.iter().zip(resolved_prerequisites) {
            if raw.contains("$$") {
                resolution_error = Some(format!(
                    "aggregate product prerequisite `{raw}` contains an escaped dollar"
                ));
                break;
            }
            let Some(output) = resolved else {
                resolution_error = Some(format!(
                    "cannot resolve aggregate product prerequisite `{raw}`"
                ));
                break;
            };
            let Some(kind) = output_root(&output, &roots) else {
                resolution_error = Some(format!(
                    "aggregate prerequisite `{output}` is outside configured include and hosttools roots"
                ));
                break;
            };
            if !safe_cmake_path(&output) {
                resolution_error = Some(format!(
                    "aggregate product path `{output}` contains an unsafe component"
                ));
                break;
            }
            let emitted_output = normalize_output_path(&output, kind, &roots);
            if !safe_cmake_path(&emitted_output) {
                resolution_error = Some(format!(
                    "normalized product path `{emitted_output}` contains an unsafe component"
                ));
                break;
            }
            if !seen_outputs.insert(emitted_output.clone()) {
                resolution_error =
                    Some(format!("aggregate names product `{output}` more than once"));
                break;
            }
            kinds.insert(kind);
            outputs.push((output, emitted_output, kind));
        }
        if let Some(reason) = resolution_error {
            rejections.push(rejection(reason));
            continue;
        }
        if !kinds.contains(&OutputKind::HostTool)
            || !kinds.contains(&OutputKind::DeveloperInclude)
                && !kinds.contains(&OutputKind::GeneratedInclude)
        {
            rejections.push(rejection(
                "source-text aggregate must include a host tool and a generated include product"
                    .into(),
            ));
            continue;
        }

        let meta_edges = edges
            .iter()
            .filter(|edge| edge.owner == owner && edge.state != ConditionalTruth::False)
            .collect::<Vec<_>>();
        let [meta] = meta_edges.as_slice() else {
            rejections.push(rejection(if meta_edges.is_empty() {
                "source-text owner has no matching `#MM owner : fetch` edge".into()
            } else {
                "source-text owner has duplicate `#MM` prerequisite edges".into()
            }));
            continue;
        };
        if meta.state != ConditionalTruth::True || meta.conditional_syntax {
            rejections.push(rejection(conditional_problem(
                meta.state,
                meta.conditional_syntax,
            )));
            continue;
        }
        if meta.prerequisites.len() != 1 || !safe_target_name(&meta.prerequisites[0]) {
            rejections.push(rejection(
                "source-text `#MM` edge must name exactly one safe fetch target".into(),
            ));
            continue;
        }
        let fetch_owner = meta.prerequisites[0].clone();
        let matching_fetches = fetches
            .iter()
            .filter(|fetch| fetch.owner == fetch_owner && fetch.state != ConditionalTruth::False)
            .collect::<Vec<_>>();
        let [fetch] = matching_fetches.as_slice() else {
            rejections.push(rejection(if matching_fetches.is_empty() {
                format!("`#MM` fetch prerequisite `{fetch_owner}` has no `%fetch` declaration")
            } else {
                format!("`%fetch` target `{fetch_owner}` is declared more than once")
            }));
            continue;
        };
        if fetch.state != ConditionalTruth::True || fetch.conditional_syntax {
            rejections.push(rejection(conditional_problem(
                fetch.state,
                fetch.conditional_syntax,
            )));
            continue;
        }
        let fetch_prefix = format!("{}/", fetch.destination.trim_end_matches('/'));

        let mut products = Vec::with_capacity(outputs.len());
        let mut product_error = None;
        for (source_output, output, kind) in outputs {
            let product_rules = rules
                .iter()
                .filter(|rule| rule.state != ConditionalTruth::False)
                .filter(|rule| {
                    let make_context =
                        MakeExprContext::new(scope, dirs, rule.source_line, source_root, rel_dir);
                    evaluate_make_expr(rule.target.trim(), &make_context)
                        .is_ok_and(|target| target == source_output)
                })
                .collect::<Vec<_>>();
            let [product_rule] = product_rules.as_slice() else {
                product_error = Some(if product_rules.is_empty() {
                    format!("aggregate product `{output}` has no ordinary file rule")
                } else {
                    format!("aggregate product `{output}` has duplicate ordinary file rules")
                });
                break;
            };
            if product_rule.state != ConditionalTruth::True || product_rule.conditional_syntax {
                product_error = Some(conditional_problem(
                    product_rule.state,
                    product_rule.conditional_syntax,
                ));
                break;
            }
            let product_context =
                MakeExprContext::new(scope, dirs, product_rule.source_line, source_root, rel_dir);
            let raw_inputs = product_rule
                .prerequisites
                .split_whitespace()
                .collect::<Vec<_>>();
            let [raw_input] = raw_inputs.as_slice() else {
                product_error = Some(format!(
                    "source-text product `{output}` must have exactly one input prerequisite"
                ));
                break;
            };
            if raw_input.contains("$$") {
                product_error = Some(format!(
                    "source-text input `{raw_input}` contains an escaped dollar"
                ));
                break;
            }
            let input = match evaluate_make_expr(raw_input, &product_context) {
                Ok(input) if safe_cmake_path(&input) => input,
                Ok(input) => {
                    product_error = Some(format!(
                        "source-text input path `{input}` contains an unsafe component"
                    ));
                    break;
                }
                Err(error) => {
                    product_error = Some(format!(
                        "cannot resolve source-text input `{raw_input}`: {error}"
                    ));
                    break;
                }
            };
            if !input.starts_with("${AROS_PORTS_DIR}/") || !input.starts_with(&fetch_prefix) {
                product_error = Some(format!(
                    "source-text input `{input}` must be a fetched file below `{}`",
                    fetch.destination
                ));
                break;
            }
            let Some((operations, mode)) =
                parse_product_recipe(product_rule, &source_output, kind, &roots, &product_context)
            else {
                product_error = Some(format!(
                    "source-text product recipe for `{output}` is outside the exact echo, mkdir, literal sed, and optional chmod subset"
                ));
                break;
            };
            products.push(SourceTextOutput {
                input,
                output,
                operations,
                mode,
            });
        }
        if let Some(reason) = product_error {
            rejections.push(rejection(reason));
            continue;
        }

        declarations.push(SourceTextRuleDecl {
            owner: owner.to_owned(),
            file: file.clone(),
            line: owner_rule.source_line + 1,
            fetch_owner,
            outputs: products,
        });
    }

    (declarations, rejections)
}

#[derive(Debug, Clone)]
struct LogicalLine {
    text: String,
    source_line: usize,
    state: ConditionalTruth,
    conditional_syntax: bool,
}

#[derive(Debug, Clone)]
struct Rule {
    target: String,
    prerequisites: String,
    source_line: usize,
    state: ConditionalTruth,
    conditional_syntax: bool,
    recipes: Vec<LogicalLine>,
}

#[derive(Debug, Clone)]
struct MetaEdge {
    owner: String,
    prerequisites: Vec<String>,
    state: ConditionalTruth,
    conditional_syntax: bool,
}

#[derive(Debug, Clone)]
struct FetchRule {
    owner: String,
    destination: String,
    state: ConditionalTruth,
    conditional_syntax: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OutputKind {
    DeveloperInclude,
    GeneratedInclude,
    HostTool,
}

#[derive(Debug, Clone)]
struct OutputRoots {
    developer_include_source: String,
    generated_include_source: String,
    host_tools: String,
}

fn output_roots(context: &MakeExprContext<'_>) -> Result<OutputRoots, String> {
    let developer_include_source =
        evaluate_make_expr("$(AROS_INCLUDES)", context).map_err(|error| error.to_string())?;
    let configured_developer_include =
        evaluate_make_expr("$(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)", context)
            .map_err(|error| error.to_string())?;
    if developer_include_source != configured_developer_include {
        return Err(
            "AROS_INCLUDES does not resolve to the configured developer include root".into(),
        );
    }
    let generated_include_source =
        evaluate_make_expr("$(GENINCDIR)", context).map_err(|error| error.to_string())?;
    let configured_generated_include =
        evaluate_make_expr("$(GENDIR)/include", context).map_err(|error| error.to_string())?;
    if generated_include_source != configured_generated_include {
        return Err("GENINCDIR does not resolve to GENDIR/include".into());
    }
    let host_tools =
        evaluate_make_expr("$(TOOLDIR)", context).map_err(|error| error.to_string())?;
    for root in [
        &developer_include_source,
        &generated_include_source,
        &host_tools,
    ] {
        if !safe_cmake_path(root) {
            return Err(format!("configured output root `{root}` is unsafe"));
        }
    }
    Ok(OutputRoots {
        developer_include_source,
        generated_include_source,
        host_tools,
    })
}

fn output_root(path: &str, roots: &OutputRoots) -> Option<OutputKind> {
    if under_root(path, &roots.host_tools) {
        Some(OutputKind::HostTool)
    } else if under_root(path, &roots.developer_include_source) {
        Some(OutputKind::DeveloperInclude)
    } else if under_root(path, &roots.generated_include_source) {
        Some(OutputKind::GeneratedInclude)
    } else {
        None
    }
}

fn normalize_output_path(path: &str, kind: OutputKind, roots: &OutputRoots) -> String {
    match kind {
        OutputKind::DeveloperInclude => normalize_rooted_path(
            path,
            &roots.developer_include_source,
            "${AROS_SDK_INCLUDE_DIR}",
        ),
        OutputKind::GeneratedInclude => {
            normalize_rooted_path(path, &roots.generated_include_source, "${AROS_GENINC_DIR}")
        }
        OutputKind::HostTool => path.to_owned(),
    }
}

fn normalize_rooted_path(path: &str, source_root: &str, cmake_root: &str) -> String {
    let Some(tail) = path.strip_prefix(source_root) else {
        return path.to_owned();
    };
    format!("{cmake_root}{tail}")
}

fn under_root(path: &str, root: &str) -> bool {
    path.strip_prefix(root)
        .is_some_and(|tail| tail.starts_with('/') && tail.len() > 1)
}

fn parse_product_recipe(
    rule: &Rule,
    output: &str,
    kind: OutputKind,
    roots: &OutputRoots,
    context: &MakeExprContext<'_>,
) -> Option<(Vec<SourceTextOperation>, Option<String>)> {
    let has_mode = rule
        .recipes
        .iter()
        .any(|line| line.text.trim() == "@chmod 744 $@");
    let expected_recipe_count = if has_mode { 4 } else { 3 };
    if rule.recipes.len() != expected_recipe_count
        || rule
            .recipes
            .iter()
            .any(|line| line.state != ConditionalTruth::True || line.conditional_syntax)
    {
        return None;
    }
    if has_mode && kind != OutputKind::HostTool {
        return None;
    }
    let echo = &rule.recipes[0].text;
    parse_literal_echo(echo)?;
    let output_parent = output.rsplit_once('/')?.0;
    let directory = parse_mkdir(&rule.recipes[1].text)?;
    let directory = evaluate_make_expr(&directory, context).ok()?;
    if !safe_cmake_path(&directory) || directory != output_parent {
        return None;
    }
    let operations = parse_sed_command(&rule.recipes[2].text, context, roots)?;
    if has_mode && rule.recipes[3].text.trim() != "@chmod 744 $@" {
        return None;
    }
    Some((operations, has_mode.then(|| "744".to_owned())))
}

fn parse_literal_echo(command: &str) -> Option<()> {
    let words = shell_words(command)?;
    if words.len() != 2 || words[0] != ShellWord::Bare("@$(ECHO)".into()) {
        return None;
    }
    let ShellWord::DoubleQuoted(message) = &words[1] else {
        return None;
    };
    if message.is_empty() || message.contains(['$', '`', '\\', '\n', '\r', ';', '|', '&']) {
        return None;
    }
    Some(())
}

fn parse_mkdir(command: &str) -> Option<String> {
    let body = command.trim().strip_prefix("%mkdir_q")?.trim();
    if body.is_empty() {
        return None;
    }
    let arguments = directive_arguments(body)?;
    if arguments.len() != 1 {
        return None;
    }
    let (name, value) = arguments[0].split_once('=')?;
    (name == "dir" && !value.is_empty()).then(|| value.to_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellWord {
    Bare(String),
    DoubleQuoted(String),
}

fn shell_words(command: &str) -> Option<Vec<ShellWord>> {
    let mut words = Vec::new();
    let mut cursor = command.trim_start();
    while !cursor.trim_start().is_empty() {
        cursor = cursor.trim_start();
        if let Some(rest) = cursor.strip_prefix('"') {
            let (quoted, after) = take_double_quoted(rest)?;
            words.push(ShellWord::DoubleQuoted(quoted.to_owned()));
            cursor = after;
            if !cursor.is_empty() && !cursor.starts_with(char::is_whitespace) {
                return None;
            }
            continue;
        }
        let end = cursor.find(char::is_whitespace).unwrap_or(cursor.len());
        let word = &cursor[..end];
        let exact_make_tool = word == "@$(SED)" || word == "@$(ECHO)";
        if word.is_empty()
            || (!exact_make_tool && word.contains(['\'', '"', '`', ';', '&', '|', '(', ')']))
        {
            return None;
        }
        words.push(ShellWord::Bare(word.to_owned()));
        cursor = &cursor[end..];
    }
    Some(words)
}

fn take_double_quoted(input: &str) -> Option<(&str, &str)> {
    let mut escaped = false;
    for (at, character) in input.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Some((&input[..at], &input[at + character.len_utf8()..]));
        }
    }
    None
}

fn parse_sed_command(
    command: &str,
    context: &MakeExprContext<'_>,
    roots: &OutputRoots,
) -> Option<Vec<SourceTextOperation>> {
    let words = shell_words(command)?;
    if words.first()? != &ShellWord::Bare("@$(SED)".into()) || words.len() < 7 {
        return None;
    }
    let redirect = words.len().checked_sub(3)?;
    if words[redirect..]
        != [
            ShellWord::Bare("$<".into()),
            ShellWord::Bare(">".into()),
            ShellWord::Bare("$@".into()),
        ]
    {
        return None;
    }
    let mut operations = Vec::new();
    let mut cursor = 1;
    while cursor < redirect {
        if words.get(cursor)? != &ShellWord::Bare("-e".into()) {
            return None;
        }
        let ShellWord::DoubleQuoted(raw_expression) = words.get(cursor + 1)? else {
            return None;
        };
        if raw_expression.contains("$$") {
            return None;
        }
        let expression = evaluate_make_expr(raw_expression, context).ok()?;
        let expression = decode_shell_double_quoted(&expression)?;
        operations.push(normalize_operation(
            parse_sed_expression(&expression)?,
            roots,
        ));
        cursor += 2;
    }
    (!operations.is_empty()).then_some(operations)
}

fn decode_shell_double_quoted(value: &str) -> Option<String> {
    let mut decoded = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            decoded.push(character);
            continue;
        }
        match characters.next()? {
            '"' => decoded.push('"'),
            '\\' => decoded.push('\\'),
            '$' | '`' | '\n' => return None,
            other => {
                // POSIX double quotes preserve a backslash before ordinary
                // characters. Only sed's newline escape is admitted below.
                decoded.push('\\');
                decoded.push(other);
            }
        }
    }
    Some(decoded)
}

fn parse_sed_expression(expression: &str) -> Option<SourceTextOperation> {
    let fields = expression.split('|').collect::<Vec<_>>();
    let ["s", pattern, replacement, "g"] = fields.as_slice() else {
        return None;
    };
    let token = if let Some(token) = pattern
        .strip_prefix(".*")
        .and_then(|tail| tail.strip_suffix(".*"))
    {
        if !safe_sed_token(token) {
            return None;
        }
        let replacement = decode_sed_replacement(replacement)?;
        return Some(SourceTextOperation::ReplaceWholeLineContaining {
            token: (*token).to_owned(),
            replacement,
        });
    } else {
        if !safe_sed_token(pattern) {
            return None;
        }
        pattern
    };
    Some(SourceTextOperation::ReplaceAll {
        token: (*token).to_owned(),
        replacement: decode_sed_replacement(replacement)?,
    })
}

fn normalize_operation(operation: SourceTextOperation, roots: &OutputRoots) -> SourceTextOperation {
    let normalize_replacement = |replacement: String| {
        if under_root(&replacement, &roots.developer_include_source)
            || replacement == roots.developer_include_source
        {
            normalize_rooted_path(
                &replacement,
                &roots.developer_include_source,
                "${AROS_SDK_INCLUDE_DIR}",
            )
        } else if under_root(&replacement, &roots.generated_include_source)
            || replacement == roots.generated_include_source
        {
            normalize_rooted_path(
                &replacement,
                &roots.generated_include_source,
                "${AROS_GENINC_DIR}",
            )
        } else {
            replacement
        }
    };
    match operation {
        SourceTextOperation::ReplaceAll { token, replacement } => SourceTextOperation::ReplaceAll {
            token,
            replacement: normalize_replacement(replacement),
        },
        SourceTextOperation::ReplaceWholeLineContaining { token, replacement } => {
            SourceTextOperation::ReplaceWholeLineContaining {
                token,
                replacement: normalize_replacement(replacement),
            }
        }
    }
}

fn safe_sed_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_@%+-/:=\"".contains(character))
}

fn decode_sed_replacement(value: &str) -> Option<String> {
    let mut decoded = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next()? {
                'n' => decoded.push('\n'),
                _ => return None,
            },
            '&' | '|' | ';' | '\r' | '\0' => return None,
            character if character.is_control() && character != '\n' => return None,
            character => decoded.push(character),
        }
    }
    if !safe_replacement_dollars(&decoded) {
        return None;
    }
    Some(decoded)
}

fn safe_replacement_dollars(value: &str) -> bool {
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        rest = &rest[at + 1..];
        let Some(tail) = rest.strip_prefix("{AROS_") else {
            return false;
        };
        let Some(end) = tail.find('}') else {
            return false;
        };
        let name = &tail[..end];
        if !matches!(name, "BUILD_DIR" | "SDK_INCLUDE_DIR" | "GENINC_DIR") {
            return false;
        }
        rest = &tail[end + 1..];
    }
    true
}

fn directive_arguments(body: &str) -> Option<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in body.chars() {
        match character {
            '"' if !quoted => quoted = true,
            '"' => quoted = false,
            character if character.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    arguments.push(std::mem::take(&mut current));
                }
            }
            '`' | ';' | '|' | '&' if !quoted => return None,
            _ => current.push(character),
        }
    }
    if quoted {
        return None;
    }
    if !current.is_empty() {
        arguments.push(current);
    }
    Some(arguments)
}

fn parse_meta_edges(lines: &[LogicalLine]) -> Vec<MetaEdge> {
    lines
        .iter()
        .filter_map(|line| {
            let comment = line.text.trim_start();
            let body = comment
                .strip_prefix("#MM- ")
                .or_else(|| comment.strip_prefix("#MM "))?
                .trim();
            let (owner, prerequisites) = body.split_once(':')?;
            let owner = owner.trim();
            if !safe_target_name(owner) {
                return None;
            }
            Some(MetaEdge {
                owner: owner.to_owned(),
                prerequisites: prerequisites
                    .split_whitespace()
                    .filter(|item| *item != "\\")
                    .map(str::to_owned)
                    .collect(),
                state: line.state,
                conditional_syntax: line.conditional_syntax,
            })
        })
        .collect()
}

fn parse_fetch_rules(
    lines: &[LogicalLine],
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
) -> Vec<FetchRule> {
    let mut fetches = Vec::new();
    for line in lines {
        let Some(body) = line.text.trim().strip_prefix("%fetch") else {
            continue;
        };
        if line.state == ConditionalTruth::False {
            continue;
        }
        let Some(arguments) = directive_arguments(body) else {
            continue;
        };
        let Some(raw_owner) = directive_value(&arguments, "mmake") else {
            continue;
        };
        let Some(raw_destination) = directive_value(&arguments, "destination") else {
            continue;
        };
        let context = MakeExprContext::new(scope, dirs, line.source_line, source_root, rel_dir);
        let (Ok(owner), Ok(destination)) = (
            evaluate_make_expr(&raw_owner, &context),
            evaluate_make_expr(&raw_destination, &context),
        ) else {
            continue;
        };
        if !safe_target_name(&owner)
            || !safe_cmake_path(&destination)
            || !destination.starts_with("${AROS_PORTS_DIR}/")
        {
            continue;
        }
        fetches.push(FetchRule {
            owner,
            destination,
            state: line.state,
            conditional_syntax: line.conditional_syntax,
        });
    }
    fetches
}

fn directive_value(arguments: &[String], key: &str) -> Option<String> {
    let matching = arguments
        .iter()
        .filter_map(|argument| {
            let (name, value) = argument.split_once('=')?;
            (name == key).then(|| value.to_owned())
        })
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [value] => Some(value.clone()),
        _ => None,
    }
}

fn parse_rules(lines: &[LogicalLine]) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut current: Option<Rule> = None;
    for line in lines {
        if line.text.starts_with('\t') {
            if let Some(rule) = current.as_mut() {
                if line.state == ConditionalTruth::False {
                    continue;
                }
                rule.state = combine_state(rule.state, line.state);
                rule.conditional_syntax |= line.conditional_syntax;
                rule.recipes.push(line.clone());
            }
            continue;
        }
        let statement = line.text.trim();
        if statement.is_empty() || statement.starts_with('#') || is_conditional_directive(statement)
        {
            continue;
        }
        if let Some(rule) = current.take() {
            rules.push(rule);
        }
        let Some((target, prerequisites)) = statement.split_once(':') else {
            continue;
        };
        let target = target.trim();
        if target.is_empty()
            || target.contains(['=', '\\'])
            || prerequisites.starts_with([':', '='])
        {
            continue;
        }
        current = Some(Rule {
            target: target.to_owned(),
            prerequisites: prerequisites.trim().to_owned(),
            source_line: line.source_line,
            state: line.state,
            conditional_syntax: line.conditional_syntax,
            recipes: Vec::new(),
        });
    }
    if let Some(rule) = current {
        rules.push(rule);
    }
    rules
}

fn logical_lines(content: &str, states: Option<&[ConditionalTruth]>) -> Vec<LogicalLine> {
    let physical = content.lines().collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut conditional_depth = 0usize;
    let mut at = 0usize;
    while at < physical.len() {
        let first = physical[at];
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

fn has_unescaped_trailing_backslash(value: &str) -> bool {
    let tail = value.trim_end();
    let count = tail
        .chars()
        .rev()
        .take_while(|character| *character == '\\')
        .count();
    count % 2 == 1
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

fn is_conditional_directive(line: &str) -> bool {
    ["ifeq", "ifneq", "ifdef", "ifndef"]
        .iter()
        .any(|directive| line == *directive || line.starts_with(&format!("{directive} ")))
        || line == "else"
        || line == "endif"
}

fn is_conditional_open(line: &str) -> bool {
    ["ifeq", "ifneq", "ifdef", "ifndef"]
        .iter()
        .any(|directive| line == *directive || line.starts_with(&format!("{directive} ")))
}

fn is_conditional_close(line: &str) -> bool {
    line == "endif"
}

fn conditional_problem(state: ConditionalTruth, conditional_syntax: bool) -> String {
    if state == ConditionalTruth::Unknown {
        "source-text owner or producer is in an undecided Make conditional".into()
    } else if conditional_syntax {
        "source-text owner or producer is conditional but no line-state analysis is available"
            .into()
    } else {
        "source-text declaration is not unconditionally active".into()
    }
}

fn owner_rule_count(owner: &str, rules: &[Rule]) -> usize {
    rules
        .iter()
        .filter(|rule| rule.state != ConditionalTruth::False && rule.target.trim() == owner)
        .count()
}

fn safe_target_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
}

fn safe_cmake_path(path: &str) -> bool {
    !path.is_empty()
        && !path.chars().any(char::is_whitespace)
        && !path.contains([';', '\\', '\n', '\r', '"', '\'', '`', '|', '&', '<', '>'])
        && !path.contains("$(")
        && safe_path_dollars(path)
        && Path::new(path)
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

fn safe_path_dollars(value: &str) -> bool {
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        rest = &rest[at + 1..];
        let Some(tail) = rest.strip_prefix("{") else {
            return false;
        };
        let Some(end) = tail.find('}') else {
            return false;
        };
        let name = &tail[..end];
        if !matches!(
            name,
            "AROS_BUILD_DIR"
                | "AROS_PORTS_DIR"
                | "AROS_PORTS_SOURCE_DIR"
                | "AROS_TARGET_CPU"
                | "AROS_TARGET_PLATFORM"
                | "AROS_TARGET_LEGACY_PLATFORM"
                | "AROS_GENINC_DIR"
                | "AROS_SDK_INCLUDE_DIR"
                | "AROS_DEVELOPER_INCLUDE_DIR"
        ) {
            return false;
        }
        rest = &tail[end + 1..];
    }
    true
}

fn source_relative_directory(path: &Path) -> Result<String, String> {
    if path.is_absolute() {
        return Err("source-text mmakefile directory must be source-relative".into());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(part) = component else {
            if matches!(component, Component::CurDir) {
                continue;
            }
            return Err("source-text mmakefile directory escapes the source root".into());
        };
        let part = part
            .to_str()
            .ok_or("source-text mmakefile directory is not UTF-8")?;
        if part.is_empty()
            || !part
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_.+-".contains(character))
        {
            return Err("source-text mmakefile directory has an unsafe component".into());
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn owner_hint(content: &str) -> String {
    content
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("#MM- ")
                .or_else(|| line.trim().strip_prefix("#MM "))
        })
        .filter_map(|line| line.split_once(':').map(|(owner, _)| owner.trim()))
        .find(|owner| safe_target_name(owner))
        .unwrap_or("<unknown-owner>")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_with_context;
    use crate::parser::TargetContext;
    use std::fs;
    use tempfile::TempDir;

    fn fixture_root() -> TempDir {
        let root = tempfile::tempdir().expect("temporary source root");
        fs::create_dir_all(root.path().join("config")).expect("config directory");
        fs::write(
            root.path().join("config/make.cfg.in"),
            "AROS_DIR_DEVELOPER := Developer\nAROS_DIR_INCLUDE := include\nAROS_DIR_LIB := lib\nAROSDIR := $(TARGETDIR)/SYS\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\nAROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\nGENINCDIR := $(GENDIR)/include\n",
        )
        .expect("config paths");
        root
    }

    fn scan(
        root: &TempDir,
        source: &str,
        line_states: Option<&[ConditionalTruth]>,
    ) -> (Vec<SourceTextRuleDecl>, Vec<SourceTextRuleRejection>) {
        let target = TargetContext::default();
        let scope = collect_vars_with_context(source, &target);
        let dirs = DirVars::load(root.path());
        collect_source_text_rules_with_context(
            source,
            root.path(),
            Path::new("workbench/libs/renamed"),
            &scope,
            &dirs,
            line_states,
        )
    }

    fn producer_source() -> String {
        r#"INPUT_ROOT := $(PORTSDIR)/archive/renamed
TOOL_VERSION := 7.4
%fetch mmake=renamed-fetch archive=renamed destination=$(INPUT_ROOT)
#MM- renamed-products : renamed-fetch
$(AROS_INCLUDES)/renamed/options.h : $(INPUT_ROOT)/options.in
	@$(ECHO) "Generating renamed options ..."
	%mkdir_q dir="$(AROS_INCLUDES)/renamed"
	@$(SED) -e "s|.*OPTION_ONE.*|#define OPTION_ONE \\n|g" -e "s|.*OPTION_TWO.*|#define OPTION_TWO\n|g" $< > $@
$(TOOLDIR)/$(AROS_TARGET_CPU)-$(AROS_TARGET_ARCH)/renamed-config : $(INPUT_ROOT)/config.in
	@$(ECHO) "Generating renamed-config ..."
	%mkdir_q dir="$(TOOLDIR)/$(AROS_TARGET_CPU)-$(AROS_TARGET_ARCH)"
	@$(SED) -e "s|dynamic_libs=\"-lfixture\"|dynamic_libs=\"-lfixture2\"|g" -e "s|%prefix%|$(AROS_DEVELOPER)|g" -e "s|%includedir%|$(AROS_INCLUDES)|g" -e "s|%libdir%|$(AROS_LIB)|g" -e "s|%version%|$(TOOL_VERSION)|g" $< > $@
	@chmod 744 $@
renamed-products : $(AROS_INCLUDES)/renamed/options.h $(TOOLDIR)/$(AROS_TARGET_CPU)-$(AROS_TARGET_ARCH)/renamed-config
	@$(NOP)
"#
        .to_owned()
    }

    #[test]
    fn scans_arbitrary_named_multi_output_make_chain_and_preserves_sed_semantics() {
        let root = fixture_root();
        let source = producer_source();
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(rejections.is_empty(), "{rejections:#?}");
        assert_eq!(declarations.len(), 1);
        let declaration = &declarations[0];
        assert_eq!(declaration.owner, "renamed-products");
        assert_eq!(declaration.fetch_owner, "renamed-fetch");
        assert_eq!(declaration.outputs.len(), 2);
        let header = &declaration.outputs[0];
        assert_eq!(header.output, "${AROS_SDK_INCLUDE_DIR}/renamed/options.h");
        assert!(header.input.ends_with("/archive/renamed/options.in"));
        assert_eq!(header.mode, None);
        assert_eq!(
            header.operations,
            vec![
                SourceTextOperation::ReplaceWholeLineContaining {
                    token: "OPTION_ONE".into(),
                    replacement: "#define OPTION_ONE \n".into(),
                },
                SourceTextOperation::ReplaceWholeLineContaining {
                    token: "OPTION_TWO".into(),
                    replacement: "#define OPTION_TWO\n".into(),
                },
            ]
        );
        let script = &declaration.outputs[1];
        assert!(script
            .output
            .ends_with("/hosttools/${AROS_TARGET_CPU}-${AROS_TARGET_PLATFORM}/renamed-config"));
        assert!(script.input.ends_with("/archive/renamed/config.in"));
        assert_eq!(script.mode.as_deref(), Some("744"));
        assert_eq!(
            script.operations[0],
            SourceTextOperation::ReplaceAll {
                token: "dynamic_libs=\"-lfixture\"".into(),
                replacement: "dynamic_libs=\"-lfixture2\"".into(),
            }
        );
        assert_eq!(
            script.operations[1],
            SourceTextOperation::ReplaceAll {
                token: "%prefix%".into(),
                replacement: "${AROS_BUILD_DIR}/SYS/Developer".into(),
            }
        );
        assert_eq!(
            script.operations[2],
            SourceTextOperation::ReplaceAll {
                token: "%includedir%".into(),
                replacement: "${AROS_SDK_INCLUDE_DIR}".into(),
            }
        );
        assert_eq!(
            script.operations[3],
            SourceTextOperation::ReplaceAll {
                token: "%libdir%".into(),
                replacement: "${AROS_BUILD_DIR}/SYS/Developer/lib".into(),
            }
        );
        assert_eq!(
            script.operations[4],
            SourceTextOperation::ReplaceAll {
                token: "%version%".into(),
                replacement: "7.4".into(),
            }
        );
    }

    #[test]
    fn accepts_virtual_mm_fetch_edges_used_by_source_aggregates() {
        let root = fixture_root();
        let source = producer_source();
        assert!(source.contains("#MM- renamed-products : renamed-fetch"));
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(rejections.is_empty(), "{rejections:#?}");
        assert_eq!(declarations.len(), 1);
    }

    #[test]
    fn normalizes_generated_include_outputs_to_engine_roots() {
        let root = fixture_root();
        let source = producer_source()
            .replace(
                "$(AROS_INCLUDES)/renamed/options.h",
                "$(GENINCDIR)/renamed/options.h",
            )
            .replace(
                "dir=\"$(AROS_INCLUDES)/renamed\"",
                "dir=\"$(GENINCDIR)/renamed\"",
            );
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(rejections.is_empty(), "{rejections:#?}");
        assert_eq!(declarations.len(), 1);
        assert_eq!(
            declarations[0].outputs[0].output,
            "${AROS_GENINC_DIR}/renamed/options.h"
        );
    }

    #[test]
    fn missing_second_product_rejects_the_aggregate_atomically() {
        let root = fixture_root();
        let source = producer_source();
        let start = source
            .find("$(TOOLDIR)/$(AROS_TARGET_CPU)-$(AROS_TARGET_ARCH)/renamed-config :")
            .expect("script rule");
        let end = source.find("\nrenamed-products :").expect("aggregate rule");
        let source = format!("{}{}", &source[..start], &source[end + 1..]);
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(declarations.is_empty());
        assert!(rejections.iter().any(|item| {
            item.owner == "renamed-products" && item.reason.contains("no ordinary file rule")
        }));
    }

    #[test]
    fn duplicate_named_owner_cannot_be_hidden_by_an_unrelated_sibling_rule() {
        let root = fixture_root();
        let source = format!("renamed-products : unrelated\n{}", producer_source());
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(declarations.is_empty());
        assert!(rejections.iter().any(|item| {
            item.owner == "renamed-products"
                && item.reason.contains("exactly one ordinary Make rule")
        }));
    }

    #[test]
    fn rejects_injected_shell_commands_unknown_conditions_and_extra_chmod() {
        let root = fixture_root();
        let source = producer_source().replace(
            "\t%mkdir_q dir=\"$(AROS_INCLUDES)/renamed\"\n",
            "\t%mkdir_q dir=\"$(AROS_INCLUDES)/renamed\"\n\t@touch unexpected\n",
        );
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.owner == "renamed-products"));

        let source = producer_source().replace("$(AROS_DEVELOPER)", "$${AROS_BUILD_DIR}/untrusted");
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.owner == "renamed-products"));

        let source = producer_source();
        let mut states = vec![ConditionalTruth::True; source.lines().count()];
        let recipe_line = source
            .lines()
            .position(|line| line.contains("@$(SED) -e \"s|.*OPTION_ONE"))
            .expect("first sed line");
        states[recipe_line] = ConditionalTruth::Unknown;
        let (declarations, rejections) = scan(&root, &source, Some(&states));
        assert!(declarations.is_empty());
        assert!(rejections.iter().any(|item| {
            item.owner == "renamed-products" && item.reason.contains("undecided Make conditional")
        }));

        let source = producer_source().replace(
            "\t@chmod 744 $@\nrenamed-products :",
            "\t@chmod 744 $@\n\t@chmod 755 $@\nrenamed-products :",
        );
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.owner == "renamed-products"));
    }

    #[test]
    fn rejects_unsafe_and_additional_aggregate_products() {
        let root = fixture_root();
        let source = producer_source().replace(
            "$(TOOLDIR)/$(AROS_TARGET_CPU)-$(AROS_TARGET_ARCH)/renamed-config",
            "$(TOOLDIR)/../outside/renamed-config",
        );
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| { item.owner == "renamed-products" && item.reason.contains("unsafe") }));

        let source = producer_source().replace(
            "renamed-products : $(AROS_INCLUDES)/renamed/options.h",
            "renamed-products : $(AROS_LIB)/unexpected.pc $(AROS_INCLUDES)/renamed/options.h",
        );
        let (declarations, rejections) = scan(&root, &source, None);
        assert!(declarations.is_empty());
        assert!(rejections.iter().any(|item| {
            item.owner == "renamed-products" && item.reason.contains("outside configured")
        }));
    }

    #[test]
    fn does_not_claim_single_output_sdk_text_producers() {
        let root = fixture_root();
        let source = r#"INPUT_ROOT := $(PORTSDIR)/archive/renamed
%fetch mmake=renamed-fetch destination=$(INPUT_ROOT)
#MM renamed-pkgconfig : renamed-fetch
$(AROS_LIB)/pkgconfig/renamed.pc : $(INPUT_ROOT)/renamed.pc.in
	@$(SED) -e "s|%version%|7.4|g" $< > $@
renamed-pkgconfig : $(AROS_LIB)/pkgconfig/renamed.pc
	@$(NOP)
"#;
        let (declarations, rejections) = scan(&root, source, None);
        assert!(declarations.is_empty());
        assert!(rejections.is_empty(), "{rejections:#?}");
    }
}
