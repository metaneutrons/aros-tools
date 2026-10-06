//! Closed model for finite SDK bin aliases and literal man-page stubs.
//!
//! This recognizes only source-local `%rule_copy` declarations and literal
//! `$(ECHO)` redirections into configured Developer/bin or Developer/man/man1
//! files, gathered under an ordinary bare-`#MM` aggregate. It never runs Make
//! or shell commands and does not infer the source producer of a copied file.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{variable_assignment, ConditionalTruth, VarScope};
use crate::parser::{macro_arg, macro_invocations};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

const BIN_ALIAS: &str = "${AROS_DEVELOPER_BIN_DIR}";
const MAN1_ALIAS: &str = "${AROS_DEVELOPER_MAN1_DIR}";
const MAX_SOURCE_BYTES: usize = 1_048_576;
const MAX_SOURCE_LINES: usize = 20_000;
const MAX_OUTPUTS: usize = 512;
const MAX_TEXT_BYTES: usize = 512;

/// One validated source-owned ordinary `#MM` aggregate for SDK file assets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkAssetRuleDecl {
    /// Literal MetaMake owner.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of the ordinary owner rule.
    pub line: usize,
    /// Operations in the order listed by the owner rule.
    pub operations: Vec<SdkAssetOperationDecl>,
}

/// One bounded file-producing operation owned by an SDK asset aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkAssetOperationDecl {
    /// One-based line of `%rule_copy` or the text-producing Make rule.
    pub line: usize,
    /// The operation and its normalized output path.
    pub operation: SdkAssetOperation,
}

/// Supported SDK asset operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkAssetOperation {
    /// Copy one configured Developer/bin file to another file in that root.
    Copy { input: String, output: String },
    /// Write one safe literal line to a configured Developer/man/man1 file.
    WriteText { output: String, text: String },
}

/// A relevant SDK asset producer that cannot be represented safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkAssetRuleRejection {
    /// Best available owning `#MM` target, or `<unknown>`.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of the closest producer or aggregate declaration.
    pub line: usize,
    /// Why the declaration was refused.
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetRoot {
    Bin,
    Man1,
}

impl AssetRoot {
    const fn alias(self) -> &'static str {
        match self {
            Self::Bin => BIN_ALIAS,
            Self::Man1 => MAN1_ALIAS,
        }
    }

    const fn directory(self) -> &'static str {
        match self {
            Self::Bin => "bin",
            Self::Man1 => "man/man1",
        }
    }
}

#[derive(Debug, Clone)]
struct ClassifiedPath {
    root: AssetRoot,
    /// Expanded spelling used to match producers to owner prerequisites.
    key: String,
    /// CMake-rooted path, present only for one safe direct file.
    normalized: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct Producer {
    key: String,
    line: usize,
    operation: Option<SdkAssetOperation>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct RecipeLine {
    text: String,
    line: usize,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct MakeRule {
    target: String,
    prerequisites: String,
    line: usize,
    state: ConditionalTruth,
    recipes: Vec<RecipeLine>,
}

#[derive(Debug, Clone)]
struct Aggregate {
    owner: String,
    line: usize,
    state: ConditionalTruth,
    rule: MakeRule,
    outputs: Vec<ClassifiedPath>,
    error: Option<String>,
}

struct AggregateContext<'a> {
    states: Option<&'a [ConditionalTruth]>,
    hidden: &'a [bool],
    rules: &'a [MakeRule],
    scope: &'a VarScope,
    roots: &'a [(AssetRoot, String); 2],
    root: &'a Path,
    dirs: &'a DirVars,
    rel_dir: &'a Path,
}

/// Collects source-local SDK asset aggregates from a continuation-joined
/// mmakefile snapshot. `line_states` must describe this same snapshot; absent
/// or out-of-range states are treated as unknown.
#[must_use]
pub(crate) fn collect_from_snapshot(
    content: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SdkAssetRuleDecl>, Vec<SdkAssetRuleRejection>) {
    let file = match source_file(rel_dir) {
        Ok(file) => file,
        Err(reason) => {
            return if looks_relevant(content) {
                (Vec::new(), vec![rejection("<unknown>", rel_dir, 1, reason)])
            } else {
                (Vec::new(), Vec::new())
            };
        }
    };
    if content.len() > MAX_SOURCE_BYTES || content.lines().count() > MAX_SOURCE_LINES {
        return if looks_relevant(content) {
            (
                Vec::new(),
                vec![SdkAssetRuleRejection {
                    owner: "<unknown>".into(),
                    file,
                    line: 1,
                    reason: "SDK asset source exceeds bounded parser limits".into(),
                }],
            )
        } else {
            (Vec::new(), Vec::new())
        };
    }
    let lines: Vec<_> = content.lines().collect();
    if lines.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let Some(bin_root) = dirs.expand("$(AROS_DEVELOPER)/bin") else {
        return unresolved_root(
            content,
            &file,
            "configured Developer/bin root is unresolved",
        );
    };
    let Some(man1_root) = dirs.expand("$(AROS_DEVELOPER)/man/man1") else {
        return unresolved_root(
            content,
            &file,
            "configured Developer/man/man1 root is unresolved",
        );
    };
    let roots = [(AssetRoot::Bin, bin_root), (AssetRoot::Man1, man1_root)];
    let hidden = hidden_make_lines(&lines);
    let control_error = source_control_error(&lines, &hidden);
    let shadowed = shadowed_commands(&lines, line_states, &hidden);
    let rules = parse_make_rules(&lines, line_states, &hidden);
    let context_at = |line| MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let invocations = macro_invocations(content);
    let mut producers = Vec::new();

    for invocation in invocations.iter().filter(|item| item.name == "rule_copy") {
        if hidden.get(invocation.line).copied().unwrap_or(true)
            || !lines
                .get(invocation.line)
                .is_some_and(|line| line.starts_with("%rule_copy"))
        {
            continue;
        }
        let make_context = context_at(invocation.line);
        let Some(raw_to) = macro_arg(&invocation.args, "to") else {
            continue;
        };
        let Some(output) = classify_path(&raw_to, &make_context, scope, invocation.line, &roots)
        else {
            continue;
        };
        let mut error = output.error.clone();
        if state_at(line_states, invocation.line) != ConditionalTruth::True {
            error.get_or_insert_with(|| "`%rule_copy` is conditional or has unknown state".into());
        }
        if let Some(reason) = &control_error {
            error.get_or_insert_with(|| reason.clone());
        }
        if shadowed.contains("CP") {
            error.get_or_insert_with(|| "source redefines Make copy command `CP`".into());
        }
        if has_target_specific_shadow(&output.key, "CP", &rules, scope, dirs, root, rel_dir) {
            error.get_or_insert_with(|| "copy output has a target-specific `CP` override".into());
        }
        let operation = match exact_arguments(&invocation.args, &["from", "to"]) {
            Ok(arguments) => {
                let raw_from = arguments.get("from").expect("validated field");
                let from = classify_path(raw_from, &make_context, scope, invocation.line, &roots);
                match from {
                    Some(from)
                        if output.root == AssetRoot::Bin
                            && from.root == AssetRoot::Bin
                            && output.normalized.is_some()
                            && from.normalized.is_some() =>
                    {
                        if let Some(reason) = from.error {
                            error.get_or_insert(reason);
                        }
                        Some(SdkAssetOperation::Copy {
                            input: from.normalized.unwrap(),
                            output: output.normalized.clone().unwrap(),
                        })
                    }
                    _ => {
                        error.get_or_insert_with(|| {
                            "`%rule_copy` must copy between direct files in Developer/bin".into()
                        });
                        None
                    }
                }
            }
            Err(reason) => {
                error.get_or_insert(reason);
                None
            }
        };
        producers.push(Producer {
            key: output.key,
            line: invocation.line,
            operation,
            error,
        });
    }

    for rule in &rules {
        if rule.state == ConditionalTruth::False {
            continue;
        }
        let make_context = context_at(rule.line);
        let Some(output) = classify_path(&rule.target, &make_context, scope, rule.line, &roots)
        else {
            continue;
        };
        if output.root != AssetRoot::Man1 {
            continue;
        }
        let mut error = output.error.clone();
        if rule.state != ConditionalTruth::True
            || rule
                .recipes
                .iter()
                .any(|recipe| recipe.state != ConditionalTruth::True)
        {
            error.get_or_insert_with(|| "man-page rule or recipe is conditional or unknown".into());
        }
        if let Some(reason) = &control_error {
            error.get_or_insert_with(|| reason.clone());
        }
        if shadowed.contains("ECHO") {
            error.get_or_insert_with(|| "source redefines Make echo command `ECHO`".into());
        }
        if has_target_specific_shadow(&output.key, "ECHO", &rules, scope, dirs, root, rel_dir) {
            error.get_or_insert_with(|| {
                "man-page output has a target-specific `ECHO` override".into()
            });
        }
        if !exact_man_order_only(rule, &output, &make_context, &roots) {
            error.get_or_insert_with(|| {
                "man-page output requires exactly its own Developer/man/man1 directory as order-only prerequisite".into()
            });
        }
        let (operation, recipe_error) =
            parse_text_recipe(rule, &output, scope, dirs, root, rel_dir, &shadowed);
        if let Some(reason) = recipe_error {
            error.get_or_insert(reason);
        }
        producers.push(Producer {
            key: output.key,
            line: rule.line,
            operation,
            error,
        });
    }

    let mut aggregates = collect_aggregates(
        &lines,
        &AggregateContext {
            states: line_states,
            hidden: &hidden,
            rules: &rules,
            scope,
            roots: &roots,
            root,
            dirs,
            rel_dir,
        },
    );
    for aggregate in &mut aggregates {
        if let Some(reason) = &control_error {
            aggregate.error.get_or_insert_with(|| reason.clone());
        }
        if aggregate
            .rule
            .recipes
            .iter()
            .any(|recipe| recipe.text == "\t@$(NOP)")
            && shadowed.contains("NOP")
        {
            aggregate
                .error
                .get_or_insert_with(|| "source redefines Make no-op command `NOP`".into());
        }
        if has_target_specific_shadow(&aggregate.owner, "NOP", &rules, scope, dirs, root, rel_dir) {
            aggregate.error.get_or_insert_with(|| {
                "SDK asset owner has a target-specific `NOP` override".into()
            });
        }
    }

    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    let mut used = BTreeSet::new();
    let mut owners_by_key = BTreeMap::<String, String>::new();
    let mut claims_by_key = BTreeMap::<String, usize>::new();
    for aggregate in &aggregates {
        for output in &aggregate.outputs {
            *claims_by_key.entry(output.key.clone()).or_default() += 1;
        }
    }
    for aggregate in aggregates {
        if aggregate.outputs.is_empty() {
            continue;
        }
        let mut failure = aggregate.error;
        if aggregate.state != ConditionalTruth::True
            || aggregate.rule.state != ConditionalTruth::True
        {
            failure.get_or_insert_with(|| {
                "SDK asset owner marker or rule is conditional or unknown".into()
            });
        }
        if !safe_owner(&aggregate.owner) {
            failure.get_or_insert_with(|| "SDK asset owner is not a safe literal target".into());
        }
        if !valid_aggregate_recipe(&aggregate.rule) {
            failure.get_or_insert_with(|| {
                "SDK asset owner permits no recipe or exactly one `@$(NOP)`".into()
            });
        }
        let mut operations = Vec::new();
        let mut seen_outputs = BTreeSet::new();
        for output in &aggregate.outputs {
            owners_by_key
                .entry(output.key.clone())
                .or_insert_with(|| aggregate.owner.clone());
            if claims_by_key.get(&output.key).copied().unwrap_or_default() > 1 {
                failure.get_or_insert_with(|| {
                    "SDK asset output is listed by multiple aggregates".into()
                });
            }
            if !seen_outputs.insert(output.key.as_str()) {
                failure.get_or_insert_with(|| "SDK asset owner repeats an output".into());
                continue;
            }
            if output.normalized.is_none() || output.error.is_some() {
                failure.get_or_insert_with(|| {
                    output
                        .error
                        .clone()
                        .unwrap_or_else(|| "unsafe aggregate output".into())
                });
                continue;
            }
            let matches = producers
                .iter()
                .enumerate()
                .filter(|(_, producer)| producer.key == output.key)
                .collect::<Vec<_>>();
            let [(producer_index, producer)] = matches.as_slice() else {
                failure.get_or_insert_with(|| {
                    if matches.is_empty() {
                        format!(
                            "SDK asset output `{}` has no supported producer",
                            output.key
                        )
                    } else {
                        format!("SDK asset output `{}` has multiple producers", output.key)
                    }
                });
                continue;
            };
            used.insert(*producer_index);
            if let Some(reason) = &producer.error {
                failure.get_or_insert_with(|| reason.clone());
                continue;
            }
            let Some(operation) = &producer.operation else {
                failure.get_or_insert_with(|| "SDK asset producer is not fully proven".into());
                continue;
            };
            operations.push(SdkAssetOperationDecl {
                line: producer.line + 1,
                operation: operation.clone(),
            });
        }
        if let Some(reason) = failure {
            rejections.push(SdkAssetRuleRejection {
                owner: aggregate.owner,
                file: file.clone(),
                line: aggregate.line + 1,
                reason,
            });
            continue;
        }
        if operations.len() != aggregate.outputs.len() {
            rejections.push(SdkAssetRuleRejection {
                owner: aggregate.owner,
                file: file.clone(),
                line: aggregate.line + 1,
                reason: "not every aggregate output has exactly one complete producer".into(),
            });
            continue;
        }
        declarations.push(SdkAssetRuleDecl {
            owner: aggregate.owner,
            file: file.clone(),
            line: aggregate.line + 1,
            operations,
        });
    }
    for (index, producer) in producers.iter().enumerate() {
        if used.contains(&index) {
            continue;
        }
        rejections.push(SdkAssetRuleRejection {
            owner: owners_by_key
                .get(&producer.key)
                .cloned()
                .unwrap_or_else(|| "<unknown>".into()),
            file: file.clone(),
            line: producer.line + 1,
            reason: producer
                .error
                .clone()
                .unwrap_or_else(|| "SDK asset producer has no unique `#MM` aggregate".into()),
        });
    }
    declarations.sort_by(|left, right| (left.line, &left.owner).cmp(&(right.line, &right.owner)));
    rejections.sort_by(|left, right| {
        (left.line, &left.owner, &left.reason).cmp(&(right.line, &right.owner, &right.reason))
    });
    (declarations, rejections)
}

fn unresolved_root(
    content: &str,
    file: &str,
    reason: &str,
) -> (Vec<SdkAssetRuleDecl>, Vec<SdkAssetRuleRejection>) {
    if !looks_relevant(content) {
        return (Vec::new(), Vec::new());
    }
    (
        Vec::new(),
        vec![SdkAssetRuleRejection {
            owner: "<unknown>".into(),
            file: file.to_owned(),
            line: 1,
            reason: reason.into(),
        }],
    )
}

fn source_file(rel_dir: &Path) -> Result<String, String> {
    if rel_dir.is_absolute()
        || rel_dir
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("SDK asset source directory is not source-relative".into());
    }
    let directory = rel_dir.to_string_lossy().replace('\\', "/");
    Ok(if directory.is_empty() {
        "mmakefile.src".into()
    } else {
        format!("{directory}/mmakefile.src")
    })
}

fn rejection(owner: &str, rel_dir: &Path, line: usize, reason: String) -> SdkAssetRuleRejection {
    SdkAssetRuleRejection {
        owner: owner.into(),
        file: source_file(rel_dir).unwrap_or_else(|_| rel_dir.to_string_lossy().into_owned()),
        line,
        reason,
    }
}

fn looks_relevant(content: &str) -> bool {
    content.contains("%rule_copy")
        || (content.contains("AROS_DEVELOPER")
            && (content.contains("/bin/") || content.contains("/man/man1")))
}

fn state_at(states: Option<&[ConditionalTruth]>, line: usize) -> ConditionalTruth {
    states
        .and_then(|states| states.get(line))
        .copied()
        .unwrap_or(ConditionalTruth::Unknown)
}

fn classify_path(
    raw: &str,
    context: &MakeExprContext<'_>,
    scope: &VarScope,
    line: usize,
    roots: &[(AssetRoot, String); 2],
) -> Option<ClassifiedPath> {
    let alias_text = expand_local_aliases(raw, scope, line, 16, &mut BTreeSet::new());
    let resolved = evaluate_make_expr(raw, context).ok();
    let exact = roots.iter().find_map(|(root, base)| {
        resolved
            .as_deref()
            .and_then(|path| normalize_file(path, base, root.alias()).map(|_| *root))
    });
    let family = exact.or_else(|| {
        if !mentions_developer_root(&alias_text)
            && !scope.is_known_local("AROS_DEVELOPER")
            && !scope.is_known_local("AROS_DIR_DEVELOPER")
        {
            return None;
        }
        let path_text = resolved.as_deref().unwrap_or(&alias_text);
        if path_text.contains("/man/man1/") || alias_text.contains("/man/man1/") {
            Some(AssetRoot::Man1)
        } else if path_text.contains("/bin/")
            || alias_text.contains("/bin/")
            || alias_text.contains("AROS_DEVELOPER_BIN_DIR")
        {
            Some(AssetRoot::Bin)
        } else if alias_text.contains("AROS_DEVELOPER_MAN1_DIR") {
            Some(AssetRoot::Man1)
        } else {
            None
        }
    })?;
    let base = roots.iter().find(|(root, _)| *root == family)?.1.as_str();
    let mut error = None;
    let (key, normalized) = if let Some(path) = resolved {
        let normalized = normalize_file(&path, base, family.alias());
        if normalized.is_none() {
            error = Some(format!(
                "SDK asset path `{path}` is not one safe direct file below configured Developer/{}",
                family.directory()
            ));
        }
        (path, normalized)
    } else {
        error = Some(format!("cannot resolve SDK asset path `{raw}`"));
        (alias_text, None)
    };
    if normalized.is_some() {
        let suffix = match family {
            AssetRoot::Bin => "/bin",
            AssetRoot::Man1 => "/man/man1",
        };
        let configured = format!("$(AROS_DEVELOPER){suffix}");
        if evaluate_make_expr(&configured, context).is_ok_and(|value| value != base) {
            error = Some("source overrides the configured Developer asset root".into());
        }
    }
    Some(ClassifiedPath {
        root: family,
        key,
        normalized,
        error,
    })
}

fn normalize_file(path: &str, base: &str, alias: &str) -> Option<String> {
    let leaf = path.strip_prefix(base)?.strip_prefix('/')?;
    if leaf.contains('/')
        || !safe_basename(leaf)
        || path.contains([';', '\\', '\n', '\r', '"', '\'', '`'])
    {
        return None;
    }
    Some(format!("{alias}/{leaf}"))
}

fn safe_basename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
}

fn mentions_developer_root(raw: &str) -> bool {
    [
        "AROS_DEVELOPER",
        "AROS_DIR_DEVELOPER",
        "AROS_DEVELOPER_BIN_DIR",
        "AROS_DEVELOPER_MAN1_DIR",
    ]
    .iter()
    .any(|name| raw.contains(name))
}

fn expand_local_aliases(
    raw: &str,
    scope: &VarScope,
    line: usize,
    depth: usize,
    visiting: &mut BTreeSet<String>,
) -> String {
    if depth == 0 || !raw.contains("$(") {
        return raw.to_owned();
    }
    let mut output = String::new();
    let mut cursor = 0;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find("$(") else {
            output.push_str(&raw[cursor..]);
            break;
        };
        let start = cursor + relative;
        output.push_str(&raw[cursor..start]);
        let Some((body, end)) = make_reference(raw, start) else {
            output.push_str(&raw[start..]);
            break;
        };
        if developer_anchor(body) {
            // Preserve the configured root's identity even when the source
            // locally rebinds it; the resolved path then fails closed below.
            output.push_str(&raw[start..end]);
        } else if simple_make_name(body) && visiting.insert(body.to_owned()) {
            if let Some(value) = scope.raw_at(body, line) {
                output.push_str(&expand_local_aliases(
                    &value,
                    scope,
                    line,
                    depth - 1,
                    visiting,
                ));
            } else {
                output.push_str(&raw[start..end]);
            }
            visiting.remove(body);
        } else {
            output.push_str(&raw[start..end]);
        }
        cursor = end;
    }
    output
}

fn make_reference(raw: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = raw.as_bytes();
    let mut depth = 1usize;
    let mut cursor = start + 2;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' {
            depth -= 1;
            if depth == 0 {
                return Some((&raw[start + 2..cursor], cursor + 1));
            }
        }
        cursor += 1;
    }
    None
}

fn simple_make_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn developer_anchor(name: &str) -> bool {
    matches!(
        name,
        "AROS_DEVELOPER"
            | "AROS_DIR_DEVELOPER"
            | "AROS_DEVELOPER_BIN_DIR"
            | "AROS_DEVELOPER_MAN1_DIR"
    )
}

fn exact_arguments(raw: &str, expected: &[&str]) -> Result<BTreeMap<String, String>, String> {
    if raw.len() > 4096 || raw.contains(['\n', '\r', ';', '\\', '`']) {
        return Err("macro arguments contain unsafe syntax".into());
    }
    let mut rest = raw.trim();
    let mut values = BTreeMap::new();
    while !rest.is_empty() {
        let Some((key, tail)) = rest.split_once('=') else {
            return Err("macro arguments contain an unkeyed token".into());
        };
        if !expected.contains(&key) || values.contains_key(key) {
            return Err("macro arguments contain an unsupported or duplicate field".into());
        }
        let (value, after) = if let Some(quoted) = tail.strip_prefix('"') {
            let Some((value, after)) = quoted.split_once('"') else {
                return Err("macro argument has an unterminated quote".into());
            };
            if !after.is_empty() && !after.starts_with(char::is_whitespace) {
                return Err("macro argument has unsafe text after a quoted value".into());
            }
            (value, after)
        } else {
            let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
            let (value, after) = tail.split_at(end);
            if value.is_empty() || value.contains(['"', '\'']) {
                return Err("macro argument is not one scalar value".into());
            }
            (value, after)
        };
        if value.is_empty() {
            return Err("macro argument value is empty".into());
        }
        values.insert(key.to_owned(), value.to_owned());
        rest = after.trim_start();
    }
    if values.len() != expected.len() {
        return Err("macro invocation omits a required field".into());
    }
    Ok(values)
}

fn exact_man_order_only(
    rule: &MakeRule,
    output: &ClassifiedPath,
    context: &MakeExprContext<'_>,
    roots: &[(AssetRoot, String); 2],
) -> bool {
    let Some((regular, order_only)) = rule.prerequisites.split_once('|') else {
        return false;
    };
    if !regular.trim().is_empty() || order_only.split_whitespace().count() != 1 {
        return false;
    }
    let Some(base) = roots
        .iter()
        .find(|(root, _)| *root == output.root)
        .map(|(_, base)| base)
    else {
        return false;
    };
    evaluate_make_expr(order_only.trim(), context).is_ok_and(|path| path == *base)
        && !order_only.trim().contains([';', '\\', '"', '\'', '`'])
}

fn parse_text_recipe(
    rule: &MakeRule,
    output: &ClassifiedPath,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    shadowed: &BTreeSet<&'static str>,
) -> (Option<SdkAssetOperation>, Option<String>) {
    if output.root != AssetRoot::Man1 || output.normalized.is_none() {
        return (
            None,
            Some("text output is not a configured Developer/man/man1 file".into()),
        );
    }
    let messages: Vec<_> = rule
        .recipes
        .iter()
        .filter(|line| {
            line.text
                .trim_start_matches('\t')
                .starts_with("%fileactionmsg")
        })
        .collect();
    let echoes: Vec<_> = rule
        .recipes
        .iter()
        .filter(|line| line.text.starts_with('\t'))
        .filter(|line| {
            line.text
                .strip_prefix('\t')
                .is_some_and(|text| text.starts_with("@$(ECHO)"))
        })
        .collect();
    if echoes.len() != 1
        || messages.len() > 1
        || rule.recipes.len() != echoes.len() + messages.len()
    {
        return (None, Some("man-page rule must contain one literal ECHO and at most one fileaction message, with no extra commands".into()));
    }
    let echo = echoes[0];
    if echo.state != ConditionalTruth::True || shadowed.contains("ECHO") {
        return (
            None,
            Some("ECHO recipe is conditional or source-shadowed".into()),
        );
    }
    let Some(command) = echo.text.strip_prefix("\t@$(ECHO) \"") else {
        return (
            None,
            Some("man-page recipe is not the exact quoted ECHO form".into()),
        );
    };
    let Some((text, raw_output)) = command.split_once("\" > ") else {
        return (
            None,
            Some("man-page ECHO recipe has no exact output redirection".into()),
        );
    };
    if !safe_text(text) || raw_output.is_empty() || raw_output.chars().any(char::is_whitespace) {
        return (
            None,
            Some("man-page text or output redirection is not one safe literal form".into()),
        );
    }
    let final_context = MakeExprContext::new(scope, dirs, usize::MAX, root, rel_dir);
    if evaluate_make_expr(raw_output, &final_context)
        .ok()
        .as_deref()
        != Some(output.key.as_str())
    {
        return (
            None,
            Some("man-page ECHO output differs from its declaration-time target".into()),
        );
    }
    if let Some(message) = messages.first() {
        if message.state != ConditionalTruth::True {
            return (
                None,
                Some("fileaction message is conditional or unknown".into()),
            );
        }
        let raw = message
            .text
            .trim_start_matches('\t')
            .strip_prefix("%fileactionmsg")
            .unwrap_or_default()
            .trim();
        let Ok(arguments) = exact_arguments(raw, &["msg", "file"]) else {
            return (
                None,
                Some("fileaction message has unsupported or duplicate arguments".into()),
            );
        };
        let context = MakeExprContext::new(scope, dirs, message.line, root, rel_dir);
        let Some(raw_file) = arguments.get("file") else {
            return (
                None,
                Some("fileaction message omits its output path".into()),
            );
        };
        if arguments.get("msg").map(String::as_str) != Some("Creating")
            || evaluate_make_expr(raw_file, &context).ok().as_deref() != Some(output.key.as_str())
        {
            return (
                None,
                Some("fileaction message does not match the output".into()),
            );
        }
    }
    (
        Some(SdkAssetOperation::WriteText {
            output: output.normalized.clone().unwrap(),
            text: text.to_owned(),
        }),
        None,
    )
}

fn safe_text(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TEXT_BYTES
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" ._+-:/=()".contains(&byte))
}

fn parse_make_rules(
    lines: &[&str],
    states: Option<&[ConditionalTruth]>,
    hidden: &[bool],
) -> Vec<MakeRule> {
    let mut rules: Vec<MakeRule> = Vec::new();
    let mut current: Option<usize> = None;
    for (line_no, raw) in lines.iter().enumerate() {
        if hidden.get(line_no).copied().unwrap_or(true) {
            continue;
        }
        if raw.starts_with('\t') {
            if let Some(index) = current {
                rules[index].recipes.push(RecipeLine {
                    text: (*raw).to_owned(),
                    line: line_no,
                    state: state_at(states, line_no),
                });
            }
            continue;
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if is_assignment(trimmed) || trimmed.starts_with('%') || make_directive(trimmed) {
            current = None;
            continue;
        }
        current = None;
        let Some((target, prerequisites)) = raw.split_once(':') else {
            continue;
        };
        let target = target.trim();
        if target.is_empty() || target.contains(':') {
            continue;
        }
        rules.push(MakeRule {
            target: target.to_owned(),
            prerequisites: prerequisites.trim().to_owned(),
            line: line_no,
            state: state_at(states, line_no),
            recipes: Vec::new(),
        });
        current = Some(rules.len() - 1);
    }
    rules
}

fn collect_aggregates(lines: &[&str], context: &AggregateContext<'_>) -> Vec<Aggregate> {
    let mut output = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        if context.hidden.get(index).copied().unwrap_or(true) || line.trim() != "#MM" {
            continue;
        }
        let Some(owner_rule_line) = lines.get(index + 1) else {
            continue;
        };
        let Some((raw_owner, _)) = owner_rule_line.split_once(':') else {
            continue;
        };
        let owner = raw_owner.trim();
        if !safe_owner(owner) {
            continue;
        }
        let Some(rule) = context
            .rules
            .iter()
            .find(|rule| rule.line == index + 1 && rule.target == owner)
        else {
            continue;
        };
        let make_context = MakeExprContext::new(
            context.scope,
            context.dirs,
            rule.line,
            context.root,
            context.rel_dir,
        );
        let words: Vec<_> = rule.prerequisites.split_whitespace().collect();
        if words.is_empty() || words.len() > MAX_OUTPUTS || words.contains(&"|") {
            continue;
        }
        let mut outputs = Vec::new();
        let owner_rule_count = context
            .rules
            .iter()
            .filter(|other| other.target == owner && other.state != ConditionalTruth::False)
            .count();
        let mut error = if owner_rule_count == 1 {
            None
        } else {
            Some("SDK asset owner has multiple ordinary Make rules".into())
        };
        let mut has_unsupported = false;
        for word in words {
            if let Some(path) =
                classify_path(word, &make_context, context.scope, rule.line, context.roots)
            {
                outputs.push(path);
            } else {
                has_unsupported = true;
            }
        }
        if outputs.is_empty() {
            continue;
        }
        if has_unsupported {
            error.get_or_insert_with(|| {
                "SDK asset aggregate mixes supported outputs with unrelated prerequisites".into()
            });
        }
        if !seen.insert(owner.to_owned()) {
            error.get_or_insert_with(|| "SDK asset owner has duplicate bare `#MM` markers".into());
        }
        output.push(Aggregate {
            owner: owner.to_owned(),
            line: index,
            state: state_at(context.states, index),
            rule: rule.clone(),
            outputs,
            error,
        });
    }
    output
}

fn valid_aggregate_recipe(rule: &MakeRule) -> bool {
    rule.recipes.is_empty()
        || (rule.recipes.len() == 1
            && rule.recipes[0].state == ConditionalTruth::True
            && rule.recipes[0].text == "\t@$(NOP)")
}

fn safe_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 160
        && owner.as_bytes()[0].is_ascii_alphanumeric()
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
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

fn source_control_error(lines: &[&str], hidden: &[bool]) -> Option<String> {
    for (index, raw) in lines.iter().enumerate() {
        if hidden.get(index).copied().unwrap_or(false) {
            return Some("source contains an opaque Make `define` body".into());
        }
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if line.contains("$(eval") || line.contains("${eval") {
            return Some("source contains Make `eval` with hidden parse-time effects".into());
        }
        if starts_include(line) && line != "include $(SRCDIR)/config/aros.cfg" {
            return Some("source imports an unmodelled Make fragment".into());
        }
    }
    None
}

fn starts_include(line: &str) -> bool {
    ["include", "-include", "sinclude"].iter().any(|directive| {
        line.strip_prefix(directive)
            .is_some_and(|tail| tail.starts_with(char::is_whitespace))
    })
}

fn shadowed_commands(
    lines: &[&str],
    states: Option<&[ConditionalTruth]>,
    hidden: &[bool],
) -> BTreeSet<&'static str> {
    let mut output = BTreeSet::new();
    for (index, raw) in lines.iter().enumerate() {
        if hidden.get(index).copied().unwrap_or(false)
            || state_at(states, index) == ConditionalTruth::False
        {
            continue;
        }
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some(name) =
            assignment_name(line).or_else(|| line.strip_prefix("undefine ").map(str::trim))
        {
            if let Some(command) = command_name(name) {
                output.insert(command);
            }
        }
    }
    output
}

fn has_target_specific_shadow(
    target: &str,
    command: &str,
    rules: &[MakeRule],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
) -> bool {
    rules.iter().any(|rule| {
        if assignment_name(&rule.prerequisites) != Some(command) {
            return false;
        }
        let context = MakeExprContext::new(scope, dirs, rule.line, root, rel_dir);
        evaluate_make_expr(&rule.target, &context).is_ok_and(|value| value == target)
    })
}

fn assignment_name(line: &str) -> Option<&str> {
    let mut body = line;
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

fn command_name(name: &str) -> Option<&'static str> {
    match name {
        "CP" => Some("CP"),
        "ECHO" => Some("ECHO"),
        "NOP" => Some("NOP"),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_impl;
    use crate::parser::join_continuations;
    use std::fs;
    use tempfile::tempdir;

    const CONFIG: &str = "AROS_DIR_AROS := SYS\nAROS_DIR_DEVELOPER := Developer\nAROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\n";

    fn setup(
        source: &str,
    ) -> (
        tempfile::TempDir,
        String,
        VarScope,
        DirVars,
        Vec<ConditionalTruth>,
    ) {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/make.cfg.in"), CONFIG).unwrap();
        let joined = join_continuations(source);
        let (scope, states) = collect_vars_impl(&joined, None);
        let dirs = DirVars::load(root.path());
        (root, joined, scope, dirs, states)
    }

    fn collect(source: &str) -> (Vec<SdkAssetRuleDecl>, Vec<SdkAssetRuleRejection>) {
        let (root, joined, scope, dirs, states) = setup(source);
        collect_from_snapshot(
            &joined,
            &scope,
            &dirs,
            root.path(),
            Path::new("external/bz2"),
            Some(&states),
        )
    }

    fn valid_source() -> String {
        concat!(
            "BIN_DIR := $(AROS_DEVELOPER)/bin\n",
            "MAN_DIR := $(AROS_DEVELOPER)/man/man1\n",
            "#MM\n",
            "fixture-bin : $(BIN_DIR)/bzcat\n",
            "%rule_copy from=$(BIN_DIR)/bzip2 to=$(BIN_DIR)/bzcat\n",
            "#MM\n",
            "fixture-man : $(MAN_DIR)/bzgrep.1\n",
            "$(MAN_DIR)/bzgrep.1: | $(MAN_DIR)\n",
            "\t%fileactionmsg msg=\"Creating\" file=\"$(MAN_DIR)/bzgrep.1\"\n",
            "\t@$(ECHO) \".so man1/bzgrep.1\" > $(MAN_DIR)/bzgrep.1\n"
        )
        .into()
    }

    #[test]
    fn accepts_exact_copy_and_literal_text_aggregates() {
        let (declarations, rejected) = collect(&valid_source());
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations[0].owner, "fixture-bin");
        assert_eq!(
            declarations[0].operations[0].operation,
            SdkAssetOperation::Copy {
                input: "${AROS_DEVELOPER_BIN_DIR}/bzip2".into(),
                output: "${AROS_DEVELOPER_BIN_DIR}/bzcat".into(),
            }
        );
        assert_eq!(declarations[1].owner, "fixture-man");
        assert_eq!(
            declarations[1].operations[0].operation,
            SdkAssetOperation::WriteText {
                output: "${AROS_DEVELOPER_MAN1_DIR}/bzgrep.1".into(),
                text: ".so man1/bzgrep.1".into(),
            }
        );
    }

    #[test]
    fn rejects_copy_argument_injection_and_wrong_roots() {
        for (replacement, owner) in [
            (
                "%rule_copy from=$(BIN_DIR)/bzip2 to=$(BIN_DIR)/bzcat extra=x",
                "fixture-bin",
            ),
            (
                "%rule_copy from=$(BIN_DIR)/bzip2 to=$(BIN_DIR)/bzcat to=$(BIN_DIR)/other",
                "fixture-bin",
            ),
            (
                "%rule_copy from=$(BIN_DIR)/bzip2;touch to=$(BIN_DIR)/bzcat",
                "fixture-bin",
            ),
            (
                "%rule_copy from=$(BIN_DIR)/bzip2 to=$(AROS_DEVELOPER)/lib/bzcat",
                "fixture-bin",
            ),
            (
                "%rule_copy from=$(BIN_DIR)/../bzip2 to=$(BIN_DIR)/bzcat",
                "fixture-bin",
            ),
        ] {
            let source = valid_source().replace(
                "%rule_copy from=$(BIN_DIR)/bzip2 to=$(BIN_DIR)/bzcat",
                replacement,
            );
            let (declarations, rejected) = collect(&source);
            assert!(
                !declarations.iter().any(|decl| decl.owner == owner),
                "{replacement}: {declarations:#?}"
            );
            assert!(
                rejected.iter().any(|item| item.owner == owner),
                "{replacement}: {rejected:#?}"
            );
        }
    }

    #[test]
    fn rejects_unknown_conditions_extra_commands_and_source_shadowing() {
        let cases = [
            (
                valid_source().replace("BIN_DIR :=", "CP := /bin/false\nBIN_DIR :="),
                "fixture-bin",
            ),
            (
                valid_source().replace("MAN_DIR :=", "ECHO := /bin/false\nMAN_DIR :="),
                "fixture-man",
            ),
            (
                valid_source().replace(
                    "#MM\nfixture-man",
                    "ifeq ($(UNKNOWN),yes)\n#MM\nfixture-man",
                ),
                "fixture-man",
            ),
            (
                valid_source().replace(
                    "\t@$(ECHO) \".so man1/bzgrep.1\" > $(MAN_DIR)/bzgrep.1\n",
                    "\t@$(ECHO) \".so man1/bzgrep.1\" > $(MAN_DIR)/bzgrep.1\n\t@touch /tmp/evil\n",
                ),
                "fixture-man",
            ),
            (
                valid_source().replace(
                    "fixture-man : $(MAN_DIR)/bzgrep.1\n",
                    "fixture-man : $(MAN_DIR)/bzgrep.1\n\t@$(NOP); touch x\n",
                ),
                "fixture-man",
            ),
        ];
        for (source, owner) in cases {
            let (declarations, rejected) = collect(&source);
            assert!(
                !declarations.iter().any(|decl| decl.owner == owner),
                "{source}: {declarations:#?}"
            );
            assert!(
                rejected.iter().any(|item| item.owner == owner),
                "{source}: {rejected:#?}"
            );
        }
    }

    #[test]
    fn rejects_root_override_late_recipe_change_and_opaque_controls() {
        let cases = [
            (
                valid_source().replace(
                    "BIN_DIR := $(AROS_DEVELOPER)/bin",
                    "AROS_DEVELOPER := $(TARGETDIR)/foreign\nBIN_DIR := $(AROS_DEVELOPER)/bin",
                ),
                vec!["fixture-bin", "fixture-man"],
            ),
            (
                format!("{}MAN_DIR := $(AROS_DEVELOPER)/elsewhere\n", valid_source()),
                vec!["fixture-man"],
            ),
            (
                format!("{}ECHO = /tmp/echo\n", valid_source()),
                vec!["fixture-man"],
            ),
            (
                format!("{}\ndefine hidden\nNOOP := opaque\nendef\n", valid_source()),
                vec!["fixture-bin", "fixture-man"],
            ),
            (
                format!(
                    "$(eval BIN_DIR := $(AROS_DEVELOPER)/elsewhere)\n{}",
                    valid_source()
                ),
                vec!["fixture-bin", "fixture-man"],
            ),
            (
                format!("include optional-rules.mk\n{}", valid_source()),
                vec!["fixture-bin", "fixture-man"],
            ),
        ];
        for (source, rejected_owners) in cases {
            let (declarations, rejected) = collect(&source);
            for owner in rejected_owners {
                assert!(
                    !declarations.iter().any(|decl| decl.owner == owner),
                    "{source}: {declarations:#?}"
                );
                assert!(
                    rejected.iter().any(|item| item.owner == owner),
                    "{source}: {rejected:#?}"
                );
            }
        }
    }

    #[test]
    fn rejects_target_specific_command_overrides() {
        let cases = [
            (
                valid_source().replace(
                    "fixture-bin : $(BIN_DIR)/bzcat\n",
                    "fixture-bin : $(BIN_DIR)/bzcat\n$(BIN_DIR)/bzcat: CP = /bin/false\n",
                ),
                "fixture-bin",
            ),
            (
                valid_source().replace(
                    "$(MAN_DIR)/bzgrep.1: | $(MAN_DIR)\n",
                    "$(MAN_DIR)/bzgrep.1: | $(MAN_DIR)\n$(MAN_DIR)/bzgrep.1: ECHO = /bin/false\n",
                ),
                "fixture-man",
            ),
            (
                valid_source().replace(
                    "fixture-bin : $(BIN_DIR)/bzcat\n",
                    "fixture-bin : $(BIN_DIR)/bzcat\nfixture-bin: NOP = /bin/false\n\t@$(NOP)\n",
                ),
                "fixture-bin",
            ),
        ];
        for (source, owner) in cases {
            let (declarations, rejected) = collect(&source);
            assert!(
                !declarations.iter().any(|decl| decl.owner == owner),
                "{source}: {declarations:#?}"
            );
            assert!(
                rejected.iter().any(|item| item.owner == owner),
                "{source}: {rejected:#?}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_sdk_output_producers_and_owner_claims() {
        let duplicate_producer = valid_source().replace(
            "#MM\nfixture-man",
            "%rule_copy from=$(BIN_DIR)/bzip2 to=$(BIN_DIR)/bzcat\n#MM\nfixture-man",
        );
        let duplicate_owner_output = valid_source().replace(
            "fixture-bin : $(BIN_DIR)/bzcat",
            "fixture-bin : $(BIN_DIR)/bzcat $(BIN_DIR)/bzcat",
        );
        let duplicate_aggregate = valid_source().replace(
            "#MM\nfixture-man :",
            "#MM\nfixture-bin-alias : $(BIN_DIR)/bzcat\n#MM\nfixture-man :",
        );
        for (source, owner) in [
            (duplicate_producer, "fixture-bin"),
            (duplicate_owner_output, "fixture-bin"),
            (duplicate_aggregate, "fixture-bin-alias"),
        ] {
            let (declarations, rejected) = collect(&source);
            assert!(
                !declarations.iter().any(|decl| decl.owner == owner),
                "{source}: {declarations:#?}"
            );
            assert!(
                rejected.iter().any(|item| item.owner == owner),
                "{source}: {rejected:#?}"
            );
        }
    }

    #[test]
    fn p4_bzip2_source_is_an_opt_in_probe() {
        let Some(source_root) = std::env::var_os("AROS_P4_SOURCE") else {
            return;
        };
        let source_root = Path::new(&source_root);
        let source =
            std::fs::read_to_string(source_root.join("external/bz2/mmakefile.src")).unwrap();
        let joined = join_continuations(&source);
        let (scope, states) = collect_vars_impl(&joined, None);
        let dirs = DirVars::load(source_root);
        let (declarations, rejected) = collect_from_snapshot(
            &joined,
            &scope,
            &dirs,
            source_root,
            Path::new("external/bz2"),
            Some(&states),
        );
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(declarations.len(), 2);
        assert!(declarations.iter().any(|item| {
            item.owner == "external-bz2-bzip2-install-aliases" && item.operations.len() == 6
        }));
        assert!(declarations.iter().any(|item| {
            item.owner == "external-bz2-bzip2-install-man" && item.operations.len() == 4
        }));
    }
}
