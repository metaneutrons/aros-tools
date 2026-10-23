use super::expand;
use crate::dirs::DirVars;
use crate::make_definitions::MakeDefinitions;
use crate::make_expr::{evaluate_make_rule_expansion, MakeExprContext, MakeRuleExpansion};
use crate::make_vars::collect_vars;
use std::cell::{Cell, RefCell};
use std::path::Path;

#[test]
fn foreach_rules_keep_order_and_original_source_provenance() {
    let source = concat!(
        "BASE := early\n",
        "define emit_object\n",
        "$(BASE)_$(1).o: $(BASE)_$(1).c\n",
        "\t@compile $$< -o $$@\n",
        "endef\n",
        "$(foreach variant,solid wire,$(eval $(call emit_object,$(variant))))\n",
        "BASE := later\n",
    );
    let states = vec![Some(true); source.lines().count()];
    let invocation_line = source
        .lines()
        .position(|line| line.starts_with("$(foreach variant,"))
        .unwrap();
    let later_assignment_line = source
        .lines()
        .position(|line| line == "BASE := later")
        .unwrap();
    let scope = collect_vars(source);
    let dirs = DirVars::default();
    let evaluate = |expression: &str,
                    line: usize,
                    definitions: &MakeDefinitions|
     -> Result<MakeRuleExpansion, String> {
        assert_eq!(definitions.line, line);
        let context =
            MakeExprContext::new(&scope, &dirs, line, Path::new("."), Path::new("fixture"))
                .with_definitions(definitions)
                .without_filesystem();
        evaluate_make_rule_expansion(expression, &context).map_err(|error| error.to_string())
    };

    let expanded = expand(source, &states, &evaluate).unwrap();
    let fragments = expanded
        .fragments
        .iter()
        .map(|fragment| {
            assert_eq!(fragment.source_line, invocation_line);
            let text = &expanded.text[fragment.start_byte..fragment.end_byte];
            assert!(text.ends_with('\n'));
            text
        })
        .collect::<Vec<_>>();
    assert_eq!(fragments.len(), 2);
    assert!(fragments[0].contains("early_solid.o: early_solid.c"));
    assert!(fragments[1].contains("early_wire.o: early_wire.c"));

    let output_lines = expanded.text.lines().collect::<Vec<_>>();
    assert_eq!(output_lines.len(), expanded.lines.len());
    let generated_lines = expanded
        .lines
        .iter()
        .filter(|line| line.eval_fragment.is_some())
        .collect::<Vec<_>>();
    assert_eq!(generated_lines.len(), 4);
    for line in generated_lines {
        assert_eq!(line.source_line, invocation_line);
        assert_eq!(line.active, Some(true));
    }
    let later_output_line = output_lines
        .iter()
        .position(|line| *line == "BASE := later")
        .unwrap();
    assert_eq!(
        expanded.lines[later_output_line].source_line,
        later_assignment_line
    );
    assert_eq!(expanded.lines[later_output_line].active, Some(true));
    assert_eq!(expanded.lines[later_output_line].eval_fragment, None);
}

#[test]
fn inactive_lines_definitions_and_recipes_are_not_evaluated() {
    let source = concat!(
        "define stored_only\n",
        "$(from_definition)\n",
        "endef\n",
        "ordinary:\n",
        "\t$(from_recipe)\n",
        "$(from_inactive_line)\n",
        "$(from_active_line)\n",
    );
    let mut states = vec![Some(true); source.lines().count()];
    states[5] = Some(false);
    let invocations = RefCell::new(Vec::new());
    let evaluate =
        |expression: &str, line: usize, _: &MakeDefinitions| -> Result<MakeRuleExpansion, String> {
            invocations.borrow_mut().push((expression.to_owned(), line));
            Ok(MakeRuleExpansion {
                value: String::new(),
                rules: Vec::new(),
            })
        };

    expand(source, &states, &evaluate).unwrap();

    assert_eq!(
        invocations.into_inner(),
        vec![("$(from_active_line)".to_owned(), 6)]
    );
}

#[test]
fn unresolved_statement_selection_fails_before_evaluation() {
    let source = "$(unknown_selection)\n";
    let states = [None];
    let calls = Cell::new(0);
    let evaluate = |_: &str, _: usize, _: &MakeDefinitions| {
        calls.set(calls.get() + 1);
        Ok(MakeRuleExpansion {
            value: String::new(),
            rules: Vec::new(),
        })
    };

    assert!(expand(source, &states, &evaluate)
        .unwrap_err()
        .contains("unresolved selection"));
    assert_eq!(calls.get(), 0);
}

#[test]
fn custom_evaluator_cannot_return_top_level_text_or_global_effects() {
    let source = "$(generated_statement)\n";
    let states = [Some(true)];
    let outputs = [
        MakeRuleExpansion {
            value: "unexpected text".into(),
            rules: Vec::new(),
        },
        MakeRuleExpansion {
            value: " \t ".into(),
            rules: vec!["SETTING := changed\n".into()],
        },
    ];

    for (output, expected) in outputs
        .into_iter()
        .zip(["unsupported top-level text", "eval supports rule data only"])
    {
        let called = Cell::new(false);
        let evaluate = |_: &str, _: usize, _: &MakeDefinitions| {
            called.set(true);
            Ok(output.clone())
        };
        assert!(expand(source, &states, &evaluate)
            .unwrap_err()
            .contains(expected));
        assert!(called.get());
    }
}

#[test]
fn state_count_must_match_the_joined_source_lines() {
    let source = "first line\nsecond line\n";
    let states = [Some(true)];
    let calls = Cell::new(0);
    let evaluate = |_: &str, _: usize, _: &MakeDefinitions| {
        calls.set(calls.get() + 1);
        Ok(MakeRuleExpansion {
            value: String::new(),
            rules: Vec::new(),
        })
    };

    assert!(expand(source, &states, &evaluate)
        .unwrap_err()
        .contains("invalid Make rule-program input"));
    assert_eq!(calls.get(), 0);
}

#[test]
fn variable_prefixed_targets_and_assignments_are_not_expansion_statements() {
    let source = "$(OBJECT): input\n$(OWNER)_FLAGS := literal\n";
    let output = expand(source, &[Some(true); 2], &|_, _, _| {
        panic!("target or assignment must not be evaluated as a statement")
    })
    .unwrap();
    assert_eq!(output.text, source);
    assert!(output.fragments.is_empty());
}

#[test]
fn projected_rules_and_later_target_assignments_share_original_positioned_scope() {
    use crate::make_expr::evaluate_make_expr;
    use std::collections::{BTreeMap, BTreeSet};

    let source = concat!(
        "GLOBAL = early\n",
        "OBJECTS = solid.o wire.o\n",
        "define seed_target\n",
        "$(1): EXTRA := $(GLOBAL)-$(1)\n",
        "endef\n",
        "$(foreach object,$(OBJECTS),$(eval $(call seed_target,$(object))))\n",
        "GLOBAL = later\n",
        "$(OBJECTS): FLAGS := $(EXTRA) $(GLOBAL)\n",
        "GLOBAL = final\n",
    );
    let scope = collect_vars(source);
    let dirs = DirVars::default();
    let projected = expand(
        source,
        &vec![Some(true); source.lines().count()],
        &|raw, line, defs| {
            let context =
                MakeExprContext::new(&scope, &dirs, line, Path::new("/source"), Path::new("."))
                    .with_definitions(defs);
            evaluate_make_rule_expansion(raw, &context).map_err(|error| error.to_string())
        },
    )
    .unwrap();
    let states = projected
        .lines
        .iter()
        .map(|line| line.active)
        .collect::<Vec<_>>();
    let captured = crate::make_target_capture::capture(
        &projected.text,
        &BTreeSet::from(["FLAGS".to_owned()]),
        &states,
        &|raw, output_line, overlay| {
            let literal_overlay = overlay
                .iter()
                .map(|(name, value)| (name.clone(), value.replace('$', "$$")))
                .collect::<BTreeMap<_, _>>();
            let lookup = |name: &str| literal_overlay.get(name).cloned();
            let source_line = projected.lines[output_line].source_line;
            let context = MakeExprContext::new(
                &scope,
                &dirs,
                source_line,
                Path::new("/source"),
                Path::new("."),
            )
            .with_lookup(&lookup);
            evaluate_make_expr(raw, &context).map_err(|error| error.to_string())
        },
    )
    .unwrap();
    assert_eq!(captured.targets["solid.o"]["FLAGS"], "early-solid.o later");
    assert_eq!(captured.targets["wire.o"]["FLAGS"], "early-wire.o later");
    assert_eq!(captured.assignments.len(), 2);
    for assignment in captured.assignments {
        assert_eq!(
            source
                .lines()
                .nth(projected.lines[assignment.line].source_line),
            Some("$(OBJECTS): FLAGS := $(EXTRA) $(GLOBAL)")
        );
    }
}

#[test]
fn nested_brace_and_parenthesis_forms_share_statement_and_argument_parsing() {
    let source = "define emit\n$(1).o: $(1).c\nendef\n${foreach part,solid wire,${eval ${call emit,${part}}}}\n";
    let scope = collect_vars(source);
    let dirs = DirVars::default();
    let output = expand(
        source,
        &vec![Some(true); source.lines().count()],
        &|raw, line, defs| {
            let context =
                MakeExprContext::new(&scope, &dirs, line, Path::new("/source"), Path::new("."))
                    .with_definitions(defs);
            evaluate_make_rule_expansion(raw, &context).map_err(|error| error.to_string())
        },
    )
    .unwrap();
    assert_eq!(output.fragments.len(), 2);
    assert_eq!(
        &output.text[output.fragments[0].start_byte..output.fragments[0].end_byte],
        "solid.o: solid.c\n"
    );
    assert_eq!(
        &output.text[output.fragments[1].start_byte..output.fragments[1].end_byte],
        "wire.o: wire.c\n"
    );
    let context = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("."));
    // Same print-only GNU Make oracle: (b,c) and pa pb respectively.
    assert_eq!(
        crate::make_expr::evaluate_make_expr("$(subst a,b,(a,c))", &context).unwrap(),
        "(b,c)"
    );
    assert_eq!(
        crate::make_expr::evaluate_make_expr("${foreach x,a b,${addprefix p,${x}}}", &context)
            .unwrap(),
        "pa pb"
    );
}

#[test]
fn empty_eval_has_no_rule_identity_and_control_targets_refuse() {
    let scope = collect_vars("");
    let dirs = DirVars::default();
    for source in ["$(eval )\n", "${eval }\n"] {
        let output = expand(source, &[Some(true)], &|raw, line, defs| {
            let context =
                MakeExprContext::new(&scope, &dirs, line, Path::new("/source"), Path::new("."))
                    .with_definitions(defs);
            evaluate_make_rule_expansion(raw, &context).map_err(|error| error.to_string())
        });
        let output = output.unwrap();
        assert!(output.fragments.is_empty());
        assert!(output.lines.iter().all(|line| line.eval_fragment.is_none()));
    }
    let comments = expand("$(custom)\n", &[Some(true)], &|_, _, _| {
        Ok(MakeRuleExpansion {
            value: String::new(),
            rules: vec!["# retained comment\n".to_owned()],
        })
    })
    .unwrap();
    assert!(comments.fragments.is_empty());
    assert_eq!(comments.text, "# retained comment\n");
    for target in [".PHONY", ".ONESHELL", ".SUFFIXES", ".SECONDEXPANSION"] {
        let error = expand("$(custom)\n", &[Some(true)], &|_, _, _| {
            Ok(MakeRuleExpansion {
                value: String::new(),
                rules: vec![format!("{target}: worker")],
            })
        })
        .unwrap_err();
        assert!(
            error.contains("eval supports rule data only"),
            "{target}: {error}"
        );
    }
}

#[test]
fn bounded_projection_refuses_fragment_line_and_definition_scan_exhaustion() {
    let expression = "$(custom)\n";
    let too_many_fragments = expand(expression, &[Some(true)], &|_, _, _| {
        Ok(MakeRuleExpansion {
            value: String::new(),
            rules: vec!["unit.o: unit.c".to_owned(); super::MAX_FRAGMENTS + 1],
        })
    })
    .unwrap_err();
    assert!(too_many_fragments.contains("statement exceeds output limits"));

    let too_many_lines = expand(expression, &[Some(true)], &|_, _, _| {
        Ok(MakeRuleExpansion {
            value: String::new(),
            rules: vec!["unit.o: unit.c\n".repeat(super::MAX_OUTPUT_LINES + 1)],
        })
    })
    .unwrap_err();
    assert!(too_many_lines.contains("output byte or line limit"));

    let mut large_program = format!("#{}\n", "x".repeat(1024 * 1024));
    large_program.push_str(&expression.repeat(65));
    let error = expand(
        &large_program,
        &vec![Some(true); large_program.lines().count()],
        &|_, _, _| {
            Ok(MakeRuleExpansion {
                value: String::new(),
                rules: Vec::new(),
            })
        },
    )
    .unwrap_err();
    assert!(error.contains("definition-scan work limit"));
}
