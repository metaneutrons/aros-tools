//! Fixed Genmodule Make DATA operation under the original live collection.
//! No original writefiles recipe or project stamp is executed here.

use super::{
    owner_identity, CollectedMesaNativeDirect, MesaNativeCollectionSelection,
    MesaNativeDirectOrigin,
};
use crate::{
    compatibility::mesa_preflight::{
        failure, invocation::InvocationContext, read_bytes, run_prepared_observe,
        MesaPreflightRequest, NativeGenmoduleMakeData, NativeGenmoduleMakeSelection,
        SelectedNativeGenmoduleInventory,
    },
    ContractError,
};
use aros_common::{
    captured_programs::{CapturedProgramSet, ProgramRootSelection, ProgramSelection},
    directory_entry_names_nofollow_bounded,
    make_native_invocation::PrivateNativeCaptureDirectory,
    measure_tree_content_cas,
    mesa_genmodule_collection::MesaGenmoduleOutputBinding,
    mesa_genmodule_inventory::MesaGenmoduleProducer,
    mesa_genmodule_make::MesaGenmoduleMakefile,
    publication::filesystem_paths_overlap,
    sha256_bytes, CancellationToken, Sha256Digest,
};
use aros_transpiler::{
    make_shell_data::{
        artifact_binding::{RecipeArtifactDirection, RecipeArtifactRequest, RecipeArtifactSlot},
        LiteralRecipeStatement,
    },
    native_owner_projection::RetainedChildSourceBatchLimits,
};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
};

const ROLE: &str = "source-genmodule";

struct PreparedData {
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
    inherited_environment: BTreeMap<String, String>,
    directory: PathBuf,
    configurations: Vec<Sha256Digest>,
    source_input: PathBuf,
}

impl CollectedMesaNativeDirect {
    /// Produce Make data using the original Source tuple, environment and host tool.
    ///
    /// This is a fixed `writemakefile` data operation, not execution of the
    /// selected project's `writefiles` recipe. Only its validated destination
    /// and operation words are replaced; original configuration/module words
    /// stay ordered. It neither creates project stubs/stamps nor satisfies
    /// prerequisite scheduling or productive native graph admission.
    ///
    /// # Errors
    /// Consumes the owner on failure. Refuses foreign inventory/Source/tool,
    /// unsupported literal recipe or engine state, unsafe paths, changed inputs,
    /// unexpected output, process failure, cancellation or original deadline.
    pub fn produce_genmodule_make_data(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        inventory: &SelectedNativeGenmoduleInventory,
        limits: RetainedChildSourceBatchLimits,
        selection: &NativeGenmoduleMakeSelection<'_>,
    ) -> Result<(Self, NativeGenmoduleMakeData), ContractError> {
        let (request, origin, collection, cancellation) = control;
        self.prepared
            .recorded
            .launch_inputs()
            .host
            .root
            .prepared
            .remaining()?;
        if cancellation.is_cancelled() {
            return Err(failure("native genmodule data preparation cancelled"));
        }
        inventory.revalidate_evidence(&self)?;
        let (producer, slot) = selected_producer(inventory, selection.owner)?;
        validate_scope(
            request,
            origin,
            collection,
            inventory,
            selection.evidence_root,
        )?;
        let (program, programs) = self.genmodule_program(request)?;
        let mut prepared =
            self.prepare_genmodule_data(control, limits, producer, slot, selection, &programs)?;
        let name = format!("Makefile.{}{}", producer.module, producer.module_type);
        let count = prepared.arguments.len();
        // The validated interface ends in: -d DEST writefiles MODULE TYPE.
        selection
            .evidence_root
            .to_str()
            .ok_or_else(|| failure("native genmodule data path is not text"))?
            .clone_into(&mut prepared.arguments[count - 4]);
        prepared.arguments[count - 3] = "writemakefile".into();
        let root = &self.prepared.recorded.launch_inputs().host.root;
        let mut inherited = root.prepared.configured_invocation()?.environment().clone();
        // The selected child inherits Source's configured tool-search prefix,
        // not the earlier root preparation PATH. Retain that exact owner-call
        // baseline from the checked reader transaction. Host-tool identities
        // remain selected; this fixed absolute program gains no shell policy.
        inherited.variables = prepared.inherited_environment;
        let environment = crate::compatibility::recipe_shell::captured_recipe_environment(
            inherited,
            &prepared.environment,
        )?;
        let context = InvocationContext::new(
            &program,
            &prepared.arguments,
            &prepared.directory,
            environment,
        )?;
        // The Source callbacks above were data-only; launch follows their final
        // snapshot checks and another owning check, never inside a callback.
        programs
            .verify()
            .map_err(|_| failure("native genmodule host program changed"))?;
        inventory.revalidate(&self, control)?;
        // Refused environments, programs and owners leave no DATA directory.
        fs::DirBuilder::new()
            .mode(0o700)
            .create(selection.evidence_root)
            .map_err(|_| failure("cannot create fresh native genmodule data directory"))?;
        let directory = PrivateNativeCaptureDirectory::open(selection.evidence_root)
            .map_err(|_| failure("cannot retain native genmodule data directory"))?;
        let (stage, _) = run_prepared_observe(
            (request, cancellation, root.prepared.configured.started),
            "native-genmodule-make-data",
            context,
            0,
            |stdout, stderr| {
                directory
                    .write_new(OsStr::new("stdout"), stdout)
                    .map_err(|_| failure("cannot retain genmodule data stdout"))?;
                directory
                    .write_new(OsStr::new("stderr"), stderr)
                    .map_err(|_| failure("cannot retain genmodule data stderr"))
            },
        )?;
        let bytes = read_bytes(&directory.path().join(&name))?;
        let makefile = MesaGenmoduleMakefile::parse(
            &producer.module,
            std::str::from_utf8(&bytes)
                .map_err(|_| failure("native genmodule Make data is not UTF-8"))?,
        )
        .map_err(|_| failure("native genmodule Make data has unsupported assignments"))?;
        let names = directory_entry_names_nofollow_bounded(directory.path(), 4)
            .map_err(|_| failure("cannot inspect native genmodule data outputs"))?;
        let mut expected: Vec<std::ffi::OsString> =
            vec![name.into(), "stdout".into(), "stderr".into()];
        expected.sort();
        if names != expected {
            return Err(failure("native genmodule data has unexpected outputs"));
        }
        let result = NativeGenmoduleMakeData {
            evidence: measure_tree_content_cas(directory.path())
                .map_err(|_| failure("cannot retain native genmodule data identity"))?,
            directory,
            stage,
            source_input: prepared.source_input,
            makefile,
            output: MesaGenmoduleOutputBinding {
                producer: producer.clone(),
                makefile_sha256: sha256_bytes(&bytes),
                base_configuration_sha256: prepared.configurations[0].clone(),
                override_configuration_sha256: prepared.configurations.get(1).cloned(),
            },
            owner_sha256: owner_identity(&self)?,
            inventory_sha256: inventory.evidence.payload_digest_excluding(None),
        };
        programs
            .verify()
            .map_err(|_| failure("native genmodule program changed after data generation"))?;
        result.revalidate(&self, control, inventory)?;
        Ok((self, result))
    }

    pub(super) fn genmodule_program(
        &self,
        request: &MesaPreflightRequest<'_>,
    ) -> Result<(PathBuf, CapturedProgramSet), ContractError> {
        let host = &self.prepared.recorded.launch_inputs().host;
        let path =
            PathBuf::from(host.root.values.get("GENMODULE").ok_or_else(|| {
                failure("native genmodule host role was not independently queried")
            })?);
        let relative = path
            .strip_prefix(request.output_root)
            .ok()
            .and_then(Path::to_str)
            .ok_or_else(|| failure("native genmodule role is outside original host outputs"))?;
        let digest = host
            .output_tree
            .regular_file_digest(relative)
            .ok_or_else(|| {
                failure("native genmodule role is not an original regular host output")
            })?;
        if host
            .root
            .prepared
            .output_snapshot()
            .entry_kind(relative)
            .is_some()
        {
            return Err(failure(
                "native genmodule role was not produced by the original host build",
            ));
        }
        if self.output_tree.regular_file_digest(relative) != Some(digest) {
            return Err(failure(
                "native genmodule host output changed during collection",
            ));
        }
        let parent = path
            .parent()
            .ok_or_else(|| failure("native genmodule host role has no directory"))?;
        let prefix = parent
            .strip_prefix(request.output_root)
            .ok()
            .and_then(Path::to_str)
            .ok_or_else(|| failure("native genmodule host directory escaped original outputs"))?;
        let programs = CapturedProgramSet::capture(
            &[ProgramRootSelection {
                root: parent.to_owned(),
                content_sha256: self
                    .output_tree
                    .subtree_payload_digest(prefix)
                    .ok_or_else(|| failure("native genmodule host namespace is not captured"))?,
            }],
            &[ProgramSelection {
                role: ROLE.into(),
                root_index: 0,
                invocation_path: path.clone(),
                executable_sha256: digest.clone(),
            }],
        )
        .map_err(|_| failure("native genmodule host role differs from original output identity"))?;
        Ok((path, programs))
    }

    fn prepare_genmodule_data(
        &self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
        producer: &MesaGenmoduleProducer,
        slot: usize,
        selection: &NativeGenmoduleMakeSelection<'_>,
        programs: &CapturedProgramSet,
    ) -> Result<PreparedData, ContractError> {
        let (request, origin, collection, cancellation) = control;
        self.with_source_child_batch(request, origin, collection, cancellation, |snapshot, children, check_control| {
            let child = children.get(slot).ok_or("native genmodule original child is absent")?;
            self.prepared.policy.projection.with_retained_child_recipe_reader(
                &self.prepared.roots, (child, selection.recipe_index), snapshot, limits, check_control,
                |recipes, recipe, _, coordinate, reader| {
                if !coordinate.goals().iter().any(|goal| goal == &producer.owner)
                    || Path::new(coordinate.source_input()).parent() != Some(Path::new(&producer.declaration_directory)) {
                    return Err("native genmodule Source coordinate differs from inventory owner".into());
                }
                if !recipes.declared().traversal().scope_chain(recipe.visit())?.iter().any(|target| *target == producer.owner) {
                    return Err("native genmodule recipe is outside the selected owner dependency path".into());
                }
                let shell = recipe.expression().parse_literal_shell()?;
                if shell.prefixes.ignore_errors || shell.prefixes.explicit_recursive {
                    return Err("native genmodule data recipe has unsupported Make controls".into());
                }
                let [LiteralRecipeStatement::Command(command)] = shell.statements.as_slice() else {
                    return Err("native genmodule data requires one literal command".into());
                };
                let slots = aros_transpiler::genmodule_writefiles_rules::validate_native_writefiles_tuple(
                    command, producer, request.source_root, selection.nominal_destination,
                )?;
                let requests = slots.iter().map(|index| RecipeArtifactRequest {
                    recipe: 0, command: 0, slot: RecipeArtifactSlot::Word {index: *index, range: 0..command.words()[*index].len()}, direction: RecipeArtifactDirection::Input,
                }).collect::<Vec<_>>();
                let (arguments, configurations) = recipes.with_recipe_input_bindings_in_reader(selection.recipe_index, programs, &requests, reader, |binding, inputs| {
                    if binding.commands().len() != 1 {return Err("native genmodule command count changed".into());}
                    let bound = binding.unique_role(ROLE)?;
                    if inputs.len() != slots.len() || inputs.iter().zip(&slots).any(|(input, index)| input.path() != Path::new(&command.words()[*index])) {
                        return Err("native genmodule configuration is not an exact nonlinked Source path".into());
                    }
                    let hashes = inputs.iter().map(|input| input.input_sha256().cloned().ok_or_else(|| "native genmodule configuration is not a captured input".to_owned())).collect::<Result<Vec<_>, String>>()?;
                    Ok((bound.command().words()[1..].to_vec(), hashes))
                })?;
                Ok(PreparedData {
                    arguments, configurations,
                    source_input: PathBuf::from(coordinate.source_input()),
                    inherited_environment: self.calls[slot].invocation().environment().clone(),
                    environment: recipe.expression().serial_native_environment(self.calls[slot].invocation())?,
                    directory: self.calls[slot].invocation().working_directory().to_owned(),
                })
            })
        })
    }
}

impl NativeGenmoduleMakeData {
    /// Exact relative Source input matched inside the original reader transaction.
    /// This is retained data, not a fresh owner/evidence revalidation.
    #[must_use]
    pub fn source_input(&self) -> &Path {
        &self.source_input
    }

    /// Parsed output retained from the original checked producer bytes.
    /// This does not revalidate evidence or discharge original writefiles/stamps.
    #[must_use]
    pub const fn parsed_makefile(&self) -> &MesaGenmoduleMakefile {
        &self.makefile
    }

    #[must_use]
    pub const fn output_binding(&self) -> &MesaGenmoduleOutputBinding {
        &self.output
    }

    #[must_use]
    pub const fn producer_sha256(&self) -> &Sha256Digest {
        &self.stage.program
    }

    #[must_use]
    pub fn raw_makefile_path(&self) -> PathBuf {
        self.directory.path().join(format!(
            "Makefile.{}{}",
            self.output.producer.module, self.output.producer.module_type
        ))
    }

    /// Recheck the same original owner, inventory, data, process and deadline.
    /// # Errors
    /// Refuses another owner/selection, any evidence/input drift or expiry.
    pub fn revalidate(
        &self,
        owner: &CollectedMesaNativeDirect,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        inventory: &SelectedNativeGenmoduleInventory,
    ) -> Result<(), ContractError> {
        inventory.revalidate(owner, control)?;
        if owner_identity(owner)? != self.owner_sha256
            || inventory.evidence.payload_digest_excluding(None) != self.inventory_sha256
        {
            return Err(failure(
                "native genmodule data belongs to another owner or inventory",
            ));
        }
        let (producer, slot) = selected_producer(inventory, &self.output.producer.owner)?;
        if producer != &self.output.producer
            || self.makefile.module() != producer.module
            || owner
                .coordinates
                .get(slot)
                .map(|coordinate| Path::new(coordinate.source_input()))
                != Some(self.source_input.as_path())
        {
            return Err(failure(
                "native genmodule data differs from its original Source coordinate or tuple",
            ));
        }
        self.directory
            .revalidate()
            .map_err(|_| failure("native genmodule data directory changed"))?;
        self.stage.invocation_context.revalidate()?;
        if measure_tree_content_cas(self.directory.path())
            .map_err(|_| failure("cannot remeasure native genmodule data"))?
            != self.evidence
            || self.evidence.regular_file_digest(
                self.raw_makefile_path()
                    .file_name()
                    .and_then(OsStr::to_str)
                    .ok_or_else(|| failure("invalid native genmodule output name"))?,
            ) != Some(&self.output.makefile_sha256)
            || self.evidence.regular_file_digest("stdout") != Some(&self.stage.stdout)
            || self.evidence.regular_file_digest("stderr") != Some(&self.stage.stderr)
        {
            return Err(failure("native genmodule data evidence changed"));
        }
        if control.3.is_cancelled() {
            return Err(failure("native genmodule data revalidation cancelled"));
        }
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
}

fn selected_producer<'a>(
    inventory: &'a SelectedNativeGenmoduleInventory,
    owner: &str,
) -> Result<(&'a MesaGenmoduleProducer, usize), ContractError> {
    let index = inventory
        .inventory
        .producers()
        .binary_search_by(|producer| producer.owner.as_str().cmp(owner))
        .map_err(|_| failure("native genmodule data owner is not selected"))?;
    let producer = &inventory.inventory.producers()[index];
    let binding = inventory
        .owners
        .iter()
        .filter(|binding| binding.owner == producer.owner)
        .collect::<Vec<_>>();
    let [binding] = binding.as_slice() else {
        return Err(failure(
            "native genmodule data owner has ambiguous original calls",
        ));
    };
    Ok((producer, binding.call_index))
}

fn validate_scope(
    request: &MesaPreflightRequest<'_>,
    origin: &MesaNativeDirectOrigin<'_, '_>,
    collection: &MesaNativeCollectionSelection<'_>,
    inventory: &SelectedNativeGenmoduleInventory,
    output: &Path,
) -> Result<(), ContractError> {
    let helper = &inventory.stage.invocation_context;
    super::validate_evidence_scope(request, origin, collection, helper.program(), output)?;
    if filesystem_paths_overlap(output, inventory.directory.path())
        .map_err(|_| failure("cannot compare native genmodule data evidence"))?
    {
        return Err(failure("native genmodule data overlaps original inventory"));
    }
    Ok(())
}
