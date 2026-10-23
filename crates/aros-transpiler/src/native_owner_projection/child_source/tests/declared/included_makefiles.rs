use super::*;
use crate::make_vars::MakeFileEventKind;

use super::super::super::MatchedChildDeclaredRules;

fn include_event_index(rules: &MatchedChildDeclaredRules, requested: &str) -> usize {
    rules
        .file()
        .events
        .iter()
        .position(|event| {
            event.active
                && matches!(
                    &event.kind,
                    MakeFileEventKind::Include {
                        requested: observed,
                        ..
                    } if observed == requested
                )
        })
        .expect("synthetic include event is absent")
}

fn targets(trace: &crate::make_rule_graph::traversal::DeclaredTrace) -> Vec<&str> {
    trace
        .visits()
        .iter()
        .map(crate::make_rule_graph::traversal::TargetVisit::target)
        .collect()
}

#[test]
fn missing_include_view_traces_only_its_exact_producer_and_keeps_root_coordinates() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.working_directory.join("outside")).unwrap();
    let target = fixture
        .working_directory
        .join("outside/generated-rules.mk")
        .to_str()
        .unwrap()
        .to_owned();
    install_makefile(
        &fixture,
        &format!(
            "-include {target}\nroot:\n\t@printf root\n{target}: producer-input\n\t@printf generated\nproducer-input:\n\t@printf producer-input\n"
        ),
    );

    let rules = declared_child(&fixture);
    assert_eq!(targets(rules.traversal()), vec!["root"]);
    assert_eq!(
        rules
            .coordinates()
            .goals()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["root"]
    );

    let event_index = include_event_index(&rules, &target);
    let included = rules
        .declared_included_makefile(event_index)
        .unwrap()
        .expect("the exact missing include target has an explicit producer");

    assert!(included.include_event().active);
    assert!(matches!(
        &included.include_event().kind,
        MakeFileEventKind::Include {
            requested,
            present: false,
            resolved_identity: None,
        } if requested == &target
    ));
    assert_eq!(
        targets(included.traversal()),
        vec![target.as_str(), "producer-input"]
    );
    assert!(included.traversal().visits()[0].instance().is_some());
    assert_eq!(included.traversal().visits()[0].parent(), None);
    assert_eq!(included.traversal().visits()[1].parent(), Some(0));

    assert_eq!(included.coordinates().generated_owner(), "one/mmakefile");
    assert_eq!(included.coordinates().source_input(), "one/mmakefile.src");
    assert_eq!(
        included
            .coordinates()
            .goals()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["root"]
    );
    assert_eq!(targets(rules.traversal()), vec!["root"]);
}

#[test]
fn unmatched_optional_include_returns_none_without_basename_matching() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.working_directory.join("path")).unwrap();
    let requested = fixture
        .working_directory
        .join("path/x.mk")
        .to_str()
        .unwrap()
        .to_owned();
    install_makefile(
        &fixture,
        &format!("-include {requested}\nroot:\nx.mk: basename-only-prerequisite\n"),
    );

    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &requested);
    assert!(
        rules
            .declared_included_makefile(event_index)
            .unwrap()
            .is_none()
    );
    assert_eq!(targets(rules.traversal()), vec!["root"]);
}

#[test]
fn present_include_is_refused() {
    let fixture = Fixture::new();
    install_makefile(&fixture, "include present.mk\nroot:\n");
    std::fs::write(
        fixture.working_directory.join("present.mk"),
        "present-target:\n",
    )
    .unwrap();

    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, "present.mk");
    assert!(matches!(
        &rules.file().events[event_index].kind,
        MakeFileEventKind::Include { present: true, .. }
    ));
    assert!(rules.declared_included_makefile(event_index).is_err());
}

#[test]
fn non_include_and_out_of_range_events_are_refused() {
    let fixture = Fixture::new();
    install_makefile(&fixture, "root:\n");

    let rules = declared_child(&fixture);
    assert!(matches!(
        &rules.file().events[0].kind,
        MakeFileEventKind::Rule(_)
    ));
    assert!(rules.declared_included_makefile(0).is_err());
    assert!(rules.declared_included_makefile(usize::MAX).is_err());
}

#[test]
fn double_colon_and_cycle_in_include_producers_are_refused() {
    for producer in [
        "generated.mk:: prerequisite\n",
        "generated.mk: loop\nloop: generated.mk\n",
    ] {
        let fixture = Fixture::new();
        let target = fixture
            .working_directory
            .join("generated.mk")
            .to_str()
            .unwrap()
            .to_owned();
        let producer = producer.replace("generated.mk", &target);
        install_makefile(&fixture, &format!("-include {target}\nroot:\n{producer}"));

        let rules = declared_child(&fixture);
        let event_index = include_event_index(&rules, &target);
        assert!(rules.declared_included_makefile(event_index).is_err());
    }
}

#[test]
fn included_view_verification_refuses_source_drift() {
    let fixture = Fixture::new();
    let target = fixture
        .working_directory
        .join("future.mk")
        .to_str()
        .unwrap()
        .to_owned();
    install_makefile(
        &fixture,
        &format!("-include {target}\nroot:\n{target}: producer-input\n"),
    );

    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &target);
    let included = rules
        .declared_included_makefile(event_index)
        .unwrap()
        .expect("the missing include target has an explicit producer");
    included.verify().unwrap();

    std::fs::write(
        fixture.source_root.join("one/mmakefile.src"),
        "# source changed after the declared view was created\n",
    )
    .unwrap();
    assert!(included.verify().is_err());
}

#[test]
fn a_missing_include_that_appears_afterward_invalidates_the_old_view() {
    let fixture = Fixture::new();
    let target = fixture
        .working_directory
        .join("future.mk")
        .to_str()
        .unwrap()
        .to_owned();
    install_makefile(
        &fixture,
        &format!("-include {target}\nroot:\n{target}: producer-input\n"),
    );

    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &target);
    let included = rules
        .declared_included_makefile(event_index)
        .unwrap()
        .expect("the missing include target has an explicit producer");

    std::fs::write(&target, "future-target:\n").unwrap();
    assert!(included.verify().is_err());
    assert!(rules.declared_included_makefile(event_index).is_err());
}
