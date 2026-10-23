//! Atomic interpretation of independently selected, descriptor-bound inputs.

use super::{
    file::FileContext, Evaluation, MakeFileProgram, MakeProgramPaths, MakeVariableProgram, State,
};
use aros_common::make_input_snapshot::{
    MakeInputReader, MakeInputSnapshot, VerifiedMakeInputReads,
};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;

fn charge_recipe_storage(
    storage: &mut usize,
    bytes: usize,
    work: &Cell<usize>,
) -> Result<(), String> {
    let cost = super::retained_cost(bytes).map_err(|error| error.to_string())?;
    *storage = storage
        .checked_add(cost)
        .filter(|size| *size <= super::MAX_STORAGE)
        .ok_or("Make target recipe batch exceeds storage limit")?;
    super::charge_work(work, cost).map_err(|error| error.to_string())
}

/// Data evaluated under one explicitly supplied recipe automatic-variable frame.
///
/// This neither selects a recipe nor proves its target-specific/inherited scope,
/// native child environment, executable identity or execution receipt. Recipe
/// control prefixes remain inert text, not permission to execute the result.
#[derive(Clone, Eq, PartialEq)]
pub struct MakeRecipeProjection {
    expression: String,
    source_exports: BTreeMap<String, String>,
    serial_environment: Result<super::serial_recipe_environment::SerialRecipeEnvironment, String>,
}

impl std::fmt::Debug for MakeRecipeProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MakeRecipeProjection")
            .field("expression_bytes", &self.expression.len())
            .field("source_export_count", &self.source_exports.len())
            .finish_non_exhaustive()
    }
}

impl MakeRecipeProjection {
    /// Expanded inert text, including any unprocessed recipe control prefixes.
    #[must_use]
    pub fn expression(&self) -> &str {
        &self.expression
    }

    /// Source-controlled exports, not the complete native child environment.
    #[must_use]
    pub const fn source_exports(&self) -> &BTreeMap<String, String> {
        &self.source_exports
    }

    /// Derive the child environment for the independently observed serial GNU
    /// context, retaining Source exports and engine-owned level/flag behavior.
    ///
    /// This is data consistency, not execution authority. The caller must bind
    /// this projection to `original`, its observed GNU executable and admitted
    /// Source program. It does not admit parallel/jobserver state, administrative
    /// rule controls, arbitrary Source engine changes or a new shell scope.
    /// # Errors
    /// Refuses unmeasured engine values, changed engine controls, unsupported
    /// startup, dry-run execution and unresolved export controls.
    pub fn serial_native_environment(
        &self,
        original: &aros_common::make_native_invocation::NativeMakeInvocation,
    ) -> Result<BTreeMap<String, String>, String> {
        self.serial_environment
            .as_ref()
            .map_err(Clone::clone)?
            .apply(&self.source_exports, original)
    }
}

fn serial_environment(
    state: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<Result<super::serial_recipe_environment::SerialRecipeEnvironment, String>, String> {
    let before = state.borrow().events.len();
    let selected =
        super::serial_recipe_environment::SerialRecipeEnvironment::capture(state, execution);
    if state.borrow().events.len() != before {
        return Err("serial recipe engine projection attempted a state-changing effect".into());
    }
    let bytes = selected.as_ref().map_or_else(
        String::len,
        super::serial_recipe_environment::SerialRecipeEnvironment::retained_bytes,
    );
    charge_recipe_storage(&mut 0, bytes, execution.work)?;
    Ok(selected)
}

/// One independently selected target frame and deferred Source expression.
/// No language, compiler role or option formula is inferred by this request.
#[derive(Clone, Copy)]
pub struct MakeTargetRecipeRequest<'a> {
    pub chain: &'a [&'a str],
    pub expression: &'a str,
    pub automatic_variables: &'a BTreeMap<String, String>,
}

fn validate_target_recipe_requests(
    requests: &[MakeTargetRecipeRequest<'_>],
    work: &Cell<usize>,
) -> Result<(), String> {
    let mut request_bytes = 0usize;
    for request in requests {
        if request.chain.is_empty()
            || request.chain.last().copied()
                != request.automatic_variables.get("@").map(String::as_str)
        {
            return Err("Make target scope differs from recipe automatic target".into());
        }
        for value in std::iter::once(request.expression)
            .chain(request.chain.iter().copied())
            .chain(
                request
                    .automatic_variables
                    .iter()
                    .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
            )
        {
            charge_recipe_storage(&mut request_bytes, value.len(), work)?;
        }
    }
    Ok(())
}

fn check_target_recipe_request_count(
    requests: &[MakeTargetRecipeRequest<'_>],
) -> Result<(), String> {
    if requests.len() > super::MAX_STATEMENTS {
        Err("Make target recipe batch exceeds count limit".into())
    } else {
        Ok(())
    }
}

fn evaluate_target_recipe_requests(
    requests: &[MakeTargetRecipeRequest<'_>],
    state: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<Vec<MakeRecipeProjection>, String> {
    let mut results = Vec::with_capacity(requests.len());
    let mut result_bytes = 0usize;
    for request in requests {
        let frame = Evaluation {
            automatic_variables: Some(request.automatic_variables),
            ..*execution
        };
        let scoped = super::target_variables::overlay(state, request.chain, &frame)
            .map_err(|error| error.to_string())?;
        let before = scoped.borrow().events.len();
        let global_before = state.borrow().events.len();
        let expression = super::evaluate(request.expression, usize::MAX, &scoped, &frame)
            .map_err(|error| error.to_string())?;
        let source_exports =
            super::exports::values(&scoped, &frame, true).map_err(|error| error.to_string())?;
        if scoped.borrow().events.len() != before || state.borrow().events.len() != global_before {
            return Err("Make target recipe has unsupported state-changing effects".into());
        }
        for value in std::iter::once(expression.as_str()).chain(
            source_exports
                .iter()
                .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
        ) {
            charge_recipe_storage(&mut result_bytes, value.len(), execution.work)?;
        }
        results.push(MakeRecipeProjection {
            expression,
            source_exports,
            serial_environment: serial_environment(&scoped, &frame)?,
        });
    }
    Ok(results)
}

fn validate_direct_recipe_request(
    request: MakeTargetRecipeRequest<'_>,
    values: &BTreeMap<String, String>,
    work: &Cell<usize>,
    request_bytes: &mut usize,
) -> Result<(), String> {
    if request.chain.len() != 1
        || request.chain.first().copied()
            != request.automatic_variables.get("@").map(String::as_str)
    {
        return Err("literal private overrides require their exact direct automatic target".into());
    }
    for value in std::iter::once(request.expression)
        .chain(request.chain.iter().copied())
        .chain(
            values
                .iter()
                .chain(request.automatic_variables)
                .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
        )
    {
        charge_recipe_storage(request_bytes, value.len(), work)?;
    }
    Ok(())
}

fn evaluate_direct_recipe_request(
    request: MakeTargetRecipeRequest<'_>,
    values: &BTreeMap<String, String>,
    state: &RefCell<State>,
    execution: &Evaluation<'_>,
    result_bytes: &mut usize,
) -> Result<MakeRecipeProjection, String> {
    let frame = Evaluation {
        automatic_variables: Some(request.automatic_variables),
        ..*execution
    };
    let scoped =
        super::target_variables::direct_literal_overlay(state, request.chain, values, &frame)
            .map_err(|error| error.to_string())?;
    let before = scoped.borrow().events.len();
    let global_before = state.borrow().events.len();
    let expression = super::evaluate(request.expression, usize::MAX, &scoped, &frame)
        .map_err(|error| error.to_string())?;
    let source_exports =
        super::exports::values(&scoped, &frame, true).map_err(|error| error.to_string())?;
    if scoped.borrow().events.len() != before || state.borrow().events.len() != global_before {
        return Err(
            "literal private recipe projection has unsupported state-changing effects".into(),
        );
    }
    for value in std::iter::once(expression.as_str()).chain(
        source_exports
            .iter()
            .flat_map(|(key, value)| [key.as_str(), value.as_str()]),
    ) {
        charge_recipe_storage(result_bytes, value.len(), execution.work)?;
    }
    Ok(MakeRecipeProjection {
        expression,
        source_exports,
        serial_environment: serial_environment(&scoped, &frame)?,
    })
}

impl MakeVariableProgram {
    /// Record only the read-time input-list effect of a known generated file.
    ///
    /// The exact bytes are digest-bound in the captured transaction, but are
    /// deliberately NOT parsed. The caller must independently validate the
    /// generator and model every other effect (for example private overrides).
    /// This is not a substitute for interpreting arbitrary Source inputs and
    /// grants no native selection, complete-file interpretation or launch proof.
    /// Input spelling, current list flavor and CLI priority follow the same
    /// generic read-time transition as parsed files. State publishes atomically.
    /// Failed attempts still consume the shared monotone work budget.
    ///
    /// # Errors
    /// Refuses missing/changed/unbound files or digest, wrong cwd, unsupported
    /// input-list semantics and shared state/input/work bounds.
    pub fn record_captured_input_list_entry(
        &mut self,
        input: &str,
        expected_sha256: &aros_common::Sha256Digest,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<VerifiedMakeInputReads, String> {
        self.check_paths(paths).map_err(|error| error.to_string())?;
        if snapshot.working_directory() != paths.working_directory {
            return Err("Make captured input cwd differs from program context".into());
        }
        let staged = self.stage().map_err(|error| error.to_string())?;
        let ((), reads) = snapshot.transaction(|reader| {
            let bytes = reader
                .read_bytes(input)?
                .ok_or("known generated Make input is absent")?;
            if aros_common::sha256_bytes(&bytes.bytes) != *expected_sha256 {
                return Err("known generated Make input digest differs".into());
            }
            let execution = Evaluation {
                paths,
                expressions: &self.expressions,
                work: &self.state_work,
                file: None,
                captured_wildcard: None,
                captured_realpath: None,
                captured_shell: None,
                automatic_variables: None,
            };
            super::file::record_input_list_entry(input, &staged, &execution)
                .map_err(|error| error.to_string())
        })?;
        self.state = staged.into_inner();
        Ok(reads)
    }

    /// Project one direct recipe with caller-selected literal simple overrides.
    ///
    /// Equivalent data semantics to target-private override `:=` bindings in
    /// this one direct frame: CLI values yield, RHS dollars stay literal,
    /// original export flags remain, and no descendant/global state is changed.
    /// This does NOT parse arbitrary assignment modifiers or prove that native
    /// Make selected a sole prerequisite-free goal. The caller binds that rule,
    /// override bytes and frame independently before using this projection.
    /// No command executes and no launch authority is granted.
    ///
    /// # Errors
    /// Refuses inherited frames, invalid names, unsupported effects, input drift
    /// and shared work/storage limits; no partial projection is returned.
    pub fn evaluate_captured_direct_recipe_with_overrides(
        &mut self,
        request: MakeTargetRecipeRequest<'_>,
        values: &BTreeMap<String, String>,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(MakeRecipeProjection, VerifiedMakeInputReads), String> {
        validate_direct_recipe_request(request, values, &self.state_work, &mut 0)?;
        self.captured_evaluation(paths, snapshot, None, |state, execution| {
            evaluate_direct_recipe_request(request, values, state, execution, &mut 0)
        })
    }

    /// Compose scalar values and direct recipes in one staged input transaction.
    ///
    /// No private overrides are introduced. The existing direct-target overlay
    /// and automatic-variable checks apply to each recipe. Per-section reads
    /// remain separate; the data-only consumer can validate the complete batch
    /// before variable state is committed. There is no execution permission.
    /// # Errors
    /// Refuses the scalar/direct evaluators' limits and input/path/effect errors,
    /// consumer failure and final snapshot/search drift without publishing state.
    pub(crate) fn with_captured_direct_recipes<T>(
        &mut self,
        expressions: &[&str],
        requests: &[MakeTargetRecipeRequest<'_>],
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
        consume: impl FnOnce(
            (Vec<String>, VerifiedMakeInputReads),
            Vec<(MakeRecipeProjection, VerifiedMakeInputReads)>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        super::batch::check_count(expressions).map_err(|error| error.to_string())?;
        check_target_recipe_request_count(requests)?;
        if expressions.len().saturating_add(requests.len()) > super::MAX_STATEMENTS {
            return Err("Make direct recipe batch exceeds aggregate count limit".into());
        }
        let overrides = BTreeMap::new();
        let mut request_bytes = 0usize;
        for request in requests {
            validate_direct_recipe_request(
                *request,
                &overrides,
                &self.state_work,
                &mut request_bytes,
            )?;
        }
        self.captured_evaluation_with_reads(paths, snapshot, None, |state, execution, reader| {
            let start = reader.borrow().read_checkpoint();
            let values = super::batch::evaluate_staged(expressions, state, execution)
                .map_err(|error| error.to_string())?;
            let reads = reader.borrow().provisional_reads_since(start)?;
            let mut recipes = Vec::with_capacity(requests.len());
            // Scalar result storage was already charged by evaluate_staged;
            // include it in this batch's retained cap without charging twice.
            let mut result_bytes = values.iter().try_fold(0usize, |bytes, value| {
                let cost = super::retained_cost(value.len()).map_err(|error| error.to_string())?;
                bytes
                    .checked_add(cost)
                    .filter(|size| *size <= super::MAX_STORAGE)
                    .ok_or("Make direct recipe batch exceeds storage limit")
                    .map_err(str::to_owned)
            })?;
            for request in requests {
                let start = reader.borrow().read_checkpoint();
                let recipe = evaluate_direct_recipe_request(
                    *request,
                    &overrides,
                    state,
                    execution,
                    &mut result_bytes,
                )?;
                recipes.push((recipe, reader.borrow().provisional_reads_since(start)?));
            }
            consume((values, reads), recipes)
        })
        .map(|(value, _)| value)
    }

    /// Evaluate ordered expressions against one revalidated input snapshot.
    ///
    /// All values share one staged state, captured filesystem and work budget.
    /// State and the complete result batch publish only after every expression
    /// and both whole-root checks succeed; no partial language/ABI view escapes.
    /// This is data evaluation, not execution or configuration authentication.
    ///
    /// # Errors
    /// Refuses count/storage/work limits, cwd mismatch, unsafe or changed roots
    /// and all scalar captured-evaluation errors, without committing state.
    pub fn evaluate_many_captured(
        &mut self,
        expressions: &[&str],
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(Vec<String>, VerifiedMakeInputReads), String> {
        super::batch::check_count(expressions).map_err(|error| error.to_string())?;
        self.captured_evaluation(paths, snapshot, None, |state, execution| {
            super::batch::evaluate_staged(expressions, state, execution)
                .map_err(|error| error.to_string())
        })
    }

    /// Evaluate a later expression against the same explicit captured roots.
    /// Recursive values query the selected inventory at evaluation time; they
    /// are not frozen during file parsing or allowed ambient filesystem access.
    ///
    /// # Errors
    /// Refuses changed/unsafe input roots, mismatched cwd and expression errors.
    pub fn evaluate_captured(
        &mut self,
        expression: &str,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(String, VerifiedMakeInputReads), String> {
        self.captured_evaluation(paths, snapshot, None, |state, execution| {
            super::evaluate(expression, usize::MAX, state, execution)
                .map_err(|error| error.to_string())
        })
    }

    /// Project source-controlled exports using the explicit captured roots.
    ///
    /// Recursive wildcard and realpath expressions share the same bounded,
    /// descriptor-bound reader as other captured evaluation. The result and
    /// staged variable state publish only after complete tree revalidation.
    /// This has the source-only scope of `Self::source_export_values`: it does
    /// not supply native Make's engine-managed environment transformations,
    /// target-specific inheritance, executable binding or execution authority.
    ///
    /// # Errors
    /// Refuses invalid export state, unmeasured values, unsafe/changing roots,
    /// mismatched cwd, expansion effects and exhausted budgets atomically.
    pub fn source_export_values_captured(
        &mut self,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(BTreeMap<String, String>, VerifiedMakeInputReads), String> {
        self.captured_evaluation(paths, snapshot, None, |state, execution| {
            super::exports::values(state, execution, true).map_err(|error| error.to_string())
        })
    }

    /// Evaluate recipe text and source exports in one captured transaction.
    ///
    /// The caller independently supplies the known automatic-variable frame;
    /// unavailable timestamp/archive/stem values must not be guessed. Recursive
    /// source definitions and exports see the same transient frame and captured
    /// filesystem. It is never installed as global file/environment bindings.
    /// This does not choose a rule, perform target-variable inheritance or apply
    /// native Make's engine-owned child environment transformations. Expressions
    /// with state-changing effects refuse rather than assume recipe/env timing.
    ///
    /// # Errors
    /// Refuses invalid/missing automatic bindings, expression/export effects,
    /// unsafe/changing roots, cwd mismatch and exhausted interpreter budgets.
    pub fn evaluate_captured_recipe(
        &mut self,
        expression: &str,
        automatic_variables: &BTreeMap<String, String>,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(MakeRecipeProjection, VerifiedMakeInputReads), String> {
        self.captured_evaluation(
            paths,
            snapshot,
            Some(automatic_variables),
            |state, execution| {
                let before = state.borrow().events.len();
                let expression = super::evaluate(expression, usize::MAX, state, execution)
                    .map_err(|error| error.to_string())?;
                if state.borrow().events.len() != before {
                    return Err(
                        "Make recipe projection has unsupported state-changing effects".into(),
                    );
                }
                let source_exports = super::exports::values(state, execution, true)
                    .map_err(|error| error.to_string())?;
                Ok(MakeRecipeProjection {
                    expression,
                    source_exports,
                    serial_environment: serial_environment(state, execution)?,
                })
            },
        )
    }

    /// Project a recipe and exports in the selected inherited target frame.
    ///
    /// `chain` is the independently established root-to-leaf build traversal;
    /// its leaf must equal automatic `$@`. The caller still proves the actual
    /// rule, first parent of shared prerequisites, automatic values and phase.
    /// Source filesystem queries use the same descriptor-bound transaction.
    /// No target overlay or expression effects are published into global state.
    /// Variable patterns follow the supplied target spelling; this does not
    /// authenticate implicit rule selection. Assignment modifiers still refuse.
    ///
    /// # Errors
    /// Refuses mismatched target/automatic frames, unsupported scope/effects,
    /// unsafe/changed inputs and exhausted interpreter or snapshot limits.
    pub fn evaluate_captured_target_recipe(
        &mut self,
        chain: &[&str],
        expression: &str,
        automatic_variables: &BTreeMap<String, String>,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(MakeRecipeProjection, VerifiedMakeInputReads), String> {
        let (mut results, reads) = self.evaluate_many_captured_target_recipes(
            &[MakeTargetRecipeRequest {
                chain,
                expression,
                automatic_variables,
            }],
            paths,
            snapshot,
        )?;
        Ok((
            results.pop().ok_or("Make target recipe result is absent")?,
            reads,
        ))
    }

    /// Evaluate several target recipes/exports in one captured transaction.
    ///
    /// All requests use the same staged Source program and descriptor-bound
    /// filesystem. Each target overlay/automatic frame remains isolated; no
    /// partial result or state is returned if any request or final root check
    /// fails. Shared expression/work/result limits are not reset per request.
    /// This does not select rules, parse shell commands or authorize execution.
    ///
    /// # Errors
    /// Refuses mismatched frames, state-changing effects, input drift and
    /// aggregate request/result/work limits, as in the scalar method.
    pub fn evaluate_many_captured_target_recipes(
        &mut self,
        requests: &[MakeTargetRecipeRequest<'_>],
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(Vec<MakeRecipeProjection>, VerifiedMakeInputReads), String> {
        check_target_recipe_request_count(requests)?;
        self.check_paths(paths).map_err(|error| error.to_string())?;
        if snapshot.working_directory() != paths.working_directory {
            return Err("Make captured input cwd differs from program context".into());
        }
        validate_target_recipe_requests(requests, &self.state_work)?;
        self.captured_evaluation(paths, snapshot, None, |state, execution| {
            evaluate_target_recipe_requests(requests, state, execution)
        })
    }

    /// Evaluate target recipe projections within a caller-owned captured reader.
    ///
    /// The caller must use a privately owned program and discard it if the
    /// enclosing reader transaction or its postchecks fail. This method checks
    /// the bound program paths, reader cwd and captured shell search capability,
    /// but does not verify the reader's complete roots or authorize execution.
    /// It commits staged Source state only when recipe evaluation and the
    /// captured shell search postcheck both succeed.
    ///
    /// # Errors
    /// Refuses mismatched frames, paths or cwd, state-changing effects, input
    /// drift and aggregate request/result/work limits.
    pub(crate) fn evaluate_many_captured_target_recipes_in_reader(
        &mut self,
        requests: &[MakeTargetRecipeRequest<'_>],
        paths: &MakeProgramPaths<'_>,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<Vec<MakeRecipeProjection>, String> {
        check_target_recipe_request_count(requests)?;
        self.check_paths(paths).map_err(|error| error.to_string())?;
        if reader.working_directory() != paths.working_directory {
            return Err("Make captured input cwd differs from program context".into());
        }
        validate_target_recipe_requests(requests, &self.state_work)?;
        self.revalidate_captured_search()?;
        let staged = self.stage().map_err(|error| error.to_string())?;
        let result =
            self.evaluate_captured_reader(paths, reader, &staged, None, |state, execution| {
                evaluate_target_recipe_requests(requests, state, execution)
            });
        let search_after = self.revalidate_captured_search();
        let results = match (result, search_after) {
            (Ok(results), Ok(())) => results,
            (Err(error), _) | (Ok(_), Err(error)) => return Err(error),
        };
        self.state = staged.into_inner();
        Ok(results)
    }

    fn captured_evaluation<T>(
        &mut self,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
        automatic_variables: Option<&BTreeMap<String, String>>,
        operation: impl FnOnce(&RefCell<State>, &Evaluation<'_>) -> Result<T, String>,
    ) -> Result<(T, VerifiedMakeInputReads), String> {
        self.captured_evaluation_with_reads(
            paths,
            snapshot,
            automatic_variables,
            |state, execution, _| operation(state, execution),
        )
    }

    fn captured_evaluation_with_reads<T>(
        &mut self,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
        automatic_variables: Option<&BTreeMap<String, String>>,
        operation: impl FnOnce(
            &RefCell<State>,
            &Evaluation<'_>,
            &RefCell<&mut MakeInputReader<'_>>,
        ) -> Result<T, String>,
    ) -> Result<(T, VerifiedMakeInputReads), String> {
        self.check_paths(paths).map_err(|error| error.to_string())?;
        if snapshot.working_directory() != paths.working_directory {
            return Err("Make captured input cwd differs from program context".into());
        }
        self.revalidate_captured_search()?;
        let staged = self.stage().map_err(|error| error.to_string())?;
        let result = snapshot.transaction(|reader| {
            self.evaluate_captured_reader_with_reads(
                paths,
                reader,
                &staged,
                automatic_variables,
                operation,
            )
        })?;
        self.revalidate_captured_search()?;
        self.state = staged.into_inner();
        Ok(result)
    }

    fn evaluate_captured_reader<T>(
        &self,
        paths: &MakeProgramPaths<'_>,
        reader: &mut MakeInputReader<'_>,
        staged: &RefCell<State>,
        automatic_variables: Option<&BTreeMap<String, String>>,
        operation: impl FnOnce(&RefCell<State>, &Evaluation<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.evaluate_captured_reader_with_reads(
            paths,
            reader,
            staged,
            automatic_variables,
            |state, execution, _| operation(state, execution),
        )
    }

    fn evaluate_captured_reader_with_reads<T>(
        &self,
        paths: &MakeProgramPaths<'_>,
        reader: &mut MakeInputReader<'_>,
        staged: &RefCell<State>,
        automatic_variables: Option<&BTreeMap<String, String>>,
        operation: impl FnOnce(
            &RefCell<State>,
            &Evaluation<'_>,
            &RefCell<&mut MakeInputReader<'_>>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        let reader = RefCell::new(reader);
        let wildcard_work = Cell::new(0);
        let wildcard = |pattern: &str| {
            super::captured_wildcard::query(&mut reader.borrow_mut(), pattern, &wildcard_work)
        };
        let realpath = |input: &str| captured_realpath(&mut reader.borrow_mut(), input);
        let shell = |command: &str| {
            super::captured_shell::evaluate(
                command,
                &reader,
                staged,
                paths,
                &self.expressions,
                (&self.state_work, &wildcard_work),
                None,
            )
        };
        operation(
            staged,
            &Evaluation {
                paths,
                expressions: &self.expressions,
                work: &self.state_work,
                file: None,
                captured_wildcard: Some(&wildcard),
                captured_realpath: Some(&realpath),
                captured_shell: Some(&shell),
                automatic_variables,
            },
            &reader,
        )
    }

    fn revalidate_captured_search(&self) -> Result<(), String> {
        if let Some(search) = self
            .state
            .captured_shell_policy
            .as_ref()
            .and_then(|policy| policy.source_search.as_ref())
        {
            search.revalidate()?;
        }
        Ok(())
    }

    /// Interpret a root and includes from a complete captured input selection.
    ///
    /// Exact read identities, not canonical aliases, feed input-list semantics.
    /// The reader resolves relative includes against the bound process cwd,
    /// never the including file's parent. State and events publish only after
    /// every selected tree passes the final identity/content revalidation.
    /// This is data interpretation, not authentication of a native invocation,
    /// proof of a complete source program, or permission to execute recipes.
    ///
    /// # Errors
    /// Refuses mismatched cwd, unsafe/changing inputs, unbound include search,
    /// unsupported Make syntax and exhausted interpreter/input budgets.
    pub fn read_captured_file(
        &mut self,
        input: &str,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(MakeFileProgram, VerifiedMakeInputReads), String> {
        self.read_captured_files(&[input], paths, snapshot)
    }

    /// Read the ordered explicit Make inputs in one captured transaction.
    ///
    /// Repeated read spellings remain repeated source reads. File-list and
    /// assignment state flow between inputs; failure in any input publishes
    /// neither partial events nor state. Includes share the same bounded reader.
    /// Recipes remain inert and no Make update/remake schedule is executed.
    ///
    /// # Errors
    /// Refuses empty/oversized selections and every refusal of
    /// [`Self::read_captured_file`], atomically across the complete selection.
    pub fn read_captured_files(
        &mut self,
        inputs: &[&str],
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
    ) -> Result<(MakeFileProgram, VerifiedMakeInputReads), String> {
        self.validate_captured_file_request(inputs, paths, snapshot.working_directory())?;
        self.revalidate_captured_search()?;
        let staged = self.stage().map_err(|error| error.to_string())?;
        // Keep the state private until the snapshot's post-read whole-root
        // verification succeeds. A failed transaction must leave `self` intact.
        let ((file, _file_retained, staged), reads) = snapshot.transaction(|reader| {
            self.interpret_captured_files_with_staged(inputs, paths, reader, staged, None)
        })?;
        self.revalidate_captured_search()?;
        self.state = staged;
        Ok((file, reads))
    }

    /// Read complete selected files using an explicit shell-effect session.
    ///
    /// Every shell expression, including an otherwise modeled builtin, uses
    /// the selected provider. Missing effects never fall back to simulation or
    /// ambient execution. Source-derived controls and exports are checked at
    /// every effect. The provider must independently bind original execution
    /// and input/runtime policy; a result packet alone is not authority.
    /// The provider's postcheck runs even after a failed Source/input read.
    /// State is published only after complete coverage and all postchecks.
    ///
    /// This does not launch Make or authenticate a provider, reproduce GNU's
    /// process phase, or complete generated-input/recipe/SDK obligations.
    ///
    /// # Errors
    /// Refuses an absent bound shell policy, changed roots/runtime/controls,
    /// provider refusal, incomplete effects and all ordinary Source refusals.
    pub fn read_captured_files_with_shell_effects(
        &mut self,
        inputs: &[&str],
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
        provider: &dyn crate::make_shell_effects::CapturedShellEffects,
    ) -> Result<(MakeFileProgram, VerifiedMakeInputReads), String> {
        self.validate_captured_file_request(inputs, paths, snapshot.working_directory())?;
        if !self.captured_shell_policy_bound() {
            return Err("observed shell file read needs independently bound shell policy".into());
        }
        self.revalidate_captured_search()?;
        let staged = self.stage().map_err(|error| error.to_string())?;
        provider
            .revalidate()
            .map_err(|_| "shell effect provider precheck refused")?;
        let session = super::captured_shell::EffectSession {
            provider,
            next: Cell::new(0),
            native_context: None,
        };
        let parsed = snapshot.transaction(|reader| {
            let result = self.interpret_captured_files_with_staged(
                inputs,
                paths,
                reader,
                staged,
                Some(&session),
            )?;
            provider
                .finish(session.next.get())
                .map_err(|_| "shell effect provider coverage refused")?;
            Ok(result)
        });
        let provider_after = provider
            .revalidate()
            .map_err(|_| "shell effect provider postcheck refused".to_owned());
        let search_after = self.revalidate_captured_search();
        let ((file, _, state), reads) = match (parsed, provider_after, search_after) {
            (Ok(result), Ok(()), Ok(())) => result,
            (Err(error), _, _) | (Ok(_), Err(error), _) | (Ok(_), Ok(()), Err(error)) => {
                return Err(error);
            }
        };
        self.state = state;
        Ok((file, reads))
    }

    /// Interpret explicit files with a caller-owned verified reader.
    ///
    /// This does not verify the complete roots itself. The reader's enclosing
    /// transaction must verify roots before and after its complete operation,
    /// including error returns. The parser stages state locally and commits it
    /// only after every selected file has parsed successfully.
    ///
    /// # Errors
    /// Refuses the same invalid paths, startup inputs, reads and source syntax
    /// as [`Self::read_captured_files`].
    pub(crate) fn read_captured_files_in_reader(
        &mut self,
        inputs: &[&str],
        paths: &MakeProgramPaths<'_>,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<(MakeFileProgram, usize), String> {
        self.validate_captured_file_request(inputs, paths, reader.working_directory())?;
        self.revalidate_captured_search()?;
        let staged = self.stage().map_err(|error| error.to_string())?;
        let (file, file_retained, staged) =
            self.interpret_captured_files_with_staged(inputs, paths, reader, staged, None)?;
        self.revalidate_captured_search()?;
        self.state = staged;
        Ok((file, file_retained))
    }

    /// Effects-aware counterpart under the SAME enclosing input transaction.
    /// The caller must discard this local program if whole-batch postchecks
    /// fail. Provider/search postchecks run even when parsing/coverage refuses.
    pub(crate) fn read_captured_files_in_reader_with_shell_effects(
        &mut self,
        inputs: &[&str],
        paths: &MakeProgramPaths<'_>,
        reader: &mut MakeInputReader<'_>,
        effects: &crate::make_shell_effects::ShellEffectSelection<'_>,
    ) -> Result<(MakeFileProgram, usize), String> {
        let provider = effects.provider;
        self.validate_captured_file_request(inputs, paths, reader.working_directory())?;
        if !self.captured_shell_policy_bound() {
            return Err("observed shell file read needs independently bound shell policy".into());
        }
        self.revalidate_captured_search()?;
        let staged = self.stage().map_err(|error| error.to_string())?;
        provider
            .revalidate()
            .map_err(|_| "shell effect provider precheck refused")?;
        let session = super::captured_shell::EffectSession {
            provider,
            next: Cell::new(0),
            native_context: Some(effects.observation),
        };
        let parsed = self
            .interpret_captured_files_with_staged(inputs, paths, reader, staged, Some(&session))
            .and_then(|result| {
                provider
                    .finish(session.next.get())
                    .map_err(|_| "shell effect provider coverage refused")?;
                Ok(result)
            });
        let provider_after = provider
            .revalidate()
            .map_err(|_| "shell effect provider postcheck refused".to_owned());
        let search_after = self.revalidate_captured_search();
        let (file, retained, staged) = match (parsed, provider_after, search_after) {
            (Ok(result), Ok(()), Ok(())) => result,
            (Err(error), _, _) | (Ok(_), Err(error), _) | (Ok(_), Ok(()), Err(error)) => {
                return Err(error);
            }
        };
        self.state = staged;
        Ok((file, retained))
    }

    fn validate_captured_file_request(
        &self,
        inputs: &[&str],
        paths: &MakeProgramPaths<'_>,
        working_directory: &Path,
    ) -> Result<(), String> {
        if inputs.is_empty() || inputs.len() > super::MAX_STATEMENTS {
            return Err("Make explicit input count exceeds its admitted range".into());
        }
        self.check_paths(paths).map_err(|error| error.to_string())?;
        if working_directory != paths.working_directory {
            return Err("Make captured input cwd differs from program context".into());
        }
        Ok(())
    }

    fn interpret_captured_files_with_staged(
        &self,
        inputs: &[&str],
        paths: &MakeProgramPaths<'_>,
        reader: &mut MakeInputReader<'_>,
        staged: RefCell<State>,
        effects: Option<&super::captured_shell::EffectSession<'_>>,
    ) -> Result<(MakeFileProgram, usize, State), String> {
        let reader = RefCell::new(reader);
        let file = {
            let includes =
                |requested: &str, _from: &str| reader.borrow_mut().include_bytes(requested);
            let wildcard_work = Cell::new(0);
            let wildcard = |pattern: &str| {
                super::captured_wildcard::query(&mut reader.borrow_mut(), pattern, &wildcard_work)
            };
            let realpath = |input: &str| captured_realpath(&mut reader.borrow_mut(), input);
            let shell = |command: &str| {
                super::captured_shell::evaluate(
                    command,
                    &reader,
                    &staged,
                    paths,
                    &self.expressions,
                    (&self.state_work, &wildcard_work),
                    effects,
                )
            };
            let file = FileContext::new_bytes(&includes);
            let execution = Evaluation {
                paths,
                expressions: &self.expressions,
                work: &self.state_work,
                file: Some(&file),
                captured_wildcard: Some(&wildcard),
                captured_realpath: Some(&realpath),
                captured_shell: Some(&shell),
                automatic_variables: None,
            };
            for input in inputs {
                let bytes = reader
                    .borrow_mut()
                    .read_bytes(input)?
                    .ok_or("required Make root is absent")?;
                file.read_input_bytes(input, &bytes.bytes, &staged, &execution)
                    .map_err(|error| error.to_string())?;
            }
            file.finish_accounted()
        };
        let (file, file_retained) = file;
        Ok((file, file_retained, staged.into_inner()))
    }
}

fn captured_realpath(
    reader: &mut aros_common::make_input_snapshot::MakeInputReader<'_>,
    input: &str,
) -> Result<Option<String>, String> {
    reader
        .realpath(input)?
        .map(|path| {
            path.into_os_string()
                .into_string()
                .map_err(|_| "captured realpath is not UTF-8".into())
        })
        .transpose()
}

#[cfg(test)]
#[path = "captured_target_recipe_reader_tests.rs"]
mod captured_target_recipe_reader_tests;
