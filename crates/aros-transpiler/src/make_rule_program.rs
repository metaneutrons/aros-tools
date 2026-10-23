//! Source-ordered expansion of bounded Make rule-generating statements.
//!
//! The caller supplies the original program's independently evaluated branch
//! states and positional expression evaluator. This module expands no recipes
//! and admits no eval-produced global effects. Every resulting line remains
//! bound to its original source line, including all eval-generated rules.

use crate::make_definitions::MakeDefinitions;
use crate::make_expr::{standalone_expansion, validate_rule_eval_output, MakeRuleExpansion};
use crate::make_vars::{make_define_suppression, strip_make_comment};

const MAX_PROGRAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const MAX_DEFINITION_SCAN_BYTES: usize = 64 * 1024 * 1024;
const MAX_FRAGMENTS: usize = 16_384;
const MAX_OUTPUT_LINES: usize = 262_144;

/// One output line's original joined-program position and branch selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramLine {
    pub source_line: usize,
    pub active: Option<bool>,
    /// Global index into [`RuleProgram::fragments`], not a recipe identity.
    pub eval_fragment: Option<usize>,
}

/// A half-open output byte range generated at one original expansion site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleFragment {
    pub source_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// Original text with active rule-generating statements expanded in place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleProgram {
    pub text: String,
    pub lines: Vec<ProgramLine>,
    pub fragments: Vec<RuleFragment>,
}

/// Expand standalone top-level Make expression statements at their original sites.
///
/// Definitions and tab-prefixed recipes are data. Inactive statements are kept
/// unchanged and are not evaluated. The evaluator must use the original scope
/// at the supplied line, not final file-global values. Eval output may contain
/// only validated rule data; all global assignments/effects are refused. This
/// does not execute commands or establish full rule/source coverage.
///
/// # Errors
/// Rejects inconsistent states, unknown statement selection, evaluation errors,
/// unsupported output, malformed expressions or bounded-work/output exhaustion.
pub fn expand(
    program: &str,
    states: &[Option<bool>],
    evaluate: &impl Fn(&str, usize, &MakeDefinitions) -> Result<MakeRuleExpansion, String>,
) -> Result<RuleProgram, String> {
    if program.len() > MAX_PROGRAM_BYTES
        || states.len() > MAX_OUTPUT_LINES
        || states.len() != program.lines().count()
    {
        return Err("invalid Make rule-program input or limits".into());
    }
    let suppressed = make_define_suppression(program);
    let mut result = RuleProgram::default();
    let mut scan_bytes = 0usize;
    for (source_line, raw) in program.split_inclusive('\n').enumerate() {
        let visible = strip_make_comment(raw.trim_end_matches('\n')).trim();
        if suppressed[source_line]
            || raw.starts_with('\t')
            || states[source_line] == Some(false)
            || !standalone_expansion(visible).map_err(|error| error.to_string())?
        {
            append(&mut result, raw, source_line, states[source_line], None)?;
            continue;
        }
        if states[source_line] != Some(true) {
            return Err("Make rule statement has unresolved selection".into());
        }
        scan_bytes = scan_bytes
            .checked_add(program.len())
            .filter(|bytes| *bytes <= MAX_DEFINITION_SCAN_BYTES)
            .ok_or("Make rule program exceeds definition-scan work limit")?;
        let definitions = MakeDefinitions::collect_at(program, source_line, states)?;
        let expansion = evaluate(visible, source_line, &definitions)?;
        if expansion.value.len() > MAX_OUTPUT_BYTES
            || expansion.rules.len() > MAX_FRAGMENTS.saturating_sub(result.fragments.len())
        {
            return Err("Make rule statement exceeds output limits".into());
        }
        if !expansion.value.trim().is_empty() {
            return Err("Make rule statement produced unsupported top-level text".into());
        }
        let statement_start = result.text.len();
        for rules in expansion.rules {
            if result.fragments.len() == MAX_FRAGMENTS {
                return Err("Make rule program exceeds generated-fragment limit".into());
            }
            validate_rule_eval_output(&rules).map_err(|error| error.to_string())?;
            if rules.len() >= MAX_OUTPUT_BYTES {
                return Err("Make rule program exceeds fragment byte limit".into());
            }
            let index = result.fragments.len();
            let start_byte = result.text.len();
            let rules = if rules.ends_with('\n') {
                rules
            } else {
                format!("{rules}\n")
            };
            if rules
                .lines()
                .all(|line| strip_make_comment(line).trim().is_empty())
            {
                // Empty/comment-only eval has no rule identity. Retain its
                // source mapping without claiming a generated-rule fragment.
                append(&mut result, &rules, source_line, Some(true), None)?;
                continue;
            }
            append(&mut result, &rules, source_line, Some(true), Some(index))?;
            result.fragments.push(RuleFragment {
                source_line,
                start_byte,
                end_byte: result.text.len(),
            });
        }
        if result.text.len() == statement_start {
            // Preserve an empty source site; do not let adjacent lines join.
            append(&mut result, "\n", source_line, Some(true), None)?;
        }
    }
    if result.lines.len() != result.text.lines().count() {
        return Err("Make rule program has inconsistent output line origins".into());
    }
    Ok(result)
}

fn append(
    program: &mut RuleProgram,
    text: &str,
    source_line: usize,
    active: Option<bool>,
    eval_fragment: Option<usize>,
) -> Result<(), String> {
    if program.text.len().saturating_add(text.len()) > MAX_OUTPUT_BYTES
        || program.lines.len().saturating_add(text.lines().count()) > MAX_OUTPUT_LINES
    {
        return Err("Make rule program exceeds output byte or line limit".into());
    }
    program.text.push_str(text);
    program.lines.extend(text.lines().map(|_| ProgramLine {
        source_line,
        active,
        eval_fragment,
    }));
    Ok(())
}

#[cfg(test)]
#[path = "make_rule_program_tests.rs"]
mod tests;
