//! Source-local projection of ordinary Make directory prerequisites.
//!
//! This collector recognizes a dependency-only named target and its matching
//! directory rule only when the directory list is explicitly resolvable from
//! the source/configuration scope and every resulting path stays below an
//! admitted generated or include root. It does not create a global `setup`
//! owner, inspect existing output directories, or execute the source recipe.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_list, MakeExprContext};
use crate::make_vars::{
    strip_make_comment, undefine_directive, variable_assignment, AssignmentKind, ConditionalTruth,
    VarScope,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINES: usize = 16_384;
const MAX_DIRECTORIES: usize = 4_096;
const MAX_REFERENCE_DEPTH: usize = 32;

/// A file-local named target whose sole represented effect is requiring a
/// finite set of SDK/build directories to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDirectoryGroupDecl {
    /// Ordinary Make owner, local to the source file that declared it.
    pub owner: String,
    /// One-based source line of the dependency-only owner declaration.
    pub source_line: usize,
    /// One-based source line of the matching directory creation rule.
    pub directory_rule_line: usize,
    /// Ordered CMake paths, each rooted below an admitted configured root.
    pub directories: Vec<String>,
}

/// A candidate source-local directory group that was not completely proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDirectoryGroupRejection {
    /// Best-effort ordinary target name, or the raw dynamic target spelling.
    pub owner: String,
    /// One-based source line anchoring the rejection.
    pub source_line: usize,
    /// Why the source pattern was outside the bounded representation.
    pub reason: String,
}

#[derive(Debug)]
struct LogicalLine {
    text: String,
    source_line: usize,
}

#[derive(Debug)]
struct RecipeLine {
    text: String,
    source_line: usize,
    state: ConditionalTruth,
}

#[derive(Debug)]
struct Rule {
    targets: String,
    prerequisites: String,
    source_line: usize,
    state: ConditionalTruth,
    recipes: Vec<RecipeLine>,
}

#[derive(Debug, Default)]
struct ParseResult {
    rules: Vec<Rule>,
    assignments: BTreeMap<String, Vec<SourceAssignment>>,
    malformed_conditionals: bool,
    unsupported_global_controls: Vec<(usize, String)>,
}

#[derive(Debug)]
struct SourceAssignment {
    source_line: usize,
    rhs: String,
    state: ConditionalTruth,
    simple_expansion: bool,
    is_undefine: bool,
}

#[derive(Debug)]
struct ExplicitValue {
    value: String,
    expansion_line: usize,
}

/// Collects the finite source-local directory prerequisite pattern.
///
/// `snapshot` is the same continuation-joined Make text used to build
/// `scope`; `states` uses its zero-based logical-line positions. A missing or
/// short state map is not treated as unconditional. The returned owner is
/// file-local identity only: callers must attach the directory dependency to
/// an already-proven consumer rather than publish a synthetic global target.
#[must_use]
pub fn collect_source_directory_groups(
    snapshot: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    states: &[ConditionalTruth],
) -> (
    Vec<SourceDirectoryGroupDecl>,
    Vec<SourceDirectoryGroupRejection>,
) {
    if snapshot.len() > MAX_SNAPSHOT_BYTES {
        return (
            Vec::new(),
            vec![rejection(
                "setup",
                1,
                "Make snapshot exceeds its byte limit".into(),
            )],
        );
    }
    let lines = logical_lines(snapshot);
    if lines.len() > MAX_LINES {
        return (
            Vec::new(),
            vec![rejection(
                "setup",
                1,
                "Make snapshot exceeds its logical-line limit".into(),
            )],
        );
    }

    let parsed = parse_rules(&lines, states);
    let setup_rules = parsed
        .rules
        .iter()
        .filter(|rule| mentions_target(&rule.targets, "setup"))
        .collect::<Vec<_>>();
    let directory_rules = parsed
        .rules
        .iter()
        .filter(|rule| {
            rule.targets.contains("$(INCL_DIRS)") || rule.targets.contains("${INCL_DIRS}")
        })
        .collect::<Vec<_>>();

    if setup_rules.is_empty() && directory_rules.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut rejections = Vec::new();
    if parsed.malformed_conditionals {
        let anchor = setup_rules
            .first()
            .or_else(|| directory_rules.first())
            .map_or(1, |rule| rule.source_line + 1);
        rejections.push(rejection(
            "setup",
            anchor,
            "malformed or unclosed Make conditional controls affect the source snapshot".into(),
        ));
        return (Vec::new(), rejections);
    }

    if let Some((line, reason)) = parsed.unsupported_global_controls.first() {
        let candidate_line = setup_rules
            .first()
            .or_else(|| directory_rules.first())
            .map_or(*line, |rule| rule.source_line + 1);
        rejections.push(rejection(
            "setup",
            candidate_line,
            format!(
                "source contains an unmodeled Make control at line {}: {reason}",
                line + 1
            ),
        ));
        return (Vec::new(), rejections);
    }

    let active_setup = setup_rules
        .into_iter()
        .filter(|rule| rule.state != ConditionalTruth::False)
        .collect::<Vec<_>>();
    let active_directories = directory_rules
        .into_iter()
        .filter(|rule| rule.state != ConditionalTruth::False)
        .collect::<Vec<_>>();

    if active_setup.len() != 1 {
        let anchor = active_setup
            .first()
            .or_else(|| active_directories.first())
            .map_or(1, |rule| rule.source_line + 1);
        rejections.push(rejection(
            "setup",
            anchor,
            format!(
                "expected one active literal dependency-only setup rule, found {}",
                active_setup.len()
            ),
        ));
        return (Vec::new(), rejections);
    }
    if active_directories.len() != 1 {
        rejections.push(rejection(
            "setup",
            active_setup[0].source_line + 1,
            format!(
                "expected one active $(INCL_DIRS) directory rule, found {}",
                active_directories.len()
            ),
        ));
        return (Vec::new(), rejections);
    }

    let setup = active_setup[0];
    let directory_rule = active_directories[0];
    macro_rules! reject {
        ($owner:expr, $line:expr, $reason:expr $(,)?) => {
            rejections.push(rejection($owner, $line + 1, $reason))
        };
    }

    if setup.state != ConditionalTruth::True {
        reject!(
            "setup",
            setup.source_line,
            "setup rule is conditional or its state is unresolved".into(),
        );
    }
    if directory_rule.state != ConditionalTruth::True {
        reject!(
            "setup",
            directory_rule.source_line,
            "directory rule is conditional or its state is unresolved".into(),
        );
    }
    if setup.targets.trim() != "setup" || setup.prerequisites.trim() != "$(INCL_DIRS)" {
        reject!(
            "setup",
            setup.source_line,
            "owner must be exactly `setup : $(INCL_DIRS)` with no extra targets, prerequisites, or target-specific controls".into(),
        );
    }
    if !setup.recipes.is_empty() {
        reject!(
            "setup",
            setup.source_line,
            "setup aggregate must be dependency-only and have no recipe".into(),
        );
    }
    if directory_rule.targets.trim() != "$(INCL_DIRS)"
        || !directory_rule.prerequisites.trim().is_empty()
    {
        reject!(
            "setup",
            directory_rule.source_line,
            "directory rule must target only `$(INCL_DIRS)` and have no prerequisites".into(),
        );
    }
    if directory_rule.recipes.len() == 2 {
        let recipe_states_are_true = directory_rule
            .recipes
            .iter()
            .all(|recipe| recipe.state == ConditionalTruth::True);
        if !recipe_states_are_true {
            reject!(
                "setup",
                directory_rule
                    .recipes
                    .iter()
                    .find(|recipe| recipe.state != ConditionalTruth::True)
                    .map_or(directory_rule.source_line, |recipe| recipe.source_line),
                "directory recipe is conditional or its state is unresolved".into(),
            );
        }
        let first = directory_rule.recipes[0].text.trim();
        let second = directory_rule.recipes[1].text.trim();
        if first != "@$(ECHO) \"Creating   $@...\"" || second != "@$(MKDIR) $@" {
            reject!(
                "setup",
                directory_rule.source_line,
                "directory recipe must be exactly the literal informational ECHO followed by `@$(MKDIR) $@`".into(),
            );
        }
    } else {
        reject!(
            "setup",
            directory_rule.source_line,
            format!(
                "directory rule must contain exactly the echo and MKDIR recipes, found {}",
                directory_rule.recipes.len()
            ),
        );
    }

    if !rejections.is_empty() {
        return (Vec::new(), rejections);
    }

    let required_line = setup.source_line;
    let context = MakeExprContext::new(scope, dirs, required_line, root, rel_dir);
    let setup_directories = match evaluate_directory_list(
        "$(INCL_DIRS)",
        scope,
        dirs,
        &parsed.assignments,
        &context,
        required_line,
    ) {
        Ok(directories) => directories,
        Err(reason) => {
            reject!("setup", setup.source_line, reason);
            return (Vec::new(), rejections);
        }
    };

    let directory_line = directory_rule.source_line;
    let directory_context = MakeExprContext::new(scope, dirs, directory_line, root, rel_dir);
    let rule_directories = match evaluate_directory_list(
        "$(INCL_DIRS)",
        scope,
        dirs,
        &parsed.assignments,
        &directory_context,
        directory_line,
    ) {
        Ok(directories) => directories,
        Err(reason) => {
            reject!("setup", directory_rule.source_line, reason);
            return (Vec::new(), rejections);
        }
    };

    if setup_directories != rule_directories {
        reject!(
            "setup",
            directory_rule.source_line,
            "`$(INCL_DIRS)` resolves differently at the setup and directory rule declarations"
                .into(),
        );
        return (Vec::new(), rejections);
    }
    if setup_directories.is_empty() {
        reject!(
            "setup",
            setup.source_line,
            "directory aggregate resolves to an empty list".into(),
        );
        return (Vec::new(), rejections);
    }
    if setup_directories.len() > MAX_DIRECTORIES {
        reject!(
            "setup",
            setup.source_line,
            format!("directory aggregate exceeds {MAX_DIRECTORIES} entries"),
        );
        return (Vec::new(), rejections);
    }

    let mut seen = BTreeSet::new();
    for directory in &setup_directories {
        if !is_contained_configured_directory(directory) {
            reject!(
                "setup",
                setup.source_line,
                format!(
                    "directory {directory:?} is not below an admitted configured build/include root"
                ),
            );
            return (Vec::new(), rejections);
        }
        let identity = directory.to_ascii_lowercase();
        if !seen.insert(identity) {
            reject!(
                "setup",
                setup.source_line,
                format!("directory aggregate repeats or case-fold-collides at {directory:?}"),
            );
            return (Vec::new(), rejections);
        }
    }

    (
        vec![SourceDirectoryGroupDecl {
            owner: "setup".into(),
            source_line: setup.source_line + 1,
            directory_rule_line: directory_rule.source_line + 1,
            directories: setup_directories,
        }],
        rejections,
    )
}

fn evaluate_directory_list(
    expression: &str,
    scope: &VarScope,
    dirs: &DirVars,
    assignments: &BTreeMap<String, Vec<SourceAssignment>>,
    context: &MakeExprContext<'_>,
    line: usize,
) -> Result<Vec<String>, String> {
    let effective = scope.raw_at("INCL_DIRS", line).ok_or_else(|| {
        "`INCL_DIRS` is not explicitly defined at the directory declaration".to_owned()
    })?;
    if scope.conditionally_assigned_before("INCL_DIRS", line) {
        return Err("`INCL_DIRS` depends on an unresolved conditional or opaque assignment".into());
    }
    if let Some(reason) = scope.flavor_uncertainty_reason_at("INCL_DIRS", line) {
        return Err(format!("`INCL_DIRS` has uncertain Make flavor: {reason}"));
    }
    let source_value = explicit_value_at("INCL_DIRS", line, assignments, scope)?
        .ok_or_else(|| "`INCL_DIRS` has no explicit source or configuration value".to_owned())?;
    let mut variable_sets = (BTreeSet::new(), BTreeSet::new());
    validate_expression_provenance(
        &source_value.value,
        scope,
        dirs,
        assignments,
        source_value.expansion_line,
        &mut variable_sets,
        0,
    )?;
    let values = evaluate_make_list(expression, context)
        .map_err(|error| format!("cannot resolve explicit directory aggregate: {error}"))?;
    if effective.is_empty() && !values.is_empty() {
        return Err("`INCL_DIRS` source value differs from its effective Make value".into());
    }
    if values.len() > MAX_DIRECTORIES {
        return Err(format!(
            "directory aggregate exceeds {MAX_DIRECTORIES} entries"
        ));
    }
    Ok(values)
}

fn validate_expression_provenance(
    expression: &str,
    scope: &VarScope,
    dirs: &DirVars,
    assignments: &BTreeMap<String, Vec<SourceAssignment>>,
    line: usize,
    variable_sets: &mut (BTreeSet<String>, BTreeSet<String>),
    depth: usize,
) -> Result<(), String> {
    if depth >= MAX_REFERENCE_DEPTH {
        return Err("directory expression exceeds its variable-reference depth limit".into());
    }
    let mut cursor = 0usize;
    let bytes = expression.as_bytes();
    while cursor < bytes.len() {
        if bytes.get(cursor) != Some(&b'$') || !matches!(bytes.get(cursor + 1), Some(b'(' | b'{')) {
            cursor += 1;
            continue;
        }
        let Some((body_start, body_end, next)) = reference_bounds(bytes, cursor) else {
            return Err("directory expression has an unterminated Make reference".into());
        };
        let body = &expression[body_start..body_end];
        let head_end = body
            .find(|character: char| character.is_ascii_whitespace() || character == ',')
            .unwrap_or(body.len());
        let head = &body[..head_end];
        let tail = body[head_end..].trim_start();

        if matches!(
            head,
            "wildcard" | "shell" | "file" | "guile" | "eval" | "call"
        ) {
            return Err(format!(
                "directory expression uses non-source-bounded Make function `{head}`"
            ));
        }
        if head == "foreach" {
            let arguments = split_top_level_commas(tail)?;
            if arguments.len() != 3 {
                return Err("directory foreach must have exactly three arguments".into());
            }
            let variable = arguments[0];
            let list = arguments[1];
            let body_expression = arguments[2];
            let variable = variable.trim();
            if !safe_variable_name(variable) {
                return Err("directory foreach requires one literal iterator name".into());
            }
            validate_expression_provenance(
                list,
                scope,
                dirs,
                assignments,
                line,
                &mut *variable_sets,
                depth + 1,
            )?;
            let inserted = variable_sets.0.insert(variable.to_owned());
            let result = validate_expression_provenance(
                body_expression,
                scope,
                dirs,
                assignments,
                line,
                &mut *variable_sets,
                depth + 1,
            );
            if inserted {
                variable_sets.0.remove(variable);
            }
            result?;
        } else if tail.is_empty() {
            if !safe_variable_name(head) {
                return Err(format!(
                    "directory expression has unsupported reference `{body}`"
                ));
            }
            if !variable_sets.0.contains(head) {
                if scope.conditionally_assigned_before(head, line) {
                    return Err(format!(
                        "directory variable `{head}` depends on an unresolved conditional assignment"
                    ));
                }
                if let Some(reason) = scope.flavor_uncertainty_reason_at(head, line) {
                    return Err(format!(
                        "directory variable `{head}` has uncertain Make flavor: {reason}"
                    ));
                }
                if let Some(value) = explicit_value_at(head, line, assignments, scope)? {
                    if !variable_sets.1.insert(head.to_owned()) {
                        return Err(format!("directory variable cycle through `{head}`"));
                    }
                    let result = validate_expression_provenance(
                        &value.value,
                        scope,
                        dirs,
                        assignments,
                        value.expansion_line,
                        &mut *variable_sets,
                        depth + 1,
                    );
                    variable_sets.1.remove(head);
                    result?;
                } else if dirs.expand(&format!("$({head})")).is_none() {
                    return Err(format!("directory variable `{head}` has no explicit value"));
                }
            }
        } else {
            // Let the shared evaluator decide whether a pure list function is
            // supported, but inspect all nested variable references first.
            validate_expression_provenance(
                tail,
                scope,
                dirs,
                assignments,
                line,
                &mut *variable_sets,
                depth + 1,
            )?;
        }
        cursor = next;
    }
    Ok(())
}

fn explicit_value_at(
    name: &str,
    line: usize,
    assignments: &BTreeMap<String, Vec<SourceAssignment>>,
    scope: &VarScope,
) -> Result<Option<ExplicitValue>, String> {
    let prior = assignments
        .get(name)
        .into_iter()
        .flatten()
        .filter(|assignment| assignment.source_line < line)
        .collect::<Vec<_>>();
    if prior
        .iter()
        .any(|assignment| assignment.state == ConditionalTruth::Unknown)
    {
        return Err(format!(
            "directory variable `{name}` has a definition under an unresolved conditional"
        ));
    }
    let active = prior
        .into_iter()
        .filter(|assignment| assignment.state == ConditionalTruth::True)
        .collect::<Vec<_>>();
    if active.len() > 1 {
        return Err(format!(
            "directory variable `{name}` is reassigned; override order is outside this model"
        ));
    }
    if let Some(assignment) = active.first() {
        if assignment.is_undefine {
            return Err(format!(
                "directory variable `{name}` is explicitly undefined"
            ));
        }
        if !assignment.simple_expansion {
            return Err(format!(
                "directory variable `{name}` is not assigned with a single simple `:=` value"
            ));
        }
        return Ok(Some(ExplicitValue {
            value: assignment.rhs.clone(),
            expansion_line: assignment.source_line,
        }));
    }
    Ok(scope.raw_at(name, line).map(|value| ExplicitValue {
        value,
        expansion_line: line,
    }))
}

fn split_top_level_commas(text: &str) -> Result<Vec<&str>, String> {
    let bytes = text.as_bytes();
    let mut arguments = Vec::new();
    let mut start = 0usize;
    let mut stack = Vec::<u8>::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && matches!(bytes.get(cursor + 1), Some(b'(' | b'{')) {
            stack.push(if bytes[cursor + 1] == b'(' {
                b')'
            } else {
                b'}'
            });
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b'(' && stack.last() == Some(&b')') {
            stack.push(b')');
        } else if stack.last() == Some(&bytes[cursor]) {
            stack.pop();
        } else if bytes[cursor] == b',' && stack.is_empty() {
            arguments.push(&text[start..cursor]);
            start = cursor + 1;
        }
        cursor += 1;
    }
    if !stack.is_empty() {
        return Err("directory foreach has unbalanced nested references".into());
    }
    arguments.push(&text[start..]);
    Ok(arguments)
}

fn reference_bounds(bytes: &[u8], start: usize) -> Option<(usize, usize, usize)> {
    let opener = *bytes.get(start + 1)?;
    let mut stack = vec![if opener == b'(' { b')' } else { b'}' }];
    let body_start = start + 2;
    let mut cursor = body_start;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && matches!(bytes.get(cursor + 1), Some(b'(' | b'{')) {
            stack.push(if bytes[cursor + 1] == b'(' {
                b')'
            } else {
                b'}'
            });
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b'(' && stack.last() == Some(&b')') {
            stack.push(b')');
        } else if bytes[cursor] == *stack.last()? {
            stack.pop();
            if stack.is_empty() {
                return Some((body_start, cursor, cursor + 1));
            }
        }
        cursor += 1;
    }
    None
}

fn safe_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn is_contained_configured_directory(value: &str) -> bool {
    const ROOTS: &[&str] = &[
        "${AROS_BUILD_DIR}",
        "${AROS_GENINC_DIR}",
        "${AROS_DEVELOPER_INCLUDE_DIR}",
        "${AROS_SDK_INCLUDE_DIR}",
    ];
    if value.contains([';', '"', '\'', '\\']) {
        return false;
    }
    let Some(root) = ROOTS.iter().find(|root| {
        value == **root
            || value
                .strip_prefix(**root)
                .is_some_and(|suffix| suffix.starts_with('/'))
    }) else {
        return false;
    };
    let suffix = value.strip_prefix(*root).unwrap_or_default();
    if suffix.is_empty() {
        return true;
    }
    suffix.strip_prefix('/').is_some_and(|path| {
        !path.is_empty()
            && path.split('/').all(|component| {
                !component.is_empty()
                    && component != "."
                    && component != ".."
                    && component
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
            })
    })
}

fn parse_rules(lines: &[LogicalLine], states: &[ConditionalTruth]) -> ParseResult {
    #[derive(Debug)]
    struct ConditionalFrame {
        saw_else: bool,
    }
    let mut result = ParseResult::default();
    let mut current: Option<Rule> = None;
    let mut conditionals = Vec::<ConditionalFrame>::new();
    let mut define_depth = 0usize;
    let mut metamake_define_depth = 0usize;

    for logical in lines {
        let source_line = logical.source_line;
        let line_state = states
            .get(source_line)
            .copied()
            .unwrap_or(ConditionalTruth::Unknown);
        let define = (!logical.text.starts_with('\t'))
            .then(|| define_directive(&logical.text))
            .flatten();
        if define_depth > 0 {
            match define {
                Some(("define", _)) => define_depth = define_depth.saturating_add(1),
                Some(("endef", "")) => define_depth -= 1,
                _ => {}
            }
            continue;
        }
        if matches!(define, Some(("define", _))) {
            if let Some(rule) = current.take() {
                result.rules.push(rule);
            }
            define_depth = 1;
            continue;
        }

        let uncommented = strip_make_comment(logical.text.trim_start()).trim();
        if metamake_define_depth > 0 {
            if uncommented.starts_with("%define") {
                metamake_define_depth = metamake_define_depth.saturating_add(1);
            } else if uncommented == "%end" {
                metamake_define_depth -= 1;
            }
            continue;
        }
        if uncommented.starts_with("%define")
            && (uncommented.len() == 7
                || uncommented
                    .as_bytes()
                    .get(7)
                    .is_some_and(u8::is_ascii_whitespace))
        {
            if let Some(rule) = current.take() {
                result.rules.push(rule);
            }
            metamake_define_depth = 1;
            continue;
        }

        if logical.text.starts_with('\t') {
            if let Some(rule) = current.as_mut() {
                if line_state != ConditionalTruth::False {
                    rule.recipes.push(RecipeLine {
                        text: logical.text.trim_start_matches('\t').to_owned(),
                        source_line,
                        state: line_state,
                    });
                }
            }
            continue;
        }

        let header = uncommented;
        if header.is_empty() || header.starts_with('#') {
            continue;
        }
        if let Some(word) = header.split_whitespace().next() {
            match word {
                "ifeq" | "ifneq" | "ifdef" | "ifndef" => {
                    conditionals.push(ConditionalFrame { saw_else: false });
                    continue;
                }
                "else" => {
                    if header != "else" || conditionals.last().is_none_or(|frame| frame.saw_else) {
                        result.malformed_conditionals = true;
                    } else if let Some(frame) = conditionals.last_mut() {
                        frame.saw_else = true;
                    }
                    continue;
                }
                "endif" => {
                    if header != "endif" || conditionals.pop().is_none() {
                        result.malformed_conditionals = true;
                    }
                    continue;
                }
                _ => {}
            }
        }

        if has_make_function(header, "eval") {
            result.unsupported_global_controls.push((
                source_line,
                "dynamic `eval` can synthesize or override Make rules".into(),
            ));
        }
        if is_include_directive(header) {
            result.unsupported_global_controls.push((
                source_line,
                "unresolved Make include can alter the directory aggregate".into(),
            ));
        }
        if is_modified_assignment(header) {
            result.unsupported_global_controls.push((
                source_line,
                "modified Make assignment is outside the source value model".into(),
            ));
        }
        if variable_assignment(header).is_some_and(|(name, _, _)| name == "SHELL") {
            result.unsupported_global_controls.push((
                source_line,
                "source changes the shell used to run the directory recipe".into(),
            ));
        }
        if is_target_specific_assignment(header) {
            result.unsupported_global_controls.push((
                source_line,
                "target-specific assignment may override directory recipe semantics".into(),
            ));
        }
        if is_shell_semantics_control(header) {
            result.unsupported_global_controls.push((
                source_line,
                "special Make target changes recipe execution semantics".into(),
            ));
        }
        if header.starts_with(".RECIPEPREFIX") {
            result.unsupported_global_controls.push((
                source_line,
                "custom .RECIPEPREFIX makes recipe boundaries unprovable".into(),
            ));
        }

        if let Some((name, rhs, kind)) = variable_assignment(header) {
            if matches!(name, "ECHO" | "MKDIR") {
                result.unsupported_global_controls.push((
                    source_line,
                    format!("source redefines directory recipe command `{name}`"),
                ));
            }
            result
                .assignments
                .entry(name.to_owned())
                .or_default()
                .push(SourceAssignment {
                    source_line,
                    rhs: rhs.to_owned(),
                    state: line_state,
                    simple_expansion: kind == AssignmentKind::SimpleSet,
                    is_undefine: false,
                });
            continue;
        }
        if let Ok(Some(name)) = undefine_directive(header) {
            result
                .assignments
                .entry(name.to_owned())
                .or_default()
                .push(SourceAssignment {
                    source_line,
                    rhs: String::new(),
                    state: line_state,
                    simple_expansion: false,
                    is_undefine: true,
                });
            continue;
        }

        if let Some(rule) = current.take() {
            result.rules.push(rule);
        }
        if let Some((targets, prerequisites)) = header.split_once(':') {
            if targets.trim().is_empty() || prerequisites.starts_with(':') {
                continue;
            }
            result.rules.push(Rule {
                targets: targets.trim().to_owned(),
                prerequisites: prerequisites.trim().to_owned(),
                source_line,
                state: line_state,
                recipes: Vec::new(),
            });
            // The current rule stays live so following tab recipes attach to it.
            current = result.rules.pop();
        }
    }
    if let Some(rule) = current {
        result.rules.push(rule);
    }
    if !conditionals.is_empty() {
        result.malformed_conditionals = true;
    }
    result
}

fn mentions_target(targets: &str, wanted: &str) -> bool {
    targets.split_whitespace().any(|target| target == wanted)
}

fn is_include_directive(line: &str) -> bool {
    ["include", "-include", "sinclude"].iter().any(|directive| {
        line.strip_prefix(directive)
            .is_some_and(|tail| tail.starts_with(char::is_whitespace))
    })
}

fn is_modified_assignment(line: &str) -> bool {
    let mut words = line.splitn(2, char::is_whitespace);
    let first = words.next().unwrap_or_default();
    let rest = words.next().unwrap_or_default();
    matches!(first, "override" | "export" | "private")
        && crate::make_vars::variable_assignment(rest).is_some()
}

fn is_target_specific_assignment(line: &str) -> bool {
    let Some((_, rhs)) = line.split_once(':') else {
        return false;
    };
    variable_assignment(rhs).is_some() || is_modified_assignment(rhs.trim())
}

fn is_shell_semantics_control(line: &str) -> bool {
    line.starts_with(".SHELLFLAGS")
        || [".ONESHELL", ".POSIX"].iter().any(|target| {
            line == *target
                || line
                    .strip_prefix(target)
                    .is_some_and(|tail| tail.trim_start().starts_with(':'))
        })
}

fn has_make_function(line: &str, wanted: &str) -> bool {
    let bytes = line.as_bytes();
    let mut cursor = 0usize;
    while cursor + 2 <= bytes.len() {
        if bytes[cursor] == b'$' && matches!(bytes.get(cursor + 1), Some(b'(' | b'{')) {
            let mut end = cursor + 2;
            while end < bytes.len()
                && !bytes[end].is_ascii_whitespace()
                && bytes[end] != b','
                && !matches!(bytes[end], b')' | b'}')
            {
                end += 1;
            }
            if &line[cursor + 2..end] == wanted {
                return true;
            }
        }
        cursor += 1;
    }
    false
}

fn define_directive(line: &str) -> Option<(&'static str, &str)> {
    let line = strip_make_comment(line.trim_start()).trim_start();
    let mut words = line.splitn(2, char::is_whitespace);
    let mut word = words.next().unwrap_or_default();
    let mut rest = words.next().unwrap_or_default().trim();
    let mut modifiers = 0usize;
    while matches!(word, "override" | "export" | "private") && modifiers < 3 {
        modifiers += 1;
        let mut parts = rest.splitn(2, char::is_whitespace);
        word = parts.next().unwrap_or_default();
        rest = parts.next().unwrap_or_default().trim();
    }
    if word == "define" {
        Some(("define", rest))
    } else if word == "endef" {
        Some(("endef", rest))
    } else if modifiers == 3 && matches!(word, "override" | "export" | "private") {
        Some(("define", ""))
    } else {
        None
    }
}

fn logical_lines(content: &str) -> Vec<LogicalLine> {
    let mut result = Vec::new();
    let mut pending: Option<(String, usize)> = None;
    for (source_line, physical) in content.lines().enumerate() {
        let (line, first_source_line) =
            if let Some((mut previous, first_source_line)) = pending.take() {
                previous.push_str(physical.trim_start());
                (previous, first_source_line)
            } else {
                (physical.to_owned(), source_line)
            };
        let trimmed = line.trim_end();
        if let Some(without_slash) = trimmed.strip_suffix('\\') {
            pending = Some((format!("{} ", without_slash.trim_end()), first_source_line));
        } else {
            result.push(LogicalLine {
                text: line,
                source_line: first_source_line,
            });
        }
    }
    if let Some((text, source_line)) = pending {
        result.push(LogicalLine { text, source_line });
    }
    result
}

fn rejection(owner: &str, source_line: usize, reason: String) -> SourceDirectoryGroupRejection {
    SourceDirectoryGroupRejection {
        owner: owner.to_owned(),
        source_line,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::make_vars::collect_vars_impl;
    use std::fs;

    const VALID_RULES: &str = "INCSUBDIRS := include api\nINCEMPTYDIRS :=\nINCL_DIRS := $(foreach dir,$(INCSUBDIRS) $(INCEMPTYDIRS),$(AROS_INCLUDES)/$(dir)) $(foreach dir,$(INCSUBDIRS) $(INCEMPTYDIRS),$(GENINCDIR)/$(dir))\nsetup : $(INCL_DIRS)\n$(INCL_DIRS) :\n\t@$(ECHO) \"Creating   $@...\"\n\t@$(MKDIR) $@\n";

    fn fixture_dirs(config: &str) -> (tempfile::TempDir, DirVars) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/make.cfg.in"), config).unwrap();
        let dirs = DirVars::load(root.path());
        (root, dirs)
    }

    fn collect(
        content: &str,
        config: &str,
    ) -> (
        Vec<SourceDirectoryGroupDecl>,
        Vec<SourceDirectoryGroupRejection>,
    ) {
        let (root, dirs) = fixture_dirs(config);
        let joined = crate::parser::join_continuations(content);
        let (scope, states) = collect_vars_impl(&joined, None);
        let (declarations, rejections) = collect_source_directory_groups(
            &joined,
            &scope,
            &dirs,
            root.path(),
            Path::new("compiler/include"),
            &states,
        );
        // Keep the temporary source tree alive until after collection.
        assert!(root.path().is_dir());
        (declarations, rejections)
    }

    const DIR_CONFIG: &str = "AROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\nAROS_DEVELOPER := $(AROSDIR)/Developer\nAROS_DIR_INCLUDE := include\nAROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\nGENINCDIR := $(GENDIR)/include\n";

    #[test]
    fn admits_explicit_ordered_directory_group_below_build_roots() {
        let (declarations, rejections) = collect(VALID_RULES, DIR_CONFIG);
        assert!(rejections.is_empty(), "{rejections:?}");
        assert_eq!(declarations.len(), 1);
        let declaration = &declarations[0];
        assert_eq!(declaration.owner, "setup");
        assert_eq!(declaration.source_line, 4);
        assert_eq!(declaration.directory_rule_line, 5);
        assert_eq!(
            declaration.directories,
            [
                "${AROS_BUILD_DIR}/SYS/Developer/include/include",
                "${AROS_BUILD_DIR}/SYS/Developer/include/api",
                "${AROS_BUILD_DIR}/gen/include/include",
                "${AROS_BUILD_DIR}/gen/include/api",
            ]
        );
    }

    #[test]
    fn missing_explicit_empty_directory_list_is_not_assumed_empty() {
        let content = VALID_RULES.replacen("INCEMPTYDIRS :=\n", "", 1);
        let (declarations, rejections) = collect(&content, DIR_CONFIG);
        assert!(declarations.is_empty());
        assert!(rejections.iter().any(|item| {
            item.reason.contains("INCEMPTYDIRS") || item.reason.contains("explicit value")
        }));
    }

    #[test]
    fn unknown_conditional_extra_recipe_and_duplicate_rules_fail_closed() {
        let unknown = format!("ifeq ($(UNKNOWN),yes)\n{VALID_RULES}endif\n");
        let (declarations, rejections) = collect(&unknown, DIR_CONFIG);
        assert!(declarations.is_empty());
        assert!(!rejections.is_empty());

        let extra = VALID_RULES.replace("\t@$(MKDIR) $@\n", "\t@$(MKDIR) $@\n\t@touch $@\n");
        let (declarations, rejections) = collect(&extra, DIR_CONFIG);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("exactly")));

        let duplicate = format!("{VALID_RULES}setup : $(INCL_DIRS)\n");
        let (declarations, rejections) = collect(&duplicate, DIR_CONFIG);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("found 2")));
    }

    #[test]
    fn wildcard_and_source_tree_writable_roots_are_refused() {
        let wildcard = VALID_RULES.replace(
            "INCSUBDIRS := include api",
            "INCSUBDIRS := $(wildcard include/*)",
        );
        let (declarations, rejections) = collect(&wildcard, DIR_CONFIG);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("wildcard")));

        let source_root_config = DIR_CONFIG.replace(
            "AROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)",
            "AROS_INCLUDES := $(SRCDIR)/writable-source",
        );
        let (declarations, rejections) = collect(VALID_RULES, &source_root_config);
        assert!(declarations.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("not below")));
    }

    #[test]
    fn command_redefinitions_and_target_specific_recipe_controls_are_refused() {
        for source in [
            format!("{VALID_RULES}MKDIR := @echo unsafe\n"),
            format!("{VALID_RULES}%.dir : MKDIR := @echo unsafe\n"),
            format!("{VALID_RULES}.SHELLFLAGS := -c; touch unsafe\n"),
        ] {
            let (declarations, rejections) = collect(&source, DIR_CONFIG);
            assert!(declarations.is_empty(), "{source}");
            assert!(!rejections.is_empty(), "{source}");
        }
    }
}
