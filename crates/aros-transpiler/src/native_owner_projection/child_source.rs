//! Join coordinate-consistent child data to the captured source interpreter.

use super::{MatchedOwnerChildCoordinates, NativeOwnerProjection};
use crate::make_vars::{MakeFileProgram, MakeProgramPaths, MakeVariableProgram};
use aros_common::{
    make_input_snapshot::{MakeInputBatchLimits, MakeInputSnapshot, VerifiedMakeInputReads},
    make_native_invocation::NativeMakeInvocation,
    make_native_observation::RetainedMakeObservation,
};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

mod declared;
mod folding;
mod recipes;
pub use declared::{
    DeclaredChildJoinSelection, DeclaredChildOutputCandidate, DeclaredChildOutputLinks,
    DeclaredIncludedMakefile, DeclaredIncludedRecipes, MatchedChildDeclaredRules,
    Mesa26OriginalRecipeBinding, NativeGenmoduleIncludePlan,
};
use folding::read_retained_child_in_batch;
pub use folding::{MatchedChildSourceFold, RetainedChildSourceFoldLimits};
pub use recipes::{
    BoundChildRecipeBatch, DeclaredChildRecipe, DeclaredChildRecipeProjection,
    MatchedChildRecipePlan, SelectedChildRecipe,
};

#[cfg(test)]
mod tests;

/// Explicit caller selections, never reconstructed from the current process.
///
/// Raw observation data has no Debug implementation. The caller must bind
/// observation phase, full argv/env bytes, executable and parent evidence.
#[derive(Clone, Copy)]
pub struct CapturedChildSourceRequest<'a> {
    pub roots: &'a [String],
    pub generated_owner: &'a str,
    pub invocation: &'a NativeMakeInvocation,
    pub initial_stdout: &'a [u8],
    pub environment_names: &'a BTreeSet<String>,
    pub snapshot: &'a MakeInputSnapshot,
}

/// One retained native child observation paired with its independently bound
/// source interpreter paths. This request supplies no execution authentication.
pub struct RetainedChildSourceBatchRequest<'a, 'p> {
    pub paths: &'a MakeProgramPaths<'p>,
    pub generated_owner: &'a str,
    pub observation: &'a RetainedMakeObservation,
}

/// Exact one-to-one session selection for an effects-aware child fold.
///
/// Neither these descriptors nor retained observations authorize execution.
#[derive(Clone, Copy)]
pub struct RetainedChildSourceEffectBatch<'a, 'p, 's> {
    pub children: &'a [RetainedChildSourceBatchRequest<'a, 'p>],
    pub effects: &'a [crate::make_shell_effects::ShellEffectSelection<'s>],
}

/// Aggregate input and retained-result limits for one source batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedChildSourceBatchLimits {
    pub inputs: MakeInputBatchLimits,
    /// Bound for all returned variable/file-program retained-data estimates
    /// combined. This uses interpreter accounting, not allocator capacities,
    /// and must be in `1..=1 GiB`.
    pub maximum_retained_bytes: usize,
}

const MAX_RETAINED_CHILD_SOURCE_BATCH_BYTES: usize = 1024 * 1024 * 1024;

/// Coordinate-matched, currently consistent source data, NOT execution proof.
///
/// The source input identity and read records remain separate. This does not
/// establish that a generated Makefile came from the corresponding GenMF input,
/// that all preparation phases ran, or that an absent product can be admitted.
pub struct MatchedChildSource {
    coordinates: MatchedOwnerChildCoordinates,
    build_root: PathBuf,
    program: MakeVariableProgram,
    file: MakeFileProgram,
    reads: VerifiedMakeInputReads,
    snapshot: MakeInputSnapshot,
    /// The native original-recipe lane must not import a separate DirVars table.
    default_directory_context: bool,
    /// Present only for children constructed under aggregate batch accounting.
    retained_estimate_bytes: Option<usize>,
}

impl MatchedChildSource {
    #[must_use]
    pub const fn coordinates(&self) -> &MatchedOwnerChildCoordinates {
        &self.coordinates
    }

    #[must_use]
    /// Interpreter-accounted variable/file data for a batch-created child.
    /// This is not allocator capacity or measured process memory. Standalone
    /// children have no aggregate accounting sidecar and return `None`.
    pub const fn retained_estimate_bytes(&self) -> Option<usize> {
        self.retained_estimate_bytes
    }

    /// Consume the data without adding producer or native-selection authority.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        MatchedOwnerChildCoordinates,
        MakeVariableProgram,
        MakeFileProgram,
        VerifiedMakeInputReads,
    ) {
        (self.coordinates, self.program, self.file, self.reads)
    }
}

impl NativeOwnerProjection {
    /// Interpret a bounded set of retained child inputs through one shared-root
    /// batch transaction.
    ///
    /// Every owner is matched once against this projection before parsing. The
    /// child observations remain caller-authenticated data: this method does not
    /// establish phase, parent, executable, receipt, completeness or native
    /// scheduling semantics. Each child receives a cwd-specific reader view;
    /// relative includes resolve directly from that cwd, while any missing
    /// relative include still requires a separately bound include search. No
    /// directory-variable fallback fills omitted native values. Source
    /// projection verification and whole-root verification cover the complete
    /// parser operation, including failures, without one projection scan per
    /// child. Program-state and file-event estimates share one checked aggregate
    /// budget before each child is added to the private result vector.
    ///
    /// # Errors
    /// Refuses empty/oversized batches, path or coordinate mismatches, control
    /// refusal, changed roots, invalid native packets and unsupported source.
    pub fn read_retained_child_sources(
        &self,
        roots: &[String],
        requests: &[RetainedChildSourceBatchRequest<'_, '_>],
        snapshot: &MakeInputSnapshot,
        limits: RetainedChildSourceBatchLimits,
        check_control: impl FnMut() -> Result<(), String>,
    ) -> Result<Vec<MatchedChildSource>, String> {
        if limits.maximum_retained_bytes == 0
            || limits.maximum_retained_bytes > MAX_RETAINED_CHILD_SOURCE_BATCH_BYTES
        {
            return Err(
                "retained child source batch retained-data limit is outside 1..=1 GiB".into(),
            );
        }
        self.with_retained_child_source_batch(roots, requests, snapshot, limits.inputs, check_control, |batch, coordinates| {
            let mut matched = Vec::with_capacity(requests.len());
            let mut retained_total = 0usize;
            for (index, (request, coordinates)) in requests.iter().zip(coordinates).enumerate() {
                let (child, variable_retained, file_retained) = read_retained_child_in_batch(batch, index, request, coordinates)?;
                let child_retained = variable_retained
                    .checked_add(file_retained)
                    .ok_or("retained child source size overflow")?;
                let next_retained = retained_total
                    .checked_add(child_retained)
                    .ok_or("retained child source batch size overflow")?;
                if next_retained > limits.maximum_retained_bytes {
                    // Only checked scalar accounting enters this diagnostic;
                    // no owner names, Source text, argv or environment values.
                    return Err(format!(
                        "retained child source batch at index {index}: retained-data limit exceeded (prior={retained_total}, variable={variable_retained}, file={file_retained}, next={next_retained}, limit={})",
                        limits.maximum_retained_bytes,
                    ));
                }
                retained_total = next_retained;
                matched.push(child);
            }
            Ok(matched)
        })
    }

    /// Read one digest-selected retained observation through the native reader.
    ///
    /// The portable packet does not choose roots, owner or input snapshots and
    /// does not turn its context/phase labels into execution authentication.
    /// The caller retains the observation's binding when associating the result
    /// with a producer; this helper neither infers a phase nor proves equivalence
    /// between phases. The interpreter consumes the supplied initial state only.
    ///
    /// # Errors
    /// Refuses every coordinate, input or source-interpretation inconsistency
    /// described by `read_captured_child_source`.
    pub fn read_retained_child_source(
        &self,
        paths: &MakeProgramPaths<'_>,
        roots: &[String],
        generated_owner: &str,
        observation: &RetainedMakeObservation,
        snapshot: &MakeInputSnapshot,
    ) -> Result<MatchedChildSource, String> {
        self.read_captured_child_source(
            paths,
            CapturedChildSourceRequest {
                roots,
                generated_owner,
                invocation: observation.invocation(),
                initial_stdout: observation.initial_stdout(),
                environment_names: observation.environment_names(),
                snapshot,
            },
        )
    }

    /// Interpret source only after the native child's declared coordinates match.
    ///
    /// Inputs come from its original admitted file arguments in order. Includes,
    /// namespace/default/CLI/environment values and effects use the existing
    /// captured reader; no owner/path/CPU/value table fills missing state. The
    /// native source mode disables legacy DirVars projection/fallbacks; a
    /// caller's directory aliases cannot fill omitted measured variables.
    /// independent source/build/cwd paths must agree with the selected owner.
    /// Source projection and captured roots are revalidated before returning.
    /// This is data consistency, not parent/phase/receipt authentication.
    ///
    /// # Errors
    /// Refuses foreign coordinates before source evaluation, incomplete native
    /// packets, unsafe/changed inputs, unknown semantics and existing budgets.
    pub fn read_captured_child_source(
        &self,
        paths: &MakeProgramPaths<'_>,
        request: CapturedChildSourceRequest<'_>,
    ) -> Result<MatchedChildSource, String> {
        let relative = Path::new(request.generated_owner)
            .parent()
            .ok_or("child owner has no source directory")?;
        if paths.relative_dir.as_os_str() != relative.as_os_str()
            || paths.working_directory.as_os_str()
                != request.invocation.working_directory().as_os_str()
        {
            return Err("child source interpreter paths differ from selected coordinates".into());
        }
        let coordinates = self.match_captured_child_coordinates(
            paths.source_root,
            request.roots,
            paths.top_directory,
            request.generated_owner,
            request.invocation,
        )?;
        let (program, file, reads) = MakeVariableProgram::read_native_captured_inputs(
            paths,
            request.initial_stdout,
            request.invocation,
            request.environment_names,
            request.snapshot,
        )?;
        self.verify(paths.source_root)
            .map_err(|_| "child source projection changed during interpretation")?;
        Ok(MatchedChildSource {
            build_root: paths.top_directory.to_owned(),
            coordinates,
            program,
            file,
            reads,
            snapshot: request.snapshot.clone(),
            default_directory_context: paths.dirs == &crate::dirs::DirVars::default(),
            retained_estimate_bytes: None,
        })
    }
}
