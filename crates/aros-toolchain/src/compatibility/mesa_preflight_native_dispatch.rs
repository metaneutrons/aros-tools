//! Original-owner Root initial observation plus complete captured source selection.
//! This phase never launches an original Root recipe or MetaMake.

use super::super::super::{digest_json, invocation};
use super::{
    failure, MesaNativeHostBuildSelection, MesaNativeRootQuerySelection, MesaPreflightRequest,
    ObservedMesaNativeLaunchInputs,
};
use crate::ContractError;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot, VerifiedMakeInputReads},
    make_native_invocation::{NativeMakeInvocation, PrivateNativeCaptureDirectory},
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    make_native_sampler::{
        observe_first_input_make_with_recipe_controls, InitialMakeObservationRequest,
        NativeMakeObservationStage,
    },
    make_startup_arguments::LiteralMakeStartupArguments,
    measure_tree_content_cas,
    publication::filesystem_paths_overlap,
    sha256_bytes, CancellationToken, Sha256Digest, TreeContentCas,
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::{MakeProgramPaths, MakeRecipeProjection, MakeVariableProgram},
    root_dispatch_capture::{
        RootDispatchCapturePlan, RootDispatchCaptureSelection, SourceCheckedRootDispatchPlan,
    },
};
use std::{ffi::OsStr, fs, os::unix::fs::DirBuilderExt as _, path::Path};

#[path = "mesa_preflight_native_dispatch_fenced.rs"]
mod fenced;
pub use fenced::{ObservedFencedMesaNativeDispatch, RecordedMesaNativeDispatch};

/// Caller-selected dispatch roles and a fresh private initial-observation leaf.
pub struct MesaNativeDispatchSelection<'a> {
    pub output_root: &'a Path,
    /// Explicitly trusted locally built capture frontend. Caller selection
    /// establishes that trust; its digest pins identity, not no-child semantics.
    pub observer: &'a Path,
    pub observer_sha256: &'a Sha256Digest,
    /// Independently selected POSIX-shell invocation spelling, never inferred
    /// from Source controls or replaced by a canonical executable path.
    pub recipe_shell: &'a Path,
    pub pattern: &'a str,
    pub project: &'a str,
    pub prefix_variable: &'a str,
    pub program_variable: &'a str,
    pub options_variable: &'a str,
}

/// Move-only result of a supervised first-input observation and source check.
///
/// Retains the original preparation/host/query owner and its original clock.
/// Not a successful Root command capture, MetaMake run or SDK qualification.
/// A future recorder phase must consume this owner and verify an exact fresh
/// pending-command.json transition; writing it invalidates this phase's tree.
pub struct ObservedMesaNativeDispatch {
    launch: ObservedMesaNativeLaunchInputs,
    directory: PrivateNativeCaptureDirectory,
    evidence_tree: TreeContentCas,
    selection_sha256: Sha256Digest,
    context: invocation::InvocationContext,
    observation: RetainedMakeObservation,
    source_plan: SourceCheckedRootDispatchPlan,
    recorder_projection: MakeRecipeProjection,
    recorder_reads: VerifiedMakeInputReads,
}

impl ObservedMesaNativeDispatch {
    fn validate_original_shell_environment(&self) -> Result<(), ContractError> {
        let original = self
            .source_plan
            .original_recipe()
            .ok_or_else(|| failure("native dispatch lost its original recipe projection"))?;
        aros_common::make_observer::validate_posix_shell_environment(
            self.observation.invocation().environment(),
            original.source_exports(),
        )
        .map_err(|_| {
            failure(
                "original recipe shell has unmodeled startup, option, function or loader controls",
            )
        })
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
        self.revalidate_non_snapshot_inputs(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        self.source_plan
            .revalidate()
            .map_err(|_| failure("native dispatch captured source changed"))
    }

    // The completed fenced owner joins this snapshot with its final snapshot
    // immediately after these checks. No public validation path may omit it.
    fn revalidate_non_snapshot_inputs(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeDispatchSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.launch
            .revalidate(request, host_query, host_build, launch_query, cancellation)?;
        if selection_identity(selected)? != self.selection_sha256 {
            return Err(failure("native dispatch selection changed"));
        }
        check_observer(selected)?;
        self.context.revalidate()?;
        self.directory
            .revalidate()
            .map_err(|_| failure("native dispatch evidence directory changed"))?;
        self.launch.host.root.prepared.remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure("native dispatch revalidation cancelled"));
        }
        Ok(())
    }

    #[must_use]
    pub const fn recorder_projection(&self) -> &MakeRecipeProjection {
        &self.recorder_projection
    }

    #[must_use]
    pub const fn recorder_reads(&self) -> &VerifiedMakeInputReads {
        &self.recorder_reads
    }

    #[must_use]
    pub const fn initial_observation(&self) -> &RetainedMakeObservation {
        &self.observation
    }

    #[must_use]
    pub const fn source_plan(&self) -> &SourceCheckedRootDispatchPlan {
        &self.source_plan
    }

    /// Recheck original owners, selected roles, pinned programs and private evidence.
    ///
    /// # Errors
    /// Refuses expired/reselected/changed inputs. No original recipes run here.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeDispatchSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.launch
            .revalidate(request, host_query, host_build, launch_query, cancellation)?;
        if selection_identity(selected)? != self.selection_sha256 {
            return Err(failure("native dispatch selection changed"));
        }
        check_observer(selected)?;
        self.context.revalidate()?;
        self.source_plan
            .revalidate()
            .map_err(|_| failure("native dispatch captured source changed"))?;
        self.directory
            .revalidate()
            .map_err(|_| failure("native dispatch evidence directory changed"))?;
        if measure_tree_content_cas(self.directory.path())
            .map_err(|_| failure("cannot remeasure native dispatch evidence"))?
            != self.evidence_tree
        {
            return Err(failure("native dispatch evidence changed"));
        }
        self.launch.host.root.prepared.remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure("native dispatch revalidation cancelled"));
        }
        Ok(())
    }
}

/// Observe the selected Root invocation before Source, then verify its source selection.
///
/// Both native observations stop at the first-input prelude. Original argv,
/// argv0, explicit environment and cwd come from this live preparation, not a
/// portable case or reconstructed export roster. No Root target or MetaMake is
/// executed, and no native collection/SDK receipt is produced.
/// This is candidate discovery, not a binding for a later argv containing
/// generated override/fence inputs and old-file options. That final invocation
/// requires its own native observation and ordered input/effect revalidation.
///
/// # Errors
/// Refuses ordinary non-quick goals, input/program/scope drift, expired original
/// budget, failed observation, ambiguous source dispatch or evidence changes.
pub fn observe_mesa_native_dispatch(
    launch: ObservedMesaNativeLaunchInputs,
    request: &MesaPreflightRequest<'_>,
    host_query: &MesaNativeRootQuerySelection<'_>,
    host_build: &MesaNativeHostBuildSelection<'_>,
    launch_query: &MesaNativeRootQuerySelection<'_>,
    selected: &MesaNativeDispatchSelection<'_>,
    cancellation: &CancellationToken,
) -> Result<ObservedMesaNativeDispatch, ContractError> {
    launch.revalidate(request, host_query, host_build, launch_query, cancellation)?;
    let [goal] = launch_query.goals else {
        return Err(failure("native dispatch requires one direct Root goal"));
    };
    if goal.strip_suffix("-quick").is_none_or(str::is_empty) {
        return Err(failure("native dispatch requires a Source quick goal"));
    }
    super::super::super::validate_selection(
        request,
        &MesaNativeRootQuerySelection {
            output_root: selected.output_root,
            variables: launch_query.variables,
            goals: launch_query.goals,
        },
    )?;
    for input in [
        host_query.output_root,
        host_build.output_root,
        launch_query.output_root,
        selected.observer,
    ] {
        if filesystem_paths_overlap(selected.output_root, input)
            .map_err(|_| failure("cannot compare native dispatch evidence scope"))?
        {
            return Err(failure(
                "native dispatch overlaps original inputs or evidence",
            ));
        }
    }
    check_observer(selected)?;
    let parent = launch.host.root.prepared.configured_invocation()?;
    let build = parent.working_directory();
    let root = build.join("Makefile");
    let root_word = root
        .to_str()
        .ok_or_else(|| failure("native dispatch Root path is not UTF-8"))?;
    let root_sha = launch
        .query
        .read_inputs
        .iter()
        .find_map(|(path, digest)| (path == &root).then_some(digest))
        .ok_or_else(|| failure("native dispatch lacks the original queried Root digest"))?;
    let value = |name: &str| {
        launch
            .values()
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| failure("native dispatch lacks a selected role value"))
    };
    let prefix = value(selected.prefix_variable)?;
    let program = value(selected.program_variable)?;
    let options = value(selected.options_variable)?;
    if Path::new(program) != launch.host.executable() {
        return Err(failure(
            "native dispatch program differs from original host producer",
        ));
    }
    let arguments = vec![
        "-s".into(),
        "-r".into(),
        "--no-print-directory".into(),
        "-f".into(),
        root_word.into(),
        goal.clone(),
    ];
    let startup = LiteralMakeStartupArguments::admit(&arguments)
        .map_err(|_| failure("native dispatch startup is unsupported"))?;
    let context = invocation::InvocationContext::new(
        parent.program(),
        &arguments,
        build,
        parent.environment().clone(),
    )?;
    let invocation = NativeMakeInvocation::new(
        arguments,
        build.to_owned(),
        context.environment().variables.clone(),
    )
    .map_err(|_| failure("native dispatch invocation exceeds supported data"))?;
    let selection_sha256 = selection_identity(selected)?;
    let context_bytes = crate::canonical::bytes(&serde_json::json!({
        "selection": selection_sha256, "configuration": launch.host.root.prepared.configured.initial_configuration.identity(),
        "invocation": sha256_bytes(&invocation.to_bytes().map_err(|_| failure("cannot encode native dispatch invocation"))?),
        "program":context.program_sha256(), "host_executable":launch.host.executable_sha256(),
        "launch_query":launch.query.query_selection_sha256,
    }))?;
    let context_sha = sha256_bytes(&context_bytes);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(selected.output_root)
        .map_err(|_| failure("cannot create fresh native dispatch evidence"))?;
    let directory = PrivateNativeCaptureDirectory::open(selected.output_root)
        .map_err(|_| failure("cannot hold native dispatch evidence"))?;
    directory
        .write_new(OsStr::new("context.json"), &context_bytes)
        .map_err(|_| failure("cannot retain native dispatch context"))?;
    directory
        .write_new(
            OsStr::new("invocation.json"),
            &invocation
                .to_bytes()
                .map_err(|_| failure("cannot encode native dispatch invocation"))?,
        )
        .map_err(|_| failure("cannot retain native dispatch invocation"))?;
    let mut initial_stdout_sha = None;
    let observation = observe_first_input_make_with_recipe_controls(&InitialMakeObservationRequest {
        program: context.program(), executable_sha256: context.program_sha256(), invocation: &invocation,
        context_sha256: &context_sha, timeout: launch.host.root.prepared.remaining()?,
    }, cancellation, |stage, output| {
        if matches!(stage, NativeMakeObservationStage::Initial) {
            initial_stdout_sha = output.stdout.exact_bytes().map(sha256_bytes);
        }
        let stage = match stage { NativeMakeObservationStage::Discovery => "discovery", NativeMakeObservationStage::Initial => "initial" };
        for (suffix, stream) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
            let mut bytes = Vec::new(); stream.write_rendered(&mut bytes).map_err(|_| "cannot encode private sampler stream")?;
            directory.write_new(OsStr::new(&format!("{stage}.{suffix}")), &bytes).map_err(|_| "cannot retain private sampler stream")?;
        }
        let status = serde_json::to_vec(&serde_json::json!({"exit_code":output.status.code(), "elapsed_ms":output.elapsed.as_millis()})).map_err(|_| "cannot encode sampler status")?;
        directory.write_new(OsStr::new(&format!("{stage}.status.json")), &status).map_err(|_| "cannot retain sampler status")?;
        Ok(())
    }).map_err(|_| failure("native dispatch initial observation failed"))?;
    let expected_binding = MakeObservationBinding {
        phase: MakeObservationPhase::FirstInputPrelude,
        context_sha256: context_sha,
        invocation_sha256: sha256_bytes(
            &invocation
                .to_bytes()
                .map_err(|_| failure("cannot encode expected dispatch invocation"))?,
        ),
        stdout_sha256: initial_stdout_sha.ok_or_else(|| {
            failure("native dispatch sampler did not retain complete initial stdout")
        })?,
    };
    if observation.binding() != &expected_binding
        || observation
            .invocation()
            .to_bytes()
            .map_err(|_| failure("cannot encode observed dispatch invocation"))?
            != invocation
                .to_bytes()
                .map_err(|_| failure("cannot encode expected dispatch invocation"))?
    {
        return Err(failure(
            "native dispatch sampler binding differs from supervised context",
        ));
    }
    launch.revalidate(request, host_query, host_build, launch_query, cancellation)?;
    directory
        .write_new(
            OsStr::new("observation.json"),
            &observation
                .to_bytes()
                .map_err(|_| failure("cannot encode native dispatch observation"))?,
        )
        .map_err(|_| failure("cannot retain native dispatch observation"))?;
    let output_selection = [
        MakeInputRootSelection {
            root: request.output_root.into(),
            content_sha256: launch.host.output_tree.payload_digest_excluding(None),
        },
        MakeInputRootSelection {
            root: request.drivers.compiler_root().into(),
            content_sha256: request.drivers.compiler_tree_sha256().clone(),
        },
    ];
    let snapshot = request
        .local_source
        .map_or_else(
            || {
                MakeInputSnapshot::capture(
                    build,
                    &[
                        MakeInputRootSelection {
                            root: request.source_root.into(),
                            content_sha256: request.source_tree_sha256.clone(),
                        },
                        output_selection[0].clone(),
                        output_selection[1].clone(),
                    ],
                )
            },
            |local| {
                MakeInputSnapshot::capture_with_local_source(
                    build,
                    request.source_root,
                    local,
                    &output_selection,
                )
            },
        )
        .map_err(|_| {
            failure("native dispatch cannot bind original Source and host output trees")
        })?;
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: request.source_root,
        relative_dir: Path::new(""),
        top_directory: build,
        working_directory: build,
    };
    let mut source = MakeVariableProgram::from_native_invocation_observation(
        &paths,
        observation.initial_stdout(),
        observation.invocation(),
        observation.environment_names(),
    )
    .map_err(|_| failure("native dispatch initial state cannot form a source program"))?;
    source
        .require_captured_shell_policy(
            selected
                .recipe_shell
                .to_str()
                .ok_or_else(|| failure("selected recipe shell is not UTF-8"))?,
            &["-c"],
            observation.invocation().environment(),
        )
        .map_err(|_| failure("cannot bind native dispatch Source shell-effect policy"))?;
    if let crate::compatibility::CompatibilityEnvironment::SealedHostTools { host_tools, .. } =
        request.environment
    {
        source
            .bind_captured_shell_source_path_prefixes(std::sync::Arc::new(host_tools.clone()))
            .map_err(|_| failure("cannot bind native dispatch Source search-prefix policy"))?;
    }
    let (source_plan, recorder_projection, recorder_reads) =
        RootDispatchCapturePlan::derive_captured(
            root_word,
            root_sha,
            RootDispatchCaptureSelection {
                target_pattern: selected.pattern,
                goal,
                prefix_variable: selected.prefix_variable,
                program_variable: selected.program_variable,
                options_variable: selected.options_variable,
                project: selected.project,
                original_prefix: prefix,
                program,
                options,
                observer: selected.observer,
                output: &selected.output_root.join("pending-command.json"),
                working_directory: build,
            },
            &mut source,
            &paths,
            &snapshot,
            &startup,
        )
        .and_then(|plan| plan.with_dispatch_recipe_projections(&mut source, &paths))
        .and_then(|(plan, projection, reads)| {
            let original = plan
                .original_recipe()
                .ok_or("native dispatch lacks its original recipe projection")?;
            aros_common::make_observer::validate_recipe_startup_hooks(
                observation.invocation().environment(),
                original.source_exports(),
            )
            .map_err(|_| "native dispatch original recipe has an unmodeled startup hook")?;
            aros_common::make_observer::validate_recipe_startup_hooks(
                observation.invocation().environment(),
                projection.source_exports(),
            )
            .map_err(|_| "native dispatch has an unmodeled recipe shell startup hook")?;
            Ok((plan, projection, reads))
        })
        .map_err(|detail| {
            if directory
                .write_new(OsStr::new("source-refusal.txt"), detail.as_bytes())
                .is_err()
            {
                failure(
                "native dispatch source selection failed and private refusal could not be retained",
            )
            } else {
                failure("native dispatch source selection failed; private refusal retained")
            }
        })?;
    directory
        .write_new(
            OsStr::new("capture-override.mk"),
            source_plan.capture_plan().override_bytes(),
        )
        .map_err(|_| failure("cannot retain native dispatch override"))?;
    check_evidence_names(directory.path())?;
    let evidence_tree = measure_tree_content_cas(directory.path())
        .map_err(|_| failure("cannot measure native dispatch evidence"))?;
    let result = ObservedMesaNativeDispatch {
        launch,
        directory,
        evidence_tree,
        selection_sha256,
        context,
        observation,
        source_plan,
        recorder_projection,
        recorder_reads,
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

fn selection_identity(
    selected: &MesaNativeDispatchSelection<'_>,
) -> Result<Sha256Digest, ContractError> {
    digest_json(
        &serde_json::json!({"output_root":selected.output_root,"observer":selected.observer,"observer_sha256":selected.observer_sha256,
        "pattern":selected.pattern,"project":selected.project,"prefix":selected.prefix_variable,"program":selected.program_variable,"options":selected.options_variable,
        "recipe_shell":selected.recipe_shell}),
    )
}

fn check_observer(selected: &MesaNativeDispatchSelection<'_>) -> Result<(), ContractError> {
    let executable = crate::compatibility::checked_executable(selected.observer)?;
    if crate::compatibility::measure_executable(&executable)?.0 != *selected.observer_sha256 {
        return Err(failure("native dispatch observer executable changed"));
    }
    Ok(())
}

fn check_evidence_names(root: &Path) -> Result<(), ContractError> {
    let expected = [
        "context.json",
        "invocation.json",
        "observation.json",
        "capture-override.mk",
        "discovery.stdout",
        "discovery.stderr",
        "discovery.status.json",
        "initial.stdout",
        "initial.stderr",
        "initial.status.json",
    ];
    let mut names = std::collections::BTreeSet::new();
    for entry in fs::read_dir(root).map_err(|_| failure("cannot list dispatch evidence"))? {
        let entry = entry.map_err(|_| failure("cannot read dispatch evidence name"))?;
        if names.len() == expected.len()
            || !entry
                .file_type()
                .map_err(|_| failure("cannot inspect dispatch evidence entry"))?
                .is_file()
        {
            return Err(failure(
                "native dispatch evidence entry kind or count changed",
            ));
        }
        names.insert(entry.file_name());
    }
    if names != expected.into_iter().map(std::ffi::OsString::from).collect() {
        return Err(failure(
            "native dispatch evidence contains unexpected entries",
        ));
    }
    Ok(())
}
