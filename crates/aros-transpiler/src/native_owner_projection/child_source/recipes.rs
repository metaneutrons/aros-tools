//! Derive recipe frames from one retained Source program, not caller formulas.

use super::{MatchedChildDeclaredRules, MatchedChildSource, MatchedOwnerChildCoordinates};
use crate::{
    make_interpreted_rules,
    make_rule_graph::traversal::{select_explicit, ExplicitTraversal, TargetVisit},
    make_vars::{
        MakeFileEvent, MakeFileProgram, MakeProgramPaths, MakeRecipeProjection,
        MakeRecipeShellProjection, MakeTargetRecipeRequest, MakeVariableProgram,
    },
};
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot, VerifiedMakeInputReads},
    TreeContentEntryKind,
};
use std::cell::RefCell;

const MAX_RECIPES: usize = 65_536;
const MAX_STORAGE: usize = 64 * 1024 * 1024;

/// One original Source recipe, evaluated in its derived first-parent frame.
///
/// The indices address this plan's traversal and effective rule, never a
/// separately supplied AST or a line number guessed from an owner name.
pub struct SelectedChildRecipe {
    visit: usize,
    recipe: usize,
    expression: MakeRecipeProjection,
    shell: MakeRecipeShellProjection,
}

/// One declared recipe expression evaluated in its original child frame.
///
/// Shell syntax is deliberately not parsed here: unsupported commands remain
/// available as source-derived expressions and do not invalidate other frames.
pub struct DeclaredChildRecipe {
    pub(super) visit: usize,
    pub(super) recipe: usize,
    pub(super) expression: MakeRecipeProjection,
}

impl DeclaredChildRecipe {
    #[must_use]
    pub const fn visit(&self) -> usize {
        self.visit
    }

    #[must_use]
    pub const fn recipe(&self) -> usize {
        self.recipe
    }

    #[must_use]
    pub const fn expression(&self) -> &MakeRecipeProjection {
        &self.expression
    }
}

impl std::fmt::Debug for DeclaredChildRecipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeclaredChildRecipe")
            .field("visit", &self.visit)
            .field("recipe", &self.recipe)
            .finish_non_exhaustive()
    }
}

/// Captured Make recipe expressions projected over a declaration-only trace.
///
/// Unresolved leaves, implicit rules and non-rule obligations remain available
/// through `declared()`. The expressions are data, not admitted recipes or an
/// execution/scheduling claim; callers may parse each expression separately.
pub struct DeclaredChildRecipeProjection {
    pub(super) declared: MatchedChildDeclaredRules,
    pub(super) recipes: Vec<DeclaredChildRecipe>,
    pub(super) evaluation_reads: VerifiedMakeInputReads,
}

impl std::fmt::Debug for DeclaredChildRecipeProjection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeclaredChildRecipeProjection")
            .field("recipe_count", &self.recipes.len())
            .finish_non_exhaustive()
    }
}

impl DeclaredChildRecipeProjection {
    #[must_use]
    pub const fn declared(&self) -> &MatchedChildDeclaredRules {
        &self.declared
    }

    #[must_use]
    pub fn recipes(&self) -> &[DeclaredChildRecipe] {
        &self.recipes
    }

    #[must_use]
    pub const fn input_reads(&self) -> &VerifiedMakeInputReads {
        self.declared.input_reads()
    }

    #[must_use]
    pub const fn evaluation_reads(&self) -> &VerifiedMakeInputReads {
        &self.evaluation_reads
    }

    /// Revalidate the original captured child inputs.
    /// # Errors
    /// Refuses any original source/build/input drift.
    pub fn verify(&self) -> Result<(), String> {
        self.declared.verify()
    }

    /// Return the original event for one recipe expression.
    /// # Errors
    /// Refuses an index or source origin outside this internally derived result.
    pub fn origin(&self, recipe_index: usize) -> Result<&MakeFileEvent, String> {
        let selected = self
            .recipes
            .get(recipe_index)
            .ok_or("declared Source recipe is absent")?;
        let recipe = self
            .declared
            .traversal()
            .visits()
            .get(selected.visit)
            .and_then(|visit| visit.instance())
            .and_then(|rule| rule.recipes.get(selected.recipe))
            .ok_or("declared Source recipe origin is absent")?;
        self.declared
            .file()
            .events
            .get(recipe.output_line)
            .ok_or_else(|| "declared Source event origin is absent".into())
    }
}

impl SelectedChildRecipe {
    #[must_use]
    pub const fn visit(&self) -> usize {
        self.visit
    }
    #[must_use]
    pub const fn recipe(&self) -> usize {
        self.recipe
    }
    #[must_use]
    pub const fn expression(&self) -> &MakeRecipeProjection {
        &self.expression
    }
    #[must_use]
    pub const fn shell(&self) -> &MakeRecipeShellProjection {
        &self.shell
    }
}

impl std::fmt::Debug for SelectedChildRecipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectedChildRecipe")
            .field("visit", &self.visit)
            .field("recipe", &self.recipe)
            .finish_non_exhaustive()
    }
}

/// Data projection of finite explicit rules, not a completed Make execution.
///
/// Implicit declarations and non-rule policies remain retained obligations.
/// No timestamp/VPATH/default/phony/ONESHELL policy is silently discharged.
pub struct MatchedChildRecipePlan {
    coordinates: MatchedOwnerChildCoordinates,
    build_root: std::path::PathBuf,
    file: MakeFileProgram,
    traversal: ExplicitTraversal,
    recipes: Vec<SelectedChildRecipe>,
    implicit_events: Vec<usize>,
    non_rule_events: Vec<usize>,
    snapshot: MakeInputSnapshot,
    reads: [VerifiedMakeInputReads; 3],
}

/// Original recipe commands and explicitly selected file fields, bound together.
///
/// No execution, writable-root or complete-field/producer authority is added.
/// The owning adapter must revalidate before later use and preserve all unknown
/// commands, original branch/pipeline structure and remaining rule obligations.
pub struct BoundChildRecipeBatch<'a> {
    bindings: Vec<crate::make_shell_data::program_binding::RecipeProgramBindings<'a>>,
    artifacts: Vec<crate::make_shell_data::artifact_binding::BoundRecipeArtifact>,
    reads: VerifiedMakeInputReads,
}

impl std::fmt::Debug for BoundChildRecipeBatch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundChildRecipeBatch")
            .field("recipe_count", &self.bindings.len())
            .field("artifact_count", &self.artifacts.len())
            .finish_non_exhaustive()
    }
}

impl<'a> BoundChildRecipeBatch<'a> {
    #[must_use]
    pub fn bindings(
        &self,
    ) -> &[crate::make_shell_data::program_binding::RecipeProgramBindings<'a>] {
        &self.bindings
    }
    #[must_use]
    pub fn artifacts(&self) -> &[crate::make_shell_data::artifact_binding::BoundRecipeArtifact] {
        &self.artifacts
    }
    #[must_use]
    pub const fn reads(&self) -> &VerifiedMakeInputReads {
        &self.reads
    }
}

impl std::fmt::Debug for MatchedChildRecipePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MatchedChildRecipePlan")
            .field("recipe_count", &self.recipes.len())
            .field("implicit_count", &self.implicit_events.len())
            .field("non_rule_count", &self.non_rule_events.len())
            .finish_non_exhaustive()
    }
}

impl MatchedChildRecipePlan {
    /// Bind program identities and explicit file fields in one original-input batch.
    ///
    /// The caller selects argv spans/redirections, not copied option formulas.
    /// Inputs are descriptor-read regular files (scripts need not be executable).
    /// Outputs require the original matched build root, with its independently
    /// selected content already present in the original snapshot. A captured
    /// Source root or a replacement build coordinate cannot be substituted.
    /// The owning native adapter still binds configured preparation authority;
    /// this consistency boundary does not grant writes.
    /// Any later invalid field discards the entire batch. No replacement input
    /// snapshot is accepted and no command, script or target compiler executes.
    ///
    /// # Errors
    /// Refuses mismatched output-root selection, original input/program drift,
    /// invalid artifact fields/kinds or complete batch resource exhaustion.
    pub fn bind_artifacts<'a>(
        &'a self,
        programs: &'a aros_common::captured_programs::CapturedProgramSet,
        output_root: Option<&MakeInputRootSelection>,
        requests: &[crate::make_shell_data::artifact_binding::RecipeArtifactRequest],
    ) -> Result<BoundChildRecipeBatch<'a>, String> {
        if output_root.is_some_and(|selected| {
            selected.root != self.build_root
                || !self
                    .snapshot
                    .matches_root_selection(&selected.root, &selected.content_sha256)
        }) {
            return Err("native Source output root differs from original selection".into());
        }
        let recipes: Vec<_> = self
            .recipes
            .iter()
            .map(|recipe| recipe.shell.statements.as_slice())
            .collect();
        let ((bindings, artifacts), reads) = self
            .snapshot
            .transaction(|reader| {
                crate::make_shell_data::program_binding::with_literal_program_bindings(
                    &recipes,
                    programs,
                    |bindings| {
                        let artifacts =
                            crate::make_shell_data::artifact_binding::bind_recipe_artifacts(
                                &bindings,
                                reader,
                                output_root.map(|selected| selected.root.as_path()),
                                requests,
                            )?;
                        Ok((bindings, artifacts))
                    },
                )
            })
            .map_err(|_| "native Source artifact binding transaction failed".to_owned())?;
        Ok(BoundChildRecipeBatch {
            bindings,
            artifacts,
            reads,
        })
    }

    /// Bind all recipe program words while original Source/preparation and
    /// independently selected program roots both remain unchanged.
    ///
    /// This retains unresolved commands/policies. The owning native caller
    /// must also tie the selected program roots to its compiler/host contracts;
    /// neither this data join nor role labels authenticate that authority.
    ///
    /// # Errors
    /// Refuses original input/program drift or any aggregate binding error.
    pub fn bind_programs<'a>(
        &'a self,
        programs: &'a aros_common::captured_programs::CapturedProgramSet,
    ) -> Result<Vec<crate::make_shell_data::program_binding::RecipeProgramBindings<'a>>, String>
    {
        let recipes: Vec<_> = self
            .recipes
            .iter()
            .map(|recipe| recipe.shell.statements.as_slice())
            .collect();
        self.snapshot
            .transaction(|_| {
                crate::make_shell_data::program_binding::bind_literal_program_batches(
                    &recipes, programs,
                )
            })
            .map(|(bindings, _)| bindings)
            .map_err(|_| "native Source program binding transaction failed".into())
    }

    #[must_use]
    pub const fn coordinates(&self) -> &MatchedOwnerChildCoordinates {
        &self.coordinates
    }
    #[must_use]
    pub const fn file(&self) -> &MakeFileProgram {
        &self.file
    }
    #[must_use]
    pub const fn traversal(&self) -> &ExplicitTraversal {
        &self.traversal
    }
    #[must_use]
    pub fn recipes(&self) -> &[SelectedChildRecipe] {
        &self.recipes
    }
    #[must_use]
    pub fn implicit_events(&self) -> &[usize] {
        &self.implicit_events
    }
    #[must_use]
    pub fn non_rule_events(&self) -> &[usize] {
        &self.non_rule_events
    }
    #[must_use]
    pub const fn reads(&self) -> &[VerifiedMakeInputReads; 3] {
        &self.reads
    }

    /// Revalidate the original input selection, never a freshly adopted tree.
    /// # Errors
    /// Refuses any Source/preparation/input identity or content drift.
    pub fn verify(&self) -> Result<(), String> {
        self.snapshot.verify()
    }

    /// Return the source-positioned event for one selected recipe.
    /// # Errors
    /// Refuses an index not belonging to this internally derived plan.
    pub fn origin(&self, recipe_index: usize) -> Result<&MakeFileEvent, String> {
        let selected = self
            .recipes
            .get(recipe_index)
            .ok_or("selected Source recipe is absent")?;
        let recipe = self
            .traversal
            .visits()
            .get(selected.visit)
            .and_then(|visit| visit.instance())
            .and_then(|rule| rule.recipes.get(selected.recipe))
            .ok_or("selected Source rule origin is absent")?;
        self.file
            .events
            .get(recipe.output_line)
            .ok_or_else(|| "selected Source event origin is absent".into())
    }
}

/// Evaluate every recipe frame on one bounded traversal, without parsing shell
/// syntax. Strict and declaration-only projections share the same source,
/// first-parent chains, automatic values, budgets and captured evaluation.
pub(super) fn evaluate_recipe_frames<'a>(
    program: &mut MakeVariableProgram,
    visits: &'a [TargetVisit],
    completion_order: &[usize],
    scope_chain: &impl Fn(usize) -> Result<Vec<&'a str>, String>,
    paths: &MakeProgramPaths<'_>,
    snapshot: &MakeInputSnapshot,
) -> Result<(Vec<DeclaredChildRecipe>, VerifiedMakeInputReads), String> {
    evaluate_recipe_frames_selected(
        program,
        visits,
        completion_order,
        scope_chain,
        paths,
        snapshot,
        None,
    )
}

pub(super) fn evaluate_recipe_frames_in_reader<'a>(
    program: &mut MakeVariableProgram,
    visits: &'a [TargetVisit],
    completion_order: &[usize],
    scope_chain: &impl Fn(usize) -> Result<Vec<&'a str>, String>,
    paths: &MakeProgramPaths<'_>,
    reader: &mut aros_common::make_input_snapshot::MakeInputReader<'_>,
) -> Result<(Vec<DeclaredChildRecipe>, VerifiedMakeInputReads), String> {
    let snapshot = reader.snapshot().clone();
    evaluate_recipe_frames_selected(
        program,
        visits,
        completion_order,
        scope_chain,
        paths,
        &snapshot,
        Some(reader),
    )
}

fn evaluate_recipe_frames_selected<'a>(
    program: &mut MakeVariableProgram,
    visits: &'a [TargetVisit],
    completion_order: &[usize],
    scope_chain: &impl Fn(usize) -> Result<Vec<&'a str>, String>,
    paths: &MakeProgramPaths<'_>,
    snapshot: &MakeInputSnapshot,
    reader: Option<&mut aros_common::make_input_snapshot::MakeInputReader<'_>>,
) -> Result<(Vec<DeclaredChildRecipe>, VerifiedMakeInputReads), String> {
    let mut selected = Vec::new();
    let mut budget = 0usize;
    for visit in completion_order {
        let Some(rule) = visits
            .get(*visit)
            .ok_or("native Source target visit is unavailable")?
            .instance()
        else {
            continue;
        };
        if rule.recipes.is_empty() {
            continue;
        }
        let chain = scope_chain(*visit).map_err(|_| "native Source target frame failed")?;
        // Automatic values repeat several prerequisite/path lists. Charge
        // before constructing them or duplicating per-recipe frames.
        let mut automatic_bound = 1024usize;
        for value in std::iter::once(rule.target.as_str())
            .chain(rule.stem.as_deref())
            .chain(rule.prerequisites.iter().map(String::as_str))
            .chain(rule.order_only.iter().map(String::as_str))
        {
            let cost = value
                .len()
                .checked_add(128)
                .and_then(|bytes| bytes.checked_mul(32))
                .ok_or("native Source automatic frame size overflow")?;
            automatic_bound = automatic_bound
                .checked_add(cost)
                .filter(|size| *size <= MAX_STORAGE)
                .ok_or("native Source automatic frame exceeds storage limit")?;
        }
        for (recipe, source) in rule.recipes.iter().enumerate() {
            if selected.len() == MAX_RECIPES {
                return Err("native Source recipe count exceeds limit".into());
            }
            budget = budget
                .checked_add(automatic_bound)
                .and_then(|size| size.checked_add(source.text.len() + chain.len() * 16 + 128))
                .filter(|size| *size <= MAX_STORAGE)
                .ok_or("native Source recipe frames exceed storage limit")?;
            selected.push((
                *visit,
                recipe,
                chain.clone(),
                rule.automatic_variables(),
                source.text.as_str(),
            ));
        }
    }
    let requests: Vec<_> = selected
        .iter()
        .map(
            |(_, _, chain, automatic_variables, expression)| MakeTargetRecipeRequest {
                chain,
                expression,
                automatic_variables,
            },
        )
        .collect();
    let (values, evaluated_reads) = if let Some(reader) = reader {
        let start = reader.read_checkpoint();
        let values = program
            .evaluate_many_captured_target_recipes_in_reader(&requests, paths, reader)
            .map_err(|_| "native Source recipe evaluation failed")?;
        (values, reader.provisional_reads_since(start)?)
    } else {
        program
            .evaluate_many_captured_target_recipes(&requests, paths, snapshot)
            .map_err(|_| "native Source recipe evaluation failed")?
    };
    if values.len() != selected.len() {
        return Err("native Source recipe result count differs".into());
    }
    let evaluated = selected
        .into_iter()
        .zip(values)
        .map(
            |((visit, recipe, _, _, _), expression)| DeclaredChildRecipe {
                visit,
                recipe,
                expression,
            },
        )
        .collect();
    Ok((evaluated, evaluated_reads))
}

impl MatchedChildSource {
    /// Trace the original observed goals through this paired Source program.
    ///
    /// Headers, recipes and source origins all come from the same interpreted
    /// event stream. Shared prerequisites retain the first selected parent;
    /// automatic variables come from its merged rule instance. External leaves
    /// require regular-file membership in the original captured roots. Recipe
    /// frames and shell projections publish together after complete revalidation.
    /// The caller cannot replace the original snapshot with new prepared bytes.
    ///
    /// This is not implicit/default search, timestamp scheduling, native
    /// parallel first-parent proof or complete policy coverage. Remaining
    /// implicit/non-rule events stay explicit on the result. Shell parsing
    /// does not bind command identities or authorize executing any command.
    ///
    /// # Errors
    /// Refuses unsupported finite rules/selected shell syntax, unbound inputs,
    /// cycles, input drift, and aggregate selection/evaluation/result bounds.
    /// Errors omit raw Source, argv and interpreter values.
    pub fn trace_explicit_recipes(
        self,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<MatchedChildRecipePlan, String> {
        if paths.top_directory != self.build_root {
            return Err("native Source recipe build root differs from original match".into());
        }
        let Self {
            coordinates,
            build_root,
            mut program,
            file,
            reads,
            snapshot,
            retained_estimate_bytes: _,
            default_directory_context: _,
        } = self;
        let (projected, non_rule_events) = {
            let interpreted = make_interpreted_rules::collect(&file)
                .map_err(|_| "native Source rule collection failed")?;
            (
                interpreted
                    .project_explicit()
                    .map_err(|_| "native Source finite rule projection failed")?,
                interpreted.other_events,
            )
        };
        let goals: Vec<_> = coordinates.goals().iter().map(String::as_str).collect();
        let ((traversal, recipes, evaluated_reads), selection_reads) = snapshot
            .transaction(|reader| {
                let reader = RefCell::new(reader);
                let traversal = select_explicit(&projected.graph, &goals, &|path| {
                    Ok(reader.borrow_mut().entry_kind(path)?
                        == Some(TreeContentEntryKind::RegularFile))
                })
                .map_err(|_| "native Source explicit traversal failed")?;
                let (evaluated, evaluated_reads) = evaluate_recipe_frames(
                    &mut program,
                    traversal.visits(),
                    traversal.completion_order(),
                    &|visit| traversal.scope_chain(visit),
                    paths,
                    &snapshot,
                )?;
                let mut recipes = Vec::with_capacity(evaluated.len());
                let mut shell_bytes = 0usize;
                for recipe in evaluated {
                    let shell = recipe
                        .expression
                        .parse_literal_shell()
                        .map_err(|_| "native Source shell projection failed")?;
                    charge_shell(&shell.statements, &mut shell_bytes)?;
                    recipes.push(SelectedChildRecipe {
                        visit: recipe.visit,
                        recipe: recipe.recipe,
                        expression: recipe.expression,
                        shell,
                    });
                }
                Ok((traversal, recipes, evaluated_reads))
            })
            .map_err(|_| "native Source recipe transaction failed")?;
        Ok(MatchedChildRecipePlan {
            coordinates,
            build_root,
            file,
            traversal,
            recipes,
            implicit_events: projected.implicit_rule_events,
            non_rule_events,
            snapshot,
            reads: [reads, selection_reads, evaluated_reads],
        })
    }
}

fn charge_shell(
    statements: &[crate::make_shell_data::LiteralRecipeStatement],
    bytes: &mut usize,
) -> Result<(), String> {
    use crate::make_shell_data::LiteralRecipeStatement;
    for statement in statements {
        *bytes = bytes
            .checked_add(128)
            .filter(|size| *size <= MAX_STORAGE)
            .ok_or("native Source shell batch exceeds storage limit")?;
        match statement {
            LiteralRecipeStatement::Command(command) => {
                charge_command(command, bytes)?;
            }
            LiteralRecipeStatement::Pipeline(commands) => {
                for command in commands {
                    *bytes = bytes
                        .checked_add(128)
                        .filter(|size| *size <= MAX_STORAGE)
                        .ok_or("native Source shell batch exceeds storage limit")?;
                    charge_command(command, bytes)?;
                }
            }
            LiteralRecipeStatement::AndOr { first, rest } => {
                charge_shell(std::slice::from_ref(first.as_ref()), bytes)?;
                for (_, operand) in rest {
                    charge_shell(std::slice::from_ref(operand), bytes)?;
                }
            }
            LiteralRecipeStatement::If {
                condition,
                then_body,
                else_body,
            } => {
                charge_shell(condition, bytes)?;
                charge_shell(then_body, bytes)?;
                charge_shell(else_body, bytes)?;
            }
        }
    }
    Ok(())
}

fn charge_command(
    command: &crate::make_shell_data::LiteralRecipeCommand,
    bytes: &mut usize,
) -> Result<(), String> {
    for value in command.words().iter().map(String::as_str).chain(
        command
            .redirections()
            .iter()
            .map(crate::make_shell_data::RecipeRedirection::target),
    ) {
        *bytes = bytes
            .checked_add(value.len() + 64)
            .filter(|size| *size <= MAX_STORAGE)
            .ok_or("native Source shell batch exceeds storage limit")?;
    }
    Ok(())
}
