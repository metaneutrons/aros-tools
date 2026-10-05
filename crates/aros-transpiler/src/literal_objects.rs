//! Closed collector for source-declared, standalone C and C++ object rules.
//!
//! Only literal compile-only commands, source-tree files, and finite local
//! aggregate owners are represented. This collector does not run Make or a
//! compiler and does not infer arguments from CMake's global flag variables.

use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext};
use crate::make_vars::{
    strip_make_comment, undefine_directive, variable_assignment, ConditionalTruth, VarScope,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

const BUILD_GENERATED_ROOT: &str = "${AROS_BUILD_DIR}/gen";
const SOURCE_ROOT_ALIAS: &str = "${AROS_SOURCE_DIR}";
const BUILD_ROOT_ALIAS: &str = "${AROS_BUILD_DIR}";
const MAX_SNAPSHOT_BYTES: usize = 2 * 1024 * 1024;
const MAX_RULES: usize = 16_384;
const MAX_REJECTIONS: usize = 4096;

/// One compile-only object producer owned by a source-local Make aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralObjectDecl {
    /// Existing C or C++ source below the configured source root.
    pub source: String,
    /// Generated object path in the configured build tree.
    pub output: String,
    /// `C` or `CXX`.
    pub language: String,
    /// Ordered compiler arguments, excluding the source, `-c`, and output pair.
    pub arguments: Vec<String>,
    /// One-based line containing the object rule.
    pub line: usize,
}

/// A finite ordinary Make target owning one or more literal object producers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralObjectGroupDecl {
    /// Safe literal target name in the joined mmakefile.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line containing the aggregate rule.
    pub line: usize,
    /// Object producers owned by this aggregate, in prerequisite order.
    pub objects: Vec<LiteralObjectDecl>,
}

/// A relevant literal object producer or aggregate outside the closed model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralObjectRejection {
    /// Best available local target name, or `<unknown>`.
    pub owner: String,
    /// Source-relative mmakefile path.
    pub file: String,
    /// One-based line of the closest relevant declaration.
    pub line: usize,
    /// Why the declaration was refused.
    pub reason: String,
}

#[derive(Debug, Clone)]
struct Recipe {
    text: String,
    state: ConditionalTruth,
    valid: bool,
}

#[derive(Debug, Clone)]
struct Rule {
    target: String,
    prerequisites: String,
    order_only: Option<String>,
    line: usize,
    state: ConditionalTruth,
    valid: bool,
    double_colon: bool,
    inline_recipe: Option<String>,
    recipes: Vec<Recipe>,
}

#[derive(Debug, Clone)]
struct TargetAssignment {
    target: String,
    line: usize,
}

#[derive(Debug, Clone)]
struct Candidate {
    output: Option<String>,
    line: usize,
    state: ConditionalTruth,
    declaration: Option<LiteralObjectDecl>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct AggregateCandidate {
    line: usize,
    state: ConditionalTruth,
    owner: String,
    owner_is_safe: bool,
    prerequisites: Vec<String>,
    order_only: Vec<String>,
    recipe_count: usize,
    inline_recipe: bool,
    valid: bool,
    error: Option<String>,
}

#[derive(Debug)]
struct SourceControls {
    suppressed: Vec<bool>,
    valid: Vec<bool>,
    default_states: Vec<ConditionalTruth>,
}

/// Collects literal C/C++ object rules and their source-local aggregate owners.
///
/// Recipe variables use their final file-scope values. Target and prerequisite
/// expressions use the values visible at their rule line. `line_states` uses
/// zero-based positions in the continuation-joined snapshot.
#[must_use]
pub(crate) fn collect_from_snapshot(
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line_states: Option<&[ConditionalTruth]>,
    source_snapshot: &str,
) -> (Vec<LiteralObjectGroupDecl>, Vec<LiteralObjectRejection>) {
    let file = match source_file_name(rel_dir) {
        Ok(file) => file,
        Err(reason) => {
            return (
                Vec::new(),
                vec![rejection(
                    "<unknown>",
                    &file_name(rel_dir),
                    1,
                    format!("literal-object source directory: {reason}"),
                )],
            );
        }
    };
    if source_snapshot.len() > MAX_SNAPSHOT_BYTES {
        return (
            Vec::new(),
            vec![rejection(
                "<unknown>",
                &file,
                1,
                "joined mmakefile exceeds the literal-object scan limit",
            )],
        );
    }

    let lines = source_snapshot.lines().collect::<Vec<_>>();
    let controls = scan_source_controls(&lines);
    let (rules, target_assignments) = parse_rules(&lines, &controls, line_states);
    if rules.len() > MAX_RULES {
        return (
            Vec::new(),
            vec![rejection(
                "<unknown>",
                &file,
                1,
                "joined mmakefile exceeds the literal-object rule limit",
            )],
        );
    }

    let generated_root = match dirs.expand("$(GENDIR)") {
        Some(path) if path == BUILD_GENERATED_ROOT => path,
        Some(_) => {
            return (
                Vec::new(),
                vec![rejection(
                    "<unknown>",
                    &file,
                    1,
                    "configured `GENDIR` is outside `${AROS_BUILD_DIR}/gen`",
                )],
            );
        }
        None => {
            return (
                Vec::new(),
                vec![rejection(
                    "<unknown>",
                    &file,
                    1,
                    "configured `GENDIR` is unresolved",
                )],
            );
        }
    };
    let local_generated = match local_generated_root(&generated_root, rel_dir) {
        Ok(path) => path,
        Err(reason) => {
            return (
                Vec::new(),
                vec![rejection(
                    "<unknown>",
                    &file,
                    1,
                    format!("literal-object generated directory: {reason}"),
                )],
            );
        }
    };

    let driver_issue = driver_binding_issue(&lines, &controls, line_states, dirs);
    let mut candidates = Vec::new();
    for rule in &rules {
        if rule.state == ConditionalTruth::False || rule.target.contains('%') {
            continue;
        }
        let hinted = generated_object_hint(&rule.target);
        let targets = match evaluate_rule_list(&rule.target, scope, dirs, root, rel_dir, rule.line)
        {
            Ok(targets) => targets,
            Err(error) if hinted => {
                candidates.push(Candidate {
                    output: None,
                    line: rule.line,
                    state: rule.state,
                    declaration: None,
                    error: Some(format!("cannot resolve literal object target: {error}")),
                });
                continue;
            }
            Err(_) => continue,
        };
        let generated_targets = targets
            .iter()
            .filter(|target| is_generated_object_path(target, &generated_root))
            .cloned()
            .collect::<Vec<_>>();
        if generated_targets.is_empty() {
            continue;
        }

        if targets.len() != 1 || generated_targets.len() != 1 {
            candidates.push(Candidate {
                output: generated_targets.first().cloned(),
                line: rule.line,
                state: rule.state,
                declaration: None,
                error: Some("literal object rule must declare exactly one generated target".into()),
            });
            continue;
        }
        let output = generated_targets[0].clone();
        let result = validate_local_object_output(&output, &local_generated)
            .and_then(|()| parse_compile_rule(rule, &output, scope, dirs, root, rel_dir));
        let mut candidate = match result {
            Ok(declaration) => Candidate {
                output: Some(output.clone()),
                line: rule.line,
                state: rule.state,
                declaration: Some(declaration),
                error: None,
            },
            Err(reason) => Candidate {
                output: Some(output.clone()),
                line: rule.line,
                state: rule.state,
                declaration: None,
                error: Some(reason),
            },
        };
        if let Some((issue_line, reason)) = &driver_issue {
            candidate.declaration = None;
            candidate.error = Some(format!("{reason} (line {})", issue_line + 1));
        }
        if target_assignments.iter().any(|assignment| {
            assignment_matches_output(assignment, &output, scope, dirs, root, rel_dir)
        }) {
            candidate.declaration = None;
            candidate.error =
                Some("literal object target has a target-specific Make assignment".into());
        }
        candidates.push(candidate);
    }

    let candidates_by_output = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| {
            candidate
                .output
                .as_ref()
                .map(|output| (output.clone(), index))
        })
        .fold(
            BTreeMap::<String, Vec<usize>>::new(),
            |mut map, (path, index)| {
                map.entry(path).or_default().push(index);
                map
            },
        );
    for (output, indexes) in &candidates_by_output {
        if indexes.len() > 1 {
            for index in indexes {
                candidates[*index].declaration = None;
                candidates[*index].error = Some(format!(
                    "generated object output `{output}` has more than one literal producer"
                ));
            }
        }
    }

    let aggregates = collect_relevant_aggregates(
        &rules,
        &target_assignments,
        &candidates_by_output,
        scope,
        dirs,
        root,
        rel_dir,
    );
    let mut groups = Vec::new();
    let mut rejections = Vec::new();
    let mut claimed_outputs = BTreeMap::<String, Vec<(String, usize)>>::new();
    let mut owners_seen = BTreeMap::<String, Vec<usize>>::new();

    for aggregate in aggregates {
        let owner = if aggregate.owner.is_empty() {
            "<unknown>".to_owned()
        } else {
            aggregate.owner.clone()
        };
        let matched = aggregate
            .prerequisites
            .iter()
            .filter_map(|path| {
                candidates_by_output
                    .get(path)
                    .map(|indexes| (path, indexes))
            })
            .flat_map(|(path, indexes)| indexes.iter().map(move |index| (path.clone(), *index)))
            .collect::<Vec<_>>();
        if matched.is_empty() {
            continue;
        }

        let mut reason = aggregate.error.clone();
        if let Some((output, _)) = matched.iter().find(|(output, _)| {
            candidates_by_output
                .get(output)
                .is_some_and(|indexes| indexes.len() > 1)
        }) {
            reason.get_or_insert_with(|| {
                format!("generated object output `{output}` has more than one literal producer")
            });
        }
        if aggregate.state == ConditionalTruth::Unknown {
            reason.get_or_insert_with(|| {
                "literal object aggregate is in an unresolved Make conditional".into()
            });
        }
        if !aggregate.valid {
            reason.get_or_insert_with(|| {
                "literal object aggregate is governed by malformed Make control".into()
            });
        }
        if !aggregate.owner_is_safe {
            reason.get_or_insert_with(|| {
                "literal object prerequisites have no safe named aggregate owner".into()
            });
        }
        if aggregate.recipe_count != 0 || aggregate.inline_recipe {
            reason.get_or_insert_with(|| {
                "literal object aggregate must be a dependency-only Make rule".into()
            });
        }
        if !aggregate.order_only.is_empty() {
            reason.get_or_insert_with(|| {
                "literal object aggregate has unsupported order-only prerequisites".into()
            });
        }
        if aggregate.prerequisites.is_empty()
            || aggregate
                .prerequisites
                .iter()
                .any(|path| Path::new(path).extension() != Some(std::ffi::OsStr::new("o")))
        {
            reason.get_or_insert_with(|| {
                "literal object aggregate must have only finite `.o` prerequisites".into()
            });
        }
        if aggregate
            .prerequisites
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != aggregate.prerequisites.len()
        {
            reason.get_or_insert_with(|| {
                "literal object aggregate repeats an object prerequisite".into()
            });
        }
        if aggregate.prerequisites.len() != matched.len() {
            reason.get_or_insert_with(|| {
                "literal object aggregate includes an object without a local literal producer"
                    .into()
            });
        }
        // GNU Make also inherits target-specific variables through callers of
        // this aggregate. Until the whole caller context is represented, an
        // assignment elsewhere in this source scope is not proven irrelevant.
        if !target_assignments.is_empty() {
            reason.get_or_insert_with(|| {
                "literal object aggregate has a target-specific Make assignment inherited by its object prerequisites".into()
            });
        }

        let mut objects = Vec::new();
        for (output, candidate_index) in &matched {
            let candidate = &candidates[*candidate_index];
            if candidate.state == ConditionalTruth::Unknown {
                reason.get_or_insert_with(|| {
                    "literal object producer is in an unresolved Make conditional".into()
                });
            }
            if let Some(error) = &candidate.error {
                reason.get_or_insert_with(|| {
                    format!("literal object producer `{output}` was refused: {error}")
                });
            }
            if let Some(declaration) = &candidate.declaration {
                objects.push(declaration.clone());
            }
            claimed_outputs
                .entry(output.clone())
                .or_default()
                .push((owner.clone(), aggregate.line));
        }
        if aggregate.owner_is_safe {
            owners_seen
                .entry(owner.clone())
                .or_default()
                .push(aggregate.line);
        }
        if let Some(reason) = reason {
            rejections.push(rejection(&owner, &file, aggregate.line, reason));
        } else if objects.len() != aggregate.prerequisites.len() {
            rejections.push(rejection(
                &owner,
                &file,
                aggregate.line,
                "literal object aggregate has a missing producer declaration",
            ));
        } else {
            groups.push(LiteralObjectGroupDecl {
                owner,
                file: file.clone(),
                line: aggregate.line + 1,
                objects,
            });
        }
    }

    for (output, owners) in &claimed_outputs {
        let distinct = owners
            .iter()
            .map(|(owner, _)| owner)
            .collect::<BTreeSet<_>>();
        if distinct.len() > 1 {
            groups.retain(|group| !group.objects.iter().any(|object| object.output == *output));
            for (owner, line) in owners {
                rejections.push(rejection(
                    owner,
                    &file,
                    *line,
                    format!("literal object output `{output}` has conflicting aggregate owners"),
                ));
            }
        }
    }
    for (owner, lines) in owners_seen {
        if lines.len() > 1 {
            groups.retain(|group| group.owner != owner);
            for line in lines {
                rejections.push(rejection(
                    &owner,
                    &file,
                    line,
                    "literal object aggregate owner has duplicate producer rules",
                ));
            }
        }
    }

    let claimed = claimed_outputs.keys().cloned().collect::<BTreeSet<_>>();
    for candidate in &candidates {
        if candidate.state == ConditionalTruth::False {
            continue;
        }
        if candidate
            .output
            .as_ref()
            .is_some_and(|output| claimed.contains(output))
        {
            continue;
        }
        rejections.push(rejection(
            "<unknown>",
            &file,
            candidate.line,
            candidate.error.clone().unwrap_or_else(|| {
                "literal object producer has no named local aggregate owner".into()
            }),
        ));
    }

    rejections.truncate(MAX_REJECTIONS);
    (groups, rejections)
}

fn parse_compile_rule(
    rule: &Rule,
    output: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
) -> Result<LiteralObjectDecl, String> {
    if rule.state == ConditionalTruth::Unknown {
        return Err("literal object rule is in an unresolved Make conditional".into());
    }
    if !rule.valid {
        return Err("literal object rule is governed by malformed Make control".into());
    }
    if rule.double_colon {
        return Err("literal object rule uses unsupported double-colon syntax".into());
    }
    let prerequisites =
        evaluate_rule_list(&rule.prerequisites, scope, dirs, root, rel_dir, rule.line)
            .map_err(|error| format!("cannot resolve literal object prerequisites: {error}"))?;
    if prerequisites.len() != 1 {
        return Err("literal object rule must have exactly one source prerequisite".into());
    }
    let source = prerequisites[0].clone();
    let source_relative = source_path_from_alias(&source)?;
    let source_relative_path = validate_relative_path(source_relative)?;
    let source_path = safe_source_file(root, &source_relative_path)?;
    let extension = source_path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let language = match extension {
        "c" => "C",
        "cpp" => "CXX",
        _ => return Err("literal object source must end in `.c` or `.cpp`".into()),
    };

    let order_only = rule
        .order_only
        .as_deref()
        .map(|raw| evaluate_rule_list(raw, scope, dirs, root, rel_dir, rule.line))
        .transpose()
        .map_err(|error| format!("cannot resolve literal object order-only prerequisite: {error}"))?
        .unwrap_or_default();
    if order_only.len() > 1 {
        return Err("literal object rule has more than one order-only prerequisite".into());
    }
    if let Some(directory) = order_only.first() {
        let output_parent = Path::new(output)
            .parent()
            .map(path_text)
            .ok_or_else(|| "literal object target has no generated parent directory".to_owned())?;
        if directory != &output_parent {
            return Err(
                "literal object order-only prerequisite must equal the object parent directory"
                    .into(),
            );
        }
    }

    let mut recipes = Vec::new();
    if let Some(inline) = &rule.inline_recipe {
        recipes.push(Recipe {
            text: inline.clone(),
            state: rule.state,
            valid: rule.valid,
        });
    }
    recipes.extend(
        rule.recipes
            .iter()
            .filter(|recipe| recipe.state != ConditionalTruth::False)
            .cloned(),
    );
    if recipes
        .iter()
        .any(|recipe| recipe.state != ConditionalTruth::True || !recipe.valid)
    {
        return Err(
            "literal object recipe is conditional or governed by malformed Make control".into(),
        );
    }
    let compiler_recipes = recipes
        .iter()
        .filter(|recipe| !is_echo_recipe(&recipe.text))
        .collect::<Vec<_>>();
    if compiler_recipes.len() != 1 {
        return Err("literal object rule must have exactly one compiler recipe".into());
    }
    let (recipe_language, arguments) =
        parse_compile_recipe(&compiler_recipes[0].text, scope, dirs, root, rel_dir)?;
    if recipe_language != language {
        return Err(format!(
            "literal object source extension requires {language}, but recipe selects {recipe_language}"
        ));
    }
    Ok(LiteralObjectDecl {
        source,
        output: output.to_owned(),
        language: language.to_owned(),
        arguments,
        line: rule.line + 1,
    })
}

fn parse_compile_recipe(
    raw: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
) -> Result<(String, Vec<String>), String> {
    let command = raw.trim_start_matches('\t').trim();
    let command = command.strip_prefix('@').unwrap_or(command);
    let (language, rest) = if let Some(rest) = command.strip_prefix("$(TARGET_CC)") {
        ("C", rest)
    } else if let Some(rest) = command.strip_prefix("$(TARGET_CXX)") {
        ("CXX", rest)
    } else {
        return Err(
            "literal object recipe must begin with `$(TARGET_CC)` or `$(TARGET_CXX)`".into(),
        );
    };
    if !rest.starts_with(char::is_whitespace) {
        return Err("literal object compiler role must be one complete command token".into());
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(
            "literal object compiler recipe is empty or contains shell syntax/quoting".into(),
        );
    }
    let source_token = "__AROS_LITERAL_SOURCE__";
    let output_token = "__AROS_LITERAL_OUTPUT__";
    let replaced = rest.replace("$<", source_token).replace("$@", output_token);
    if contains_shell_syntax(&replaced) {
        return Err("literal object compiler recipe contains shell syntax or quoting".into());
    }
    let final_context = MakeExprContext::new(scope, dirs, usize::MAX, root, rel_dir);
    let lookup = |name: &str| scope.path_raw_at(name, usize::MAX);
    let guard = |name: &str| variable_guard(scope, name, usize::MAX);
    let context = final_context.with_lookup(&lookup).with_guard(&guard);
    let expanded = evaluate_make_expr(&replaced, &context)
        .map_err(|error| format!("cannot resolve final literal compiler flags: {error}"))?;
    if has_unsupported_dollar_reference(&expanded) {
        return Err(
            "literal object compiler recipe retains an unsupported automatic or Make reference"
                .into(),
        );
    }
    let words = expanded
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    validate_compile_arguments(&words, source_token, output_token, dirs)
        .map(|arguments| (language.to_owned(), arguments))
}

fn validate_compile_arguments(
    words: &[String],
    source_token: &str,
    output_token: &str,
    dirs: &DirVars,
) -> Result<Vec<String>, String> {
    let mut compile_count = 0usize;
    let mut source_positions = Vec::new();
    let mut output_positions = Vec::new();
    let mut arguments = Vec::new();
    let mut index = 0usize;
    while index < words.len() {
        let word = &words[index];
        if word == "-c" {
            compile_count += 1;
            index += 1;
            continue;
        }
        if word == source_token {
            source_positions.push(index);
            index += 1;
            continue;
        }
        if word == "-o" {
            if words.get(index + 1).map(String::as_str) != Some(output_token) {
                return Err("literal object recipe must use exactly `-o $@`".into());
            }
            output_positions.push(index);
            index += 2;
            continue;
        }
        if word == output_token {
            return Err("literal object recipe uses `$@` outside the exact `-o $@` pair".into());
        }
        validate_flag_word(word, words.get(index + 1).map(String::as_str), dirs)?;
        arguments.push(word.clone());
        if matches!(word.as_str(), "-D" | "-U" | "-I") {
            let value = words
                .get(index + 1)
                .ok_or_else(|| format!("compiler option `{word}` has no value"))?;
            validate_flag_value(word, value, dirs)?;
            arguments.push(value.clone());
            index += 2;
        } else {
            index += 1;
        }
    }
    if compile_count != 1 {
        return Err("literal object compiler recipe must contain `-c` exactly once".into());
    }
    if source_positions.len() != 1 {
        return Err("literal object compiler recipe must use `$<` exactly once".into());
    }
    if output_positions.len() != 1 {
        return Err("literal object compiler recipe must use exactly one `-o $@` pair".into());
    }
    if output_positions[0] < source_positions[0] {
        return Err("literal object output pair must follow source automatic variable `$<`".into());
    }
    Ok(arguments)
}

fn validate_flag_word(word: &str, next: Option<&str>, dirs: &DirVars) -> Result<(), String> {
    if matches!(word, "-D" | "-U" | "-I") {
        return next.map_or_else(
            || Err(format!("compiler option `{word}` has no value")),
            |value| validate_flag_value(word, value, dirs),
        );
    }
    if let Some(value) = word.strip_prefix("-D").filter(|value| !value.is_empty()) {
        return validate_macro(value);
    }
    if let Some(value) = word.strip_prefix("-U").filter(|value| !value.is_empty()) {
        return validate_identifier(value).then_some(()).ok_or_else(|| {
            format!("compiler undefinition `{word}` is outside the closed identifier vocabulary")
        });
    }
    if let Some(path) = word.strip_prefix("-I").filter(|path| !path.is_empty()) {
        return validate_include_path(path);
    }
    if word.starts_with("--sysroot=") {
        return validate_sysroot(word.strip_prefix("--sysroot=").unwrap_or_default(), dirs);
    }
    if matches!(
        word,
        "-pipe"
            | "-pthread"
            | "-fPIC"
            | "-fpic"
            | "-fPIE"
            | "-fpie"
            | "-fno-pic"
            | "-fno-PIE"
            | "-fcommon"
            | "-fno-common"
            | "-fno-builtin"
            | "-fbuiltin"
            | "-fstrict-aliasing"
            | "-fno-strict-aliasing"
            | "-fomit-frame-pointer"
            | "-fno-omit-frame-pointer"
            | "-fstack-protector"
            | "-fstack-protector-strong"
            | "-fno-stack-protector"
            | "-fexceptions"
            | "-fno-exceptions"
            | "-frtti"
            | "-fno-rtti"
            | "-fshort-wchar"
            | "-funsigned-char"
            | "-fsigned-char"
            | "-fwrapv"
            | "-fno-wrapv"
            | "-fno-plt"
            | "-fno-inline"
            | "-fdelete-null-pointer-checks"
            | "-fno-delete-null-pointer-checks"
            | "-g"
            | "-g0"
            | "-g1"
            | "-g2"
            | "-g3"
            | "-ggdb"
            | "-ggdb0"
            | "-ggdb1"
            | "-ggdb2"
            | "-ggdb3"
            | "-O0"
            | "-O1"
            | "-O2"
            | "-O3"
            | "-Os"
            | "-Og"
            | "-Oz"
            | "-Ofast"
    ) {
        return Ok(());
    }
    if word.starts_with("-fvisibility=")
        && matches!(
            word.trim_start_matches("-fvisibility="),
            "default" | "hidden" | "internal" | "protected"
        )
    {
        return Ok(());
    }
    if let Some(std) = word.strip_prefix("-std=") {
        return (!std.is_empty()
            && std.len() <= 32
            && std
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-')))
        .then_some(())
        .ok_or_else(|| format!("standard selector `{word}` is not a safe literal"));
    }
    if let Some(warning) = word.strip_prefix("-W") {
        let lower = word.to_ascii_lowercase();
        if !matches!(lower.as_str(), "-wl" | "-wa" | "-wp")
            && !lower.starts_with("-wl,")
            && !lower.starts_with("-wa,")
            && !lower.starts_with("-wp,")
            && word.len() <= 128
            && warning.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'=' | b',' | b'+')
            })
        {
            return Ok(());
        }
    }
    if word.starts_with("-m") && safe_machine_option(word) {
        return Ok(());
    }
    Err(format!(
        "compiler argument `{word}` is outside the closed literal flag vocabulary"
    ))
}

fn validate_flag_value(option: &str, value: &str, _dirs: &DirVars) -> Result<(), String> {
    match option {
        "-D" => validate_macro(value),
        "-U" => validate_identifier(value).then_some(()).ok_or_else(|| {
            format!("compiler undefinition `{value}` is outside the closed identifier vocabulary")
        }),
        "-I" => validate_include_path(value),
        _ => Err(format!("unsupported compiler option `{option}`")),
    }
}

fn validate_macro(value: &str) -> Result<(), String> {
    let (name, definition) = value.split_once('=').unwrap_or((value, ""));
    if !validate_identifier(name) {
        return Err(format!(
            "compiler definition `{value}` has an unsafe identifier"
        ));
    }
    if !definition.is_empty()
        && (definition.len() > 128
            || !definition.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'.' | b',' | b'+' | b'/' | b'-')
            }))
    {
        return Err(format!(
            "compiler definition value `{value}` is not a safe literal"
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn validate_include_path(value: &str) -> Result<(), String> {
    let valid = [SOURCE_ROOT_ALIAS, BUILD_ROOT_ALIAS].iter().any(|prefix| {
        value == *prefix
            || value
                .strip_prefix(&format!("{prefix}/"))
                .is_some_and(|tail| validate_cmake_relative(tail).is_ok())
    });
    valid.then_some(()).ok_or_else(|| {
        format!("include directory `{value}` is outside configured source/build roots")
    })
}

fn validate_sysroot(value: &str, dirs: &DirVars) -> Result<(), String> {
    let expected = dirs
        .expand("$(AROS_DEVELOPER)")
        .filter(|path| path.starts_with(&format!("{BUILD_ROOT_ALIAS}/")));
    let Some(expected) = expected else {
        return Err("configured Developer sysroot alias is unresolved".into());
    };
    (value == expected
        || value
            .strip_prefix(&format!("{expected}/"))
            .is_some_and(|tail| validate_cmake_relative(tail).is_ok()))
    .then_some(())
    .ok_or_else(|| format!("sysroot `{value}` is not below configured Developer path `{expected}`"))
}

fn safe_machine_option(value: &str) -> bool {
    value.len() <= 128
        && value.strip_prefix("-m").is_some_and(|tail| {
            !tail.is_empty()
                && tail.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(byte, b'_' | b'-' | b'+' | b'.' | b'=' | b',')
                })
        })
        && !value.contains("..")
}

fn parse_rules(
    lines: &[&str],
    controls: &SourceControls,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<Rule>, Vec<TargetAssignment>) {
    let mut rules = Vec::<Rule>::new();
    let mut target_assignments = Vec::new();
    let mut active_rule: Option<usize> = None;
    for (line_no, raw) in lines.iter().enumerate() {
        if controls.suppressed.get(line_no).copied().unwrap_or(true) {
            active_rule = None;
            continue;
        }
        if raw.starts_with('\t') {
            if let Some(index) = active_rule {
                rules[index].recipes.push(Recipe {
                    text: (*raw).to_owned(),
                    state: line_state(line_states, controls, line_no),
                    valid: controls.valid.get(line_no).copied().unwrap_or(false),
                });
            }
            continue;
        }
        let uncommented = strip_make_comment(raw).trim();
        if uncommented.is_empty() {
            continue;
        }
        if is_make_control(uncommented) || uncommented.starts_with('#') {
            active_rule = None;
            continue;
        }
        let Some((target, rhs, double_colon, inline_recipe)) = split_rule_header(uncommented)
        else {
            active_rule = None;
            continue;
        };
        if target_specific_variable(rhs) {
            if line_state(line_states, controls, line_no) == ConditionalTruth::False {
                active_rule = None;
                continue;
            }
            target_assignments.push(TargetAssignment {
                target: target.trim().to_owned(),
                line: line_no,
            });
            active_rule = None;
            continue;
        }
        let (prerequisites, order_only, malformed_order) = split_order_only(rhs);
        rules.push(Rule {
            target: target.trim().to_owned(),
            prerequisites: prerequisites.trim().to_owned(),
            order_only: order_only.map(|value| value.trim().to_owned()),
            line: line_no,
            state: line_state(line_states, controls, line_no),
            valid: controls.valid.get(line_no).copied().unwrap_or(false) && !malformed_order,
            double_colon,
            inline_recipe: inline_recipe.map(|recipe| recipe.trim().to_owned()),
            recipes: Vec::new(),
        });
        active_rule = Some(rules.len() - 1);
    }
    (rules, target_assignments)
}

fn split_rule_header(line: &str) -> Option<(&str, &str, bool, Option<&str>)> {
    if line.starts_with('#') || line.starts_with('%') || variable_assignment(line).is_some() {
        return None;
    }
    let (header, inline) = split_top_level_once(line, ';');
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (at, character) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' | '{' if at > 0 && header.as_bytes()[at - 1] == b'$' => depth += 1,
            ')' | '}' if depth > 0 => depth -= 1,
            ':' if depth == 0 => {
                if header[at..].starts_with(":=") {
                    return None;
                }
                let double = header[at..].starts_with("::");
                let width = if double { 2 } else { 1 };
                return Some((&header[..at], &header[at + width..], double, inline));
            }
            _ => {}
        }
    }
    None
}

fn split_top_level_once(raw: &str, delimiter: char) -> (&str, Option<&str>) {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (at, character) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(mark) = quote {
            if character == mark {
                quote = None;
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            '(' | '{' if at > 0 && raw.as_bytes()[at - 1] == b'$' => depth += 1,
            ')' | '}' if depth > 0 => depth -= 1,
            current if current == delimiter && depth == 0 => {
                return (&raw[..at], Some(&raw[at + current.len_utf8()..]));
            }
            _ => {}
        }
    }
    (raw, None)
}

fn split_order_only(raw: &str) -> (&str, Option<&str>, bool) {
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut separator = None;
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at] == b'$' && matches!(bytes.get(at + 1), Some(b'(' | b'{')) {
            depth += 1;
            at += 2;
            continue;
        }
        if depth > 0 && matches!(bytes[at], b')' | b'}') {
            depth -= 1;
            at += 1;
            continue;
        }
        if bytes[at] == b'|' && depth == 0 && separator.replace(at).is_some() {
            return (raw, None, true);
        }
        at += 1;
    }
    separator.map_or((raw, None, false), |index| {
        (&raw[..index], Some(&raw[index + 1..]), false)
    })
}

fn target_specific_variable(rhs: &str) -> bool {
    let mut remaining = rhs.trim();
    loop {
        let (word, rest) = split_first_word(remaining);
        if matches!(word, "override" | "export" | "private") {
            remaining = rest;
        } else {
            break;
        }
    }
    variable_assignment(remaining).is_some()
}

fn split_first_word(raw: &str) -> (&str, &str) {
    let trimmed = raw.trim_start();
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    (&trimmed[..end], trimmed[end..].trim_start())
}

fn collect_relevant_aggregates(
    rules: &[Rule],
    target_assignments: &[TargetAssignment],
    candidates_by_output: &BTreeMap<String, Vec<usize>>,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
) -> Vec<AggregateCandidate> {
    let mut aggregates = Vec::new();
    for rule in rules {
        if rule.target.contains('%') || rule.state == ConditionalTruth::False {
            continue;
        }
        let prerequisites =
            evaluate_rule_list(&rule.prerequisites, scope, dirs, root, rel_dir, rule.line);
        let order_only = rule
            .order_only
            .as_deref()
            .map(|raw| evaluate_rule_list(raw, scope, dirs, root, rel_dir, rule.line))
            .transpose();
        let has_object_hint = rule.prerequisites.contains(".o")
            || rule
                .order_only
                .as_deref()
                .is_some_and(|raw| raw.contains(".o"));
        let (Ok(prerequisites), Ok(order_only)) = (prerequisites, order_only) else {
            if has_object_hint
                && candidates_by_output.keys().any(|output| {
                    Path::new(output)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| rule.prerequisites.contains(name))
                })
            {
                let owner = evaluate_rule_list(&rule.target, scope, dirs, root, rel_dir, rule.line)
                    .ok()
                    .filter(|targets| targets.len() == 1)
                    .map_or_else(
                        || rule.target.trim().to_owned(),
                        |targets| targets[0].clone(),
                    );
                aggregates.push(AggregateCandidate {
                    line: rule.line,
                    state: rule.state,
                    owner,
                    owner_is_safe: false,
                    prerequisites: Vec::new(),
                    order_only: Vec::new(),
                    recipe_count: rule.recipes.len(),
                    inline_recipe: rule.inline_recipe.is_some(),
                    valid: rule.valid,
                    error: Some("cannot resolve literal object aggregate prerequisites".into()),
                });
            }
            continue;
        };
        let order_only = order_only.unwrap_or_default();
        let matched = prerequisites
            .iter()
            .chain(&order_only)
            .any(|path| candidates_by_output.contains_key(path));
        if !matched {
            continue;
        }
        let targets = evaluate_rule_list(&rule.target, scope, dirs, root, rel_dir, rule.line);
        let (owner, owner_is_safe) = match targets {
            Ok(targets) if targets.len() == 1 => {
                let owner = targets[0].clone();
                (owner.clone(), safe_target_name(&owner))
            }
            _ => (rule.target.trim().to_owned(), false),
        };
        let assignment_error = !target_assignments.is_empty();
        aggregates.push(AggregateCandidate {
            line: rule.line,
            state: rule.state,
            owner,
            owner_is_safe,
            prerequisites,
            order_only,
            recipe_count: rule.recipes.len(),
            inline_recipe: rule.inline_recipe.is_some(),
            valid: rule.valid && !assignment_error,
            error: assignment_error.then(|| {
                "literal object aggregate has a target-specific Make assignment inherited by its object prerequisites".into()
            }),
        });
    }
    aggregates
}

fn assignment_matches_output(
    assignment: &TargetAssignment,
    output: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
) -> bool {
    let targets = evaluate_rule_list(
        &assignment.target,
        scope,
        dirs,
        root,
        rel_dir,
        assignment.line,
    );
    targets.map_or_else(
        |_| assignment.target.contains('%') || assignment.target.contains('$'),
        |targets| targets.iter().any(|target| target == output),
    )
}

fn evaluate_rule_list(
    expression: &str,
    scope: &VarScope,
    dirs: &DirVars,
    root: &Path,
    rel_dir: &Path,
    line: usize,
) -> Result<Vec<String>, String> {
    let lookup = |name: &str| scope.path_raw_at(name, line);
    let guard = |name: &str| variable_guard(scope, name, line);
    let context = MakeExprContext::new(scope, dirs, line, root, rel_dir)
        .with_lookup(&lookup)
        .with_guard(&guard);
    evaluate_make_list(expression, &context).map_err(|error| error.to_string())
}

fn variable_guard(scope: &VarScope, name: &str, line: usize) -> Option<String> {
    if scope.conditionally_assigned_before(name, line) {
        return Some("the effective value depends on an unresolved Make conditional".into());
    }
    if let Some(reason) = scope.flavor_uncertainty_reason_at(name, line) {
        return Some(reason);
    }
    if scope.raw_at(name, line).is_some() && scope.path_raw_at(name, line).is_none() {
        return Some("the effective value is ambiguous in the source-scoped Make history".into());
    }
    None
}

fn line_state(
    line_states: Option<&[ConditionalTruth]>,
    controls: &SourceControls,
    line: usize,
) -> ConditionalTruth {
    line_states
        .and_then(|states| states.get(line))
        .copied()
        .unwrap_or_else(|| {
            controls
                .default_states
                .get(line)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
        })
}

fn scan_source_controls(lines: &[&str]) -> SourceControls {
    #[derive(Clone, Copy)]
    struct Conditional {
        start: usize,
        seen_else: bool,
    }
    #[derive(Clone, Copy)]
    enum DefinitionEnd {
        Endef,
        PercentEnd,
    }

    let mut suppressed = vec![false; lines.len()];
    let mut valid = vec![true; lines.len()];
    let mut default_states = vec![ConditionalTruth::True; lines.len()];
    let mut definitions = Vec::<DefinitionEnd>::new();
    let mut conditionals = Vec::<Conditional>::new();
    let mut malformed_from = None::<usize>;
    for (line_no, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_start();
        if let Some(expected_end) = definitions.last().copied() {
            suppressed[line_no] = true;
            if raw.starts_with('\t') {
                valid[line_no] = malformed_from.is_none();
                continue;
            }
            if let Some(rest) = make_define_header(trimmed) {
                if !valid_define_name(rest) {
                    malformed_from.get_or_insert(line_no);
                }
                definitions.push(DefinitionEnd::Endef);
            } else if let Some(rest) = trimmed.strip_prefix("%define ") {
                if !valid_define_name(rest) {
                    malformed_from.get_or_insert(line_no);
                }
                definitions.push(DefinitionEnd::PercentEnd);
            } else if (is_endef(trimmed) && matches!(expected_end, DefinitionEnd::Endef))
                || (trimmed == "%end" && matches!(expected_end, DefinitionEnd::PercentEnd))
            {
                definitions.pop();
            }
            continue;
        }
        if raw.starts_with('\t') {
            default_states[line_no] = if conditionals.is_empty() {
                ConditionalTruth::True
            } else {
                ConditionalTruth::Unknown
            };
            valid[line_no] = malformed_from.is_none();
            continue;
        }
        let clean = strip_make_comment(trimmed).trim();
        if clean.is_empty() || clean.starts_with('#') {
            continue;
        }
        if let Some(rest) = make_define_header(clean) {
            suppressed[line_no] = true;
            if !valid_define_name(rest) {
                malformed_from.get_or_insert(line_no);
            }
            definitions.push(DefinitionEnd::Endef);
            continue;
        }
        if let Some(rest) = clean.strip_prefix("%define ") {
            suppressed[line_no] = true;
            if !valid_define_name(rest) {
                malformed_from.get_or_insert(line_no);
            }
            definitions.push(DefinitionEnd::PercentEnd);
            continue;
        }
        if is_endef(clean) || clean == "%end" {
            suppressed[line_no] = true;
            valid[line_no] = false;
            malformed_from.get_or_insert(line_no);
            continue;
        }
        default_states[line_no] = if conditionals.is_empty() {
            ConditionalTruth::True
        } else {
            ConditionalTruth::Unknown
        };
        valid[line_no] = malformed_from.is_none();
        let (word, tail) = split_first_word(clean);
        match word {
            "ifeq" | "ifneq" | "ifdef" | "ifndef" => {
                conditionals.push(Conditional {
                    start: line_no,
                    seen_else: false,
                });
            }
            "else" => {
                if !tail.is_empty() {
                    malformed_from.get_or_insert(line_no);
                    valid[line_no] = false;
                }
                if let Some(frame) = conditionals.last_mut() {
                    if frame.seen_else {
                        for item in valid.iter_mut().take(line_no + 1).skip(frame.start) {
                            *item = false;
                        }
                        malformed_from.get_or_insert(line_no);
                    }
                    frame.seen_else = true;
                } else {
                    malformed_from.get_or_insert(line_no);
                    valid[line_no] = false;
                }
            }
            "endif" if !tail.is_empty() || conditionals.pop().is_none() => {
                malformed_from.get_or_insert(line_no);
                valid[line_no] = false;
            }
            _ => {}
        }
    }
    if let Some(frame) = conditionals.first() {
        for item in valid.iter_mut().skip(frame.start) {
            *item = false;
        }
    }
    if !definitions.is_empty() {
        if let Some(start) = lines.iter().position(|line| {
            let trimmed = line.trim_start();
            make_define_header(trimmed).is_some() || trimmed.starts_with("%define ")
        }) {
            for index in start..lines.len() {
                suppressed[index] = true;
                valid[index] = false;
            }
        }
    }
    if let Some(start) = malformed_from {
        for item in valid.iter_mut().skip(start) {
            *item = false;
        }
    }
    SourceControls {
        suppressed,
        valid,
        default_states,
    }
}

fn driver_binding_issue(
    lines: &[&str],
    controls: &SourceControls,
    line_states: Option<&[ConditionalTruth]>,
    dirs: &DirVars,
) -> Option<(usize, String)> {
    for (line, raw) in lines.iter().enumerate() {
        if raw.starts_with('\t') {
            continue;
        }
        let state = line_state(line_states, controls, line);
        if state == ConditionalTruth::False {
            continue;
        }
        let clean = strip_make_comment(raw).trim();
        if controls.suppressed.get(line).copied().unwrap_or(true) {
            if make_define_header(clean)
                .and_then(define_name)
                .is_some_and(is_driver_role)
            {
                return Some((line, "source defines the target compiler role".into()));
            }
            continue;
        }
        if let Some(rest) = make_define_header(clean) {
            if define_name(rest).is_some_and(is_driver_role) {
                return Some((line, "source defines the target compiler role".into()));
            }
        }
        if let Some((name, value, kind)) = variable_assignment(clean) {
            if is_driver_role(name) {
                if state == ConditionalTruth::True
                    && name == "TARGET_CC"
                    && kind == crate::make_vars::AssignmentKind::RecursiveSet
                    && value.trim() == "$(NATIVE_TARGET_CC)"
                    && dirs.expand("$(NATIVE_TARGET_CC)").as_deref() == Some("${CMAKE_C_COMPILER}")
                {
                    continue;
                }
                return Some((
                    line,
                    format!("source locally assigns the compiler role `{name}`"),
                ));
            }
        }
        if let Ok(Some(name)) = undefine_directive(clean) {
            if is_driver_role(name) {
                return Some((
                    line,
                    format!("source locally undefines the compiler role `{name}`"),
                ));
            }
        }
        if is_include_directive(clean) {
            return Some((
                line,
                "Make include may rebind TARGET_CC/TARGET_CXX and is not resolved by this collector".into(),
            ));
        }
        if state == ConditionalTruth::Unknown
            && (clean.starts_with("define TARGET_CC")
                || clean.starts_with("define TARGET_CXX")
                || clean.starts_with("undefine TARGET_CC")
                || clean.starts_with("undefine TARGET_CXX"))
        {
            return Some((
                line,
                "compiler role mutation is in an unresolved Make conditional".into(),
            ));
        }
    }
    None
}

fn make_define_header(line: &str) -> Option<&str> {
    let mut remaining = strip_make_comment(line).trim();
    loop {
        let (word, rest) = split_first_word(remaining);
        match word {
            "define" => return Some(rest),
            "override" | "export" | "private" if !rest.is_empty() => remaining = rest,
            _ => return None,
        }
    }
}

fn define_name(raw: &str) -> Option<&str> {
    let first = raw.split_whitespace().next()?;
    let name = variable_assignment(first).map_or(first, |(name, _, _)| name);
    Some(name)
}

fn valid_define_name(raw: &str) -> bool {
    define_name(raw).is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    })
}

fn is_driver_role(name: &str) -> bool {
    matches!(name, "TARGET_CC" | "TARGET_CXX" | "NATIVE_TARGET_CC")
}

fn is_include_directive(line: &str) -> bool {
    let (word, _) = split_first_word(line);
    matches!(word, "include" | "-include" | "sinclude")
}

fn is_endef(line: &str) -> bool {
    let (word, tail) = split_first_word(strip_make_comment(line).trim());
    word == "endef" && tail.is_empty()
}

fn is_make_control(line: &str) -> bool {
    let (word, _) = split_first_word(line);
    matches!(
        word,
        "ifeq" | "ifneq" | "ifdef" | "ifndef" | "else" | "endif" | "define" | "endef"
    ) || line.starts_with("%define ")
        || line == "%end"
}

fn generated_object_hint(raw: &str) -> bool {
    !raw.contains('%')
        && (raw.contains("$(GENDIR)") || raw.contains("${GENDIR}"))
        && raw.contains(".o")
}

fn is_generated_object_path(path: &str, generated_root: &str) -> bool {
    path.strip_prefix(&format!("{generated_root}/"))
        .is_some_and(|relative| Path::new(relative).extension() == Some(std::ffi::OsStr::new("o")))
}

fn validate_local_object_output(output: &str, local_generated: &str) -> Result<(), String> {
    let relative = output
        .strip_prefix(&format!("{local_generated}/"))
        .ok_or_else(|| {
            "literal object target is not below the local `GENDIR` directory".to_owned()
        })?;
    validate_cmake_relative(relative)?;
    if Path::new(relative).extension() != Some(std::ffi::OsStr::new("o")) {
        return Err("literal object target must end in `.o`".into());
    }
    Ok(())
}

fn local_generated_root(generated_root: &str, rel_dir: &Path) -> Result<String, String> {
    let relative = validate_relative_directory(&path_text(rel_dir))?;
    if relative.as_os_str().is_empty() {
        Ok(generated_root.to_owned())
    } else {
        Ok(format!("{generated_root}/{}", path_text(&relative)))
    }
}

fn source_path_from_alias(path: &str) -> Result<&str, String> {
    path.strip_prefix(&format!("{SOURCE_ROOT_ALIAS}/"))
        .ok_or_else(|| "literal object prerequisite must be relative to configured `SRCDIR`".into())
}

fn safe_source_file(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("selected source root cannot be canonicalized: {error}"))?;
    if !canonical_root.is_dir() {
        return Err("selected source root is not a directory".into());
    }
    let relative = validate_relative_path(&path_text(relative))?;
    let components = relative.components().collect::<Vec<_>>();
    let mut current = canonical_root.clone();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err("source path contains a non-normal path component".into());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!(
                "source prerequisite `{}` is unavailable: {error}",
                current.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "source prerequisite `{}` has a symlink component",
                current.display()
            ));
        }
        let is_last = index + 1 == components.len();
        if is_last && !metadata.is_file() {
            return Err(
                "literal object prerequisite must be an existing regular source file".into(),
            );
        }
        if !is_last && !metadata.is_dir() {
            return Err("literal object source path crosses a non-directory component".into());
        }
    }
    let canonical_file = current
        .canonicalize()
        .map_err(|error| format!("source prerequisite cannot be canonicalized: {error}"))?;
    if !canonical_file.starts_with(&canonical_root) {
        return Err("literal object source escapes the selected source root".into());
    }
    Ok(canonical_file)
}

fn validate_relative_directory(value: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value == "." {
        return Ok(PathBuf::new());
    }
    validate_relative_path(value)
}

fn validate_relative_path(value: &str) -> Result<PathBuf, String> {
    if value.is_empty() || value.starts_with('/') || value.contains('\\') {
        return Err("path is not a non-empty slash-separated relative path".into());
    }
    let path = Path::new(value);
    if path.components().any(|component| {
        !matches!(component, Component::Normal(_))
            || !safe_path_component(component.as_os_str().to_string_lossy().as_ref())
    }) {
        return Err("path contains an unsafe or non-normal component".into());
    }
    Ok(path.to_path_buf())
}

fn validate_cmake_relative(value: &str) -> Result<(), String> {
    validate_relative_path(value).map(|_| ())
}

fn safe_path_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'+' | b'.'))
}

fn safe_target_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn source_file_name(rel_dir: &Path) -> Result<String, String> {
    let relative = validate_relative_directory(&path_text(rel_dir))?;
    let mut file = relative;
    file.push("mmakefile.src");
    Ok(path_text(&file))
}

fn file_name(rel_dir: &Path) -> String {
    let mut path = rel_dir.to_path_buf();
    path.push("mmakefile.src");
    path_text(&path)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn contains_shell_syntax(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character,
            '\'' | '"' | '`' | ';' | '|' | '&' | '<' | '>' | '\\' | '*' | '?' | '[' | ']' | '#'
        )
    })
}

fn has_unsupported_dollar_reference(value: &str) -> bool {
    value
        .replace("${AROS_BUILD_DIR}", "")
        .replace("${AROS_SOURCE_DIR}", "")
        .contains('$')
}

fn is_echo_recipe(raw: &str) -> bool {
    let command = raw.trim_start_matches('\t').trim();
    let Some(command) = command.strip_prefix("@$(ECHO) ") else {
        return false;
    };
    command == "\"Compiling  $<\"" || command == "\"Compiling $<\""
}

fn rejection(
    owner: &str,
    file: &str,
    line: usize,
    reason: impl Into<String>,
) -> LiteralObjectRejection {
    LiteralObjectRejection {
        owner: if owner.is_empty() { "<unknown>" } else { owner }.to_owned(),
        file: file.to_owned(),
        line: line + 1,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dirs::DirVars;
    use crate::make_vars::collect_vars;
    use std::fs;
    use tempfile::tempdir;

    fn fixture(
        source: &str,
        symlink_source: bool,
    ) -> (Vec<LiteralObjectGroupDecl>, Vec<LiteralObjectRejection>) {
        let temp = tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("compiler/libinit")).unwrap();
        fs::write(
            root.join("compiler/libinit/real.c"),
            "int entry(void) { return 0; }\n",
        )
        .unwrap();
        if symlink_source {
            #[cfg(unix)]
            std::os::unix::fs::symlink(
                root.join("compiler/libinit/real.c"),
                root.join("compiler/libinit/entry.c"),
            )
            .unwrap();
        } else {
            fs::write(
                root.join("compiler/libinit/entry.c"),
                "int entry(void) { return 0; }\n",
            )
            .unwrap();
        }
        let joined = crate::parser::join_continuations(source);
        let scope = collect_vars(&joined);
        let dirs = DirVars::load(root);
        collect_from_snapshot(
            &scope,
            &dirs,
            root,
            Path::new("compiler/libinit"),
            None,
            &joined,
        )
    }

    fn source(recipe: &str) -> String {
        format!(
            "CFLAGS := -O0\n\
             $(GENDIR)/$(CURDIR)/entry.o: $(SRCDIR)/$(CURDIR)/entry.c | $(GENDIR)/$(CURDIR)\n\
             \t{recipe}\n\
             CFLAGS := -O2 -DORDER=first -DORDER=second -UOLD -mabi=lp64d\n\
             entry-owner : $(GENDIR)/compiler/libinit/entry.o\n"
        )
    }

    #[test]
    fn accepts_warning_flags_but_not_driver_forwarding() {
        let (groups, errors) = fixture(
            &source("@$(TARGET_CC) -Wall -Werror -Wno-pointer-sign -Wno-parentheses -c $< -o $@"),
            false,
        );
        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(
            groups[0].objects[0].arguments,
            ["-Wall", "-Werror", "-Wno-pointer-sign", "-Wno-parentheses"]
        );
        for unsafe_flag in ["-Wa,option", "-Wp,option", "-Wl,option"] {
            let (groups, errors) = fixture(
                &source(&format!("@$(TARGET_CC) {unsafe_flag} -c $< -o $@")),
                false,
            );
            assert!(groups.is_empty());
            assert!(!errors.is_empty());
        }
    }

    #[test]
    fn caller_target_specific_flags_cannot_be_discarded() {
        let text = format!(
            "{}\nparent : CFLAGS = -DCALLER\nparent : entry-owner\n",
            source("@$(TARGET_CC) $(CFLAGS) -c $< -o $@")
        );
        let (groups, errors) = fixture(&text, false);
        assert!(groups.is_empty());
        assert!(
            errors
                .iter()
                .any(|error| error.reason.contains("target-specific")),
            "{errors:#?}"
        );
    }

    #[test]
    fn preserves_duplicate_flag_order_and_uses_final_assignment() {
        let (groups, rejections) = fixture(&source("@$(TARGET_CC) $(CFLAGS) -c $< -o $@"), false);
        assert!(rejections.is_empty(), "{rejections:#?}");
        assert_eq!(groups.len(), 1, "{groups:#?}");
        let object = &groups[0].objects[0];
        assert_eq!(object.source, "${AROS_SOURCE_DIR}/compiler/libinit/entry.c");
        assert_eq!(object.language, "C");
        assert_eq!(
            object.arguments,
            [
                "-O2",
                "-DORDER=first",
                "-DORDER=second",
                "-UOLD",
                "-mabi=lp64d"
            ]
        );
    }

    #[test]
    fn unknown_flags_are_not_treated_as_empty() {
        let (groups, rejections) =
            fixture(&source("@$(TARGET_CC) $(UNKNOWN_FLAGS) -c $< -o $@"), false);
        assert!(groups.is_empty());
        assert!(
            rejections.iter().any(|item| {
                item.reason
                    .contains("cannot resolve final literal compiler flags")
                    && item.reason.contains("UNKNOWN_FLAGS")
            }),
            "{rejections:#?}"
        );
    }

    #[test]
    fn admitted_target_role_is_explicit_and_cannot_be_rebound() {
        let source = "TARGET_CC = $(NATIVE_TARGET_CC)\n";
        let lines = source.lines().collect::<Vec<_>>();
        let controls = scan_source_controls(&lines);
        let root = tempdir().unwrap();
        let mut dirs = DirVars::load(root.path());
        assert!(driver_binding_issue(&lines, &controls, None, &dirs).is_some());
        dirs.bind_native_target_tool_roles();
        assert!(driver_binding_issue(&lines, &controls, None, &dirs).is_none());
        let changed = format!("{source}NATIVE_TARGET_CC := wrapper\n");
        let changed_lines = changed.lines().collect::<Vec<_>>();
        assert!(driver_binding_issue(
            &changed_lines,
            &scan_source_controls(&changed_lines),
            None,
            &dirs
        )
        .is_some());
        let states = [ConditionalTruth::Unknown];
        assert!(driver_binding_issue(&lines, &controls, Some(&states), &dirs).is_some());
    }

    #[test]
    fn rejects_local_driver_rebinding_and_includes_that_may_rebind_it() {
        for prefix in [
            "TARGET_CC := wrapper\n",
            "define TARGET_CC\nwrapper\nendef\n",
            "undefine TARGET_CXX\n",
            "include unresolved-fragment.mk\n",
        ] {
            let (groups, rejections) = fixture(
                &format!("{prefix}{}", source("@$(TARGET_CC) -c $< -o $@")),
                false,
            );
            assert!(groups.is_empty(), "{prefix}");
            assert!(!rejections.is_empty(), "{prefix}");
        }
    }

    #[test]
    fn rejects_source_symlink_components() {
        let (groups, rejections) = fixture(&source("@$(TARGET_CC) -c $< -o $@"), true);
        #[cfg(unix)]
        {
            assert!(groups.is_empty());
            assert!(rejections
                .iter()
                .any(|item| item.reason.contains("symlink component")));
        }
        #[cfg(not(unix))]
        {
            let _ = (groups, rejections);
        }
    }

    #[test]
    fn rejects_duplicate_output_and_conflicting_aggregate_owner() {
        let duplicate = source("@$(TARGET_CC) -c $< -o $@").replace(
            "entry-owner :",
            "$(GENDIR)/$(CURDIR)/entry.o: $(SRCDIR)/$(CURDIR)/entry.c\n\
             \t@$(TARGET_CC) -c $< -o $@\n\
             entry-owner :",
        );
        let (groups, rejections) = fixture(&duplicate, false);
        assert!(groups.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("more than one literal producer")));

        let conflicting = source("@$(TARGET_CC) -c $< -o $@").replace(
            "entry-owner :",
            "other-owner : $(GENDIR)/compiler/libinit/entry.o\nentry-owner :",
        );
        let (groups, rejections) = fixture(&conflicting, false);
        assert!(groups.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("conflicting aggregate owners")));
    }

    #[test]
    fn define_bodies_and_false_conditionals_are_inert() {
        let body = source("@$(TARGET_CC) -c $< -o $@");
        let joined_source =
            format!("define HIDDEN\n{body}\nendef\nifeq (a,b)\n{body}\nendif\n{body}");
        let temp = tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("compiler/libinit")).unwrap();
        fs::write(root.join("compiler/libinit/entry.c"), "int entry(void);\n").unwrap();
        let joined = crate::parser::join_continuations(&joined_source);
        let dirs = DirVars::load(root);
        let scope = crate::make_vars::collect_vars(&joined);
        let mut states = vec![ConditionalTruth::True; joined.lines().count()];
        // The target-agnostic variable scanner deliberately treats conditions
        // as unknown; this known-false branch exercises the collector's state
        // input in the same way the pipeline supplies target-evaluated states.
        let false_branch_start = joined
            .lines()
            .position(|line| line == "ifeq (a,b)")
            .unwrap();
        let false_branch_end = joined.lines().position(|line| line == "endif").unwrap();
        states[false_branch_start..=false_branch_end].fill(ConditionalTruth::False);
        let (groups, rejections) = collect_from_snapshot(
            &scope,
            &dirs,
            root,
            Path::new("compiler/libinit"),
            Some(&states),
            &joined,
        );
        assert_eq!(groups.len(), 1, "{groups:#?}\n{rejections:#?}");
        assert!(rejections.is_empty(), "{rejections:#?}");
    }
}
