//! Bounded projection of the generic header copies in `compiler/include`.
//!
//! This collector proves only the finite, source-local generic `.h` and
//! `.hpp` copies selected by the Makefile's `INCLUDES` expression. It does not
//! claim the complete `compiler-includes` target: unbound Make includes,
//! `setup` and `includes-execbase_h` are deliberately retained as unresolved
//! prerequisites, and architecture/family headers are used only to exclude
//! generic names, never as inferred producers.

use crate::dirs::DirVars;
use crate::genmodule_header_rules::{logical_lines, parse_rules, Rule};
use crate::make_expr::{evaluate_make_list, MakeExprContext};
use crate::make_vars::{strip_make_comment, variable_assignment, ConditionalTruth, VarScope};
use crate::parser::TargetContext;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path};

const MAX_MAKEFILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_OUTPUTS: usize = 8192;
const MAX_HEADER_PATH_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayeredHeaderCopyDecl {
    pub owner: String,
    pub file: String,
    /// One-based line containing the owner target.
    pub line: usize,
    /// Copies are in source Make prerequisite order: all SDK outputs, then all
    /// generated-include outputs. `source_relative` is relative to the source
    /// root, and each output uses a stable CMake include-root alias.
    pub copies: Vec<LayeredHeaderCopy>,
    /// Unique source-root-relative headers in the order selected by INCLUDES.
    pub inputs: Vec<String>,
    /// These prerequisites participate in the Make owner but are not claimed
    /// as produced by this partial capability.
    pub unresolved_prerequisites: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayeredHeaderCopy {
    pub source_relative: String,
    pub output_alias: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rejection {
    pub(crate) owner: Option<String>,
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) reason: String,
}

#[derive(Debug)]
struct CandidateResult {
    declaration: Option<LayeredHeaderCopyDecl>,
    outputs: Vec<String>,
    rejection: Option<Rejection>,
}

const COPY_RULES: [(&str, &str, &str); 4] = [
    (
        "$(AROS_INCLUDES)/%.h",
        "$(SRCDIR)/$(CURDIR)/%.h",
        "@$(ECHO) \"Copying    C   includes to $(AROS_INCLUDES)...\"",
    ),
    (
        "$(GENINCDIR)/%.h",
        "$(SRCDIR)/$(CURDIR)/%.h",
        "@$(ECHO) \"Copying    C   includes to $(GENINCDIR)...\"",
    ),
    (
        "$(AROS_INCLUDES)/%.hpp",
        "$(SRCDIR)/$(CURDIR)/%.hpp",
        "@$(ECHO) \"Copying    C++ includes to $(AROS_INCLUDES)...\"",
    ),
    (
        "$(GENINCDIR)/%.hpp",
        "$(SRCDIR)/$(CURDIR)/%.hpp",
        "@$(ECHO) \"Copying    C++ includes to $(GENINCDIR)...\"",
    ),
];

/// Collects finite generic include copies from a continuation-joined
/// source-owned Makefile. `line_states` uses zero-based lines in `content`.
/// Unknown or unavailable conditional states fail closed for a candidate.
pub(crate) fn collect(
    content: &str,
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
    target: &TargetContext,
    line_states: Option<&[ConditionalTruth]>,
) -> (Vec<LayeredHeaderCopyDecl>, Vec<Rejection>) {
    let relative_directory = match safe_relative_directory(relative_dir) {
        Ok(value) => value,
        Err(reason) => {
            return (Vec::new(), vec![rejection(None, relative_dir, 0, reason)]);
        }
    };
    if content.len() > MAX_MAKEFILE_BYTES {
        return (
            Vec::new(),
            vec![rejection(
                None,
                relative_dir,
                0,
                "layered header-copy Makefile exceeds the 4 MiB scan limit",
            )],
        );
    }

    let lines = logical_lines(content, line_states);
    let rules = parse_rules(lines);
    let candidates = rules
        .iter()
        .filter(|rule| {
            rule.state != ConditionalTruth::False
                && rule.prerequisites.contains("$(DEST_INCLUDES)")
                && rule.prerequisites.contains("$(GEN_INCLUDES)")
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let source_root = match source_root.canonicalize() {
        Ok(root) if root.is_dir() => root,
        Ok(_) => {
            return rejected_candidates(
                &candidates,
                &rules,
                relative_dir,
                "selected source root is not a directory",
            );
        }
        Err(error) => {
            return rejected_candidates(
                &candidates,
                &rules,
                relative_dir,
                format!("selected source root cannot be resolved: {error}"),
            );
        }
    };
    let makefile_relative = if relative_directory.is_empty() {
        "mmakefile.src".to_owned()
    } else {
        format!("{relative_directory}/mmakefile.src")
    };
    if let Err(reason) = validate_source_file(&source_root, Path::new(&makefile_relative)) {
        return rejected_candidates(
            &candidates,
            &rules,
            relative_dir,
            format!("source Makefile: {reason}"),
        );
    }
    let physical_source = match fs::read_to_string(source_root.join(&makefile_relative)) {
        Ok(source) => source,
        Err(error) => {
            return rejected_candidates(
                &candidates,
                &rules,
                relative_dir,
                format!("source Makefile cannot be read: {error}"),
            );
        }
    };
    let physical_joined = crate::parser::join_continuations(&physical_source);
    let bound_source = crate::local_make_includes::inline_native_make_configuration_with_templates(
        &physical_joined,
        &source_root,
        Path::new(&makefile_relative),
        crate::local_make_includes::LocalMakeIncludeLimits::default(),
        &target.make_include_bindings,
        &target.generated_make_templates,
    );
    let supplied_joined = crate::parser::join_continuations(content);
    let source_matches =
        if target.make_include_bindings.is_empty() && target.generated_make_templates.is_empty() {
            supplied_joined == physical_joined
        } else {
            bound_source.issues.is_empty()
                && supplied_joined == crate::parser::join_continuations(&bound_source.expanded)
        };
    if !source_matches {
        return rejected_candidates(
            &candidates,
            &rules,
            relative_dir,
            "supplied Makefile text differs from the source-owned mmakefile or its bound native-configuration expansion",
        );
    }

    if let Some(reason) = unsupported_make_construct(content) {
        return rejected_candidates(&candidates, &rules, relative_dir, reason);
    }

    let unresolved_includes = unresolved_make_includes(
        content,
        &source_root,
        Path::new(&makefile_relative),
        &target.make_include_bindings,
    );

    let mut results = Vec::new();
    for rule in &candidates {
        results.push(scan_owner(
            rule,
            &rules,
            content,
            scope,
            dirs,
            &source_root,
            relative_dir,
            &relative_directory,
            &makefile_relative,
            target,
            line_states,
            &unresolved_includes,
        ));
    }

    let mut output_owners: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, result) in results.iter().enumerate() {
        for output in &result.outputs {
            output_owners
                .entry(output.to_ascii_lowercase())
                .or_default()
                .push(index);
        }
    }
    for (output, indexes) in output_owners {
        let unique = indexes.iter().copied().collect::<HashSet<_>>();
        if unique.len() < 2 {
            continue;
        }
        for index in unique {
            let result = &mut results[index];
            let owner = result
                .declaration
                .as_ref()
                .map(|declaration| declaration.owner.clone())
                .or_else(|| {
                    result
                        .rejection
                        .as_ref()
                        .and_then(|item| item.owner.clone())
                });
            let line = result
                .declaration
                .as_ref()
                .map(|declaration| declaration.line)
                .or_else(|| result.rejection.as_ref().map(|item| item.line))
                .unwrap_or_default();
            result.declaration = None;
            result.rejection = Some(rejection(
                owner.as_deref(),
                relative_dir,
                line,
                format!("duplicate generic header-copy output `{output}`"),
            ));
        }
    }

    let mut declarations = Vec::new();
    let mut rejections = Vec::new();
    for result in results {
        if let Some(rejection) = result.rejection {
            rejections.push(rejection);
        } else if let Some(declaration) = result.declaration {
            declarations.push(declaration);
        }
    }
    (declarations, rejections)
}

#[allow(clippy::too_many_arguments)]
fn scan_owner(
    rule: &Rule,
    rules: &[Rule],
    content: &str,
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
    relative_directory: &str,
    file: &str,
    target: &TargetContext,
    line_states: Option<&[ConditionalTruth]>,
    unresolved_includes: &[String],
) -> CandidateResult {
    let marker = find_mm_owner_marker(content, rule.line);
    let owner = safe_owner_name(rule.target.trim()).then(|| rule.target.trim().to_owned());
    let lookup = |name: &str| target.value_of(name);
    let makefile_context = MakeExprContext::new(scope, dirs, rule.line, source_root, relative_dir)
        .with_lookup(&lookup);
    let mut outputs = Vec::new();
    let result = (|| -> Result<LayeredHeaderCopyDecl, String> {
        if marker.is_none() {
            return Err(
                "include-list target is not immediately owned by a source `#MM` marker".into(),
            );
        }
        if owner.is_none() {
            return Err("generic include owner is not one safe literal Make target".into());
        }
        let owner = owner.as_ref().expect("owner checked above").clone();
        let marker_line = marker.expect("marker checked above");
        let marker_state = line_states.map_or(ConditionalTruth::True, |states| {
            states
                .get(marker_line)
                .copied()
                .unwrap_or(ConditionalTruth::Unknown)
        });
        if marker_state != ConditionalTruth::True {
            return Err("generic include owner marker is not in a proven active branch".into());
        }
        if !ordinary_unconditional(rule)
            || !rule.recipes.is_empty()
            || rule.prerequisites.contains('|')
        {
            return Err(
                "generic include owner must be unconditional, dependency-only, and have no order-only prerequisites".into(),
            );
        }
        let same_owner_count = rules
            .iter()
            .filter(|candidate| {
                candidate.state != ConditionalTruth::False
                    && candidate.target.trim() == rule.target.trim()
            })
            .count();
        if same_owner_count != 1 {
            return Err("generic include owner has multiple active Make rule blocks".into());
        }
        if scope.is_known_local("CP")
            || scope.conditionally_assigned_before("CP", usize::MAX)
            || scope.is_known_local("ECHO")
            || scope.conditionally_assigned_before("ECHO", usize::MAX)
            || scope.is_known_local("SHELL")
            || scope.conditionally_assigned_before("SHELL", usize::MAX)
            || scope.is_known_local("RECIPEPREFIX")
        {
            return Err(
                "source-local CP/ECHO/SHELL/RECIPEPREFIX overrides make the copy recipe uncertain"
                    .into(),
            );
        }
        if rules
            .iter()
            .filter(|candidate| candidate.state != ConditionalTruth::False)
            .any(target_specific_control)
        {
            return Err(
                "target-specific CP/ECHO/SHELL/RECIPEPREFIX assignment affects a copy recipe"
                    .into(),
            );
        }

        for variable in [
            "INCSUBDIRS",
            "INCLUDES_BASE",
            "INCLUDES",
            "ARCHINCDIR",
            "ARCHFAMILYINCDIR",
            "ARCH_INCLUDES",
            "ARCHFAMILY_INCLUDES",
            "DEST_INCLUDES",
            "GEN_INCLUDES",
        ] {
            if scope.conditionally_assigned_before(variable, rule.line)
                || scope
                    .flavor_uncertainty_reason_at(variable, rule.line)
                    .is_some()
            {
                return Err(format!(
                    "Make variable `{variable}` has conditional or flavor uncertainty"
                ));
            }
        }

        let includes_base_paths = evaluate_variable_list("INCLUDES_BASE", &makefile_context)
            .map_err(|reason| format!("cannot evaluate INCLUDES_BASE: {reason}"))?;
        let source_prefix = if relative_directory.is_empty() {
            "${AROS_SOURCE_DIR}/".to_owned()
        } else {
            format!("${{AROS_SOURCE_DIR}}/{relative_directory}/")
        };
        let includes_base = includes_base_paths
            .iter()
            .map(|path| {
                path.strip_prefix(&source_prefix)
                    .unwrap_or(path.as_str())
                    .to_owned()
            })
            .collect::<Vec<_>>();
        let includes = evaluate_variable_list("INCLUDES", &makefile_context)
            .map_err(|reason| format!("cannot evaluate INCLUDES: {reason}"))?;
        let arch_includes = evaluate_variable_list("ARCH_INCLUDES", &makefile_context)
            .map_err(|reason| format!("cannot evaluate ARCH_INCLUDES: {reason}"))?;
        let family_includes = evaluate_variable_list("ARCHFAMILY_INCLUDES", &makefile_context)
            .map_err(|reason| format!("cannot evaluate ARCHFAMILY_INCLUDES: {reason}"))?;
        for name in [
            "INCLUDES_BASE",
            "INCLUDES",
            "ARCH_INCLUDES",
            "ARCHFAMILY_INCLUDES",
        ] {
            if evaluate_variable_list(name, &makefile_context)
                .map_err(|reason| format!("cannot evaluate {name}: {reason}"))?
                .len()
                > MAX_OUTPUTS
            {
                return Err(format!(
                    "{name} exceeds the {MAX_OUTPUTS}-header scan limit"
                ));
            }
        }
        validate_header_list("INCLUDES_BASE", &includes_base)?;
        validate_header_list("ARCH_INCLUDES", &arch_includes)?;
        validate_header_list("ARCHFAMILY_INCLUDES", &family_includes)?;
        validate_header_list("INCLUDES", &includes)?;
        if includes.is_empty() {
            return Err("INCLUDES has no generic header outputs".into());
        }

        let arch_names = arch_includes.iter().cloned().collect::<HashSet<_>>();
        let family_names = family_includes.iter().cloned().collect::<HashSet<_>>();
        let expected_generic = includes_base
            .iter()
            .filter(|header| !arch_names.contains(*header) && !family_names.contains(*header))
            .cloned()
            .collect::<Vec<_>>();
        if includes != expected_generic {
            return Err(
                "INCLUDES is not the ordered generic-only result of filtering ARCH and FAMILY names from INCLUDES_BASE".into(),
            );
        }
        for header in &includes_base {
            validate_source_file(source_root, &relative_header_path(relative_dir, header)?)?;
        }
        let arch_directory = evaluated_source_directory("$(ARCHINCDIR)", &makefile_context)?;
        let family_directory =
            evaluated_source_directory("$(ARCHFAMILYINCDIR)", &makefile_context)?;
        for header in &arch_includes {
            validate_source_file(source_root, &relative_header_path(&arch_directory, header)?)?;
        }
        for header in &family_includes {
            validate_source_file(
                source_root,
                &relative_header_path(&family_directory, header)?,
            )?;
        }

        let sdk_root = crate::copy_directories::render_copy_directory_path(
            "$(AROS_INCLUDES)",
            &makefile_context,
            relative_dir,
        )
        .map_err(|reason| format!("AROS_INCLUDES root: {reason}"))?;
        let generated_root = crate::copy_directories::render_copy_directory_path(
            "$(GENINCDIR)",
            &makefile_context,
            relative_dir,
        )
        .map_err(|reason| format!("GENINCDIR root: {reason}"))?;
        if sdk_root != "${AROS_SDK_INCLUDE_DIR}" || generated_root != "${AROS_GENINC_DIR}" {
            return Err(format!(
                "include roots are not the expected stable SDK/generated aliases: `{sdk_root}`, `{generated_root}`"
            ));
        }

        let destinations = evaluate_variable_list("DEST_INCLUDES", &makefile_context)
            .map_err(|reason| format!("cannot evaluate DEST_INCLUDES: {reason}"))?;
        let generated = evaluate_variable_list("GEN_INCLUDES", &makefile_context)
            .map_err(|reason| format!("cannot evaluate GEN_INCLUDES: {reason}"))?;
        let expected_destinations = includes
            .iter()
            .map(|header| format!("$(AROS_INCLUDES)/{header}"))
            .collect::<Vec<_>>();
        let expected_generated = includes
            .iter()
            .map(|header| format!("$(GENINCDIR)/{header}"))
            .collect::<Vec<_>>();
        let resolved_expected_destinations = expected_destinations
            .iter()
            .map(|value| evaluate_single(value, &makefile_context))
            .collect::<Result<Vec<_>, _>>()?;
        let resolved_expected_generated = expected_generated
            .iter()
            .map(|value| evaluate_single(value, &makefile_context))
            .collect::<Result<Vec<_>, _>>()?;
        if destinations != resolved_expected_destinations
            || generated != resolved_expected_generated
        {
            return Err(
                "DEST_INCLUDES or GEN_INCLUDES does not exactly map the generic INCLUDES list"
                    .into(),
            );
        }
        outputs.extend(destinations.iter().cloned());
        outputs.extend(generated.iter().cloned());
        if outputs.len() > MAX_OUTPUTS.saturating_mul(2) {
            return Err("generic include outputs exceed the bounded copy limit".into());
        }
        ensure_unique_case_insensitive(&outputs, "output")?;

        let mut expected_prerequisites = vec!["setup".to_owned()];
        expected_prerequisites.extend(destinations.iter().cloned());
        expected_prerequisites.extend(generated.iter().cloned());
        expected_prerequisites.push("includes-execbase_h".to_owned());
        let actual_prerequisites = evaluate_make_list(&rule.prerequisites, &makefile_context)
            .map_err(|error| {
                format!("cannot evaluate generic include owner prerequisites: {error}")
            })?;
        if actual_prerequisites != expected_prerequisites {
            return Err("owner prerequisites do not exactly preserve setup, generic outputs, and unresolved execbase ordering".into());
        }

        validate_copy_rules(rules)?;
        reject_competing_producers(
            rules,
            &outputs,
            scope,
            dirs,
            source_root,
            relative_dir,
            target,
        )?;

        let mut copies = Vec::with_capacity(outputs.len());
        for header in &includes {
            let source_relative = if relative_directory.is_empty() {
                header.clone()
            } else {
                format!("{relative_directory}/{header}")
            };
            copies.push(LayeredHeaderCopy {
                source_relative: source_relative.clone(),
                output_alias: format!("${{AROS_SDK_INCLUDE_DIR}}/{header}"),
            });
        }
        for header in &includes {
            let source_relative = if relative_directory.is_empty() {
                header.clone()
            } else {
                format!("{relative_directory}/{header}")
            };
            copies.push(LayeredHeaderCopy {
                source_relative,
                output_alias: format!("${{AROS_GENINC_DIR}}/{header}"),
            });
        }
        let inputs = includes
            .iter()
            .map(|header| {
                if relative_directory.is_empty() {
                    header.clone()
                } else {
                    format!("{relative_directory}/{header}")
                }
            })
            .collect::<Vec<_>>();
        ensure_unique_case_insensitive(&inputs, "input")?;

        Ok(LayeredHeaderCopyDecl {
            owner,
            file: file.to_owned(),
            line: rule.line + 1,
            copies,
            inputs,
            unresolved_prerequisites: unresolved_includes
                .iter()
                .cloned()
                .chain(["setup".into(), "includes-execbase_h".into()])
                .collect(),
        })
    })();

    match result {
        Ok(declaration) => CandidateResult {
            outputs,
            declaration: Some(declaration),
            rejection: None,
        },
        Err(reason) => CandidateResult {
            outputs,
            declaration: None,
            rejection: Some(rejection(
                owner.as_deref(),
                relative_dir,
                rule.line + 1,
                reason,
            )),
        },
    }
}

fn evaluate_variable_list(
    variable: &str,
    context: &MakeExprContext<'_>,
) -> Result<Vec<String>, String> {
    evaluate_make_list(&format!("$({variable})"), context).map_err(|error| error.to_string())
}

fn evaluate_single(value: &str, context: &MakeExprContext<'_>) -> Result<String, String> {
    let resolved = evaluate_make_list(value, context).map_err(|error| error.to_string())?;
    if resolved.len() != 1 {
        return Err(format!("`{value}` does not resolve to exactly one path"));
    }
    Ok(resolved.into_iter().next().expect("one result checked"))
}

fn evaluated_source_directory(
    variable: &str,
    context: &MakeExprContext<'_>,
) -> Result<std::path::PathBuf, String> {
    let value = evaluate_single(variable, context)?;
    let relative = value
        .strip_prefix("${AROS_SOURCE_DIR}/")
        .ok_or_else(|| format!("`{variable}` is not rooted below the source tree"))?
        .trim_end_matches('/');
    let relative = safe_relative_directory(Path::new(relative))?;
    Ok(Path::new(&relative).to_path_buf())
}

fn validate_copy_rules(rules: &[Rule]) -> Result<(), String> {
    for (target, prerequisites, echo) in COPY_RULES {
        let matches = rules
            .iter()
            .filter(|rule| rule.state != ConditionalTruth::False && rule.target.trim() == target)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(format!(
                "copy pattern `{target}` has {} active Make rule blocks",
                matches.len()
            ));
        }
        let rule = matches[0];
        if !ordinary_unconditional(rule)
            || rule.continued
            || rule.prerequisites.contains('|')
            || rule.prerequisites.trim() != prerequisites
        {
            return Err(format!(
                "copy pattern `{target}` is guarded or has unsupported prerequisites"
            ));
        }
        if rule.recipes.len() != 2
            || rule.recipes[0].text != echo
            || rule.recipes[1].text != "@$(CP) $< $@"
            || rule
                .recipes
                .iter()
                .any(|recipe| recipe.state != ConditionalTruth::True || recipe.conditional_syntax)
        {
            return Err(format!(
                "copy pattern `{target}` must have exactly the recognized log and `@$(CP) $< $@` recipes"
            ));
        }
    }
    Ok(())
}

fn reject_competing_producers(
    rules: &[Rule],
    outputs: &[String],
    scope: &VarScope,
    dirs: &DirVars,
    source_root: &Path,
    relative_dir: &Path,
    target: &TargetContext,
) -> Result<(), String> {
    for rule in rules {
        if rule.state == ConditionalTruth::False
            || COPY_RULES
                .iter()
                .any(|(expected, _, _)| rule.target.trim() == *expected)
        {
            continue;
        }
        let raw = rule.target.trim();
        if !raw.contains("$(AROS_INCLUDES)") && !raw.contains("$(GENINCDIR)") {
            continue;
        }
        let lookup = |name: &str| target.value_of(name);
        let context = MakeExprContext::new(scope, dirs, rule.line, source_root, relative_dir)
            .with_lookup(&lookup);
        let targets = match evaluate_make_list(raw, &context) {
            Ok(targets) => targets,
            Err(error) => {
                return Err(format!(
                    "cannot rule out an overlapping include-root producer `{raw}`: {error}"
                ));
            }
        };
        for candidate in targets {
            if outputs
                .iter()
                .any(|output| target_pattern_matches(&candidate, output))
            {
                return Err(format!(
                    "another active Make rule `{}` can produce generic include output `{candidate}`",
                    rule.target.trim()
                ));
            }
        }
    }
    Ok(())
}

fn target_pattern_matches(pattern: &str, output: &str) -> bool {
    if !pattern.contains('%') {
        return pattern == output;
    }
    let Some((prefix, suffix)) = pattern.split_once('%') else {
        return false;
    };
    !suffix.contains('%')
        && output.len() >= prefix.len() + suffix.len()
        && output.starts_with(prefix)
        && output.ends_with(suffix)
}

fn target_specific_control(rule: &Rule) -> bool {
    let assignment = rule.prerequisites.trim();
    let mut value = assignment;
    for _ in 0..3 {
        let mut parts = value.splitn(2, char::is_whitespace);
        let modifier = parts.next().unwrap_or_default();
        if !matches!(modifier, "override" | "export" | "private") {
            break;
        }
        value = parts.next().unwrap_or_default().trim();
    }
    variable_assignment(value)
        .is_some_and(|(name, _, _)| matches!(name, "CP" | "ECHO" | "SHELL" | "RECIPEPREFIX"))
}

fn ordinary_unconditional(rule: &Rule) -> bool {
    rule.state == ConditionalTruth::True
        && !rule.conditional_syntax
        && rule
            .recipes
            .iter()
            .all(|recipe| recipe.state == ConditionalTruth::True && !recipe.conditional_syntax)
}

fn find_mm_owner_marker(content: &str, target_line: usize) -> Option<usize> {
    let lines = content.lines().collect::<Vec<_>>();
    for (index, raw) in lines.iter().enumerate() {
        if raw.trim() != "#MM" {
            continue;
        }
        let mut next = index + 1;
        while next < lines.len() {
            let statement = strip_make_comment(lines[next].trim()).trim();
            if statement.is_empty() {
                next += 1;
                continue;
            }
            if next != target_line {
                break;
            }
            return Some(index);
        }
    }
    None
}

fn unsupported_make_construct(content: &str) -> Option<String> {
    for raw in content.lines() {
        let statement = strip_make_comment(raw.trim_start()).trim_start();
        if statement.contains("$(eval") || statement.contains("${eval") {
            return Some("Make eval is unsupported in a generic header-copy source".into());
        }
        if raw.starts_with('\t') {
            continue;
        }
        let mut words = statement.splitn(2, char::is_whitespace);
        let mut word = words.next().unwrap_or_default();
        let mut rest = words.next().unwrap_or_default().trim();
        let mut modifiers = 0usize;
        while matches!(word, "override" | "export" | "private") && modifiers < 3 {
            modifiers += 1;
            let mut nested = rest.splitn(2, char::is_whitespace);
            word = nested.next().unwrap_or_default();
            rest = nested.next().unwrap_or_default().trim();
        }
        if word == "define" {
            return Some("Make define bodies are opaque to the generic header-copy scanner".into());
        }
        if modifiers == 3 && matches!(word, "override" | "export" | "private") {
            return Some("Make assignment has unsupported nested modifiers".into());
        }
    }
    None
}

/// Keep every still-visible Make include attached to this partial owner.
/// Explicitly bound native configuration fragments disappear from `expanded`;
/// any unbound or unsafe include remains as an unresolved contract entry and
/// is never interpreted as an empty variable scope.
fn unresolved_make_includes(
    content: &str,
    source_root: &Path,
    makefile_relative: &Path,
    bindings: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let scan = crate::local_make_includes::inline_native_make_configuration(
        content,
        source_root,
        makefile_relative,
        crate::local_make_includes::LocalMakeIncludeLimits::default(),
        bindings,
    );
    let mut unresolved = Vec::new();
    for raw in crate::parser::join_continuations(&scan.expanded).lines() {
        if raw.starts_with('\t') {
            continue;
        }
        let statement = strip_make_comment(raw.trim_start()).trim_start();
        let argument = ["-include", "sinclude", "include"]
            .iter()
            .find_map(|directive| {
                statement
                    .strip_prefix(directive)
                    .filter(|rest| rest.starts_with(char::is_whitespace))
                    .map(str::trim)
                    .filter(|rest| !rest.is_empty())
            });
        if let Some(argument) = argument {
            let value = format!("unbound Make include: {argument}");
            if !unresolved.contains(&value) {
                unresolved.push(value);
            }
        }
    }
    for issue in scan.issues {
        let value = format!("unresolved Make include: {issue}");
        if !unresolved.contains(&value) {
            unresolved.push(value);
        }
    }
    unresolved
}

fn validate_header_list(name: &str, headers: &[String]) -> Result<(), String> {
    if headers.len() > MAX_OUTPUTS {
        return Err(format!(
            "{name} exceeds the {MAX_OUTPUTS}-header scan limit"
        ));
    }
    ensure_unique_case_insensitive(headers, name)?;
    for header in headers {
        if !safe_header_relative(header) {
            return Err(format!(
                "{name} path `{header}` is not a safe source-relative .h/.hpp"
            ));
        }
    }
    Ok(())
}

fn ensure_unique_case_insensitive(paths: &[String], kind: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    for path in paths {
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(format!(
                "duplicate or case-colliding generic header {kind} `{path}`"
            ));
        }
    }
    Ok(())
}

fn safe_header_relative(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_HEADER_PATH_BYTES
        || value.starts_with('/')
        || value.contains(['\\', '$', '%', ';', ':', '\n', '\r', '\t', '"', '\'', '`'])
    {
        return false;
    }
    if !value.split('/').all(|component| {
        !component.is_empty()
            && component != "."
            && component != ".."
            && component
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '+'))
    }) {
        return false;
    }
    Path::new(value)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "h" || extension == "hpp")
}

fn safe_owner_name(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && !value.contains('$')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-'))
}

fn safe_relative_directory(path: &Path) -> Result<String, String> {
    let raw = path
        .to_str()
        .ok_or_else(|| "declaring directory is not valid UTF-8".to_owned())?
        .replace('\\', "/");
    if raw.is_empty() || raw == "." {
        return Ok(String::new());
    }
    if raw.starts_with('/') || raw.contains(['$', ';', '\n', '\r']) {
        return Err(format!(
            "declaring directory `{raw}` is not source-relative"
        ));
    }
    for component in raw.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || !component
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-'))
        {
            return Err(format!(
                "declaring directory `{raw}` contains an unsafe component"
            ));
        }
    }
    Ok(raw)
}

fn relative_header_path(relative_dir: &Path, header: &str) -> Result<std::path::PathBuf, String> {
    let mut path = relative_dir.to_path_buf();
    path.push(header);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("header `{header}` escapes the source tree"));
    }
    Ok(path)
}

fn validate_source_file(source_root: &Path, relative: &Path) -> Result<(), String> {
    let root = source_root
        .canonicalize()
        .map_err(|error| format!("source root cannot be canonicalized: {error}"))?;
    let mut current = root.clone();
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return Err("source path is empty".into());
    }
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return Err("source path contains traversal or an absolute component".into());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            format!("source path {} cannot be read: {error}", current.display())
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "source path crosses a symlink: {}",
                current.display()
            ));
        }
        let final_component = index + 1 == components.len();
        if final_component && !metadata.is_file() {
            return Err(format!(
                "source path is not a regular file: {}",
                current.display()
            ));
        }
        if !final_component && !metadata.is_dir() {
            return Err(format!(
                "source parent is not a directory: {}",
                current.display()
            ));
        }
    }
    let physical = current
        .canonicalize()
        .map_err(|error| format!("source path cannot be canonicalized: {error}"))?;
    if !physical.starts_with(root) {
        return Err("source path resolves outside the selected source tree".into());
    }
    Ok(())
}

fn rejection(
    owner: Option<&str>,
    relative_dir: &Path,
    line: usize,
    reason: impl Into<String>,
) -> Rejection {
    let directory = relative_dir.to_string_lossy().replace('\\', "/");
    Rejection {
        owner: owner.map(str::to_owned),
        file: if directory.is_empty() || directory == "." {
            "mmakefile.src".to_owned()
        } else {
            format!("{directory}/mmakefile.src")
        },
        line,
        reason: reason.into(),
    }
}

fn rejected_candidates(
    candidates: &[&Rule],
    _rules: &[Rule],
    relative_dir: &Path,
    reason: impl Into<String>,
) -> (Vec<LayeredHeaderCopyDecl>, Vec<Rejection>) {
    let reason = reason.into();
    let rejections = candidates
        .iter()
        .map(|rule| {
            rejection(
                safe_owner_name(rule.target.trim()).then_some(rule.target.trim()),
                relative_dir,
                rule.line + 1,
                reason.clone(),
            )
        })
        .collect();
    (Vec::new(), rejections)
}

#[cfg(test)]
mod tests {
    use super::{collect, LayeredHeaderCopyDecl};
    use crate::dirs::DirVars;
    use crate::make_vars::collect_vars_impl;
    use crate::parser::{join_continuations, TargetContext};
    use crate::testing::TempTree;
    use aros_common::native_build_contract::load_bound_native_build_contract;
    use std::fs;
    use std::path::{Path, PathBuf};

    const FIXTURE: &str = concat!(
        "INCSUBDIRS := aros api\n",
        "INCLUDES_BASE := $(foreach d,$(addprefix $(SRCDIR)/$(CURDIR)/,$(INCSUBDIRS)),$(wildcard $(d)/*.h) $(wildcard $(d)/*.hpp)) $(wildcard *.h)\n",
        "INCLUDES := $(subst $(SRCDIR)/$(CURDIR)/,,$(INCLUDES_BASE))\n",
        "ARCHINCDIR := $(SRCDIR)/arch/$(CPU)-$(ARCH)/include/\n",
        "ARCHFAMILYINCDIR := $(SRCDIR)/arch/$(CPU)-$(FAMILY)/include/\n",
        "ARCH_INCLUDES := $(subst $(ARCHINCDIR),,$(foreach d,$(addprefix $(ARCHINCDIR),$(INCSUBDIRS)),$(wildcard $(d)/*.h) $(wildcard $(d)/*.hpp)))\n",
        "ARCHFAMILY_INCLUDES := $(subst $(ARCHFAMILYINCDIR),,$(foreach d,$(addprefix $(ARCHFAMILYINCDIR),$(INCSUBDIRS)),$(wildcard $(d)/*.h) $(wildcard $(d)/*.hpp)))\n",
        "INCLUDES := $(filter-out $(strip $(ARCH_INCLUDES) $(filter-out $(ARCH_INCLUDES),$(ARCHFAMILY_INCLUDES))),$(INCLUDES))\n",
        "DEST_INCLUDES := $(foreach f,$(INCLUDES),$(AROS_INCLUDES)/$(f))\n",
        "GEN_INCLUDES := $(foreach f,$(INCLUDES),$(GENINCDIR)/$(f))\n",
        "#MM\n",
        "header-stage : setup $(DEST_INCLUDES) $(GEN_INCLUDES) includes-execbase_h\n",
        "\n",
        "$(AROS_INCLUDES)/%.h : $(SRCDIR)/$(CURDIR)/%.h\n",
        "\t@$(ECHO) \"Copying    C   includes to $(AROS_INCLUDES)...\"\n",
        "\t@$(CP) $< $@\n",
        "$(GENINCDIR)/%.h : $(SRCDIR)/$(CURDIR)/%.h\n",
        "\t@$(ECHO) \"Copying    C   includes to $(GENINCDIR)...\"\n",
        "\t@$(CP) $< $@\n",
        "$(AROS_INCLUDES)/%.hpp : $(SRCDIR)/$(CURDIR)/%.hpp\n",
        "\t@$(ECHO) \"Copying    C++ includes to $(AROS_INCLUDES)...\"\n",
        "\t@$(CP) $< $@\n",
        "$(GENINCDIR)/%.hpp : $(SRCDIR)/$(CURDIR)/%.hpp\n",
        "\t@$(ECHO) \"Copying    C++ includes to $(GENINCDIR)...\"\n",
        "\t@$(CP) $< $@\n",
    );

    struct FixtureTree(TempTree);

    impl FixtureTree {
        fn new() -> Self {
            let tree = TempTree::new();
            let root = &tree.0;
            fs::create_dir_all(root.join("config")).unwrap();
            fs::write(
                root.join("config/make.cfg.in"),
                concat!(
                    "AROS_DIR_DEVELOPER := Developer\n",
                    "AROS_DIR_INCLUDE := include\n",
                    "AROSDIR := $(TARGETDIR)/$(AROS_DIR_AROS)\n",
                    "AROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\n",
                    "AROS_INCLUDES := $(AROS_DEVELOPER)/$(AROS_DIR_INCLUDE)\n",
                    "GENINCDIR := $(GENDIR)/include\n",
                ),
            )
            .unwrap();
            for path in [
                "compiler/include/aros",
                "compiler/include/api",
                "arch/unitcpu-unitarch/include/api",
                "arch/unitcpu-unitfamily/include/api",
            ] {
                fs::create_dir_all(root.join(path)).unwrap();
            }
            for (path, body) in [
                ("compiler/include/aros/generic.h", "generic\n"),
                ("compiler/include/api/base.h", "base\n"),
                ("compiler/include/api/base.hpp", "base++\n"),
                ("compiler/include/api/arch.h", "generic arch name\n"),
                ("compiler/include/api/family.hpp", "generic family name\n"),
                ("compiler/include/root.h", "root\n"),
                (
                    "compiler/include/root.hpp",
                    "not selected by INCLUDES_BASE\n",
                ),
                (
                    "arch/unitcpu-unitarch/include/api/arch.h",
                    "arch override\n",
                ),
                (
                    "arch/unitcpu-unitfamily/include/api/family.hpp",
                    "family override\n",
                ),
            ] {
                fs::write(root.join(path), body).unwrap();
            }
            Self(tree)
        }

        fn root(&self) -> &Path {
            &self.0 .0
        }

        fn run(&self, source: &str) -> (Vec<LayeredHeaderCopyDecl>, Vec<super::Rejection>) {
            let joined = join_continuations(source);
            let target = target();
            let (scope, states) = collect_vars_impl(&joined, Some(&target));
            let relative_dir = Path::new("compiler/include");
            fs::write(self.root().join(relative_dir).join("mmakefile.src"), source).unwrap();
            let dirs = DirVars::load(self.root());
            collect(
                &joined,
                &scope,
                &dirs,
                self.root(),
                relative_dir,
                &target,
                Some(&states),
            )
        }
    }

    fn target() -> TargetContext {
        TargetContext {
            cpu: Some("unitcpu".into()),
            platform: Some("unitarch".into()),
            family: Some("unitfamily".into()),
            ..TargetContext::default()
        }
    }

    #[test]
    fn generic_list_filters_arch_family_without_inventing_overlay_copies() {
        let tree = FixtureTree::new();
        let (declarations, rejected) = tree.run(FIXTURE);
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(declarations.len(), 1);
        let declaration = &declarations[0];
        assert_eq!(declaration.owner, "header-stage");
        assert_eq!(declaration.file, "compiler/include/mmakefile.src");
        assert_eq!(declaration.line, 12);
        assert_eq!(
            declaration.inputs,
            [
                "compiler/include/aros/generic.h",
                "compiler/include/api/base.h",
                "compiler/include/api/base.hpp",
                "compiler/include/root.h",
            ]
        );
        assert_eq!(declaration.copies.len(), 8);
        assert!(!declaration
            .copies
            .iter()
            .any(|copy| copy.source_relative.ends_with("api/arch.h")));
        assert!(!declaration
            .copies
            .iter()
            .any(|copy| copy.source_relative.ends_with("api/family.hpp")));
        assert!(!declaration
            .copies
            .iter()
            .any(|copy| copy.source_relative.ends_with("root.hpp")));
        assert!(declaration.copies[..4]
            .iter()
            .all(|copy| copy.output_alias.starts_with("${AROS_SDK_INCLUDE_DIR}/")));
        assert!(declaration.copies[4..]
            .iter()
            .all(|copy| copy.output_alias.starts_with("${AROS_GENINC_DIR}/")));
        assert_eq!(
            declaration.unresolved_prerequisites,
            ["setup", "includes-execbase_h"]
        );
        assert!(!declaration
            .copies
            .iter()
            .any(|copy| copy.source_relative.ends_with("execbase.h")));
    }

    #[test]
    fn unresolved_functions_and_path_traversal_reject_the_owned_list() {
        let tree = FixtureTree::new();
        let source = FIXTURE.replace("INCSUBDIRS := aros api", "INCSUBDIRS := $(shell unsafe)");
        let (declarations, rejected) = tree.run(&source);
        assert!(declarations.is_empty());
        assert_eq!(rejected.len(), 1);
        assert!(
            rejected[0].reason.contains("INCLUDES_BASE"),
            "{rejected:#?}"
        );

        let tree = FixtureTree::new();
        fs::create_dir_all(tree.root().join("compiler/outside")).unwrap();
        fs::write(tree.root().join("compiler/outside/escape.h"), "outside\n").unwrap();
        let source = FIXTURE.replace("INCSUBDIRS := aros api", "INCSUBDIRS := ../outside");
        let (declarations, rejected) = tree.run(&source);
        assert!(declarations.is_empty());
        assert_eq!(rejected.len(), 1);
        assert!(
            rejected[0].reason.contains("safe source-relative"),
            "{rejected:#?}"
        );
    }

    #[test]
    fn command_shadowing_extra_recipe_and_dynamic_make_are_refused() {
        let tree = FixtureTree::new();
        let source = FIXTURE.replace("#MM\n", "CP := cp -f\n#MM\n");
        let (declarations, rejected) = tree.run(&source);
        assert!(declarations.is_empty());
        assert!(rejected[0].reason.contains("CP/ECHO"), "{rejected:#?}");

        let tree = FixtureTree::new();
        let source = FIXTURE.replace(
            "\t@$(CP) $< $@\n$(GENINCDIR)/%.h",
            "\t@$(CP) $< $@\n\t@touch $@\n$(GENINCDIR)/%.h",
        );
        let (declarations, rejected) = tree.run(&source);
        assert!(declarations.is_empty());
        assert!(
            rejected[0].reason.contains("exactly the recognized"),
            "{rejected:#?}"
        );

        let tree = FixtureTree::new();
        let source = FIXTURE.replace("#MM\n", "$(eval $(warning opaque))\n#MM\n");
        let (declarations, rejected) = tree.run(&source);
        assert!(declarations.is_empty());
        assert!(rejected[0].reason.contains("eval"), "{rejected:#?}");
    }

    #[cfg(unix)]
    #[test]
    fn source_symlinks_and_unknown_conditional_patterns_are_refused() {
        use std::os::unix::fs::symlink;

        let tree = FixtureTree::new();
        let source = tree.root().join("compiler/include/aros/generic.h");
        let target_file = tree.root().join("compiler/include/aros/real.h");
        fs::rename(&source, &target_file).unwrap();
        symlink(&target_file, &source).unwrap();
        let (declarations, rejected) = tree.run(FIXTURE);
        assert!(declarations.is_empty());
        assert!(rejected[0].reason.contains("symlink"), "{rejected:#?}");

        let tree = FixtureTree::new();
        let guarded = FIXTURE.replace(
            "$(AROS_INCLUDES)/%.h : $(SRCDIR)/$(CURDIR)/%.h",
            "ifeq ($(UNKNOWN_SWITCH),enabled)\n$(AROS_INCLUDES)/%.h : $(SRCDIR)/$(CURDIR)/%.h",
        );
        let guarded = guarded.replace(
            "$(GENINCDIR)/%.h : $(SRCDIR)/$(CURDIR)/%.h",
            "endif\n$(GENINCDIR)/%.h : $(SRCDIR)/$(CURDIR)/%.h",
        );
        let (declarations, rejected) = tree.run(&guarded);
        assert!(declarations.is_empty());
        assert!(rejected[0].reason.contains("copy pattern"), "{rejected:#?}");
    }

    #[test]
    fn duplicate_pattern_and_unmarked_owner_reject_instead_of_guessing() {
        let tree = FixtureTree::new();
        let source = format!(
            "{FIXTURE}{}",
            &FIXTURE[FIXTURE.find("$(AROS_INCLUDES)/%.h").unwrap()..]
        );
        let (declarations, rejected) = tree.run(&source);
        assert!(declarations.is_empty());
        assert!(rejected[0].reason.contains("copy pattern"), "{rejected:#?}");

        let tree = FixtureTree::new();
        let source = FIXTURE.replace("#MM\nheader-stage", "header-stage");
        let (declarations, rejected) = tree.run(&source);
        assert!(declarations.is_empty());
        assert!(rejected[0].reason.contains("#MM"), "{rejected:#?}");
    }

    #[test]
    #[ignore = "requires AROS_P4_SOURCE_ROOT pointing at the pinned P4 source checkout"]
    fn actual_p4_compiler_include_owner_is_partial_and_execbase_stays_unresolved() {
        let source_root = std::env::var_os("AROS_P4_SOURCE_ROOT")
            .expect("AROS_P4_SOURCE_ROOT must name the pinned P4 source checkout");

        let source_root = PathBuf::from(source_root).canonicalize().unwrap();
        let relative_dir = Path::new("compiler/include");
        let raw = fs::read_to_string(source_root.join(relative_dir).join("mmakefile.src")).unwrap();
        let profiles =
            aros_common::TargetProfile::load_from_file(&source_root.join("aros-targets.toml"))
                .unwrap();
        let profile = profiles
            .iter()
            .find(|profile| profile.name == "esp32p4-d1001")
            .unwrap();
        let contract_path = profile.native_build_contract.as_deref().unwrap();
        let loaded =
            load_bound_native_build_contract(&source_root, Path::new(contract_path), profile)
                .unwrap();
        let selectors = profile.transpiler.as_ref().unwrap();
        let generated_make_templates =
            aros_common::native_make_template::resolve_generated_make_templates(
                &source_root,
                &loaded.contract.generated_make_templates,
                &loaded
                    .contract
                    .inputs
                    .iter()
                    .map(|input| (input.path.clone(), input.sha256.clone()))
                    .collect(),
            )
            .unwrap();
        let target = TargetContext {
            cpu: Some(loaded.contract.abi.source_cpu),
            platform: Some(profile.platform.clone()),
            family: Some(selectors.family.clone()),
            variant: Some(selectors.variant.clone()),
            toolchain: Some(selectors.toolchain.clone()),
            cpu32: Some(selectors.cpu32.clone()),
            use_mmu: Some(if selectors.use_mmu { "1" } else { "0" }.into()),
            float_abi: profile.float_abi.clone(),
            make_variables: loaded.contract.make_variables,
            make_include_bindings: loaded.contract.make_include_bindings,
            generated_make_templates,
            ..TargetContext::default()
        };
        let native_configuration =
            crate::local_make_includes::inline_native_make_configuration_with_templates(
                &raw,
                &source_root,
                &relative_dir.join("mmakefile.src"),
                crate::local_make_includes::LocalMakeIncludeLimits::default(),
                &target.make_include_bindings,
                &target.generated_make_templates,
            );
        assert!(
            native_configuration.issues.is_empty(),
            "{:#?}",
            native_configuration.issues
        );
        assert!(!native_configuration
            .expanded
            .contains("include $(SRCDIR)/config/aros.cfg"));
        assert!(!native_configuration
            .expanded
            .contains("include $(TOP)/$(CURDIR)/geninc.cfg"));
        let literal_joined = join_continuations(&native_configuration.expanded);
        let (scope, states) = collect_vars_impl(&literal_joined, Some(&target));
        assert_eq!(scope.raw_at("EXECSMP", usize::MAX).as_deref(), Some("\"\""));
        let dirs = DirVars::load(&source_root);
        let (declarations, rejected) = collect(
            &literal_joined,
            &scope,
            &dirs,
            &source_root,
            relative_dir,
            &target,
            Some(&states),
        );
        assert!(rejected.is_empty(), "{rejected:#?}");
        assert_eq!(declarations.len(), 1);
        let declaration = &declarations[0];
        assert_eq!(declaration.owner, "compiler-includes");
        assert_eq!(declaration.file, "compiler/include/mmakefile.src");
        assert!(!declaration.inputs.is_empty());
        assert_eq!(
            declaration.unresolved_prerequisites,
            ["setup", "includes-execbase_h"]
        );
        assert!(!declaration
            .copies
            .iter()
            .any(|copy| copy.source_relative.ends_with("execbase.h")));
    }
}
