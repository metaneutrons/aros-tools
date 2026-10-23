//! Original-owner inventory/child association, not genmodule completion.

use super::{CollectedMesaNativeDirect, MesaNativeCollectionSelection, MesaNativeDirectOrigin};
use crate::compatibility::mesa_preflight::{
    failure, genmodule, invocation, read_bytes, run_prepared_observe, MesaPreflightRequest,
    NativeGenmoduleInventorySelection, SelectedNativeGenmoduleInventory,
};
use crate::ContractError;
use aros_common::{
    directory_entry_names_nofollow_bounded, make_native_invocation::PrivateNativeCaptureDirectory,
    measure_tree_content_cas, mesa_genmodule_collection::MESA_GENMODULE_SELECTION_FILE,
    mesa_genmodule_inventory::ValidatedMesaGenmoduleSelection,
    mesa_native_make::MesaNativeOwnerCall, publication::filesystem_paths_overlap,
    CancellationToken, Sha256Digest,
};
use std::{
    ffi::{OsStr, OsString},
    fs,
    os::unix::fs::DirBuilderExt as _,
    os::unix::fs::MetadataExt as _,
    path::Path,
};

const STDOUT: &str = "inventory.stdout";
const STDERR: &str = "inventory.stderr";

#[path = "mesa_preflight_native_direct_include.rs"]
mod included_makefile;
#[path = "mesa_preflight_native_direct_genmodule_data.rs"]
mod make_data;
pub use included_makefile::{GeneratedNativeMakeInclude, NativeMakeIncludeExecution};

impl CollectedMesaNativeDirect {
    /// Execute the selected scanner and associate its owners with original calls.
    ///
    /// Source/profile and the complete call roster stay with this live owner.
    /// Each selected producer must match exactly one observed Source input and
    /// goal. No inherited environment is reconstructed and no original call is
    /// replaced. This does not run genmodule, prove its module tuple from a
    /// parsed Source program, sample post-parse state or admit a build graph.
    ///
    /// # Errors
    /// Consumes this owner on failure: rejects drift, unsafe evidence scope,
    /// changed helper, invalid inventory, missing/ambiguous native coordinates,
    /// cancellation or expiry of the original preparation deadline.
    pub fn collect_genmodule_inventory(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        selection: &NativeGenmoduleInventorySelection<'_>,
    ) -> Result<(Self, SelectedNativeGenmoduleInventory), ContractError> {
        let (request, origin, collection, cancellation) = control;
        self.revalidate(request, origin, collection, cancellation)?;
        self.prepared.profile.revalidate()?;
        validate_evidence_scope(
            request,
            origin,
            collection,
            selection.helper,
            selection.evidence_root,
        )?;
        let root = &self.prepared.recorded.launch_inputs().host.root;
        let ports = root
            .prepared
            .configured
            .values
            .get("PORTSDIR")
            .ok_or_else(|| failure("native inventory lost its configured ports directory"))?;
        let path = selection.evidence_root.join(MESA_GENMODULE_SELECTION_FILE);
        let arguments = genmodule::inventory_arguments(
            request,
            &self.prepared.profile,
            selection.scope,
            &path,
            ports,
        )?;
        let context = invocation::InvocationContext::new(
            selection.helper,
            &arguments,
            request.source_root,
            root.prepared.configured_invocation()?.environment().clone(),
        )?;
        if &crate::compatibility::measure_executable(selection.helper)?.0 != selection.helper_sha256
        {
            return Err(failure("native genmodule inventory helper changed"));
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(selection.evidence_root)
            .map_err(|_| failure("cannot create fresh native inventory evidence"))?;
        let directory = PrivateNativeCaptureDirectory::open(selection.evidence_root)
            .map_err(|_| failure("cannot hold native inventory evidence"))?;
        let (stage, _) = run_prepared_observe(
            (request, cancellation, root.prepared.configured.started),
            "native-genmodule-inventory",
            context,
            0,
            |stdout, stderr| {
                directory
                    .write_new(OsStr::new(STDOUT), stdout)
                    .map_err(|_| failure("cannot retain native inventory stdout"))?;
                directory
                    .write_new(OsStr::new(STDERR), stderr)
                    .map_err(|_| failure("cannot retain native inventory stderr"))
            },
        )?;
        if &stage.program != selection.helper_sha256 {
            return Err(failure(
                "native genmodule inventory executed a changed helper",
            ));
        }
        check_evidence(&directory)?;
        let bytes = read_bytes(&path)?;
        let inventory = ValidatedMesaGenmoduleSelection::parse_bound(
            &bytes,
            &self
                .prepared
                .profile
                .genmodule_inventory_binding(selection.scope),
        )
        .map_err(|_| failure("native genmodule inventory differs from selected Source scope"))?;
        let mut required = Vec::with_capacity(inventory.producers().len());
        for producer in inventory.producers() {
            // Use the independently matched Source coordinate, not a guessed
            // default filename appended to the declaration directory.
            let mut matches = self.coordinates.iter().filter(|coordinate| {
                Path::new(coordinate.generated_owner()).parent()
                    == Some(Path::new(&producer.declaration_directory))
                    && coordinate.goals().contains(&producer.owner)
            });
            let coordinate = matches.next().ok_or_else(|| {
                failure("native genmodule producer lacks a unique original Source child")
            })?;
            if matches.next().is_some() {
                return Err(failure(
                    "native genmodule producer lacks a unique original Source child",
                ));
            }
            required.push((coordinate.source_input().to_owned(), producer.owner.clone()));
        }
        if required.is_empty() {
            return Err(failure(
                "native genmodule inventory has no selected producers",
            ));
        }
        let children = self
            .coordinates
            .iter()
            .zip(&self.calls)
            .map(|(coordinate, call)| (coordinate.generated_owner(), call.invocation()))
            .collect::<Vec<_>>();
        let slots = self
            .prepared
            .policy
            .projection
            .select_required_child_coordinates(
                request.source_root,
                &self.prepared.roots,
                self.prepared.invocation.working_directory(),
                &children,
                &required,
            )
            .map_err(|_| {
                failure("native genmodule producer lacks a unique original Source child")
            })?;
        let owners = inventory
            .producers()
            .iter()
            .zip(slots)
            .map(|(producer, call_index)| MesaNativeOwnerCall {
                owner: producer.owner.clone(),
                call_index,
            })
            .collect();
        let result = SelectedNativeGenmoduleInventory {
            inventory,
            owners,
            evidence: measure_tree_content_cas(directory.path())
                .map_err(|_| failure("cannot retain native inventory evidence identity"))?,
            directory,
            owner_sha256: owner_identity(&self)?,
            stage,
        };
        if result
            .evidence
            .regular_file_digest(MESA_GENMODULE_SELECTION_FILE)
            != Some(&aros_common::sha256_bytes(&bytes))
            || result.evidence.regular_file_digest(STDOUT) != Some(&result.stage.stdout)
            || result.evidence.regular_file_digest(STDERR) != Some(&result.stage.stderr)
        {
            return Err(failure(
                "native genmodule inventory evidence changed during selection",
            ));
        }
        result.revalidate(&self, control)?;
        Ok((self, result))
    }
}

impl SelectedNativeGenmoduleInventory {
    #[must_use]
    pub const fn inventory(&self) -> &ValidatedMesaGenmoduleSelection {
        &self.inventory
    }

    /// Original complete-collection slots, not a new invocation roster.
    #[must_use]
    pub fn owners(&self) -> &[MesaNativeOwnerCall] {
        &self.owners
    }

    /// Revalidate this association with the same live collection and clock.
    /// # Errors
    /// Refuses another owner, changed evidence/helper, input drift or expiry.
    pub fn revalidate(
        &self,
        owner: &CollectedMesaNativeDirect,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
    ) -> Result<(), ContractError> {
        let (request, origin, selected, cancellation) = control;
        owner.revalidate(request, origin, selected, cancellation)?;
        self.revalidate_evidence(owner)?;
        owner
            .prepared
            .recorded
            .launch_inputs()
            .host
            .root
            .prepared
            .remaining()?;
        Ok(())
    }

    // Association/evidence only, not original-owner input authentication. The
    // data producer uses this before its Source-child transaction performs the
    // full owner entry/post checks; full revalidate still precedes execution.
    fn revalidate_evidence(&self, owner: &CollectedMesaNativeDirect) -> Result<(), ContractError> {
        if owner_identity(owner)? != self.owner_sha256 {
            return Err(failure(
                "native genmodule inventory belongs to another collection",
            ));
        }
        self.stage.invocation_context.revalidate()?;
        check_evidence(&self.directory)?;
        if measure_tree_content_cas(self.directory.path())
            .map_err(|_| failure("cannot remeasure native inventory evidence"))?
            != self.evidence
        {
            return Err(failure("native genmodule inventory evidence changed"));
        }
        Ok(())
    }
}

fn owner_identity(owner: &CollectedMesaNativeDirect) -> Result<Sha256Digest, ContractError> {
    crate::compatibility::mesa_preflight::digest_json(&serde_json::json!({
        "domain":"native-genmodule-original-collection-v1",
        "directory":owner.directory.path(),
        "selection":owner.prepared.selection_sha256(),
        "invocation":owner.stage.invocation,
        "environment":owner.stage.environment,
        "evidence":owner.evidence_tree.payload_digest_excluding(None),
    }))
}

fn check_evidence(directory: &PrivateNativeCaptureDirectory) -> Result<(), ContractError> {
    directory
        .revalidate()
        .map_err(|_| failure("native inventory evidence directory changed"))?;
    // The actual selected scanner publishes durably. Keep its persistent lock
    // inode and admit only the exact name derived by that publisher, never a
    // wildcard for hidden files or lock-shaped names.
    let lock = aros_transpiler::publication::Publication::for_output(
        directory.path().join(MESA_GENMODULE_SELECTION_FILE),
    )
    .persistent_lock_path()
    .map_err(|_| failure("cannot derive native inventory publication lock"))?;
    let lock_file = aros_common::open_regular_file_nofollow(&lock)
        .map_err(|_| failure("native inventory publication lock is not a regular file"))?;
    let metadata = lock_file
        .metadata()
        .map_err(|_| failure("cannot inspect native inventory publication lock"))?;
    if metadata.len() != 0 || metadata.nlink() != 1 {
        return Err(failure(
            "native inventory publication lock must be empty and singly linked",
        ));
    }
    let mut names = directory_entry_names_nofollow_bounded(directory.path(), 4)
        .map_err(|_| failure("native inventory evidence membership changed"))?;
    names.sort();
    let mut expected: [OsString; 4] = [
        MESA_GENMODULE_SELECTION_FILE.into(),
        STDOUT.into(),
        STDERR.into(),
        lock.file_name()
            .ok_or_else(|| failure("native inventory publication lock lacks a name"))?
            .to_owned(),
    ];
    expected.sort();
    if names.as_slice() != expected {
        return Err(failure("native inventory evidence membership changed"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "mesa_preflight_native_inventory_evidence_tests.rs"]
mod evidence_tests;

fn validate_evidence_scope(
    request: &MesaPreflightRequest<'_>,
    origin: &MesaNativeDirectOrigin<'_, '_>,
    collection: &MesaNativeCollectionSelection<'_>,
    helper: &Path,
    evidence_root: &Path,
) -> Result<(), ContractError> {
    if !helper.is_absolute() || !evidence_root.is_absolute() {
        return Err(failure("native inventory selection must be absolute"));
    }
    let mut protected: Vec<&Path> = vec![
        request.source_root,
        request.output_root,
        request.drivers.compiler_root(),
        helper,
        collection.output_root,
        origin.host_query.output_root,
        origin.host_build.output_root,
        origin.launch_query.output_root,
        origin.dispatch.output_root,
    ];
    if let crate::compatibility::CompatibilityEnvironment::SealedHostTools { host_tools, .. } =
        request.environment
    {
        protected.push(&host_tools.root);
    }
    for input in protected {
        if filesystem_paths_overlap(evidence_root, input)
            .map_err(|_| failure("cannot compare native inventory evidence scope"))?
        {
            return Err(failure(
                "native inventory evidence overlaps selected inputs",
            ));
        }
    }
    Ok(())
}
