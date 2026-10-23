//! Explicit Source-owned intrinsic producer scope, not transitive admission.

use super::{DeclaredChildRecipeProjection, MakeRecipeProjection, GOAL};
use crate::{make_rule_graph::traversal::TargetVisit, make_vars::MakeFileEventKind};

pub(super) fn verify_producer_selection(
    recipes: &DeclaredChildRecipeProjection,
    selected: usize,
    values: &[MakeRecipeProjection],
) -> Result<aros_common::Sha256Digest, String> {
    if values.is_empty() || values.len() > 48 || !values.len().is_multiple_of(3) {
        return Err("intrinsic producer lacks bounded explicit Source policy".into());
    }
    let declared = &recipes.declared;
    let visits = declared.traversal.visits();
    let producer = visits
        .get(selected)
        .and_then(TargetVisit::instance)
        .ok_or("intrinsic producer has no declared rule")?;
    let mut matching = values
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|selection| selection[0].expression() == producer.target);
    let selection = matching
        .next()
        .ok_or("deeper recipe is not an explicitly selected intrinsic producer")?;
    if matching.next().is_some() {
        return Err("intrinsic producer selection is ambiguous".into());
    }
    let names = selection
        .iter()
        .map(MakeRecipeProjection::expression)
        .collect::<Vec<_>>();
    let output_paths = names
        .iter()
        .map(|name| {
            super::super::links::literal_output(declared, name, &declared.source.build_root)
                .ok_or("intrinsic role is not a normalized output under its original build")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if output_paths
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != 3
    {
        return Err("intrinsic producer roles overlap".into());
    }
    let alias = unique_visit(visits, names[1])?;
    let consumer = unique_visit(visits, names[2])?;
    let alias_rule = visits[alias]
        .instance()
        .ok_or("intrinsic cooutput lacks a rule")?;
    let consumer_rule = visits[consumer]
        .instance()
        .ok_or("intrinsic consumer lacks a rule")?;
    if producer.recipes.is_empty()
        || consumer_rule.recipes.is_empty()
        || !alias_rule.recipes.is_empty()
        || alias_rule.prerequisites.as_slice() != [producer.target.as_str()]
        || !alias_rule.order_only.is_empty()
        || alias_rule.prerequisite_order.as_slice() != [producer.target.as_str()]
        || !consumer_rule
            .prerequisites
            .iter()
            .any(|name| name == names[1])
    {
        return Err(
            "intrinsic producer/cooutput/consumer declarations differ from Source policy".into(),
        );
    }
    let consumer_chain = declared.traversal.scope_chain(consumer)?;
    if consumer_chain.as_slice() != [GOAL, names[2]] {
        return Err("intrinsic consumer is not the selected direct GCA consumer".into());
    }
    for index in [selected, alias] {
        let chain = declared.traversal.scope_chain(index)?;
        if chain.get(..2) != Some([GOAL, names[2]].as_slice()) {
            return Err(
                "intrinsic producer is outside its selected consumer's first-parent frame".into(),
            );
        }
    }
    let generated = declared
        .source
        .build_root
        .join(declared.coordinates().generated_owner());
    declared.source.snapshot.transaction(|reader| {
        for index in [selected, alias, consumer] {
            let rule = visits[index].instance().ok_or("intrinsic role lost its rule")?;
            if rule.double_colon || rule.stem.is_some() || declared.file().events.iter().any(|event| {
                matches!(&event.kind, MakeFileEventKind::PhonyTargets(names) if names.contains(&rule.target))
            }) {
                return Err("intrinsic role is not a nonphony explicit single-colon rule".into());
            }
            let Some(crate::make_rule_graph::TargetRules::Ordinary(target)) =
                declared.graph.targets.get(&rule.target) else {
                return Err("intrinsic role lacks its original ordinary declarations".into());
            };
            // An effective rule merges prerequisites from all declarations. Its
            // primary output line alone cannot establish those edges' origins.
            for declaration in target.declarations() {
                let header = declared.file().events.get(declaration.output_line)
                    .ok_or("intrinsic role header is absent")?;
                if !matches!(header.kind, MakeFileEventKind::Rule(_))
                    || reader.realpath(&header.input)?.as_ref() != Some(&generated) {
                    return Err("intrinsic role header is outside the Source-expanded owner".into());
                }
            }
            for (recipe_index, recipe) in recipes.recipes.iter().enumerate()
                .filter(|(_, recipe)| recipe.visit() == index) {
                let origin = recipes.origin(recipe_index)?;
                if reader.realpath(&origin.input)?.as_ref() != Some(&generated)
                    || recipe.expression().expression().contains('\0') {
                    return Err("intrinsic recipe sequence is outside the Source-expanded owner".into());
                }
            }
        }
        Ok(())
    })?;
    serde_json::to_vec(&(
        "mesa26-intrinsic-source-roles-v1",
        names,
        selected,
        alias,
        consumer,
    ))
    .map(|bytes| aros_common::sha256_bytes(&bytes))
    .map_err(|_| "cannot bind intrinsic Source role selection".into())
}

fn unique_visit(visits: &[TargetVisit], target: &str) -> Result<usize, String> {
    let mut matches = visits
        .iter()
        .enumerate()
        .filter(|(_, visit)| visit.target() == target);
    let (index, _) = matches
        .next()
        .ok_or("intrinsic role is not reached from the selected goal")?;
    if matches.next().is_some() {
        return Err("intrinsic role has ambiguous visits".into());
    }
    Ok(index)
}
