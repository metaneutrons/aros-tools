//! Initialization of matched pattern-variable operations, distinct from implicit
//! build-rule search. Matching uses the complete target spelling.

use super::{
    binding_guard, binding_inspect, binding_lookup, charge_work, evaluate_with_queries,
    globally_defined, invalid, retained_cost, AssignmentKind, Binding, Evaluation, MakeExprError,
    State, VariableFlavor, MAX_STORAGE, MAX_VALUE,
};
use std::{cell::RefCell, collections::BTreeMap};

fn stem_length(pattern: &str, target: &str) -> Option<usize> {
    let (prefix, suffix) = pattern.split_once('%')?;
    let fixed = prefix.len().checked_add(suffix.len())?;
    (target.len() >= fixed && target.starts_with(prefix) && target.ends_with(suffix))
        .then(|| target.len() - fixed)
}

pub(super) fn matching_bindings(
    globals: &RefCell<State>,
    target: &str,
    execution: &Evaluation<'_>,
) -> Result<BTreeMap<String, Binding>, MakeExprError> {
    let mut matching = Vec::new();
    for operation in &globals.borrow().pattern_variables {
        charge_work(execution.work, operation.pattern.len() + target.len())?;
        if let Some(stem) = stem_length(&operation.pattern, target) {
            charge_work(
                execution.work,
                retained_cost(
                    operation.pattern.len() + operation.name.len() + operation.raw.len(),
                )?,
            )?;
            matching.push((stem, operation.clone()));
        }
    }
    // Stable sorting retains source order between equal stem lengths, including
    // later definitions of an already encountered pattern.
    matching.sort_by_key(|(stem, _)| std::cmp::Reverse(*stem));
    let mut bindings: BTreeMap<String, Binding> = BTreeMap::new();
    let mut storage = 0usize;
    let initialization = Evaluation {
        automatic_variables: None,
        ..*execution
    };
    for (_, operation) in matching {
        let old = bindings.get(&operation.name);
        if operation.kind == AssignmentKind::SetIfUnset
            && (old.is_some() || globally_defined(&globals.borrow(), &operation.name)?)
        {
            continue;
        }
        let binding = if operation.kind == AssignmentKind::Append {
            if let Some(old) = old {
                let value = if old.flavor == VariableFlavor::Simple && operation.raw.contains('$') {
                    let before = globals.borrow().events.len();
                    let file_before = execution
                        .file
                        .map(super::super::file::FileContext::event_count);
                    let value = evaluate_with_queries(
                        &operation.raw,
                        usize::MAX,
                        globals,
                        &initialization,
                        &|name| binding_lookup(&globals.borrow(), bindings.get(name), name),
                        &|query, name| {
                            binding_inspect(&globals.borrow(), bindings.get(name), query, name)
                        },
                        &|name| binding_guard(&globals.borrow(), bindings.get(name), name),
                    )?;
                    if globals.borrow().events.len() != before
                        || execution
                            .file
                            .map(super::super::file::FileContext::event_count)
                            != file_before
                    {
                        return Err(invalid(
                            "pattern initialization has unsupported state effects",
                        ));
                    }
                    value.replace('$', "$$")
                } else {
                    operation.raw
                };
                let mut binding = old.clone();
                if !value.is_empty() {
                    if !binding.raw.is_empty() {
                        binding.raw.push(' ');
                    }
                    binding.raw.push_str(&value);
                }
                binding
            } else {
                Binding {
                    raw: operation.raw,
                    flavor: VariableFlavor::Recursive,
                    append_parent: true,
                }
            }
        } else {
            Binding {
                raw: operation.raw,
                flavor: if operation.kind == AssignmentKind::SimpleSet {
                    VariableFlavor::Simple
                } else {
                    VariableFlavor::Recursive
                },
                append_parent: false,
            }
        };
        if binding.raw.len() > MAX_VALUE {
            return Err(invalid("matched pattern variable exceeds value bound"));
        }
        let cost = retained_cost(operation.name.len() + binding.raw.len())?;
        charge_work(execution.work, cost)?;
        storage = storage
            .checked_add(cost)
            .filter(|size| *size <= MAX_STORAGE)
            .ok_or_else(|| invalid("matched pattern variables exceed storage bound"))?;
        bindings.insert(operation.name, binding);
    }
    Ok(bindings)
}

#[cfg(test)]
mod tests;
