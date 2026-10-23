//! Consuming, supervised direct MetaMake collection under the original owner.
use super::super::{
    check_home_fallback, read_bytes, selected_outputs, write_new_fallback, CONFIG, FALLBACK,
};
use super::{
    failure, MesaNativeCollectionSelection, MesaNativeDirectOrigin, MesaPreflightRequest,
    PreparedMesaNativeDirectCollection,
};
use crate::compatibility::mesa_preflight::{
    invocation,
    native_collection_observer::{CollectionObserver, CollectionObserverSelection},
    run_prepared_observe, Stage,
};
use crate::ContractError;
use aros_common::{
    directory_entry_names_nofollow_bounded, make_child_command::LiteralMakeChildCommand,
    make_native_invocation::PrivateNativeCaptureDirectory,
    make_native_observation::RetainedMakeObservation, measure_tree_content_cas, sha256_bytes,
    CancellationToken, Sha256Digest, TreeContentCas,
};
use aros_transpiler::{
    metamake_project::MetaMakeLauncherConfiguration,
    native_owner_projection::MatchedOwnerChildCoordinates,
};
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::DirBuilderExt as _,
    time::Instant,
};

const STAGE: &str = "native-direct-metamake";

#[path = "mesa_preflight_native_direct_genmodule.rs"]
mod genmodule_inventory;
#[path = "mesa_preflight_native_direct_recipe.rs"]
mod recipe_execution;
#[path = "mesa_preflight_native_direct_reader.rs"]
mod source_reader;
pub use genmodule_inventory::{GeneratedNativeMakeInclude, NativeMakeIncludeExecution};
pub use recipe_execution::{
    ExecutedMesa26OriginalRecipe, Mesa26GeneratedPathListSelection, Mesa26OriginalRecipeExecution,
};

/// Completed instrumented MetaMake with independently matched Source owners.
///
/// The observer stops before leaf Source parsing. This is not ordinary Root
/// equivalence, successful leaf recipes, generated ABI parity or SDK proof.
pub struct CollectedMesaNativeDirect {
    prepared: PreparedMesaNativeDirectCollection,
    directory: PrivateNativeCaptureDirectory,
    observer: CollectionObserver,
    output_tree: TreeContentCas,
    evidence_tree: TreeContentCas,
    fresh_files: Vec<String>,
    derived_configuration_sha256: Sha256Digest,
    stage: Stage,
    calls: Vec<RetainedMakeObservation>,
    coordinates: Vec<MatchedOwnerChildCoordinates>,
}

impl CollectedMesaNativeDirect {
    #[must_use]
    pub fn calls(&self) -> &[RetainedMakeObservation] {
        &self.calls
    }

    #[must_use]
    pub fn coordinates(&self) -> &[MatchedOwnerChildCoordinates] {
        &self.coordinates
    }

    #[must_use]
    pub const fn preparation(&self) -> &PreparedMesaNativeDirectCollection {
        &self.prepared
    }

    /// Recheck the exact allowed transition, old recorder/input closure and
    /// all retained observer packets without renewing the original clock.
    /// # Errors
    /// Refuses drift, foreign selection, stale/missing calls, expiry or cancel.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        origin: &MesaNativeDirectOrigin<'_, '_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.revalidate_output(request, origin, selected, &self.output_tree, cancellation)
    }

    fn revalidate_output(
        &self,
        request: &MesaPreflightRequest<'_>,
        origin: &MesaNativeDirectOrigin<'_, '_>,
        selected: &MesaNativeCollectionSelection<'_>,
        current_output: &TreeContentCas,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.prepared.revalidate_policy(request, selected)?;
        let launch = self.prepared.recorded.launch_inputs();
        let files: Vec<_> = self.fresh_files.iter().map(String::as_str).collect();
        if !launch
            .host
            .output_tree
            .matches_fresh_outputs(&self.output_tree, &files, &[])
            || measure_tree_content_cas(request.output_root)
                .map_err(|_| failure("cannot remeasure direct native output"))?
                != *current_output
        {
            return Err(failure("direct native output transition changed"));
        }
        self.prepared.recorded.revalidate_extended_output(
            request,
            origin,
            current_output,
            cancellation,
        )?;
        let build = self.prepared.invocation.working_directory();
        if sha256_bytes(&read_bytes(&build.join(FALLBACK))?) != self.derived_configuration_sha256 {
            return Err(failure("direct native fallback configuration changed"));
        }
        check_home_fallback(self.stage.invocation_context.environment())?;
        self.stage.invocation_context.revalidate()?;
        self.observer.revalidate()?;
        check_evidence(&self.directory)?;
        if measure_tree_content_cas(self.directory.path())
            .map_err(|_| failure("cannot remeasure direct native evidence"))?
            != self.evidence_tree
        {
            return Err(failure("direct native evidence changed"));
        }
        let calls =
            self.observer
                .read_after_reaped(build, deadline(&self.prepared)?, cancellation)?;
        if !same_calls(&calls, &self.calls) {
            return Err(failure("direct native retained calls changed"));
        }
        validate_call_runtime(&self.prepared, request, origin, &calls, cancellation)?;
        launch.host.root.prepared.remaining()?;
        Ok(())
    }
}

impl PreparedMesaNativeDirectCollection {
    /// Consume the checked entry and start only its originally built MetaMake.
    ///
    /// The configured project is instrumented to use the pinned first-input
    /// observer; the default project cannot launch ordinary Make. Trusted
    /// Source MetaMake/GenMF can still run regeneration, so this is not a
    /// sandbox. All original inputs must remain exclusively owned.
    /// # Errors
    /// Refuses unsafe runtime selection, preexisting output, changed inputs,
    /// failed/late children, unexpected writes or incomplete owner coverage.
    pub fn collect(
        self,
        request: &MesaPreflightRequest<'_>,
        origin: &MesaNativeDirectOrigin<'_, '_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<CollectedMesaNativeDirect, ContractError> {
        self.revalidate(request, origin, selected, cancellation)?;
        let launch = self.recorded.launch_inputs();
        let parent = launch.host.root.prepared.configured_invocation()?;
        let build = self.invocation.working_directory();
        let fresh_files = selected_outputs(
            &launch.host.output_tree,
            request.output_root,
            build,
            &self.policy.generated_files,
        )?;
        let mut environment = parent.environment().clone();
        environment.variables = self.invocation.environment().clone();
        check_home_fallback(&environment)?;
        let original = read_bytes(&build.join(CONFIG))?;
        let make_word = LiteralMakeChildCommand::parse(
            launch
                .values()
                .get("HOST_MAKE")
                .ok_or_else(|| failure("direct native entry lacks original host Make"))?,
        )
        .map_err(|_| failure("direct native host Make is not literal"))?;
        if !make_word.arguments().is_empty() || !make_word.environment_assignments().is_empty() {
            return Err(failure(
                "direct native host Make has unsupported extra arguments",
            ));
        }
        let make = which::which_in(
            make_word.program(),
            environment.variables.get("PATH"),
            build,
        )
        .map_err(|_| failure("cannot locate direct native observer Make"))?;
        let make = fs::canonicalize(make)
            .map_err(|_| failure("cannot resolve direct native observer Make"))?;
        // Fresh private writes occur only after all original input selections.
        launch.host.root.prepared.remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure(
                "direct native collection cancelled before preparation",
            ));
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(selected.output_root)
            .map_err(|_| failure("cannot create fresh direct native evidence"))?;
        let directory = PrivateNativeCaptureDirectory::open(selected.output_root)
            .map_err(|_| failure("cannot hold direct native evidence"))?;
        let observer = CollectionObserver::prepare_with_recipe_controls(
            selected.output_root,
            &CollectionObserverSelection {
                observer: selected.observer,
                observer_sha256: selected.observer_sha256,
                program: &make,
                program_sha256: parent.program_sha256(),
                argument_zero: make_word.program(),
                maximum_calls: selected.maximum_calls,
            },
            &serde_json::json!({
                "qualification":"direct-metamake-first-input-data-not-leaf-or-sdk",
                "selection":self.selection_sha256, "roots":self.roots,
                "source_metadata":self.policy.metadata_selection,
                "recorded_invocation":sha256_bytes(&self.recorded.captured().raw().to_bytes()
                    .map_err(|_| failure("cannot encode direct native recorder binding"))?),
            }),
        )?;
        let instrumented = MetaMakeLauncherConfiguration::derive_confined_project(
            &original,
            &self.configuration_sha256,
            selected.project,
            selected.source_command_word,
            &observer.launcher(),
        )
        .map_err(|_| failure("cannot instrument direct native configured project"))?;
        directory
            .write_new(OsStr::new(CONFIG), instrumented.bytes())
            .map_err(|_| failure("cannot retain direct native configuration"))?;
        environment.variables.insert(
            "MMAKE_CONFIG".into(),
            directory
                .path()
                .join(CONFIG)
                .to_str()
                .ok_or_else(|| failure("direct native configuration path is not UTF-8"))?
                .to_owned(),
        );
        let program = self
            .invocation
            .argument_zero()
            .ok_or_else(|| failure("direct native entry lost its selected program"))?;
        let context = invocation::InvocationContext::new(
            std::path::Path::new(program),
            self.invocation.arguments(),
            build,
            environment,
        )?;
        if context.program_sha256() != launch.host.executable_sha256() {
            return Err(failure("direct native MetaMake executable changed"));
        }
        self.revalidate(request, origin, selected, cancellation)?;
        observer.revalidate()?;
        launch.host.root.prepared.remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure("direct native collection cancelled before launch"));
        }
        write_new_fallback(&build.join(FALLBACK), instrumented.bytes())?;
        let (stage, _) = run_prepared_observe(
            (
                request,
                cancellation,
                launch.host.root.prepared.configured.started,
            ),
            STAGE,
            context,
            0,
            |stdout, stderr| {
                directory
                    .write_new(OsStr::new(&format!("{STAGE}.stdout")), stdout)
                    .map_err(|_| failure("cannot retain direct native stdout"))?;
                directory
                    .write_new(OsStr::new(&format!("{STAGE}.stderr")), stderr)
                    .map_err(|_| failure("cannot retain direct native stderr"))
            },
        )?;
        let output_tree = measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot measure direct native output transition"))?;
        let files: Vec<_> = fresh_files.iter().map(String::as_str).collect();
        if !launch
            .host
            .output_tree
            .matches_fresh_outputs(&output_tree, &files, &[])
        {
            return Err(failure(
                "direct native wrote outside its exact fresh outputs",
            ));
        }
        self.recorded
            .revalidate_extended_output(request, origin, &output_tree, cancellation)?;
        let calls = observer.read_after_reaped(build, deadline(&self)?, cancellation)?;
        validate_call_runtime(&self, request, origin, &calls, cancellation)?;
        let coordinates = match_coordinates(&self, build, &calls)?;
        let status = crate::canonical::bytes(&serde_json::json!({
            "selection":self.selection_sha256, "stage":stage, "calls":calls.len(),
        }))?;
        directory
            .write_new(OsStr::new("direct.status.json"), &status)
            .map_err(|_| failure("cannot retain direct native status"))?;
        check_evidence(&directory)?;
        let evidence_tree = measure_tree_content_cas(directory.path())
            .map_err(|_| failure("cannot measure direct native evidence"))?;
        let result = CollectedMesaNativeDirect {
            prepared: self,
            directory,
            observer,
            output_tree,
            evidence_tree,
            fresh_files,
            derived_configuration_sha256: instrumented.derived_sha256().clone(),
            stage,
            calls,
            coordinates,
        };
        result.revalidate(request, origin, selected, cancellation)?;
        Ok(result)
    }
}

fn validate_call_runtime(
    prepared: &PreparedMesaNativeDirectCollection,
    request: &MesaPreflightRequest<'_>,
    origin: &MesaNativeDirectOrigin<'_, '_>,
    calls: &[RetainedMakeObservation],
    cancellation: &CancellationToken,
) -> Result<(), ContractError> {
    let launch = prepared.recorded.launch_inputs();
    let prefix = launch
        .values()
        .get(origin.dispatch.prefix_variable)
        .ok_or_else(|| failure("direct native calls lost selected Source prefix"))?;
    let prefix = aros_common::make_environment_prefix::LiteralMakeEnvironmentPrefix::parse(prefix)
        .map_err(|_| failure("direct native call prefix is unsupported"))?;
    // Every call still supplies its own complete environment and independently
    // digest-bound ordered globals. Only identical Source-projection checks
    // move to the enclosing before/after transaction; no call is omitted.
    prepared
        .policy
        .projection
        .with_runtime_configuration_checks(request.source_root, |check| {
            for call in calls {
                launch
                    .host
                    .root
                    .prepared
                    .remaining()
                    .map_err(|error| error.to_string())?;
                if cancellation.is_cancelled() {
                    return Err("direct native runtime validation cancelled".into());
                }
                let environment = call.invocation().environment();
                if prefix
                    .assignments()
                    .iter()
                    .any(|(name, value)| environment.get(name) != Some(value))
                {
                    return Err("direct native child changed Source prefix environment".into());
                }
                let globals = super::read_global_inputs(
                    &prepared.policy.project,
                    environment.clone(),
                    &launch.query.read_inputs,
                    prepared.invocation.working_directory(),
                    request.source_root,
                )
                .map_err(|error| error.to_string())?;
                check.compare(&prepared.policy.project, &globals)?;
            }
            Ok(())
        })
        .map_err(|error| failure(&error))?;
    launch.host.root.prepared.remaining()?;
    if cancellation.is_cancelled() {
        return Err(failure("direct native runtime validation cancelled"));
    }
    Ok(())
}

fn match_coordinates(
    prepared: &PreparedMesaNativeDirectCollection,
    build: &std::path::Path,
    calls: &[RetainedMakeObservation],
) -> Result<Vec<MatchedOwnerChildCoordinates>, ContractError> {
    let owners = calls
        .iter()
        .map(|call| {
            let relative = call
                .invocation()
                .working_directory()
                .strip_prefix(build)
                .map_err(|_| failure("direct native child escaped its original build"))?;
            let owner = relative
                .join(&prepared.policy.project.default_makefile)
                .to_str()
                .ok_or_else(|| failure("direct native child owner is not UTF-8"))?
                .to_owned();
            Ok((owner, call.invocation()))
        })
        .collect::<Result<Vec<_>, ContractError>>()?;
    let references = owners
        .iter()
        .map(|(owner, call)| (owner.as_str(), *call))
        .collect::<Vec<_>>();
    prepared
        .policy
        .projection
        .match_observed_owner_coordinates(
            prepared.profile.source_root(),
            &prepared.roots,
            build,
            &references,
        )
        .map_err(|_| {
            failure("direct native child coordinate coverage differs from selected Source")
        })
}

fn same_calls(left: &[RetainedMakeObservation], right: &[RetainedMakeObservation]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a.binding() == b.binding()
                && a.invocation().to_bytes().ok() == b.invocation().to_bytes().ok()
                && a.initial_stdout() == b.initial_stdout()
        })
}

fn deadline(prepared: &PreparedMesaNativeDirectCollection) -> Result<Instant, ContractError> {
    Instant::now()
        .checked_add(
            prepared
                .recorded
                .launch_inputs()
                .host
                .root
                .prepared
                .remaining()?,
        )
        .ok_or_else(|| failure("direct native deadline overflow"))
}

fn check_evidence(directory: &PrivateNativeCaptureDirectory) -> Result<(), ContractError> {
    let expected = [
        "c",
        "d",
        "o",
        "native-context.json",
        CONFIG,
        "direct.status.json",
        "native-direct-metamake.stdout",
        "native-direct-metamake.stderr",
    ]
    .map(OsString::from)
    .into_iter()
    .collect::<BTreeSet<_>>();
    let actual = directory_entry_names_nofollow_bounded(directory.path(), expected.len())
        .map_err(|_| failure("direct native evidence namespace exceeds selection"))?
        .into_iter()
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(failure("direct native evidence namespace changed"));
    }
    directory
        .revalidate()
        .map_err(|_| failure("direct native evidence directory changed"))
}
