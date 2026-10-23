//! Bounded Source-leaf checks for the original Source GENMODULE prerequisite.
//!
//! The original host owner separately proves successful completion of the
//! captured GENMODULE target. This module only checks declared Source inputs;
//! it neither checks timestamps nor recreates Make's scheduler or implicit-rule
//! search.

use super::{
    genmodule::absolute_make_path, DeclaredIncludedRecipes, MakeInputReader,
    NativeGenmoduleIncludePlan,
};
use crate::{
    make_interpreted_rules,
    make_rule_events::RuleInstance,
    make_rule_header::RuleHeader,
    make_vars::{MakeFileEventKind, MakeFileProgram},
};
use aros_common::{sha256_bytes, Sha256Digest};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const MAX_LEAF_FILES: usize = 16_384;
const MAX_PRODUCER_CHECKS: usize = 1_048_576;
const MAX_SUFFIX_CHECKS: usize = 1_048_576;

/// Check configuration and direct GENMODULE prerequisites as Source leaves.
///
/// `None` for the GENMODULE visit is valid: the caller authenticates that
/// already-completed host target and its captured artifact. If the Source trace
/// has an explicit GENMODULE rule, its normal prerequisites must instead be
/// existing, producer-free Source leaves; order-only dependencies are refused.
pub(super) fn validate_genmodule_include_readiness_in_reader(
    included: &DeclaredIncludedRecipes,
    plan: &NativeGenmoduleIncludePlan,
    captured_program: &Path,
    reader: &mut MakeInputReader<'_>,
) -> Result<(), String> {
    if !included
        .declared
        .source
        .snapshot
        .same_read_context(reader.snapshot())
    {
        return Err("GENMODULE readiness reader differs from original Source context".into());
    }
    let Some(program_argument) = plan.arguments().first() else {
        return Err("GENMODULE readiness has no captured program argument".into());
    };
    if !captured_program.is_absolute() || Path::new(program_argument) != captured_program {
        return Err("GENMODULE readiness program differs from captured Source role".into());
    }

    let visits = included.traversal().visits();
    let [include_visit_index] = included.traversal().goals() else {
        return Err("GENMODULE readiness requires one selected include goal".into());
    };
    let include_visit = visits
        .get(*include_visit_index)
        .ok_or("GENMODULE readiness include visit is absent")?;
    if absolute_make_path(included, include_visit.target(), "GENMODULE include target")?
        != plan.include_path()
    {
        return Err("GENMODULE readiness plan differs from selected include target".into());
    }

    // These indices belong to the retained include and directory recipes. The
    // GENMODULE target is a prerequisite visit and may be an already-completed
    // external input with no Source rule of its own.
    let include_recipe = included
        .recipes()
        .get(plan.generator_recipe())
        .ok_or("GENMODULE readiness include recipe is absent")?;
    if include_recipe.visit() != *include_visit_index {
        return Err("GENMODULE readiness recipe no longer belongs to include target".into());
    }
    let directory_recipe = included
        .recipes()
        .get(plan.directory_recipe())
        .ok_or("GENMODULE readiness directory recipe is absent")?;
    let directory_visit_index = directory_recipe.visit();
    let directory_visit = visits
        .get(directory_visit_index)
        .ok_or("GENMODULE readiness directory visit is absent")?;
    if absolute_make_path(
        included,
        directory_visit.target(),
        "GENMODULE include directory",
    )? != plan.directory()
    {
        return Err("GENMODULE readiness directory differs from selected include".into());
    }

    let program_visit_index = unique_visit_for_path(included, captured_program)?;
    let program_visit = visits
        .get(program_visit_index)
        .ok_or("GENMODULE readiness tool visit is absent")?;
    let dependencies = tool_prerequisites(program_visit.instance())?;
    if dependencies.len() > MAX_LEAF_FILES {
        return Err("GENMODULE readiness Source dependency count exceeds limit".into());
    }

    let mut checked_visits = BTreeSet::from([
        *include_visit_index,
        directory_visit_index,
        program_visit_index,
    ]);
    let mut leaves = BTreeMap::<PathBuf, String>::new();

    // The private plan has already bound these exact config paths and digests
    // to the include recipe's normal prerequisites in this same reader.
    for (path, digest) in plan.configurations() {
        let visit_index = unique_visit_for_path(included, path)?;
        let visit = visits
            .get(visit_index)
            .ok_or("GENMODULE configuration visit is absent")?;
        if visit.instance().is_some() {
            return Err("GENMODULE configuration has an explicit Source producer".into());
        }
        let resolved = read_source_leaf(included, reader, visit.target(), Some(digest))?;
        if resolved != *path {
            return Err("GENMODULE configuration identity differs from bound input".into());
        }
        checked_visits.insert(visit_index);
        leaves.insert(resolved, visit.target().to_owned());
    }

    for dependency in dependencies {
        let visit_index = unique_visit_for_name(included, dependency)?;
        let visit = visits
            .get(visit_index)
            .ok_or("GENMODULE Source dependency visit is absent")?;
        if visit.instance().is_some() {
            return Err("GENMODULE Source dependency has an explicit producer".into());
        }
        let resolved = read_source_leaf(included, reader, visit.target(), None)?;
        checked_visits.insert(visit_index);
        leaves.insert(resolved, visit.target().to_owned());
    }

    if visits
        .iter()
        .enumerate()
        .any(|(index, _)| !checked_visits.contains(&index))
    {
        return Err("GENMODULE readiness trace contains an unexamined dependency".into());
    }
    reject_leaf_producers(included.declared.file(), included, &leaves)
}

/// A source GENMODULE rule may have no prerequisites; absence of a rule means
/// its completed target is discharged by the caller's original-host proof.
fn tool_prerequisites(rule: Option<&RuleInstance>) -> Result<&[String], String> {
    let Some(rule) = rule else {
        return Ok(&[]);
    };
    if rule.double_colon {
        return Err("GENMODULE readiness refuses a double-colon tool rule".into());
    }
    if !rule.order_only.is_empty() {
        return Err("GENMODULE readiness tool has order-only dependencies".into());
    }
    Ok(&rule.prerequisites)
}

fn read_source_leaf(
    included: &DeclaredIncludedRecipes,
    reader: &mut MakeInputReader<'_>,
    spelling: &str,
    expected_digest: Option<&Sha256Digest>,
) -> Result<PathBuf, String> {
    let path = absolute_make_path(included, spelling, "GENMODULE Source prerequisite")?;
    if !path.starts_with(&included.source_root) {
        return Err("GENMODULE prerequisite is not a Source-root leaf".into());
    }
    let identity = path
        .to_str()
        .ok_or("GENMODULE readiness path is not UTF-8")?;
    let resolved = reader
        .realpath(identity)?
        .ok_or("GENMODULE readiness input is absent")?;
    if resolved != path {
        return Err("GENMODULE readiness input path is not its captured identity".into());
    }
    let input = reader
        .read_bytes(identity)?
        .ok_or("GENMODULE readiness input bytes are absent")?;
    if expected_digest.is_some_and(|digest| sha256_bytes(&input.bytes) != *digest) {
        return Err("GENMODULE readiness input digest differs from Source binding".into());
    }
    Ok(resolved)
}

fn unique_visit_for_name(included: &DeclaredIncludedRecipes, name: &str) -> Result<usize, String> {
    let mut matches = included
        .traversal()
        .visits()
        .iter()
        .enumerate()
        .filter(|(_, visit)| visit.target() == name);
    let (index, _) = matches
        .next()
        .ok_or("GENMODULE dependency is absent from declaration trace")?;
    if matches.next().is_some() {
        return Err("GENMODULE dependency is ambiguous in declaration trace".into());
    }
    Ok(index)
}

fn unique_visit_for_path(included: &DeclaredIncludedRecipes, path: &Path) -> Result<usize, String> {
    let mut found = None;
    for (index, visit) in included.traversal().visits().iter().enumerate() {
        if absolute_make_path(included, visit.target(), "GENMODULE declaration target")? == path
            && found.replace(index).is_some()
        {
            return Err("GENMODULE path is ambiguous in declaration trace".into());
        }
    }
    found.ok_or_else(|| "GENMODULE path is absent from declaration trace".into())
}

fn reject_leaf_producers(
    file: &MakeFileProgram,
    included: &DeclaredIncludedRecipes,
    leaves: &BTreeMap<PathBuf, String>,
) -> Result<(), String> {
    if file
        .events
        .iter()
        .any(|event| event.active && matches!(&event.kind, MakeFileEventKind::DefaultRule(_)))
    {
        return Err("GENMODULE readiness cannot exclude an active .DEFAULT producer".into());
    }

    for event in file.events.iter().filter(|event| event.active) {
        let MakeFileEventKind::PhonyTargets(names) = &event.kind else {
            continue;
        };
        for name in names {
            if absolute_make_path(included, name, "GENMODULE phony target")
                .is_ok_and(|path| leaves.contains_key(&path))
            {
                return Err("GENMODULE Source leaf is marked phony".into());
            }
        }
    }

    let interpreted = make_interpreted_rules::collect(file)?;
    let mut producer_checks = 0usize;
    let mut suffix_checks = 0usize;
    let suffixes = file
        .suffixes(Some(&[]))?
        .ok_or("GENMODULE readiness suffix state is unknown")?;
    for (leaf_path, spelling) in leaves {
        let name = leaf_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("GENMODULE Source leaf has no UTF-8 basename")?;

        for rule in &interpreted.rules {
            let RuleHeader::Rule {
                targets,
                target_pattern,
                ..
            } = rule.header()
            else {
                continue;
            };
            producer_checks = producer_checks
                .checked_add(1)
                .filter(|count| *count <= MAX_PRODUCER_CHECKS)
                .ok_or("GENMODULE readiness producer check limit exceeded")?;

            if target_pattern.is_none() && targets.contains('%') {
                let patterns = targets.split_whitespace().collect::<Vec<_>>();
                for literal in patterns.iter().filter(|pattern| !pattern.contains('%')) {
                    if absolute_make_path(included, literal, "GENMODULE mixed implicit target")?
                        == *leaf_path
                    {
                        return Err(format!(
                            "GENMODULE Source leaf `{spelling}` has an explicit producer"
                        ));
                    }
                }
                let simple = patterns
                    .iter()
                    .all(|pattern| pattern.matches('%').count() == 1 && !pattern.contains('/'));
                if simple {
                    if !rule.instantiate_implicit_candidate(name)?.is_empty()
                        || patterns
                            .iter()
                            .any(|pattern| empty_stem_match(pattern, name))
                    {
                        return Err(format!(
                            "GENMODULE Source leaf `{spelling}` has an applicable implicit producer"
                        ));
                    }
                } else if patterns
                    .iter()
                    .filter(|pattern| pattern.contains('%'))
                    .any(|pattern| {
                        unsupported_implicit_may_match(
                            pattern,
                            if pattern.contains('/') {
                                leaf_path.to_str().unwrap_or("")
                            } else {
                                name
                            },
                        )
                    })
                {
                    // Unsupported pattern forms are not matched here. If their
                    // output suffix could name this leaf, conservatively refuse.
                    return Err(
                        "GENMODULE Source leaf may match an unsupported implicit pattern".into(),
                    );
                }
                continue;
            }

            for instance in rule.instantiate_explicit()? {
                if absolute_make_path(included, &instance.target, "GENMODULE explicit target")?
                    == *leaf_path
                {
                    return Err(format!(
                        "GENMODULE Source leaf `{spelling}` has an explicit producer"
                    ));
                }
            }

            if target_pattern.is_none() {
                for target in targets.split_whitespace() {
                    suffix_checks = suffix_checks
                        .checked_add(suffixes.len())
                        .filter(|count| *count <= MAX_SUFFIX_CHECKS)
                        .ok_or("GENMODULE readiness suffix check limit exceeded")?;
                    if suffix_rule_may_match(target, name, &suffixes) {
                        return Err(format!(
                            "GENMODULE Source leaf `{spelling}` has an applicable suffix producer"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn empty_stem_match(pattern: &str, target: &str) -> bool {
    pattern
        .split_once('%')
        .is_some_and(|(prefix, suffix)| target == format!("{prefix}{suffix}"))
}

fn unsupported_implicit_may_match(pattern: &str, target: &str) -> bool {
    let Some((_, after_first)) = pattern.split_once('%') else {
        return false;
    };
    let after_last = pattern
        .rsplit_once('%')
        .map_or(after_first, |(_, suffix)| suffix);
    target.ends_with(after_first) || target.ends_with(after_last)
}

fn suffix_rule_may_match(rule_target: &str, target: &str, suffixes: &[String]) -> bool {
    suffixes.iter().any(|input| {
        rule_target.strip_prefix(input).is_some_and(|output| {
            output.is_empty()
                || (suffixes.iter().any(|item| item == output) && target.ends_with(output))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{suffix_rule_may_match, tool_prerequisites, unsupported_implicit_may_match};
    use crate::make_rule_events::RuleInstance;

    fn rule(prerequisites: &[&str], order_only: &[&str]) -> RuleInstance {
        RuleInstance {
            output_line: 0,
            source_line: 0,
            target: "GENMODULE".into(),
            stem: None,
            prerequisites: prerequisites.iter().map(|word| (*word).into()).collect(),
            order_only: order_only.iter().map(|word| (*word).into()).collect(),
            prerequisite_order: prerequisites
                .iter()
                .chain(order_only)
                .map(|word| (*word).into())
                .collect(),
            double_colon: false,
            recipes: Vec::new(),
        }
    }

    #[test]
    fn completed_external_tool_and_explicit_empty_rule_need_no_invented_inputs() {
        assert!(tool_prerequisites(None).unwrap().is_empty());
        assert!(tool_prerequisites(Some(&rule(&[], &[])))
            .unwrap()
            .is_empty());
        let explicit = rule(&["../source/genmodule.c"], &[]);
        let prerequisites = tool_prerequisites(Some(&explicit)).unwrap();
        assert_eq!(prerequisites.len(), 1);
        assert_eq!(prerequisites[0], "../source/genmodule.c");
    }

    #[test]
    fn explicit_tool_order_only_dependencies_are_not_discarded() {
        assert!(tool_prerequisites(Some(&rule(&[], &["generated-config"]))).is_err());
    }

    #[test]
    fn suffix_candidate_requires_active_input_and_output_suffixes() {
        let suffixes = vec![".c".to_owned(), ".h".to_owned(), ".conf".to_owned()];
        assert!(suffix_rule_may_match(".c.h", "unit.h", &suffixes));
        assert!(!suffix_rule_may_match(".c.h", "unit.c", &suffixes));
        assert!(!suffix_rule_may_match(".c.o", "unit.o", &suffixes));
        assert!(
            suffix_rule_may_match(".c", "unit.h", &suffixes),
            "a single suffix can build a no-output-suffix target from its suffixed prerequisite"
        );
        assert!(unsupported_implicit_may_match(
            "/source/dir/%/header",
            "/source/dir/x/header"
        ));
        assert!(unsupported_implicit_may_match(
            "dir/%/header",
            "/source/dir/x/header"
        ));
    }
}
