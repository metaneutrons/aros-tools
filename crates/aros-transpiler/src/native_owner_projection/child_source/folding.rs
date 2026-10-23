//! Sequential Source projection under the same shared-root batch transaction.

use super::{
    MatchedChildSource, MatchedOwnerChildCoordinates, NativeOwnerProjection,
    RetainedChildSourceBatchRequest, MAX_RETAINED_CHILD_SOURCE_BATCH_BYTES,
};
use crate::make_vars::MakeVariableProgram;
use aros_common::make_input_snapshot::{
    MakeInputBatch, MakeInputBatchLimits, MakeInputReader, MakeInputSnapshot,
};
use std::path::Path;

/// Separate transient interpreter and staged projection accounting limits.
///
/// Neither is an RSS/allocator ceiling. The callback must account accurately
/// for its owned projected representation, independently of the interpreter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedChildSourceFoldLimits {
    pub inputs: MakeInputBatchLimits,
    pub maximum_child_retained_bytes: usize,
    pub maximum_projected_bytes: usize,
}

/// A staged fold released only after every input and projection postcheck.
/// No execution, generated-input provenance or complete rule proof is added.
pub struct MatchedChildSourceFold<T> {
    value: T,
    children: usize,
    maximum_child_retained_bytes: usize,
    projected_bytes: usize,
}

impl<T> std::fmt::Debug for MatchedChildSourceFold<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MatchedChildSourceFold")
            .field("children", &self.children)
            .field(
                "maximum_child_retained_bytes",
                &self.maximum_child_retained_bytes,
            )
            .field("projected_bytes", &self.projected_bytes)
            .finish_non_exhaustive()
    }
}

impl<T> MatchedChildSourceFold<T> {
    #[must_use]
    pub const fn children(&self) -> usize {
        self.children
    }
    #[must_use]
    pub const fn maximum_child_retained_bytes(&self) -> usize {
        self.maximum_child_retained_bytes
    }
    #[must_use]
    pub const fn projected_bytes(&self) -> usize {
        self.projected_bytes
    }
    #[must_use]
    pub fn into_value(self) -> T {
        self.value
    }
}

impl NativeOwnerProjection {
    /// Project a missing include and its recipe/input bindings in this Source reader.
    ///
    /// The separate trace cannot grant original-goal or Mesa26 shell authority.
    /// The callback must remain data-only; its result is released only after
    /// original Source, input, control and program postchecks. Prerequisites
    /// still need independent producer/input resolution before any execution.
    /// `selected.2` optionally supplies the original owning output root and
    /// inventory, not a newly captured tree or the current reader's inventory.
    /// The owning caller must independently select and revalidate that pair
    /// before/after this data-only transaction. Per-file hashes bind generated
    /// configuration programs without admitting additional Source programs.
    /// # Errors
    /// Refuses foreign context/origins, invalid include/index data and all
    /// existing retained-child interpretation, binding and resource errors.
    pub fn with_retained_child_include_reader<T>(
        &self,
        roots: &[String],
        selected: (
            &RetainedChildSourceBatchRequest<'_, '_>,
            usize,
            Option<(&Path, &aros_common::TreeContentCas)>,
        ),
        snapshot: &MakeInputSnapshot,
        limits: super::RetainedChildSourceBatchLimits,
        check_control: impl FnMut() -> Result<(), String>,
        consume: impl FnOnce(
            &super::DeclaredIncludedRecipes,
            &mut MakeInputReader<'_>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_retained_owned_child_include_reader(
            roots,
            selected,
            snapshot,
            limits,
            check_control,
            |included, reader| consume(&included, reader),
        )
    }

    /// Retain the parsed include-remake state after the enclosing input checks.
    ///
    /// The owned value is data only, not a completed producer or permission to
    /// execute. It lets an owning runner preserve immediate Make assignments
    /// while re-evaluating later recipe frames after a checked directory step.
    /// Failure of the callback or any enclosing check discards the whole value.
    /// # Errors
    /// Refuses all original include-reader, Source, context and resource errors.
    pub fn with_retained_owned_child_include_reader<T>(
        &self,
        roots: &[String],
        selected: (
            &RetainedChildSourceBatchRequest<'_, '_>,
            usize,
            Option<(&Path, &aros_common::TreeContentCas)>,
        ),
        snapshot: &MakeInputSnapshot,
        limits: super::RetainedChildSourceBatchLimits,
        check_control: impl FnMut() -> Result<(), String>,
        consume: impl FnOnce(
            super::DeclaredIncludedRecipes,
            &mut MakeInputReader<'_>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        let (request, include_event, original_output) = selected;
        self.with_retained_child_source_reader(
            roots,
            request,
            snapshot,
            limits,
            check_control,
            |source, reader| {
                let included = source
                    .trace_declared_rules_in_reader(reader)?
                    .project_included_recipes_in_reader(
                        self,
                        include_event,
                        request.paths,
                        reader,
                        original_output,
                    )?;
                consume(included, reader)
            },
        )
    }

    /// Compose one exact Source-generated recipe under this batch's policy.
    ///
    /// Matching, parsing, tracing, frame evaluation and generated-origin checks
    /// share the same reader and this projection's before/after verification.
    /// The callback remains data-only; its result is released only after all
    /// enclosing input/control/projection postchecks. No recipe is executed.
    /// # Errors
    /// Refuses foreign context/origin, invalid recipe index, callback failure
    /// and all ordinary child batch, interpretation and resource errors.
    pub fn with_retained_child_recipe_reader<T>(
        &self,
        roots: &[String],
        selected: (&RetainedChildSourceBatchRequest<'_, '_>, usize),
        snapshot: &MakeInputSnapshot,
        limits: super::RetainedChildSourceBatchLimits,
        check_control: impl FnMut() -> Result<(), String>,
        consume: impl FnOnce(
            &super::DeclaredChildRecipeProjection,
            &super::DeclaredChildRecipe,
            &crate::make_vars::MakeFileEvent,
            &MatchedOwnerChildCoordinates,
            &mut MakeInputReader<'_>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        let (request, recipe_index) = selected;
        self.with_retained_child_source_reader(
            roots,
            request,
            snapshot,
            limits,
            check_control,
            |source, reader| {
                let recipes = source
                    .trace_declared_rules_in_reader(reader)?
                    .project_recipes_in_reader(request.paths, reader)?;
                // The enclosing batch verifies this same Source projection
                // before and after; no caller-selected substitute is accepted.
                recipes.source_generated_recipe_in_reader(
                    self,
                    recipe_index,
                    reader,
                    |recipe, origin, coordinates, reader| {
                        consume(&recipes, recipe, origin, coordinates, reader)
                    },
                )
            },
        )
    }

    /// Compose one matched child's data operations inside its existing reader.
    ///
    /// The callback must remain data-only and return privately owned data. It
    /// must not publish intermediate results. Input manifests are provisional
    /// until the child, shared batch and Source-projection postchecks finish.
    /// This grants no process, recipe, output or scheduling authority.
    /// # Errors
    /// Refuses invalid limits, coordinate/context drift, parser/callback errors,
    /// aggregate read/work exhaustion or changed original inputs.
    pub fn with_retained_child_source_reader<T>(
        &self,
        roots: &[String],
        request: &RetainedChildSourceBatchRequest<'_, '_>,
        snapshot: &MakeInputSnapshot,
        limits: super::RetainedChildSourceBatchLimits,
        check_control: impl FnMut() -> Result<(), String>,
        consume: impl FnOnce(MatchedChildSource, &mut MakeInputReader<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        if limits.maximum_retained_bytes == 0
            || limits.maximum_retained_bytes > MAX_RETAINED_CHILD_SOURCE_BATCH_BYTES
        {
            return Err(
                "retained child source batch retained-data limit is outside 1..=1 GiB".into(),
            );
        }
        self.with_retained_child_source_batch(
            roots,
            std::slice::from_ref(request),
            snapshot,
            limits.inputs,
            check_control,
            |batch, coordinates| {
                let [coordinate] = <[MatchedOwnerChildCoordinates; 1]>::try_from(coordinates)
                    .map_err(|_| "retained child composition coordinate is not unique")?;
                consume_retained_child_in_batch_selected(
                    batch,
                    0,
                    request,
                    coordinate,
                    None,
                    |child, reader| {
                        if child
                            .retained_estimate_bytes
                            .ok_or("composed child lost interpreter accounting")?
                            > limits.maximum_retained_bytes
                        {
                            return Err(
                                "retained child source composition retained-data limit exceeded"
                                    .into(),
                            );
                        }
                        consume(child, reader)
                    },
                )
                .map(|(value, _, _)| value)
            },
        )
    }

    /// Interpret every selected child, projecting it before reading the next.
    ///
    /// The callback consumes a child and returns the COMPLETE current estimate
    /// for its owned accumulator, not a delta. It must release transient state,
    /// account its projected representation accurately, and must not publish or
    /// retain data outside the owned accumulator. A callback is trusted Rust
    /// application logic, not untrusted Source or a new authentication source.
    /// A dishonest callback is not made memory-safe by its declared estimate.
    ///
    /// The interpreter working limit and projected-data limit are distinct from
    /// the existing vector API's aggregate program retention bound. All input,
    /// read/query/work, control and postchecks remain shared across the batch.
    /// On any failure the staged accumulator is dropped; no partial result is
    /// returned. This grants neither command execution nor early publication.
    ///
    /// # Errors
    /// Refuses invalid limits, coordinates, inputs, Source semantics, working
    /// or projected accounting exhaustion, callback refusal or input drift.
    pub fn fold_retained_child_sources<T, F>(
        &self,
        roots: &[String],
        requests: &[RetainedChildSourceBatchRequest<'_, '_>],
        snapshot: &MakeInputSnapshot,
        limits: RetainedChildSourceFoldLimits,
        check_control: impl FnMut() -> Result<(), String>,
        fold: (T, F),
    ) -> Result<MatchedChildSourceFold<T>, String>
    where
        F: FnMut(&mut T, usize, MatchedChildSource) -> Result<usize, String>,
    {
        self.fold_selected_child_sources(
            roots,
            (requests, None),
            snapshot,
            limits,
            check_control,
            fold,
        )
    }

    /// Fold first-input children with independently selected per-child effects.
    ///
    /// Sessions run INSIDE each shared-batch child read, before projection.
    /// Every child needs exactly one supplied selection; absent effects never
    /// fall back. All original batch input/control/accounting/postchecks apply.
    /// A provider remains caller-owned execution/observation policy, not
    /// authentication inferred from a packet, descriptor or unsupported Source.
    /// Deferred recipe/recursive effects remain outside these read sessions.
    ///
    /// # Errors
    /// Refuses mismatched selections, wrong phase/missing recipe controls,
    /// provider failures and all ordinary fold/input/working/projection limits.
    pub fn fold_retained_child_sources_with_shell_effects<T, F>(
        &self,
        roots: &[String],
        batch: super::RetainedChildSourceEffectBatch<'_, '_, '_>,
        snapshot: &MakeInputSnapshot,
        limits: RetainedChildSourceFoldLimits,
        check_control: impl FnMut() -> Result<(), String>,
        fold: (T, F),
    ) -> Result<MatchedChildSourceFold<T>, String>
    where
        F: FnMut(&mut T, usize, MatchedChildSource) -> Result<usize, String>,
    {
        if batch.children.len() != batch.effects.len() {
            return Err("child Source shell session count differs from selection".into());
        }
        self.fold_selected_child_sources(
            roots,
            (batch.children, Some(batch.effects)),
            snapshot,
            limits,
            check_control,
            fold,
        )
    }

    fn fold_selected_child_sources<T, F>(
        &self,
        roots: &[String],
        selected: (
            &[RetainedChildSourceBatchRequest<'_, '_>],
            Option<&[crate::make_shell_effects::ShellEffectSelection<'_>]>,
        ),
        snapshot: &MakeInputSnapshot,
        limits: RetainedChildSourceFoldLimits,
        check_control: impl FnMut() -> Result<(), String>,
        fold: (T, F),
    ) -> Result<MatchedChildSourceFold<T>, String>
    where
        F: FnMut(&mut T, usize, MatchedChildSource) -> Result<usize, String>,
    {
        let (requests, effects) = selected;
        if [
            limits.maximum_child_retained_bytes,
            limits.maximum_projected_bytes,
        ]
        .into_iter()
        .any(|bytes| bytes == 0 || bytes > MAX_RETAINED_CHILD_SOURCE_BATCH_BYTES)
        {
            return Err("retained child source fold limit is outside 1..=1 GiB".into());
        }
        self.with_retained_child_source_batch(roots, requests, snapshot, limits.inputs, check_control, |batch, coordinates| {
            let (mut value, mut project) = fold;
            let mut maximum_child_retained_bytes = 0;
            let mut projected_bytes = 0;
            for (index, (request, coordinate)) in requests.iter().zip(coordinates).enumerate() {
                let (child, _, _) = read_retained_child_in_batch_selected(
                    batch, index, request, coordinate, effects.map(|effects| &effects[index]),
                )?;
                let child_bytes = child.retained_estimate_bytes
                    .ok_or("folded child lost interpreter accounting")?;
                if child_bytes > limits.maximum_child_retained_bytes {
                    return Err(format!("retained child source fold at index {index}: working child limit exceeded (estimate={child_bytes}, limit={})", limits.maximum_child_retained_bytes));
                }
                maximum_child_retained_bytes = maximum_child_retained_bytes.max(child_bytes);
                projected_bytes = project(&mut value, index, child)
                    .map_err(|_| format!("retained child source fold at index {index}: projection refused"))?;
                if projected_bytes > limits.maximum_projected_bytes {
                    return Err(format!("retained child source fold at index {index}: projected-data limit exceeded"));
                }
            }
            Ok(MatchedChildSourceFold { value, children: requests.len(), maximum_child_retained_bytes, projected_bytes })
        })
    }

    pub(super) fn with_retained_child_source_batch<T>(
        &self,
        roots: &[String],
        requests: &[RetainedChildSourceBatchRequest<'_, '_>],
        snapshot: &MakeInputSnapshot,
        limits: MakeInputBatchLimits,
        check_control: impl FnMut() -> Result<(), String>,
        operation: impl FnOnce(
            &mut MakeInputBatch<'_>,
            Vec<MatchedOwnerChildCoordinates>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        if requests.is_empty() || requests.len() > 4096 {
            return Err("retained child source batch count is outside its admitted range".into());
        }
        if requests.len() > limits.maximum_children {
            return Err("retained child source batch exceeds its configured child limit".into());
        }
        let first_paths = requests[0].paths;
        for request in requests {
            let paths = request.paths;
            if paths.source_root != first_paths.source_root
                || paths.top_directory != first_paths.top_directory
            {
                return Err("retained child source batch paths differ".into());
            }
            let owner_parent = Path::new(request.generated_owner)
                .parent()
                .ok_or("child owner has no source directory")?;
            if paths.relative_dir.as_os_str() != owner_parent.as_os_str()
                || paths.working_directory != request.observation.invocation().working_directory()
            {
                return Err(
                    "child source interpreter paths differ from selected coordinates".into(),
                );
            }
        }
        self.verify(first_paths.source_root)
            .map_err(|_| "child source projection revalidation failed")?;
        let coordinate_requests = requests
            .iter()
            .map(|request| (request.generated_owner, request.observation.invocation()))
            .collect::<Vec<_>>();
        let coordinates = self.match_captured_children_coordinates(
            first_paths.source_root,
            roots,
            first_paths.top_directory,
            &coordinate_requests,
        )?;
        if coordinates.len() != requests.len() {
            return Err("retained child source coordinate count changed".into());
        }
        let parsed = snapshot
            .batch_transaction(limits, check_control, |batch| operation(batch, coordinates));
        // Run this even when the callback, root transaction or accounting failed.
        let projection_after = self
            .verify(first_paths.source_root)
            .map_err(|_| "child source projection changed during interpretation");
        match (parsed, projection_after) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }
}

pub(super) fn read_retained_child_in_batch(
    batch: &mut MakeInputBatch<'_>,
    index: usize,
    request: &RetainedChildSourceBatchRequest<'_, '_>,
    coordinates: MatchedOwnerChildCoordinates,
) -> Result<(MatchedChildSource, usize, usize), String> {
    read_retained_child_in_batch_selected(batch, index, request, coordinates, None)
}

fn read_retained_child_in_batch_selected(
    batch: &mut MakeInputBatch<'_>,
    index: usize,
    request: &RetainedChildSourceBatchRequest<'_, '_>,
    coordinates: MatchedOwnerChildCoordinates,
    effects: Option<&crate::make_shell_effects::ShellEffectSelection<'_>>,
) -> Result<(MatchedChildSource, usize, usize), String> {
    consume_retained_child_in_batch_selected(
        batch,
        index,
        request,
        coordinates,
        effects,
        |child, _reader| Ok(child),
    )
}

fn consume_retained_child_in_batch_selected<T>(
    batch: &mut MakeInputBatch<'_>,
    index: usize,
    request: &RetainedChildSourceBatchRequest<'_, '_>,
    coordinates: MatchedOwnerChildCoordinates,
    effects: Option<&crate::make_shell_effects::ShellEffectSelection<'_>>,
    consume: impl FnOnce(MatchedChildSource, &mut MakeInputReader<'_>) -> Result<T, String>,
) -> Result<(T, usize, usize), String> {
    let paths = request.paths;
    let observation = request.observation;
    if effects.is_some_and(|effects| effects.observation != observation.binding()) {
        return Err(format!(
            "retained child source fold at index {index}: shell session child binding differs"
        ));
    }
    if effects.is_some()
        && observation.binding().phase
            != aros_common::make_native_observation::MakeObservationPhase::FirstInputPrelude
    {
        return Err(format!(
            "retained child source fold at index {index}: shell session needs first-input phase"
        ));
    }
    let (result, _reads) = batch
        .read_child(
            observation.invocation().working_directory(),
            None,
            |child_snapshot, reader| {
                let read_start = reader.read_checkpoint();
                let (program, file, file_retained) = if let Some(effects) = effects {
                    MakeVariableProgram::read_native_captured_inputs_in_reader_with_shell_effects(
                        paths,
                        observation,
                        reader,
                        effects,
                    )?
                } else {
                    MakeVariableProgram::read_native_captured_inputs_in_reader(
                        paths,
                        observation.initial_stdout(),
                        observation.invocation(),
                        observation.environment_names(),
                        reader,
                    )?
                };
                let variable_retained = program.retained_storage_bytes();
                let child_retained = variable_retained
                    .checked_add(file_retained)
                    .ok_or("retained child source size overflow")?;
                let child = MatchedChildSource {
                    coordinates,
                    build_root: paths.top_directory.to_owned(),
                    program,
                    file,
                    reads: reader.provisional_reads_since(read_start)?,
                    snapshot: child_snapshot.clone(),
                    default_directory_context: paths.dirs == &crate::dirs::DirVars::default(),
                    retained_estimate_bytes: Some(child_retained),
                };
                Ok((consume(child, reader)?, variable_retained, file_retained))
            },
        )
        .map_err(|error| format!("retained child source batch at index {index}: {error}"))?;
    Ok(result)
}
