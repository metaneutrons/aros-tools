//! Literal products use source-native values, but only physical recipe owners.

use crate::dirs::DirVars;
use crate::make_vars::{collect_vars_impl, ConditionalTruth, VarScope};
use crate::source_literal_text_rules::{
    collect_with_context, SourceLiteralTextRuleDecl, SourceLiteralTextRuleRejection,
};
use crate::TargetContext;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub fn collect(
    physical: &str,
    legacy: &str,
    root: &Path,
    rel_dir: &Path,
    scope: &VarScope,
    dirs: &DirVars,
    states: Option<&[ConditionalTruth]>,
    target: Option<&TargetContext>,
    physical_positions: Option<&[Option<usize>]>,
) -> (
    Vec<SourceLiteralTextRuleDecl>,
    Vec<SourceLiteralTextRuleRejection>,
) {
    // Configuration fragments supply values only. A rule outside the physical
    // mmakefile cannot acquire its source digest and declaring-directory lane.
    if !physical.contains("#MM")
        || !(physical.contains("mmakefile.src")
            || physical.contains("$(ECHO)") && physical.contains("$@"))
    {
        return (Vec::new(), Vec::new());
    }
    let mut legacy_owners = vec![None; legacy.lines().count()];
    if let Some(positions) = physical_positions {
        for (physical, joined) in positions.iter().enumerate() {
            if let Some(slot) = joined.and_then(|joined| legacy_owners.get_mut(joined)) {
                *slot = Some(physical);
            }
        }
    }
    let legacy_rules = physical_rules(legacy, &legacy_owners);
    let baseline = remap(
        collect_with_context(&legacy_rules, root, rel_dir, scope, dirs, states),
        &legacy_owners,
    );
    let Some(target) = target.filter(|target| {
        !target.make_include_bindings.is_empty() || !target.generated_make_templates.is_empty()
    }) else {
        return baseline;
    };
    // No input include can invent an executable owner in a physical recipe.
    let native =
        match crate::assembly_headers::native_configuration_snapshot(
            physical,
            target,
            dirs,
            root,
            &rel_dir.join("mmakefile.src"),
        ) {
            Ok(native) => native,
            Err(reason) => {
                let (declarations, mut rejected) = baseline;
                rejected.extend(declarations.into_iter().map(|rule| {
                    SourceLiteralTextRuleRejection {
                        owner: rule.owner,
                        file: rule.file,
                        line: rule.line,
                        reason: format!(
                            "native literal text configuration is not proved: {reason}"
                        ),
                    }
                }));
                return (Vec::new(), rejected);
            }
        };
    let rules = physical_rules(&native.joined, &native.physical_owner_lines);
    let (scope, states) = collect_vars_impl(&native.joined, Some(target));
    remap(
        collect_with_context(&rules, root, rel_dir, &scope, dirs, Some(&states)),
        &native.physical_owner_lines,
    )
}

fn physical_rules(joined: &str, positions: &[Option<usize>]) -> String {
    let mut rules = String::with_capacity(joined.len());
    for (index, line) in joined.lines().enumerate() {
        if positions.get(index).copied().flatten().is_some() {
            rules.push_str(line);
        }
        rules.push('\n');
    }
    rules
}

fn remap(
    (declarations, mut rejected): (
        Vec<SourceLiteralTextRuleDecl>,
        Vec<SourceLiteralTextRuleRejection>,
    ),
    positions: &[Option<usize>],
) -> (
    Vec<SourceLiteralTextRuleDecl>,
    Vec<SourceLiteralTextRuleRejection>,
) {
    for rejection in &mut rejected {
        rejection.line = physical_line(positions, rejection.line).unwrap_or(1);
    }
    let mut accepted = Vec::new();
    for mut rule in declarations {
        if let Some(line) = physical_line(positions, rule.line) {
            rule.line = line;
            accepted.push(rule);
        } else {
            rejected.push(SourceLiteralTextRuleRejection {
                owner: rule.owner,
                file: rule.file,
                line: 1,
                reason: "literal text owner has no physical recipe location; fix upstream".into(),
            });
        }
    }
    (accepted, rejected)
}

fn physical_line(positions: &[Option<usize>], line: usize) -> Option<usize> {
    positions
        .get(line.checked_sub(1)?)
        .copied()
        .flatten()?
        .checked_add(1)
}

#[cfg(test)]
#[path = "source_literal_text_scope_tests.rs"]
mod tests;
