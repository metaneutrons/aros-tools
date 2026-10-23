//! A bounded, data-only parser for a deliberately small `env` prefix form.
//!
//! This module never invokes a shell or `env`, and its result grants no
//! execution authority. It accepts a strict subset of shell word syntax for
//! extracting literal assignment data. Refusing syntax here is not a claim of
//! POSIX shell compatibility.

use std::collections::BTreeMap;

const CONFIGURATION_FILE: &str = "Literal make environment prefix";
const MAX_PREFIX_BYTES: usize = 128 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024;
const MAX_ASSIGNMENTS: usize = 64;
const MAX_NAME_BYTES: usize = 128;
const MAX_VALUE_BYTES: usize = 4096;

/// A validated, deterministic set of literal environment assignment data.
///
/// The values are inert strings. This type does not execute them, interpret
/// them as shell code, or establish authority to run a command.
#[derive(Clone, Eq, PartialEq)]
pub struct LiteralMakeEnvironmentPrefix {
    assignments: BTreeMap<String, String>,
}

impl std::fmt::Debug for LiteralMakeEnvironmentPrefix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiteralMakeEnvironmentPrefix")
            .field("assignment_count", &self.assignments.len())
            .finish_non_exhaustive()
    }
}

impl LiteralMakeEnvironmentPrefix {
    /// Parse exactly `env` followed by one or more literal `NAME=value` words.
    ///
    /// Unquoted shell operators and expansions are refused. Single quotes,
    /// double quotes, and backslash quoting support literal word construction
    /// within the limits of this parser's intentionally narrow grammar.
    ///
    /// # Errors
    /// Returns a sanitized configuration error for malformed input, duplicate
    /// names, unsupported shell syntax, or any configured size limit. Error
    /// messages never include the supplied prefix or assignment values.
    pub fn parse(prefix: &str) -> crate::Result<Self> {
        let words = tokenize_literal_command(prefix, MAX_PREFIX_BYTES, MAX_ASSIGNMENTS + 1)?;
        if words.first().map(String::as_str) != Some("env") {
            return Err(prefix_error("command must be env"));
        }
        if words.len() < 2 {
            return Err(prefix_error("at least one assignment is required"));
        }
        if words.len() - 1 > MAX_ASSIGNMENTS {
            return Err(prefix_error("assignment count exceeds its limit"));
        }

        let mut assignments = BTreeMap::new();
        let mut total_bytes = 0usize;
        for word in words.iter().skip(1) {
            let (name, value) = word
                .split_once('=')
                .ok_or_else(|| prefix_error("each argument must be an assignment"))?;
            if !is_ascii_scalar_name(name) {
                return Err(prefix_error(
                    "assignment name is not a valid ASCII identifier",
                ));
            }
            if name.len() > MAX_NAME_BYTES {
                return Err(prefix_error("assignment name exceeds its byte limit"));
            }
            if value.len() > MAX_VALUE_BYTES {
                return Err(prefix_error("assignment value exceeds its byte limit"));
            }
            if value.chars().any(char::is_control) {
                return Err(prefix_error(
                    "assignment value contains a control character",
                ));
            }

            total_bytes = total_bytes
                .saturating_add(name.len())
                .saturating_add(1)
                .saturating_add(value.len());
            if total_bytes > MAX_TOTAL_BYTES {
                return Err(prefix_error(
                    "combined assignment data exceeds its byte limit",
                ));
            }
            if assignments
                .insert(name.to_owned(), value.to_owned())
                .is_some()
            {
                return Err(prefix_error("duplicate assignment name"));
            }
        }

        Ok(Self { assignments })
    }

    /// Return assignments in deterministic, name-sorted order.
    #[must_use]
    pub const fn assignments(&self) -> &BTreeMap<String, String> {
        &self.assignments
    }
}

/// Tokenize a literal shell command without evaluating it.
///
/// This accepts only the deliberately narrow POSIX-word subset used by the
/// make environment prefix parser. `max_bytes` bounds the input before any
/// token allocation, and `max_words` is checked before a new word is built.
/// Errors are sanitized and never include command contents.
pub(crate) fn tokenize_literal_command(
    command: &str,
    max_bytes: usize,
    max_words: usize,
) -> crate::Result<Vec<String>> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Quote {
        Unquoted,
        Single,
        Double,
    }

    if command.len() > max_bytes {
        return Err(literal_command_error(
            "literal command exceeds its byte limit",
        ));
    }

    let mut words = Vec::new();
    let mut word = String::new();
    let mut word_started = false;
    let mut quote = Quote::Unquoted;
    let mut chars = command.chars();

    while let Some(ch) = chars.next() {
        match quote {
            Quote::Unquoted => match ch {
                ' ' | '\t' => {
                    if word_started {
                        push_word(&mut words, &mut word, max_words)?;
                        word_started = false;
                    }
                }
                '\'' => {
                    quote = Quote::Single;
                    begin_word(&words, &mut word_started, max_words)?;
                }
                '"' => {
                    quote = Quote::Double;
                    begin_word(&words, &mut word_started, max_words)?;
                }
                '\\' => {
                    begin_word(&words, &mut word_started, max_words)?;
                    let escaped = chars.next().ok_or_else(|| {
                        literal_command_error("literal command ends with an incomplete escape")
                    })?;
                    if escaped.is_control() {
                        return Err(literal_command_error(
                            "escaped control characters are refused",
                        ));
                    }
                    word.push(escaped);
                }
                '$' | '`' => {
                    return Err(literal_command_error("shell expansion syntax is refused"));
                }
                ';' | '|' | '&' | '<' | '>' | '(' | ')' | '*' | '?' | '[' | ']' | '{' | '}'
                | '~' | '#' => {
                    return Err(literal_command_error("unsupported shell syntax is refused"));
                }
                _ if ch.is_control() => {
                    return Err(literal_command_error("control characters are refused"));
                }
                _ => {
                    begin_word(&words, &mut word_started, max_words)?;
                    word.push(ch);
                }
            },
            Quote::Single => {
                if ch == '\'' {
                    quote = Quote::Unquoted;
                } else if ch == '\t' || ch.is_control() {
                    return Err(literal_command_error(
                        "control characters are refused inside quotes",
                    ));
                } else {
                    word.push(ch);
                }
            }
            Quote::Double => match ch {
                '"' => quote = Quote::Unquoted,
                '$' | '`' => {
                    return Err(literal_command_error(
                        "expansion syntax is refused inside double quotes",
                    ));
                }
                '\\' => {
                    let escaped = chars.next().ok_or_else(|| {
                        literal_command_error("literal command ends with an incomplete escape")
                    })?;
                    if escaped.is_control() {
                        return Err(literal_command_error(
                            "control characters are refused inside quotes",
                        ));
                    }
                    if !matches!(escaped, '"' | '\\' | '$' | '`') {
                        // POSIX double quotes preserve a backslash before
                        // ordinary characters; this subset models that rule.
                        word.push('\\');
                    }
                    word.push(escaped);
                }
                _ if ch == '\t' || ch.is_control() => {
                    return Err(literal_command_error(
                        "control characters are refused inside quotes",
                    ));
                }
                _ => word.push(ch),
            },
        }
    }

    if quote != Quote::Unquoted {
        return Err(literal_command_error(
            "literal command contains an unterminated quote",
        ));
    }
    if word_started {
        push_word(&mut words, &mut word, max_words)?;
    }
    Ok(words)
}

fn begin_word(words: &[String], word_started: &mut bool, max_words: usize) -> crate::Result<()> {
    if !*word_started {
        if words.len() >= max_words {
            return Err(literal_command_error("word count exceeds its limit"));
        }
        *word_started = true;
    }
    Ok(())
}

fn push_word(words: &mut Vec<String>, word: &mut String, max_words: usize) -> crate::Result<()> {
    if words.len() >= max_words {
        return Err(literal_command_error("word count exceeds its limit"));
    }
    words.push(std::mem::take(word));
    Ok(())
}

pub(crate) fn is_ascii_scalar_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() || first == b'_' => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn prefix_error(message: &'static str) -> crate::ArosError {
    crate::ArosError::Configuration {
        file: CONFIGURATION_FILE.to_owned(),
        message: message.to_owned(),
    }
}

fn literal_command_error(message: &'static str) -> crate::ArosError {
    crate::ArosError::Configuration {
        file: "Literal shell command".to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    #[test]
    fn generic_tokenizer_preserves_empty_quoted_and_multiple_words() {
        let words = tokenize_literal_command("tool '' 'two words' plain", 64, 4).unwrap();

        assert_eq!(
            words,
            vec![
                "tool".to_owned(),
                String::new(),
                "two words".to_owned(),
                "plain".to_owned(),
            ]
        );
    }

    #[test]
    fn generic_tokenizer_enforces_byte_and_word_limits() {
        assert_eq!(
            tokenize_literal_command("one two", 7, 2).unwrap(),
            vec!["one".to_owned(), "two".to_owned()]
        );
        assert!(tokenize_literal_command("one two three", 32, 2).is_err());
        assert!(tokenize_literal_command("one two", 6, 2).is_err());
    }

    #[test]
    fn generic_tokenizer_rejects_operators_without_echoing_input() {
        for command in [
            "tool private; command",
            "tool private | command",
            "tool >private",
        ] {
            let error = tokenize_literal_command(command, 64, 4).unwrap_err();

            assert!(!error.to_string().contains("private"));
        }
    }

    #[test]
    fn parses_literal_words_and_shell_quoting_as_data() {
        let parsed = LiteralMakeEnvironmentPrefix::parse(
            "  env FIRST=plain EMPTY= SINGLE='two words' MIXED=ab\" c\"\\ d DOUBLE=\"a\\\"b\\\\c\" DOLLAR='$HOME'  ",
        )
        .unwrap();

        assert_eq!(parsed.assignments()["FIRST"], "plain");
        assert_eq!(parsed.assignments()["EMPTY"], "");
        assert_eq!(parsed.assignments()["SINGLE"], "two words");
        assert_eq!(parsed.assignments()["MIXED"], "ab c d");
        assert_eq!(parsed.assignments()["DOUBLE"], "a\"b\\c");
        assert_eq!(parsed.assignments()["DOLLAR"], "$HOME");
        let escaped = LiteralMakeEnvironmentPrefix::parse(r#"env SAFE="\$HOME\`literal""#).unwrap();
        assert_eq!(escaped.assignments()["SAFE"], "$HOME`literal");
    }

    #[test]
    fn accepts_exactly_the_supported_assignment_bounds() {
        let mut prefix = String::from("env");
        for index in 0..MAX_ASSIGNMENTS {
            write!(prefix, " V{index}=x").unwrap();
        }
        let parsed = LiteralMakeEnvironmentPrefix::parse(&prefix).unwrap();
        assert_eq!(parsed.assignments().len(), MAX_ASSIGNMENTS);

        let value = "v".repeat(MAX_VALUE_BYTES);
        let parsed = LiteralMakeEnvironmentPrefix::parse(&format!("env V={value}")).unwrap();
        assert_eq!(parsed.assignments()["V"].len(), MAX_VALUE_BYTES);
    }

    #[test]
    fn rejects_duplicate_names_and_malformed_or_non_env_commands() {
        for prefix in [
            "env A=first A=second",
            "other A=value",
            "env -i A=value",
            "env A=value command",
            "env",
            "env A",
            "env 1INVALID=value",
        ] {
            assert!(
                LiteralMakeEnvironmentPrefix::parse(prefix).is_err(),
                "{prefix:?}"
            );
        }
    }

    #[test]
    fn rejects_expansion_and_unmodeled_shell_syntax() {
        for prefix in [
            "env A=$SECRET",
            "env A=`id`",
            "env A=\"$SECRET\"",
            "env A=\"`id`\"",
            "env A=x; command",
            "env A=x | command",
            "env A=$(id)",
            "env A=*",
            "env A=x >file",
        ] {
            assert!(
                LiteralMakeEnvironmentPrefix::parse(prefix).is_err(),
                "{prefix:?}"
            );
        }
    }

    #[test]
    fn enforces_bounds_and_returns_sanitized_errors() {
        let oversized_value = format!("env TOKEN={}", "secret".repeat(MAX_VALUE_BYTES));
        let oversized_name = format!("env {}=x", "N".repeat(MAX_NAME_BYTES + 1));
        let too_many_assignments = std::iter::once("env".to_owned())
            .chain((0..=MAX_ASSIGNMENTS).map(|index| format!("V{index}=x")))
            .collect::<Vec<_>>()
            .join(" ");
        let oversized_prefix = format!("env A={}", "x".repeat(MAX_PREFIX_BYTES));
        let combined = std::iter::once("env".to_owned())
            .chain((0..MAX_ASSIGNMENTS).map(|index| format!("V{index}={}", "x".repeat(1024))))
            .collect::<Vec<_>>()
            .join(" ");

        for prefix in [
            oversized_value,
            oversized_name,
            too_many_assignments,
            oversized_prefix,
            combined,
            "env TOKEN='private\nvalue'".to_owned(),
        ] {
            let error = LiteralMakeEnvironmentPrefix::parse(&prefix).unwrap_err();
            let rendered = error.to_string();
            assert!(!rendered.contains("private"));
            assert!(!rendered.contains("secret"));
            assert!(!rendered.contains(&prefix));
        }
    }
}
