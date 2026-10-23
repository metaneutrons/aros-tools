//! Build a bounded GNU Make wrapper that reads configure-time Mesa values.
//!
//! Only the generated Makefile prefix through its one config include is kept.
//! The ordinary Makefile rules after that boundary are never copied into the
//! wrapper, and the sole requested target has no recipe or prerequisites.

use std::collections::{BTreeMap, BTreeSet};

use crate::ContractError;

const MAX_GENERATED_MAKEFILE_BYTES: usize = 1024 * 1024;
const MAX_WRAPPER_BYTES: usize = 64 * 1024;
const CONFIG_INCLUDE_LINE: &str = "include $(TOP)/config/make.cfg\n";
const PREFLIGHT_TARGET_DECLARATION: &str =
    ".PHONY: aros-mesa-preflight-values\naros-mesa-preflight-values:\n";

/// Construct a restricted wrapper ending at the generated config include.
///
/// The prefix may contain comments, simple variable assignments, the Make
/// version conditionals, and the generated `default: all` rule. Other rules,
/// recipes, includes, shell assignments, and parse-time output/evaluation are
/// rejected. The included config is retained because it supplies the values
/// being measured; no ordinary Makefile target is copied after it.
pub(super) fn wrapper(makefile: &str, keys: &[&str]) -> Result<String, ContractError> {
    let prefix = configuration_prefix(makefile)?;

    let mut output_size = prefix.len();
    if output_size > MAX_WRAPPER_BYTES {
        return Err(error("Mesa preflight wrapper exceeds its byte bound"));
    }
    for key in keys {
        validate_key(key)?;
        // `$(info ` + KEY + `=$(strip $(` + KEY + `)))\n`
        let info_line_size = key
            .len()
            .checked_mul(2)
            .and_then(|length| length.checked_add(22))
            .ok_or_else(|| error("Mesa preflight wrapper size overflowed"))?;
        output_size = output_size
            .checked_add(info_line_size)
            .ok_or_else(|| error("Mesa preflight wrapper size overflowed"))?;
        if output_size > MAX_WRAPPER_BYTES {
            return Err(error("Mesa preflight wrapper exceeds its byte bound"));
        }
    }
    validate_keys_unique(keys)?;
    output_size = output_size
        .checked_add(PREFLIGHT_TARGET_DECLARATION.len())
        .ok_or_else(|| error("Mesa preflight wrapper size overflowed"))?;
    if output_size > MAX_WRAPPER_BYTES {
        return Err(error("Mesa preflight wrapper exceeds its byte bound"));
    }

    let mut wrapper = String::with_capacity(output_size);
    wrapper.push_str(prefix);
    for key in keys {
        wrapper.push_str("$(info ");
        wrapper.push_str(key);
        wrapper.push_str("=$(strip $(");
        wrapper.push_str(key);
        wrapper.push_str(")))\n");
    }
    wrapper.push_str(PREFLIGHT_TARGET_DECLARATION);
    debug_assert_eq!(wrapper.len(), output_size);
    Ok(wrapper)
}

/// Validate and borrow the generated configuration prefix without a query target.
pub(super) fn configuration_prefix(makefile: &str) -> Result<&str, ContractError> {
    if makefile.len() > MAX_GENERATED_MAKEFILE_BYTES {
        return Err(error("generated Mesa Makefile exceeds its byte bound"));
    }

    let prefix_end = unique_include_boundary(makefile)?;
    let prefix = &makefile[..prefix_end];
    validate_prefix(&prefix[..prefix.len() - CONFIG_INCLUDE_LINE.len()])?;

    if prefix.len() > MAX_WRAPPER_BYTES {
        return Err(error("Mesa preflight wrapper exceeds its byte bound"));
    }
    Ok(prefix)
}

/// Parse exactly one `KEY=value` record for each requested key.
pub(super) fn parse_values(
    bytes: &[u8],
    keys: &[&str],
) -> Result<BTreeMap<String, String>, ContractError> {
    let output_limit =
        if keys.contains(&aros_common::make_initial_values::MAKE_INITIAL_VALUES_METADATA_KEY) {
            // The separately bounded namespace (64 KiB) and raw-value packet
            // (128 KiB) join ordinary measurements. Legacy probes retain 64 KiB.
            256 * 1024
        } else {
            MAX_WRAPPER_BYTES
        };
    if bytes.len() > output_limit {
        return Err(error("Mesa preflight output exceeds its byte bound"));
    }
    validate_keys_unique(keys)?;

    let output =
        std::str::from_utf8(bytes).map_err(|_| error("Mesa preflight output is not UTF-8"))?;
    if output.chars().any(|character| {
        (character.is_control() && character != '\n')
            || matches!(character, '\u{2028}' | '\u{2029}')
    }) {
        return Err(error("Mesa preflight output contains a control character"));
    }

    let expected: BTreeSet<&str> = keys.iter().copied().collect();
    let mut values = BTreeMap::new();
    for line in output.split_terminator('\n') {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| error("Mesa preflight output contains a malformed value line"))?;
        if !expected.contains(key) {
            return Err(error("Mesa preflight output contains an unexpected key"));
        }
        if values.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(error("Mesa preflight output contains a duplicate key"));
        }
    }
    if values.len() != expected.len() {
        return Err(error("Mesa preflight output is missing a requested key"));
    }
    Ok(values)
}

fn unique_include_boundary(makefile: &str) -> Result<usize, ContractError> {
    let mut boundary = None;
    for (start, line) in makefile.split_inclusive('\n').scan(0usize, |offset, line| {
        let start = *offset;
        *offset += line.len();
        Some((start, line))
    }) {
        if line == CONFIG_INCLUDE_LINE && boundary.replace(start + line.len()).is_some() {
            return Err(error(
                "generated Mesa Makefile has duplicate config include boundaries",
            ));
        }
    }
    boundary.ok_or_else(|| error("generated Mesa Makefile has no exact config include boundary"))
}

fn validate_prefix(prefix: &str) -> Result<(), ContractError> {
    if prefix
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err(error(
            "generated Mesa Makefile prefix contains a control character",
        ));
    }

    let mut conditionals = Vec::new();
    let mut saw_default_target = false;
    for line in prefix.split_terminator('\n') {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(error(
                "generated Mesa Makefile prefix contains an indented recipe or directive",
            ));
        }
        if trimmed.ends_with('\\') {
            return Err(error(
                "generated Mesa Makefile prefix contains a continued line",
            ));
        }
        if has_forbidden_make_function(trimmed) {
            return Err(error(
                "generated Mesa Makefile prefix contains an unsafe Make function",
            ));
        }

        match trimmed {
            "default: all" => {
                if std::mem::replace(&mut saw_default_target, true) {
                    return Err(error(
                        "generated Mesa Makefile prefix repeats its default target",
                    ));
                }
                continue;
            }
            "endif" => {
                if conditionals.pop().is_none() {
                    return Err(error(
                        "generated Mesa Makefile prefix has an unmatched endif",
                    ));
                }
                continue;
            }
            "else" => {
                let Some(saw_else) = conditionals.last_mut() else {
                    return Err(error(
                        "generated Mesa Makefile prefix has an unmatched else",
                    ));
                };
                if *saw_else {
                    return Err(error("generated Mesa Makefile prefix has a duplicate else"));
                }
                *saw_else = true;
                continue;
            }
            _ => {}
        }

        if trimmed.starts_with("include ")
            || trimmed.starts_with("-include ")
            || trimmed.starts_with("sinclude ")
        {
            return Err(error(
                "generated Mesa Makefile prefix contains another include",
            ));
        }
        if let Some(variable) = trimmed
            .strip_prefix("ifdef ")
            .or_else(|| trimmed.strip_prefix("ifndef "))
        {
            if !valid_make_variable_name(variable.trim()) {
                return Err(error(
                    "generated Mesa Makefile prefix has an invalid conditional",
                ));
            }
            conditionals.push(false);
            continue;
        }
        if trimmed.starts_with("ifeq ") || trimmed.starts_with("ifneq ") {
            let expression = trimmed
                .split_once(' ')
                .map_or("", |(_, expression)| expression.trim());
            if expression.is_empty() {
                return Err(error(
                    "generated Mesa Makefile prefix has an invalid conditional",
                ));
            }
            conditionals.push(false);
            continue;
        }
        if let Some(message) = trimmed
            .strip_prefix("$(error ")
            .and_then(|message| message.strip_suffix(')'))
        {
            let message = message.replace("$(MAKE_VERSION)", "");
            if message.contains('$') || message.contains('\\') {
                return Err(error(
                    "generated Mesa Makefile prefix has an unsafe error expression",
                ));
            }
            continue;
        }

        if is_assignment(trimmed) {
            continue;
        }
        return Err(error(
            "generated Mesa Makefile prefix contains an unsupported rule or directive",
        ));
    }
    if !conditionals.is_empty() {
        return Err(error(
            "generated Mesa Makefile prefix has an unterminated conditional",
        ));
    }
    if !saw_default_target {
        return Err(error(
            "generated Mesa Makefile prefix has no known default target",
        ));
    }
    Ok(())
}

fn is_assignment(line: &str) -> bool {
    let line = line.strip_prefix("export ").unwrap_or(line);
    let Some(equal) = line.find('=') else {
        return false;
    };
    let operator_start = match line.as_bytes().get(equal.wrapping_sub(1)) {
        Some(b':' | b'?' | b'+') => equal - 1,
        Some(b'!') => return false,
        _ => equal,
    };
    let variable = line[..operator_start].trim_end();
    valid_make_variable_name(variable)
}

fn valid_make_variable_name(variable: &str) -> bool {
    !variable.is_empty()
        && variable
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn has_forbidden_make_function(line: &str) -> bool {
    const FORBIDDEN: [&str; 8] = [
        "shell", "eval", "file", "guile", "call", "foreach", "info", "warning",
    ];

    for opener in ["$(", "${"] {
        let mut rest = line;
        while let Some(index) = rest.find(opener) {
            rest = &rest[index + opener.len()..];
            let name = rest
                .bytes()
                .take_while(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
                .count();
            let function = &rest[..name];
            if FORBIDDEN.contains(&function) {
                return true;
            }
        }
    }
    false
}

pub(super) fn validate_keys_unique(keys: &[&str]) -> Result<(), ContractError> {
    if keys.is_empty() {
        return Err(error("Mesa preflight key set is empty"));
    }
    let mut seen = BTreeSet::new();
    for key in keys {
        validate_key(key)?;
        if !seen.insert(*key) {
            return Err(error("Mesa preflight key set contains a duplicate"));
        }
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), ContractError> {
    let bytes = key.as_bytes();
    if bytes.is_empty()
        || !(bytes[0].is_ascii_uppercase() || bytes[0] == b'_')
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
    {
        return Err(error(
            "Mesa preflight key is not a closed uppercase identifier",
        ));
    }
    Ok(())
}

fn error(message: &'static str) -> ContractError {
    ContractError::compatibility(message)
}

#[cfg(test)]
mod tests {
    use super::{parse_values, wrapper, MAX_GENERATED_MAKEFILE_BYTES, MAX_WRAPPER_BYTES};

    const MINIMAL_FIXED_FIXTURE: &str = "# Main makefile for AROS\n\
ifndef MAKE_VERSION\n\
$(error GNU Make 4.0 or newer is required)\n\
endif\n\
ifneq ($(filter 0 1 2 3,$(word 1,$(subst ., ,$(MAKE_VERSION)))),)\n\
$(error GNU Make 4.0 or newer is required; found $(MAKE_VERSION))\n\
endif\n\
default: all\n\
TOP := /build\n\
SRCDIR := /source\n\
include $(TOP)/config/make.cfg\n";

    #[test]
    fn wraps_the_minimal_fixed_makefile_prefix() {
        let result = wrapper(MINIMAL_FIXED_FIXTURE, &["AROS_TARGET_ARCH", "TOP"])
            .expect("the fixed generated prefix is safe");
        assert!(result.starts_with(MINIMAL_FIXED_FIXTURE));
        assert!(result.contains("$(info AROS_TARGET_ARCH=$(strip $(AROS_TARGET_ARCH)))\n"));
        assert!(result.contains("$(info TOP=$(strip $(TOP)))\n"));
        assert!(
            result.ends_with(".PHONY: aros-mesa-preflight-values\naros-mesa-preflight-values:\n")
        );
        assert!(!result.contains("$(MAKE)"));
    }

    #[test]
    fn rejects_missing_and_duplicate_boundaries() {
        assert!(wrapper("TOP := /build\n", &["TOP"]).is_err());
        let duplicate = format!("{MINIMAL_FIXED_FIXTURE}include $(TOP)/config/make.cfg\n");
        assert!(wrapper(&duplicate, &["TOP"]).is_err());
    }

    #[test]
    fn rejects_other_includes_and_recipes() {
        let other_include =
            MINIMAL_FIXED_FIXTURE.replace("TOP := /build\n", "include extra.mk\nTOP := /build\n");
        assert!(wrapper(&other_include, &["TOP"]).is_err());

        let recipe =
            MINIMAL_FIXED_FIXTURE.replace("default: all\n", "default: all\n\t@echo unsafe\n");
        assert!(wrapper(&recipe, &["TOP"]).is_err());
    }

    #[test]
    fn rejects_extra_targets_and_parse_time_commands() {
        let target =
            MINIMAL_FIXED_FIXTURE.replace("TOP := /build\n", "extra: all\nTOP := /build\n");
        assert!(wrapper(&target, &["TOP"]).is_err());

        let shell = MINIMAL_FIXED_FIXTURE.replace(
            "TOP := /build\n",
            "VALUE := $(shell whoami)\nTOP := /build\n",
        );
        assert!(wrapper(&shell, &["TOP"]).is_err());
    }

    #[test]
    fn parses_only_requested_values_and_allows_empty_values() {
        let values = parse_values(b"TOP=/build\nEMPTY=\n", &["TOP", "EMPTY"])
            .expect("both requested records are present");
        assert_eq!(values.get("TOP").map(String::as_str), Some("/build"));
        assert_eq!(values.get("EMPTY").map(String::as_str), Some(""));
    }

    #[test]
    fn rejects_unexpected_duplicate_and_missing_values() {
        assert!(parse_values(b"TOP=/build\nEXTRA=x\n", &["TOP"]).is_err());
        assert!(parse_values(b"TOP=/build\nTOP=/other\n", &["TOP"]).is_err());
        assert!(parse_values(b"TOP=/build\n", &["TOP", "SRCDIR"]).is_err());
    }

    #[test]
    fn rejects_bad_identifiers_non_utf8_and_oversize_inputs() {
        assert!(wrapper(MINIMAL_FIXED_FIXTURE, &["top"]).is_err());
        assert!(wrapper(MINIMAL_FIXED_FIXTURE, &["TOP", "TOP"]).is_err());
        assert!(parse_values(b"TOP=\xff\n", &["TOP"]).is_err());
        assert!(parse_values(&vec![b'x'; MAX_WRAPPER_BYTES + 1], &["TOP"]).is_err());
        assert!(wrapper(&"x".repeat(MAX_GENERATED_MAKEFILE_BYTES + 1), &["TOP"]).is_err());
    }
}
