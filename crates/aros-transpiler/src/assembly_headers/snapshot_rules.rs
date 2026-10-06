//! Physical snapshot parsing into Make rules and path classification helpers.

use super::{
    strip_make_comment, Component, ConditionalTruth, MetaRule, Path, Recipe, Rule, SnapshotRules,
    BUILD_ALIAS, MAX_RULES, SOURCE_ALIAS,
};

pub(super) fn parse_snapshot(snapshot: &str, line_states: &[ConditionalTruth]) -> SnapshotRules {
    let mut parsed = SnapshotRules {
        rules: Vec::new(),
        meta_rules: Vec::new(),
    };
    let mut pending_bare_meta = None;
    for (line, raw) in snapshot.lines().enumerate() {
        if raw.starts_with('\t') {
            if let Some(rule) = parsed.rules.last_mut() {
                rule.recipes.push(Recipe {
                    text: raw.trim().to_owned(),
                    state: line_state(line_states, line),
                });
            }
            continue;
        }

        let trimmed = raw.trim();
        if let Some(meta) = parse_meta(trimmed, line, line_state(line_states, line)) {
            match meta {
                ParsedMeta::Bare => pending_bare_meta = Some(line),
                ParsedMeta::Rule(meta_rule) => {
                    pending_bare_meta = None;
                    parsed.meta_rules.push(meta_rule);
                }
            }
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let clean = strip_make_comment(raw).trim();
        if let Some((target, body)) = parse_rule(clean) {
            if parsed.rules.len() < MAX_RULES {
                parsed.rules.push(Rule {
                    target: target.to_owned(),
                    prerequisites: body.prerequisites,
                    order_only: body.order_only,
                    line,
                    state: line_state(line_states, line),
                    bare_meta_line: pending_bare_meta.take(),
                    inline_recipe: body.inline_recipe,
                    recipes: Vec::new(),
                });
            }
        } else if !clean.starts_with('.') && !clean.starts_with('%') {
            // A non-rule statement ends a pending bare marker; Make would not
            // associate the marker across an assignment or directive.
            if !clean.starts_with("ifeq")
                && !clean.starts_with("ifneq")
                && clean != "else"
                && clean != "endif"
            {
                pending_bare_meta = None;
            }
        }
    }
    parsed
}

#[derive(Debug)]
enum ParsedMeta {
    Bare,
    Rule(MetaRule),
}

fn parse_meta(line: &str, source_line: usize, state: ConditionalTruth) -> Option<ParsedMeta> {
    let after = line.strip_prefix("#MM")?;
    let mut rest = after.trim_start();
    if let Some(without_dash) = rest.strip_prefix('-') {
        if without_dash.starts_with(char::is_whitespace) {
            rest = without_dash.trim_start();
        }
    }
    if rest.is_empty() {
        return Some(ParsedMeta::Bare);
    }
    let (target, prerequisites) = split_target_colon(rest)?;
    (!target.is_empty()).then(|| {
        ParsedMeta::Rule(MetaRule {
            target: target.trim().to_owned(),
            prerequisites: prerequisites.to_owned(),
            line: source_line,
            state,
        })
    })
}

#[derive(Debug)]
struct RuleBody {
    prerequisites: String,
    order_only: Option<String>,
    inline_recipe: Option<String>,
}

fn parse_rule(line: &str) -> Option<(&str, RuleBody)> {
    let (target, rest) = split_target_colon(line)?;
    let target = target.trim();
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    let (body, inline_recipe) = split_unquoted_semicolon(rest);
    let (prerequisites, order_only) = split_order_only(body);
    Some((
        target.trim(),
        RuleBody {
            prerequisites: prerequisites.trim().to_owned(),
            order_only: order_only.map(|value| value.trim().to_owned()),
            inline_recipe: inline_recipe
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        },
    ))
}

pub(super) fn split_target_colon(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut make_depth = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            make_depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && make_depth > 0 {
            make_depth -= 1;
        } else if bytes[cursor] == b':' && make_depth == 0 {
            if bytes.get(cursor + 1) == Some(&b'=') {
                return None;
            }
            return Some((&text[..cursor], &text[cursor + 1..]));
        }
        cursor += 1;
    }
    None
}

fn split_order_only(text: &str) -> (&str, Option<&str>) {
    let bytes = text.as_bytes();
    let mut make_depth = 0usize;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            make_depth += 1;
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b')' && make_depth > 0 {
            make_depth -= 1;
        } else if bytes[cursor] == b'|' && make_depth == 0 {
            return (&text[..cursor], Some(&text[cursor + 1..]));
        }
        cursor += 1;
    }
    (text, None)
}

fn split_unquoted_semicolon(text: &str) -> (&str, Option<&str>) {
    let bytes = text.as_bytes();
    let mut make_depth = 0usize;
    let mut quote = None;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let byte = bytes[cursor];
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
        } else if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
        } else if byte == b'$' && bytes.get(cursor + 1) == Some(&b'(') {
            make_depth += 1;
            cursor += 2;
            continue;
        } else if byte == b')' && make_depth > 0 {
            make_depth -= 1;
        } else if byte == b';' && make_depth == 0 {
            return (&text[..cursor], Some(&text[cursor + 1..]));
        }
        cursor += 1;
    }
    (text, None)
}

pub(super) fn line_state(states: &[ConditionalTruth], line: usize) -> ConditionalTruth {
    states
        .get(line)
        .copied()
        .unwrap_or(ConditionalTruth::Unknown)
}

pub(super) fn safe_owner_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

pub(super) fn is_build_output(path: &str, extension: &str) -> bool {
    path.strip_prefix(&format!("{BUILD_ALIAS}/"))
        .is_some_and(|tail| valid_relative_tail(tail) && tail.ends_with(extension))
}

pub(super) fn is_generated_output(path: &str, extension: &str) -> bool {
    path.strip_prefix(&format!("{BUILD_ALIAS}/gen/"))
        .is_some_and(|tail| valid_relative_tail(tail) && tail.ends_with(extension))
}

pub(super) fn is_build_directory(path: &str) -> bool {
    path.strip_prefix(&format!("{BUILD_ALIAS}/"))
        .is_some_and(valid_relative_tail)
}

pub(super) fn is_build_path(path: &str) -> bool {
    path == BUILD_ALIAS || is_build_directory(path)
}

pub(super) fn valid_relative_tail(tail: &str) -> bool {
    !tail.is_empty()
        && !tail
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        && Path::new(tail)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && !tail.contains('\\')
}

pub(super) fn source_relative_path(path: &str) -> Option<String> {
    let tail = path.strip_prefix(&format!("{SOURCE_ALIAS}/"))?;
    if valid_relative_tail(tail) {
        Some(tail.to_owned())
    } else {
        None
    }
}

pub(super) fn has_extension(path: &str, extension: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
}
