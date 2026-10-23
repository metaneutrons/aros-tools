//! Bounded evaluation of the GNU Make list expressions used by the AROS tree.
//!
//! The source inventory currently contains 175 `addprefix`, 71 `addsuffix`,
//! 116 `filter`, 15 `filter-out`, 119 `patsubst`, 24 `subst`, 28 `notdir`, 73 `dir`, 11
//! `basename`, seven `sort`, 20 `strip`, 41 `wildcard`, 143
//! `call WILDCARD`, and 44 substitution-reference occurrences. The evaluator
//! implements the complete side-effect-free GNU Make word/list vocabulary used
//! by those expressions. Functions which execute commands or mutate Make's
//! parser (`shell`, `eval`, `file`, `guile`) remain explicit errors by default.
//! Explicit bounded modes interpret only rule-data eval or pure echo/sed data,
//! never host commands or hidden environment inputs. Unsupported functions and unresolved
//! variables never turn into an empty list by accident.
//!
//! Evaluation is positional through [`VarScope::raw_at`]. Directory variables
//! fall back to [`DirVars::expand_with`]. `SRCDIR` and `CURDIR` can be
//! materialised temporarily so a source-tree `wildcard` is evaluated during
//! transpilation; `TOP` remains the CMake build root. Other deferred CMake
//! paths remain strings, but are rejected if used as filesystem glob patterns.

use crate::dirs::DirVars;
use crate::make_vars::VarScope;
use glob::{glob_with, MatchOptions, Pattern};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

const MAX_EXPANSION_DEPTH: usize = 32;
const MAX_VALUE_BYTES: usize = 4 * 1024 * 1024;
const MAX_EMITTED_BYTES: usize = 16 * 1024 * 1024;
const MAX_SCANNED_BYTES: usize = 64 * 1024 * 1024;
const MAX_WORK_UNITS: usize = 1_048_576;
const MAX_LIST_ITEMS: usize = 65_536;
const MAX_ERROR_PREVIEW_BYTES: usize = 128;
const ESCAPED_DOLLAR: char = '\u{e000}';

/// Collector-specific raw variable lookup used to extend the shared scopes.
pub type MakeVariableLookup<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// A source-state query distinct from recursive expression lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MakeVariableQuery {
    Origin,
    Flavor,
    RawValue,
}

/// Return a known literal answer; None means unproved, not undefined.
pub type MakeVariableInspection<'a> = dyn Fn(MakeVariableQuery, &str) -> Option<String> + 'a;

/// Returns a diagnostic when a collector knows a variable is unsafe to use.
pub type MakeVariableGuard<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// One already-expanded wildcard pattern over independently captured inputs.
/// The provider returns GNU-order path spellings, never ambient host results.
pub type MakeCapturedWildcard<'a> = dyn Fn(&str) -> Result<Vec<String>, String> + 'a;

/// Resolve one literal path within independently captured inputs, or prove absence.
pub type MakeCapturedRealpath<'a> = dyn Fn(&str) -> Result<Option<String>, String> + 'a;

/// Interpret already-expanded shell data against independently bound inputs.
///
/// The provider owns the source-state status effect and returns GNU-folded
/// stdout. This is not permission to launch a shell or access ambient data.
pub type MakeCapturedShell<'a> = dyn Fn(&str) -> Result<String, String> + 'a;

/// Synchronous second-stage interpreter for first-expanded Make eval text.
pub type MakeProgramEffect<'a> = dyn Fn(&str) -> Result<(), MakeExprError> + 'a;

/// Shared expression-work and recursion limits for one Make program evaluation.
///
/// Pass this same budget to every recursive second-stage evaluation and later
/// expression in that program. Statement parsing/storage need their own bounds.
#[derive(Default)]
pub struct MakeProgramBudget {
    evaluation: RefCell<EvaluationBudget>,
    effects: Cell<usize>,
    depth: Cell<usize>,
}

#[derive(Clone, Copy)]
enum ProgramCharge {
    Scanned,
    Emitted,
    Work,
    Items,
}

impl MakeProgramBudget {
    fn enter(&self) -> Result<ProgramDepth<'_>, MakeExprError> {
        if self.depth.get() == MAX_EXPANSION_DEPTH {
            return Err(Evaluator::resource_limit(
                "synchronous eval depth",
                MAX_EXPANSION_DEPTH,
            ));
        }
        self.depth.set(self.depth.get() + 1);
        Ok(ProgramDepth(self))
    }

    fn charge_effect(&self) -> Result<(), MakeExprError> {
        if self.effects.get() == 16_384 {
            return Err(Evaluator::resource_limit(
                "synchronous eval effects",
                16_384,
            ));
        }
        self.effects.set(self.effects.get() + 1);
        Ok(())
    }

    fn charge(&self, amount: usize, kind: ProgramCharge) -> Result<(), MakeExprError> {
        let mut budget = self.evaluation.borrow_mut();
        match kind {
            ProgramCharge::Scanned => Evaluator::charge(
                &mut budget.scanned_bytes,
                amount,
                MAX_SCANNED_BYTES,
                "scanned bytes",
            ),
            ProgramCharge::Emitted => Evaluator::charge(
                &mut budget.emitted_bytes,
                amount,
                MAX_EMITTED_BYTES,
                "aggregate emitted bytes",
            ),
            ProgramCharge::Work => {
                Evaluator::charge(&mut budget.work_units, amount, MAX_WORK_UNITS, "work units")
            }
            ProgramCharge::Items => {
                Evaluator::charge(&mut budget.list_items, amount, MAX_LIST_ITEMS, "list items")
            }
        }
    }
}

struct ProgramDepth<'a>(&'a MakeProgramBudget);
impl Drop for ProgramDepth<'_> {
    fn drop(&mut self) {
        self.0.depth.set(self.0.depth.get() - 1);
    }
}

/// Inputs needed to evaluate one expression at its declaration site.
#[derive(Clone, Copy)]
pub struct MakeExprContext<'a> {
    scope: &'a VarScope,
    dirs: &'a DirVars,
    line: usize,
    source_dir: &'a Path,
    relative_dir: &'a Path,
    lookup: Option<&'a MakeVariableLookup<'a>>,
    inspection: Option<&'a MakeVariableInspection<'a>>,
    guard: Option<&'a MakeVariableGuard<'a>>,
    filesystem_enabled: bool,
    captured_wildcard: Option<&'a MakeCapturedWildcard<'a>>,
    captured_realpath: Option<&'a MakeCapturedRealpath<'a>>,
    captured_shell: Option<&'a MakeCapturedShell<'a>>,
    definitions: Option<&'a crate::make_definitions::MakeDefinitions>,
    working_directory: Option<&'a Path>,
    build_directory: Option<&'a Path>,
    automatic_variables: Option<&'a BTreeMap<String, String>>,
    pure_text_pipeline: bool,
    source_projection: bool,
}

impl<'a> MakeExprContext<'a> {
    /// Creates a context for an mmakefile below `source_dir`.
    ///
    /// `relative_dir` is the directory containing the mmakefile, relative to
    /// `source_dir`. `line` uses the same zero-based, continuation-joined line
    /// coordinates as [`VarScope::raw_at`].
    #[must_use]
    pub const fn new(
        scope: &'a VarScope,
        dirs: &'a DirVars,
        line: usize,
        source_dir: &'a Path,
        relative_dir: &'a Path,
    ) -> Self {
        Self {
            scope,
            dirs,
            line,
            source_dir,
            relative_dir,
            lookup: None,
            inspection: None,
            guard: None,
            filesystem_enabled: true,
            captured_wildcard: None,
            captured_realpath: None,
            captured_shell: None,
            definitions: None,
            working_directory: None,
            build_directory: None,
            automatic_variables: None,
            pure_text_pipeline: false,
            source_projection: true,
        }
    }

    /// Adds a raw-value lookup checked before `VarScope` and `DirVars`.
    ///
    /// This keeps the evaluator usable with collector-specific maps without
    /// coupling it to their value representation. Returned values may contain
    /// further Make expressions; they are recursively evaluated and can fall
    /// back to the positional local scope and then the global directory table.
    #[must_use]
    pub const fn with_lookup(mut self, lookup: &'a MakeVariableLookup<'a>) -> Self {
        self.lookup = Some(lookup);
        self
    }

    /// Supply exact source-state origin/flavor/raw-value answers, without
    /// inferring them from expression strings or a directory table.
    #[must_use]
    pub const fn with_variable_inspection(
        mut self,
        inspection: &'a MakeVariableInspection<'a>,
    ) -> Self {
        self.inspection = Some(inspection);
        self
    }

    /// Add independently collected recursive definitions at this exact source
    /// position. Their bodies remain data; this does not enable `eval` effects.
    #[must_use]
    pub const fn with_definitions(
        mut self,
        definitions: &'a crate::make_definitions::MakeDefinitions,
    ) -> Self {
        self.definitions = Some(definitions);
        self
    }

    /// Bind GNU Make's actual working directory for pure relative `abspath`.
    /// No ambient process directory is inferred or read from the host.
    #[must_use]
    pub const fn with_working_directory(mut self, directory: &'a Path) -> Self {
        self.working_directory = Some(directory);
        self
    }

    /// Bind the independent build root for pure path-function evaluation.
    /// Ordinary TOP expansion remains relocatable. No root is inferred from
    /// the current directory or from an unverified variable value.
    #[must_use]
    pub const fn with_build_directory(mut self, directory: &'a Path) -> Self {
        self.build_directory = Some(directory);
        self
    }

    /// Interpret source variables/functions rather than engine projection aliases.
    /// This is for complete source-program replay from explicit initial state.
    /// It does not authenticate that state or authorize a filesystem import.
    #[must_use]
    pub const fn without_source_projection(mut self) -> Self {
        self.source_projection = false;
        self
    }

    /// Bind known literal automatic values for one concrete recipe instance.
    /// Unprovided automatic values refuse; suffix/time/archive facts are not
    /// inferred from a filename or from the host filesystem.
    #[must_use]
    pub const fn with_automatic_variables(mut self, values: &'a BTreeMap<String, String>) -> Self {
        self.automatic_variables = Some(values);
        self
    }

    /// Interpret the closed, pure echo/sed/tr text-data subset inside `shell`.
    /// No process, environment lookup or filesystem operation is enabled.
    /// This is a hermetic ASCII byte dialect, not proof of native utility or
    /// locale equivalence. Captured native effects require separately bound
    /// shell/search/locale policy. Source projection retains its sed-only subset.
    /// Other commands, expansions and utility features continue to refuse.
    /// This leaves existing Make wildcard policy unchanged; pair it with
    /// `without_filesystem` when nested expressions must not inspect files.
    #[must_use]
    pub const fn with_pure_text_pipeline(mut self) -> Self {
        self.pure_text_pipeline = true;
        self
    }

    /// Bind a data-only source shell interpreter and its explicit input scope.
    #[must_use]
    pub const fn with_captured_shell(mut self, shell: &'a MakeCapturedShell<'a>) -> Self {
        self.captured_shell = Some(shell);
        self
    }

    /// Adds a guard for conditional or otherwise ambiguous local variables.
    ///
    /// The callback returns a human-readable reason for rejection. It is
    /// checked before collector values, `VarScope`, and nested local overrides
    /// used while resolving `DirVars`.
    #[must_use]
    pub const fn with_guard(mut self, guard: &'a MakeVariableGuard<'a>) -> Self {
        self.guard = Some(guard);
        self
    }

    /// Disables filesystem enumeration by Make's `wildcard` functions.
    ///
    /// This mode is suitable for evaluating expressions against sealed scopes
    /// whose values are already available in memory. Reached `wildcard` and
    /// `call WILDCARD` functions return [`MakeExprError::FilesystemAccessDisabled`]
    /// unless ordinary `wildcard` has an explicit captured-data provider.
    /// Lazy branches which are not selected are not evaluated.
    #[must_use]
    pub const fn without_filesystem(mut self) -> Self {
        self.filesystem_enabled = false;
        self
    }

    /// Supply data-only wildcard queries from an independently captured tree.
    /// This does not enable ambient filesystem enumeration or shell commands.
    #[must_use]
    pub const fn with_captured_wildcard(mut self, lookup: &'a MakeCapturedWildcard<'a>) -> Self {
        self.captured_wildcard = Some(lookup);
        self
    }

    /// Supply data-only canonical path queries without ambient canonicalization.
    #[must_use]
    pub const fn with_captured_realpath(mut self, lookup: &'a MakeCapturedRealpath<'a>) -> Self {
        self.captured_realpath = Some(lookup);
        self
    }

    /// Returns a positional local value only when it is safe to expand before
    /// expression evaluation. Conditional values must reach the evaluator so
    /// its `UnsafeVariable` error remains fatal for the complete source lane.
    pub(crate) fn safe_local_raw(&self, name: &str) -> Option<String> {
        if self.scope.conditionally_assigned_before(name, self.line)
            || self.guard.is_some_and(|guard| guard(name).is_some())
        {
            None
        } else {
            self.scope.raw_at(name, self.line)
        }
    }
}

/// Why a bounded Make expression could not be evaluated faithfully.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MakeExprError {
    /// Parentheses, arguments, or the context itself are malformed.
    InvalidSyntax { expression: String, detail: String },
    /// A variable was not present in either the local or directory scope.
    UnresolvedVariables {
        names: Vec<String>,
        expansion_chain: Vec<String>,
    },
    /// Recursive variable definitions formed a cycle.
    VariableCycle { expansion_chain: Vec<String> },
    /// A collector identified a conditional or ambiguous variable definition.
    UnsafeVariable {
        name: String,
        detail: String,
        expansion_chain: Vec<String>,
    },
    /// The bounded recursion limit was reached.
    ExpansionLimit { expression: String },
    /// A per-evaluation resource budget was exhausted.
    ResourceLimit {
        resource: &'static str,
        limit: usize,
    },
    /// The expression asks for a GNU Make function outside the safe subset.
    UnsupportedFunction { name: String },
    /// A single-character or automatic Make reference cannot be decided here.
    UnsupportedReference { reference: String },
    /// A glob whose build-tree inventory is not materialized yet. Source-list
    /// collectors preserve the pattern for an owning-fetch preparation pass,
    /// but must not invent compilation units from it. The full evaluator runs
    /// again on the materialized sources before graph qualification.
    DeferredWildcard { pattern: String },
    /// A glob pattern or one of its filesystem results was invalid.
    Wildcard { pattern: String, detail: String },
    /// Filesystem enumeration was requested in a context which disabled it.
    FilesystemAccessDisabled { function: String },
}

impl fmt::Display for MakeExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSyntax { expression, detail } => {
                write!(f, "invalid Make expression `{expression}`: {detail}")
            }
            Self::UnresolvedVariables {
                names,
                expansion_chain,
            } => {
                write!(f, "unresolved Make variable(s): {}", names.join(", "))?;
                if !expansion_chain.is_empty() {
                    write!(f, " while expanding {}", expansion_chain.join(" -> "))?;
                }
                Ok(())
            }
            Self::VariableCycle { expansion_chain } => {
                write!(f, "Make variable cycle: {}", expansion_chain.join(" -> "))
            }
            Self::UnsafeVariable {
                name,
                detail,
                expansion_chain,
            } => {
                write!(f, "unsafe Make variable `{name}`: {detail}")?;
                if !expansion_chain.is_empty() {
                    write!(f, " while expanding {}", expansion_chain.join(" -> "))?;
                }
                Ok(())
            }
            Self::ExpansionLimit { expression } => {
                write!(
                    f,
                    "Make expression exceeded the expansion limit: `{expression}`"
                )
            }
            Self::ResourceLimit { resource, limit } => {
                write!(f, "Make expression exceeded the {resource} limit ({limit})")
            }
            Self::UnsupportedFunction { name } => {
                write!(
                    f,
                    "unsupported Make function `{name}`; upstream now requires syntax that aros-transpiler does not model, so the transpiler must be updated"
                )
            }
            Self::UnsupportedReference { reference } => {
                write!(
                    f,
                    "unsupported Make reference `{reference}`; upstream now requires syntax that aros-transpiler does not model, so the transpiler must be updated"
                )
            }
            Self::DeferredWildcard { pattern } => {
                write!(
                    f,
                    "wildcard requires a materialized source inventory: `{pattern}`"
                )
            }
            Self::Wildcard { pattern, detail } => {
                write!(f, "cannot evaluate wildcard `{pattern}`: {detail}")
            }
            Self::FilesystemAccessDisabled { function } => write!(
                f,
                "filesystem access is disabled for Make function `{function}`"
            ),
        }
    }
}

impl std::error::Error for MakeExprError {}

/// Evaluates an expression to GNU Make's whitespace-separated string form.
///
/// This supports nested `$(...)` references, suffix and `%` substitution
/// references, and the functions documented by this module. Quotes retain
/// their ordinary Make meaning: they are characters, not shell quoting.
///
/// # Errors
///
/// Returns an error for unsupported syntax, unresolved values, unsafe
/// wildcards, recursion limits, or invalid function arguments.
pub fn evaluate_make_expr(
    raw: &str,
    context: &MakeExprContext<'_>,
) -> Result<String, MakeExprError> {
    let mut evaluator = Evaluator::new(context)?;
    let value = evaluator.expand_text(raw, MAX_EXPANSION_DEPTH)?;
    evaluator.finish_value(value)
}

/// Data-only results of generic rule-generating `eval` expressions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakeRuleExpansion {
    /// Ordinary expression result (`eval` itself returns an empty value).
    pub value: String,
    /// First-expanded rule fragments, in GNU Make expansion order. These must
    /// still be parsed as Make rules; they are not executable shell commands.
    pub rules: Vec<String>,
}

/// Expand generic `foreach`/`call` expressions which generate rules via `eval`.
///
/// This opt-in path collects rule data only. Global assignments, definitions,
/// include directives or other top-level programs in eval output refuse:
/// deferring those side effects until after a foreach loop would change Make
/// semantics. The ordinary expression API still rejects `eval` entirely.
/// No shell or filesystem mutation is performed.
///
/// # Errors
/// Rejects unsupported effects, unknown expressions and expansion limits.
pub fn evaluate_make_rule_expansion(
    raw: &str,
    context: &MakeExprContext<'_>,
) -> Result<MakeRuleExpansion, MakeExprError> {
    let mut evaluator = Evaluator::new(context)?;
    evaluator.rule_outputs = Some(Vec::new());
    let expanded = evaluator.expand_text(raw, MAX_EXPANSION_DEPTH)?;
    let value = evaluator.finish_value(expanded)?;
    Ok(MakeRuleExpansion {
        value,
        rules: evaluator.rule_outputs.take().unwrap_or_default(),
    })
}

/// Expand an expression with synchronous, caller-interpreted `eval` effects.
///
/// Each first-expanded eval argument is supplied immediately to `apply_eval`.
/// The callback must parse and apply it before returning; later lookups through
/// the context can then observe those effects, including inside one foreach or
/// nested expression. The callback owns second-stage Make interpretation,
/// transactionality and statement limits. Reuse `budget` in recursive callbacks.
/// This API does not run a shell or interpret generated statements itself.
/// Unknown variables still refuse, including one-character references.
///
/// # Errors
/// Returns expression/resource failures and callback refusals. Previously
/// applied callback effects are not rolled back by this expression layer.
pub fn evaluate_make_program_expression<'a>(
    raw: &str,
    context: &MakeExprContext<'a>,
    budget: &'a MakeProgramBudget,
    apply_eval: &'a MakeProgramEffect<'a>,
) -> Result<String, MakeExprError> {
    let _depth = budget.enter()?;
    let mut evaluator = Evaluator::new(context)?;
    evaluator.program_effect = Some(apply_eval);
    evaluator.program_budget = Some(budget);
    let expanded = evaluator.expand_text(raw, MAX_EXPANSION_DEPTH)?;
    evaluator.finish_value(expanded)
}

pub(crate) fn validate_rule_eval_output(text: &str) -> Result<(), MakeExprError> {
    let invalid = || MakeExprError::InvalidSyntax {
        expression: "eval rule output".into(),
        detail:
            "eval supports rule data only; global effects require synchronous program evaluation"
                .into(),
    };
    let mut has_rule = false;
    for raw in text.lines() {
        if raw.starts_with('\t') {
            if !has_rule {
                return Err(invalid());
            }
            continue;
        }
        let line = crate::make_vars::strip_make_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if [
            "include", "-include", "sinclude", "define", "endef", "undefine", "export", "unexport",
            "override", "private", "vpath", "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif",
            "load", "-load",
        ]
        .iter()
        .any(|directive| crate::make_vars::directive_tail(line, directive).is_some())
        {
            return Err(invalid());
        }
        if crate::make_vars::variable_assignment(line).is_some() {
            return Err(invalid());
        }
        let Some((target, rest)) = line.split_once(':') else {
            return Err(invalid());
        };
        if target.trim().is_empty()
            // Make expands prerequisites and immediate target assignments
            // again while parsing eval output. Only recipe lines may retain
            // deferred references in this rule-data-only mode.
            || line.contains('$')
            || target.contains(['=', ';', '\\'])
            // Special dot/uppercase control targets can alter Make-wide
            // behavior. Their semantics are outside rule-data-only eval.
            || target.split_whitespace().any(crate::make_rule_header::is_control_target)
            || rest.starts_with('=')
            || rest.starts_with(":=")
        {
            return Err(invalid());
        }
        has_rule = true;
    }
    Ok(())
}

/// Evaluates an expression and splits its result into Make list words.
///
/// # Errors
///
/// Returns the same evaluation errors as [`evaluate_make_expr`].
pub fn evaluate_make_list(
    raw: &str,
    context: &MakeExprContext<'_>,
) -> Result<Vec<String>, MakeExprError> {
    let mut evaluator = Evaluator::new(context)?;
    let expanded = evaluator.expand_text(raw, MAX_EXPANSION_DEPTH)?;
    let value = evaluator.finish_value(expanded)?;
    evaluator.make_words(&value)
}

struct Evaluator<'a> {
    scope: &'a VarScope,
    dirs: &'a DirVars,
    line: usize,
    source_text: String,
    relative_text: String,
    wildcard_root: PathBuf,
    lookup: Option<&'a MakeVariableLookup<'a>>,
    inspection: Option<&'a MakeVariableInspection<'a>>,
    guard: Option<&'a MakeVariableGuard<'a>>,
    filesystem_enabled: bool,
    captured_wildcard: Option<&'a MakeCapturedWildcard<'a>>,
    captured_realpath: Option<&'a MakeCapturedRealpath<'a>>,
    captured_shell: Option<&'a MakeCapturedShell<'a>>,
    definitions: Option<&'a crate::make_definitions::MakeDefinitions>,
    working_directory: Option<&'a Path>,
    build_directory: Option<&'a Path>,
    automatic_variables: Option<&'a BTreeMap<String, String>>,
    pure_text_pipeline: bool,
    source_projection: bool,
    rule_outputs: Option<Vec<String>>,
    program_effect: Option<&'a MakeProgramEffect<'a>>,
    program_budget: Option<&'a MakeProgramBudget>,
    expansion_chain: Vec<String>,
    /// Innermost-last bindings of `$(foreach var,...)` loop variables. Make
    /// gives the loop variable a temporary value that shadows any global of
    /// the same name, so this is consulted before every other source.
    loop_vars: Vec<(String, String)>,
    /// Largest active call frame, including parameter zero. Nested shorter
    /// calls shadow the caller's remaining positional parameters with empties.
    call_arity: usize,
    budget: EvaluationBudget,
}

#[derive(Default)]
struct EvaluationBudget {
    scanned_bytes: usize,
    emitted_bytes: usize,
    work_units: usize,
    list_items: usize,
}

impl<'a> Evaluator<'a> {
    fn new(context: &MakeExprContext<'a>) -> Result<Self, MakeExprError> {
        if let Some(values) = context.automatic_variables {
            if values.len() > 32 || values.keys().any(|name| !automatic_variable_name(name)) {
                return Err(MakeExprError::InvalidSyntax {
                    expression: "automatic variable bindings".into(),
                    detail: "automatic context contains an unsupported variable name or count"
                        .into(),
                });
            }
            if values.values().any(|value| value.len() > MAX_VALUE_BYTES)
                || values.values().map(String::len).sum::<usize>() > MAX_EMITTED_BYTES
            {
                return Err(Self::resource_limit(
                    "automatic variable bytes",
                    MAX_EMITTED_BYTES,
                ));
            }
        }
        if context
            .working_directory
            .is_some_and(|directory| !directory.is_absolute())
        {
            return Err(MakeExprError::InvalidSyntax {
                expression: "abspath working directory".into(),
                detail: "Make working directory must be explicitly absolute".into(),
            });
        }
        if context
            .build_directory
            .is_some_and(|directory| !directory.is_absolute())
        {
            return Err(MakeExprError::InvalidSyntax {
                expression: "path-function build directory".into(),
                detail: "Make build directory must be explicitly absolute".into(),
            });
        }
        if context
            .definitions
            .is_some_and(|definitions| definitions.line != context.line)
        {
            return Err(MakeExprError::InvalidSyntax {
                expression: "define snapshot".into(),
                detail: "Make definitions belong to a different source position".into(),
            });
        }
        if context.relative_dir.is_absolute() {
            return Err(MakeExprError::InvalidSyntax {
                expression: context.relative_dir.display().to_string(),
                detail: "the mmakefile directory must be relative to the source tree".to_owned(),
            });
        }
        let source_text = path_text(context.source_dir, "source directory")?;
        let relative_text = if context.relative_dir.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            path_text(context.relative_dir, "relative mmakefile directory")?
        };
        Ok(Self {
            scope: context.scope,
            dirs: context.dirs,
            line: context.line,
            source_text,
            relative_text,
            wildcard_root: context.source_dir.join(context.relative_dir),
            lookup: context.lookup,
            inspection: context.inspection,
            guard: context.guard,
            filesystem_enabled: context.filesystem_enabled,
            captured_wildcard: context.captured_wildcard,
            captured_realpath: context.captured_realpath,
            captured_shell: context.captured_shell,
            definitions: context.definitions,
            working_directory: context.working_directory,
            build_directory: context.build_directory,
            automatic_variables: context.automatic_variables,
            pure_text_pipeline: context.pure_text_pipeline,
            source_projection: context.source_projection,
            rule_outputs: None,
            program_effect: None,
            program_budget: None,
            expansion_chain: Vec::new(),
            loop_vars: Vec::new(),
            call_arity: 0,
            budget: EvaluationBudget::default(),
        })
    }

    const fn resource_limit(resource: &'static str, limit: usize) -> MakeExprError {
        MakeExprError::ResourceLimit { resource, limit }
    }

    fn finish_value(&mut self, value: String) -> Result<String, MakeExprError> {
        self.charge_scanned(value.len())?;
        reject_unsupported_references(&value)?;
        if !value.contains(ESCAPED_DOLLAR) {
            return Ok(value);
        }
        let escaped_count = value.matches(ESCAPED_DOLLAR).count();
        let output_len = value
            .len()
            .checked_sub(escaped_count.saturating_mul(ESCAPED_DOLLAR.len_utf8()))
            .and_then(|length| length.checked_add(escaped_count))
            .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
        Self::check_value_size(output_len)?;
        self.charge_emitted(output_len)?;
        self.charge_work(escaped_count)?;
        Ok(value.replace(ESCAPED_DOLLAR, "$"))
    }

    fn literal_data_value(&mut self, value: String) -> Result<String, MakeExprError> {
        // A shell/filesystem result is data, not a second Make expansion.
        // Protect dollars until the deliberate finish/eval boundary, just as
        // for literal variable inspection and captured filesystem results.
        if value.contains(ESCAPED_DOLLAR) {
            return Err(MakeExprError::InvalidSyntax {
                expression: "literal data".into(),
                detail: "literal data contains a reserved internal delimiter".into(),
            });
        }
        let encoded_len = value
            .len()
            .checked_add(
                value
                    .bytes()
                    .filter(|byte| *byte == b'$')
                    .count()
                    .saturating_mul(ESCAPED_DOLLAR.len_utf8() - 1),
            )
            .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
        Self::check_value_size(encoded_len)?;
        self.charge_scanned(value.len())?;
        self.charge_work(value.len())?;
        self.charge_emitted(encoded_len)?;
        if !value.contains('$') {
            return Ok(value);
        }
        Ok(value.replace('$', &ESCAPED_DOLLAR.to_string()))
    }

    const fn charge(
        current: &mut usize,
        amount: usize,
        limit: usize,
        resource: &'static str,
    ) -> Result<(), MakeExprError> {
        let Some(next) = current.checked_add(amount) else {
            return Err(MakeExprError::ResourceLimit { resource, limit });
        };
        if next > limit {
            return Err(MakeExprError::ResourceLimit { resource, limit });
        }
        *current = next;
        Ok(())
    }

    fn charge_scanned(&mut self, amount: usize) -> Result<(), MakeExprError> {
        if let Some(budget) = self.program_budget {
            budget.charge(amount, ProgramCharge::Scanned)?;
        }
        Self::charge(
            &mut self.budget.scanned_bytes,
            amount,
            MAX_SCANNED_BYTES,
            "scanned bytes",
        )
    }

    fn charge_emitted(&mut self, amount: usize) -> Result<(), MakeExprError> {
        if let Some(budget) = self.program_budget {
            budget.charge(amount, ProgramCharge::Emitted)?;
        }
        Self::charge(
            &mut self.budget.emitted_bytes,
            amount,
            MAX_EMITTED_BYTES,
            "aggregate emitted bytes",
        )
    }

    fn charge_work(&mut self, amount: usize) -> Result<(), MakeExprError> {
        if let Some(budget) = self.program_budget {
            budget.charge(amount, ProgramCharge::Work)?;
        }
        Self::charge(
            &mut self.budget.work_units,
            amount,
            MAX_WORK_UNITS,
            "work units",
        )
    }

    fn charge_items(&mut self, amount: usize) -> Result<(), MakeExprError> {
        if let Some(budget) = self.program_budget {
            budget.charge(amount, ProgramCharge::Items)?;
        }
        Self::charge(
            &mut self.budget.list_items,
            amount,
            MAX_LIST_ITEMS,
            "list items",
        )
    }

    const fn check_value_size(size: usize) -> Result<(), MakeExprError> {
        if size > MAX_VALUE_BYTES {
            Err(Self::resource_limit("value bytes", MAX_VALUE_BYTES))
        } else {
            Ok(())
        }
    }

    fn append_output(&mut self, output: &mut String, piece: &str) -> Result<(), MakeExprError> {
        let next_len = output
            .len()
            .checked_add(piece.len())
            .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
        Self::check_value_size(next_len)?;
        self.charge_work(1)?;
        self.charge_emitted(piece.len())?;
        output.push_str(piece);
        Ok(())
    }

    fn append_char(&mut self, output: &mut String, character: char) -> Result<(), MakeExprError> {
        let len = character.len_utf8();
        let next_len = output
            .len()
            .checked_add(len)
            .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
        Self::check_value_size(next_len)?;
        self.charge_work(1)?;
        self.charge_emitted(len)?;
        output.push(character);
        Ok(())
    }

    fn append_word(&mut self, output: &mut String, pieces: &[&str]) -> Result<(), MakeExprError> {
        let word_len = pieces
            .iter()
            .try_fold(0usize, |size, piece| size.checked_add(piece.len()));
        let Some(word_len) = word_len else {
            return Err(Self::resource_limit("value bytes", MAX_VALUE_BYTES));
        };
        self.charge_work(1)?;
        if word_len == 0 {
            return Ok(());
        }
        let separator_len = usize::from(!output.is_empty());
        let next_len = output
            .len()
            .checked_add(separator_len)
            .and_then(|size| size.checked_add(word_len))
            .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
        Self::check_value_size(next_len)?;
        self.charge_items(1)?;
        self.charge_emitted(separator_len.saturating_add(word_len))?;
        if !output.is_empty() {
            output.push(' ');
        }
        for piece in pieces {
            output.push_str(piece);
        }
        Ok(())
    }

    fn make_words(&mut self, raw: &str) -> Result<Vec<String>, MakeExprError> {
        self.charge_scanned(raw.len())?;
        let mut words = Vec::new();
        // GNU Make's C-locale word list uses C whitespace bytes, not Unicode
        // White_Space. NBSP and other UTF-8 filename bytes remain ordinary data.
        for word in raw
            .split(|ch: char| ch.is_ascii_whitespace() || ch == '\u{b}')
            .filter(|word| !word.is_empty())
        {
            self.charge_items(1)?;
            self.charge_emitted(word.len())?;
            self.charge_work(1)?;
            words.push(word.to_owned());
        }
        Ok(words)
    }

    fn join_words(&mut self, words: &[String]) -> Result<String, MakeExprError> {
        let payload_bytes = words
            .iter()
            .try_fold(0usize, |size, word| size.checked_add(word.len()));
        let Some(payload_bytes) = payload_bytes else {
            return Err(Self::resource_limit("value bytes", MAX_VALUE_BYTES));
        };
        let separator_bytes = words.len().saturating_sub(1);
        let bytes = payload_bytes
            .checked_add(separator_bytes)
            .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
        Self::check_value_size(bytes)?;
        self.charge_emitted(bytes)?;
        self.charge_work(words.len())?;
        let mut output = String::with_capacity(bytes);
        for (index, word) in words.iter().enumerate() {
            if index != 0 {
                output.push(' ');
            }
            output.push_str(word);
        }
        Ok(output)
    }

    fn function_arguments<'b>(
        &mut self,
        function: &str,
        raw: &'b str,
        expected: usize,
    ) -> Result<Vec<&'b str>, MakeExprError> {
        self.function_arguments_between(function, raw, expected, expected)
    }

    fn function_arguments_between<'b>(
        &mut self,
        function: &str,
        raw: &'b str,
        minimum: usize,
        maximum: usize,
    ) -> Result<Vec<&'b str>, MakeExprError> {
        self.charge_scanned(raw.len())?;
        let args = split_top_level(raw, ',', Some(maximum))?;
        if args.len() < minimum || args.len() > maximum {
            return Err(MakeExprError::InvalidSyntax {
                expression: bounded_preview(raw),
                detail: format!(
                    "function `{function}` expects {minimum}..={maximum} argument(s), got {}",
                    args.len()
                ),
            });
        }
        self.charge_items(args.len())?;
        Ok(args)
    }

    fn function_arguments_at_least<'b>(
        &mut self,
        function: &str,
        raw: &'b str,
        minimum: usize,
    ) -> Result<Vec<&'b str>, MakeExprError> {
        self.charge_scanned(raw.len())?;
        let args = split_top_level(raw, ',', None)?;
        if args.len() < minimum {
            return Err(MakeExprError::InvalidSyntax {
                expression: bounded_preview(raw),
                detail: format!(
                    "function `{function}` expects at least {minimum} argument(s), got {}",
                    args.len()
                ),
            });
        }
        self.charge_items(args.len())?;
        Ok(args)
    }

    fn filter_words(
        &mut self,
        patterns: &[String],
        words: &[String],
        keep_matches: bool,
    ) -> Result<String, MakeExprError> {
        let comparisons = patterns
            .len()
            .checked_mul(words.len())
            .ok_or_else(|| Self::resource_limit("work units", MAX_WORK_UNITS))?;
        self.charge_work(comparisons)?;
        let mut output = String::new();
        for word in words {
            let mut matched = false;
            for pattern in patterns {
                self.charge_scanned(pattern.len().saturating_add(word.len()))?;
                self.charge_work(pattern.len().saturating_add(word.len()))?;
                if pattern_stem(pattern, word).is_some() {
                    matched = true;
                    break;
                }
            }
            if matched == keep_matches {
                self.append_word(&mut output, &[word])?;
            }
        }
        Ok(output)
    }

    fn patsubst_words(
        &mut self,
        pattern: &str,
        replacement: &str,
        words: &[String],
    ) -> Result<String, MakeExprError> {
        let mut output = String::new();
        for word in words {
            self.charge_scanned(word.len().saturating_add(pattern.len()))?;
            self.charge_work(word.len().saturating_add(pattern.len()))?;
            if let Some(stem) = pattern_stem(pattern, word) {
                if let Some(percent) = replacement.find('%') {
                    self.append_word(
                        &mut output,
                        &[&replacement[..percent], stem, &replacement[percent + 1..]],
                    )?;
                } else {
                    self.append_word(&mut output, &[replacement])?;
                }
            } else {
                self.append_word(&mut output, &[word])?;
            }
        }
        Ok(output)
    }

    fn suffix_substitute_words(
        &mut self,
        words: &[String],
        from: &str,
        to: &str,
    ) -> Result<String, MakeExprError> {
        let mut output = String::new();
        for word in words {
            self.charge_scanned(word.len().saturating_add(from.len()))?;
            self.charge_work(word.len().saturating_add(from.len()))?;
            if let Some(stem) = word.strip_suffix(from) {
                self.append_word(&mut output, &[stem, to])?;
            } else {
                self.append_word(&mut output, &[word])?;
            }
        }
        Ok(output)
    }

    fn expand_text(&mut self, raw: &str, depth: usize) -> Result<String, MakeExprError> {
        if depth == 0 {
            return Err(MakeExprError::ExpansionLimit {
                expression: bounded_preview(raw),
            });
        }

        self.charge_scanned(raw.len())?;
        let mut out = String::new();
        let mut cursor = 0usize;
        while cursor < raw.len() {
            let Some(relative_dollar) = raw[cursor..].find('$') else {
                self.append_output(&mut out, &raw[cursor..])?;
                break;
            };
            let dollar = cursor + relative_dollar;
            self.append_output(&mut out, &raw[cursor..dollar])?;
            match raw.as_bytes().get(dollar + 1) {
                Some(b'$') => {
                    self.append_char(&mut out, ESCAPED_DOLLAR)?;
                    cursor = dollar + 2;
                }
                Some(b'(') => {
                    self.charge_scanned(raw.len().saturating_sub(dollar))?;
                    let end = reference_end(raw, dollar)?;
                    let expanded = self.evaluate_reference(&raw[dollar + 2..end], depth - 1)?;
                    self.append_output(&mut out, &expanded)?;
                    cursor = end + 1;
                }
                Some(b'{') => {
                    self.charge_scanned(raw.len().saturating_sub(dollar))?;
                    let end = reference_end(raw, dollar)?;
                    let body = &raw[dollar + 2..end];
                    if self.source_projection && is_deferred_cmake_reference(body) {
                        self.append_output(&mut out, &raw[dollar..=end])?;
                    } else {
                        let expanded = self.evaluate_reference(body, depth - 1)?;
                        self.append_output(&mut out, &expanded)?;
                    }
                    cursor = end + 1;
                }
                Some(character)
                    if self.automatic_variables.is_some() && b"@%<?^+|*".contains(character) =>
                {
                    let name = char::from(*character).to_string();
                    let expanded = self.evaluate_variable(&name, depth - 1)?;
                    self.append_output(&mut out, &expanded)?;
                    cursor = dollar + 2;
                }
                Some(character) if self.program_effect.is_some() && character.is_ascii() => {
                    let name = char::from(*character).to_string();
                    let expanded = self.evaluate_variable(&name, depth - 1)?;
                    self.append_output(&mut out, &expanded)?;
                    cursor = dollar + 2;
                }
                Some(_) if self.program_effect.is_some() => {
                    return Err(MakeExprError::UnsupportedReference {
                        reference: bounded_preview(&raw[dollar..]),
                    });
                }
                _ => {
                    self.append_char(&mut out, '$')?;
                    cursor = dollar + 1;
                }
            }
        }
        Ok(out)
    }

    fn evaluate_reference(&mut self, body: &str, depth: usize) -> Result<String, MakeExprError> {
        // Function argument text retains its trailing whitespace: foreach,
        // subst and other text-valued functions observe those bytes.
        let trimmed = body.trim_start();
        self.charge_scanned(trimmed.len())?;
        if trimmed.is_empty() {
            return Err(MakeExprError::InvalidSyntax {
                expression: "$()".to_owned(),
                detail: "empty variable name".to_owned(),
            });
        }

        if let Some(head_end) = top_level_whitespace(trimmed) {
            let name = &trimmed[..head_end];
            let args = trimmed[head_end..].trim_start();
            return self.evaluate_function(name, args, depth);
        }

        self.evaluate_variable(trimmed.trim_end(), depth)
    }

    fn evaluate_variable(&mut self, body: &str, depth: usize) -> Result<String, MakeExprError> {
        self.charge_scanned(
            body.len()
                .checked_mul(2)
                .ok_or_else(|| Self::resource_limit("scanned bytes", MAX_SCANNED_BYTES))?,
        )?;
        let (raw_name, substitution) = split_substitution_reference(body)?;
        let expanded_name = self.expand_text(raw_name, depth)?;
        let trimmed_name = expanded_name.trim();
        Self::check_value_size(trimmed_name.len())?;
        self.charge_emitted(trimmed_name.len())?;
        self.charge_work(1)?;
        let name = trimmed_name.to_owned();
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(MakeExprError::InvalidSyntax {
                expression: bounded_preview(body),
                detail: format!("invalid expanded variable name `{name}`"),
            });
        }

        self.charge_work(self.expansion_chain.len())?;
        let compared_bytes = self
            .expansion_chain
            .iter()
            .try_fold(
                name.len().saturating_mul(self.expansion_chain.len()),
                |size, item| size.checked_add(item.len()),
            )
            .ok_or_else(|| Self::resource_limit("scanned bytes", MAX_SCANNED_BYTES))?;
        self.charge_scanned(compared_bytes)?;
        if let Some(at) = self.expansion_chain.iter().position(|item| item == &name) {
            let chain_bytes = self.expansion_chain[at..]
                .iter()
                .try_fold(0usize, |size, item| size.checked_add(item.len()))
                .and_then(|size| size.checked_add(name.len()))
                .ok_or_else(|| {
                    Self::resource_limit("aggregate emitted bytes", MAX_EMITTED_BYTES)
                })?;
            self.charge_emitted(chain_bytes)?;
            self.charge_work(self.expansion_chain.len() - at + 1)?;
            let mut chain = self.expansion_chain[at..].to_vec();
            chain.push(name);
            return Err(MakeExprError::VariableCycle {
                expansion_chain: chain,
            });
        }

        let raw_value = self.resolve_variable(&name)?;
        self.expansion_chain.push(name);
        let expanded = self.expand_text(&raw_value, depth);
        self.expansion_chain.pop();
        let expanded = expanded?;

        let Some((raw_from, raw_to)) = substitution else {
            return Ok(expanded);
        };
        let expanded_from = self.expand_text(raw_from, depth)?;
        let from = expanded_from.trim().to_owned();
        let expanded_to = self.expand_text(raw_to, depth)?;
        let to = expanded_to.trim().to_owned();
        let words = self.make_words(&expanded)?;
        if from.contains('%') {
            self.patsubst_words(&from, &to, &words)
        } else {
            self.suffix_substitute_words(&words, &from, &to)
        }
    }

    fn resolve_variable(&self, name: &str) -> Result<String, MakeExprError> {
        if let Some((_, value)) = self.loop_vars.iter().rev().find(|(bound, _)| bound == name) {
            return Ok(value.clone());
        }
        if let Some(values) = self
            .automatic_variables
            .filter(|_| automatic_variable_name(name))
        {
            return values
                .get(name)
                .map(|value| value.replace('$', "$$"))
                .ok_or_else(|| MakeExprError::UnresolvedVariables {
                    names: vec![name.to_owned()],
                    expansion_chain: self.expansion_chain.clone(),
                });
        }
        if let Some(value) = self.context_value(name) {
            return Ok(value);
        }
        if let Some(body) = self
            .definitions
            .and_then(|definitions| definitions.bodies.get(name))
        {
            if let Some(detail) = self.guard.and_then(|guard| guard(name)) {
                return Err(MakeExprError::UnsafeVariable {
                    name: name.into(),
                    detail,
                    expansion_chain: self.expansion_chain.clone(),
                });
            }
            if let Some(value) = self.lookup.and_then(|lookup| lookup(name)) {
                return Ok(value);
            }
            return Ok(body.clone());
        }
        self.check_guard(name)?;
        if let Some(value) = self.lookup.and_then(|lookup| lookup(name)) {
            return Ok(value);
        }
        if let Some(value) = self.scope.raw_at(name, self.line) {
            return Ok(value);
        }

        if !self.source_projection {
            return Err(MakeExprError::UnresolvedVariables {
                names: vec![name.to_owned()],
                expansion_chain: self.expansion_chain.clone(),
            });
        }

        let reference = format!("$({name})");
        // Variables imported from make.cfg were simply expanded before a
        // project mmakefile can shadow names such as TARGETDIR. Resolve that
        // configured chain on its own first. Falling through to expand_with is
        // reserved for the few collector/local expressions that genuinely
        // need values from both scopes.
        if let Some(expanded) = self.dirs.expand(&reference) {
            return Ok(expanded);
        }
        let guarded = RefCell::new(None);
        let local = |nested: &str| {
            if let Some(value) = self.context_value(nested) {
                return Some(value);
            }
            if let Some(detail) = self.guard_reason(nested) {
                *guarded.borrow_mut() = Some((nested.to_owned(), detail));
                return None;
            }
            self.lookup
                .and_then(|lookup| lookup(nested))
                // A global directory value can legitimately refer to a name
                // shadowed by the local variable currently being expanded.
                // `TARGETDIR := $(AROS_TESTS)/Library` is the real example:
                // the configured AROS_TESTS chain reaches the global
                // TARGETDIR. Re-entering the local assignment creates a fake
                // cycle; skipping active names gives that nested lookup the
                // configured value Make had when `:=` ran.
                .or_else(|| {
                    (!self.expansion_chain.iter().any(|item| item == nested))
                        .then(|| self.scope.raw_at(nested, self.line))
                        .flatten()
                })
        };
        let expanded = self.dirs.expand_with(&reference, &local);
        if let Some((name, detail)) = guarded.into_inner() {
            return Err(MakeExprError::UnsafeVariable {
                name,
                detail,
                expansion_chain: self.expansion_chain.clone(),
            });
        }
        expanded.map_err(|names| MakeExprError::UnresolvedVariables {
            names,
            expansion_chain: self.expansion_chain.clone(),
        })
    }

    fn context_value(&self, name: &str) -> Option<String> {
        if !self.source_projection {
            return None;
        }
        match name {
            "SRCDIR" => Some("${AROS_SOURCE_DIR}".to_owned()),
            "TOP" => Some("${AROS_BUILD_DIR}".to_owned()),
            "CURDIR" => Some(self.relative_text.clone()),
            _ => None,
        }
    }

    fn check_guard(&self, name: &str) -> Result<(), MakeExprError> {
        let Some(detail) = self.guard_reason(name) else {
            return Ok(());
        };
        Err(MakeExprError::UnsafeVariable {
            name: name.to_owned(),
            detail,
            expansion_chain: self.expansion_chain.clone(),
        })
    }

    fn guard_reason(&self, name: &str) -> Option<String> {
        if self.scope.conditionally_assigned_before(name, self.line) {
            return Some("assigned inside an unevaluated Make conditional".to_owned());
        }
        self.guard.and_then(|guard| guard(name))
    }

    fn inspect_variable(
        &mut self,
        name: &str,
        raw_args: &str,
        depth: usize,
    ) -> Result<String, MakeExprError> {
        let args = self.function_arguments(name, raw_args, 1)?;
        let expanded_name = self.expand_text(args[0], depth)?;
        let variable = self.finish_value(expanded_name)?;
        if variable.is_empty() {
            return Ok(if name == "value" {
                String::new()
            } else {
                "undefined".into()
            });
        }
        if variable.contains(char::is_whitespace) {
            return Err(MakeExprError::InvalidSyntax {
                expression: bounded_preview(raw_args),
                detail: "inspection requires one supported variable name".into(),
            });
        }
        let query = match name {
            "origin" => MakeVariableQuery::Origin,
            "flavor" => MakeVariableQuery::Flavor,
            _ => MakeVariableQuery::RawValue,
        };
        let automatic = self
            .loop_vars
            .iter()
            .rev()
            .find(|(bound, _)| bound == &variable)
            .map(|(_, value)| value)
            .or_else(|| {
                self.automatic_variables
                    .and_then(|values| values.get(&variable))
            });
        let literal = if let Some(value) = automatic {
            Some(match query {
                MakeVariableQuery::Origin => "automatic".into(),
                MakeVariableQuery::Flavor => "simple".into(),
                MakeVariableQuery::RawValue => value.clone(),
            })
        } else {
            self.inspection
                .and_then(|inspect| inspect(query, &variable))
        };
        let value = if let Some(value) = literal {
            // Raw dollars are data until a deliberate later eval stage.
            Self::check_value_size(value.len())?;
            self.charge_scanned(value.len())?;
            self.charge_work(value.len())?;
            let encoded_len = value
                .len()
                .checked_add(
                    value
                        .bytes()
                        .filter(|byte| *byte == b'$')
                        .count()
                        .saturating_mul(ESCAPED_DOLLAR.len_utf8() - 1),
                )
                .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
            Self::check_value_size(encoded_len)?;
            value.replace('$', &ESCAPED_DOLLAR.to_string())
        } else if self.inspection.is_none() && name == "value" {
            // Legacy collectors do not prove origin/flavor or literal-dollar
            // provenance. Keep their existing bounded behavior.
            self.resolve_variable(&variable)?
        } else {
            return Err(MakeExprError::UnsafeVariable {
                name: "<source inspection>".into(),
                detail: "variable inspection is not proved by source state".into(),
                expansion_chain: Vec::new(),
            });
        };
        Self::check_value_size(value.len())?;
        self.charge_emitted(value.len())?;
        self.charge_work(value.len())?;
        Ok(value)
    }

    fn evaluate_function(
        &mut self,
        name: &str,
        raw_args: &str,
        depth: usize,
    ) -> Result<String, MakeExprError> {
        match name {
            "shell" if self.pure_text_pipeline => {
                let expanded = self.expand_text(raw_args, depth)?;
                let command = self.finish_value(expanded)?;
                if !self.source_projection {
                    if let Some(shell) = self.captured_shell {
                        let value =
                            shell(&command).map_err(|detail| MakeExprError::InvalidSyntax {
                                expression: "captured shell data".into(),
                                detail,
                            })?;
                        Self::check_value_size(value.len())?;
                        self.charge_scanned(command.len())?;
                        return self.literal_data_value(value);
                    }
                }
                let result =
                    if self.source_projection {
                        crate::make_text_pipeline::evaluate_with_echo_data(&command, &|data| {
                            let token = "${AROS_SOURCE_DIR}";
                            let count = data.matches(token).count();
                            data.len()
                        .checked_sub(
                            count
                                .checked_mul(token.len())
                                .ok_or("source token size overflow")?,
                        )
                        .and_then(|bytes| {
                            bytes.checked_add(count.checked_mul(self.source_text.len())?)
                        })
                        .filter(|bytes| *bytes <= MAX_VALUE_BYTES)
                        .ok_or("pure text source-token materialization exceeds value byte limit")?;
                            Ok(data.replace(token, &self.source_text))
                        })
                    } else {
                        crate::make_text_pipeline::evaluate_source(&command)
                    };
                let value = result.map_err(|detail| MakeExprError::InvalidSyntax {
                    expression: "pure text pipeline".into(),
                    detail,
                })?;
                Self::check_value_size(value.len())?;
                self.charge_scanned(command.len())?;
                self.literal_data_value(value)
            }
            "realpath" if self.captured_realpath.is_some() => {
                // GNU's max-one-argument function preserves commas as data;
                // an empty argument simply contains no pathname words.
                let expanded = self.expand_text(raw_args, depth)?;
                let lookup = self.captured_realpath.ok_or_else(|| {
                    MakeExprError::FilesystemAccessDisabled {
                        function: "realpath".into(),
                    }
                })?;
                let mut output = Vec::new();
                for input in self.make_words(&expanded)? {
                    self.charge_work(input.len())?;
                    let input = input.replace(ESCAPED_DOLLAR, "$");
                    if let Some(path) =
                        lookup(&input).map_err(|detail| MakeExprError::InvalidSyntax {
                            expression: "captured realpath".into(),
                            detail: bounded_preview(&detail),
                        })?
                    {
                        self.charge_work(path.len())?;
                        if !Path::new(&path).is_absolute()
                            || path.chars().any(char::is_control)
                            || path.contains(ESCAPED_DOLLAR)
                            || Path::new(&path).components().any(|component| {
                                matches!(
                                    component,
                                    std::path::Component::ParentDir | std::path::Component::CurDir
                                )
                            })
                        {
                            return Err(MakeExprError::InvalidSyntax {
                                expression: "captured realpath".into(),
                                detail: "provider returned a noncanonical absolute path".into(),
                            });
                        }
                        // Filesystem results are data, not another Make
                        // expansion. Keep dollars protected until finish_value.
                        output.push(path.replace('$', &ESCAPED_DOLLAR.to_string()));
                    }
                }
                self.join_words(&output)
            }
            "abspath" => {
                let args = self.function_arguments(name, raw_args, 1)?;
                let expanded = self.expand_text(args[0], depth)?;
                // GNU Make expands variable values before splitting the
                // function's word list, including spaces in explicit roots.
                let mut expanded = if self.source_projection {
                    let source = self.literal_data_value(self.source_text.clone())?;
                    materialize_path_token(&expanded, "${AROS_SOURCE_DIR}", &source)?
                } else {
                    expanded
                };
                if let Some(build) = self.build_directory.filter(|_| self.source_projection) {
                    let build = build.to_str().ok_or_else(|| MakeExprError::InvalidSyntax {
                        expression: "abspath build directory".into(),
                        detail: "Make build directory is not valid UTF-8".into(),
                    })?;
                    let build = self.literal_data_value(build.to_owned())?;
                    expanded = materialize_path_token(&expanded, "${AROS_BUILD_DIR}", &build)?;
                }
                let words = self.make_words(&expanded)?;
                let mut output = String::new();
                for word in words {
                    if word.contains('$') {
                        return Err(MakeExprError::UnsupportedReference {
                            reference: bounded_preview(&word),
                        });
                    }
                    let word = word.replace(ESCAPED_DOLLAR, "$");
                    let path = Path::new(&word);
                    let path = if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        let cwd =
                            self.working_directory
                                .ok_or_else(|| {
                                    MakeExprError::InvalidSyntax {
                                expression: "abspath".into(),
                                detail:
                                    "relative abspath requires an explicit Make working directory"
                                        .into(),
                            }
                                })?;
                        let joined_length = cwd
                            .as_os_str()
                            .len()
                            .checked_add(path.as_os_str().len())
                            .and_then(|length| length.checked_add(1))
                            .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
                        Self::check_value_size(joined_length)?;
                        cwd.join(path)
                    };
                    let mut normalized = PathBuf::new();
                    for component in path.components() {
                        match component {
                            std::path::Component::ParentDir => {
                                normalized.pop();
                            }
                            std::path::Component::CurDir => {}
                            part => normalized.push(part.as_os_str()),
                        }
                    }
                    let normalized =
                        self.literal_data_value(path_text(&normalized, "abspath result")?)?;
                    self.append_word(&mut output, &[&normalized])?;
                }
                Ok(output)
            }
            "eval" if self.program_effect.is_some() => {
                let expanded = self.expand_text(raw_args, depth)?;
                let statements = self.finish_value(expanded)?;
                self.program_budget
                    .expect("program mode supplies a shared budget")
                    .charge_effect()?;
                self.charge_work(1)?;
                self.charge_scanned(statements.len())?;
                self.program_effect.expect("program effect was checked")(&statements)?;
                Ok(String::new())
            }
            "eval" if self.rule_outputs.is_some() => {
                let expanded = self.expand_text(raw_args, depth)?;
                let rules = self.finish_value(expanded)?;
                validate_rule_eval_output(&rules)?;
                let outputs = self.rule_outputs.as_mut().ok_or_else(|| {
                    MakeExprError::UnsupportedFunction {
                        name: "eval".into(),
                    }
                })?;
                if outputs.len() >= 16_384 {
                    return Err(Self::resource_limit("eval rule outputs", 16_384));
                }
                outputs.push(rules);
                Ok(String::new())
            }
            // $(foreach var,list,text): bind var to each word of list in turn
            // and expand text, joining the results with a single space. The
            // binding is temporary and shadows a global of the same name.
            //
            // rom/dos needs this for its image loaders,
            // `$(foreach img, aos elf, internalloadseg_$(img))`, without which
            // dos.library is built with no ELF loader at all. muimaster needs
            // it for its 44 classes.
            "foreach" => {
                let args = self.function_arguments(name, raw_args, 3)?;
                let variable = self.expand_text(args[0].trim(), depth)?;
                let variable = variable.trim().to_owned();
                if variable.is_empty() {
                    return Err(MakeExprError::InvalidSyntax {
                        expression: raw_args.to_owned(),
                        detail: "foreach has an empty loop variable name".to_owned(),
                    });
                }
                let expanded_list = self.expand_text(args[1].trim(), depth)?;
                let list = self.make_words(&expanded_list)?;
                let mut output = String::new();
                for (index, word) in list.into_iter().enumerate() {
                    self.charge_items(1)?;
                    self.charge_emitted(variable.len())?;
                    self.charge_work(1)?;
                    self.loop_vars.push((variable.clone(), word));
                    let expanded = self.expand_text(args[2], depth);
                    self.loop_vars.pop();
                    let expanded = expanded?;
                    if index != 0 {
                        self.append_char(&mut output, ' ')?;
                    }
                    self.append_output(&mut output, &expanded)?;
                }
                Ok(output)
            }
            "addprefix" | "addsuffix" | "filter" | "filter-out" => {
                let args = self.function_arguments(name, raw_args, 2)?;
                let first = self.expand_text(args[0].trim(), depth)?;
                let expanded_words = self.expand_text(args[1].trim(), depth)?;
                let words = self.make_words(&expanded_words)?;
                match name {
                    "addprefix" | "addsuffix" => {
                        let prefix_or_suffix = first.trim();
                        let mut output = String::new();
                        for word in &words {
                            if name == "addprefix" {
                                self.append_word(&mut output, &[prefix_or_suffix, word])?;
                            } else {
                                self.append_word(&mut output, &[word, prefix_or_suffix])?;
                            }
                        }
                        Ok(output)
                    }
                    "filter" | "filter-out" => {
                        let patterns = self.make_words(&first)?;
                        self.filter_words(&patterns, &words, name == "filter")
                    }
                    unsupported => Err(MakeExprError::UnsupportedFunction {
                        name: unsupported.to_owned(),
                    }),
                }
            }
            "patsubst" => {
                let args = self.function_arguments(name, raw_args, 3)?;
                let pattern = self.expand_text(args[0].trim(), depth)?;
                let replacement = self.expand_text(args[1].trim(), depth)?;
                let expanded_words = self.expand_text(args[2].trim(), depth)?;
                let words = self.make_words(&expanded_words)?;
                self.patsubst_words(pattern.trim(), replacement.trim(), &words)
            }
            "subst" => {
                let args = self.function_arguments(name, raw_args, 3)?;
                let from = self.expand_text(args[0], depth)?;
                if from.is_empty() {
                    return Err(MakeExprError::InvalidSyntax {
                        expression: bounded_preview(raw_args),
                        detail: "subst with an empty search string is not supported".to_owned(),
                    });
                }
                let to = self.expand_text(args[1], depth)?;
                let text = self.expand_text(args[2], depth)?;
                self.charge_scanned(text.len())?;
                let matches = text.match_indices(&from).count();
                let removed = matches
                    .checked_mul(from.len())
                    .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
                let added = matches
                    .checked_mul(to.len())
                    .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
                let output_len = text
                    .len()
                    .checked_sub(removed)
                    .and_then(|length| length.checked_add(added))
                    .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
                Self::check_value_size(output_len)?;
                self.charge_work(matches)?;
                self.charge_emitted(output_len)?;
                Ok(text.replace(&from, &to))
            }
            "findstring" => {
                let args = self.function_arguments(name, raw_args, 2)?;
                let needle = self.expand_text(args[0], depth)?;
                let haystack = self.expand_text(args[1], depth)?;
                self.charge_scanned(needle.len().saturating_add(haystack.len()))?;
                self.charge_work(1)?;
                Ok(if haystack.contains(&needle) {
                    needle
                } else {
                    String::new()
                })
            }
            "word" | "wordlist" => {
                let expected = if name == "word" { 2 } else { 3 };
                let args = self.function_arguments(name, raw_args, expected)?;
                let first =
                    parse_word_index(name, &self.expand_text(args[0].trim(), depth)?, raw_args)?;
                let (last, text_at) = if name == "word" {
                    (first, 1)
                } else {
                    (
                        parse_word_index(
                            name,
                            &self.expand_text(args[1].trim(), depth)?,
                            raw_args,
                        )?,
                        2,
                    )
                };
                let expanded_words = self.expand_text(args[text_at], depth)?;
                let words = self.make_words(&expanded_words)?;
                if first == 0 || last < first || first > words.len() {
                    return Ok(String::new());
                }
                let end = last.min(words.len());
                self.join_words(&words[first - 1..end])
            }
            "join" => {
                let args = self.function_arguments(name, raw_args, 2)?;
                let expanded_left = self.expand_text(args[0], depth)?;
                let left = self.make_words(&expanded_left)?;
                let expanded_right = self.expand_text(args[1], depth)?;
                let right = self.make_words(&expanded_right)?;
                let length = left.len().max(right.len());
                let mut output = String::new();
                for index in 0..length {
                    self.append_word(
                        &mut output,
                        &[
                            left.get(index).map_or("", String::as_str),
                            right.get(index).map_or("", String::as_str),
                        ],
                    )?;
                }
                Ok(output)
            }
            "if" => {
                let args = self.function_arguments_between(name, raw_args, 2, 3)?;
                let condition = self.expand_text(args[0], depth)?;
                let selected = if condition.trim().is_empty() {
                    args.get(2).copied().unwrap_or("")
                } else {
                    args[1]
                };
                self.expand_text(selected, depth)
            }
            "or" | "and" => {
                let args = self.function_arguments_at_least(name, raw_args, 1)?;
                let mut last = String::new();
                for argument in args {
                    let value = self.expand_text(argument, depth)?;
                    let nonempty = !value.is_empty();
                    if (name == "or" && nonempty) || (name == "and" && !nonempty) {
                        return Ok(value);
                    }
                    last = value;
                }
                if name == "and" {
                    Ok(last)
                } else {
                    Ok(String::new())
                }
            }
            "notdir" | "dir" | "basename" | "suffix" | "sort" | "strip" | "wildcard" => {
                let args = self.function_arguments(name, raw_args, 1)?;
                let expanded = self.expand_text(args[0].trim(), depth)?;
                match name {
                    "notdir" | "dir" | "basename" | "suffix" => {
                        let words = self.make_words(&expanded)?;
                        let mut output = String::new();
                        for word in &words {
                            self.charge_work(word.len())?;
                            let transformed = match name {
                                "notdir" => notdir(word),
                                "dir" => Some(directory_part(word)),
                                "basename" => basename(word),
                                "suffix" => suffix(word),
                                _ => unreachable!(),
                            };
                            if let Some(transformed) = transformed {
                                self.append_word(&mut output, &[&transformed])?;
                            }
                        }
                        Ok(output)
                    }
                    "sort" => {
                        let mut words = self.make_words(&expanded)?;
                        let log_word_count = usize::try_from(words.len().max(1).ilog2())
                            .map_err(|_| Self::resource_limit("work units", MAX_WORK_UNITS))?;
                        let comparisons = words
                            .len()
                            .checked_mul(log_word_count + 1)
                            .ok_or_else(|| Self::resource_limit("work units", MAX_WORK_UNITS))?;
                        let maximum_word_len = words.iter().map(String::len).max().unwrap_or(0);
                        self.charge_work(
                            comparisons
                                .checked_mul(maximum_word_len.max(1))
                                .ok_or_else(|| {
                                    Self::resource_limit("work units", MAX_WORK_UNITS)
                                })?,
                        )?;
                        words.sort();
                        words.dedup();
                        self.join_words(&words)
                    }
                    "strip" => {
                        let words = self.make_words(&expanded)?;
                        self.join_words(&words)
                    }
                    "wildcard" => {
                        let words = self.wildcard(&expanded, false)?;
                        self.join_words(&words)
                    }
                    unsupported => Err(MakeExprError::UnsupportedFunction {
                        name: unsupported.to_owned(),
                    }),
                }
            }
            "firstword" | "lastword" | "words" => {
                let args = self.function_arguments(name, raw_args, 1)?;
                let expanded = self.expand_text(args[0], depth)?;
                let words = self.make_words(&expanded)?;
                match name {
                    "firstword" => Ok(words.into_iter().next().unwrap_or_default()),
                    "lastword" => Ok(words.into_iter().last().unwrap_or_default()),
                    "words" => Ok(words.len().to_string()),
                    unsupported => Err(MakeExprError::UnsupportedFunction {
                        name: unsupported.to_owned(),
                    }),
                }
            }
            "value" | "origin" | "flavor" => self.inspect_variable(name, raw_args, depth),
            "call" => {
                let args = self.function_arguments_at_least(name, raw_args, 1)?;
                // GNU expands the complete first argument, then uses its
                // first whitespace-delimited word as the callee. Remaining
                // text is not an implicit argument or part of the name.
                let expanded_callee = self.expand_text(args[0], depth)?;
                let Some(callee) = expanded_callee.split_ascii_whitespace().next() else {
                    return Ok(String::new());
                };
                if self.source_projection && callee == "WILDCARD" {
                    if args.len() != 2 {
                        return Err(MakeExprError::InvalidSyntax {
                            expression: bounded_preview(raw_args),
                            detail: "AROS WILDCARD expects one argument".to_owned(),
                        });
                    }
                    let patterns = self.expand_text(args[1].trim(), depth)?;
                    let words = self.wildcard(&patterns, true)?;
                    return self.join_words(&words);
                }
                // GNU Make gives built-in function names precedence over
                // user variables in call. Do not silently invoke a same-name
                // definition until call's built-in argument semantics exist.
                if is_make_builtin_function(callee) {
                    return Err(MakeExprError::UnsupportedFunction {
                        name: format!("call built-in {callee}"),
                    });
                }
                let body = self.resolve_variable(callee)?;
                let mut bindings = Vec::with_capacity(args.len());
                bindings.push(("0".to_owned(), callee.to_owned()));
                for (index, argument) in args.iter().skip(1).enumerate() {
                    bindings.push(((index + 1).to_string(), self.expand_text(argument, depth)?));
                }
                for index in args.len()..self.call_arity {
                    bindings.push((index.to_string(), String::new()));
                }
                self.charge_items(bindings.len())?;
                let binding_count = bindings.len();
                let previous_arity = self.call_arity;
                self.call_arity = previous_arity.max(args.len());
                self.loop_vars.extend(bindings);
                let expanded = self.expand_text(&body, depth);
                self.loop_vars
                    .truncate(self.loop_vars.len() - binding_count);
                self.call_arity = previous_arity;
                expanded
            }
            _ => Err(MakeExprError::UnsupportedFunction {
                name: name.to_owned(),
            }),
        }
    }

    fn wildcard(
        &mut self,
        expanded_patterns: &str,
        regular_files_only: bool,
    ) -> Result<Vec<String>, MakeExprError> {
        if let Some(lookup) = self.captured_wildcard.filter(|_| !regular_files_only) {
            self.charge_scanned(expanded_patterns.len())?;
            reject_unsupported_references(expanded_patterns)?;
            let mut result = Vec::new();
            for pattern in self.make_words(expanded_patterns)? {
                self.charge_work(pattern.len())?;
                let pattern = pattern.replace(ESCAPED_DOLLAR, "$");
                let paths = lookup(&pattern).map_err(|detail| MakeExprError::Wildcard {
                    pattern: bounded_preview(&pattern),
                    detail: bounded_preview(&detail),
                })?;
                self.charge_items(paths.len())?;
                for path in paths {
                    self.charge_work(path.len())?;
                    self.charge_emitted(path.len())?;
                    if path.chars().any(char::is_control) || path.contains(ESCAPED_DOLLAR) {
                        return Err(MakeExprError::Wildcard {
                            pattern: bounded_preview(&pattern),
                            detail: "invalid captured wildcard path data".into(),
                        });
                    }
                    result.push(self.literal_data_value(path)?);
                }
            }
            return Ok(result);
        }
        if !self.filesystem_enabled {
            return Err(MakeExprError::FilesystemAccessDisabled {
                function: if regular_files_only {
                    "call WILDCARD".to_owned()
                } else {
                    "wildcard".to_owned()
                },
            });
        }
        self.charge_scanned(expanded_patterns.len())?;
        reject_unsupported_references(expanded_patterns)?;
        let patterns = self.make_words(expanded_patterns)?;
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        let wildcard_root = if self.source_projection {
            self.wildcard_root.as_path()
        } else {
            self.working_directory
                .ok_or_else(|| MakeExprError::InvalidSyntax {
                    expression: "wildcard".into(),
                    detail: "source wildcard requires an explicit Make working directory".into(),
                })?
        };
        let root_text = path_text(wildcard_root, "wildcard root")?;
        let escaped_root = Pattern::escape(&root_text);
        let escaped_source = Pattern::escape(&self.source_text);
        let mut output = Vec::new();
        let mut output_bytes = 0usize;

        for original_pattern in patterns {
            self.charge_work(original_pattern.len())?;
            let (materialized_pattern, glob_pattern, backing) = if let Some(suffix) =
                original_pattern
                    .strip_prefix("${AROS_SOURCE_DIR}")
                    .filter(|_| self.source_projection)
            {
                if suffix.contains("${") {
                    return Err(MakeExprError::DeferredWildcard {
                        pattern: bounded_preview(&original_pattern),
                    });
                }
                (
                    concatenate_path_prefix(&self.source_text, suffix),
                    concatenate_path_prefix(&escaped_source, suffix),
                    Some((self.source_text.clone(), "${AROS_SOURCE_DIR}".to_owned())),
                )
            } else if let Some(suffix) = original_pattern
                .strip_prefix("${AROS_PORTS_DIR}")
                .filter(|_| self.source_projection)
            {
                if suffix.contains("${") {
                    return Err(MakeExprError::DeferredWildcard {
                        pattern: bounded_preview(&original_pattern),
                    });
                }
                let Some(ports_root) = self.dirs.materialized_path("AROS_PORTS_DIR") else {
                    return Err(MakeExprError::DeferredWildcard {
                        pattern: bounded_preview(&original_pattern),
                    });
                };
                let ports_text = path_text(ports_root, "Ports directory")?;
                let escaped_ports = Pattern::escape(&ports_text);
                (
                    concatenate_path_prefix(&ports_text, suffix),
                    concatenate_path_prefix(&escaped_ports, suffix),
                    Some((ports_text, "${AROS_PORTS_DIR}".to_owned())),
                )
            } else {
                if original_pattern.contains("${") {
                    return Err(MakeExprError::DeferredWildcard {
                        pattern: bounded_preview(&original_pattern),
                    });
                }
                let absolute = Path::new(&original_pattern).is_absolute();
                let glob_pattern = if absolute || escaped_root.is_empty() {
                    original_pattern.clone()
                } else {
                    concatenate_path_prefix(&escaped_root, &original_pattern)
                };
                (original_pattern.clone(), glob_pattern, None)
            };
            if materialized_pattern.contains("${") {
                return Err(MakeExprError::DeferredWildcard {
                    pattern: bounded_preview(&original_pattern),
                });
            }
            let absolute = Path::new(&materialized_pattern).is_absolute();
            let glob_pattern = glob_pattern.replace(ESCAPED_DOLLAR, "$");
            let paths =
                glob_with(&glob_pattern, options).map_err(|error| MakeExprError::Wildcard {
                    pattern: bounded_preview(&original_pattern),
                    detail: bounded_preview(&error.to_string()),
                })?;
            let mut matches = Vec::new();
            let mut pattern_bytes = 0usize;
            for result in paths {
                let path = result.map_err(|error| MakeExprError::Wildcard {
                    pattern: bounded_preview(&original_pattern),
                    detail: bounded_preview(&error.to_string()),
                })?;
                self.charge_work(1)?;
                self.charge_items(1)?;
                if regular_files_only && !path.is_file() {
                    continue;
                }
                let shown = if let Some((physical_root, logical_root)) = &backing {
                    let relative = path.strip_prefix(Path::new(physical_root)).map_err(|_| {
                        MakeExprError::Wildcard {
                            pattern: bounded_preview(&original_pattern),
                            detail: format!(
                                "match escaped materialized directory `{}`",
                                bounded_preview(physical_root)
                            ),
                        }
                    })?;
                    let relative = self
                        .literal_data_value(path_text(relative, "materialized wildcard result")?)?;
                    if relative.is_empty() {
                        logical_root.clone()
                    } else {
                        format!("{logical_root}/{relative}")
                    }
                } else {
                    let shown = if absolute {
                        path.as_path()
                    } else {
                        path.strip_prefix(Path::new(&root_text))
                            .unwrap_or(path.as_path())
                    };
                    self.literal_data_value(path_text(shown, "wildcard result")?)?
                };
                self.charge_emitted(shown.len())?;
                pattern_bytes = pattern_bytes
                    .checked_add(shown.len())
                    .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
                if pattern_bytes > MAX_VALUE_BYTES {
                    return Err(Self::resource_limit("value bytes", MAX_VALUE_BYTES));
                }
                matches.push(shown);
            }
            let separator_count = matches
                .len()
                .saturating_sub(1)
                .saturating_add(usize::from(!output.is_empty() && !matches.is_empty()));
            let next_output_bytes = output_bytes
                .checked_add(pattern_bytes)
                .and_then(|size| size.checked_add(separator_count))
                .ok_or_else(|| Self::resource_limit("value bytes", MAX_VALUE_BYTES))?;
            if next_output_bytes > MAX_VALUE_BYTES {
                return Err(Self::resource_limit("value bytes", MAX_VALUE_BYTES));
            }
            let log_match_count = usize::try_from(matches.len().max(1).ilog2())
                .map_err(|_| Self::resource_limit("work units", MAX_WORK_UNITS))?;
            let sort_comparisons = matches
                .len()
                .checked_mul(log_match_count + 1)
                .ok_or_else(|| Self::resource_limit("work units", MAX_WORK_UNITS))?;
            let maximum_match_len = matches.iter().map(String::len).max().unwrap_or(0);
            self.charge_work(
                sort_comparisons
                    .checked_mul(maximum_match_len.max(1))
                    .ok_or_else(|| Self::resource_limit("work units", MAX_WORK_UNITS))?,
            )?;
            matches.sort();
            // An empty source-tree wildcard is ordinary Make behaviour. An
            // empty wildcard below a fetched Ports root, however, means the
            // configure-time source inventory is not present yet. Preserve
            // that distinction so the graph can order the fetch and rerun
            // CMake instead of silently linking a partial module.
            if matches.is_empty()
                && backing
                    .as_ref()
                    .is_some_and(|(_, logical)| logical == "${AROS_PORTS_DIR}")
            {
                return Err(MakeExprError::DeferredWildcard {
                    pattern: bounded_preview(&original_pattern),
                });
            }
            output_bytes = next_output_bytes;
            output.extend(matches);
        }
        Ok(output)
    }
}

fn automatic_variable_name(name: &str) -> bool {
    match name.as_bytes() {
        [automatic] | [automatic, b'D' | b'F'] => b"@%<?^+|*".contains(automatic),
        _ => false,
    }
}

fn is_make_builtin_function(name: &str) -> bool {
    matches!(
        name,
        "subst"
            | "patsubst"
            | "strip"
            | "findstring"
            | "filter"
            | "filter-out"
            | "sort"
            | "word"
            | "wordlist"
            | "words"
            | "firstword"
            | "lastword"
            | "dir"
            | "notdir"
            | "suffix"
            | "basename"
            | "addsuffix"
            | "addprefix"
            | "join"
            | "wildcard"
            | "realpath"
            | "abspath"
            | "if"
            | "or"
            | "and"
            | "intcmp"
            | "foreach"
            | "file"
            | "call"
            | "value"
            | "eval"
            | "origin"
            | "flavor"
            | "let"
            | "shell"
            | "error"
            | "warning"
            | "info"
            | "guile"
    )
}

fn concatenate_path_prefix(prefix: &str, suffix: &str) -> String {
    match (prefix.ends_with('/'), suffix.starts_with('/')) {
        (true, true) => format!("{}{}", prefix, &suffix[1..]),
        (false, false) if !prefix.is_empty() && !suffix.is_empty() => {
            format!("{prefix}/{suffix}")
        }
        _ => format!("{prefix}{suffix}"),
    }
}

fn bounded_preview(value: &str) -> String {
    if value.len() <= MAX_ERROR_PREVIEW_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_ERROR_PREVIEW_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn materialize_path_token(
    word: &str,
    token: &str,
    replacement: &str,
) -> Result<String, MakeExprError> {
    let count = word.matches(token).count();
    let length = count
        .checked_mul(token.len())
        .and_then(|removed| word.len().checked_sub(removed))
        .and_then(|remaining| remaining.checked_add(count.checked_mul(replacement.len())?))
        .ok_or_else(|| Evaluator::resource_limit("value bytes", MAX_VALUE_BYTES))?;
    Evaluator::check_value_size(length)?;
    Ok(word.replace(token, replacement))
}

fn path_text(path: &Path, purpose: &str) -> Result<String, MakeExprError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| MakeExprError::InvalidSyntax {
            expression: path.display().to_string(),
            detail: format!("{purpose} is not valid UTF-8"),
        })
}

/// Recognize a complete top-level expansion statement without confusing a
/// variable-prefixed assignment or target with an executable expression.
pub(crate) fn standalone_expansion(raw: &str) -> Result<bool, MakeExprError> {
    let mut remaining = raw.trim();
    if !(remaining.starts_with("$(") || remaining.starts_with("${")) {
        return Ok(false);
    }
    loop {
        let end = reference_end(remaining, 0)?;
        remaining = remaining[end + 1..].trim_start();
        if remaining.is_empty() {
            return Ok(true);
        }
        if !(remaining.starts_with("$(") || remaining.starts_with("${")) {
            return Ok(false);
        }
    }
}

fn reference_end(raw: &str, start: usize) -> Result<usize, MakeExprError> {
    delimited_end(raw, start + 1)
}

fn delimited_end(raw: &str, opening: usize) -> Result<usize, MakeExprError> {
    let bytes = raw.as_bytes();
    match delimited_end_bytes(bytes, opening) {
        Ok(end) => Ok(end),
        Err(DelimitedEndError::DepthLimit) => Err(MakeExprError::ExpansionLimit {
            expression: bounded_preview(&raw[opening..]),
        }),
        Err(DelimitedEndError::Unclosed) => Err(MakeExprError::InvalidSyntax {
            expression: bounded_preview(&raw[opening..]),
            detail: "unclosed Make reference".to_owned(),
        }),
    }
}

/// Find the matching close of a `$(`/`${` reference in untrusted source bytes.
///
/// This shares the evaluator's bounded delimiter walk without requiring the
/// whole source line to be UTF-8. `None` means the opener is invalid,
/// unbalanced, or exceeds the evaluator's reference nesting limit.
pub(crate) fn reference_end_bytes(bytes: &[u8], dollar: usize) -> Option<usize> {
    let opening = dollar.checked_add(1)?;
    if bytes.get(dollar) != Some(&b'$') || !matches!(bytes.get(opening), Some(b'(' | b'{')) {
        return None;
    }
    delimited_end_bytes(bytes, opening).ok()
}

enum DelimitedEndError {
    DepthLimit,
    Unclosed,
}

fn delimited_end_bytes(bytes: &[u8], opening: usize) -> Result<usize, DelimitedEndError> {
    let opener = *bytes.get(opening).ok_or(DelimitedEndError::Unclosed)?;
    if !matches!(opener, b'(' | b'{') {
        return Err(DelimitedEndError::Unclosed);
    }
    let mut endings = Vec::with_capacity(MAX_EXPANSION_DEPTH);
    endings.push(if opener == b'{' { b'}' } else { b')' });
    let mut cursor = opening + 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && bytes.get(cursor + 1) == Some(&b'$') {
            cursor += 2;
        } else if bytes[cursor] == b'$' && matches!(bytes.get(cursor + 1), Some(b'(' | b'{')) {
            endings.push(if bytes[cursor + 1] == b'{' {
                b'}'
            } else {
                b')'
            });
            cursor += 2;
        } else if endings.last() == Some(&bytes[cursor]) {
            endings.pop();
            if endings.is_empty() {
                return Ok(cursor);
            }
            cursor += 1;
        } else {
            if matches!(
                (bytes[cursor], endings.last()),
                (b'(', Some(b')')) | (b'{', Some(b'}'))
            ) {
                endings.push(*endings.last().expect("nonempty reference stack"));
            }
            cursor += 1;
        }
        if endings.len() > MAX_EXPANSION_DEPTH {
            return Err(DelimitedEndError::DepthLimit);
        }
    }
    Err(DelimitedEndError::Unclosed)
}

fn parse_word_index(function: &str, value: &str, raw_args: &str) -> Result<usize, MakeExprError> {
    let index = value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|index| *index > 0)
        .ok_or_else(|| MakeExprError::InvalidSyntax {
            expression: bounded_preview(raw_args),
            detail: format!("function `{function}` requires a positive word index, got `{value}`"),
        })?;
    Ok(index)
}

/// Split balanced top-level separators. A fixed maximum partitions only the
/// first `max_fields - 1`; subsequent separators remain data in the final
/// argument. `None` preserves unbounded-arity splitting subject to list limits.
fn split_top_level(
    raw: &str,
    separator: char,
    max_fields: Option<usize>,
) -> Result<Vec<&str>, MakeExprError> {
    let bytes = raw.as_bytes();
    let separator = separator as u8;
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut cursor = 0usize;
    let max_separators = max_fields.map(|fields| fields.saturating_sub(1));
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && matches!(bytes.get(cursor + 1), Some(b'(' | b'{')) {
            cursor = reference_end(raw, cursor)? + 1;
            continue;
        }
        if bytes[cursor] == b'(' {
            cursor = delimited_end(raw, cursor)? + 1;
            continue;
        }
        if bytes[cursor] == separator && max_separators.is_none_or(|maximum| out.len() < maximum) {
            if out.len() >= MAX_LIST_ITEMS.saturating_sub(1) {
                return Err(MakeExprError::ResourceLimit {
                    resource: "list items",
                    limit: MAX_LIST_ITEMS,
                });
            }
            out.push(&raw[start..cursor]);
            start = cursor + 1;
        }
        cursor += 1;
    }
    if out.len() >= MAX_LIST_ITEMS {
        return Err(MakeExprError::ResourceLimit {
            resource: "list items",
            limit: MAX_LIST_ITEMS,
        });
    }
    out.push(&raw[start..]);
    Ok(out)
}

type SubstitutionReference<'a> = (&'a str, Option<(&'a str, &'a str)>);

fn split_substitution_reference(body: &str) -> Result<SubstitutionReference<'_>, MakeExprError> {
    let Some(colon) = top_level_byte(body, b':') else {
        return Ok((body, None));
    };
    let remainder = &body[colon + 1..];
    let Some(equal) = top_level_byte(remainder, b'=') else {
        return Err(MakeExprError::InvalidSyntax {
            expression: bounded_preview(body),
            detail: "substitution reference has `:` but no `=`".to_owned(),
        });
    };
    Ok((
        &body[..colon],
        Some((&remainder[..equal], &remainder[equal + 1..])),
    ))
}

pub(crate) fn top_level_byte(raw: &str, needle: u8) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && matches!(bytes.get(cursor + 1), Some(b'(' | b'{')) {
            cursor = reference_end(raw, cursor).ok()? + 1;
            continue;
        }
        if bytes[cursor] == needle {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn top_level_whitespace(raw: &str) -> Option<usize> {
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'$' && matches!(bytes.get(cursor + 1), Some(b'(' | b'{')) {
            cursor = reference_end(raw, cursor).ok()? + 1;
            continue;
        }
        if bytes[cursor].is_ascii_whitespace() {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn pattern_stem<'a>(pattern: &str, word: &'a str) -> Option<&'a str> {
    let Some(percent) = pattern.find('%') else {
        return (pattern == word).then_some("");
    };
    let prefix = &pattern[..percent];
    let suffix = &pattern[percent + 1..];
    if word.len() < prefix.len() + suffix.len()
        || !word.starts_with(prefix)
        || !word.ends_with(suffix)
    {
        return None;
    }
    Some(&word[prefix.len()..word.len() - suffix.len()])
}

fn notdir(word: &str) -> Option<String> {
    let value = word.rsplit_once('/').map_or(word, |(_, tail)| tail);
    (!value.is_empty()).then(|| value.to_owned())
}

fn directory_part(word: &str) -> String {
    word.rfind('/')
        .map_or_else(|| "./".to_owned(), |slash| word[..=slash].to_owned())
}

fn basename(word: &str) -> Option<String> {
    let component = word.rsplit_once('/').map_or(word, |(_, tail)| tail);
    let value = component.rfind('.').map_or_else(
        || word.to_owned(),
        |dot| word[..word.len() - component.len() + dot].to_owned(),
    );
    (!value.is_empty()).then_some(value)
}

fn suffix(word: &str) -> Option<String> {
    let component = word.rsplit_once('/').map_or(word, |(_, tail)| tail);
    component.rfind('.').map(|dot| component[dot..].to_owned())
}

fn is_deferred_cmake_reference(name: &str) -> bool {
    matches!(
        name,
        "AROS_SOURCE_DIR"
            | "CMAKE_BINARY_DIR"
            | "AROS_BUILD_DIR"
            | "AROS_SYS_DIR"
            | "AROS_BOOT_DIR"
            | "AROS_BOOT_ARCH_DIR"
            | "AROS_PORTS_DIR"
            | "AROS_PORTS_SOURCE_DIR"
            | "AROS_TARGET_CPU"
            | "AROS_TARGET_CPU32"
            | "AROS_TARGET_PLATFORM"
            | "AROS_TARGET_LEGACY_PLATFORM"
            | "AROS_TARGET_FAMILY"
            | "AROS_TARGET_VARIANT"
            | "AROS_TARGET_ICONSET"
            | "AROS_BUILD_DATE_DMY"
            | "AROS_BUILD_DATE_ISO"
    )
}

fn reject_unsupported_references(raw: &str) -> Result<(), MakeExprError> {
    let bytes = raw.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] != b'$' {
            cursor += 1;
            continue;
        }
        match bytes.get(cursor + 1) {
            Some(b'$') => cursor += 2,
            Some(b'{') => {
                let Some(relative_end) = raw[cursor + 2..].find('}') else {
                    return Err(MakeExprError::UnsupportedReference {
                        reference: bounded_preview(&raw[cursor..]),
                    });
                };
                cursor += relative_end + 3;
            }
            Some(_) => {
                let end = cursor
                    + 1
                    + raw[cursor + 1..]
                        .chars()
                        .next()
                        .expect("reference has a next byte")
                        .len_utf8();
                return Err(MakeExprError::UnsupportedReference {
                    reference: raw[cursor..end].to_owned(),
                });
            }
            None => {
                return Err(MakeExprError::UnsupportedReference {
                    reference: "$".to_owned(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "make_expr_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "make_text_pipeline_context_tests.rs"]
mod text_pipeline_context_tests;

#[cfg(test)]
#[path = "make_program_expression_tests.rs"]
mod program_expression_tests;
