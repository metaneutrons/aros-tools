//! Bounded first-encounter traversal of an explicitly resolved finite graph.
//!
//! This is dependency/scope selection, not timestamp scheduling, implicit rule
//! search, VPATH resolution, native execution authentication or permission to
//! execute recipes. The caller must discharge those separate source policies.

use super::{instance_bytes, RuleGraph, TargetRules};
use crate::{make_rule_events::RuleInstance, make_rule_header::is_control_target};
use std::collections::BTreeMap;

const MAX_TARGETS: usize = 16_384;
const MAX_EDGES: usize = 262_144;
const MAX_DEPTH: usize = 64;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 1024 * 1024;

/// One first-encounter target and its retained effective explicit declaration.
///
/// A missing declaration denotes an external input: independently admitted in
/// `ExplicitTraversal`, but still unresolved in `DeclaredTrace`.
#[derive(Debug)]
pub struct TargetVisit {
    target: String,
    parent: Option<usize>,
    instance: Option<RuleInstance>,
}

impl TargetVisit {
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// First parent's visit index, or none for a first-encounter root goal.
    #[must_use]
    pub const fn parent(&self) -> Option<usize> {
        self.parent
    }

    #[must_use]
    pub const fn instance(&self) -> Option<&RuleInstance> {
        self.instance.as_ref()
    }
}

/// Internally consistent paths derived from one graph/goals traversal. These
/// records are data, not proof that a native owner selected the same graph.
#[derive(Debug, Default)]
pub struct ExplicitTraversal {
    visits: Vec<TargetVisit>,
    completion_order: Vec<usize>,
    goals: Vec<usize>,
}

impl ExplicitTraversal {
    #[must_use]
    pub fn visits(&self) -> &[TargetVisit] {
        &self.visits
    }

    /// Dependency-first order; recipes need independent timestamp/phase checks.
    #[must_use]
    pub fn completion_order(&self) -> &[usize] {
        &self.completion_order
    }

    /// Original goal order, retaining repeated references to an existing visit.
    #[must_use]
    pub fn goals(&self) -> &[usize] {
        &self.goals
    }

    /// Build the selected first-parent variable path without copying target text.
    ///
    /// # Errors
    /// Refuses an index outside this traversal. Paths are bounded at construction.
    pub fn scope_chain(&self, index: usize) -> Result<Vec<&str>, String> {
        let mut current = Some(index);
        let mut chain = Vec::new();
        while let Some(index) = current {
            let visit = self
                .visits
                .get(index)
                .ok_or("Make traversal visit is unavailable")?;
            chain.push(visit.target());
            current = visit.parent;
        }
        chain.reverse();
        Ok(chain)
    }
}

/// Declaration-only dependency paths, with every non-graph leaf unresolved.
///
/// This deliberately has no conversion to an admitted `ExplicitTraversal`.
/// It never claims input existence, complete implicit/default search, native
/// first-parent scheduling or permission to execute a selected recipe. A
/// caller may separately project inert recipe expressions in these declared
/// frames; that does not admit inputs or authenticate native recipe selection.
#[derive(Debug)]
pub struct DeclaredTrace {
    trace: ExplicitTraversal,
}

impl DeclaredTrace {
    #[must_use]
    pub fn visits(&self) -> &[TargetVisit] {
        self.trace.visits()
    }

    /// Dependency-first order within this declaration-only trace.
    ///
    /// This is a deterministic graph order, not admission of unresolved inputs,
    /// Make scheduling, or permission to execute any recipe.
    #[must_use]
    pub fn completion_order(&self) -> &[usize] {
        self.trace.completion_order()
    }

    #[must_use]
    pub fn goals(&self) -> &[usize] {
        self.trace.goals()
    }

    /// All external declarations, not a list of existing or absent files.
    pub fn unresolved_inputs(&self) -> impl Iterator<Item = (usize, &TargetVisit)> {
        self.visits()
            .iter()
            .enumerate()
            .filter(|(_, visit)| visit.instance().is_none())
    }

    /// First-encounter path in this declared graph only, not native scope proof.
    ///
    /// # Errors
    /// Refuses an index outside this bounded trace.
    pub fn scope_chain(&self, index: usize) -> Result<Vec<&str>, String> {
        self.trace.scope_chain(index)
    }
}

/// Trace finite declared rules without pretending that external inputs exist.
///
/// The same declaration order, identity, cycle and resource checks as strict
/// traversal apply. External leaves are retained for later owner/phase/input
/// resolution; no file lookup is attempted. Source policies may still add or
/// change dependencies. Do not use this trace as a native buildability proof.
///
/// # Errors
/// Refuses malformed/control/double-colon rules, cycles and shared limits.
pub fn trace_declared(graph: &RuleGraph, goals: &[&str]) -> Result<DeclaredTrace, String> {
    Ok(DeclaredTrace {
        trace: walk(graph, goals, None)?,
    })
}

/// Traverse supplied explicit goals in order, retaining the first parent of
/// shared prerequisites.
///
/// All targets are visited at most once; normal/repeated/
/// order-only edges retain their declaration-local positions. No ambient file
/// lookup is performed. `existing_input` must independently prove leaves absent
/// from the explicit graph; false refuses rather than guessing an implicit rule.
///
/// Recipe-less declarations still remain explicit graph nodes, not a claim that
/// GNU Make would omit implicit/default search. Callers must prove that such
/// searches cannot add a selected rule before using the path as native scope.
/// Double-colon scheduling, cycles, control targets and unsupported names refuse.
/// No partial traversal is returned, and this never executes commands or proves
/// a parallel scheduler's native first-parent decision.
///
/// # Errors
/// Refuses unbound inputs, unsupported selected rules, invalid graph identity,
/// empty goals and depth/edge/target/retained-data exhaustion.
pub fn select_explicit(
    graph: &RuleGraph,
    goals: &[&str],
    existing_input: &impl Fn(&str) -> Result<bool, String>,
) -> Result<ExplicitTraversal, String> {
    walk(graph, goals, Some(existing_input))
}

type ExistingInput<'a> = &'a dyn Fn(&str) -> Result<bool, String>;

fn walk(
    graph: &RuleGraph,
    goals: &[&str],
    existing_input: Option<ExistingInput<'_>>,
) -> Result<ExplicitTraversal, String> {
    if goals.is_empty() || goals.len() > MAX_TARGETS || graph.targets.len() > MAX_TARGETS {
        return Err("Make traversal requires bounded explicit goals and targets".into());
    }
    let mut walker = Walker {
        graph,
        existing_input,
        result: ExplicitTraversal::default(),
        seen: BTreeMap::new(),
        edges: 0,
        bytes: goals.len() * std::mem::size_of::<usize>(),
    };
    for goal in goals {
        let visit = walker.visit(goal, None, 0)?;
        walker.result.goals.push(visit);
    }
    Ok(walker.result)
}

struct Walker<'a> {
    graph: &'a RuleGraph,
    existing_input: Option<ExistingInput<'a>>,
    result: ExplicitTraversal,
    seen: BTreeMap<String, (usize, bool)>,
    edges: usize,
    bytes: usize,
}

impl Walker<'_> {
    fn charge(&mut self, bytes: usize) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|size| *size <= MAX_BYTES)
            .ok_or("Make traversal exceeds retained data limit")?;
        Ok(())
    }

    fn visit(
        &mut self,
        target: &str,
        parent: Option<usize>,
        depth: usize,
    ) -> Result<usize, String> {
        // Repeated/shared edges still incur name validation and lookup work.
        // Charge before the seen fast path rather than accepting unbounded
        // rescans of long duplicate goals or prerequisites.
        self.charge(target.len())?;
        validate_name(target)?;
        if let Some((index, complete)) = self.seen.get(target) {
            return if *complete {
                Ok(*index)
            } else {
                Err("Make traversal contains a dependency cycle".into())
            };
        }
        if depth >= MAX_DEPTH || self.result.visits.len() == MAX_TARGETS {
            return Err("Make traversal exceeds depth or target limit".into());
        }
        self.charge(target.len().saturating_mul(3).saturating_add(256))?;
        let graph = self.graph;
        let rule = match graph.targets.get(target) {
            Some(TargetRules::Ordinary(rule)) => {
                for declaration in rule.declarations() {
                    if declaration.target != target || declaration.double_colon {
                        return Err("Make traversal graph/declaration identity differs".into());
                    }
                    declaration.validate_declaration_counts()?;
                    self.charge(instance_bytes(
                        declaration,
                        MAX_BYTES.saturating_sub(self.bytes),
                    )?)?;
                    declaration.validate_declaration_order()?;
                }
                Some(rule)
            }
            Some(TargetRules::DoubleColon(_)) => {
                return Err("Make traversal does not schedule double-colon rules".into())
            }
            None => {
                if let Some(existing_input) = self.existing_input {
                    if !existing_input(target)? {
                        return Err(
                            "Make traversal leaf has no selected rule or admitted input".into()
                        );
                    }
                }
                None
            }
        };
        let index = self.result.visits.len();
        self.result.visits.push(TargetVisit {
            target: target.into(),
            parent,
            instance: rule.map(super::OrdinaryTarget::effective_instance),
        });
        self.seen.insert(target.into(), (index, false));
        if let Some(rule) = rule {
            for prerequisite in rule.prerequisite_walk() {
                self.edges = self
                    .edges
                    .checked_add(1)
                    .filter(|edges| *edges <= MAX_EDGES)
                    .ok_or("Make traversal exceeds prerequisite edge limit")?;
                self.visit(prerequisite, Some(index), depth + 1)?;
            }
        }
        self.seen
            .get_mut(target)
            .ok_or("Make traversal lost its target")?
            .1 = true;
        self.result.completion_order.push(index);
        Ok(index)
    }
}

fn validate_name(target: &str) -> Result<(), String> {
    if target.is_empty()
        || target.len() > MAX_NAME_BYTES
        || is_control_target(target)
        || target.chars().any(|c| {
            c.is_control()
                || c.is_whitespace()
                || matches!(c, '$' | ':' | ';' | '|' | '=' | '\\' | '%')
        })
    {
        return Err("Make traversal requires a safe concrete target name".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
