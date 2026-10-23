//! Fresh full-argv observation and ordered Source replay; no recorder execution.
use super::{
    failure, invocation, measure_tree_content_cas, observe_first_input_make_with_recipe_controls,
    sha256_bytes, CancellationToken, ContractError, DirVars, InitialMakeObservationRequest,
    MakeInputRootSelection, MakeInputSnapshot, MakeObservationBinding, MakeObservationPhase,
    MakeProgramPaths, MakeVariableProgram, MesaNativeDispatchSelection,
    MesaNativeHostBuildSelection, MesaNativeRootQuerySelection, MesaPreflightRequest,
    NativeMakeInvocation, NativeMakeObservationStage, ObservedMesaNativeDispatch, OsStr, Path,
    RetainedMakeObservation, RootDispatchCapturePlan, RootDispatchCaptureSelection, TreeContentCas,
};
use aros_transpiler::root_dispatch_capture::{
    FencedRootDispatchPlan, FencedRootDispatchRequest, RootDispatchFencedFiles,
};

#[path = "mesa_preflight_native_dispatch_recorder.rs"]
mod recorder;
pub use recorder::RecordedMesaNativeDispatch;

const FRESH_FILES: [&str; 10] = [
    "default-fence.mk",
    "fenced.context.json",
    "fenced.invocation.json",
    "fenced.observation.json",
    "fenced.discovery.stdout",
    "fenced.discovery.stderr",
    "fenced.discovery.status.json",
    "fenced.initial.stdout",
    "fenced.initial.stderr",
    "fenced.initial.status.json",
];

/// Second observation still owned by the original live preparation and clock.
/// No ordinary Root recipe, shell/recorder launch or SDK proof is produced.
pub struct ObservedFencedMesaNativeDispatch {
    candidate: ObservedMesaNativeDispatch,
    context: invocation::InvocationContext,
    observation: RetainedMakeObservation,
    source: FencedRootDispatchPlan,
    evidence_tree: TreeContentCas,
    shell: crate::compatibility::recipe_shell::BoundRecipeShell,
}

impl ObservedFencedMesaNativeDispatch {
    #[must_use]
    pub const fn initial_observation(&self) -> &RetainedMakeObservation {
        &self.observation
    }
    #[must_use]
    pub const fn source_plan(&self) -> &FencedRootDispatchPlan {
        &self.source
    }

    /// Recheck original owners plus the exact ten-file fresh evidence transition.
    /// # Errors
    /// Refuses changed old/new evidence, input/selection drift, expiry or cancel.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeDispatchSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.candidate.revalidate_non_snapshot_inputs(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        self.revalidate_shell(request, cancellation)?;
        self.source
            .source()
            .revalidate_with(&self.candidate.source_plan)
            .map_err(|_| failure("fenced dispatch Source or instrumentation changed"))?;
        let measured = measure_tree_content_cas(self.candidate.directory.path())
            .map_err(|_| failure("cannot remeasure fenced dispatch evidence"))?;
        if measured != self.evidence_tree
            || !self
                .candidate
                .evidence_tree
                .matches_fresh_outputs(&measured, &FRESH_FILES, &[])
        {
            return Err(failure("fenced dispatch evidence transition changed"));
        }
        self.candidate.launch.host.root.prepared.remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure("fenced dispatch revalidation cancelled"));
        }
        Ok(())
    }

    fn revalidate_inputs(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeDispatchSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.candidate.revalidate_inputs(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        self.revalidate_shell(request, cancellation)
    }

    fn revalidate_shell(
        &self,
        request: &MesaPreflightRequest<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.candidate.validate_original_shell_environment()?;
        self.context.revalidate()?;
        let controls = self
            .source
            .source()
            .recipe_controls()
            .ok_or_else(|| failure("fenced dispatch lost its recipe controls"))?;
        let crate::compatibility::CompatibilityEnvironment::SealedHostTools { host_tools, .. } =
            request.environment
        else {
            return Err(failure("fenced dispatch lost its sealed host roles"));
        };
        self.shell.revalidate(
            controls.shell(),
            controls.flags(),
            host_tools,
            self.observation.invocation().environment(),
            self.source.projection().source_exports(),
        )?;
        self.candidate.launch.host.root.prepared.remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure("fenced dispatch revalidation cancelled"));
        }
        Ok(())
    }
}

impl ObservedMesaNativeDispatch {
    /// Consume discovery and observe the exact prospective fenced invocation.
    ///
    /// Both native stages stop before Source parsing. Source is then re-read
    /// through the captured interpreter with verified trailing file effects.
    /// No candidate observation or expired budget is promoted to launch proof.
    /// Shell controls are bound to independent host policy before returning;
    /// actual recorder capture is a separate consuming phase.
    ///
    /// # Errors
    /// Refuses old-owner drift, nonfresh evidence, native packet mismatch,
    /// changed Source selection/closure, hooks, expiry and cancellation.
    pub fn observe_fenced(
        self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeDispatchSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<ObservedFencedMesaNativeDispatch, ContractError> {
        self.revalidate(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        let override_path = self.directory.path().join("capture-override.mk");
        let fence_path = self.directory.path().join("default-fence.mk");
        let files = RootDispatchFencedFiles {
            override_input: override_path
                .to_str()
                .ok_or_else(|| failure("fenced override path is not UTF-8"))?,
            fence_input: fence_path
                .to_str()
                .ok_or_else(|| failure("fenced fallback path is not UTF-8"))?,
        };
        let startup = self
            .source_plan
            .prospective_fenced_startup(files, &self.recorder_reads)
            .map_err(|_| failure("cannot derive complete fenced dispatch startup"))?;
        let context = invocation::InvocationContext::new(
            self.context.program(),
            startup.arguments(),
            self.context.working_directory(),
            self.context.environment().clone(),
        )?;
        if context.program_sha256() != self.context.program_sha256() {
            return Err(failure("fenced dispatch changed its original Make program"));
        }
        let invocation = NativeMakeInvocation::new(
            startup.arguments().to_vec(),
            context.working_directory().into(),
            context.environment().variables.clone(),
        )
        .map_err(|_| failure("cannot bind fenced native invocation"))?;
        let invocation_bytes = invocation
            .to_bytes()
            .map_err(|_| failure("cannot encode fenced invocation"))?;
        let context_bytes = crate::canonical::bytes(&serde_json::json!({
            "candidate_context":self.observation.binding().context_sha256,
            "candidate_evidence":self.evidence_tree.payload_digest_excluding(None),
            "selection":self.selection_sha256,"program":context.program_sha256(),
            "invocation":sha256_bytes(&invocation_bytes),
        }))?;
        let context_sha256 = sha256_bytes(&context_bytes);
        self.directory
            .write_new(
                OsStr::new("default-fence.mk"),
                RootDispatchCapturePlan::fallback_fence_bytes(),
            )
            .map_err(|_| failure("cannot retain fresh dispatch fallback fence"))?;
        self.directory
            .write_new(OsStr::new("fenced.context.json"), &context_bytes)
            .map_err(|_| failure("cannot retain fenced context"))?;
        self.directory
            .write_new(OsStr::new("fenced.invocation.json"), &invocation_bytes)
            .map_err(|_| failure("cannot retain fenced invocation"))?;
        let mut initial_stdout_sha256 = None;
        let observation = observe_first_input_make_with_recipe_controls(&InitialMakeObservationRequest {
            program:context.program(), executable_sha256:context.program_sha256(), invocation:&invocation,
            context_sha256:&context_sha256, timeout:self.launch.host.root.prepared.remaining()?,
        }, cancellation, |stage, output| {
            if matches!(stage, NativeMakeObservationStage::Initial) {
                initial_stdout_sha256 = output.stdout.exact_bytes().map(sha256_bytes);
            }
            let stage = match stage { NativeMakeObservationStage::Discovery => "discovery", NativeMakeObservationStage::Initial => "initial" };
            for (suffix, stream) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
                let mut bytes = Vec::new(); stream.write_rendered(&mut bytes).map_err(|_| "cannot encode fenced sampler stream")?;
                self.directory.write_new(OsStr::new(&format!("fenced.{stage}.{suffix}")), &bytes).map_err(|_| "cannot retain fenced sampler stream")?;
            }
            let bytes = serde_json::to_vec(&serde_json::json!({"exit_code":output.status.code(),"elapsed_ms":output.elapsed.as_millis()}))
                .map_err(|_| "cannot encode fenced sampler status")?;
            self.directory.write_new(OsStr::new(&format!("fenced.{stage}.status.json")), &bytes).map_err(|_| "cannot retain fenced sampler status")?;
            Ok(())
        }).map_err(|_| failure("fenced dispatch native initial observation failed"))?;
        let expected_binding = MakeObservationBinding {
            phase: MakeObservationPhase::FirstInputPrelude,
            context_sha256,
            invocation_sha256: sha256_bytes(&invocation_bytes),
            stdout_sha256: initial_stdout_sha256
                .ok_or_else(|| failure("fenced dispatch initial stdout is incomplete"))?,
        };
        if observation.binding() != &expected_binding || observation.invocation() != &invocation {
            return Err(failure(
                "fenced native packet differs from independently retained binding",
            ));
        }
        self.directory
            .write_new(
                OsStr::new("fenced.observation.json"),
                &observation
                    .to_bytes()
                    .map_err(|_| failure("cannot encode fenced observation"))?,
            )
            .map_err(|_| failure("cannot retain fenced observation"))?;
        self.revalidate_inputs(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        let evidence_tree = measure_tree_content_cas(self.directory.path())
            .map_err(|_| failure("cannot measure fenced evidence"))?;
        if !self
            .evidence_tree
            .matches_fresh_outputs(&evidence_tree, &FRESH_FILES, &[])
        {
            return Err(failure(
                "fenced dispatch wrote outside its exact fresh evidence selection",
            ));
        }
        let snapshot = final_snapshot(&self, request, &evidence_tree)?;
        let dirs = DirVars::default();
        let paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: request.source_root,
            relative_dir: Path::new(""),
            top_directory: context.working_directory(),
            working_directory: context.working_directory(),
        };
        let mut program = MakeVariableProgram::from_native_invocation_observation(
            &paths,
            observation.initial_stdout(),
            observation.invocation(),
            observation.environment_names(),
        )
        .map_err(|_| failure("cannot compose fenced native Source state"))?;
        program
            .require_captured_shell_policy(
                selected
                    .recipe_shell
                    .to_str()
                    .ok_or_else(|| failure("selected recipe shell is not UTF-8"))?,
                &["-c"],
                observation.invocation().environment(),
            )
            .map_err(|_| failure("cannot bind fenced Source shell-effect policy"))?;
        if let crate::compatibility::CompatibilityEnvironment::SealedHostTools {
            host_tools, ..
        } = request.environment
        {
            program
                .bind_captured_shell_source_path_prefixes(std::sync::Arc::new(host_tools.clone()))
                .map_err(|_| failure("cannot bind fenced Source search-prefix policy"))?;
        }
        let root_input = self
            .source_plan
            .startup()
            .input_files()
            .first()
            .ok_or_else(|| failure("candidate Root is absent"))?;
        let value = |name: &str| {
            self.launch
                .values()
                .get(name)
                .map(String::as_str)
                .ok_or_else(|| failure("candidate dispatch role is absent"))
        };
        let [goal] = launch_query.goals else {
            return Err(failure("fenced dispatch lost its sole direct goal"));
        };
        let source = self
            .source_plan
            .rebind_fenced(
                FencedRootDispatchRequest {
                    root_input,
                    selected: RootDispatchCaptureSelection {
                        target_pattern: selected.pattern,
                        goal,
                        prefix_variable: selected.prefix_variable,
                        program_variable: selected.program_variable,
                        options_variable: selected.options_variable,
                        project: selected.project,
                        original_prefix: value(selected.prefix_variable)?,
                        program: value(selected.program_variable)?,
                        options: value(selected.options_variable)?,
                        observer: selected.observer,
                        output: &selected.output_root.join("pending-command.json"),
                        working_directory: paths.working_directory,
                    },
                    files,
                    startup: &startup,
                    candidate_recorder_reads: &self.recorder_reads,
                },
                &mut program,
                &paths,
                &snapshot,
            )
            .map_err(|detail| {
                if self
                    .directory
                    .write_new(OsStr::new("fenced.source-refusal.txt"), detail.as_bytes())
                    .is_err()
                {
                    failure("fenced Source replay failed and refusal could not be retained")
                } else {
                    failure("fenced Source replay failed; private refusal retained")
                }
            })?;
        aros_common::make_observer::validate_recipe_startup_hooks(
            observation.invocation().environment(),
            source.projection().source_exports(),
        )
        .map_err(|_| failure("fenced dispatch has an unmodeled recipe shell startup hook"))?;
        let crate::compatibility::CompatibilityEnvironment::SealedHostTools { host_tools, .. } =
            request.environment
        else {
            return Err(failure("fenced dispatch lost its sealed host roles"));
        };
        let controls = source
            .source()
            .recipe_controls()
            .ok_or_else(|| failure("fenced dispatch lost its recipe controls"))?;
        self.validate_original_shell_environment()?;
        let shell = crate::compatibility::recipe_shell::BoundRecipeShell::bind(
            selected.recipe_shell,
            controls.shell(),
            controls.flags(),
            host_tools,
            observation.invocation().environment(),
            source.projection().source_exports(),
        )?;
        let result = ObservedFencedMesaNativeDispatch {
            candidate: self,
            context,
            observation,
            source,
            evidence_tree,
            shell,
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

fn final_snapshot(
    candidate: &ObservedMesaNativeDispatch,
    request: &MesaPreflightRequest<'_>,
    evidence: &TreeContentCas,
) -> Result<MakeInputSnapshot, ContractError> {
    snapshot_with_output(
        candidate,
        request,
        evidence,
        &candidate.launch.host.output_tree,
    )
}

fn snapshot_with_output(
    candidate: &ObservedMesaNativeDispatch,
    request: &MesaPreflightRequest<'_>,
    evidence: &TreeContentCas,
    output: &TreeContentCas,
) -> Result<MakeInputSnapshot, ContractError> {
    let mut roots = vec![
        MakeInputRootSelection {
            root: request.drivers.compiler_root().into(),
            content_sha256: request.drivers.compiler_tree_sha256().clone(),
        },
        MakeInputRootSelection {
            root: request.output_root.into(),
            content_sha256: output.payload_digest_excluding(None),
        },
        MakeInputRootSelection {
            root: candidate.directory.path().into(),
            content_sha256: evidence.payload_digest_excluding(None),
        },
    ];
    let cwd = candidate.context.working_directory();
    (match request.local_source {
        None => {
            roots.push(MakeInputRootSelection {
                root: request.source_root.into(),
                content_sha256: request.source_tree_sha256.clone(),
            });
            MakeInputSnapshot::capture(cwd, &roots)
        }
        Some(local) => {
            MakeInputSnapshot::capture_with_local_source(cwd, request.source_root, local, &roots)
        }
    })
    .map_err(|_| failure("cannot bind fenced Source/build/evidence snapshot"))
}
