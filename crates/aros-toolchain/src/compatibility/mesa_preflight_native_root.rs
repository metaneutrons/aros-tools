//! Live, read-only Root query before native host preparation.
//!
//! This observes selected Source Make data, not a complete native preparation.
//! GNU parsing can execute selected Source parse-time expressions: this is not
//! a sandbox or authority for arbitrary Make input. The supervising caller
//! exclusively owns the selected source/preparation/evidence paths. A fatal
//! second input prevents GNU's include-remake and target-recipe phases.

use super::{
    digest_json, failure, invocation, parse_values, preparation, prepare_mesa_preflight,
    read_bytes, run_prepared_observe, validate_make_path, CompatibilityEnvironment,
    MesaPreflightRequest, Stage,
};
use crate::ContractError;
use aros_common::{
    make_native_invocation::PrivateNativeCaptureDirectory,
    make_startup_arguments::LiteralMakeStartupArguments, measure_tree_content_cas,
    mesa_configuration::ValidatedMesaConfiguration, mesa_native_make::NATIVE_PREPARATION_STAGES,
    mesa_source::MesaSourceSnapshot, sha256_bytes, CancellationToken, Sha256Digest, TreeContentCas,
};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fmt::Write as _,
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

const QUERY_FILE: &str = "query.mk";
const INPUT_LIST: &str = "MAKEFILE_LIST";
const MAX_VARIABLES: usize = 128;
const MAX_GOALS: usize = 64;
const MAX_SELECTION_BYTES: usize = 16 * 1024;
const MAX_INPUTS: usize = 1024;
const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// Explicit query selection; no CPU, compiler-option or target-name inference.
pub struct MesaNativeRootQuerySelection<'a> {
    /// Fresh private leaf disjoint from all preparation and selected inputs.
    pub output_root: &'a Path,
    /// Ordered literal measurement names. MAKEFILE_LIST is always also retained.
    pub variables: &'a [String],
    /// Exact Source root goals whose parse-time context is being queried.
    pub goals: &'a [String],
}

/// Captured live query data that can be reused by a later native phase.
///
/// This is evidence of the query process and its selected inputs, not an owner
/// revalidation, deadline claim, or authorization to run another phase.
pub(super) struct NativeRootQueryData {
    pub(super) directory: PrivateNativeCaptureDirectory,
    pub(super) query_selection_sha256: Sha256Digest,
    pub(super) query_tree: TreeContentCas,
    pub(super) stage: Stage,
    pub(super) exit_code: i32,
    pub(super) values: BTreeMap<String, String>,
    pub(super) read_inputs: Vec<(PathBuf, Sha256Digest)>,
}

/// Live first native phase, deliberately not a completed preflight or receipt.
///
/// Private fields cannot be reconstructed from portable data. Subsequent native
/// phases must consume this same preparation, deadline and input selection;
/// this object alone grants no host-build, compiler, graph or publication right.
pub struct ObservedMesaNativeRoot {
    prepared: preparation::PreparedMesaPreflight,
    directory: PrivateNativeCaptureDirectory,
    query_selection_sha256: Sha256Digest,
    query_tree: TreeContentCas,
    source_snapshot: MesaSourceSnapshot,
    stage: Stage,
    exit_code: i32,
    values: BTreeMap<String, String>,
    read_inputs: Vec<(PathBuf, Sha256Digest)>,
}

impl ObservedMesaNativeRoot {
    /// Exact observed query values; these are not reconstructed option formulas.
    #[must_use]
    pub const fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }

    /// Ordered canonical input names and measured bytes reported by this query.
    /// This is not an independent completeness/authentication assertion.
    #[must_use]
    pub fn read_inputs(&self) -> &[(PathBuf, Sha256Digest)] {
        &self.read_inputs
    }

    /// Caller-selected private evidence directory.
    #[must_use]
    pub fn output_root(&self) -> &Path {
        self.directory.path()
    }

    /// Measured process exit code, not a caller-supplied success label.
    #[must_use]
    pub const fn observed_exit_code(&self) -> i32 {
        self.exit_code
    }

    /// The original source/configuration identity, without a completion claim.
    #[must_use]
    pub const fn configuration(&self) -> &ValidatedMesaConfiguration {
        &self.prepared.configured.initial_configuration
    }

    /// Revalidate the original selection, clock, preparation and query evidence.
    ///
    /// # Errors
    /// Refuses reselection, cancellation, expiry, changed inputs/evidence or
    /// replacement of the held private directory. Errors omit raw query data.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        selection: &MesaNativeRootQuerySelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.prepared.revalidate(request, cancellation)?;
        self.revalidate_inputs(request, selection, cancellation)
    }

    fn revalidate_inputs(
        &self,
        request: &MesaPreflightRequest<'_>,
        selection: &MesaNativeRootQuerySelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.prepared.revalidate_selection_with_source(
            request,
            cancellation,
            &self.source_snapshot,
        )?;
        if selection_identity(selection)? != self.query_selection_sha256 {
            return Err(failure("native Root query selected different inputs"));
        }
        self.directory
            .revalidate()
            .map_err(|_| failure("native Root query directory changed"))?;
        self.stage.invocation_context.revalidate()?;
        for (path, expected) in &self.read_inputs {
            if sha256_bytes(&read_bytes(path)?) != *expected {
                return Err(failure("native Root query input changed"));
            }
        }
        if measure_tree_content_cas(self.directory.path())
            .map_err(|_| failure("cannot remeasure native Root query evidence"))?
            != self.query_tree
        {
            return Err(failure("native Root query evidence changed"));
        }
        self.directory
            .revalidate()
            .map_err(|_| failure("native Root query directory changed"))?;
        self.prepared.remaining()?;
        Ok(())
    }
}

#[path = "mesa_preflight_native_host.rs"]
mod native_host;
pub use native_host::{
    GeneratedNativeMakeInclude, NativeMakeIncludeExecution,
    build_mesa_native_host, collect_mesa_native_root, join_mesa_native_source,
    observe_mesa_native_dispatch, observe_mesa_native_launch_inputs,
    prepare_mesa_native_direct_collection, BuiltMesaNativeHost, CollectedMesaNativeDirect,
    CollectedMesaNativeRoot, ExecutedMesa26OriginalRecipe, Mesa26GeneratedPathListSelection,
    Mesa26OriginalRecipeExecution, MesaNativeCollectionSelection, MesaNativeDirectOrigin,
    MesaNativeDispatchSelection, MesaNativeHostBuildSelection, MesaNativeSourceSelection,
    ObservedFencedMesaNativeDispatch, ObservedMesaNativeDispatch, ObservedMesaNativeLaunchInputs,
    PreparedMesaNativeDirectCollection, RecordedMesaNativeDispatch, SourceJoinedMesaNativeRoot,
};

/// Configure the selected Source and observe Root before any native build.
///
/// The query is a second Make input ending in a selected fatal-stop marker.
/// Exit two without that exact stop, complete requested records and confined
/// reported inputs is rejected. No completion receipt or graph input is emitted.
/// Source parse-time expressions still execute; caller-selected Source trust,
/// exclusive ownership and later complete native phase qualification remain
/// preconditions, not properties inferred from an exit code or these values.
///
/// # Errors
/// Refuses malformed or adopted selections, stale inputs, child failure,
/// invalid query protocol, input/evidence drift, cancellation or shared expiry.
pub fn observe_mesa_native_root(
    request: &MesaPreflightRequest<'_>,
    selection: &MesaNativeRootQuerySelection<'_>,
    cancellation: &CancellationToken,
) -> Result<ObservedMesaNativeRoot, ContractError> {
    validate_selection(request, selection)?;
    let prepared = prepare_mesa_preflight(request, cancellation)?;
    query_prepared_root(request, selection, cancellation, prepared)
}

fn query_prepared_root(
    request: &MesaPreflightRequest<'_>,
    selection: &MesaNativeRootQuerySelection<'_>,
    cancellation: &CancellationToken,
    prepared: preparation::PreparedMesaPreflight,
) -> Result<ObservedMesaNativeRoot, ContractError> {
    prepared.revalidate(request, cancellation)?;
    validate_selection(request, selection)?;
    let source_snapshot = MesaSourceSnapshot::capture(
        request.source_root,
        request.source_tree_sha256,
        request.local_source,
    )
    .map_err(|_| failure("cannot retain native Root query source inputs"))?;
    let data = query_root_inputs(
        request,
        selection,
        cancellation,
        &prepared,
        &source_snapshot,
        prepared.output_snapshot(),
        || prepared.revalidate(request, cancellation),
    )?;
    let result = ObservedMesaNativeRoot {
        prepared,
        directory: data.directory,
        query_selection_sha256: data.query_selection_sha256,
        query_tree: data.query_tree,
        source_snapshot,
        stage: data.stage,
        exit_code: data.exit_code,
        values: data.values,
        read_inputs: data.read_inputs,
    };
    result.revalidate(request, selection, cancellation)?;
    Ok(result)
}

/// Execute the selected Root query against retained Source and output owners.
///
/// The owner callback is checked before creating evidence and again after the
/// child and its reported inputs have been inspected. The supplied output tree
/// remains the original owner snapshot across later native phase transitions.
pub(super) fn query_root_inputs(
    request: &MesaPreflightRequest<'_>,
    selection: &MesaNativeRootQuerySelection<'_>,
    cancellation: &CancellationToken,
    prepared: &preparation::PreparedMesaPreflight,
    source_snapshot: &MesaSourceSnapshot,
    output_snapshot: &TreeContentCas,
    revalidate_owner: impl Fn() -> Result<(), ContractError>,
) -> Result<NativeRootQueryData, ContractError> {
    let keys = validate_selection(request, selection)?;
    revalidate_owner()?;
    let parent = prepared.configured_invocation()?;
    let build = parent.working_directory();
    let root_makefile = build.join("Makefile");
    if output_snapshot
        .regular_file_digest("configure/Makefile")
        .is_none()
    {
        return Err(failure(
            "native Root query lost its original generated Makefile",
        ));
    }
    let query_selection_sha256 = selection_identity(selection)?;
    let marker = format!("AROS_NATIVE_ROOT_QUERY_STOP_{query_selection_sha256}");
    let mut text = String::new();
    for key in &keys {
        writeln!(text, "$(info {key}=$({key}))")
            .map_err(|_| failure("cannot encode Root query"))?;
    }
    let stop_line = keys.len() + 1;
    writeln!(text, "$(error {marker})").map_err(|_| failure("cannot encode Root query"))?;
    fs::DirBuilder::new()
        .mode(0o700)
        .create(selection.output_root)
        .map_err(|_| failure("cannot create fresh native Root query directory"))?;
    let directory = PrivateNativeCaptureDirectory::open(selection.output_root)
        .map_err(|_| failure("cannot hold private native Root query directory"))?;
    directory
        .write_new(OsStr::new(QUERY_FILE), text.as_bytes())
        .map_err(|_| failure("cannot retain selected native Root query"))?;
    let query_path = directory.path().join(QUERY_FILE);
    let query_digest = sha256_bytes(text.as_bytes());
    let mut arguments = vec![
        "-s".into(),
        "-r".into(),
        "--no-print-directory".into(),
        "-f".into(),
        super::path_text(&root_makefile)?,
        "-f".into(),
        super::path_text(&query_path)?,
    ];
    arguments.extend_from_slice(selection.goals);
    let admitted = LiteralMakeStartupArguments::admit(&arguments)
        .map_err(|_| failure("native Root query startup arguments are unsupported"))?;
    if !admitted.command_line_names().is_empty() || admitted.goals() != selection.goals {
        return Err(failure(
            "native Root query goals changed invocation semantics",
        ));
    }
    let context = invocation::InvocationContext::new(
        parent.program(),
        &arguments,
        build,
        parent.environment().clone(),
    )?;
    let stage_name = NATIVE_PREPARATION_STAGES[0];
    let stdout_name = format!("{stage_name}.stdout");
    let stderr_name = format!("{stage_name}.stderr");
    let (stage, exit_code) = run_prepared_observe(
        (request, cancellation, prepared.configured.started),
        stage_name,
        context,
        2,
        |stdout, stderr| {
            directory
                .write_new(OsStr::new(&stdout_name), stdout)
                .map_err(|_| failure("cannot retain native Root query stdout"))?;
            directory
                .write_new(OsStr::new(&stderr_name), stderr)
                .map_err(|_| failure("cannot retain native Root query stderr"))
        },
    )?;
    let stdout = directory
        .read_selected(OsStr::new(&stdout_name), &stage.stdout, super::CAPTURE)
        .map_err(|_| failure("native Root query stdout changed"))?;
    let stderr = directory
        .read_selected(OsStr::new(&stderr_name), &stage.stderr, super::CAPTURE)
        .map_err(|_| failure("native Root query stderr changed"))?;
    let diagnostic_prefix = format!("{}:{stop_line}: *** {marker}.", query_path.display());
    if std::str::from_utf8(&stderr)
        .ok()
        .and_then(|value| value.strip_prefix(&diagnostic_prefix))
        .map(str::trim)
        != Some("Stop.")
    {
        return Err(failure(
            "native Root query did not reach its selected fatal stop",
        ));
    }
    let names: Vec<&str> = keys.iter().map(String::as_str).collect();
    let values = parse_values(&stdout, &names)?;
    let mut read_inputs = Vec::new();
    let mut input_bytes = 0usize;
    for word in values[INPUT_LIST].split_ascii_whitespace() {
        if read_inputs.len() >= MAX_INPUTS {
            return Err(failure("native Root query input count exceeded"));
        }
        let path = Path::new(word);
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            build.join(path)
        };
        let canonical =
            fs::canonicalize(&path).map_err(|_| failure("native Root query input is absent"))?;
        let expected = if canonical == query_path {
            Some(&query_digest)
        } else if let Ok(relative) = canonical.strip_prefix(request.output_root) {
            output_snapshot.regular_file_digest(relative.to_str().unwrap_or_default())
        } else if let Ok(relative) = canonical.strip_prefix(request.source_root) {
            source_snapshot.regular_input_digest(relative.to_str().unwrap_or_default())
        } else {
            None
        };
        let expected =
            expected.ok_or_else(|| failure("native Root query read an unbound input"))?;
        let bytes = read_bytes(&canonical)?;
        input_bytes = input_bytes.saturating_add(bytes.len());
        if input_bytes > MAX_INPUT_BYTES {
            return Err(failure("native Root query input bytes exceeded"));
        }
        let digest = sha256_bytes(&bytes);
        if &digest != expected {
            return Err(failure("native Root query input changed"));
        }
        read_inputs.push((canonical, digest));
    }
    if read_inputs.first().map(|(path, _)| path) != Some(&root_makefile)
        || read_inputs.last().map(|(path, _)| path) != Some(&query_path)
    {
        return Err(failure(
            "native Root query input order differs from selected Root and stop",
        ));
    }
    revalidate_owner()?;
    let query_tree = measure_tree_content_cas(directory.path())
        .map_err(|_| failure("cannot retain native Root query evidence"))?;
    if query_tree.entry_count() != 3 || query_tree.has_symlinks() {
        return Err(failure("native Root query produced unexpected evidence"));
    }
    let result = NativeRootQueryData {
        directory,
        query_selection_sha256,
        query_tree,
        stage,
        exit_code,
        values,
        read_inputs,
    };
    revalidate_query_data(&result, selection, cancellation)?;
    Ok(result)
}

/// Recheck query-owned evidence without claiming validity of its preparation.
///
/// This intentionally does not revalidate the outer owner or shared deadline;
/// callers must retain and revalidate those independently.
pub(super) fn revalidate_query_data(
    data: &NativeRootQueryData,
    selection: &MesaNativeRootQuerySelection<'_>,
    cancellation: &CancellationToken,
) -> Result<(), ContractError> {
    if cancellation.is_cancelled() {
        return Err(failure("native Root query was cancelled"));
    }
    if selection_identity(selection)? != data.query_selection_sha256 {
        return Err(failure("native Root query selected different inputs"));
    }
    data.directory
        .revalidate()
        .map_err(|_| failure("native Root query directory changed"))?;
    data.stage.invocation_context.revalidate()?;
    for (path, expected) in &data.read_inputs {
        if sha256_bytes(&read_bytes(path)?) != *expected {
            return Err(failure("native Root query input changed"));
        }
    }
    if measure_tree_content_cas(data.directory.path())
        .map_err(|_| failure("cannot remeasure native Root query evidence"))?
        != data.query_tree
    {
        return Err(failure("native Root query evidence changed"));
    }
    data.directory
        .revalidate()
        .map_err(|_| failure("native Root query directory changed"))?;
    Ok(())
}

fn selection_identity(
    selection: &MesaNativeRootQuerySelection<'_>,
) -> Result<Sha256Digest, ContractError> {
    digest_json(
        &serde_json::json!({"schema":"aros-mesa-native-root-query-v1",
        "output_root":selection.output_root,"variables":selection.variables,"goals":selection.goals}),
    )
}

fn validate_selection(
    request: &MesaPreflightRequest<'_>,
    selection: &MesaNativeRootQuerySelection<'_>,
) -> Result<Vec<String>, ContractError> {
    if selection.variables.is_empty()
        || selection.variables.len() > MAX_VARIABLES
        || selection.goals.is_empty()
        || selection.goals.len() > MAX_GOALS
        || selection
            .variables
            .iter()
            .chain(selection.goals)
            .fold(0usize, |size, value| size.saturating_add(value.len()))
            > MAX_SELECTION_BYTES
    {
        return Err(failure("native Root query selection exceeds its bounds"));
    }
    let mut keys = selection.variables.to_vec();
    if !keys.iter().any(|key| key == INPUT_LIST) {
        keys.push(INPUT_LIST.into());
    }
    super::super::mesa_preflight_measurement::validate_keys_unique(
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    // Reuse the closed startup grammar instead of inventing a second goal parser.
    let mut goals = vec!["-f".into(), "selected-root.mk".into()];
    goals.extend_from_slice(selection.goals);
    let admitted = LiteralMakeStartupArguments::admit(&goals)
        .map_err(|_| failure("native Root query goals are unsupported"))?;
    if admitted.goals() != selection.goals
        || !admitted.command_line_names().is_empty()
        || admitted.input_files() != ["selected-root.mk"]
        || !admitted.old_files().is_empty()
    {
        return Err(failure(
            "native Root query goals changed invocation semantics",
        ));
    }
    let parent = selection
        .output_root
        .parent()
        .ok_or_else(|| failure("native Root query output has no parent"))?;
    if !selection.output_root.is_absolute()
        || selection.output_root.file_name().is_none()
        || super::super::checked_directory(parent, "native query parent")? != parent
    {
        return Err(failure(
            "native Root query output must be a canonical absolute leaf",
        ));
    }
    validate_make_path(selection.output_root)?;
    let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment else {
        return Err(failure("native Root query requires sealed host tools"));
    };
    for root in [
        request.output_root,
        request.source_root,
        request.drivers.compiler_root(),
        host_tools.root.as_path(),
    ] {
        if aros_common::publication::filesystem_paths_overlap(selection.output_root, root)
            .map_err(|_| failure("cannot resolve native Root query output scope"))?
        {
            return Err(failure("native Root query output overlaps selected inputs"));
        }
    }
    if fs::symlink_metadata(selection.output_root).is_ok() {
        return Err(failure("native Root query output already exists"));
    }
    Ok(keys)
}
