//! Inert derivation of a literal Root command-prefix capture.
//!
//! This does not authorize a Root launch, authenticate a process, or prove a
//! complete Make program. The live caller must bind every original input,
//! executable, private output and deadline. In particular, included Make files
//! and Root recipes remain trusted Source, not an OS-sandboxed language.

use aros_common::{
    make_child_command::LiteralMakeChildCommand,
    make_environment_prefix::LiteralMakeEnvironmentPrefix,
    make_native_invocation::NativeMakeInvocation, sha256_bytes, Sha256Digest,
};
use std::path::{Component, Path, PathBuf};

const MAX_ROOT_BYTES: usize = 8 * 1024 * 1024;
const MAX_COMMAND_BYTES: usize = 128 * 1024;

mod source;
pub use source::{
    CapturedRootRecipeControls, FencedRootDispatchPlan, FencedRootDispatchRequest,
    RootDispatchFencedFiles, SourceCheckedRootDispatchPlan,
};

/// Independently selected roles and observed literal Root values.
/// These strings do not gain provenance by being supplied to this structure.
#[derive(Clone, Copy)]
pub struct RootDispatchCaptureSelection<'a> {
    pub target_pattern: &'a str,
    pub goal: &'a str,
    pub prefix_variable: &'a str,
    pub program_variable: &'a str,
    pub options_variable: &'a str,
    pub project: &'a str,
    pub original_prefix: &'a str,
    pub program: &'a str,
    pub options: &'a str,
    pub observer: &'a Path,
    pub output: &'a Path,
    pub working_directory: &'a Path,
}

/// A source-byte-bound post-parse override, not a file or execution receipt.
///
/// A selected-goal static pattern retains the original stem and leaves all
/// Source variables untouched. Load this only after the verified Source inputs.
pub struct RootDispatchCapturePlan {
    original_sha256: Sha256Digest,
    override_bytes: Vec<u8>,
    selected_roles: Vec<(String, String)>,
    recorder_recipe: String,
    prefix: LiteralMakeEnvironmentPrefix,
    captured_arguments: Vec<String>,
    original_recipe_words: Vec<String>,
    expected_recorder_words: Vec<String>,
    program: String,
    arguments: Vec<String>,
    working_directory: PathBuf,
}

/// Original captured data alongside its explicit literal-prefix derivation.
/// Neither object establishes successful execution of the selected program.
pub struct CapturedRootDispatch {
    raw: NativeMakeInvocation,
    invocation: NativeMakeInvocation,
}

impl CapturedRootDispatch {
    #[must_use]
    pub const fn raw(&self) -> &NativeMakeInvocation {
        &self.raw
    }

    #[must_use]
    pub const fn invocation(&self) -> &NativeMakeInvocation {
        &self.invocation
    }
}

impl RootDispatchCapturePlan {
    /// Bind recorder data and apply only a separately checked Source export
    /// delta, then the original literal prefix. Opaque native keys remain.
    /// This does not reconstruct engine/shell effects or authorize a launch.
    /// # Errors
    /// Refuses mismatching capture/changed exports, missing Source coverage of
    /// exported original roles, or native invocation data limits.
    pub fn bind_capture_with_source_exports(
        &self,
        raw: NativeMakeInvocation,
        original: &crate::make_vars::MakeRecipeProjection,
        instrumented: &crate::make_vars::MakeRecipeProjection,
    ) -> Result<CapturedRootDispatch, String> {
        self.validate_capture(&raw)?;
        for (name, source) in &self.selected_roles {
            if raw.environment().contains_key(name)
                && (instrumented.source_exports().get(name) != raw.environment().get(name)
                    || original.source_exports().get(name) != Some(source))
            {
                return Err("exported Root role lacks its independent Source projections".into());
            }
        }
        let mut environment = original.apply_changed_source_exports(instrumented, &raw)?;
        environment.extend(self.prefix.assignments().clone());
        let invocation = NativeMakeInvocation::new(
            self.arguments.clone(),
            self.working_directory.clone(),
            environment,
        )
        .and_then(|invocation| invocation.with_argument_zero(self.program.clone()))
        .map_err(|_| "Root Source-restored command data exceeds invocation bounds")?;
        Ok(CapturedRootDispatch { raw, invocation })
    }

    /// Bind one prerequisite-free pattern rule with one literal command form.
    /// The returned override must be loaded AFTER the original Root inputs.
    /// It sends the original command words to the no-child capture frontend.
    /// No original command, shell, file access or environment lookup occurs.
    ///
    /// # Errors
    /// Refuses changed bytes, ambiguous/conditional rules, additional recipes,
    /// prerequisites, mismatched roles, nonliteral values or resource excess.
    pub fn derive(
        root_bytes: &[u8],
        expected_sha256: &Sha256Digest,
        selected: RootDispatchCaptureSelection<'_>,
    ) -> Result<Self, String> {
        if root_bytes.len() > MAX_ROOT_BYTES || sha256_bytes(root_bytes) != *expected_sha256 {
            return Err("Root capture source identity or size differs".into());
        }
        for name in [
            selected.prefix_variable,
            selected.program_variable,
            selected.options_variable,
        ] {
            if !identifier(name) {
                return Err("Root capture role is not an identifier".into());
            }
            if name == "SHELL" {
                return Err("Root capture role cannot override the recipe shell".into());
            }
        }
        if selected.prefix_variable == selected.program_variable
            || selected.prefix_variable == selected.options_variable
            || selected.program_variable == selected.options_variable
            || !token(selected.project)
            || !matches_pattern(selected.target_pattern, selected.goal)
        {
            return Err("Root capture roles, project or selected pattern differ".into());
        }
        for path in [
            selected.observer,
            selected.output,
            selected.working_directory,
        ] {
            literal_path(path)?;
        }
        literal_path(Path::new(selected.program))?;
        // Original Root syntax expands these role values without shell quoting
        // around the program reference. A multiword program would change argv.
        if !Path::new(selected.program).is_absolute()
            || selected.program.chars().any(char::is_whitespace)
        {
            return Err("Root capture program is not one absolute literal word".into());
        }
        let program = LiteralMakeChildCommand::parse(selected.program)
            .map_err(|_| "Root capture program is not literal")?;
        if program.program() != selected.program
            || !program.arguments().is_empty()
            || !program.environment_assignments().is_empty()
        {
            return Err("Root capture program has unsupported prefixes or arguments".into());
        }
        let prefix = LiteralMakeEnvironmentPrefix::parse(selected.original_prefix)
            .map_err(|_| "Root capture environment prefix is not literal")?;
        let options =
            LiteralMakeChildCommand::parse(&format!("capture-options {}", selected.options))
                .map_err(|_| "Root capture options are not literal")?;
        if options.program() != "capture-options" || !options.environment_assignments().is_empty() {
            return Err("Root capture options changed their literal context".into());
        }
        let source = aros_common::decode_source_bytes(root_bytes.to_vec());
        let joined = crate::parser::join_continuations(&source);
        let (_, states) = crate::make_vars::collect_vars_impl(&joined, None);
        let suppressed = crate::make_vars::make_define_suppression(&joined);
        let lines: Vec<_> = joined.lines().collect();
        let expected_recipe = format!(
            "$({}) $({}) $({}) {}.$@",
            selected.prefix_variable,
            selected.program_variable,
            selected.options_variable,
            selected.project
        );
        let mut matched = false;
        let mut recipe_prefixes = String::new();
        for (index, line) in lines.iter().enumerate() {
            if suppressed.get(index).copied().unwrap_or(true) || line.starts_with('\t') {
                continue;
            }
            if line.contains(".RECIPEPREFIX") || line.trim() == ".ONESHELL:" {
                return Err("Root capture has unsupported recipe control".into());
            }
            let parsed = crate::make_rule_header::parse(line);
            if let Ok(Some(crate::make_rule_header::RuleHeader::TargetAssignment {
                targets, ..
            })) = &parsed
            {
                if targets.contains('$')
                    || targets.split_whitespace().any(|target| {
                        target == selected.goal
                            || target == selected.target_pattern
                            || matches_pattern(target, selected.goal)
                    })
                {
                    return Err("Root capture has target-specific configuration outside its observed values".into());
                }
            }
            let Ok(Some(crate::make_rule_header::RuleHeader::Rule {
                targets,
                target_pattern,
                prerequisites,
                order_only,
                double_colon,
                inline_recipe,
            })) = parsed
            else {
                continue;
            };
            if !targets
                .split_whitespace()
                .any(|target| target == selected.target_pattern)
                && targets
                    .split_whitespace()
                    .any(|target| target == selected.goal || matches_pattern(target, selected.goal))
            {
                return Err("Root capture has a competing explicit or pattern rule".into());
            }
            if !targets
                .split_whitespace()
                .any(|target| target == selected.target_pattern)
            {
                continue;
            }
            if matched
                || states.get(index) != Some(&crate::make_vars::ConditionalTruth::True)
                || target_pattern.is_some()
                || !prerequisites.is_empty()
                || !order_only.is_empty()
                || double_colon
                || inline_recipe.is_some()
            {
                return Err(
                    "Root capture rule is conditional, ambiguous or has prerequisites".into(),
                );
            }
            let mut recipes = Vec::new();
            for following in &lines[index + 1..] {
                if let Some(recipe) = following.strip_prefix('\t') {
                    recipes.push(recipe);
                } else if !following.trim().is_empty() && !following.trim_start().starts_with('#') {
                    break;
                }
            }
            if recipes.len() != 1 {
                return Err("Root capture requires exactly one dispatch recipe".into());
            }
            let recipe = recipes[0].trim_start_matches(['@', '+']);
            recipes[0][..recipes[0].len() - recipe.len()].clone_into(&mut recipe_prefixes);
            if recipe != expected_recipe {
                return Err(
                    "Root capture recipe differs from selected literal dispatch roles".into(),
                );
            }
            matched = true;
        }
        if !matched {
            return Err("Root capture pattern rule is absent".into());
        }
        let mut captured_arguments = vec!["env".to_owned()];
        captured_arguments.extend(
            prefix
                .assignments()
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        captured_arguments.push(selected.program.to_owned());
        captured_arguments.extend_from_slice(options.arguments());
        captured_arguments.push(format!("{}.{}", selected.project, selected.goal));
        let original_prefix =
            crate::make_shell_data::parse_literal_recipe(selected.original_prefix)?;
        let [crate::make_shell_data::LiteralRecipeStatement::Command(original_prefix)] =
            original_prefix.as_slice()
        else {
            return Err("Root original prefix is not one literal command".into());
        };
        let mut original_recipe_words = original_prefix.words().to_vec();
        original_recipe_words
            .extend_from_slice(&captured_arguments[1 + prefix.assignments().len()..]);
        let mut expected_recorder_words = vec![
            path_text(selected.observer)?.to_owned(),
            "--capture-make-invocation".into(),
            path_text(selected.output)?.to_owned(),
            "--".into(),
        ];
        expected_recorder_words.extend_from_slice(&captured_arguments);
        let recorder_recipe = format!(
            "{recipe_prefixes}{}",
            expected_recorder_words
                .iter()
                .map(|word| quote_assignment_word(word))
                .collect::<Result<Vec<_>, _>>()?
                .join(" ")
        );
        // Unlike a plain explicit rule, a static pattern retains $*. Unlike
        // appending another implicit pattern, it selects this exact goal.
        // The goal/pattern match and prerequisite-free original frame were
        // proved above; GNU Make merely warns on a mismatched static pattern.
        let override_bytes = format!(
            "{}: {}:\n\t{recorder_recipe}\n",
            selected.goal, selected.target_pattern
        )
        .into_bytes();
        if override_bytes.len() > MAX_COMMAND_BYTES {
            return Err("Root literal recorder override exceeds command budget".into());
        }
        let selected_roles = vec![
            (
                selected.prefix_variable.to_owned(),
                selected.original_prefix.to_owned(),
            ),
            (
                selected.program_variable.to_owned(),
                selected.program.to_owned(),
            ),
            (
                selected.options_variable.to_owned(),
                selected.options.to_owned(),
            ),
        ];
        let mut arguments = options.arguments().to_vec();
        arguments.push(format!("{}.{}", selected.project, selected.goal));
        // Apply the format's original aggregate argv/environment limits now,
        // before any caller can materialize this inert derivation.
        NativeMakeInvocation::new(
            captured_arguments.clone(),
            selected.working_directory.to_owned(),
            [],
        )
        .map_err(|_| "Root capture arguments exceed invocation bounds")?;
        Ok(Self {
            original_sha256: expected_sha256.clone(),
            override_bytes,
            selected_roles,
            recorder_recipe,
            prefix,
            captured_arguments,
            original_recipe_words,
            expected_recorder_words,
            program: selected.program.into(),
            arguments,
            working_directory: selected.working_directory.to_owned(),
        })
    }

    #[must_use]
    pub fn override_bytes(&self) -> &[u8] {
        &self.override_bytes
    }

    #[must_use]
    pub const fn original_sha256(&self) -> &Sha256Digest {
        &self.original_sha256
    }

    /// Match the supervised capture's exact argv and cwd, then apply the
    /// independently selected literal env prefix to the ENTIRE captured env.
    /// No Source role variable was overwritten. Unknown variables are retained,
    /// not guessed or filtered; deferred input-list effects need the separate
    /// Source-export-aware binding.
    /// Instrumentation can affect other process data (for example `_`); this
    /// does not establish observer-free Root environment equivalence.
    ///
    /// # Errors
    /// Refuses changed argv/cwd, a present role differing from its selected
    /// original value, or invocation limits. Use Source-aware binding when an
    /// export depends on generated input-list metadata.
    pub fn bind_capture(&self, raw: NativeMakeInvocation) -> Result<CapturedRootDispatch, String> {
        self.validate_capture(&raw)?;
        for (name, expected) in &self.selected_roles {
            if raw
                .environment()
                .get(name)
                .is_some_and(|value| value != expected)
            {
                return Err("Root role export differs from selected original data".into());
            }
        }
        let mut environment = raw.environment().clone();
        environment.extend(self.prefix.assignments().clone());
        let invocation = NativeMakeInvocation::new(
            self.arguments.clone(),
            self.working_directory.clone(),
            environment,
        )
        .and_then(|invocation| invocation.with_argument_zero(self.program.clone()))
        .map_err(|_| "Root effective command data exceeds invocation bounds")?;
        Ok(CapturedRootDispatch { raw, invocation })
    }

    fn validate_capture(&self, raw: &NativeMakeInvocation) -> Result<(), String> {
        if raw.argument_zero().is_some()
            || raw.arguments() != self.captured_arguments
            || raw.working_directory() != self.working_directory
        {
            return Err("Root command capture differs from selected argv or cwd".into());
        }
        Ok(())
    }
}

fn identifier(value: &str) -> bool {
    value.len() <= 128
        && value
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'))
}

fn matches_pattern(pattern: &str, goal: &str) -> bool {
    let Some((before, after)) = pattern.split_once('%') else {
        return false;
    };
    !after.contains('%')
        && token(goal)
        && goal.len() > before.len() + after.len()
        && goal.starts_with(before)
        && goal.ends_with(after)
        && before
            .bytes()
            .chain(after.bytes())
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+'))
}

fn path_text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "Root capture path is not UTF-8".into())
}

fn literal_path(path: &Path) -> Result<(), String> {
    let text = path_text(path)?;
    if !path.is_absolute()
        || text.len() > 8192
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err("Root capture path is not normalized absolute data".into());
    }
    quote_assignment_word(text).map(|_| ())
}

fn quote_assignment_word(value: &str) -> Result<String, String> {
    // Closed literal Make recipe grammar. Shell quoting alone does not protect
    // Make's own dollar expansion; keep the existing bounded spelling policy.
    if value.contains(['$', '#', '\\']) || value.chars().any(char::is_control) {
        return Err("Root capture value requires unsupported Make recipe escaping".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
