//! Small, syntax-only parser for one continuation-joined Make rule header.
//!
//! This module deliberately preserves Make expressions verbatim. It does not
//! evaluate variables, read files, or infer what a dynamic target expands to.

use crate::make_expr::top_level_byte;
use crate::make_vars::{
    directive_tail, make_define_suppression, strip_make_comment, variable_assignment,
};

const MAX_RULE_HEADER_BYTES: usize = 1024 * 1024;

/// GNU Make's special dot/uppercase targets can alter parse/execution policy.
/// A data-only reader must model their effects explicitly or refuse them.
pub(crate) fn is_control_target(word: &str) -> bool {
    word.strip_prefix('.').is_some_and(|tail| {
        !tail.is_empty()
            && tail
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleHeader {
    Rule {
        targets: String,
        target_pattern: Option<String>,
        prerequisites: String,
        order_only: String,
        double_colon: bool,
        inline_recipe: Option<String>,
    },
    TargetAssignment {
        targets: String,
        assignment: String,
    },
}

/// Select only a source rule's unexpanded target prefix. The tail is deliberately
/// not parsed: GNU Make discards it before expansion when no targets remain.
/// The strict standalone `parse` API still requires a nonempty literal prefix.
pub(crate) fn source_target_prefix(raw: &str) -> Result<Option<&str>, String> {
    if raw.len() > MAX_RULE_HEADER_BYTES || raw.contains(['\n', '\r', '\0']) {
        return Err("Make source header byte/line limit".into());
    }
    if raw.starts_with('\t') {
        return Ok(None);
    }
    let visible = strip_make_comment(raw).trim_start_matches([' ', '\t']);
    if is_inert_make_line(visible) {
        return Ok(None);
    }
    let Some(colon) = top_level_byte(visible, b':') else {
        return Ok(None);
    };
    // A semicolon before the separator cannot introduce a rule header.
    if top_level_byte(visible, b';').is_some_and(|at| at < colon) {
        return Err("Make source target prefix contains a recipe separator".into());
    }
    let targets = visible[..colon].trim_matches([' ', '\t']);
    validate_reference_boundaries(targets)?;
    if targets.contains('\\') || targets.ends_with('&') {
        return Err("escaped/grouped Make source targets are unsupported".into());
    }
    if targets
        .chars()
        .any(|c| c.is_whitespace() && !matches!(c, ' ' | '\t'))
    {
        return Err("non-ASCII whitespace in Make source targets is unsupported".into());
    }
    Ok(Some(targets))
}

/// Parse one continuation-joined, non-recipe Make logical line.
///
/// `Ok(None)` means the line is a comment, a directive/variable assignment,
/// or not a rule header. Unsupported rule syntax is returned as an error so a
/// caller cannot accidentally reinterpret it as an ordinary prerequisite.
///
/// # Errors
/// Rejects oversized, malformed or unsupported rule syntax and recipe prefixes.
pub fn parse(raw: &str) -> Result<Option<RuleHeader>, String> {
    if raw.len() > MAX_RULE_HEADER_BYTES {
        return Err("Make rule header exceeds the 1 MiB byte limit".into());
    }
    if raw.contains(['\n', '\r']) {
        return Err("Make rule header must be one continuation-joined logical line".into());
    }
    if raw.starts_with('\t') {
        return Ok(None);
    }

    // After a rule's semicolon the shell owns comment/quote semantics. Do
    // not truncate an inline recipe at '#', even inside a quoted argument.
    let commentless = strip_make_comment(raw);
    let first_semicolon =
        top_level_byte(raw, b';').filter(|semicolon| *semicolon < commentless.len());
    let make_part = first_semicolon.map_or(commentless, |semicolon| &raw[..semicolon]);
    let inline_recipe = first_semicolon.map(|semicolon| raw[semicolon + 1..].trim().to_owned());
    let visible = make_part.trim();
    if visible.is_empty() {
        return Ok(None);
    }
    if has_recipe_prefix_assignment(visible) {
        return Err("custom .RECIPEPREFIX makes Make rule boundaries unprovable".into());
    }
    if is_inert_make_line(visible) {
        return Ok(None);
    }

    if top_level_byte(visible, b':').is_none() {
        return Ok(None);
    }
    let header = visible;
    validate_reference_boundaries(header)?;
    if header.contains('\\') {
        return Err("raw Make rule-header escapes are unsupported".into());
    }

    // Re-run the shared scanner on the recipe-free header. A separator seen
    // only in a later recipe must never affect the parsed fields.
    let colon = top_level_byte(header, b':')
        .ok_or_else(|| "malformed Make rule header separator".to_owned())?;
    let targets = header[..colon].trim();
    if targets.is_empty() {
        return Err("Make rule header has no targets".into());
    }
    if targets.ends_with('&') {
        return Err("grouped-target Make rules are unsupported".into());
    }

    let tail = header[colon + 1..].trim();
    if let Some(assignment) = parse_target_assignment(tail)? {
        if inline_recipe.is_some() {
            return Err("inline recipes on target-specific assignments are unsupported".into());
        }
        return Ok(Some(RuleHeader::TargetAssignment {
            targets: targets.to_owned(),
            assignment,
        }));
    }

    let double_colon = header.as_bytes().get(colon + 1) == Some(&b':');
    let rule_tail = if double_colon {
        &header[colon + 2..]
    } else {
        &header[colon + 1..]
    };
    let pattern_colon = top_level_byte(rule_tail, b':');
    if double_colon && pattern_colon.is_some() {
        return Err("double-colon static-pattern rules are unsupported".into());
    }

    let (target_pattern, prerequisite_text) = if let Some(pattern_colon) = pattern_colon {
        let pattern = rule_tail[..pattern_colon].trim();
        if pattern.is_empty() {
            return Err("static-pattern rule has an empty target pattern".into());
        }
        if has_top_level_whitespace(pattern) {
            return Err("static-pattern rule must have one target-pattern word".into());
        }
        if top_level_byte(pattern, b'|').is_some() {
            return Err("static-pattern target pattern contains an order-only separator".into());
        }
        // A pattern may obtain its percent from a deferred Make reference.
        // Validate the expanded pattern when the caller instantiates it.
        (Some(pattern.to_owned()), &rule_tail[pattern_colon + 1..])
    } else {
        (None, rule_tail)
    };

    let (prerequisites, order_only) = split_order_only(prerequisite_text)?;
    Ok(Some(RuleHeader::Rule {
        targets: targets.to_owned(),
        target_pattern,
        prerequisites,
        order_only,
        double_colon,
        inline_recipe,
    }))
}

fn is_inert_make_line(line: &str) -> bool {
    if variable_assignment(line).is_some()
        || make_define_suppression(line)
            .first()
            .copied()
            .unwrap_or(false)
    {
        return true;
    }

    [
        "include", "-include", "sinclude", "undefine", "export", "unexport", "override", "private",
        "vpath", "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "load", "-load",
    ]
    .iter()
    .any(|directive| directive_tail(line, directive).is_some())
}

pub(crate) fn has_recipe_prefix_assignment(line: &str) -> bool {
    let (_, statement, _) = strip_assignment_modifiers(line);
    assignment_name(statement).is_some_and(|name| name == ".RECIPEPREFIX")
}

fn strip_assignment_modifiers(mut line: &str) -> (bool, &str, bool) {
    let mut modified = false;
    for _ in 0..3 {
        let trimmed = line.trim_start();
        let Some(end) = trimmed.find(char::is_whitespace) else {
            return (modified, trimmed, false);
        };
        let word = &trimmed[..end];
        if !matches!(word, "override" | "export" | "private") {
            return (modified, trimmed, true);
        }
        modified = true;
        line = trimmed[end..].trim_start();
    }
    (modified, line.trim_start(), true)
}

fn assignment_name(line: &str) -> Option<&str> {
    let equal = top_level_byte(line, b'=')?;
    let mut left = line[..equal].trim_end();
    if left.ends_with("::") {
        left = left[..left.len() - 2].trim_end();
    } else if left.ends_with([':', '+', '?']) {
        left = left[..left.len() - 1].trim_end();
    }
    let name = left.trim();
    (!name.is_empty()).then_some(name)
}

fn has_top_level_whitespace(raw: &str) -> bool {
    b" \t"
        .iter()
        .any(|whitespace| top_level_byte(raw, *whitespace).is_some())
}

fn parse_target_assignment(tail: &str) -> Result<Option<String>, String> {
    if variable_assignment(tail).is_some() {
        return Ok(Some(tail.trim().to_owned()));
    }

    let (modified, statement, well_formed_modifiers) = strip_assignment_modifiers(tail);
    if !well_formed_modifiers {
        return Ok(None);
    }
    if variable_assignment(statement).is_some() {
        return Err("target-specific assignment modifiers are unsupported".into());
    }

    let Some(name) = assignment_name(statement) else {
        return Ok(None);
    };
    let assignment_like_name = name.contains('$')
        || (!name
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b':')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)));
    if modified || assignment_like_name {
        return Err("unsupported target-specific assignment syntax".into());
    }
    Ok(None)
}

fn validate_reference_boundaries(header: &str) -> Result<(), String> {
    if header.contains('\0') {
        return Err("NUL byte in Make rule header".into());
    }
    let mut probe = String::with_capacity(header.len() + 1);
    probe.push_str(header);
    probe.push('\0');
    if top_level_byte(&probe, b'\0') != Some(header.len()) {
        return Err("malformed Make reference in rule header".into());
    }
    Ok(())
}

fn split_order_only(raw: &str) -> Result<(String, String), String> {
    let Some(separator) = top_level_byte(raw, b'|') else {
        return Ok((raw.trim().to_owned(), String::new()));
    };
    let prerequisites = raw[..separator].trim();
    let order_only = raw[separator + 1..].trim();
    if top_level_byte(order_only, b'|').is_some() {
        return Err("Make rule has more than one order-only separator".into());
    }
    if order_only.is_empty() {
        return Err("Make rule has an empty order-only prerequisite list".into());
    }
    Ok((prerequisites.to_owned(), order_only.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{parse, RuleHeader};

    #[test]
    fn parses_ordinary_rule_and_preserves_order_and_duplicates() {
        assert_eq!(
            parse("  app: src/a.c src/a.c | stamp stamp  ").unwrap(),
            Some(RuleHeader::Rule {
                targets: "app".into(),
                target_pattern: None,
                prerequisites: "src/a.c src/a.c".into(),
                order_only: "stamp stamp".into(),
                double_colon: false,
                inline_recipe: None,
            })
        );
    }

    #[test]
    fn parses_static_pattern_with_deferred_target_pattern() {
        assert_eq!(
            parse("objects: $(owner_PATTERN): $(SOURCES:.c=.o)").unwrap(),
            Some(RuleHeader::Rule {
                targets: "objects".into(),
                target_pattern: Some("$(owner_PATTERN)".into()),
                prerequisites: "$(SOURCES:.c=.o)".into(),
                order_only: String::new(),
                double_colon: false,
                inline_recipe: None,
            })
        );
    }

    #[test]
    fn parses_double_colon_rule() {
        assert_eq!(
            parse("archive:: member.o").unwrap(),
            Some(RuleHeader::Rule {
                targets: "archive".into(),
                target_pattern: None,
                prerequisites: "member.o".into(),
                order_only: String::new(),
                double_colon: true,
                inline_recipe: None,
            })
        );
    }

    #[test]
    fn recognizes_target_specific_assignment_before_static_pattern_colon() {
        assert_eq!(
            parse("object.o: CFLAGS += -g").unwrap(),
            Some(RuleHeader::TargetAssignment {
                targets: "object.o".into(),
                assignment: "CFLAGS += -g".into(),
            })
        );
    }

    #[test]
    fn inline_recipe_does_not_change_header_field_splitting() {
        assert_eq!(
            parse("app: main.c | stamp ; cc -DVALUE='x:y|z' main.c -o app").unwrap(),
            Some(RuleHeader::Rule {
                targets: "app".into(),
                target_pattern: None,
                prerequisites: "main.c".into(),
                order_only: "stamp".into(),
                double_colon: false,
                inline_recipe: Some("cc -DVALUE='x:y|z' main.c -o app".into()),
            })
        );
    }

    #[test]
    fn inline_shell_comments_are_preserved_but_header_comments_stay_make_comments() {
        let parsed = parse("target: input ; printf '%s' '#literal' # shell comment")
            .unwrap()
            .unwrap();
        assert!(
            matches!(parsed, RuleHeader::Rule { inline_recipe: Some(text), .. } if text == "printf '%s' '#literal' # shell comment")
        );
        let parsed = parse("target: input # Make comment; not a recipe")
            .unwrap()
            .unwrap();
        assert!(
            matches!(parsed, RuleHeader::Rule { prerequisites, inline_recipe: None, .. } if prerequisites == "input")
        );
    }

    #[test]
    fn nested_substitution_colon_is_not_a_rule_or_order_separator() {
        assert_eq!(
            parse("objects: $(SOURCES:.c=.o) $(addprefix dir:,file.o) | generated").unwrap(),
            Some(RuleHeader::Rule {
                targets: "objects".into(),
                target_pattern: None,
                prerequisites: "$(SOURCES:.c=.o) $(addprefix dir:,file.o)".into(),
                order_only: "generated".into(),
                double_colon: false,
                inline_recipe: None,
            })
        );
    }

    #[test]
    fn skips_comments_global_assignments_and_known_controls() {
        for line in [
            " # comment: ignored",
            "MODE := debug",
            "ifeq ($(MODE),debug)",
            "include local.mk",
            "export MODE",
            "define RULES",
        ] {
            assert_eq!(parse(line).unwrap(), None, "line: {line}");
        }
    }

    #[test]
    fn rejects_grouped_targets_double_static_patterns_and_extra_order_separators() {
        for line in [
            "one two &: input",
            "objects:: %.o: %.c",
            "app: input | order | extra",
        ] {
            assert!(parse(line).is_err(), "line: {line}");
        }
    }

    #[test]
    fn rejects_escapes_and_malformed_static_patterns() {
        for line in ["bad\\ name: input", "objects: : input"] {
            assert!(parse(line).is_err(), "line: {line}");
        }
    }

    #[test]
    fn rejects_computed_or_modified_target_assignments() {
        for line in [
            "object.o: $(FLAGS) := -g",
            "object.o: override CFLAGS := -g",
        ] {
            assert!(parse(line).is_err(), "line: {line}");
        }
    }

    #[test]
    fn rejects_custom_recipe_prefix_and_malformed_references() {
        assert!(parse(".RECIPEPREFIX := >").is_err());
        assert!(parse("target: $(BROKEN").is_err());
    }
}
