//! Source-bound preparation for direct MetaMake, never an ordinary Root goal.
//!
//! Preparation retains the original recorder owner and clock. It admits no
//! process or generated output yet; collection must consume this object and
//! separately bind the confined observer and exact fresh-output transition.
use super::source_join::{prepare_native_source_policy, PreparedNativeSourcePolicy};
use super::{
    check_native_options, failure, read_bytes, read_global_inputs, selection_identity,
    MesaNativeCollectionSelection, MesaNativeHostBuildSelection, MesaNativeRootQuerySelection,
    MesaNativeSourceSelection, MesaPreflightRequest, CONFIG,
};
use crate::compatibility::{
    MesaNativeDispatchSelection, RecordedMesaNativeDispatch, SourceMesaPreflightProfile,
};
use crate::ContractError;
use aros_common::{
    make_environment_prefix::LiteralMakeEnvironmentPrefix,
    make_native_invocation::NativeMakeInvocation, publication::filesystem_paths_overlap,
    sha256_bytes, CancellationToken, Sha256Digest,
};
use std::{collections::BTreeSet, path::Path};

#[path = "mesa_preflight_native_direct_runtime.rs"]
mod runtime;
pub use runtime::{
    CollectedMesaNativeDirect, ExecutedMesa26OriginalRecipe, GeneratedNativeMakeInclude,
    Mesa26GeneratedPathListSelection, Mesa26OriginalRecipeExecution, NativeMakeIncludeExecution,
};

/// An original live recorder with a separately selected Source owner closure.
///
/// The command's environment is explicit checked recorder-derived data, not a
/// claim of observer-free Root equivalence. No Root or leaf Make is run here.
pub struct PreparedMesaNativeDirectCollection {
    recorded: RecordedMesaNativeDispatch,
    profile: SourceMesaPreflightProfile,
    policy: PreparedNativeSourcePolicy,
    invocation: NativeMakeInvocation,
    roots: Vec<String>,
    selection_sha256: Sha256Digest,
    collection_sha256: Sha256Digest,
    configuration_sha256: Sha256Digest,
}

/// Original query/host/recorder selections, not a new owner or renewed clock.
pub struct MesaNativeDirectOrigin<'a, 'b> {
    pub host_query: &'a MesaNativeRootQuerySelection<'b>,
    pub host_build: &'a MesaNativeHostBuildSelection<'b>,
    pub launch_query: &'a MesaNativeRootQuerySelection<'b>,
    pub dispatch: &'a MesaNativeDispatchSelection<'b>,
}

impl PreparedMesaNativeDirectCollection {
    #[must_use]
    pub const fn invocation(&self) -> &NativeMakeInvocation {
        &self.invocation
    }

    /// Complete direct selection identity for the later observer context.
    #[must_use]
    pub const fn selection_sha256(&self) -> &Sha256Digest {
        &self.selection_sha256
    }

    #[must_use]
    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    #[must_use]
    pub const fn metadata_selection(
        &self,
    ) -> &aros_transpiler::native_owner_projection::SourceMetadataSelection {
        &self.policy.metadata_selection
    }

    /// Recheck the original recorder, exact collection selection and Source
    /// policy; the original total preparation clock is never renewed.
    /// # Errors
    /// Refuses reselection, drift, cancellation or expiry before any launch.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        origin: &MesaNativeDirectOrigin<'_, '_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.recorded.revalidate(
            request,
            origin.host_query,
            origin.host_build,
            origin.launch_query,
            origin.dispatch,
            cancellation,
        )?;
        self.revalidate_policy(request, selected)
    }

    fn revalidate_policy(
        &self,
        request: &MesaPreflightRequest<'_>,
        selected: &MesaNativeCollectionSelection<'_>,
    ) -> Result<(), ContractError> {
        if selection_identity(selected)? != self.collection_sha256 {
            return Err(failure("direct native collection selection changed"));
        }
        self.profile.revalidate()?;
        self.policy
            .projection
            .verify(request.source_root)
            .map_err(|_| failure("direct native Source policy changed"))?;
        if sha256_bytes(&read_bytes(
            &self.invocation.working_directory().join(CONFIG),
        )?) != self.configuration_sha256
        {
            return Err(failure("direct native project configuration changed"));
        }
        Ok(())
    }
}

/// Prepare a distinct direct entry without admitting non-quick Root recipes.
///
/// The requested Source roots may include ordinary MetaMake goals. Every
/// existing owner in their metadata selection must be captured exactly once.
/// Unknown dependency nodes remain in `metadata_selection`; this data-only
/// preparation does not prove dependency completeness or successful producers.
/// Unknown requested roots refuse before any future process/observer runs.
/// # Errors
/// Refuses foreign program/options/project/cwd, unknown requested Source roots,
/// runtime-policy mismatch, overlapping evidence, owner drift or expired clock.
pub fn prepare_mesa_native_direct_collection(
    recorded: RecordedMesaNativeDispatch,
    request: &MesaPreflightRequest<'_>,
    origin: &MesaNativeDirectOrigin<'_, '_>,
    source: &MesaNativeSourceSelection<'_, '_>,
    roots: &[String],
    cancellation: &CancellationToken,
) -> Result<PreparedMesaNativeDirectCollection, ContractError> {
    let MesaNativeDirectOrigin {
        host_query,
        host_build,
        launch_query,
        dispatch,
    } = *origin;
    recorded.revalidate(
        request,
        host_query,
        host_build,
        launch_query,
        dispatch,
        cancellation,
    )?;
    let selected = source.collection;
    let launch = recorded.launch_inputs();
    super::validate_scope(
        launch,
        request,
        host_query,
        host_build,
        launch_query,
        selected,
    )?;
    if filesystem_paths_overlap(selected.output_root, recorded.evidence_root())
        .map_err(|_| failure("cannot compare direct native recorder scope"))?
    {
        return Err(failure(
            "direct native collection overlaps recorder evidence",
        ));
    }
    if selected.project != dispatch.project {
        return Err(failure(
            "direct native project differs from captured Source dispatch",
        ));
    }
    let build = launch
        .host
        .root
        .prepared
        .configured_invocation()?
        .working_directory();
    let options = launch
        .values()
        .get(dispatch.options_variable)
        .ok_or_else(|| failure("direct native entry lacks measured original options"))?;
    check_native_options(options, request.source_root, build)?;
    let options = aros_common::make_child_command::LiteralMakeChildCommand::parse(&format!(
        "metamake {options}"
    ))
    .map_err(|_| failure("direct native original options are not literal"))?;
    let [original_goal] = launch_query.goals else {
        return Err(failure(
            "direct native entry requires its one original captured goal",
        ));
    };
    let mut original_arguments = options.arguments().to_vec();
    original_arguments.push(format!("{}.{}", selected.project, original_goal));
    let invocation = direct_invocation(
        recorded.captured().invocation(),
        launch.host.executable(),
        build,
        &original_arguments,
        selected.project,
        roots,
    )?;
    let prefix = launch
        .values()
        .get(dispatch.prefix_variable)
        .ok_or_else(|| failure("direct native entry lacks its selected Source prefix"))?;
    check_prefix(prefix, &invocation)?;
    let coordinate = build
        .join(CONFIG)
        .strip_prefix(request.output_root)
        .ok()
        .and_then(Path::to_str)
        .map(str::to_owned)
        .ok_or_else(|| failure("direct native configuration escaped original output"))?;
    let configuration_sha256 = launch
        .host
        .output_tree
        .regular_file_digest(&coordinate)
        .ok_or_else(|| failure("direct native configuration is not an original regular output"))?
        .clone();
    let configuration = read_bytes(&build.join(CONFIG))?;
    if sha256_bytes(&configuration) != configuration_sha256 {
        return Err(failure(
            "direct native configuration differs from original host output",
        ));
    }
    crate::compatibility::mesa_preflight::native_collection_observer::CollectionObserver::check_project_layout(
        selected.output_root, &configuration, &configuration_sha256,
        selected.project, selected.source_command_word,
    )?;
    let policy = prepare_native_source_policy(
        launch,
        request,
        selected,
        source.source_profile,
        source.host,
        roots,
        &configuration_sha256,
    )?;
    // This collection retains actual existing owner calls, not a successful
    // dependency graph. MetaMake skips unknown subtargets; keep those names in
    // metadata_selection rather than inventing producers or omitting evidence.
    // Unknown explicitly requested roots still refuse before native collection.
    if roots.iter().any(|root| {
        policy
            .metadata_selection
            .selection()
            .missing_endpoints
            .contains(root)
    }) || !policy
        .metadata_selection
        .selection()
        .unbound_native_endpoints
        .is_empty()
        || policy
            .metadata_selection
            .selection()
            .selected_owner_targets
            .is_empty()
    {
        return Err(failure(
            "direct native Source requested roots are unknown or have no bound owners",
        ));
    }
    check_capacity(
        selected.maximum_calls,
        policy
            .metadata_selection
            .selection()
            .selected_owner_targets
            .values()
            .map(BTreeSet::len),
    )?;
    let globals = read_global_inputs(
        &policy.project,
        invocation.environment().clone(),
        &launch.query.read_inputs,
        build,
        request.source_root,
    )?;
    policy
        .projection
        .match_runtime_configuration(request.source_root, &policy.project, &globals)
        .map_err(|_| {
            failure("direct native Source policy differs from captured runtime configuration")
        })?;
    let collection_sha256 = selection_identity(selected)?;
    let selection_sha256 = crate::compatibility::mesa_preflight::digest_json(&serde_json::json!({
        "collection": collection_sha256, "roots": roots, "host": source.host,
        "source_profile": crate::compatibility::MesaPreflightProfile::Source(source.source_profile).document_sha256(),
        "source_policy": policy.projection.policy_sha256(),
        "invocation": sha256_bytes(&invocation.to_bytes().map_err(|_| failure("cannot encode direct native selection"))?),
        "configuration": configuration_sha256,
    }))?;
    let prepared = PreparedMesaNativeDirectCollection {
        recorded,
        profile: source.source_profile.clone(),
        policy,
        invocation,
        roots: roots.to_vec(),
        selection_sha256,
        collection_sha256,
        configuration_sha256,
    };
    prepared.revalidate(request, origin, selected, cancellation)?;
    Ok(prepared)
}

fn check_prefix(prefix: &str, invocation: &NativeMakeInvocation) -> Result<(), ContractError> {
    let prefix = LiteralMakeEnvironmentPrefix::parse(prefix)
        .map_err(|_| failure("direct native Source environment prefix is unsupported"))?;
    if prefix
        .assignments()
        .iter()
        .any(|(name, value)| invocation.environment().get(name) != Some(value))
    {
        return Err(failure(
            "direct native environment differs from its selected Source prefix",
        ));
    }
    Ok(())
}

fn check_capacity(
    maximum_calls: usize,
    goal_counts: impl IntoIterator<Item = usize>,
) -> Result<(), ContractError> {
    // MetaMake may invoke the same owner separately for each reached target.
    // Directory count cannot bound those calls. The Source-selected pair set
    // is independent of observer slots and remains the coverage authority.
    let expected_calls = goal_counts.into_iter().try_fold(0usize, |total, count| {
        if count == 0 {
            return None;
        }
        total.checked_add(count)
    });
    if !(1..=aros_common::make_native_collection::MAX_NATIVE_MAKE_CALLS).contains(&maximum_calls)
        || expected_calls.is_none_or(|count| count == 0 || count > maximum_calls)
    {
        return Err(failure(
            "direct native observer capacity cannot cover selected owner-goal pairs",
        ));
    }
    Ok(())
}

fn direct_invocation(
    captured: &NativeMakeInvocation,
    program: &Path,
    build: &Path,
    original_arguments: &[String],
    project: &str,
    roots: &[String],
) -> Result<NativeMakeInvocation, ContractError> {
    let word = |value: &str| {
        !value.is_empty()
            && value.len() <= 4096
            && !value.starts_with('-')
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.+-".contains(&c))
    };
    if !program.is_absolute()
        || captured.argument_zero() != program.to_str()
        || captured.working_directory() != build
        || captured.arguments() != original_arguments
        || original_arguments.last().is_none_or(|last| {
            last.strip_prefix(project)
                .and_then(|value| value.strip_prefix('.'))
                .is_none_or(|goal| !word(goal))
        })
        || !word(project)
        || project.contains('.')
        || roots.is_empty()
        || roots.len() > 256
        || roots.iter().any(|root| !word(root))
        || roots.iter().collect::<BTreeSet<_>>().len() != roots.len()
    {
        return Err(failure(
            "direct native command differs from selected literal Source entry",
        ));
    }
    let mut arguments = original_arguments[..original_arguments.len() - 1].to_vec();
    arguments.extend(roots.iter().map(|root| format!("{project}.{root}")));
    NativeMakeInvocation::new(arguments, build.to_owned(), captured.environment().clone())
        .and_then(|value| value.with_argument_zero(program.to_str().unwrap().to_owned()))
        .map_err(|_| failure("direct native command exceeds invocation bounds"))
}

#[cfg(test)]
#[path = "mesa_preflight_native_direct_tests.rs"]
mod tests;
