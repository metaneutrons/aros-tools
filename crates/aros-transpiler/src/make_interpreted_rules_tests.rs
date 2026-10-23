use super::collect;
use crate::{
    dirs::DirVars,
    make_rule_header::RuleHeader,
    make_vars::{
        MakeFileEvent, MakeFileEventKind, MakeFileProgram, MakeProgramPaths, MakeResolvedInput,
        MakeVariableProgram,
    },
    parser::TargetContext,
};
use std::path::Path;

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new("unit"),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build"),
    }
}

fn read(source: &str) -> (MakeVariableProgram, MakeFileProgram) {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let file = program
        .read_file("synthetic/main.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();
    (program, file)
}

fn ordinary_rule(targets: &str) -> RuleHeader {
    RuleHeader::Rule {
        targets: targets.into(),
        target_pattern: None,
        prerequisites: String::new(),
        order_only: String::new(),
        double_colon: false,
        inline_recipe: None,
    }
}

fn event(kind: MakeFileEventKind, active: bool) -> MakeFileEvent {
    MakeFileEvent {
        input: "synthetic/main.mk".into(),
        source_line: 0,
        generated: false,
        active,
        variable_position: 0,
        kind,
    }
}

#[test]
fn source_projection_composes_static_rules_and_keeps_every_policy_obligation() {
    let (_, file) = read(concat!(
        ".PHONY: all\n",
        ".SUFFIXES:\n",
        "vpath %.src source\n",
        "all: a.obj b.obj\n",
        "a.obj b.obj: %.obj: %.src | directory\n",
        "\tcompile $< -o $@\n",
        "a.obj: extra\n",
        "%.obj: generated-header\n",
        "%.other: %.input; implicit-command\n",
        ".DEFAULT:; default-command\n",
    ));
    let interpreted = collect(&file).unwrap();
    let projected = interpreted.project_explicit().unwrap();
    assert_eq!(projected.graph.target_order, ["all", "a.obj", "b.obj"]);
    let crate::make_rule_graph::TargetRules::Ordinary(a) = &projected.graph.targets["a.obj"] else {
        panic!("expected ordinary static-pattern instance");
    };
    let instance = a.effective_instance();
    assert_eq!(instance.stem.as_deref(), Some("a"));
    assert_eq!(instance.prerequisites, ["a.src", "extra"]);
    assert_eq!(instance.prerequisite_order, ["a.src", "directory", "extra"]);
    assert_eq!(instance.recipes[0].text, "compile $< -o $@");
    assert_eq!(projected.implicit_rule_events.len(), 2);
    for index in &projected.implicit_rule_events {
        assert!(matches!(&file.events[*index].kind,
            MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) if targets.contains('%')));
    }
    assert!(interpreted
        .other_events
        .iter()
        .any(|index| matches!(file.events[*index].kind, MakeFileEventKind::PhonyTargets(_))));
    assert!(interpreted
        .other_events
        .iter()
        .any(|index| matches!(file.events[*index].kind, MakeFileEventKind::DefaultRule(_))));
    assert!(!projected.graph.targets.contains_key("generated-header"));
}

#[test]
fn source_projection_does_not_hide_invalid_explicit_rules_behind_patterns() {
    let (_, file) = read("%.obj: %.src\nx:; first\nx:; second\n");
    assert!(collect(&file).unwrap().project_explicit().is_err());
    let (_, file) = read("%.obj: %.src\nx.obj: %.wrong: %.src\n");
    assert!(collect(&file).unwrap().project_explicit().is_err());
}

#[test]
fn source_projection_bounds_public_event_collections_before_partitioning() {
    let (_, file) = read("all:\n");
    let mut interpreted = collect(&file).unwrap();
    interpreted.other_events = vec![0; super::MAX_EVENTS + 1];
    assert!(interpreted.project_explicit().is_err());
}

#[test]
fn source_projection_rejects_reordered_implicit_and_mixed_source_rules() {
    let (_, file) = read("%.first: %.src\n%.second: %.input\n");
    let mut interpreted = collect(&file).unwrap();
    interpreted.rules.reverse();
    assert!(interpreted.project_explicit().is_err());

    let (_, first) = read("%.first: %.src\n");
    let (_, second) = read("VALUE := input\n%.second: %.input\n");
    let mut interpreted = collect(&first).unwrap();
    interpreted.rules.extend(collect(&second).unwrap().rules);
    assert!(interpreted.project_explicit().is_err());

    let (_, empty) = read(".PHONY: all\n");
    let mut interpreted = collect(&empty).unwrap();
    interpreted.rules.extend(collect(&first).unwrap().rules);
    assert!(interpreted.project_explicit().is_err());
}

#[test]
fn frozen_headers_are_not_reexpanded_after_global_variables_change() {
    let (mut variables, file) = read(concat!(
        "TARGETS := early\n",
        "PREREQUISITES := first\n",
        "$(TARGETS): $(PREREQUISITES)\n",
        "\tcompile $@ $(PREREQUISITES)\n",
        "TARGETS := late\n",
        "PREREQUISITES := later\n",
    ));

    let dirs = DirVars::default();
    let paths = paths(&dirs);
    assert_eq!(variables.evaluate("$(TARGETS)", &paths).unwrap(), "late");
    assert_eq!(
        variables.evaluate("$(PREREQUISITES)", &paths).unwrap(),
        "later"
    );

    let interpreted = collect(&file).unwrap();
    assert_eq!(interpreted.rules.len(), 1);
    assert!(matches!(
        interpreted.rules[0].header(),
        RuleHeader::Rule { targets, prerequisites, .. }
            if targets == "early" && prerequisites == "first"
    ));
    let instance = interpreted.rules[0]
        .instantiate_explicit()
        .unwrap()
        .remove(0);
    assert_eq!(instance.target, "early");
    assert_eq!(instance.prerequisites, ["first"]);
    assert_eq!(instance.recipes[0].text, "compile $@ $(PREREQUISITES)");
}

#[test]
fn inline_and_tabbed_recipes_keep_their_rule_association_and_positions() {
    let (_, file) = read(concat!(
        "inline-target: input; inline-command $@\n",
        "tabbed-target: other-input\n",
        "\ttabbed-command $@\n",
    ));
    let interpreted = collect(&file).unwrap();
    assert_eq!(interpreted.rules.len(), 2);

    let inline = &interpreted.rules[0];
    assert!(inline.recipes().is_empty());
    assert!(matches!(
        inline.header(),
        RuleHeader::Rule { inline_recipe: Some(text), .. } if text == "inline-command $@"
    ));
    let inline_instance = inline.instantiate_explicit().unwrap().remove(0);
    assert_eq!(inline_instance.recipes.len(), 1);
    assert_eq!(inline_instance.recipes[0].text, "inline-command $@");
    assert_eq!(inline_instance.recipes[0].output_line, inline.event_index());
    assert_eq!(inline_instance.recipes[0].source_line, 0);

    let tabbed = &interpreted.rules[1];
    assert!(matches!(
        tabbed.header(),
        RuleHeader::Rule {
            inline_recipe: None,
            ..
        }
    ));
    assert_eq!(tabbed.recipes().len(), 1);
    let recipe_index = tabbed.recipes()[0];
    assert!(matches!(
        &file.events[recipe_index].kind,
        MakeFileEventKind::Recipe { rule_event, .. } if *rule_event == tabbed.event_index()
    ));
    let tabbed_instance = tabbed.instantiate_explicit().unwrap().remove(0);
    assert_eq!(tabbed_instance.recipes.len(), 1);
    assert_eq!(tabbed_instance.recipes[0].text, "tabbed-command $@");
    assert_eq!(tabbed_instance.recipes[0].output_line, recipe_index);
    assert_eq!(tabbed_instance.recipes[0].source_line, 2);
}

#[test]
fn included_and_eval_generated_rules_keep_distinct_source_identity() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut variables = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    let file = variables
        .read_file_resolved(
            "synthetic/main.mk",
            concat!(
                "include child.mk\n",
                "define generated_rule\n",
                "eval-target: eval-prerequisite\n",
                "\teval-command $$@\n",
                "endef\n",
                "$(eval $(generated_rule))\n",
                "root-target: root-prerequisite\n",
                "\troot-command $@\n",
            ),
            &paths,
            &|requested, from| {
                if (requested, from) == ("child.mk", "synthetic/main.mk") {
                    Ok(Some(MakeResolvedInput {
                        identity: "synthetic/selected/child.mk".into(),
                        text: "CHILD := included\nchild-$(CHILD): child-prerequisite\n\tchild-command $@\n"
                            .into(),
                    }))
                } else {
                    Err(format!("unexpected include {requested} from {from}"))
                }
            },
        )
        .unwrap();

    let interpreted = collect(&file).unwrap();
    let provenance = interpreted
        .rules
        .iter()
        .map(|rule| {
            let source = &file.events[rule.event_index()];
            (
                rule.header(),
                source.input.as_str(),
                source.source_line,
                source.generated,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(provenance.len(), 3);
    assert!(matches!(
        provenance[0],
        (
            RuleHeader::Rule { targets, .. },
            "synthetic/selected/child.mk",
            1,
            false
        ) if targets == "child-included"
    ));
    assert!(matches!(
        provenance[1],
        (
            RuleHeader::Rule { targets, .. },
            "synthetic/main.mk",
            5,
            true
        ) if targets == "eval-target"
    ));
    assert!(matches!(
        provenance[2],
        (
            RuleHeader::Rule { targets, .. },
            "synthetic/main.mk",
            6,
            false
        ) if targets == "root-target"
    ));

    let eval_instance = interpreted.rules[1]
        .instantiate_explicit()
        .unwrap()
        .remove(0);
    assert_eq!(eval_instance.source_line, 5);
    assert_eq!(eval_instance.recipes[0].source_line, 5);
    assert_eq!(
        eval_instance.recipes[0].output_line,
        interpreted.rules[1].recipes()[0]
    );
}

#[test]
fn static_pattern_instances_preserve_repeats_and_automatic_values() {
    let (_, file) = read(concat!(
        "TARGETS := unit/a.obj unit/b.obj\n",
        "$(TARGETS): unit/%.obj: source/%.src common common | marker marker\n",
        "\tcompile $$@ $$< $$^ $$+ $$|\n",
    ));
    let interpreted = collect(&file).unwrap();
    assert_eq!(interpreted.rules.len(), 1);
    let instances = interpreted.rules[0].instantiate_explicit().unwrap();
    assert_eq!(instances.len(), 2);
    assert_eq!(instances[0].target, "unit/a.obj");
    assert_eq!(instances[0].stem.as_deref(), Some("a"));
    assert_eq!(
        instances[0].prerequisites,
        ["source/a.src", "common", "common"]
    );
    assert_eq!(instances[0].order_only, ["marker", "marker"]);
    assert_eq!(instances[1].target, "unit/b.obj");
    assert_eq!(instances[1].stem.as_deref(), Some("b"));
    assert_eq!(
        instances[1].prerequisites,
        ["source/b.src", "common", "common"]
    );

    let automatic = instances[0].automatic_variables();
    assert_eq!(automatic.get("@").map(String::as_str), Some("unit/a.obj"));
    assert_eq!(automatic.get("<").map(String::as_str), Some("source/a.src"));
    assert_eq!(
        automatic.get("^").map(String::as_str),
        Some("source/a.src common")
    );
    assert_eq!(
        automatic.get("+").map(String::as_str),
        Some("source/a.src common common")
    );
    assert_eq!(automatic.get("|").map(String::as_str), Some("marker"));
}

#[test]
fn special_and_nonordinary_events_remain_in_other_events() {
    let (_, file) = read(concat!(
        "target: LOCAL = value\n",
        ".PHONY: clean\n",
        ".SUFFIXES: .src .obj\n",
        "vpath %.obj generated\n",
        ".DEFAULT: fallback\n",
        "\tdefault-command $@\n",
        "ordinary: prerequisite\n",
        "\tordinary-command $@\n",
    ));
    let interpreted = collect(&file).unwrap();
    assert_eq!(interpreted.rules.len(), 1);
    assert!(matches!(
        interpreted.rules[0].header(),
        RuleHeader::Rule { targets, .. } if targets == "ordinary"
    ));

    let ordinary_events = interpreted
        .rules
        .iter()
        .flat_map(|rule| std::iter::once(rule.event_index()).chain(rule.recipes().iter().copied()))
        .collect::<Vec<_>>();
    let expected_other = file
        .events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            (event.active && !ordinary_events.contains(&index)).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(interpreted.other_events, expected_other);

    for (index, event) in file.events.iter().enumerate() {
        let retained = match &event.kind {
            MakeFileEventKind::Rule(RuleHeader::TargetAssignment { .. })
            | MakeFileEventKind::PhonyTargets(_)
            | MakeFileEventKind::Suffixes(_)
            | MakeFileEventKind::SearchPath(_)
            | MakeFileEventKind::DefaultRule(_) => true,
            MakeFileEventKind::Recipe { rule_event, .. }
                if matches!(
                    &file.events[*rule_event].kind,
                    MakeFileEventKind::DefaultRule(_)
                ) =>
            {
                true
            }
            _ => false,
        };
        if retained {
            assert!(interpreted.other_events.contains(&index));
        }
    }
}

#[test]
fn active_recipe_links_to_orphan_forward_inactive_or_nonrule_events_refuse() {
    let cases = [
        MakeFileProgram {
            events: vec![event(
                MakeFileEventKind::Recipe {
                    rule_event: 17,
                    text: "orphan".into(),
                },
                true,
            )],
        },
        MakeFileProgram {
            events: vec![
                event(
                    MakeFileEventKind::Recipe {
                        rule_event: 1,
                        text: "forward".into(),
                    },
                    true,
                ),
                event(MakeFileEventKind::Rule(ordinary_rule("later")), true),
            ],
        },
        MakeFileProgram {
            events: vec![
                event(MakeFileEventKind::Rule(ordinary_rule("inactive")), false),
                event(
                    MakeFileEventKind::Recipe {
                        rule_event: 0,
                        text: "inactive-reference".into(),
                    },
                    true,
                ),
            ],
        },
        MakeFileProgram {
            events: vec![
                event(
                    MakeFileEventKind::Rule(RuleHeader::TargetAssignment {
                        targets: "assigned".into(),
                        assignment: "LOCAL = value".into(),
                    }),
                    true,
                ),
                event(
                    MakeFileEventKind::Recipe {
                        rule_event: 0,
                        text: "nonrule-reference".into(),
                    },
                    true,
                ),
            ],
        },
    ];

    for file in &cases {
        assert!(
            collect(file).is_err(),
            "invalid recipe link must be refused"
        );
    }
}

#[test]
fn implicit_pattern_rules_are_not_instantiated_as_explicit_rules() {
    let (_, file) = read("%.obj: %.src\n\tcompile $< -o $@\n");
    let interpreted = collect(&file).unwrap();
    assert_eq!(interpreted.rules.len(), 1);
    assert!(interpreted.rules[0]
        .instantiate_explicit()
        .unwrap_err()
        .contains("implicit pattern"));
}

#[test]
fn recipe_source_and_generated_origins_cannot_be_swapped() {
    for counter in 0..3 {
        let rule = event(MakeFileEventKind::Rule(ordinary_rule("selected")), true);
        let mut recipe = event(
            MakeFileEventKind::Recipe {
                rule_event: 0,
                text: "command $@".into(),
            },
            true,
        );
        let mut file = MakeFileProgram {
            events: vec![rule.clone(), recipe.clone()],
        };
        assert!(collect(&file).is_ok());
        match counter {
            0 => recipe.input = "foreign.mk".into(),
            1 => recipe.generated = true,
            _ => {
                file.events[0].generated = true;
                recipe.generated = true;
                recipe.source_line = 1;
            }
        }
        file.events[1] = recipe;
        assert!(matches!(collect(&file), Err(error) if error.contains("foreign source")));
    }
}

#[test]
fn event_count_limit_is_checked_before_collection() {
    let file = MakeFileProgram {
        events: (0..=65_536)
            .map(|_| event(MakeFileEventKind::Control("inactive".into()), false))
            .collect(),
    };
    assert!(matches!(collect(&file), Err(error) if error.contains("event limit")));
}
