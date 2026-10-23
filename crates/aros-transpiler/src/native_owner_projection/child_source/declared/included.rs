//! Separate include-remake data; no conversion to original-goal shell authority.

use super::{MakeInputReader, MatchedChildDeclaredRules, VerifiedMakeInputReads};
use crate::{
    make_rule_graph::{traversal::DeclaredTrace, TargetRules},
    make_vars::{MakeFileEvent, MakeFileEventKind, MakeProgramPaths},
    native_owner_projection::{
        DeclaredChildRecipe, MatchedOwnerChildCoordinates, NativeOwnerProjection,
    },
};

mod genmodule;
mod genmodule_ready;
pub use genmodule::NativeGenmoduleIncludePlan;

/// Source-bound recipe data from one active missing include's separate trace.
///
/// Original command-line goals remain on the held child. Prerequisites remain
/// unresolved, and neither recipes nor file names authorize execution/outputs.
/// This deliberately has no Mesa26 shell binding or original-goal conversion.
pub struct DeclaredIncludedRecipes {
    pub(super) declared: MatchedChildDeclaredRules,
    include_event: usize,
    source_root: std::path::PathBuf,
    traversal: DeclaredTrace,
    recipes: Vec<DeclaredChildRecipe>,
    pub(super) evaluation_reads: VerifiedMakeInputReads,
}

impl DeclaredIncludedRecipes {
    /// Original child obligations; borrowing this does not replace its goals.
    #[must_use]
    pub const fn declared(&self) -> &MatchedChildDeclaredRules {
        &self.declared
    }

    #[must_use]
    pub const fn traversal(&self) -> &DeclaredTrace {
        &self.traversal
    }

    #[must_use]
    pub fn recipes(&self) -> &[DeclaredChildRecipe] {
        &self.recipes
    }

    #[must_use]
    pub fn include_event(&self) -> &MakeFileEvent {
        &self.declared.file().events[self.include_event]
    }

    #[must_use]
    pub const fn coordinates(&self) -> &MatchedOwnerChildCoordinates {
        self.declared.coordinates()
    }

    /// Original event belonging to this projection's indexed recipe.
    /// # Errors
    /// Refuses an absent recipe, visit or original event.
    pub fn origin(&self, index: usize) -> Result<&MakeFileEvent, String> {
        let selected = self
            .recipes
            .get(index)
            .ok_or("declared Source include recipe is absent")?;
        let recipe = self
            .traversal
            .visits()
            .get(selected.visit())
            .and_then(|visit| visit.instance())
            .and_then(|rule| rule.recipes.get(selected.recipe()))
            .ok_or("declared Source include recipe origin is absent")?;
        self.declared
            .file()
            .events
            .get(recipe.output_line)
            .ok_or_else(|| "declared Source include event origin is absent".into())
    }

    /// Bind explicit input fields and selected programs in this exact reader.
    ///
    /// The enclosing original Source/input transaction must finish before its
    /// data result is used. No output request, recipe launch or scheduling is
    /// permitted; missing/unselected commands remain unresolved.
    /// # Errors
    /// Refuses a different reader, recipe/index/field errors or changed programs.
    pub fn with_recipe_input_bindings_in_reader<T>(
        &self,
        recipe_index: usize,
        programs: &aros_common::captured_programs::CapturedProgramSet,
        requests: &[crate::make_shell_data::artifact_binding::RecipeArtifactRequest],
        reader: &mut MakeInputReader<'_>,
        consume: impl FnOnce(
            &crate::make_shell_data::program_binding::RecipeProgramBindings<'_>,
            &[crate::make_shell_data::artifact_binding::BoundRecipeArtifact],
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        if !self
            .declared
            .source
            .snapshot
            .same_read_context(reader.snapshot())
        {
            return Err("declared include input reader context differs from original match".into());
        }
        let selected = self
            .recipes
            .get(recipe_index)
            .ok_or("declared Source include recipe is absent")?;
        super::bind_recipe_inputs(selected, programs, requests, reader, consume)
    }

    /// Bind the exact missing include's GENMODULE producer and its directory rule.
    ///
    /// This uses the already retained Source recipes and the same original
    /// reader. The result contains only source-derived paths, argv and config
    /// digests; it does not assert completion or permit recipe execution.
    /// # Errors
    /// Refuses a different reader/source root, ambiguous or unsupported rules,
    /// unbound GENMODULE/config inputs, and any non-exact prerequisite tuple.
    pub fn prepare_genmodule_include_in_reader(
        &self,
        source_root: &std::path::Path,
        programs: &aros_common::captured_programs::CapturedProgramSet,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<NativeGenmoduleIncludePlan, String> {
        genmodule::prepare_genmodule_include_in_reader(self, source_root, programs, reader)
    }

    /// Check the Source dependencies of the selected original GENMODULE target.
    ///
    /// This checks the held declaration trace, Source leaf bytes, and bounded
    /// producer candidates using the same original reader. It makes no
    /// timestamp or scheduler-equivalence claim. The caller must separately
    /// prove original host completion for this exact captured program and
    /// validate the original invocation flags. This result is data-only: it
    /// does not schedule or execute Make.
    /// # Errors
    /// Refuses unsupported or potentially applicable producers, non-Source
    /// leaves, missing inputs, and reader/context drift.
    pub fn validate_genmodule_include_readiness_in_reader(
        &self,
        plan: &NativeGenmoduleIncludePlan,
        captured_program: &std::path::Path,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<(), String> {
        genmodule_ready::validate_genmodule_include_readiness_in_reader(
            self,
            plan,
            captured_program,
            reader,
        )
    }

    /// Re-evaluate held include recipe frames after the selected fresh directory exists.
    ///
    /// The original parsed Make program and declaration trace are retained;
    /// no source file is reparsed. Only the one selected fresh directory may
    /// differ in the new snapshot, and original source/evaluation file reads
    /// (including optional include absence) are replayed before evaluation.
    /// This data-only transition does not claim directory-query equivalence or
    /// grant recipe execution authority.
    /// # Errors
    /// Refuses a different path context, directory, snapshot transition, read
    /// drift or recipe-frame evaluation failure.
    pub fn after_fresh_directory_in_reader(
        mut self,
        paths: &MakeProgramPaths<'_>,
        directory: &std::path::Path,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<Self, String> {
        let expected_directory = genmodule::selected_include_directory(&self)?;
        if directory != expected_directory.as_path() {
            return Err(
                "native Source include directory differs from selected order-only target".into(),
            );
        }
        if paths.source_root != self.source_root.as_path()
            || paths.top_directory != self.declared.source.build_root.as_path()
            || paths.working_directory != self.declared.source.snapshot.working_directory()
            || reader.working_directory() != paths.working_directory
        {
            return Err(
                "native Source include post-directory paths differ from original context".into(),
            );
        }
        let relative = directory
            .strip_prefix(&self.declared.source.build_root)
            .map_err(|_| "native Source include directory is outside the original build root")?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("native Source include directory is not a fresh build-root child".into());
        }
        let relative = relative
            .to_str()
            .ok_or("native Source include directory is not UTF-8")?;
        self.declared.source.snapshot.verify_output_transition(
            reader.snapshot(),
            &self.declared.source.build_root,
            (&[], &[], &[relative]),
            &[&self.declared.source.reads, &self.evaluation_reads],
        )?;

        let (recipes, evaluation_reads) = super::super::recipes::evaluate_recipe_frames_in_reader(
            &mut self.declared.source.program,
            self.traversal.visits(),
            self.traversal.completion_order(),
            &|visit| self.traversal.scope_chain(visit),
            paths,
            reader,
        )?;
        self.declared.source.snapshot = reader.snapshot().clone();
        self.recipes = recipes;
        self.evaluation_reads = evaluation_reads;
        Ok(self)
    }
}

impl MatchedChildDeclaredRules {
    // Only the standalone wrapper or an enclosing retained Source reader may
    // call this. They own Source verification and discard this owned result on
    // any enclosing postcheck failure; no staged program can escape failure.
    pub(in super::super::super) fn project_included_recipes_in_reader(
        mut self,
        projection: &NativeOwnerProjection,
        include_event: usize,
        paths: &MakeProgramPaths<'_>,
        reader: &mut MakeInputReader<'_>,
        original_output: Option<(&std::path::Path, &aros_common::TreeContentCas)>,
    ) -> Result<DeclaredIncludedRecipes, String> {
        if paths.top_directory != self.source.build_root {
            return Err("native Source recipe build root differs from original match".into());
        }
        let traversal = self
            .declared_included_makefile_in_reader(include_event, reader)?
            .ok_or("native Source included Makefile has no explicit producer")?
            .traversal;
        let generated = super::verify_source_generated_origin(
            &self.source,
            projection,
            &self.file().events[include_event],
            reader,
        )?;
        for visit in traversal.visits() {
            let Some(TargetRules::Ordinary(target)) = self.graph.targets.get(visit.target()) else {
                continue;
            };
            for declaration in target.declarations() {
                let header = self
                    .file()
                    .events
                    .get(declaration.output_line)
                    .ok_or("native Source include declaration is absent")?;
                if !matches!(header.kind, MakeFileEventKind::Rule(_))
                    || !verified_include_origin(
                        projection,
                        paths,
                        &generated,
                        header,
                        reader,
                        original_output,
                    )?
                {
                    return Err(
                        "include producer declaration is outside its Source-expanded program"
                            .into(),
                    );
                }
            }
        }
        let (recipes, evaluation_reads) = super::super::recipes::evaluate_recipe_frames_in_reader(
            &mut self.source.program,
            traversal.visits(),
            traversal.completion_order(),
            &|visit| traversal.scope_chain(visit),
            paths,
            reader,
        )?;
        let result = DeclaredIncludedRecipes {
            declared: self,
            include_event,
            source_root: paths.source_root.to_path_buf(),
            traversal,
            recipes,
            evaluation_reads,
        };
        for index in 0..result.recipes.len() {
            if !verified_include_origin(
                projection,
                paths,
                &generated,
                result.origin(index)?,
                reader,
                original_output,
            )? {
                return Err("recipe origin is outside its Source-expanded program".into());
            }
        }
        // Header/recipe locations alone miss variable-only included programs:
        // their assignments can change a rule whose event still belongs to the
        // generated root. Bind every active present include, not only producers.
        for event in &result.declared.file().events {
            if let MakeFileEventKind::Include {
                present: true,
                resolved_identity: Some(identity),
                ..
            } = &event.kind
            {
                if event.active
                    && !verified_include_program(
                        projection,
                        paths,
                        &generated,
                        identity,
                        reader,
                        original_output,
                    )?
                {
                    return Err(
                        "included program is outside the independently bound Source or original-output inventory"
                            .into(),
                    );
                }
            }
        }
        Ok(result)
    }
}

// A prerequisite can be declared in a separately included Source program.
// Membership in a fresh reader snapshot is insufficient: its exact bytes must
// already belong to the independent Source projection or original owning
// output inventory supplied by that owner. This is data projection only, not
// original-goal or compound-shell authority.
fn verified_include_origin(
    projection: &NativeOwnerProjection,
    paths: &MakeProgramPaths<'_>,
    generated: &std::path::Path,
    origin: &MakeFileEvent,
    reader: &mut MakeInputReader<'_>,
    original_output: Option<(&std::path::Path, &aros_common::TreeContentCas)>,
) -> Result<bool, String> {
    verified_include_program(
        projection,
        paths,
        generated,
        &origin.input,
        reader,
        original_output,
    )
}

fn verified_include_program(
    projection: &NativeOwnerProjection,
    paths: &MakeProgramPaths<'_>,
    generated: &std::path::Path,
    input: &str,
    reader: &mut MakeInputReader<'_>,
    original_output: Option<(&std::path::Path, &aros_common::TreeContentCas)>,
) -> Result<bool, String> {
    let Some(identity) = reader.realpath(input)? else {
        return Ok(false);
    };
    if identity == generated {
        return Ok(true);
    }
    // An output inventory cannot authorize an unselected Source program.
    let expected = identity.strip_prefix(paths.source_root).map_or_else(
        |_| {
            original_output.and_then(|(root, tree)| {
                if !root.is_absolute()
                    || !paths.top_directory.starts_with(root)
                    || root.starts_with(paths.source_root)
                    || paths.source_root.starts_with(root)
                    || root.components().any(|part| {
                        matches!(
                            part,
                            std::path::Component::CurDir | std::path::Component::ParentDir
                        )
                    })
                {
                    return None;
                }
                identity
                    .strip_prefix(root)
                    .ok()
                    .and_then(std::path::Path::to_str)
                    .and_then(|relative| tree.regular_file_digest(relative))
                    .map(aros_common::Sha256Digest::as_str)
            })
        },
        |relative| {
            relative
                .to_str()
                .and_then(|relative| projection.snapshots.get(relative))
                .map(String::as_str)
        },
    );
    let Some(expected) = expected else {
        return Ok(false);
    };
    let Some(bytes) = reader.read_bytes(input)? else {
        return Ok(false);
    };
    Ok(aros_common::sha256_bytes(&bytes.bytes).as_str() == expected)
}
