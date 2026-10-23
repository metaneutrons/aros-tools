//! Bounded syntax scanning for assignments with computed variable names.
//!
//! This module only locates the assignment operator. It deliberately does not
//! expand Make expressions or interpret the right-hand side.

const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_EXPANSION_DEPTH: usize = 128;
const MAX_LITERAL_NESTING: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpansionError {
    Unclosed,
    DepthLimit,
}

/// Splits a top-level assignment if its left-hand side contains a Make expansion.
///
/// The left-hand side is trimmed at its outer whitespace boundary. The operator
/// is returned verbatim, and the right-hand side starts immediately after that
/// operator so all of its whitespace is preserved. Assignment-looking syntax
/// following a top-level rule colon is left to the rule parser.
pub(super) fn split(line: &str) -> Result<Option<(&str, &str, &str)>, String> {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut lhs_has_expansion = false;

    while index < bytes.len() {
        match bytes[index] {
            b'#' if !is_escaped_hash(bytes, index) => return Ok(None),
            b'$' => {
                if bytes.get(index + 1) == Some(&b'$') {
                    index += 2;
                    continue;
                }
                match bytes.get(index + 1) {
                    Some(b'(' | b'{') => {
                        lhs_has_expansion = true;
                        match skip_expansion(line, index, 1) {
                            Ok(next) => index = next,
                            Err(error) => {
                                return if has_assignment_spelling(&line[index..]) {
                                    Err(expansion_error_message(error).into())
                                } else {
                                    Ok(None)
                                };
                            }
                        }
                    }
                    Some(_) => {
                        lhs_has_expansion = true;
                        index += 1 + next_char_len(line, index + 1);
                    }
                    None => index += 1,
                }
            }
            b':' => match operator_at(bytes, index) {
                Some(Operator::Supported(operator)) => {
                    return assignment_parts(line, index, operator, lhs_has_expansion);
                }
                Some(Operator::Unsupported(operator)) => {
                    return unsupported_assignment(operator, lhs_has_expansion);
                }
                None => return Ok(None),
            },
            b'+' | b'?' | b'!' if bytes.get(index + 1) == Some(&b'=') => {
                match operator_at(bytes, index) {
                    Some(Operator::Supported(operator)) => {
                        return assignment_parts(line, index, operator, lhs_has_expansion);
                    }
                    Some(Operator::Unsupported(operator)) => {
                        return unsupported_assignment(operator, lhs_has_expansion);
                    }
                    None => unreachable!("assignment punctuation was checked"),
                }
            }
            b'=' => {
                return assignment_parts(line, index, "=", lhs_has_expansion);
            }
            _ => index += next_char_len(line, index),
        }
    }

    Ok(None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Supported(&'static str),
    Unsupported(&'static str),
}

fn operator_at(bytes: &[u8], index: usize) -> Option<Operator> {
    let tail = bytes.get(index..)?;
    if tail.starts_with(b":::=") {
        Some(Operator::Unsupported(":::="))
    } else if tail.starts_with(b"::=") {
        Some(Operator::Supported("::="))
    } else if tail.starts_with(b":=") {
        Some(Operator::Supported(":="))
    } else if tail.starts_with(b"+=") {
        Some(Operator::Supported("+="))
    } else if tail.starts_with(b"?=") {
        Some(Operator::Supported("?="))
    } else if tail.starts_with(b"!=") {
        Some(Operator::Unsupported("!="))
    } else {
        None
    }
}

fn assignment_parts<'a>(
    line: &'a str,
    index: usize,
    operator: &'static str,
    lhs_has_expansion: bool,
) -> Result<Option<(&'a str, &'a str, &'a str)>, String> {
    if !lhs_has_expansion {
        return Ok(None);
    }
    if line.len() > MAX_LINE_BYTES {
        return Err("computed Make assignment line exceeds scanner byte limit".into());
    }
    let lhs = line[..index].trim();
    if lhs.is_empty() {
        return Ok(None);
    }
    let rhs_start = index + operator.len();
    Ok(Some((lhs, operator, &line[rhs_start..])))
}

fn unsupported_assignment(
    operator: &str,
    lhs_has_expansion: bool,
) -> Result<Option<(&str, &str, &str)>, String> {
    if lhs_has_expansion {
        Err(format!(
            "unsupported computed Make assignment operator {operator}"
        ))
    } else {
        Ok(None)
    }
}

fn skip_expansion(line: &str, start: usize, depth: usize) -> Result<usize, ExpansionError> {
    if depth > MAX_EXPANSION_DEPTH {
        return Err(ExpansionError::DepthLimit);
    }

    let bytes = line.as_bytes();
    let opening = *bytes.get(start + 1).ok_or(ExpansionError::Unclosed)?;
    let closing = match opening {
        b'(' => b')',
        b'{' => b'}',
        _ => return Err(ExpansionError::Unclosed),
    };
    let mut literal_depth = 0usize;
    let mut index = start + 2;

    while index < bytes.len() {
        match bytes[index] {
            b'$' => match bytes.get(index + 1) {
                Some(b'$') => index += 2,
                Some(b'(' | b'{') => {
                    index = skip_expansion(line, index, depth + 1)?;
                }
                Some(_) => index += 1 + next_char_len(line, index + 1),
                None => index += 1,
            },
            byte if byte == opening => {
                literal_depth += 1;
                if literal_depth > MAX_LITERAL_NESTING {
                    return Err(ExpansionError::DepthLimit);
                }
                index += 1;
            }
            byte if byte == closing => {
                if literal_depth == 0 {
                    return Ok(index + 1);
                }
                literal_depth -= 1;
                index += 1;
            }
            _ => index += next_char_len(line, index),
        }
    }

    Err(ExpansionError::Unclosed)
}

fn has_assignment_spelling(candidate: &str) -> bool {
    // Once an expression is malformed, there is no reliable top-level boundary
    // left to inspect. An assignment token after its opening delimiter is enough
    // to treat the line as a plausible computed assignment and refuse it.
    candidate.contains(":::=")
        || candidate.contains("::=")
        || candidate.contains(":=")
        || candidate.contains("+=")
        || candidate.contains("?=")
        || candidate.contains("!=")
        || candidate.contains('=')
}

const fn expansion_error_message(error: ExpansionError) -> &'static str {
    match error {
        ExpansionError::Unclosed => "unclosed Make expansion in computed assignment lhs",
        ExpansionError::DepthLimit => "computed Make assignment expansion exceeds nesting limit",
    }
}

const fn is_escaped_hash(bytes: &[u8], index: usize) -> bool {
    let mut backslashes = 0;
    let mut previous = index;
    while previous > 0 && bytes[previous - 1] == b'\\' {
        backslashes += 1;
        previous -= 1;
    }
    backslashes % 2 == 1
}

fn next_char_len(line: &str, index: usize) -> usize {
    line[index..].chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::{split, MAX_EXPANSION_DEPTH, MAX_LINE_BYTES};

    fn assert_split(line: &str, expected: (&str, &str, &str)) {
        assert_eq!(split(line).unwrap(), Some(expected));
    }

    #[test]
    fn recognizes_supported_assignment_operators() {
        assert_split("$N_suffix = short", ("$N_suffix", "=", " short"));
        for (operator, rhs) in [
            ("=", "  recursive  "),
            (":=", "  simple  "),
            ("::=", "  simple-alias  "),
            ("+=", "  append  "),
            ("?=", "  conditional  "),
        ] {
            let line = format!("  RESULT_$(NAME) {operator}{rhs}");
            assert_split(&line, ("RESULT_$(NAME)", operator, rhs));
        }
    }

    #[test]
    fn ignores_nested_substitution_equals_and_delimiters() {
        let line = "VALUE_$(subst =,x,$(call f,(left,right),{brace})) :=  result";
        assert_split(
            line,
            (
                "VALUE_$(subst =,x,$(call f,(left,right),{brace}))",
                ":=",
                "  result",
            ),
        );
    }

    #[test]
    fn balances_nested_braces_and_expansions() {
        let line = "${outer{literal${inner{nested}}}}?= value";
        assert_split(line, ("${outer{literal${inner{nested}}}}", "?=", " value"));
    }

    #[test]
    fn leaves_rhs_opaque_and_preserves_its_whitespace() {
        assert_split(
            "$(NAME) :=  tail $(BROKEN",
            ("$(NAME)", ":=", "  tail $(BROKEN"),
        );
        assert_split(
            "$(NAME) = $(OTHER) := end",
            ("$(NAME)", "=", " $(OTHER) := end"),
        );
    }

    #[test]
    fn top_level_rule_colons_precede_target_specific_assignments() {
        assert_eq!(split("$(TARGET): LOCAL := value").unwrap(), None);
        assert_eq!(split("target: LOCAL_$(NAME) = value").unwrap(), None);
        assert_eq!(split("$(eval COUNT += 1): dependency").unwrap(), None);
        assert_split(
            "$(NAME) := dependency: value",
            ("$(NAME)", ":=", " dependency: value"),
        );
    }

    #[test]
    fn escaped_dollars_do_not_form_computed_lhs_references() {
        assert_eq!(split("$$(NAME) = literal").unwrap(), None);
        assert_split("$(NAME) = $$(BROKEN", ("$(NAME)", "=", " $$(BROKEN"));
        assert_split(
            "$(subst $$(x = y),z,a) += value",
            ("$(subst $$(x = y),z,a)", "+=", " value"),
        );
    }

    #[test]
    fn ignores_assignment_characters_inside_nested_references() {
        assert_split(
            "$(outer$(inner:=value)tail) ::= rhs",
            ("$(outer$(inner:=value)tail)", "::=", " rhs"),
        );
    }

    #[test]
    fn refuses_unsupported_computed_assignment_operators() {
        assert!(split("$(NAME) :::= value").unwrap_err().contains(":::="));
        assert!(split("$(NAME) != command").unwrap_err().contains("!="));
        assert_eq!(split("NAME :::= value").unwrap(), None);
        assert_eq!(split("NAME != command").unwrap(), None);
    }

    #[test]
    fn reports_plausibly_computed_assignments_with_unclosed_lhs_references() {
        assert!(split("$(NAME := value").is_err());
        assert!(split("PREFIX_$(NAME+=value").is_err());
        assert_eq!(split("$(NAME").unwrap(), None);
        assert_split("$(NAME) := $(BROKEN", ("$(NAME)", ":=", " $(BROKEN"));
    }

    #[test]
    fn stops_at_unescaped_comments() {
        assert_eq!(split("# $(NAME) = ignored").unwrap(), None);
        assert_split("NAME\\#$(PART) = value", ("NAME\\#$(PART)", "=", " value"));
    }

    #[test]
    fn enforces_line_and_expansion_depth_bounds() {
        let oversized = format!("$(NAME) = {}", "x".repeat(MAX_LINE_BYTES));
        assert!(split(&oversized).is_err());
        // Non-computed lines and rule tails keep their existing reader limits.
        let literal = format!("LITERAL = {}", "x".repeat(MAX_LINE_BYTES));
        assert_eq!(split(&literal).unwrap(), None);
        let rule = format!("$(TARGET): {}", "x".repeat(MAX_LINE_BYTES));
        assert_eq!(split(&rule).unwrap(), None);

        let nesting = MAX_EXPANSION_DEPTH + 1;
        let nested = format!("{}X{} := value", "$(".repeat(nesting), ")".repeat(nesting));
        assert!(split(&nested).is_err());
    }
}
