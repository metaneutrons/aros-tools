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
    /// Per name, the assignments in file order as (line, values).
    assignments: HashMap<String, Vec<(usize, Vec<String>)>>,
    /// Per name, the right-hand side as written, in file order.
    ///
    /// A list is not enough for a path. `EXEDIR := $(AROS_TOOLS)/QuickPart` is
    /// one word either way, but `dir=$(AROS_PRESETS)/Icons/Gorilla/Small/$(AROS_DIR_AROS)`
    /// has to keep its slashes and its references, so path resolution reads
    /// this instead of the word list.
    raw: HashMap<String, Vec<(usize, String)>>,
    /// Assignments made inside a Make conditional, by source line.
    ///
    /// The legacy list collector intentionally retains its historical
    /// last-assignment behaviour because the icon collector evaluates
    /// condition branches separately. Generic expression evaluation must be
    /// stricter: using the last textual branch would silently merge or select
    /// architecture-specific source lists without knowing the condition.
    conditional_assignments: HashMap<String, Vec<(usize, AssignmentKind)>>,
    /// Names introduced as file-local switches, including an assignment in a
    /// branch proven false and explicitly commented-out `#NAME=value` feature
    /// toggles. Once seen, absence of an active assignment has GNU Make's
    /// ordinary empty value. Names never introduced by the file remain unknown
    /// because they may come from an included configuration fragment.
    local_names: HashSet<String>,
}

impl VarScope {
    /// The variable state as Make would see it at `line`.
    ///
    /// A declaration on line N sees every assignment made before it and none of
    /// those made after.
    pub(crate) fn snapshot(&self, line: usize) -> HashMap<String, Vec<String>> {
        self.assignments
            .iter()
            .filter_map(|(name, history)| {
                history
                    .iter()
                    .rev()
                    .find(|(at, _)| *at < line)
                    .map(|(_, values)| (name.clone(), values.clone()))
            })
            .collect()
    }

    /// The right-hand side of `name` as written, as of `line`.
    #[must_use]
    pub fn raw_at(&self, name: &str, line: usize) -> Option<String> {
        self.raw
            .get(name)?
            .iter()
            .rev()
            .find(|(at, _)| *at < line)
            .map(|(_, v)| v.clone())
    }

    /// Whether `name` was assigned in a Make conditional before `line`.
    ///
    /// A caller without an evaluated condition context must reject such a
    /// value rather than taking whichever branch happened to occur last in the
    /// source file.
    #[must_use]
    pub fn conditionally_assigned_before(&self, name: &str, line: usize) -> bool {
        self.conditional_assignments
            .get(name)
            .is_some_and(|assignments| assignments.iter().any(|(at, _)| *at < line))
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

    /// The most recent raw value of `name` while the assignment scan is in
    /// progress. Appending is defined in terms of the value accumulated so
    /// far, not merely the last right-hand side.
    fn latest_raw(&self, name: &str) -> Option<&str> {
        self.raw
            .get(name)
            .and_then(|h| h.last())
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssignmentKind {
    SimpleSet,
    RecursiveSet,
    SetIfUnset,
    Append,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VariableFlavor {
    Simple,
    Recursive,
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
            .filter(|name| !matches!(*name, "CURDIR" | "OBJDIR"))
            .and_then(|name| scope.latest_raw(name));
        if let Some(value) = local_value {
            output.push_str(&expand_immediate_locals(value, scope, depth - 1));
        } else {
            output.push('$');
            output.push(open as char);
            output.push_str(&expand_immediate_locals(body, scope, depth - 1));
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
    trailing.trim().is_empty().then_some((left, right))
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
) -> Option<String> {
    let split = body.find(char::is_whitespace)?;
    let name = body[..split].trim();
    let args = body[split..].trim();
    match name {
        "strip" => Some(
            expand_condition_operand(args, scope, context, depth - 1)?
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        ),
        "findstring" => {
            let (needle, haystack) = split_top_level_comma(args)?;
            let needle = expand_condition_operand(needle, scope, context, depth - 1)?;
            let haystack = expand_condition_operand(haystack, scope, context, depth - 1)?;
            Some(if haystack.contains(&needle) {
                needle
            } else {
                String::new()
            })
        }
        "filter" | "filter-out" => {
            let (patterns, words) = split_top_level_comma(args)?;
            let patterns = expand_condition_operand(patterns, scope, context, depth - 1)?;
            let words = expand_condition_operand(words, scope, context, depth - 1)?;
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
) -> Option<String> {
    let body = body.trim();
    if !body.is_empty()
        && body.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        if scope
            .conditional_assignments
            .get(body)
            .is_some_and(|assignments| !assignments.is_empty())
        {
            return None;
        }
        if let Some(value) = scope.latest_raw(body) {
            return expand_condition_operand(value, scope, context, depth - 1);
        }
        if let Some(value) = context.value(body) {
            return Some(value);
        }
        return scope.local_names.contains(body).then(String::new);
    }
    expand_condition_function(body, scope, context, depth)
}

fn expand_condition_operand(
    raw: &str,
    scope: &VarScope,
    context: &TargetContext,
    depth: usize,
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
        )?);
        cursor = end + 1;
    }
    Some(unquote_condition_value(output.trim()).to_owned())
}

pub(crate) fn evaluate_conditional(
    directive: &str,
    args: &str,
    scope: &VarScope,
    context: &TargetContext,
) -> ConditionalTruth {
    let value = match directive {
        "ifeq" | "ifneq" => equality_operands(args).and_then(|(left, right)| {
            Some(
                expand_condition_operand(left, scope, context, MAX_DEPTH_FOR_IMMEDIATE_EXPANSION)?
                    == expand_condition_operand(
                        right,
                        scope,
                        context,
                        MAX_DEPTH_FOR_IMMEDIATE_EXPANSION,
                    )?,
            )
        }),
        "ifdef" | "ifndef" => {
            let name = args.trim();
            let value = scope
                .latest_raw(name)
                .map(str::to_owned)
                .or_else(|| context.value(name));
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

pub(crate) fn collect_vars_impl_with_forward_locals(
    joined: &str,
    context: Option<&TargetContext>,
    forward_locals: bool,
) -> (VarScope, Vec<ConditionalTruth>) {
    let mut scope = VarScope {
        assignments: HashMap::new(),
        raw: HashMap::new(),
        conditional_assignments: HashMap::new(),
        local_names: HashSet::new(),
    };
    if context.is_some() && forward_locals {
        for raw_line in joined.lines() {
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
    let mut line_states = Vec::with_capacity(joined.lines().count());

    for (line_no, raw_line) in joined.lines().enumerate() {
        let branch_state = context.map_or_else(
            || {
                if conditional_depth > 0 {
                    ConditionalTruth::Unknown
                } else {
                    ConditionalTruth::True
                }
            },
            |_| {
                conditional_stack
                    .last()
                    .map_or(ConditionalTruth::True, |frame| frame.current)
            },
        );
        line_states.push(branch_state);

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

        if let Some((directive, args)) = ["ifeq", "ifneq", "ifdef", "ifndef"]
            .into_iter()
            .find_map(|word| directive_tail(trimmed, word).map(|tail| (word, tail)))
        {
            if let Some(context) = context {
                let parent = conditional_stack
                    .last()
                    .map_or(ConditionalTruth::True, |frame| frame.current);
                let condition = evaluate_conditional(directive, args, &scope, context);
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
                        let condition = evaluate_conditional(directive, args, &scope, context);
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
        let Some((var_name, value, kind)) = variable_assignment(line) else {
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

        if kind == AssignmentKind::SetIfUnset && scope.assignments.contains_key(var_name) {
            continue;
        }

        let flavor = match kind {
            AssignmentKind::SimpleSet => VariableFlavor::Simple,
            AssignmentKind::RecursiveSet | AssignmentKind::SetIfUnset => VariableFlavor::Recursive,
            AssignmentKind::Append => flavors
                .get(var_name)
                .copied()
                .unwrap_or(VariableFlavor::Recursive),
        };
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
