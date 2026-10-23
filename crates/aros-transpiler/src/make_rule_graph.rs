//! Finite explicit-target rule composition, independent of language or owner.
//!
//! This layer consumes already positionally resolved rule instances. It does
//! not resolve paths, inherit variables, choose implicit rules or execute recipes.
//! Ordinary declarations compose; double-colon declarations remain independent.

use crate::make_rule_events::RuleInstance;
use std::collections::{BTreeMap, BTreeSet};

pub mod traversal;

const MAX_DECLARATIONS: usize = 16_384;
const MAX_RETAINED_BYTES: usize = 64 * 1024 * 1024;

/// All declarations for one target, in original output order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetRules {
    Ordinary(OrdinaryTarget),
    DoubleColon(Vec<RuleInstance>),
}

/// One ordinary target with at most one explicitly supplied recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrdinaryTarget {
    declarations: Vec<RuleInstance>,
    recipe_index: Option<usize>,
}

impl OrdinaryTarget {
    fn declaration_order(&self) -> impl Iterator<Item = usize> + '_ {
        let primary = self.recipe_index.unwrap_or(0);
        std::iter::once(primary)
            .chain((0..self.declarations.len()).filter(move |index| *index != primary))
    }

    /// Prerequisite traversal order, not the grouped automatic-variable lists.
    /// Each declaration retains normal then order-only inputs in its own source
    /// position, with the recipe-bearing declaration first. Grouping all normal
    /// inputs before every order-only input would change first-parent selection.
    pub fn prerequisite_walk(&self) -> impl Iterator<Item = &str> {
        self.declaration_order().flat_map(|index| {
            let declaration = &self.declarations[index];
            declaration.prerequisite_order.iter().map(String::as_str)
        })
    }
    /// Original records remain available for provenance and variable scope.
    #[must_use]
    pub fn declarations(&self) -> &[RuleInstance] {
        &self.declarations
    }

    /// Compose prerequisites with the recipe-bearing declaration first, then
    /// all other declarations in original order. Do not deduplicate normal
    /// prerequisites: `$+` must retain repeated inputs. Automatic `$^` and `$|`
    /// normalization remains in the common instance evaluator.
    /// A separate interleaved prerequisite order preserves dependency walking
    /// and the unique normal-list order when an order-only input is promoted.
    ///
    /// The last static-pattern declaration supplies a known stem. No explicit
    /// suffix-based stem or implicit-rule search is inferred. Recipe metadata
    /// stays attached to the declaration that actually supplies that recipe.
    #[must_use]
    pub fn effective_instance(&self) -> RuleInstance {
        let primary = self.recipe_index.unwrap_or(0);
        let mut effective = self.declarations[primary].clone();
        effective.prerequisites.clear();
        effective.order_only.clear();
        effective.prerequisite_order = self.prerequisite_walk().map(str::to_owned).collect();
        for index in self.declaration_order() {
            let declaration = &self.declarations[index];
            effective
                .prerequisites
                .extend(declaration.prerequisites.iter().cloned());
            effective
                .order_only
                .extend(declaration.order_only.iter().cloned());
        }
        effective.stem = self
            .declarations
            .iter()
            .rev()
            .find_map(|declaration| declaration.stem.clone());
        effective
    }
}

/// A finite graph's target records; this is not a build schedule or proof of
/// complete source coverage. Callers must separately discharge non-rule events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleGraph {
    pub targets: BTreeMap<String, TargetRules>,
    /// First declaration order, not lexicographic map iteration order.
    pub target_order: Vec<String>,
}

/// Compose already resolved declarations in original output-line order.
///
/// Inputs are internal projection data, not authenticated persisted records.
/// Their strings must already have passed the rule instantiator's expression
/// checks; source/output-root binding remains the caller's responsibility.
/// Instances from the same output line retain the caller's original target
/// order: line numbers alone cannot independently verify that ordering.
/// The size bound covers retained logical data with conservative metadata,
/// not allocator capacity/RSS of arbitrarily fabricated public input values.
/// No partially composed graph is returned on failure.
///
/// # Errors
/// Rejects resource exhaustion, empty targets, out-of-order/duplicate origins,
/// mixed ordinary/double-colon declarations, or multiple ordinary recipes.
/// GNU Make can override recipes with warnings; this bounded mode refuses that
/// ambiguity rather than silently discarding a recipe's effects.
pub fn compose(instances: Vec<RuleInstance>) -> Result<RuleGraph, String> {
    compose_with_limits(instances, MAX_DECLARATIONS, MAX_RETAINED_BYTES)
}

fn compose_with_limits(
    instances: Vec<RuleInstance>,
    max_declarations: usize,
    max_retained_bytes: usize,
) -> Result<RuleGraph, String> {
    if instances.len() > max_declarations {
        return Err("Make rule graph exceeds declaration limit".into());
    }
    compose_result_iter_with_limits(
        instances.into_iter().map(Ok),
        max_declarations,
        max_retained_bytes,
    )
}

/// Compose incrementally, stopping at the first expansion or graph failure.
/// This avoids collecting every expanded source declaration before the graph's
/// aggregate count/byte checks. Each producer must bound its own current batch.
pub(crate) fn compose_fallible(
    instances: impl IntoIterator<Item = Result<RuleInstance, String>>,
) -> Result<RuleGraph, String> {
    compose_result_iter_with_limits(instances, MAX_DECLARATIONS, MAX_RETAINED_BYTES)
}

fn compose_result_iter_with_limits(
    instances: impl IntoIterator<Item = Result<RuleInstance, String>>,
    max_declarations: usize,
    max_retained_bytes: usize,
) -> Result<RuleGraph, String> {
    let mut result = RuleGraph::default();
    let mut retained_bytes = 0usize;
    let mut previous_line = None;
    let mut origins = BTreeSet::new();
    for (index, instance) in instances.into_iter().enumerate() {
        if index >= max_declarations {
            return Err("Make rule graph exceeds declaration limit".into());
        }
        let instance = instance?;
        if instance.target.is_empty() {
            return Err("Make rule graph has an empty target".into());
        }
        instance.validate_declaration_counts()?;
        retained_bytes = retained_bytes
            .checked_add(instance_bytes(
                &instance,
                max_retained_bytes.saturating_sub(retained_bytes),
            )?)
            .filter(|bytes| *bytes <= max_retained_bytes)
            .ok_or("Make rule graph exceeds retained byte limit")?;
        instance.validate_declaration_order()?;
        if previous_line.is_some_and(|line| line > instance.output_line)
            || !origins.insert((instance.output_line, instance.target.clone()))
        {
            return Err("Make rule graph has unordered or duplicate declaration origins".into());
        }
        previous_line = Some(instance.output_line);
        let key = instance.target.clone();
        if let Some(target) = result.targets.get_mut(&key) {
            match target {
                TargetRules::Ordinary(target) if !instance.double_colon => {
                    if !instance.recipes.is_empty() {
                        if target.recipe_index.is_some() {
                            return Err("Make ordinary target has multiple recipe providers".into());
                        }
                        target.recipe_index = Some(target.declarations.len());
                    }
                    target.declarations.push(instance);
                }
                TargetRules::DoubleColon(declarations) if instance.double_colon => {
                    declarations.push(instance);
                }
                _ => return Err("Make target mixes ordinary and double-colon rules".into()),
            }
        } else {
            result.target_order.push(key.clone());
            let target = if instance.double_colon {
                TargetRules::DoubleColon(vec![instance])
            } else {
                TargetRules::Ordinary(OrdinaryTarget {
                    recipe_index: (!instance.recipes.is_empty()).then_some(0),
                    declarations: vec![instance],
                })
            };
            result.targets.insert(key, target);
        }
    }
    Ok(result)
}

fn instance_bytes(instance: &RuleInstance, limit: usize) -> Result<usize, String> {
    // Conservatively include auxiliary target keys/order/origin copies and
    // per-declaration collection metadata before allocating those copies.
    let mut bytes = instance
        .target
        .len()
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(256 + std::mem::size_of::<RuleInstance>()))
        .ok_or("Make rule graph size overflow")?;
    if bytes > limit {
        return Err("Make rule graph exceeds retained byte limit".into());
    }
    let strings = std::iter::once(instance.target.as_str())
        .chain(instance.stem.as_deref())
        .chain(instance.prerequisites.iter().map(String::as_str))
        .chain(instance.order_only.iter().map(String::as_str))
        .chain(instance.prerequisite_order.iter().map(String::as_str))
        .chain(instance.recipes.iter().map(|recipe| recipe.text.as_str()));
    for value in strings {
        bytes = bytes
            .checked_add(value.len())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<String>()))
            .filter(|bytes| *bytes <= limit)
            .ok_or("Make rule graph exceeds retained byte limit")?;
    }
    bytes = bytes
        .checked_add(
            instance
                .recipes
                .len()
                .checked_mul(std::mem::size_of::<crate::make_rule_events::SourceRecipe>())
                .ok_or("Make recipe metadata size overflow")?,
        )
        .filter(|bytes| *bytes <= limit)
        .ok_or("Make rule graph exceeds retained byte limit")?;
    Ok(bytes)
}

#[cfg(test)]
#[path = "make_rule_graph_tests.rs"]
mod tests;
