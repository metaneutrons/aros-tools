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
#[derive(Clone)]
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

mod program;
pub use program::{
    validate_captured_source_commands, CapturedShellSearch, MakeFileEvent, MakeFileEventKind,
    MakeFileProgram, MakeIncludeResolver, MakeInitialFlavor, MakeInitialValue, MakeProgramPaths,
    MakeRecipePrefixes, MakeRecipeProjection, MakeRecipeShellProjection,
    MakeResolvedIncludeResolver, MakeResolvedInput, MakeTargetRecipeRequest, MakeVariableEvent,
    MakeVariableProgram,
};

#[cfg(test)]
mod program_tests;

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
        self.raw_at_ref(name, line).map(str::to_owned)
    }

    fn raw_at_ref(&self, name: &str, line: usize) -> Option<&str> {
        if self.has_opaque_assignment_before(name, line)
            || self.has_uncertain_path_value_before(name, line)
        {
            return None;
        }
        self.raw
            .get(name)
            .and_then(|history| history.iter().rev().find(|(at, _)| *at < line))
            .map(|(_, value)| value.as_str())
            .or_else(|| self.configuration.get(name).map(String::as_str))
    }

    /// A simple (`:=`) assignment that froze a reference to a variable which
    /// was unresolved at that point: its text as written and the line it was
    /// assigned on. `raw_at` refuses such a value, which is right for a path
    /// but loses every literal token of a flag string. A caller expanding the
    /// text at the assignment line keeps those tokens and still sees the
    /// unresolved reference as unresolved. A conditional or opaque assignment
    /// of the variable itself is never returned.
    pub(crate) fn frozen_raw_at(&self, name: &str, line: usize) -> Option<(String, usize)> {
        if self.has_opaque_assignment_before(name, line)
            || !self.has_uncertain_path_value_before(name, line)
        {
            return None;
        }
        let reset = self
            .path_replacements
            .get(name)
            .and_then(|history| history.iter().rev().find(|at| **at < line).copied());
        let conditional = self
            .conditional_assignments
            .get(name)
            .is_some_and(|history| {
                history
                    .iter()
                    .any(|(at, _)| *at < line && reset.is_none_or(|reset| *at > reset))
            });
        if conditional {
            return None;
        }
        self.raw
            .get(name)
            .and_then(|history| history.iter().rev().find(|(at, _)| *at < line))
            .map(|(at, value)| (value.clone(), *at))
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
        self.flavor_uncertainty_reason_with(name, line, &|_| None)
    }

    /// Check uncertainty through exact raw overrides without copying history.
    pub(crate) fn flavor_uncertainty_reason_with(
        &self,
        name: &str,
        line: usize,
        raw_override: &dyn Fn(&str) -> Option<String>,
    ) -> Option<String> {
        self.flavor_uncertainty_reason_inner(name, line, 16, &mut HashSet::new(), raw_override)
    }

    fn flavor_uncertainty_reason_inner(
        &self,
        name: &str,
        line: usize,
        depth: usize,
        visiting: &mut HashSet<String>,
        raw_override: &dyn Fn(&str) -> Option<String>,
    ) -> Option<String> {
        let overridden = raw_override(name);
        if let Some(reason) = overridden
            .is_none()
            .then(|| {
                self.flavor_uncertainties
                    .get(name)
                    .and_then(|history| history.iter().rev().find(|(at, _)| *at < line))
                    .and_then(|(_, reason)| reason.as_ref())
            })
            .flatten()
        {
            return Some(reason.clone());
        }
        if depth == 0 || !visiting.insert(name.to_owned()) {
            return None;
        }
        let reason = overridden
            .or_else(|| self.raw_at(name, line))
            .and_then(|value| {
                scoped_variable_references(&value)
                    .into_iter()
                    .find_map(|dependency| {
                        self.flavor_uncertainty_reason_inner(
                            &dependency,
                            line,
                            depth - 1,
                            visiting,
                            raw_override,
                        )
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
    variable_assignment_impl(line, false)
}

fn variable_assignment_impl(
    line: &str,
    preserve_whitespace: bool,
) -> Option<(&str, &str, AssignmentKind)> {
    let trimmed = if preserve_whitespace {
        line.trim_start()
    } else {
        line.trim()
    };
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
    if !literal_variable_name(name) {
        return None;
    }
    let rhs = &trimmed[at + width..];
    Some((
        name,
        if preserve_whitespace {
            rhs.trim_start()
        } else {
            rhs.trim()
        },
        kind,
    ))
}

/// Shared admitted literal-name grammar for assignment/undefine/define readers.
pub(crate) fn literal_variable_name(name: &str) -> bool {
    // This GNU Make variable selects shell controls; an assignment is not a
    // dot/uppercase special-target rule. Other control-target policies remain
    // separately refused by the file/rule readers.
    name == ".SHELLFLAGS"
        || !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
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
    if literal_variable_name(name) {
        Ok(Some(name))
    } else {
        Err(())
    }
}

/// Find the first Make comment byte in a logical line.
///
/// Hashes inside balanced Make references and one-character references are
/// data, as are hashes escaped by an odd backslash run. `None` also means the
/// line has an unbalanced or over-deep reference, so callers must preserve it
/// rather than discard any suffix based on an uncertain boundary.
pub(crate) fn make_comment_start(bytes: &[u8]) -> Option<usize> {
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => {
                let start = cursor;
                while cursor < bytes.len() && bytes[cursor] == b'\\' {
                    cursor += 1;
                }
                if bytes.get(cursor) == Some(&b'#') {
                    if (cursor - start).is_multiple_of(2) {
                        return Some(cursor);
                    }
                    cursor += 1;
                }
            }
            b'$' => match bytes.get(cursor + 1) {
                Some(b'(' | b'{') => {
                    cursor = crate::make_expr::reference_end_bytes(bytes, cursor)? + 1;
                }
                Some(_) => cursor += 2,
                None => cursor += 1,
            },
            b'#' => return Some(cursor),
            _ => cursor += 1,
        }
    }
    None
}

/// Removes a Make comment from one continuation-joined logical line.
///
/// A `#` starts a comment even when attached to the preceding word, except
/// inside balanced variable/function references or a one-character reference.
/// An odd run of backslashes escapes the marker.
pub(crate) fn strip_make_comment(line: &str) -> &str {
    make_comment_start(line.as_bytes()).map_or(line, |at| &line[..at])
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

const MAX_CONDITION_VALUE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONDITION_WORK_BYTES: usize = 16 * 1024 * 1024;

fn condition_append(output: &mut String, value: &str, budget: &mut usize) -> Option<()> {
    if output.len().checked_add(value.len())? > MAX_CONDITION_VALUE_BYTES {
        return None;
    }
    *budget = budget.checked_sub(value.len().checked_add(1)?)?;
    output.push_str(value);
    Some(())
}

fn expand_condition_function(
    body: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
    line: usize,
    budget: &mut usize,
) -> Option<String> {
    let split = body.find(char::is_whitespace)?;
    let name = body[..split].trim();
    let args = body[split..].trim_start();
    match name {
        "word" => {
            let (index, words) = split_top_level_comma(args)?;
            let index = expand_condition_operand(index, scope, context, depth - 1, line, budget)?;
            if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let index = index.parse::<usize>().ok()?.checked_sub(1)?;
            let words = expand_condition_text(words, scope, context, depth - 1, line, budget)?;
            Some(
                words
                    .split_whitespace()
                    .nth(index)
                    .unwrap_or_default()
                    .to_owned(),
            )
        }
        "subst" => {
            let (from, remaining) = split_top_level_comma(args)?;
            let (to, text) = split_top_level_comma(remaining)?;
            let from = expand_condition_text(from, scope, context, depth - 1, line, budget)?;
            let to = expand_condition_text(to, scope, context, depth - 1, line, budget)?;
            let text = expand_condition_text(text, scope, context, depth - 1, line, budget)?;
            let replacements = if from.is_empty() {
                1
            } else {
                text.matches(&from).count()
            };
            let bytes = text
                .len()
                .checked_sub(replacements.checked_mul(from.len())?)?
                .checked_add(replacements.checked_mul(to.len())?)?;
            if bytes > MAX_CONDITION_VALUE_BYTES {
                return None;
            }
            Some(if from.is_empty() {
                text + &to
            } else {
                text.replace(&from, &to)
            })
        }
        "strip" => Some(
            expand_condition_operand(args, scope, context, depth - 1, line, budget)?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "findstring" => {
            let (needle, haystack) = split_top_level_comma(args)?;
            let needle = expand_condition_operand(needle, scope, context, depth - 1, line, budget)?;
            let haystack =
                expand_condition_operand(haystack, scope, context, depth - 1, line, budget)?;
            Some(if haystack.contains(&needle) {
                needle
            } else {
                String::new()
            })
        }
        "filter" | "filter-out" => {
            let (patterns, words) = split_top_level_comma(args)?;
            let patterns =
                expand_condition_operand(patterns, scope, context, depth - 1, line, budget)?;
            let words = expand_condition_operand(words, scope, context, depth - 1, line, budget)?;
            *budget = budget.checked_sub(
                patterns
                    .len()
                    .checked_mul(words.split_whitespace().count())?,
            )?;
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
    budget: &mut usize,
) -> Option<String> {
    let reference = body.trim();
    if !reference.is_empty()
        && reference.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        if scope.path_is_conditional_at(reference, line) {
            return None;
        }
        if let Some(value) = scope.raw_at_ref(reference, line) {
            return expand_condition_text(value, scope, context, depth - 1, line, budget);
        }
        if let Some(value) = context
            .value_bounded(reference, MAX_CONDITION_VALUE_BYTES.min(*budget))
            .ok()?
        {
            return expand_condition_text(&value, scope, context, depth - 1, line, budget);
        }
        return scope.local_names.contains(reference).then(String::new);
    }
    expand_condition_function(body.trim_start(), scope, context, depth, line, budget)
}

fn expand_condition_operand(
    raw: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
    line: usize,
    budget: &mut usize,
) -> Option<String> {
    expand_condition_text(raw, scope, context, depth, line, budget)
        .map(|value| value.trim().to_owned())
}

fn expand_condition_text(
    raw: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
    line: usize,
    budget: &mut usize,
) -> Option<String> {
    if depth == 0 || raw.len() > MAX_CONDITION_VALUE_BYTES {
        return None;
    }
    *budget = budget.checked_sub(raw.len().checked_add(1)?)?;
    let mut output = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    while cursor < raw.len() {
        let Some(relative) = raw[cursor..].find('$') else {
            condition_append(&mut output, &raw[cursor..], budget)?;
            break;
        };
        let dollar = cursor + relative;
        condition_append(&mut output, &raw[cursor..dollar], budget)?;
        let next = *raw.as_bytes().get(dollar + 1)?;
        if next == b'$' {
            condition_append(&mut output, "$", budget)?;
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
        let expanded = expand_condition_reference(
            &raw[dollar + 2..end],
            scope,
            context,
            depth - 1,
            line,
            budget,
        )?;
        condition_append(&mut output, &expanded, budget)?;
        cursor = end + 1;
    }
    Some(output)
}

pub(crate) fn evaluate_conditional(
    directive: &str,
    args: &str,
    scope: &VarScope,
    context: &TargetContext,
    line: usize,
) -> ConditionalTruth {
    let mut budget = MAX_CONDITION_WORK_BYTES;
    let value = match directive {
        "ifeq" | "ifneq" => equality_operands(args).and_then(|(left, right)| {
            Some(
                expand_condition_operand(
                    left,
                    scope,
                    context,
                    MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                    line,
                    &mut budget,
                )? == expand_condition_operand(
                    right,
                    scope,
                    context,
                    MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                    line,
                    &mut budget,
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
            scope
                .raw_at_ref(name, line)
                .map(|value| !value.is_empty())
                .or_else(|| context.value_nonempty(name))
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

pub(crate) fn make_define_directive(line: &str) -> Option<(&str, &str)> {
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

/// Reuse the ordinary Make scanner's inert define-body boundary in consumers
/// which project original expanded target-specific assignments.
pub(crate) fn make_define_suppression(joined: &str) -> Vec<bool> {
    scan_define_bodies(&joined.lines().collect::<Vec<_>>()).suppressed
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
    collect_vars_impl_projected(joined, context, forward_locals, None)
}

/// Opt-in exact expansion of active simply-expanded assignments.
///
/// The supplied evaluator sees the partial scope at the original source position, with no
/// future assignments. Returned strings are literal values, not Make syntax.
/// Existing collectors retain their original behavior through the default path.
///
/// # Errors
/// Returns the evaluator's first error without exposing the partial scope.
pub fn collect_vars_with_immediate_evaluator(
    joined: &str,
    context: &TargetContext,
    evaluate: &impl Fn(&str, &VarScope, usize) -> Result<String, String>,
) -> Result<(VarScope, Vec<Option<bool>>), String> {
    let failure = std::cell::RefCell::new(None);
    let project = |raw: &str, scope: &VarScope, line: usize| {
        if failure.borrow().is_some() {
            return String::new();
        }
        match evaluate(raw, scope, line) {
            Ok(value) => value.replace('$', "$$"),
            Err(error) => {
                *failure.borrow_mut() = Some(error);
                String::new()
            }
        }
    };
    let result = collect_vars_impl_projected(joined, Some(context), false, Some(&project));
    match failure.into_inner() {
        Some(error) => Err(error),
        None => Ok((
            result.0,
            result
                .1
                .into_iter()
                .map(|state| match state {
                    ConditionalTruth::True => Some(true),
                    ConditionalTruth::False => Some(false),
                    ConditionalTruth::Unknown => None,
                })
                .collect(),
        )),
    }
}

type ImmediateProjection<'a> = dyn Fn(&str, &VarScope, usize) -> String + 'a;

#[derive(Clone, Copy)]
struct AssignmentInput<'a> {
    /// The transactional driver can prove a skipped default assignment even
    /// in an unknown branch. Legacy collection retains its conservative guard.
    prove_default_noop: bool,
    line: &'a str,
    line_no: usize,
    branch_state: ConditionalTruth,
    selected: bool,
    path_names: &'a HashSet<String>,
    preserve_whitespace: bool,
    flavor: Option<VariableFlavor>,
    append_base: Option<&'a str>,
    /// A streaming define body is already delimited and must retain leading
    /// whitespace/newlines rather than being reparsed as assignment syntax.
    raw_value: Option<&'a str>,
}

#[cfg(test)]
#[path = "make_immediate_evaluator_tests.rs"]
mod immediate_evaluator_tests;

fn collect_vars_impl_projected(
    joined: &str,
    context: Option<&TargetContext>,
    forward_locals: bool,
    project: Option<&ImmediateProjection<'_>>,
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

        apply_assignment(
            &mut scope,
            &mut flavors,
            AssignmentInput {
                prove_default_noop: false,
                line,
                line_no,
                branch_state,
                selected: context.is_none() || branch_state == ConditionalTruth::True,
                path_names: &path_names,
                preserve_whitespace: false,
                flavor: None,
                append_base: None,
                raw_value: None,
            },
            project,
        );
    }

    (scope, line_states)
}

fn apply_assignment(
    scope: &mut VarScope,
    flavors: &mut HashMap<String, VariableFlavor>,
    input: AssignmentInput<'_>,
    project: Option<&ImmediateProjection<'_>>,
) {
    // Make has five assignment spellings here and the tree uses them.
    // Reading only `:=` lost every list written with `=` or `?=`:
    // rom/hidds/pci/pcitool declares `FILES = main pciids support locale`
    // that way, while the icon sets append to two lists with `+=`.
    let assignment = variable_assignment_impl(input.line, input.preserve_whitespace);
    let undefine = undefine_directive(input.line).ok().flatten();
    let (var_name, value, kind) = if let Some((name, value, kind)) = assignment {
        (name, value, kind)
    } else if let Some(name) = undefine {
        (name, "", AssignmentKind::Undefine)
    } else {
        return;
    };
    let value = input.raw_value.unwrap_or(value);
    scope.local_names.insert(var_name.to_owned());

    // A default assignment cannot alter an already proved definition, even
    // when its surrounding branch is unknown. Do not invent uncertainty for
    // that no-op. Earlier unresolved writes still require the normal guard.
    if input.prove_default_noop
        && kind == AssignmentKind::SetIfUnset
        && scope.is_defined_before(var_name, input.line_no)
        && !scope.conditionally_assigned_before(var_name, input.line_no)
    {
        return;
    }

    if input.branch_state == ConditionalTruth::Unknown {
        scope
            .conditional_assignments
            .entry(var_name.to_owned())
            .or_default()
            .push((input.line_no, kind));
    }
    if !input.selected {
        return;
    }

    if kind == AssignmentKind::SetIfUnset && scope.is_defined_before(var_name, input.line_no) {
        return;
    }

    if kind == AssignmentKind::Undefine {
        scope
            .undefinitions
            .entry(var_name.to_owned())
            .or_default()
            .push(input.line_no);
    }

    if input.branch_state == ConditionalTruth::True
        && matches!(
            kind,
            AssignmentKind::SimpleSet | AssignmentKind::RecursiveSet | AssignmentKind::Undefine
        )
    {
        scope
            .path_replacements
            .entry(var_name.to_owned())
            .or_default()
            .push(input.line_no);
    }

    let flavor = input.flavor.unwrap_or_else(|| match kind {
        AssignmentKind::SimpleSet => VariableFlavor::Simple,
        AssignmentKind::RecursiveSet | AssignmentKind::SetIfUnset | AssignmentKind::Undefine => {
            VariableFlavor::Recursive
        }
        AssignmentKind::Append => flavors.get(var_name).copied().unwrap_or_else(|| {
            if scope.configuration.contains_key(var_name) {
                VariableFlavor::Unknown
            } else {
                VariableFlavor::Recursive
            }
        }),
    });
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
                    .flavor_uncertainty_reason_at(&dependency, input.line_no)
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
                .push((input.line_no, flavor_uncertainty));
        }
        AssignmentKind::Append => {
            if let Some(reason) = flavor_uncertainty {
                scope
                    .flavor_uncertainties
                    .entry(var_name.to_owned())
                    .or_default()
                    .push((input.line_no, Some(reason)));
            }
        }
    }
    if flavor == VariableFlavor::Simple
        && scalar_references(value).any(|name| {
            scope.path_depends_on_conditional_at(
                name,
                input.line_no,
                MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                &mut Vec::new(),
            )
        })
    {
        scope
            .uncertain_path_values
            .entry(var_name.to_owned())
            .or_default()
            .push(input.line_no);
    }
    let projected =
        if flavor == VariableFlavor::Simple && input.branch_state == ConditionalTruth::True {
            project.map(|project| project(value, scope, input.line_no))
        } else {
            None
        };
    let path_rhs = projected.as_ref().map_or_else(
        || {
            if flavor == VariableFlavor::Simple {
                expand_immediate_locals_impl(
                    value,
                    scope,
                    MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                    Some(input.path_names),
                )
            } else {
                value.to_owned()
            }
        },
        Clone::clone,
    );
    let path_value = if kind == AssignmentKind::Append {
        let previous = input
            .append_base
            .or_else(|| {
                scope
                    .path_raw
                    .get(var_name)
                    .and_then(|history| history.last())
                    .map(|(_, value)| value.as_str())
                    .or_else(|| scope.configuration.get(var_name).map(String::as_str))
            })
            .unwrap_or("");
        if input.preserve_whitespace {
            append_words_value(previous, &path_rhs)
        } else {
            format!("{previous} {path_rhs}").trim().to_owned()
        }
    } else {
        path_rhs
    };
    scope
        .path_raw
        .entry(var_name.to_owned())
        .or_default()
        .push((input.line_no, path_value));
    let expanded_rhs = projected.unwrap_or_else(|| {
        if flavor == VariableFlavor::Simple {
            expand_immediate_locals(value, scope, MAX_DEPTH_FOR_IMMEDIATE_EXPANSION)
        } else {
            value.to_owned()
        }
    });
    let expanded = if kind == AssignmentKind::Append {
        match input.append_base.or_else(|| scope.latest_raw(var_name)) {
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
    scope.raw.entry(var_name.to_owned()).or_default().push((
        input.line_no,
        if input.preserve_whitespace {
            expanded
        } else {
            expanded.trim().to_owned()
        },
    ));
    scope
        .assignments
        .entry(var_name.to_owned())
        .or_default()
        .push((input.line_no, values));
    flavors.insert(var_name.to_owned(), flavor);
}

fn append_words_value(previous: &str, rhs: &str) -> String {
    match (previous.is_empty(), rhs.is_empty()) {
        (false, false) => format!("{previous} {rhs}"),
        (false, true) => previous.to_owned(),
        _ => rhs.to_owned(),
    }
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
mod configuration_tests;

#[cfg(test)]
mod make_comment_tests {
    use super::strip_make_comment;

    #[test]
    fn comments_respect_nested_references_one_character_refs_and_dollars() {
        assert_eq!(
            strip_make_comment("VALUE := $(subst #,X,a#b) # trailing"),
            "VALUE := $(subst #,X,a#b) "
        );
        assert_eq!(
            strip_make_comment("$(info RESULT=$(subst #,X,a#b)) # trailing"),
            "$(info RESULT=$(subst #,X,a#b)) "
        );
        assert_eq!(
            strip_make_comment("VALUE := $#tail # trailing"),
            "VALUE := $#tail "
        );
        assert_eq!(strip_make_comment("VALUE := $$# trailing"), "VALUE := $$");
        assert_eq!(
            strip_make_comment("VALUE := escaped\\#data # trailing"),
            "VALUE := escaped\\#data "
        );
    }

    #[test]
    fn ambiguous_unbalanced_or_overdeep_references_do_not_strip_suffixes() {
        assert_eq!(
            strip_make_comment("VALUE := $(open # retained"),
            "VALUE := $(open # retained"
        );
        let nested = format!("$({}# retained{})", "$((".repeat(32), ")".repeat(32));
        // The helper is conservative when nesting exceeds the evaluator bound.
        assert_eq!(strip_make_comment(&nested), nested);
    }

    #[test]
    fn delimiter_walk_keeps_gnu_closing_rules_after_single_dollar() {
        // Fixed GNU probes reject both forms as a one-argument `subst`.
        // A one-character `$)`/`$}` does not hide a closing delimiter while
        // GNU is locating the end of the surrounding function's arguments.
        for input in [
            b"$(subst $),X,aa)".as_slice(),
            b"${subst $},X,aa}".as_slice(),
        ] {
            assert_eq!(crate::make_expr::reference_end_bytes(input, 0), Some(9));
        }
    }
}
