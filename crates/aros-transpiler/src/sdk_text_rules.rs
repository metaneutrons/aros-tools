//! Closed model for source-declared SDK text products.
//!
//! This capability recognizes only a named Make owner, its one `.pc` output,
//! one fetched template input, a unique explicit-or-path-derived fetch owner,
//! bounded static logging and directory creation, and ordered literal sed
//! operations. It does not execute Make or preserve arbitrary shell commands.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{ConditionalTruth, VarScope};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

const SDK_TEXT_OUTPUT_PATH_ROOTS: &[&str] = &[
    "${AROS_DEVELOPER_LIB_DIR}",
    "${AROS_BUILD_DIR}",
    "${AROS_SYS_DIR}",
    "${CMAKE_BINARY_DIR}",
];
const SOURCE_PATH_ROOTS: &[&str] = &["${AROS_SOURCE_DIR}"];
const PORTS_PATH_ROOTS: &[&str] = &["${AROS_PORTS_DIR}"];

/// One source-derived SDK text product and its complete producer chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkTextRuleDecl {
    /// Named Make target which owns the generated product.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of the file-producing rule.
    pub line: usize,
    /// Fetch-produced template, rendered as a CMake path.
    pub input: String,
    /// Product under the configured AROS_LIB/pkgconfig root.
    pub output: String,
    /// The unique `%fetch mmake=` target named by an explicit `#MM` edge or
    /// inferred from the input's most-specific fetch destination.
    pub fetch_owner: String,
    /// SHA-256 of the exact source snapshot that declared this rule. The
    /// parser fills this from its already-read source bytes.
    pub file_sha256: String,
    /// Ordered, safe operations taken from the Make sed recipe.
    pub operations: Vec<SdkTextOperation>,
}

/// Safe operations supported by the SDK text interpreter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdkTextOperation {
    /// Replace all occurrences of a literal token.
    ReplaceAll { token: String, replacement: String },
    /// Replace the first occurrence of a literal token on each input line.
    ReplaceFirstPerLine { token: String, replacement: String },
    /// Delete each line beginning with a literal prefix.
    DeleteLinePrefix { prefix: String },
    /// Replace a complete line beginning with a literal prefix.
    ReplaceLine { prefix: String, replacement: String },
}

impl SdkTextOperation {
    /// Encodes one operation for `aros_transform_sdk_text(OPERATIONS ...)`.
    ///
    /// The scanner rejects `|`, semicolons and line breaks in fields, making
    /// the representation unambiguous while allowing an empty replacement.
    #[must_use]
    pub fn cmake_argument(&self) -> String {
        match self {
            Self::ReplaceAll { token, replacement } => {
                format!("REPLACE_ALL|{token}|{replacement}")
            }
            Self::ReplaceFirstPerLine { token, replacement } => {
                format!("REPLACE_FIRST_PER_LINE|{token}|{replacement}")
            }
            Self::DeleteLinePrefix { prefix } => {
                format!("DELETE_LINE_PREFIX|{prefix}")
            }
            Self::ReplaceLine {
                prefix,
                replacement,
            } => {
                format!("REPLACE_LINE|{prefix}|{replacement}")
            }
        }
    }
}

/// A text-producing Make owner that cannot be represented by the closed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkTextRuleRejection {
    /// Best available named owner.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of the closest related Make declaration.
    pub line: usize,
    /// Why the producer is outside the supported contract.
    pub reason: String,
    /// Exact commented owner attribution, not evidence of an active producer.
    /// Set only after physical-source matching and active-consumer vetoes.
    pub disabled_owner_only: bool,
}

/// Diagnostic attribution mode that must not fabricate an active Make endpoint.
pub(crate) const DISABLED_OWNER_DIAGNOSTIC_MODE: &str = "disabled_owner_attribution";

#[derive(Debug, Clone)]
struct LogicalLine {
    text: String,
    source_line: usize,
    state: ConditionalTruth,
    conditional_syntax: bool,
}

#[derive(Debug)]
struct DefineMaskedLines {
    active: Vec<LogicalLine>,
    masked_source_lines: BTreeSet<usize>,
    malformed: bool,
}

#[derive(Clone, Copy)]
struct SourceLocation<'a> {
    root: &'a Path,
    rel_dir: &'a Path,
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
    source_line: usize,
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

#[derive(Debug, Clone)]
struct Candidate {
    output: String,
    raw_output: String,
    input: String,
    source_line: usize,
    rule_index: usize,
}

/// Collects named SDK `.pc` products with their exact source and fetch owners.
///
/// `line_states` uses zero-based positions in the continuation-joined input.
/// False branches are ignored. An unknown owner, recipe, metadata edge, or
/// fetch directive rejects the complete candidate rather than selecting one
/// possible Make expansion.
#[must_use]
#[cfg(test)]
pub(crate) fn collect_sdk_text_rules_with_context(
    content: &str,
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SdkTextRuleDecl>, Vec<SdkTextRuleRejection>) {
    collect_sdk_text_rules_with_physical_source(
        content,
        content,
        source_root,
        rel_dir,
        scope,
        dirs,
        line_states,
    )
}

/// Collects named SDK text products and retains the physical source text
/// for diagnostic-only disabled-owner attribution. The parsed
/// `content` may have Make continuations joined; `physical_source` must be the
/// matching pre-join source snapshot.
#[must_use]
pub(crate) fn collect_sdk_text_rules_with_physical_source(
    content: &str,
    physical_source: &str,
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<SdkTextRuleDecl>, Vec<SdkTextRuleRejection>) {
    let file = match source_relative_directory(rel_dir) {
        Ok(directory) if directory.is_empty() => "mmakefile.src".to_owned(),
        Ok(directory) => format!("{directory}/mmakefile.src"),
        Err(reason) => {
            return (
                Vec::new(),
                vec![SdkTextRuleRejection {
                    owner: owner_hint(content),
                    file: rel_dir.to_string_lossy().replace('\\', "/"),
                    line: 1,
                    reason,
                    disabled_owner_only: false,
                }],
            );
        }
    };
    if source_root.canonicalize().is_err() {
        return (
            Vec::new(),
            vec![SdkTextRuleRejection {
                owner: owner_hint(content),
                file,
                line: 1,
                reason: "source root cannot be canonicalized".into(),
                disabled_owner_only: false,
            }],
        );
    }

    let define_mask = mask_define_bodies(content, line_states);
    let lines = define_mask.active;
    let has_dynamic_make_expansion =
        has_active_dynamic_make_expansion(content, &lines, line_states, scope);
    let rules = parse_rules(&lines);
    let meta_edges = parse_meta_edges(content, line_states);
    let (fetch_rules, has_unresolved_fetch) =
        parse_fetch_rules(&lines, source_root, rel_dir, scope, dirs);
    let mut declarations = Vec::new();
    let mut candidates = Vec::new();
    let mut rejections = Vec::new();

    for (rule_index, rule) in rules.iter().enumerate() {
        if rule.state == ConditionalTruth::False {
            continue;
        }
        let raw_target = rule.target.trim();
        let expression_context =
            MakeExprContext::new(scope, dirs, rule.source_line, source_root, rel_dir);
        let raw_target_has_pc_suffix =
            target_has_pc_suffix_after_local_aliases(raw_target, scope, rule.source_line);
        let output = match evaluate_make_expr(raw_target, &expression_context) {
            Ok(output) => output,
            Err(error) => {
                // Makefiles contain many unrelated target expressions which
                // this bounded evaluator cannot resolve (for example a Mesa
                // generated header below top_builddir). Do not turn those
                // into SDK-text diagnostics unless the target itself, or a
                // bounded source-local alias chain, identifies a `.pc` file.
                if raw_target_has_pc_suffix {
                    rejections.push(rejection(
                        &file,
                        owner_for_output(raw_target, &rules),
                        rule.source_line,
                        format!("cannot resolve SDK text output `{raw_target}`: {error}"),
                    ));
                }
                continue;
            }
        };
        if !raw_target_has_pc_suffix && !has_sdk_text_path_shape(&output) {
            continue;
        }
        let output_has_pc_suffix = output.split_whitespace().any(has_exact_pc_suffix);
        let library_root = match evaluate_make_expr("$(AROS_LIB)", &expression_context) {
            Ok(root) => root,
            Err(error) => {
                if raw_target_has_pc_suffix || output_has_pc_suffix {
                    rejections.push(rejection(
                        &file,
                        owner_for_output(raw_target, &rules),
                        rule.source_line,
                        format!("cannot resolve configured AROS_LIB: {error}"),
                    ));
                }
                continue;
            }
        };
        let pkg_root = format!("{}/pkgconfig", library_root.trim_end_matches('/'));
        // The selected output may be spelled through a source-local Make
        // variable. Resolve it before deciding whether it belongs to this
        // capability; unrelated build targets remain invisible.
        let looks_relevant =
            raw_target_has_pc_suffix || output.starts_with(&format!("{pkg_root}/"));
        if !looks_relevant {
            continue;
        }
        let Some(basename) = output.strip_prefix(&format!("{pkg_root}/")) else {
            // A plain target outside the configured SDK lib tree is not this
            // producer. An explicit pkgconfig-looking target is a drift and
            // must be reported with its best named owner.
            if raw_target_has_pc_suffix {
                rejections.push(rejection(
                    &file,
                    owner_for_output(raw_target, &rules),
                    rule.source_line,
                    "text product does not resolve below configured AROS_LIB/pkgconfig".into(),
                ));
            }
            continue;
        };
        if !safe_pc_basename(basename) || basename.contains('/') {
            rejections.push(rejection(
                &file,
                owner_for_output(raw_target, &rules),
                rule.source_line,
                "SDK text output must be one safe `.pc` file directly below pkgconfig".into(),
            ));
            continue;
        }
        if rule.state == ConditionalTruth::Unknown || rule.conditional_syntax {
            rejections.push(rejection(
                &file,
                owner_for_output(raw_target, &rules),
                rule.source_line,
                conditional_problem(rule.state, rule.conditional_syntax),
            ));
            continue;
        }
        if !safe_cmake_path(&output, SDK_TEXT_OUTPUT_PATH_ROOTS)
            || !safe_cmake_path(&library_root, SDK_TEXT_OUTPUT_PATH_ROOTS)
        {
            rejections.push(rejection(
                &file,
                owner_for_output(raw_target, &rules),
                rule.source_line,
                "SDK text output contains an unsafe path component".into(),
            ));
            continue;
        }
        let prereqs = rule.prerequisites.split_whitespace().collect::<Vec<_>>();
        if !(prereqs.len() == 1 || prereqs.len() == 2) {
            rejections.push(rejection(
                &file,
                owner_for_output(raw_target, &rules),
                rule.source_line,
                "SDK text output must have one fetched input and at most its own declaring source as the second prerequisite".into(),
            ));
            continue;
        }
        let raw_input = prereqs[0];
        let input = match evaluate_make_expr(raw_input, &expression_context) {
            Ok(input) => input,
            Err(error) => {
                rejections.push(rejection(
                    &file,
                    owner_for_output(raw_target, &rules),
                    rule.source_line,
                    format!("cannot resolve SDK text input `{raw_input}`: {error}"),
                ));
                continue;
            }
        };
        if !safe_cmake_path(&input, PORTS_PATH_ROOTS) || !input.starts_with("${AROS_PORTS_DIR}/") {
            rejections.push(rejection(
                &file,
                owner_for_output(raw_target, &rules),
                rule.source_line,
                "SDK text input must be one safe path below AROS_PORTS_DIR".into(),
            ));
            continue;
        }
        if let Some(raw_source) = prereqs.get(1) {
            let declaring_source = match evaluate_make_expr(
                "$(SRCDIR)/$(CURDIR)/mmakefile.src",
                &expression_context,
            ) {
                Ok(path) => path,
                Err(error) => {
                    rejections.push(rejection(
                        &file,
                        owner_for_output(raw_target, &rules),
                        rule.source_line,
                        format!("cannot resolve the declaring mmakefile path: {error}"),
                    ));
                    continue;
                }
            };
            let source = match evaluate_make_expr(raw_source, &expression_context) {
                Ok(path) => path,
                Err(error) => {
                    rejections.push(rejection(
                        &file,
                        owner_for_output(raw_target, &rules),
                        rule.source_line,
                        format!(
                            "cannot resolve second SDK text prerequisite `{raw_source}`: {error}"
                        ),
                    ));
                    continue;
                }
            };
            if !safe_cmake_path(&source, SOURCE_PATH_ROOTS) || source != declaring_source {
                rejections.push(rejection(
                    &file,
                    owner_for_output(raw_target, &rules),
                    rule.source_line,
                    "second SDK text prerequisite must be this mmakefile's declaring source".into(),
                ));
                continue;
            }
        }
        candidates.push(Candidate {
            output,
            raw_output: raw_target.to_owned(),
            input,
            source_line: rule.source_line,
            rule_index,
        });
    }

    if define_mask.malformed {
        for candidate in candidates {
            rejections.push(rejection(
                &file,
                owner_for_output(&candidate.raw_output, &rules),
                candidate.source_line,
                "SDK text source contains a malformed or unclosed Make `define` body".into(),
            ));
        }
        return (declarations, rejections);
    }

    if has_dynamic_make_expansion {
        for candidate in candidates {
            rejections.push(rejection(
                &file,
                "<unknown-owner>".into(),
                candidate.source_line,
                "active Make expansion may instantiate or alter SDK text ownership rules".into(),
            ));
        }
        return (declarations, rejections);
    }

    let physical_proof_matches =
        content == physical_source || crate::parser::join_continuations(physical_source) == content;
    let mut output_owners = BTreeMap::<String, String>::new();
    for candidate in candidates {
        let output_rule = &rules[candidate.rule_index];
        let possible_owners = owner_rules(&candidate, &rules, source_root, rel_dir, scope, dirs);
        let owner = match possible_owners.as_slice() {
            [owner] => owner.target.trim().to_owned(),
            [] => {
                let active_consumer = has_active_output_consumer(
                    &candidate.output,
                    content,
                    &meta_edges,
                    SourceLocation {
                        root: source_root,
                        rel_dir,
                    },
                    scope,
                    dirs,
                    line_states,
                );
                let disabled_owner = if active_consumer {
                    None
                } else {
                    physical_proof_matches
                        .then(|| {
                            disabled_owner_for_output(
                                physical_source,
                                &candidate,
                                source_root,
                                rel_dir,
                                scope,
                                dirs,
                                if content == physical_source {
                                    line_states
                                } else {
                                    None
                                },
                            )
                        })
                        .flatten()
                };
                let mut diagnostic = rejection(
                    &file,
                    if active_consumer {
                        "<unknown-owner>".to_owned()
                    } else {
                        disabled_owner
                            .clone()
                            .unwrap_or_else(|| owner_for_output(&candidate.raw_output, &rules))
                    },
                    candidate.source_line,
                    "SDK text output has no unique named Make owner rule".into(),
                );
                diagnostic.disabled_owner_only = disabled_owner.is_some();
                rejections.push(diagnostic);
                continue;
            }
            owners => {
                rejections.push(rejection(
                    &file,
                    owners
                        .iter()
                        .map(|rule| rule.target.trim())
                        .collect::<Vec<_>>()
                        .join(","),
                    candidate.source_line,
                    "SDK text output has more than one named Make owner rule".into(),
                ));
                continue;
            }
        };
        let reject =
            |reason: String| rejection(&file, owner.clone(), candidate.source_line, reason);
        if !safe_target_name(&owner) {
            rejections.push(reject("SDK text owner is not one safe named target".into()));
            continue;
        }
        if let Some(previous) = output_owners.insert(candidate.output.clone(), owner.clone()) {
            rejections.push(reject(format!(
                "SDK text output is already claimed by owner `{previous}`"
            )));
            continue;
        }
        if owner_rule_count(&owner, &rules) != 1 {
            rejections.push(reject(
                "SDK text owner must have exactly one ordinary Make rule".into(),
            ));
            continue;
        }
        let owner_rule = possible_owners[0];
        if owner_rule.state == ConditionalTruth::Unknown || owner_rule.conditional_syntax {
            rejections.push(reject(conditional_problem(
                owner_rule.state,
                owner_rule.conditional_syntax,
            )));
            continue;
        }
        if !owner_rule.recipes.is_empty() {
            rejections.push(reject(
                "SDK text owner rule must not contain a shell recipe".into(),
            ));
            continue;
        }
        let owner_prereqs = owner_rule
            .prerequisites
            .split_whitespace()
            .collect::<Vec<_>>();
        if owner_prereqs.len() != 1 {
            rejections.push(reject(
                "SDK text owner must name only its concrete output prerequisite".into(),
            ));
            continue;
        }
        let owner_context =
            MakeExprContext::new(scope, dirs, owner_rule.source_line, source_root, rel_dir);
        let owner_output = match evaluate_make_expr(owner_prereqs[0], &owner_context) {
            Ok(output) => output,
            Err(error) => {
                rejections.push(reject(format!(
                    "cannot resolve owner output prerequisite: {error}"
                )));
                continue;
            }
        };
        if owner_output != candidate.output {
            rejections.push(reject(
                "named Make owner and file rule do not name the same output".into(),
            ));
            continue;
        }

        let owner_meta = meta_edges
            .iter()
            .filter(|edge| edge.owner == owner && edge.state != ConditionalTruth::False)
            .collect::<Vec<_>>();
        let fetch_owner = match owner_meta.as_slice() {
            [] => match infer_fetch_owner(&candidate.input, &fetch_rules, has_unresolved_fetch) {
                Ok(owner) => owner,
                Err(reason) => {
                    rejections.push(reject(reason));
                    continue;
                }
            },
            [meta] => {
                // A malformed local fetch could redeclare this target with an
                // unsafe or unresolved destination. Without retaining that
                // declaration's identity, a single parsed match is not proof
                // that the explicit edge has one provider.
                if has_unresolved_fetch {
                    rejections.push(reject(
                        "cannot validate explicit SDK text fetch ownership while a local `%fetch` declaration is unresolved".into(),
                    ));
                    continue;
                }
                if meta.state == ConditionalTruth::Unknown || meta.conditional_syntax {
                    rejections.push(reject(conditional_problem(
                        meta.state,
                        meta.conditional_syntax,
                    )));
                    continue;
                }
                if meta.prerequisites.len() != 1 || !safe_target_name(&meta.prerequisites[0]) {
                    rejections.push(reject(
                        "SDK text meta edge must name exactly one safe fetch target".into(),
                    ));
                    continue;
                }
                let fetch_owner = meta.prerequisites[0].clone();
                let fetches = fetch_rules
                    .iter()
                    .filter(|fetch| {
                        fetch.owner == fetch_owner && fetch.state != ConditionalTruth::False
                    })
                    .collect::<Vec<_>>();
                let [fetch] = fetches.as_slice() else {
                    rejections.push(reject(if fetches.is_empty() {
                        format!(
                            "`#MM` fetch prerequisite `{fetch_owner}` has no `%fetch` declaration"
                        )
                    } else {
                        format!("`%fetch` target `{fetch_owner}` is declared more than once")
                    }));
                    continue;
                };
                if fetch.state == ConditionalTruth::Unknown || fetch.conditional_syntax {
                    rejections.push(reject(conditional_problem(
                        fetch.state,
                        fetch.conditional_syntax,
                    )));
                    continue;
                }
                if !is_strict_descendant(&candidate.input, &fetch.destination) {
                    rejections.push(reject(format!(
                        "source input `{}` is not below `%fetch` destination `{}`",
                        candidate.input, fetch.destination
                    )));
                    continue;
                }
                fetch_owner
            }
            _ => {
                rejections.push(reject(
                    "SDK text owner has duplicate `#MM` prerequisite edges".into(),
                ));
                continue;
            }
        };

        let Some(recipe) = parse_sdk_text_recipe(
            output_rule,
            &candidate.output,
            source_root,
            rel_dir,
            scope,
            dirs,
        ) else {
            rejections.push(reject(
                "SDK text recipe is outside the exact mkdir plus literal sed subset".into(),
            ));
            continue;
        };
        if recipe.is_empty() {
            rejections.push(reject("SDK text recipe has no supported operations".into()));
            continue;
        }
        declarations.push(SdkTextRuleDecl {
            owner,
            file: file.clone(),
            line: candidate.source_line + 1,
            input: candidate.input,
            output: candidate.output,
            fetch_owner,
            file_sha256: String::new(),
            operations: recipe,
        });
    }

    (declarations, rejections)
}

fn parse_sdk_text_recipe(
    rule: &Rule,
    output: &str,
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
) -> Option<Vec<SdkTextOperation>> {
    if rule
        .recipes
        .iter()
        .any(|line| line.state != ConditionalTruth::True || line.conditional_syntax)
    {
        return None;
    }
    let context = MakeExprContext::new(scope, dirs, rule.source_line, source_root, rel_dir);
    let output_parent = output.rsplit_once('/')?.0;
    let sed_command = match rule.recipes.as_slice() {
        [mkdir_guard, sed] if parse_exact_directory_guard(mkdir_guard, output_parent, &context) => {
            sed
        }
        [echo, mkdir, sed]
            if parse_static_echo(&echo.text, output).is_some()
                && parse_mkdir_q(&mkdir.text, output_parent, &context) =>
        {
            sed
        }
        _ => return None,
    };
    parse_sed_recipe(&sed_command.text, &context)
}

fn parse_exact_directory_guard(
    line: &LogicalLine,
    output_parent: &str,
    context: &MakeExprContext<'_>,
) -> bool {
    let Some(guard) = shell_words(&line.text) else {
        return false;
    };
    let (Some(guard_parent_a), Some(guard_parent_b)) = (guard.get(4), guard.get(8)) else {
        return false;
    };
    let (Ok(parent_a), Ok(parent_b)) = (
        evaluate_make_expr(guard_parent_a, context),
        evaluate_make_expr(guard_parent_b, context),
    ) else {
        return false;
    };
    let expected_guard = [
        "@$(IF)",
        "$(TEST)",
        "!",
        "-d",
        guard_parent_a,
        ";",
        "then",
        "$(MKDIR)",
        guard_parent_b,
        ";",
        "else",
        "$(NOP)",
        ";",
        "fi",
    ];
    guard.len() == expected_guard.len()
        && guard
            .iter()
            .zip(expected_guard)
            .all(|(got, want)| got == want)
        && parent_a == parent_b
        && parent_a == output_parent
}

fn parse_static_echo(command: &str, output: &str) -> Option<()> {
    let command = command.strip_prefix('\t')?;
    let message = command.strip_prefix("@$(ECHO) \"")?.strip_suffix('"')?;
    let basename = output.rsplit('/').next()?;
    if message.len() > 256
        || !message.starts_with("Generating ")
        || !message.ends_with(" ...")
        || !message.contains(basename)
        || !message
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || " _./:+-".contains(ch))
    {
        return None;
    }
    Some(())
}

fn parse_mkdir_q(command: &str, output_parent: &str, context: &MakeExprContext<'_>) -> bool {
    let Some(argument) = command
        .strip_prefix('\t')
        .and_then(|command| command.strip_prefix("%mkdir_q dir="))
    else {
        return false;
    };
    if argument.is_empty() || argument.chars().any(char::is_whitespace) {
        return false;
    }
    evaluate_make_expr(argument, context).is_ok_and(|parent| {
        safe_cmake_path(&parent, SDK_TEXT_OUTPUT_PATH_ROOTS) && parent == output_parent
    })
}

fn parse_sed_recipe(command: &str, context: &MakeExprContext<'_>) -> Option<Vec<SdkTextOperation>> {
    let words = shell_words(command)?;
    if words.first().map(String::as_str) != Some("@$(SED)") || words.len() < 5 {
        return None;
    }
    let redirect = if words.len() >= 3
        && words[words.len() - 3..]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            == ["$<", ">", "$@"]
    {
        words.len() - 3
    } else if words.len() >= 2
        && words[words.len() - 2..]
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            == ["$<", ">$@"]
    {
        words.len() - 2
    } else {
        return None;
    };
    let mut cursor = 1usize;
    let mut operations = Vec::new();
    while cursor < redirect {
        if words.get(cursor)?.as_str() != "-e" {
            return None;
        }
        let expression = words.get(cursor + 1)?;
        operations.push(parse_sed_expression(expression, context)?);
        cursor += 2;
    }
    (cursor == redirect && !operations.is_empty()).then_some(operations)
}

fn parse_sed_expression(
    expression: &str,
    context: &MakeExprContext<'_>,
) -> Option<SdkTextOperation> {
    if let Some(body) = expression.strip_prefix('/') {
        let pattern = body.strip_suffix("/d")?;
        let prefix = parse_anchored_literal_prefix(pattern)?;
        return Some(SdkTextOperation::DeleteLinePrefix { prefix });
    }
    let body = expression.strip_prefix("s|")?;
    let pattern_end = body.find('|')?;
    let pattern = &body[..pattern_end];
    let replacement_and_flags = &body[pattern_end + 1..];
    let replacement_end = replacement_and_flags.find('|')?;
    let raw_replacement = &replacement_and_flags[..replacement_end];
    let flags = &replacement_and_flags[replacement_end + 1..];
    if !flags.is_empty() && flags != "g" {
        return None;
    }
    let replacement = evaluate_make_expr(raw_replacement, context).ok()?;
    if !safe_sed_replacement(&replacement) {
        return None;
    }
    if let Some(prefix) = pattern
        .strip_prefix('^')
        .and_then(|tail| tail.strip_suffix(".*"))
    {
        if safe_line_prefix(prefix) {
            return Some(SdkTextOperation::ReplaceLine {
                prefix: prefix.to_owned(),
                replacement,
            });
        }
        return None;
    }
    if let Some(token) = decode_make_sed_literal(pattern) {
        return Some(if flags == "g" {
            SdkTextOperation::ReplaceAll { token, replacement }
        } else {
            SdkTextOperation::ReplaceFirstPerLine { token, replacement }
        });
    }
    None
}

fn parse_anchored_literal_prefix(pattern: &str) -> Option<String> {
    let body = pattern.strip_prefix('^')?;
    let mut output = String::new();
    let mut chars = body.chars();
    while let Some(character) = chars.next() {
        if character == '\\' {
            let escaped = chars.next()?;
            if escaped != '.' {
                return None;
            }
            output.push('.');
        } else if character == '.' {
            return None;
        } else if character.is_ascii_alphanumeric() || "_+-".contains(character) {
            output.push(character);
        } else {
            return None;
        }
    }
    (!output.is_empty()).then_some(output)
}

fn decode_make_sed_literal(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > 256 {
        return None;
    }
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'$' {
            if bytes.get(at..at + 3)? != b"$${" {
                return None;
            }
            let start = at + 3;
            let end = bytes[start..].iter().position(|byte| *byte == b'}')? + start;
            let identifier = &bytes[start..end];
            if identifier.is_empty()
                || identifier.len() > 64
                || !(identifier[0].is_ascii_alphabetic() || identifier[0] == b'_')
                || !identifier
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                return None;
            }
            output.push_str("${");
            output.push_str(std::str::from_utf8(identifier).ok()?);
            output.push('}');
            at = end + 1;
            continue;
        }
        if !(byte.is_ascii_alphanumeric() || b"_@:/- ".contains(&byte)) {
            return None;
        }
        output.push(byte as char);
        at += 1;
    }
    Some(output)
}

fn safe_line_prefix(value: &str) -> bool {
    value.ends_with('=')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_+-=".contains(ch))
}

fn safe_sed_replacement(value: &str) -> bool {
    !value.contains(['|', ';', '\n', '\r', '\\', '&', '\t'])
        && value.matches("${prefix}").count() <= 1
        && value.replace("${prefix}", "").find('$').is_none()
}

fn shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut single_quoted = false;
    let mut active = false;
    for character in command.chars() {
        match character {
            '\'' => {
                single_quoted = !single_quoted;
                active = true;
            }
            '"' | '`' => return None,
            ';' if !single_quoted => {
                if active {
                    words.push(std::mem::take(&mut current));
                    active = false;
                }
                words.push(";".to_owned());
            }
            '&' | '|' if !single_quoted => return None,
            character if character.is_whitespace() && !single_quoted => {
                if active {
                    words.push(std::mem::take(&mut current));
                    active = false;
                }
            }
            _ => {
                current.push(character);
                active = true;
            }
        }
    }
    if single_quoted {
        return None;
    }
    if active {
        words.push(current);
    }
    Some(words)
}

fn owner_rules<'a>(
    candidate: &Candidate,
    rules: &'a [Rule],
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
) -> Vec<&'a Rule> {
    rules
        .iter()
        .filter(|rule| {
            if rule.state == ConditionalTruth::False || !safe_target_name(rule.target.trim()) {
                return false;
            }
            let prerequisites = rule.prerequisites.split_whitespace().collect::<Vec<_>>();
            if prerequisites.len() != 1 {
                return false;
            }
            let context = MakeExprContext::new(scope, dirs, rule.source_line, source_root, rel_dir);
            evaluate_make_expr(prerequisites[0], &context)
                .is_ok_and(|output| output == candidate.output)
        })
        .collect()
}

fn owner_rule_count(owner: &str, rules: &[Rule]) -> usize {
    rules
        .iter()
        .filter(|rule| rule.state != ConditionalTruth::False && rule.target.trim() == owner)
        .count()
}

fn owner_for_output(raw_output: &str, rules: &[Rule]) -> String {
    let owners = rules
        .iter()
        .filter(|rule| {
            rule.state != ConditionalTruth::False
                && rule
                    .prerequisites
                    .split_whitespace()
                    .any(|word| word == raw_output)
                && safe_target_name(rule.target.trim())
        })
        .map(|rule| rule.target.trim().to_owned())
        .collect::<Vec<_>>();
    match owners.as_slice() {
        [owner] => owner.clone(),
        [] => "<unknown-owner>".into(),
        _ => owners.join(","),
    }
}

/// Recovers an owner name for diagnostics only from one exact, source-declared
/// disabled MetaMake edge. This never admits a producer. Active Make or
/// MetaMake consumers of the output make the disabled declaration ambiguous
/// and prevent the attribution.
fn disabled_owner_for_output(
    physical_source: &str,
    candidate: &Candidate,
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    physical_line_states: Option<&[ConditionalTruth]>,
) -> Option<String> {
    let define_mask = mask_define_bodies(physical_source, physical_line_states);
    if define_mask.malformed {
        return None;
    }
    let logical = define_mask.active;
    let physical = physical_source.lines().collect::<Vec<_>>();
    let mut claims = Vec::new();
    let mut unproven_matching_claim = false;
    let mut index = 0usize;
    while index < physical.len() {
        if define_mask.masked_source_lines.contains(&index) {
            index += 1;
            continue;
        }
        let line = physical[index].trim_end_matches('\r');
        let Some(rest) = line.strip_prefix("##MM") else {
            index += 1;
            continue;
        };

        let marker_continued = line.trim_end().ends_with('\\');
        let marker_has_trailing_space = !rest.is_empty() && rest.trim().is_empty();
        let (declaration, declaration_line, continued) = if marker_continued {
            let Some(joined_marker) = logical.iter().find(|entry| entry.source_line == index)
            else {
                index += 1;
                continue;
            };
            let Some(joined_rest) = joined_marker.text.strip_prefix("##MM") else {
                index += 1;
                continue;
            };
            let Some(declaration) = joined_rest.strip_prefix([' ', '\t']) else {
                index += 1;
                continue;
            };
            (declaration, index, true)
        } else if rest.trim().is_empty() {
            let Some(commented_rule) = physical.get(index + 1) else {
                index += 1;
                continue;
            };
            let commented_rule = commented_rule.trim_end_matches('\r');
            let Some(declaration) = commented_rule.strip_prefix('#') else {
                index += 1;
                continue;
            };
            if commented_rule.trim_end().ends_with('\\') {
                let Some(joined_declaration) =
                    logical.iter().find(|entry| entry.source_line == index + 1)
                else {
                    index += 1;
                    continue;
                };
                let Some(declaration) = joined_declaration.text.strip_prefix('#') else {
                    index += 1;
                    continue;
                };
                (declaration, index + 1, true)
            } else {
                (declaration, index + 1, false)
            }
        } else {
            let Some(declaration) = rest.strip_prefix([' ', '\t']) else {
                index += 1;
                continue;
            };
            (declaration, index, false)
        };

        let Some((raw_owner_field, raw_prerequisites)) = declaration.split_once(':') else {
            index += if declaration_line > index { 2 } else { 1 };
            continue;
        };
        let raw_owner = raw_owner_field.trim();
        let prerequisites = raw_prerequisites.split_whitespace().collect::<Vec<_>>();
        let context = MakeExprContext::new(scope, dirs, declaration_line, source_root, rel_dir);
        let may_match_output = prerequisites.iter().any(|prerequisite| {
            prerequisite_may_consume(prerequisite, &candidate.output, &context)
        });
        if may_match_output {
            let owner_field_is_canonical = raw_owner_field == raw_owner
                || raw_owner_field
                    .strip_suffix([' ', '\t'])
                    .is_some_and(|without_separator| without_separator == raw_owner);
            let prerequisite_field_is_canonical = {
                let without_one_separator = raw_prerequisites
                    .strip_prefix([' ', '\t'])
                    .unwrap_or(raw_prerequisites);
                raw_prerequisites.trim_end() == raw_prerequisites
                    && !without_one_separator.starts_with([' ', '\t'])
            };
            let exact_prerequisite = prerequisites.len() == 1
                && !prerequisites[0].contains('|')
                && !continued
                && declaration.trim_end() == declaration
                && owner_field_is_canonical
                && prerequisite_field_is_canonical
                && !marker_has_trailing_space;
            let owner_is_safe = safe_target_name(raw_owner);
            let declaration_is_unconditional = [index, declaration_line].into_iter().all(|line| {
                logical
                    .iter()
                    .find(|entry| entry.source_line == line)
                    .is_some_and(|entry| {
                        entry.state == ConditionalTruth::True && !entry.conditional_syntax
                    })
            });
            if exact_prerequisite && owner_is_safe && declaration_is_unconditional {
                claims.push(raw_owner.to_owned());
            } else {
                // A multi-prerequisite, continued, malformed, or conditional
                // matching declaration could be another owner. Do not ignore
                // it while attributing a unique one.
                unproven_matching_claim = true;
            }
        }
        index += if declaration_line > index { 2 } else { 1 };
    }

    if unproven_matching_claim || claims.len() != 1 {
        None
    } else {
        claims.pop()
    }
}

fn has_active_output_consumer(
    output: &str,
    source_text: &str,
    meta_edges: &[MetaEdge],
    source_location: SourceLocation<'_>,
    scope: &VarScope,
    dirs: &DirVars,
    line_states: Option<&[ConditionalTruth]>,
) -> bool {
    let define_mask = mask_define_bodies(source_text, line_states);
    if define_mask.malformed {
        return true;
    }
    let make_rule_may_consume = define_mask.active.iter().any(|line| {
        if line.state == ConditionalTruth::False
            || line.text.starts_with('\t')
            || line.text.trim().is_empty()
        {
            return false;
        }
        let statement = line.text.trim();
        if statement.starts_with('#') || is_conditional_directive(statement) {
            return false;
        }
        let Some((target, raw_prerequisites)) = statement.split_once(':') else {
            return false;
        };
        let target = target.trim();
        if target.is_empty() || target.contains(['=', '\\']) || raw_prerequisites.starts_with('=') {
            return false;
        }
        let raw_prerequisites = raw_prerequisites
            .trim_start()
            .strip_prefix(':')
            .unwrap_or_else(|| raw_prerequisites.trim_start());
        let make_context = MakeExprContext::new(
            scope,
            dirs,
            line.source_line,
            source_location.root,
            source_location.rel_dir,
        );
        raw_prerequisites
            .split_whitespace()
            .any(|prerequisite| prerequisite_may_consume(prerequisite, output, &make_context))
    });

    make_rule_may_consume
        || meta_edges.iter().any(|edge| {
            if edge.state == ConditionalTruth::False {
                return false;
            }
            let make_context = MakeExprContext::new(
                scope,
                dirs,
                edge.source_line,
                source_location.root,
                source_location.rel_dir,
            );
            edge.prerequisites
                .iter()
                .any(|prerequisite| prerequisite_may_consume(prerequisite, output, &make_context))
        })
}

/// Dynamic Make expansion can add rules while the file is read. This scanner
/// does not interpret `eval` or `call`; it vetoes a candidate if either appears
/// in active non-recipe text, including through a source-local variable alias.
/// Bodies of unreferenced `define`s remain inert.
fn has_active_dynamic_make_expansion(
    content: &str,
    lines: &[LogicalLine],
    line_states: Option<&[ConditionalTruth]>,
    scope: &VarScope,
) -> bool {
    let (dynamic_definitions, unknown_dynamic_definition) =
        dynamic_make_definitions(content, line_states);
    let mut probe = DynamicReferenceProbe {
        scope,
        dynamic_definitions: &dynamic_definitions,
        unknown_dynamic_definition,
        remaining_visits: 65_536,
        remaining_bytes: 4 * 1024 * 1024,
    };
    lines.iter().any(|line| {
        if line.state == ConditionalTruth::False
            || line.text.starts_with('\t')
            || line.text.trim().is_empty()
        {
            return false;
        }
        let statement = line.text.trim();
        if statement.starts_with('#') {
            return false;
        }
        if contains_dynamic_make_function(statement) || is_standalone_make_expansion(statement) {
            return true;
        }
        let mut resolving = BTreeSet::new();
        probe.references_dynamic(statement, line.source_line, 0, &mut resolving)
    })
}

fn dynamic_make_definitions(
    content: &str,
    line_states: Option<&[ConditionalTruth]>,
) -> (BTreeSet<String>, bool) {
    let mut dynamic_names = BTreeSet::new();
    let mut unknown_dynamic_definition = false;
    let mut definition: Option<(Option<String>, ConditionalTruth, String)> = None;
    let mut depth = 0usize;

    for line in logical_lines(content, line_states) {
        let statement = line.text.trim();
        let opens = is_make_define_open(statement);
        let closes = is_make_define_close(statement);
        if let Some((_, _, body)) = definition.as_mut() {
            if opens {
                body.push_str(statement);
                body.push('\n');
                depth = depth.saturating_add(1);
            } else if closes {
                body.push_str(statement);
                body.push('\n');
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let (name, state, body) = definition.take().expect("open definition");
                    if state != ConditionalTruth::False && contains_dynamic_make_function(&body) {
                        if let Some(name) = name {
                            dynamic_names.insert(name);
                        } else {
                            unknown_dynamic_definition = true;
                        }
                    }
                }
            } else {
                body.push_str(&line.text);
                body.push('\n');
            }
        } else if opens {
            definition = Some((make_define_name(statement), line.state, String::new()));
            depth = 1;
        }
    }
    (dynamic_names, unknown_dynamic_definition)
}

fn make_define_name(statement: &str) -> Option<String> {
    let mut words = statement.split_whitespace();
    while matches!(
        words.clone().next(),
        Some("override" | "export" | "unexport" | "private")
    ) {
        words.next();
    }
    if words.next() != Some("define") {
        return None;
    }
    let rest = words.collect::<Vec<_>>().join(" ");
    let name = crate::make_vars::variable_assignment(&rest)
        .map_or_else(|| rest.split_whitespace().next(), |(name, _, _)| Some(name))?;
    safe_make_variable_name(name).then(|| name.to_owned())
}

fn safe_make_variable_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters.next().is_some_and(|first| {
        (first.is_ascii_alphabetic() || first == '_')
            && characters.all(|ch| ch.is_ascii_alphanumeric() || "_-".contains(ch))
    })
}

fn contains_dynamic_make_function(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index + 2 < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        if bytes[index + 1] == b'$' {
            index += 2;
            continue;
        }
        if !matches!(bytes[index + 1], b'(' | b'{') {
            index += 1;
            continue;
        }
        let body = &text[index + 2..];
        if function_tail(body, "eval").is_some() {
            return true;
        }
        if let Some(arguments) = function_tail(body, "call") {
            let callee = arguments
                .trim_start()
                .split([' ', '\t', ',', ')', '}'])
                .next()
                .unwrap_or_default();
            // The bounded Make evaluator models this one pure list helper.
            // Other call targets are macros whose expansion could execute
            // eval or produce rule syntax, so they remain unproven.
            if callee != "WILDCARD" {
                return true;
            }
        }
        index += 2;
    }
    false
}

fn function_tail<'a>(body: &'a str, function: &str) -> Option<&'a str> {
    body.strip_prefix(function)
        .filter(|tail| tail.starts_with([' ', '\t', ',', ')', '}']))
}

fn is_standalone_make_expansion(statement: &str) -> bool {
    let statement = statement.trim();
    if statement.contains([':', '=']) {
        return false;
    }
    if standalone_wildcard_call(statement) {
        return false;
    }
    (statement.starts_with("$(") && statement.ends_with(')'))
        || (statement.starts_with("${") && statement.ends_with('}'))
}

fn standalone_wildcard_call(statement: &str) -> bool {
    let body = statement
        .strip_prefix("$(")
        .and_then(|body| body.strip_suffix(')'))
        .or_else(|| {
            statement
                .strip_prefix("${")
                .and_then(|body| body.strip_suffix('}'))
        });
    let Some(arguments) = body.and_then(|body| function_tail(body, "call")) else {
        return false;
    };
    arguments
        .trim_start()
        .split([' ', '\t', ',', ')', '}'])
        .next()
        == Some("WILDCARD")
}

/// Limits total alias work across the complete file, not only one recursion
/// path. Small branching definitions can otherwise cause exponential work.
struct DynamicReferenceProbe<'a> {
    scope: &'a VarScope,
    dynamic_definitions: &'a BTreeSet<String>,
    unknown_dynamic_definition: bool,
    remaining_visits: usize,
    remaining_bytes: usize,
}

impl DynamicReferenceProbe<'_> {
    fn references_dynamic(
        &mut self,
        text: &str,
        source_line: usize,
        depth: usize,
        resolving: &mut BTreeSet<String>,
    ) -> bool {
        if self.remaining_visits == 0 || text.len() > self.remaining_bytes {
            return true;
        }
        self.remaining_visits -= 1;
        self.remaining_bytes -= text.len();
        if depth >= 16 || text.len() > 16 * 1024 || contains_dynamic_make_function(text) {
            return true;
        }
        for name in simple_make_variable_references(text) {
            let raw_value = self.scope.raw_at(&name, source_line);
            if self.dynamic_definitions.contains(&name)
                || self.scope.conditionally_assigned_before(&name, source_line)
                || (self.unknown_dynamic_definition && raw_value.is_none())
            {
                // Opaque defines may reference another define containing eval.
                // An absent raw value is not proof that an active use is inert.
                return true;
            }
            if !resolving.insert(name.clone()) {
                return true;
            }
            let dynamic = raw_value.is_some_and(|value| {
                self.references_dynamic(&value, source_line, depth + 1, resolving)
            });
            resolving.remove(&name);
            if dynamic {
                return true;
            }
        }
        false
    }
}

fn simple_make_variable_references(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut names = Vec::new();
    let mut index = 0usize;
    while index + 2 < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        if bytes[index + 1] == b'$' {
            index += 2;
            continue;
        }
        let close = match bytes[index + 1] {
            b'(' => b')',
            b'{' => b'}',
            _ => {
                index += 1;
                continue;
            }
        };
        let start = index + 2;
        let Some(relative_end) = bytes[start..].iter().position(|byte| *byte == close) else {
            index += 2;
            continue;
        };
        let end = start + relative_end;
        if let Ok(name) = std::str::from_utf8(&bytes[start..end]) {
            if safe_make_variable_name(name) {
                names.push(name.to_owned());
            }
        }
        index += 2;
    }
    names
}

/// Unresolvable prerequisites conservatively prevent an absence proof. A `%`
/// wildcard is treated using the Make pattern's fixed prefix and suffix.
fn prerequisite_may_consume(
    prerequisite: &str,
    output: &str,
    context: &MakeExprContext<'_>,
) -> bool {
    let Ok(path) = evaluate_make_expr(prerequisite, context) else {
        return true;
    };
    if path == output {
        return true;
    }
    let Some((prefix, suffix)) = path.split_once('%') else {
        return false;
    };
    if suffix.contains('%') {
        return true;
    }
    output.len() >= prefix.len() + suffix.len()
        && output.starts_with(prefix)
        && output.ends_with(suffix)
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
            || prerequisites.trim_start().starts_with('=')
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

fn parse_meta_edges(content: &str, line_states: Option<&[ConditionalTruth]>) -> Vec<MetaEdge> {
    let lines = mask_define_bodies(content, line_states);
    lines
        .active
        .iter()
        .filter_map(|line| {
            let body = meta_edge_body(&line.text)?.trim();
            let (owner, prerequisites) = body.split_once(':')?;
            let owner = owner.trim();
            if !safe_target_name(owner) {
                return None;
            }
            Some(MetaEdge {
                owner: owner.to_owned(),
                prerequisites: prerequisites
                    .split_whitespace()
                    .filter(|word| *word != "\\")
                    .map(str::to_owned)
                    .collect(),
                source_line: line.source_line,
                state: line.state,
                conditional_syntax: line.conditional_syntax,
            })
        })
        .collect()
}

fn meta_edge_body(line: &str) -> Option<&str> {
    let after_marker = if let Some(body) = line.strip_prefix("#MM-") {
        body
    } else {
        line.strip_prefix("#MM")?
    };
    after_marker
        .starts_with([' ', '\t'])
        .then_some(after_marker)
}

fn parse_fetch_rules(
    lines: &[LogicalLine],
    source_root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
) -> (Vec<FetchRule>, bool) {
    let mut fetches = Vec::new();
    let mut unresolved_fetch = false;
    for line in lines {
        let Some(body) = line.text.trim().strip_prefix("%fetch") else {
            continue;
        };
        if line.state == ConditionalTruth::False {
            continue;
        }
        let Some(arguments) = directive_arguments(body) else {
            unresolved_fetch = true;
            continue;
        };
        let Some(raw_owner) = last_argument(&arguments, "mmake") else {
            unresolved_fetch = true;
            continue;
        };
        let Some(raw_destination) = last_argument(&arguments, "destination") else {
            unresolved_fetch = true;
            continue;
        };
        let context = MakeExprContext::new(scope, dirs, line.source_line, source_root, rel_dir);
        let (Ok(owner), Ok(destination)) = (
            evaluate_make_expr(&raw_owner, &context),
            evaluate_make_expr(&raw_destination, &context),
        ) else {
            unresolved_fetch = true;
            continue;
        };
        if !safe_target_name(&owner) || !safe_cmake_path(&destination, PORTS_PATH_ROOTS) {
            unresolved_fetch = true;
            continue;
        }
        fetches.push(FetchRule {
            owner,
            destination,
            state: line.state,
            conditional_syntax: line.conditional_syntax,
        });
    }
    (fetches, unresolved_fetch)
}

fn infer_fetch_owner(
    input: &str,
    fetches: &[FetchRule],
    has_unresolved_fetch: bool,
) -> Result<String, String> {
    if has_unresolved_fetch {
        return Err(
            "cannot infer SDK text fetch owner while a local `%fetch` declaration is unresolved"
                .into(),
        );
    }
    let mut matching = fetches
        .iter()
        .filter(|fetch| {
            fetch.state != ConditionalTruth::False
                && is_strict_descendant(input, &fetch.destination)
        })
        .collect::<Vec<_>>();
    let Some(longest) = matching
        .iter()
        .map(|fetch| fetch.destination.trim_end_matches('/').len())
        .max()
    else {
        return Err(format!(
            "SDK text input `{input}` has no matching `%fetch` destination"
        ));
    };
    matching.retain(|fetch| fetch.destination.trim_end_matches('/').len() == longest);
    if matching.len() != 1 {
        let owners = matching
            .iter()
            .map(|fetch| fetch.owner.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "SDK text input `{input}` has ambiguous most-specific `%fetch` owners [{owners}]"
        ));
    }
    let fetch = matching[0];
    if fetch.state == ConditionalTruth::Unknown || fetch.conditional_syntax {
        return Err(conditional_problem(fetch.state, fetch.conditional_syntax));
    }
    let duplicate_declarations = fetches
        .iter()
        .filter(|other| other.owner == fetch.owner && other.state != ConditionalTruth::False)
        .count();
    if duplicate_declarations != 1 {
        return Err(format!(
            "`%fetch` target `{}` is declared more than once",
            fetch.owner
        ));
    }
    Ok(fetch.owner.clone())
}

fn is_strict_descendant(path: &str, parent: &str) -> bool {
    let parent = parent.trim_end_matches('/');
    path.strip_prefix(parent)
        .is_some_and(|suffix| suffix.starts_with('/') && suffix.len() > 1)
}

fn directive_arguments(body: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in body.chars() {
        match character {
            '\'' | '"' if quote.is_none() => quote = Some(character),
            character if Some(character) == quote => quote = None,
            character if character.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        words.push(current);
    }
    Some(words)
}

fn last_argument(arguments: &[String], key: &str) -> Option<String> {
    arguments
        .iter()
        .filter_map(|argument| {
            let (name, value) = argument.split_once('=')?;
            (name == key).then(|| value.to_owned())
        })
        .next_back()
}

fn mask_define_bodies(content: &str, states: Option<&[ConditionalTruth]>) -> DefineMaskedLines {
    let physical_line_count = content.lines().count();
    let logical = logical_lines(content, states);
    let mut masked_source_lines = BTreeSet::new();
    let mut define_depth = 0usize;
    let mut malformed = false;

    for (index, line) in logical.iter().enumerate() {
        let statement = line.text.trim();
        let opens_define = is_make_define_open(statement);
        let closes_define = is_make_define_close(statement);
        let in_define = define_depth > 0;
        let next_source_line = logical
            .get(index + 1)
            .map_or(physical_line_count, |next| next.source_line);
        let range_end = next_source_line.max(line.source_line.saturating_add(1));

        if in_define || opens_define || closes_define {
            masked_source_lines.extend(line.source_line..range_end.min(physical_line_count));
        }
        if in_define {
            if opens_define {
                define_depth = define_depth.saturating_add(1);
            } else if closes_define {
                define_depth = define_depth.saturating_sub(1);
            }
        } else if opens_define {
            define_depth = 1;
        } else if closes_define {
            malformed = true;
        }
    }
    if define_depth != 0 {
        malformed = true;
    }

    let mut sanitized = String::with_capacity(content.len());
    for (index, line) in content.split_inclusive('\n').enumerate() {
        if masked_source_lines.contains(&index) {
            if line.ends_with("\r\n") {
                sanitized.push_str("\r\n");
            } else if line.ends_with('\n') {
                sanitized.push('\n');
            }
        } else {
            sanitized.push_str(line);
        }
    }

    DefineMaskedLines {
        active: logical_lines(&sanitized, states),
        masked_source_lines,
        malformed,
    }
}

fn is_make_define_open(statement: &str) -> bool {
    let mut words = statement.split_whitespace();
    while matches!(
        words.clone().next(),
        Some("override" | "export" | "unexport" | "private")
    ) {
        words.next();
    }
    words.next() == Some("define")
}

fn is_make_define_close(statement: &str) -> bool {
    statement
        .strip_prefix("endef")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t', '\r']))
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
        while text.trim_end().ends_with('\\') && at + 1 < physical.len() {
            let trimmed = text.trim_end();
            text.truncate(trimmed.len() - 1);
            at += 1;
            let next = physical[at];
            state = combine_state(state, line_state(states, at));
            conditional_syntax |= states.is_none() && conditional_depth > 0;
            text.push(' ');
            text.push_str(next.trim());
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
    is_conditional_open(line) || is_conditional_close(line) || line == "else"
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
        "SDK text owner or recipe is in an undecided Make conditional".into()
    } else if conditional_syntax {
        "SDK text owner or recipe is conditionally defined but no line-state analysis is available"
            .into()
    } else {
        "SDK text declaration is not unconditionally active".into()
    }
}

fn safe_target_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_.+-".contains(ch))
}

fn safe_pc_basename(name: &str) -> bool {
    has_exact_pc_suffix(name)
        && name.len() > 3
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_.+-".contains(ch))
}

fn has_exact_pc_suffix(path: &str) -> bool {
    path.as_bytes().ends_with(b".pc")
}

const MAX_TARGET_ALIAS_DEPTH: usize = 16;
const MAX_TARGET_ALIAS_BYTES: usize = 16 * 1024;

fn target_has_pc_suffix_after_local_aliases(
    raw_target: &str,
    scope: &VarScope,
    source_line: usize,
) -> bool {
    let expanded =
        expand_target_local_aliases(raw_target, scope, source_line, 0, &mut BTreeSet::new());
    expanded.split_whitespace().any(has_exact_pc_suffix)
}

fn expand_target_local_aliases(
    raw: &str,
    scope: &VarScope,
    source_line: usize,
    depth: usize,
    resolving: &mut BTreeSet<String>,
) -> String {
    if depth >= MAX_TARGET_ALIAS_DEPTH || raw.len() > MAX_TARGET_ALIAS_BYTES {
        return raw.to_owned();
    }

    let mut expanded = String::with_capacity(raw.len());
    let mut cursor = 0;
    while cursor < raw.len() {
        let Some(relative_dollar) = raw[cursor..].find('$') else {
            expanded.push_str(&raw[cursor..]);
            break;
        };
        let dollar = cursor + relative_dollar;
        expanded.push_str(&raw[cursor..dollar]);
        let bytes = raw.as_bytes();
        if dollar + 1 < bytes.len() && bytes[dollar + 1] == b'$' {
            expanded.push_str("$$");
            cursor = dollar + 2;
            continue;
        }
        if dollar + 1 >= bytes.len() || !matches!(bytes[dollar + 1], b'(' | b'{') {
            expanded.push('$');
            cursor = dollar + 1;
            continue;
        }

        let close = if bytes[dollar + 1] == b'(' {
            b')'
        } else {
            b'}'
        };
        let body_start = dollar + 2;
        let Some(relative_close) = bytes[body_start..].iter().position(|byte| *byte == close)
        else {
            expanded.push_str(&raw[dollar..]);
            break;
        };
        let close_index = body_start + relative_close;
        let body = &raw[body_start..close_index];
        let original_end = close_index + 1;
        let Some(name) = simple_target_variable_name(body) else {
            expanded.push_str(&raw[dollar..original_end]);
            cursor = original_end;
            continue;
        };

        let Some(value) = scope
            .raw_at(name, source_line)
            .or_else(|| scope.path_raw_at(name, source_line))
        else {
            expanded.push_str(&raw[dollar..original_end]);
            cursor = original_end;
            continue;
        };
        if !resolving.insert(name.to_owned()) {
            expanded.push_str(&raw[dollar..original_end]);
            cursor = original_end;
            continue;
        }
        let value = expand_target_local_aliases(&value, scope, source_line, depth + 1, resolving);
        resolving.remove(name);
        if expanded.len() + value.len() + raw.len() - original_end > MAX_TARGET_ALIAS_BYTES {
            return raw.to_owned();
        }
        expanded.push_str(&value);
        cursor = original_end;
    }
    expanded
}

fn simple_target_variable_name(body: &str) -> Option<&str> {
    let mut chars = body.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() && first != '_' {
        return None;
    }
    chars
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        .then_some(body)
}

fn has_sdk_text_path_shape(output: &str) -> bool {
    output.split_whitespace().any(|path| {
        has_exact_pc_suffix(path) || path.split('/').any(|component| component == "pkgconfig")
    })
}

fn safe_cmake_path(path: &str, permitted_roots: &[&str]) -> bool {
    if path.is_empty()
        || path.contains([';', '\\', '\n', '\r', '"', '\'', '`', '|', '&'])
        || path.contains("$(")
    {
        return false;
    }
    let Some(root) = permitted_roots.iter().find(|root| {
        path == **root
            || path
                .strip_prefix(**root)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }) else {
        return false;
    };
    let suffix = &path[root.len()..];
    !suffix.contains('$')
        && Path::new(path)
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

fn source_relative_directory(path: &Path) -> Result<String, String> {
    if path.is_absolute() {
        return Err("SDK text mmakefile directory must be source-relative".into());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(part) = component else {
            if matches!(component, Component::CurDir) {
                continue;
            }
            return Err("SDK text mmakefile directory escapes the source root".into());
        };
        let part = part
            .to_str()
            .ok_or("SDK text mmakefile directory is not UTF-8")?;
        if part.is_empty()
            || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "_.+-".contains(ch))
        {
            return Err("SDK text mmakefile directory has an unsafe component".into());
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn owner_hint(content: &str) -> String {
    content
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#MM "))
        .filter_map(|line| line.split_once(':').map(|(owner, _)| owner.trim()))
        .find(|owner| safe_target_name(owner))
        .unwrap_or("<unknown-owner>")
        .to_owned()
}

fn rejection(file: &str, owner: String, line: usize, reason: String) -> SdkTextRuleRejection {
    SdkTextRuleRejection {
        owner,
        file: file.to_owned(),
        line: line + 1,
        reason,
        disabled_owner_only: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_with_context;
    use crate::parser::TargetContext;
    use std::fs;
    use tempfile::tempdir;

    fn fixture_root() -> tempfile::TempDir {
        let temp = tempdir().expect("temporary source root");
        fs::create_dir_all(temp.path().join("config")).expect("config directory");
        fs::write(
            temp.path().join("config/make.cfg.in"),
            "AROS_DIR_DEVELOPER := Developer\nAROS_DIR_LIB := lib\nAROSDIR := $(TARGETDIR)/SYS\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\n",
        )
        .expect("config paths");
        temp
    }

    fn scan(source: &str) -> (Vec<SdkTextRuleDecl>, Vec<SdkTextRuleRejection>) {
        scan_with_states(source, None)
    }

    fn scan_with_states(
        source: &str,
        line_states: Option<&[ConditionalTruth]>,
    ) -> (Vec<SdkTextRuleDecl>, Vec<SdkTextRuleRejection>) {
        let root = fixture_root();
        let target = TargetContext::default();
        let scope = collect_vars_with_context(source, &target);
        let dirs = DirVars::load(root.path());
        collect_sdk_text_rules_with_context(
            source,
            root.path(),
            Path::new("workbench/libs/example"),
            &scope,
            &dirs,
            line_states,
        )
    }

    fn scan_with_physical_source(
        content: &str,
        physical_source: &str,
        line_states: Option<&[ConditionalTruth]>,
    ) -> (Vec<SdkTextRuleDecl>, Vec<SdkTextRuleRejection>) {
        let root = fixture_root();
        let target = TargetContext::default();
        let scope = collect_vars_with_context(content, &target);
        let dirs = DirVars::load(root.path());
        collect_sdk_text_rules_with_physical_source(
            content,
            physical_source,
            root.path(),
            Path::new("workbench/libs/example"),
            &scope,
            &dirs,
            line_states,
        )
    }

    #[test]
    fn unresolved_non_pc_make_target_is_not_an_sdk_text_candidate() {
        let source = "$(top_builddir)/$(CUR_MESADIR)/main/dispatch.h: $(GLAPI_DEPS)\n\t@python3 gen_dispatch.py\n";
        let (declarations, rejected) = scan(source);

        assert!(declarations.is_empty());
        assert!(
            rejected.is_empty(),
            "unrelated generated headers must not be diagnosed as SDK text: {rejected:#?}"
        );

        let source = "AROS_LIB := $(MISSING_LIBROOT)\n/foreign/pkgconfig/dispatch.h: input.h\n";
        let (declarations, rejected) = scan(source);
        assert!(declarations.is_empty());
        assert!(
            rejected.is_empty(),
            "a non-PC target cannot become a candidate merely because its path contains `pkgconfig`: {rejected:#?}"
        );
    }

    #[test]
    fn unresolved_pc_targets_are_rejected_directly_and_through_local_aliases() {
        let cases = [
            "AROS_LIB := $(MISSING_LIBROOT)\nexample-pkgc: $(AROS_LIB)/pkgconfig/example.pc\n$(AROS_LIB)/pkgconfig/example.pc: $(PORTSDIR)/example.pc.in\n",
            "AROS_LIB := $(MISSING_LIBROOT)\nSDK_TARGET := $(AROS_LIB)/pkgconfig/example.pc\nexample-pkgc: $(SDK_TARGET)\n$(SDK_TARGET): $(PORTSDIR)/example.pc.in\n",
            "AROS_LIB := $(MISSING_LIBROOT)\nPC_PATH := $(AROS_LIB)/pkgconfig/example.pc\nSDK_TARGET := $(PC_PATH)\nexample-pkgc: $(SDK_TARGET)\n$(SDK_TARGET): $(PORTSDIR)/example.pc.in\n",
        ];

        for source in cases {
            let (declarations, rejected) = scan(source);
            assert!(declarations.is_empty());
            assert!(
                rejected.iter().any(|item| {
                    item.owner == "example-pkgc"
                        && item.reason.contains("cannot resolve SDK text output")
                }),
                "unresolved `.pc` output must remain a named fail-closed diagnostic: {rejected:#?}"
            );
        }
    }

    fn source(recipe: &str) -> String {
        format!(
            "VERSION := 1.2.3\nARCHSRCDIR := $(PORTSDIR)/pkg/archive\n%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n#MM example-pkgc : example-fetch\nexample-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n$(AROS_LIB)/pkgconfig/example.pc : $(ARCHSRCDIR)/example.pc.in\n{recipe}\n"
        )
    }

    fn source_with_disabled_owner() -> String {
        source(actual_shape_recipe())
            .replace(
                "#MM example-pkgc : example-fetch\n",
                "##MM\n#example-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n",
            )
            .replace("\nexample-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n", "\n")
    }

    fn actual_shape_recipe() -> &'static str {
        "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) -e 's|@PREFIX@|/Developer|g' \\\n\t    -e 's|@VERSION@|$(VERSION)|g' \\\n\t    -e '/^Libs\\.private/d' \\\n\t    -e 's|^exec_prefix=.*|exec_prefix=$${prefix}|' \\\n\t    $< > $@"
    }

    fn implicit_fetch_source(prerequisites: &str, recipe: &str) -> String {
        format!(
            "VERSION := 3.4.5\nARCHBASE := bundle+variant\n%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\nlib-tools+config : $(AROS_LIB)/pkgconfig/lib.tools+config.pc\n$(AROS_LIB)/pkgconfig/lib.tools+config.pc : {prerequisites}\n{recipe}\n"
        )
    }

    fn echo_mkdir_q_sed_recipe(sed: &str) -> String {
        format!(
            "\t@$(ECHO) \"Generating /Developer/lib/pkgconfig/lib.tools+config.pc ...\"\n\t%mkdir_q dir=$(AROS_LIB)/pkgconfig\n\t@$(SED) {sed}\n"
        )
    }

    #[test]
    fn scans_ordered_literal_sdk_template_operations() {
        let input = source(actual_shape_recipe());
        let (decls, rejected) = scan(&input);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        let declaration = &decls[0];
        assert_eq!(declaration.owner, "example-pkgc");
        assert_eq!(declaration.fetch_owner, "example-fetch");
        assert!(declaration.input.ends_with("/pkg/archive/example.pc.in"));
        assert!(declaration
            .output
            .ends_with("/SYS/Developer/lib/pkgconfig/example.pc"));
        assert_eq!(
            declaration.operations,
            vec![
                SdkTextOperation::ReplaceAll {
                    token: "@PREFIX@".into(),
                    replacement: "/Developer".into(),
                },
                SdkTextOperation::ReplaceAll {
                    token: "@VERSION@".into(),
                    replacement: "1.2.3".into(),
                },
                SdkTextOperation::DeleteLinePrefix {
                    prefix: "Libs.private".into(),
                },
                SdkTextOperation::ReplaceLine {
                    prefix: "exec_prefix=".into(),
                    replacement: "exec_prefix=${prefix}".into(),
                },
            ]
        );
        assert!(declaration
            .operations
            .last()
            .unwrap()
            .cmake_argument()
            .contains("${prefix}"));
    }

    #[test]
    fn disabled_exact_meta_owner_is_attributed_for_diagnostics_only() {
        let input = source_with_disabled_owner();
        let (decls, rejected) = scan(&input);

        assert!(
            decls.is_empty(),
            "a disabled MetaMake owner is not a producer"
        );
        let [diagnostic] = rejected.as_slice() else {
            panic!("expected one rejected SDK text output: {rejected:#?}");
        };
        assert_eq!(diagnostic.owner, "example-pkgc");
        assert!(diagnostic.disabled_owner_only);
        assert_eq!(
            diagnostic.reason,
            "SDK text output has no unique named Make owner rule"
        );
    }

    #[test]
    fn disabled_owner_attribution_refuses_ambiguous_or_unknown_claims() {
        let ambiguous = format!(
            "{}##MM second-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n",
            source_with_disabled_owner()
        );
        let (decls, rejected) = scan(&ambiguous);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| {
            item.owner == "<unknown-owner>"
                && !item.disabled_owner_only
                && item.reason == "SDK text output has no unique named Make owner rule"
        }));

        let unknown = source_with_disabled_owner();
        let owner_line = unknown
            .lines()
            .position(|line| line.starts_with("#example-pkgc :"))
            .expect("disabled owner line");
        let mut states = vec![ConditionalTruth::True; unknown.lines().count()];
        states[owner_line] = ConditionalTruth::Unknown;
        let (decls, rejected) = scan_with_states(&unknown, Some(&states));
        assert!(decls.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.owner == "<unknown-owner>" && !item.disabled_owner_only));
    }

    #[test]
    fn disabled_competitors_with_extra_or_unresolved_prerequisites_block_attribution() {
        let cases = [
            "##MM second-pkgc : $(AROS_LIB)/pkgconfig/example.pc extra-input\n",
            "##MM second-pkgc : $(AROS_LIB)/pkgconfig/example.pc | order-only-input\n",
            "##MM second-pkgc : $(UNKNOWN_PC_OUTPUT)\n",
        ];
        for competitor in cases {
            let input = format!("{}{competitor}", source_with_disabled_owner());
            let (decls, rejected) = scan(&input);
            assert!(decls.is_empty());
            assert!(
                rejected.iter().any(|item| {
                    item.owner == "<unknown-owner>"
                        && item.reason == "SDK text output has no unique named Make owner rule"
                }),
                "disabled competitor must veto attribution: {competitor:?}: {rejected:#?}"
            );
        }
    }

    #[test]
    fn potentially_matching_continued_disabled_competitor_blocks_attribution() {
        let input = format!(
            "{}##MM second-pkgc : \\\n$(AROS_LIB)/pkgconfig/example.pc\n",
            source_with_disabled_owner()
        );
        let (decls, rejected) = scan(&input);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| item.owner == "<unknown-owner>"));

        // The parser pipeline supplies continuation-joined text. The extra
        // separator left by this unsupported continuation must still prevent
        // a false unique-owner attribution.
        let joined = crate::parser::join_continuations(&input);
        let (decls, rejected) = scan(&joined);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| item.owner == "<unknown-owner>"));
    }

    #[test]
    fn physical_source_rejects_a_continued_marker_that_joined_text_makes_valid() {
        let physical = format!(
            "{}##MM \\\nsecond-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n",
            source_with_disabled_owner()
        );
        let joined = crate::parser::join_continuations(&physical);
        let (decls, rejected) = scan_with_physical_source(&joined, &physical, None);

        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| {
            item.owner == "<unknown-owner>"
                && item.reason == "SDK text output has no unique named Make owner rule"
        }));
    }

    #[test]
    fn unrelated_physical_snapshot_cannot_supply_diagnostic_ownership() {
        let physical = source_with_disabled_owner();
        let parsed = physical.replace("##MM\n", "# unrelated marker\n");
        let (decls, rejected) = scan_with_physical_source(&parsed, &physical, None);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| item.owner == "<unknown-owner>"));
    }

    #[test]
    fn define_bodies_are_inert_for_owner_rules_edges_and_consumers() {
        let candidate_in_define = format!(
            "VERSION := 1.2.3\nARCHSRCDIR := $(PORTSDIR)/pkg/archive\n%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\ndefine HIDDEN\n{}\nendef\n",
            source_with_disabled_owner()
        );
        let (decls, rejected) = scan(&candidate_in_define);
        assert!(decls.is_empty());
        assert!(
            rejected.is_empty(),
            "Make define bodies are data, not active SDK text rules: {rejected:#?}"
        );

        for prefix in ["override define", "export define", "unexport define"] {
            let candidate_in_define = format!(
                "VERSION := 1.2.3\nARCHSRCDIR := $(PORTSDIR)/pkg/archive\n%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n{prefix} HIDDEN\n{}\nendef\n",
                source_with_disabled_owner()
            );
            let (decls, rejected) = scan(&candidate_in_define);
            assert!(decls.is_empty(), "{prefix} body admitted an SDK producer");
            assert!(
                rejected.is_empty(),
                "{prefix} body was parsed as Make rules: {rejected:#?}"
            );
        }

        let inert_competitors = format!(
            "define HIDDEN\n##MM hidden-pkgc : $(AROS_LIB)/pkgconfig/example.pc\n#MM hidden-meta-consumer : $(AROS_LIB)/pkgconfig/example.pc\nhidden-consumer : $(AROS_LIB)/pkgconfig/example.pc extra\nendef\n{}",
            source_with_disabled_owner()
        );
        let (decls, rejected) = scan(&inert_competitors);
        assert!(decls.is_empty());
        assert_eq!(rejected.len(), 1, "{rejected:#?}");
        assert_eq!(rejected[0].owner, "example-pkgc");
    }

    #[test]
    fn active_dynamic_make_expansion_vetoes_sdk_text_owner_admission() {
        let hidden_rule = "hidden-consumer : $(AROS_LIB)/pkgconfig/example.pc";
        let cases = [
            format!(
                "define HIDDEN\n{hidden_rule}\nendef\n{}\n$(eval $(HIDDEN))",
                source_with_disabled_owner()
            ),
            format!(
                "define HIDDEN\n{hidden_rule}\nendef\nRULES := $(eval $(HIDDEN))\n{}",
                source_with_disabled_owner()
            ),
            format!(
                "define HIDDEN\n$(eval $(RULE_TEXT))\nendef\ndefine RULE_TEXT\n{hidden_rule}\nendef\nEXPANDED := $(HIDDEN)\n{}",
                source_with_disabled_owner()
            ),
            format!(
                "define HIDDEN\n$(eval $(RULE_TEXT))\nendef\ndefine RULE_TEXT\n{hidden_rule}\nendef\nEXPANDED := $(call HIDDEN)\n{}",
                source_with_disabled_owner()
            ),
            format!(
                "define HIDDEN\n{hidden_rule}\nendef\n{}\n$(call HIDDEN)",
                source(actual_shape_recipe())
            ),
            format!(
                "define RULE_TEXT\n{hidden_rule}\nendef\n{}\n$(RULE_TEXT)",
                source(actual_shape_recipe())
            ),
        ];

        for input in cases {
            let (declarations, rejected) = scan(&input);
            assert!(declarations.is_empty(), "dynamic source was admitted");
            assert!(
                rejected.iter().any(|item| {
                    item.owner == "<unknown-owner>" && item.reason.contains("active Make expansion")
                }),
                "dynamic expansion must remain unowned: {rejected:#?}"
            );
        }
    }

    #[test]
    fn opaque_define_alias_cannot_hide_an_active_eval_consumer() {
        let input = format!(
            "define ALIAS\n$(eval extra-consumer : $(AROS_LIB)/pkgconfig/example.pc)\nendef\ndefine HIDDEN\n$(ALIAS)\nendef\nreal-target : $(if 1,$(HIDDEN))\n{}",
            source_with_disabled_owner()
        );
        let (declarations, rejected) = scan(&input);
        assert!(declarations.is_empty());
        assert!(
            rejected.iter().any(|item| {
                item.owner == "<unknown-owner>" && item.reason.contains("active Make expansion")
            }),
            "opaque alias must not prove a disabled owner: {rejected:#?}"
        );
    }

    #[test]
    fn branching_alias_scan_has_a_complete_file_work_budget() {
        use std::fmt::Write as _;
        let mut input = String::from("A14 = literal\n");
        for index in (0..14).rev() {
            writeln!(
                input,
                "A{index} = {}",
                vec![format!("$(A{})", index + 1); 8].join(" ")
            )
            .unwrap();
        }
        input.push_str("real-target : $(A0)\n");
        input.push_str(&source_with_disabled_owner());
        let (declarations, rejected) = scan(&input);
        assert!(declarations.is_empty());
        assert!(
            rejected.iter().any(|item| {
                item.owner == "<unknown-owner>" && item.reason.contains("active Make expansion")
            }),
            "exhausted proof budget must veto admission: {rejected:#?}"
        );
    }

    #[test]
    fn unreferenced_define_expansion_is_inert_but_unknown_active_expansion_vetoes() {
        let inert = format!(
            "define HIDDEN\n$(eval $(RULE_TEXT))\nendef\ndefine RULE_TEXT\nhidden-consumer : $(AROS_LIB)/pkgconfig/example.pc\nendef\n{}",
            source(actual_shape_recipe())
        );
        let (declarations, rejected) = scan(&inert);
        assert_eq!(
            declarations.len(),
            1,
            "unreferenced define is inert: {rejected:#?}"
        );
        assert!(rejected.is_empty(), "{rejected:#?}");

        let dynamic = format!("{}\n$(eval $(HIDDEN))", source(actual_shape_recipe()));
        let eval_line = dynamic
            .lines()
            .position(|line| line == "$(eval $(HIDDEN))")
            .expect("active eval line");
        let mut states = vec![ConditionalTruth::True; dynamic.lines().count()];
        states[eval_line] = ConditionalTruth::Unknown;
        let (declarations, rejected) = scan_with_states(&dynamic, Some(&states));
        assert!(declarations.is_empty());
        assert!(
            rejected.iter().any(|item| {
                item.owner == "<unknown-owner>" && item.reason.contains("active Make expansion")
            }),
            "unknown active expansion must veto admission: {rejected:#?}"
        );

        states[eval_line] = ConditionalTruth::False;
        let (declarations, rejected) = scan_with_states(&dynamic, Some(&states));
        assert_eq!(
            declarations.len(),
            1,
            "literal false expansion is ignored: {rejected:#?}"
        );
        assert!(rejected.is_empty(), "{rejected:#?}");
    }

    #[test]
    fn malformed_define_vetoes_sdk_text_producer_selection() {
        let input = format!("{}define UNTERMINATED\n", source(actual_shape_recipe()));
        let (decls, rejected) = scan(&input);

        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| {
            item.reason
                .contains("malformed or unclosed Make `define` body")
        }));
    }

    #[test]
    fn active_consumers_and_comment_drift_do_not_prove_disabled_owner() {
        let active_consumer = format!(
            "{}unrelated-target : $(AROS_LIB)/pkgconfig/example.pc extra-input\n",
            source_with_disabled_owner()
        );
        let (decls, rejected) = scan(&active_consumer);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| {
            item.owner == "<unknown-owner>"
                && item.reason == "SDK text output has no unique named Make owner rule"
        }));

        let drifted = source_with_disabled_owner().replace(
            "##MM\n#example-pkgc :",
            "##MM documentation drift\n#example-pkgc :",
        );
        let (decls, rejected) = scan(&drifted);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| item.owner == "<unknown-owner>"));
    }

    #[test]
    fn unresolved_double_colon_and_pattern_consumers_block_disabled_attribution() {
        let cases = [
            "unresolved-consumer : $(UNKNOWN_PC_PATH) other-input\n",
            "double-colon-consumer :: $(AROS_LIB)/pkgconfig/example.pc other-input\n",
            "pattern-consumer : $(AROS_LIB)/pkgconfig/%.pc\n",
        ];
        for active_rule in cases {
            let input = format!("{}{active_rule}", source_with_disabled_owner());
            let (decls, rejected) = scan(&input);
            assert!(decls.is_empty());
            assert!(
                rejected.iter().any(|item| {
                    item.owner == "<unknown-owner>"
                        && item.reason == "SDK text output has no unique named Make owner rule"
                }),
                "active rule must block diagnostic owner attribution: {active_rule:?}: {rejected:#?}"
            );
        }
    }

    #[test]
    fn rejects_unsafe_regex_and_extra_commands_with_owner() {
        let unsafe_regex = source(
            "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) -e 's|@PREFIX@.*|/Developer|g' $< > $@",
        );
        let (decls, rejected) = scan(&unsafe_regex);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| item.owner == "example-pkgc"));

        let extra_command = source(&format!(
            "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(ECHO) unexpected\n{}",
            actual_shape_recipe()
                .lines()
                .skip(1)
                .collect::<Vec<_>>()
                .join("\n")
        ));
        let (decls, rejected) = scan(&extra_command);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| item.owner == "example-pkgc"));
    }

    #[test]
    fn infers_unique_most_specific_fetch_without_requiring_an_mm_edge() {
        let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
        let source = implicit_fetch_source(
            "$(PORTSDIR)/bundle/archive/template.pc.in",
            &recipe,
        )
        .replace(
            "%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\n",
            "%fetch mmake=fetch-bundle archive=root destination=$(PORTSDIR)\n%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\n",
        );
        let (decls, rejected) = scan(&source);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].owner, "lib-tools+config");
        assert_eq!(decls[0].fetch_owner, "fetch-bundle.v2");
        assert_eq!(decls[0].file_sha256, "");
        assert!(decls[0].input.ends_with("/bundle/archive/template.pc.in"));
        assert!(decls[0].output.ends_with("/lib.tools+config.pc"));
    }

    #[test]
    fn implicit_fetch_inference_rejects_missing_tied_and_unknown_providers() {
        let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
        let base = implicit_fetch_source("$(PORTSDIR)/bundle/archive/template.pc.in", &recipe);

        let missing = base.replace(
            "%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\n",
            "",
        );
        let (decls, rejected) = scan(&missing);
        assert!(decls.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("no matching `%fetch`")));

        let tied = base.replace(
            "%fetch mmake=fetch-bundle.v2 archive=$(ARCHBASE) destination=$(PORTSDIR)/bundle\n",
            "%fetch mmake=fetch-left archive=left destination=$(PORTSDIR)/bundle\n%fetch mmake=fetch-right archive=right destination=$(PORTSDIR)/bundle\n",
        );
        let (decls, rejected) = scan(&tied);
        assert!(decls.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("ambiguous most-specific `%fetch`")));

        let mut states = vec![ConditionalTruth::True; base.lines().count()];
        let fetch_line = base
            .lines()
            .position(|line| line.starts_with("%fetch"))
            .expect("fetch line");
        states[fetch_line] = ConditionalTruth::Unknown;
        let (decls, rejected) = scan_with_states(&base, Some(&states));
        assert!(decls.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("undecided Make conditional")));
    }

    #[test]
    fn explicit_fetch_edge_is_not_replaced_by_path_inference() {
        let wrong_source = source(actual_shape_recipe()).replace(
            "#MM example-pkgc : example-fetch",
            "#MM example-pkgc : unrelated-fetch",
        );
        let wrong_source = wrong_source.replace(
            "%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive",
            "%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n%fetch mmake=unrelated-fetch archive=elsewhere destination=$(PORTSDIR)/elsewhere",
        );
        let (decls, rejected) = scan(&wrong_source);
        assert!(decls.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("not below `%fetch`")));

        let wrong_source = source(actual_shape_recipe()).replace(
            "$(ARCHSRCDIR)/example.pc.in",
            "$(PORTSDIR)/elsewhere/example.pc.in",
        );
        let (decls, rejected) = scan(&wrong_source);
        assert!(decls.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("not below `%fetch`")));
    }

    #[test]
    fn virtual_and_tabbed_meta_edges_are_binding_and_conflicts_reject() {
        let virtual_edge = source(actual_shape_recipe()).replace(
            "#MM example-pkgc : example-fetch",
            "#MM-\texample-pkgc : example-fetch",
        );
        let (decls, rejected) = scan(&virtual_edge);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].fetch_owner, "example-fetch");

        let conflict = virtual_edge.replace(
            "example-pkgc : $(AROS_LIB)/pkgconfig/example.pc",
            "#MM\texample-pkgc : other-fetch\n%fetch mmake=other-fetch archive=other destination=$(PORTSDIR)/other\nexample-pkgc : $(AROS_LIB)/pkgconfig/example.pc",
        );
        let (decls, rejected) = scan(&conflict);
        assert!(decls.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("duplicate `#MM` prerequisite edges")));

        let mut states = vec![ConditionalTruth::True; virtual_edge.lines().count()];
        let edge_line = virtual_edge
            .lines()
            .position(|line| line.starts_with("#MM-\t"))
            .expect("virtual edge line");
        states[edge_line] = ConditionalTruth::Unknown;
        let (decls, rejected) = scan_with_states(&virtual_edge, Some(&states));
        assert!(decls.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("undecided Make conditional")));
    }

    #[test]
    fn bare_mm_marker_still_allows_generic_fetch_inference() {
        let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
        let source = implicit_fetch_source("$(PORTSDIR)/bundle/archive/template.pc.in", &recipe)
            .replace("\nlib-tools+config :", "\n#MM\nlib-tools+config :");
        assert!(source.contains("#MM\nlib-tools+config :"));
        let (decls, rejected) = scan(&source);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].fetch_owner, "fetch-bundle.v2");
    }

    #[test]
    fn unresolved_fetches_cannot_be_ignored_beside_an_explicit_edge() {
        let source = source(actual_shape_recipe()).replace(
            "%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n",
            "%fetch mmake=example-fetch archive=pkg destination=$(PORTSDIR)/pkg/archive\n%fetch mmake=example-fetch archive=unsafe destination=$(PORTSDIR)/pkg/../outside\n",
        );
        let (decls, rejected) = scan(&source);
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| {
            item.reason
                .contains("while a local `%fetch` declaration is unresolved")
        }));
    }

    #[test]
    fn only_known_single_cmake_roots_are_preserved_in_paths() {
        let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
        let valid_input = "$(PORTSDIR)/bundle/archive/template.pc.in";
        let base = implicit_fetch_source(valid_input, &recipe);

        let input_with_injected_root = base.replace(
            "$(PORTSDIR)/bundle/archive/template.pc.in",
            "$(PORTSDIR)/bundle/$${EVIL}/template.pc.in",
        );
        let (decls, rejected) = scan(&input_with_injected_root);
        assert!(decls.is_empty());
        assert!(!rejected.is_empty());

        let output_with_injected_root = base.replace(
            "VERSION := 3.4.5",
            "AROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)/$${EVIL}\nVERSION := 3.4.5",
        );
        let (decls, rejected) = scan(&output_with_injected_root);
        assert!(decls.is_empty());
        assert!(!rejected.is_empty());

        let fetch_with_injected_root = base.replace(
            "destination=$(PORTSDIR)/bundle",
            "destination=$(PORTSDIR)/bundle/$${EVIL}",
        );
        let (decls, rejected) = scan(&fetch_with_injected_root);
        assert!(decls.is_empty());
        assert!(!rejected.is_empty());

        assert!(safe_cmake_path(
            "${AROS_DEVELOPER_LIB_DIR}/pkgconfig/example.pc",
            SDK_TEXT_OUTPUT_PATH_ROOTS
        ));
    }

    #[test]
    fn static_echo_mkdir_q_recipe_accepts_source_prerequisite_and_first_per_line_sed() {
        let recipe = echo_mkdir_q_sed_recipe(
            "-e 's|@exec_prefix@|$${prefix}|' \\\n\t    -e 's|@includedir@/libtiff@TIFFLIB_MAJOR@@TIFFLIB_MINOR@|$${prefix}/include|' \\\n\t    -e 's|-ltiff@TIFFLIB_MAJOR@@TIFFLIB_MINOR@|-ltiff|' \\\n\t    -e 's|@libdir@|$${prefix}/lib|' \\\n\t    -e 's|@prefix@|/Developer|' \\\n\t    -e 's|@LIBS@||' \\\n\t    -e 's|@TIFFLIB_VERSION@|$(VERSION)|' \\\n\t    -e 's| -I$${includedir}||' \\\n\t    $< >$@",
        );
        let input_and_own_source =
            "$(PORTSDIR)/bundle/archive/template.pc.in $(SRCDIR)/$(CURDIR)/mmakefile.src";
        let source = implicit_fetch_source(input_and_own_source, &recipe);
        let (decls, rejected) = scan(&source);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].fetch_owner, "fetch-bundle.v2");
        assert_eq!(
            decls[0].operations,
            vec![
                SdkTextOperation::ReplaceFirstPerLine {
                    token: "@exec_prefix@".into(),
                    replacement: "${prefix}".into(),
                },
                SdkTextOperation::ReplaceFirstPerLine {
                    token: "@includedir@/libtiff@TIFFLIB_MAJOR@@TIFFLIB_MINOR@".into(),
                    replacement: "${prefix}/include".into(),
                },
                SdkTextOperation::ReplaceFirstPerLine {
                    token: "-ltiff@TIFFLIB_MAJOR@@TIFFLIB_MINOR@".into(),
                    replacement: "-ltiff".into(),
                },
                SdkTextOperation::ReplaceFirstPerLine {
                    token: "@libdir@".into(),
                    replacement: "${prefix}/lib".into(),
                },
                SdkTextOperation::ReplaceFirstPerLine {
                    token: "@prefix@".into(),
                    replacement: "/Developer".into(),
                },
                SdkTextOperation::ReplaceFirstPerLine {
                    token: "@LIBS@".into(),
                    replacement: String::new(),
                },
                SdkTextOperation::ReplaceFirstPerLine {
                    token: "@TIFFLIB_VERSION@".into(),
                    replacement: "3.4.5".into(),
                },
                SdkTextOperation::ReplaceFirstPerLine {
                    token: " -I${includedir}".into(),
                    replacement: String::new(),
                },
            ]
        );
        assert_eq!(
            decls[0].operations[7].cmake_argument(),
            "REPLACE_FIRST_PER_LINE| -I${includedir}|"
        );
        let repeated = "@exec_prefix@ @exec_prefix@\n@exec_prefix@\n";
        let rendered = repeated
            .split_inclusive('\n')
            .map(|line| line.replacen("@exec_prefix@", "${prefix}", 1))
            .collect::<String>();
        assert_eq!(rendered, "${prefix} @exec_prefix@\n${prefix}\n");
    }

    #[test]
    fn source_prerequisite_must_be_second_and_name_this_mmakefile() {
        let recipe = echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@");
        let input = "$(PORTSDIR)/bundle/archive/template.pc.in";
        for prerequisites in [
            format!("$(SRCDIR)/$(CURDIR)/mmakefile.src {input}"),
            format!("{input} $(SRCDIR)/elsewhere/mmakefile.src"),
            format!("{input} $(SRCDIR)/$(CURDIR)/mmakefile.src foreign.in"),
        ] {
            let source = implicit_fetch_source(&prerequisites, &recipe);
            let (decls, rejected) = scan(&source);
            assert!(decls.is_empty(), "accepted prerequisites: {prerequisites}");
            assert!(!rejected.is_empty(), "missing rejection: {prerequisites}");
        }
    }

    #[test]
    fn echo_and_mkdir_q_are_static_and_exactly_for_the_output_parent() {
        let valid_sed = "-e 's|@TOKEN@|x|' $< >$@";
        for recipe in [
            echo_mkdir_q_sed_recipe("-e 's|@TOKEN@|x|' $< >$@").replace(
                "Generating /Developer/lib/pkgconfig/lib.tools+config.pc ...",
                "Generating $(SHELL) lib.tools+config.pc ...",
            ),
            echo_mkdir_q_sed_recipe(valid_sed)
                .replace("dir=$(AROS_LIB)/pkgconfig", "dir=$(AROS_LIB)/other"),
            echo_mkdir_q_sed_recipe(valid_sed).replace("%mkdir_q dir=", "%mkdir_q dir=extra "),
        ] {
            let input = "$(PORTSDIR)/bundle/archive/template.pc.in";
            let source = implicit_fetch_source(input, &recipe);
            let (decls, rejected) = scan(&source);
            assert!(decls.is_empty(), "accepted unsafe recipe: {recipe}");
            assert!(!rejected.is_empty(), "missing rejection for: {recipe}");
        }
    }

    #[test]
    fn sed_literal_flags_distinguish_default_first_match_from_global() {
        let first = source(
            "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) -e 's|TOKEN|X|' $< > $@",
        );
        let (decls, rejected) = scan(&first);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(
            decls[0].operations,
            vec![SdkTextOperation::ReplaceFirstPerLine {
                token: "TOKEN".into(),
                replacement: "X".into(),
            }]
        );

        let all = source(
            "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) -e 's|TOKEN|X|g' $< > $@",
        );
        let (decls, rejected) = scan(&all);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(
            decls[0].operations,
            vec![SdkTextOperation::ReplaceAll {
                token: "TOKEN".into(),
                replacement: "X".into(),
            }]
        );
    }

    #[test]
    fn sed_patterns_and_replacements_reject_dynamic_or_regex_syntax() {
        for sed in [
            "-e 's|TOKEN.*|X|g' $< >$@",
            "-e 's|$(VERSION)|X|g' $< >$@",
            "-e 's|TOKEN|$(UNKNOWN_SDK_VALUE)|g' $< >$@",
            "-e 's|TOKEN|$${untrusted_value}|g' $< >$@",
        ] {
            let input = source(&format!(
                "\t@$(IF) $(TEST) ! -d $(AROS_LIB)/pkgconfig ; then $(MKDIR) $(AROS_LIB)/pkgconfig ; else $(NOP) ; fi\n\t@$(SED) {sed}"
            ));
            let (decls, rejected) = scan(&input);
            assert!(decls.is_empty(), "accepted unsafe sed: {sed}");
            assert!(
                rejected.iter().any(|item| item.owner == "example-pkgc"),
                "missing scanner rejection for sed: {sed}"
            );
        }
    }

    #[test]
    fn unknown_recipe_line_rejects_selected_owner() {
        let input = source(actual_shape_recipe());
        let mut states = vec![ConditionalTruth::True; input.lines().count()];
        let line = input
            .lines()
            .position(|line| line.contains("@$(SED) -e 's|@PREFIX@"))
            .expect("sed recipe line");
        states[line] = ConditionalTruth::Unknown;
        let root = fixture_root();
        let target = TargetContext::default();
        let scope = collect_vars_with_context(&input, &target);
        let dirs = DirVars::load(root.path());
        let (decls, rejected) = collect_sdk_text_rules_with_context(
            &input,
            root.path(),
            Path::new("workbench/libs/example"),
            &scope,
            &dirs,
            Some(&states),
        );
        assert!(decls.is_empty());
        assert!(rejected.iter().any(|item| item.owner == "example-pkgc"));
    }
}
