//! Supervised no-child Root recorder, retaining the original live owner.
//! The selected frontend implementation is an explicit caller trust premise,
//! not inferred from its filename/hash. This is not an execution sandbox.
use super::{
    failure, final_snapshot, measure_tree_content_cas, sha256_bytes, CancellationToken,
    ContractError, MakeInputSnapshot, MesaNativeDispatchSelection, MesaNativeHostBuildSelection,
    MesaNativeRootQuerySelection, MesaPreflightRequest, NativeMakeInvocation,
    ObservedFencedMesaNativeDispatch, OsStr, TreeContentCas,
};
use crate::compatibility::mesa_preflight::{run_prepared_observe, Stage};
use aros_transpiler::root_dispatch_capture::CapturedRootDispatch;

const FRESH_FILES: [&str; 4] = [
    "pending-command.json",
    "recorder.stdout",
    "recorder.stderr",
    "recorder.status.json",
];

/// Successful supervised recorder data, not original-child environment
/// equivalence, a MetaMake launch permission or Mesa/SDK qualification.
pub struct RecordedMesaNativeDispatch {
    owner: ObservedFencedMesaNativeDispatch,
    snapshot: MakeInputSnapshot,
    evidence_tree: TreeContentCas,
    captured: CapturedRootDispatch,
    stage: Stage,
}

impl RecordedMesaNativeDispatch {
    pub(crate) const fn launch_inputs(&self) -> &super::super::ObservedMesaNativeLaunchInputs {
        &self.owner.candidate.launch
    }

    pub(crate) fn evidence_root(&self) -> &std::path::Path {
        self.owner.candidate.directory.path()
    }

    /// Only for a consuming phase which independently checks the exact fresh
    /// output transition against the original post-host tree. All old reads
    /// and recorder evidence remain fixed; the old owner is never recreated.
    pub(crate) fn revalidate_extended_output(
        &self,
        request: &MesaPreflightRequest<'_>,
        origin: &crate::compatibility::MesaNativeDirectOrigin<'_, '_>,
        output: &TreeContentCas,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        let candidate = &self.owner.candidate;
        candidate.launch.revalidate_inputs(
            request,
            origin.host_query,
            origin.host_build,
            origin.launch_query,
            cancellation,
        )?;
        if super::super::selection_identity(origin.dispatch)? != candidate.selection_sha256 {
            return Err(failure(
                "extended recorder selected different dispatch inputs",
            ));
        }
        super::super::check_observer(origin.dispatch)?;
        candidate.validate_original_shell_environment()?;
        candidate.context.revalidate()?;
        self.owner.context.revalidate()?;
        self.stage.invocation_context.revalidate()?;
        candidate
            .directory
            .revalidate()
            .map_err(|_| failure("extended recorder evidence directory changed"))?;
        let measured = measure_tree_content_cas(candidate.directory.path())
            .map_err(|_| failure("cannot remeasure extended recorder evidence"))?;
        if measured != self.evidence_tree {
            return Err(failure("extended recorder evidence changed"));
        }
        let controls = self
            .owner
            .source
            .source()
            .recipe_controls()
            .ok_or_else(|| failure("extended recorder lost its recipe controls"))?;
        let crate::compatibility::CompatibilityEnvironment::SealedHostTools { host_tools, .. } =
            request.environment
        else {
            return Err(failure("extended recorder lost its sealed host roles"));
        };
        self.owner.shell.revalidate(
            controls.shell(),
            controls.flags(),
            host_tools,
            self.owner.observation.invocation().environment(),
            self.owner.source.projection().source_exports(),
        )?;
        // The consuming owner already checked this exact output transition.
        // Retain the original Source/compiler/evidence baselines rather than
        // recapturing them, and keep both full-root read-replay boundaries.
        self.snapshot
            .verify_read_groups_with_complete_root_transitions(
                &[(request.output_root, output)],
                &self.owner.source.read_groups(),
            )
            .map_err(|_| failure("extended recorder Source or generated read closure changed"))?;
        candidate.launch.host.root.prepared.remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure("extended recorder revalidation cancelled"));
        }
        Ok(())
    }

    #[must_use]
    pub const fn captured(&self) -> &CapturedRootDispatch {
        &self.captured
    }

    /// Recheck the original clock/inputs, exact capture transition and all old
    /// Source/generated input reads in the explicitly extended evidence root.
    /// # Errors
    /// Refuses drift, reselection, cancellation, expired budget or changed data.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeDispatchSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.owner.revalidate_inputs(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        self.stage.invocation_context.revalidate()?;
        let measured = measure_tree_content_cas(self.owner.candidate.directory.path())
            .map_err(|_| failure("cannot remeasure Root recorder evidence"))?;
        if measured != self.evidence_tree
            || !self
                .owner
                .evidence_tree
                .matches_fresh_outputs(&measured, &FRESH_FILES, &[])
        {
            return Err(failure("Root recorder evidence transition changed"));
        }
        self.owner
            .source
            .verify_read_closure(&self.snapshot)
            .map_err(|_| failure("Root recorder Source or generated read closure changed"))?;
        self.owner.candidate.launch.host.root.prepared.remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure("Root recorder revalidation cancelled"));
        }
        Ok(())
    }
}

impl ObservedFencedMesaNativeDispatch {
    /// Execute the exact fully observed fenced Make invocation. Its only
    /// admitted recipe is the pinned no-child capture frontend. Do not replace
    /// this with a reconstructed child call or renew the preparation deadline.
    /// # Errors
    /// Refuses old evidence, failed/late/cancelled process, changed inputs,
    /// unexpected writes, malformed capture or mismatched selected child data.
    pub fn capture_recorder(
        self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeDispatchSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<RecordedMesaNativeDispatch, ContractError> {
        if !self.source.source().shell_effect_policy_bound() {
            return Err(failure(
                "Root recorder lacks per-effect Source shell control binding",
            ));
        }
        self.revalidate(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        let (stage, exit_code) = run_prepared_observe(
            (
                request,
                cancellation,
                self.candidate.launch.host.root.prepared.configured.started,
            ),
            "root-dispatch-recorder",
            self.context.clone(),
            0,
            |stdout, stderr| {
                self.candidate
                    .directory
                    .write_new(OsStr::new("recorder.stdout"), stdout)
                    .map_err(|_| failure("cannot retain Root recorder stdout"))?;
                self.candidate
                    .directory
                    .write_new(OsStr::new("recorder.stderr"), stderr)
                    .map_err(|_| failure("cannot retain Root recorder stderr"))
            },
        )?;
        let status = crate::canonical::bytes(&serde_json::json!({
            "exit_code":exit_code,"program":stage.program,"invocation":stage.invocation,
            "environment":stage.environment,"stdout":stage.stdout,"stderr":stage.stderr,
            "initial_context":self.observation.binding().context_sha256,
        }))?;
        self.candidate
            .directory
            .write_new(OsStr::new("recorder.status.json"), &status)
            .map_err(|_| failure("cannot retain Root recorder status"))?;
        self.revalidate_inputs(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        let evidence_tree = measure_tree_content_cas(self.candidate.directory.path())
            .map_err(|_| failure("cannot measure Root recorder evidence"))?;
        if !self
            .evidence_tree
            .matches_fresh_outputs(&evidence_tree, &FRESH_FILES, &[])
        {
            return Err(failure(
                "Root recorder wrote outside the exact fresh capture selection",
            ));
        }
        let digest = evidence_tree
            .regular_file_digest("pending-command.json")
            .ok_or_else(|| failure("Root recorder has no unique regular capture"))?;
        let bytes = self
            .candidate
            .directory
            .read_selected(OsStr::new("pending-command.json"), digest, 2 * 1024 * 1024)
            .map_err(|_| failure("cannot read pinned Root recorder capture"))?;
        if sha256_bytes(&bytes) != *digest {
            return Err(failure("Root recorder capture bytes changed"));
        }
        let raw = NativeMakeInvocation::parse(&bytes)
            .map_err(|_| failure("Root recorder capture is not bounded invocation data"))?;
        let captured = self
            .source
            .source()
            .capture_plan()
            .bind_capture_with_source_exports(
                raw,
                self.candidate
                    .source_plan
                    .original_recipe()
                    .ok_or_else(|| {
                        failure("Root recorder lost its original Source export projection")
                    })?,
                self.source.projection(),
            )
            .map_err(|_| failure("Root recorder child data or changed Source exports differ"))?;
        let snapshot = final_snapshot(&self.candidate, request, &evidence_tree)?;
        self.source
            .verify_read_closure(&snapshot)
            .map_err(|_| failure("Root recorder changed its verified input closure"))?;
        let result = RecordedMesaNativeDispatch {
            owner: self,
            snapshot,
            evidence_tree,
            captured,
            stage,
        };
        result.revalidate(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        Ok(result)
    }
}
