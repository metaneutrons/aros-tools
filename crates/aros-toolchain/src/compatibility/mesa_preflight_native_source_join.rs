//! Independent Source-policy/actual collection coordinate join, not execution.

use super::{
    failure, read_bytes, read_global_inputs, BTreeSet, CancellationToken, CollectedMesaNativeRoot,
    ConfiguredProject, ContractError, LiteralMakeEnvironmentPrefix, MatchedOwnerChildCoordinates,
    MesaNativeCollectionSelection, MesaNativeHostBuildSelection, MesaNativeRootQuerySelection,
    MesaPreflightRequest, MetaMakeRegenerationPlan, NativeOwnerProjection, CONFIG,
};
use crate::compatibility::SourceMesaPreflightProfile;
use aros_common::make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot};
use aros_transpiler::native_owner_projection::SourceMetadataSelection;
use aros_transpiler::{
    dirs::DirVars, make_vars::MakeProgramPaths, native_owner_projection::MatchedChildSource,
};

/// Source-owned expected project and owner/goal set, prepared independently of
/// observer slots. The direct entry uses this before starting MetaMake; the
/// Root entry uses the same checks when joining its observations.
pub(super) struct PreparedNativeSourcePolicy {
    pub(super) project: ConfiguredProject,
    pub(super) projection: NativeOwnerProjection,
    pub(super) metadata_selection: SourceMetadataSelection,
    pub(super) generated_files: Vec<String>,
}

/// Bind policy to the original Source snapshot and configured-project bytes.
/// This function does not read generated owner files or observed call slots.
pub(super) fn prepare_native_source_policy(
    launch: &super::ObservedMesaNativeLaunchInputs,
    request: &MesaPreflightRequest<'_>,
    selected: &MesaNativeCollectionSelection<'_>,
    source_profile: &SourceMesaPreflightProfile,
    host: &str,
    roots: &[String],
    configuration_sha256: &aros_common::Sha256Digest,
) -> Result<PreparedNativeSourcePolicy, ContractError> {
    let source_identity = crate::compatibility::MesaPreflightProfile::Source(source_profile);
    source_profile.revalidate()?;
    if source_profile.source_root() != request.source_root
        || source_profile.target().target_triple != request.profile.target_triple()
        || source_profile.configure_target() != request.profile.configure_target()
        || source_identity.name() != request.profile.name()
        || source_identity.cpu() != request.profile.cpu()
        || source_identity.platform() != request.profile.platform()
        || source_identity.float_abi() != request.profile.float_abi()
    {
        return Err(failure(
            "native Source owner profile differs from original selection",
        ));
    }
    let build = launch
        .host
        .root
        .prepared
        .configured_invocation()?
        .working_directory();
    let original = read_bytes(&build.join(CONFIG))?;
    let project = ConfiguredProject::parse_only_project(
        std::str::from_utf8(&original).map_err(|_| failure("native project is not UTF-8"))?,
        selected.project,
    )
    .map_err(|_| failure("native project metadata is unsupported"))?;
    let plan = MetaMakeRegenerationPlan::derive(
        &launch.host.root.source_snapshot,
        &original,
        configuration_sha256,
        selected.project,
    )
    .map_err(|_| failure("cannot derive original Source owner discovery"))?;
    let files: Vec<_> = plan
        .discovered_inputs()
        .keys()
        .map(|logical| request.source_root.join(logical))
        .collect();
    let mut excluded = BTreeSet::new();
    if let Some(path) = launch
        .host
        .root
        .source_snapshot
        .excluded_generated_subtree()
    {
        excluded.insert(path.to_owned());
    }
    let projection =
        source_profile.native_owner_projection(&files, host, &excluded, request.local_source)?;
    if projection.original_inputs() != plan.discovered_inputs() {
        return Err(failure(
            "Source policy input referents differ from original discovery",
        ));
    }
    let expected_inputs: BTreeSet<_> = plan
        .owners()
        .iter()
        .map(|owner| owner.input.as_str())
        .collect();
    if projection.effective_inputs().collect::<BTreeSet<_>>() != expected_inputs {
        return Err(failure(
            "Source policy owner discovery differs from original collection",
        ));
    }
    for (relative, digest) in projection.snapshots() {
        let original = projection.original_input(relative).unwrap_or(relative);
        if launch
            .host
            .root
            .source_snapshot
            .regular_input_digest(original)
            .is_none_or(|original| original.as_str() != digest)
        {
            return Err(failure(
                "Source policy input differs from original held snapshot",
            ));
        }
    }
    let metadata_selection = projection
        .select_source_metadata(request.source_root, roots)
        .map_err(|_| failure("native Source metadata selection is unsupported"))?;
    Ok(PreparedNativeSourcePolicy {
        project,
        projection,
        metadata_selection,
        generated_files: plan.generated_files().to_vec(),
    })
}

/// Explicit Source-policy selection alongside the original collection scope.
pub struct MesaNativeSourceSelection<'a, 'b> {
    pub collection: &'a MesaNativeCollectionSelection<'b>,
    pub source_profile: &'a SourceMesaPreflightProfile,
    pub host: &'a str,
}

/// Original live collection plus its independently selected Source coordinates.
///
/// This is not leaf Make execution, compiler invocation or SDK qualification.
/// Generated Makefile content and MetaMake dispatch/order semantics still need
/// their own binding; exact-once metadata coverage is deliberately narrower.
pub struct SourceJoinedMesaNativeRoot {
    collection: CollectedMesaNativeRoot,
    profile: SourceMesaPreflightProfile,
    projection: NativeOwnerProjection,
    coordinates: Vec<MatchedOwnerChildCoordinates>,
    metadata_selection: SourceMetadataSelection,
}

impl SourceJoinedMesaNativeRoot {
    #[must_use]
    pub fn coordinates(&self) -> &[MatchedOwnerChildCoordinates] {
        &self.coordinates
    }

    /// Exact Source-derived selection and every proven optional leaf omission.
    #[must_use]
    pub const fn metadata_selection(&self) -> &SourceMetadataSelection {
        &self.metadata_selection
    }

    /// Retained original collection, not a reconstructed or renewed owner.
    #[must_use]
    pub const fn collection(&self) -> &CollectedMesaNativeRoot {
        &self.collection
    }

    /// Interpret every original observed child against the held Source/build.
    ///
    /// The original parent CAS selects the build subtree. Source identity and
    /// local exclusions remain those of the original request, not digests
    /// chosen from the current filesystem. No additional GNU query, copied
    /// option formula, recipe execution or portable owner is introduced.
    /// The initial state is the collection's instrumented first-input prelude;
    /// equivalence to an observer-free GNU invocation is not inferred here.
    /// Returned programs are input-consistent data; this owner must remain
    /// live and be revalidated before later producer or compiler use.
    ///
    /// # Errors
    /// Refuses original-owner drift, unbound roots/coordinates, unsupported
    /// Make semantics, cancellation or the original preparation deadline.
    pub fn read_children(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<MatchedChildSource>, ContractError> {
        self.revalidate(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        let parent = self
            .collection
            .launch
            .host
            .root
            .prepared
            .configured_invocation()?;
        let build = parent.working_directory();
        let relative = build
            .strip_prefix(request.output_root)
            .ok()
            .and_then(std::path::Path::to_str)
            .ok_or_else(|| failure("native Source build escaped original output root"))?;
        let build_selection = MakeInputRootSelection {
            root: build.to_owned(),
            content_sha256: self
                .collection
                .output_tree
                .subtree_payload_digest(relative)
                .ok_or_else(|| failure("native Source build subtree is not captured"))?,
        };
        // Link-library discovery is part of parsing the original child Make
        // program. Admit the independently selected compiler inventory, never
        // paths inferred from a wildcard or freshly adopted compiler bytes.
        let compiler_selection = MakeInputRootSelection {
            root: request.drivers.compiler_root().to_owned(),
            content_sha256: request.drivers.compiler_tree_sha256().clone(),
        };
        if self.coordinates.len() != self.collection.calls.len() {
            return Err(failure("native Source child coordinate count changed"));
        }
        // Native interpretation disables legacy directory-variable projection;
        // this empty table cannot supply omitted native values or aliases.
        let dirs = DirVars::default();
        let mut children = Vec::with_capacity(self.coordinates.len());
        for (coordinate, call) in self.coordinates.iter().zip(&self.collection.calls) {
            self.collection.launch.host.root.prepared.remaining()?;
            if call.binding().phase
                != aros_common::make_native_observation::MakeObservationPhase::FirstInputPrelude
            {
                return Err(failure(
                    "native Source child has a different observation phase",
                ));
            }
            let cwd = call.invocation().working_directory();
            let snapshot = request
                .local_source
                .map_or_else(
                    || {
                        MakeInputSnapshot::capture(
                            cwd,
                            &[
                                MakeInputRootSelection {
                                    root: request.source_root.to_owned(),
                                    content_sha256: request.source_tree_sha256.clone(),
                                },
                                build_selection.clone(),
                                compiler_selection.clone(),
                            ],
                        )
                    },
                    |local| {
                        MakeInputSnapshot::capture_with_local_source(
                            cwd,
                            request.source_root,
                            local,
                            &[build_selection.clone(), compiler_selection.clone()],
                        )
                    },
                )
                .map_err(|_| failure("cannot bind native Source Make input roots"))?;
            let relative_dir = std::path::Path::new(coordinate.generated_owner())
                .parent()
                .ok_or_else(|| failure("native Source generated owner has no directory"))?;
            let paths = MakeProgramPaths {
                dirs: &dirs,
                source_root: request.source_root,
                relative_dir,
                top_directory: build,
                working_directory: cwd,
            };
            let child = self
                .projection
                .read_retained_child_source(
                    &paths,
                    launch_query.goals,
                    coordinate.generated_owner(),
                    call,
                    &snapshot,
                )
                .map_err(|_| failure("cannot interpret original native Source child program"))?;
            if child.coordinates().generated_owner() != coordinate.generated_owner()
                || child.coordinates().source_input() != coordinate.source_input()
                || child.coordinates().goals() != coordinate.goals()
            {
                return Err(failure(
                    "interpreted native Source child coordinates changed",
                ));
            }
            children.push(child);
        }
        self.revalidate(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        Ok(children)
    }

    /// Retain declared output candidates under this original live owner.
    ///
    /// Scope and input digests come from the original launch/request/parent
    /// CAS. This neither admits external files nor grants compile/scheduling
    /// authority. Every child keeps its own original program and variable frame.
    /// # Errors
    /// Refuses child interpretation, incomplete/foreign/ambiguous source
    /// selection, original input drift, cancellation or the original deadline.
    pub fn read_declared_output_links(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<aros_transpiler::native_owner_projection::DeclaredChildOutputLinks, ContractError>
    {
        let children = self.read_children(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        let mut declared = Vec::with_capacity(children.len());
        for child in children {
            self.collection.launch.host.root.prepared.remaining()?;
            declared.push(
                child.trace_declared_rules().map_err(|_| {
                    failure("cannot trace original native Source child declarations")
                })?,
            );
        }
        let parent = self
            .collection
            .launch
            .host
            .root
            .prepared
            .configured_invocation()?;
        let build = parent.working_directory();
        let relative = build
            .strip_prefix(request.output_root)
            .ok()
            .and_then(std::path::Path::to_str)
            .ok_or_else(|| failure("native Source build escaped original output root"))?;
        let prepared_tree_sha256 = self
            .collection
            .output_tree
            .subtree_payload_digest(relative)
            .ok_or_else(|| failure("native Source build subtree is not captured"))?;
        let linked = self
            .projection
            .link_declared_child_outputs(
                &aros_transpiler::native_owner_projection::DeclaredChildJoinSelection {
                    source_root: request.source_root,
                    source_inputs_sha256: request.source_tree_sha256,
                    build_root: build,
                    prepared_tree_sha256: &prepared_tree_sha256,
                    roots: launch_query.goals,
                },
                declared,
            )
            .map_err(|_| failure("cannot bind selected native Source child output candidates"))?;
        self.revalidate(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        Ok(linked)
    }

    /// Recheck the original Source, runtime inputs, collection and clock.
    ///
    /// # Errors
    /// Refuses any original-owner/input drift, cancellation or expiry.
    pub fn revalidate(
        &self,
        request: &MesaPreflightRequest<'_>,
        host_query: &MesaNativeRootQuerySelection<'_>,
        host_build: &MesaNativeHostBuildSelection<'_>,
        launch_query: &MesaNativeRootQuerySelection<'_>,
        selected: &MesaNativeCollectionSelection<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(), ContractError> {
        self.collection.revalidate(
            request,
            host_query,
            host_build,
            launch_query,
            selected,
            cancellation,
        )?;
        self.profile.revalidate()?;
        self.projection
            .verify(request.source_root)
            .map_err(|_| failure("native Source owner inputs changed"))?;
        self.collection.launch.host.root.prepared.remaining()?;
        Ok(())
    }
}

/// Consume a live collection and join it to an independent sealed Source policy.
///
/// Actual generated Makefiles are not authority for the expected owner set.
/// The existing Source interpreter expands original GenMF inputs/templates;
/// every resulting input digest must match the original held Source snapshot.
/// A separate selected profile is required even for a producer-owned request.
/// No host/profile is guessed, and no deadline or snapshot is refreshed.
///
/// This proves only exact-once coordinate coverage. It does not independently
/// establish MetaMake scheduling semantics or successful leaf execution.
///
/// # Errors
/// Refuses changed owners, unbound profile/runtime configuration, incomplete
/// original discovery, metadata ambiguity, coordinate mismatch or stale budget.
pub fn join_mesa_native_source(
    collection: CollectedMesaNativeRoot,
    request: &MesaPreflightRequest<'_>,
    host_query: &MesaNativeRootQuerySelection<'_>,
    host_build: &MesaNativeHostBuildSelection<'_>,
    launch_query: &MesaNativeRootQuerySelection<'_>,
    source_selection: &MesaNativeSourceSelection<'_, '_>,
    cancellation: &CancellationToken,
) -> Result<SourceJoinedMesaNativeRoot, ContractError> {
    let selected = source_selection.collection;
    let source_profile = source_selection.source_profile;
    collection.revalidate(
        request,
        host_query,
        host_build,
        launch_query,
        selected,
        cancellation,
    )?;
    let launch = &collection.launch;
    let parent = launch.host.root.prepared.configured_invocation()?;
    let build = parent.working_directory();
    let PreparedNativeSourcePolicy {
        project,
        projection,
        metadata_selection,
        ..
    } = prepare_native_source_policy(
        launch,
        request,
        selected,
        source_profile,
        source_selection.host,
        launch_query.goals,
        &collection.configuration_sha256,
    )?;
    let prefix = LiteralMakeEnvironmentPrefix::parse(
        launch
            .values()
            .get("CALL")
            .ok_or_else(|| failure("native Source join lacks measured environment prefix"))?,
    )
    .map_err(|_| failure("native Source environment prefix is unsupported"))?;
    if collection.calls.is_empty() {
        return Err(failure(
            "native Source join lacks an observed runtime environment",
        ));
    }
    // Root may export variables in addition to CALL. Its parent environment
    // alone is not MetaMake's complete runtime input. Compare every supervised
    // observation; the selected Source MetaMake must preserve those values
    // when launching children. This is not process authentication.
    for call in &collection.calls {
        let environment = call.invocation().environment();
        if prefix
            .assignments()
            .iter()
            .any(|(name, value)| environment.get(name) != Some(value))
        {
            return Err(failure(
                "native Source environment differs from measured Root prefix",
            ));
        }
        let globals = read_global_inputs(
            &project,
            environment.clone(),
            &launch.query.read_inputs,
            build,
            request.source_root,
        )?;
        projection
            .match_runtime_configuration(request.source_root, &project, &globals)
            .map_err(|_| failure("native Source policy differs from runtime configuration"))?;
    }
    let mut children = Vec::with_capacity(collection.calls.len());
    for call in &collection.calls {
        let relative = call
            .invocation()
            .working_directory()
            .strip_prefix(build)
            .map_err(|_| failure("native Source child escaped selected build"))?;
        let owner = relative.join(&project.default_makefile);
        let owner = owner
            .to_str()
            .ok_or_else(|| failure("native Source child owner is not UTF-8"))?
            .to_owned();
        children.push((owner, call.invocation()));
    }
    let references: Vec<_> = children
        .iter()
        .map(|(owner, child)| (owner.as_str(), *child))
        .collect();
    let coordinates = projection
        .match_complete_child_coordinates(
            request.source_root,
            launch_query.goals,
            build,
            &references,
        )
        .map_err(|_| {
            failure("native Source selected child coordinates are incomplete or inconsistent")
        })?;
    let joined = SourceJoinedMesaNativeRoot {
        collection,
        profile: source_profile.clone(),
        projection,
        coordinates,
        metadata_selection,
    };
    joined.revalidate(
        request,
        host_query,
        host_build,
        launch_query,
        selected,
        cancellation,
    )?;
    Ok(joined)
}
