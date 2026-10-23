//! Literal shell-to-Make command data; never execution authority.
//!
//! MetaMake's POSIX launcher expands a source command string before passing it
//! to a shell. This parser models only literal words and environment assignment
//! prefixes in that string. It does not resolve PATH, interpret Make options,
//! discover inherited environment, or authorize launching the selected program.

use crate::make_environment_prefix::{is_ascii_scalar_name, tokenize_literal_command};
use std::{collections::BTreeMap, fmt};

const MAX_BYTES: usize = 128 * 1024;
const MAX_WORDS: usize = 4096;
const MAX_ENVIRONMENT_BYTES: usize = 128 * 1024;

/// Bounded literal command words and explicit environment overrides.
///
/// The selected executable, inherited process context and Make options still
/// require independent binding before this data can describe a native launch.
/// Interpreting the literal `env` prefix assumes POSIX env semantics; this type
/// does not authenticate the env executable selected by the native shell's PATH.
#[derive(Clone, Eq, PartialEq)]
pub struct LiteralMakeChildCommand {
    program: String,
    arguments: Vec<String>,
    assignments: BTreeMap<String, String>,
}

// Inherited/source values may be sensitive. Debug is structural, not a dump.
impl fmt::Debug for LiteralMakeChildCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiteralMakeChildCommand")
            .field("argument_count", &self.arguments.len())
            .field("assignment_count", &self.assignments.len())
            .finish_non_exhaustive()
    }
}

impl LiteralMakeChildCommand {
    /// Parse one literal command, optionally prefixed by `env` assignments.
    /// Assignment order follows env's last-write wins.
    /// Words following the program are argv, including `NAME=value` words.
    ///
    /// # Errors
    /// Refuses shell effects/expansion, env options, missing program, malformed
    /// assignment data and exhausted bounds. Diagnostics omit supplied values.
    pub fn parse(command: &str) -> crate::Result<Self> {
        let words = tokenize_literal_command(command, MAX_BYTES, MAX_WORDS)?;
        let mut cursor = 0;
        let mut assignments = BTreeMap::new();
        let via_env = words.get(cursor).map(String::as_str) == Some("env");
        if via_env {
            cursor += 1;
            if words.get(cursor).map(String::as_str) == Some("--") {
                cursor += 1;
            } else if words.get(cursor).is_some_and(|word| word.starts_with('-')) {
                return Err(invalid("env options need explicit effect modeling"));
            }
            take_assignments(&words, &mut cursor, &mut assignments)?;
        } else if words.first().is_some_and(|word| word.contains('=')) {
            // Shell assignment recognition depends on quote provenance of the
            // name and equals sign; decoded literal argv alone cannot prove it.
            return Err(invalid(
                "leading shell assignments need explicit syntax modeling",
            ));
        }
        let program = words
            .get(cursor)
            .filter(|program| !program.is_empty() && !program.starts_with('-'))
            .ok_or_else(|| invalid("literal child program is missing or invalid"))?
            .clone();
        if !via_env
            && matches!(
                program.as_str(),
                "!" | "if"
                    | "then"
                    | "else"
                    | "elif"
                    | "fi"
                    | "for"
                    | "while"
                    | "until"
                    | "do"
                    | "done"
                    | "case"
                    | "esac"
                    | "in"
                    | "time"
                    | "."
                    | ":"
                    | "alias"
                    | "bg"
                    | "break"
                    | "cd"
                    | "command"
                    | "continue"
                    | "echo"
                    | "printf"
                    | "test"
                    | "["
                    | "eval"
                    | "exec"
                    | "exit"
                    | "export"
                    | "false"
                    | "fc"
                    | "fg"
                    | "getopts"
                    | "hash"
                    | "jobs"
                    | "kill"
                    | "newgrp"
                    | "pwd"
                    | "read"
                    | "readonly"
                    | "return"
                    | "set"
                    | "shift"
                    | "times"
                    | "trap"
                    | "true"
                    | "type"
                    | "ulimit"
                    | "umask"
                    | "unalias"
                    | "unset"
                    | "wait"
            )
        {
            // Decoded words do not retain reserved-word/builtin quote context.
            // Refuse these forms, including quoted spellings, rather than
            // misrepresent a shell effect as an external child executable.
            return Err(invalid(
                "shell control or builtin command needs explicit modeling",
            ));
        }
        let arguments = words[cursor + 1..].to_vec();
        Ok(Self {
            program,
            arguments,
            assignments,
        })
    }

    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }

    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    #[must_use]
    pub const fn environment_assignments(&self) -> &BTreeMap<String, String> {
        &self.assignments
    }

    /// Apply source assignments to an explicitly supplied complete inherited
    /// environment. Never consult the current process environment implicitly.
    /// These values have environment origin in the child, not file origin.
    /// Inherited keys are opaque environment data, not shell-assignment syntax;
    /// preserve names such as `HAS-DASH` or `1DIGIT` rather than dropping them.
    ///
    /// # Errors
    /// Refuses unsupported environment names/data or exhausted bounds.
    pub fn apply_environment(
        &self,
        inherited: &BTreeMap<String, String>,
    ) -> crate::Result<BTreeMap<String, String>> {
        validate_environment(inherited)?;
        let mut result = inherited.clone();
        result.extend(self.assignments.clone());
        validate_environment(&result)?;
        Ok(result)
    }
}

fn take_assignments(
    words: &[String],
    cursor: &mut usize,
    assignments: &mut BTreeMap<String, String>,
) -> crate::Result<()> {
    while let Some(word) = words.get(*cursor) {
        let Some((name, value)) = word.split_once('=') else {
            break;
        };
        if !is_ascii_scalar_name(name) {
            return Err(invalid("unsupported literal assignment name"));
        }
        assignments.insert(name.to_owned(), value.to_owned());
        *cursor += 1;
    }
    validate_environment(assignments)
}

fn validate_environment(environment: &BTreeMap<String, String>) -> crate::Result<()> {
    let mut bytes = 0usize;
    for (name, value) in environment {
        if name.is_empty()
            || name.contains('=')
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err(invalid("unsupported literal environment name or data"));
        }
        bytes = bytes
            .checked_add(name.len())
            .and_then(|bytes| bytes.checked_add(value.len()))
            .filter(|bytes| *bytes <= MAX_ENVIRONMENT_BYTES)
            .ok_or_else(|| invalid("literal environment exceeds byte bound"))?;
    }
    Ok(())
}

fn invalid(message: &'static str) -> crate::ArosError {
    crate::ArosError::Configuration {
        file: "Literal Make child command".into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_argv_and_applies_environment_without_a_variable_table() {
        let command = LiteralMakeChildCommand::parse(
            "env OUTER=before OUTER=after EMPTY= 'a program' '-r' 'NAME=command value' '' 'literal$byte'",
        ).unwrap();
        assert_eq!(command.program(), "a program");
        assert_eq!(
            command.arguments(),
            ["-r", "NAME=command value", "", "literal$byte"]
        );
        let inherited = BTreeMap::from([
            ("KEEP".into(), "inherited".into()),
            ("OUTER".into(), "old".into()),
            ("HAS-DASH".into(), "literal-key".into()),
            ("1DIGIT".into(), "literal-key".into()),
        ]);
        assert_eq!(
            command.apply_environment(&inherited).unwrap(),
            BTreeMap::from([
                ("KEEP".into(), "inherited".into()),
                ("OUTER".into(), "after".into()),
                ("HAS-DASH".into(), "literal-key".into()),
                ("1DIGIT".into(), "literal-key".into()),
                ("EMPTY".into(), String::new())
            ])
        );
        assert_eq!(inherited["OUTER"], "old");
        assert!(!format!("{command:?}").contains("inherited"));
        let prefix = crate::make_environment_prefix::LiteralMakeEnvironmentPrefix::parse(
            "env TOKEN='sensitive-value'",
        )
        .unwrap();
        assert!(!format!("{prefix:?}").contains("sensitive-value"));
        assert!(LiteralMakeChildCommand::parse("env -- /make -f file.mk").is_ok());
        assert!(LiteralMakeChildCommand::parse("/make -f file.mk").is_ok());
        assert_eq!(
            LiteralMakeChildCommand::parse("env X=one echo hi")
                .unwrap()
                .program(),
            "echo"
        );
        let literal_negation = LiteralMakeChildCommand::parse("env A='! value' /make '!'").unwrap();
        assert_eq!(literal_negation.environment_assignments()["A"], "! value");
        assert_eq!(literal_negation.arguments(), ["!"]);
        assert!(LiteralMakeChildCommand::parse("! /make")
            .unwrap_err()
            .to_string()
            .contains("shell control"));
    }

    #[test]
    fn refuses_unmodeled_effects_and_never_reports_values() {
        for text in [
            "env",
            "A=secret",
            "env -i A=secret /make",
            "env -u SECRET /make",
            "env A=$SECRET /make",
            "env A=secret /make; other",
            "env A=secret /make | other",
            "env A=secret /make >output",
            "env 1BAD=secret /make",
            "''",
            "! /make",
            "! env A=secret /make",
            "'!' /make",
            "if /make",
            "exec /make",
            "eval /make",
        ] {
            let error = LiteralMakeChildCommand::parse(text)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret"));
        }
        let command = LiteralMakeChildCommand::parse("/make").unwrap();
        assert!(command
            .apply_environment(&BTreeMap::from([("BAD=NAME".into(), "secret".into())]))
            .is_err());
        assert!(command
            .apply_environment(&BTreeMap::from([("OK".into(), "secret\n".into())]))
            .is_err());
    }
}
