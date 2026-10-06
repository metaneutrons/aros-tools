//! Recipe parsing for SDK text rules.

use super::{safe_cmake_path, LogicalLine, Rule, SdkTextOperation, SDK_TEXT_OUTPUT_PATH_ROOTS};
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use crate::make_vars::{ConditionalTruth, VarScope};
use std::path::Path;

pub(super) fn parse_sdk_text_recipe(
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
