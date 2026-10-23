//! Bounded POSIX-style matching for one captured directory-entry component.

use std::cell::Cell;

const MAX_PATTERN_BYTES: usize = 8192;
const MAX_NAME_BYTES: usize = 8192;
const MAX_WORK_UNITS: usize = 4 * 1024 * 1024;

pub(super) struct ComponentPattern {
    tokens: Vec<Token>,
    starts_with_literal_dot: bool,
}

#[derive(Debug)]
enum Token {
    Literal(u8),
    AnyByte,
    Star,
    Class { negated: bool, bytes: [u64; 4] },
}

#[derive(Clone, Copy)]
enum ClassItem {
    Byte(u8),
    Hyphen,
}

impl ComponentPattern {
    pub(super) fn parse(pattern: &str, work: &Cell<usize>) -> Result<Self, String> {
        if pattern.len() > MAX_PATTERN_BYTES {
            return Err("unsupported captured component pattern".into());
        }
        for character in pattern.chars() {
            charge_work(work, 1)?;
            if character == '\\' || character == '/' || character.is_control() {
                return Err("unsupported captured component pattern".into());
            }
        }

        let bytes = pattern.as_bytes();
        charge_work(work, 1)?;
        let mut tokens = Vec::with_capacity(bytes.len());
        let mut position = 0;
        while position < bytes.len() {
            charge_work(work, 1)?;
            match bytes[position] {
                b'*' => {
                    if !matches!(tokens.last(), Some(&Token::Star)) {
                        tokens.push(Token::Star);
                    }
                    position += 1;
                }
                b'?' => {
                    tokens.push(Token::AnyByte);
                    position += 1;
                }
                b'[' => {
                    if let Some((token, end)) = parse_class(bytes, position, work)? {
                        tokens.push(token);
                        position = end;
                    } else {
                        // POSIX treats an opening bracket without a closing
                        // bracket as an ordinary literal character.
                        tokens.push(Token::Literal(b'['));
                        position += 1;
                    }
                }
                byte => {
                    tokens.push(Token::Literal(byte));
                    position += 1;
                }
            }
        }

        let starts_with_literal_dot = matches!(tokens.first(), Some(&Token::Literal(b'.')));
        Ok(Self {
            tokens,
            starts_with_literal_dot,
        })
    }

    #[cfg(test)]
    pub(super) fn new(pattern: &str) -> Result<Self, String> {
        Self::parse(pattern, &Cell::new(0))
    }

    pub(super) fn matches(&self, name: &str, work: &Cell<usize>) -> Result<bool, String> {
        if name.len() > MAX_NAME_BYTES || name.contains('/') || name.chars().any(char::is_control) {
            return Err("unsupported captured directory-entry component".into());
        }

        let name = name.as_bytes();
        let rows = self
            .tokens
            .len()
            .checked_add(1)
            .ok_or("captured component match work limit exceeded")?;
        let columns = name
            .len()
            .checked_add(1)
            .ok_or("captured component match work limit exceeded")?;
        let charge = rows
            .checked_mul(columns)
            .ok_or("captured component match work limit exceeded")?;
        let next_work = work
            .get()
            .checked_add(charge)
            .filter(|total| *total <= MAX_WORK_UNITS)
            .ok_or("captured component match work limit exceeded")?;

        // Charge before either allocating the DP rows or entering their loops.
        work.set(next_work);
        if name.first() == Some(&b'.') && !self.starts_with_literal_dot {
            return Ok(false);
        }

        let mut previous = vec![false; columns];
        let mut current = vec![false; columns];
        previous[0] = true;

        for token in &self.tokens {
            if matches!(token, Token::Star) {
                current[0] = previous[0];
                for index in 1..columns {
                    current[index] = previous[index] || current[index - 1];
                }
            } else {
                current[0] = false;
                for index in 1..columns {
                    current[index] = previous[index - 1] && token.matches_byte(name[index - 1]);
                }
            }
            std::mem::swap(&mut previous, &mut current);
        }

        Ok(previous[name.len()])
    }
}

impl Token {
    fn matches_byte(&self, byte: u8) -> bool {
        match self {
            Self::Literal(expected) => byte == *expected,
            Self::AnyByte | Self::Star => true,
            Self::Class { negated, bytes } => {
                let contained = bytes[usize::from(byte) / 64] & (1u64 << u32::from(byte % 64)) != 0;
                contained != *negated
            }
        }
    }
}

fn parse_class(
    bytes: &[u8],
    start: usize,
    work: &Cell<usize>,
) -> Result<Option<(Token, usize)>, String> {
    let mut position = start + 1;
    let negated = matches!(bytes.get(position), Some(&b'!' | &b'^'));
    if negated {
        position += 1;
    }

    let mut first_item = true;
    let mut unsupported_construct = false;
    let mut items = Vec::new();

    loop {
        charge_work(work, 1)?;
        let Some(&byte) = bytes.get(position) else {
            return Ok(None);
        };

        if byte == b']' {
            if first_item {
                items.push(ClassItem::Byte(b']'));
                first_item = false;
                position += 1;
                continue;
            }
            if unsupported_construct {
                return Err("unsupported POSIX named, collating, or equivalence class".into());
            }
            return Ok(Some((
                Token::Class {
                    negated,
                    bytes: class_bytes(&items, work)?,
                },
                position + 1,
            )));
        }

        if byte == b'[' {
            if let Some(&kind) = bytes.get(position + 1) {
                if !matches!(kind, b':' | b'.' | b'=') {
                    items.push(ClassItem::Byte(byte));
                    first_item = false;
                    position += 1;
                    continue;
                }
                let terminator = match kind {
                    b':' => b':',
                    b'.' => b'.',
                    b'=' => b'=',
                    _ => unreachable!(),
                };
                if let Some(end) = find_class_construct_end(bytes, position + 2, terminator, work)?
                {
                    unsupported_construct = true;
                    first_item = false;
                    position = end;
                    continue;
                }
            }
        }

        items.push(if byte == b'-' {
            ClassItem::Hyphen
        } else {
            ClassItem::Byte(byte)
        });
        first_item = false;
        position += 1;
    }
}

fn find_class_construct_end(
    bytes: &[u8],
    start: usize,
    terminator: u8,
    work: &Cell<usize>,
) -> Result<Option<usize>, String> {
    let mut position = start;
    while position + 1 < bytes.len() {
        charge_work(work, 1)?;
        if bytes[position] == terminator && bytes[position + 1] == b']' {
            return Ok(Some(position + 2));
        }
        if bytes[position] == b']' {
            return Ok(None);
        }
        position += 1;
    }
    Ok(None)
}

fn class_bytes(items: &[ClassItem], work: &Cell<usize>) -> Result<[u64; 4], String> {
    let mut bytes = [0u64; 4];
    let mut position = 0;
    while position < items.len() {
        charge_work(work, 1)?;
        if let (
            Some(&ClassItem::Byte(start)),
            Some(&ClassItem::Hyphen),
            Some(&ClassItem::Byte(end)),
        ) = (
            items.get(position),
            items.get(position + 1),
            items.get(position + 2),
        ) {
            for byte in start..=end {
                charge_work(work, 1)?;
                bytes[usize::from(byte) / 64] |= 1u64 << u32::from(byte % 64);
            }
            position += 3;
            continue;
        }

        match items[position] {
            ClassItem::Byte(byte) => {
                charge_work(work, 1)?;
                bytes[usize::from(byte) / 64] |= 1u64 << u32::from(byte % 64);
            }
            ClassItem::Hyphen => {
                charge_work(work, 1)?;
                bytes[0] |= 1u64 << u32::from(b'-');
            }
        }
        position += 1;
    }
    Ok(bytes)
}

fn charge_work(work: &Cell<usize>, amount: usize) -> Result<(), String> {
    let next_work = work
        .get()
        .checked_add(amount)
        .filter(|total| *total <= MAX_WORK_UNITS)
        .ok_or("captured component match work limit exceeded")?;
    work.set(next_work);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, name: &str) -> Result<bool, String> {
        ComponentPattern::new(pattern)?.matches(name, &Cell::new(0))
    }

    #[test]
    fn matches_classes_ranges_and_literal_bracket_members() {
        assert!(matches("[abc]", "b").unwrap());
        assert!(!matches("[abc]", "z").unwrap());
        assert!(matches("[!abc]", "z").unwrap());
        assert!(!matches("[!abc]", "b").unwrap());
        assert!(matches("[a-c]", "c").unwrap());
        assert!(!matches("[a-c]", "z").unwrap());
        assert!(matches("*[a-c]*", "xby").unwrap());
        assert!(matches("*[-a]*", "x-y").unwrap());
        assert!(!matches("[z-a]", "z").unwrap());
        assert!(!matches("[z-a]", "m").unwrap());
        assert!(matches("[]a]", "]").unwrap());
        assert!(matches("[-a]", "-").unwrap());
        assert!(matches("[a-]", "-").unwrap());
        assert!(matches("[!]]", "x").unwrap());
        assert!(!matches("[!]]", "]").unwrap());
        assert!(matches("[", "[").unwrap());
        assert!(matches("[abc", "[abc").unwrap());
        assert!(ComponentPattern::new("[[:alpha:]]").is_err());
        assert!(ComponentPattern::new("[[.ch.]]").is_err());
        assert!(ComponentPattern::new("[[=a=]]").is_err());
    }

    #[test]
    fn collapses_star_runs_outside_classes_only() {
        let ordinary = ComponentPattern::new("a***b").unwrap();
        assert_eq!(ordinary.tokens.len(), 3);
        assert!(ordinary.matches("axxb", &Cell::new(0)).unwrap());

        let classes = ComponentPattern::new("[*][*]**").unwrap();
        assert_eq!(classes.tokens.len(), 3);
        assert!(classes.matches("***", &Cell::new(0)).unwrap());
        assert!(!ComponentPattern::new("[**]")
            .unwrap()
            .matches("**", &Cell::new(0))
            .unwrap());
    }

    #[test]
    fn requires_a_literal_leading_dot_for_hidden_names() {
        assert!(!matches("*", ".hidden").unwrap());
        assert!(!matches("?.hidden", ".hidden").unwrap());
        assert!(!matches("[.]hidden", ".hidden").unwrap());
        assert!(matches(".*", ".hidden").unwrap());
    }

    #[test]
    fn question_mark_consumes_one_utf8_byte() {
        assert!(!matches("?", "é").unwrap());
        assert!(matches("??", "é").unwrap());
        assert!(matches("é", "é").unwrap());
    }

    #[test]
    fn repeated_stars_and_literals_handle_a_near_miss_with_bounded_dp() {
        let pattern = format!("{}*b", "*a".repeat(32));
        let name = "a".repeat(512);
        let compiled = ComponentPattern::new(&pattern).unwrap();
        assert!(!compiled.matches(&name, &Cell::new(0)).unwrap());
    }

    #[test]
    fn match_work_budget_is_shared_and_exhaustion_precedes_more_work() {
        let pattern = ComponentPattern::new("*").unwrap();
        let name = "x".repeat(4096);
        let work = Cell::new(0);
        for _ in 0..511 {
            assert!(pattern.matches(&name, &work).unwrap());
        }
        assert_eq!(work.get(), 4_187_134);
        assert!(pattern.matches(&name, &work).is_err());
        assert_eq!(work.get(), 4_187_134);
    }

    #[test]
    fn compilation_charges_unclosed_and_nested_class_scans() {
        let unclosed = "[".repeat(4096);
        let unclosed_work = Cell::new(0);
        assert!(ComponentPattern::parse(&unclosed, &unclosed_work).is_err());
        assert_eq!(unclosed_work.get(), MAX_WORK_UNITS);

        let nested = "[[::".repeat(2000);
        let nested_work = Cell::new(0);
        assert!(ComponentPattern::parse(&nested, &nested_work).is_err());
        assert_eq!(nested_work.get(), MAX_WORK_UNITS);

        let valid_work = Cell::new(0);
        let pattern = ComponentPattern::parse("*.[ch]", &valid_work).unwrap();
        let before_match = valid_work.get();
        assert!(pattern.matches("source.c", &valid_work).unwrap());
        assert!(valid_work.get() > before_match);
    }
}
