//! Bounded evaluation of the literal CMake conditions emitted for icons.
//!
//! This deliberately accepts only the small boolean expression subset used
//! by `icons.rs`; it does not attempt to interpret general CMake syntax.

use crate::parser::TargetContext;

const MAX_BYTES: usize = 4096;
const MAX_TOKENS: usize = 128;
const MAX_DEPTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token<'a> {
    Identifier(&'a str),
    Quoted(&'a str),
    OpenParen,
    CloseParen,
}

/// Evaluates an emitted icon condition if every selector and token is known.
///
/// Unknown selectors and syntax are left undecided so callers can preserve
/// the icon conservatively.
pub fn condition_matches(condition: &str, context: &TargetContext) -> Option<bool> {
    if condition.len() > MAX_BYTES || !condition.is_ascii() {
        return None;
    }

    let tokens = lex(condition)?;
    let mut parser = Parser {
        tokens: &tokens,
        position: 0,
        context,
    };
    let result = parser.parse_or(0)?;
    (parser.position == tokens.len()).then_some(result)
}

fn lex(input: &str) -> Option<Vec<Token<'_>>> {
    let bytes = input.as_bytes();
    let mut position = 0;
    let mut tokens = Vec::new();

    while position < bytes.len() {
        match bytes[position] {
            byte if byte.is_ascii_whitespace() => position += 1,
            b'(' => {
                tokens.push(Token::OpenParen);
                position += 1;
            }
            b')' => {
                tokens.push(Token::CloseParen);
                position += 1;
            }
            b'"' => {
                position += 1;
                let start = position;
                while position < bytes.len() && bytes[position] != b'"' {
                    if !is_safe_literal_byte(bytes[position]) {
                        return None;
                    }
                    position += 1;
                }
                if position == bytes.len() {
                    return None;
                }
                tokens.push(Token::Quoted(&input[start..position]));
                position += 1;
            }
            byte if is_identifier_byte(byte) => {
                let start = position;
                while position < bytes.len() && is_identifier_byte(bytes[position]) {
                    position += 1;
                }
                tokens.push(Token::Identifier(&input[start..position]));
            }
            _ => return None,
        }

        if tokens.len() > MAX_TOKENS {
            return None;
        }
    }

    (!tokens.is_empty()).then_some(tokens)
}

const fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

const fn is_safe_literal_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+' | b':' | b'/')
}

struct Parser<'tokens, 'source, 'context> {
    tokens: &'tokens [Token<'source>],
    position: usize,
    context: &'context TargetContext,
}

impl<'source> Parser<'_, 'source, '_> {
    fn parse_or(&mut self, depth: usize) -> Option<bool> {
        let mut result = self.parse_and(depth)?;
        while self.take_identifier("OR") {
            let right = self.parse_and(depth)?;
            result |= right;
        }
        Some(result)
    }

    fn parse_and(&mut self, depth: usize) -> Option<bool> {
        let mut result = self.parse_unary(depth)?;
        while self.take_identifier("AND") {
            let right = self.parse_unary(depth)?;
            result &= right;
        }
        Some(result)
    }

    fn parse_unary(&mut self, depth: usize) -> Option<bool> {
        if self.take_identifier("NOT") {
            if depth >= MAX_DEPTH {
                return None;
            }
            return Some(!self.parse_unary(depth + 1)?);
        }

        match self.peek()? {
            Token::OpenParen => {
                if depth >= MAX_DEPTH {
                    return None;
                }
                self.position += 1;
                let result = self.parse_or(depth + 1)?;
                if self.peek() != Some(Token::CloseParen) {
                    return None;
                }
                self.position += 1;
                Some(result)
            }
            _ => self.parse_comparison(),
        }
    }

    fn parse_comparison(&mut self) -> Option<bool> {
        let Token::Identifier(selector) = self.next()? else {
            return None;
        };
        if !self.take_identifier("STREQUAL") {
            return None;
        }
        let Token::Quoted(literal) = self.next()? else {
            return None;
        };
        let selected = match selector {
            "AROS_TARGET_PLATFORM" => self.context.platform.as_deref(),
            "AROS_TARGET_CPU" => self.context.cpu.as_deref(),
            _ => return None,
        }?;
        Some(selected == literal)
    }

    fn take_identifier(&mut self, expected: &str) -> bool {
        match self.peek() {
            Some(Token::Identifier(identifier)) if identifier == expected => {
                self.position += 1;
                true
            }
            _ => false,
        }
    }

    fn peek(&self) -> Option<Token<'source>> {
        self.tokens.get(self.position).copied()
    }

    fn next(&mut self) -> Option<Token<'source>> {
        let token = self.peek()?;
        self.position += 1;
        Some(token)
    }
}

#[cfg(test)]
mod tests {
    use super::condition_matches;
    use crate::parser::TargetContext;

    fn context(platform: &str, cpu: &str) -> TargetContext {
        TargetContext {
            platform: Some(platform.to_owned()),
            cpu: Some(cpu.to_owned()),
            ..TargetContext::default()
        }
    }

    #[test]
    fn evaluates_current_emitted_icon_conditions() {
        let cases = [
            ("AROS_TARGET_PLATFORM STREQUAL \"pc\"", "pc", "x86_64"),
            (
                "AROS_TARGET_PLATFORM STREQUAL \"opensbi\"",
                "opensbi",
                "riscv64",
            ),
            (
                "(AROS_TARGET_PLATFORM STREQUAL \"amiga\") AND (AROS_TARGET_CPU STREQUAL \"m68k\")",
                "amiga",
                "m68k",
            ),
            (
                "(AROS_TARGET_PLATFORM STREQUAL \"amiga\") AND (NOT AROS_TARGET_CPU STREQUAL \"m68k\")",
                "amiga",
                "ppc",
            ),
            (
                "(NOT AROS_TARGET_PLATFORM STREQUAL \"pc\") AND (NOT AROS_TARGET_PLATFORM STREQUAL \"opensbi\") AND (NOT AROS_TARGET_PLATFORM STREQUAL \"amiga\")",
                "raspi",
                "aarch64",
            ),
        ];

        for (condition, platform, cpu) in cases {
            assert_eq!(
                condition_matches(condition, &context(platform, cpu)),
                Some(true),
                "condition: {condition}"
            );
        }
    }

    #[test]
    fn returns_false_for_known_nonmatching_selectors() {
        let cases = [
            ("AROS_TARGET_PLATFORM STREQUAL \"opensbi\"", "pc", "x86_64"),
            (
                "(AROS_TARGET_PLATFORM STREQUAL \"amiga\") AND (AROS_TARGET_CPU STREQUAL \"m68k\")",
                "amiga",
                "ppc",
            ),
            (
                "(AROS_TARGET_PLATFORM STREQUAL \"amiga\") AND (NOT AROS_TARGET_CPU STREQUAL \"m68k\")",
                "amiga",
                "m68k",
            ),
            (
                "(NOT AROS_TARGET_PLATFORM STREQUAL \"pc\") AND (NOT AROS_TARGET_PLATFORM STREQUAL \"opensbi\") AND (NOT AROS_TARGET_PLATFORM STREQUAL \"amiga\")",
                "pc",
                "x86_64",
            ),
        ];

        for (condition, platform, cpu) in cases {
            assert_eq!(
                condition_matches(condition, &context(platform, cpu)),
                Some(false),
                "condition: {condition}"
            );
        }
    }

    #[test]
    fn applies_not_and_or_precedence_and_parentheses() {
        let target = context("pc", "m68k");

        assert_eq!(
            condition_matches(
                "AROS_TARGET_PLATFORM STREQUAL \"pc\" OR AROS_TARGET_CPU STREQUAL \"ppc\" AND AROS_TARGET_CPU STREQUAL \"ppc\"",
                &target,
            ),
            Some(true)
        );
        assert_eq!(
            condition_matches(
                "AROS_TARGET_PLATFORM STREQUAL \"other\" OR AROS_TARGET_CPU STREQUAL \"m68k\" AND NOT AROS_TARGET_CPU STREQUAL \"m68k\"",
                &target,
            ),
            Some(false)
        );
        assert_eq!(
            condition_matches(
                "NOT (AROS_TARGET_PLATFORM STREQUAL \"pc\" AND AROS_TARGET_CPU STREQUAL \"ppc\")",
                &target,
            ),
            Some(true)
        );
    }

    #[test]
    fn unknown_selectors_and_syntax_remain_undecided() {
        let target = context("pc", "x86_64");
        let unknown = TargetContext {
            cpu: None,
            ..context("pc", "x86_64")
        };

        for condition in [
            "OTHER_SELECTOR STREQUAL \"pc\"",
            "AROS_TARGET_PLATFORM STREQUAL pc",
            "AROS_TARGET_PLATFORM EQUAL \"pc\"",
            "AROS_TARGET_PLATFORM STREQUAL \"pc\" trailing",
            "(AROS_TARGET_PLATFORM STREQUAL \"pc\"",
            "AROS_TARGET_PLATFORM STREQUAL \"pc\")",
            "AROS_TARGET_PLATFORM STREQUAL \"pc\" AND",
            "DEFINED AROS_TARGET_PLATFORM",
            "AROS_TARGET_PLATFORM STREQUAL \"$<bad>\"",
        ] {
            assert_eq!(condition_matches(condition, &target), None, "{condition}");
        }

        assert_eq!(
            condition_matches("AROS_TARGET_CPU STREQUAL \"x86_64\"", &unknown),
            None
        );
    }

    #[test]
    fn rejects_expressions_over_resource_limits() {
        let target = context("pc", "x86_64");
        let too_long = format!("{}AROS_TARGET_PLATFORM STREQUAL \"pc\"", " ".repeat(4096));
        assert_eq!(condition_matches(&too_long, &target), None);

        let too_many_tokens = std::iter::repeat_n("AROS_TARGET_PLATFORM STREQUAL \"pc\"", 33)
            .collect::<Vec<_>>()
            .join(" OR ");
        assert_eq!(condition_matches(&too_many_tokens, &target), None);

        let too_deep = format!(
            "{}AROS_TARGET_PLATFORM STREQUAL \"pc\"{}",
            "(".repeat(17),
            ")".repeat(17)
        );
        assert_eq!(condition_matches(&too_deep, &target), None);
    }
}
