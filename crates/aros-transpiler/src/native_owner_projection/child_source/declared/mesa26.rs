//! Mesa-26-only binding for the VC4 GCA original-recipe exception.
//!
//! This is Source/frame consistency, not a process, output or authentication
//! capability. A runtime must still bind the selected shell/tools and own all
//! generated inputs, effects, producer order and original execution budgets.

use super::super::DeclaredChildRecipeProjection;
use crate::make_vars::{MakeProgramPaths, MakeRecipeProjection, MakeTargetRecipeRequest};
use crate::native_owner_projection::NativeOwnerProjection;
use std::path::{Path, PathBuf};

const SOURCE: &str = "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/mmakefile.src";
const GOAL: &str = "linklibs-gallium_vc4-gca";

mod intrinsic;

/// A frozen, source-positioned recipe from the selected Mesa 26 VC4 GCA lane.
///
/// Private construction prevents substituting an arbitrary shell string. No
/// process may be launched merely because this data binding exists. It does
/// not grant the exception to other recipes in a Mesa build's dependency tree.
pub struct Mesa26OriginalRecipeBinding<'a> {
    source_root: PathBuf,
    projection: &'a NativeOwnerProjection,
    recipes: &'a DeclaredChildRecipeProjection,
    recipe_index: usize,
    shell: String,
    flags: String,
    frame_reads: aros_common::make_input_snapshot::VerifiedMakeInputReads,
    intrinsic_selection: Option<aros_common::Sha256Digest>,
}

impl Mesa26OriginalRecipeBinding<'_> {
    /// Expanded intrinsic-role consistency, not a producer authentication digest.
    #[must_use]
    pub const fn intrinsic_selection_sha256(&self) -> Option<&aros_common::Sha256Digest> {
        self.intrinsic_selection.as_ref()
    }

    /// The original expanded body and exports, not an independently supplied command.
    #[must_use]
    pub fn recipe(&self) -> &MakeRecipeProjection {
        self.recipes.recipes[self.recipe_index].expression()
    }

    /// Exact shell spelling selected in the original target/automatic frame.
    #[must_use]
    pub fn shell(&self) -> &str {
        &self.shell
    }

    /// Exact shell controls selected in that same frame; no default is inferred.
    #[must_use]
    pub fn flags(&self) -> &str {
        &self.flags
    }

    /// Original captured child directory; callers cannot substitute another frame.
    #[must_use]
    pub fn working_directory(&self) -> &Path {
        self.recipes.declared.source.snapshot.working_directory()
    }

    /// Return the selected rule's normalized literal output under its captured build root.
    ///
    /// This is a data-only candidate path. It does not claim that the recipe
    /// creates the path, that the path exists, or that the caller may write it.
    /// # Errors
    /// Refuses Source/frame drift or an absent selected rule.
    pub fn declared_output_path(&self) -> Result<Option<PathBuf>, String> {
        self.revalidate()?;
        let visit = self
            .recipes
            .declared
            .traversal
            .visits()
            .get(self.recipes.recipes[self.recipe_index].visit())
            .ok_or("original recipe lost its selected visit")?;
        let rule = visit
            .instance()
            .ok_or("original recipe lost its selected rule")?;
        Ok(super::links::literal_output(
            &self.recipes.declared,
            &rule.target,
            &self.recipes.declared.source.build_root,
        ))
    }

    /// Resolve this rule's direct normal/order-only prerequisites to captured physical files.
    ///
    /// The returned vector preserves Source order and repetitions. Names are
    /// selected by the retained rule; phony, missing, linked, and non-file
    /// entries refuse. This makes no producer or scheduling claim.
    /// # Errors
    /// Refuses Source/input drift or any prerequisite that is not a captured regular file.
    pub fn captured_prerequisite_paths(&self) -> Result<Vec<PathBuf>, String> {
        self.revalidate()?;
        let visit = self
            .recipes
            .declared
            .traversal
            .visits()
            .get(self.recipes.recipes[self.recipe_index].visit())
            .ok_or("original recipe lost its selected visit")?;
        let rule = visit
            .instance()
            .ok_or("original recipe lost its selected rule")?;
        self.recipes
            .declared
            .source
            .snapshot
            .transaction(|reader| {
                let mut paths = Vec::with_capacity(rule.prerequisite_order.len());
                for prerequisite in &rule.prerequisite_order {
                    if self.recipes.declared.file().events.iter().any(|event| {
                        matches!(&event.kind,
                            crate::make_vars::MakeFileEventKind::PhonyTargets(names)
                                if names.contains(prerequisite))
                    }) {
                        return Err(
                            "original recipe prerequisite is phony, not a captured file input"
                                .into(),
                        );
                    }
                    if reader.entry_kind(prerequisite)?
                        != Some(aros_common::TreeContentEntryKind::RegularFile)
                    {
                        return Err(
                            "original recipe prerequisite is not a captured regular input".into(),
                        );
                    }
                    paths.push(
                        reader
                            .realpath(prerequisite)?
                            .ok_or("original recipe prerequisite has no captured physical path")?,
                    );
                }
                Ok(paths)
            })
            .map(|(paths, _)| paths)
    }

    /// Return reached, recipe-less literal aliases of this rule's output.
    ///
    /// Only single-normal-prerequisite rules naming this exact target qualify.
    /// Paths are normalized under the original build root and returned in
    /// deterministic order. This is Source graph data, not evidence that an
    /// alias exists, was written, or has an independent producer.
    /// # Errors
    /// Refuses Source/frame drift or an absent selected rule.
    pub fn declared_cooutput_paths(&self) -> Result<Vec<PathBuf>, String> {
        self.revalidate()?;
        let declared = &self.recipes.declared;
        let selected_visit = declared
            .traversal
            .visits()
            .get(self.recipes.recipes[self.recipe_index].visit())
            .ok_or("original recipe lost its selected visit")?;
        let selected_rule = selected_visit
            .instance()
            .ok_or("original recipe lost its selected rule")?;
        let selected_target = selected_rule.target.as_str();
        let mut paths = std::collections::BTreeSet::new();
        for visit in declared.traversal.visits() {
            let Some(alias) = visit.instance() else {
                continue;
            };
            if alias.prerequisites.len() != 1
                || alias.prerequisites[0] != selected_target
                || !alias.order_only.is_empty()
                || alias.prerequisite_order.len() != 1
                || alias.prerequisite_order[0] != selected_target
                || alias.double_colon
                || !alias.recipes.is_empty()
                || declared.file().events.iter().any(|event| {
                    matches!(&event.kind,
                        crate::make_vars::MakeFileEventKind::PhonyTargets(names)
                            if names.contains(&alias.target))
                })
            {
                continue;
            }
            if let Some(path) =
                super::links::literal_output(declared, &alias.target, &declared.source.build_root)
            {
                paths.insert(path);
            }
        }
        Ok(paths.into_iter().collect())
    }

    /// Require this rule's direct normal/order-only prerequisites as captured files.
    ///
    /// This is an explicit captured-input execution mode, not Make timestamp
    /// scheduling or proof that their declared producers ran. Names cannot be
    /// supplied by the caller; phony/missing/directory prerequisites refuse.
    /// Captured prerequisites must be disjoint from every selected output.
    /// # Errors
    /// Refuses unavailable/unsafe prerequisites or an overlapping effect selection.
    pub fn verify_captured_prerequisites(
        &self,
        outputs: (&[&str], &[&str], &[&str]),
        replaceable_regular_files: &[&str],
    ) -> Result<(), String> {
        self.revalidate()?;
        let rule = self.recipes.declared.traversal.visits()
            [self.recipes.recipes[self.recipe_index].visit()]
        .instance()
        .ok_or("original recipe lost its selected rule")?;
        self.recipes
            .declared
            .source
            .snapshot
            .transaction(|reader| {
                let selected = outputs
                    .0
                    .iter()
                    .chain(outputs.1)
                    .chain(outputs.2)
                    .chain(replaceable_regular_files)
                    .copied()
                    .collect::<Vec<_>>();
                let mut unique = std::collections::BTreeSet::new();
                if selected.len() > 65_536 {
                    return Err("original recipe output selection exceeds path bound".into());
                }
                for path in &selected {
                    if path.is_empty()
                        || path.len() > 8192
                        || path.starts_with('/')
                        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
                        || !unique.insert(*path)
                    {
                        return Err("original recipe output path is not normalized/unique".into());
                    }
                }
                for path in &selected {
                    if path
                        .match_indices('/')
                        .any(|(at, _)| unique.contains(&path[..at]))
                    {
                        return Err("original recipe output selections overlap by ancestry".into());
                    }
                    let output = self.recipes.declared.source.build_root.join(path);
                    for group in [
                        self.recipes.input_reads(),
                        self.recipes.evaluation_reads(),
                        &self.frame_reads,
                    ] {
                        if group
                            .reads()
                            .iter()
                            .any(|read| read.path.starts_with(&output))
                        {
                            return Err(
                                "original recipe selected a retained Make input as output".into()
                            );
                        }
                    }
                }
                for path in outputs.0.iter().chain(replaceable_regular_files) {
                    let path = self.recipes.declared.source.build_root.join(path);
                    if reader.entry_kind(path.to_str().ok_or("non-text original output path")?)?
                        != Some(aros_common::TreeContentEntryKind::RegularFile)
                    {
                        return Err(
                            "original recipe mutable output is not a captured regular file".into(),
                        );
                    }
                }
                for path in outputs.1.iter().chain(outputs.2) {
                    let path = self.recipes.declared.source.build_root.join(path);
                    if reader
                        .entry_kind(path.to_str().ok_or("non-text original output path")?)?
                        .is_some()
                    {
                        return Err("original recipe fresh output already exists".into());
                    }
                }
                for prerequisite in &rule.prerequisite_order {
                    if self.recipes.declared.file().events.iter().any(|event| {
                        matches!(&event.kind,
                            crate::make_vars::MakeFileEventKind::PhonyTargets(names)
                                if names.contains(prerequisite))
                    }) {
                        return Err(
                            "original recipe prerequisite is phony, not a captured file input"
                                .into(),
                        );
                    }
                    if reader.entry_kind(prerequisite)?
                        != Some(aros_common::TreeContentEntryKind::RegularFile)
                    {
                        return Err(
                            "original recipe prerequisite is not a captured regular input".into(),
                        );
                    }
                    let path = reader
                        .realpath(prerequisite)?
                        .ok_or("original recipe prerequisite has no captured physical path")?;
                    for output in outputs
                        .0
                        .iter()
                        .chain(outputs.1)
                        .chain(outputs.2)
                        .chain(replaceable_regular_files)
                    {
                        let output = self.recipes.declared.source.build_root.join(output);
                        if path.starts_with(&output) || output.starts_with(&path) {
                            return Err(
                                "original recipe prerequisite overlaps a selected output".into()
                            );
                        }
                    }
                }
                Ok(())
            })
            .map(|_| ())
    }

    /// Observe and check selected output effects against the held original inputs.
    ///
    /// An empty selection means no filesystem effect is allowed. This never
    /// renews the old snapshot or establishes producer/scheduling authority.
    /// # Errors
    /// Refuses unselected effects, Source/frame drift or retained-input changes.
    pub fn observe_output_transition(
        &self,
        outputs: (&[&str], &[&str], &[&str]),
        replaceable_regular_files: &[&str],
    ) -> Result<(), String> {
        if outputs.0.is_empty()
            && outputs.1.is_empty()
            && outputs.2.is_empty()
            && replaceable_regular_files.is_empty()
        {
            return self.revalidate();
        }
        let after = self
            .recipes
            .declared
            .source
            .snapshot
            .observe_output_root(&self.recipes.declared.source.build_root)?;
        self.verify_output_transition_with_replacements(&after, outputs, replaceable_regular_files)
    }

    /// Recheck the held Source expansion and generated program before use.
    ///
    /// This does not renew a runtime budget, create a producer receipt or admit outputs.
    /// # Errors
    /// Refuses changed Source, generated entry program, origin or input snapshot.
    pub fn revalidate(&self) -> Result<(), String> {
        self.recipes.with_source_generated_recipe(
            &self.source_root,
            self.projection,
            self.recipe_index,
            |_, _, _| Ok(()),
        )
    }

    /// Compare admitted output effects while preserving this recipe's input reads.
    ///
    /// Supplies all held Source-file, recipe-evaluation and concrete frame read
    /// groups and the original build root; a caller cannot replace either.
    /// Output names remain a separate
    /// owning-runner selection, not producer/scheduling permission. This does
    /// not renew the original snapshot or authorize subsequent recipe evaluation.
    /// # Errors
    /// Refuses Source drift, changed input domains/reads or effects outside the
    /// supplied `(mutable files, fresh files, fresh directories)` selection.
    pub fn verify_output_transition(
        &self,
        after: &aros_common::make_input_snapshot::MakeInputSnapshot,
        outputs: (&[&str], &[&str], &[&str]),
    ) -> Result<(), String> {
        self.verify_output_transition_with_replacements(after, outputs, &[])
    }

    /// Check explicitly replaceable regular outputs under the same fixed build root.
    ///
    /// Replacement is distinct from in-place mutation and never applies to held
    /// Source/frame inputs. This data check does not itself grant execution,
    /// producer/order authority or renew this binding's original snapshot.
    /// # Errors
    /// Refuses any unselected effect, retained-input replacement or input drift.
    pub fn verify_output_transition_with_replacements(
        &self,
        after: &aros_common::make_input_snapshot::MakeInputSnapshot,
        outputs: (&[&str], &[&str], &[&str]),
        replaceable_regular_files: &[&str],
    ) -> Result<(), String> {
        self.projection.verify(&self.source_root)?;
        self.recipes
            .declared
            .source
            .snapshot
            .verify_output_transition_with_replacements(
                after,
                &self.recipes.declared.source.build_root,
                outputs,
                replaceable_regular_files,
                &[
                    self.recipes.input_reads(),
                    self.recipes.evaluation_reads(),
                    &self.frame_reads,
                ],
            )?;
        self.projection.verify(&self.source_root)
    }
}

impl DeclaredChildRecipeProjection {
    /// Bind one original VC4 GCA recipe only in the independently selected Mesa 26 lane.
    ///
    /// The default covers only the GCA goal or its direct prerequisite.
    /// A deeper intrinsic producer additionally requires an explicit sealed
    /// Source policy and its Source-owned producer/cooutput/consumer relation;
    /// reachability alone never selects deeper recipes.
    /// Besides exact Source-owner and first-parent goal selection, all selected
    /// closed target selectors are compared in this recipe's actual frame.
    /// A command-line or target-specific Mesa20/ABI override therefore refuses.
    /// The body, exports, SHELL and .SHELLFLAGS are evaluated together under the
    /// original captured snapshot, not taken from global or ambient defaults.
    /// This data-only operation does not widen the literal reader or launch a shell.
    ///
    /// # Errors
    /// Refuses other Mesa versions, owners, goals, target/frame drift, changed
    /// original inputs, unknown expressions and existing evaluation bounds.
    pub fn bind_mesa26_original_recipe<'a>(
        &'a mut self,
        projection: &'a NativeOwnerProjection,
        paths: &MakeProgramPaths<'_>,
        recipe_index: usize,
    ) -> Result<Mesa26OriginalRecipeBinding<'a>, String> {
        let context = &projection.architecture_context;
        if crate::capability::mesa::mesa26::profile(Some(context))?.is_none() {
            return Err("original-recipe exception requires selected Mesa 26 policy".into());
        }
        if self.declared.coordinates().source_input() != SOURCE
            || !self
                .declared
                .coordinates()
                .goals()
                .iter()
                .any(|goal| goal == GOAL)
        {
            return Err(
                "original-recipe exception requires the exact Mesa26 GCA owner/goal".into(),
            );
        }
        if paths.top_directory != self.declared.source.build_root {
            return Err("original-recipe frame differs from its captured build root".into());
        }
        if !self.declared.source.default_directory_context
            || paths.dirs != &crate::dirs::DirVars::default()
        {
            return Err(
                "original-recipe frame requires the original native directory context".into(),
            );
        }
        self.with_source_generated_recipe(
            paths.source_root,
            projection,
            recipe_index,
            |_, _, _| Ok(()),
        )?;
        let selected = self
            .recipes
            .get(recipe_index)
            .ok_or("selected original recipe is absent")?;
        let chain = self.declared.traversal.scope_chain(selected.visit())?;
        let producer_policies: Vec<_> = projection
            .mesa26_original_producers
            .iter()
            .filter(|selection| selection.source_input == SOURCE && selection.goal == GOAL)
            .collect();
        if chain.first().copied() != Some(GOAL) || (chain.len() > 2 && producer_policies.is_empty())
        {
            return Err(
                "original-recipe exception does not cover another goal or deeper dependency".into(),
            );
        }
        if self.declared.coordinates().goals().len() != 1 {
            return Err("original-recipe exception requires the exact single GCA goal".into());
        }
        let rule = self.declared.traversal.visits()[selected.visit()]
            .instance()
            .ok_or("original recipe rule is absent")?;
        let automatic = rule.automatic_variables();
        let original = rule
            .recipes
            .get(selected.recipe())
            .ok_or("original recipe text is absent")?;
        let expected: Vec<_> = aros_common::native_build_contract::NATIVE_RESERVED_MAKE_VARIABLES
            .iter()
            .map(|name| (*name, context.value_of(name)))
            .collect();
        let expressions: Vec<_> = std::iter::once(original.text.clone())
            .chain(expected.iter().map(|(name, selected)| {
                if selected.is_some() {
                    format!("$({name})")
                } else {
                    // Unknown policy is not known-empty. Reject even a defined
                    // empty frame value instead of silently inheriting it.
                    format!("$(origin {name})")
                }
            }))
            .chain(producer_policies.iter().flat_map(|selection| {
                [
                    selection.producer.clone(),
                    selection.cooutput.clone(),
                    selection.consumer.clone(),
                ]
            }))
            .chain(["$(SHELL)".to_owned(), "$(.SHELLFLAGS)".to_owned()])
            .collect();
        let requests: Vec<_> = expressions
            .iter()
            .map(|expression| MakeTargetRecipeRequest {
                chain: &chain,
                expression,
                automatic_variables: &automatic,
            })
            .collect();
        let (values, frame_reads) = self
            .declared
            .source
            .program
            .evaluate_many_captured_target_recipes(&requests, paths, &self.declared.source.snapshot)
            .map_err(|_| "original recipe frame cannot be independently resolved")?;
        if values.len() != expressions.len() || values.first() != Some(selected.expression()) {
            return Err("original recipe body or exports changed during frame binding".into());
        }
        for (observed, (_, expected)) in values.iter().skip(1).zip(&expected) {
            if let Some(expected) = expected {
                if observed.expression() == expected {
                    continue;
                }
                return Err(
                    "original recipe target selectors differ from selected Mesa26 policy".into(),
                );
            }
            if observed.expression() != "undefined" {
                return Err("original recipe has an unselected target selector".into());
            }
        }
        let intrinsic_selection = if chain.len() > 2 {
            Some(intrinsic::verify_producer_selection(
                self,
                selected.visit(),
                &values[1 + expected.len()..values.len() - 2],
            )?)
        } else {
            None
        };
        let shell = values[values.len() - 2].expression().to_owned();
        let flags = values[values.len() - 1].expression().to_owned();
        let binding = Mesa26OriginalRecipeBinding {
            source_root: paths.source_root.to_path_buf(),
            projection,
            recipes: self,
            recipe_index,
            shell,
            flags,
            frame_reads,
            intrinsic_selection,
        };
        binding.revalidate()?;
        Ok(binding)
    }
}
