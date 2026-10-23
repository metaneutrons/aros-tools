use super::*;
use crate::{
    make_vars::MakeFileEventKind,
    native_owner_projection::{RetainedChildSourceBatchLimits, RetainedChildSourceBatchRequest},
};
use aros_common::{
    make_input_snapshot::MakeInputBatchLimits,
    make_native_invocation::NativeMakeInvocation,
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    sha256_bytes,
};
use std::{cell::Cell, path::Path};

fn include_target(fixture: &Fixture) -> String {
    fixture
        .working_directory
        .join("outside/generated-rules.mk")
        .to_str()
        .expect("synthetic include target is not UTF-8")
        .to_owned()
}

fn install_include_program(
    fixture: &mut Fixture,
    target: &str,
    configuration: Option<&Path>,
    genmodule_recipe: &str,
) {
    std::fs::create_dir_all(Path::new(target).parent().unwrap()).unwrap();
    let configuration_prerequisite = configuration
        .map(|path| {
            format!(
                " {}",
                path.to_str().expect("configuration path is not UTF-8")
            )
        })
        .unwrap_or_default();
    let source = format!(
        concat!(
            "#MM root\n",
            "-include {target}\n",
            "root:\n",
            "\t@printf root > included-input-root-executed\n",
            "{target}: GENMODULE | order-only-dir\n",
            "\t@printf include > included-input-include-executed\n",
            "order-only-dir:\n",
            "\t@printf order-only > included-input-order-only-executed\n",
            "GENMODULE:{configuration_prerequisite}\n",
            "\t{genmodule_recipe}\n",
        ),
        target = target,
        configuration_prerequisite = configuration_prerequisite,
        genmodule_recipe = genmodule_recipe,
    );
    install_source_expanded_program(fixture, &source);
}

fn retained_observation(invocation: &NativeMakeInvocation) -> RetainedMakeObservation {
    let (stdout, environment_names) = pre_file_observation(invocation);
    let invocation_bytes = require(
        invocation.to_bytes(),
        "could not encode synthetic retained include invocation",
    );
    let binding = MakeObservationBinding {
        phase: MakeObservationPhase::PreFirstFile,
        context_sha256: sha256_bytes(b"declared include input reader context"),
        invocation_sha256: sha256_bytes(&invocation_bytes),
        stdout_sha256: sha256_bytes(&stdout),
    };
    require(
        RetainedMakeObservation::from_captured_bytes(
            &invocation_bytes,
            &stdout,
            &environment_names,
            &binding,
        ),
        "could not retain synthetic include observation",
    )
}

fn limits(maximum_retained_bytes: usize) -> RetainedChildSourceBatchLimits {
    RetainedChildSourceBatchLimits {
        inputs: MakeInputBatchLimits {
            maximum_children: 1,
            maximum_read_bytes: 1024 * 1024,
            maximum_reads: 128,
            maximum_queries: 128,
            maximum_query_names: 4096,
            maximum_query_bytes: 1024 * 1024,
            maximum_record_bytes: 1024 * 1024,
        },
        maximum_retained_bytes,
    }
}

fn include_event_index(
    rules: &crate::native_owner_projection::MatchedChildDeclaredRules,
    target: &str,
) -> usize {
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

fn recipe_index_for(
    included: &crate::native_owner_projection::DeclaredIncludedRecipes,
    target: &str,
) -> usize {
    let matches: Vec<_> = included
        .recipes()
        .iter()
        .enumerate()
        .filter(|(_, recipe)| included.traversal().visits()[recipe.visit()].target() == target)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(matches.len(), 1, "included recipe selection is not unique");
    matches[0]
}

fn assert_refused<T>(result: Result<T, String>, expected: &str) {
    let Err(error) = result else {
        panic!("operation unexpectedly succeeded; expected refusal containing {expected:?}");
    };
    assert!(
        error.contains(expected),
        "refusal did not contain {expected:?}: {error}"
    );
}

#[test]
fn retained_include_reader_keeps_original_goals_and_projects_all_auxiliary_frames() {
    let mut fixture = Fixture::new();
    let target = include_target(&fixture);
    install_include_program(
        &mut fixture,
        &target,
        None,
        "@printf genmodule > included-input-genmodule-executed",
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let environment = synthetic_environment();
    let child_invocation = invocation(&fixture.working_directory, &environment);
    let observation = retained_observation(&child_invocation);
    let request = RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    };
    let snapshot = fixture.snapshot();
    let roots = ["root".to_owned()];
    let event_index = include_event_index(&declared_child(&fixture), &target);

    let projected = fixture
        .projection
        .with_retained_child_include_reader(
            &roots,
            (&request, event_index, None),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, _reader| {
                let original_visits = included
                    .declared()
                    .traversal()
                    .visits()
                    .iter()
                    .map(|visit| visit.target().to_owned())
                    .collect::<Vec<_>>();
                let original_goals = included.coordinates().goals().to_vec();
                let included_visits = included
                    .traversal()
                    .visits()
                    .iter()
                    .map(|visit| visit.target().to_owned())
                    .collect::<Vec<_>>();
                let recipe_targets = included
                    .recipes()
                    .iter()
                    .map(|recipe| {
                        included.traversal().visits()[recipe.visit()]
                            .target()
                            .to_owned()
                    })
                    .collect::<Vec<_>>();
                let recipe_expressions = included
                    .recipes()
                    .iter()
                    .map(|recipe| recipe.expression().expression().to_owned())
                    .collect::<Vec<_>>();
                let genmodule_index = recipe_index_for(included, "GENMODULE");
                let origin_inputs = (0..included.recipes().len())
                    .map(|index| included.origin(index).map(|origin| origin.input.clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((
                    original_visits,
                    original_goals,
                    included_visits,
                    recipe_targets,
                    recipe_expressions,
                    genmodule_index,
                    origin_inputs,
                    matches!(
                        &included.include_event().kind,
                        MakeFileEventKind::Include {
                            requested,
                            present: false,
                            resolved_identity: None,
                        } if requested == &target
                    ),
                ))
            },
        )
        .unwrap();

    assert_eq!(
        projected.0.iter().map(String::as_str).collect::<Vec<_>>(),
        ["root"]
    );
    assert_eq!(
        projected.1.iter().map(String::as_str).collect::<Vec<_>>(),
        ["root"]
    );
    assert_eq!(
        projected.2.first().map(String::as_str),
        Some(target.as_str())
    );
    assert!(projected.2.contains(&"GENMODULE".to_owned()));
    assert!(projected.2.contains(&"order-only-dir".to_owned()));
    assert_eq!(
        projected.3.len(),
        3,
        "not all auxiliary recipes were evaluated"
    );
    assert!(projected.3.iter().any(|target| target == "GENMODULE"));
    assert!(projected.3.iter().any(|target| target == "order-only-dir"));
    assert_eq!(projected.4.len(), 3);
    assert!(projected
        .4
        .iter()
        .any(|expression| { expression.contains("included-input-order-only-executed") }));
    assert_eq!(projected.3[projected.5], "GENMODULE");
    assert_eq!(
        projected.6.iter().map(String::as_str).collect::<Vec<_>>(),
        ["mmakefile", "mmakefile", "mmakefile"]
    );
    assert!(
        projected.7,
        "the exact missing include event was not retained"
    );
    for marker in [
        "included-input-root-executed",
        "included-input-include-executed",
        "included-input-order-only-executed",
        "included-input-genmodule-executed",
    ] {
        assert!(!fixture.working_directory.join(marker).exists());
    }
}

#[cfg(unix)]
fn captured_genmodule_program() -> (
    tempfile::TempDir,
    aros_common::captured_programs::CapturedProgramSet,
    std::path::PathBuf,
) {
    captured_genmodule_program_for_role("source-genmodule")
}

#[cfg(unix)]
fn captured_genmodule_program_for_role(
    role: &str,
) -> (
    tempfile::TempDir,
    aros_common::captured_programs::CapturedProgramSet,
    std::path::PathBuf,
) {
    use aros_common::{
        captured_programs::{ProgramRootSelection, ProgramSelection},
        measure_tree_content_cas,
    };
    use std::os::unix::fs::PermissionsExt;

    const TOOL_BYTES: &[u8] = b"synthetic source genmodule program; never executed";
    let directory = tempfile::tempdir().expect("could not create selected program root");
    let root = directory
        .path()
        .canonicalize()
        .expect("could not resolve selected program root");
    let tool = root.join("source-genmodule");
    std::fs::write(&tool, TOOL_BYTES).expect("could not write selected program bytes");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
        .expect("could not mark selected program executable");
    let programs = aros_common::captured_programs::CapturedProgramSet::capture(
        &[ProgramRootSelection {
            content_sha256: measure_tree_content_cas(&root)
                .expect("could not measure selected program root")
                .payload_digest_excluding(None),
            root,
        }],
        &[ProgramSelection {
            role: role.into(),
            root_index: 0,
            invocation_path: tool.clone(),
            executable_sha256: sha256_bytes(TOOL_BYTES),
        }],
    )
    .expect("could not capture selected genmodule program");
    (directory, programs, tool)
}

#[cfg(unix)]
fn install_exact_genmodule_include_program(
    fixture: &mut Fixture,
    paths: (&Path, &Path, &Path, &Path),
    source_prefix: &str,
    extra_prerequisites: &str,
    directory_command: &str,
    extra_rules: &str,
) {
    let (target, program_word, configuration, directory) = paths;
    let source = format!(
        concat!(
            "#MM root\n",
            "{source_prefix}",
            "-include {target}\n",
            "root:\n",
            "{target}: {program_word} {configuration}{extra_prerequisites} | {directory}\n",
            "\t@'{program_word}' -c '{configuration}' -d '{directory}' writemakefile module library\n",
            "{directory}:\n",
            "\t@{directory_command}\n",
            "{extra_rules}"
        ),
        target = target.to_str().expect("include target is not UTF-8"),
        source_prefix = source_prefix,
        extra_prerequisites = extra_prerequisites,
        directory_command = directory_command,
        extra_rules = extra_rules,
        program_word = program_word
            .to_str()
            .expect("GENMODULE program is not UTF-8"),
        configuration = configuration
            .to_str()
            .expect("GENMODULE configuration is not UTF-8"),
        directory = directory
            .to_str()
            .expect("GENMODULE directory is not UTF-8"),
    );
    install_source_expanded_program(fixture, &source);
}

#[cfg(unix)]
fn input_request(path: &Path) -> crate::make_shell_data::artifact_binding::RecipeArtifactRequest {
    use crate::make_shell_data::artifact_binding::{
        RecipeArtifactDirection, RecipeArtifactRequest, RecipeArtifactSlot,
    };

    let word = path
        .to_str()
        .expect("synthetic configuration path is not UTF-8");
    RecipeArtifactRequest {
        recipe: 0,
        command: 0,
        slot: RecipeArtifactSlot::Word {
            index: 2,
            range: 0..word.len(),
        },
        direction: RecipeArtifactDirection::Input,
    }
}

#[cfg(unix)]
#[test]
fn retained_include_reader_binds_exact_source_configuration_and_selected_program() {
    use crate::make_shell_data::artifact_binding::RecipeArtifactDirection;

    let mut fixture = Fixture::new();
    let config_bytes = b"Mesa configuration input\nabi=retained\n";
    let config = fixture.source_root.join("config/mesa.ini");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, config_bytes).unwrap();
    let (_program_directory, programs, tool) = captured_genmodule_program();
    let target = include_target(&fixture);
    let recipe = format!(
        "@'{}' --config '{}'",
        tool.to_str().expect("selected program path is not UTF-8"),
        config.to_str().expect("configuration path is not UTF-8")
    );
    install_include_program(&mut fixture, &target, Some(&config), &recipe);
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let environment = synthetic_environment();
    let child_invocation = invocation(&fixture.working_directory, &environment);
    let observation = retained_observation(&child_invocation);
    let request = RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    };
    let snapshot = fixture.snapshot();
    let roots = ["root".to_owned()];
    let event_index = include_event_index(&declared_child(&fixture), &target);
    let artifact_request = input_request(&config);

    let bound = fixture
        .projection
        .with_retained_child_include_reader(
            &roots,
            (&request, event_index, None),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, reader| {
                let recipe_index = recipe_index_for(included, "GENMODULE");
                let checkpoint = reader.read_checkpoint();
                let binding = included.with_recipe_input_bindings_in_reader(
                    recipe_index,
                    &programs,
                    std::slice::from_ref(&artifact_request),
                    reader,
                    |bindings, artifacts| {
                        let selected = bindings.unique_role("source-genmodule")?;
                        let program = selected
                            .program()
                            .ok_or("selected genmodule role was unresolved")?;
                        let artifact = artifacts
                            .first()
                            .ok_or("configuration artifact binding is absent")?;
                        Ok((
                            program.role().to_owned(),
                            program.invocation_path().to_owned(),
                            selected.command().words().to_vec(),
                            artifact.path().to_owned(),
                            artifact.input_sha256().cloned(),
                            artifact.request().direction,
                        ))
                    },
                )?;
                let suffix = reader.provisional_reads_since(checkpoint)?;
                let configuration_reads = suffix
                    .reads()
                    .iter()
                    .filter(|read| read.path == config)
                    .map(|read| (read.requested.clone(), read.content_sha256.clone()))
                    .collect::<Vec<_>>();
                Ok((binding, configuration_reads))
            },
        )
        .unwrap();

    assert_eq!(bound.0 .0, "source-genmodule");
    assert_eq!(bound.0 .1, tool);
    assert_eq!(bound.0 .2[1], "--config");
    assert_eq!(bound.0 .2[2], config.to_str().unwrap());
    assert_eq!(bound.0 .3, config);
    assert_eq!(bound.0 .4, Some(sha256_bytes(config_bytes)));
    assert_eq!(bound.0 .5, RecipeArtifactDirection::Input);
    assert_eq!(
        bound.1.len(),
        1,
        "configuration was not read in this reader"
    );
    assert_eq!(bound.1[0].0, config.to_str().unwrap());
    assert_eq!(bound.1[0].1, Some(sha256_bytes(config_bytes)));
}

#[cfg(unix)]
#[test]
fn retained_include_input_binding_rejects_output_coordinates_and_invalid_slots() {
    use crate::make_shell_data::artifact_binding::{RecipeArtifactDirection, RecipeArtifactSlot};

    let mut fixture = Fixture::new();
    let config_bytes = b"configuration for invalid binding checks\n";
    let config = fixture.source_root.join("config/mesa.ini");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, config_bytes).unwrap();
    let (_program_directory, programs, tool) = captured_genmodule_program();
    let target = include_target(&fixture);
    let recipe = format!(
        "@'{}' --config '{}'",
        tool.to_str().expect("selected program path is not UTF-8"),
        config.to_str().expect("configuration path is not UTF-8")
    );
    install_include_program(&mut fixture, &target, Some(&config), &recipe);
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let environment = synthetic_environment();
    let child_invocation = invocation(&fixture.working_directory, &environment);
    let observation = retained_observation(&child_invocation);
    let request = RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    };
    let snapshot = fixture.snapshot();
    let roots = ["root".to_owned()];
    let event_index = include_event_index(&declared_child(&fixture), &target);
    let valid = input_request(&config);

    fixture
        .projection
        .with_retained_child_include_reader(
            &roots,
            (&request, event_index, None),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, reader| {
                let recipe_index = recipe_index_for(included, "GENMODULE");
                let mut output = valid.clone();
                output.direction = RecipeArtifactDirection::Output;
                let mut wrong_coordinate = valid.clone();
                wrong_coordinate.recipe = 1;
                let mut invalid_slot = valid.clone();
                invalid_slot.slot = RecipeArtifactSlot::Word {
                    index: usize::MAX,
                    range: 0..1,
                };

                for (bad_request, expected) in [
                    (output, "input fields only"),
                    (wrong_coordinate, "batch coordinate is invalid"),
                    (invalid_slot, "word is absent"),
                ] {
                    let consumer_called = Cell::new(false);
                    let refused = included.with_recipe_input_bindings_in_reader(
                        recipe_index,
                        &programs,
                        &[bad_request],
                        reader,
                        |_, _| {
                            consumer_called.set(true);
                            Ok(())
                        },
                    );
                    assert!(!consumer_called.get());
                    assert_refused(refused, expected);
                }
                Ok(())
            },
        )
        .unwrap();
    assert!(!fixture
        .working_directory
        .join("included-input-genmodule-executed")
        .exists());
}

#[cfg(unix)]
#[test]
fn included_recipe_binding_rejects_a_different_reader_context_before_consumer() {
    let mut fixture = Fixture::new();
    let config = fixture.source_root.join("config/mesa.ini");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, b"reader context input\n").unwrap();
    let (_program_directory, programs, tool) = captured_genmodule_program();
    let target = include_target(&fixture);
    let recipe = format!(
        "@'{}' --config '{}'",
        tool.to_str().expect("selected program path is not UTF-8"),
        config.to_str().expect("configuration path is not UTF-8")
    );
    install_include_program(&mut fixture, &target, Some(&config), &recipe);
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let environment = synthetic_environment();
    let child_invocation = invocation(&fixture.working_directory, &environment);
    let observation = retained_observation(&child_invocation);
    let request = RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    };
    let snapshot = fixture.snapshot();
    let roots = ["root".to_owned()];
    let event_index = include_event_index(&declared_child(&fixture), &target);
    let artifact_request = input_request(&config);
    let wrong_context = fixture.snapshot().with_include_directories(&[]).unwrap();

    fixture
        .projection
        .with_retained_child_include_reader(
            &roots,
            (&request, event_index, None),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, _reader| {
                let recipe_index = recipe_index_for(included, "GENMODULE");
                let consumer_called = Cell::new(false);
                let refused = wrong_context.transaction(|reader| {
                    included.with_recipe_input_bindings_in_reader(
                        recipe_index,
                        &programs,
                        std::slice::from_ref(&artifact_request),
                        reader,
                        |_, _| {
                            consumer_called.set(true);
                            Ok(())
                        },
                    )
                });
                assert!(!consumer_called.get());
                assert_refused(refused, "reader context");
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn retained_include_reader_refuses_foreign_header_before_consumer() {
    let mut fixture = Fixture::new();
    let target = include_target(&fixture);
    std::fs::create_dir_all(Path::new(&target).parent().unwrap()).unwrap();
    install_source_expanded_program(
        &mut fixture,
        &format!(
            concat!(
                "#MM root\n",
                "-include {target}\n",
                "include foreign-include.mk\n",
                "root:\n",
                "\t@printf root > included-input-root-executed\n",
            ),
            target = target,
        ),
    );
    std::fs::write(
        fixture.working_directory.join("foreign-include.mk"),
        format!(
            "{target}: foreign-input\n\t@printf foreign > included-input-foreign-executed\nforeign-input:\n"
        ),
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let environment = synthetic_environment();
    let child_invocation = invocation(&fixture.working_directory, &environment);
    let observation = retained_observation(&child_invocation);
    let request = RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    };
    let snapshot = fixture.snapshot();
    let roots = ["root".to_owned()];
    let event_index = include_event_index(&declared_child(&fixture), &target);
    let consumer_called = Cell::new(false);

    let error = fixture.projection.with_retained_child_include_reader(
        &roots,
        (&request, event_index, None),
        &snapshot,
        limits(1024 * 1024 * 1024),
        || Ok(()),
        |_, _| {
            consumer_called.set(true);
            Ok(())
        },
    );

    assert_refused(error, "include producer declaration is outside");
    assert!(!consumer_called.get());
    assert!(!fixture
        .working_directory
        .join("included-input-foreign-executed")
        .exists());
}

#[test]
fn retained_include_reader_discards_callback_result_after_generated_program_drift() {
    let mut fixture = Fixture::new();
    let target = include_target(&fixture);
    install_include_program(
        &mut fixture,
        &target,
        None,
        "@printf genmodule > included-input-genmodule-executed",
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let environment = synthetic_environment();
    let child_invocation = invocation(&fixture.working_directory, &environment);
    let observation = retained_observation(&child_invocation);
    let request = RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    };
    let snapshot = fixture.snapshot();
    let roots = ["root".to_owned()];
    let event_index = include_event_index(&declared_child(&fixture), &target);
    let consumer_called = Cell::new(false);

    let result = fixture.projection.with_retained_child_include_reader(
        &roots,
        (&request, event_index, None),
        &snapshot,
        limits(1024 * 1024 * 1024),
        || Ok(()),
        |included, _reader| {
            assert_eq!(included.recipes().len(), 3);
            consumer_called.set(true);
            std::fs::write(
                fixture.working_directory.join("mmakefile"),
                b"# generated Source program drifted in callback\n",
            )
            .map_err(|_| "could not mutate generated Makefile during callback".to_owned())?;
            Ok("private-include-callback-sentinel".to_owned())
        },
    );

    assert!(consumer_called.get());
    let Err(error) = result else {
        panic!("generated-program drift allowed a composed callback result to escape");
    };
    assert!(
        error.contains("changed"),
        "unexpected transaction error: {error}"
    );
    assert!(!error.contains("private-include-callback-sentinel"));
    assert!(!fixture
        .working_directory
        .join("included-input-genmodule-executed")
        .exists());
    assert!(!fixture
        .working_directory
        .join("included-input-order-only-executed")
        .exists());
}

#[test]
fn included_prerequisite_requires_independently_held_source_program() {
    for mode in 0..3 {
        let held = mode != 0;
        let mut fixture = Fixture::new();
        let target = include_target(&fixture);
        let helper = fixture.source_root.join("directory-rules.mk");
        let helper_bytes = b"order-only-dir:\n\t@mkdir -p order-only-dir\n";
        std::fs::write(&helper, helper_bytes).unwrap();
        install_source_expanded_program(
            &mut fixture,
            &format!(
                "#MM root\ninclude {}\n-include {target}\nroot:\n{target}: | order-only-dir\n\t@printf include\n",
                helper.display(),
            ),
        );
        if held {
            fixture.projection.snapshots.insert(
                "directory-rules.mk".into(),
                sha256_bytes(helper_bytes).to_string(),
            );
        }
        let dirs = DirVars::default();
        let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
        let environment = synthetic_environment();
        let child_invocation = invocation(&fixture.working_directory, &environment);
        let observation = retained_observation(&child_invocation);
        let request = RetainedChildSourceBatchRequest {
            paths: &paths,
            generated_owner: "one/mmakefile",
            observation: &observation,
        };
        let snapshot = fixture.snapshot();
        let event_index = include_event_index(&declared_child(&fixture), &target);
        let called = Cell::new(false);
        let result = fixture.projection.with_retained_child_include_reader(
            &["root".into()],
            (&request, event_index, None),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, _| {
                called.set(true);
                let index = recipe_index_for(included, "order-only-dir");
                assert_eq!(included.origin(index)?.input, helper.to_str().unwrap());
                if mode == 2 {
                    std::fs::write(&helper, b"order-only-dir:\n\t@printf drift\n")
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            },
        );
        if mode == 2 {
            assert!(called.get());
            assert!(result.is_err(), "changed Source helper escaped postchecks");
        } else if held {
            result.unwrap();
            assert!(called.get());
        } else {
            assert_refused(result, "include producer declaration is outside");
            assert!(!called.get());
        }
        assert!(!fixture.working_directory.join("order-only-dir").exists());
    }
}

#[test]
fn variable_only_include_cannot_supply_unheld_recipe_values() {
    for mode in 0..3 {
        let held = mode == 1;
        let mut fixture = Fixture::new();
        let target = include_target(&fixture);
        let helper = fixture.source_root.join("variable-only.mk");
        let bytes = b"INCLUDE_VALUE := selected-value\n";
        std::fs::write(&helper, bytes).unwrap();
        install_source_expanded_program(&mut fixture, &format!(
            "#MM root\ninclude {}\n-include {target}\nroot:\n{target}:\n\t@printf '%s' '$(INCLUDE_VALUE)'\n",
            helper.display(),
        ));
        if held {
            fixture
                .projection
                .snapshots
                .insert("variable-only.mk".into(), sha256_bytes(bytes).to_string());
        }
        let dirs = DirVars::default();
        let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
        let observation = retained_observation(&invocation(
            &fixture.working_directory,
            &synthetic_environment(),
        ));
        let request = RetainedChildSourceBatchRequest {
            paths: &paths,
            generated_owner: "one/mmakefile",
            observation: &observation,
        };
        let snapshot = fixture.snapshot();
        let event = include_event_index(&declared_child(&fixture), &target);
        let called = Cell::new(false);
        // Even supplying a captured Source tree in the output-inventory slot
        // cannot promote an unselected Source helper into policy membership.
        let source_tree = aros_common::measure_tree_content_cas(&fixture.source_root).unwrap();
        let original_output = (mode == 2).then_some((fixture.source_root.as_path(), &source_tree));
        let result = fixture.projection.with_retained_child_include_reader(
            &["root".into()],
            (&request, event, original_output),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, _| {
                called.set(true);
                assert!(included.recipes()[0]
                    .expression()
                    .expression()
                    .contains("selected-value"));
                Ok(())
            },
        );
        if held {
            result.unwrap();
            assert!(called.get());
        } else {
            assert_refused(result, "included program is outside");
            assert!(!called.get());
        }
    }
}

#[test]
fn generated_include_requires_original_per_file_inventory_not_a_fresh_reader() {
    for mode in ["absent", "original", "changed", "unrelated", "wrong-root"] {
        let mut fixture = Fixture::new();
        let target = include_target(&fixture);
        let helper = fixture.build_root.join("settings.mk");
        std::fs::write(&helper, b"GENERATED_VALUE := original-value\n").unwrap();
        install_source_expanded_program(&mut fixture, &format!(
            "#MM root\ninclude {}\n-include {target}\nroot:\n{target}:\n\t@printf '%s' '$(GENERATED_VALUE)'\n", helper.display(),
        ));
        // Held before subsequent effects. A new snapshot below does not choose
        // expected bytes for the optional original-output comparison.
        let original = aros_common::measure_tree_content_cas(&fixture.build_root).unwrap();
        if mode == "changed" {
            std::fs::write(&helper, b"GENERATED_VALUE := foreign-value\n").unwrap();
        } else if mode == "unrelated" {
            std::fs::write(fixture.build_root.join("other-output"), b"later effect").unwrap();
        }
        let dirs = DirVars::default();
        let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
        let observation = retained_observation(&invocation(
            &fixture.working_directory,
            &synthetic_environment(),
        ));
        let request = RetainedChildSourceBatchRequest {
            paths: &paths,
            generated_owner: "one/mmakefile",
            observation: &observation,
        };
        let snapshot = fixture.snapshot();
        let event = include_event_index(&declared_child(&fixture), &target);
        let root = if mode == "wrong-root" {
            &fixture.working_directory
        } else {
            &fixture.build_root
        };
        let expected = (mode != "absent").then_some((root.as_path(), &original));
        let called = Cell::new(false);
        let result = fixture.projection.with_retained_child_include_reader(
            &["root".into()],
            (&request, event, expected),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, _| {
                called.set(true);
                assert!(included.recipes()[0]
                    .expression()
                    .expression()
                    .contains("original-value"));
                Ok(())
            },
        );
        if mode == "original" || mode == "unrelated" {
            result.unwrap();
            assert!(called.get());
        } else {
            assert_refused(result, "included program is outside");
            assert!(!called.get());
        }
    }
}

#[cfg(unix)]
#[test]
fn retained_genmodule_include_reader_prepares_exact_writemakefile_tuple() {
    let mut fixture = Fixture::new();
    let config_bytes = b"configuration for exact native GENMODULE tuple\n";
    let configuration = fixture.source_root.join("config/mesa.ini");
    std::fs::create_dir_all(configuration.parent().unwrap()).unwrap();
    std::fs::write(&configuration, config_bytes).unwrap();
    let (_program_directory, programs, program) = captured_genmodule_program();
    let directory = fixture.working_directory.join("outside");
    let target = directory.join("Makefile.modulelibrary");
    install_exact_genmodule_include_program(
        &mut fixture,
        (&target, &program, &configuration, &directory),
        "",
        "",
        &format!("mkdir -p {}", directory.display()),
        "",
    );

    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let environment = synthetic_environment();
    let child_invocation = invocation(&fixture.working_directory, &environment);
    let observation = retained_observation(&child_invocation);
    let request = RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    };
    let snapshot = fixture.snapshot();
    let roots = ["root".to_owned()];
    let target_text = target.to_str().unwrap();
    let directory_text = directory.to_str().unwrap();
    let configuration_text = configuration.to_str().unwrap();
    let event_index = include_event_index(&declared_child(&fixture), target_text);

    let prepared = fixture
        .projection
        .with_retained_child_include_reader(
            &roots,
            (&request, event_index, None),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, reader| {
                let plan = included.prepare_genmodule_include_in_reader(
                    &fixture.source_root,
                    &programs,
                    reader,
                )?;
                included.validate_genmodule_include_readiness_in_reader(&plan, &program, reader)?;
                let generator_recipe_target = included.traversal().visits()
                    [included.recipes()[plan.generator_recipe()].visit()]
                .target()
                .to_owned();
                let directory_recipe_target = included.traversal().visits()
                    [included.recipes()[plan.directory_recipe()].visit()]
                .target()
                .to_owned();
                Ok((
                    plan.include_path().to_owned(),
                    plan.directory().to_owned(),
                    plan.arguments().to_vec(),
                    plan.configurations()
                        .iter()
                        .map(|(path, digest)| (path.clone(), digest.clone()))
                        .collect::<Vec<_>>(),
                    generator_recipe_target,
                    directory_recipe_target,
                ))
            },
        )
        .unwrap();

    assert_eq!(prepared.0, target);
    assert_eq!(prepared.1, directory);
    assert_eq!(
        prepared.2,
        vec![
            program.to_str().unwrap(),
            "-c",
            configuration_text,
            "-d",
            directory_text,
            "writemakefile",
            "module",
            "library",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    );
    assert_eq!(
        prepared.3,
        vec![(configuration, sha256_bytes(config_bytes))]
    );
    assert_eq!(prepared.4, target_text.to_owned());
    assert_eq!(prepared.5, directory_text.to_owned());
}

#[cfg(unix)]
#[test]
fn retained_genmodule_include_reader_refuses_nonexact_selected_inputs_and_recipes() {
    for mode in [
        "phony-include",
        "phony-configuration",
        "phony-directory",
        "extra-prerequisite",
        "missing-role",
        "foreign-command",
        "changed-directory-command",
        "configuration-producer",
        "implicit-configuration-producer",
        "default-configuration-producer",
        "suffix-configuration-producer",
        "single-suffix-configuration-producer",
        "path-implicit-configuration-producer",
    ] {
        let mut fixture = Fixture::new();
        let config = fixture.source_root.join("config/mesa.ini");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, b"configuration for refused include\n").unwrap();
        let role = if mode == "missing-role" {
            "other-genmodule"
        } else {
            "source-genmodule"
        };
        let (_program_directory, programs, program) = captured_genmodule_program_for_role(role);
        let directory = fixture.working_directory.join("outside");
        let target = directory.join("Makefile.modulelibrary");
        let foreign_program = fixture.build_root.join("foreign-genmodule");
        let program_word = if mode == "foreign-command" {
            foreign_program.as_path()
        } else {
            program.as_path()
        };
        let extra_configuration = fixture.source_root.join("config/extra.ini");
        if mode == "extra-prerequisite" {
            std::fs::write(&extra_configuration, b"extra prerequisite\n").unwrap();
        }
        let extra_prerequisites = if mode == "extra-prerequisite" {
            format!(" {}", extra_configuration.display())
        } else {
            String::new()
        };
        let source_prefix = match mode {
            "phony-include" => format!(".PHONY: {}\n", target.display()),
            "phony-configuration" => format!(".PHONY: {}\n", config.display()),
            "phony-directory" => format!(".PHONY: {}\n", directory.display()),
            "configuration-producer" => format!("{}:\n\t@printf unfinished\n", config.display()),
            "implicit-configuration-producer" => "%.ini:\n\t@printf unfinished\n".into(),
            "default-configuration-producer" => ".DEFAULT:\n\t@printf unfinished\n".into(),
            "suffix-configuration-producer" => {
                ".SUFFIXES: .src .ini\n.src.ini:\n\t@printf unfinished\n".into()
            }
            "single-suffix-configuration-producer" => {
                ".SUFFIXES: .c\n.c:\n\t@printf unfinished\n".into()
            }
            "path-implicit-configuration-producer" => format!(
                "{}/%/mesa.ini:\n\t@printf unfinished\n",
                fixture.source_root.display()
            ),
            _ => String::new(),
        };
        let directory_command = if mode == "changed-directory-command" {
            format!("mkdir -p {}/other", directory.display())
        } else {
            format!("mkdir -p {}", directory.display())
        };
        install_exact_genmodule_include_program(
            &mut fixture,
            (&target, program_word, &config, &directory),
            &source_prefix,
            &extra_prerequisites,
            &directory_command,
            "",
        );

        let dirs = DirVars::default();
        let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
        let environment = synthetic_environment();
        let child_invocation = invocation(&fixture.working_directory, &environment);
        let observation = retained_observation(&child_invocation);
        let request = RetainedChildSourceBatchRequest {
            paths: &paths,
            generated_owner: "one/mmakefile",
            observation: &observation,
        };
        let snapshot = fixture.snapshot();
        let event_index = include_event_index(&declared_child(&fixture), target.to_str().unwrap());
        let result = fixture.projection.with_retained_child_include_reader(
            &["root".into()],
            (&request, event_index, None),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, reader| {
                let plan = included.prepare_genmodule_include_in_reader(
                    &fixture.source_root,
                    &programs,
                    reader,
                )?;
                included.validate_genmodule_include_readiness_in_reader(&plan, &program, reader)
            },
        );
        let expected = match mode {
            "phony-include" | "phony-configuration" | "phony-directory" => {
                "native GENMODULE selected target or prerequisite is phony"
            }
            "extra-prerequisite" => {
                "native GENMODULE include prerequisites differ from bound inputs"
            }
            "missing-role" | "foreign-command" => "selected recipe program role is absent",
            "changed-directory-command" => {
                "native GENMODULE directory command differs from mkdir -p parent"
            }
            "configuration-producer" => "GENMODULE configuration has an explicit Source producer",
            "implicit-configuration-producer" => "has an applicable implicit producer",
            "default-configuration-producer" => {
                "GENMODULE readiness cannot exclude an active .DEFAULT producer"
            }
            "suffix-configuration-producer" | "single-suffix-configuration-producer" => {
                "has an applicable suffix producer"
            }
            "path-implicit-configuration-producer" => "may match an unsupported implicit pattern",
            _ => unreachable!(),
        };
        assert_refused(result, expected);
    }
}

#[cfg(unix)]
#[test]
fn after_fresh_genmodule_directory_reuses_held_state_for_deferred_recipes() {
    let mut fixture = Fixture::new();
    let config_bytes = b"configuration for held state\n";
    let configuration = fixture.source_root.join("config/mesa.ini");
    std::fs::create_dir_all(configuration.parent().unwrap()).unwrap();
    std::fs::write(&configuration, config_bytes).unwrap();
    let (_program_directory, programs, program) = captured_genmodule_program();
    let directory = fixture.working_directory.join("outside");
    let target = directory.join("Makefile.modulelibrary");
    let source_prefix = format!(
        "IMMEDIATE_DIRECTORY := $(wildcard {directory})\nDEFERRED_DIRECTORY = $(wildcard {directory})\n",
        directory = directory.display(),
    );
    let deferred_probe = format!(
        "{}:\n\t@printf '%s' '$(IMMEDIATE_DIRECTORY)|$(DEFERRED_DIRECTORY)'\n",
        configuration.display(),
    );
    install_exact_genmodule_include_program(
        &mut fixture,
        (&target, &program, &configuration, &directory),
        &source_prefix,
        "",
        &format!("mkdir -p {}", directory.display()),
        &deferred_probe,
    );

    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let environment = synthetic_environment();
    let child_invocation = invocation(&fixture.working_directory, &environment);
    let observation = retained_observation(&child_invocation);
    let request = RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    };
    let snapshot = fixture.snapshot();
    let event_index = include_event_index(&declared_child(&fixture), target.to_str().unwrap());
    let configuration_text = configuration.to_str().unwrap().to_owned();
    let directory_text = directory.to_str().unwrap().to_owned();
    let (included, before_expression) = fixture
        .projection
        .with_retained_owned_child_include_reader(
            &["root".into()],
            (&request, event_index, None),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |included, reader| {
                included.prepare_genmodule_include_in_reader(
                    &fixture.source_root,
                    &programs,
                    reader,
                )?;
                let index = recipe_index_for(&included, &configuration_text);
                let before = included.recipes()[index]
                    .expression()
                    .expression()
                    .to_owned();
                Ok((included, before))
            },
        )
        .unwrap();
    assert!(!before_expression.contains(&directory_text));

    std::fs::create_dir(&directory).unwrap();
    let after_snapshot = fixture.snapshot();
    let (after_expression, _) = after_snapshot
        .transaction(|reader| {
            let included = included.after_fresh_directory_in_reader(&paths, &directory, reader)?;
            let index = recipe_index_for(&included, &configuration_text);
            Ok(included.recipes()[index]
                .expression()
                .expression()
                .to_owned())
        })
        .unwrap();
    assert!(after_expression.contains(&format!("|{directory_text}")));
    assert!(!after_expression.contains(&format!("{directory_text}|")));
    assert_eq!(after_expression.matches(&directory_text).count(), 1);
}

#[cfg(unix)]
#[test]
fn after_fresh_genmodule_directory_refuses_extra_output_and_changed_existing_file() {
    for mode in ["extra-output", "changed-existing-file"] {
        let mut fixture = Fixture::new();
        let config = fixture.source_root.join("config/mesa.ini");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, b"configuration for output transition\n").unwrap();
        let (_program_directory, programs, program) = captured_genmodule_program();
        let directory = fixture.working_directory.join("outside");
        let target = directory.join("Makefile.modulelibrary");
        install_exact_genmodule_include_program(
            &mut fixture,
            (&target, &program, &config, &directory),
            "",
            "",
            &format!("mkdir -p {}", directory.display()),
            "",
        );

        let dirs = DirVars::default();
        let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
        let environment = synthetic_environment();
        let child_invocation = invocation(&fixture.working_directory, &environment);
        let observation = retained_observation(&child_invocation);
        let request = RetainedChildSourceBatchRequest {
            paths: &paths,
            generated_owner: "one/mmakefile",
            observation: &observation,
        };
        let snapshot = fixture.snapshot();
        let event_index = include_event_index(&declared_child(&fixture), target.to_str().unwrap());
        let included = fixture
            .projection
            .with_retained_owned_child_include_reader(
                &["root".into()],
                (&request, event_index, None),
                &snapshot,
                limits(1024 * 1024 * 1024),
                || Ok(()),
                |included, reader| {
                    included.prepare_genmodule_include_in_reader(
                        &fixture.source_root,
                        &programs,
                        reader,
                    )?;
                    Ok(included)
                },
            )
            .unwrap();

        std::fs::create_dir(&directory).unwrap();
        if mode == "extra-output" {
            std::fs::write(
                fixture.working_directory.join("unexpected-output"),
                b"extra",
            )
            .unwrap();
        } else {
            std::fs::write(
                fixture.working_directory.join("mmakefile"),
                b"# changed existing generated Makefile\n",
            )
            .unwrap();
        }
        let after_snapshot = fixture.snapshot();
        let refused = after_snapshot.transaction(|reader| {
            included.after_fresh_directory_in_reader(&paths, &directory, reader)
        });
        assert_refused(refused, "outside selected effects");
    }
}
