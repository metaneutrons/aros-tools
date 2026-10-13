//! Physical, dependency-only Make providers backed by local script outputs.

use crate::copy_includes::ScriptOutputDecl;
use crate::dirs::DirVars;
use crate::make_vars::ConditionalTruth;
use crate::TargetContext;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// One exact physical provider, not an arbitrary metadata alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptOutputAggregate {
    pub owner: String,
    pub file: String,
    pub line: usize,
    pub outputs: Vec<String>,
    /// Populated only after exact same-recipe generator resolution.
    pub generators: Vec<String>,
}

/// Admit explicitly phony, complete no-recipe providers only.
/// Bare `#MM` does not prevent GNU Make from applying an implicit recipe.
pub(crate) fn collect(
    content: &str,
    recipe: &Path,
    root: &Path,
    dirs: &DirVars,
    target: Option<&TargetContext>,
    providers: &[String],
    generators: &[ScriptOutputDecl],
) -> Vec<ScriptOutputAggregate> {
    if providers.is_empty() || generators.is_empty() {
        return Vec::new();
    }
    // Included Make files can change recipe syntax or add opaque effects.
    // Verify the bounded source configuration before interpreting the physical
    // owner rule; never treat an unexpanded include as harmless.
    let configuration = crate::local_make_includes::inline_native_make_configuration_for_selection(
        content,
        root,
        recipe,
        crate::LocalMakeIncludeLimits::default(),
        target,
    );
    if !configuration.issues.is_empty() {
        return Vec::new();
    }
    let (joined, physical) = crate::parser::join_continuations_with_origins(content);
    if configuration
        .expanded
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .any(|line| {
            // Modifier-prefixed assignments can also change recipe syntax.
            // Do not parse a custom recipe prefix as an ordinary rule boundary.
            line.contains(".RECIPEPREFIX")
                || line.contains(".SECONDEXPANSION")
                || line.contains("$(eval")
                || line.contains("${eval")
                || (!line.starts_with('\t')
                    && line
                        .split_once('=')
                        .is_some_and(|(name, _)| name.contains('$')))
                || line.split_whitespace().next().is_some_and(|word| {
                    matches!(
                        word,
                        "include" | "-include" | "sinclude" | "-sinclude" | "load" | "-load"
                    )
                })
        })
    {
        return Vec::new();
    }
    let effective = crate::parser::join_continuations(&configuration.expanded);
    let Some(positions) = crate::parser::architecture_scope_positions(
        root,
        recipe,
        content,
        &configuration,
        &effective,
    ) else {
        return Vec::new();
    };
    let (scope, states) = crate::make_vars::collect_vars_impl(&effective, target);
    let lines: Vec<_> = joined.lines().collect();
    let directory = recipe.parent().unwrap_or_else(|| Path::new(""));
    let effective_lines: Vec<_> = effective.lines().collect();
    let phony_owners =
        proved_phony_owners(&effective_lines, &scope, &states, dirs, root, directory);
    let directory_text = directory.to_string_lossy().replace('\\', "/");
    let mut outputs = BTreeMap::<String, usize>::new();
    for output in generators
        .iter()
        .filter(|generator| generator.directory == directory_text)
        .flat_map(|generator| {
            std::iter::once(&generator.output).chain(&generator.additional_outputs)
        })
        .map(|output| canonical_output(output))
    {
        *outputs.entry(output).or_default() += 1;
    }
    let mut candidates: BTreeMap<String, Vec<ScriptOutputAggregate>> = BTreeMap::new();
    let mut definitions = BTreeMap::<String, usize>::new();
    let mut define_depth = 0usize;
    for (line, raw) in lines.iter().enumerate() {
        let text = raw.trim();
        let mut words = text.split_whitespace();
        let mut directive = words.next();
        while matches!(
            directive,
            Some("override" | "export" | "unexport" | "private")
        ) {
            directive = words.next();
        }
        if directive == Some("define") {
            define_depth += 1;
        } else if text == "endef" {
            define_depth = define_depth.saturating_sub(1);
        }
        if define_depth != 0 || raw.starts_with('\t') || text.starts_with('#') {
            continue;
        }
        let Some((owner, dependencies)) = text.split_once(':') else {
            continue;
        };
        let owner = owner.trim();
        if !providers.iter().any(|provider| provider == owner) {
            continue;
        }
        let Some(position) = physical
            .get(line)
            .and_then(|line| positions.get(*line))
            .copied()
            .flatten()
        else {
            continue;
        };
        if states.get(position) == Some(&ConditionalTruth::False) {
            continue;
        }
        *definitions.entry(owner.to_owned()).or_default() += 1;
        if !safe_owner(owner)
            || !phony_owners.contains(owner)
            || states.get(position) != Some(&ConditionalTruth::True)
            || dependencies.contains([':', ';', '|', '#', '='])
            || dependencies.trim().is_empty()
            || has_recipe(&lines[line + 1..])
        {
            continue;
        }
        let expression_scope = crate::MakeExprContext::new(&scope, dirs, position, root, directory)
            .without_filesystem();
        let Ok(prerequisites) = crate::evaluate_make_list(dependencies, &expression_scope) else {
            continue;
        };
        let prerequisites: Vec<_> = prerequisites
            .iter()
            .map(|value| canonical_output(value))
            .collect();
        let unique: BTreeSet<_> = prerequisites.iter().cloned().collect();
        if unique.is_empty()
            || unique.len() != prerequisites.len()
            || !unique.iter().all(|path| outputs.get(path) == Some(&1))
        {
            continue;
        }
        candidates
            .entry(owner.to_owned())
            .or_default()
            .push(ScriptOutputAggregate {
                owner: owner.to_owned(),
                file: recipe.to_string_lossy().replace('\\', "/"),
                line: physical.get(line).copied().unwrap_or(line) + 1,
                outputs: prerequisites,
                generators: Vec::new(),
            });
    }
    candidates
        .into_iter()
        .filter_map(|(owner, mut rules)| {
            (definitions.get(&owner) == Some(&1) && rules.len() == 1).then(|| rules.remove(0))
        })
        .collect()
}

fn proved_phony_owners(
    lines: &[&str],
    scope: &crate::make_vars::VarScope,
    states: &[ConditionalTruth],
    dirs: &DirVars,
    root: &Path,
    directory: &Path,
) -> BTreeSet<String> {
    let mut owners = BTreeSet::new();
    let mut define_depth = 0usize;
    for (line, raw) in lines.iter().enumerate() {
        let text = raw.trim();
        let mut words = text.split_whitespace();
        let mut directive = words.next();
        while matches!(
            directive,
            Some("override" | "export" | "unexport" | "private")
        ) {
            directive = words.next();
        }
        if directive == Some("define") {
            define_depth += 1;
        } else if text == "endef" {
            define_depth = define_depth.saturating_sub(1);
        }
        if define_depth != 0
            || raw.starts_with('\t')
            || states.get(line) != Some(&ConditionalTruth::True)
        {
            continue;
        }
        let Some((name, prerequisites)) = text.split_once(':') else {
            continue;
        };
        if name.trim() != ".PHONY" || prerequisites.contains([':', ';', '|', '#', '=']) {
            continue;
        }
        let expression_scope =
            crate::MakeExprContext::new(scope, dirs, line, root, directory).without_filesystem();
        if let Ok(names) = crate::evaluate_make_list(prerequisites, &expression_scope) {
            owners.extend(names.into_iter().filter(|name| safe_owner(name)));
        }
    }
    owners
}

fn safe_owner(owner: &str) -> bool {
    !owner.is_empty()
        && !matches!(owner, "all" | "clean" | "install" | "help" | "test")
        && owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn has_recipe(lines: &[&str]) -> bool {
    for raw in lines {
        let text = raw.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        if raw.starts_with('\t') {
            return true;
        }
        if text.contains(':') && !text.contains(":=") {
            break;
        }
        if text.starts_with("%build_linklib ") || text == "%common" {
            break;
        }
        return true;
    }
    false
}

pub(crate) fn canonical_output(path: &str) -> String {
    path.replace("${AROS_BUILD_DIR}", "${CMAKE_BINARY_DIR}")
}

#[cfg(test)]
#[path = "script_output_aggregates_tests.rs"]
mod tests;
