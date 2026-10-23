use super::*;
use crate::make_vars::MakeFileEventKind;
use crate::native_owner_projection::MatchedChildDeclaredRules;
use std::{cell::Cell, fmt::Write as _, path::Path};

fn include_target(fixture: &Fixture) -> String {
    fixture
        .working_directory
        .join("outside/generated-rules.mk")
        .to_str()
        .expect("synthetic include target is not UTF-8")
        .to_owned()
}

fn install_include_source(fixture: &mut Fixture, with_producer: bool) -> String {
    std::fs::create_dir_all(fixture.working_directory.join("outside")).unwrap();
    let target = include_target(fixture);
    let mut source = format!("#MM root\nGLOBAL_VALUE := inherited-value\n-include {target}\n");
    if with_producer {
        write!(
            source,
            concat!(
                "{target}: INCLUDE_VALUE := target-specific\n",
                "{target}: include-child\n",
                "include-child: include-input.dat\n",
                "\t@printf '%s' '$(GLOBAL_VALUE)|$(INCLUDE_VALUE)|$(origin INCLUDE_VALUE)|$@|$<' > include-recipe-executed\n",
            ),
            target = target,
        )
        .unwrap();
    }
    source.push_str("root:\n\t@printf root > include-root-recipe-executed\n");
    install_source_expanded_program(fixture, &source);
    target
}

fn include_event_index(rules: &MatchedChildDeclaredRules, target: &str) -> usize {
    rules
        .file()
        .events
        .iter()
        .position(|event| {
            event.active
                && matches!(
                    &event.kind,
                    MakeFileEventKind::Include {
                        requested,
                        present: false,
                        resolved_identity: None,
                    } if requested == target
                )
        })
        .expect("synthetic missing include event is absent")
}

fn paths_with_roots<'a>(
    fixture: &'a Fixture,
    dirs: &'a crate::dirs::DirVars,
    source_root: &'a Path,
    build_root: &'a Path,
) -> crate::make_vars::MakeProgramPaths<'a> {
    crate::make_vars::MakeProgramPaths {
        dirs,
        source_root,
        relative_dir: Path::new("one"),
        top_directory: build_root,
        working_directory: &fixture.working_directory,
    }
}

fn refused<T>(result: Result<T, String>) -> String {
    match result {
        Err(error) => error,
        Ok(_) => panic!("operation unexpectedly reached its successful result"),
    }
}

fn refuse_without_consumer(
    rules: MatchedChildDeclaredRules,
    projection: &crate::native_owner_projection::NativeOwnerProjection,
    selection: (usize, usize),
    paths: &crate::make_vars::MakeProgramPaths<'_>,
) -> String {
    let called = Cell::new(false);
    let result =
        rules.with_source_generated_include_recipe(projection, selection, paths, |_, _, _, _| {
            called.set(true);
            Ok(())
        });
    assert!(!called.get(), "refused include recipe reached its consumer");
    refused(result)
}

#[test]
fn source_include_recipe_uses_inherited_target_frame_without_running_shell() {
    let mut fixture = Fixture::new();
    let target = install_include_source(&mut fixture, true);
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let rules = declared_child(&fixture);
    assert_eq!(
        rules
            .traversal()
            .visits()
            .iter()
            .map(crate::make_rule_graph::traversal::TargetVisit::target)
            .collect::<Vec<_>>(),
        ["root"]
    );
    assert_eq!(
        rules
            .coordinates()
            .goals()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["root"]
    );
    let event_index = include_event_index(&rules, &target);

    // The only recipe is on a prerequisite of the include target. This checks
    // inherited frame data without treating it as a direct producer proof.
    let (rules, selected, reads) = rules
        .with_source_generated_include_recipe(
            &fixture.projection,
            (event_index, 0),
            &paths,
            |trace, recipe, origin, coordinates| {
                Ok((
                    trace
                        .visits()
                        .iter()
                        .map(|visit| visit.target().to_owned())
                        .collect::<Vec<_>>(),
                    recipe.expression().expression().to_owned(),
                    origin.input.clone(),
                    origin.generated,
                    coordinates.generated_owner().to_owned(),
                    coordinates.source_input().to_owned(),
                    coordinates.goals().to_vec(),
                ))
            },
        )
        .unwrap();

    assert_eq!(
        selected.0,
        [target.as_str(), "include-child", "include-input.dat"]
    );
    assert!(
        selected
            .1
            .contains("'inherited-value|target-specific|file|include-child|include-input.dat'")
    );
    assert_eq!(selected.2, "mmakefile");
    assert!(!selected.3);
    assert_eq!(selected.4, "one/mmakefile");
    assert_eq!(selected.5, "one/mmakefile.src");
    assert_eq!(
        fixture
            .working_directory
            .join(&selected.2)
            .canonicalize()
            .unwrap(),
        fixture.build_root.join(&selected.4).canonicalize().unwrap()
    );
    assert_eq!(
        selected.6.iter().map(String::as_str).collect::<Vec<_>>(),
        ["root"]
    );
    assert_eq!(
        rules
            .traversal()
            .visits()
            .iter()
            .map(crate::make_rule_graph::traversal::TargetVisit::target)
            .collect::<Vec<_>>(),
        ["root"]
    );
    assert_eq!(
        rules
            .coordinates()
            .goals()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["root"]
    );
    assert!(reads.reads().is_empty());
    assert!(
        !fixture
            .working_directory
            .join("include-recipe-executed")
            .exists()
    );
    assert!(
        !fixture
            .working_directory
            .join("include-root-recipe-executed")
            .exists()
    );
}

#[test]
fn a_fresh_foreign_program_is_refused_before_the_include_recipe_consumer() {
    let mut fixture = Fixture::new();
    let target = install_include_source(&mut fixture, true);
    install_makefile(
        &fixture,
        &format!("-include {target}\n{target}: child\n\t@printf foreign\nchild:\nroot:\n"),
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &target);
    let called = Cell::new(false);

    let error = refused(rules.with_source_generated_include_recipe(
        &fixture.projection,
        (event_index, 0),
        &paths,
        |_, _, _, _| {
            called.set(true);
            Ok(())
        },
    ));

    assert!(
        error.contains("differs from original Source expansion"),
        "{error}"
    );
    assert!(!called.get());
}

#[test]
fn a_recipe_from_a_present_foreign_include_is_refused_before_the_consumer() {
    let mut fixture = Fixture::new();
    std::fs::create_dir_all(fixture.working_directory.join("outside")).unwrap();
    let target = include_target(&fixture);
    install_source_expanded_program(
        &mut fixture,
        &format!("#MM root\n-include {target}\ninclude foreign-rules.mk\nroot:\n"),
    );
    std::fs::write(
        fixture.working_directory.join("foreign-rules.mk"),
        format!(
            "{target}: foreign-child\n\t@printf foreign > foreign-recipe-executed\nforeign-child:\n"
        ),
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &target);
    let called = Cell::new(false);

    let error = refused(rules.with_source_generated_include_recipe(
        &fixture.projection,
        (event_index, 0),
        &paths,
        |_, _, _, _| {
            called.set(true);
            Ok(())
        },
    ));

    assert_eq!(
        error,
        "include producer declaration is outside its Source-expanded program"
    );
    assert!(!called.get());
    assert!(
        !fixture
            .working_directory
            .join("foreign-recipe-executed")
            .exists()
    );
}

#[test]
fn a_missing_include_declared_inside_a_foreign_program_is_refused() {
    let mut fixture = Fixture::new();
    std::fs::create_dir_all(fixture.working_directory.join("outside")).unwrap();
    let target = include_target(&fixture);
    install_source_expanded_program(
        &mut fixture,
        &format!(
            concat!(
                "#MM root\n",
                "include foreign-include.mk\n",
                "{target}: source-child\n",
                "\t@printf source-generated > nested-include-executed\n",
                "source-child:\n",
                "root:\n",
            ),
            target = target,
        ),
    );
    std::fs::write(
        fixture.working_directory.join("foreign-include.mk"),
        format!("-include {target}\n"),
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &target);

    let error = refuse_without_consumer(rules, &fixture.projection, (event_index, 0), &paths);

    assert_eq!(
        error,
        "recipe origin is outside its Source-expanded program"
    );
    assert!(
        !fixture
            .working_directory
            .join("nested-include-executed")
            .exists()
    );
}

#[test]
fn a_foreign_same_target_declaration_cannot_change_include_recipe_automatic_values() {
    let mut fixture = Fixture::new();
    std::fs::create_dir_all(fixture.working_directory.join("outside")).unwrap();
    let target = include_target(&fixture);
    install_source_expanded_program(
        &mut fixture,
        &format!(
            concat!(
                "#MM root\n",
                "-include {target}\n",
                "include foreign-prerequisite.mk\n",
                "{target}: source-prerequisite\n",
                "\t@printf '%s' '$^' > foreign-prerequisite-executed\n",
                "root:\n",
            ),
            target = target,
        ),
    );
    std::fs::write(
        fixture.working_directory.join("foreign-prerequisite.mk"),
        format!("{target}: foreign-prerequisite\n"),
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &target);
    let included = rules
        .declared_included_makefile(event_index)
        .unwrap()
        .expect("the exact missing include has a source-generated producer");
    let visits = included
        .traversal()
        .visits()
        .iter()
        .map(crate::make_rule_graph::traversal::TargetVisit::target)
        .collect::<Vec<_>>();
    assert_eq!(visits.first(), Some(&target.as_str()));
    assert_eq!(visits.len(), 3);
    assert!(visits.contains(&"source-prerequisite"));
    assert!(visits.contains(&"foreign-prerequisite"));

    let error = refuse_without_consumer(rules, &fixture.projection, (event_index, 0), &paths);

    assert_eq!(
        error,
        "include producer declaration is outside its Source-expanded program"
    );
    assert!(
        !fixture
            .working_directory
            .join("foreign-prerequisite-executed")
            .exists()
    );
}

fn callback_mutation_is_not_returned(mutate: impl FnOnce(&Fixture) -> std::io::Result<()>) {
    let mut fixture = Fixture::new();
    let target = install_include_source(&mut fixture, true);
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &target);
    let called = Cell::new(false);

    let result = rules.with_source_generated_include_recipe(
        &fixture.projection,
        (event_index, 0),
        &paths,
        |_, _, _, _| {
            called.set(true);
            mutate(&fixture).map_err(|error| error.to_string())?;
            Ok("callback result must stay private after drift")
        },
    );

    assert!(called.get());
    assert!(result.is_err(), "input drift returned the callback result");
}

#[test]
fn source_or_build_drift_inside_the_callback_discards_its_result() {
    callback_mutation_is_not_returned(|fixture| {
        std::fs::write(
            fixture.source_root.join("one/mmakefile.src"),
            "# changed Source after consumer\n",
        )
    });
    callback_mutation_is_not_returned(|fixture| {
        std::fs::write(
            fixture.working_directory.join("mmakefile"),
            "# changed generated program after consumer\n",
        )
    });
}

#[test]
fn wrong_roots_invalid_indices_and_missing_producers_are_refused_before_consumers() {
    let mut fixture = Fixture::new();
    let target = install_include_source(&mut fixture, true);
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let rules = declared_child(&fixture);
    let event_index = include_event_index(&rules, &target);

    let other_build = tempfile::tempdir().unwrap();
    let wrong_build = paths_with_roots(&fixture, &dirs, &fixture.source_root, other_build.path());
    assert!(
        refuse_without_consumer(
            declared_child(&fixture),
            &fixture.projection,
            (event_index, 0),
            &wrong_build,
        )
        .contains("build root")
    );

    let other_source = tempfile::tempdir().unwrap();
    let wrong_source = paths_with_roots(&fixture, &dirs, other_source.path(), &fixture.build_root);
    let wrong_source_error = refuse_without_consumer(
        declared_child(&fixture),
        &fixture.projection,
        (event_index, 0),
        &wrong_source,
    );
    assert!(!wrong_source_error.is_empty());

    assert!(
        refuse_without_consumer(rules, &fixture.projection, (usize::MAX, 0), &paths,)
            .contains("include event is absent")
    );
    assert!(
        refuse_without_consumer(
            declared_child(&fixture),
            &fixture.projection,
            (event_index, usize::MAX),
            &paths,
        )
        .contains("recipe is absent")
    );

    let mut no_producer_fixture = Fixture::new();
    let no_producer_target = install_include_source(&mut no_producer_fixture, false);
    let no_producer_paths = no_producer_fixture.paths(
        &dirs,
        Path::new("one"),
        &no_producer_fixture.working_directory,
    );
    let no_producer_rules = declared_child(&no_producer_fixture);
    let no_producer_event = include_event_index(&no_producer_rules, &no_producer_target);
    assert!(
        refuse_without_consumer(
            no_producer_rules,
            &no_producer_fixture.projection,
            (no_producer_event, 0),
            &no_producer_paths,
        )
        .contains("no explicit producer")
    );
}
