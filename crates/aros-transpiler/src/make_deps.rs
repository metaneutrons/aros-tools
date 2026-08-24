//! Which Make variables an expression depends on.
//!
//! A capability that pins a declaration has to know what its text reads, not
//! only what it says: `$(basename $(HAL_OBJS))` depends on `HAL_OBJS`, and a
//! conditional depends on whatever its operands mention. Walking the references
//! is the only way to decide whether two declarations are talking about the same
//! variables, which is what a provider/consumer pair has to establish.
//!
//! Nested references are resolved by finding the matching parenthesis rather
//! than by a regex, because `$(patsubst %.c,%.o,$(FILES))` nests and a
//! non-greedy match gets the wrong end. Whitespace at the top level of an
//! expression is found the same way: inside a nested reference it is not a
//! separator.

use crate::make_vars::strip_make_comment;
use std::collections::HashSet;

pub(crate) fn references_any_make_variable(raw: &str, names: &[String]) -> bool {
    names
        .iter()
        .any(|name| raw.contains(&format!("$({name})")) || raw.contains(&format!("${{{name}}}")))
}

pub(crate) fn make_variable_reference_count(raw: &str, name: &str) -> usize {
    raw.match_indices(&format!("$({name})")).count()
        + raw.match_indices(&format!("${{{name}}}")).count()
}

pub(crate) fn make_reference_end(raw: &str, dollar: usize) -> Option<usize> {
    let bytes = raw.as_bytes();
    let first = *bytes.get(dollar + 1)?;
    let mut closers = vec![match first {
        b'(' => b')',
        b'{' => b'}',
        _ => return None,
    }];
    let mut cursor = dollar + 2;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' {
            match bytes.get(cursor + 1) {
                Some(b'(') => {
                    closers.push(b')');
                    cursor += 2;
                    continue;
                }
                Some(b'{') => {
                    closers.push(b'}');
                    cursor += 2;
                    continue;
                }
                Some(b'$') => {
                    cursor += 2;
                    continue;
                }
                _ => {}
            }
        }
        if Some(&bytes[cursor]) == closers.last() {
            closers.pop();
            if closers.is_empty() {
                return Some(cursor);
            }
        }
        cursor += 1;
    }
    None
}

pub(crate) fn top_level_make_whitespace(raw: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut closers = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' {
            match bytes.get(cursor + 1) {
                Some(b'(') => {
                    closers.push(b')');
                    cursor += 2;
                    continue;
                }
                Some(b'{') => {
                    closers.push(b'}');
                    cursor += 2;
                    continue;
                }
                Some(b'$') => {
                    cursor += 2;
                    continue;
                }
                _ => {}
            }
        }
        if Some(&bytes[cursor]) == closers.last() {
            closers.pop();
        } else if closers.is_empty() && bytes[cursor].is_ascii_whitespace() {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

pub(crate) fn collect_make_expression_dependencies(
    raw: &str,
    dependencies: &mut HashSet<String>,
) -> Option<()> {
    let mut cursor = 0usize;
    while let Some(relative) = raw[cursor..].find('$') {
        let dollar = cursor + relative;
        match raw.as_bytes().get(dollar + 1) {
            Some(b'$') => {
                cursor = dollar + 2;
            }
            Some(b'(' | b'{') => {
                let end = make_reference_end(raw, dollar)?;
                let body = &raw[dollar + 2..end];
                let trimmed = body.trim();
                if trimmed.is_empty() {
                    return None;
                }
                if let Some(space) = top_level_make_whitespace(trimmed) {
                    let function = &trimmed[..space];
                    if !matches!(
                        function,
                        "addprefix"
                            | "addsuffix"
                            | "filter"
                            | "filter-out"
                            | "patsubst"
                            | "subst"
                            | "notdir"
                            | "dir"
                            | "basename"
                            | "suffix"
                            | "sort"
                            | "strip"
                            | "wildcard"
                            | "call"
                    ) {
                        return None;
                    }
                    collect_make_expression_dependencies(
                        trimmed[space..].trim_start(),
                        dependencies,
                    )?;
                } else {
                    let raw_name = trimmed.split_once(':').map_or(trimmed, |(name, _)| name);
                    // Computed variable names can select a hidden target or
                    // file-scope control property. The literal-header scope
                    // deliberately admits only statically named dependencies.
                    if raw_name.is_empty()
                        || raw_name.contains('$')
                        || !raw_name.chars().all(|character| {
                            character.is_ascii_alphanumeric()
                                || matches!(character, '_' | '-' | '.')
                        })
                    {
                        return None;
                    }
                    dependencies.insert(raw_name.to_owned());
                    if let Some((_, substitution)) = trimmed.split_once(':') {
                        collect_make_expression_dependencies(substitution, dependencies)?;
                    }
                }
                cursor = end + 1;
            }
            // Single-character automatic variables and an unterminated `$`
            // have no stable declaration-time meaning here.
            _ => return None,
        }
    }
    Some(())
}

pub(crate) fn make_expression_dependencies(raw: &str) -> Option<HashSet<String>> {
    let mut dependencies = HashSet::new();
    collect_make_expression_dependencies(raw, &mut dependencies)?;
    Some(dependencies)
}

pub(crate) fn make_conditional_dependencies(
    directive: &str,
    args: &str,
) -> Option<HashSet<String>> {
    if matches!(directive, "ifdef" | "ifndef") {
        let name = args.trim();
        if name.is_empty()
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
            })
        {
            return None;
        }
        return Some(std::iter::once(name.to_owned()).collect());
    }
    make_expression_dependencies(args)
}

pub(crate) fn make_semantic_lines(content: &str) -> String {
    let mut semantic = String::with_capacity(content.len());
    for line in content.lines() {
        // A #MM marker has no expression of its own. Its following ordinary
        // rule remains visible and is counted like every other Make line.
        if !line.trim_start().starts_with("#MM") {
            semantic.push_str(strip_make_comment(line));
        }
        semantic.push('\n');
    }
    semantic
}
