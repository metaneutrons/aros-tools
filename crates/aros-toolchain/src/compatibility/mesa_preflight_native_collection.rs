//! Supervised, instrumented Source Root launch, not native graph admission.
//!
//! The caller trusts the selected Source program and exclusively owns its
//! inputs/output paths. This is not an OS sandbox: Source MetaMake's temporary
//! GenMF list may live outside the build tree. The returned calls are observed
//! data; their independent Source-owner/call-set join remains a separate gate.

use super::{
    MesaNativeHostBuildSelection, MesaNativeRootQuerySelection, MesaPreflightRequest,
    ObservedMesaNativeLaunchInputs,
};
use crate::compatibility::mesa_preflight::{
    digest_json, failure, invocation,
    native_collection_observer::{CollectionObserver, CollectionObserverSelection},
    read_bytes, run_prepared_observe, validate_make_path, Stage,
};
use crate::ContractError;
use aros_common::{
    directory_entry_names_nofollow_bounded, make_child_command::LiteralMakeChildCommand,
    make_environment_prefix::LiteralMakeEnvironmentPrefix,
    make_native_invocation::PrivateNativeCaptureDirectory,
    make_native_observation::RetainedMakeObservation,
    make_startup_arguments::LiteralMakeStartupArguments, measure_tree_content_cas,
    mesa_native_make::NATIVE_PREPARATION_STAGES, publication::filesystem_paths_overlap,
    sha256_bytes, CancellationToken, Sha256Digest, TreeContentCas, TreeContentEntryKind,
};
use aros_transpiler::metamake_project::{
    ConfiguredProject, MetaMakeLauncherConfiguration, MetaMakeRegenerationPlan,
    ProjectGlobalPathContext,
};
use aros_transpiler::native_owner_projection::{
    MatchedOwnerChildCoordinates, NativeOwnerProjection,
};
use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs,
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Component, Path, PathBuf},
    time::Instant,
};

const CONFIG: &str = "mmake.config";
const FALLBACK: &str = ".mmake.config";
const CACHE: &str = "mmake.cache";
const FENCE_NAME: &str = "default-fence.mk";
const FENCE: &[u8] =
    b".DEFAULT:\n.DEFAULT:\n\t$(error AROS_NATIVE_COLLECTION_UNEXPECTED_DEFAULT)\n";

#[path = "mesa_preflight_native_direct.rs"]
mod direct;
#[path = "mesa_preflight_native_source_join.rs"]
mod source_join;
pub use direct::{
    GeneratedNativeMakeInclude, NativeMakeIncludeExecution,
    prepare_mesa_native_direct_collection, CollectedMesaNativeDirect, ExecutedMesa26OriginalRecipe,
    Mesa26GeneratedPathListSelection, Mesa26OriginalRecipeExecution, MesaNativeDirectOrigin,
    PreparedMesaNativeDirectCollection,
};
pub use source_join::{
    join_mesa_native_source, MesaNativeSourceSelection, SourceJoinedMesaNativeRoot,
};

/// Explicit observer and configured-project selection; no inferred CPU paths.
pub struct MesaNativeCollectionSelection<'a> {
    /// Fresh short private evidence leaf, disjoint from inputs and prior phases.
    pub output_root: &'a Path,
    /// Exact locally built observer frontend and independently selected bytes.
    pub observer: &'a Path,
    pub observer_sha256: &'a Sha256Digest,
    /// Explicit configured Source project and its leading command token.
    pub project: &'a str,
    pub source_command_word: &'a str,
    /// Upper resource bound, NOT the expected independent Source call count.
    pub maximum_calls: usize,
}

/// Retained supervised collection without native completion or graph authority.
pub struct CollectedMesaNativeRoot {
    launch: ObservedMesaNativeLaunchInputs,
    observer: CollectionObserver,
    directory: PrivateNativeCaptureDirectory,
    selection_sha256: Sha256Digest,
    output_tree: TreeContentCas,
    evidence_tree: TreeContentCas,
    fresh_files: Vec<String>,
    configuration_sha256: Sha256Digest,
    derived_configuration_sha256: Sha256Digest,
    stage: Stage,
    calls: Vec<RetainedMakeObservation>,
}

impl CollectedMesaNativeRoot {
    /// Instrumented runtime data, not an independently complete owner roster.
    #[must_use]
    pub fn calls(&self) -> &[RetainedMakeObservation] {
        &self.calls
    }

    /// Data identity of the supervisor-selected observer context, not authority.
    #[must_use]
    pub const fn observer_context_sha256(&self) -> &Sha256Digest {
        self.observer.context_sha256()
    }

    /// Data identity of the retained observer control, not process authentication.
    #[must_use]
    pub const fn observer_control_sha256(&self) -> &Sha256Digest {
        self.observer.control_sha256()
    }

    /// Recheck original owners, selected transitions and retained collection.
    ///
    /// # Errors
    /// Refuses drift, changed scope, expiry/cancellation or inconsistent data.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.launch.revalidate_inputs(
            request,
            host_query,
            host_build,
            launch_query,
            cancellation,
        )?;
        if selection_identity(selected)? != self.selection_sha256 {
            return Err(failure("native collection selected different inputs"));
        }
        let files: Vec<&str> = self.fresh_files.iter().map(String::as_str).collect();
        if !self
            .launch
            .host
            .output_tree
            .matches_fresh_outputs(&self.output_tree, &files, &[])
            || measure_tree_content_cas(request.output_root)
                .map_err(|_| failure("cannot remeasure native collection output"))?
                != self.output_tree
        {
            return Err(failure("native collection output transition changed"));
        }
        let build = self
            .launch
            .host
            .root
            .prepared
            .configured_invocation()?
            .working_directory();
        if sha256_bytes(&read_bytes(&build.join(CONFIG))?) != self.configuration_sha256
            || sha256_bytes(&read_bytes(&build.join(FALLBACK))?)
                != self.derived_configuration_sha256
        {
            return Err(failure("native collection configuration changed"));
        }
        check_home_fallback(
            self.launch
                .host
                .root
                .prepared
                .configured_invocation()?
                .environment(),
        )?;
        self.stage.invocation_context.revalidate()?;
        self.observer.revalidate()?;
        check_evidence_names(&self.directory)?;
        if measure_tree_content_cas(self.directory.path())
            .map_err(|_| failure("cannot remeasure native collection evidence"))?
            != self.evidence_tree
        {
            return Err(failure("native collection evidence changed"));
        }
        let calls =
            self.observer
                .read_after_reaped(build, deadline(&self.launch)?, cancellation)?;
        if calls.len() != self.calls.len()
            || calls.iter().zip(&self.calls).any(|(left, right)| {
                left.binding() != right.binding()
                    || left.invocation().to_bytes().ok() != right.invocation().to_bytes().ok()
                    || left.initial_stdout() != right.initial_stdout()
            })
        {
            return Err(failure("native collection retained calls changed"));
        }
        self.directory
            .revalidate()
            .map_err(|_| failure("native collection directory changed"))?;
        self.launch.host.root.prepared.remaining()?;
        Ok(())
    }
}

/// Run the original Root quick rule with a Source-derived instrumented project.
///
/// Does not run leaf Source parsing/recipes: the selected observer stops at
/// GNU's first-input boundary. This requires trusted Root/MetaMake Source and
/// exclusive ownership; it is not a universal shell/recipe sandbox. Observed
/// packets do not independently prove the expected Source owners or call set.
///
/// # Errors
/// Refuses stale owners, unsafe scope, preexisting generated outputs, unsupported
/// Source configuration, unexpected writes, failed children or expired budget.
pub fn collect_mesa_native_root(
    launch: ObservedMesaNativeLaunchInputs,
    request: &MesaPreflightRequest<'_>,
    host_query: &MesaNativeRootQuerySelection<'_>,
    host_build: &MesaNativeHostBuildSelection<'_>,
    launch_query: &MesaNativeRootQuerySelection<'_>,
    selected: &MesaNativeCollectionSelection<'_>,
    cancellation: &CancellationToken,
) -> Result<CollectedMesaNativeRoot, ContractError> {
    launch.revalidate(request, host_query, host_build, launch_query, cancellation)?;
    validate_scope(
        &launch,
        request,
        host_query,
        host_build,
        launch_query,
        selected,
    )?;
    let parent = launch.host.root.prepared.configured_invocation()?;
    let build = parent.working_directory();
    if launch.values().get("TOP").map(Path::new) != Some(build)
        || launch.values().get("SRCDIR").map(Path::new) != Some(request.source_root)
        || launch_query
            .goals
            .iter()
            .any(|goal| goal.strip_suffix("-quick").is_none_or(str::is_empty))
    {
        return Err(failure(
            "native collection requires the selected TOP and Source quick goals",
        ));
    }
    check_home_fallback(parent.environment())?;
    let original = read_bytes(&build.join(CONFIG))?;
    let original_sha256 = sha256_bytes(&original);
    let project = ConfiguredProject::parse_only_project(
        std::str::from_utf8(&original)
            .map_err(|_| failure("native project configuration is not UTF-8"))?,
        selected.project,
    )
    .map_err(|_| failure("native collection requires exactly its selected configured project"))?;
    if project.top.is_some() {
        return Err(failure(
            "native collection requires the original implicit Source root",
        ));
    }
    check_native_options(
        launch
            .values()
            .get("MMAKE_OPTIONS")
            .ok_or_else(|| failure("native collection lacks measured MetaMake options"))?,
        request.source_root,
        build,
    )?;
    check_global_inputs(&project, launch.values(), &launch.query.read_inputs, build)?;
    check_optional_fences(launch.values(), host_build, build)?;
    let plan = MetaMakeRegenerationPlan::derive(
        &launch.host.root.source_snapshot,
        &original,
        &original_sha256,
        selected.project,
    )
    .map_err(|_| failure("cannot derive Source native regeneration outputs"))?;
    let fresh_files = selected_outputs(
        &launch.host.output_tree,
        request.output_root,
        build,
        plan.generated_files(),
    )?;
    let make_word = LiteralMakeChildCommand::parse(
        launch
            .values()
            .get("HOST_MAKE")
            .ok_or_else(|| failure("native collection lacks host Make"))?,
    )
    .map_err(|_| failure("native collection host Make is not literal"))?;
    let resolved_make = which::which_in(
        make_word.program(),
        parent.environment().variables.get("PATH"),
        build,
    )
    .map_err(|_| failure("cannot resolve native collection GNU Make"))?;
    let resolved_make = fs::canonicalize(resolved_make)
        .map_err(|_| failure("cannot resolve native collection GNU Make"))?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(selected.output_root)
        .map_err(|_| failure("cannot create fresh native collection evidence"))?;
    let directory = PrivateNativeCaptureDirectory::open(selected.output_root)
        .map_err(|_| failure("cannot hold native collection evidence"))?;
    let mut environment = parent.environment().clone();
    environment.variables.insert(
        "MMAKE_CONFIG".into(),
        path_text(&directory.path().join(CONFIG))?,
    );
    let arguments = root_arguments(
        build,
        directory.path(),
        &launch.query.read_inputs,
        host_build.absent_inputs,
        launch_query.goals,
    )?;
    let context =
        invocation::InvocationContext::new(parent.program(), &arguments, build, environment)?;
    let observer = CollectionObserver::prepare(
        selected.output_root,
        &CollectionObserverSelection {
            observer: selected.observer,
            observer_sha256: selected.observer_sha256,
            program: &resolved_make,
            program_sha256: parent.program_sha256(),
            argument_zero: make_word.program(),
            maximum_calls: selected.maximum_calls,
        },
        &serde_json::json!({
            "qualification":"supervised-collection-data-not-owner-authentication",
            "source":launch.host.root.configuration().identity(),
            "host_sha256":launch.host.executable_sha256,
            "launch_query":launch.query.query_selection_sha256,
            "parent_invocation":context.invocation_sha256(),
            "parent_environment":context.environment_sha256(),
            "original_configuration":original_sha256,
            "selection":selection_identity(selected)?,
        }),
    )?;
    let instrumented = MetaMakeLauncherConfiguration::derive_confined_project(
        &original,
        &original_sha256,
        selected.project,
        selected.source_command_word,
        &observer.launcher(),
    )
    .map_err(|_| failure("cannot instrument selected Source native configuration"))?;
    directory
        .write_new(OsStr::new(CONFIG), instrumented.bytes())
        .map_err(|_| failure("cannot retain native collection configuration"))?;
    directory
        .write_new(OsStr::new(FENCE_NAME), FENCE)
        .map_err(|_| failure("cannot retain native collection default fence"))?;
    // Recheck the original full post-host tree immediately before first write.
    launch.revalidate(request, host_query, host_build, launch_query, cancellation)?;
    write_new_fallback(&build.join(FALLBACK), instrumented.bytes())?;
    let name = NATIVE_PREPARATION_STAGES[2];
    observer.revalidate()?;
    check_home_fallback(parent.environment())?;
    let (stage, _) = run_prepared_observe(
        (
            request,
            cancellation,
            launch.host.root.prepared.configured.started,
        ),
        name,
        context,
        0,
        |stdout, stderr| {
            directory
                .write_new(OsStr::new(&format!("{name}.stdout")), stdout)
                .map_err(|_| failure("cannot retain native collection stdout"))?;
            directory
                .write_new(OsStr::new(&format!("{name}.stderr")), stderr)
                .map_err(|_| failure("cannot retain native collection stderr"))
        },
    )?;
    launch.revalidate_inputs(request, host_query, host_build, launch_query, cancellation)?;
    let output_tree = measure_tree_content_cas(request.output_root)
        .map_err(|_| failure("cannot retain native collection output transition"))?;
    let files: Vec<&str> = fresh_files.iter().map(String::as_str).collect();
    if !launch
        .host
        .output_tree
        .matches_fresh_outputs(&output_tree, &files, &[])
    {
        return Err(failure(
            "native collection wrote outside exact Source fresh outputs",
        ));
    }
    let calls = observer.read_after_reaped(build, deadline(&launch)?, cancellation)?;
    check_evidence_names(&directory)?;
    let evidence_tree = measure_tree_content_cas(directory.path())
        .map_err(|_| failure("cannot retain native collection evidence"))?;
    let result = CollectedMesaNativeRoot {
        selection_sha256: selection_identity(selected)?,
        configuration_sha256: original_sha256,
        derived_configuration_sha256: instrumented.derived_sha256().clone(),
        launch,
        observer,
        directory,
        output_tree,
        evidence_tree,
        fresh_files,
        stage,
        calls,
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

fn path_text(path: &Path) -> Result<String, ContractError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| failure("unsupported native collection path"))
}

fn check_global_inputs(
    project: &ConfiguredProject,
    values: &std::collections::BTreeMap<String, String>,
    inputs: &[(PathBuf, Sha256Digest)],
    build: &Path,
) -> Result<(), ContractError> {
    read_global_inputs(
        project,
        values.clone(),
        inputs,
        build,
        Path::new(
            values
                .get("SRCDIR")
                .ok_or_else(|| failure("native collection lacks Source root"))?,
        ),
    )
    .map(|_| ())
}

fn read_global_inputs(
    project: &ConfiguredProject,
    mut globals: std::collections::BTreeMap<String, String>,
    inputs: &[(PathBuf, Sha256Digest)],
    build: &Path,
    source: &Path,
) -> Result<ProjectGlobalPathContext, ContractError> {
    // Explicit project variables shadow environment fallbacks. Source then
    // sets these builtins before reading global files in configured order.
    globals.extend(project.variables.clone());
    globals.insert("TOP".into(), path_text(build)?);
    globals.insert("SRCDIR".into(), path_text(source)?);
    globals.insert("CURDIR".into(), String::new());
    let mut context = ProjectGlobalPathContext::new(&globals)
        .map_err(|_| failure("native collection global path inputs are not modeled"))?;
    for template in &project.global_variable_files {
        let resolved = context
            .resolve(template)
            .map_err(|_| failure("native collection global file lacks measured Root variables"))?;
        let path = Path::new(&resolved);
        if !path.starts_with(build) || fs::canonicalize(path).map_or(true, |actual| actual != path)
        {
            return Err(failure(
                "native collection global file escaped selected build",
            ));
        }
        let expected = inputs
            .iter()
            .find_map(|(input, digest)| (input == path).then_some(digest))
            .ok_or_else(|| {
                failure("native collection global file was not an original Root input")
            })?;
        let bytes = read_bytes(path)?;
        if sha256_bytes(&bytes) != *expected {
            return Err(failure("native collection global file changed"));
        }
        context
            .advance(&resolved, &bytes)
            .map_err(|_| failure("native collection global file paths are not modeled"))?;
    }
    Ok(context)
}

fn check_optional_fences(
    values: &std::collections::BTreeMap<String, String>,
    host: &MesaNativeHostBuildSelection<'_>,
    build: &Path,
) -> Result<(), ContractError> {
    let generated = values
        .get("GENDIR")
        .ok_or_else(|| failure("native collection lacks measured generated directory"))?;
    let generated = Path::new(generated);
    if !generated.starts_with(build)
        || !generated
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
    {
        return Err(failure(
            "native collection generated directory escaped selected build",
        ));
    }
    for path in [
        build.join("Build.extra"),
        generated.join("config/make.opts"),
    ] {
        if !host.absent_inputs.contains(&path) {
            return Err(failure(
                "native collection optional input lacks original absence fence",
            ));
        }
    }
    Ok(())
}

fn deadline(launch: &ObservedMesaNativeLaunchInputs) -> Result<Instant, ContractError> {
    Instant::now()
        .checked_add(launch.host.root.prepared.remaining()?)
        .ok_or_else(|| failure("native collection deadline overflow"))
}

fn check_native_options(options: &str, source: &Path, build: &Path) -> Result<(), ContractError> {
    let command = LiteralMakeChildCommand::parse(&format!("metamake {options}"))
        .map_err(|_| failure("native collection MetaMake options are not literal"))?;
    let expected = [
        format!("--srcdir={}", path_text(source)?),
        format!("--builddir={}", path_text(build)?),
    ];
    let arguments = command.arguments();
    if command.program() != "metamake"
        || !command.environment_assignments().is_empty()
        || !(arguments == expected
            || (arguments.len() == 3 && arguments[..2] == expected && arguments[2] == "-q"))
    {
        return Err(failure(
            "native collection MetaMake options changed Source or output scope",
        ));
    }
    Ok(())
}

fn selection_identity(
    selected: &MesaNativeCollectionSelection<'_>,
) -> Result<Sha256Digest, ContractError> {
    digest_json(&serde_json::json!({
        "output_root":selected.output_root, "observer":selected.observer,
        "observer_sha256":selected.observer_sha256, "project":selected.project,
        "source_command_word":selected.source_command_word, "maximum_calls":selected.maximum_calls,
    }))
}

fn check_home_fallback(
    environment: &crate::compatibility::environment::ResolvedProbeEnvironment,
) -> Result<(), ContractError> {
    // The invocation is env_clear + the held explicit map. With no HOME in
    // that map, Source MetaMake skips this fallback; no ambient HOME is added.
    let Some(home) = environment.variables.get("HOME") else {
        return Ok(());
    };
    let home = Path::new(home);
    if !home.is_absolute() || fs::canonicalize(home).map_or(true, |actual| actual != home) {
        return Err(failure(
            "native collection HOME is not a canonical directory",
        ));
    }
    match fs::symlink_metadata(home.join(FALLBACK)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(failure("native collection HOME fallback is not absent")),
    }
}

fn validate_scope(
    launch: &ObservedMesaNativeLaunchInputs,
    request: &MesaPreflightRequest<'_>,
    host_query: &MesaNativeRootQuerySelection<'_>,
    host_build: &MesaNativeHostBuildSelection<'_>,
    launch_query: &MesaNativeRootQuerySelection<'_>,
    selected: &MesaNativeCollectionSelection<'_>,
) -> Result<(), ContractError> {
    super::super::super::validate_selection(
        request,
        &MesaNativeRootQuerySelection {
            output_root: selected.output_root,
            variables: launch_query.variables,
            goals: launch_query.goals,
        },
    )?;
    for old in [
        host_query.output_root,
        host_build.output_root,
        launch_query.output_root,
        selected.observer,
    ] {
        if filesystem_paths_overlap(selected.output_root, old)
            .map_err(|_| failure("cannot compare native collection evidence scope"))?
        {
            return Err(failure(
                "native collection overlaps selected inputs or evidence",
            ));
        }
    }
    launch.host.root.prepared.remaining()?;
    Ok(())
}

fn selected_outputs(
    before: &TreeContentCas,
    root: &Path,
    build: &Path,
    generated: &[String],
) -> Result<Vec<String>, ContractError> {
    if generated.len() > 200_000 {
        return Err(failure("native collection output plan exceeds bound"));
    }
    let mut paths = BTreeSet::new();
    for relative in generated
        .iter()
        .map(String::as_str)
        .chain([CACHE, FALLBACK])
    {
        if relative.is_empty()
            || !Path::new(relative)
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(failure("native collection output plan is not relative"));
        }
        let full = build.join(relative);
        let coordinate = full
            .strip_prefix(root)
            .ok()
            .and_then(Path::to_str)
            .ok_or_else(|| failure("native collection output escaped selected root"))?;
        if !paths.insert(coordinate.to_owned())
            || before.entry_kind(coordinate).is_some()
            || fs::symlink_metadata(&full).is_ok()
            || fs::symlink_metadata(PathBuf::from(format!("{}tmp", full.display()))).is_ok()
        {
            return Err(failure(
                "native collection requires distinct absent output coordinates",
            ));
        }
        for ancestor in Path::new(coordinate).ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            if before
                .entry_kind(ancestor.to_str().unwrap_or_default())
                .is_some_and(|kind| kind != TreeContentEntryKind::Directory)
            {
                return Err(failure(
                    "native collection output has a non-directory ancestor",
                ));
            }
        }
    }
    for path in &paths {
        if Path::new(path)
            .ancestors()
            .skip(1)
            .any(|ancestor| ancestor.to_str().is_some_and(|name| paths.contains(name)))
            || paths.contains(&format!("{path}tmp"))
        {
            return Err(failure("native collection output coordinates overlap"));
        }
    }
    Ok(paths.into_iter().collect())
}

fn root_arguments(
    build: &Path,
    evidence: &Path,
    inputs: &[(PathBuf, Sha256Digest)],
    absent: &[PathBuf],
    goals: &[String],
) -> Result<Vec<String>, ContractError> {
    let mut arguments = vec![
        "-s".into(),
        "-r".into(),
        "-f".into(),
        path_text(&build.join("Makefile"))?,
        "-f".into(),
        path_text(&evidence.join(FENCE_NAME))?,
    ];
    for path in inputs
        .iter()
        .map(|(path, _)| path)
        .chain(absent)
        .chain([&evidence.join(FENCE_NAME)])
    {
        validate_make_path(path)?;
        arguments.push(format!("--old-file={}", path_text(path)?));
    }
    arguments.extend_from_slice(goals);
    let startup = LiteralMakeStartupArguments::admit(&arguments)
        .map_err(|_| failure("native collection Root arguments are unsupported"))?;
    if startup.goals() != goals || !startup.command_line_names().is_empty() {
        return Err(failure(
            "native collection Root arguments changed selected goals",
        ));
    }
    Ok(arguments)
}

fn write_new_fallback(path: &Path, bytes: &[u8]) -> Result<(), ContractError> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| failure("cannot create fresh native collection fallback"))?;
    file.write_all(bytes)
        .map_err(|_| failure("cannot write native collection fallback"))
}

fn check_evidence_names(directory: &PrivateNativeCaptureDirectory) -> Result<(), ContractError> {
    let expected = [
        "c",
        "d",
        "o",
        "native-context.json",
        CONFIG,
        FENCE_NAME,
        "native-root-launch.stdout",
        "native-root-launch.stderr",
    ]
    .map(std::ffi::OsString::from)
    .into_iter()
    .collect::<BTreeSet<_>>();
    let actual = directory_entry_names_nofollow_bounded(directory.path(), expected.len())
        .map_err(|_| failure("native collection evidence namespace exceeds selection"))?
        .into_iter()
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(failure("native collection evidence namespace changed"));
    }
    directory
        .revalidate()
        .map_err(|_| failure("native collection evidence directory changed"))
}

#[cfg(test)]
#[path = "mesa_preflight_native_collection_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mesa_preflight_native_runtime_diagnostic.rs"]
mod runtime_diagnostic;

#[cfg(all(test, unix))]
#[path = "mesa_preflight_native_runtime_batch_probe.rs"]
mod runtime_batch_probe;

#[cfg(all(test, unix))]
#[path = "mesa_preflight_native_child_source_probe.rs"]
mod child_source_probe;

#[cfg(test)]
#[path = "mesa_preflight_native_child_diagnostic.rs"]
mod child_diagnostic;
