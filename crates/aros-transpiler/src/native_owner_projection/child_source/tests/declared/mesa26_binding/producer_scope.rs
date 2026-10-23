use super::*;
use crate::native_owner_projection::Mesa26OriginalProducerSelection;

const SELECTED_STAMP: &str = "selected-stamp";
const GENERATED_LIST: &str = "generated-list";
const EXISTING_INPUT: &str = "existing-input";
const EXTRA_INPUT: &str = "extra-input";

#[derive(Clone, Copy, Debug)]
enum ProducerScopeGraph {
    Normal,
    MultipleSourceDeclarations,
    ForeignConsumerEdge,
    AliasOrderOnly,
    AliasOwnRecipe,
    AliasExtraPrerequisite,
    ConsumerOrderOnly,
    ConsumerDeeper,
}

fn source_owned_rules(graph: ProducerScopeGraph) -> String {
    let mut source = String::new();
    if matches!(graph, ProducerScopeGraph::ConsumerDeeper) {
        writeln!(source, "{MESA26_GCA_GOAL}: intermediate").unwrap();
        writeln!(source, "intermediate: {ARCHIVE_FINALIZER}").unwrap();
    } else {
        writeln!(source, "{MESA26_GCA_GOAL}: {ARCHIVE_FINALIZER}").unwrap();
    }

    let consumer_prerequisites = match graph {
        ProducerScopeGraph::ConsumerOrderOnly => format!("| {GENERATED_LIST}"),
        ProducerScopeGraph::MultipleSourceDeclarations
        | ProducerScopeGraph::ForeignConsumerEdge => String::new(),
        _ => GENERATED_LIST.to_owned(),
    };
    writeln!(source, "{ARCHIVE_FINALIZER}: {consumer_prerequisites}").unwrap();
    writeln!(source, "\t@printf 'consumer\\n' > $@").unwrap();
    if matches!(graph, ProducerScopeGraph::MultipleSourceDeclarations) {
        writeln!(source, "{ARCHIVE_FINALIZER}: {GENERATED_LIST}").unwrap();
    }

    let alias_prerequisites = match graph {
        ProducerScopeGraph::AliasOrderOnly => format!("| {SELECTED_STAMP}"),
        ProducerScopeGraph::AliasExtraPrerequisite => {
            format!("{SELECTED_STAMP} {EXTRA_INPUT}")
        }
        _ => SELECTED_STAMP.to_owned(),
    };
    writeln!(source, "{GENERATED_LIST}: {alias_prerequisites}").unwrap();
    if matches!(graph, ProducerScopeGraph::AliasOwnRecipe) {
        writeln!(source, "\t@printf 'alias\\n' > $@").unwrap();
    }

    writeln!(source, "{SELECTED_STAMP}: {EXISTING_INPUT}").unwrap();
    writeln!(source, "\t@printf 'stamp\\n' > $@").unwrap();
    source
}

fn selection(producer: &str, cooutput: &str, consumer: &str) -> Mesa26OriginalProducerSelection {
    Mesa26OriginalProducerSelection {
        source_input: MESA26_GCA_SOURCE.into(),
        goal: MESA26_GCA_GOAL.into(),
        producer: producer.into(),
        cooutput: cooutput.into(),
        consumer: consumer.into(),
    }
}

fn install_source_producer_case(
    context: TargetContext,
    graph: ProducerScopeGraph,
    selections: Vec<Mesa26OriginalProducerSelection>,
) -> Fixture {
    install_source_producer_case_with_foreign_edge(context, graph, selections, false)
}

fn install_source_producer_case_with_foreign_edge(
    context: TargetContext,
    graph: ProducerScopeGraph,
    selections: Vec<Mesa26OriginalProducerSelection>,
    foreign_consumer_edge: bool,
) -> Fixture {
    let mut fixture = install_case(MESA26_GCA_SOURCE, context.clone(), false, &[]);
    let source_path = fixture.source_root.join(MESA26_GCA_SOURCE);
    let source = std::fs::read_to_string(&source_path)
        .expect("synthetic Mesa Source owner should be readable");
    let original_goal =
        format!("{MESA26_GCA_GOAL}:\n\t@printf '%s\\n' '$@' 'captured original recipe'\n");
    assert_eq!(source.matches(&original_goal).count(), 1);
    let mut source = source.replacen(&original_goal, &source_owned_rules(graph), 1);
    if foreign_consumer_edge {
        source.push_str("include foreign-graph.mk\n");
    }
    std::fs::write(&source_path, source).expect("synthetic Source owner should be rewritten");

    super::captured_inputs::refresh_mesa26_source(&mut fixture);
    if foreign_consumer_edge {
        std::fs::write(
            fixture.working_directory.join("foreign-graph.mk"),
            format!("{ARCHIVE_FINALIZER}: {GENERATED_LIST}\n"),
        )
        .expect("synthetic foreign included rule should be written");
    }
    fixture.projection.architecture_context = context;
    fixture.projection.mesa26_original_producers = selections;
    for input in [EXISTING_INPUT, EXTRA_INPUT] {
        std::fs::write(
            fixture.working_directory.join(input),
            "captured producer input",
        )
        .expect("synthetic producer input should be captured before projection");
    }
    fixture
}

fn producer_binding_refusal(fixture: &Fixture) -> String {
    let mut recipes = declared_recipes(fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let index = recipe_index_for_target(&recipes, SELECTED_STAMP);
    binding_refusal_at(&mut recipes, fixture, index)
}

fn assert_source_rule_header(recipes: &DeclaredChildRecipeProjection, target: &str) {
    assert_source_rule_header_count(recipes, target, 1);
}

fn assert_source_rule_header_count(
    recipes: &DeclaredChildRecipeProjection,
    target: &str,
    expected: usize,
) {
    let events = &recipes.declared().file().events;
    let matching = events
        .iter()
        .filter(|event| {
            event.input == "mmakefile"
                && matches!(
                    &event.kind,
                    crate::make_vars::MakeFileEventKind::Rule(
                        crate::make_rule_header::RuleHeader::Rule { targets, .. }
                    ) if targets == target
                )
        })
        .count();
    assert_eq!(
        matching, expected,
        "captured Source rule header count for {target}"
    );
}

#[test]
fn mesa26_binding_accepts_explicit_source_owned_depth_four_producer_scope() {
    let fixture = install_source_producer_case(
        mesa26_arm_context(),
        ProducerScopeGraph::Normal,
        vec![selection(SELECTED_STAMP, GENERATED_LIST, ARCHIVE_FINALIZER)],
    );
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let producer_index = recipe_index_for_target(&recipes, SELECTED_STAMP);
    let consumer_index = recipe_index_for_target(&recipes, ARCHIVE_FINALIZER);

    for target in [SELECTED_STAMP, GENERATED_LIST, ARCHIVE_FINALIZER] {
        assert_source_rule_header(&recipes, target);
    }
    for index in [producer_index, consumer_index] {
        let (input, source_input, owner) = recipes
            .with_source_generated_recipe(
                &fixture.source_root,
                &fixture.projection,
                index,
                |_, origin, coordinates| {
                    Ok((
                        origin.input.clone(),
                        coordinates.source_input().to_owned(),
                        coordinates.generated_owner().to_owned(),
                    ))
                },
            )
            .expect("producer and consumer recipes must bind to the captured Source entry");
        assert_eq!(input, "mmakefile");
        assert_eq!(source_input, MESA26_GCA_SOURCE);
        assert_eq!(owner, MESA26_GCA_SOURCE.strip_suffix(".src").unwrap());
    }

    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE.strip_suffix(".src").unwrap())
        .parent()
        .unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, producer_index)
        .expect("the explicitly selected depth-four Source producer should bind");
    assert!(binding.intrinsic_selection_sha256().is_some());
    assert!(binding.recipe().expression().contains("stamp"));
    binding
        .revalidate()
        .expect("the source-owned producer binding should retain original inputs");
}

#[test]
fn mesa26_binding_accepts_multiple_role_declarations_from_the_same_source_entry() {
    let fixture = install_source_producer_case(
        mesa26_arm_context(),
        ProducerScopeGraph::MultipleSourceDeclarations,
        vec![selection(SELECTED_STAMP, GENERATED_LIST, ARCHIVE_FINALIZER)],
    );
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let producer_index = recipe_index_for_target(&recipes, SELECTED_STAMP);
    assert_source_rule_header_count(&recipes, ARCHIVE_FINALIZER, 2);

    let dirs = DirVars::default();
    let relative = Path::new(MESA26_GCA_SOURCE.strip_suffix(".src").unwrap())
        .parent()
        .unwrap();
    let paths = fixture.paths(&dirs, relative, &fixture.working_directory);
    let binding = recipes
        .bind_mesa26_original_recipe(&fixture.projection, &paths, producer_index)
        .expect("both ordinary declarations should retain the original Source entry");
    assert!(binding.intrinsic_selection_sha256().is_some());
}

#[test]
fn mesa26_binding_rejects_a_foreign_included_consumer_edge() {
    let fixture = install_source_producer_case_with_foreign_edge(
        mesa26_arm_context(),
        ProducerScopeGraph::ForeignConsumerEdge,
        vec![selection(SELECTED_STAMP, GENERATED_LIST, ARCHIVE_FINALIZER)],
        true,
    );
    let mut recipes = declared_recipes(&fixture, MESA26_GCA_SOURCE, &[MESA26_GCA_GOAL]);
    let producer_index = recipe_index_for_target(&recipes, SELECTED_STAMP);
    let foreign_consumer_header = recipes.declared().file().events.iter().any(|event| {
        event.input.ends_with("foreign-graph.mk")
            && matches!(
                &event.kind,
                crate::make_vars::MakeFileEventKind::Rule(
                    crate::make_rule_header::RuleHeader::Rule { targets, .. }
                ) if targets == ARCHIVE_FINALIZER
            )
    });
    assert!(
        foreign_consumer_header,
        "the edge must come from a captured include"
    );

    let refusal = binding_refusal_at(&mut recipes, &fixture, producer_index);
    assert_eq!(
        refusal,
        "intrinsic role header is outside the Source-expanded owner"
    );
}

#[test]
fn mesa26_binding_rejects_unselected_deep_scope_and_wrong_producer_roles() {
    let graph = ProducerScopeGraph::Normal;
    let no_policy = install_source_producer_case(mesa26_arm_context(), graph, vec![]);
    assert!(producer_binding_refusal(&no_policy).contains("another goal or deeper dependency"));

    for policy in [
        vec![selection("other-stamp", GENERATED_LIST, ARCHIVE_FINALIZER)],
        vec![selection(SELECTED_STAMP, "other-list", ARCHIVE_FINALIZER)],
        vec![selection(SELECTED_STAMP, GENERATED_LIST, "other-consumer")],
        {
            let selected = selection(SELECTED_STAMP, GENERATED_LIST, ARCHIVE_FINALIZER);
            vec![selected.clone(), selected]
        },
    ] {
        let fixture = install_source_producer_case(mesa26_arm_context(), graph, policy);
        assert!(
            !producer_binding_refusal(&fixture).is_empty(),
            "an unselected or duplicate producer role must refuse"
        );
    }

    let mut mesa20 = mesa26_arm_context();
    mesa20.mesa_version = Some("20.0.8".into());
    let mesa20 = install_source_producer_case(
        mesa20,
        graph,
        vec![selection(SELECTED_STAMP, GENERATED_LIST, ARCHIVE_FINALIZER)],
    );
    assert!(producer_binding_refusal(&mesa20).contains("requires selected Mesa 26 policy"));
}

#[test]
fn mesa26_binding_rejects_invalid_source_producer_alias_and_consumer_graphs() {
    for graph in [
        ProducerScopeGraph::AliasOrderOnly,
        ProducerScopeGraph::AliasOwnRecipe,
        ProducerScopeGraph::AliasExtraPrerequisite,
        ProducerScopeGraph::ConsumerOrderOnly,
        ProducerScopeGraph::ConsumerDeeper,
    ] {
        let fixture = install_source_producer_case(
            mesa26_arm_context(),
            graph,
            vec![selection(SELECTED_STAMP, GENERATED_LIST, ARCHIVE_FINALIZER)],
        );
        assert!(
            !producer_binding_refusal(&fixture).is_empty(),
            "invalid Source-owned role graph should refuse: {graph:?}"
        );
    }
}
