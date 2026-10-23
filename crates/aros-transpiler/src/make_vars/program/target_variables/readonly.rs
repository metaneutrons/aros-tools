//! Small target-local views over one shared global stage; never eval effects.

#[cfg(test)]
mod tests;

use super::{
    binding_inspect, binding_lookup, charge_work, check_origin_write, invalid, matching_bindings,
    retained_cost, target_names, Binding, Evaluation, MakeExprError, MakeProgramPaths,
    MakeVariableProgram, State, VariableFlavor, MAX_STATEMENTS, MAX_STORAGE, MAX_VALUE,
};
use crate::make_expr::{evaluate_make_program_expression, MakeExprContext, MakeVariableQuery};
use aros_common::make_namespace::MakeVariableOrigin;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

struct ScopedBinding {
    binding: Binding,
    append_fragment: Option<String>,
}

type View = BTreeMap<String, ScopedBinding>;

fn value(globals: &State, view: &View, name: &str) -> Option<String> {
    binding_lookup(globals, view.get(name).map(|entry| &entry.binding), name)
}

fn inspection(
    globals: &State,
    view: &View,
    query: MakeVariableQuery,
    name: &str,
) -> Option<String> {
    if query == MakeVariableQuery::RawValue && check(globals, view, name).is_some() {
        return None;
    }
    if query == MakeVariableQuery::RawValue {
        if let Some(fragment) = view
            .get(name)
            .and_then(|entry| entry.append_fragment.as_ref())
        {
            return Some(fragment.clone());
        }
    }
    let observed = binding_inspect(
        globals,
        view.get(name).map(|entry| &entry.binding),
        query,
        name,
    );
    if observed.is_none() && query == MakeVariableQuery::RawValue && !view.contains_key(name) {
        let raw = super::super::lookup(globals, name)?;
        return match globals.flavors.get(name) {
            Some(VariableFlavor::Simple) => Some(raw.replace("$$", "$")),
            Some(VariableFlavor::Recursive) => Some(raw),
            _ => None,
        };
    }
    observed
}

fn check(globals: &State, view: &View, name: &str) -> Option<String> {
    let at = globals.events.len();
    if !view.contains_key(name) {
        if globals.scope.conditionally_assigned_before(name, at) {
            return Some("Make variable has unresolved statement state".into());
        }
        if globals.initial_namespace.is_some()
            && !super::super::reference_absent(globals, name)
            && !globals.scope.is_defined_before(name, at)
            && !globals.scope.undefinitions.contains_key(name)
            && !globals.undefined.contains(name)
        {
            return Some("initial Make variable value or absence has not been measured".into());
        }
    }
    globals
        .scope
        .flavor_uncertainty_reason_with(name, at, &|name| {
            view.get(name).map(|entry| entry.binding.raw.clone())
        })
}

fn add(
    globals: &RefCell<State>,
    view: &mut View,
    name: &str,
    binding: &Binding,
    execution: &Evaluation<'_>,
    storage: &mut usize,
) -> Result<(), MakeExprError> {
    let state = globals.borrow();
    if state.origins.get(name) == Some(&MakeVariableOrigin::CommandLine) {
        return Ok(());
    }
    check_origin_write(&state, name, Some(true))?;
    let base = if binding.append_parent {
        if check(&state, view, name).is_some() {
            return Err(invalid("target append parent is unmeasured"));
        }
        Some(
            value(&state, view, name)
                .ok_or_else(|| invalid("target append parent is unavailable"))?,
        )
    } else {
        None
    };
    let bytes = base
        .as_ref()
        .map_or(0, |base| base.len() + usize::from(!base.is_empty()))
        .checked_add(binding.raw.len())
        .filter(|bytes| *bytes <= MAX_VALUE)
        .ok_or_else(|| invalid("target view value exceeds value bound"))?;
    let cost = retained_cost(
        name.len()
            + bytes
            + if binding.append_parent {
                binding.raw.len()
            } else {
                0
            },
    )?;
    charge_work(execution.work, cost)?;
    *storage = storage
        .checked_add(cost)
        .filter(|bytes| *bytes <= MAX_STORAGE)
        .ok_or_else(|| invalid("target view exceeds retained storage bound"))?;
    let raw = match base {
        Some(base) if !base.is_empty() => format!("{base} {}", binding.raw),
        _ => binding.raw.clone(),
    };
    view.insert(
        name.into(),
        ScopedBinding {
            binding: Binding {
                raw,
                flavor: binding.flavor,
                append_parent: false,
            },
            append_fragment: binding.append_parent.then(|| binding.raw.clone()),
        },
    );
    Ok(())
}

fn view(
    globals: &RefCell<State>,
    chain: &[&str],
    execution: &Evaluation<'_>,
) -> Result<View, MakeExprError> {
    if chain.len() > 64 || chain.iter().copied().collect::<BTreeSet<_>>().len() != chain.len() {
        return Err(invalid(
            "target scope path is cyclic or exceeds depth bound",
        ));
    }
    let mut view = View::new();
    let mut storage = 0usize;
    for target in chain {
        charge_work(execution.work, target.len())?;
        if target_names(target)?.as_slice() != [*target] || target.contains('%') {
            return Err(invalid(
                "target scope path requires exact concrete target names",
            ));
        }
        for (name, binding) in matching_bindings(globals, target, execution)? {
            add(globals, &mut view, &name, &binding, execution, &mut storage)?;
        }
        let state = globals.borrow();
        if let Some(bindings) = state.target_variables.get(*target) {
            for (name, binding) in bindings {
                add(globals, &mut view, name, binding, execution, &mut storage)?;
            }
        }
    }
    Ok(view)
}

impl MakeVariableProgram {
    /// Query the same ordered expressions across independently selected paths.
    ///
    /// One global stage serves all contexts; only target-local bindings and
    /// matched pattern operations are overlaid per path. Globals are never
    /// copied per target. Results keep context/expression order, with shared
    /// work and aggregate result bounds. No state or partial results publish.
    /// Eval effects and filesystem/recipe/automatic frames are unavailable.
    /// This does not prove native traversal, scheduling or input existence.
    ///
    /// # Errors
    /// Refuses unsupported scope, effects, unmeasured values and shared limits.
    pub fn evaluate_target_contexts(
        &mut self,
        chains: &[Vec<&str>],
        expressions: &[&str],
        paths: &MakeProgramPaths<'_>,
    ) -> Result<Vec<Vec<String>>, MakeExprError> {
        self.check_paths(paths)?;
        if chains.len() > MAX_STATEMENTS
            || expressions.len() > MAX_STATEMENTS
            || chains
                .len()
                .checked_mul(expressions.len())
                .is_none_or(|count| count > MAX_STATEMENTS)
        {
            return Err(invalid("target context batch exceeds count bound"));
        }
        let globals = self.stage()?;
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
        let mut groups = Vec::with_capacity(chains.len());
        let mut storage = 0usize;
        for chain in chains {
            let view = view(&globals, chain, &execution)?;
            let lookup = |name: &str| value(&globals.borrow(), &view, name);
            let inspect = |query, name: &str| inspection(&globals.borrow(), &view, query, name);
            let guard = |name: &str| check(&globals.borrow(), &view, name);
            let empty = super::super::collect_vars("");
            let mut context =
                MakeExprContext::new(&empty, paths.dirs, 0, paths.source_root, paths.relative_dir)
                    .with_lookup(&lookup)
                    .with_variable_inspection(&inspect)
                    .with_guard(&guard)
                    .with_working_directory(paths.working_directory)
                    .with_build_directory(paths.top_directory)
                    .with_pure_text_pipeline()
                    .without_filesystem();
            if !globals.borrow().coordinate_projection {
                context = context.without_source_projection();
            }
            let mut results = Vec::with_capacity(expressions.len());
            for expression in expressions {
                let result = evaluate_make_program_expression(
                    expression,
                    &context,
                    &self.expressions,
                    &|_| Err(invalid("read-only target context refuses eval effects")),
                )?;
                let cost = retained_cost(result.len())?;
                storage = storage
                    .checked_add(cost)
                    .filter(|bytes| *bytes <= MAX_STORAGE)
                    .ok_or_else(|| invalid("target context results exceed retained bound"))?;
                charge_work(&self.state_work, cost)?;
                results.push(result);
            }
            groups.push(results);
        }
        Ok(groups)
    }
}
