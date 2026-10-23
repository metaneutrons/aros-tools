//! Bridge interpreted source events to generic finite rule instantiation.
//!
//! Headers have already expanded at their original read positions. They must
//! not be expanded again using the final variable state. Recipe text is still
//! deferred. This does not select an implicit rule, apply target variables or
//! authorize execution; every other active source event remains explicit.

use crate::{
    make_rule_events::{instantiate, RuleInstance, SourceRecipe, SourceRule},
    make_rule_header::RuleHeader,
    make_vars::{MakeFileEventKind, MakeFileProgram},
};

const MAX_EVENTS: usize = 65_536;
const MAX_RULE_BYTES: usize = 64 * 1024 * 1024;

/// An ordinary rule and deferred recipes from one interpreted input stream.
/// Event indices address that stream, not joined source lines or graph order.
pub struct InterpretedRule<'a> {
    event_index: usize,
    header: &'a RuleHeader,
    /// Tab-prefixed recipe events; an inline recipe remains in `header`.
    recipes: Vec<usize>,
    file: &'a MakeFileProgram,
}

/// No active non-rule event is silently claimed to have been discharged.
pub struct InterpretedRules<'a> {
    pub rules: Vec<InterpretedRule<'a>>,
    pub other_events: Vec<usize>,
    file: &'a MakeFileProgram,
}

/// Explicit/static declarations composed without discarding implicit headers.
/// Neither this graph nor the remaining event indices prove native selection.
pub struct ExplicitRuleProjection {
    pub graph: crate::make_rule_graph::RuleGraph,
    /// Original event positions of all implicit declarations, even recipe-less
    /// ones. Their search/cancellation semantics remain the caller's obligation.
    pub implicit_rule_events: Vec<usize>,
}

impl InterpretedRules<'_> {
    /// Compose finite declarations directly from the already-interpreted stream.
    ///
    /// Static patterns use the existing generic instantiator. Implicit patterns
    /// remain explicit obligations, not silently converted into extra edges.
    /// All `other_events` (including phony/suffix/search/default policies) also
    /// remain undispatched. Callers must handle them before native-scope claims.
    /// No header is re-expanded and no recipe is evaluated or executed.
    ///
    /// # Errors
    /// Refuses unsupported finite syntax, conflicting recipes and aggregate
    /// graph limits. Expansion is batched per bounded declaration, not retained
    /// for every declaration before aggregate limits are enforced.
    pub fn project_explicit(&self) -> Result<ExplicitRuleProjection, String> {
        if self.rules.len() > MAX_EVENTS || self.other_events.len() > MAX_EVENTS {
            return Err("interpreted Make projection exceeds event limit".into());
        }
        if self
            .rules
            .iter()
            .any(|rule| !std::ptr::eq(rule.file, self.file))
            || self
                .rules
                .windows(2)
                .any(|pair| pair[0].event_index >= pair[1].event_index)
        {
            return Err("interpreted Make projection has reordered or mixed source rules".into());
        }
        let implicit = |rule: &&InterpretedRule<'_>| {
            matches!(rule.header(), RuleHeader::Rule { targets, target_pattern: None, .. }
                if targets.contains('%'))
        };
        let implicit_rule_events = self
            .rules
            .iter()
            .filter(implicit)
            .map(InterpretedRule::event_index)
            .collect();
        let instances =
            self.rules
                .iter()
                .filter(|rule| !implicit(rule))
                .flat_map(|rule| match rule.instantiate_explicit() {
                    Ok(instances) => instances.into_iter().map(Ok).collect::<Vec<_>>(),
                    Err(error) => vec![Err(error)],
                });
        Ok(ExplicitRuleProjection {
            graph: crate::make_rule_graph::compose_fallible(instances)?,
            implicit_rule_events,
        })
    }
}

/// Collect active ordinary declarations without reparsing or re-expansion.
///
/// Variable, target-variable, control, include, default, phony, suffix and search
/// events remain in `other_events`, including recipes of `.DEFAULT`. Callers
/// must account for them before claiming complete selection/execution scope.
/// Borrowing retains the actual input/source/generated positions without
/// flattening includes or pairing recipes with a separate caller-supplied file.
/// Origin metadata is checked for consistency, not authenticated: separate eval
/// fragments can share the same input/generated/source-line coordinates.
///
/// # Errors
/// Refuses oversized input and active recipes with foreign, forward, inactive
/// or non-rule references. No partial collection is returned.
pub fn collect(file: &MakeFileProgram) -> Result<InterpretedRules<'_>, String> {
    if file.events.len() > MAX_EVENTS {
        return Err("interpreted Make rules exceed event limit".into());
    }
    let mut result = InterpretedRules {
        rules: Vec::new(),
        other_events: Vec::new(),
        file,
    };
    let mut rule_indices = vec![None; file.events.len()];
    let mut bytes = 0usize;
    for (index, event) in file.events.iter().enumerate() {
        if !event.active {
            continue;
        }
        let charge = match &event.kind {
            MakeFileEventKind::Rule(header) | MakeFileEventKind::DefaultRule(header) => {
                header_bytes(header)?
            }
            MakeFileEventKind::Recipe { text, .. } => text.len(),
            _ => 0,
        };
        bytes = bytes
            .checked_add(charge)
            .filter(|bytes| *bytes <= MAX_RULE_BYTES)
            .ok_or("interpreted Make rules exceed retained byte limit")?;
        match &event.kind {
            MakeFileEventKind::Rule(header @ RuleHeader::Rule { .. }) => {
                rule_indices[index] = Some(result.rules.len());
                result.rules.push(InterpretedRule {
                    event_index: index,
                    header,
                    recipes: Vec::new(),
                    file,
                });
            }
            MakeFileEventKind::Recipe { rule_event, .. } => {
                if *rule_event >= index || !file.events[*rule_event].active {
                    return Err("interpreted Make recipe has an invalid rule reference".into());
                }
                let origin = &file.events[*rule_event];
                if origin.input != event.input
                    || origin.generated != event.generated
                    || (event.generated && origin.source_line != event.source_line)
                {
                    return Err("interpreted Make recipe has a foreign source origin".into());
                }
                match &file.events[*rule_event].kind {
                    MakeFileEventKind::Rule(RuleHeader::Rule { .. }) => {
                        let rule = rule_indices[*rule_event]
                            .ok_or("interpreted Make recipe lost its rule")?;
                        result.rules[rule].recipes.push(index);
                    }
                    MakeFileEventKind::DefaultRule(RuleHeader::Rule { .. }) => {
                        result.other_events.push(index);
                    }
                    _ => return Err("interpreted Make recipe references a non-rule event".into()),
                }
            }
            _ => result.other_events.push(index),
        }
    }
    Ok(result)
}

impl InterpretedRule<'_> {
    /// Original event position, not the current rule's collection index.
    #[must_use]
    pub const fn event_index(&self) -> usize {
        self.event_index
    }

    /// Header already expanded by the source reader at its original position.
    #[must_use]
    pub const fn header(&self) -> &RuleHeader {
        self.header
    }

    /// Original event positions of the deferred tab-prefixed recipe lines.
    #[must_use]
    pub fn recipes(&self) -> &[usize] {
        &self.recipes
    }

    /// Instantiate a finite explicit/static-pattern rule from frozen headers.
    ///
    /// Header strings are data here, never expressions to expand again. The
    /// existing instantiator refuses unresolved/dollar/unsafe words and implicit
    /// patterns. Recipe text remains unchanged, and event indices retain exact
    /// source origins through includes and generated eval fragments. This does
    /// not supply a target-variable frame, inheritance or native phase proof.
    ///
    /// # Errors
    /// Returns ordinary finite-rule instantiation and resource-limit failures.
    pub fn instantiate_explicit(&self) -> Result<Vec<RuleInstance>, String> {
        instantiate(&self.source_rule()?, &|frozen, _| Ok(frozen.to_owned()))
    }

    /// Instantiate data-only implicit candidates for an explicit literal name.
    ///
    /// This retains this stream's actual header and recipes. It does not select
    /// a winning rule, resolve prerequisites or establish native recipe scope.
    /// See `make_rule_events::instantiate_implicit_candidate` for the bounded
    /// pattern grammar; unsupported directory semantics refuse, never default.
    ///
    /// # Errors
    /// Returns source consistency, pattern and finite candidate limit failures.
    pub fn instantiate_implicit_candidate(
        &self,
        target: &str,
    ) -> Result<Vec<RuleInstance>, String> {
        crate::make_rule_events::instantiate_implicit_candidate(&self.source_rule()?, target)
    }

    fn source_rule(&self) -> Result<SourceRule, String> {
        let RuleHeader::Rule {
            targets,
            target_pattern,
            prerequisites,
            order_only,
            double_colon,
            inline_recipe,
        } = self.header
        else {
            return Err("interpreted Make declaration is not an ordinary rule".into());
        };
        let source_line = self.file.events[self.event_index].source_line;
        let mut recipes =
            Vec::with_capacity(self.recipes.len() + usize::from(inline_recipe.is_some()));
        if let Some(text) = inline_recipe {
            recipes.push(SourceRecipe {
                output_line: self.event_index,
                source_line,
                text: text.clone(),
            });
        }
        for index in &self.recipes {
            let event = &self.file.events[*index];
            let MakeFileEventKind::Recipe { text, .. } = &event.kind else {
                return Err("interpreted Make recipe data differs".into());
            };
            recipes.push(SourceRecipe {
                output_line: *index,
                source_line: event.source_line,
                text: text.clone(),
            });
        }
        Ok(SourceRule {
            output_line: self.event_index,
            source_line,
            targets: targets.clone(),
            target_pattern: target_pattern.clone(),
            prerequisites: prerequisites.clone(),
            order_only: order_only.clone(),
            double_colon: *double_colon,
            recipes,
        })
    }
}

fn header_bytes(header: &RuleHeader) -> Result<usize, String> {
    let values: Vec<&str> = match header {
        RuleHeader::Rule {
            targets,
            target_pattern,
            prerequisites,
            order_only,
            inline_recipe,
            ..
        } => std::iter::once(targets.as_str())
            .chain(target_pattern.as_deref())
            .chain([prerequisites.as_str(), order_only.as_str()])
            .chain(inline_recipe.as_deref())
            .collect(),
        RuleHeader::TargetAssignment {
            targets,
            assignment,
        } => vec![targets, assignment],
    };
    values.into_iter().try_fold(0usize, |total, value| {
        total
            .checked_add(value.len())
            .ok_or_else(|| "interpreted Make header size overflow".into())
    })
}

#[cfg(test)]
#[path = "make_interpreted_rules_tests.rs"]
mod tests;
