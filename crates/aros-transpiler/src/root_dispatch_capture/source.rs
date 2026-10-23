//! Full captured-input interpretation before literal dispatch instrumentation.

use super::{
    matches_pattern, RootDispatchCapturePlan, RootDispatchCaptureSelection, MAX_ROOT_BYTES,
};
use crate::{
    make_interpreted_rules,
    make_rule_header::RuleHeader,
    make_vars::{
        MakeFileEventKind, MakeFileProgram, MakeProgramPaths, MakeRecipeProjection,
        MakeTargetRecipeRequest, MakeVariableProgram,
    },
};
use aros_common::{
    make_input_snapshot::{MakeInputSnapshot, VerifiedMakeInputReads},
    make_startup_arguments::LiteralMakeStartupArguments,
    sha256_bytes, Sha256Digest,
};

mod fenced;
pub use fenced::{FencedRootDispatchPlan, FencedRootDispatchRequest, RootDispatchFencedFiles};

/// A closed source interpretation joined to the literal capture plan.
///
/// The caller still authenticates the initial Make program, original snapshot,
/// startup controls and process owner. This is not native execution authority,
/// a Makefile remake schedule or an arbitrary GNU implicit-search receipt.
pub struct SourceCheckedRootDispatchPlan {
    plan: RootDispatchCapturePlan,
    snapshot: MakeInputSnapshot,
    source_reads: VerifiedMakeInputReads,
    role_reads: VerifiedMakeInputReads,
    startup: LiteralMakeStartupArguments,
    recipe_controls: Option<CapturedRootRecipeControls>,
    selected_recipe: String,
    selected_stem: String,
    shell_effect_policy_bound: bool,
    original_recipe: Option<(MakeRecipeProjection, VerifiedMakeInputReads)>,
}

/// Final Source-expanded shell values with the captured reads they required.
/// This is input data, not shell authentication or recipe execution authority.
pub struct CapturedRootRecipeControls {
    shell: String,
    flags: String,
    reads: VerifiedMakeInputReads,
}

impl CapturedRootRecipeControls {
    fn from_evaluation(values: Vec<String>, reads: VerifiedMakeInputReads) -> Result<Self, String> {
        let [shell, flags]: [String; 2] = values
            .try_into()
            .map_err(|_| "Root recipe controls lost their fixed value count")?;
        if shell.is_empty() {
            return Err("Root recipe controls require a measured shell".into());
        }
        Ok(Self {
            shell,
            flags,
            reads,
        })
    }

    #[must_use]
    pub fn shell(&self) -> &str {
        &self.shell
    }

    #[must_use]
    pub fn flags(&self) -> &str {
        &self.flags
    }

    #[must_use]
    pub const fn reads(&self) -> &VerifiedMakeInputReads {
        &self.reads
    }
}

impl SourceCheckedRootDispatchPlan {
    /// Retain the original recipe and deferred Source exports BEFORE adding
    /// recorder inputs or overrides. This is Source-controlled data, not the
    /// complete native child environment or permission to execute MetaMake.
    /// # Errors
    /// Refuses repeated binding, changed inputs, effectful expansions or a
    /// command differing from the independently selected original roles.
    pub fn with_original_recipe(
        mut self,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<Self, String> {
        if self.original_recipe.is_some() {
            return Err("Root original recipe is already bound".into());
        }
        let projection = self.project_recipe(program, paths, false)?;
        self.original_recipe = Some(projection);
        Ok(self)
    }

    #[must_use]
    pub fn original_recipe(&self) -> Option<&MakeRecipeProjection> {
        self.original_recipe
            .as_ref()
            .map(|(projection, _)| projection)
    }

    #[must_use]
    pub fn original_recipe_reads(&self) -> Option<&VerifiedMakeInputReads> {
        self.original_recipe.as_ref().map(|(_, reads)| reads)
    }

    /// Project the already-selected, prerequisite-free direct recipe using the
    /// exact literal static-pattern recipe retained in the generated input.
    /// Source exports are evaluated in the same isolated automatic frame.
    ///
    /// This checks one literal recorder command, not shell executable policy,
    /// environment equivalence, Makefile-remake safety or launch permission.
    /// The caller must use the original post-Source program bound to this plan.
    ///
    /// # Errors
    /// Refuses changed inputs/cwd, expressions with unsupported effects, any
    /// nonliteral/compound/redirected recipe or a different recorder argv.
    pub fn project_instrumented_recipe(
        &self,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<(MakeRecipeProjection, VerifiedMakeInputReads), String> {
        self.project_recipe(program, paths, true)
    }

    /// Bind controls, original and recorder recipe data in one input transaction.
    ///
    /// This composes the standalone projections without changing their order,
    /// direct automatic frame or exact literal-command checks. Section reads
    /// remain separate. Variable state and all three results publish only after
    /// the complete snapshot and captured-search postchecks pass. Startup hooks,
    /// executable identities and launch permission remain the caller's checks.
    /// # Errors
    /// Refuses repeated binding, different cwd/inputs, unsupported effects,
    /// empty controls and the standalone recipe shape/argv refusals.
    pub fn with_dispatch_recipe_projections(
        mut self,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<(Self, MakeRecipeProjection, VerifiedMakeInputReads), String> {
        if self.original_recipe.is_some() || self.recipe_controls.is_some() {
            return Err("Root dispatch recipe projections are already bound".into());
        }
        if paths.working_directory != self.plan.working_directory {
            return Err("Root recorder projection uses a different working directory".into());
        }
        let automatic = self.direct_automatic_variables()?;
        let goal = self.startup.goals()[0].as_str();
        let requests = [&self.selected_recipe, &self.plan.recorder_recipe].map(|expression| {
            MakeTargetRecipeRequest {
                chain: std::slice::from_ref(&goal),
                expression,
                automatic_variables: &automatic,
            }
        });
        let (controls, original, instrumented) = program.with_captured_direct_recipes(
            &["$(SHELL)", "$(.SHELLFLAGS)"],
            &requests,
            paths,
            &self.snapshot,
            |(values, reads), recipes| {
                let controls = CapturedRootRecipeControls::from_evaluation(values, reads)?;
                let [original, instrumented]: [(MakeRecipeProjection, VerifiedMakeInputReads); 2] =
                    recipes
                        .try_into()
                        .map_err(|_| "Root dispatch lost its exact recipe count")?;
                self.verify_recipe_projection(&original.0, false)?;
                self.verify_recipe_projection(&instrumented.0, true)?;
                Ok((controls, original, instrumented))
            },
        )?;
        self.recipe_controls = Some(controls);
        self.original_recipe = Some(original);
        Ok((self, instrumented.0, instrumented.1))
    }

    fn project_recipe(
        &self,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
        instrumented: bool,
    ) -> Result<(MakeRecipeProjection, VerifiedMakeInputReads), String> {
        if paths.working_directory != self.plan.working_directory {
            return Err("Root recorder projection uses a different working directory".into());
        }
        let automatic = self.direct_automatic_variables()?;
        let goal = self.startup.goals()[0].as_str();
        let projected = program.evaluate_captured_direct_recipe_with_overrides(
            MakeTargetRecipeRequest {
                chain: &[goal],
                expression: if instrumented {
                    &self.plan.recorder_recipe
                } else {
                    &self.selected_recipe
                },
                automatic_variables: &automatic,
            },
            &std::collections::BTreeMap::new(),
            paths,
            &self.snapshot,
        )?;
        self.verify_recipe_projection(&projected.0, instrumented)?;
        Ok(projected)
    }

    fn direct_automatic_variables(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, String> {
        let goal = self
            .startup
            .goals()
            .first()
            .ok_or("Root recorder direct goal is absent")?;
        let mut automatic = std::collections::BTreeMap::new();
        for (name, value) in [
            ("@", goal.as_str()),
            ("*", self.selected_stem.as_str()),
            ("%", ""),
            ("<", ""),
            ("?", ""),
            ("^", ""),
            ("+", ""),
            ("|", ""),
        ] {
            automatic.insert(name.to_owned(), value.to_owned());
            let (directory, file) = if value.is_empty() {
                ("", "")
            } else {
                value.rsplit_once('/').unwrap_or((".", value))
            };
            automatic.insert(format!("{name}D"), directory.to_owned());
            automatic.insert(format!("{name}F"), file.to_owned());
        }
        Ok(automatic)
    }

    fn verify_recipe_projection(
        &self,
        projected: &MakeRecipeProjection,
        instrumented: bool,
    ) -> Result<(), String> {
        let shell = projected.parse_literal_shell()?;
        let [crate::make_shell_data::LiteralRecipeStatement::Command(command)] =
            shell.statements.as_slice()
        else {
            return Err("Root recorder projection is not one simple command".into());
        };
        if shell.prefixes.ignore_errors
            || !command.redirections().is_empty()
            || command.words()
                != if instrumented {
                    &self.plan.expected_recorder_words
                } else {
                    &self.plan.original_recipe_words
                }
        {
            return Err("Root recipe projection differs from its exact literal command".into());
        }
        Ok(())
    }

    #[must_use]
    pub const fn recipe_controls(&self) -> Option<&CapturedRootRecipeControls> {
        self.recipe_controls.as_ref()
    }

    /// Whether every interpreted shell effect used caller-bound controls from
    /// before Source parsing. This does not authenticate the shell executable.
    #[must_use]
    pub const fn shell_effect_policy_bound(&self) -> bool {
        self.shell_effect_policy_bound
    }

    /// Bind final shell values in the same post-Source program and snapshot.
    ///
    /// The caller supplies the original observed program already used for this
    /// plan. No fallback shell/flags are synthesized. A later launcher must
    /// independently authenticate the shell and admit its flags and exports.
    ///
    /// # Errors
    /// Refuses changed roots/cwd, unresolved or effectful expressions and empty
    /// shell selections. Does not authorize an ordinary Source recipe launch.
    pub fn with_captured_recipe_controls(
        mut self,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<Self, String> {
        if paths.working_directory != self.plan.working_directory {
            return Err("Root recipe controls use a different working directory".into());
        }
        let (values, reads) = program.evaluate_many_captured(
            &["$(SHELL)", "$(.SHELLFLAGS)"],
            paths,
            &self.snapshot,
        )?;
        let controls = CapturedRootRecipeControls::from_evaluation(values, reads)?;
        self.snapshot.verify()?;
        self.recipe_controls = Some(controls);
        Ok(self)
    }

    #[must_use]
    pub const fn capture_plan(&self) -> &RootDispatchCapturePlan {
        &self.plan
    }

    #[must_use]
    pub const fn source_reads(&self) -> &VerifiedMakeInputReads {
        &self.source_reads
    }

    #[must_use]
    pub const fn role_reads(&self) -> &VerifiedMakeInputReads {
        &self.role_reads
    }

    #[must_use]
    pub const fn startup(&self) -> &LiteralMakeStartupArguments {
        &self.startup
    }

    /// Recheck every held input tree before using its derived data.
    ///
    /// # Errors
    /// Refuses changed roots or input identity. This does not check a process.
    pub fn revalidate(&self) -> Result<(), String> {
        self.snapshot.verify()
    }

    /// Recheck both retained plans, measuring their shared input domains once.
    /// This neither adopts outputs nor authenticates an execution owner.
    /// # Errors
    /// Refuses conflicting retained roots or changed input identities.
    pub fn revalidate_with(&self, other: &Self) -> Result<(), String> {
        self.snapshot.verify_with(&other.snapshot)
    }
}

impl RootDispatchCapturePlan {
    /// Interpret the complete captured Root and its includes before derivation.
    ///
    /// Dynamic/eval targets are expanded by the generic source reader at their
    /// read positions. A unique prerequisite-free selected pattern must remain,
    /// with no competing explicit/pattern rule, relevant target assignment or
    /// phony goal. Unknown syntax refuses in the reader, not in a line scanner.
    /// Startup must separately bind disabled builtin rules, no unmodeled inputs
    /// or parser overrides, and protection against Makefile remakes. The caller
    /// supplies the original initial-state program, not post-Source query values
    /// as an invented replacement for the initial namespace.
    /// The selected goal must be the sole direct requested goal of the bound
    /// startup, not a dependency inheriting an ancestor's target variables.
    ///
    /// # Errors
    /// Refuses Source drift, unsupported interpretation, competing declarations,
    /// changed role values, scope mismatch and all captured input limits.
    pub fn derive_captured(
        root_input: &str,
        expected_sha256: &Sha256Digest,
        selected: RootDispatchCaptureSelection<'_>,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
        startup: &LiteralMakeStartupArguments,
    ) -> Result<SourceCheckedRootDispatchPlan, String> {
        if startup.goals() != [selected.goal]
            || startup.input_files() != [root_input]
            || !startup.arguments().iter().any(|argument| argument == "-r")
        {
            return Err("Root dispatch requires its sole direct goal, exact Root input and disabled builtin rules".into());
        }
        Self::derive_captured_source(
            root_input,
            expected_sha256,
            selected,
            program,
            paths,
            snapshot,
            startup,
        )
    }

    fn derive_captured_source(
        root_input: &str,
        expected_sha256: &Sha256Digest,
        selected: RootDispatchCaptureSelection<'_>,
        program: &mut MakeVariableProgram,
        paths: &MakeProgramPaths<'_>,
        snapshot: &MakeInputSnapshot,
        startup: &LiteralMakeStartupArguments,
    ) -> Result<SourceCheckedRootDispatchPlan, String> {
        if selected.working_directory != paths.working_directory {
            return Err("Root dispatch source cwd differs from selected invocation".into());
        }
        let (root, _) = snapshot.transaction(|reader| {
            let root = reader
                .read_bytes(root_input)?
                .ok_or("Root dispatch source is absent")?;
            if root.bytes.len() > MAX_ROOT_BYTES || sha256_bytes(&root.bytes) != *expected_sha256 {
                return Err("Root dispatch source digest or size changed".into());
            }
            Ok(root)
        })?;
        // Validate identifiers and literal command data before expression use.
        let plan = Self::derive(&root.bytes, expected_sha256, selected)?;
        let (file, source_reads) = program.read_captured_file(root_input, paths, snapshot)?;
        let selected_recipe = verify_rules(&file, &root.identity, selected)?;
        let (pattern_prefix, pattern_suffix) = selected
            .target_pattern
            .split_once('%')
            .ok_or("Root recorder selected pattern has no stem")?;
        let selected_stem = selected.goal
            [pattern_prefix.len()..selected.goal.len() - pattern_suffix.len()]
            .to_owned();
        let expressions = [
            format!("$({})", selected.prefix_variable),
            format!("$({})", selected.program_variable),
            format!("$({})", selected.options_variable),
        ];
        let (values, role_reads) = program.evaluate_many_captured(
            &expressions.iter().map(String::as_str).collect::<Vec<_>>(),
            paths,
            snapshot,
        )?;
        if values != [selected.original_prefix, selected.program, selected.options] {
            return Err("Root dispatch interpreted role values differ from selected query".into());
        }
        snapshot.verify()?;
        Ok(SourceCheckedRootDispatchPlan {
            plan,
            snapshot: snapshot.clone(),
            source_reads,
            role_reads,
            startup: startup.clone(),
            recipe_controls: None,
            selected_recipe,
            selected_stem,
            shell_effect_policy_bound: program.captured_shell_policy_bound(),
            original_recipe: None,
        })
    }
}

fn relevant(target: &str, selected: RootDispatchCaptureSelection<'_>) -> Result<bool, String> {
    if target.contains(['$', '\\']) || target.matches('%').count() > 1 {
        return Err("Root dispatch has an unsupported expanded target spelling".into());
    }
    Ok(target == selected.goal || matches_pattern(target, selected.goal))
}

fn verify_rules(
    file: &MakeFileProgram,
    root_identity: &str,
    selected: RootDispatchCaptureSelection<'_>,
) -> Result<String, String> {
    let rules = make_interpreted_rules::collect(file)?;
    // -r clears the native initial suffix list, but Source can re-enable it.
    // Account for converted suffix candidates instead of treating .SUFFIXES
    // as inert or assuming every target is extension-free.
    let suffixes = file
        .suffixes(Some(&[]))?
        .ok_or("Root dispatch suffix state is unknown")?;
    let suffix_set = suffixes
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let mut suffix_work = 0usize;
    let mut found = None;
    for rule in &rules.rules {
        let RuleHeader::Rule {
            targets,
            target_pattern,
            prerequisites,
            order_only,
            double_colon,
            inline_recipe,
        } = rule.header()
        else {
            return Err("Root dispatch interpreted rule lost ordinary header".into());
        };
        let mut selected_pattern = false;
        for target in targets.split_whitespace() {
            if target_pattern.is_none()
                && prerequisites.is_empty()
                && order_only.is_empty()
                && (!rule.recipes().is_empty() || inline_recipe.is_some())
            {
                for input_suffix in &suffix_set {
                    suffix_work = suffix_work
                        .checked_add(1)
                        .filter(|work| *work <= 1_048_576)
                        .ok_or("Root dispatch suffix-candidate work limit")?;
                    let Some(output_suffix) = target.strip_prefix(*input_suffix) else {
                        continue;
                    };
                    if output_suffix.is_empty() || suffix_set.contains(output_suffix) {
                        if output_suffix.contains(['%', '$', '\\']) {
                            return Err("Root dispatch has an unsupported suffix candidate".into());
                        }
                        if relevant(&format!("%{output_suffix}"), selected)? {
                            return Err("Root dispatch has a competing Source suffix rule".into());
                        }
                    }
                }
            }
            if !relevant(target, selected)? {
                continue;
            }
            if target != selected.target_pattern || selected_pattern {
                return Err("Root dispatch has a competing interpreted rule".into());
            }
            selected_pattern = true;
        }
        if !selected_pattern {
            continue;
        }
        let event = &file.events[rule.event_index()];
        if found.is_some()
            || event.input != root_identity
            || event.generated
            || target_pattern.is_some()
            || !prerequisites.is_empty()
            || !order_only.is_empty()
            || *double_colon
            || inline_recipe.is_some()
            || rule.recipes().len() != 1
        {
            return Err(
                "Root dispatch interpreted selection is ambiguous or not the original recipe"
                    .into(),
            );
        }
        let MakeFileEventKind::Recipe { text, .. } = &file.events[rule.recipes()[0]].kind else {
            return Err("Root dispatch interpreted recipe is absent".into());
        };
        if text.trim_start_matches(['@', '+'])
            != format!(
                "$({}) $({}) $({}) {}.$@",
                selected.prefix_variable,
                selected.program_variable,
                selected.options_variable,
                selected.project
            )
        {
            return Err("Root dispatch interpreted recipe differs from literal roles".into());
        }
        found = Some(text.clone());
    }
    for event in file.events.iter().filter(|event| event.active) {
        match &event.kind {
            MakeFileEventKind::Rule(RuleHeader::TargetAssignment { targets, .. }) => {
                for target in targets.split_whitespace() {
                    if relevant(target, selected)? {
                        return Err(
                            "Root dispatch has relevant interpreted target configuration".into(),
                        );
                    }
                }
            }
            MakeFileEventKind::PhonyTargets(targets)
                if targets.iter().any(|target| target == selected.goal) =>
            {
                return Err(
                    "Root dispatch selected goal is phony and bypasses implicit search".into(),
                );
            }
            _ => {}
        }
    }
    found.ok_or_else(|| "Root dispatch interpreted pattern is absent".into())
}
