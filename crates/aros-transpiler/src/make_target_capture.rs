//! Source-position capture of immediate target-local Make variables.
//!
//! This is one recipe primitive, not complete Make or compiler projection.
//! The caller supplies independently evaluated conditional states and a bounded
//! expression evaluator at each original joined line. Eval-generated recipes,
//! pattern variables and target-variable inheritance are not implemented here.

use crate::make_vars::{
    literal_variable_name, make_define_suppression, strip_make_comment, variable_assignment,
    AssignmentKind,
};
use std::collections::{BTreeMap, BTreeSet};

const MAX_PROGRAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_TARGETS: usize = 16384;
const MAX_ASSIGNMENTS: usize = 262_144;
const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;

/// One frozen target-local value at its zero-based original joined line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetAssignment {
    pub line: usize,
    pub target: String,
    pub variable: String,
    pub value: String,
}

/// Current target-local values and their ordered capture history.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetCapture {
    pub targets: BTreeMap<String, BTreeMap<String, String>>,
    pub assignments: Vec<TargetAssignment>,
}

/// Capture selected `target-list : VARIABLE := expression` assignments.
///
/// `program` is continuation-joined; `states` must contain one entry per line
/// (`Some(true)` active, `Some(false)` inactive, `None` unproved). The evaluator
/// resolves target lists with an empty target overlay and each RHS with that
/// particular target's earlier frozen values. Overlay values are literal
/// expanded strings, not recursive Make definitions. It must not replace a
/// missing value with an inferred empty value. No processes are executed here.
/// All target-local assignments participate in evaluation, including variables
/// not selected for output: they may shadow a global used by a selected RHS.
/// Unmodelled target-local assignment flavors therefore refuse the operation,
/// rather than silently dropping an unselected dependency.
///
/// # Errors
/// Rejects unresolved selection, unsupported selected assignment flavor,
/// patterns/unsafe/duplicate targets, malformed assignment syntax or limits.
/// Successful capture alone does not establish complete recipe/source coverage.
pub fn capture(
    program: &str,
    selected_variables: &BTreeSet<String>,
    states: &[Option<bool>],
    evaluate: &impl Fn(&str, usize, &BTreeMap<String, String>) -> Result<String, String>,
) -> Result<TargetCapture, String> {
    if program.len() > MAX_PROGRAM_BYTES
        || selected_variables.len() > 64
        || selected_variables
            .iter()
            .any(|name| !literal_variable_name(name))
        || states.len() != program.lines().count()
    {
        return Err("invalid Make target-capture input or limits".into());
    }
    let suppressed = make_define_suppression(program);
    let mut captured = TargetCapture::default();
    let mut target_values = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut processed_assignments = 0usize;
    let mut captured_bytes = 0usize;
    let empty = BTreeMap::new();
    for (line, raw) in program.lines().enumerate() {
        if suppressed[line] || raw.starts_with('\t') || states[line] == Some(false) {
            continue;
        }
        let visible = strip_make_comment(raw).trim();
        if visible.starts_with(".RECIPEPREFIX") {
            return Err("Make target capture does not admit a custom recipe prefix".into());
        }
        if variable_assignment(visible).is_some() {
            continue;
        }
        let Some(crate::make_rule_header::RuleHeader::TargetAssignment {
            targets,
            assignment,
        }) = crate::make_rule_header::parse(raw)?
        else {
            continue;
        };
        let Some((name, expression, kind)) = variable_assignment(&assignment) else {
            if assignment.contains('=') {
                return Err("unsupported Make target-specific assignment syntax".into());
            }
            continue;
        };
        if states[line] != Some(true) {
            return Err("Make target-specific assignment has unresolved selection".into());
        }
        if !matches!(kind, AssignmentKind::SimpleSet) {
            return Err("Make target capture requires an immediate target assignment".into());
        }
        let targets = evaluate(&targets, line, &empty)?;
        if targets.len() > MAX_PROGRAM_BYTES {
            return Err("Make target capture exceeds target-list byte limit".into());
        }
        let mut seen = BTreeSet::new();
        for target in targets.split_whitespace() {
            if !safe_target(target) || !seen.insert(target) {
                return Err(
                    "Make target capture has an unsafe, pattern or duplicate target".into(),
                );
            }
            if !target_values.contains_key(target) && target_values.len() == MAX_TARGETS {
                return Err("Make target capture exceeds target limit".into());
            }
            if processed_assignments == MAX_ASSIGNMENTS {
                return Err("Make target capture exceeds assignment limit".into());
            }
            processed_assignments += 1;
            let overlay = target_values.entry(target.to_owned()).or_default();
            let value = evaluate(expression, line, overlay)?;
            if value.len() > MAX_PROGRAM_BYTES {
                return Err("Make target capture exceeds value limit".into());
            }
            captured_bytes = captured_bytes
                .checked_add(value.len().saturating_mul(3))
                .and_then(|bytes| bytes.checked_add((target.len() + name.len()).saturating_mul(3)))
                .filter(|bytes| *bytes <= MAX_CAPTURE_BYTES)
                .ok_or("Make target capture exceeds retained byte limit")?;
            if selected_variables.contains(name) {
                captured.assignments.push(TargetAssignment {
                    line,
                    target: target.to_owned(),
                    variable: name.to_owned(),
                    value: value.clone(),
                });
                captured
                    .targets
                    .entry(target.to_owned())
                    .or_default()
                    .insert(name.to_owned(), value.clone());
            }
            overlay.insert(name.to_owned(), value);
        }
    }
    Ok(captured)
}

fn safe_target(target: &str) -> bool {
    !target.is_empty()
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+/".contains(&byte))
        && target.split('/').all(|part| part != "..")
}

#[cfg(test)]
#[path = "make_target_capture_tests.rs"]
mod tests;
