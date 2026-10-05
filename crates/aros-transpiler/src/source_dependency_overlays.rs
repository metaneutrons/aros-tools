//! Projection-only, source-local evidence from ordinary Make dependency rules.
//!
//! This module deliberately does not prove a producer, source owner, or CMake
//! emission. A later binder must connect every `output` to a complete compile
//! producer before using any projection. It also does not execute GenMF or GNU
//! includes; projections accompanied by those source effects are inspectable
//! candidates, not complete Make semantics or admission evidence.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_list, MakeExprContext};
use crate::make_vars::{
    directive_tail, strip_make_comment, variable_assignment, ConditionalTruth, VarScope,
};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Component, Path};

const SOURCE_ALIAS: &str = "${AROS_SOURCE_DIR}";
const BUILD_ALIAS: &str = "${AROS_BUILD_DIR}";
const GENERATED_ALIAS: &str = "${AROS_BUILD_DIR}/gen";
const MAX_SNAPSHOT_BYTES: usize = 1_048_576;
const MAX_LINES: usize = 16_384;
const MAX_OUTPUT_IDENTITIES: usize = 4_096;
const MAX_PREREQUISITE_REFERENCES: usize = 8_192;
const MAX_IDENTITY_BYTES: usize = 16 * 1024 * 1024;

/// One ordinary-rule projection of source-local Make dependency metadata.
///
/// Outputs remain candidate identities until a later binding pass proves their
/// selected compile producer and all prerequisite producers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDependencyOverlay {
    /// Relative joined mmakefile path that declared the rule.
    pub file: String,
    /// One-based source line containing the dependency-only rule.
    pub line: usize,
    /// Canonical generated `.o` or `.d` output identities.
    pub outputs: Vec<String>,
    /// File prerequisites that retain timestamp-based dependency semantics.
    pub normal_prerequisites: Vec<String>,
    /// Target prerequisites that constrain order without timestamp semantics.
    pub order_only_prerequisites: Vec<String>,
}

/// A relevant source dependency rule that could not be represented safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDependencyOverlayRejection {
    /// Relative joined mmakefile path, or `<unknown>` if unavailable.
    pub file: String,
    /// One-based source line of the rejected rule or control.
    pub line: usize,
    /// Why the rule could not be projected without guessing.
    pub reason: String,
}

#[derive(Debug, Clone)]
struct LogicalLine {
    text: String,
    start_line: usize,
    state: ConditionalTruth,
}

#[derive(Debug, Clone)]
struct RawRule {
    target: String,
    prerequisites: String,
    line: usize,
    state: ConditionalTruth,
    recipe_form: RecipeForm,
    colon_form: ColonForm,
    target_specific: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColonForm {
    Single,
    Double,
    StaticPattern,
    DoubleStaticPattern,
}

impl ColonForm {
    const fn from_parts(double_colon: bool, extra_colon: bool) -> Self {
        match (double_colon, extra_colon) {
            (false, false) => Self::Single,
            (true, false) => Self::Double,
            (false, true) => Self::StaticPattern,
            (true, true) => Self::DoubleStaticPattern,
        }
    }

    const fn is_double(self) -> bool {
        matches!(self, Self::Double | Self::DoubleStaticPattern)
    }

    const fn is_static_pattern(self) -> bool {
        matches!(self, Self::StaticPattern | Self::DoubleStaticPattern)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecipeForm {
    None,
    Inline,
    Following,
    InlineAndFollowing,
}

impl RecipeForm {
    const fn from_inline(has_inline: bool) -> Self {
        if has_inline {
            Self::Inline
        } else {
            Self::None
        }
    }

    const fn with_following(self) -> Self {
        match self {
            Self::None | Self::Following => Self::Following,
            Self::Inline | Self::InlineAndFollowing => Self::InlineAndFollowing,
        }
    }

    const fn has_inline(self) -> bool {
        matches!(self, Self::Inline | Self::InlineAndFollowing)
    }

    const fn has_following(self) -> bool {
        matches!(self, Self::Following | Self::InlineAndFollowing)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DefinitionEnd {
    Make,
    GenMf,
}

#[derive(Default)]
struct OverlayAccumulator {
    overlays: Vec<SourceDependencyOverlay>,
    rejections: Vec<SourceDependencyOverlayRejection>,
    output_identities: HashSet<String>,
    duplicate_declarations: HashSet<(Vec<String>, Vec<String>, Vec<String>)>,
    retained_references: usize,
    retained_identity_bytes: usize,
}

impl OverlayAccumulator {
    fn retain(&mut self, overlay: SourceDependencyOverlay) -> Result<(), &'static str> {
        let mut sorted_outputs = overlay.outputs.clone();
        sorted_outputs.sort();
        let declaration_key = (
            sorted_outputs,
            overlay.normal_prerequisites.clone(),
            overlay.order_only_prerequisites.clone(),
        );
        if self.duplicate_declarations.try_reserve(1).is_err() {
            return Err("allocation for retained declarations failed");
        }
        if !self.duplicate_declarations.insert(declaration_key) {
            self.rejections.push(rejection(
                &overlay.file,
                overlay.line,
                "duplicate identical dependency declaration",
            ));
            return Ok(());
        }

        let new_output_identities = overlay
            .outputs
            .iter()
            .filter(|target| !self.output_identities.contains(*target))
            .count();
        self.output_identities
            .len()
            .checked_add(new_output_identities)
            .filter(|count| *count <= MAX_OUTPUT_IDENTITIES)
            .ok_or("4096 output identities per file")?;
        let new_reference_count = overlay
            .normal_prerequisites
            .len()
            .checked_add(overlay.order_only_prerequisites.len())
            .ok_or("8192 prerequisite references per file")?;
        let next_reference_count = self
            .retained_references
            .checked_add(new_reference_count)
            .filter(|count| *count <= MAX_PREREQUISITE_REFERENCES)
            .ok_or("8192 prerequisite references per file")?;
        let declaration_bytes = overlay
            .outputs
            .iter()
            .chain(overlay.normal_prerequisites.iter())
            .chain(overlay.order_only_prerequisites.iter())
            .try_fold(0usize, |total, identity| total.checked_add(identity.len()));
        let next_identity_bytes = declaration_bytes
            .and_then(|count| self.retained_identity_bytes.checked_add(count))
            .filter(|count| *count <= MAX_IDENTITY_BYTES)
            .ok_or("16 MiB retained identity bytes per file")?;
        if self
            .output_identities
            .try_reserve(new_output_identities)
            .is_err()
            || self.overlays.try_reserve(1).is_err()
        {
            return Err("allocation for retained identities failed");
        }

        self.retained_references = next_reference_count;
        self.retained_identity_bytes = next_identity_bytes;
        self.output_identities
            .extend(overlay.outputs.iter().cloned());
        self.overlays.push(overlay);
        Ok(())
    }
}

struct OverlaySourceContext<'a> {
    file: &'a str,
    scope: &'a VarScope,
    dirs: &'a DirVars,
    source_root: &'a Path,
    relative_dir: &'a Path,
}

impl OverlaySourceContext<'_> {
    const fn make_context(&self, one_based_line: usize) -> MakeExprContext<'_> {
        MakeExprContext::new(
            self.scope,
            self.dirs,
            one_based_line - 1,
            self.source_root,
            self.relative_dir,
        )
    }
}

/// Collects dependency-only Make rule candidates for one source snapshot.
///
/// It never proves producers or owner reachability. Callers must preserve every
/// rejection and bind all projected outputs and prerequisites before use.
/// Supplied per-physical-line states use `Some(true)` for active,
/// `Some(false)` for inactive, and `None` for unproved conditions.
#[must_use]
pub fn collect(
    content: &str,
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
    line_states: Option<&[Option<bool>]>,
) -> (
    Vec<SourceDependencyOverlay>,
    Vec<SourceDependencyOverlayRejection>,
) {
    let file = match source_file(relative_dir) {
        Ok(file) => file,
        Err(reason) => {
            return (
                Vec::new(),
                vec![rejection(
                    "<unknown>",
                    1,
                    format!("source directory: {reason}"),
                )],
            );
        }
    };
    if content.len() > MAX_SNAPSHOT_BYTES {
        return (
            Vec::new(),
            vec![rejection(&file, 1, "joined source snapshot exceeds 1 MiB")],
        );
    }
    if content.lines().count() > MAX_LINES {
        return (
            Vec::new(),
            vec![rejection(&file, 1, "source snapshot exceeds 16384 lines")],
        );
    }

    let logical = logical_lines(content, line_states);
    let mut rejections = genmf_effect_rejections(&logical, &file);
    let mut mutation_rejections = make_eval_rejections(&logical, &file);
    let (active, control_rejections) =
        mask_defines_and_validate_controls(&logical, &file, line_states.is_some());
    if !control_rejections.is_empty() {
        rejections.append(&mut mutation_rejections);
        rejections.extend(control_rejections);
        return (Vec::new(), rejections);
    }
    mutation_rejections.extend(parser_mutation_rejections(&active, &file));
    if !mutation_rejections.is_empty() {
        rejections.append(&mut mutation_rejections);
        return (Vec::new(), rejections);
    }
    rejections.extend(unexpanded_include_rejections(&active, &file));

    let source_context = OverlaySourceContext {
        file: &file,
        scope,
        dirs,
        source_root,
        relative_dir,
    };
    match collect_rule_overlays(&active, &source_context) {
        Ok((overlays, mut rule_rejections)) => {
            rejections.append(&mut rule_rejections);
            (overlays, rejections)
        }
        Err((line, resource)) => budget_rejection(&file, line, resource),
    }
}

type RuleOverlayCollection = (
    Vec<SourceDependencyOverlay>,
    Vec<SourceDependencyOverlayRejection>,
);
type RuleOverlayCollectionResult = Result<RuleOverlayCollection, (usize, &'static str)>;

fn collect_rule_overlays(
    lines: &[LogicalLine],
    context: &OverlaySourceContext<'_>,
) -> RuleOverlayCollectionResult {
    let mut accumulator = OverlayAccumulator::default();
    for rule in parse_rules(lines) {
        match project_rule(&rule, context) {
            Ok(Some(overlay)) => {
                let line = overlay.line;
                if let Err(resource) = accumulator.retain(overlay) {
                    return Err((line, resource));
                }
            }
            Ok(None) => {}
            Err(reason) => accumulator
                .rejections
                .push(rejection(context.file, rule.line, reason)),
        }
    }
    Ok((accumulator.overlays, accumulator.rejections))
}

fn project_rule(
    rule: &RawRule,
    context: &OverlaySourceContext<'_>,
) -> Result<Option<SourceDependencyOverlay>, String> {
    if rule.state == ConditionalTruth::False {
        return Ok(None);
    }
    let Some(targets) = evaluate_targets(rule, context)? else {
        return Ok(None);
    };
    if rule.state == ConditionalTruth::Unknown {
        return Err("ordinary object rule is in an unknown conditional branch".into());
    }
    if rule.recipe_form.has_following() {
        return Ok(None);
    }
    validate_rule_shape(rule, &targets)?;

    let make_context = context.make_context(rule.line);
    let (normal_prerequisites, order_only_prerequisites) =
        evaluate_prerequisites(rule, &make_context)?;
    Ok(Some(SourceDependencyOverlay {
        file: context.file.to_owned(),
        line: rule.line,
        outputs: targets,
        normal_prerequisites,
        order_only_prerequisites,
    }))
}

fn evaluate_targets(
    rule: &RawRule,
    context: &OverlaySourceContext<'_>,
) -> Result<Option<Vec<String>>, String> {
    let relevant = looks_like_object_identity(&rule.target);
    if contains_glob_syntax(&rule.target) {
        return if relevant {
            Err("target expression contains a glob or path-control character".into())
        } else {
            Ok(None)
        };
    }
    let make_context = context.make_context(rule.line);
    let targets = evaluate_make_list(&rule.target, &make_context)
        .map_err(|error| format!("cannot resolve ordinary-rule targets: {error}"))?;
    if targets.is_empty() {
        return if relevant {
            Err("target expression resolved to no outputs".into())
        } else {
            Ok(None)
        };
    }

    let object_flags = targets
        .iter()
        .map(|target| object_extension(target).is_some())
        .collect::<Vec<_>>();
    if !object_flags.iter().any(|is_object| *is_object) {
        return Ok(None);
    }
    if object_flags.iter().any(|is_object| !is_object) {
        return Err("ordinary rule mixes object/dependency outputs with non-object targets".into());
    }
    Ok(Some(targets))
}

fn validate_rule_shape(rule: &RawRule, targets: &[String]) -> Result<(), String> {
    if rule.colon_form.is_double() {
        return Err("double-colon rules are unsupported".into());
    }
    if rule.target_specific {
        return Err("target-specific assignment makes rule scope ambiguous".into());
    }
    if rule.colon_form.is_static_pattern() {
        return Err("static-pattern rules are unsupported".into());
    }
    if rule.recipe_form.has_inline() {
        return Err("dependency rule has an inline or tab recipe".into());
    }
    if rule.target.contains("$$") || rule.prerequisites.contains("$$") {
        return Err("escaped `$$` may be rebound by .SECONDEXPANSION and is unsupported".into());
    }
    if targets.iter().any(|target| has_non_root_dollar(target)) {
        return Err(
            "target retains a late or escaped Make reference after parse-time expansion".into(),
        );
    }

    let mut unique_outputs = HashSet::new();
    for target in targets {
        if !unique_outputs.insert(target) {
            return Err("rule repeats an output identity".into());
        }
        if !is_generated_object_path(target) {
            return Err(format!(
                "output is not a canonical {GENERATED_ALIAS} .o/.d path"
            ));
        }
    }
    Ok(())
}

fn evaluate_prerequisites(
    rule: &RawRule,
    context: &MakeExprContext<'_>,
) -> Result<(Vec<String>, Vec<String>), String> {
    if contains_glob_syntax(&rule.prerequisites) {
        return Err("prerequisite expression contains a glob or path-control character".into());
    }
    let (normal_expression, order_only_expression) =
        split_order_only(&rule.prerequisites).map_err(str::to_owned)?;
    let normal_prerequisites = evaluate_make_list(normal_expression, context)
        .map_err(|error| format!("cannot resolve normal prerequisites: {error}"))?;
    let order_only_prerequisites = evaluate_make_list(order_only_expression, context)
        .map_err(|error| format!("cannot resolve order-only prerequisites: {error}"))?;
    if normal_prerequisites
        .iter()
        .chain(order_only_prerequisites.iter())
        .any(|path| has_non_root_dollar(path))
    {
        return Err(
            "prerequisite retains a late or escaped Make reference after parse-time expansion"
                .into(),
        );
    }
    if normal_prerequisites.is_empty() && order_only_prerequisites.is_empty() {
        return Err("dependency-only rule has no prerequisites".into());
    }
    if let Some(invalid) = normal_prerequisites
        .iter()
        .find(|path| !is_normal_prerequisite(path))
    {
        return Err(format!(
            "normal prerequisite is not a canonical source/build path or safe endpoint: `{invalid}`"
        ));
    }
    if let Some(invalid) = order_only_prerequisites
        .iter()
        .find(|path| !is_order_only_prerequisite(path))
    {
        return Err(format!(
            "order-only prerequisite is not a canonical build path or safe endpoint: `{invalid}`"
        ));
    }
    Ok((normal_prerequisites, order_only_prerequisites))
}

fn logical_lines(content: &str, states: Option<&[Option<bool>]>) -> Vec<LogicalLine> {
    let physical = content.lines().collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut index = 0usize;
    while index < physical.len() {
        let start_line = index;
        let mut text = physical[index].to_owned();
        let mut state = line_state(states, index);
        while text.trim_end().ends_with('\\') && index + 1 < physical.len() {
            let trimmed = text.trim_end();
            text.truncate(trimmed.len() - 1);
            index += 1;
            state = combine_state(state, line_state(states, index));
            text.push(' ');
            text.push_str(physical[index].trim());
        }
        lines.push(LogicalLine {
            text,
            start_line,
            state,
        });
        index += 1;
    }
    lines
}

fn genmf_effect_rejections(
    lines: &[LogicalLine],
    file: &str,
) -> Vec<SourceDependencyOverlayRejection> {
    let mut rejections = Vec::new();
    for line in lines {
        let tokens = genmf_tokens(&line.text);
        if !tokens.is_empty() {
            rejections.push(rejection(
                file,
                line.start_line + 1,
                format!(
                    "unexpanded GenMF token(s) {} may alter Make rules or prerequisites",
                    tokens.join(", ")
                ),
            ));
        }
    }
    rejections
}

fn genmf_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] != b'%'
            || !(bytes[index + 1].is_ascii_alphabetic() || bytes[index + 1] == b'_')
        {
            index += 1;
            continue;
        }
        let start = index;
        index += 2;
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
        {
            index += 1;
        }
        tokens.push(text[start..index].to_owned());
    }
    tokens
}

fn make_eval_rejections(
    lines: &[LogicalLine],
    file: &str,
) -> Vec<SourceDependencyOverlayRejection> {
    lines
        .iter()
        .filter(|line| line.state != ConditionalTruth::False)
        .filter(|line| contains_make_eval(&line.text))
        .map(|line| {
            rejection(
                file,
                line.start_line + 1,
                "Make eval may synthesize dependency rules or alter parser state",
            )
        })
        .collect()
}

fn contains_make_eval(line: &str) -> bool {
    let text = if line.starts_with('\t') {
        line
    } else {
        strip_make_comment(line)
    };
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        if bytes[index] == b'$' && matches!(bytes[index + 1], b'(' | b'{') {
            let mut function = index + 2;
            while function < bytes.len() && bytes[function].is_ascii_whitespace() {
                function += 1;
            }
            if bytes.get(function..function + 4) == Some(b"eval")
                && bytes
                    .get(function + 4)
                    .is_some_and(|next| next.is_ascii_whitespace() || *next == b',')
            {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn parser_mutation_rejections(
    lines: &[LogicalLine],
    file: &str,
) -> Vec<SourceDependencyOverlayRejection> {
    let mut rejections = Vec::new();
    for line in lines {
        if line.state == ConditionalTruth::False || line.text.starts_with('\t') {
            continue;
        }
        let statement = strip_make_comment(&line.text).trim();
        if statement.is_empty() || statement.starts_with('#') || contains_make_eval(statement) {
            continue;
        }
        if let Some(reason) = recipe_prefix_mutation_reason(statement) {
            rejections.push(rejection(file, line.start_line + 1, reason));
        }
        if is_unresolved_expansion_statement(statement) {
            rejections.push(rejection(
                file,
                line.start_line + 1,
                "standalone Make expansion may synthesize rules or mutate parser state",
            ));
        }
    }
    rejections
}

fn recipe_prefix_mutation_reason(statement: &str) -> Option<&'static str> {
    if undefines_recipe_prefix(statement) {
        return Some("source undefines .RECIPEPREFIX; recipe boundaries are unprovable");
    }
    let Some(lhs) = assignment_lhs(statement) else {
        return None;
    };
    let name = strip_assignment_modifiers(lhs);
    if name == ".RECIPEPREFIX" {
        Some("source changes .RECIPEPREFIX; recipe boundaries are unprovable")
    } else if contains_make_expansion(name) {
        Some("dynamic assignment name may mutate .RECIPEPREFIX; recipe boundaries are unprovable")
    } else {
        None
    }
}

fn assignment_lhs(statement: &str) -> Option<&str> {
    let equals = top_level_positions(statement, '=').first().copied()?;
    if first_rule_colon(statement).is_some_and(|(colon, _)| colon > equals) {
        return None;
    }
    let prefix = &statement[..equals];
    let bytes = prefix.as_bytes();
    let operator_start = match bytes.last().copied() {
        Some(b':') if bytes.get(bytes.len().checked_sub(2)?) == Some(&b':') => bytes.len() - 2,
        Some(b':' | b'+' | b'?') => bytes.len() - 1,
        _ => bytes.len(),
    };
    if top_level_positions(statement, ':')
        .first()
        .is_some_and(|colon| *colon < operator_start)
    {
        return None;
    }
    Some(prefix[..operator_start].trim())
}

fn strip_assignment_modifiers(mut lhs: &str) -> &str {
    loop {
        let Some((modifier, remainder)) = lhs.split_once(char::is_whitespace) else {
            return lhs.trim();
        };
        if matches!(modifier, "override" | "export" | "unexport" | "private") {
            lhs = remainder.trim_start();
        } else {
            return lhs.trim();
        }
    }
}

fn undefines_recipe_prefix(statement: &str) -> bool {
    let directive = strip_assignment_modifiers(statement);
    let Some(name) = directive_tail(directive, "undefine") else {
        return false;
    };
    let name = name.trim();
    name == ".RECIPEPREFIX" || contains_make_expansion(name)
}

fn contains_make_expansion(text: &str) -> bool {
    text.as_bytes()
        .windows(2)
        .any(|pair| pair[0] == b'$' && matches!(pair[1], b'(' | b'{'))
}

fn is_unresolved_expansion_statement(statement: &str) -> bool {
    starts_with_make_expansion(statement)
        && variable_assignment(statement).is_none()
        && first_rule_colon(statement).is_none()
        && !is_make_control(statement)
}

fn starts_with_make_expansion(statement: &str) -> bool {
    let bytes = statement.as_bytes();
    let mut index = 0usize;
    while bytes.get(index) == Some(&b'$') {
        index += 1;
    }
    index != 0 && matches!(bytes.get(index), Some(b'(' | b'{'))
}

fn unexpanded_include_rejections(
    lines: &[LogicalLine],
    file: &str,
) -> Vec<SourceDependencyOverlayRejection> {
    lines
        .iter()
        .filter(|line| line.state != ConditionalTruth::False)
        .filter_map(|line| {
            let statement = strip_make_comment(&line.text).trim().to_owned();
            let directive = statement
                .split_whitespace()
                .next()
                .unwrap_or_default();
            matches!(directive, "include" | "-include" | "sinclude").then(|| {
                rejection(
                    file,
                    line.start_line + 1,
                    "GNU Make include remains unexpanded; its variables, rules, and special targets are outside this projection",
                )
            })
        })
        .collect()
}

#[derive(Default)]
struct ControlScanner {
    active: Vec<LogicalLine>,
    rejections: Vec<SourceDependencyOverlayRejection>,
    definitions: Vec<DefinitionEnd>,
    define_open_line: usize,
    conditional_stack: Vec<bool>,
}

impl ControlScanner {
    fn process_line(&mut self, line: &LogicalLine, file: &str, has_external_states: bool) {
        let raw_statement = line.text.trim();
        let statement = strip_make_comment(raw_statement).trim();
        if self.process_definition(raw_statement, statement, line, file) {
            return;
        }
        if self.process_conditional(statement, line, file) {
            return;
        }
        if !statement.is_empty() && !statement.starts_with('#') {
            let mut active_line = line.clone();
            if !has_external_states
                && line.state == ConditionalTruth::True
                && !self.conditional_stack.is_empty()
            {
                // Without externally selected states, do not assume that
                // this conditional branch is active.
                active_line.state = ConditionalTruth::Unknown;
            }
            self.active.push(active_line);
        }
    }

    fn process_definition(
        &mut self,
        raw_statement: &str,
        statement: &str,
        line: &LogicalLine,
        file: &str,
    ) -> bool {
        let genmf_open = genmf_define_tail(raw_statement);
        let genmf_close = raw_statement == "%end";
        let open_kind = if genmf_open.is_some() {
            Some(DefinitionEnd::GenMf)
        } else if is_define_open(statement) {
            Some(DefinitionEnd::Make)
        } else {
            None
        };
        let close_kind = if genmf_close {
            Some(DefinitionEnd::GenMf)
        } else if is_define_close(statement) {
            Some(DefinitionEnd::Make)
        } else {
            None
        };
        if !self.definitions.is_empty() || open_kind.is_some() || close_kind.is_some() {
            if let Some(kind) = open_kind {
                if self.definitions.is_empty()
                    && kind == DefinitionEnd::Make
                    && line.state != ConditionalTruth::False
                {
                    if let Some(reason) = recipe_prefix_definition_reason(statement) {
                        self.rejections
                            .push(rejection(file, line.start_line + 1, reason));
                    }
                }
                let valid_name = match kind {
                    DefinitionEnd::Make => define_name(statement).is_some(),
                    DefinitionEnd::GenMf => genmf_open.is_some_and(|name| !name.is_empty()),
                };
                if !valid_name {
                    self.rejections.push(rejection(
                        file,
                        line.start_line + 1,
                        "definition directive has no variable or macro name",
                    ));
                }
                if self.definitions.len() >= MAX_LINES {
                    self.rejections.push(rejection(
                        file,
                        line.start_line + 1,
                        "definition nesting exceeds source line limit",
                    ));
                    return true;
                }
                self.definitions.push(kind);
                self.define_open_line = line.start_line + 1;
            } else if let Some(kind) = close_kind {
                if self.definitions.last() == Some(&kind) {
                    self.definitions.pop();
                } else {
                    self.rejections.push(rejection(
                        file,
                        line.start_line + 1,
                        "definition close does not match the active define form",
                    ));
                }
            } else if starts_control_word(statement, "endef")
                || starts_control_word(raw_statement, "%end")
            {
                self.rejections.push(rejection(
                    file,
                    line.start_line + 1,
                    "malformed definition close directive",
                ));
            }
            return true;
        }
        if starts_control_word(statement, "endef") || starts_control_word(raw_statement, "%end") {
            self.rejections.push(rejection(
                file,
                line.start_line + 1,
                "unmatched or malformed definition close directive",
            ));
            return true;
        }
        false
    }

    fn process_conditional(&mut self, statement: &str, line: &LogicalLine, file: &str) -> bool {
        if let Some(open) = conditional_open(statement) {
            if open.trim().is_empty() {
                self.rejections.push(rejection(
                    file,
                    line.start_line + 1,
                    "conditional directive has no condition",
                ));
            }
            self.conditional_stack.push(false);
            return true;
        }
        if is_else(statement) {
            let Some(saw_else) = self.conditional_stack.last_mut() else {
                self.rejections.push(rejection(
                    file,
                    line.start_line + 1,
                    "else without matching conditional",
                ));
                return true;
            };
            if *saw_else {
                self.rejections.push(rejection(
                    file,
                    line.start_line + 1,
                    "duplicate else in conditional",
                ));
            } else {
                *saw_else = true;
            }
            return true;
        }
        if starts_control_word(statement, "else") {
            self.rejections.push(rejection(
                file,
                line.start_line + 1,
                "malformed else directive",
            ));
            return true;
        }
        if is_endif(statement) {
            if self.conditional_stack.pop().is_none() {
                self.rejections.push(rejection(
                    file,
                    line.start_line + 1,
                    "endif without matching conditional",
                ));
            }
            return true;
        }
        if starts_control_word(statement, "endif") {
            self.rejections.push(rejection(
                file,
                line.start_line + 1,
                "malformed endif directive",
            ));
            return true;
        }
        false
    }

    fn finish(&mut self, file: &str, last_line: usize) {
        if !self.definitions.is_empty() {
            self.rejections.push(rejection(
                file,
                self.define_open_line,
                "unterminated define or GenMF template body",
            ));
        }
        if !self.conditional_stack.is_empty() {
            self.rejections
                .push(rejection(file, last_line, "unterminated conditional"));
        }
    }
}

fn mask_defines_and_validate_controls(
    lines: &[LogicalLine],
    file: &str,
    has_external_states: bool,
) -> (Vec<LogicalLine>, Vec<SourceDependencyOverlayRejection>) {
    let mut scanner = ControlScanner::default();
    for line in lines {
        scanner.process_line(line, file, has_external_states);
    }
    let last_line = lines.last().map_or(1, |line| line.start_line + 1);
    scanner.finish(file, last_line);
    (scanner.active, scanner.rejections)
}

fn parse_rules(lines: &[LogicalLine]) -> Vec<RawRule> {
    let mut rules = Vec::<RawRule>::new();
    let mut previous_rule: Option<usize> = None;
    for line in lines {
        if line.text.starts_with('\t') {
            if let Some(index) = previous_rule {
                rules[index].recipe_form = rules[index].recipe_form.with_following();
            }
            continue;
        }
        let uncommented = strip_make_comment(&line.text);
        let statement = uncommented.trim();
        if statement.is_empty() || statement.starts_with('%') || statement.starts_with('#') {
            continue;
        }
        if is_make_control(statement) {
            previous_rule = None;
            continue;
        }
        let Some((colon, colon_form)) = first_rule_colon(statement) else {
            previous_rule = None;
            continue;
        };
        let target = statement[..colon].trim();
        if target.is_empty() {
            previous_rule = None;
            continue;
        }
        let after_colon = &statement[colon + if colon_form.is_double() { 2 } else { 1 }..];
        let (prerequisites, inline_recipe) = split_inline_recipe(after_colon);
        let target_specific = variable_assignment(prerequisites.trim()).is_some();
        rules.push(RawRule {
            target: target.to_owned(),
            prerequisites: prerequisites.to_owned(),
            line: line.start_line + 1,
            state: line.state,
            recipe_form: RecipeForm::from_inline(inline_recipe),
            colon_form,
            target_specific,
        });
        previous_rule = Some(rules.len() - 1);
    }
    rules
}

fn first_rule_colon(line: &str) -> Option<(usize, ColonForm)> {
    let colons = top_level_positions(line, ':');
    let first = *colons.first()?;
    let double_colon = line.as_bytes().get(first + 1) == Some(&b':');
    let extra_colon = colons
        .iter()
        .skip(1)
        .any(|position| !double_colon || *position != first + 1);
    Some((first, ColonForm::from_parts(double_colon, extra_colon)))
}

fn split_inline_recipe(prerequisites: &str) -> (&str, bool) {
    top_level_positions(prerequisites, ';')
        .first()
        .map_or((prerequisites, false), |position| {
            (&prerequisites[..*position], true)
        })
}

fn split_order_only(prerequisites: &str) -> Result<(&str, &str), &'static str> {
    let pipes = top_level_positions(prerequisites, '|');
    if pipes.len() > 1 {
        return Err("multiple order-only separators are unsupported");
    }
    let Some(position) = pipes.first().copied() else {
        return Ok((prerequisites.trim(), ""));
    };
    let normal = prerequisites[..position].trim();
    let order_only = prerequisites[position + 1..].trim();
    if order_only.is_empty() {
        return Err("order-only separator has no prerequisites");
    }
    Ok((normal, order_only))
}

fn top_level_positions(text: &str, needle: char) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut paren_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        match character {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            _ => {}
        }
        if character == needle && paren_depth == 0 && brace_depth == 0 {
            positions.push(index);
        }
    }
    positions
}

fn object_extension(path: &str) -> Option<&'static str> {
    if Path::new(path).extension() == Some(OsStr::new("o")) {
        Some(".o")
    } else if Path::new(path).extension() == Some(OsStr::new("d")) {
        Some(".d")
    } else {
        None
    }
}

fn looks_like_object_identity(expression: &str) -> bool {
    expression.contains(".o") || expression.contains(".d")
}

fn is_generated_object_path(path: &str) -> bool {
    let Some(tail) = path.strip_prefix(&format!("{GENERATED_ALIAS}/")) else {
        return false;
    };
    object_extension(path).is_some() && safe_components(tail)
}

fn has_non_root_dollar(value: &str) -> bool {
    value
        .replace(SOURCE_ALIAS, "")
        .replace(BUILD_ALIAS, "")
        .contains('$')
}

fn is_normal_prerequisite(path: &str) -> bool {
    if rooted_alias_tail(path, SOURCE_ALIAS).is_some()
        || rooted_alias_tail(path, BUILD_ALIAS).is_some()
    {
        return true;
    }
    safe_endpoint(path)
}

fn is_order_only_prerequisite(path: &str) -> bool {
    if rooted_alias_tail(path, BUILD_ALIAS).is_some() {
        return true;
    }
    safe_endpoint(path)
}

fn rooted_alias_tail<'a>(path: &'a str, alias: &str) -> Option<&'a str> {
    if path == alias {
        return Some("");
    }
    path.strip_prefix(&format!("{alias}/"))
        .filter(|tail| safe_components(tail))
}

fn safe_components(tail: &str) -> bool {
    !tail.is_empty() && tail.split('/').all(safe_component)
}

fn safe_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-'))
}

fn safe_endpoint(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-'))
}

fn contains_glob_syntax(value: &str) -> bool {
    value.contains(['*', '?', '[', ']'])
}

fn source_file(relative_dir: &Path) -> Result<String, String> {
    if relative_dir.is_absolute()
        || relative_dir
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err("relative directory contains an absolute or non-normal component".into());
    }
    let directory = relative_dir.to_string_lossy().replace('\\', "/");
    if directory.is_empty() || directory == "." {
        Ok("mmakefile.src".into())
    } else {
        Ok(format!("{directory}/mmakefile.src"))
    }
}

fn line_state(states: Option<&[Option<bool>]>, index: usize) -> ConditionalTruth {
    states.map_or(ConditionalTruth::True, |states| {
        match states.get(index).copied().flatten() {
            Some(true) => ConditionalTruth::True,
            Some(false) => ConditionalTruth::False,
            None => ConditionalTruth::Unknown,
        }
    })
}

const fn combine_state(left: ConditionalTruth, right: ConditionalTruth) -> ConditionalTruth {
    match (left, right) {
        (ConditionalTruth::False, _) | (_, ConditionalTruth::False) => ConditionalTruth::False,
        (ConditionalTruth::Unknown, _) | (_, ConditionalTruth::Unknown) => {
            ConditionalTruth::Unknown
        }
        _ => ConditionalTruth::True,
    }
}

fn conditional_open(statement: &str) -> Option<&str> {
    ["ifeq", "ifneq", "ifdef", "ifndef"]
        .into_iter()
        .find_map(|directive| directive_tail(statement, directive))
}

fn genmf_define_tail(statement: &str) -> Option<&str> {
    let tail = statement.strip_prefix("%define")?;
    if !tail.is_empty() && !tail.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    Some(tail.trim())
}

fn is_else(statement: &str) -> bool {
    statement == "else"
        || statement.strip_prefix("else").is_some_and(|tail| {
            tail.chars().next().is_some_and(char::is_whitespace)
                && conditional_open(tail.trim_start()).is_some()
        })
}

fn is_endif(statement: &str) -> bool {
    statement == "endif" || statement.strip_prefix("endif").is_some_and(str::is_empty)
}

fn is_make_control(statement: &str) -> bool {
    conditional_open(statement).is_some() || is_else(statement) || is_endif(statement)
}

fn is_define_open(statement: &str) -> bool {
    let mut words = statement.split_whitespace();
    while matches!(
        words.clone().next(),
        Some("override" | "export" | "unexport" | "private")
    ) {
        words.next();
    }
    words.next() == Some("define")
}

fn define_name(statement: &str) -> Option<&str> {
    let mut rest = statement;
    loop {
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..word_end];
        if matches!(word, "override" | "export" | "unexport" | "private") {
            rest = rest[word_end..].trim_start();
            continue;
        }
        if word != "define" {
            return None;
        }
        let name = rest[word_end..].trim();
        return (!name.is_empty()).then_some(name);
    }
}

fn recipe_prefix_definition_reason(statement: &str) -> Option<&'static str> {
    let name = define_name(statement)?.split_whitespace().next()?;
    if name == ".RECIPEPREFIX" {
        Some("source defines .RECIPEPREFIX; recipe boundaries are unprovable")
    } else if contains_make_expansion(name) {
        Some("dynamic define name may mutate .RECIPEPREFIX; recipe boundaries are unprovable")
    } else {
        None
    }
}

fn is_define_close(statement: &str) -> bool {
    statement == "endef" || statement.strip_prefix("endef").is_some_and(str::is_empty)
}

fn starts_control_word(statement: &str, word: &str) -> bool {
    statement
        .strip_prefix(word)
        .is_some_and(|tail| tail.is_empty() || tail.chars().next().is_some_and(char::is_whitespace))
}

fn rejection(
    file: &str,
    line: usize,
    reason: impl Into<String>,
) -> SourceDependencyOverlayRejection {
    SourceDependencyOverlayRejection {
        file: file.to_owned(),
        line,
        reason: reason.into(),
    }
}

fn budget_rejection(
    file: &str,
    line: usize,
    resource: &str,
) -> (
    Vec<SourceDependencyOverlay>,
    Vec<SourceDependencyOverlayRejection>,
) {
    (
        Vec::new(),
        vec![rejection(
            file,
            line,
            format!("dependency overlay exceeds {resource}"),
        )],
    )
}

#[cfg(test)]
mod tests {
    use super::{collect, SourceDependencyOverlay};
    use crate::dirs::DirVars;
    use crate::make_vars::{collect_vars_impl, ConditionalTruth};
    use std::fmt::Write as _;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    const REL: &str = "fixture/unit";

    fn scan(
        content: &str,
        states_override: Option<&[ConditionalTruth]>,
    ) -> (
        Vec<SourceDependencyOverlay>,
        Vec<super::SourceDependencyOverlayRejection>,
    ) {
        scan_at(content, REL, states_override)
    }

    fn scan_at(
        content: &str,
        relative_dir: &str,
        states_override: Option<&[ConditionalTruth]>,
    ) -> (
        Vec<SourceDependencyOverlay>,
        Vec<super::SourceDependencyOverlayRejection>,
    ) {
        scan_at_with_source_files(content, relative_dir, &[], states_override)
    }

    fn scan_at_with_source_files(
        content: &str,
        relative_dir: &str,
        source_files: &[&str],
        states_override: Option<&[ConditionalTruth]>,
    ) -> (
        Vec<SourceDependencyOverlay>,
        Vec<super::SourceDependencyOverlayRejection>,
    ) {
        let tree = TempDir::new().unwrap();
        fs::create_dir_all(tree.path().join("config")).unwrap();
        fs::write(tree.path().join("config/make.cfg.in"), "").unwrap();
        let source_dir = tree.path().join(relative_dir);
        fs::create_dir_all(&source_dir).unwrap();
        for source_file in source_files {
            fs::write(source_dir.join(source_file), "").unwrap();
        }
        let (scope, states) = collect_vars_impl(content, None);
        let dirs = DirVars::load(tree.path());
        let root = PathBuf::from(tree.path());
        let public_states: Vec<_> = states_override
            .unwrap_or(&states)
            .iter()
            .map(|state| match state {
                ConditionalTruth::True => Some(true),
                ConditionalTruth::False => Some(false),
                ConditionalTruth::Unknown => None,
            })
            .collect();
        collect(
            content,
            &scope,
            &dirs,
            &root,
            Path::new(relative_dir),
            Some(&public_states),
        )
    }
    fn assign_true_states(content: &str) -> Vec<ConditionalTruth> {
        vec![ConditionalTruth::True; content.lines().count()]
    }

    fn object_rule(content: &str) -> String {
        format!("OBJDIR := $(GENDIR)/{REL}\n{content}")
    }

    #[test]
    fn recognizes_i386_order_only_directory_overlay() {
        let content =
            "OBJDIR := $(GENDIR)/arch/i386-pc/kernel\n$(OBJDIR)/smpbootstrap.o : | $(OBJDIR)\n";
        let (objects, rejected) = scan_at(content, "arch/i386-pc/kernel", None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(objects[0].file, "arch/i386-pc/kernel/mmakefile.src");
        assert_eq!(
            objects[0].outputs,
            ["${AROS_BUILD_DIR}/gen/arch/i386-pc/kernel/smpbootstrap.o"]
        );
        assert!(objects[0].normal_prerequisites.is_empty());
        assert_eq!(
            objects[0].order_only_prerequisites,
            ["${AROS_BUILD_DIR}/gen/arch/i386-pc/kernel"]
        );
    }

    #[test]
    fn recognizes_x86_64_distinct_order_only_directories() {
        let content = concat!(
            "OBJDIR := $(GENDIR)/arch/x86_64-pc/kernel\n",
            "MAINDIR := rom/kernel\n",
            "ARCHOBJDIR := $(GENDIR)/$(MAINDIR)/kernel/arch\n",
            "$(OBJDIR)/smpbootstrap.o : | $(OBJDIR) $(ARCHOBJDIR)\n",
        );
        let (objects, rejected) = scan_at(content, "arch/x86_64-pc/kernel", None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(
            objects[0].outputs,
            ["${AROS_BUILD_DIR}/gen/arch/x86_64-pc/kernel/smpbootstrap.o"]
        );
        assert!(objects[0].normal_prerequisites.is_empty());
        assert_eq!(
            objects[0].order_only_prerequisites,
            [
                "${AROS_BUILD_DIR}/gen/arch/x86_64-pc/kernel",
                "${AROS_BUILD_DIR}/gen/rom/kernel/kernel/arch",
            ]
        );
    }

    #[test]
    fn resolves_multiple_locale_basenames_through_make_wildcard() {
        let content = concat!(
            "LANGUAGES := $(basename $(call WILDCARD, *.c))\n",
            "OBJDIR := $(GENDIR)/$(CURDIR)\n",
            "OBJS := $(addprefix $(OBJDIR)/,$(addsuffix .o,$(LANGUAGES)))\n",
            "DEPS := $(addprefix $(OBJDIR)/,$(addsuffix .d,$(LANGUAGES)))\n",
            "$(OBJS) $(DEPS) : | $(OBJDIR)\n",
        );
        let (objects, rejected) = scan_at_with_source_files(
            content,
            "workbench/locale/languages",
            &["english.c", "german.c", "polish.c"],
            None,
        );
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(
            objects[0].outputs,
            [
                "${AROS_BUILD_DIR}/gen/workbench/locale/languages/english.o",
                "${AROS_BUILD_DIR}/gen/workbench/locale/languages/german.o",
                "${AROS_BUILD_DIR}/gen/workbench/locale/languages/polish.o",
                "${AROS_BUILD_DIR}/gen/workbench/locale/languages/english.d",
                "${AROS_BUILD_DIR}/gen/workbench/locale/languages/german.d",
                "${AROS_BUILD_DIR}/gen/workbench/locale/languages/polish.d",
            ]
        );
        assert!(objects[0].normal_prerequisites.is_empty());
        assert_eq!(
            objects[0].order_only_prerequisites,
            ["${AROS_BUILD_DIR}/gen/workbench/locale/languages"]
        );
    }

    #[test]
    fn recognizes_aboutaros_generated_headers_for_both_outputs() {
        let content = concat!(
            "GENERATED = $(TOP)/$(CURDIR)/authors.h $(TOP)/$(CURDIR)/sponsors.h ",
            "$(TOP)/$(CURDIR)/acknowledgements.h\n",
            "$(GENDIR)/$(CURDIR)/aboutaros.d: $(GENERATED)\n",
            "$(GENDIR)/$(CURDIR)/aboutaros.o: $(GENERATED)\n",
        );
        let (objects, rejected) = scan_at(content, "workbench/system/AboutAROS", None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(objects.len(), 2);
        assert_eq!(
            objects[0].outputs,
            ["${AROS_BUILD_DIR}/gen/workbench/system/AboutAROS/aboutaros.d"]
        );
        assert_eq!(
            objects[1].outputs,
            ["${AROS_BUILD_DIR}/gen/workbench/system/AboutAROS/aboutaros.o"]
        );
        let expected_generated = [
            "${AROS_BUILD_DIR}/workbench/system/AboutAROS/authors.h",
            "${AROS_BUILD_DIR}/workbench/system/AboutAROS/sponsors.h",
            "${AROS_BUILD_DIR}/workbench/system/AboutAROS/acknowledgements.h",
        ];
        assert_eq!(objects[0].normal_prerequisites, expected_generated);
        assert_eq!(objects[1].normal_prerequisites, expected_generated);
        assert!(objects[0].order_only_prerequisites.is_empty());
        assert!(objects[1].order_only_prerequisites.is_empty());
        assert!(!objects[1]
            .normal_prerequisites
            .iter()
            .any(|path| path.ends_with("/aboutaros.d")));
    }

    #[test]
    fn preserves_normal_and_order_only_dependencies_separately() {
        let content = object_rule(
            "$(OBJDIR)/x.o: $(SRCDIR)/fixture/x.c $(OBJDIR)/config.h | $(OBJDIR) setup\n",
        );
        let (objects, rejected) = scan(&content, None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(
            objects[0].normal_prerequisites,
            [
                "${AROS_SOURCE_DIR}/fixture/x.c",
                "${AROS_BUILD_DIR}/gen/fixture/unit/config.h",
            ]
        );
        assert_eq!(
            objects[0].order_only_prerequisites,
            ["${AROS_BUILD_DIR}/gen/fixture/unit", "setup",]
        );
    }

    #[test]
    fn preserves_distinct_additions_for_one_output_and_rejects_exact_duplicates() {
        let distinct =
            object_rule("$(OBJDIR)/shared.o: first-input\n$(OBJDIR)/shared.o: second-input\n");
        let (objects, rejected) = scan(&distinct, None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0].outputs, objects[1].outputs);
        assert_eq!(objects[0].normal_prerequisites, ["first-input"]);
        assert_eq!(objects[1].normal_prerequisites, ["second-input"]);

        let duplicate =
            object_rule("$(OBJDIR)/shared.o: first-input\n$(OBJDIR)/shared.o: first-input\n");
        let (objects, rejected) = scan(&duplicate, None);
        assert_eq!(objects.len(), 1);
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].reason.contains("duplicate identical"));

        let repeated_output = object_rule("$(OBJDIR)/same.o $(OBJDIR)/same.o: first-input\n");
        let (objects, rejected) = scan(&repeated_output, None);
        assert!(objects.is_empty());
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].reason.contains("repeats an output"));
    }

    #[test]
    fn evaluates_variables_at_the_rule_parse_time_snapshot() {
        let content = "OBJDIR := $(GENDIR)/early\nOBJS := $(OBJDIR)/unit.o\n$(OBJS): $(OBJDIR)/source.h | $(OBJDIR)\nOBJDIR := $(GENDIR)/late\n";
        let (objects, rejected) = scan(content, None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(objects[0].outputs, ["${AROS_BUILD_DIR}/gen/early/unit.o"]);
        assert_eq!(
            objects[0].normal_prerequisites,
            ["${AROS_BUILD_DIR}/gen/early/source.h"]
        );
    }

    #[test]
    fn rejects_recipe_prefix_mutations_instead_of_projecting_partial_rules() {
        let hidden_producer = concat!(
            ".RECIPEPREFIX := >\n",
            "$(GENDIR)/fixture/unit/compiled.o: $(SRCDIR)/fixture/compiled.c\n",
            "> $(CC) -c $< -o $@\n",
            "$(GENDIR)/fixture/unit/literal.o: literal-input\n",
        );
        let (objects, rejected) = scan(hidden_producer, None);
        assert!(objects.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains(".RECIPEPREFIX")));

        for mutation in [
            "override export .RECIPEPREFIX := >\n",
            "$(PREFIX) := >\n",
            "undefine .RECIPEPREFIX\n",
            "define .RECIPEPREFIX\n>\nendef\n",
        ] {
            let content = format!("$(GENDIR)/fixture/unit/literal.o: literal-input\n{mutation}");
            let (objects, rejected) = scan(&content, None);
            assert!(
                objects.is_empty(),
                "mutation was not fail-closed: {mutation}"
            );
            assert!(rejected
                .iter()
                .any(|item| item.reason.contains(".RECIPEPREFIX")));
        }
    }

    #[test]
    fn skips_only_proven_inactive_recipe_prefix_mutations() {
        let unknown = concat!(
            "ifeq ($(UNKNOWN_CONDITION),yes)\n",
            ".RECIPEPREFIX := >\n",
            "endif\n",
            "$(GENDIR)/fixture/unit/literal.o: literal-input\n",
        );
        let (objects, rejected) = scan(unknown, None);
        assert!(objects.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains(".RECIPEPREFIX")));

        let inactive = concat!(
            "$(GENDIR)/fixture/unit/literal.o: literal-input\n",
            ".RECIPEPREFIX := >\n",
        );
        let mut states = assign_true_states(inactive);
        states[1] = ConditionalTruth::False;
        let (objects, rejected) = scan(inactive, Some(&states));
        assert_eq!(objects.len(), 1);
        assert!(rejected.is_empty(), "{rejected:#?}");
    }

    #[test]
    fn rejects_direct_nested_and_escaped_eval_before_define_masking() {
        for effect in [
            "$(eval $(GENDIR)/fixture/unit/hidden.o: generated-input)\n",
            "${eval $(GENDIR)/fixture/unit/hidden.o: generated-input}\n",
            "$(if 1,$(eval $(GENDIR)/fixture/unit/hidden.o: generated-input))\n",
            "define MUTATOR\n$(eval hidden := value)\nendef\n",
            "HIDDEN := $$(eval hidden := value)\n",
        ] {
            let content = format!("$(GENDIR)/fixture/unit/literal.o: literal-input\n{effect}");
            let (objects, rejected) = scan(&content, None);
            assert!(objects.is_empty(), "eval was not fail-closed: {effect}");
            assert!(rejected.iter().any(|item| item.reason.contains("eval")));
        }

        let unknown = concat!(
            "ifeq ($(UNKNOWN_CONDITION),yes)\n",
            "$(eval $(GENDIR)/fixture/unit/hidden.o: generated-input)\n",
            "endif\n",
            "$(GENDIR)/fixture/unit/literal.o: literal-input\n",
        );
        let (objects, rejected) = scan(unknown, None);
        assert!(objects.is_empty());
        assert!(rejected.iter().any(|item| item.reason.contains("eval")));

        let inactive = "$(GENDIR)/fixture/unit/literal.o: literal-input\n$(eval hidden := value)\n";
        let mut states = assign_true_states(inactive);
        states[1] = ConditionalTruth::False;
        let (objects, rejected) = scan(inactive, Some(&states));
        assert_eq!(objects.len(), 1);
        assert!(rejected.is_empty(), "{rejected:#?}");

        let commented =
            "$(GENDIR)/fixture/unit/literal.o: literal-input\n# $(eval hidden := value)\n";
        let (objects, rejected) = scan(commented, None);
        assert_eq!(objects.len(), 1);
        assert!(rejected.is_empty(), "{rejected:#?}");
    }

    #[test]
    fn rejects_standalone_expansion_statements_as_unresolved_mutations() {
        for invocation in ["$(call MUTATOR)", "${call MUTATOR}", "$$(call MUTATOR)"] {
            let content =
                format!("$(GENDIR)/fixture/unit/literal.o: literal-input\n{invocation}\n");
            let (objects, rejected) = scan(&content, None);
            assert!(
                objects.is_empty(),
                "invocation was not fail-closed: {invocation}"
            );
            assert!(rejected
                .iter()
                .any(|item| item.reason.contains("standalone Make expansion")));
        }

        let unknown = concat!(
            "ifeq ($(UNKNOWN_CONDITION),yes)\n",
            "$(call MUTATOR)\n",
            "endif\n",
            "$(GENDIR)/fixture/unit/literal.o: literal-input\n",
        );
        let (objects, rejected) = scan(unknown, None);
        assert!(objects.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("standalone Make expansion")));
    }

    #[test]
    fn records_unexpanded_include_and_genmf_effects_without_hiding_literal_candidates() {
        let content = concat!(
            "OBJDIR := $(GENDIR)/fixture/unit\n",
            "include fragment.mk\n",
            "# %compile_q may expand before Make strips this comment\n",
            "%define generated_rule\n",
            "$(GENDIR)/fixture/unit/template.o: template-input\n",
            "%end\n",
            "$(OBJDIR)/literal.o: literal-input\n",
        );
        let (objects, rejected) = scan(content, None);
        assert_eq!(objects.len(), 1);
        assert!(objects[0].outputs[0].ends_with("literal.o"));
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("GNU Make include remains unexpanded")));
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains("%compile_q")));
        assert!(rejected.iter().any(|item| item.reason.contains("%define")));
        assert!(rejected.iter().any(|item| item.reason.contains("%end")));
        assert!(!objects
            .iter()
            .flat_map(|object| &object.outputs)
            .any(|output| output.ends_with("template.o")));
    }

    #[test]
    fn rejects_second_expansion_references_even_when_the_first_pass_is_parse_time() {
        let content = concat!(
            ".SECONDEXPANSION:\n",
            "MODE := global\n",
            "DYNAMIC := dep-$(MODE)\n",
            "$(GENDIR)/fixture/unit/late.o: MODE := target\n",
            "$(GENDIR)/fixture/unit/late.o: $$(DYNAMIC)\n",
        );
        let (objects, rejected) = scan(content, None);
        assert!(objects.is_empty());
        assert!(rejected
            .iter()
            .any(|item| item.reason.contains(".SECONDEXPANSION")));
    }

    #[test]
    fn skips_false_rules_rejects_unknown_branches_and_ignores_nested_define_text() {
        let false_content = "$(UNKNOWN).o: setup\n";
        let mut false_states = assign_true_states(false_content);
        false_states[0] = ConditionalTruth::False;
        let (objects, rejected) = scan(false_content, Some(&false_states));
        assert!(objects.is_empty());
        assert!(rejected.is_empty());

        let unknown =
            "ifeq ($(UNKNOWN_CONDITION),yes)\n$(GENDIR)/fixture/unit/unknown.o: setup\nendif\n";
        let (objects, rejected) = scan(unknown, None);
        assert!(objects.is_empty());
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].reason.contains("unknown conditional"));

        let define = "define OUTER\n$(GENDIR)/fixture/unit/fake.o: setup\ndefine INNER\n$(GENDIR)/fixture/unit/fake2.d: setup\nendef\nendef\n$(GENDIR)/fixture/unit/real.o: setup\n";
        let (objects, rejected) = scan(define, None);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(objects.len(), 1);
        assert!(objects[0].outputs[0].ends_with("real.o"));
    }

    #[test]
    fn rejects_unresolved_or_mixed_outputs_and_unsupported_rule_forms() {
        for content in [
            "$(UNKNOWN).o: setup\n",
            "$(GENDIR)/fixture/unit/ok.o phony: setup\n",
            "$(GENDIR)/fixture/unit/inline.o: setup ; @touch $@\n",
            "$(GENDIR)/fixture/unit/specific.o: CFLAGS := -O2\n",
            "$(GENDIR)/fixture/unit/double.o:: setup\n",
        ] {
            let (objects, rejected) = scan(&object_rule(content), None);
            assert!(
                objects.is_empty(),
                "source was unexpectedly projected: {content}"
            );
            assert!(
                !rejected.is_empty(),
                "source was silently skipped: {content}"
            );
        }

        let static_pattern =
            object_rule("OBJS := $(GENDIR)/fixture/unit/static.o\n$(OBJS): %.o: %.c\n");
        let (objects, rejected) = scan(&static_pattern, None);
        assert!(objects.is_empty());
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].reason.contains("static-pattern"));
    }

    #[test]
    fn rejects_aliases_globs_empty_prerequisites_and_malformed_controls() {
        for content in [
            "${CMAKE_BINARY_DIR}/gen/unit.o: setup\n",
            "$(GENDIR)/fixture/unit/../escape.o: setup\n",
            "$(GENDIR)/fixture/unit/*.o: setup\n",
            "$(GENDIR)/fixture/unit/empty.o:\n",
            "$(GENDIR)/fixture/unit/unresolved.o: $(MISSING_PREREQUISITE)\n",
            "$(GENDIR)/fixture/unit/glob.o: $(wildcard *.h)\n",
            "endif\n$(GENDIR)/fixture/unit/bad.o: setup\n",
        ] {
            let (objects, rejected) = scan(&object_rule(content), None);
            assert!(
                objects.is_empty(),
                "source was unexpectedly projected: {content}"
            );
            assert!(
                !rejected.is_empty(),
                "source was silently skipped: {content}"
            );
        }

        let producer = object_rule(
            "$(OBJDIR)/compiled.o: $(SRCDIR)/fixture/compiled.c\n\t$(CC) -c $< -o $@\n",
        );
        let (objects, rejected) = scan(&producer, None);
        assert!(objects.is_empty());
        assert!(
            rejected.is_empty(),
            "recipe-bearing producer is outside overlay scope: {rejected:#?}"
        );
    }

    #[test]
    fn enforces_snapshot_line_output_and_reference_budgets() {
        let too_large = "#".repeat(1_048_577);
        let (objects, rejected) = scan(&too_large, None);
        assert!(objects.is_empty());
        assert!(rejected[0].reason.contains("1 MiB"));

        let too_many_lines = "\n".repeat(16_385);
        let (objects, rejected) = scan(&too_many_lines, None);
        assert!(objects.is_empty());
        assert!(rejected[0].reason.contains("16384 lines"));

        let mut many_outputs = String::new();
        for index in 0..4_097 {
            writeln!(many_outputs, "$(GENDIR)/fixture/unit/o{index}.o: setup").unwrap();
        }
        let (objects, rejected) = scan(&many_outputs, None);
        assert!(objects.is_empty());
        assert!(rejected[0].reason.contains("4096 output identities"));

        let mut many_refs = String::new();
        for index in 0..8_193 {
            writeln!(many_refs, "$(GENDIR)/fixture/unit/ref.o: dep{index}").unwrap();
        }
        let (objects, rejected) = scan(&many_refs, None);
        assert!(objects.is_empty());
        assert!(rejected[0].reason.contains("8192 prerequisite references"));
    }
}
