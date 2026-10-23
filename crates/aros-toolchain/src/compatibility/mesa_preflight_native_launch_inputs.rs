//! Query Root in the selected launch-goal context without renewing ownership.

use super::super::{query_root_inputs, revalidate_query_data, NativeRootQueryData};
use super::{
    check_host_role_values, failure, BuiltMesaNativeHost, MesaNativeHostBuildSelection,
    MesaNativeRootQuerySelection, MesaPreflightRequest,
};
use crate::ContractError;
use aros_common::{publication::filesystem_paths_overlap, CancellationToken};
use std::{collections::BTreeMap, path::PathBuf};

#[path = "mesa_preflight_native_collection.rs"]
mod collection;
#[path = "mesa_preflight_native_dispatch.rs"]
mod dispatch;
pub use collection::{
    GeneratedNativeMakeInclude, NativeMakeIncludeExecution,
    collect_mesa_native_root, join_mesa_native_source, prepare_mesa_native_direct_collection,
    CollectedMesaNativeDirect, CollectedMesaNativeRoot, ExecutedMesa26OriginalRecipe,
    Mesa26GeneratedPathListSelection, Mesa26OriginalRecipeExecution, MesaNativeCollectionSelection,
    MesaNativeDirectOrigin, MesaNativeSourceSelection, PreparedMesaNativeDirectCollection,
    SourceJoinedMesaNativeRoot,
};
pub use dispatch::{
    observe_mesa_native_dispatch, MesaNativeDispatchSelection, ObservedFencedMesaNativeDispatch,
    ObservedMesaNativeDispatch, RecordedMesaNativeDispatch,
};

/// Original host producer plus a live Root query for the selected launch goals.
///
/// This is not a Root launch, observed child collection, native completion or
/// graph admission. Values retain the actual selected goal context rather than
/// assuming that the earlier host-build goal has the same parse-time inputs.
pub struct ObservedMesaNativeLaunchInputs {
    host: BuiltMesaNativeHost,
    query: NativeRootQueryData,
}

impl ObservedMesaNativeLaunchInputs {
    /// Actual Source values in this launch-goal context, not copied formulas.
    #[must_use]
    pub const fn values(&self) -> &BTreeMap<String, String> {
        &self.query.values
    }

    /// Observed, byte-bound Root input list in the selected launch context.
    /// This is not an independent completeness or authentication assertion.
    #[must_use]
    pub fn read_inputs(&self) -> &[(PathBuf, aros_common::Sha256Digest)] {
        &self.query.read_inputs
    }

    /// Revalidate the original owner, clock, host bytes and both query contexts.
    ///
    /// # Errors
    /// Refuses reselection, expiry/cancellation, changed inputs, host artifacts
    /// or retained query evidence. No snapshot is refreshed after execution.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.host
            .revalidate(request, host_query, host_build, cancellation)?;
        revalidate_query_data(&self.query, launch_query, cancellation)?;
        self.host.root.prepared.remaining()?;
        Ok(())
    }

    // Only the consuming collection phase may use this check, paired with its
    // own exact post-host -> post-collection output transition. Ordinary launch
    // input revalidation above continues to require the full post-host tree.
    fn revalidate_inputs(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.host
            .revalidate_inputs(request, host_query, host_build, cancellation)?;
        revalidate_query_data(&self.query, launch_query, cancellation)?;
        self.host.root.prepared.remaining()?;
        Ok(())
    }
}

/// Consume the original host producer and query Root for its intended launch.
///
/// GNU stops at the same exact selected fatal-query protocol before include
/// remaking or target recipes. The original post-host tree binds inputs, and
/// the original producer's clock and source/compiler selection remain active.
///
/// # Errors
/// Refuses unbound roles, overlapping evidence, goal/input drift, changed host
/// state, unsafe query results, cancellation or expiry. Does not launch MetaMake.
pub fn observe_mesa_native_launch_inputs(
    host: BuiltMesaNativeHost,
    request: &MesaPreflightRequest<'_>,
    host_query: &MesaNativeRootQuerySelection<'_>,
    host_build: &MesaNativeHostBuildSelection<'_>,
    launch_query: &MesaNativeRootQuerySelection<'_>,
    cancellation: &CancellationToken,
) -> Result<ObservedMesaNativeLaunchInputs, ContractError> {
    // The query's owner callback performs the complete check immediately
    // before creating evidence. The selection/overlap checks below have no
    // effects, so do not duplicate that full Source scan at entry.
    for required in [
        "CROSSTOOLS_TARGET",
        "HOST_CC",
        "HOST_MAKE",
        host_build.executable_variable,
    ] {
        if !launch_query.variables.iter().any(|name| name == required) {
            return Err(failure("native launch query lacks a selected host role"));
        }
    }
    for original in [host.root.directory.path(), host.directory.path()] {
        if filesystem_paths_overlap(original, launch_query.output_root)
            .map_err(|_| failure("cannot compare native launch query scope"))?
        {
            return Err(failure("native launch query overlaps original evidence"));
        }
    }
    let query = query_root_inputs(
        request,
        launch_query,
        cancellation,
        &host.root.prepared,
        &host.root.source_snapshot,
        &host.output_tree,
        || host.revalidate(request, host_query, host_build, cancellation),
    )?;
    if query
        .values
        .get(host_build.executable_variable)
        .is_none_or(|value| std::path::Path::new(value) != host.executable.as_path())
    {
        return Err(failure(
            "native launch goal selected a different host executable",
        ));
    }
    check_host_role_values(
        &query.values,
        host.root.prepared.configured_invocation()?.program_sha256(),
        request,
    )?;
    let result = ObservedMesaNativeLaunchInputs { host, query };
    result.revalidate(request, host_query, host_build, launch_query, cancellation)?;
    Ok(result)
}
