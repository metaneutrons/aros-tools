//! Native projection of declared virtual routes and explicit selector hooks.
//!
//! Classic missing-target tolerance is deliberately not an admission rule.
//! No physical Make owner can become a producer through this metadata bridge.

use super::NativeOwnerProjection;
use crate::ast::MetaTargetRule;
use crate::graph::native_endpoint;
use crate::{DependencyGraph, TargetContext};
use aros_common::native_build_contract::{NativeMetaAbsence, NativeOptionalMetaDependency};
use aros_common::Diagnostic;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const MAX_WORK: usize = 4_000_000;
const MAX_ALIASES: usize = 20_000;

#[derive(Debug, Serialize)]
pub struct SourceVirtualAlias {
    pub name: String,
    pub dependencies: BTreeSet<String>,
    pub recipes: BTreeSet<String>,
}

#[derive(Debug, Serialize)]
pub struct SourceArchitectureHookOmission {
    pub target: String,
    pub dependency: String,
    pub recipes: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct VerifiedSelectorContract {
    pub recipe: String,
    pub target: String,
    pub expression: String,
}

/// Source-backed metadata routing, never executable producer qualification.
#[derive(Debug, Default, Serialize)]
pub struct NativeMetaSemanticsEvidence {
    pub virtual_aliases: Vec<SourceVirtualAlias>,
    pub architecture_hook_omissions: Vec<SourceArchitectureHookOmission>,
    pub verified_selector_contracts: BTreeSet<VerifiedSelectorContract>,
}

/// Exact inputs of the native selection, with no ambient fallback.
#[derive(Clone, Copy)]
pub struct NativeMetaSemanticsSelection<'a> {
    pub context: &'a TargetContext,
    pub roots: &'a [String],
    pub declarations: &'a [NativeOptionalMetaDependency],
    pub diagnostics: &'a [Diagnostic],
    /// Actual parser inputs of each raw native metadata edge, not aggregate
    /// endpoint origins (which include unrelated sibling prerequisites).
    pub meta_edge_origins: &'a BTreeMap<(String, String), BTreeSet<String>>,
}

impl NativeMetaSemanticsEvidence {
    /// Whether the exact source contract edge was independently verified.
    #[must_use]
    pub fn verifies(&self, edge: &NativeOptionalMetaDependency) -> bool {
        edge.absence == NativeMetaAbsence::Selector
            && self
                .verified_selector_contracts
                .contains(&VerifiedSelectorContract {
                    recipe: edge.recipe.clone(),
                    target: edge.target.clone(),
                    expression: edge.dependency.clone(),
                })
    }
}

impl NativeOwnerProjection {
    /// Import only reachable, source-declared pure virtual routes. Optional
    /// leaves require an exact selector contract and complete edge provenance.
    /// Real source owners, literal typos and rejected producers stay required.
    ///
    /// # Errors
    /// Refuses changed inputs, unmatched source contracts, unsafe bindings and
    /// bounded traversal excess. Does not admit disabled concrete owners.
    pub fn bind_native_meta_semantics(
        &self,
        root: &Path,
        graph: &mut DependencyGraph,
        selection: NativeMetaSemanticsSelection<'_>,
        parser_origins: &mut BTreeMap<String, BTreeSet<String>>,
    ) -> Result<NativeMetaSemanticsEvidence, String> {
        let NativeMetaSemanticsSelection {
            context,
            roots,
            declarations,
            diagnostics,
            meta_edge_origins,
        } = selection;
        self.verify(root)?;
        if roots.is_empty() {
            return Err("native metadata semantics require resolved roots".into());
        }
        let mut work = 0usize;
        let mut evidence = NativeMetaSemanticsEvidence::default();
        let mut edge_origins = BTreeMap::<(String, String), BTreeSet<String>>::new();
        for ((target, dependency), origins) in meta_edge_origins {
            charge(&mut work)?;
            if let (Ok(target), Ok(dependency)) = (
                native_endpoint(target, context),
                native_endpoint(dependency, context),
            ) {
                edge_origins
                    .entry((target, dependency))
                    .or_default()
                    .extend(origins.iter().cloned());
            }
        }
        let mut by_target = BTreeMap::<String, Vec<_>>::new();
        for declaration in self.graph.declarations() {
            charge(&mut work)?;
            by_target
                .entry(declaration.target.clone())
                .or_default()
                .push(declaration);
        }

        // Bind virtual declarations before looking at optional leaves. Real
        // owners never become producers here. For an already represented
        // endpoint, union virtual metadata without replacing its provider.
        let mut imported = BTreeSet::new();
        let mut imported_edges = BTreeSet::new();
        let mut native_meta_edges = bound_meta_edges(graph, context, &mut work)?;
        loop {
            let audit = graph.audit_native_dependency_graph(roots, context, diagnostics);
            let known_native = graph.native_metadata_endpoint_names(context);
            let mut added = false;
            for name in audit.reachable {
                charge(&mut work)?;
                if !self.graph.known_targets().contains(&name)
                    || imported.contains(&name)
                    || (self.graph.owner_files(&name).is_some() && !known_native.contains(&name))
                {
                    continue;
                }
                let source_declarations = by_target.get(&name).ok_or_else(|| {
                    format!("source virtual target {name} lacks declaration provenance")
                })?;
                let mut recipes = BTreeSet::new();
                let mut dependencies = BTreeSet::new();
                for declaration in source_declarations {
                    charge(&mut work)?;
                    if !declaration.virtual_target || declaration.claims_make_owner {
                        continue;
                    }
                    recipes.insert(self.bound_recipe(&declaration.file)?);
                    dependencies.extend(
                        declaration
                            .dependencies
                            .iter()
                            .map(|edge| edge.concrete.clone()),
                    );
                }
                if recipes.is_empty() {
                    imported.insert(name);
                    continue;
                }
                for dependency in &dependencies {
                    charge(&mut work)?;
                    require_concrete(dependency, context)?;
                    // Only edges introduced here can use a concrete spelling
                    // as selector proof. A pre-existing literal is not waived.
                    if !native_meta_edges.contains(&(name.clone(), dependency.clone())) {
                        imported_edges.insert((name.clone(), dependency.clone()));
                        edge_origins
                            .entry((name.clone(), dependency.clone()))
                            .or_default()
                            .extend(recipes.iter().cloned());
                    }
                }
                require_concrete(&name, context)?;
                let added_dependencies: Vec<_> = dependencies
                    .iter()
                    .filter(|dependency| {
                        !native_meta_edges.contains(&(name.clone(), (*dependency).clone()))
                    })
                    .cloned()
                    .collect();
                if known_native.contains(&name) && added_dependencies.is_empty() {
                    imported.insert(name);
                    continue;
                }
                native_meta_edges.extend(
                    added_dependencies
                        .iter()
                        .map(|child| (name.clone(), child.clone())),
                );
                graph.add_meta_rule(MetaTargetRule {
                    name: name.clone(),
                    dependencies: added_dependencies,
                });
                parser_origins
                    .entry(name.clone())
                    .or_default()
                    .extend(recipes.iter().cloned());
                evidence.virtual_aliases.push(SourceVirtualAlias {
                    name: name.clone(),
                    dependencies,
                    recipes,
                });
                imported.insert(name);
                if evidence.virtual_aliases.len() > MAX_ALIASES {
                    return Err("native virtual route limit exceeded".into());
                }
                added = true;
            }
            if !added {
                break;
            }
        }

        let selected = graph
            .audit_native_dependency_graph(roots, context, diagnostics)
            .reachable;
        let mut seeds = BTreeMap::<(String, String), BTreeSet<String>>::new();
        for edge in declarations
            .iter()
            .filter(|edge| edge.absence == NativeMetaAbsence::Selector)
        {
            charge(&mut work)?;
            let dependency =
                native_endpoint(&edge.dependency, context).map_err(|e| e.to_string())?;
            let mut matches = false;
            for declaration in by_target.get(&edge.target).into_iter().flatten() {
                charge(&mut work)?;
                for source_edge in &declaration.dependencies {
                    charge(&mut work)?;
                    matches |= self.owners.get(&declaration.file) == Some(&edge.recipe)
                        && source_edge.concrete == dependency
                        && architecture_expression(&source_edge.raw_expression).as_deref()
                            == Some(edge.dependency.as_str());
                }
            }
            if !matches || !native_meta_edges.contains(&(edge.target.clone(), dependency.clone())) {
                return Err(format!(
                    "selector contract {} -> {} is not its exact source declaration in {}",
                    edge.target, edge.dependency, edge.recipe
                ));
            }
            evidence
                .verified_selector_contracts
                .insert(VerifiedSelectorContract {
                    recipe: edge.recipe.clone(),
                    target: edge.target.clone(),
                    expression: edge.dependency.clone(),
                });
            if selected.contains(&edge.target) {
                seeds
                    .entry((edge.target.clone(), dependency))
                    .or_default()
                    .insert(edge.recipe.clone());
            }
        }

        // Contract seeds are exact source-owned edges. Their declared virtual
        // routes may have selector leaves, but not arbitrary literal holes.
        let mut candidates = BTreeMap::<(String, String), bool>::new();
        for (target, dependency) in seeds.keys() {
            candidates.insert((target.clone(), dependency.clone()), true);
            let mut pending = BTreeSet::from([dependency.clone()]);
            let mut visited = BTreeSet::new();
            while let Some(name) = pending.pop_first() {
                charge(&mut work)?;
                if !visited.insert(name.clone()) || self.graph.owner_files(&name).is_some() {
                    continue;
                }
                if let Some(children) = self.graph.dependencies(&name) {
                    for child in children {
                        charge(&mut work)?;
                        candidates
                            .entry((name.clone(), child.clone()))
                            .or_insert(false);
                        pending.insert(child.clone());
                    }
                }
            }
        }
        let rejected: BTreeSet<_> = diagnostics
            .iter()
            .filter_map(|diagnostic| {
                diagnostic
                    .context
                    .as_ref()?
                    .target
                    .as_ref()
                    .and_then(|name| native_endpoint(name, context).ok())
            })
            .collect();
        // A seed can cut required ingress only when *all* claims agree,
        // including native origins/spellings. One colliding declaration must
        // not authorize omissions further down a shared virtual route.
        let mut cuts = BTreeSet::new();
        for (target, dependency) in seeds.keys() {
            charge(&mut work)?;
            let mut recipes = BTreeSet::new();
            let mut expressions = BTreeSet::new();
            let mut justified = true;
            let mut claims = 0usize;
            for declaration in by_target.get(target).into_iter().flatten() {
                for source_edge in &declaration.dependencies {
                    charge(&mut work)?;
                    if &source_edge.concrete != dependency {
                        continue;
                    }
                    claims += 1;
                    let recipe = self.bound_recipe(&declaration.file)?;
                    if let Some(expression) = architecture_expression(&source_edge.raw_expression) {
                        justified &= evidence.verified_selector_contracts.contains(
                            &VerifiedSelectorContract {
                                recipe: recipe.clone(),
                                target: target.clone(),
                                expression: expression.clone(),
                            },
                        );
                        expressions.insert(expression);
                    } else {
                        justified = false;
                    }
                    recipes.insert(recipe);
                }
            }
            if claims > 0
                && justified
                && edge_origins
                    .get(&(target.clone(), dependency.clone()))
                    .is_some_and(|origins| origins.is_subset(&recipes))
                && native_edge_spellings_proven(
                    graph,
                    target,
                    dependency,
                    context,
                    &expressions,
                    imported_edges.contains(&(target.clone(), dependency.clone())),
                )
            {
                cuts.insert((target.clone(), dependency.clone()));
            }
        }
        let required = graph.native_required_metadata_reachability(roots, context, &cuts)?;
        // A native-only concrete provider also prevents omission. Missing
        // endpoints have no registry entry; diagnostic-only endpoints above
        // remain protected even when they have no source Make owner.
        let known_native = graph.native_metadata_endpoint_names(context);
        let mut omissions = Vec::new();
        for ((target, dependency), seed) in candidates {
            charge(&mut work)?;
            if self.graph.known_targets().contains(&dependency)
                || known_native.contains(&dependency)
                || rejected.contains(&dependency)
                || !native_meta_edges.contains(&(target.clone(), dependency.clone()))
                || (!seed && required.contains(&target))
            {
                continue;
            }
            let mut recipes = BTreeSet::new();
            let mut justified = true;
            let mut claims = 0usize;
            let mut expressions = BTreeSet::new();
            for declaration in by_target.get(&target).into_iter().flatten() {
                charge(&mut work)?;
                for source_edge in &declaration.dependencies {
                    charge(&mut work)?;
                    if source_edge.concrete != dependency {
                        continue;
                    }
                    claims += 1;
                    let recipe = self.bound_recipe(&declaration.file)?;
                    let expression = architecture_expression(&source_edge.raw_expression);
                    if let Some(expression) = &expression {
                        expressions.insert(expression.clone());
                    }
                    let exact_seed = expression.as_ref().is_some_and(|expression| {
                        evidence
                            .verified_selector_contracts
                            .contains(&VerifiedSelectorContract {
                                recipe: recipe.clone(),
                                target: target.clone(),
                                expression: expression.clone(),
                            })
                    });
                    // Every source declaration of this unioned edge must
                    // agree. A colliding literal or nonvirtual route cannot
                    // borrow another file's optional selector proof.
                    if expression.is_none()
                        || (seed && !exact_seed)
                        || (!seed && (!declaration.virtual_target || declaration.claims_make_owner))
                    {
                        justified = false;
                    }
                    recipes.insert(recipe);
                }
            }
            if claims == 0 || !justified {
                continue;
            }
            // Reject extra native origins and literal/native-generated edges
            // which merely collide with a proven selector's bound identity.
            if edge_origins
                .get(&(target.clone(), dependency.clone()))
                .is_none_or(|origins| !origins.is_subset(&recipes))
                || !native_edge_spellings_proven(
                    graph,
                    &target,
                    &dependency,
                    context,
                    &expressions,
                    imported_edges.contains(&(target.clone(), dependency.clone())),
                )
            {
                continue;
            }
            omissions.push(SourceArchitectureHookOmission {
                target,
                dependency,
                recipes,
            });
        }
        // No graph mutation of absent edges until every seed is verified.
        self.verify(root)?;
        for omission in &omissions {
            graph.remove_native_meta_edge(&omission.target, &omission.dependency, context);
        }
        evidence.architecture_hook_omissions = omissions;
        Ok(evidence)
    }

    fn bound_recipe(&self, file: &str) -> Result<String, String> {
        let recipe = self
            .owners
            .get(file)
            .ok_or_else(|| format!("virtual route has unbound source input {file}"))?;
        if !self.snapshots.contains_key(recipe) {
            return Err(format!(
                "virtual route source {recipe} lacks captured bytes"
            ));
        }
        Ok(recipe.clone())
    }
}

fn charge(work: &mut usize) -> Result<(), String> {
    *work = work.checked_add(1).ok_or("native metadata work overflow")?;
    if *work > MAX_WORK {
        return Err("native metadata work limit exceeded".into());
    }
    Ok(())
}

fn require_concrete(name: &str, context: &TargetContext) -> Result<(), String> {
    let bound = native_endpoint(name, context).map_err(|e| e.to_string())?;
    if bound != name {
        return Err(format!(
            "source virtual endpoint is not concretely bound: {name}"
        ));
    }
    Ok(())
}

fn bound_meta_edges(
    graph: &DependencyGraph,
    context: &TargetContext,
    work: &mut usize,
) -> Result<BTreeSet<(String, String)>, String> {
    let mut edges = BTreeSet::new();
    for (name, dependencies) in &graph.meta_targets {
        charge(work)?;
        let Ok(name) = native_endpoint(name, context) else {
            continue;
        };
        for dependency in dependencies {
            charge(work)?;
            if let Ok(dependency) = native_endpoint(dependency, context) {
                edges.insert((name.clone(), dependency));
            }
        }
    }
    Ok(edges)
}

fn native_edge_spellings_proven(
    graph: &DependencyGraph,
    target: &str,
    dependency: &str,
    context: &TargetContext,
    expressions: &BTreeSet<String>,
    imported: bool,
) -> bool {
    graph
        .meta_targets
        .iter()
        .filter(|(name, _)| native_endpoint(name, context).ok().as_deref() == Some(target))
        .flat_map(|(_, dependencies)| dependencies)
        .filter(|name| native_endpoint(name, context).ok().as_deref() == Some(dependency))
        .all(|name| expressions.contains(name) || (imported && name == dependency))
}

/// A strict source selector expression, without identifier sanitization.
fn architecture_expression(raw: &str) -> Option<String> {
    let mut rest = raw;
    let mut out = String::new();
    let mut selectors = 0usize;
    while let Some(start) = rest.find("$(") {
        let literal = &rest[..start];
        if !safe_literal(literal) {
            return None;
        }
        out.push_str(literal);
        let after = &rest[start + 2..];
        let end = after.find(')')?;
        let canonical = match &after[..end] {
            "ARCH" | "AROS_TARGET_ARCH" => "AROS_TARGET_PLATFORM",
            "CPU" | "AROS_TARGET_CPU" => "AROS_TARGET_CPU",
            "FAMILY" | "AROS_TARGET_FAMILY" => "AROS_TARGET_FAMILY",
            "AROS_TARGET_PLATFORM" => "AROS_TARGET_LEGACY_PLATFORM",
            "AROS_TARGET_VARIANT" => "AROS_TARGET_VARIANT",
            "AROS_TARGET_CPU32" => "AROS_TARGET_CPU32",
            _ => return None,
        };
        out.push_str("${");
        out.push_str(canonical);
        out.push('}');
        selectors += 1;
        rest = &after[end + 1..];
    }
    if !safe_literal(rest) || selectors == 0 {
        return None;
    }
    out.push_str(rest);
    Some(out)
}

fn safe_literal(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::architecture_expression;

    #[test]
    fn selectors_are_exact_not_sanitized_or_inferred() {
        assert_eq!(
            architecture_expression("hook-$(ARCH)-$(CPU)-$(AROS_TARGET_VARIANT)").as_deref(),
            Some("hook-${AROS_TARGET_PLATFORM}-${AROS_TARGET_CPU}-${AROS_TARGET_VARIANT}")
        );
        for raw in [
            "literal-typo",
            "hook-$(UNKNOWN)",
            "hook-$(shell echo hi)",
            "hook-$(CPU)+unsafe",
            "hook-${AROS_TARGET_CPU}",
        ] {
            assert!(architecture_expression(raw).is_none(), "accepted {raw}");
        }
    }
}
