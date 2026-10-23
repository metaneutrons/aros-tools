// Source-derived deferred recipe data for the explicit captured-root test.
// Neither a declared path nor its automatic values prove native scheduling,
// completed prerequisites, the complete child environment or launch authority.

use super::{read_records, PrivateOutput};
use aros_common::make_input_snapshot::MakeInputSnapshot;
use aros_transpiler::{
    make_interpreted_rules,
    make_rule_graph::traversal::trace_declared,
    make_vars::{MakeFileProgram, MakeProgramPaths, MakeVariableProgram},
};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    goals: Vec<String>,
    target: String,
    recipe_index: usize,
    #[serde(default)]
    implicit_declaration: Option<SourceLocation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceLocation {
    input: String,
    source_line_one_based: usize,
}

impl Selection {
    pub(super) fn validate_arguments(&self, arguments: &[String]) -> Result<(), String> {
        let startup =
            aros_common::make_startup_arguments::LiteralMakeStartupArguments::admit(arguments)
                .map_err(|_| "recipe selection has unsupported original startup arguments")?;
        if startup.goals() != self.goals || self.target.is_empty() {
            return Err("recipe selection differs from original explicit invocation goals".into());
        }
        Ok(())
    }
}

pub(super) fn project(
    output: &PrivateOutput,
    selection: &Selection,
    file: &MakeFileProgram,
    program: &mut MakeVariableProgram,
    paths: &MakeProgramPaths<'_>,
    snapshot: &MakeInputSnapshot,
) -> Result<Value, String> {
    let mut operation = || -> Result<Value, String> {
        // Never discard unresolved implicit/default/phony/suffix policies or
        // silently relabel this trace as an admitted native traversal.
        let rules = make_interpreted_rules::collect(file)?;
        let projected = rules.project_explicit()?;
        let goals = selection
            .goals
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let (instance, scope, unresolved, mode) = if let Some(location) =
            &selection.implicit_declaration
        {
            // The case independently selects an actual source declaration. No
            // search/ranking/default selection is inferred from one match.
            if selection.goals.len() != 1 || selection.goals[0] != selection.target {
                return Err(
                    "implicit candidate projection requires its exact sole root goal".into(),
                );
            }
            let mut matching = rules.rules.iter().filter(|rule| {
                let origin = &file.events[rule.event_index()];
                origin.input == location.input
                    && origin.source_line.checked_add(1) == Some(location.source_line_one_based)
            });
            let rule = matching
                .next()
                .ok_or("selected implicit source declaration is absent")?;
            if matching.next().is_some() {
                return Err("selected implicit source location is ambiguous".into());
            }
            let mut candidates = rule.instantiate_implicit_candidate(&selection.target)?;
            if candidates.len() != 1 {
                return Err("selected implicit source declaration has no unique candidate".into());
            }
            (
                candidates.pop().ok_or("implicit candidate disappeared")?,
                vec![selection.target.clone()],
                None,
                "source-selected implicit candidate; not winning-rule selection",
            )
        } else {
            let trace = trace_declared(&projected.graph, &goals)?;
            let index = trace
                .visits()
                .iter()
                .position(|visit| visit.target() == selection.target)
                .ok_or("selected recipe target is not reachable in declared goals")?;
            let instance = trace.visits()[index]
                .instance()
                .ok_or("selected recipe target has no explicit declaration")?
                .clone();
            let scope = trace
                .scope_chain(index)?
                .into_iter()
                .map(str::to_owned)
                .collect();
            (
                instance,
                scope,
                Some(trace.unresolved_inputs().count()),
                "explicit declaration-only trace",
            )
        };
        let recipe = instance
            .recipes
            .get(selection.recipe_index)
            .ok_or("selected recipe index is absent from source declaration")?;
        let origin = file
            .events
            .get(recipe.output_line)
            .ok_or("selected recipe has no retained source event")?;
        if !origin.active || origin.source_line != recipe.source_line {
            return Err("selected recipe differs from retained source origin".into());
        }
        let chain = scope.iter().map(String::as_str).collect::<Vec<_>>();
        let automatic = instance.automatic_variables();
        let (data, reads) = program.evaluate_captured_target_recipe(
            &chain,
            &recipe.text,
            &automatic,
            paths,
            snapshot,
        )?;
        output.write_json(
            "recipe-results.json",
            &json!({
                "goals": selection.goals,
                "target": selection.target,
                "recipe_index": selection.recipe_index,
                "selection_mode": mode,
                "scope_chain": chain,
                "automatic_variables": automatic,
                "origin": {"input": origin.input, "line": origin.source_line,
                           "event_index": recipe.output_line, "generated": origin.generated},
                "source_recipe": recipe.text,
                "expanded_recipe": data.expression(),
                "source_exports": data.source_exports(),
                "reads": read_records(&reads),
                "limits": "inert declared recipe data; not admitted traversal, preparation, native recipe phase, complete child environment or launch authority",
            }),
        )?;
        Ok(json!({
            "selected_source_recipe": true,
            "scope_depth": chain.len(),
            "source_export_count": data.source_exports().len(),
            "unresolved_declared_input_count": unresolved,
            "selection_mode": mode,
            "implicit_rule_count": projected.implicit_rule_events.len(),
            "unaccounted_source_event_count": rules.other_events.len(),
            "raw_results_file": "recipe-results.json",
            "qualification": "inert declaration-only data; not native phase or complete child environment",
        }))
    };
    match operation() {
        Ok(value) => Ok(value),
        Err(error) => {
            output.write_json("recipe-errors.json", &json!({"error": error}))?;
            Err("captured source recipe projection refused; private evidence retained".into())
        }
    }
}

#[test]
fn selected_recipe_goals_are_bound_to_original_startup_arguments() {
    let selection: Selection = serde_json::from_value(json!({
        "goals": ["sample-entry"], "target": "sample-entry", "recipe_index": 0,
        "implicit_declaration": {"input": "root.mk", "source_line_one_based": 3},
    }))
    .unwrap();
    let arguments = ["-r", "-f", "root.mk", "sample-entry"].map(str::to_owned);
    selection.validate_arguments(&arguments).unwrap();
    for goal in ["foreign-entry", "sample-entry extra"] {
        let arguments = ["-r", "-f", "root.mk", goal].map(str::to_owned);
        assert!(selection.validate_arguments(&arguments).is_err());
    }
    assert!(serde_json::from_value::<Selection>(json!({
        "goals": ["sample-entry"], "target": "sample-entry", "recipe_index": 0,
        "implicit_declaration": {"input": "root.mk", "source_line": 3},
    }))
    .is_err());
}
