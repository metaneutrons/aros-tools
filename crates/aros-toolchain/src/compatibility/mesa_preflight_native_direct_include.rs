//! Two literal prerequisite steps under the original live collection.
//! This does not grant the Mesa26 shell exception to transitive recipes.

use super::{
    validate_evidence_scope, CollectedMesaNativeDirect, MesaNativeCollectionSelection,
    MesaNativeDirectOrigin,
};
use crate::{
    compatibility::{
        mesa_preflight::{
            failure, invocation::InvocationContext, read_bytes, run_prepared_observe,
            MesaPreflightRequest, Stage,
        },
        CompatibilityEnvironment,
    },
    ContractError,
};
use aros_common::{
    captured_programs::CapturedProgramSet, make_native_invocation::PrivateNativeCaptureDirectory,
    measure_tree_content_cas, sha256_bytes, CancellationToken, Sha256Digest, TreeContentCas,
    TreeContentEntryKind,
};
use aros_transpiler::{
    make_vars::{
        validate_captured_source_commands, MakeFileEvent, MakeFileEventKind, MakeRecipeProjection,
    },
    native_owner_projection::{MatchedChildDeclaredRules, RetainedChildSourceBatchLimits},
};
use std::{
    ffi::OsStr,
    fs,
    io::Read as _,
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

/// Original observed child/include coordinates and a fresh private evidence root.
/// No command text, replacement destination or caller-supplied output digest.
pub struct NativeMakeIncludeExecution<'a> {
    pub child_slot: usize,
    pub include_event: usize,
    pub evidence_root: &'a Path,
}

/// An actual original GENMODULE include producer, retaining its live owner.
///
/// The original collection inventory is never rebased. This result proves one
/// directory and one include file, not other missing includes, a Make scheduler,
/// the Mesa/GCA build, compiler authentication or a completed SDK.
pub struct GeneratedNativeMakeInclude {
    original: CollectedMesaNativeDirect,
    directory_output: TreeContentCas,
    output: TreeContentCas,
    include_path: PathBuf,
    directory_path: PathBuf,
    include_sha256: Sha256Digest,
    child_slot: usize,
    include_site: MakeFileEvent,
    programs: CapturedProgramSet,
    stages: [Stage; 2],
    evidence_directory: PrivateNativeCaptureDirectory,
    evidence: TreeContentCas,
    root_modes: (u32, u32),
}

impl CollectedMesaNativeDirect {
    /// Execute the Source-declared directory then its GENMODULE Make producer.
    ///
    /// Both commands must be the existing strict literal forms. After mkdir,
    /// generator frames are evaluated from the held parsed Make state, not a
    /// fresh parse with changed immediate assignments. The original deadline,
    /// cancellation, selected tools, serial environment and bounded runner apply.
    /// No shell is launched and failure consumes the original owner.
    /// # Errors
    /// Refuses unsupported dependencies/controls, changed inputs, nonfresh or
    /// linked paths, process errors and any effect beyond the two exact steps.
    pub fn generate_original_make_include(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
        execution: &NativeMakeIncludeExecution<'_>,
    ) -> Result<GeneratedNativeMakeInclude, ContractError> {
        let (request, origin, collection, cancellation) = control;
        self.revalidate(request, origin, collection, cancellation)?;
        let (program, programs) = self.genmodule_program(request)?;
        require_native_program(&program)?;
        if !std::iter::once(origin.host_build.goal)
            .chain(
                origin
                    .host_build
                    .additional_goals
                    .iter()
                    .map(String::as_str),
            )
            .any(|goal| Path::new(goal) == program)
        {
            return Err(failure(
                "include GENMODULE was not an original completed host goal",
            ));
        }
        validate_evidence_scope(
            request,
            origin,
            collection,
            &program,
            execution.evidence_root,
        )?;
        let modes = root_modes(request, &self)?;
        let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = request.environment
        else {
            return Err(failure("include producer has no selected host tools"));
        };
        host_tools.revalidate()?;
        let mkdir = host_tools.root.join("mkdir");
        require_native_program(
            &host_tools
                .tools
                .get("mkdir")
                .ok_or_else(|| failure("include directory role is absent"))?
                .program,
        )?;
        let (included, first, directory_context, include_site) = self.with_source_child_batch(
            request,
            origin,
            collection,
            cancellation,
            |snapshot, children, check_control| {
                let child = children
                    .get(execution.child_slot)
                    .ok_or("original include child is absent")?;
                self.prepared
                    .policy
                    .projection
                    .with_retained_owned_child_include_reader(
                        &self.prepared.roots,
                        (
                            child,
                            execution.include_event,
                            Some((request.output_root, &self.output_tree)),
                        ),
                        snapshot,
                        limits,
                        check_control,
                        |included, reader| {
                            let plan = included.prepare_genmodule_include_in_reader(
                                request.source_root,
                                &programs,
                                reader,
                            )?;
                            included.validate_genmodule_include_readiness_in_reader(&plan, &program, reader)?;
                            let startup = aros_common::make_startup_arguments::LiteralMakeStartupArguments::admit(
                                child.observation.invocation().arguments(),
                            ).map_err(|_| "include producer has unsupported native startup controls")?;
                            if !startup.arguments().iter().any(|argument| argument == "-r") {
                                return Err("include producer requires captured disabled builtin rules".into());
                            }
                            if startup.old_files().iter().any(|old| Path::new(old).components()
                                .any(|component| component == std::path::Component::ParentDir)) {
                                return Err("include producer old-file alias has unsupported parent traversal".into());
                            }
                            if startup.old_files().iter().any(|old| {
                                let old = Path::new(old);
                                let old = if old.is_absolute() { old.to_owned() }
                                    else { child.paths.working_directory.join(old) };
                                old == plan.include_path() || old == plan.directory()
                            }) {
                                return Err("include producer is disabled by an original old-file control".into());
                            }
                            validate_directory(
                                reader,
                                self.prepared.invocation.working_directory(),
                                plan.directory(),
                            )?;
                            if aros_common::publication::filesystem_paths_overlap(
                                plan.directory(),
                                program.parent().ok_or("GENMODULE parent is absent")?,
                            )
                            .map_err(|_| "cannot compare GENMODULE and directory scopes")?
                            {
                                return Err(
                                    "include directory overlaps the captured GENMODULE namespace"
                                        .into(),
                                );
                            }
                            let expression =
                                included.recipes()[plan.directory_recipe()].expression();
                            let exports = expression
                                .serial_native_environment(child.observation.invocation())?;
                            validate_captured_source_commands(
                                host_tools,
                                exports.get("PATH").ok_or("directory recipe has no PATH")?,
                                &["mkdir"],
                                reader,
                            )?;
                            let directory_shell = expression.parse_literal_shell()?;
                            let [aros_transpiler::make_shell_data::LiteralRecipeStatement::Command(
                                directory_command,
                            )] = directory_shell.statements.as_slice()
                            else {
                                return Err("directory command is not literal".into());
                            };
                            let arguments = directory_command.words()[1..].to_vec();
                            let context = self
                                .include_context(
                                    request,
                                    execution.child_slot,
                                    expression,
                                    &mkdir,
                                    &arguments,
                                )
                                .map_err(|error| error.to_string())?;
                            let site = included.include_event().clone();
                            if included
                                .declared()
                                .file()
                                .events
                                .iter()
                                .filter(|event| same_include_site(event, &site))
                                .count()
                                != 1
                            {
                                return Err("original include source site is ambiguous".into());
                            }
                            Ok((included, plan, context, site))
                        },
                    )
            },
        )?;
        programs
            .verify()
            .map_err(|_| failure("include GENMODULE identity changed before directory creation"))?;
        self.revalidate(request, origin, collection, cancellation)?;
        check_modes(request, &self, modes)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(execution.evidence_root)
            .map_err(|_| failure("cannot create fresh include execution evidence"))?;
        let evidence_directory = PrivateNativeCaptureDirectory::open(execution.evidence_root)
            .map_err(|_| failure("cannot retain include execution evidence"))?;
        let started = self
            .prepared
            .recorded
            .launch_inputs()
            .host
            .root
            .prepared
            .configured
            .started;
        let directory_stage = include_step(
            control,
            started,
            &evidence_directory,
            "directory",
            directory_context,
        )?;
        let directory_output = measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot measure original directory result"))?;
        let directory_name = output_name(request, first.directory())?;
        if !self
            .output_tree
            .matches_fresh_directory_chain(&directory_output, &directory_name)
        {
            return Err(failure(
                "original directory recipe changed unselected outputs",
            ));
        }
        check_modes(request, &self, modes)?;
        self.revalidate_output(request, origin, collection, &directory_output, cancellation)?;
        host_tools.revalidate()?;
        programs
            .verify()
            .map_err(|_| failure("include GENMODULE identity changed after directory creation"))?;

        // Preserve the original parsed state. GNU evaluates the next recipe
        // after its directory prerequisite; reparsing here would change :=.
        let (plan, generator_context) = self.with_source_child_batch_at_output(
            request, origin, collection, cancellation, &directory_output,
            |snapshot, children, check_control| {
                check_control()?;
                let child = children.get(execution.child_slot).ok_or("original include child is absent after directory")?;
                snapshot.batch_transaction(limits.inputs, check_control, |batch| {
                    batch.read_child(child.paths.working_directory, None, |_, reader| {
                    let included = included.after_fresh_directory_in_reader(child.paths, first.directory(), reader)?;
                    let plan = included.prepare_genmodule_include_in_reader(request.source_root, &programs, reader)?;
                    included.validate_genmodule_include_readiness_in_reader(&plan, &program, reader)?;
                    if Path::new(&plan.arguments()[0]) != program {
                        return Err("GENMODULE spelling differs from original selected role".into());
                    }
                    if plan.include_path() != first.include_path() || plan.directory() != first.directory() || plan.configurations() != first.configurations() {
                        return Err("include producer identity or configuration changed after directory".into());
                    }
                    let context = self.include_context(request, execution.child_slot, included.recipes()[plan.generator_recipe()].expression(), &program, &plan.arguments()[1..]).map_err(|error| error.to_string())?;
                    Ok((plan, context))
                    }).map(|(value, _)| value)
                })
            },
        )?;
        check_modes(request, &self, modes)?;
        let generator_stage = include_step(
            control,
            started,
            &evidence_directory,
            "genmodule",
            generator_context,
        )?;
        let output = measure_tree_content_cas(request.output_root)
            .map_err(|_| failure("cannot measure original include result"))?;
        let include_name = output_name(request, plan.include_path())?;
        if !directory_output.matches_fresh_outputs(&output, &[&include_name], &[]) {
            return Err(failure("original GENMODULE changed unselected outputs"));
        }
        let bytes = read_bytes(plan.include_path())?;
        // The original positioned Make reader below owns include semantics;
        // do not replace it with a second, fixed GENMODULE-data grammar.
        std::str::from_utf8(&bytes).map_err(|_| failure("generated include is not UTF-8"))?;
        let result = GeneratedNativeMakeInclude {
            include_path: plan.include_path().to_owned(),
            directory_path: plan.directory().to_owned(),
            include_sha256: sha256_bytes(&bytes),
            child_slot: execution.child_slot,
            include_site,
            original: self,
            directory_output,
            output,
            programs,
            stages: [directory_stage, generator_stage],
            evidence: measure_tree_content_cas(evidence_directory.path())
                .map_err(|_| failure("cannot capture include execution evidence"))?,
            evidence_directory,
            root_modes: modes,
        };
        result.revalidate(control)?;
        Ok(result)
    }

    fn include_context(
        &self,
        _request: &MesaPreflightRequest<'_>,
        slot: usize,
        expression: &MakeRecipeProjection,
        program: &Path,
        arguments: &[String],
    ) -> Result<InvocationContext, ContractError> {
        let original = self
            .calls
            .get(slot)
            .ok_or_else(|| failure("include original invocation is absent"))?
            .invocation();
        let exports = expression
            .serial_native_environment(original)
            .map_err(|_| failure("include recipe lacks the original serial environment"))?;
        let mut inherited = self
            .prepared
            .recorded
            .launch_inputs()
            .host
            .root
            .prepared
            .configured_invocation()?
            .environment()
            .clone();
        inherited.variables = original.environment().clone();
        let environment =
            crate::compatibility::recipe_shell::captured_recipe_environment(inherited, &exports)?;
        InvocationContext::new(
            program,
            arguments,
            original.working_directory(),
            environment,
        )
    }
}

impl GeneratedNativeMakeInclude {
    /// The exact Source-selected output, not a publication or SDK artifact.
    #[must_use]
    pub fn include_path(&self) -> &Path {
        &self.include_path
    }

    /// Recheck both original process steps, effects, inputs and the original clock.
    /// # Errors
    /// Refuses changed evidence/programs/outputs, owner drift or expiry/cancel.
    pub fn revalidate(
        &self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
    ) -> Result<(), ContractError> {
        let (request, origin, selected, cancellation) = control;
        let directory = output_name(request, &self.directory_path)?;
        let include = output_name(request, &self.include_path)?;
        if !self
            .original
            .output_tree
            .matches_fresh_directory_chain(&self.directory_output, &directory)
            || !self
                .directory_output
                .matches_fresh_outputs(&self.output, &[&include], &[])
            || self.output.regular_file_digest(&include) != Some(&self.include_sha256)
        {
            return Err(failure(
                "retained original include producer transition changed",
            ));
        }
        check_modes(request, &self.original, self.root_modes)?;
        self.programs
            .verify()
            .map_err(|_| failure("original include GENMODULE changed"))?;
        self.evidence_directory
            .revalidate()
            .map_err(|_| failure("original include evidence directory changed"))?;
        if measure_tree_content_cas(self.evidence_directory.path())
            .map_err(|_| failure("cannot inspect include evidence"))?
            != self.evidence
            || self.evidence.entry_count() != 6
        {
            return Err(failure("original include evidence membership changed"));
        }
        for (stage, prefix) in self.stages.iter().zip(["directory", "genmodule"]) {
            stage.invocation_context.revalidate()?;
            if self
                .evidence
                .regular_file_digest(&format!("{prefix}/stdout"))
                != Some(&stage.stdout)
                || self
                    .evidence
                    .regular_file_digest(&format!("{prefix}/stderr"))
                    != Some(&stage.stderr)
            {
                return Err(failure("original include process evidence changed"));
            }
        }
        self.original
            .revalidate_output(request, origin, selected, &self.output, cancellation)
    }

    /// Reparse original children with the completed include at its original site.
    /// Other missing includes and unresolved prerequisites remain obligations.
    /// This does not execute the parsed graph or qualify Mesa/SDK products.
    /// # Errors
    /// Consumes the owner on drift, changed include identity, parse/input error,
    /// cancellation or expiry of the original total bound.
    pub fn read_declared_children(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
    ) -> Result<(Self, Vec<MatchedChildDeclaredRules>), ContractError> {
        self.read_declared_selection(control, limits, false)
    }

    /// Reparse only the original child that owns the completed include.
    /// Complete original call coverage and owner checks still apply. Unread
    /// siblings remain unresolved; this is not a full graph qualification.
    /// # Errors
    /// Refuses any original-owner/coordinate/input/include drift or expiry.
    pub fn read_declared_child(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
    ) -> Result<(Self, MatchedChildDeclaredRules), ContractError> {
        let (owner, mut children) = self.read_declared_selection(control, limits, true)?;
        let child = children
            .pop()
            .ok_or_else(|| failure("completed include child is absent"))?;
        Ok((owner, child))
    }

    fn read_declared_selection(
        self,
        control: (
            &MesaPreflightRequest<'_>,
            &MesaNativeDirectOrigin<'_, '_>,
            &MesaNativeCollectionSelection<'_>,
            &CancellationToken,
        ),
        limits: RetainedChildSourceBatchLimits,
        single: bool,
    ) -> Result<(Self, Vec<MatchedChildDeclaredRules>), ContractError> {
        self.revalidate(control)?;
        let parsed = if single {
            self.original.with_source_child_batch_at_output(
                control.0,
                control.1,
                control.2,
                control.3,
                &self.output,
                |snapshot, requests, check| {
                    let child = requests
                        .get(self.child_slot)
                        .ok_or("completed include original slot is absent")?;
                    self.original
                        .prepared
                        .policy
                        .projection
                        .read_retained_child_sources(
                            &self.original.prepared.roots,
                            std::slice::from_ref(child),
                            snapshot,
                            limits,
                            check,
                        )
                },
            )?
        } else {
            self.original
                .read_children_at_output(control, limits, &self.output)?
        };
        let children = parsed
            .into_iter()
            .map(|child| {
                child
                    .trace_declared_rules()
                    .map_err(|_| failure("cannot trace original child after include production"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let child = children
            .get(if single { 0 } else { self.child_slot })
            .ok_or_else(|| failure("completed include lost its original child"))?;
        let original = self
            .original
            .coordinates
            .get(self.child_slot)
            .ok_or_else(|| failure("completed include lost its original coordinate"))?;
        if child.coordinates().generated_owner() != original.generated_owner()
            || child.coordinates().source_input() != original.source_input()
            || child.coordinates().goals() != original.goals()
        {
            return Err(failure("completed include child coordinates changed"));
        }
        let sites = child
            .file()
            .events
            .iter()
            .filter(|event| same_include_site(event, &self.include_site))
            .collect::<Vec<_>>();
        if sites.len() != 1
            || !sites[0].active
            || !matches!(&sites[0].kind, MakeFileEventKind::Include { present: true, resolved_identity: Some(identity), .. } if Path::new(identity) == self.include_path)
        {
            return Err(failure(
                "completed include was not read at its original include site",
            ));
        }
        self.revalidate(control)?;
        Ok((self, children))
    }
}

fn same_include_site(current: &MakeFileEvent, original: &MakeFileEvent) -> bool {
    current.input == original.input
        && current.source_line == original.source_line
        && current.generated == original.generated
        && matches!((&current.kind, &original.kind), (MakeFileEventKind::Include {requested: a, ..}, MakeFileEventKind::Include {requested: b, ..}) if a == b)
}

fn require_native_program(program: &Path) -> Result<(), ContractError> {
    let mut header = [0; 4];
    aros_common::open_regular_file_nofollow(program)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|_| failure("cannot inspect original include tool image"))?;
    if !crate::compatibility::recipe_shell::native_image(header) {
        return Err(failure(
            "original include tool must be native, not an unbound script interpreter",
        ));
    }
    Ok(())
}

fn validate_directory(
    reader: &mut aros_common::make_input_snapshot::MakeInputReader<'_>,
    build: &Path,
    directory: &Path,
) -> Result<(), String> {
    let relative = directory
        .strip_prefix(build)
        .map_err(|_| "include directory leaves original build root")?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("include directory is not a normalized build child".into());
    }
    let mut path = build.to_owned();
    for part in relative.components() {
        path.push(part);
        if !matches!(
            reader.wildcard_entry_kind(path.to_str().ok_or("include directory is not text")?)?,
            None | Some(TreeContentEntryKind::Directory)
        ) {
            return Err("include directory has a linked or non-directory prefix".into());
        }
    }
    if reader
        .wildcard_entry_kind(directory.to_str().ok_or("include directory is not text")?)?
        .is_some()
    {
        return Err("original include directory is not fresh".into());
    }
    Ok(())
}

fn include_step(
    control: (
        &MesaPreflightRequest<'_>,
        &MesaNativeDirectOrigin<'_, '_>,
        &MesaNativeCollectionSelection<'_>,
        &CancellationToken,
    ),
    started: std::time::Instant,
    evidence: &PrivateNativeCaptureDirectory,
    name: &str,
    context: InvocationContext,
) -> Result<Stage, ContractError> {
    evidence
        .revalidate()
        .map_err(|_| failure("include evidence changed before step"))?;
    let path = evidence.path().join(name);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(|_| failure("cannot create include step evidence"))?;
    let directory = PrivateNativeCaptureDirectory::open(&path)
        .map_err(|_| failure("cannot retain include step evidence"))?;
    let (stage, _) = run_prepared_observe(
        (control.0, control.3, started),
        &format!("native-include-{name}"),
        context,
        0,
        |stdout, stderr| {
            directory
                .write_new(OsStr::new("stdout"), stdout)
                .map_err(|_| failure("cannot retain include stdout"))?;
            directory
                .write_new(OsStr::new("stderr"), stderr)
                .map_err(|_| failure("cannot retain include stderr"))
        },
    )?;
    directory
        .revalidate()
        .map_err(|_| failure("include step evidence changed"))?;
    Ok(stage)
}

fn output_name(request: &MesaPreflightRequest<'_>, path: &Path) -> Result<String, ContractError> {
    path.strip_prefix(request.output_root)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .and_then(Path::to_str)
        .map(str::to_owned)
        .ok_or_else(|| failure("include output is outside the original owner"))
}

fn root_modes(
    request: &MesaPreflightRequest<'_>,
    owner: &CollectedMesaNativeDirect,
) -> Result<(u32, u32), ContractError> {
    let mode = |path| {
        fs::symlink_metadata(path)
            .map(|metadata| metadata.permissions().mode())
            .map_err(|_| failure("cannot inspect include owner root permissions"))
    };
    Ok((
        mode(request.output_root)?,
        mode(owner.prepared.invocation.working_directory())?,
    ))
}

fn check_modes(
    request: &MesaPreflightRequest<'_>,
    owner: &CollectedMesaNativeDirect,
    expected: (u32, u32),
) -> Result<(), ContractError> {
    if root_modes(request, owner)? != expected {
        return Err(failure("include execution changed owner root permissions"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{same_include_site, MakeFileEvent, MakeFileEventKind};

    #[test]
    fn original_include_site_preserves_request_spelling_and_source_location() {
        let original = MakeFileEvent {
            input: "/selected/generated-owner/mmakefile".into(),
            source_line: 17,
            generated: false,
            active: true,
            variable_position: 23,
            kind: MakeFileEventKind::Include {
                requested: "generated/Makefile.modulelibrary".into(),
                present: false,
                resolved_identity: None,
            },
        };
        let mut completed = original.clone();
        completed.variable_position += 16;
        completed.kind = MakeFileEventKind::Include {
            requested: "generated/Makefile.modulelibrary".into(),
            present: true,
            resolved_identity: Some("/build/owner/generated/Makefile.modulelibrary".into()),
        };
        assert!(same_include_site(&completed, &original));
        let mut alias = completed.clone();
        if let MakeFileEventKind::Include { requested, .. } = &mut alias.kind {
            *requested = "/build/owner/generated/Makefile.modulelibrary".into();
        }
        assert!(
            !same_include_site(&alias, &original),
            "another spelling at the same site must not substitute for the selected include"
        );
        let mut other_line = completed.clone();
        other_line.source_line += 1;
        assert!(!same_include_site(&other_line, &original));
        completed.input = "/another/owner/mmakefile".into();
        assert!(!same_include_site(&completed, &original));
    }
}
