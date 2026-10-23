//! Ordered target-local bindings, distinct from the changing global state.

use super::{
    charge_work, check_origin_write, evaluate, evaluate_with_queries, guard, inspect, invalid,
    lookup, record_assignment, reference_absent, retained_cost, variable_assignment_impl,
    AssignmentKind, AssignmentValue, Evaluation, MakeExprError, MakeProgramPaths,
    MakeVariableProgram, State, VariableFlavor, MAX_STATEMENTS, MAX_STORAGE, MAX_VALUE,
};
use crate::{make_expr::MakeVariableQuery, make_rule_header::RuleHeader};
use aros_common::make_namespace::MakeVariableOrigin;
use std::{cell::RefCell, collections::BTreeSet};

mod patterns;
mod readonly;
use patterns::matching_bindings;

#[derive(Clone)]
pub(super) struct Binding {
    raw: String,
    flavor: VariableFlavor,
    append_parent: bool,
}

/// A pattern definition is an operation, not a final per-pattern binding.
/// Matching interleaves different patterns, so even repeated identical patterns
/// cannot be collapsed. Only simple assignments freeze their RHS while reading.
#[derive(Clone)]
pub(super) struct PatternAssignment {
    pattern: String,
    name: String,
    raw: String,
    kind: AssignmentKind,
}

pub(super) fn read_assignment(
    targets: &str,
    assignment: &str,
    line: usize,
    data: &RefCell<State>,
    execution: &Evaluation<'_>,
) -> Result<Vec<RuleHeader>, MakeExprError> {
    let (name, rhs, kind) = variable_assignment_impl(assignment, true)
        .ok_or_else(|| invalid("unsupported target variable assignment"))?;
    let targets = target_names(targets)?;
    let mut result = Vec::with_capacity(targets.len());
    let mut emitted = 0usize;
    for target in targets {
        let pattern = target.contains('%');
        check_origin_write(&data.borrow(), name, Some(true))?;
        if data.borrow().coordinate_projection && super::bound_coordinate(name) {
            return Err(invalid("cannot replace a bound Make target coordinate"));
        }
        let old = if pattern {
            None
        } else {
            data.borrow()
                .target_variables
                .get(target)
                .and_then(|scope| scope.get(name))
                .cloned()
        };
        let simple = kind == AssignmentKind::SimpleSet
            || (kind == AssignmentKind::Append
                && old
                    .as_ref()
                    .is_some_and(|binding| binding.flavor == VariableFlavor::Simple));
        let value = if simple && rhs.contains('$') {
            // Pattern assignments are frozen in global context; an explicit
            // target sees only its own previous locals, not matched patterns.
            let global_before = data.borrow().events.len();
            let file_before = execution.file.map(super::file::FileContext::event_count);
            let value = if pattern {
                evaluate(rhs, line, data, execution)?
            } else {
                evaluate_with_queries(
                    rhs,
                    line,
                    data,
                    execution,
                    &|name| read_lookup(&data.borrow(), target, name),
                    &|query, name| read_inspect(&data.borrow(), target, query, name),
                    &|name| read_guard(&data.borrow(), target, name),
                )?
            };
            // Eval and intrinsic shell-status mutations need their separate
            // global effect handoff; never discard them and accept a value.
            if data.borrow().events.len() != global_before
                || execution.file.map(super::file::FileContext::event_count) != file_before
            {
                return Err(invalid(
                    "target immediate expression has unsupported state effects",
                ));
            }
            value.replace('$', "$$")
        } else {
            rhs.to_owned()
        };
        let normalized = if kind == AssignmentKind::SimpleSet {
            format!("{name} := {value}")
        } else {
            assignment.to_owned()
        };
        let command_line =
            data.borrow().origins.get(name) == Some(&MakeVariableOrigin::CommandLine);
        if !command_line {
            if pattern {
                retain_pattern(data, target, name, value, kind, execution)?;
            } else {
                let skip = kind == AssignmentKind::SetIfUnset
                    && (old.is_some() || globally_defined(&data.borrow(), name)?);
                if !skip {
                    let binding = match kind {
                        AssignmentKind::Append => match old {
                            Some(mut binding) => {
                                if !value.is_empty() {
                                    if !binding.raw.is_empty() {
                                        binding.raw.push(' ');
                                    }
                                    binding.raw.push_str(&value);
                                }
                                binding
                            }
                            None => Binding {
                                raw: value,
                                flavor: VariableFlavor::Recursive,
                                append_parent: true,
                            },
                        },
                        AssignmentKind::SimpleSet => Binding {
                            raw: value,
                            flavor: VariableFlavor::Simple,
                            append_parent: false,
                        },
                        AssignmentKind::RecursiveSet | AssignmentKind::SetIfUnset => Binding {
                            raw: value,
                            flavor: VariableFlavor::Recursive,
                            append_parent: false,
                        },
                        AssignmentKind::Undefine => {
                            return Err(invalid("unsupported target undefinition"))
                        }
                    };
                    retain(data, target, name, binding, execution)?;
                }
            }
        }
        let cost = retained_cost(target.len() + normalized.len())?;
        emitted = emitted
            .checked_add(cost)
            .filter(|bytes| *bytes <= MAX_STORAGE)
            .ok_or_else(|| invalid("target assignment expansion exceeds retained header bound"))?;
        charge_work(execution.work, cost)?;
        result.push(RuleHeader::TargetAssignment {
            targets: target.into(),
            assignment: normalized,
        });
    }
    Ok(result)
}

fn retain_pattern(
    data: &RefCell<State>,
    pattern: &str,
    name: &str,
    raw: String,
    kind: AssignmentKind,
    execution: &Evaluation<'_>,
) -> Result<(), MakeExprError> {
    if raw.len() > MAX_VALUE || kind == AssignmentKind::Undefine {
        return Err(invalid("unsupported pattern variable value or operation"));
    }
    let cost = retained_cost(pattern.len() + name.len() + raw.len())?;
    charge_work(execution.work, cost)?;
    let mut state = data.borrow_mut();
    state.storage = state
        .storage
        .checked_add(cost)
        .filter(|size| *size <= MAX_STORAGE)
        .ok_or_else(|| invalid("pattern variables exceed state storage bound"))?;
    state.pattern_variables.push(PatternAssignment {
        pattern: pattern.into(),
        name: name.into(),
        raw,
        kind,
    });
    Ok(())
}

fn read_binding<'a>(state: &'a State, target: &str, name: &str) -> Option<&'a Binding> {
    if state.origins.get(name) == Some(&MakeVariableOrigin::CommandLine) {
        return None;
    }
    state.target_variables.get(target)?.get(name)
}

fn read_lookup(state: &State, target: &str, name: &str) -> Option<String> {
    binding_lookup(state, read_binding(state, target, name), name)
}

fn binding_lookup(state: &State, binding: Option<&Binding>, name: &str) -> Option<String> {
    let Some(binding) = binding else {
        return lookup(state, name);
    };
    if !binding.append_parent {
        return Some(binding.raw.clone());
    }
    let base = lookup(state, name)?;
    if base
        .len()
        .saturating_add(binding.raw.len())
        .saturating_add(1)
        > MAX_VALUE
    {
        return None;
    }
    Some(if base.is_empty() {
        binding.raw.clone()
    } else {
        format!("{base} {}", binding.raw)
    })
}

fn read_guard(state: &State, target: &str, name: &str) -> Option<String> {
    binding_guard(state, read_binding(state, target, name), name)
}

fn binding_guard(state: &State, binding: Option<&Binding>, name: &str) -> Option<String> {
    let Some(binding) = binding else {
        return guard(state, name);
    };
    if binding.append_parent {
        if let Some(reason) = guard(state, name) {
            return Some(reason);
        }
        if binding_lookup(state, Some(binding), name).is_none() {
            return Some("target append parent is unavailable or exceeds its value bound".into());
        }
    }
    None
}

fn read_inspect(
    state: &State,
    target: &str,
    query: MakeVariableQuery,
    name: &str,
) -> Option<String> {
    binding_inspect(state, read_binding(state, target, name), query, name)
}

fn binding_inspect(
    state: &State,
    binding: Option<&Binding>,
    query: MakeVariableQuery,
    name: &str,
) -> Option<String> {
    let Some(binding) = binding else {
        return inspect(state, query, name);
    };
    Some(match query {
        MakeVariableQuery::Origin => "file".into(),
        MakeVariableQuery::Flavor => match binding.flavor {
            VariableFlavor::Simple => "simple".into(),
            VariableFlavor::Recursive => "recursive".into(),
            VariableFlavor::Unknown => return None,
        },
        MakeVariableQuery::RawValue => {
            if binding.flavor == VariableFlavor::Simple {
                binding.raw.replace("$$", "$")
            } else {
                binding.raw.clone()
            }
        }
    })
}

fn target_names(targets: &str) -> Result<Vec<&str>, MakeExprError> {
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for target in targets.split_ascii_whitespace() {
        if result.len() == 16_384
            || target.len() > MAX_VALUE
            || target
                .chars()
                .any(|c| c.is_control() || matches!(c, '$' | ':' | ';' | '|' | '=' | '\\'))
            || crate::make_rule_header::is_control_target(target)
            || !seen.insert(target)
        {
            return Err(invalid(
                "unsupported or repeated Make target assignment name",
            ));
        }
        result.push(target);
    }
    if result.is_empty() {
        return Err(invalid("empty Make target assignment name"));
    }
    Ok(result)
}

fn globally_defined(state: &State, name: &str) -> Result<bool, MakeExprError> {
    if let Some(detail) = guard(state, name) {
        return Err(invalid(&format!(
            "target assignment has unmeasured definedness: {detail}"
        )));
    }
    if state.scope.is_defined_before(name, state.events.len()) {
        return Ok(true);
    }
    if state.undefined.contains(name)
        || state.scope.undefinitions.contains_key(name)
        || reference_absent(state, name)
    {
        return Ok(false);
    }
    Err(invalid(
        "target assignment needs proved variable definedness",
    ))
}

fn retain(
    data: &RefCell<State>,
    target: &str,
    name: &str,
    binding: Binding,
    execution: &Evaluation<'_>,
) -> Result<(), MakeExprError> {
    if binding.raw.len() > MAX_VALUE {
        return Err(invalid("target variable exceeds value bound"));
    }
    let cost = retained_cost(target.len() + name.len() + binding.raw.len())?;
    charge_work(execution.work, cost)?;
    let mut state = data.borrow_mut();
    state.storage = state
        .storage
        .checked_add(cost)
        .filter(|size| *size <= MAX_STORAGE)
        .ok_or_else(|| invalid("target variables exceed state storage bound"))?;
    state
        .target_variables
        .entry(target.into())
        .or_default()
        .insert(name.into(), binding);
    Ok(())
}

pub(super) fn overlay(
    data: &RefCell<State>,
    chain: &[&str],
    execution: &Evaluation<'_>,
) -> Result<RefCell<State>, MakeExprError> {
    if chain.len() > 64 || chain.iter().copied().collect::<BTreeSet<_>>().len() != chain.len() {
        return Err(invalid(
            "target scope path is cyclic or exceeds depth bound",
        ));
    }
    charge_work(execution.work, data.borrow().storage)?;
    let staged = RefCell::new(data.borrow().clone());
    for target in chain {
        if target_names(target)?.as_slice() != [*target] || target.contains('%') {
            return Err(invalid(
                "target scope path requires exact concrete target names",
            ));
        }
        let patterns = matching_bindings(data, target, execution)?;
        let bindings = data
            .borrow()
            .target_variables
            .get(*target)
            .cloned()
            .unwrap_or_default();
        for (name, binding) in patterns.into_iter().chain(bindings) {
            if staged.borrow().origins.get(&name) == Some(&MakeVariableOrigin::CommandLine) {
                continue;
            }
            check_origin_write(&staged.borrow(), &name, Some(true))?;
            let raw = if binding.append_parent {
                if guard(&staged.borrow(), &name).is_some() {
                    return Err(invalid("target append parent is unmeasured"));
                }
                let base = lookup(&staged.borrow(), &name)
                    .ok_or_else(|| invalid("target append parent is unavailable"))?;
                if base.is_empty() {
                    binding.raw.clone()
                } else {
                    format!("{base} {}", binding.raw)
                }
            } else {
                binding.raw.clone()
            };
            record_assignment(
                &format!("{name} ="),
                &name,
                AssignmentValue {
                    raw: (binding.flavor != VariableFlavor::Simple).then_some(raw.as_str()),
                    flavor: Some(binding.flavor),
                    frozen: (binding.flavor == VariableFlavor::Simple).then_some(raw.as_str()),
                },
                usize::MAX,
                Some(true),
                &staged,
            )?;
            if binding.append_parent {
                staged
                    .borrow_mut()
                    .target_raw_values
                    .insert(name, binding.raw);
            } else {
                staged.borrow_mut().target_raw_values.remove(&name);
            }
        }
    }
    Ok(staged)
}

/// A direct recipe frame only: literal simple override bindings are private
/// because no descendant scope or modified Source state escapes this view.
pub(super) fn direct_literal_overlay(
    data: &RefCell<State>,
    chain: &[&str],
    values: &std::collections::BTreeMap<String, String>,
    execution: &Evaluation<'_>,
) -> Result<RefCell<State>, MakeExprError> {
    if chain.len() != 1 || values.len() > MAX_STATEMENTS {
        return Err(invalid(
            "literal private overrides require one direct target frame",
        ));
    }
    let staged = overlay(data, chain, execution)?;
    for (name, value) in values {
        if !super::literal_variable_name(name) || value.len() > MAX_VALUE {
            return Err(invalid(
                "unsupported literal private override name or value",
            ));
        }
        // Simple values contain literal dollars, not deferred Make effects.
        let frozen = value.replace('$', "$$");
        if frozen.len() > MAX_VALUE {
            return Err(invalid("literal private override exceeds value bound"));
        }
        // Only this isolated frame bypasses the weaker original CLI priority.
        // No Source writer is given permission to ignore origin admission.
        staged.borrow_mut().origins.remove(name);
        record_assignment(
            &format!("{name} :="),
            name,
            AssignmentValue {
                raw: None,
                flavor: Some(VariableFlavor::Simple),
                frozen: Some(&frozen),
            },
            usize::MAX,
            Some(true),
            &staged,
        )?;
        let mut state = staged.borrow_mut();
        state
            .origins
            .insert(name.clone(), MakeVariableOrigin::Override);
        state.target_raw_values.remove(name);
    }
    Ok(staged)
}

impl MakeVariableProgram {
    /// Evaluate data in an explicitly selected root-to-leaf target scope path.
    ///
    /// The caller must prove the actual traversal/first parent of shared
    /// prerequisites. This does not choose a rule or provide automatic variables,
    /// filesystem access, modifiers or execution authority. Variable patterns
    /// match the supplied complete target name; this does not select an implicit
    /// build rule or prove that the native traversal selected this scope.
    /// Global values remain current; target locals retain read-time flavor and
    /// immediate values. No target overlay is published into global state.
    ///
    /// # Errors
    /// Refuses unbound/unsupported scope, effects and shared resource exhaustion.
    pub fn evaluate_target_expression(
        &mut self,
        chain: &[&str],
        expression: &str,
        paths: &MakeProgramPaths<'_>,
    ) -> Result<String, MakeExprError> {
        Ok(self
            .evaluate_target_expressions(chain, &[expression], paths)?
            .remove(0))
    }

    /// Evaluate ordered data expressions in one inherited target frame.
    ///
    /// The caller supplies an independently justified root-to-leaf path. One
    /// global stage and target overlay serve the whole batch, rather than two
    /// full-state copies for every scalar field. This is still not multi-target
    /// batching, native first-parent proof, automatic variables or file access.
    /// No expression effects, partial results or target state are published.
    ///
    /// # Errors
    /// Refuses unsupported scope/effects and shared count/work/result bounds.
    pub fn evaluate_target_expressions(
        &mut self,
        chain: &[&str],
        expressions: &[&str],
        paths: &MakeProgramPaths<'_>,
    ) -> Result<Vec<String>, MakeExprError> {
        self.check_paths(paths)?;
        if expressions.len() > MAX_STATEMENTS {
            return Err(MakeExprError::ResourceLimit {
                resource: "Make target expression batch count",
                limit: MAX_STATEMENTS,
            });
        }
        let execution = Evaluation {
            paths,
            expressions: &self.expressions,
            work: &self.state_work,
            file: None,
            captured_wildcard: None,
            captured_realpath: None,
            captured_shell: None,
            automatic_variables: None,
        };
        let globals = self.stage()?;
        let staged = overlay(&globals, chain, &execution)?;
        let before = staged.borrow().events.len();
        let mut results = Vec::with_capacity(expressions.len());
        let mut storage = 0usize;
        for expression in expressions {
            let result = evaluate(expression, usize::MAX, &staged, &execution)?;
            if staged.borrow().events.len() != before {
                return Err(invalid("target expression has unsupported state effects"));
            }
            let cost = retained_cost(result.len())?;
            storage = storage
                .checked_add(cost)
                .filter(|bytes| *bytes <= MAX_STORAGE)
                .ok_or(MakeExprError::ResourceLimit {
                    resource: "Make target expression batch results",
                    limit: MAX_STORAGE,
                })?;
            charge_work(&self.state_work, cost)?;
            results.push(result);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod priority_tests;
#[cfg(test)]
mod tests;
