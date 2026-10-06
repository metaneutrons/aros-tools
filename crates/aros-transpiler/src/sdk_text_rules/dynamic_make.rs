//! Detection of dynamic Make constructs that may consume an SDK text output.

use super::{
    is_conditional_directive, is_make_define_close, is_make_define_open, logical_lines,
    mask_define_bodies, LogicalLine, MetaEdge, SourceLocation,
};
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{ConditionalTruth, VarScope};
use std::collections::BTreeSet;

pub(super) fn has_active_output_consumer(
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
pub(super) fn has_active_dynamic_make_expansion(
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
pub(super) fn prerequisite_may_consume(
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
