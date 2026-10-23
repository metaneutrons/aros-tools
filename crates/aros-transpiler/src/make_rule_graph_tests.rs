use crate::make_rule_events::{collect, instantiate, RuleInstance};
use crate::make_rule_graph::{compose, RuleGraph, TargetRules};
use crate::make_rule_program::{expand, RuleProgram};

fn positioned(source: &str) -> RuleProgram {
    let states = vec![Some(true); source.lines().count()];
    expand(source, &states, &|_, _, _| {
        Err("unexpected rule-program expression callback".into())
    })
    .unwrap()
}

fn parsed_instances(source: &str) -> Vec<RuleInstance> {
    let events = collect(&positioned(source)).unwrap();
    assert!(events.other_lines.is_empty());
    assert!(events.target_assignment_lines.is_empty());
    events
        .rules
        .iter()
        .flat_map(|rule| instantiate(rule, &|value, _| Ok(value.to_owned())).unwrap())
        .collect()
}

fn compose_source(source: &str) -> RuleGraph {
    compose(parsed_instances(source)).unwrap()
}

#[test]
fn fallible_composition_stops_before_consuming_later_declarations() {
    use std::cell::Cell;
    let pulled = Cell::new(0usize);
    let instances = parsed_instances("first:\nsecond:\nthird:\n");
    let stream = instances.into_iter().enumerate().map(|(index, instance)| {
        pulled.set(pulled.get() + 1);
        if index == 1 {
            Err("bounded producer failure".to_owned())
        } else {
            Ok(instance)
        }
    });
    let error = super::compose_fallible(stream).unwrap_err();
    assert_eq!(error, "bounded producer failure");
    assert_eq!(pulled.get(), 2);
}

#[test]
fn streaming_graph_count_and_byte_limits_stop_consumption() {
    use std::cell::Cell;
    for (max_count, max_bytes, expected_pulls) in [(1, usize::MAX, 2), (3, 0, 1)] {
        let pulled = Cell::new(0usize);
        let stream = parsed_instances("first:\nsecond:\nthird:\n")
            .into_iter()
            .map(|instance| {
                pulled.set(pulled.get() + 1);
                Ok(instance)
            });
        assert!(super::compose_result_iter_with_limits(stream, max_count, max_bytes).is_err());
        assert_eq!(pulled.get(), expected_pulls);
    }
}

#[test]
fn recipe_declaration_heads_merged_prerequisites_and_keeps_original_provenance() {
    let source = concat!(
        "all: before.c | before.stamp\n",
        "all: recipe.c | recipe.stamp\n",
        "\t+@build $< -o $@\n",
        "all: after.c | after.stamp\n",
    );
    let graph = compose_source(source);
    let TargetRules::Ordinary(target) = &graph.targets["all"] else {
        panic!("expected an ordinary target");
    };

    assert_eq!(
        target
            .declarations()
            .iter()
            .map(|declaration| declaration.output_line)
            .collect::<Vec<_>>(),
        [0, 1, 3]
    );
    assert_eq!(
        target
            .declarations()
            .iter()
            .map(|declaration| declaration.source_line)
            .collect::<Vec<_>>(),
        [0, 1, 3]
    );

    let effective = target.effective_instance();
    assert_eq!(effective.output_line, 1);
    assert_eq!(effective.source_line, 1);
    assert_eq!(effective.prerequisites, ["recipe.c", "before.c", "after.c"]);
    assert_eq!(
        effective.order_only,
        ["recipe.stamp", "before.stamp", "after.stamp"]
    );
    assert_eq!(effective.recipes.len(), 1);
    assert_eq!(effective.recipes[0].output_line, 2);
    assert_eq!(effective.recipes[0].source_line, 2);
    assert_eq!(effective.recipes[0].text, "+@build $< -o $@");
}

#[test]
fn recipe_less_target_falls_back_to_first_declaration_and_preserves_plus_order() {
    let graph = compose_source(concat!("all: a.c shared.c\n", "all: shared.c b.c a.c\n",));
    let TargetRules::Ordinary(target) = &graph.targets["all"] else {
        panic!("expected an ordinary target");
    };
    let effective = target.effective_instance();

    assert_eq!(effective.output_line, 0);
    assert_eq!(
        effective.prerequisites,
        ["a.c", "shared.c", "shared.c", "b.c", "a.c"]
    );
    let automatic = effective.automatic_variables();
    assert_eq!(automatic["+"], "a.c shared.c shared.c b.c a.c");
    assert_eq!(automatic["^"], "a.c shared.c b.c");
}

#[test]
fn normal_prerequisites_take_precedence_over_order_only_prerequisites() {
    let graph = compose_source(concat!(
        "all: common.c | common.c setup.stamp\n",
        "all: next.c | common.c next.c done.stamp\n",
    ));
    let TargetRules::Ordinary(target) = &graph.targets["all"] else {
        panic!("expected an ordinary target");
    };
    let effective = target.effective_instance();

    assert_eq!(effective.prerequisites, ["common.c", "next.c"]);
    assert_eq!(
        effective.order_only,
        [
            "common.c",
            "setup.stamp",
            "common.c",
            "next.c",
            "done.stamp"
        ]
    );
    let automatic = effective.automatic_variables();
    assert_eq!(automatic["^"], "common.c next.c");
    assert_eq!(automatic["|"], "setup.stamp done.stamp");
}

#[test]
fn promoted_order_only_input_retains_walk_and_unique_automatic_position() {
    let graph = compose_source("target: | early\ntarget: normal early\ntarget:\n\tprintf\n");
    let TargetRules::Ordinary(target) = &graph.targets["target"] else {
        panic!("ordinary target");
    };
    assert_eq!(
        target.prerequisite_walk().collect::<Vec<_>>(),
        ["early", "normal", "early"]
    );
    let effective = target.effective_instance();
    assert_eq!(effective.prerequisites, ["normal", "early"]);
    assert_eq!(effective.prerequisite_order, ["early", "normal", "early"]);
    let automatic = effective.automatic_variables();
    assert_eq!(automatic["<"], "normal");
    assert_eq!(automatic["+"], "normal early");
    assert_eq!(automatic["^"], "early normal");
    assert_eq!(automatic["^D"], ". .");
    assert_eq!(automatic["^F"], "early normal");
    assert_eq!(automatic["|"], "");
}

#[test]
fn composed_graph_refuses_inconsistent_prerequisite_order_records() {
    let original = parsed_instances("target: first first | stamp\n").remove(0);
    assert!(compose(vec![original.clone()]).is_ok());
    for order in [
        vec!["first", "stamp"],
        vec!["first", "first", "foreign"],
        vec!["first", "stamp", "stamp"],
        vec!["stamp", "first", "first"],
    ] {
        let mut changed = original.clone();
        changed.prerequisite_order = order.into_iter().map(str::to_owned).collect();
        assert!(compose(vec![changed]).is_err());
    }
}

#[test]
fn graph_checks_collection_counts_before_order_or_byte_scans() {
    let original = parsed_instances("target: input\n\tprintf\n").remove(0);
    assert!(compose(vec![original.clone()]).is_ok());
    let mut too_many_words = original.clone();
    too_many_words.prerequisites = vec![String::new(); 16_385];
    too_many_words.prerequisite_order = too_many_words.prerequisites.clone();
    let mut too_many_recipes = original.clone();
    too_many_recipes.recipes = vec![original.recipes[0].clone(); 262_145];
    for invalid in [too_many_words, too_many_recipes] {
        assert_eq!(
            compose(vec![invalid]).unwrap_err(),
            "Make prerequisite order has inconsistent size or exceeds collection limit"
        );
    }
    // The size estimator must stop at the remaining budget, not size the
    // complete supplied record first or allocate any composed target copies.
    assert!(super::instance_bytes(&original, usize::MAX).is_ok());
    assert_eq!(
        super::instance_bytes(&original, 0).unwrap_err(),
        "Make rule graph exceeds retained byte limit"
    );
}

#[test]
fn an_empty_tab_recipe_still_selects_its_declaration_as_provider() {
    let graph = compose_source(concat!(
        "all: before.c\n",
        "all:\n",
        "\t\n",
        "all: after.c\n"
    ));
    let TargetRules::Ordinary(target) = &graph.targets["all"] else {
        panic!("expected an ordinary target");
    };
    let effective = target.effective_instance();

    assert_eq!(effective.output_line, 1);
    assert_eq!(effective.prerequisites, ["before.c", "after.c"]);
    assert_eq!(effective.recipes.len(), 1);
    assert_eq!(effective.recipes[0].output_line, 2);
    assert_eq!(effective.recipes[0].text, "");
}

#[test]
fn refuses_two_ordinary_recipe_providers_with_the_exact_reason() {
    let error = compose(parsed_instances(concat!(
        "all: first.c\n",
        "\tbuild-first\n",
        "all: second.c\n",
        "\tbuild-second\n",
    )))
    .unwrap_err();

    assert_eq!(error, "Make ordinary target has multiple recipe providers");
    for replacement in ["all:;\n", "all:\n\t\n"] {
        let source = format!("all: first.c\n\tbuild-first\n{replacement}");
        assert_eq!(
            compose(parsed_instances(&source)).unwrap_err(),
            "Make ordinary target has multiple recipe providers"
        );
    }
}

#[test]
fn refuses_to_mix_ordinary_and_double_colon_rules() {
    let error = compose(parsed_instances(concat!(
        "all: ordinary.c\n",
        "all:: separate.c\n"
    )))
    .unwrap_err();

    assert_eq!(error, "Make target mixes ordinary and double-colon rules");
}

#[test]
fn double_colon_declarations_remain_independent() {
    let graph = compose_source(concat!(
        "all:: first.c\n",
        "\tbuild-first\n",
        "all:: second.c\n",
        "\tbuild-second\n",
    ));
    let TargetRules::DoubleColon(declarations) = &graph.targets["all"] else {
        panic!("expected independent double-colon declarations");
    };

    assert_eq!(declarations.len(), 2);
    assert_eq!(declarations[0].prerequisites, ["first.c"]);
    assert_eq!(declarations[0].recipes[0].text, "build-first");
    assert_eq!(declarations[1].prerequisites, ["second.c"]);
    assert_eq!(declarations[1].recipes[0].text, "build-second");
}

#[test]
fn no_prerequisite_recipe_target_is_valid() {
    let graph = compose_source("all:\n\t@build\n");
    let TargetRules::Ordinary(target) = &graph.targets["all"] else {
        panic!("expected an ordinary target");
    };
    let effective = target.effective_instance();

    assert!(effective.prerequisites.is_empty());
    assert_eq!(effective.recipes[0].text, "@build");
}

#[test]
fn target_order_preserves_first_appearance_instead_of_map_sort_order() {
    let graph = compose_source(concat!(
        "zeta: first.c\n",
        "alpha: second.c\n",
        "zeta: third.c\n"
    ));

    assert_eq!(graph.target_order, ["zeta", "alpha"]);
    assert_eq!(
        graph.targets.keys().map(String::as_str).collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
}

#[test]
fn unordered_and_duplicate_declaration_origins_are_refused() {
    let parsed = parsed_instances(concat!("first: one.c\n", "second: two.c\n"));
    let unordered = compose(vec![parsed[1].clone(), parsed[0].clone()]).unwrap_err();
    assert_eq!(
        unordered,
        "Make rule graph has unordered or duplicate declaration origins"
    );

    let duplicate = compose(vec![parsed[0].clone(), parsed[0].clone()]).unwrap_err();
    assert_eq!(
        duplicate,
        "Make rule graph has unordered or duplicate declaration origins"
    );
}

#[test]
fn the_last_static_pattern_stem_survives_a_later_recipe_declaration() {
    let graph = compose_source(concat!(
        "obj/a.o: obj/%.o: src/%.c\n",
        "obj/a.o: obj/a%: side/%.c\n",
        "obj/a.o: recipe.c\n",
        "\tbuild\n",
    ));
    let TargetRules::Ordinary(target) = &graph.targets["obj/a.o"] else {
        panic!("expected an ordinary target");
    };
    let effective = target.effective_instance();

    assert_eq!(effective.stem.as_deref(), Some(".o"));
    assert_eq!(effective.automatic_variables()["*"], ".o");
    assert_eq!(
        effective.prerequisites,
        ["recipe.c", "src/a.c", "side/.o.c"]
    );
}

#[test]
fn declaration_and_retained_byte_limits_refuse_with_small_test_thresholds() {
    let instance = parsed_instances("all: source.c\n").remove(0);
    let bytes = super::instance_bytes(&instance, usize::MAX).unwrap();
    assert!(super::compose_with_limits(vec![instance.clone()], 1, bytes).is_ok());
    assert_eq!(
        super::compose_with_limits(vec![instance.clone()], 1, bytes - 1).unwrap_err(),
        "Make rule graph exceeds retained byte limit"
    );

    assert_eq!(
        super::compose_with_limits(vec![instance.clone()], 0, usize::MAX).unwrap_err(),
        "Make rule graph exceeds declaration limit"
    );
    assert_eq!(
        super::compose_with_limits(vec![instance], 1, 0).unwrap_err(),
        "Make rule graph exceeds retained byte limit"
    );
}
