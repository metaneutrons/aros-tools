//! Preserve source-local MetaMake preparation before ordinary archive compilers.
//!
//! MetaMake visits a declaration's prerequisites synchronously in token order
//! (tools/MetaMake/project.c, maketarget). A wrapper's earlier prerequisites
//! must therefore finish before a later archive's object commands. Edges only
//! on the wrapper would make those commands parallel siblings in Ninja.

use super::NativeOwnerProjection;
use crate::ast::MetaTargetRule;
use crate::graph::{native_endpoint, private_source_object_owner};
use crate::{DependencyGraph, TargetContext};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
#[path = "native_archive_preparation_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SourceArchivePreparation {
    pub recipe: String,
    pub wrapper: String,
    pub archive: String,
    pub producer: String,
    pub prerequisite: String,
}

impl NativeOwnerProjection {
    pub(super) fn bind_archive_preparations(
        &self,
        graph: &mut DependencyGraph,
        parser_origins: &mut BTreeMap<String, BTreeSet<String>>,
        omitted_dependencies: &BTreeSet<(String, String, String)>,
        selected_endpoints: &BTreeSet<String>,
        context: &TargetContext,
    ) -> Result<Vec<SourceArchivePreparation>, String> {
        let mut bindings = BTreeSet::new();
        let mut work = 0usize;
        let mut literal_groups = BTreeMap::<_, Vec<_>>::new();
        for group in &graph.literal_object_groups {
            charge_preparation(&mut work)?;
            literal_groups
                .entry(group.owner.as_str())
                .or_default()
                .push(group);
        }
        for archive in &graph.source_archives {
            let owner = &archive.declaration.owner;
            let recipe = &archive.declaration.file;
            let producers = archive
                .compile_groups
                .iter()
                .map(|group| {
                    if group.parent.is_some() {
                        group.owner.clone()
                    } else {
                        private_source_object_owner(group)
                    }
                })
                .collect::<BTreeSet<_>>();
            for declaration in self.graph.declarations() {
                charge_preparation(&mut work)?;
                if !selected_endpoints.contains(&declaration.target)
                    || !declaration.virtual_target
                    || declaration.claims_make_owner
                {
                    continue;
                }
                // Global aggregates are not preparation wrappers for every
                // archive sibling. Require the actual declaring source file.
                if self.bound_recipe(&declaration.file)? != *recipe {
                    continue;
                }
                let mut position = None;
                for (index, dependency) in declaration.dependencies.iter().enumerate() {
                    charge_preparation(&mut work)?;
                    if dependency.concrete == *owner {
                        position = Some(index);
                        break;
                    }
                }
                let Some(position) = position else {
                    continue;
                };
                for predecessor in &declaration.dependencies[..position] {
                    charge_preparation(&mut work)?;
                    // Absence was already proved for this exact source edge,
                    // never inferred merely from a missing native endpoint.
                    if omitted_dependencies.contains(&(
                        recipe.clone(),
                        declaration.target.clone(),
                        predecessor.concrete.clone(),
                    )) {
                        continue;
                    }
                    for producer in &producers {
                        charge_preparation(&mut work)?;
                        if bindings.len() >= 20_000 {
                            return Err("ordered archive preparation binding limit exceeded".into());
                        }
                        if predecessor.concrete == *owner
                            || predecessor.concrete == declaration.target
                            || predecessor.concrete == *producer
                        {
                            return Err(format!(
                                "{recipe}: ordered archive preparation re-enters its owner"
                            ));
                        }
                        let matching = literal_groups
                            .get(producer.as_str())
                            .map_or(&[][..], Vec::as_slice);
                        let mut exact = false;
                        if let [group] = matching {
                            for source in &archive.compile_groups {
                                charge_preparation(&mut work)?;
                                if source.file == group.file && source.line == group.line {
                                    // Charge the full object comparison, not only
                                    // the outer owner join.
                                    for _ in &source.objects {
                                        charge_preparation(&mut work)?;
                                    }
                                    exact |= source.objects == group.objects;
                                }
                            }
                        }
                        if !exact {
                            return Err(format!(
                                "{recipe}: ordered archive preparation has no exact object producer"
                            ));
                        }
                        bindings.insert(SourceArchivePreparation {
                            recipe: recipe.clone(),
                            wrapper: declaration.target.clone(),
                            archive: owner.clone(),
                            producer: producer.clone(),
                            prerequisite: predecessor.concrete.clone(),
                        });
                    }
                }
            }
        }
        // Commit only after all source and producer joins have been checked.
        // Ordinary strict graph validation still rejects a missing prerequisite.
        let mut prospective = graph.native_selection_dependency_edges(context);
        for children in prospective.values() {
            charge_preparation(&mut work)?;
            for _ in children {
                charge_preparation(&mut work)?;
            }
        }
        for binding in &bindings {
            prospective
                .entry(binding.producer.clone())
                .or_default()
                .insert(binding.prerequisite.clone());
        }
        for binding in &bindings {
            let mut pending = vec![binding.prerequisite.as_str()];
            let mut visited = BTreeSet::new();
            while let Some(endpoint) = pending.pop() {
                charge_preparation(&mut work)?;
                let endpoint = native_endpoint(endpoint, context).map_err(|error| {
                    format!("ordered archive preparation has an unresolved prerequisite: {error}")
                })?;
                if endpoint == binding.producer {
                    return Err(format!(
                        "{}: ordered archive preparation would form a producer cycle",
                        binding.recipe
                    ));
                }
                if visited.insert(endpoint.clone()) {
                    if visited.len() > 20_000 {
                        return Err("ordered archive preparation traversal limit exceeded".into());
                    }
                    if let Some(children) = prospective.get(&endpoint) {
                        pending.extend(children.iter().map(String::as_str));
                    }
                }
            }
        }
        for binding in &bindings {
            graph.add_explicit_meta_rule(MetaTargetRule {
                name: binding.producer.clone(),
                dependencies: vec![binding.prerequisite.clone()],
            });
            parser_origins
                .entry(binding.producer.clone())
                .or_default()
                .insert(binding.recipe.clone());
        }
        Ok(bindings.into_iter().collect())
    }
}

fn charge_preparation(work: &mut usize) -> Result<(), String> {
    *work += 1;
    if *work > 4_000_000 {
        return Err("ordered archive preparation work limit exceeded".into());
    }
    Ok(())
}
