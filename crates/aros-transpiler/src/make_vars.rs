//! Make variables, as GNU Make would see them at a given line.
//!
//! A declaration's arguments are expanded where the declaration stands, so
//! reading a file-global value for each name is wrong: `arch/m68k-amiga/c`
//! assigns `FILES` twice with a `%build_progs` between the two, and taking the
//! last value made both declarations build the same program. Sixteen
//! declarations across nine mmakefiles read a variable that is reassigned later
//! in the same file, so the scope keeps every assignment in file order and
//! answers per line.
//!
//! Conditionals are evaluated rather than skipped, with three outcomes instead
//! of two: true, false, and unknown, the last for a condition on something this
//! transpiler cannot decide. An unknown branch contributes nothing and is
//! reported, which is what keeps a guessed value out of the build.
//!
//! Eight modules read this vocabulary -- includes, flags, icons, catalogs,
//! arch_sources, local_make_includes and two capability families -- so it was
//! already the crate's Make-variable layer while it lived in `parser.rs`.

use crate::parser::TargetContext;

/// How deep an immediately-expanded assignment may recurse before the value is
/// treated as unresolvable.
const MAX_DEPTH_FOR_IMMEDIATE_EXPANSION: usize = 16;
use std::collections::{HashMap, HashSet};

const CLOSED_TARGET_MAKE_SELECTORS: &[&str] =
    aros_common::native_build_contract::NATIVE_RESERVED_MAKE_VARIABLES;

fn initial_make_configuration(
    context: Option<&TargetContext>,
) -> std::collections::BTreeMap<String, String> {
    let Some(context) = context else {
        return std::collections::BTreeMap::new();
    };

    let mut configuration = context.make_variables.clone();
    for selector in CLOSED_TARGET_MAKE_SELECTORS {
        // These names are closed target selectors, not user-provided make.cfg
        // values. If the target context does not know one, leave it absent.
        configuration.remove(*selector);
        if let Some(value) = context.value_of(selector) {
            configuration.insert((*selector).to_owned(), value);
        }
    }
    configuration
}

/// Variable assignments in the order the file makes them.
///
/// Make expands a declaration's arguments where the declaration stands.
/// `%build_progs files=$(FILES)` therefore takes the value FILES held at that
/// line, because the macro emits `<mmake>_FILES := %(files)` -- a simple
/// assignment, evaluated in place (config/make.tmpl:1868).
///
/// Reading one file-global value instead gave every declaration the file's last
/// assignment. arch/m68k-amiga/c declares `FILES := gdbstub`, a %build_progs,
/// `FILES := gdbstop`, and a second %build_progs; both came out building
/// gdbstop, two targets claimed the output SYS/C/.../gdbstop, and Ninja refused
/// to generate the build at all. 16 declarations across 9 mmakefiles read a
/// variable that is reassigned later in the same file.
pub struct VarScope {
    /// Literal source configuration loaded before this file, not command-line
    /// overrides. Local assignment history shadows these defaults.
    configuration: std::collections::BTreeMap<String, String>,
    /// Per name, the assignments in file order as (line, values).
    assignments: HashMap<String, Vec<(usize, Vec<String>)>>,
    /// Per name, the right-hand side as written, in file order.
    ///
    /// A list is not enough for a path. `EXEDIR := $(AROS_TOOLS)/QuickPart` is
    /// one word either way, but `dir=$(AROS_PRESETS)/Icons/Gorilla/Small/$(AROS_DIR_AROS)`
    /// has to keep its slashes and its references, so path resolution reads
    /// this instead of the word list.
    raw: HashMap<String, Vec<(usize, String)>>,
    /// Source undefine events. Raw and list histories hold an empty value for
    /// expansion, while this history preserves undefined status for later ?=.
    undefinitions: HashMap<String, Vec<usize>>,
    /// Effective source values whose contents cannot be known because a `+=`
    /// depended on an unproven Make flavor, or because a simple assignment
    /// froze such a value. A later replacing assignment clears the state.
    flavor_uncertainties: HashMap<String, Vec<(usize, Option<String>)>>,
    /// Scalar path history with declaration-time Make assignment semantics;
    /// the legacy general collector remains unchanged for its other consumers.
    path_raw: HashMap<String, Vec<(usize, String)>>,
    /// Assignments made inside a Make conditional, by source line.
    ///
    /// The legacy list collector intentionally retains its historical
    /// last-assignment behaviour because the icon collector evaluates
    /// condition branches separately. Generic expression evaluation must be
    /// stricter: using the last textual branch would silently merge or select
    /// architecture-specific source lists without knowing the condition.
    conditional_assignments: HashMap<String, Vec<(usize, AssignmentKind)>>,
    /// Active or possibly active `define NAME` assignments whose multiline
    /// value is intentionally not evaluated by this scanner.
    opaque_assignments: HashMap<String, Vec<usize>>,
    /// A define header with an unsupported dynamic name may shadow any local.
    opaque_all_assignments: Vec<usize>,
    /// Proven replacement assignments; an unconditional `=` or `:=` supersedes
    /// earlier unknown branches and opaque definitions for a local scalar path.
    path_replacements: HashMap<String, Vec<usize>>,
    /// Simply-expanded paths whose RHS read an unresolved branch. Preserve
    /// that uncertainty even when the legacy raw collector freezes a value.
    uncertain_path_values: HashMap<String, Vec<usize>>,
    /// Names introduced as file-local switches, including an assignment in a
    /// branch proven false and explicitly commented-out `#NAME=value` feature
    /// toggles. Once seen, absence of an active assignment has GNU Make's
    /// ordinary empty value. Names never introduced by the file remain unknown
    /// because they may come from an included configuration fragment.
    local_names: HashSet<String>,
}

impl VarScope {
    fn has_opaque_assignment_before(&self, name: &str, line: usize) -> bool {
        let latest_opaque = self
            .opaque_assignments
            .get(name)
            .and_then(|history| history.iter().rev().find(|at| **at < line).copied())
            .into_iter()
            .chain(
                self.opaque_all_assignments
                    .iter()
                    .rev()
                    .find(|at| **at < line)
                    .copied(),
            )
            .max();
        let reset = self
            .path_replacements
            .get(name)
            .and_then(|history| history.iter().rev().find(|at| **at < line).copied());
        latest_opaque.is_some_and(|opaque| reset.is_none_or(|reset| opaque > reset))
    }

    fn has_uncertain_path_value_before(&self, name: &str, line: usize) -> bool {
        let reset = self
            .path_replacements
            .get(name)
            .and_then(|history| history.iter().rev().find(|at| **at < line).copied());
        self.uncertain_path_values.get(name).is_some_and(|history| {
            history
                .iter()
                .any(|at| *at < line && reset.is_none_or(|reset| *at >= reset))
        })
    }

    /// Whether a scalar path still depends on an unresolved conditional or
    /// opaque definition. Appends and `?=` cannot erase it; a replacement can.
    pub(crate) fn path_is_conditional_at(&self, name: &str, line: usize) -> bool {
        let reset = self
            .path_replacements
            .get(name)
            .and_then(|history| history.iter().rev().find(|at| **at < line).copied());
        self.has_opaque_assignment_before(name, line)
            || self
                .conditional_assignments
                .get(name)
                .is_some_and(|history| {
                    history
                        .iter()
                        .any(|(at, _)| *at < line && reset.is_none_or(|reset| *at > reset))
                })
            || self.has_uncertain_path_value_before(name, line)
    }

    pub(crate) fn is_known_local(&self, name: &str) -> bool {
        self.local_names.contains(name)
    }

    fn path_depends_on_conditional_at(
        &self,
        name: &str,
        line: usize,
        depth: usize,
        guard: &mut Vec<String>,
    ) -> bool {
        if self.path_is_conditional_at(name, line)
            || depth == 0
            || guard.iter().any(|item| item == name)
        {
            return true;
        }
        let Some(value) = self.path_raw_at(name, line) else {
            return false;
        };
        guard.push(name.to_owned());
        let uncertain = scalar_references(&value).any(|dependency| {
            self.path_depends_on_conditional_at(dependency, line, depth - 1, guard)
        });
        guard.pop();
        uncertain
    }

    /// The variable state as Make would see it at `line`.
    ///
    /// A declaration on line N sees every assignment made before it and none of
    /// those made after.
    pub(crate) fn snapshot(&self, line: usize) -> HashMap<String, Vec<String>> {
        let mut snapshot: HashMap<_, _> = self
            .configuration
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    value.split_whitespace().map(str::to_owned).collect(),
                )
            })
            .collect();
        snapshot.extend(self.assignments.iter().filter_map(|(name, history)| {
            history
                .iter()
                .rev()
                .find(|(at, _)| *at < line)
                .map(|(_, values)| (name.clone(), values.clone()))
        }));
        snapshot.retain(|name, _| {
            self.is_defined_before(name, line)
                && !self.has_opaque_assignment_before(name, line)
                && !self.has_uncertain_path_value_before(name, line)
        });
        snapshot
    }

    /// The right-hand side of `name` as written, as of `line`.
    #[must_use]
    pub fn raw_at(&self, name: &str, line: usize) -> Option<String> {
        if self.has_opaque_assignment_before(name, line)
            || self.has_uncertain_path_value_before(name, line)
        {
            return None;
        }
        self.raw
            .get(name)
            .and_then(|history| history.iter().rev().find(|(at, _)| *at < line))
            .map(|(_, v)| v.clone())
            .or_else(|| self.configuration.get(name).cloned())
    }

    pub(crate) fn path_raw_at(&self, name: &str, line: usize) -> Option<String> {
        if self.has_opaque_assignment_before(name, line)
            || self.has_uncertain_path_value_before(name, line)
        {
            return None;
        }
        self.path_raw
            .get(name)
            .and_then(|history| history.iter().rev().find(|(at, _)| *at < line))
            .map(|(_, value)| value.clone())
            .or_else(|| self.configuration.get(name).cloned())
    }

    /// Whether `name` has an unresolved conditional, opaque definition, or
    /// frozen path uncertainty before `line`.
    ///
    /// Callers must reject such a value rather than borrowing a stale local or
    /// configured fallback.
    #[must_use]
    pub fn conditionally_assigned_before(&self, name: &str, line: usize) -> bool {
        self.path_is_conditional_at(name, line)
    }

    /// Whether every unresolved conditional assignment before `line` merely
    /// appends to the known value accumulated outside those branches.
    ///
    /// This is useful for flag bundles: their unconditional prefix remains a
    /// sound lower bound when optional feature probes only add flags. An
    /// unresolved replacement (`=`, `:=` or `?=`) invalidates the whole value.
    #[must_use]
    pub(crate) fn conditionally_appended_only_before(&self, name: &str, line: usize) -> bool {
        let Some(assignments) = self.conditional_assignments.get(name) else {
            return false;
        };
        let mut assignments = assignments.iter().filter(|(at, _)| *at < line).peekable();
        assignments.peek().is_some() && assignments.all(|(_, kind)| *kind == AssignmentKind::Append)
    }

    /// Why the effective source value at `line` may differ depending on an
    /// unrecorded variable flavor. Context configuration supplies values but
    /// not GNU Make's recursive/simple flavor, so a source `+=` with references
    /// cannot safely choose one expansion time.
    pub(crate) fn flavor_uncertainty_reason_at(&self, name: &str, line: usize) -> Option<String> {
        self.flavor_uncertainty_reason_inner(name, line, 16, &mut HashSet::new())
    }

    fn flavor_uncertainty_reason_inner(
        &self,
        name: &str,
        line: usize,
        depth: usize,
        visiting: &mut HashSet<String>,
    ) -> Option<String> {
        if let Some(reason) = self
            .flavor_uncertainties
            .get(name)
            .and_then(|history| history.iter().rev().find(|(at, _)| *at < line))
            .and_then(|(_, reason)| reason.as_ref())
        {
            return Some(reason.clone());
        }
        if depth == 0 || !visiting.insert(name.to_owned()) {
            return None;
        }
        let reason = self.raw_at(name, line).and_then(|value| {
            scoped_variable_references(&value)
                .into_iter()
                .find_map(|dependency| {
                    self.flavor_uncertainty_reason_inner(&dependency, line, depth - 1, visiting)
                        .map(|reason| {
                            format!(
                                "{name} depends on {dependency}, whose value is uncertain: {reason}"
                            )
                        })
                })
        });
        visiting.remove(name);
        reason
    }

    /// The most recent raw value of `name` while the assignment scan is in
    /// progress. Appending is defined in terms of the value accumulated so
    /// far, not merely the last right-hand side.
    fn latest_raw(&self, name: &str) -> Option<&str> {
        if self.has_opaque_assignment_before(name, usize::MAX)
            || self.has_uncertain_path_value_before(name, usize::MAX)
        {
            return None;
        }
        self.raw
            .get(name)
            .and_then(|h| h.last())
            .map(|(_, value)| value.as_str())
            .or_else(|| self.configuration.get(name).map(String::as_str))
    }

    fn is_defined_before(&self, name: &str, line: usize) -> bool {
        if self.has_opaque_assignment_before(name, line) {
            return true;
        }
        let last_source_value = self
            .raw
            .get(name)
            .and_then(|history| history.iter().rev().find(|(at, _)| *at < line));
        let last_undefine = self
            .undefinitions
            .get(name)
            .and_then(|history| history.iter().rev().find(|at| **at < line));
        match (last_source_value, last_undefine) {
            (Some((set_at, _)), Some(undefine_at)) => set_at > undefine_at,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => self.configuration.contains_key(name),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssignmentKind {
    SimpleSet,
    RecursiveSet,
    SetIfUnset,
    Append,
    Undefine,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VariableFlavor {
    Simple,
    Recursive,
    Unknown,
}

/// Splits a plain Make variable assignment without mistaking a rule for one.
///
/// The tree uses `::=`, `:=`, `=`, `?=` and `+=`. Keeping the operator is important:
/// two icon lists are built incrementally, and treating their `+=` lines as
/// either invalid or ordinary assignments silently drops 118 generated files.
pub(crate) fn variable_assignment(line: &str) -> Option<(&str, &str, AssignmentKind)> {
    let trimmed = line.trim();
    let (at, width, kind) = [
        ("::=", AssignmentKind::SimpleSet),
        (":=", AssignmentKind::SimpleSet),
        ("+=", AssignmentKind::Append),
        ("?=", AssignmentKind::SetIfUnset),
        ("=", AssignmentKind::RecursiveSet),
    ]
    .into_iter()
    .filter_map(|(op, kind)| trimmed.find(op).map(|at| (at, op.len(), kind)))
    .min_by_key(|(at, _, _)| *at)?;

    let name = trimmed[..at].trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some((name, trimmed[at + width..].trim(), kind))
}

/// Parses the literal undefine NAME form supported by source-scoped Make
/// evaluation. Ok(None) means this is not an undefine directive; Err(()) means
/// the directive has a missing, dynamic, or otherwise unsupported name.
pub(crate) fn undefine_directive(line: &str) -> Result<Option<&str>, ()> {
    let trimmed = strip_make_comment(line).trim();
    if variable_assignment(trimmed).is_some() {
        return Ok(None);
    }
    let Some(tail) = directive_tail(trimmed, "undefine") else {
        return Ok(None);
    };
    let name = tail.trim();
    if !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        Ok(Some(name))
    } else {
        Err(())
    }
}

/// Removes an unescaped GNU Make comment from one logical line.
///
/// A `#` starts a comment even when it is attached to the preceding word.
/// Keeping it in an assignment made `FILES := a b #disabled` compile a bogus
/// source named `#disabled`. An odd run of backslashes escapes the marker.
pub(crate) fn strip_make_comment(line: &str) -> &str {
    for (at, character) in line.char_indices() {
        if character != '#' {
            continue;
        }
        let escaped = line[..at]
            .bytes()
            .rev()
            .take_while(|byte| *byte == b'\\')
            .count()
            % 2
            == 1;
        if !escaped {
            return &line[..at];
        }
    }
    line
}

/// Freezes local variable references in a simply-expanded (`:=`) assignment.
///
/// Global/configured variables remain as Make references for [`DirVars`] to
/// render later. Function calls are retained too, but their nested local
/// arguments are frozen now, which preserves the source-order semantics the
/// bounded evaluator needs at the declaration line.
fn expand_immediate_locals(raw: &str, scope: &VarScope, depth: usize) -> String {
    expand_immediate_locals_impl(raw, scope, depth, None)
}

fn expand_immediate_locals_impl(
    raw: &str,
    scope: &VarScope,
    depth: usize,
    path_names: Option<&HashSet<String>>,
) -> String {
    if depth == 0 || !raw.contains('$') {
        return raw.to_owned();
    }

    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            output.push_str(&raw[cursor..]);
            break;
        };
        let dollar = cursor + relative;
        output.push_str(&raw[cursor..dollar]);
        let Some(next) = raw.as_bytes().get(dollar + 1) else {
            output.push('$');
            break;
        };
        if *next == b'$' {
            output.push('$');
            cursor = dollar + 2;
            continue;
        }
        let (open, close) = match *next {
            b'(' => (b'(', b')'),
            b'{' => (b'{', b'}'),
            _ => {
                output.push('$');
                cursor = dollar + 1;
                continue;
            }
        };

        let mut nesting = 1usize;
        let mut end = dollar + 2;
        while end < raw.len() {
            let byte = raw.as_bytes()[end];
            if byte == b'$' && raw.as_bytes().get(end + 1) == Some(&open) {
                nesting += 1;
                end += 2;
                continue;
            }
            if byte == close {
                nesting -= 1;
                if nesting == 0 {
                    break;
                }
            }
            end += 1;
        }
        if end == raw.len() {
            output.push_str(&raw[dollar..]);
            break;
        }

        let body = &raw[dollar + 2..end];
        let simple_name = (!body.is_empty()
            && body.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            }))
        .then_some(body);
        // These are configured/built-in Make path variables. In particular,
        // OBJDIR is $(GENDIR)/$(CURDIR), and CURDIR comes from GNU Make rather
        // than an assignment in an mmakefile. A collector prelude may know the
        // name while holding no physical current-directory value; freezing
        // that provisional state would turn $(OBJDIR)/x into $(GENDIR)/x.
        let local_value = simple_name
            .filter(|name| {
                !matches!(*name, "CURDIR" | "OBJDIR")
                    && (path_names.is_none() || !crate::includes::is_deferred_path_var(name))
            })
            .and_then(|name| {
                if path_names.is_some() {
                    scope
                        .path_raw
                        .get(name)
                        .and_then(|history| history.last())
                        .map(|(_, value)| value.as_str())
                        .or_else(|| scope.configuration.get(name).map(String::as_str))
                } else {
                    scope.latest_raw(name)
                }
            });
        if let Some(value) = local_value {
            output.push_str(&expand_immediate_locals_impl(
                value,
                scope,
                depth - 1,
                path_names,
            ));
        } else if simple_name.is_some_and(|name| {
            name != "CURDIR"
                && !crate::includes::is_deferred_path_var(name)
                && path_names.is_some_and(|names| names.contains(name))
        }) {
            // A file-local variable absent now is empty even if assigned later.
            // Any unresolved conditional dependency is separately recorded.
        } else {
            output.push('$');
            output.push(open as char);
            output.push_str(&expand_immediate_locals_impl(
                body,
                scope,
                depth - 1,
                path_names,
            ));
            output.push(close as char);
        }
        cursor = end + 1;
    }
    output
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConditionalTruth {
    False,
    True,
    Unknown,
}

impl ConditionalTruth {
    const fn not(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::True => Self::False,
            Self::Unknown => Self::Unknown,
        }
    }

    const fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }

    const fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ConditionalFrame {
    pub(crate) parent: ConditionalTruth,
    pub(crate) matched: ConditionalTruth,
    pub(crate) current: ConditionalTruth,
}

impl ConditionalFrame {
    pub(crate) const fn new(parent: ConditionalTruth, condition: ConditionalTruth) -> Self {
        Self {
            parent,
            matched: condition,
            current: parent.and(condition),
        }
    }

    pub(crate) const fn else_if(&mut self, condition: ConditionalTruth) {
        self.current = self.parent.and(self.matched.not()).and(condition);
        self.matched = self.matched.or(condition);
    }

    pub(crate) const fn otherwise(&mut self) {
        self.current = self.parent.and(self.matched.not());
        self.matched = ConditionalTruth::True;
    }
}

pub(crate) fn directive_tail<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let tail = line.strip_prefix(word)?;
    (tail.is_empty()
        || tail
            .chars()
            .next()
            .is_some_and(|character| character.is_whitespace() || character == '('))
    .then(|| tail.trim())
}

fn split_top_level_comma(raw: &str) -> Option<(&str, &str)> {
    let mut paren_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut quote = None;
    for (at, character) in raw.char_indices() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if paren_depth == 0 && brace_depth == 0 => {
                return Some((&raw[..at], &raw[at + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn take_condition_word(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.trim_start();
    let first = raw.chars().next()?;
    if matches!(first, '\'' | '"') {
        let after_quote = &raw[first.len_utf8()..];
        let end = after_quote.find(first)?;
        let word = &raw[..end + 2];
        return Some((word, &after_quote[end + 1..]));
    }
    let end = raw.find(char::is_whitespace).unwrap_or(raw.len());
    Some((&raw[..end], &raw[end..]))
}

fn equality_operands(raw: &str) -> Option<(&str, &str)> {
    let raw = raw.trim();
    if raw.starts_with('(') && raw.ends_with(')') {
        return split_top_level_comma(&raw[1..raw.len() - 1]);
    }
    let (left, rest) = take_condition_word(raw)?;
    let (right, trailing) = take_condition_word(rest)?;
    // Quotes delimit operands only in the quoted directive form. In the
    // parenthesized form and in expanded variable values they are data.
    trailing.trim().is_empty().then_some((
        unquote_condition_value(left),
        unquote_condition_value(right),
    ))
}

fn unquote_condition_value(raw: &str) -> &str {
    let raw = raw.trim();
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if matches!(bytes[0], b'\'' | b'"') && bytes[0] == bytes[raw.len() - 1] {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
}

fn condition_pattern_matches(pattern: &str, word: &str) -> bool {
    let Some(percent) = pattern.find('%') else {
        return pattern == word;
    };
    let prefix = &pattern[..percent];
    let suffix = &pattern[percent + 1..];
    word.len() >= prefix.len() + suffix.len() && word.starts_with(prefix) && word.ends_with(suffix)
}

fn expand_condition_function(
    body: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
    line: usize,
) -> Option<String> {
    let split = body.find(char::is_whitespace)?;
    let name = body[..split].trim();
    let args = body[split..].trim();
    match name {
        "strip" => Some(
            expand_condition_operand(args, scope, context, depth - 1, line)?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "findstring" => {
            let (needle, haystack) = split_top_level_comma(args)?;
            let needle = expand_condition_operand(needle, scope, context, depth - 1, line)?;
            let haystack = expand_condition_operand(haystack, scope, context, depth - 1, line)?;
            Some(if haystack.contains(&needle) {
                needle
            } else {
                String::new()
            })
        }
        "filter" | "filter-out" => {
            let (patterns, words) = split_top_level_comma(args)?;
            let patterns = expand_condition_operand(patterns, scope, context, depth - 1, line)?;
            let words = expand_condition_operand(words, scope, context, depth - 1, line)?;
            let keep_matches = name == "filter";
            Some(
                words
                    .split_whitespace()
                    .filter(|word| {
                        let matches = patterns
                            .split_whitespace()
                            .any(|pattern| condition_pattern_matches(pattern, word));
                        matches == keep_matches
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
        _ => None,
    }
}

fn expand_condition_reference(
    body: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
    line: usize,
) -> Option<String> {
    let body = body.trim();
    if !body.is_empty()
        && body.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        if scope.path_is_conditional_at(body, line) {
            return None;
        }
        if let Some(value) = scope.raw_at(body, line) {
            return expand_condition_operand(&value, scope, context, depth - 1, line);
        }
        if let Some(value) = context.value(body) {
            return Some(value);
        }
        return scope.local_names.contains(body).then(String::new);
    }
    expand_condition_function(body, scope, context, depth, line)
}

fn expand_condition_operand(
    raw: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
    line: usize,
) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            output.push_str(&raw[cursor..]);
            break;
        };
        let dollar = cursor + relative;
        output.push_str(&raw[cursor..dollar]);
        let next = *raw.as_bytes().get(dollar + 1)?;
        if next == b'$' {
            output.push('$');
            cursor = dollar + 2;
            continue;
        }
        let (open, close) = match next {
            b'(' => (b'(', b')'),
            b'{' => (b'{', b'}'),
            _ => return None,
        };
        let mut nesting = 1usize;
        let mut end = dollar + 2;
        while end < raw.len() {
            let byte = raw.as_bytes()[end];
            if byte == b'$' && raw.as_bytes().get(end + 1) == Some(&open) {
                nesting += 1;
                end += 2;
                continue;
            }
            if byte == close {
                nesting -= 1;
                if nesting == 0 {
                    break;
                }
            }
            end += 1;
        }
        if end == raw.len() {
            return None;
        }
        output.push_str(&expand_condition_reference(
            &raw[dollar + 2..end],
            scope,
            context,
            depth - 1,
            line,
        )?);
        cursor = end + 1;
    }
    Some(output.trim().to_owned())
}

pub(crate) fn evaluate_conditional(
    directive: &str,
    args: &str,
    scope: &VarScope,
    context: &TargetContext,
    line: usize,
) -> ConditionalTruth {
    let value = match directive {
        "ifeq" | "ifneq" => equality_operands(args).and_then(|(left, right)| {
            Some(
                expand_condition_operand(
                    left,
                    scope,
                    context,
                    MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                    line,
                )? == expand_condition_operand(
                    right,
                    scope,
                    context,
                    MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                    line,
                )?,
            )
        }),
        "ifdef" | "ifndef" => {
            let name = args.trim();
            // A configured default cannot hide an unresolved source write.
            // Respect reset writes before this condition, never later ones.
            if scope.path_is_conditional_at(name, line) {
                return ConditionalTruth::Unknown;
            }
            let value = scope.raw_at(name, line).or_else(|| context.value(name));
            value.map(|value| !value.is_empty())
        }
        _ => None,
    };
    let Some(value) = value else {
        return ConditionalTruth::Unknown;
    };
    let value = if matches!(directive, "ifneq" | "ifndef") {
        !value
    } else {
        value
    };
    if value {
        ConditionalTruth::True
    } else {
        ConditionalTruth::False
    }
}

/// Reads every variable assignment from continuation-joined mmakefile text.
#[must_use]
pub fn collect_vars(joined: &str) -> VarScope {
    collect_vars_impl(joined, None).0
}

/// Reads variable assignments while selecting every Make conditional that the
/// concrete target context makes decidable.
///
/// Assignments in a false branch are discarded. Assignments in an unknown
/// branch are also kept out of the value history, but are recorded as unsafe so
/// expression evaluation reports the unresolved lane instead of silently
/// treating it as empty or merging it with its alternative.
#[must_use]
pub fn collect_vars_with_context(joined: &str, context: &TargetContext) -> VarScope {
    collect_vars_impl(joined, Some(context)).0
}

pub(crate) fn collect_vars_impl(
    joined: &str,
    context: Option<&TargetContext>,
) -> (VarScope, Vec<ConditionalTruth>) {
    collect_vars_impl_with_forward_locals(joined, context, false)
}

#[derive(Clone, Debug)]
enum DefineBinding {
    Known(String),
    Unknown,
}

struct DefineScan {
    suppressed: Vec<bool>,
    headers: Vec<Option<DefineBinding>>,
}

fn make_define_directive(line: &str) -> Option<(&str, &str)> {
    let line = strip_make_comment(line.trim_start()).trim_start();
    let mut words = line.splitn(2, char::is_whitespace);
    let mut word = words.next().unwrap_or_default();
    let mut rest = words.next().unwrap_or_default().trim();
    let mut modifiers = 0;
    while matches!(word, "override" | "export" | "private") && modifiers < 3 {
        modifiers += 1;
        let mut modifier_words = rest.splitn(2, char::is_whitespace);
        word = modifier_words.next().unwrap_or_default();
        rest = modifier_words.next().unwrap_or_default().trim();
    }
    if matches!(word, "define" | "endef") {
        Some((word, rest))
    } else if modifiers == 3 && matches!(word, "override" | "export" | "private") {
        // More than the three known modifiers is malformed or unsupported.
        // Treat it as an opaque open define so its following body cannot leak.
        Some(("define", ""))
    } else {
        None
    }
}

fn define_variable_name(arguments: &str) -> Option<String> {
    let arguments = arguments.trim();
    let name = variable_assignment(arguments).map_or(arguments, |(name, _, _)| name);
    is_scoped_variable_name(name).then(|| name.to_owned())
}

/// Marks GNU Make `define` directives and their bodies as inert for the source
/// variable scan. A define body is data until expanded by `eval`; assignments
/// and conditionals in it must not affect the surrounding declaration scope.
/// Nested definitions are counted so an inner `endef` cannot expose the rest
/// of an outer body to the scanner. An unclosed or malformed body stays
/// suppressed through EOF.
fn scan_define_bodies(lines: &[&str]) -> DefineScan {
    let mut suppressed = Vec::with_capacity(lines.len());
    let mut headers = Vec::with_capacity(lines.len());
    let mut depth = 0usize;

    for raw_line in lines {
        if raw_line.starts_with('\t') {
            suppressed.push(depth > 0);
            headers.push(None);
            continue;
        }

        let directive = make_define_directive(raw_line);
        let inside_define = depth > 0;
        suppressed.push(inside_define || directive.is_some());
        headers.push(match directive {
            Some(("define", arguments)) if !inside_define => Some(
                define_variable_name(arguments)
                    .map_or(DefineBinding::Unknown, DefineBinding::Known),
            ),
            _ => None,
        });
        match directive {
            Some(("define", _)) => depth = depth.saturating_add(1),
            // GNU Make's endef has no arguments. Treat malformed terminators
            // as body text and fail closed by keeping the remainder inert.
            Some(("endef", "")) if inside_define => depth -= 1,
            _ => {}
        }
    }

    DefineScan {
        suppressed,
        headers,
    }
}

/// A top-level `$(error ...)` call: GNU Make stops reading the makefile when
/// it expands one.
pub(crate) fn is_make_error_directive(line: &str) -> bool {
    let line = line.trim();
    line.strip_prefix("$(error")
        .is_some_and(|rest| rest.starts_with([' ', '\t', ')']) && line.ends_with(')'))
}

pub(crate) fn collect_vars_impl_with_forward_locals(
    joined: &str,
    context: Option<&TargetContext>,
    forward_locals: bool,
) -> (VarScope, Vec<ConditionalTruth>) {
    let mut scope = VarScope {
        configuration: initial_make_configuration(context),
        assignments: HashMap::new(),
        raw: HashMap::new(),
        undefinitions: HashMap::new(),
        flavor_uncertainties: HashMap::new(),
        path_raw: HashMap::new(),
        conditional_assignments: HashMap::new(),
        opaque_assignments: HashMap::new(),
        opaque_all_assignments: Vec::new(),
        path_replacements: HashMap::new(),
        uncertain_path_values: HashMap::new(),
        local_names: HashSet::new(),
    };
    let lines = joined.lines().collect::<Vec<_>>();
    let define_scan = scan_define_bodies(&lines);
    let define_suppressed = define_scan.suppressed.as_slice();
    let path_names: HashSet<String> = lines
        .iter()
        .zip(define_suppressed)
        .filter(|(line, suppressed)| !**suppressed && !line.starts_with('\t'))
        .filter_map(|(line, _)| {
            let line = line
                .trim_start()
                .strip_prefix('#')
                .map_or(*line, str::trim_start);
            variable_assignment(strip_make_comment(line)).map(|(name, _, _)| name.to_owned())
        })
        .collect();
    if context.is_some() && forward_locals {
        for (raw_line, suppressed) in lines.iter().zip(define_suppressed) {
            if *suppressed || raw_line.starts_with('\t') {
                continue;
            }
            let commented = raw_line.trim_start().strip_prefix('#').map(str::trim_start);
            let assignment = commented
                .and_then(variable_assignment)
                .or_else(|| variable_assignment(strip_make_comment(raw_line)));
            if let Some((name, _, _)) = assignment {
                scope.local_names.insert(name.to_owned());
            }
        }
    }
    let mut conditional_depth = 0usize;
    let mut conditional_stack: Vec<ConditionalFrame> = Vec::new();
    let mut flavors: HashMap<String, VariableFlavor> = HashMap::new();
    let mut line_states = Vec::with_capacity(lines.len());
    // Set once an $(error) may have run. Make stops there, so with a selected
    // target nothing after it is proven, whatever the later conditionals say.
    let mut halted = false;

    for (line_no, raw_line) in lines.iter().copied().enumerate() {
        let branch_state = context.map_or_else(
            || {
                if conditional_depth > 0 {
                    ConditionalTruth::Unknown
                } else {
                    ConditionalTruth::True
                }
            },
            |_| {
                if halted {
                    ConditionalTruth::Unknown
                } else {
                    conditional_stack
                        .last()
                        .map_or(ConditionalTruth::True, |frame| frame.current)
                }
            },
        );
        line_states.push(branch_state);

        if let Some(binding) = define_scan.headers[line_no].as_ref() {
            if branch_state != ConditionalTruth::False {
                match binding {
                    DefineBinding::Known(name) => scope
                        .opaque_assignments
                        .entry(name.clone())
                        .or_default()
                        .push(line_no),
                    DefineBinding::Unknown => scope.opaque_all_assignments.push(line_no),
                }
            }
        }

        // The define line, every body line, and its matching endef are data
        // or structural markers. In particular, body conditionals must not
        // change the branch state used by subsequent declarations.
        if define_suppressed[line_no] {
            continue;
        }

        // GNU Make's default recipe prefix is a tab. A shell command named
        // `else`, `endif`, or `NAME=...` is not a Make control/assignment and
        // must not change the proof for subsequent source declarations.
        if raw_line.starts_with('\t') {
            continue;
        }

        if context.is_some() {
            let commented = raw_line.trim_start().strip_prefix('#').map(str::trim_start);
            if let Some((name, _, _)) = commented.and_then(variable_assignment) {
                scope.local_names.insert(name.to_owned());
            }
        }
        let line = strip_make_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.starts_with('%') {
            continue;
        }
        if context.is_some()
            && branch_state != ConditionalTruth::False
            && is_make_error_directive(trimmed)
        {
            halted = true;
            continue;
        }

        if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
            .into_iter()
            .find_map(|word| directive_tail(trimmed, word).map(|tail| (word, tail)))
        {
            if let Some(context) = context {
                let parent = conditional_stack
                    .last()
                    .map_or(ConditionalTruth::True, |frame| frame.current);
                let condition = evaluate_conditional(directive, args, &scope, context, line_no);
                conditional_stack.push(ConditionalFrame::new(parent, condition));
            } else {
                conditional_depth += 1;
            }
            continue;
        }
        if trimmed == "endif" {
            if context.is_some() {
                conditional_stack.pop();
            } else {
                conditional_depth = conditional_depth.saturating_sub(1);
            }
            continue;
        }
        if trimmed == "else" || trimmed.starts_with("else ") {
            if let Some(context) = context {
                if let Some(frame) = conditional_stack.last_mut() {
                    let tail = trimmed.strip_prefix("else").unwrap().trim();
                    if tail.is_empty() {
                        frame.otherwise();
                    } else if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
                        .into_iter()
                        .find_map(|word| directive_tail(tail, word).map(|args| (word, args)))
                    {
                        let condition =
                            evaluate_conditional(directive, args, &scope, context, line_no);
                        frame.else_if(condition);
                    } else {
                        frame.else_if(ConditionalTruth::Unknown);
                    }
                }
            }
            continue;
        }

        // Make has five assignment spellings here and the tree uses them.
        // Reading only `:=` lost every list written with `=` or `?=`:
        // rom/hidds/pci/pcitool declares `FILES = main pciids support locale`
        // that way, while the icon sets append to two lists with `+=`.
        let assignment = variable_assignment(line);
        let undefine = undefine_directive(line).ok().flatten();
        let (var_name, value, kind) = if let Some((name, value, kind)) = assignment {
            (name, value, kind)
        } else if let Some(name) = undefine {
            (name, "", AssignmentKind::Undefine)
        } else {
            continue;
        };
        scope.local_names.insert(var_name.to_owned());

        if branch_state == ConditionalTruth::Unknown {
            scope
                .conditional_assignments
                .entry(var_name.to_owned())
                .or_default()
                .push((line_no, kind));
        }
        if context.is_some() && branch_state != ConditionalTruth::True {
            continue;
        }

        if kind == AssignmentKind::SetIfUnset && scope.is_defined_before(var_name, line_no) {
            continue;
        }

        if kind == AssignmentKind::Undefine {
            scope
                .undefinitions
                .entry(var_name.to_owned())
                .or_default()
                .push(line_no);
        }

        if branch_state == ConditionalTruth::True
            && matches!(
                kind,
                AssignmentKind::SimpleSet | AssignmentKind::RecursiveSet | AssignmentKind::Undefine
            )
        {
            scope
                .path_replacements
                .entry(var_name.to_owned())
                .or_default()
                .push(line_no);
        }

        let flavor = match kind {
            AssignmentKind::SimpleSet => VariableFlavor::Simple,
            AssignmentKind::RecursiveSet
            | AssignmentKind::SetIfUnset
            | AssignmentKind::Undefine => VariableFlavor::Recursive,
            AssignmentKind::Append => flavors.get(var_name).copied().unwrap_or_else(|| {
                if scope.configuration.contains_key(var_name) {
                    VariableFlavor::Unknown
                } else {
                    VariableFlavor::Recursive
                }
            }),
        };
        let flavor_uncertainty = if kind == AssignmentKind::Append
            && flavor == VariableFlavor::Unknown
            && value.contains('$')
        {
            Some(format!(
                "{var_name} appends Make references to a configuration value whose recursive/simple flavor is unknown"
            ))
        } else if matches!(kind, AssignmentKind::SimpleSet | AssignmentKind::Append)
            && flavor == VariableFlavor::Simple
        {
            scoped_variable_references(value)
                .into_iter()
                .find_map(|dependency| {
                    scope
                        .flavor_uncertainty_reason_at(&dependency, line_no)
                        .map(|reason| {
                            format!(
                                "{var_name} freezes a value from {dependency} with uncertain Make flavor: {reason}"
                            )
                        })
                })
        } else {
            None
        };
        match kind {
            AssignmentKind::SimpleSet
            | AssignmentKind::RecursiveSet
            | AssignmentKind::Undefine
            | AssignmentKind::SetIfUnset => {
                scope
                    .flavor_uncertainties
                    .entry(var_name.to_owned())
                    .or_default()
                    .push((line_no, flavor_uncertainty));
            }
            AssignmentKind::Append => {
                if let Some(reason) = flavor_uncertainty {
                    scope
                        .flavor_uncertainties
                        .entry(var_name.to_owned())
                        .or_default()
                        .push((line_no, Some(reason)));
                }
            }
        }
        if flavor == VariableFlavor::Simple
            && scalar_references(value).any(|name| {
                scope.path_depends_on_conditional_at(
                    name,
                    line_no,
                    MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                    &mut Vec::new(),
                )
            })
        {
            scope
                .uncertain_path_values
                .entry(var_name.to_owned())
                .or_default()
                .push(line_no);
        }
        let path_rhs = if flavor == VariableFlavor::Simple {
            expand_immediate_locals_impl(
                value,
                &scope,
                MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                Some(&path_names),
            )
        } else {
            value.to_owned()
        };
        let path_value = if kind == AssignmentKind::Append {
            let previous = scope
                .path_raw
                .get(var_name)
                .and_then(|history| history.last())
                .map(|(_, value)| value.as_str())
                .or_else(|| scope.configuration.get(var_name).map(String::as_str))
                .unwrap_or("");
            format!("{previous} {path_rhs}").trim().to_owned()
        } else {
            path_rhs
        };
        scope
            .path_raw
            .entry(var_name.to_owned())
            .or_default()
            .push((line_no, path_value));
        let expanded_rhs = if flavor == VariableFlavor::Simple {
            expand_immediate_locals(value, &scope, MAX_DEPTH_FOR_IMMEDIATE_EXPANSION)
        } else {
            value.to_owned()
        };
        let expanded = if kind == AssignmentKind::Append {
            match scope.latest_raw(var_name) {
                Some(old) if !old.is_empty() && !expanded_rhs.is_empty() => {
                    format!("{old} {expanded_rhs}")
                }
                Some(old) if !old.is_empty() => old.to_owned(),
                _ => expanded_rhs,
            }
        } else {
            expanded_rhs
        };

        let values: Vec<String> = expanded
            .split_whitespace()
            .filter(|s| *s != "\\")
            .map(|s| s.replace(['"', '\\'], "").trim().to_owned())
            .filter(|s| keep_list_item(s))
            .collect();
        scope
            .raw
            .entry(var_name.to_owned())
            .or_default()
            .push((line_no, expanded.trim().to_owned()));
        scope
            .assignments
            .entry(var_name.to_owned())
            .or_default()
            .push((line_no, values));
        flavors.insert(var_name.to_owned(), flavor);
    }

    (scope, line_states)
}

/// Returns the bounded variable names nested in Make references. Function and
/// substitution bodies are scanned recursively so uncertainty can propagate
/// through a simply-expanded assignment without evaluating Make functions.
fn scoped_variable_references(value: &str) -> Vec<String> {
    fn collect(value: &str, depth: usize, names: &mut Vec<String>) {
        if depth == 0 {
            return;
        }
        let mut cursor = 0usize;
        while cursor < value.len() {
            let Some(relative) = value[cursor..].find('$') else {
                return;
            };
            let dollar = cursor + relative;
            let Some(open) = value.as_bytes().get(dollar + 1).copied() else {
                return;
            };
            if open == b'$' {
                cursor = dollar + 2;
                continue;
            }
            let close = match open {
                b'(' => b')',
                b'{' => b'}',
                _ => {
                    cursor = dollar + 1;
                    continue;
                }
            };
            let mut closing = vec![close];
            let mut end = dollar + 2;
            while end < value.len() {
                if value.as_bytes()[end] == b'$' {
                    match value.as_bytes().get(end + 1) {
                        Some(b'(') => {
                            closing.push(b')');
                            end += 2;
                            continue;
                        }
                        Some(b'{') => {
                            closing.push(b'}');
                            end += 2;
                            continue;
                        }
                        _ => {}
                    }
                }
                if value.as_bytes()[end] == *closing.last().expect("reference delimiter") {
                    closing.pop();
                    if closing.is_empty() {
                        break;
                    }
                }
                end += 1;
            }
            if end == value.len() {
                return;
            }
            let body = value[dollar + 2..end].trim();
            if is_scoped_variable_name(body) {
                names.push(body.to_owned());
            } else if let Some((name, suffix)) = body.split_once(':') {
                let name = name.trim();
                if is_scoped_variable_name(name) {
                    names.push(name.to_owned());
                    collect(suffix, depth - 1, names);
                } else {
                    collect(body, depth - 1, names);
                }
            } else {
                collect(body, depth - 1, names);
            }
            cursor = end + 1;
        }
    }

    let mut names = Vec::new();
    collect(value, 16, &mut names);
    names.sort();
    names.dedup();
    names
}

fn is_scoped_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

/// Both Make spellings can be frozen by a simply-expanded assignment. Nested
/// references are visited too; unsupported function syntax remains unresolved.
fn scalar_references(value: &str) -> impl Iterator<Item = &str> {
    value.match_indices('$').filter_map(|(at, _)| {
        let reference = &value[at + 1..];
        let close = match reference.as_bytes().first() {
            Some(b'(') => ')',
            Some(b'{') => '}',
            _ => return None,
        };
        reference[1..].split_once(close).map(|(name, _)| name)
    })
}

/// Whether a word from a Make list is usable as a list item.
///
/// A slash used to disqualify one, which threw away most of what these lists
/// hold: a source name is routinely a path relative to the mmakefile, as in
/// `libudis86/decode` or `../locale`. 58 declarations came out with an empty
/// file list for that reason alone. An unresolved `$(...)` is still dropped,
/// since substituting nothing would silently compile the wrong set.
pub(crate) fn keep_list_item(s: &str) -> bool {
    if s.is_empty() || s.contains(',') {
        return false;
    }
    // A whole `$(VAR)` reference is kept, so expand_file_list can follow it:
    // `FILES := $(FILES) $(CLASSFILES)` has to survive collection or the list
    // it names is lost. A fragment carrying a stray paren is Make syntax the
    // tokeniser split apart and cannot be resolved.
    if s.starts_with("$(") && s.ends_with(')') && !s[2..s.len() - 1].contains(')') {
        return true;
    }
    !s.contains('$') && !s.contains(')')
}

#[cfg(test)]
mod configuration_tests {
    use super::*;
    use crate::dirs::DirVars;
    use crate::make_expr::{evaluate_make_expr, MakeExprContext};
    use std::path::Path;

    #[test]
    fn closed_target_selectors_seed_internal_make_configuration() {
        let context = TargetContext {
            make_variables: [("CONFIG_ONLY".into(), "config-value".into())].into(),
            cpu: Some("arm".into()),
            platform: Some("raspi".into()),
            family: Some("amiga".into()),
            variant: Some("debug".into()),
            toolchain: Some("gnu".into()),
            cpu32: Some("1".into()),
            use_mmu: Some("1".into()),
            float_abi: Some("hard".into()),
            mesa_version: Some("3".into()),
            target_llvm_ver: Some("20".into()),
            target_llvm_runtimes_style: Some("per-target".into()),
            target_rust: Some("1".into()),
            target_rust_ver: Some("1.85".into()),
            ..TargetContext::default()
        };
        let scope = collect_vars_with_context("", &context);

        for (name, expected) in [
            ("CPU", "arm"),
            ("AROS_TARGET_CPU", "arm"),
            ("ARCH", "raspi"),
            ("AROS_TARGET_ARCH", "raspi"),
            // configure.in: a variant replaces the machine except on pc.
            ("AROS_TARGET_PLATFORM", "debug-arm"),
            ("FAMILY", "amiga"),
            ("AROS_TARGET_FAMILY", "amiga"),
            ("AROS_TARGET_VARIANT", "debug"),
            ("AROS_TOOLCHAIN", "gnu"),
            ("AROS_TARGET_CPU32", "1"),
            ("USE_MMU", "1"),
            ("GCC_CONFIG_FLOAT_ABI", "hard"),
            ("OPT_MESAGL", "3"),
            ("TARGET_LLVM_VER", "20"),
            ("TARGET_LLVM_RUNTIMES_STYLE", "per-target"),
            ("TARGET_RUST", "1"),
            ("TARGET_RUST_VER", "1.85"),
        ] {
            assert_eq!(
                scope.raw_at(name, usize::MAX).as_deref(),
                Some(expected),
                "{name}"
            );
        }
        assert_eq!(
            scope.raw_at("CONFIG_ONLY", usize::MAX).as_deref(),
            Some("config-value")
        );
    }

    #[test]
    fn source_local_selector_assignment_overrides_only_its_alias() {
        let context = TargetContext {
            cpu: Some("arm".into()),
            ..TargetContext::default()
        };

        let cpu_scope = collect_vars_with_context("CPU := local-cpu\n", &context);
        assert_eq!(
            cpu_scope.raw_at("CPU", usize::MAX).as_deref(),
            Some("local-cpu")
        );
        assert_eq!(
            cpu_scope.raw_at("AROS_TARGET_CPU", usize::MAX).as_deref(),
            Some("arm")
        );

        let target_cpu_scope =
            collect_vars_with_context("AROS_TARGET_CPU := local-target-cpu\n", &context);
        assert_eq!(
            target_cpu_scope.raw_at("CPU", usize::MAX).as_deref(),
            Some("arm")
        );
        assert_eq!(
            target_cpu_scope
                .raw_at("AROS_TARGET_CPU", usize::MAX)
                .as_deref(),
            Some("local-target-cpu")
        );
    }

    #[test]
    fn absent_selectors_stay_absent_and_unknown_or_opaque_locals_block_fallback() {
        let context = TargetContext {
            // Selector aliases are internal projections, not contract-map
            // values. A malformed direct caller cannot inject them here.
            make_variables: [
                ("CPU".into(), "untrusted-cpu".into()),
                ("AROS_TARGET_CPU".into(), "untrusted-target-cpu".into()),
            ]
            .into(),
            ..TargetContext::default()
        };
        let empty_scope = collect_vars_with_context("", &context);
        for selector in CLOSED_TARGET_MAKE_SELECTORS {
            assert_eq!(empty_scope.raw_at(selector, usize::MAX), None, "{selector}");
        }
        assert_eq!(empty_scope.raw_at("AROS_HOST_ARCH", usize::MAX), None);

        let temp = tempfile::tempdir().expect("temporary source root");
        let dirs = DirVars::load(temp.path());
        let known_context = TargetContext {
            cpu: Some("arm".into()),
            ..TargetContext::default()
        };
        let unresolved_conditional =
            "ifeq ($(UNRESOLVED_SELECTOR),yes)\nCPU := branch-cpu\nendif\n";
        let conditional_scope = collect_vars_with_context(unresolved_conditional, &known_context);
        assert!(conditional_scope.conditionally_assigned_before("CPU", usize::MAX));
        let conditional_expr = MakeExprContext::new(
            &conditional_scope,
            &dirs,
            usize::MAX,
            temp.path(),
            Path::new("."),
        );
        assert!(evaluate_make_expr("$(CPU)", &conditional_expr).is_err());

        let opaque_scope = collect_vars_with_context("define CPU\nopaque\nendef\n", &known_context);
        assert!(opaque_scope.conditionally_assigned_before("CPU", usize::MAX));
        let opaque_expr = MakeExprContext::new(
            &opaque_scope,
            &dirs,
            usize::MAX,
            temp.path(),
            Path::new("."),
        );
        assert!(evaluate_make_expr("$(CPU)", &opaque_expr).is_err());
    }

    #[test]
    fn quoted_configured_empty_value_is_not_an_unquoted_empty_value() {
        // geninc.cfg.in keeps quotes around ENABLE_EXECSMP. GNU Make does
        // not remove those quotes from its variable value or equality RHS.
        let quoted =
            "EXECSMP=\"\"\nifneq ($(strip $(EXECSMP)),\"\")\nSMP := yes\nelse\nSMP := no\nendif\n";
        let (scope, _) = collect_vars_impl(quoted, Some(&TargetContext::default()));
        assert_eq!(scope.raw_at("SMP", usize::MAX).as_deref(), Some("no"));
        let unquoted = quoted.replacen("EXECSMP=\"\"", "EXECSMP=", 1);
        let (scope, _) = collect_vars_impl(&unquoted, Some(&TargetContext::default()));
        assert_eq!(scope.raw_at("SMP", usize::MAX).as_deref(), Some("yes"));
    }

    #[test]
    fn make_error_guards_stop_the_proof_only_when_they_may_run() {
        let guarded = "B ?= d1001\nifeq ($(B),bad)\n$(error bad board)\nendif\nifeq ($(B),d1001)\nF := one\nelse\n$(error unsupported $(B))\nendif\n";
        let known = TargetContext {
            make_variables: [("B".into(), "d1001".into())].into(),
            ..TargetContext::default()
        };
        let (scope, _) = collect_vars_impl(guarded, Some(&known));
        assert_eq!(scope.raw_at("F", usize::MAX).as_deref(), Some("one"));

        let rejected = TargetContext {
            make_variables: [("B".into(), "bad".into())].into(),
            ..TargetContext::default()
        };
        let (scope, states) = collect_vars_impl(guarded, Some(&rejected));
        assert_eq!(scope.raw_at("F", usize::MAX), None);
        assert!(states[4..]
            .iter()
            .all(|state| *state == ConditionalTruth::Unknown));

        let unknown = "ifeq ($(UNSET_SELECTOR),x)\n$(error stop)\nendif\nF := two\n";
        let (scope, _) = collect_vars_impl(unknown, Some(&TargetContext::default()));
        assert_eq!(scope.raw_at("F", usize::MAX), None);

        assert!(is_make_error_directive("$(error bad)"));
        assert!(!is_make_error_directive("$(errors x)"));
        assert!(!is_make_error_directive("X := $(error bad)"));
    }

    #[test]
    fn legacy_platform_follows_configure_variant_rule() {
        let context = |platform: &str, variant: Option<&str>| TargetContext {
            platform: Some(platform.into()),
            cpu: Some("riscv".into()),
            variant: variant.map(str::to_owned),
            ..TargetContext::default()
        };
        assert_eq!(
            context("esp32p4", Some("")).legacy_platform().as_deref(),
            Some("esp32p4-riscv")
        );
        assert_eq!(
            context("esp32p4", Some("smp")).legacy_platform().as_deref(),
            Some("smp-riscv")
        );
        assert_eq!(
            context("pc", Some("smp")).legacy_platform().as_deref(),
            Some("pc-riscv")
        );
        assert_eq!(context("esp32p4", None).legacy_platform(), None);
        assert_eq!(
            context("esp32p4", Some("smp"))
                .value_of("AROS_TARGET_PLATFORM")
                .as_deref(),
            Some("smp-riscv")
        );
    }

    #[test]
    fn configured_smp_value_is_cut_at_its_make_comment() {
        // configure substitutes "#define __AROSEXEC_SMP__" into geninc.cfg.in.
        // GNU Make ends the value at '#', so EXECSMP is a single quote and
        // compiler/include selects its SMP execbase lane.
        let source = "EXECSMP=\"#define __AROSEXEC_SMP__\"\nifneq ($(strip $(EXECSMP)),\"\")\nSMP := yes\nelse\nSMP := no\nendif\n";
        let (scope, _) = collect_vars_impl(source, Some(&TargetContext::default()));
        assert_eq!(scope.raw_at("EXECSMP", usize::MAX).as_deref(), Some("\""));
        assert_eq!(scope.raw_at("SMP", usize::MAX).as_deref(), Some("yes"));
    }

    #[test]
    fn directive_quotes_do_not_strip_quotes_from_expanded_variables() {
        let source = "VALUE=\"x\"\n";
        let context = TargetContext::default();
        let (scope, _) = collect_vars_impl(source, Some(&context));
        for (arguments, expected) in [
            ("($(VALUE),\"x\")", ConditionalTruth::True),
            ("($(VALUE),x)", ConditionalTruth::False),
            ("\"$(VALUE)\" 'x'", ConditionalTruth::False),
            ("'$(VALUE)' '\"x\"'", ConditionalTruth::True),
            ("\"x\" 'x'", ConditionalTruth::True),
        ] {
            assert_eq!(
                evaluate_conditional("ifeq", arguments, &scope, &context, usize::MAX),
                expected,
                "{arguments}"
            );
        }
    }
    #[test]
    fn recipe_commands_cannot_change_make_conditionals_or_variable_scope() {
        let context = TargetContext::default();
        let source = "ifeq (0,1)\n\telse\n#MM false-consumer : copy-owner\n\tendif\nFILES := inactive\nendif\nFILES := genuine\n\tFILES := shell-data\n\tundefine FILES\n";
        let (scope, states) = collect_vars_impl(source, Some(&context));
        assert_eq!(states[2], ConditionalTruth::False);
        assert_eq!(states[4], ConditionalTruth::False);
        assert_eq!(
            scope.raw_at("FILES", usize::MAX).as_deref(),
            Some("genuine")
        );
        assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));

        let unconfigured = collect_vars(source);
        assert_eq!(
            unconfigured.raw_at("FILES", usize::MAX).as_deref(),
            Some("genuine")
        );
        let (forward, forward_states) =
            collect_vars_impl_with_forward_locals(source, Some(&context), true);
        assert_eq!(forward_states[2], ConditionalTruth::False);
        assert_eq!(
            forward.raw_at("FILES", usize::MAX).as_deref(),
            Some("genuine")
        );
    }

    #[test]
    fn define_bodies_are_inert_across_value_path_and_line_state_scans() {
        let context = TargetContext {
            make_variables: [
                ("LOCAL".into(), "configured".into()),
                ("NESTED".into(), "configured-nested".into()),
            ]
            .into(),
            ..TargetContext::default()
        };
        let source = "FILES := before\n\
define TEMPLATE\n\
LOCAL := from-define\n\
# HIDDEN_COMMENT := from-comment\n\
\telse\n\
\tendif\n\
\tFILES := from-recipe\n\
ifeq (0,1)\n\
override export private define NESTED\n\
FILES := from-nested-define\n\
endef\n\
FILES := still-in-define\n\
endef\n\
FROZEN := $(LOCAL)\n\
FROZEN_NESTED := $(NESTED)\n\
FILES := outside\n";
        let outside_line = source
            .lines()
            .position(|line| line == "FILES := outside")
            .expect("outside assignment");
        let frozen_line = source
            .lines()
            .position(|line| line == "FROZEN := $(LOCAL)")
            .expect("frozen assignment");
        let nested_frozen_line = source
            .lines()
            .position(|line| line == "FROZEN_NESTED := $(NESTED)")
            .expect("nested frozen assignment");

        let (context_scope, context_states) = collect_vars_impl(source, Some(&context));
        let (forward_scope, forward_states) =
            collect_vars_impl_with_forward_locals(source, Some(&context), true);
        let (context_free_scope, context_free_states) = collect_vars_impl(source, None);

        for (scope, states) in [
            (&context_scope, &context_states),
            (&forward_scope, &forward_states),
            (&context_free_scope, &context_free_states),
        ] {
            assert_eq!(
                scope.raw_at("FILES", usize::MAX).as_deref(),
                Some("outside")
            );
            assert_eq!(states.len(), source.lines().count());
            assert_eq!(states[outside_line], ConditionalTruth::True);
            assert_eq!(states[frozen_line], ConditionalTruth::True);
            assert_eq!(states[nested_frozen_line], ConditionalTruth::True);
            assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));
        }
        assert_eq!(
            context_scope.path_raw_at("FROZEN", usize::MAX).as_deref(),
            Some("configured")
        );
        assert_eq!(
            forward_scope.path_raw_at("FROZEN", usize::MAX).as_deref(),
            Some("configured")
        );
        assert_eq!(
            context_scope
                .path_raw_at("FROZEN_NESTED", usize::MAX)
                .as_deref(),
            Some("configured-nested")
        );
        assert_eq!(
            forward_scope
                .path_raw_at("FROZEN_NESTED", usize::MAX)
                .as_deref(),
            Some("configured-nested")
        );
        assert_eq!(
            context_free_scope
                .path_raw_at("FROZEN", usize::MAX)
                .as_deref(),
            Some("$(LOCAL)")
        );
        assert_eq!(
            context_free_scope
                .path_raw_at("FROZEN_NESTED", usize::MAX)
                .as_deref(),
            Some("$(NESTED)")
        );
        assert!(!context_scope.is_known_local("LOCAL"));
        assert!(!forward_scope.is_known_local("LOCAL"));
        assert!(!forward_scope.is_known_local("HIDDEN_COMMENT"));
    }

    #[test]
    fn definitions_in_false_or_unknown_branches_do_not_assign_or_mark_locals() {
        let context = TargetContext::default();
        let source = "ifeq (0,1)\n\
define FALSE_TEMPLATE\n\
FILES := from-false-definition\n\
FALSE_LOCAL := yes\n\
endef\n\
endif\n\
ifeq ($(UNKNOWN),enabled)\n\
define UNKNOWN_TEMPLATE\n\
FILES := from-unknown-definition\n\
UNKNOWN_LOCAL := yes\n\
endef\n\
endif\n\
FILES := outside\n";
        let (scope, states) = collect_vars_impl_with_forward_locals(source, Some(&context), true);
        let (context_free, context_free_states) = collect_vars_impl(source, None);
        let false_line = source
            .lines()
            .position(|line| line == "define FALSE_TEMPLATE")
            .expect("false define line");
        let unknown_line = source
            .lines()
            .position(|line| line == "define UNKNOWN_TEMPLATE")
            .expect("unknown define line");
        let outside_line = source
            .lines()
            .position(|line| line == "FILES := outside")
            .expect("outside assignment");

        assert_eq!(states[false_line], ConditionalTruth::False);
        assert_eq!(states[unknown_line], ConditionalTruth::Unknown);
        assert_eq!(states[outside_line], ConditionalTruth::True);
        assert_eq!(context_free_states[outside_line], ConditionalTruth::True);
        for scope in [&scope, &context_free] {
            assert_eq!(
                scope.raw_at("FILES", usize::MAX).as_deref(),
                Some("outside")
            );
            assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));
        }
        assert!(!scope.is_known_local("FALSE_LOCAL"));
        assert!(!scope.is_known_local("UNKNOWN_LOCAL"));
    }

    #[test]
    fn malformed_define_bodies_remain_suppressed_through_eof() {
        let context = TargetContext::default();
        for source in [
            "define OPEN\nFILES := hidden\nLEAKED := hidden\n",
            "define MALFORMED_END\nFILES := hidden\nendef extra\nFILES := leaked\n",
        ] {
            let (scope, states) =
                collect_vars_impl_with_forward_locals(source, Some(&context), true);
            let (context_free, context_free_states) = collect_vars_impl(source, None);
            assert_eq!(states.len(), source.lines().count());
            assert_eq!(context_free_states.len(), source.lines().count());
            for scope in [&scope, &context_free] {
                assert_eq!(scope.raw_at("FILES", usize::MAX), None);
                assert_eq!(scope.raw_at("LEAKED", usize::MAX), None);
            }
            assert!(!scope.is_known_local("FILES"));
            assert!(!scope.is_known_local("LEAKED"));
        }
    }

    #[test]
    fn active_define_headers_shadow_old_and_configured_values_until_replaced() {
        let context = TargetContext {
            make_variables: [("FILES".into(), "configured".into())].into(),
            ..TargetContext::default()
        };
        for (source, previous) in [
            (
                "FILES := previous\ndefine FILES\nfrom-body\nendef\nifdef FILES\nendif\n",
                "previous",
            ),
            (
                "FILES := previous\noverride export define FILES\nfrom-body\nendef\nifdef FILES\nendif\n",
                "previous",
            ),
            (
                "FILES := previous\noverride export private define FILES\nfrom-body\nendef\nifdef FILES\nendif\n",
                "previous",
            ),
            (
                "define FILES\nfrom-body\nendef\nifdef FILES\nendif\n",
                "configured",
            ),
        ] {
            let scope = collect_vars_with_context(source, &context);
            let ifdef_line = source
                .lines()
                .position(|line| line == "ifdef FILES")
                .expect("ifdef line");
            let define_line = source
                .lines()
                .position(|line| line.ends_with("define FILES"))
                .expect("define line");
            assert_eq!(
                scope.raw_at("FILES", define_line).as_deref(),
                Some(previous)
            );
            assert_eq!(scope.raw_at("FILES", usize::MAX), None);
            assert_eq!(scope.path_raw_at("FILES", usize::MAX), None);
            assert!(!scope.snapshot(usize::MAX).contains_key("FILES"));
            assert!(scope.conditionally_assigned_before("FILES", usize::MAX));
            assert_eq!(
                evaluate_conditional("ifdef", "FILES", &scope, &context, ifdef_line),
                ConditionalTruth::Unknown
            );
        }

        let inactive =
            "FILES := known\nifeq (0,1)\noverride export define FILES\nfrom-body\nendef\nendif\n";
        let inactive_scope = collect_vars_with_context(inactive, &context);
        assert_eq!(
            inactive_scope.raw_at("FILES", usize::MAX).as_deref(),
            Some("known")
        );
        assert!(!inactive_scope.conditionally_assigned_before("FILES", usize::MAX));

        let inactive_default =
            "ifeq (1,0)\noverride export private define FILES\nfrom-body\nendef\nendif\n";
        let inactive_default_scope = collect_vars_with_context(inactive_default, &context);
        assert_eq!(
            inactive_default_scope
                .raw_at("FILES", usize::MAX)
                .as_deref(),
            Some("configured")
        );
        assert!(!inactive_default_scope.conditionally_assigned_before("FILES", usize::MAX));

        let unknown = "ifeq ($(UNKNOWN),1)\ndefine FILES\nfrom-body\nendef\nendif\n";
        let unknown_scope = collect_vars_with_context(unknown, &context);
        assert_eq!(unknown_scope.raw_at("FILES", usize::MAX), None);
        assert!(unknown_scope.conditionally_assigned_before("FILES", usize::MAX));

        let replaced = "FILES := previous\ndefine FILES\nfrom-body\nendef\nFILES := replacement\n";
        let replaced_scope = collect_vars_with_context(replaced, &context);
        assert_eq!(
            replaced_scope.raw_at("FILES", usize::MAX).as_deref(),
            Some("replacement")
        );
        assert_eq!(
            replaced_scope.path_raw_at("FILES", usize::MAX).as_deref(),
            Some("replacement")
        );
        assert!(!replaced_scope.conditionally_assigned_before("FILES", usize::MAX));

        let alias_source = "FILES := previous\ndefine FILES\nfrom-body\nendef\nALIAS := $(FILES)\nFILES := replacement\nUSER_CPPFLAGS := $(ALIAS)\n";
        for forward_locals in [false, true] {
            let (alias_scope, _) =
                collect_vars_impl_with_forward_locals(alias_source, Some(&context), forward_locals);
            assert_eq!(
                alias_scope.raw_at("FILES", usize::MAX).as_deref(),
                Some("replacement")
            );
            assert_eq!(alias_scope.raw_at("ALIAS", usize::MAX), None);
            assert_eq!(alias_scope.raw_at("USER_CPPFLAGS", usize::MAX), None);
            assert!(alias_scope.conditionally_assigned_before("ALIAS", usize::MAX));
            assert!(alias_scope.conditionally_assigned_before("USER_CPPFLAGS", usize::MAX));
            let flags = crate::flags::collect_flags_at(&alias_scope, usize::MAX);
            assert!(flags.skipped.contains(&"$(USER_CPPFLAGS)".to_owned()));
        }

        let dynamic = "define $(DYNAMIC_NAME)\nvalue\nendef\n";
        let dynamic_scope = collect_vars_with_context(dynamic, &context);
        assert_eq!(dynamic_scope.raw_at("FILES", usize::MAX), None);
        assert!(dynamic_scope.conditionally_assigned_before("FILES", usize::MAX));
    }

    #[test]
    fn unconditional_replacement_clears_conditional_guard_but_not_frozen_aliases() {
        let context = TargetContext::default();
        let replacement = "FILES := initial\n\
ifeq ($(UNKNOWN),enabled)\n\
FILES := conditional\n\
else\n\
FILES := alternative\n\
endif\n\
FILES := proven\n\
ALIAS := $(FILES)\n\
FILES := later\n";
        for (scope, _) in [
            collect_vars_impl(replacement, None),
            collect_vars_impl(replacement, Some(&context)),
            collect_vars_impl_with_forward_locals(replacement, Some(&context), true),
        ] {
            assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));
            assert!(!scope.path_is_conditional_at("FILES", usize::MAX));
            assert_eq!(scope.raw_at("FILES", usize::MAX).as_deref(), Some("later"));
            assert!(!scope.conditionally_assigned_before("ALIAS", usize::MAX));
            assert_eq!(scope.raw_at("ALIAS", usize::MAX).as_deref(), Some("proven"));
        }

        let frozen_alias = "FILES := initial\n\
ifeq ($(UNKNOWN),enabled)\n\
FILES := conditional\n\
endif\n\
ALIAS := $(FILES)\n\
FILES := proven\n\
USER_CPPFLAGS := $(ALIAS)\n";
        for (scope, _) in [
            collect_vars_impl(frozen_alias, None),
            collect_vars_impl(frozen_alias, Some(&context)),
            collect_vars_impl_with_forward_locals(frozen_alias, Some(&context), true),
        ] {
            assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));
            assert_eq!(scope.raw_at("FILES", usize::MAX).as_deref(), Some("proven"));
            assert!(scope.conditionally_assigned_before("ALIAS", usize::MAX));
            assert_eq!(scope.raw_at("ALIAS", usize::MAX), None);
            assert!(scope.conditionally_assigned_before("USER_CPPFLAGS", usize::MAX));
            assert_eq!(scope.raw_at("USER_CPPFLAGS", usize::MAX), None);
            let flags = crate::flags::collect_flags_at(&scope, usize::MAX);
            assert!(flags.skipped.contains(&"$(USER_CPPFLAGS)".to_owned()));
            assert!(!flags.defines.contains(&"INITIAL".to_owned()));
        }

        let reset_flags = "USER_CPPFLAGS := -DINITIAL\n\
ifeq ($(UNKNOWN),enabled)\n\
USER_CPPFLAGS := -DOPTIONAL\n\
endif\n\
USER_CPPFLAGS := -DRESET\n";
        for (scope, _) in [
            collect_vars_impl(reset_flags, None),
            collect_vars_impl(reset_flags, Some(&context)),
            collect_vars_impl_with_forward_locals(reset_flags, Some(&context), true),
        ] {
            assert!(!scope.conditionally_assigned_before("USER_CPPFLAGS", usize::MAX));
            let flags = crate::flags::collect_flags_at(&scope, usize::MAX);
            assert!(flags.defines.contains(&"RESET".to_owned()));
            assert!(!flags.skipped.contains(&"$(USER_CPPFLAGS)".to_owned()));
        }
    }

    #[test]
    fn later_reset_cannot_resolve_an_earlier_uncertain_ifdef() {
        let context = TargetContext {
            make_variables: [("FEATURE".into(), String::new())].into(),
            ..TargetContext::default()
        };
        let text = "ifeq ($(UNKNOWN),x)\nFEATURE := 1\nendif\nifdef FEATURE\nSELECTED := maybe\nendif\nFEATURE :=\n";
        let scope = collect_vars_with_context(text, &context);
        assert!(matches!(
            evaluate_conditional("ifdef", "FEATURE", &scope, &context, 3),
            ConditionalTruth::Unknown
        ));
        assert!(matches!(
            evaluate_conditional("ifdef", "FEATURE", &scope, &context, 7),
            ConditionalTruth::False
        ));
    }

    #[test]
    fn ifeq_recursive_expansion_is_line_scoped_and_reset_aware() {
        let context = TargetContext {
            make_variables: [("FEATURE".into(), String::new())].into(),
            ..TargetContext::default()
        };
        let text = "ifeq ($(UNKNOWN),x)\nFEATURE := 1\nendif\nifeq ($(strip $(FEATURE)),1)\nendif\nFEATURE := 1\nifeq ($(strip $(FEATURE)),1)\nendif\nFEATURE := 0\nifeq ($(FEATURE),1)\n";
        let scope = collect_vars_with_context(text, &context);

        assert!(matches!(
            evaluate_conditional("ifeq", "($(strip $(FEATURE)),1)", &scope, &context, 3),
            ConditionalTruth::Unknown
        ));
        assert!(matches!(
            evaluate_conditional("ifeq", "($(strip $(FEATURE)),1)", &scope, &context, 6),
            ConditionalTruth::True
        ));
        assert!(matches!(
            evaluate_conditional("ifeq", "($(FEATURE),1)", &scope, &context, 9),
            ConditionalTruth::False
        ));
    }

    #[test]
    fn first_line_path_assignment_freezes_configured_default_before_local_override() {
        let context = TargetContext {
            make_variables: [("CURRENT_DEVICE".into(), "timer".into())].into(),
            ..TargetContext::default()
        };
        let text = "FROZEN_DEVICE := $(CURRENT_DEVICE)\nCURRENT_DEVICE := unproven\n";
        let scope = collect_vars_with_context(text, &context);

        assert_eq!(
            scope.path_raw_at("FROZEN_DEVICE", 1).as_deref(),
            Some("timer")
        );
        assert_eq!(
            scope.path_raw_at("CURRENT_DEVICE", 1).as_deref(),
            Some("timer")
        );
        assert_eq!(
            scope.path_raw_at("CURRENT_DEVICE", 2).as_deref(),
            Some("unproven")
        );
    }

    #[test]
    fn undefine_clears_a_source_value_and_allows_later_set_if_unset() {
        let scope =
            collect_vars("VALUE := before\nundefine VALUE\nVALUE ?= fallback\nVALUE = rebound\n");

        assert_eq!(scope.raw_at("VALUE", 1).as_deref(), Some("before"));
        assert_eq!(scope.raw_at("VALUE", 2).as_deref(), Some(""));
        assert!(!scope.snapshot(2).contains_key("VALUE"));
        assert_eq!(scope.raw_at("VALUE", 3).as_deref(), Some("fallback"));
        assert_eq!(scope.raw_at("VALUE", 4).as_deref(), Some("rebound"));
        assert_eq!(
            scope.snapshot(3).get("VALUE"),
            Some(&vec!["fallback".to_owned()])
        );
    }

    #[test]
    fn undefine_masks_target_defaults_and_undefined_append_is_recursive() {
        let context = TargetContext {
            make_variables: [
                ("TARGET_DEFAULT".into(), "-target".into()),
                ("APPENDED".into(), "-configured".into()),
            ]
            .into(),
            ..TargetContext::default()
        };
        let text = "undefine TARGET_DEFAULT\nTARGET_DEFAULT ?= -source\nLATER := -early\nAPPENDED := -before\nundefine APPENDED\nAPPENDED += $(LATER)\nLATER := -late\n";
        let scope = collect_vars_with_context(text, &context);

        assert_eq!(scope.raw_at("TARGET_DEFAULT", 1).as_deref(), Some(""));
        assert_eq!(
            scope.raw_at("TARGET_DEFAULT", 2).as_deref(),
            Some("-source")
        );
        assert_eq!(scope.raw_at("APPENDED", 6).as_deref(), Some("$(LATER)"));
        assert_eq!(scope.raw_at("LATER", 6).as_deref(), Some("-early"));
        assert_eq!(scope.raw_at("APPENDED", 7).as_deref(), Some("$(LATER)"));
    }

    #[test]
    fn undefine_is_empty_when_expanded_immediately_and_later_rebinding_is_separate() {
        let context = TargetContext {
            make_variables: [("VALUE".into(), "-target".into())].into(),
            ..TargetContext::default()
        };
        let text = "undefine VALUE\nFROZEN := $(VALUE)\nVALUE ?= -fallback\n";
        let scope = collect_vars_with_context(text, &context);

        assert_eq!(scope.raw_at("FROZEN", 3).as_deref(), Some(""));
        assert_eq!(scope.path_raw_at("FROZEN", 3).as_deref(), Some(""));
        assert_eq!(scope.raw_at("VALUE", 2).as_deref(), Some(""));
        assert_eq!(scope.raw_at("VALUE", 3).as_deref(), Some("-fallback"));

        let text = "undefine VALUE\nVALUE ?= -restored\nFROZEN := $(VALUE)\nVALUE := -later\n";
        let scope = collect_vars_with_context(text, &context);
        assert_eq!(scope.raw_at("FROZEN", 4).as_deref(), Some("-restored"));
        assert_eq!(scope.raw_at("VALUE", 3).as_deref(), Some("-restored"));
        assert_eq!(scope.raw_at("VALUE", 4).as_deref(), Some("-later"));
    }

    #[test]
    fn configured_append_references_are_flavor_uncertain_but_literal_append_is_not() {
        let context = TargetContext {
            make_variables: [("VALUE".into(), "-configured".into())].into(),
            ..TargetContext::default()
        };
        let text = "LATER := -early\nVALUE += $(LATER)\nLATER := -late\n";
        let scope = collect_vars_with_context(text, &context);
        assert!(scope
            .flavor_uncertainty_reason_at("VALUE", 3)
            .is_some_and(|reason| reason.contains("configuration value")));

        let literal = collect_vars_with_context("VALUE += -literal\n", &context);
        assert_eq!(literal.flavor_uncertainty_reason_at("VALUE", 1), None);
        assert_eq!(
            literal.raw_at("VALUE", 1).as_deref(),
            Some("-configured -literal")
        );
    }

    #[test]
    fn source_assignment_establishes_flavor_and_simple_freezes_uncertain_values() {
        let context = TargetContext {
            make_variables: [("VALUE".into(), "-configured".into())].into(),
            ..TargetContext::default()
        };
        let simple = "VALUE := -source\nLATER := -early\nVALUE += $(LATER)\nLATER := -late\n";
        let scope = collect_vars_with_context(simple, &context);
        assert_eq!(scope.raw_at("VALUE", 4).as_deref(), Some("-source -early"));
        assert_eq!(scope.flavor_uncertainty_reason_at("VALUE", 4), None);

        let recursive = "VALUE = -source\nLATER := -early\nVALUE += $(LATER)\nLATER := -late\n";
        let scope = collect_vars_with_context(recursive, &context);
        assert_eq!(
            scope.raw_at("VALUE", 4).as_deref(),
            Some("-source $(LATER)")
        );
        assert_eq!(scope.flavor_uncertainty_reason_at("VALUE", 4), None);

        let transitive = "LATER := -early\nVALUE += $(LATER)\nFROZEN := $(VALUE)\nLATER := -late\n";
        let scope = collect_vars_with_context(transitive, &context);
        assert!(scope
            .flavor_uncertainty_reason_at("FROZEN", 4)
            .is_some_and(|reason| reason.contains("freezes a value")));

        let reset = "LATER := -early\nVALUE += $(LATER)\nVALUE := -reset\n";
        let scope = collect_vars_with_context(reset, &context);
        assert_eq!(scope.flavor_uncertainty_reason_at("VALUE", 3), None);
        assert_eq!(scope.raw_at("VALUE", 3).as_deref(), Some("-reset"));
    }

    #[test]
    fn unsupported_dynamic_undefine_forms_are_rejected() {
        assert_eq!(undefine_directive("undefine VALUE"), Ok(Some("VALUE")));
        assert!(undefine_directive("undefine $(NAME)").is_err());
        assert!(undefine_directive("undefine VALUE OTHER").is_err());
    }
}
