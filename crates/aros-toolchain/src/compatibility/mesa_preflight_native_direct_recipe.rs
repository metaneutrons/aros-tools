//! One consuming original-recipe observation, not a native graph admission.

use super::{
    failure, run_prepared_observe, CollectedMesaNativeDirect, ContractError,
    MesaNativeCollectionSelection, MesaNativeDirectOrigin, MesaPreflightRequest, Stage,
};
use aros_common::{
    make_native_invocation::PrivateNativeCaptureDirectory, measure_tree_content_cas,
    publication::filesystem_paths_overlap, CancellationToken, Sha256Digest, TreeContentCas,
};
use aros_transpiler::native_owner_projection::RetainedChildSourceBatchLimits;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Component, Path, PathBuf},
};

/// Independent effect/shell selection for one original observed child recipe.
///
/// Effects are explicit caller policy, not inferred from shell text. This API
/// only admits the independently Source-bound Mesa26 exception. It does not
/// make arbitrary generated files trustworthy or implement Make scheduling.
pub struct Mesa26OriginalRecipeExecution<'a> {
    pub child_slot: usize,
    pub recipe_index: usize,
    pub selected_shell: &'a Path,
    pub evidence_root: &'a Path,
    pub mutable_files: &'a [&'a str],
    pub replaceable_files: &'a [&'a str],
    pub fresh_files: &'a [&'a str],
    pub fresh_directories: &'a [&'a str],
}

/// Independently selected generated list and member directory in this owner's output.
///
/// This selects data, not another shell recipe or an execution capability.
pub struct Mesa26GeneratedPathListSelection<'a> {
    pub list_file: &'a Path,
    pub entries_root: &'a Path,
}

/// A successful original-recipe line retaining its original live owner.
///
/// No conversion to a producer receipt, completed preflight or SDK exists.
/// Contiguous lines of that same frozen rule may continue by consuming this
/// outcome. A dependent rule needs separate lineage admission; this is not an
/// automatic scheduler or a completed GCA/preflight/SDK result.
/// The measured serial environment includes GNU flags, level and SHELL export
/// semantics. Parallel/jobserver startup and Source engine-state changes refuse;
/// this does not establish unrestricted GNU Make child equivalence.
pub struct ExecutedMesa26OriginalRecipe {
    original: CollectedMesaNativeDirect,
    stage: Stage,
    output: TreeContentCas,
    evidence: TreeContentCas,
    evidence_directory: PrivateNativeCaptureDirectory,
    cursor: RecipeCursor,
    history: RecipeHistory,
}

struct RecipeCursor {
    child_slot: usize,
    visit: usize,
    plan: Sha256Digest,
    next_index: Option<usize>,
    writes: BTreeSet<String>,
}

#[derive(Default)]
struct RecipeHistory {
    prior: Vec<RecipeEvidence>,
    graph: Option<Sha256Digest>,
    footprints: BTreeSet<String>,
    footprint_bytes: usize,
    generated: BTreeMap<String, usize>,
    generated_bytes: usize,
    completed: BTreeMap<String, Sha256Digest>,
    output_owners: BTreeMap<String, usize>,
    finished: BTreeSet<usize>,
}

#[derive(Clone, Copy)]
enum Continuation<'a> {
    Line(&'a RecipeCursor),
    Dependent(&'a RecipeCursor),
}

struct RecipeEvidence {
    stage: Stage,
    directory: PrivateNativeCaptureDirectory,
    tree: TreeContentCas,
}

impl ExecutedMesa26OriginalRecipe {
    /// Read ordered generated path data from one completed producer.
    ///
    /// The list must be a declared completed output. Every member must be a
    /// captured regular file below the independently selected member directory,
    /// covered by that same producer's effects, with no conflicting later
    /// effect scope. Order and duplicate members are retained. Only literal
    /// absolute shell-word paths are accepted; no expansion or command parsing
    /// is performed. Empty lists are valid. This does not execute a consumer,
    /// authorize effects, authenticate archives, or provide filesystem isolation.
    /// # Errors
    /// Refuses incomplete/unowned data, unsafe spellings, conflicting effects,
    /// changed owner/input/evidence/output, or exhausted original bounds.
    pub fn validate_mesa26_generated_path_list(
        &self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        selection: &Mesa26GeneratedPathListSelection<'_>,
    ) -> Result<Vec<PathBuf>, ContractError> {
        let (request, origin, selected, cancellation) = control;
        if self.cursor.next_index.is_some() {
            return Err(failure("generated path list has an incomplete producer"));
        }
        let list = selected_output_name(selection.list_file, request.output_root)?;
        selected_output_name(selection.entries_root, request.output_root)?;
        let producer = self.history.output_owners.get(list).ok_or_else(|| {
            failure("generated path list is not a declared completed producer output")
        })?;
        if !self.history.finished.contains(producer)
            || self.history.completed.get(list) != self.output.regular_file_digest(list)
            || !self.history.completed.contains_key(list)
            || self.history.generated.get(list) != Some(producer)
        {
            return Err(failure(
                "generated path list lacks a completed producer outcome",
            ));
        }
        self.validate_live_recipe_evidence()?;
        let result = self.original.with_source_child_batch_at_output(
            request,
            origin,
            selected,
            cancellation,
            &self.output,
            |snapshot, _, check_control| {
                snapshot
                    .transaction(|reader| {
                        // A data-policy refusal is not a Source-parser error.
                        // Retain it as data until the snapshot and original
                        // owner postchecks have completed. Guarded read errors
                        // still poison the transaction and cannot be hidden.
                        let validation = (|| -> Result<Vec<PathBuf>, String> {
                            check_control()?;
                            let entries = selection
                                .entries_root
                                .to_str()
                                .ok_or("non-text generated member directory")?;
                            if reader.entry_kind(entries)?
                                != Some(aros_common::TreeContentEntryKind::Directory)
                            {
                                return Err(
                                "generated member directory is not a captured physical directory"
                                    .into(),
                            );
                            }
                            let spelling = selection
                                .list_file
                                .to_str()
                                .ok_or("non-text generated list")?;
                            let captured = reader
                                .read_bytes(spelling)?
                                .ok_or("generated list is absent")?;
                            let members = parse_generated_path_list(&captured.bytes)?;
                            for member in &members {
                                check_control()?;
                                if member == selection.entries_root
                                    || !member.starts_with(selection.entries_root)
                                {
                                    return Err(
                                        "generated list member escaped its selected directory"
                                            .into(),
                                    );
                                }
                                let name = selected_output_name(member, request.output_root)
                                    .map_err(|_| {
                                        "generated list member escaped its original outputs"
                                    })?;
                                if self.output.regular_file_digest(name).is_none() {
                                    return Err(
                                        "generated list member is not a captured regular file"
                                            .into(),
                                    );
                                }
                                if self.history.generated.get(name) != Some(producer) {
                                    return Err(
                                        "generated list member lacks its list producer's effects"
                                            .into(),
                                    );
                                }
                                let member = member.to_str().ok_or("non-text generated member")?;
                                if reader.entry_kind(member)?
                                    != Some(aros_common::TreeContentEntryKind::RegularFile)
                                {
                                    return Err(
                                        "generated list member is not a captured physical file"
                                            .into(),
                                    );
                                }
                            }
                            Ok(members)
                        })();
                        Ok(validation)
                    })
                    .map(|(members, _)| members)
            },
        )?;
        self.validate_live_recipe_evidence()?;
        result.map_err(|reason| failure(&reason))
    }

    fn validate_live_recipe_evidence(&self) -> Result<(), ContractError> {
        validate_prior_evidence(&self.history.prior)?;
        self.stage.invocation_context.revalidate()?;
        self.evidence_directory
            .revalidate()
            .map_err(|_| failure("original recipe evidence directory changed"))?;
        if measure_tree_content_cas(self.evidence_directory.path())
            .map_err(|_| failure("cannot remeasure original recipe evidence"))?
            != self.evidence
        {
            return Err(failure("original recipe evidence changed"));
        }
        Ok(())
    }

    /// Consume the successful previous outcome and execute only its next line.
    ///
    /// The frozen rule plan, original clock, checked current output and every
    /// prior evidence tree remain bound. This cannot switch rules or establish
    /// prerequisite producer completion, Make scheduling, or a complete GCA run.
    /// # Errors
    /// Refuses skipped/repeated lines, plan/input/evidence drift and every
    /// ordinary original-recipe execution refusal.
    pub fn execute_next_mesa26_original_recipe(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
        execution: &Mesa26OriginalRecipeExecution<'_>,
    ) -> Result<Self, ContractError> {
        let Self {
            original,
            stage,
            output,
            evidence,
            evidence_directory,
            cursor,
            mut history,
        } = self;
        history.prior.push(RecipeEvidence {
            stage,
            directory: evidence_directory,
            tree: evidence,
        });
        original.execute_recipe_at_output(
            control,
            limits,
            execution,
            &output,
            Some(Continuation::Line(&cursor)),
            history,
        )
    }

    /// Start a dependent rule only after the preceding rule completed fully.
    ///
    /// At least one direct prerequisite must be a checked completed producer
    /// output retained by this live owner. Other inputs must still be original
    /// untouched captured files or completed outputs, never arbitrary new files.
    /// Existing Mesa26 Source/goal/depth admission remains unchanged.
    /// # Errors
    /// Refuses incomplete/repeated/unrelated rules, missing producer lineage,
    /// drift or every ordinary execution refusal. No filesystem resume exists.
    pub fn execute_mesa26_dependent_recipe(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
        execution: &Mesa26OriginalRecipeExecution<'_>,
    ) -> Result<Self, ContractError> {
        let Self {
            original,
            stage,
            output,
            evidence,
            evidence_directory,
            cursor,
            mut history,
        } = self;
        history.prior.push(RecipeEvidence {
            stage,
            directory: evidence_directory,
            tree: evidence,
        });
        original.execute_recipe_at_output(
            control,
            limits,
            execution,
            &output,
            Some(Continuation::Dependent(&cursor)),
            history,
        )
    }

    #[must_use]
    pub const fn original_selection(&self) -> &Sha256Digest {
        &self.original.prepared.selection_sha256
    }

    #[must_use]
    pub const fn invocation_sha256(&self) -> &Sha256Digest {
        &self.stage.invocation
    }

    #[must_use]
    pub const fn output_snapshot(&self) -> &TreeContentCas {
        &self.output
    }

    #[must_use]
    pub const fn evidence_snapshot(&self) -> &TreeContentCas {
        &self.evidence
    }
}

impl CollectedMesaNativeDirect {
    /// Consume the original live owner and supervise one exact Source recipe.
    ///
    /// Its direct prerequisites must already be captured regular input files;
    /// declarations/existence after new writes cannot stand for completed
    /// producers. This is a deliberately explicit captured-input mode, not Make
    /// timestamp/phony/implicit scheduling. Source interpretation remains inside
    /// the data-only batch; process launch happens only after its postcheck.
    /// The original clock/cancellation and existing bounded runner are reused.
    /// Output selections grant no filesystem confinement or rollback guarantee.
    /// This admits only the independently measured serial environment subset,
    /// not arbitrary GNU Make engine state or productive native graphs.
    /// # Errors
    /// Refuses foreign recipes, missing inputs, earlier unexecuted recipe lines,
    /// unsupported prefixes, drift, unsafe evidence, process failure or effects
    /// outside the selected transition. Any failure consumes this live owner.
    pub fn execute_mesa26_original_recipe(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
        execution: &Mesa26OriginalRecipeExecution<'_>,
    ) -> Result<ExecutedMesa26OriginalRecipe, ContractError> {
        let output = self.output_tree.clone();
        self.execute_recipe_at_output(
            control,
            limits,
            execution,
            &output,
            None,
            RecipeHistory::default(),
        )
    }

    fn execute_recipe_at_output(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
        execution: &Mesa26OriginalRecipeExecution<'_>,
        current_output: &TreeContentCas,
        continuation: Option<Continuation<'_>>,
        mut history: RecipeHistory,
    ) -> Result<ExecutedMesa26OriginalRecipe, ContractError> {
        let (request, origin, selected, cancellation) = control;
        if history.prior.len() >= 1024 {
            return Err(failure("original recipe sequence exceeds its line bound"));
        }
        validate_prior_evidence(&history.prior)?;
        if let Some(Continuation::Line(previous)) = continuation {
            if previous.child_slot != execution.child_slot
                || previous.next_index != Some(execution.recipe_index)
            {
                return Err(failure("original recipe is not the next owned line"));
            }
        }
        if let Some(Continuation::Dependent(previous)) = continuation {
            if previous.next_index.is_some() || previous.child_slot != execution.child_slot {
                return Err(failure(
                    "dependent recipe has an incomplete or foreign producer",
                ));
            }
        }
        let mut recipes = self.with_source_child_batch_at_output(
            request,
            origin,
            selected,
            cancellation,
            current_output,
            |snapshot, children, check_control| {
                let child = children
                    .get(execution.child_slot)
                    .ok_or("original recipe child slot is absent")?;
                let mut sources = self
                    .prepared
                    .policy
                    .projection
                    .read_retained_child_sources(
                        &self.prepared.roots,
                        std::slice::from_ref(child),
                        snapshot,
                        limits,
                        check_control,
                    )?;
                if sources.len() != 1 {
                    return Err("original recipe child selection is not unique".into());
                }
                sources
                    .pop()
                    .ok_or("original recipe child is absent")?
                    .trace_declared_rules()?
                    .project_recipes(child.paths)
            },
        )?;
        let recipe = recipes
            .recipes()
            .get(execution.recipe_index)
            .ok_or_else(|| failure("original recipe index is absent"))?;
        // A later line must not run in place of earlier commands in its rule.
        // Staged execution will discharge this via retained prior outcomes.
        if !matches!(continuation, Some(Continuation::Line(_))) && recipe.recipe() != 0 {
            return Err(failure(
                "original recipe has unexecuted preceding recipe lines",
            ));
        }
        let visit = recipe.visit();
        if let Some(Continuation::Dependent(previous)) = continuation {
            if previous.visit == visit || history.finished.contains(&visit) {
                return Err(failure(
                    "dependent recipe repeats an already completed rule",
                ));
            }
        }
        let dirs = aros_transpiler::dirs::DirVars::default();
        let owner = recipes
            .declared()
            .coordinates()
            .generated_owner()
            .to_owned();
        let paths = aros_transpiler::make_vars::MakeProgramPaths {
            dirs: &dirs,
            source_root: request.source_root,
            relative_dir: Path::new(&owner)
                .parent()
                .ok_or_else(|| failure("original recipe owner has no directory"))?,
            top_directory: self.prepared.invocation.working_directory(),
            working_directory: self.calls[execution.child_slot]
                .invocation()
                .working_directory(),
        };
        let indices = recipes
            .recipes()
            .iter()
            .enumerate()
            .filter_map(|(index, recipe)| {
                (recipe.visit() == visit).then_some((index, recipe.recipe()))
            })
            .collect::<Vec<_>>();
        if indices.is_empty()
            || indices.len() > 1024
            || indices
                .iter()
                .enumerate()
                .any(|(line, (_, actual))| line != *actual)
        {
            return Err(failure(
                "original recipe rule has an incomplete ordered line plan",
            ));
        }
        let mut plan = Vec::with_capacity(indices.len());
        let rule_identity = declared_rule_identity(&recipes, visit)?;
        for (index, _) in &indices {
            let origin = recipes
                .origin(*index)
                .map_err(|_| failure("original rule plan lost its recipe event origin"))?;
            let event_identity = serde_json::json!({
                "input":origin.input,"line":origin.source_line,"generated":origin.generated,
                "active":origin.active,"position":origin.variable_position,
            });
            let bound = recipes
                .bind_mesa26_original_recipe(&self.prepared.policy.projection, &paths, *index)
                .map_err(|_| failure("original rule plan lacks Source/frame binding"))?;
            let (prefixes, body) = bound
                .recipe()
                .original_shell_text()
                .map_err(|_| failure("original rule plan has unsupported text"))?;
            if prefixes.ignore_errors || prefixes.explicit_recursive {
                return Err(failure(
                    "original rule plan needs unsupported execution controls",
                ));
            }
            let environment = bound
                .recipe()
                .serial_native_environment(self.calls[execution.child_slot].invocation())
                .map_err(|_| failure("original rule plan lacks a serial environment"))?;
            plan.push(serde_json::json!({
                "index":index,"body":body,"shell":bound.shell(),"flags":bound.flags(),
                "environment":environment,"source_exports":bound.recipe().source_exports(),
                "expression":bound.recipe().expression(),
                "intrinsic_selection":bound.intrinsic_selection_sha256(),
                "origin":event_identity,
            }));
        }
        let plan = aros_common::sha256_bytes(
            &serde_json::to_vec(&("mesa26-owned-original-rule-v1", rule_identity, plan))
                .map_err(|_| failure("cannot bind original rule plan"))?,
        );
        if let Some(Continuation::Line(previous)) = continuation {
            if previous.visit != visit || previous.plan != plan {
                return Err(failure(
                    "original rule plan changed after its preceding line",
                ));
            }
        }
        let graph = declared_graph_identity(&recipes)?;
        if history
            .graph
            .as_ref()
            .is_some_and(|expected| expected != &graph)
        {
            return Err(failure(
                "original declared graph changed between recipe steps",
            ));
        }
        history.graph = Some(graph);
        let line = indices
            .iter()
            .position(|(index, _)| *index == execution.recipe_index)
            .ok_or_else(|| failure("original recipe line is outside its rule plan"))?;
        let mut cursor = RecipeCursor {
            child_slot: execution.child_slot,
            visit,
            plan,
            next_index: indices.get(line + 1).map(|(index, _)| *index),
            writes: match continuation {
                Some(Continuation::Line(previous)) => previous.writes.clone(),
                _ => BTreeSet::new(),
            },
        };
        let binding = recipes
            .bind_mesa26_original_recipe(
                &self.prepared.policy.projection,
                &paths,
                execution.recipe_index,
            )
            .map_err(|_| failure("original recipe lacks Mesa26 Source/frame binding"))?;
        let outputs = (
            execution.mutable_files,
            execution.fresh_files,
            execution.fresh_directories,
        );
        binding
            .verify_captured_prerequisites(outputs, execution.replaceable_files)
            .map_err(|_| {
                failure("original recipe prerequisites are not independent captured inputs")
            })?;
        let prerequisites = binding
            .captured_prerequisite_paths()
            .map_err(|_| failure("cannot bind original recipe prerequisite paths"))?;
        let mut uses_completed = false;
        for path in prerequisites {
            self.prepared
                .recorded
                .launch_inputs()
                .host
                .root
                .prepared
                .remaining()?;
            let Ok(relative) = path.strip_prefix(request.output_root) else {
                // Other captured roots are immutable Source/compiler inputs.
                continue;
            };
            let name = relative
                .to_str()
                .ok_or_else(|| failure("non-text recipe prerequisite path"))?;
            let current = current_output
                .regular_file_digest(name)
                .ok_or_else(|| failure("recipe prerequisite is not a regular owner output"))?;
            let untouched = !history
                .footprints
                .iter()
                .any(|written| paths_overlap(name, written));
            if untouched && self.output_tree.regular_file_digest(name) == Some(current) {
                continue;
            }
            if history.completed.get(name) != Some(current) {
                return Err(failure(
                    "recipe prerequisite lacks a completed producer outcome",
                ));
            }
            uses_completed = true;
        }
        if matches!(continuation, Some(Continuation::Dependent(_))) && !uses_completed {
            return Err(failure(
                "dependent recipe does not consume a completed producer output",
            ));
        }
        let mut targets = binding
            .declared_output_path()
            .map_err(|_| failure("cannot bind original recipe output target"))?
            .into_iter()
            .collect::<Vec<_>>();
        targets.extend(
            binding
                .declared_cooutput_paths()
                .map_err(|_| failure("cannot bind original recipe declared cooutputs"))?,
        );
        let parent = self
            .prepared
            .recorded
            .launch_inputs()
            .host
            .root
            .prepared
            .configured_invocation()?;
        let (invocation, prefixes) =
            crate::compatibility::recipe_shell::prepare_mesa26_original_recipe_invocation(
                &binding,
                execution.selected_shell,
                request.environment,
                parent.environment(),
                self.calls[execution.child_slot].invocation(),
            )?;
        if prefixes.ignore_errors || prefixes.explicit_recursive {
            return Err(failure(
                "original recipe needs unsupported error/recursive execution controls",
            ));
        }
        let crate::compatibility::CompatibilityEnvironment::SealedHostTools { host_tools, .. } =
            request.environment
        else {
            return Err(failure(
                "original recipe has no sealed host tool environment",
            ));
        };
        for immutable in [
            request.source_root,
            request.drivers.compiler_root(),
            request.output_root,
            self.directory.path(),
            self.prepared.recorded.evidence_root(),
            host_tools.root.as_path(),
        ] {
            if filesystem_paths_overlap(execution.evidence_root, immutable)
                .map_err(|_| failure("cannot compare original recipe evidence paths"))?
            {
                return Err(failure(
                    "original recipe evidence overlaps held inputs or outputs",
                ));
            }
        }
        for retained in &history.prior {
            if filesystem_paths_overlap(execution.evidence_root, retained.directory.path())
                .map_err(|_| failure("cannot compare recipe sequence evidence paths"))?
            {
                return Err(failure(
                    "original recipe evidence overlaps an earlier outcome",
                ));
            }
        }
        self.revalidate_output(request, origin, selected, current_output, cancellation)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(execution.evidence_root)
            .map_err(|_| failure("cannot create fresh original recipe evidence"))?;
        let evidence = PrivateNativeCaptureDirectory::open(execution.evidence_root)
            .map_err(|_| failure("cannot bind original recipe evidence directory"))?;
        let (stage, _) = run_prepared_observe(
            (
                request,
                cancellation,
                self.prepared
                    .recorded
                    .launch_inputs()
                    .host
                    .root
                    .prepared
                    .configured
                    .started,
            ),
            "mesa26-original-recipe",
            invocation,
            0,
            |stdout, stderr| {
                evidence
                    .write_new(OsStr::new("stdout"), stdout)
                    .map_err(|_| failure("cannot retain original recipe stdout"))?;
                evidence
                    .write_new(OsStr::new("stderr"), stderr)
                    .map_err(|_| failure("cannot retain original recipe stderr"))
            },
        )?;
        binding
            .observe_output_transition(outputs, execution.replaceable_files)
            .map_err(|_| failure("original recipe changed inputs or unselected outputs"))?;
        let output = measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot measure original recipe outputs"))?;
        let build = self.prepared.invocation.working_directory();
        let prefix = build
            .strip_prefix(request.output_root)
            .map_err(|_| failure("original recipe build is outside its original output root"))?;
        let map = |names: &[&str]| -> Result<Vec<String>, ContractError> {
            names
                .iter()
                .map(|name| {
                    prefix
                        .join(name)
                        .to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| failure("original recipe output mapping is not text"))
                })
                .collect()
        };
        let mutable = map(execution.mutable_files)?;
        let replaceable = map(execution.replaceable_files)?;
        let fresh = map(execution.fresh_files)?;
        let directories = map(execution.fresh_directories)?;
        if execution.mutable_files.is_empty()
            && execution.replaceable_files.is_empty()
            && execution.fresh_files.is_empty()
            && execution.fresh_directories.is_empty()
        {
            if &output != current_output {
                return Err(failure("original recipe wrote outside its build snapshot"));
            }
        } else if !current_output.matches_output_transition_with_replacements(
            &output,
            &path_refs(&mutable),
            &path_refs(&replaceable),
            &path_refs(&fresh),
            &path_refs(&directories),
        ) {
            return Err(failure(
                "original recipe changed unselected original-owner outputs",
            ));
        }
        self.revalidate_output(request, origin, selected, &output, cancellation)?;
        validate_prior_evidence(&history.prior)?;
        evidence
            .revalidate()
            .map_err(|_| failure("original recipe evidence directory changed"))?;
        let evidence_directory = evidence;
        let evidence = measure_tree_content_cas(evidence_directory.path())
            .map_err(|_| failure("cannot retain original recipe evidence tree"))?;
        if evidence.entry_count() != 2
            || evidence.regular_file_digest("stdout") != Some(&stage.stdout)
            || evidence.regular_file_digest("stderr") != Some(&stage.stderr)
        {
            return Err(failure(
                "original recipe evidence has unselected or changed entries",
            ));
        }
        for written in mutable
            .iter()
            .chain(&replaceable)
            .chain(&fresh)
            .chain(&directories)
        {
            history
                .completed
                .retain(|path, _| !paths_overlap(path, written));
            history
                .generated
                .retain(|path, owner| *owner == visit || !paths_overlap(path, written));
            if history.footprints.insert(written.clone()) {
                history.footprint_bytes = history
                    .footprint_bytes
                    .checked_add(written.len() + 128)
                    .filter(|bytes| *bytes <= 64 * 1024 * 1024)
                    .ok_or_else(|| failure("recipe lineage exceeds its storage bound"))?;
            }
            cursor.writes.insert(written.clone());
        }
        if history.footprints.len() > 65_536 {
            return Err(failure("recipe lineage exceeds its output path bound"));
        }
        // Record only files observed to appear during this owned step, not
        // every file named by an allowed effect scope. Later foreign scopes
        // conservatively invalidate this provenance, including no-op writes.
        for (name, kind) in output.entry_kinds() {
            self.prepared
                .recorded
                .launch_inputs()
                .host
                .root
                .prepared
                .remaining()?;
            if kind != aros_common::TreeContentEntryKind::RegularFile {
                continue;
            }
            let name = match std::str::from_utf8(name) {
                Ok(name) => name,
                Err(_) if current_output.entry_kinds().any(|(old, _)| old == name) => continue,
                Err(_) => return Err(failure("generated recipe output path is not text")),
            };
            if current_output.entry_kind(name).is_none() {
                if history.generated.len() >= 65_536 {
                    return Err(failure("generated recipe lineage exceeds its path bound"));
                }
                history.generated_bytes = history
                    .generated_bytes
                    .checked_add(name.len() + 128)
                    .filter(|bytes| *bytes <= 64 * 1024 * 1024)
                    .ok_or_else(|| failure("generated recipe lineage exceeds its storage bound"))?;
                history.generated.insert(name.to_owned(), visit);
            }
        }
        if cursor.next_index.is_none() {
            history.finished.insert(visit);
            for target in targets {
                let name = target
                    .strip_prefix(request.output_root)
                    .ok()
                    .and_then(Path::to_str)
                    .ok_or_else(|| failure("completed recipe target escaped original outputs"))?;
                if cursor
                    .writes
                    .iter()
                    .any(|written| paths_overlap(name, written))
                {
                    if let Some(digest) = output.regular_file_digest(name) {
                        if history
                            .output_owners
                            .get(name)
                            .is_some_and(|owner| *owner != visit)
                        {
                            return Err(failure("recipe output has competing completed producers"));
                        }
                        history.output_owners.insert(name.to_owned(), visit);
                        history.completed.insert(name.to_owned(), digest.clone());
                    }
                }
            }
        }
        Ok(ExecutedMesa26OriginalRecipe {
            original: self,
            stage,
            output,
            evidence,
            evidence_directory,
            cursor,
            history,
        })
    }
}

fn paths_overlap(a: &str, b: &str) -> bool {
    Path::new(a).starts_with(b) || Path::new(b).starts_with(a)
}

fn selected_output_name<'a>(path: &'a Path, output: &Path) -> Result<&'a str, ContractError> {
    let normalized: PathBuf = path.components().collect();
    if !path.is_absolute()
        || normalized.as_os_str() != path.as_os_str()
        || path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(failure(
            "generated path selection is not normalized absolute data",
        ));
    }
    path.strip_prefix(output)
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .and_then(Path::to_str)
        .ok_or_else(|| failure("generated path selection escaped original outputs"))
}

fn parse_generated_path_list(bytes: &[u8]) -> Result<Vec<PathBuf>, String> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("generated path list exceeds its byte bound".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "generated path list is not UTF-8")?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err("generated path list has an incomplete final record".into());
    }
    let mut members = Vec::new();
    for line in text.split_terminator('\n') {
        if members.len() >= 65_536
            || line.is_empty()
            || !line
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/_-+.,@%=:".contains(&byte))
        {
            return Err(
                "generated path list contains unsafe or excessive shell-word records".into(),
            );
        }
        let path = PathBuf::from(line);
        let normalized: PathBuf = path.components().collect();
        if !path.is_absolute()
            || normalized.as_os_str() != path.as_os_str()
            || path
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err("generated path list member is not normalized absolute data".into());
        }
        members.push(path);
    }
    Ok(members)
}

fn declared_graph_identity(
    recipes: &aros_transpiler::native_owner_projection::DeclaredChildRecipeProjection,
) -> Result<Sha256Digest, ContractError> {
    let mut rules = Vec::new();
    for (index, visit) in recipes.declared().traversal().visits().iter().enumerate() {
        let instance = visit
            .instance()
            .map(|_| declared_rule_identity(recipes, index))
            .transpose()?;
        let chain = recipes
            .declared()
            .traversal()
            .scope_chain(index)
            .map_err(|_| failure("original graph lost its first-parent scope"))?;
        rules.push(serde_json::json!({"visit":index,"target":visit.target(),"rule":instance,"chain":chain}));
    }
    let mut lines = Vec::new();
    for (index, line) in recipes.recipes().iter().enumerate() {
        let origin = recipes
            .origin(index)
            .map_err(|_| failure("declared graph recipe origin is absent"))?;
        lines.push(serde_json::json!({
            "visit":line.visit(),"line":line.recipe(),"body":line.expression().expression(),
            "exports":line.expression().source_exports(),"input":origin.input,
            "source_line":origin.source_line,"generated":origin.generated,
            "active":origin.active,"position":origin.variable_position,
        }));
    }
    serde_json::to_vec(&("mesa26-original-declared-graph-v1", rules, lines))
        .map(|bytes| aros_common::sha256_bytes(&bytes))
        .map_err(|_| failure("cannot bind original declared graph"))
}

fn declared_rule_identity(
    recipes: &aros_transpiler::native_owner_projection::DeclaredChildRecipeProjection,
    visit: usize,
) -> Result<serde_json::Value, ContractError> {
    let rule = recipes
        .declared()
        .traversal()
        .visits()
        .get(visit)
        .and_then(aros_transpiler::make_rule_graph::traversal::TargetVisit::instance)
        .ok_or_else(|| failure("original recipe rule is absent"))?;
    let chain = recipes
        .declared()
        .traversal()
        .scope_chain(visit)
        .map_err(|_| failure("original recipe rule lost its scope chain"))?;
    Ok(serde_json::json!({
        "owner":recipes.declared().coordinates().generated_owner(),
        "source_line":rule.source_line,"output_line":rule.output_line,
        "target":rule.target,"stem":rule.stem,"prerequisites":rule.prerequisites,
        "order_only":rule.order_only,"prerequisite_order":rule.prerequisite_order,
        "double_colon":rule.double_colon,"automatic":rule.automatic_variables(),
        "origin":rule_event_identity(recipes, rule.output_line)?,"chain":chain,
    }))
}

fn rule_event_identity(
    recipes: &aros_transpiler::native_owner_projection::DeclaredChildRecipeProjection,
    index: usize,
) -> Result<serde_json::Value, ContractError> {
    let origin = recipes
        .declared()
        .file()
        .events
        .get(index)
        .ok_or_else(|| failure("original rule header event is absent"))?;
    let kind = match &origin.kind {
        aros_transpiler::make_vars::MakeFileEventKind::Rule(_) => "rule",
        aros_transpiler::make_vars::MakeFileEventKind::DefaultRule(_) => "default-rule",
        _ => return Err(failure("original rule header event has a foreign kind")),
    };
    Ok(
        serde_json::json!({"kind":kind,"input":origin.input,"line":origin.source_line,
        "generated":origin.generated,"active":origin.active,"position":origin.variable_position}),
    )
}

fn validate_prior_evidence(prior: &[RecipeEvidence]) -> Result<(), ContractError> {
    for retained in prior {
        retained.stage.invocation_context.revalidate()?;
        retained
            .directory
            .revalidate()
            .map_err(|_| failure("earlier original recipe evidence directory changed"))?;
        if measure_tree_content_cas(retained.directory.path())
            .map_err(|_| failure("cannot remeasure earlier recipe evidence"))?
            != retained.tree
        {
            return Err(failure("earlier original recipe evidence changed"));
        }
    }
    Ok(())
}

fn path_refs(names: &[String]) -> Vec<&str> {
    names.iter().map(String::as_str).collect()
}

#[cfg(test)]
mod generated_path_list_tests {
    use super::parse_generated_path_list;
    use std::path::PathBuf;

    #[test]
    fn generated_list_preserves_order_duplicates_and_empty_set() {
        assert_eq!(
            parse_generated_path_list(
                b"/build/private/second.o\n/build/private/first.o\n/build/private/second.o\n"
            )
            .unwrap(),
            [
                "/build/private/second.o",
                "/build/private/first.o",
                "/build/private/second.o"
            ]
            .map(PathBuf::from)
        );
        assert!(parse_generated_path_list(b"").unwrap().is_empty());
    }

    #[test]
    fn generated_list_refuses_expansion_noncanonical_and_incomplete_words() {
        for text in [
            "/build/private/a b.o\n",
            "/build/private/$(cmd).o\n",
            "/build/private/`cmd`.o\n",
            "/build/private/*.o\n",
            "/build/private/[ab].o\n",
            "/build/private/a;o\n",
            "/build/private/a\\b.o\n",
            "/build/private/a\rb.o\n",
            "/build/private/../outside.o\n",
            "/build/private/./a.o\n",
            "/build//private/a.o\n",
            "private/a.o\n",
            "/build/private/a.o",
            "\n",
            "/build/private/a.o\n\n",
        ] {
            assert!(
                parse_generated_path_list(text.as_bytes()).is_err(),
                "accepted {text:?}"
            );
        }
        assert!(parse_generated_path_list(&[0xff, b'\n']).is_err());
    }

    #[test]
    fn generated_list_refuses_byte_and_record_exhaustion() {
        assert!(parse_generated_path_list(&vec![b'a'; 4 * 1024 * 1024 + 1]).is_err());
        assert!(parse_generated_path_list("/a\n".repeat(65_537).as_bytes()).is_err());
    }
}
