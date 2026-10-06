//! Target name safety checks and local alias expansion for SDK text rules.

use crate::make_vars::VarScope;
use std::collections::BTreeSet;

pub(super) fn safe_target_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_.+-".contains(ch))
}

pub(super) fn safe_pc_basename(name: &str) -> bool {
    has_exact_pc_suffix(name)
        && name.len() > 3
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_.+-".contains(ch))
}

pub(super) fn has_exact_pc_suffix(path: &str) -> bool {
    path.as_bytes().ends_with(b".pc")
}

const MAX_TARGET_ALIAS_DEPTH: usize = 16;

const MAX_TARGET_ALIAS_BYTES: usize = 16 * 1024;

pub(super) fn target_has_pc_suffix_after_local_aliases(
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
