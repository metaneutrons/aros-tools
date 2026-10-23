//! Source-order composition; retained rule data is not a build schedule.

use super::{
    apply_scalar_fragment, bound_coordinate, charge_work, evaluate, invalid, lookup,
    record_assignment, AssignmentKind, AssignmentValue, Evaluation, State, VariableFlavor,
};
use crate::make_expr::{standalone_expansion, MakeExprError};
use crate::make_rule_header::{
    has_recipe_prefix_assignment, parse, source_target_prefix, RuleHeader,
};
use crate::make_vars::{
    directive_tail, equality_operands, literal_variable_name, make_define_directive,
    strip_make_comment, undefine_directive, variable_assignment_impl, ConditionalFrame,
    ConditionalTruth,
};
use std::cell::{Cell, RefCell};

mod defaults;
mod inputs;

pub(super) fn record_input_list_entry(
    input: &str,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<(), MakeExprError> {
    inputs::enter(input, data, execution)
}
mod lines;
mod suffixes;

const MAX_INPUT: usize = 16 * 1024 * 1024;
const MAX_OUTPUT: usize = 64 * 1024 * 1024;
const MAX_EVENTS: usize = 65_536;
const MAX_DEPTH: usize = 32;

/// Return exact, independently selected include text, or proved absence.
///
/// An error is not absence, including for optional includes. The interpreter
/// performs no ambient file search; the caller owns identity/authentication.
pub type MakeIncludeResolver<'a> = dyn Fn(&str, &str) -> Result<Option<String>, String> + 'a;

/// A selected include input and the identity used to read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakeResolvedInput {
    pub identity: String,
    pub text: String,
}

/// Return exact include text together with its native read identity, or proved absence.
pub type MakeResolvedIncludeResolver<'a> =
    dyn Fn(&str, &str) -> Result<Option<MakeResolvedInput>, String> + 'a;

type ByteIncludeResolver<'a> = dyn Fn(&str, &str) -> Result<Option<aros_common::make_input_snapshot::MakeInputBytes>, String>
    + 'a;

enum IncludeResolver<'a> {
    Text(&'a MakeResolvedIncludeResolver<'a>),
    Bytes(&'a ByteIncludeResolver<'a>),
}

#[derive(Clone, Copy)]
enum SourceInput<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}

impl<'a> SourceInput<'a> {
    const fn bytes(self) -> &'a [u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

enum ResolvedSourceInput {
    Text(MakeResolvedInput),
    Bytes(aros_common::make_input_snapshot::MakeInputBytes),
}

impl ResolvedSourceInput {
    fn identity(&self) -> &str {
        match self {
            Self::Text(input) => &input.identity,
            Self::Bytes(input) => &input.identity,
        }
    }
}

/// A read-time event. Deferred recipe/recursive target-variable data is not
/// evidence that later inheritance or recipe evaluation has been implemented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MakeFileEventKind {
    Variable {
        variable_event: usize,
    },
    Export {
        variable_events: std::ops::Range<usize>,
    },
    Definition {
        variable_event: usize,
    },
    Control(String),
    Include {
        requested: String,
        present: bool,
        resolved_identity: Option<String>,
    },
    Rule(RuleHeader),
    /// A source-positioned `.DEFAULT` declaration, not an ordinary rule or
    /// default-goal candidate. Prerequisites belong to `.DEFAULT` itself;
    /// graph consumers must separately select the fallback recipe after
    /// explicit/implicit rule search. Recipe events reference this event.
    DefaultRule(RuleHeader),
    /// Target expansion yielded no names. Tail and recipes are intentionally
    /// not expanded or retained; this event cannot create a graph/default goal.
    IgnoredEmptyRule,
    Recipe {
        rule_event: usize,
        text: String,
    },
    SearchPath(String),
    /// Expanded phony names, not ordinary prerequisite edges.
    /// Percent signs remain literal, and this statement is not a default goal.
    /// Consumers must apply markers across the whole file (also to earlier
    /// targets), preserving always-out-of-date/no-file/no-implicit-search behavior before using
    /// these events to admit a build graph; this reader executes no recipes.
    PhonyTargets(Vec<String>),
    /// Source-ordered suffix declaration: empty resets, nonempty appends.
    /// Names retain declaration order and duplicates; they are not ordinary
    /// prerequisites or a default goal. Implicit-rule consumers must bind the
    /// initial suffix state and apply these controls, not ignore the events.
    Suffixes(Vec<String>),
}

/// Input/source positions are distinct from event and variable-history order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakeFileEvent {
    pub input: String,
    pub source_line: usize,
    pub generated: bool,
    pub active: bool,
    pub variable_position: usize,
    pub kind: MakeFileEventKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MakeFileProgram {
    pub events: Vec<MakeFileEvent>,
}

#[derive(Clone, Default)]
struct Origin {
    input: String,
    line: usize,
    generated: bool,
}

pub(super) struct FileContext<'a> {
    includes: IncludeResolver<'a>,
    events: RefCell<Vec<MakeFileEvent>>,
    origin: RefCell<Origin>,
    inputs: RefCell<Vec<String>>,
    depth: Cell<usize>,
    retained: Cell<usize>,
    pending: Cell<Option<usize>>,
    ignore_recipes: Cell<bool>,
    default_colon: Cell<Option<bool>>,
}

impl<'a> FileContext<'a> {
    pub(super) fn event_count(&self) -> usize {
        self.events.borrow().len()
    }

    pub(super) fn new(includes: &'a MakeResolvedIncludeResolver<'a>) -> Self {
        Self {
            includes: IncludeResolver::Text(includes),
            events: RefCell::default(),
            origin: RefCell::default(),
            inputs: RefCell::default(),
            depth: Cell::new(0),
            retained: Cell::new(0),
            pending: Cell::new(None),
            ignore_recipes: Cell::new(false),
            default_colon: Cell::new(None),
        }
    }

    pub(super) fn new_bytes(includes: &'a ByteIncludeResolver<'a>) -> Self {
        Self {
            includes: IncludeResolver::Bytes(includes),
            events: RefCell::default(),
            origin: RefCell::default(),
            inputs: RefCell::default(),
            depth: Cell::new(0),
            retained: Cell::new(0),
            pending: Cell::new(None),
            ignore_recipes: Cell::new(false),
            default_colon: Cell::new(None),
        }
    }

    pub(super) fn finish(self) -> MakeFileProgram {
        self.finish_accounted().0
    }

    /// Finish the program and return its bounded retained-data estimate.
    /// The estimate is the parser's existing accounting value, not a claim
    /// about allocator capacity or process memory.
    pub(super) fn finish_accounted(self) -> (MakeFileProgram, usize) {
        let retained = self.retained.get();
        let program = MakeFileProgram {
            events: self.events.into_inner(),
        };
        (program, retained)
    }

    pub(super) fn read_input(
        &self,
        input: &str,
        text: &str,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<(), MakeExprError> {
        self.read_selected_input(input, SourceInput::Text(text), data, execution)
    }

    pub(super) fn read_input_bytes(
        &self,
        input: &str,
        bytes: &[u8],
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<(), MakeExprError> {
        self.read_selected_input(input, SourceInput::Bytes(bytes), data, execution)
    }

    fn read_selected_input(
        &self,
        input: &str,
        text: SourceInput<'_>,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<(), MakeExprError> {
        if !valid_input_identity(input) || self.inputs.borrow().iter().any(|prior| prior == input) {
            return Err(invalid("invalid or cyclic Make include identity"));
        }
        if self.inputs.borrow().len() == MAX_DEPTH {
            return Err(invalid("Make include depth limit"));
        }
        inputs::enter(input, data, execution)?;
        self.inputs.borrow_mut().push(input.into());
        let result = self.read(
            text,
            &Origin {
                input: input.into(),
                line: 0,
                generated: false,
            },
            data,
            execution,
        );
        self.inputs.borrow_mut().pop();
        result
    }

    pub(super) fn read_eval(
        &self,
        text: &str,
        line: usize,
        active: Option<bool>,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<(), MakeExprError> {
        if active != Some(true) {
            return Err(invalid("Make eval needs proved active selection"));
        }
        let mut origin = self.origin.borrow().clone();
        origin.line = line;
        origin.generated = true;
        self.read(SourceInput::Text(text), &origin, data, execution)
    }

    fn read(
        &self,
        text: SourceInput<'_>,
        origin: &Origin,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<(), MakeExprError> {
        if text.bytes().len() > MAX_INPUT
            || text.bytes().contains(&0)
            || self.depth.get() == MAX_DEPTH
        {
            return Err(invalid("Make input byte/line/depth limit"));
        }
        charge_work(execution.work, text.bytes().len())?;
        {
            let state = data.borrow();
            let selected = state
                .scope
                .is_defined_before(".RECIPEPREFIX", state.events.len())
                || state
                    .initial_namespace
                    .as_ref()
                    .is_some_and(|ns| ns.contains(".RECIPEPREFIX"));
            if selected
                && (super::guard(&state, ".RECIPEPREFIX").is_some()
                    || lookup(&state, ".RECIPEPREFIX").as_deref() != Some(""))
            {
                return Err(invalid(
                    "Make source reader requires a proved default .RECIPEPREFIX",
                ));
            }
        }
        self.depth.set(self.depth.get() + 1);
        let previous = self.origin.replace(origin.clone());
        // Crossing an input/eval boundary flushes the enclosing rule and its
        // empty-target suppression. Neither child state nor parent state may
        // legitimize a recipe on the other side of that boundary.
        self.pending.set(None);
        self.ignore_recipes.set(false);
        let result = self
            .read_lines(text, origin, data, execution)
            .map_err(|error| {
                let at = self.origin.borrow();
                invalid(&format!("input {} line {}: {error}", at.input, at.line + 1))
            });
        self.origin.replace(previous);
        self.pending.set(None);
        self.ignore_recipes.set(false);
        self.depth.set(self.depth.get() - 1);
        result
    }

    fn read_lines(
        &self,
        text: SourceInput<'_>,
        origin: &Origin,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<(), MakeExprError> {
        let logical = match text {
            SourceInput::Text(text) => lines::read(text),
            SourceInput::Bytes(bytes) => lines::read_bytes(bytes),
        }
        .map_err(|error| {
            self.origin.borrow_mut().line = if origin.generated {
                origin.line
            } else {
                origin.line + error.source_line
            };
            invalid(error.detail)
        })?;
        if logical
            .first()
            .is_some_and(|line| line.source_range.start != 0)
            || logical
                .windows(2)
                .any(|pair| pair[0].source_range.end != pair[1].source_range.start)
            || logical.last().map_or(0, |line| line.source_range.end) != text.bytes().len()
        {
            return Err(invalid(
                "Make logical-line source ranges do not cover the input",
            ));
        }
        let lines = logical
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>();
        let mut branches = Vec::<Branch>::new();
        let mut at = 0;
        while at < lines.len() {
            let raw = lines[at];
            let line = if origin.generated {
                origin.line
            } else {
                origin.line + logical[at].source_line
            };
            self.origin.replace(Origin {
                line,
                ..origin.clone()
            });
            let active = branches
                .last()
                .is_none_or(|branch| branch.frame.current == ConditionalTruth::True);
            let visible = strip_make_comment(raw).trim_start();
            if !raw.starts_with('\t')
                && self.control(visible.trim(), active, &mut branches, data, execution)?
            {
                self.emit(
                    MakeFileEventKind::Control(visible.into()),
                    active,
                    visible.len(),
                    data,
                )?;
            } else if !raw.starts_with('\t') && make_define_directive(visible).is_some() {
                let end = self.definition(&lines, at, active, line, data)?;
                at = end + 1;
                continue;
            } else if active {
                self.statement(raw, line, data, execution)?;
            }
            at += 1;
        }
        if !branches.is_empty() {
            return Err(invalid("unterminated Make conditional"));
        }
        Ok(())
    }

    fn control(
        &self,
        visible: &str,
        active: bool,
        branches: &mut Vec<Branch>,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<bool, MakeExprError> {
        if let Some((directive, args)) = conditional(visible) {
            if branches.len() == MAX_DEPTH {
                return Err(invalid("Make conditional depth limit"));
            }
            let selected = active && self.condition(directive, args, data, execution)?;
            branches.push(Branch {
                frame: ConditionalFrame::new(truth(active), truth(selected)),
                otherwise: false,
            });
        } else if let Some(tail) = directive_tail(visible, "else") {
            let branch = branches
                .last_mut()
                .ok_or_else(|| invalid("unmatched Make else"))?;
            if branch.otherwise {
                return Err(invalid("Make else after final else"));
            }
            let eligible = branch.frame.parent == ConditionalTruth::True
                && branch.frame.matched == ConditionalTruth::False;
            if tail.is_empty() {
                branch.otherwise = true;
                branch.frame.otherwise();
            } else {
                let (directive, args) =
                    conditional(tail).ok_or_else(|| invalid("unsupported Make else directive"))?;
                branch.frame.else_if(truth(
                    eligible && self.condition(directive, args, data, execution)?,
                ));
            }
        } else if let Some(tail) = directive_tail(visible, "endif") {
            if !tail.is_empty() || branches.pop().is_none() {
                return Err(invalid("malformed or unmatched Make endif"));
            }
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    fn condition(
        &self,
        directive: &str,
        args: &str,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<bool, MakeExprError> {
        let line = self.origin.borrow().line;
        let value = if matches!(directive, "ifeq" | "ifneq") {
            let (left, right) = equality_operands(args)
                .ok_or_else(|| invalid("malformed Make equality operands"))?;
            let left = evaluate(left.trim(), line, data, execution)?;
            let right = evaluate(right.trim(), line, data, execution)?;
            left == right
        } else {
            let name = evaluate(args.trim(), line, data, execution)?;
            if !literal_variable_name(&name) {
                return Err(invalid("unsupported Make ifdef name"));
            }
            if let Some(detail) = super::guard(&data.borrow(), &name) {
                return Err(invalid(&detail));
            }
            !lookup(&data.borrow(), &name)
                .ok_or_else(|| invalid("Make ifdef needs proved variable state"))?
                .is_empty()
        };
        Ok(if matches!(directive, "ifneq" | "ifndef") {
            !value
        } else {
            value
        })
    }

    fn definition(
        &self,
        lines: &[&str],
        start: usize,
        active: bool,
        line: usize,
        data: &RefCell<State>,
    ) -> Result<usize, MakeExprError> {
        let (directive, args) =
            make_define_directive(strip_make_comment(lines[start]).trim_start())
                .expect("checked define directive");
        if directive != "define" {
            return Err(invalid("unmatched Make endef"));
        }
        let (name, flavor) = match variable_assignment_impl(args, true) {
            Some((name, "", flavor)) => (name, flavor),
            None => (args.trim(), AssignmentKind::RecursiveSet),
            _ => return Err(invalid("unsupported Make define header")),
        };
        if directive_tail(strip_make_comment(lines[start]).trim_start(), "define").is_none()
            || !literal_variable_name(name)
            || name == ".RECIPEPREFIX"
            || (data.borrow().coordinate_projection && bound_coordinate(name))
            || flavor != AssignmentKind::RecursiveSet
        {
            return Err(invalid("unsupported Make define name or flavor"));
        }
        let mut depth = 1;
        let mut body = String::new();
        for (end, raw) in lines.iter().enumerate().skip(start + 1) {
            if !raw.starts_with('\t') {
                match make_define_directive(raw) {
                    Some(("define", _)) => depth += 1,
                    Some(("endef", "")) => {
                        depth -= 1;
                        if depth == 0 {
                            if active {
                                if body.ends_with('\n') {
                                    body.pop();
                                }
                                let ordinal = data.borrow().events.len();
                                record_assignment(
                                    &format!("{name} ="),
                                    name,
                                    AssignmentValue {
                                        raw: Some(&body),
                                        flavor: Some(VariableFlavor::Recursive),
                                        frozen: None,
                                    },
                                    line,
                                    Some(true),
                                    data,
                                )?;
                                self.emit(
                                    MakeFileEventKind::Definition {
                                        variable_event: ordinal,
                                    },
                                    true,
                                    body.len() + name.len(),
                                    data,
                                )?;
                                self.pending.set(None);
                            }
                            return Ok(end);
                        }
                    }
                    _ => {}
                }
            }
            if depth > 16 || body.len().saturating_add(raw.len() + 1) > 1024 * 1024 {
                return Err(invalid("Make definition nesting/body limit"));
            }
            body.push_str(raw);
            body.push('\n');
        }
        Err(invalid("unterminated Make definition"))
    }

    fn statement(
        &self,
        raw: &str,
        line: usize,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<(), MakeExprError> {
        if let Some(recipe) = raw.strip_prefix('\t') {
            if self.ignore_recipes.get() {
                return Ok(());
            }
            let Some(rule_event) = self.pending.get() else {
                // GNU Make ignores indented blank/comment lines outside a
                // rule. Inside a rule they remain recipes: even an empty
                // recipe suppresses implicit search, so do not discard it.
                if strip_make_comment(recipe)
                    .trim_matches([' ', '\t'])
                    .is_empty()
                {
                    return Ok(());
                }
                return Err(invalid("Make recipe has no preceding rule"));
            };
            self.emit(
                MakeFileEventKind::Recipe {
                    rule_event,
                    text: recipe.into(),
                },
                true,
                recipe.len(),
                data,
            )?;
            return Ok(());
        }
        let visible = strip_make_comment(raw).trim_start();
        if visible.is_empty() {
            return Ok(());
        }
        if has_recipe_prefix_assignment(visible) {
            return Err(invalid(
                "custom .RECIPEPREFIX makes Make rule boundaries unprovable",
            ));
        }
        if super::exports::is_directive(visible) {
            let before = data.borrow().events.len();
            apply_scalar_fragment(visible, line, Some(true), data, execution)?;
            let after = data.borrow().events.len();
            self.emit(
                MakeFileEventKind::Export {
                    variable_events: before..after,
                },
                true,
                visible.len(),
                data,
            )?;
            self.pending.set(None);
        } else if variable_assignment_impl(visible, true).is_some()
            || super::computed_assignment_syntax::split(visible)
                .map_err(|error| invalid(&error))?
                .is_some()
            || undefine_directive(visible)
                .map_err(|()| invalid("unsupported Make undefine"))?
                .is_some()
        {
            apply_scalar_fragment(visible, line, Some(true), data, execution)?;
            let ordinal = data.borrow().events.len() - 1;
            self.emit(
                MakeFileEventKind::Variable {
                    variable_event: ordinal,
                },
                true,
                visible.len(),
                data,
            )?;
            self.pending.set(None);
        } else if let Some((args, optional)) =
            [("include", false), ("-include", true), ("sinclude", true)]
                .iter()
                .find_map(|(word, optional)| {
                    directive_tail(visible, word).map(|args| (args, *optional))
                })
        {
            self.include(args, optional, line, data, execution)?;
        } else if let Some(args) = directive_tail(visible, "vpath") {
            let value = evaluate(args, line, data, execution)?;
            let bytes = value.len();
            self.emit(MakeFileEventKind::SearchPath(value), true, bytes, data)?;
            self.pending.set(None);
        } else if standalone_expansion(visible)? {
            if !evaluate(visible, line, data, execution)?.trim().is_empty() {
                return Err(invalid("Make expression emitted unparsed top-level text"));
            }
        } else if let Some(prefix) = source_target_prefix(raw).map_err(|error| invalid(&error))? {
            let targets = evaluate(prefix, line, data, execution)?;
            if targets.chars().any(|c| {
                (c.is_control() && c != '\t') || (c.is_whitespace() && !matches!(c, ' ' | '\t'))
            }) {
                return Err(invalid(
                    "expanded Make targets contain unsupported separators",
                ));
            }
            self.pending.set(None);
            if targets.trim_matches([' ', '\t']).is_empty() {
                self.ignore_recipes.set(true);
                self.emit(MakeFileEventKind::IgnoredEmptyRule, true, 0, data)?;
                return Ok(());
            }
            self.ignore_recipes.set(false);
            let header = parse(raw)
                .map_err(|error| invalid(&error))?
                .ok_or_else(|| invalid("source Make target prefix has no rule header"))?;
            if let RuleHeader::TargetAssignment { assignment, .. } = &header {
                for header in super::target_variables::read_assignment(
                    &targets, assignment, line, data, execution,
                )? {
                    if !valid_frozen_header(&header) {
                        return Err(invalid(
                            "expanded target assignment contains unsupported separators",
                        ));
                    }
                    let bytes = header_bytes(&header);
                    self.emit(MakeFileEventKind::Rule(header), true, bytes, data)?;
                }
                return Ok(());
            }
            let header = Self::freeze_header(header, targets, line, data, execution)?;
            if !valid_frozen_header(&header) {
                return Err(invalid(
                    "expanded Make header is empty or contains line/control separators",
                ));
            }
            let targets = match &header {
                RuleHeader::Rule { targets, .. } | RuleHeader::TargetAssignment { targets, .. } => {
                    targets
                }
            };
            if targets == ".PHONY" {
                self.phony(header, data)?;
                return Ok(());
            }
            if targets == ".SUFFIXES" {
                self.suffixes(header, strip_make_comment(raw), data)?;
                return Ok(());
            }
            if targets.split_ascii_whitespace().eq([".DEFAULT"]) {
                self.default_rule(header, data)?;
                return Ok(());
            }
            if targets
                .split_whitespace()
                .any(crate::make_rule_header::is_control_target)
            {
                return Err(invalid("Make control-target effects are not modeled"));
            }
            let is_rule = matches!(header, RuleHeader::Rule { .. });
            let bytes = header_bytes(&header);
            let event = self.emit(MakeFileEventKind::Rule(header), true, bytes, data)?;
            self.pending.set(is_rule.then_some(event));
        } else {
            return Err(invalid("unsupported Make program statement"));
        }
        Ok(())
    }

    fn phony(&self, header: RuleHeader, data: &RefCell<State>) -> Result<(), MakeExprError> {
        let RuleHeader::Rule {
            prerequisites,
            target_pattern: None,
            order_only,
            double_colon: false,
            inline_recipe: None,
            ..
        } = header
        else {
            return Err(invalid("unsupported Make phony marker form"));
        };
        if !order_only.is_empty() {
            return Err(invalid(
                "order-only prerequisites on a Make phony marker are not modeled",
            ));
        }
        let count = prerequisites
            .split_whitespace()
            .take(MAX_EVENTS + 1)
            .count();
        if count > MAX_EVENTS {
            return Err(invalid("Make phony marker name count exceeds limit"));
        }
        if prerequisites
            .split_whitespace()
            .any(crate::make_rule_header::is_control_target)
        {
            return Err(invalid(
                "phony marking of Make control targets is not modeled",
            ));
        }
        let bytes = prerequisites
            .len()
            .checked_add(count.saturating_mul(32))
            .ok_or_else(|| invalid("Make phony marker size exceeds limit"))?;
        let names = prerequisites
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        self.emit(MakeFileEventKind::PhonyTargets(names), true, bytes, data)?;
        // A following recipe must not be attached to an earlier ordinary rule.
        // Recipes on the special marker itself remain explicitly unsupported.
        self.pending.set(None);
        Ok(())
    }

    fn include(
        &self,
        args: &str,
        optional: bool,
        line: usize,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<(), MakeExprError> {
        self.pending.set(None);
        self.ignore_recipes.set(false);
        let names = evaluate(args, line, data, execution)?;
        let from = self.origin.borrow().input.clone();
        for name in names.split_whitespace() {
            if name.contains(['*', '?', '[', '$', '\0']) {
                return Err(invalid("Make include requires an exact literal input path"));
            }
            let resolved = match &self.includes {
                IncludeResolver::Text(resolver) => {
                    resolver(name, &from).map(|input| input.map(ResolvedSourceInput::Text))
                }
                IncludeResolver::Bytes(resolver) => {
                    resolver(name, &from).map(|input| input.map(ResolvedSourceInput::Bytes))
                }
            }
            .map_err(|error| invalid(&format!("Make include resolver: {error}")))?;
            if let Some(input) = &resolved {
                if !valid_input_identity(name) || !valid_input_identity(input.identity()) {
                    return Err(invalid("invalid or cyclic Make include identity"));
                }
            }
            let resolved_identity = resolved.as_ref().map(|input| input.identity().to_owned());
            let retained_bytes = name.len() + resolved_identity.as_ref().map_or(0, String::len);
            self.emit(
                MakeFileEventKind::Include {
                    requested: name.into(),
                    present: resolved.is_some(),
                    resolved_identity,
                },
                true,
                retained_bytes,
                data,
            )?;
            match resolved {
                Some(ResolvedSourceInput::Bytes(input)) => {
                    self.read_input_bytes(&input.identity, &input.bytes, data, execution)?;
                }
                Some(ResolvedSourceInput::Text(input)) => {
                    self.read_input(&input.identity, &input.text, data, execution)?;
                }
                None if optional => {}
                None => return Err(invalid("required Make include is absent")),
            }
        }
        self.pending.set(None);
        Ok(())
    }

    fn freeze_header(
        header: RuleHeader,
        frozen_targets: String,
        line: usize,
        data: &RefCell<State>,
        execution: &Evaluation<'_>,
    ) -> Result<RuleHeader, MakeExprError> {
        match header {
            RuleHeader::Rule {
                targets: _,
                target_pattern,
                prerequisites,
                order_only,
                double_colon,
                inline_recipe,
            } => Ok(RuleHeader::Rule {
                targets: frozen_targets,
                target_pattern: target_pattern
                    .map(|value| evaluate(&value, line, data, execution))
                    .transpose()?,
                prerequisites: evaluate(&prerequisites, line, data, execution)?,
                order_only: evaluate(&order_only, line, data, execution)?,
                double_colon,
                inline_recipe,
            }),
            RuleHeader::TargetAssignment { .. } => {
                Err(invalid("target assignment bypassed its source context"))
            }
        }
    }

    fn emit(
        &self,
        kind: MakeFileEventKind,
        active: bool,
        bytes: usize,
        data: &RefCell<State>,
    ) -> Result<usize, MakeExprError> {
        let origin = self.origin.borrow();
        let next = self
            .retained
            .get()
            .checked_add(bytes.saturating_mul(8))
            .and_then(|value| value.checked_add(origin.input.len() + 1024))
            .filter(|value| *value <= MAX_OUTPUT)
            .ok_or_else(|| invalid("Make file event storage limit"))?;
        let mut events = self.events.borrow_mut();
        if events.len() == MAX_EVENTS {
            return Err(invalid("Make file event count limit"));
        }
        let index = events.len();
        events.push(MakeFileEvent {
            input: origin.input.clone(),
            source_line: origin.line,
            generated: origin.generated,
            active,
            variable_position: data.borrow().events.len(),
            kind,
        });
        self.retained.set(next);
        Ok(index)
    }
}

struct Branch {
    frame: ConditionalFrame,
    otherwise: bool,
}

const fn truth(value: bool) -> ConditionalTruth {
    if value {
        ConditionalTruth::True
    } else {
        ConditionalTruth::False
    }
}

fn conditional(text: &str) -> Option<(&str, &str)> {
    ["ifeq", "ifneq", "ifdef", "ifndef"]
        .into_iter()
        .find_map(|word| directive_tail(text, word).map(|tail| (word, tail)))
}

fn valid_input_identity(input: &str) -> bool {
    !input.is_empty()
        && input.len() <= 4096
        && !input.contains(['\n', '\r', '\0'])
        && !input.contains('\u{e000}')
}

fn header_bytes(header: &RuleHeader) -> usize {
    match header {
        RuleHeader::Rule {
            targets,
            target_pattern,
            prerequisites,
            order_only,
            inline_recipe,
            ..
        } => {
            targets.len()
                + target_pattern.as_ref().map_or(0, String::len)
                + prerequisites.len()
                + order_only.len()
                + inline_recipe.as_ref().map_or(0, String::len)
        }
        RuleHeader::TargetAssignment {
            targets,
            assignment,
        } => targets.len() + assignment.len(),
    }
}

fn valid_frozen_header(header: &RuleHeader) -> bool {
    let scalar = |value: &str| {
        !value
            .chars()
            .any(|character| character.is_control() && character != '\t')
    };
    match header {
        RuleHeader::Rule {
            targets,
            target_pattern,
            prerequisites,
            order_only,
            inline_recipe,
            ..
        } => {
            !targets.trim().is_empty()
                && scalar(targets)
                && target_pattern.as_deref().is_none_or(scalar)
                && scalar(prerequisites)
                && scalar(order_only)
                && inline_recipe.as_deref().is_none_or(scalar)
        }
        RuleHeader::TargetAssignment {
            targets,
            assignment,
        } => !targets.trim().is_empty() && scalar(targets) && scalar(assignment),
    }
}
