//! Scoped declared dependencies awaiting independent input/producer resolution.

use super::{
    DeclaredChildRecipeProjection, MatchedChildRecipePlan, MatchedChildSource,
    MatchedOwnerChildCoordinates,
};
use crate::{
    make_interpreted_rules,
    make_rule_graph::traversal::{trace_declared, DeclaredTrace},
    make_vars::{MakeFileEvent, MakeFileEventKind, MakeFileProgram, MakeProgramPaths},
};
use aros_common::make_input_snapshot::{MakeInputReader, VerifiedMakeInputReads};

mod included;
mod links;
mod mesa26;
pub use included::{DeclaredIncludedRecipes, NativeGenmoduleIncludePlan};
pub use links::{
    DeclaredChildJoinSelection, DeclaredChildOutputCandidate, DeclaredChildOutputLinks,
};
pub use mesa26::Mesa26OriginalRecipeBinding;

// Shared by original-goal and include-producer data projections. An included
// foreign program needs its own Source binding; fresh capture is not authority.
fn verify_source_generated_origin(
    source: &MatchedChildSource,
    projection: &super::super::NativeOwnerProjection,
    origin: &MakeFileEvent,
    reader: &mut MakeInputReader<'_>,
) -> Result<std::path::PathBuf, String> {
    if !source.snapshot.same_read_context(reader.snapshot()) {
        return Err("generated Source recipe reader context differs from original match".into());
    }
    let coordinates = &source.coordinates;
    let owner = coordinates.generated_owner();
    if projection.owners.get(owner).map(String::as_str) != Some(coordinates.source_input()) {
        return Err("generated recipe owner differs from Source selection".into());
    }
    let expected = projection
        .expanded_program_sha256
        .get(owner)
        .ok_or("Source projection lacks the original generated program")?;
    let generated = source.build_root.join(owner);
    let identity = generated
        .to_str()
        .ok_or("generated recipe path is not UTF-8")?;
    if reader.realpath(&origin.input)?.as_ref() != Some(&generated) {
        return Err("recipe origin is outside its Source-expanded program".into());
    }
    let bytes = reader
        .read_bytes(identity)?
        .ok_or("original generated recipe program is absent")?;
    if &aros_common::sha256_bytes(&bytes.bytes) != expected {
        return Err("generated recipe program differs from original Source expansion".into());
    }
    Ok(generated)
}

fn bind_recipe_inputs<T>(
    selected: &super::DeclaredChildRecipe,
    programs: &aros_common::captured_programs::CapturedProgramSet,
    requests: &[crate::make_shell_data::artifact_binding::RecipeArtifactRequest],
    reader: &mut MakeInputReader<'_>,
    consume: impl FnOnce(
        &crate::make_shell_data::program_binding::RecipeProgramBindings<'_>,
        &[crate::make_shell_data::artifact_binding::BoundRecipeArtifact],
    ) -> Result<T, String>,
) -> Result<T, String> {
    use crate::make_shell_data::artifact_binding::RecipeArtifactDirection;

    if requests
        .iter()
        .any(|request| request.direction != RecipeArtifactDirection::Input)
    {
        return Err("declared recipe binding accepts input fields only".into());
    }
    if requests.iter().any(|request| request.recipe != 0) {
        return Err("declared recipe artifact batch coordinate is invalid".into());
    }
    let shell = selected
        .expression()
        .parse_literal_shell()
        .map_err(|_| "declared Source shell projection failed")?;
    crate::make_shell_data::program_binding::with_literal_program_bindings(
        &[shell.statements.as_slice()],
        programs,
        |bindings| {
            let binding = bindings
                .first()
                .ok_or("declared Source recipe binding result is absent")?;
            let artifacts = crate::make_shell_data::artifact_binding::bind_recipe_artifacts(
                &bindings, reader, None, requests,
            )?;
            consume(binding, &artifacts)
        },
    )
}

/// One matched child's declared graph, retaining every external leaf unresolved.
///
/// This is not a recipe plan, input existence proof, cross-owner producer join
/// or native schedule. A target's first-parent chain belongs only to this child;
/// another child's graph must never be merged to supply its variable frame.
/// The original interpreted program, input snapshot and coordinates are held
/// privately so a later strict check cannot substitute newly prepared bytes.
pub struct MatchedChildDeclaredRules {
    source: MatchedChildSource,
    graph: crate::make_rule_graph::RuleGraph,
    traversal: DeclaredTrace,
    implicit_events: Vec<usize>,
    non_rule_events: Vec<usize>,
}

/// Data-only declaration trace for one missing, active original Make include.
///
/// This borrows the original child and keeps its command-line goals unchanged.
/// The additional trace is not the original goal traversal, a remake schedule,
/// producer authentication, or permission to run a recipe. There is deliberately
/// no conversion to the original-goal recipe projection or Mesa26 shell binding.
pub struct DeclaredIncludedMakefile<'a> {
    owner: &'a MatchedChildDeclaredRules,
    include_event: usize,
    traversal: DeclaredTrace,
}

impl DeclaredIncludedMakefile<'_> {
    /// Original source-positioned include event, not an invented output path.
    #[must_use]
    pub fn include_event(&self) -> &MakeFileEvent {
        &self.owner.file().events[self.include_event]
    }

    /// Separate declaration-only trace; external prerequisites stay unresolved.
    #[must_use]
    pub const fn traversal(&self) -> &DeclaredTrace {
        &self.traversal
    }

    /// The original matched child coordinates, including unchanged original goals.
    #[must_use]
    pub const fn coordinates(&self) -> &MatchedOwnerChildCoordinates {
        self.owner.coordinates()
    }

    /// Recheck the original retained inputs without adopting generated bytes.
    /// # Errors
    /// Refuses any original input or optional-include absence drift.
    pub fn verify(&self) -> Result<(), String> {
        self.owner.verify()
    }
}

impl MatchedChildDeclaredRules {
    /// Trace explicit declarations for one missing active include in this child.
    ///
    /// Only the include's exact original requested spelling can select a graph
    /// target. An optional input with no explicit declaration returns `None`;
    /// no basename matching, implicit search, Source-owner guessing or new
    /// command-line goal is introduced. Recipe-less targets still require
    /// separate search/producer resolution. The separate borrowed result grants
    /// no execution, output or original-goal recipe authority.
    /// # Errors
    /// Refuses invalid/non-include/present events, changed original inputs,
    /// malformed or ambiguous declared rules, cycles and existing graph limits.
    pub fn declared_included_makefile(
        &self,
        include_event: usize,
    ) -> Result<Option<DeclaredIncludedMakefile<'_>>, String> {
        let snapshot = self.source.snapshot.clone();
        snapshot
            .transaction(|reader| self.declared_included_makefile_in_reader(include_event, reader))
            .map(|(result, _)| result)
    }

    fn declared_included_makefile_in_reader(
        &self,
        include_event: usize,
        reader: &MakeInputReader<'_>,
    ) -> Result<Option<DeclaredIncludedMakefile<'_>>, String> {
        if !self.source.snapshot.same_read_context(reader.snapshot()) {
            return Err("native Source include reader context differs from original match".into());
        }
        let event = self
            .file()
            .events
            .get(include_event)
            .ok_or("native Source include event is absent")?;
        let MakeFileEventKind::Include {
            requested,
            present: false,
            resolved_identity: None,
        } = &event.kind
        else {
            return Err("native Source event is not a missing include".into());
        };
        if !event.active {
            return Err("native Source include event is inactive".into());
        }
        if !self.graph.targets.contains_key(requested) {
            return Ok(None);
        }
        let traversal = trace_declared(&self.graph, &[requested.as_str()])
            .map_err(|_| "native Source included Makefile declaration traversal failed")?;
        Ok(Some(DeclaredIncludedMakefile {
            owner: self,
            include_event,
            traversal,
        }))
    }

    /// Evaluate one missing include producer in its own declaration frame.
    ///
    /// `selection` is the exact include-event index and a recipe index across
    /// its whole dependency-first trace, including dependency recipes. This
    /// selects held data, not an authenticated direct producer, replacement goal
    /// or caller recipe text. Source expansion,
    /// original inputs and evaluation limits are checked using the existing
    /// reader and variable program. The original goal traversal is unchanged.
    /// The callback is data-only: this does not execute or import a Makefile,
    /// resolve prerequisites, authorize outputs or grant Mesa26 shell policy.
    ///
    /// Ownership is consumed so evaluation state cannot escape failed final
    /// checks. On success, return the original child, callback data and retained
    /// evaluation reads; no fresh variable program or work budget is created.
    /// # Errors
    /// Refuses a different build/Source root, absent producer or recipe,
    /// foreign recipe origin/program, input drift and existing evaluation limits.
    pub fn with_source_generated_include_recipe<T>(
        self,
        projection: &super::super::NativeOwnerProjection,
        selection: (usize, usize),
        paths: &MakeProgramPaths<'_>,
        consume: impl FnOnce(
            &DeclaredTrace,
            &super::DeclaredChildRecipe,
            &MakeFileEvent,
            &MatchedOwnerChildCoordinates,
        ) -> Result<T, String>,
    ) -> Result<(Self, T, VerifiedMakeInputReads), String> {
        projection.verify(paths.source_root)?;
        let snapshot = self.source.snapshot.clone();
        let result = snapshot.transaction(|reader| {
            let included = self.project_included_recipes_in_reader(
                projection,
                selection.0,
                paths,
                reader,
                None,
            )?;
            let selected = included
                .recipes()
                .get(selection.1)
                .ok_or("declared Source include recipe is absent")?;
            let value = consume(
                included.traversal(),
                selected,
                included.origin(selection.1)?,
                included.coordinates(),
            )?;
            Ok((included.declared, value, included.evaluation_reads))
        });
        projection.verify(paths.source_root)?;
        result.map(|(value, _)| value)
    }

    #[must_use]
    pub const fn coordinates(&self) -> &MatchedOwnerChildCoordinates {
        &self.source.coordinates
    }

    #[must_use]
    pub const fn traversal(&self) -> &DeclaredTrace {
        &self.traversal
    }

    #[must_use]
    pub const fn file(&self) -> &MakeFileProgram {
        &self.source.file
    }

    #[must_use]
    pub const fn input_reads(&self) -> &aros_common::make_input_snapshot::VerifiedMakeInputReads {
        &self.source.reads
    }

    #[must_use]
    pub fn implicit_events(&self) -> &[usize] {
        &self.implicit_events
    }

    #[must_use]
    pub fn non_rule_events(&self) -> &[usize] {
        &self.non_rule_events
    }

    /// Recheck the held source/build/compiler inputs, never adopt new bytes.
    /// # Errors
    /// Refuses any original input drift, including metadata-only replacement.
    pub fn verify(&self) -> Result<(), String> {
        self.source.snapshot.verify()
    }

    /// Evaluate original recipe expressions in their declaration-derived target frames.
    ///
    /// This retains every reached explicit recipe expression, including shell
    /// syntax that cannot be projected by the literal shell parser. External
    /// leaves remain unresolved, and implicit/non-rule obligations remain on
    /// the owned declaration trace. No Make scheduling or command execution is
    /// performed.
    ///
    /// # Errors
    /// Refuses a replacement build root, original input drift, unsupported Make
    /// expression effects or aggregate frame/evaluation limits.
    pub fn project_recipes(
        self,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<DeclaredChildRecipeProjection, String> {
        let snapshot = self.source.snapshot.clone();
        snapshot
            .transaction(|reader| self.project_recipes_in_reader(paths, reader))
            .map(|(result, _)| result)
    }

    /// Compose recipe data in the exact already-open child reader.
    /// The owned child/result must remain private until the enclosing root and
    /// reader postchecks succeed; this grants no execution or publication.
    /// # Errors
    /// Refuses another reader context and all ordinary recipe-frame errors.
    pub(crate) fn project_recipes_in_reader(
        mut self,
        paths: &MakeProgramPaths<'_>,
        reader: &mut MakeInputReader<'_>,
    ) -> Result<DeclaredChildRecipeProjection, String> {
        if !self.source.snapshot.same_read_context(reader.snapshot()) {
            return Err("native Source recipe reader context differs from original match".into());
        }
        if paths.top_directory != self.source.build_root {
            return Err("native Source recipe build root differs from original match".into());
        }
        let evaluated = {
            let traversal = &self.traversal;
            let source = &mut self.source;
            super::recipes::evaluate_recipe_frames_in_reader(
                &mut source.program,
                traversal.visits(),
                traversal.completion_order(),
                &|visit| traversal.scope_chain(visit),
                paths,
                reader,
            )
        };
        let (recipes, evaluation_reads) = evaluated?;
        Ok(DeclaredChildRecipeProjection {
            declared: self,
            recipes,
            evaluation_reads,
        })
    }

    /// Attempt the unchanged strict recipe path against the original snapshot.
    ///
    /// Declared output names do not count as existing files. In particular an
    /// external producer's name cannot discharge a leaf merely because this
    /// data type retained it. Cross-child resolution remains a separate step.
    /// # Errors
    /// Refuses all original strict traversal, interpretation and input errors.
    pub fn require_captured_recipes(
        self,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<MatchedChildRecipePlan, String> {
        self.source.trace_explicit_recipes(paths)
    }
}

impl DeclaredChildRecipeProjection {
    /// Read a recipe only if its held generated program equals the original
    /// deterministic Source expansion retained by the independent projection.
    ///
    /// This joins exact program bytes, owner and Source selection; a fresh but
    /// foreign Makefile cannot supply its own replacement expected digest. It
    /// does not prove a native GenMF process ran, admit a Mesa 26 recipe or
    /// authorize execution. The callback must remain data-only.
    ///
    /// # Errors
    /// Refuses an unknown/foreign owner, absent expected expansion, different
    /// program bytes, input drift, callback failure or absent recipe origin.
    pub fn with_source_generated_recipe<T>(
        &self,
        source_root: &std::path::Path,
        projection: &super::super::NativeOwnerProjection,
        recipe_index: usize,
        consume: impl FnOnce(
            &super::DeclaredChildRecipe,
            &crate::make_vars::MakeFileEvent,
            &MatchedOwnerChildCoordinates,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        projection.verify(source_root)?;
        let result = self.declared.source.snapshot.transaction(|reader| {
            self.source_generated_recipe_in_reader(
                projection,
                recipe_index,
                reader,
                |recipe, origin, coordinates, _| consume(recipe, origin, coordinates),
            )
        });
        projection.verify(source_root)?;
        result.map(|(value, _)| value)
    }

    // Used only by the standalone wrapper or this projection's enclosing batch.
    // Both callers retain Source-projection verification around the whole read.
    pub(super) fn source_generated_recipe_in_reader<T>(
        &self,
        projection: &super::super::NativeOwnerProjection,
        recipe_index: usize,
        reader: &mut MakeInputReader<'_>,
        consume: impl FnOnce(
            &super::DeclaredChildRecipe,
            &crate::make_vars::MakeFileEvent,
            &MatchedOwnerChildCoordinates,
            &mut MakeInputReader<'_>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        if !self
            .declared
            .source
            .snapshot
            .same_read_context(reader.snapshot())
        {
            return Err(
                "generated Source recipe reader context differs from original match".into(),
            );
        }
        let selected = self
            .recipes
            .get(recipe_index)
            .ok_or("declared Source recipe is absent")?;
        let origin = self.origin(recipe_index)?;
        verify_source_generated_origin(&self.declared.source, projection, origin, reader)?;
        consume(selected, origin, self.declared.coordinates(), reader)
    }

    /// Read one original expanded recipe under its held Source transaction.
    ///
    /// The index selects this projection's own expression and origin together;
    /// neither may be replaced with caller text. The callback is data-only:
    /// it must not launch commands or publish effects. Its result is released
    /// only after the original input snapshot passes its final verification.
    /// Shell syntax is not parsed, and no recipe execution policy is granted.
    /// In particular, a Mesa 26 exception still requires independent policy,
    /// exact owner selection and a separate owning runtime transaction.
    ///
    /// # Errors
    /// Refuses an absent recipe/origin, callback failure or original input drift.
    pub fn with_original_recipe<T>(
        &self,
        recipe_index: usize,
        consume: impl FnOnce(
            &super::DeclaredChildRecipe,
            &crate::make_vars::MakeFileEvent,
            &MatchedOwnerChildCoordinates,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
        let selected = self
            .recipes
            .get(recipe_index)
            .ok_or("declared Source recipe is absent")?;
        let origin = self.origin(recipe_index)?;
        self.declared
            .source
            .snapshot
            .transaction(|_| consume(selected, origin, self.declared.coordinates()))
            .map(|(result, _)| result)
    }

    /// Bind one explicitly indexed literal recipe to selected program roles
    /// and caller-selected input fields.
    ///
    /// `recipe_index` indexes this projection's retained recipe slice; no
    /// recipe is found by target name or by scanning for a matching command.
    /// Artifact requests use the one-recipe batch coordinate `recipe: 0`, not
    /// `recipe_index`. Only input requests are accepted. The callback runs
    /// inside both the original Source snapshot transaction and the selected
    /// program-set transaction. It must derive a private result only: it must
    /// not execute commands or publish side effects before those transactions
    /// finish. The returned value is available only after both final checks.
    ///
    /// This is a data consistency join. It does not resolve other leaves,
    /// authorize output/producer ownership, execute the selected command, or
    /// grant scheduling/native execution authority. Every unselected recipe
    /// expression and the full declaration frontier remain on `self`.
    ///
    /// # Errors
    /// Refuses an invalid recipe index, unsupported literal shell, output or
    /// nonzero batch-coordinate request, invalid input field, callback error,
    /// or drift in the original Source/program inputs.
    pub fn with_recipe_input_bindings<T>(
        &self,
        recipe_index: usize,
        programs: &aros_common::captured_programs::CapturedProgramSet,
        requests: &[crate::make_shell_data::artifact_binding::RecipeArtifactRequest],
        consume: impl FnOnce(
            &crate::make_shell_data::program_binding::RecipeProgramBindings<'_>,
            &[crate::make_shell_data::artifact_binding::BoundRecipeArtifact],
        ) -> Result<T, String>,
    ) -> Result<(T, aros_common::make_input_snapshot::VerifiedMakeInputReads), String> {
        self.declared.source.snapshot.transaction(|reader| {
            self.with_recipe_input_bindings_in_reader(
                recipe_index,
                programs,
                requests,
                reader,
                consume,
            )
        })
    }

    /// Bind input fields without opening another transaction for this child.
    /// Program identities retain their own before/after checks. The result and
    /// all read manifests remain provisional until enclosing postchecks pass.
    /// # Errors
    /// Refuses reader-context drift and all ordinary input/program binding errors.
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
            return Err("declared recipe input reader context differs from original match".into());
        }
        let selected = self
            .recipes
            .get(recipe_index)
            .ok_or("declared Source recipe is absent")?;
        bind_recipe_inputs(selected, programs, requests, reader, consume)
    }
}

impl MatchedChildSource {
    /// Retain the original child's declared frontier without admitting leaves.
    ///
    /// Uses only the already interpreted events and original matched goals.
    /// Does not query missing paths, run Make, evaluate recipes or select a
    /// producer by filename. Implicit and non-rule obligations remain explicit.
    /// The separate result type has no automatic conversion to admitted rules.
    /// # Errors
    /// Refuses changed inputs, malformed declarations, cycles and graph limits.
    pub fn trace_declared_rules(self) -> Result<MatchedChildDeclaredRules, String> {
        let snapshot = self.snapshot.clone();
        snapshot
            .transaction(|reader| self.trace_declared_rules_in_reader(reader))
            .map(|(result, _)| result)
    }

    /// Trace this privately owned child within its exact captured reader.
    /// The returned data remains provisional until enclosing postchecks pass.
    /// # Errors
    /// Refuses another reader context and all ordinary declaration errors.
    pub(crate) fn trace_declared_rules_in_reader(
        self,
        reader: &MakeInputReader<'_>,
    ) -> Result<MatchedChildDeclaredRules, String> {
        if !self.snapshot.same_read_context(reader.snapshot()) {
            return Err(
                "native Source declaration reader context differs from original match".into(),
            );
        }
        let interpreted = make_interpreted_rules::collect(&self.file)
            .map_err(|_| "native Source declared rule collection failed")?;
        let projected = interpreted
            .project_explicit()
            .map_err(|_| "native Source declared rule projection failed")?;
        let goals: Vec<_> = self
            .coordinates
            .goals()
            .iter()
            .map(String::as_str)
            .collect();
        let traversal = trace_declared(&projected.graph, &goals)
            .map_err(|_| "native Source declared rule traversal failed")?;
        let non_rule_events = interpreted.other_events;
        Ok(MatchedChildDeclaredRules {
            source: self,
            graph: projected.graph,
            traversal,
            implicit_events: projected.implicit_rule_events,
            non_rule_events,
        })
    }
}
