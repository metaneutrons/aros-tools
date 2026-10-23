//! Live Source-selected host build; not complete native preparation/admission.

use super::{
    digest_json, failure, invocation, read_bytes, run_prepared_observe, validate_make_path,
    CompatibilityEnvironment, LiteralMakeStartupArguments, MesaNativeRootQuerySelection,
    MesaPreflightRequest, ObservedMesaNativeRoot, PrivateNativeCaptureDirectory, Stage,
};
use crate::ContractError;
use aros_common::{
    make_child_command::LiteralMakeChildCommand, measure_tree_content_cas,
    mesa_native_make::NATIVE_PREPARATION_STAGES, CancellationToken, Sha256Digest, TreeContentCas,
};
use std::{
    ffi::OsStr,
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

const FENCE_NAME: &str = "default-fence.mk";
const FENCE: &[u8] = b".DEFAULT:\n.DEFAULT:\n\t$(error AROS_NATIVE_HOST_UNEXPECTED_DEFAULT)\n";

#[path = "mesa_preflight_native_launch_inputs.rs"]
mod launch_inputs;
pub use launch_inputs::{
    GeneratedNativeMakeInclude, NativeMakeIncludeExecution,
    collect_mesa_native_root, join_mesa_native_source, observe_mesa_native_dispatch,
    observe_mesa_native_launch_inputs, prepare_mesa_native_direct_collection,
    CollectedMesaNativeDirect, CollectedMesaNativeRoot, ExecutedMesa26OriginalRecipe,
    Mesa26GeneratedPathListSelection, Mesa26OriginalRecipeExecution, MesaNativeCollectionSelection,
    MesaNativeDirectOrigin, MesaNativeDispatchSelection, MesaNativeSourceSelection,
    ObservedFencedMesaNativeDispatch, ObservedMesaNativeDispatch, ObservedMesaNativeLaunchInputs,
    PreparedMesaNativeDirectCollection, RecordedMesaNativeDispatch, SourceJoinedMesaNativeRoot,
};

/// Explicit caller-selected Source goal, artifact coordinate and fresh write set.
///
/// Paths are selected policy, not inferred authority or Source authentication.
/// Every existing prepared entry remains immutable, apart from directory
/// ancestor timestamps/size caused by creating the selected fresh outputs.
pub struct MesaNativeHostBuildSelection<'a> {
    /// Fresh private evidence leaf, disjoint from all inputs and query evidence.
    pub output_root: &'a Path,
    /// First goal used for the preceding live Root observation.
    pub goal: &'a str,
    /// Remaining goals in their original query order, never appended after observation.
    /// This reuses the selected Source host bootstrap, not project recipe admission.
    pub additional_goals: &'a [String],
    /// Root query variable whose exact value names the fresh host executable.
    pub executable_variable: &'a str,
    /// Exact fresh regular files, including the selected host executable.
    pub fresh_files: &'a [PathBuf],
    /// Exact fresh directory subtrees; old files may never be included here.
    pub fresh_directories: &'a [PathBuf],
    /// Explicit absent optional Make inputs fenced against include-remake.
    /// This list alone is not a proof of complete optional-input coverage.
    pub absent_inputs: &'a [PathBuf],
}

/// Move-only host-build result retaining original preparation and input owners.
/// No completed receipt or native graph admission is produced by this phase.
pub struct BuiltMesaNativeHost {
    root: ObservedMesaNativeRoot,
    directory: PrivateNativeCaptureDirectory,
    selection_sha256: Sha256Digest,
    output_tree: TreeContentCas,
    evidence_tree: TreeContentCas,
    executable: PathBuf,
    executable_sha256: Sha256Digest,
    stage: Stage,
}

impl BuiltMesaNativeHost {
    /// Actual measured host artifact, not an inferred compiler-prefix path.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Measured bytes of the produced executable, not authentication evidence.
    #[must_use]
    pub const fn executable_sha256(&self) -> &Sha256Digest {
        &self.executable_sha256
    }

    /// Recheck original input owners, exact transition and retained evidence.
    ///
    /// # Errors
    /// Refuses reselection, changed inputs/outputs, cancellation or expiry.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeHostBuildSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.root.revalidate_inputs(request, query, cancellation)?;
        self.revalidate_original_output_transition(request, selected)?;
        if measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot remeasure native host build output"))?
            != self.output_tree
        {
            return Err(failure("native host build output transition changed"));
        }
        self.revalidate_bound_evidence(request, selected)
    }

    pub(super) fn revalidate_inputs(
        &self,
        request: &MesaPreflightRequest<'_>,
        query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeHostBuildSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.root.revalidate_inputs(request, query, cancellation)?;
        self.revalidate_original_output_transition(request, selected)?;
        self.revalidate_bound_evidence(request, selected)
    }

    fn revalidate_original_output_transition(
        &self,
        request: &MesaPreflightRequest<'_>,
        selected: &MesaNativeHostBuildSelection<'_>,
    ) -> Result<(), ContractError> {
        if selection_identity(selected)? != self.selection_sha256 {
            return Err(failure("native host build selected different inputs"));
        }
        let (files, directories) = relative_outputs(request, selected)?;
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let directories: Vec<&str> = directories.iter().map(String::as_str).collect();
        if !self.root.prepared.output_snapshot().matches_fresh_outputs(
            &self.output_tree,
            &files,
            &directories,
        ) {
            return Err(failure("native host build output transition changed"));
        }
        Ok(())
    }

    fn revalidate_bound_evidence(
        &self,
        request: &MesaPreflightRequest<'_>,
        selected: &MesaNativeHostBuildSelection<'_>,
    ) -> Result<(), ContractError> {
        self.directory
            .revalidate()
            .map_err(|_| failure("native host evidence directory changed"))?;
        if measure_tree_content_cas(self.directory.path())
            .map_err(|_| failure("cannot remeasure native host evidence"))?
            != self.evidence_tree
        {
            return Err(failure("native host evidence changed"));
        }
        self.stage.invocation_context.revalidate()?;
        if super::super::super::measure_executable(&self.executable)?.0 != self.executable_sha256 {
            return Err(failure("native host executable changed"));
        }
        check_absent(request, selected)?;
        self.directory
            .revalidate()
            .map_err(|_| failure("native host evidence directory changed"))?;
        self.root.prepared.remaining()?;
        Ok(())
    }
}

/// Consume the live Root observation and execute its selected Source host goal.
///
/// The original Root Makefile executes the recipe; no copied host-build shell,
/// CPU/flag table or reconstructed receipt is used. Selected Source parse-time
/// and recipe effects still require caller trust and exclusive path ownership.
/// The post-build write-set check is not an operating-system sandbox.
///
/// # Errors
/// Refuses nonmatching query goals, stale/adopted selections, compiler-build
/// intent, changed inputs, unexpected output writes, child failure or expiry.
pub fn build_mesa_native_host(
    root: ObservedMesaNativeRoot,
    request: &MesaPreflightRequest<'_>,
    query: &MesaNativeRootQuerySelection<'_>,
    selected: &MesaNativeHostBuildSelection<'_>,
    cancellation: &CancellationToken,
) -> Result<BuiltMesaNativeHost, ContractError> {
    root.revalidate(request, query, cancellation)?;
    if query.goals.first().map(String::as_str) != Some(selected.goal)
        || query.goals.get(1..) != Some(selected.additional_goals)
        || selected.absent_inputs.len() > 64
    {
        return Err(failure("native host goal differs from original Root query"));
    }
    super::validate_selection(
        request,
        &MesaNativeRootQuerySelection {
            output_root: selected.output_root,
            variables: query.variables,
            goals: query.goals,
        },
    )?;
    if aros_common::publication::filesystem_paths_overlap(query.output_root, selected.output_root)
        .map_err(|_| failure("cannot compare native host evidence paths"))?
    {
        return Err(failure("native host evidence overlaps original query"));
    }
    let (files, directories) = relative_outputs(request, selected)?;
    validate_fresh_policy(root.prepared.output_snapshot(), &files, &directories)?;
    check_absent(request, selected)?;
    let executable = PathBuf::from(
        root.values
            .get(selected.executable_variable)
            .ok_or_else(|| failure("native host executable was not queried"))?,
    );
    if !selected.fresh_files.contains(&executable) {
        return Err(failure(
            "native host executable is outside the selected fresh files",
        ));
    }
    check_host_roles(&root, request)?;
    let parent = root.prepared.configured_invocation()?;
    let build = parent.working_directory();
    let root_makefile = build.join("Makefile");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(selected.output_root)
        .map_err(|_| failure("cannot create fresh native host evidence"))?;
    let directory = PrivateNativeCaptureDirectory::open(selected.output_root)
        .map_err(|_| failure("cannot hold native host evidence directory"))?;
    directory
        .write_new(OsStr::new(FENCE_NAME), FENCE)
        .map_err(|_| failure("cannot retain native host default fence"))?;
    let fence = directory.path().join(FENCE_NAME);
    let mut arguments = vec![
        "-s".to_owned(),
        "-r".into(),
        "-f".into(),
        super::super::path_text(&root_makefile)?,
        "-f".into(),
        super::super::path_text(&fence)?,
    ];
    for path in root.values["MAKEFILE_LIST"].split_ascii_whitespace() {
        arguments.push(format!("--old-file={path}"));
    }
    for path in selected.absent_inputs {
        arguments.push(format!("--old-file={}", super::super::path_text(path)?));
    }
    arguments.push(format!("--old-file={}", super::super::path_text(&fence)?));
    arguments.push(selected.goal.to_owned());
    arguments.extend_from_slice(selected.additional_goals);
    let startup = LiteralMakeStartupArguments::admit(&arguments)
        .map_err(|_| failure("unsupported native host startup arguments"))?;
    if startup.goals() != query.goals || !startup.command_line_names().is_empty() {
        return Err(failure("native host startup changed selected goal"));
    }
    let context = invocation::InvocationContext::new(
        parent.program(),
        &arguments,
        build,
        parent.environment().clone(),
    )?;
    let name = NATIVE_PREPARATION_STAGES[1];
    let (stage, _) = run_prepared_observe(
        (request, cancellation, root.prepared.configured.started),
        name,
        context,
        0,
        |stdout, stderr| {
            directory
                .write_new(OsStr::new(&format!("{name}.stdout")), stdout)
                .map_err(|_| failure("cannot retain native host stdout"))?;
            directory
                .write_new(OsStr::new(&format!("{name}.stderr")), stderr)
                .map_err(|_| failure("cannot retain native host stderr"))
        },
    )?;
    directory
        .read_selected(
            OsStr::new(&format!("{name}.stdout")),
            &stage.stdout,
            super::super::CAPTURE,
        )
        .map_err(|_| failure("native host stdout changed"))?;
    directory
        .read_selected(
            OsStr::new(&format!("{name}.stderr")),
            &stage.stderr,
            super::super::CAPTURE,
        )
        .map_err(|_| failure("native host stderr changed"))?;
    root.revalidate_inputs(request, query, cancellation)?;
    let output_tree = measure_tree_content_cas(request.output_root)
        .map_err(|_| failure("cannot retain native host output transition"))?;
    let file_names: Vec<&str> = files.iter().map(String::as_str).collect();
    let directory_names: Vec<&str> = directories.iter().map(String::as_str).collect();
    if !root.prepared.output_snapshot().matches_fresh_outputs(
        &output_tree,
        &file_names,
        &directory_names,
    ) {
        return Err(failure("native host wrote outside selected fresh outputs"));
    }
    let executable_sha256 = super::super::super::measure_executable(&executable)?.0;
    let evidence_tree = measure_tree_content_cas(directory.path())
        .map_err(|_| failure("cannot retain native host evidence identity"))?;
    if evidence_tree.entry_count() != 3
        || evidence_tree.has_symlinks()
        || read_bytes(&fence)? != FENCE
    {
        return Err(failure("native host evidence or default fence changed"));
    }
    let result = BuiltMesaNativeHost {
        root,
        directory,
        selection_sha256: selection_identity(selected)?,
        output_tree,
        evidence_tree,
        executable,
        executable_sha256,
        stage,
    };
    result.revalidate(request, query, selected, cancellation)?;
    Ok(result)
}

fn check_host_roles(
    root: &ObservedMesaNativeRoot,
    request: &MesaPreflightRequest<'_>,
) -> Result<(), ContractError> {
    check_host_role_values(
        &root.values,
        root.prepared.configured_invocation()?.program_sha256(),
        request,
    )
}

fn check_host_role_values(
    values: &std::collections::BTreeMap<String, String>,
    driver_sha256: &Sha256Digest,
    request: &MesaPreflightRequest<'_>,
) -> Result<(), ContractError> {
    if values.get("CROSSTOOLS_TARGET").map(String::as_str) != Some("") {
        return Err(failure(
            "native host build requires compiler construction disabled",
        ));
    }
    let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment else {
        return Err(failure("native host build requires sealed host tools"));
    };
    let cc = LiteralMakeChildCommand::parse(
        values
            .get("HOST_CC")
            .ok_or_else(|| failure("native host compiler was not queried"))?,
    )
    .map_err(|_| failure("native host compiler is not literal"))?;
    if Path::new(cc.program()) != host_tools.root.join("cc")
        || !cc.environment_assignments().is_empty()
    {
        return Err(failure("native host compiler differs from sealed role"));
    }
    let make = LiteralMakeChildCommand::parse(
        values
            .get("HOST_MAKE")
            .ok_or_else(|| failure("native recursive Make was not queried"))?,
    )
    .map_err(|_| failure("native recursive Make is not literal"))?;
    if !make.arguments().is_empty()
        || !make.environment_assignments().is_empty()
        || host_tools.tools.get(make.program()).is_none_or(|tool| {
            &tool.sha256 != driver_sha256 || !matches!(make.program(), "make" | "gmake")
        })
    {
        return Err(failure("native recursive Make differs from sealed driver"));
    }
    Ok(())
}

fn relative_outputs(
    request: &MesaPreflightRequest<'_>,
    selected: &MesaNativeHostBuildSelection<'_>,
) -> Result<(Vec<String>, Vec<String>), ContractError> {
    if selected
        .fresh_files
        .len()
        .saturating_add(selected.fresh_directories.len())
        > 32
    {
        return Err(failure("native host fresh output policy exceeded"));
    }
    let build = request.output_root.join("configure");
    let convert = |paths: &[PathBuf]| {
        paths
            .iter()
            .map(|path| {
                validate_make_path(path)?;
                if !path.starts_with(&build) || path == &build || !normalized_absolute(path) {
                    return Err(failure("native host output escaped selected build"));
                }
                path.strip_prefix(request.output_root)
                    .ok()
                    .and_then(Path::to_str)
                    .map(str::to_owned)
                    .ok_or_else(|| failure("unsupported native host output path"))
            })
            .collect::<Result<Vec<_>, _>>()
    };
    Ok((
        convert(selected.fresh_files)?,
        convert(selected.fresh_directories)?,
    ))
}

fn validate_fresh_policy(
    before: &TreeContentCas,
    files: &[String],
    directories: &[String],
) -> Result<(), ContractError> {
    let paths: Vec<&str> = files
        .iter()
        .chain(directories)
        .map(String::as_str)
        .collect();
    if paths.is_empty()
        || paths.iter().enumerate().any(|(index, path)| {
            before.entry_kind(path).is_some()
                || paths.iter().enumerate().any(|(other_index, other)| {
                    index != other_index && (path == other || Path::new(path).starts_with(other))
                })
        })
    {
        return Err(failure(
            "native host outputs are not distinct fresh coordinates",
        ));
    }
    for path in paths {
        for ancestor in Path::new(path).ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            if before
                .entry_kind(ancestor.to_str().unwrap_or_default())
                .is_some_and(|kind| kind != aros_common::TreeContentEntryKind::Directory)
            {
                return Err(failure("native host output has a non-directory ancestor"));
            }
        }
    }
    Ok(())
}

fn check_absent(
    request: &MesaPreflightRequest<'_>,
    selected: &MesaNativeHostBuildSelection<'_>,
) -> Result<(), ContractError> {
    for path in selected.absent_inputs {
        validate_make_path(path)?;
        if !path.starts_with(request.output_root)
            || !normalized_absolute(path)
            || !fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return Err(failure(
                "native host optional input is not absent and confined",
            ));
        }
        for ancestor in path.ancestors().skip(1) {
            match fs::canonicalize(ancestor) {
                Ok(canonical) if canonical == ancestor => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(failure("native host optional input has a foreign ancestor")),
            }
        }
    }
    Ok(())
}

fn normalized_absolute(path: &Path) -> bool {
    path.to_str().is_some_and(|value| {
        value.starts_with('/')
            && value[1..]
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
    })
}

fn selection_identity(
    selected: &MesaNativeHostBuildSelection<'_>,
) -> Result<Sha256Digest, ContractError> {
    digest_json(
        &serde_json::json!({"schema":"aros-native-host-build-selection-v2",
        "output_root":selected.output_root,"goal":selected.goal,
        "additional_goals":selected.additional_goals,
        "executable_variable":selected.executable_variable,"fresh_files":selected.fresh_files,
        "fresh_directories":selected.fresh_directories,"absent_inputs":selected.absent_inputs}),
    )
}
