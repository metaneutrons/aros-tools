//! Bounded Make logical-line reader with recipe-aware continuation handling.

use crate::{
    make_rule_header::{parse as parse_rule_header, RuleHeader},
    make_vars::{make_comment_start, make_define_directive},
};
use std::ops::Range;

const MAX_INPUT: usize = 16 * 1024 * 1024;
const MAX_PHYSICAL_LINES: usize = 65_536;
const MAX_DEFINE_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LogicalLine {
    pub text: String,
    pub source_line: usize,
    pub source_range: Range<usize>,
}

#[derive(Debug)]
pub(super) struct LineError {
    pub source_line: usize,
    pub detail: &'static str,
}

/// Read physical input into bounded Make logical lines.
///
/// Ranges partition the original input, including each physical newline.
/// Recipe continuation bytes remain shell data; ordinary Make continuations
/// fold to one separating space.
pub(super) fn read(text: &str) -> Result<Vec<LogicalLine>, LineError> {
    read_internal(text.as_bytes(), false)
}

/// Read bounded Make logical lines from source bytes.
///
/// Ordinary non-recipe lines outside `define` bodies discard a Make comment
/// before UTF-8 decoding. Recipes, inline recipes and definition bodies retain
/// their complete data and must be valid UTF-8. Source ranges always refer to
/// the original bytes, including comment suffixes and physical newlines.
pub(super) fn read_bytes(input: &[u8]) -> Result<Vec<LogicalLine>, LineError> {
    if input.len() > MAX_INPUT {
        return Err(LineError {
            source_line: 0,
            detail: "Make input exceeds byte limit",
        });
    }
    if let Some(nul) = input.iter().position(|byte| *byte == 0) {
        return Err(LineError {
            source_line: input[..nul].split(|byte| *byte == b'\n').count() - 1,
            detail: "Make input contains a NUL byte",
        });
    }
    read_internal(input, true)
}

fn read_internal(input: &[u8], discard_comments: bool) -> Result<Vec<LogicalLine>, LineError> {
    if input.len() > MAX_INPUT {
        return Err(LineError {
            source_line: 0,
            detail: "Make input exceeds byte limit",
        });
    }

    let physical_lines = input.split_inclusive(|byte| *byte == b'\n').count();
    if physical_lines > MAX_PHYSICAL_LINES {
        return Err(LineError {
            source_line: MAX_PHYSICAL_LINES,
            detail: "Make input exceeds physical line limit",
        });
    }

    let mut logical_lines = Vec::with_capacity(physical_lines);
    let mut next_start = 0;
    let mut source_line = 0;
    let mut define_depth = 0;
    let mut retained = 0usize;

    while next_start < input.len() {
        let line_start = next_start;
        let first_source_line = source_line;
        let first = physical(input, next_start);
        let first_is_tab = first.text.starts_with(b"\t");
        let recipe = first_is_tab && define_depth == 0;
        let mut logical = Vec::new();
        let mut current = first;
        let mut first_physical = true;

        loop {
            let slash_count = trailing_backslashes(current.text);
            let continued = current.has_newline && slash_count % 2 == 1;

            if recipe {
                let segment = if first_physical {
                    current.text
                } else {
                    current.text.strip_prefix(b"\t").unwrap_or(current.text)
                };
                logical.extend_from_slice(segment);
                if continued {
                    // Recipe continuations preserve the entire slash run and
                    // normalize CRLF's physical ending to a literal LF.
                    logical.push(b'\n');
                }
            } else {
                append_make_segment(
                    &mut logical,
                    current.text,
                    slash_count,
                    continued,
                    !first_physical,
                );
            }

            next_start = current.range_end;
            source_line += 1;
            if !continued || next_start == input.len() {
                break;
            }
            first_physical = false;
            current = physical(input, next_start);
        }

        if discard_comments && !recipe && define_depth == 0 {
            if let Some(comment) = make_comment_start(&logical) {
                let prefix = std::str::from_utf8(&logical[..comment]).map_err(|_| LineError {
                    source_line: first_source_line,
                    detail: "Make semantic text is not valid UTF-8",
                })?;
                if inline_recipe_before_comment(prefix, first_source_line)? {
                    // Once `;` starts an inline recipe, `#` belongs to the
                    // shell. Keep the whole byte sequence for strict decoding.
                } else {
                    logical.truncate(comment);
                }
            }
        }

        retained = retained
            .checked_add(logical.len())
            .filter(|size| *size <= input.len() && *size <= MAX_INPUT)
            .ok_or(LineError {
                source_line: first_source_line,
                detail: "Make logical-line output exceeds byte limit",
            })?;

        let logical = String::from_utf8(logical).map_err(|_| LineError {
            source_line: first_source_line,
            detail: "Make semantic text is not valid UTF-8",
        })?;

        if !first_is_tab {
            if let Some((directive, arguments)) = make_define_directive(&logical) {
                match directive {
                    "define" => {
                        define_depth += 1;
                        if define_depth > MAX_DEFINE_DEPTH {
                            return Err(LineError {
                                source_line: first_source_line,
                                detail: "Make define nesting limit",
                            });
                        }
                    }
                    "endef" if arguments.is_empty() && define_depth > 0 => {
                        define_depth -= 1;
                    }
                    _ => {}
                }
            }
        }

        logical_lines.push(LogicalLine {
            text: logical,
            source_line: first_source_line,
            source_range: line_start..next_start,
        });
    }

    Ok(logical_lines)
}

struct Physical<'a> {
    text: &'a [u8],
    range_end: usize,
    has_newline: bool,
}

fn physical(bytes: &[u8], start: usize) -> Physical<'_> {
    let mut end = start;
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    let has_newline = end < bytes.len();
    let range_end = end + usize::from(has_newline);
    let mut content_end = end;
    if has_newline && content_end > start && bytes[content_end - 1] == b'\r' {
        content_end -= 1;
    }
    Physical {
        text: &bytes[start..content_end],
        range_end,
        has_newline,
    }
}

fn trailing_backslashes(text: &[u8]) -> usize {
    text.iter().rev().take_while(|byte| **byte == b'\\').count()
}

fn append_make_segment(
    output: &mut Vec<u8>,
    segment: &[u8],
    slash_count: usize,
    continued: bool,
    follows_continuation: bool,
) {
    if !continued {
        output.extend_from_slice(if follows_continuation {
            trim_horizontal_start(segment)
        } else {
            segment
        });
        return;
    }

    let prefix_end = segment.len() - slash_count;
    let mut prefix = &segment[..prefix_end];
    if follows_continuation {
        prefix = trim_horizontal_start(prefix);
    }
    prefix = trim_horizontal_end(prefix);
    // Consecutive empty continuations share one separator. A nonempty next
    // segment (including a retained slash pair) still needs the prior space.
    if prefix.is_empty() && slash_count == 1 {
        while output
            .last()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
        {
            output.pop();
        }
    }
    output.extend_from_slice(prefix);
    for _ in 0..(slash_count / 2) {
        output.push(b'\\');
    }
    output.push(b' ');
}

fn trim_horizontal_start(text: &[u8]) -> &[u8] {
    let start = text
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(text.len());
    &text[start..]
}

fn trim_horizontal_end(text: &[u8]) -> &[u8] {
    let end = text
        .iter()
        .rposition(|byte| !matches!(byte, b' ' | b'\t'))
        .map_or(0, |index| index + 1);
    &text[..end]
}

fn inline_recipe_before_comment(prefix: &str, source_line: usize) -> Result<bool, LineError> {
    match parse_rule_header(prefix) {
        Ok(Some(RuleHeader::Rule {
            inline_recipe: Some(_),
            ..
        })) => Ok(true),
        Err(_) if prefix.as_bytes().contains(&b';') => Err(LineError {
            source_line,
            detail: "cannot safely classify comment after a possible inline recipe",
        }),
        Ok(_) | Err(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::{read, read_bytes, LogicalLine, MAX_DEFINE_DEPTH, MAX_INPUT, MAX_PHYSICAL_LINES};

    fn texts(input: &str) -> Vec<String> {
        read(input)
            .expect("input should lex")
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    #[test]
    fn ordinary_continuations_use_backslash_parity_and_fold_space() {
        assert_eq!(texts("X := a\\\nb\\\nc\n"), ["X := a b c"]);
        assert_eq!(texts("X := a\\\n\\\nc\n"), ["X := a c"]);
        assert_eq!(
            texts(
                "ONE:=left\\\n  right\nTHREE:=left\\\\\\\n  joined\nTWO:=left\\\\\nY:=separate\n"
            ),
            [
                "ONE:=left right",
                "THREE:=left\\ joined",
                "TWO:=left\\\\",
                "Y:=separate",
            ]
        );
    }

    #[test]
    fn recipe_continuations_preserve_slashes_newlines_and_spaces() {
        assert_eq!(
            texts("\tcmd  \\\n\t  arg  \\\n\t\tmore\n"),
            ["\tcmd  \\\n  arg  \\\n\tmore"]
        );
    }

    #[test]
    fn even_recipe_backslashes_do_not_capture_a_global_assignment() {
        assert_eq!(
            texts("\tcmd \\\\\nX:=global\n"),
            ["\tcmd \\\\", "X:=global"]
        );
    }

    #[test]
    fn tab_leading_define_body_uses_ordinary_folding() {
        let lines = read("define X\n\tbody \\\n\t  continued\nendef\n").unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["define X", "\tbody continued", "endef"]
        );
        assert_eq!(lines[1].source_line, 1);
    }

    #[test]
    fn crlf_positions_and_ranges_cover_the_input_exactly() {
        let input = "A=left\\\r\n  right\r\nB=x\r\n";
        let lines = read(input).unwrap();
        assert_eq!(
            lines,
            [
                LogicalLine {
                    text: "A=left right".into(),
                    source_line: 0,
                    source_range: 0..18,
                },
                LogicalLine {
                    text: "B=x".into(),
                    source_line: 2,
                    source_range: 18..23,
                },
            ]
        );
        let mut cursor = 0;
        for line in &lines {
            assert_eq!(line.source_range.start, cursor);
            cursor = line.source_range.end;
        }
        assert_eq!(cursor, input.len());
    }

    #[test]
    fn byte_reader_discards_only_unescaped_ordinary_comment_suffixes() {
        let input = b"A := valid # comment \xff\r\nB := escaped\\#literal\n";
        let lines = read_bytes(input).unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["A := valid ", "B := escaped\\#literal"]
        );
        assert_eq!(lines[0].source_line, 0);
        assert_eq!(lines[1].source_line, 1);
        let mut cursor = 0;
        for line in &lines {
            assert_eq!(line.source_range.start, cursor);
            cursor = line.source_range.end;
        }
        assert_eq!(cursor, input.len());

        // The compatibility reader remains a lossless UTF-8 text reader.
        assert_eq!(texts("A := valid # comment\n"), ["A := valid # comment"]);
    }

    #[test]
    fn byte_reader_refuses_invalid_semantic_escaped_hash_and_rule_bytes() {
        for input in [
            &b"A := \xff # discarded comment\n"[..],
            &b"A := escaped\\#\xff\n"[..],
            &b"target-\xff: prerequisite # discarded comment\n"[..],
            &b"A := value\0 # comment\n"[..],
        ] {
            assert!(read_bytes(input).is_err(), "accepted {input:?}");
        }
    }

    #[test]
    fn byte_reader_respects_hashes_inside_nested_make_references() {
        let input = concat!(
            "VALUE := $(subst #,X,a#b) # ignored comment\n",
            "$(info RESULT=$(subst #,X,a#b)) # ignored comment\n",
            "ESCAPED := value\\#data # ignored comment\n",
        )
        .as_bytes();
        let lines = read_bytes(input).unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            [
                "VALUE := $(subst #,X,a#b) ",
                "$(info RESULT=$(subst #,X,a#b)) ",
                "ESCAPED := value\\#data ",
            ]
        );

        // Hashes inside the balanced function reference are semantic bytes;
        // only the later top-level comment can hide invalid UTF-8.
        assert!(read_bytes(b"VALUE := $(subst #,X,a\xff#b) # comment\n").is_err());

        let after_reference = b"VALUE := $(subst #,X,a#b) # comment \xff\n";
        assert_eq!(
            read_bytes(after_reference).unwrap()[0].text,
            "VALUE := $(subst #,X,a#b) "
        );
    }

    #[test]
    fn byte_reader_keeps_hash_data_in_recipes_definitions_and_inline_recipes() {
        let input = concat!(
            "\tprintf '# recipe data'\n",
            "define OUTER\n",
            "# outer data\n",
            "  define INNER\n",
            "# nested data\n",
            "  endef\n",
            "endef\n",
            "target: prerequisite ; printf '# inline recipe data'\n",
        )
        .as_bytes();
        let lines = read_bytes(input).unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            [
                "\tprintf '# recipe data'",
                "define OUTER",
                "# outer data",
                "  define INNER",
                "# nested data",
                "  endef",
                "endef",
                "target: prerequisite ; printf '# inline recipe data'",
            ]
        );
        let mut cursor = 0;
        for line in &lines {
            assert_eq!(line.source_range.start, cursor);
            cursor = line.source_range.end;
        }
        assert_eq!(cursor, input.len());

        for input in [
            &b"\tcmd # recipe \xff\n"[..],
            &b"define BODY\n# body \xff\nendef\n"[..],
            &b"target: prerequisite ; echo # inline \xff\n"[..],
        ] {
            assert!(read_bytes(input).is_err(), "accepted {input:?}");
        }
    }

    #[test]
    fn byte_reader_preserves_crlf_continuations_and_physical_ranges() {
        let input = b"A := one\\\r\n  two # discarded \xff\r\n\tcmd \\\r\n\t  arg\r\n";
        let lines = read_bytes(input).unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["A := one two ", "\tcmd \\\n  arg"]
        );
        assert_eq!(lines[0].source_line, 0);
        assert_eq!(lines[1].source_line, 2);
        let mut cursor = 0;
        for line in &lines {
            assert_eq!(line.source_range.start, cursor);
            cursor = line.source_range.end;
        }
        assert_eq!(cursor, input.len());
    }

    #[test]
    fn byte_reader_enforces_input_line_and_define_depth_bounds() {
        assert!(read_bytes(&vec![b'x'; MAX_INPUT + 1]).is_err());
        assert!(read_bytes(&vec![b'\n'; MAX_PHYSICAL_LINES + 1]).is_err());

        let mut nested = Vec::new();
        for _ in 0..=MAX_DEFINE_DEPTH {
            nested.extend_from_slice(b"define X\n");
        }
        for _ in 0..=MAX_DEFINE_DEPTH {
            nested.extend_from_slice(b"endef\n");
        }
        assert!(read_bytes(&nested).is_err());
    }
}
