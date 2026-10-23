//! Transactional scalar assignment/eval state, not a complete Make file reader.

use super::{
    apply_assignment, collect_vars, literal_variable_name, strip_make_comment, undefine_directive,
    variable_assignment_impl, AssignmentInput, AssignmentKind, ConditionalTruth, VarScope,
    VariableFlavor, CLOSED_TARGET_MAKE_SELECTORS,
};
use crate::{
    dirs::DirVars,
    make_expr::{
        evaluate_make_program_expression, standalone_expansion, MakeExprContext, MakeExprError,
        MakeProgramBudget, MakeVariableQuery,
    },
    parser::{join_continuations, TargetContext},
};
use aros_common::make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) const MAX_VALUE: usize = 4 * 1024 * 1024;
const MAX_STORAGE: usize = 64 * 1024 * 1024;
const MAX_STATE_WORK: usize = 64 * 1024 * 1024;
const MAX_STATEMENTS: usize = 65_536;
const MAX_INITIAL_BINDINGS: usize = 16_384;
const MAX_CAPTURED_SHELL_NAME_BYTES: usize = 8_192;
const MAX_CAPTURED_SHELL_FLAG_FORMS: usize = 8;
const MAX_CAPTURED_SHELL_FLAGS_BYTES: usize = 128;
const MAX_CAPTURED_SHELL_ENVIRONMENT_ENTRIES: usize = 4_096;
const MAX_CAPTURED_SHELL_ENVIRONMENT_BYTES: usize = 128 * 1024;

mod batch;
mod captured;
#[cfg(test)]
mod captured_batch_tests;
#[cfg(test)]
mod captured_direct_batch_tests;
mod captured_export_delta;
pub use captured::{MakeRecipeProjection, MakeTargetRecipeRequest};
mod recipe_shell;
mod serial_recipe_environment;
#[cfg(test)]
mod serial_recipe_environment_tests;
pub use recipe_shell::{MakeRecipePrefixes, MakeRecipeShellProjection};
mod captured_glob;
mod captured_shell;
#[cfg(test)]
mod captured_shell_policy_tests;
mod captured_shell_search;
pub use captured_shell_search::{validate_captured_source_commands, CapturedShellSearch};
mod captured_wildcard;
mod computed_assignment;
mod computed_assignment_syntax;
mod exports;
mod file;
mod initial;
mod native_observation;
mod target_variables;
pub use file::{
    MakeFileEvent, MakeFileEventKind, MakeFileProgram, MakeIncludeResolver,
    MakeResolvedIncludeResolver, MakeResolvedInput,
};
pub use initial::{MakeInitialFlavor, MakeInitialValue};

#[derive(Clone, Copy)]
struct Evaluation<'a> {
    paths: &'a MakeProgramPaths<'a>,
    expressions: &'a MakeProgramBudget,
    work: &'a Cell<usize>,
    file: Option<&'a file::FileContext<'a>>,
    captured_wildcard: Option<&'a crate::make_expr::MakeCapturedWildcard<'a>>,
    captured_realpath: Option<&'a crate::make_expr::MakeCapturedRealpath<'a>>,
    captured_shell: Option<&'a crate::make_expr::MakeCapturedShell<'a>>,
    automatic_variables: Option<&'a std::collections::BTreeMap<String, String>>,
}

#[derive(Clone, Copy, Default)]
struct AssignmentValue<'a> {
    frozen: Option<&'a str>,
    flavor: Option<VariableFlavor>,
    raw: Option<&'a str>,
}

#[cfg(test)]
mod captured_export_tests;
#[cfg(test)]
mod captured_external_input_tests;
#[cfg(test)]
mod captured_realpath_tests;
#[cfg(test)]
mod captured_recipe_tests;
#[cfg(test)]
mod captured_shell_effect_tests;
#[cfg(test)]
mod captured_shell_tests;
#[cfg(test)]
mod captured_tests;
#[cfg(test)]
mod captured_wildcard_tests;
#[cfg(test)]
mod child_command_tests;
#[cfg(test)]
mod computed_assignment_tests;
#[cfg(test)]
mod default_rule_tests;
#[cfg(test)]
mod empty_rule_tests;
#[cfg(test)]
mod file_input_tests;
#[cfg(test)]
mod file_tests;
#[cfg(test)]
mod inherited_export_tests;
#[cfg(test)]
mod initial_observer_tests;
#[cfg(test)]
mod inspection_tests;
#[cfg(test)]
mod path_context_tests;
#[cfg(test)]
mod phony_tests;
#[cfg(test)]
mod shell_effect_environment_tests;
#[cfg(test)]
mod source_prelude_tests;
#[cfg(test)]
mod source_program_tests;
#[cfg(test)]
mod source_projection_tests;
#[cfg(test)]
mod source_reference_absence_tests;
#[cfg(test)]
mod suffix_tests;

/// Explicit path context reused by every first/second expression expansion.
pub struct MakeProgramPaths<'a> {
    pub dirs: &'a DirVars,
    pub source_root: &'a Path,
    pub relative_dir: &'a Path,
    /// Independently selected build root; it is not the process cwd.
    pub top_directory: &'a Path,
    /// Actual GNU Make process cwd, including a per-owner subdirectory.
    pub working_directory: &'a Path,
}

/// An assignment's unique execution ordinal and original source position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MakeVariableEvent {
    pub ordinal: usize,
    pub source_line: usize,
    pub active: Option<bool>,
    pub text: String,
    /// Builtin input-list updates are not statements from the source text.
    pub input_effect: bool,
}

#[derive(Clone)]
struct State {
    initial_namespace: Option<ValidatedMakeNamespace>,
    // Canonical typed invocation context, NOT the retained raw packet digest.
    native_recipe_context_sha256: Option<aros_common::Sha256Digest>,
    scope: VarScope,
    flavors: HashMap<String, VariableFlavor>,
    names: HashSet<String>,
    undefined: HashSet<String>,
    events: Vec<MakeVariableEvent>,
    storage: usize,
    bound_paths: Option<[PathBuf; 4]>,
    exports: std::collections::BTreeMap<String, Option<bool>>,
    inherited_exports: std::collections::BTreeMap<String, Option<bool>>,
    origins: HashMap<String, MakeVariableOrigin>,
    file_read_started: bool,
    captured_shell_policy: Option<Arc<CapturedShellPolicy>>,
    coordinate_projection: bool,
    target_variables: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, target_variables::Binding>,
    >,
    target_raw_values: std::collections::BTreeMap<String, String>,
    pattern_variables: Vec<target_variables::PatternAssignment>,
}

#[derive(Clone)]
struct CapturedShellPolicy {
    shell: String,
    flags: Vec<String>,
    inherited: std::collections::BTreeMap<String, String>,
    inherited_scan_cost: usize,
    source_search: Option<Arc<dyn CapturedShellSearch>>,
}

/// A bounded, transactionally published variable program.
///
/// Only scalar assignments, undefine and standalone expressions are admitted.
/// A file driver must independently select branches and handle rules, includes
/// and definitions. Unknown state is never repaired from ambient environment.
/// Expression budgets and state-work accounting persist after failed operations.
pub struct MakeVariableProgram {
    state: State,
    expressions: MakeProgramBudget,
    state_work: Cell<usize>,
}

impl MakeVariableProgram {
    /// Create state from explicit configured values and proved undefined names.
    ///
    /// # Errors
    /// Refuses inconsistent, oversized or malformed initial bindings.
    pub fn new(target: &TargetContext, undefined_names: &[&str]) -> Result<Self, String> {
        Self::initial(target, undefined_names, None, None, true)
    }

    /// Accept coordinate assertions only when they match independently supplied
    /// execution paths. Expressions retain their existing relocatable path form;
    /// configuration cannot redefine those coordinates.
    ///
    /// # Errors
    /// Refuses conflicting coordinates and the same invalid/bounded inputs as
    /// [`Self::new`]. No filesystem discovery or path inference is performed.
    pub fn new_with_paths(
        target: &TargetContext,
        undefined_names: &[&str],
        paths: &MakeProgramPaths<'_>,
    ) -> Result<Self, String> {
        Self::initial(target, undefined_names, Some(paths), None, true)
    }

    /// Bind a measured initial name inventory without fabricating its values.
    /// The caller must independently authenticate the measurement and paths.
    /// Present names without measured values remain unresolved; only absent
    /// supported scalar names acquire GNU Make's undefined expansion.
    /// Explicit target bindings (including selected target aliases) form the
    /// caller's execution overlay and take precedence over the initial snapshot.
    /// The namespace supplies absence only for otherwise unbound names.
    ///
    /// # Errors
    /// Refuses inconsistent explicit absence assertions and invalid/bounded
    /// inputs, as in [`Self::new_with_paths`].
    pub fn new_with_namespace(
        target: &TargetContext,
        undefined_names: &[&str],
        paths: &MakeProgramPaths<'_>,
        namespace: &ValidatedMakeNamespace,
    ) -> Result<Self, String> {
        Self::initial(target, undefined_names, Some(paths), Some(namespace), true)
    }

    /// Initialize measured values with their actual Make expansion flavor.
    ///
    /// Simple values are already expanded upstream; recursive values retain
    /// their raw expression. These are data bindings, not producer or source
    /// authentication. The caller must independently prove each value's origin,
    /// configuration owner and assignment-time meaning before supplying it.
    /// No environment, directory fallback or source file is read here.
    /// Ordinary environment values have recursive flavor and yield to file
    /// assignments. Command-line values retain their measured flavor and ignore
    /// ordinary assignment/undefine replacement. GNU immediate RHS expansion
    /// still occurs before a stronger-origin binding discards the new value.
    /// Override, environment-override and automatic lifetimes remain unsupported.
    ///
    /// # Errors
    /// Refuses names absent from the namespace, origin mismatches, unsupported
    /// priorities, duplicate/overlapping bindings and exhausted resource bounds.
    pub fn new_with_initial_values(
        target: &TargetContext,
        undefined_names: &[&str],
        paths: &MakeProgramPaths<'_>,
        namespace: &ValidatedMakeNamespace,
        values: &[MakeInitialValue<'_>],
    ) -> Result<Self, String> {
        let mut program =
            Self::initial(target, undefined_names, Some(paths), Some(namespace), true)?;
        initial::install(&mut program.state, namespace, values)?;
        Ok(program)
    }

    /// Replay Make from an explicitly measured initial namespace and values.
    ///
    /// Unlike projection constructors, this does not seed target selectors or
    /// replace TOP/SRCDIR/CURDIR with engine aliases. They obey ordinary source
    /// assignments and measured origin priorities. Execution paths remain bound
    /// independently for path functions and caller-controlled include resolution.
    /// Filesystem enumeration refuses until an independently validated input
    /// inventory can supply it; no ambient wildcard search is admitted here.
    /// These are data semantics, not native producer or complete graph proof.
    ///
    /// # Errors
    /// Refuses invalid initial data, paths, unsupported origins and exhausted
    /// bounds. Present names without measured values stay unresolved.
    pub fn new_source_program(
        paths: &MakeProgramPaths<'_>,
        namespace: &ValidatedMakeNamespace,
        values: &[MakeInitialValue<'_>],
    ) -> Result<Self, String> {
        let mut program = Self::initial(
            &TargetContext::default(),
            &[],
            Some(paths),
            Some(namespace),
            false,
        )?;
        initial::install(&mut program.state, namespace, values)?;
        Ok(program)
    }

    /// Bind complete original environment-name membership before command-line overrides.
    ///
    /// GNU can retain an imported export flag after a command-line override even
    /// for names which would not otherwise be auto-exported. The final namespace
    /// origin loses that history. Supply the independently measured original
    /// environment names, not an ambient lookup or names inferred from final
    /// origins. This reuses the ordinary source interpreter and authenticates
    /// neither the supplied context nor a native child invocation.
    ///
    /// # Errors
    /// Refuses the same invalid source bindings as `new_source_program`, unknown
    /// environment names and missing names still measured as environment-origin.
    pub fn new_source_program_with_environment(
        paths: &MakeProgramPaths<'_>,
        namespace: &ValidatedMakeNamespace,
        values: &[MakeInitialValue<'_>],
        initial_environment_names: &std::collections::BTreeSet<String>,
    ) -> Result<Self, String> {
        let mut program = Self::new_source_program(paths, namespace, values)?;
        exports::bind_initial_environment(
            &mut program.state,
            namespace,
            initial_environment_names,
        )?;
        Ok(program)
    }

    fn initial(
        target: &TargetContext,
        undefined_names: &[&str],
        paths: Option<&MakeProgramPaths<'_>>,
        namespace: Option<&ValidatedMakeNamespace>,
        coordinate_projection: bool,
    ) -> Result<Self, String> {
        let mut storage = 0usize;
        if let Some(namespace) = namespace {
            for entry in namespace.to_observation().entries {
                storage = storage
                    .checked_add(
                        retained_cost(entry.name.len()).map_err(|error| error.to_string())?,
                    )
                    .filter(|value| *value <= MAX_STORAGE)
                    .ok_or("initial Make namespace exceeds storage budget")?;
            }
        }
        if target.make_variables.len() + undefined_names.len() > MAX_INITIAL_BINDINGS {
            return Err("Make variable program has too many initial bindings".into());
        }
        for (name, value) in &target.make_variables {
            if !valid_observed_name(name) || value.len() > MAX_VALUE {
                return Err("invalid initial Make program binding".into());
            }
            if coordinate_projection && bound_coordinate(name) {
                let expected = paths.and_then(|paths| match name.as_str() {
                    "SRCDIR" => paths.source_root.to_str(),
                    "TOP" => paths.top_directory.to_str(),
                    "CURDIR" if paths.relative_dir.as_os_str().is_empty() => Some("."),
                    "CURDIR" => paths.relative_dir.to_str(),
                    _ => None,
                });
                if expected != Some(value.as_str()) {
                    return Err(
                        "configured Make coordinate differs from bound execution path".into(),
                    );
                }
                continue;
            }
            storage = storage
                .checked_add(
                    retained_cost(name.len() + value.len()).map_err(|error| error.to_string())?,
                )
                .filter(|value| *value <= MAX_STORAGE)
                .ok_or("initial Make bindings exceed storage budget")?;
        }
        // Normalize the same closed selectors as the ordinary collector, but
        // bound their synthesis before allocating compound values.
        let mut configuration = std::collections::BTreeMap::new();
        for (name, value) in &target.make_variables {
            if !coordinate_projection
                || (!bound_coordinate(name)
                    && !CLOSED_TARGET_MAKE_SELECTORS.contains(&name.as_str()))
            {
                configuration.insert(name.clone(), value.clone());
            }
        }
        for name in CLOSED_TARGET_MAKE_SELECTORS
            .iter()
            .filter(|_| coordinate_projection)
        {
            if let Some(value) = target.value_bounded(name, MAX_VALUE)? {
                storage = storage
                    .checked_add(
                        retained_cost(name.len() + value.len())
                            .map_err(|error| error.to_string())?,
                    )
                    .filter(|value| *value <= MAX_STORAGE)
                    .ok_or("initial Make selectors exceed storage budget")?;
                configuration.insert((*name).to_owned(), value);
            }
        }
        for name in undefined_names {
            if !valid_observed_name(name)
                || (coordinate_projection && bound_coordinate(name))
                || configuration.contains_key(*name)
                || namespace.is_some_and(|namespace| namespace.contains(name))
            {
                return Err("invalid or conflicting undefined Make program binding".into());
            }
            storage = storage
                .checked_add(retained_cost(name.len()).map_err(|error| error.to_string())?)
                .filter(|value| *value <= MAX_STORAGE)
                .ok_or("undefined Make bindings exceed storage budget")?;
        }
        let bound_paths = if let Some(paths) = paths {
            if !paths.top_directory.is_absolute() || !paths.working_directory.is_absolute() {
                return Err(
                    "Make build root and process cwd must be independently absolute".into(),
                );
            }
            let values = [
                paths.source_root,
                paths.relative_dir,
                paths.top_directory,
                paths.working_directory,
            ];
            for value in values {
                let value = value.to_str().ok_or("Make execution path is not UTF-8")?;
                if value.len() > MAX_VALUE {
                    return Err("Make execution path exceeds byte bound".into());
                }
                storage = storage
                    .checked_add(retained_cost(value.len()).map_err(|error| error.to_string())?)
                    .filter(|value| *value <= MAX_STORAGE)
                    .ok_or("Make execution paths exceed storage budget")?;
            }
            Some(values.map(Path::to_path_buf))
        } else {
            None
        };
        let origins = namespace.map_or_else(HashMap::new, |namespace| {
            namespace
                .to_observation()
                .entries
                .into_iter()
                .filter(|entry| {
                    !configuration.contains_key(&entry.name)
                        && (!coordinate_projection || !bound_coordinate(&entry.name))
                })
                .map(|entry| (entry.name, entry.origin))
                .collect()
        });
        let mut scope = collect_vars("");
        scope.configuration = configuration;
        let inherited_exports = if coordinate_projection {
            std::collections::BTreeMap::new()
        } else {
            exports::initial_selection(
                namespace.ok_or("source export selection needs a measured namespace")?,
            )
        };
        for name in inherited_exports.keys() {
            storage = storage
                .checked_add(retained_cost(name.len()).map_err(|error| error.to_string())?)
                .filter(|size| *size <= MAX_STORAGE)
                .ok_or("initial Make export selection exceeds storage budget")?;
        }
        Ok(Self {
            state: State {
                initial_namespace: namespace.cloned(),
                native_recipe_context_sha256: None,
                scope,
                flavors: HashMap::new(),
                names: HashSet::new(),
                undefined: undefined_names
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect(),
                events: Vec::new(),
                storage,
                bound_paths,
                exports: std::collections::BTreeMap::new(),
                inherited_exports,
                origins,
                file_read_started: false,
                captured_shell_policy: None,
                coordinate_projection,
                target_variables: std::collections::BTreeMap::new(),
                target_raw_values: std::collections::BTreeMap::new(),
                pattern_variables: Vec::new(),
            },
            expressions: MakeProgramBudget::default(),
            state_work: Cell::new(0),
        })
    }

    /// Require every captured source `$(shell ...)` effect to use these exact
    /// Make shell controls and an admitted inherited shell environment.
    ///
    /// Bind this before reading any Source file. This policy is data validation,
    /// not authentication of the shell executable or permission to launch it.
    ///
    /// # Errors
    /// Refuses non-source or unmeasured state, late/repeated binding, malformed
    /// controls, invalid environment data and exhausted resource bounds.
    pub fn require_captured_shell_policy(
        &mut self,
        shell: &str,
        flags: &[&str],
        inherited: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), String> {
        if self.state.coordinate_projection || self.state.initial_namespace.is_none() {
            return Err("captured shell policy needs measured source-mode state".into());
        }
        if self.state.file_read_started || !self.state.events.is_empty() {
            return Err("captured shell policy must be bound before Source effects".into());
        }
        if self.state.captured_shell_policy.is_some() {
            return Err("captured shell policy is already bound".into());
        }
        if shell.is_empty()
            || shell.len() > MAX_CAPTURED_SHELL_NAME_BYTES
            || shell.contains('\0')
            || flags.is_empty()
            || flags.len() > MAX_CAPTURED_SHELL_FLAG_FORMS
            || flags
                .iter()
                .any(|flags| flags.len() > MAX_CAPTURED_SHELL_FLAGS_BYTES || flags.contains('\0'))
        {
            return Err("captured shell policy controls exceed their bounds".into());
        }

        if inherited.len() > MAX_CAPTURED_SHELL_ENVIRONMENT_ENTRIES {
            return Err("captured shell policy environment entry limit".into());
        }
        let mut inherited_bytes = 0usize;
        for (name, value) in inherited {
            if name.contains(['=', '\0']) || value.contains('\0') {
                return Err("captured shell policy environment has an OS delimiter".into());
            }
            inherited_bytes = inherited_bytes
                .checked_add(name.len())
                .and_then(|bytes| bytes.checked_add(value.len()))
                .and_then(|bytes| bytes.checked_add(2))
                .filter(|bytes| *bytes <= MAX_CAPTURED_SHELL_ENVIRONMENT_BYTES)
                .ok_or("captured shell policy environment byte limit")?;
        }

        let mut cost = retained_cost(shell.len()).map_err(|error| error.to_string())?;
        let flag_slots = flags
            .len()
            .checked_mul(std::mem::size_of::<String>())
            .ok_or("captured shell policy storage size overflow")?;
        cost = cost
            .checked_add(retained_cost(flag_slots).map_err(|error| error.to_string())?)
            .ok_or("captured shell policy storage size overflow")?;
        for flag in flags {
            cost = cost
                .checked_add(retained_cost(flag.len()).map_err(|error| error.to_string())?)
                .ok_or("captured shell policy storage size overflow")?;
        }
        let environment_slots = inherited
            .len()
            .checked_mul(std::mem::size_of::<(String, String)>())
            .ok_or("captured shell policy storage size overflow")?;
        cost = cost
            .checked_add(retained_cost(environment_slots).map_err(|error| error.to_string())?)
            .ok_or("captured shell policy storage size overflow")?;
        for (name, value) in inherited {
            let entry_bytes = name
                .len()
                .checked_add(value.len())
                .and_then(|bytes| bytes.checked_add(2))
                .ok_or("captured shell policy storage size overflow")?;
            cost = cost
                .checked_add(retained_cost(entry_bytes).map_err(|error| error.to_string())?)
                .ok_or("captured shell policy storage size overflow")?;
        }
        // Also charge the environment copy's aggregate native representation,
        // including the two OS delimiters per entry used by NativeMakeInvocation.
        cost = cost
            .checked_add(retained_cost(inherited_bytes).map_err(|error| error.to_string())?)
            .ok_or("captured shell policy storage size overflow")?;
        charge_work(&self.state_work, cost).map_err(|error| error.to_string())?;
        let storage = self
            .state
            .storage
            .checked_add(cost)
            .filter(|size| *size <= MAX_STORAGE)
            .ok_or("captured shell policy exceeds Make state storage limit")?;

        let policy = CapturedShellPolicy {
            shell: shell.to_owned(),
            flags: flags.iter().map(|flags| (*flags).to_owned()).collect(),
            inherited: inherited.clone(),
            inherited_scan_cost: inherited_bytes,
            source_search: None,
        };
        self.state.storage = storage;
        self.state.captured_shell_policy = Some(Arc::new(policy));
        Ok(())
    }

    /// Admit Source-prepended search directories only through captured membership
    /// and non-shadowing checks at every modeled shell effect. The inherited
    /// PATH suffix and all other environment controls stay unchanged.
    ///
    /// This preserves the caller-bound inherited command search; it does not
    /// authenticate its tools, infer tool semantics, or authorize execution.
    /// The caller must independently retain and revalidate the inherited tools.
    /// # Errors
    /// Refuses absent policy/PATH, repeated or late binding and shared policies.
    pub fn bind_captured_shell_source_path_prefixes(
        &mut self,
        search: Arc<dyn CapturedShellSearch>,
    ) -> Result<(), String> {
        if self.state.file_read_started || !self.state.events.is_empty() {
            return Err("captured shell search must be bound before Source effects".into());
        }
        let policy = self
            .state
            .captured_shell_policy
            .as_mut()
            .and_then(Arc::get_mut)
            .ok_or("captured shell search needs an exclusive bound policy")?;
        if policy.source_search.is_some()
            || search.path().to_str().is_none_or(|path| {
                path.is_empty()
                    || path.contains(':')
                    || !search.path().is_absolute()
                    || policy.inherited.get("PATH").map(String::as_str) != Some(path)
            })
        {
            return Err(
                "captured shell search needs a fresh policy with exact inherited PATH".into(),
            );
        }
        search.revalidate()?;
        policy.source_search = Some(search);
        Ok(())
    }

    /// Whether captured source shell effects have an immutable shell policy.
    #[must_use]
    pub const fn captured_shell_policy_bound(&self) -> bool {
        self.state.captured_shell_policy.is_some()
    }

    /// Apply proven-active variable statements atomically.
    ///
    /// # Errors
    /// Refuses unsupported syntax, unknown values and exhausted resources.
    pub fn execute(
        &mut self,
        statement: &str,
        source_line: usize,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<(), MakeExprError> {
        self.observe(statement, source_line, Some(true), paths)
    }

    /// Record independently selected variable statements in source order.
    ///
    /// Unknown assignments remain guarded, false ones are not expanded.
    /// This does not evaluate or prove the condition which selected the state.
    ///
    /// # Errors
    /// Refuses unsupported statements, unknown reached effects and limits.
    pub fn observe(
        &mut self,
        statement: &str,
        source_line: usize,
        active: Option<bool>,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<(), MakeExprError> {
        self.check_paths(paths)?;
        let staged = self.stage()?;
        apply_fragment(
            statement,
            source_line,
            active,
            &staged,
            &Evaluation {
                paths,
                expressions: &self.expressions,
                work: &self.state_work,
                file: None,
                captured_wildcard: None,
                captured_realpath: None,
                captured_shell: None,
                automatic_variables: None,
            },
        )?;
        self.state = staged.into_inner();
        Ok(())
    }

    /// Evaluate with staged synchronous effects; publish only on full success.
    ///
    /// # Errors
    /// Returns expression, statement, binding and resource failures atomically.
    pub fn evaluate(
        &mut self,
        expression: &str,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<String, MakeExprError> {
        self.check_paths(paths)?;
        let staged = self.stage()?;
        let result = evaluate(
            expression,
            usize::MAX,
            &staged,
            &Evaluation {
                paths,
                expressions: &self.expressions,
                work: &self.state_work,
                file: None,
                captured_wildcard: None,
                captured_realpath: None,
                captured_shell: None,
                automatic_variables: None,
            },
        )?;
        self.state = staged.into_inner();
        Ok(result)
    }

    /// Interpret a source program and its explicitly supplied include inputs.
    ///
    /// Rules and recipes are retained data, never executed commands. Header
    /// expressions and simple target assignments freeze at their read position.
    /// Input validation/authentication and recipe execution remain caller duties.
    /// The entire read publishes variable state and events only on success.
    ///
    /// # Errors
    /// Refuses unsupported syntax, missing required inputs, unknown reached
    /// values, malformed control structure, cycles and exhausted budgets.
    pub fn read_file(
        &mut self,
        input: &str,
        text: &str,
        paths: &MakeProgramPaths<'_>,
        includes: &MakeIncludeResolver<'_>,
    ) -> Result<MakeFileProgram, MakeExprError> {
        let resolved_includes = |requested: &str, from: &str| {
            includes(requested, from).map(|text| {
                text.map(|text| MakeResolvedInput {
                    identity: requested.into(),
                    text,
                })
            })
        };
        self.read_file_with_resolved(input, text, paths, &resolved_includes)
    }

    /// Interpret a source program with include inputs carrying their native read identity.
    ///
    /// Requested include tokens are passed to the resolver separately from the identity
    /// used for cycle checks, input-list tracking and event source attribution.
    /// The entire read publishes variable state and events only on success.
    ///
    /// # Errors
    /// Refuses unsupported syntax, missing required inputs, unknown reached
    /// values, malformed control structure, cycles and exhausted budgets.
    pub fn read_file_resolved(
        &mut self,
        input: &str,
        text: &str,
        paths: &MakeProgramPaths<'_>,
        includes: &MakeResolvedIncludeResolver<'_>,
    ) -> Result<MakeFileProgram, MakeExprError> {
        self.read_file_with_resolved(input, text, paths, includes)
    }

    fn read_file_with_resolved(
        &mut self,
        input: &str,
        text: &str,
        paths: &MakeProgramPaths<'_>,
        includes: &MakeResolvedIncludeResolver<'_>,
    ) -> Result<MakeFileProgram, MakeExprError> {
        self.check_paths(paths)?;
        let staged = self.stage()?;
        let file = file::FileContext::new(includes);
        let execution = Evaluation {
            paths,
            expressions: &self.expressions,
            work: &self.state_work,
            file: Some(&file),
            captured_wildcard: None,
            captured_realpath: None,
            captured_shell: None,
            automatic_variables: None,
        };
        file.read_input(input, text, &staged, &execution)?;
        let result = file.finish();
        self.state = staged.into_inner();
        Ok(result)
    }

    /// Source positions are distinct from unique assignment execution ordinals.
    #[must_use]
    pub fn events(&self) -> &[MakeVariableEvent] {
        &self.state.events
    }

    /// Expand only explicit source-selected exports at command-invocation time.
    ///
    /// This is data, not execution authority or the complete inherited process
    /// environment. No ambient variables are discovered. Export directives
    /// create proved-undefined names as simple empty file values. Source recursive
    /// values see later assignments; unchanged environment-origin values retain
    /// their literal imported bytes, as GNU recipe exports do.
    ///
    /// # Errors
    /// Refuses unresolved export state, unknown values, expansion side effects,
    /// invalid environment names/data and resource-limit violations atomically.
    pub fn explicit_export_values(
        &mut self,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<std::collections::BTreeMap<String, String>, MakeExprError> {
        self.export_values(paths, false)
    }

    /// Project source-controlled exports, including ordinary initial inheritance.
    ///
    /// Initial environment/command-line origin selects automatic export without
    /// fixing an AROS variable list. Later file replacement retains that status;
    /// source export/unexport and undefine update it. Source values expand at the
    /// current position; untouched environment-origin data stays literal even
    /// when its Make flavor is recursive. This requires source mode and a measured
    /// initial namespace.
    /// Non-identifier command-line names additionally require original environment
    /// membership via `new_source_program_with_environment`; explicit directives
    /// for non-identifier names remain unsupported.
    ///
    /// This is NOT a complete native recipe environment. Make's engine-managed
    /// MAKELEVEL/MAKEFLAGS/MAKEOVERRIDES/SHELL transformations, target-specific
    /// lifetimes, executable resolution and execution/receipt authority still
    /// require independent native binding. In particular, do not blindly pass
    /// this map to a process as if automatic native changes had already occurred.
    /// Downstream child-command adapters enforce their own smaller environment
    /// byte budget; a valid source projection need not fit that execution channel.
    ///
    /// # Errors
    /// Refuses missing context, unmeasured values, uncertain selection, expansion
    /// effects, invalid data and resource-limit violations transactionally.
    pub fn source_export_values(
        &mut self,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<std::collections::BTreeMap<String, String>, MakeExprError> {
        self.export_values(paths, true)
    }

    fn export_values(
        &mut self,
        paths: &MakeProgramPaths<'_>,
        inherit_initial: bool,
    ) -> Result<std::collections::BTreeMap<String, String>, MakeExprError> {
        self.check_paths(paths)?;
        let staged = self.stage()?;
        let result = exports::values(
            &staged,
            &Evaluation {
                paths,
                expressions: &self.expressions,
                work: &self.state_work,
                file: None,
                captured_wildcard: None,
                captured_realpath: None,
                captured_shell: None,
                automatic_variables: None,
            },
            inherit_initial,
        )?;
        self.state = staged.into_inner();
        Ok(result)
    }

    fn stage(&self) -> Result<RefCell<State>, MakeExprError> {
        charge_work(&self.state_work, self.state.storage)?;
        Ok(RefCell::new(self.state.clone()))
    }

    /// Existing bounded estimate for the retained variable program state.
    /// This is crate-private batch accounting, not allocator-size evidence.
    pub(crate) const fn retained_storage_bytes(&self) -> usize {
        self.state.storage
    }

    fn check_paths(&self, paths: &MakeProgramPaths<'_>) -> Result<(), MakeExprError> {
        if self.state.bound_paths.as_ref().is_some_and(|bound| {
            bound
                .iter()
                .zip([
                    paths.source_root,
                    paths.relative_dir,
                    paths.top_directory,
                    paths.working_directory,
                ])
                .any(|(original, current)| original != current)
        }) {
            return Err(invalid("Make execution paths differ from initial binding"));
        }
        Ok(())
    }
}

fn valid_observed_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !b"$(){}=:".contains(&byte))
}

fn bound_coordinate(name: &str) -> bool {
    matches!(name, "SRCDIR" | "TOP" | "CURDIR")
}

fn invalid(detail: &str) -> MakeExprError {
    MakeExprError::InvalidSyntax {
        expression: "Make variable program".into(),
        detail: detail.into(),
    }
}

fn retained_cost(bytes: usize) -> Result<usize, MakeExprError> {
    bytes
        .checked_mul(8)
        .and_then(|value| value.checked_add(1024))
        .ok_or(MakeExprError::ResourceLimit {
            resource: "Make state storage",
            limit: MAX_STORAGE,
        })
}

fn charge_work(work: &Cell<usize>, amount: usize) -> Result<(), MakeExprError> {
    let next = work
        .get()
        .checked_add(amount)
        .filter(|value| *value <= MAX_STATE_WORK)
        .ok_or(MakeExprError::ResourceLimit {
            resource: "Make state work",
            limit: MAX_STATE_WORK,
        })?;
    work.set(next);
    Ok(())
}

fn guard(state: &State, name: &str) -> Option<String> {
    let at = state.events.len();
    if state.scope.conditionally_assigned_before(name, at) {
        return Some("Make variable has unresolved statement state".into());
    }
    if state.initial_namespace.is_some()
        && !reference_absent(state, name)
        && !state.scope.is_defined_before(name, at)
        && !state.scope.undefinitions.contains_key(name)
        && !state.undefined.contains(name)
    {
        return Some("initial Make variable value or absence has not been measured".into());
    }
    state.scope.flavor_uncertainty_reason_at(name, at)
}

fn reference_absent(state: &State, name: &str) -> bool {
    state.initial_namespace.as_ref().is_some_and(|namespace| {
        if state.coordinate_projection {
            namespace.proves_absent(name)
        } else {
            namespace.proves_absent_source_reference(name)
        }
    })
}

fn lookup(state: &State, name: &str) -> Option<String> {
    let at = state.events.len();
    if !state.scope.is_defined_before(name, at)
        && (state.undefined.contains(name)
            || state.scope.undefinitions.contains_key(name)
            || reference_absent(state, name))
    {
        return Some(String::new());
    }
    state.scope.raw_at(name, at)
}

fn inspect(state: &State, query: MakeVariableQuery, name: &str) -> Option<String> {
    let at = state.events.len();
    if state.scope.conditionally_assigned_before(name, at) {
        return None;
    }
    let undefined = !state.scope.is_defined_before(name, at)
        && (state.undefined.contains(name)
            || state.scope.undefinitions.contains_key(name)
            || reference_absent(state, name));
    if undefined {
        return Some(match query {
            MakeVariableQuery::Origin | MakeVariableQuery::Flavor => "undefined".into(),
            MakeVariableQuery::RawValue => String::new(),
        });
    }
    match query {
        MakeVariableQuery::Origin => state
            .origins
            .get(name)
            .map(|origin| origin.as_gnu_label().into()),
        MakeVariableQuery::Flavor => match state.flavors.get(name) {
            Some(VariableFlavor::Simple) => Some("simple".into()),
            Some(VariableFlavor::Recursive) => Some("recursive".into()),
            _ => None,
        },
        MakeVariableQuery::RawValue => {
            if guard(state, name).is_some() {
                return None;
            }
            if let Some(value) = state.target_raw_values.get(name) {
                return Some(value.clone());
            }
            let value = lookup(state, name)?;
            match state.flavors.get(name) {
                Some(VariableFlavor::Simple) => Some(value.replace("$$", "$")),
                Some(VariableFlavor::Recursive) => Some(value),
                _ => None,
            }
        }
    }
}

fn evaluate(
    raw: &str,
    source_line: usize,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<String, MakeExprError> {
    let lookup_value = |name: &str| lookup(&data.borrow(), name);
    let inspection = |query, name: &str| inspect(&data.borrow(), query, name);
    let check = |name: &str| guard(&data.borrow(), name);
    evaluate_with_queries(
        raw,
        source_line,
        data,
        execution,
        &lookup_value,
        &inspection,
        &check,
    )
}

fn evaluate_with_queries(
    raw: &str,
    source_line: usize,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
    lookup_value: &dyn Fn(&str) -> Option<String>,
    inspection: &dyn Fn(MakeVariableQuery, &str) -> Option<String>,
    check: &dyn Fn(&str) -> Option<String>,
) -> Result<String, MakeExprError> {
    let paths = execution.paths;
    let empty = collect_vars("");
    let mut context =
        MakeExprContext::new(&empty, paths.dirs, 0, paths.source_root, paths.relative_dir)
            .with_lookup(lookup_value)
            .with_variable_inspection(inspection)
            .with_guard(check)
            .with_working_directory(paths.working_directory)
            .with_build_directory(paths.top_directory)
            .with_pure_text_pipeline();
    if !data.borrow().coordinate_projection {
        context = context.without_source_projection().without_filesystem();
    }
    if let Some(lookup) = execution.captured_wildcard {
        context = context.with_captured_wildcard(lookup);
    }
    if let Some(lookup) = execution.captured_realpath {
        context = context.with_captured_realpath(lookup);
    }
    if let Some(shell) = execution.captured_shell {
        context = context.with_captured_shell(shell);
    }
    if let Some(values) = execution.automatic_variables {
        context = context.with_automatic_variables(values);
    }
    let apply = |fragment: &str| apply_fragment(fragment, source_line, Some(true), data, execution);
    evaluate_make_program_expression(raw, &context, execution.expressions, &apply)
}

fn apply_fragment(
    fragment: &str,
    source_line: usize,
    active: Option<bool>,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<(), MakeExprError> {
    if let Some(file) = execution.file {
        return file.read_eval(fragment, source_line, active, data, execution);
    }
    apply_scalar_fragment(fragment, source_line, active, data, execution)
}

fn apply_scalar_fragment(
    fragment: &str,
    source_line: usize,
    active: Option<bool>,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<(), MakeExprError> {
    if fragment.len() > MAX_VALUE {
        return Err(invalid("Make statement exceeds its byte limit"));
    }
    charge_work(execution.work, fragment.len())?;
    let joined = join_continuations(fragment);
    for raw in joined.lines() {
        if raw.starts_with('\t') {
            return Err(invalid("variable program does not interpret recipes"));
        }
        let visible = strip_make_comment(raw).trim_start();
        if visible.is_empty() {
            continue;
        }
        let computed_line;
        let computed = if variable_assignment_impl(visible, true).is_some() {
            None
        } else {
            computed_assignment_syntax::split(visible).map_err(|error| invalid(&error))?
        };
        let visible = if let Some((lhs, operator, rhs)) = computed {
            if data.borrow().coordinate_projection {
                return Err(invalid(
                    "computed Make assignments require complete source interpretation",
                ));
            }
            if active == Some(false) {
                continue;
            }
            if active != Some(true) {
                return Err(invalid(
                    "computed Make assignment needs proved branch selection",
                ));
            }
            computed_line =
                computed_assignment::resolve(lhs, operator, rhs, source_line, data, execution)?;
            computed_line.as_str()
        } else {
            visible
        };
        if exports::apply(visible, source_line, active, data, execution)? {
            continue;
        }
        if let Some((name, rhs, kind)) = variable_assignment_impl(visible, true) {
            if data.borrow().coordinate_projection
                && bound_coordinate(name)
                && active != Some(false)
            {
                return Err(invalid("cannot replace a bound Make execution coordinate"));
            }
            check_origin_write(&data.borrow(), name, active)?;
            if data.borrow().origins.get(name) == Some(&MakeVariableOrigin::CommandLine) {
                if kind == AssignmentKind::Append
                    && !data.borrow().flavors.contains_key(name)
                    && rhs.contains('$')
                    && active != Some(false)
                {
                    return Err(invalid("ignored append needs measured command-line flavor"));
                }
                let immediate = kind == AssignmentKind::SimpleSet
                    || (kind == AssignmentKind::Append
                        && data.borrow().flavors.get(name) == Some(&VariableFlavor::Simple));
                if immediate && active != Some(false) {
                    if active.is_none() && rhs.contains('$') {
                        return Err(invalid(
                            "ignored immediate assignment needs proved branch selection",
                        ));
                    }
                    if active == Some(true) {
                        evaluate(rhs, source_line, data, execution)?;
                    }
                }
                record_ignored(visible, source_line, active, data)?;
                continue;
            }
            let (freeze, flavor) = {
                let state = data.borrow();
                if active == Some(true)
                    && matches!(kind, AssignmentKind::Append | AssignmentKind::SetIfUnset)
                {
                    if let Some(detail) = guard(&state, name) {
                        return Err(MakeExprError::UnsafeVariable {
                            name: name.into(),
                            detail,
                            expansion_chain: Vec::new(),
                        });
                    }
                    if !state.scope.is_defined_before(name, state.events.len())
                        && !state.undefined.contains(name)
                        && !state.scope.undefinitions.contains_key(name)
                        && !state
                            .initial_namespace
                            .as_ref()
                            .is_some_and(|namespace| namespace.proves_absent(name))
                    {
                        return Err(invalid(
                            "assignment requires proved Make variable definedness",
                        ));
                    }
                }
                if kind == AssignmentKind::SetIfUnset
                    && state.scope.is_defined_before(name, state.events.len())
                {
                    (false, VariableFlavor::Recursive)
                } else if kind == AssignmentKind::Append {
                    match state.flavors.get(name).copied().unwrap_or_else(|| {
                        if state.scope.configuration.contains_key(name) {
                            VariableFlavor::Unknown
                        } else {
                            VariableFlavor::Recursive
                        }
                    }) {
                        VariableFlavor::Simple => (true, VariableFlavor::Simple),
                        VariableFlavor::Unknown if rhs.contains('$') && active == Some(true) => {
                            return Err(invalid("append requires a known Make variable flavor"));
                        }
                        flavor @ (VariableFlavor::Recursive | VariableFlavor::Unknown) => {
                            (false, flavor)
                        }
                    }
                } else {
                    (
                        kind == AssignmentKind::SimpleSet,
                        if kind == AssignmentKind::SimpleSet {
                            VariableFlavor::Simple
                        } else {
                            VariableFlavor::Recursive
                        },
                    )
                }
            };
            let frozen = if freeze && active == Some(true) {
                Some(evaluate(rhs, source_line, data, execution)?.replace('$', "$$"))
            } else {
                None
            };
            record_assignment(
                visible,
                name,
                AssignmentValue {
                    frozen: frozen.as_deref(),
                    flavor: Some(flavor),
                    raw: None,
                },
                source_line,
                active,
                data,
            )?;
        } else if let Some(name) =
            undefine_directive(visible).map_err(|()| invalid("unsupported undefine statement"))?
        {
            if data.borrow().coordinate_projection
                && bound_coordinate(name)
                && active != Some(false)
            {
                return Err(invalid("cannot undefine a bound Make execution coordinate"));
            }
            record_assignment(
                visible,
                name,
                AssignmentValue::default(),
                source_line,
                active,
                data,
            )?;
        } else if standalone_expansion(visible)? {
            if active == Some(false) {
                continue;
            }
            if active != Some(true) {
                return Err(invalid(
                    "Make expression statement has unresolved branch selection",
                ));
            }
            if !evaluate(visible, source_line, data, execution)?
                .trim()
                .is_empty()
            {
                return Err(invalid(
                    "variable statement emitted nonempty top-level text",
                ));
            }
        } else {
            return Err(invalid(
                "variable program requires separate rule/include/definition interpretation",
            ));
        }
    }
    Ok(())
}

fn record_assignment(
    line: &str,
    name: &str,
    value: AssignmentValue<'_>,
    source_line: usize,
    active: Option<bool>,
    data: &RefCell<State>,
) -> Result<(), MakeExprError> {
    let AssignmentValue {
        frozen,
        flavor,
        raw,
    } = value;
    if !literal_variable_name(name) {
        return Err(invalid("unsupported variable assignment name"));
    }
    check_origin_write(&data.borrow(), name, active)?;
    if data.borrow().origins.get(name) == Some(&MakeVariableOrigin::CommandLine) {
        return record_ignored(line, source_line, active, data);
    }
    let mut state = data.borrow_mut();
    let position = state.events.len();
    if position == MAX_STATEMENTS {
        return Err(MakeExprError::ResourceLimit {
            resource: "Make variable statements",
            limit: MAX_STATEMENTS,
        });
    }
    let old = state
        .scope
        .raw_at(name, position)
        .map_or(0, |value| value.len())
        .max(
            state
                .scope
                .path_raw_at(name, position)
                .map_or(0, |value| value.len()),
        );
    let append = variable_assignment_impl(line, true)
        .is_some_and(|(_, _, kind)| kind == AssignmentKind::Append);
    let keep_origin = variable_assignment_impl(line, true).is_some_and(|(_, _, kind)| {
        kind == AssignmentKind::SetIfUnset && state.scope.is_defined_before(name, position)
    });
    let append_base = if append && flavor == Some(VariableFlavor::Simple) && active == Some(true) {
        let current = state.scope.latest_raw(name).unwrap_or("");
        if state.flavors.get(name) == Some(&VariableFlavor::Simple) {
            Some(current.to_owned())
        } else {
            if current.len().saturating_add(current.matches('$').count()) > MAX_VALUE {
                return Err(invalid("Make append base exceeds its byte limit"));
            }
            Some(current.replace('$', "$$"))
        }
    } else {
        None
    };
    let value_bytes = frozen.or(raw).map_or(line.len(), str::len);
    let retained_value_bytes = if append {
        append_base
            .as_ref()
            .map_or(old, String::len)
            .saturating_add(value_bytes)
            .saturating_add(1)
    } else {
        value_bytes
    };
    if retained_value_bytes > MAX_VALUE {
        return Err(invalid("Make assignment exceeds its retained value bound"));
    }
    let cost = retained_cost(retained_value_bytes + line.len() + name.len())?;
    let next = state
        .storage
        .checked_add(cost)
        .filter(|value| *value <= MAX_STORAGE)
        .ok_or(MakeExprError::ResourceLimit {
            resource: "Make state storage",
            limit: MAX_STORAGE,
        })?;
    let branch_state = match active {
        Some(true) => ConditionalTruth::True,
        Some(false) => ConditionalTruth::False,
        None => ConditionalTruth::Unknown,
    };
    state.names.insert(name.into());
    let project = |_: &str, _: &VarScope, _: usize| {
        frozen
            .expect("only frozen assignments use this projection")
            .to_owned()
    };
    let State {
        scope,
        flavors,
        names,
        ..
    } = &mut *state;
    apply_assignment(
        scope,
        flavors,
        AssignmentInput {
            prove_default_noop: true,
            line,
            line_no: position,
            branch_state,
            selected: active == Some(true),
            path_names: names,
            preserve_whitespace: true,
            flavor,
            append_base: append_base.as_deref(),
            raw_value: raw,
        },
        frozen.map(|_| &project as &super::ImmediateProjection<'_>),
    );
    if active == Some(true) && !keep_origin {
        if scope.is_defined_before(name, position + 1) {
            state.origins.insert(name.into(), MakeVariableOrigin::File);
        } else {
            state.origins.remove(name);
            state.exports.remove(name);
            state.inherited_exports.remove(name);
        }
    }
    if active.is_none()
        && super::undefine_directive(line)
            .map_err(|()| invalid("unsupported undefine statement"))?
            .is_some()
    {
        if let Some(exported) = state.exports.get_mut(name) {
            *exported = None;
        }
        if let Some(exported) = state.inherited_exports.get_mut(name) {
            *exported = None;
        }
    }
    state.storage = next;
    state.events.push(MakeVariableEvent {
        ordinal: position,
        source_line,
        active,
        text: line.into(),
        input_effect: false,
    });
    Ok(())
}

fn record_ignored(
    line: &str,
    source_line: usize,
    active: Option<bool>,
    data: &RefCell<State>,
) -> Result<(), MakeExprError> {
    let mut state = data.borrow_mut();
    if state.events.len() >= MAX_STATEMENTS {
        return Err(invalid("Make variable statement count exceeds limit"));
    }
    state.storage = state
        .storage
        .checked_add(retained_cost(line.len())?)
        .filter(|size| *size <= MAX_STORAGE)
        .ok_or_else(|| invalid("Make ignored statements exceed storage budget"))?;
    let ordinal = state.events.len();
    state.events.push(MakeVariableEvent {
        ordinal,
        source_line,
        active,
        text: line.into(),
        input_effect: false,
    });
    Ok(())
}

fn check_origin_write(
    state: &State,
    name: &str,
    active: Option<bool>,
) -> Result<(), MakeExprError> {
    if active != Some(false)
        && matches!(
            state.origins.get(name),
            Some(
                MakeVariableOrigin::EnvironmentOverride
                    | MakeVariableOrigin::Override
                    | MakeVariableOrigin::Automatic
            )
        )
    {
        return Err(invalid(
            "source write has unsupported initial origin semantics",
        ));
    }
    Ok(())
}
