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

mod dynamic_make;
mod recipe_parsing;
mod target_aliases;

use dynamic_make::{
    has_active_dynamic_make_expansion, has_active_output_consumer, prerequisite_may_consume,
};
use recipe_parsing::parse_sdk_text_recipe;
use target_aliases::{
    has_exact_pc_suffix, safe_pc_basename, safe_target_name,
    target_has_pc_suffix_after_local_aliases,
};

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
#[path = "sdk_text_rules_tests.rs"]
mod tests;
