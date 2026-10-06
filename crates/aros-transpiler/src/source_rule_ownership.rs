//! Bounded, source-local ownership evidence for rejected Make outputs.
//!
//! This is diagnostic attribution only. It does not create producers,
//! dependencies, or capabilities. The graph follows exact ordinary Make and
//! `#MM` prerequisite identities, plus output identities from a small set of
//! native macros whose definitions are verified by SHA-256 before use.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{strip_make_comment, variable_assignment, ConditionalTruth, VarScope};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod macro_edges;
mod native_macros;
mod ownership_trace;

use macro_edges::add_verified_macro_edges;
use native_macros::verify_native_macros;
use ownership_trace::attribute_graph_outputs;
#[cfg(test)]
use {
    macro_edges::{closed_macro_arguments, macro_value, multi_compile_pairs},
    native_macros::{
        effective_macro_arguments, parse_native_macro_file, test_macro_contract, verified_macro,
        verified_macro_closure_with_hashes, MacroForm, NativeMacroDefinitions, VerifiedMacros,
        COMPILE_MULTI_SHA256,
    },
    ownership_trace::{trace_all_owners, trace_all_then_paired, DiagnosticBudget},
};

const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_TEMPLATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 16_384;
const MAX_IDENTITIES: usize = 65_536;
const MAX_PATTERN_MATCHES: usize = 65_536;
const MAX_DIAGNOSTIC_WORK: usize = 65_536;
const MAX_DIAGNOSTIC_PATH_BYTES: usize = 16 * 1024 * 1024;
const MAX_MACRO_OUTPUT_BYTES: usize = MAX_SNAPSHOT_BYTES;
const MAX_MACRO_ARGUMENT_BYTES: usize = 64 * 1024;
const MAX_MACRO_ARGUMENTS: usize = 256;
const MAX_TEMPLATE_CLOSURE_DEPTH: usize = 64;
const MAX_TEMPLATE_CLOSURE_WORK: usize = 8192;
const MAX_TEMPLATE_CLOSURE_DEFINITIONS: usize = 256;

/// A source-local owner reached from one rejected output identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRuleOwnership {
    /// Exact owner endpoint from a source `#MM` declaration or verified macro.
    pub owner: String,
    /// Exact source-consumer path supporting this owner attribution. For a
    /// paired compile sidecar, this is the associated object's path; no graph
    /// edge between the sidecar and object is implied.
    pub chain: Vec<String>,
}

#[derive(Debug, Default)]
struct SourceGraph {
    /// Unique identity vertices shared by ordinary rules and verified macro
    /// seeds. Checked before insertion so repeated expansions cannot multiply
    /// the graph without limit.
    identities: BTreeSet<String>,
    /// Retained endpoint occurrences across source rules and macro seeds.
    /// This also bounds line-to-output vectors when repeated variables expand
    /// to the same finite names.
    identity_references: usize,
    /// Aggregate byte budget for retained verified macro output identities.
    macro_output_bytes: usize,
    /// Prerequisite identity -> consumer target identities.
    consumers: BTreeMap<String, BTreeSet<String>>,
    /// Concrete endpoints which the source declares as MetaMake owners.
    owners: BTreeSet<String>,
    /// Producer output identity -> exact `%mmake` owner endpoint(s).
    macro_owners: BTreeMap<String, BTreeSet<String>>,
    /// Concrete ordinary-Make identities eligible to match one-stem rules.
    /// MetaMake owner labels are deliberately excluded unless the same value
    /// also occurs as an ordinary target or prerequisite.
    make_identities: BTreeSet<String>,
    /// Finite outputs of verified compile/assemble macros. The default
    /// `mmake=TMP` is a variable namespace, not a MetaMake owner.
    macro_outputs: BTreeSet<String>,
    /// Macro output identities emitted more than once by verified producers.
    /// These remain diagnostic ambiguity rather than graph edges.
    ambiguous_macro_outputs: BTreeSet<String>,
    /// Exact per-invocation `%rule_compile_multi` output pairs. This is
    /// diagnostic metadata only; it never enters `consumers` or providers.
    compile_multi_groups: Vec<CompileMultiGroup>,
    /// Bounded, one-stem Make pattern rules, instantiated only against finite
    /// identities already present in this source graph.
    patterns: Vec<PatternRule>,
    /// Ordinary rule targets at each joined snapshot line, for checking that a
    /// supplied rejection line really names its declared output.
    targets_by_line: BTreeMap<usize, Vec<String>>,
    /// Ordinary rules with a source recipe. Paired output fallback applies
    /// only to a declaration that adds no separate recipe producer.
    recipe_rule_lines: BTreeSet<usize>,
    /// Lines inside Make `define` bodies. Their contents are inert at parse
    /// time and must not seed owners or macro outputs.
    definition_lines: BTreeSet<usize>,
    edge_count: usize,
    /// Some source syntax could declare an additional consumer or owner but
    /// was conditional, malformed, or not resolvable by this bounded parser.
    uncertain: bool,
    overflow: bool,
}

#[derive(Debug)]
struct PatternRule {
    target: String,
    prerequisites: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompileMultiPair {
    object: String,
    depfile: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompileMultiGroup {
    invocation_line: usize,
    pairs: Vec<CompileMultiPair>,
}

/// Attributes the unique rejected Make target declared at `rejected_rule_line`.
///
/// This form is useful for a collector rejection that records the exact
/// physical joined-snapshot line but cannot resolve the target value itself.
/// It only uses the target identity independently resolved from that same rule;
/// unresolved variable-bearing target names do not become graph vertices.
#[must_use]
#[cfg(test)]
pub fn attribute_rejected_rule_line(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    rejected_rule_line: usize,
) -> Option<SourceRuleOwnership> {
    let owners = attribute_rejected_rule_owners(
        snapshot,
        scope,
        dirs,
        root,
        rel_dir,
        line_states,
        rejected_rule_line,
    )?;
    (owners.len() == 1)
        .then(|| owners.into_iter().next())
        .flatten()
}

/// Attributes every fully proven source owner reachable from a rejected rule.
///
/// A multi-target line is accepted only if every exact target has at least one
/// completely proven owner. Unknown/conditional statements and unsupported
/// pattern forms still veto attribution for the complete snapshot.
#[must_use]
pub fn attribute_rejected_rule_owners(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    rejected_rule_line: usize,
) -> Option<Vec<SourceRuleOwnership>> {
    let macros = verify_native_macros(root);
    let states = line_states?;
    if snapshot.len() > MAX_SNAPSHOT_BYTES || rejected_rule_line == 0 {
        return None;
    }
    let lines = snapshot.lines().collect::<Vec<_>>();
    if lines.len() > MAX_LINES || states.len() < lines.len() {
        return None;
    }
    let rejected_line = rejected_rule_line.checked_sub(1)?;
    let mut graph = parse_source_graph(&lines, scope, dirs, root, rel_dir, states);
    let outputs = graph.targets_by_line.get(&rejected_line)?.clone();
    if outputs.is_empty() {
        return None;
    }
    add_verified_macro_edges(
        &mut graph,
        &lines,
        scope,
        dirs,
        (root, rel_dir),
        states,
        macros,
    );
    instantiate_pattern_edges(&mut graph);
    if graph.overflow || graph.uncertain {
        return None;
    }
    attribute_graph_outputs(&graph, rejected_line, &outputs)
}

#[cfg(test)]
fn attribute_with_verified_macros(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    source_dirs: (&Path, &Path),
    line_states: Option<&[ConditionalTruth]>,
    rejected_rule: (&str, usize),
    macros: VerifiedMacros,
) -> Option<SourceRuleOwnership> {
    let (root, rel_dir) = source_dirs;
    let (output, rejected_rule_line) = rejected_rule;
    if snapshot.len() > MAX_SNAPSHOT_BYTES || output.is_empty() || rejected_rule_line == 0 {
        return None;
    }
    let states = line_states?;
    let lines = snapshot.lines().collect::<Vec<_>>();
    if lines.len() > MAX_LINES || states.len() < lines.len() {
        return None;
    }
    let rejected_line = rejected_rule_line.checked_sub(1)?;
    let mut graph = parse_source_graph(&lines, scope, dirs, root, rel_dir, states);
    let targets = graph.targets_by_line.get(&rejected_line)?;
    if targets.len() != 1 || targets[0] != output {
        return None;
    }
    add_verified_macro_edges(&mut graph, &lines, scope, dirs, source_dirs, states, macros);
    instantiate_pattern_edges(&mut graph);
    if graph.overflow || graph.uncertain {
        return None;
    }
    let owners = trace_all_owners(&graph, output, &mut DiagnosticBudget::default())?;
    (owners.len() == 1)
        .then(|| owners.into_iter().next())
        .flatten()
}

fn parse_source_graph(
    lines: &[&str],
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    states: &[ConditionalTruth],
) -> SourceGraph {
    let mut graph = SourceGraph::default();
    let mut line_owner_marker = false;
    let mut define_depth = 0usize;
    let mut pending_rule_line = None;

    for (line_no, raw) in lines.iter().enumerate() {
        if define_depth > 0 {
            graph.definition_lines.insert(line_no);
            match make_define_boundary(raw) {
                Some((true, valid)) => {
                    graph.uncertain |= !valid;
                    define_depth = define_depth.saturating_add(1);
                }
                Some((false, valid)) => {
                    graph.uncertain |= !valid;
                    define_depth -= 1;
                }
                None => {}
            }
            continue;
        }
        if let Some((true, valid)) = make_define_boundary(raw) {
            graph.definition_lines.insert(line_no);
            graph.uncertain |= !valid;
            define_depth = 1;
            line_owner_marker = false;
            pending_rule_line = None;
            continue;
        }
        if make_define_boundary(raw).is_some_and(|(start, _)| !start) {
            graph.definition_lines.insert(line_no);
            graph.uncertain = true;
            line_owner_marker = false;
            pending_rule_line = None;
            continue;
        }
        match state_at(states, line_no) {
            ConditionalTruth::False => {
                line_owner_marker = false;
                pending_rule_line = None;
                continue;
            }
            ConditionalTruth::Unknown => {
                graph.uncertain |= is_potential_graph_statement(raw)
                    || is_make_include(raw.trim())
                    || contains_make_eval(raw)
                    || has_unproven_make_expansion(raw, scope, dirs, root, rel_dir, line_no);
                line_owner_marker = false;
                pending_rule_line = None;
                continue;
            }
            ConditionalTruth::True => {}
        }
        if contains_make_eval(raw) {
            // GNU Make's eval function parses its expanded argument as new
            // Makefile syntax. Even when the text comes from an opaque define
            // body, it can add consumers or owners missing from this snapshot.
            graph.uncertain = true;
            line_owner_marker = false;
            continue;
        }
        let trimmed = raw.trim();
        if raw.starts_with('\t') {
            if let Some(rule_line) = pending_rule_line {
                graph.recipe_rule_lines.insert(rule_line);
            }
            line_owner_marker = false;
            continue;
        }
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            pending_rule_line = None;
        }
        if trimmed == "#MM" {
            line_owner_marker = true;
            continue;
        }
        if trimmed.starts_with("#MM") && !trimmed.starts_with("##MM") {
            let Some(edge) = parse_meta_edge(trimmed) else {
                graph.uncertain = true;
                line_owner_marker = false;
                continue;
            };
            line_owner_marker = false;
            let Some(owner) = evaluate_one(edge.target, scope, dirs, root, rel_dir, line_no)
                .and_then(|target| safe_owner(&target))
            else {
                graph.uncertain = true;
                continue;
            };
            let Some(prerequisites) =
                evaluate_words(edge.prerequisites, scope, dirs, root, rel_dir, line_no)
            else {
                graph.uncertain = true;
                continue;
            };
            if !charge_identity_references(&mut graph, prerequisites.len().saturating_add(1))
                || !record_identity(&mut graph, &owner)
            {
                return graph;
            }
            for prerequisite in &prerequisites {
                if !record_identity(&mut graph, prerequisite) {
                    return graph;
                }
            }
            graph.owners.insert(owner.clone());
            add_consumer_edges(&mut graph, &prerequisites, &owner);
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') || raw.starts_with('\t') {
            line_owner_marker = false;
            continue;
        }
        if is_make_include(trimmed) {
            // Path resolution alone does not prove the included fragment's
            // active rules or side effects are represented in this snapshot.
            graph.uncertain = true;
            line_owner_marker = false;
            continue;
        }
        if is_make_directive(trimmed) || variable_assignment(trimmed).is_some() {
            graph.uncertain |=
                has_unproven_make_expansion(raw, scope, dirs, root, rel_dir, line_no);
            line_owner_marker = false;
            continue;
        }
        let uncommented = strip_make_comment(trimmed).trim();
        let Some((target_raw, prerequisites_raw)) = split_rule(uncommented) else {
            graph.uncertain |= looks_like_ordinary_rule(uncommented);
            if starts_with_make_expansion(uncommented) {
                graph.uncertain |=
                    has_unproven_make_expansion(raw, scope, dirs, root, rel_dir, line_no);
            }
            line_owner_marker = false;
            continue;
        };
        if variable_assignment(prerequisites_raw.trim()).is_some() {
            line_owner_marker = false;
            continue;
        }
        let Some(targets) = evaluate_words(target_raw, scope, dirs, root, rel_dir, line_no) else {
            graph.uncertain = true;
            line_owner_marker = false;
            continue;
        };
        if targets.is_empty() || targets.iter().any(|target| !safe_identity(target)) {
            graph.uncertain = true;
            line_owner_marker = false;
            continue;
        }
        let has_pattern = targets.iter().any(|target| target.contains('%'));
        if has_pattern {
            if line_owner_marker || targets.len() != 1 || !is_bounded_pattern(&targets[0]) {
                graph.uncertain = true;
                line_owner_marker = false;
                continue;
            }
            let Some(prerequisites) = evaluate_pattern_prerequisites(
                prerequisites_raw,
                scope,
                dirs,
                root,
                rel_dir,
                line_no,
            ) else {
                graph.uncertain = true;
                line_owner_marker = false;
                continue;
            };
            if !charge_identity_references(
                &mut graph,
                targets.len().saturating_add(prerequisites.len()),
            ) {
                return graph;
            }
            graph.patterns.push(PatternRule {
                target: targets[0].clone(),
                prerequisites,
            });
            line_owner_marker = false;
            continue;
        }
        let Some(prerequisites) =
            evaluate_rule_prerequisites(prerequisites_raw, scope, dirs, root, rel_dir, line_no)
        else {
            graph.uncertain = true;
            continue;
        };
        if !charge_identity_references(
            &mut graph,
            targets.len().saturating_add(prerequisites.len()),
        ) {
            return graph;
        }
        for target in &targets {
            if !record_identity(&mut graph, target) {
                return graph;
            }
        }
        for prerequisite in &prerequisites {
            if !record_identity(&mut graph, prerequisite) {
                return graph;
            }
        }
        graph.targets_by_line.insert(line_no, targets.clone());
        pending_rule_line = Some(line_no);
        for target in &targets {
            graph.make_identities.insert(target.clone());
        }
        if line_owner_marker {
            for target in &targets {
                if safe_owner(target).is_some() {
                    graph.owners.insert(target.clone());
                } else {
                    graph.uncertain = true;
                }
            }
        }
        line_owner_marker = false;
        let edge_count = targets.len().saturating_mul(prerequisites.len());
        if edge_count > MAX_IDENTITIES.saturating_sub(graph.edge_count) {
            graph.overflow = true;
            return graph;
        }
        for target in &targets {
            add_make_consumer_edges(&mut graph, &prerequisites, target);
        }
    }
    if define_depth != 0 {
        graph.uncertain = true;
    }
    graph
}

/// Whether this active line contains an unescaped GNU Make `eval` function.
///
/// The diagnostic graph deliberately does not interpret the text evaluated by
/// Make. Scan every expansion opener (including nested ones) so `eval` in an
/// assignment RHS or recipe is just as disqualifying as a top-level call.
fn contains_make_eval(line: &str) -> bool {
    let line = strip_make_comment(line);
    let bytes = line.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] != b'$' || !matches!(bytes[index + 1], b'(' | b'{') {
            index += 1;
            continue;
        }
        // `$$(` and `$${` quote the following opener for Make expansion.
        let mut preceding_dollars = 0usize;
        let mut before = index;
        while before > 0 && bytes[before - 1] == b'$' {
            preceding_dollars += 1;
            before -= 1;
        }
        if preceding_dollars % 2 == 1 {
            index += 2;
            continue;
        }

        let mut function = index + 2;
        while function < bytes.len() && bytes[function].is_ascii_whitespace() {
            function += 1;
        }
        if bytes.get(function..function + 4) == Some(b"eval")
            && bytes
                .get(function + 4)
                .is_some_and(|next| next.is_ascii_whitespace() || *next == b',')
        {
            return true;
        }
        index += 2;
    }
    false
}

fn starts_with_make_expansion(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("$(") || line.starts_with("${")
}

/// Checks expansions in syntax that this graph collector otherwise ignores
/// (assignments, directives, or a top-level expansion-only statement). An
/// unsupported or unresolved expansion could invoke an opaque user variable
/// containing `eval`, so it cannot be assumed side-effect-free.
fn has_unproven_make_expansion(
    line: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_no: usize,
) -> bool {
    let uncommented = strip_make_comment(line);
    let candidate = if let Some((_, rhs, _)) = variable_assignment(uncommented) {
        rhs
    } else if is_make_directive(uncommented.trim()) || starts_with_make_expansion(uncommented) {
        uncommented
    } else {
        return false;
    };
    has_unproven_make_expansion_in_text(candidate, scope, dirs, root, rel_dir, line_no)
}

/// Checks every Make expansion in an invocation's raw source arguments,
/// including invocations whose GenMF semantics are not modeled here. This is
/// only an expansion-safety check; it does not project outputs for unknown
/// macros.
fn has_unproven_make_expansion_in_text(
    candidate: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_no: usize,
) -> bool {
    let bytes = candidate.as_bytes();
    let context = MakeExprContext::new(scope, dirs, line_no, root, rel_dir);
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] != b'$' || !matches!(bytes[index + 1], b'(' | b'{') {
            index += 1;
            continue;
        }
        let mut preceding_dollars = 0usize;
        let mut before = index;
        while before > 0 && bytes[before - 1] == b'$' {
            preceding_dollars += 1;
            before -= 1;
        }
        if preceding_dollars % 2 == 1 {
            index += 2;
            continue;
        }
        let Some(end) = matching_make_expansion_end(bytes, index) else {
            return true;
        };
        let Ok(expression) = std::str::from_utf8(&bytes[index..=end]) else {
            return true;
        };
        if evaluate_make_expr(expression, &context).is_err() {
            return true;
        }
        index += 2;
    }
    false
}

fn matching_make_expansion_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut stack = vec![match bytes.get(start + 1)? {
        b'(' => b')',
        b'{' => b'}',
        _ => return None,
    }];
    let mut index = start + 2;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = index.saturating_add(2),
            b'$' if matches!(bytes.get(index + 1), Some(b'(' | b'{')) => {
                stack.push(if bytes[index + 1] == b'(' { b')' } else { b'}' });
                index += 2;
            }
            b'(' => {
                stack.push(b')');
                index += 1;
            }
            b'{' => {
                stack.push(b'}');
                index += 1;
            }
            byte if stack.last() == Some(&byte) => {
                stack.pop();
                if stack.is_empty() {
                    return Some(index);
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    None
}

fn make_define_boundary(line: &str) -> Option<(bool, bool)> {
    let uncommented = strip_make_comment(line.trim_start()).trim_start();
    let mut words = uncommented.split_whitespace();
    let mut directive = words.next()?;
    let mut modifiers = 0usize;
    while matches!(directive, "override" | "export" | "private") {
        modifiers += 1;
        let next = words.next()?;
        directive = next;
    }
    match directive {
        "define" => Some((true, modifiers <= 3)),
        "endef" => Some((false, modifiers == 0)),
        _ => None,
    }
}

fn is_potential_graph_statement(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() || raw.starts_with('\t') {
        return false;
    }
    if trimmed == "#MM"
        || (trimmed.starts_with("#MM") && !trimmed.starts_with("##MM"))
        || trimmed.starts_with('%')
    {
        return true;
    }
    let uncommented = strip_make_comment(trimmed).trim();
    looks_like_ordinary_rule(uncommented)
}

fn looks_like_ordinary_rule(line: &str) -> bool {
    !line.is_empty()
        && !is_make_directive(line)
        && variable_assignment(line).is_none()
        && line.contains(':')
}

struct MetaEdge<'a> {
    target: &'a str,
    prerequisites: &'a str,
}

fn parse_meta_edge(line: &str) -> Option<MetaEdge<'_>> {
    let body = line
        .strip_prefix("#MM-")
        .or_else(|| line.strip_prefix("#MM"))?;
    if !body.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let body = body.trim_start();
    let (target, prerequisites) = body.split_once(':')?;
    let target = target.trim();
    Some(MetaEdge {
        target,
        prerequisites: strip_make_comment(prerequisites).trim(),
    })
}

fn split_rule(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('#') || line.starts_with('%') || line.contains(';') {
        return None;
    }
    let (target, after_colon) = line.split_once(':')?;
    let prerequisites = if let Some(tail) = after_colon.strip_prefix(':') {
        if tail.starts_with(':') {
            return None;
        }
        tail
    } else {
        after_colon
    };
    if target.is_empty() || target.contains(':') {
        return None;
    }
    let target = target.trim();
    if target.is_empty() || target.contains(['*', '?', '[', ']', '|', '&', '\\']) {
        return None;
    }
    Some((target, prerequisites.trim()))
}

fn is_bounded_pattern(pattern: &str) -> bool {
    pattern.matches('%').count() == 1
        && !pattern.contains(['*', '?', '[', ']', '|', '&', '\\'])
        && safe_identity(pattern)
}

fn evaluate_pattern_prerequisites(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<Vec<String>> {
    if raw.contains(['*', '?', '[', ']', '\\', ';']) {
        return None;
    }
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let words = evaluate_make_list(raw, &context).ok()?;
    if words.len() > MAX_IDENTITIES {
        return None;
    }
    if words.iter().filter(|word| word.as_str() == "|").count() > 1 {
        return None;
    }
    let prerequisites = words
        .into_iter()
        .filter(|word| word != "|")
        .collect::<Vec<_>>();
    prerequisites
        .iter()
        .all(|prerequisite| {
            safe_identity(prerequisite)
                && prerequisite.matches('%').count() <= 1
                && !prerequisite.contains(['*', '?', '[', ']', '|', '&', '\\'])
        })
        .then_some(prerequisites)
}

fn is_make_directive(line: &str) -> bool {
    [
        "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "define", "endef", "export",
        "unexport", "include", "-include", "sinclude", "override", "vpath",
    ]
    .iter()
    .any(|directive| {
        line.strip_prefix(directive).is_some_and(|tail| {
            tail.is_empty()
                || tail
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_whitespace() || character == '(')
        })
    })
}

fn is_make_include(line: &str) -> bool {
    ["include", "-include", "sinclude"].iter().any(|directive| {
        line.strip_prefix(directive).is_some_and(|tail| {
            tail.is_empty() || tail.chars().next().is_some_and(char::is_whitespace)
        })
    })
}

fn evaluate_rule_prerequisites(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<Vec<String>> {
    if raw.contains(['*', '?', '[', ']', '\\', ';']) {
        return None;
    }
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let words = evaluate_make_list(raw, &context).ok()?;
    if words.len() > MAX_IDENTITIES {
        return None;
    }
    let separators = words.iter().filter(|word| word.as_str() == "|").count();
    if separators > 1 {
        return None;
    }
    let prerequisites = words
        .into_iter()
        .filter(|word| word != "|")
        .collect::<Vec<_>>();
    prerequisites
        .iter()
        .all(|item| safe_identity(item) && !item.contains('%'))
        .then_some(prerequisites)
}

fn evaluate_words(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<Vec<String>> {
    if raw.len() > 64 * 1024 || raw.contains(['*', '?', '[', ']', '\\', ';']) {
        return None;
    }
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir);
    let words = evaluate_make_list(raw, &context).ok()?;
    (words.len() <= MAX_IDENTITIES && words.iter().all(|word| safe_identity(word))).then_some(words)
}

fn add_consumer_edges(graph: &mut SourceGraph, prerequisites: &[String], target: &str) {
    if !record_identity(graph, target) {
        return;
    }
    for prerequisite in prerequisites {
        if !record_identity(graph, prerequisite) {
            return;
        }
        let inserted = graph
            .consumers
            .entry(prerequisite.clone())
            .or_default()
            .insert(target.to_owned());
        if inserted {
            graph.edge_count = graph.edge_count.saturating_add(1);
            graph.overflow |= graph.edge_count > MAX_IDENTITIES;
        }
    }
}

fn add_make_consumer_edges(graph: &mut SourceGraph, prerequisites: &[String], target: &str) {
    if !record_identity(graph, target) {
        return;
    }
    graph.make_identities.insert(target.to_owned());
    for prerequisite in prerequisites {
        if !record_identity(graph, prerequisite) {
            return;
        }
        graph.make_identities.insert(prerequisite.clone());
    }
    add_consumer_edges(graph, prerequisites, target);
}

const fn charge_identity_references(graph: &mut SourceGraph, count: usize) -> bool {
    if count > MAX_IDENTITIES.saturating_sub(graph.identity_references) {
        graph.overflow = true;
        return false;
    }
    graph.identity_references += count;
    true
}

fn charge_macro_output_bytes(graph: &mut SourceGraph, outputs: &[String]) -> bool {
    let Some(bytes) = outputs
        .iter()
        .try_fold(0usize, |total, output| total.checked_add(output.len()))
    else {
        graph.overflow = true;
        return false;
    };
    if bytes > MAX_MACRO_OUTPUT_BYTES.saturating_sub(graph.macro_output_bytes) {
        graph.overflow = true;
        return false;
    }
    graph.macro_output_bytes += bytes;
    true
}

fn record_identity(graph: &mut SourceGraph, identity: &str) -> bool {
    if graph.identities.contains(identity) {
        return true;
    }
    if graph.identities.len() >= MAX_IDENTITIES {
        graph.overflow = true;
        return false;
    }
    graph.identities.insert(identity.to_owned());
    true
}

fn evaluate_one(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Option<String> {
    let values = evaluate_words(raw, scope, dirs, root, rel_dir, line)?;
    (values.len() == 1).then(|| values[0].clone())
}

fn unique_outputs(outputs: Vec<String>) -> Vec<String> {
    let unique = outputs.iter().collect::<BTreeSet<_>>();
    if outputs.len() <= MAX_IDENTITIES
        && unique.len() == outputs.len()
        && outputs.iter().all(|output| safe_identity(output))
    {
        outputs
    } else {
        Vec::new()
    }
}

fn join_output(directory: &str, basename: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{basename}")
    } else {
        format!("{directory}/{basename}")
    }
}

fn instantiate_pattern_edges(graph: &mut SourceGraph) {
    let mut applied = BTreeSet::<(usize, String)>::new();
    let mut match_attempts = 0usize;
    loop {
        let identities = graph.make_identities.iter().cloned().collect::<Vec<_>>();
        // Pattern references are snapshotted before edges are added so the
        // graph can be mutably budgeted while matching without holding an
        // immutable borrow into `graph.patterns`.
        let patterns = graph
            .patterns
            .iter()
            .map(|pattern| (pattern.target.clone(), pattern.prerequisites.clone()))
            .collect::<Vec<_>>();
        let mut additions = Vec::<(String, String)>::new();
        for (pattern_index, (pattern_target, prerequisites)) in patterns.iter().enumerate() {
            for target in &identities {
                match_attempts = match_attempts.saturating_add(1);
                if match_attempts > MAX_PATTERN_MATCHES {
                    graph.uncertain = true;
                    return;
                }
                let Some(stem) = pattern_stem(pattern_target, target) else {
                    continue;
                };
                if !applied.insert((pattern_index, target.clone())) {
                    continue;
                }
                if !charge_identity_references(graph, prerequisites.len().saturating_add(1))
                    || prerequisites.len() > MAX_IDENTITIES.saturating_sub(additions.len())
                {
                    graph.overflow = true;
                    return;
                }
                for prerequisite in prerequisites {
                    let prerequisite = if prerequisite.contains('%') {
                        substitute_stem(prerequisite, &stem)
                    } else {
                        prerequisite.clone()
                    };
                    if !safe_identity(&prerequisite) {
                        graph.uncertain = true;
                        return;
                    }
                    additions.push((prerequisite, target.clone()));
                }
            }
        }
        if additions.is_empty() {
            break;
        }
        let before = graph.edge_count;
        for (prerequisite, target) in additions {
            add_make_consumer_edges(graph, &[prerequisite], &target);
        }
        if graph.overflow {
            return;
        }
        if graph.edge_count == before {
            break;
        }
    }
}

fn pattern_stem(pattern: &str, identity: &str) -> Option<String> {
    let (prefix, suffix) = pattern.split_once('%')?;
    if !identity.starts_with(prefix)
        || !identity.ends_with(suffix)
        || identity.len() < prefix.len().saturating_add(suffix.len())
    {
        return None;
    }
    let end = identity.len().checked_sub(suffix.len())?;
    let stem = identity.get(prefix.len()..end)?;
    (!stem.is_empty()).then(|| stem.to_owned())
}

fn substitute_stem(pattern: &str, stem: &str) -> String {
    let (prefix, suffix) = pattern
        .split_once('%')
        .expect("pattern prerequisites are validated with at most one stem");
    format!("{prefix}{stem}{suffix}")
}

fn safe_filename(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.'))
}

fn safe_identity(value: &str) -> bool {
    let unresolved = value
        .replace("${AROS_BUILD_DIR}", "")
        .replace("${AROS_SOURCE_DIR}", "")
        .replace("${AROS_PORTS_DIR}", "")
        .replace("${AROS_PORTS_SOURCE_DIR}", "");
    !value.is_empty()
        && value.len() <= 4096
        && !unresolved.contains(['$', '*', '?', '[', ']', '|', '&', ';', '\\', '\n', '\r'])
        && Path::new(value)
            .components()
            .all(|component| !matches!(component, std::path::Component::ParentDir))
}

fn safe_owner(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 160
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+')))
    .then(|| value.to_owned())
}

const fn state_at(states: &[ConditionalTruth], line: usize) -> ConditionalTruth {
    if line < states.len() {
        states[line]
    } else {
        ConditionalTruth::Unknown
    }
}

#[cfg(test)]
#[path = "source_rule_ownership_tests.rs"]
mod tests;
