//! Bounded, positional recursive GNU Make `define` values.
//!
//! Bodies are data, not executed programs. This collector admits recursive
//! definitions and tracks ordinary replacement/undefine shadowing. Simple
//! multiline definitions and ambiguous branches refuse until their immediate
//! evaluation can be modelled; no project-specific variable names are used.

use crate::make_vars::{
    literal_variable_name, make_define_directive, strip_make_comment, undefine_directive,
    variable_assignment, AssignmentKind,
};
use std::collections::BTreeMap;

/// Source-position definition snapshot for opt-in Make expression evaluation.
#[derive(Clone, Debug)]
pub struct MakeDefinitions {
    pub(crate) line: usize,
    pub(crate) bodies: BTreeMap<String, String>,
}

impl MakeDefinitions {
    /// Collect definitions visible before `line` in a continuation-joined
    /// source. States use `Some(true)`/`Some(false)`/`None` for active/inactive/
    /// unproved, with one entry per source line.
    ///
    /// # Errors
    /// Rejects malformed, oversized, unsupported or ambiguous definitions.
    pub fn collect_at(program: &str, line: usize, states: &[Option<bool>]) -> Result<Self, String> {
        let lines = program.lines().collect::<Vec<_>>();
        if program.len() > 16 * 1024 * 1024 || states.len() != lines.len() {
            return Err("invalid Make definition input or limits".into());
        }
        let mut result = Self {
            line,
            bodies: BTreeMap::new(),
        };
        let mut at = 0;
        while at < line.min(lines.len()) {
            let raw = lines[at];
            if raw.starts_with('\t') {
                at += 1;
                continue;
            }
            let visible = strip_make_comment(raw).trim();
            if states[at] != Some(false)
                && matches!(
                    visible.split_whitespace().next(),
                    Some("override" | "private" | "export" | "unexport")
                )
            {
                // A modifier can replace or undefine an earlier definition.
                // Until its complete semantics are modelled, never retain a
                // stale body by silently ignoring the directive.
                return Err("unsupported Make definition modifier directive".into());
            }
            if let Some(("define", arguments)) = make_define_directive(visible) {
                let state = states[at];
                let (name, flavor) = match variable_assignment(arguments) {
                    Some((name, "", flavor)) => (name, flavor),
                    None => (arguments.trim(), AssignmentKind::RecursiveSet),
                    _ => return Err("unsupported Make define header".into()),
                };
                let mut depth = 1usize;
                let mut body = String::new();
                at += 1;
                while at < line.min(lines.len()) {
                    match (!lines[at].starts_with('\t'))
                        .then(|| make_define_directive(lines[at]))
                        .flatten()
                    {
                        Some(("define", _)) => depth += 1,
                        Some(("endef", "")) => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    if depth > 16 || body.len() + lines[at].len() + 1 > 1024 * 1024 {
                        return Err("Make definition exceeds nesting or byte limit".into());
                    }
                    body.push_str(lines[at]);
                    body.push('\n');
                    at += 1;
                }
                if depth != 0 {
                    return Err("unterminated Make definition before evaluation".into());
                }
                if state != Some(false) {
                    if state != Some(true) {
                        return Err("Make definition has unresolved selection".into());
                    }
                    if !visible
                        .strip_prefix("define")
                        .is_some_and(|tail| tail.starts_with(char::is_whitespace))
                        || !literal_variable_name(name)
                        || !matches!(flavor, AssignmentKind::RecursiveSet)
                    {
                        return Err("unsupported Make definition name or flavor".into());
                    }
                    if body.ends_with('\n') {
                        body.pop();
                    }
                    result.bodies.insert(name.to_owned(), body);
                }
            } else if matches!(make_define_directive(visible), Some(("endef", _))) {
                return Err("unmatched Make endef".into());
            } else if states[at] != Some(false) {
                if let Some((name, value, kind)) = variable_assignment(visible) {
                    if result.bodies.contains_key(name) {
                        if states[at] != Some(true) {
                            return Err("Make definition shadow has unresolved selection".into());
                        }
                        match kind {
                            AssignmentKind::SetIfUnset => {}
                            AssignmentKind::Append => {
                                let body = result
                                    .bodies
                                    .get_mut(name)
                                    .ok_or("missing Make definition")?;
                                body.push(' ');
                                body.push_str(value);
                            }
                            _ => {
                                result.bodies.remove(name);
                            }
                        }
                    }
                } else {
                    match undefine_directive(visible) {
                        Ok(Some(name)) => {
                            if states[at] != Some(true) && result.bodies.contains_key(name) {
                                return Err(
                                    "Make definition undefine has unresolved selection".into()
                                );
                            }
                            result.bodies.remove(name);
                        }
                        Err(()) => return Err("unsupported Make definition undefine".into()),
                        Ok(None) => {
                            // An expanded LHS can overwrite any collected
                            // body. Distinguish it from target-local syntax
                            // using the same nested-reference scanner as
                            // expression evaluation, never a name substring.
                            let conditional = matches!(
                                visible.split_whitespace().next(),
                                Some("ifeq" | "ifneq" | "ifdef" | "ifndef" | "else" | "endif")
                            );
                            if let Some(equal) = (!conditional)
                                .then(|| crate::make_expr::top_level_byte(visible, b'='))
                                .flatten()
                            {
                                let lhs = visible[..equal]
                                    .trim_end()
                                    .trim_end_matches([':', '?', '+']);
                                if crate::make_expr::top_level_byte(lhs, b':').is_none() {
                                    return Err(
                                        "unsupported Make definition shadow assignment".into()
                                    );
                                }
                            }
                        }
                    }
                }
            }
            if result.bodies.len() > 256
                || result.bodies.values().map(String::len).sum::<usize>() > 8 * 1024 * 1024
            {
                return Err("Make definitions exceed aggregate limit".into());
            }
            at += 1;
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "make_definitions_tests.rs"]
mod tests;
