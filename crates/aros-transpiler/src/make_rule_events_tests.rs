use super::{collect, instantiate, RuleInstance};
use crate::make_definitions::MakeDefinitions;
use crate::make_rule_program::{expand, RuleProgram};
use std::cell::RefCell;

fn positioned(source: &str, states: &[Option<bool>]) -> RuleProgram {
    expand(source, states, &|_, _, _: &MakeDefinitions| {
        Err("unexpected expression callback".into())
    })
    .unwrap()
}

fn instantiate_identity(rule: &super::SourceRule) -> Result<Vec<RuleInstance>, String> {
    instantiate(rule, &|value, _| Ok(value.to_owned()))
}

#[test]
fn collects_positioned_rules_and_preserves_conditional_recipe_attachment() {
    let source = concat!(
        "GLOBAL := configured\n",
        "same.o: first.c\n",
        "ifeq ($(MODE),enabled)\n",
        "endif\n",
        "\t+@compile $< -o $@\n",
        "same.o:: second.c\n",
        "\t@double-colon\n",
        "same.o: CFLAGS += -g\n",
    );
    let states = vec![Some(true); source.lines().count()];
    let events = collect(&positioned(source, &states)).unwrap();

    assert_eq!(events.rules.len(), 2);
    assert_eq!(events.rules[0].targets, "same.o");
    assert_eq!(events.rules[0].source_line, 1);
    assert_eq!(events.rules[0].output_line, 1);
    assert_eq!(events.rules[0].prerequisites, "first.c");
    assert!(!events.rules[0].double_colon);
    assert_eq!(events.rules[0].recipes.len(), 1);
    assert_eq!(events.rules[0].recipes[0].source_line, 4);
    assert_eq!(events.rules[0].recipes[0].output_line, 4);
    assert_eq!(events.rules[0].recipes[0].text, "+@compile $< -o $@");

    assert_eq!(events.rules[1].targets, "same.o");
    assert_eq!(events.rules[1].prerequisites, "second.c");
    assert!(events.rules[1].double_colon);
    assert_eq!(events.rules[1].recipes[0].text, "@double-colon");
    assert_eq!(events.other_lines, vec![0]);
    assert_eq!(events.target_assignment_lines, vec![7]);
}

#[test]
fn static_pattern_instances_substitute_path_stems_and_derive_automatic_values() {
    let source = concat!(
        "# fixture\n",
        "obj/sub/a.o: obj/%.o: src/%.c src/%.c | ready\n",
        "\t+@compile $< -o $@\n",
    );
    let states = vec![Some(true); source.lines().count()];
    let events = collect(&positioned(source, &states)).unwrap();
    assert_eq!(events.rules.len(), 1);
    let rule = &events.rules[0];
    assert_eq!(rule.target_pattern.as_deref(), Some("obj/%.o"));
    assert_eq!(rule.source_line, 1);

    let evaluated_lines = RefCell::new(Vec::new());
    let instances = instantiate(rule, &|value, source_line| {
        evaluated_lines.borrow_mut().push(source_line);
        Ok(value.to_owned())
    })
    .unwrap();

    assert_eq!(instances.len(), 1);
    assert!(!evaluated_lines.borrow().is_empty());
    assert!(evaluated_lines
        .borrow()
        .iter()
        .all(|line| *line == rule.source_line));
    let instance = &instances[0];
    assert_eq!(instance.target, "obj/sub/a.o");
    assert_eq!(instance.stem.as_deref(), Some("sub/a"));
    assert_eq!(instance.prerequisites, ["src/sub/a.c", "src/sub/a.c"]);
    assert_eq!(instance.order_only, ["ready"]);
    assert_eq!(instance.source_line, 1);
    assert_eq!(instance.output_line, 1);
    assert_eq!(instance.recipes[0].text, "+@compile $< -o $@");

    let automatic = instance.automatic_variables();
    assert_eq!(automatic.get("@").map(String::as_str), Some("obj/sub/a.o"));
    assert_eq!(automatic.get("<").map(String::as_str), Some("src/sub/a.c"));
    assert_eq!(automatic.get("^").map(String::as_str), Some("src/sub/a.c"));
    assert_eq!(
        automatic.get("+").map(String::as_str),
        Some("src/sub/a.c src/sub/a.c")
    );
    assert_eq!(automatic.get("|").map(String::as_str), Some("ready"));
    assert_eq!(automatic.get("*").map(String::as_str), Some("sub/a"));
    assert_eq!(automatic.get("@D").map(String::as_str), Some("obj/sub"));
    assert_eq!(automatic.get("@F").map(String::as_str), Some("a.o"));
    assert_eq!(automatic.get("<D").map(String::as_str), Some("src/sub"));
    assert_eq!(automatic.get("<F").map(String::as_str), Some("a.c"));
    assert_eq!(automatic.get("*D").map(String::as_str), Some("sub"));
    assert_eq!(automatic.get("*F").map(String::as_str), Some("a"));
}

#[test]
fn automatic_list_path_parts_preserve_unique_and_repeated_prerequisites() {
    let source = "all: a/x.c b/y.c a/x.c bare /root.c | ignored/z\n";
    let events = collect(&positioned(source, &[Some(true)])).unwrap();
    let instance = instantiate_identity(&events.rules[0]).unwrap().remove(0);
    let values = instance.automatic_variables();
    assert_eq!(values["^D"], "a b . ");
    assert_eq!(values["^F"], "x.c y.c bare root.c");
    assert_eq!(values["+D"], "a b a . ");
    assert_eq!(values["+F"], "x.c y.c x.c bare root.c");
    assert!(!values.contains_key("?D"));
    assert!(!values.contains_key("%F"));
    let empty = collect(&positioned("empty:\n", &[Some(true)])).unwrap();
    let empty = instantiate_identity(&empty.rules[0]).unwrap().remove(0);
    for name in ["^D", "^F", "+D", "+F"] {
        assert_eq!(empty.automatic_variables()[name], "");
    }
}

#[test]
fn static_patterns_expand_each_target_and_order_only_automatic_values_filter() {
    let source = "objects/a.o objects/b.o: objects/%.o: src/%.c src/%.c | src/%.c ready ready\n";
    let states = [Some(true)];
    let events = collect(&positioned(source, &states)).unwrap();
    let instances = instantiate_identity(&events.rules[0]).unwrap();

    assert_eq!(instances.len(), 2);
    assert_eq!(instances[0].target, "objects/a.o");
    assert_eq!(instances[0].stem.as_deref(), Some("a"));
    assert_eq!(instances[1].target, "objects/b.o");
    assert_eq!(instances[1].stem.as_deref(), Some("b"));
    assert_eq!(instances[0].prerequisites, ["src/a.c", "src/a.c"]);
    assert_eq!(instances[0].order_only, ["src/a.c", "ready", "ready"]);
    assert_eq!(
        instances[0]
            .automatic_variables()
            .get("|")
            .map(String::as_str),
        Some("ready")
    );
}

#[test]
fn explicit_double_colon_rules_have_no_guessed_stem() {
    let source = "archive.a:: member.o\n";
    let states = [Some(true)];
    let events = collect(&positioned(source, &states)).unwrap();
    assert_eq!(events.rules.len(), 1);

    let instance = instantiate_identity(&events.rules[0]).unwrap().remove(0);
    assert!(instance.double_colon);
    assert_eq!(instance.stem, None);
    let automatic = instance.automatic_variables();
    assert!(!automatic.contains_key("*"));
    assert!(!automatic.contains_key("*D"));
    assert!(!automatic.contains_key("*F"));
}

#[test]
fn rejects_mismatched_duplicate_implicit_and_ambiguous_static_targets() {
    for (source, reason) in [
        (
            "obj/not-object.txt: obj/%.o: src/%.c\n",
            "does not match its static pattern",
        ),
        (
            "a.o a.o: %.o: %.c\n",
            "implicit pattern or duplicate concrete target",
        ),
        (
            "obj/a.o: obj/%%.o: src/%.c\n",
            "requires exactly one percent",
        ),
        (
            "%.o: %.c\n",
            "implicit pattern or duplicate concrete target",
        ),
    ] {
        let states = vec![Some(true); source.lines().count()];
        let program = positioned(source, &states);
        let events = collect(&program).unwrap();
        let error = instantiate_identity(&events.rules[0]).unwrap_err();
        assert!(error.contains(reason), "{source}: {error}");
    }
    let double = positioned("obj/a.o:: obj/%.o: src/%.c\n", &[Some(true)]);
    assert!(collect(&double)
        .unwrap_err()
        .contains("double-colon static-pattern rules are unsupported"));
}

#[test]
fn unresolved_header_expressions_are_not_treated_as_literal_paths() {
    let source = "output.o: $(UNKNOWN_INPUT)\n";
    let states = [Some(true)];
    let events = collect(&positioned(source, &states)).unwrap();

    assert!(instantiate_identity(&events.rules[0])
        .unwrap_err()
        .contains("expression is unresolved, unsafe or exceeds word limit"));
}

#[test]
fn automatic_recipe_context_is_opt_in_literal_and_does_not_guess_unknown_facts() {
    use crate::dirs::DirVars;
    use crate::make_expr::{evaluate_make_expr, MakeExprContext, MakeExprError};
    use crate::make_vars::collect_vars;
    use std::collections::BTreeMap;
    use std::path::Path;
    let scope = collect_vars("");
    let dirs = DirVars::default();
    let plain = MakeExprContext::new(&scope, &dirs, 0, Path::new("/source"), Path::new("."));
    assert!(evaluate_make_expr("$@", &plain).is_err());
    let values = BTreeMap::from([("@".to_owned(), "literal$target.o".to_owned())]);
    let context = plain.with_automatic_variables(&values);
    assert_eq!(
        evaluate_make_expr("$@ $(@)", &context).unwrap(),
        "literal$target.o literal$target.o"
    );
    assert!(
        matches!(evaluate_make_expr("$?", &context), Err(MakeExprError::UnresolvedVariables { names, .. }) if names == ["?"])
    );
    let wrong = BTreeMap::from([("CFLAGS".to_owned(), "-DWRONG".to_owned())]);
    assert!(matches!(
        evaluate_make_expr("literal", &plain.with_automatic_variables(&wrong)),
        Err(MakeExprError::InvalidSyntax { .. })
    ));
}

#[test]
fn empty_first_prerequisite_has_empty_directory_and_pattern_amplification_refuses() {
    let source = "unit.o:\n";
    let rules = collect(&positioned(source, &[Some(true)])).unwrap().rules;
    let instance = instantiate_identity(&rules[0]).unwrap().remove(0);
    let values = instance.automatic_variables();
    assert_eq!(values["<"], "");
    assert_eq!(values["<D"], "");
    assert_eq!(values["<F"], "");
    let huge = format!(
        "{}: %: %.c %.h %.s\n",
        "x".repeat(super::MAX_VALUE_BYTES / 2)
    );
    let rule = collect(&positioned(&huge, &[Some(true)]))
        .unwrap()
        .rules
        .remove(0);
    assert!(instantiate_identity(&rule)
        .unwrap_err()
        .contains("expansion exceeds value byte limit"));
}

#[test]
fn empty_recipes_remain_events_and_cannot_bypass_selection_or_orphan_checks() {
    let events = collect(&positioned("target:\n\t\n", &[Some(true); 2])).unwrap();
    assert_eq!(events.rules[0].recipes.len(), 1);
    assert_eq!(events.rules[0].recipes[0].text, "");
    assert_eq!(events.rules[0].recipes[0].source_line, 1);
    assert!(collect(&positioned("target:\n\t\n", &[Some(true), None]))
        .unwrap_err()
        .contains("recipe event has unresolved selection"));
    assert!(collect(&positioned("\t\n", &[Some(true)]))
        .unwrap_err()
        .contains("recipe has no active rule"));
}

#[test]
fn inactive_rules_are_skipped_and_unknown_rule_or_recipe_selection_refuses() {
    let source = concat!(
        "inactive.o: ignored.c\n",
        "\t@ignore\n",
        "active.o: source.c\n",
        "\t@build\n",
    );
    let states = [Some(false), Some(false), Some(true), Some(true)];
    let events = collect(&positioned(source, &states)).unwrap();
    assert_eq!(events.rules.len(), 1);
    assert_eq!(events.rules[0].source_line, 2);
    assert_eq!(events.rules[0].recipes[0].source_line, 3);
    let evaluated_sources = RefCell::new(Vec::new());
    for rule in &events.rules {
        instantiate(rule, &|value, source_line| {
            evaluated_sources.borrow_mut().push(source_line);
            Ok(value.to_owned())
        })
        .unwrap();
    }
    assert!(!evaluated_sources.borrow().is_empty());
    assert!(evaluated_sources.borrow().iter().all(|line| *line == 2));

    for (unknown_source, unknown_states) in [
        ("uncertain.o: source.c\n", vec![None]),
        ("known.o: source.c\n\t@build\n", vec![Some(true), None]),
    ] {
        assert!(
            collect(&positioned(unknown_source, &unknown_states)).is_err(),
            "accepted unresolved selection: {unknown_source}"
        );
    }
}

#[test]
fn assignments_break_recipe_attachment_and_orphan_recipes_refuse() {
    for statement in ["GLOBAL := value", "target.o: CFLAGS += -g"] {
        let source = format!("target.o: input.c\n{statement}\n\t@orphan\n");
        let states = vec![Some(true); source.lines().count()];
        assert!(collect(&positioned(&source, &states)).is_err());
    }

    let source = "\t@orphan\n";
    let states = [Some(true)];
    assert!(collect(&positioned(source, &states)).is_err());
}
