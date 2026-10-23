use super::*;
use crate::native_owner_projection::{
    RetainedChildSourceBatchLimits, RetainedChildSourceBatchRequest,
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

fn retained_observation(invocation: &NativeMakeInvocation) -> RetainedMakeObservation {
    let (stdout, environment_names) = pre_file_observation(invocation);
    let invocation_bytes = require(
        invocation.to_bytes(),
        "could not encode synthetic native invocation",
    );
    let binding = MakeObservationBinding {
        phase: MakeObservationPhase::PreFirstFile,
        context_sha256: sha256_bytes(b"independent reader-composition child context"),
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
        "could not retain synthetic reader-composition observation",
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
fn retained_reader_composes_trace_evaluation_and_source_origin_without_execution() {
    let mut fixture = Fixture::new();
    install_source_expanded_program(
        &mut fixture,
        "#MM root\nroot:\n\t@for o in one two; do printf '%s' \"$$o\" > reader-composition-executed; done\n",
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let standalone = declared_child(&fixture)
        .project_recipes(&paths)
        .unwrap()
        .with_source_generated_recipe(
            &fixture.source_root,
            &fixture.projection,
            0,
            |recipe, _, coordinates| {
                Ok((
                    recipe.expression().original_shell_text()?.1.to_owned(),
                    coordinates.generated_owner().to_owned(),
                ))
            },
        )
        .unwrap();

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
    let composed = fixture
        .projection
        .with_retained_child_source_reader(
            &roots,
            &request,
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |child, reader| {
                let declared = child.trace_declared_rules_in_reader(reader)?;
                let projected = declared.project_recipes_in_reader(&paths, reader)?;
                let origin_read_checkpoint = reader.read_checkpoint();
                projected.source_generated_recipe_in_reader(
                    &fixture.projection,
                    0,
                    reader,
                    |recipe, _, coordinates, reader| {
                        let origin_reads =
                            reader.provisional_reads_since(origin_read_checkpoint)?;
                        Ok((
                            recipe.expression().original_shell_text()?.1.to_owned(),
                            coordinates.generated_owner().to_owned(),
                            origin_reads.reads().len(),
                        ))
                    },
                )
            },
        )
        .unwrap();

    let selected = fixture
        .projection
        .with_retained_child_recipe_reader(
            &roots,
            (&request, 0),
            &snapshot,
            limits(1024 * 1024 * 1024),
            || Ok(()),
            |_, recipe, _, coordinates, _| {
                Ok((
                    recipe.expression().original_shell_text()?.1.to_owned(),
                    coordinates.generated_owner().to_owned(),
                ))
            },
        )
        .unwrap();
    assert_eq!(selected, standalone);
    assert_eq!((&composed.0, &composed.1), (&standalone.0, &standalone.1));
    assert_eq!(composed.2, 1, "Source-origin read was not in the suffix");
    assert_eq!(
        composed.0,
        "for o in one two; do printf '%s' \"$o\" > reader-composition-executed; done"
    );
    assert_eq!(composed.1, "one/mmakefile");
    assert!(
        !fixture
            .working_directory
            .join("reader-composition-executed")
            .exists()
    );
}

#[test]
fn wrong_include_context_refuses_trace_projection_and_source_origin_before_consumers() {
    let mut fixture = Fixture::new();
    install_source_expanded_program(&mut fixture, "#MM root\nroot:\n\t@printf inert\n");
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let wrong_context = fixture.snapshot().with_include_directories(&[]).unwrap();

    assert_refused(
        wrong_context
            .transaction(|reader| matched_child(&fixture).trace_declared_rules_in_reader(reader)),
        "reader context",
    );

    let declared = declared_child(&fixture);
    assert_refused(
        wrong_context.transaction(|reader| declared.project_recipes_in_reader(&paths, reader)),
        "reader context",
    );

    let projection = declared_child(&fixture).project_recipes(&paths).unwrap();
    let origin_consumer_called = Cell::new(false);
    assert_refused(
        wrong_context.transaction(|reader| {
            projection.source_generated_recipe_in_reader(
                &fixture.projection,
                0,
                reader,
                |_, _, _, _| {
                    origin_consumer_called.set(true);
                    Ok(())
                },
            )
        }),
        "reader context",
    );
    assert!(!origin_consumer_called.get());
}

#[cfg(unix)]
#[test]
fn wrong_include_context_refuses_recipe_input_binding_before_consumer() {
    let fixture = Fixture::new();
    let (_program_directory, programs, tool) = captured_abi_program();
    let (script, _, projection) = recipe_binding_fixture(&fixture, &tool);
    let recipe_index = declared_recipe_index_for(&projection, "root");
    let requests = [recipe_input_request(&script)];
    let wrong_context = fixture.snapshot().with_include_directories(&[]).unwrap();
    let consumer_called = Cell::new(false);

    assert_refused(
        wrong_context.transaction(|reader| {
            projection.with_recipe_input_bindings_in_reader(
                recipe_index,
                &programs,
                &requests,
                reader,
                |_, _| {
                    consumer_called.set(true);
                    Ok(())
                },
            )
        }),
        "reader context",
    );
    assert!(!consumer_called.get());
}

#[test]
fn retained_reader_withholds_callback_result_after_input_mutation() {
    let mut fixture = Fixture::new();
    install_source_expanded_program(
        &mut fixture,
        "#MM root\nroot:\n\t@printf inert > mutation-composition-executed\n",
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
    let result = fixture.projection.with_retained_child_source_reader(
        &roots,
        &request,
        &snapshot,
        limits(1024 * 1024 * 1024),
        || Ok(()),
        |child, reader| {
            let declared = child.trace_declared_rules_in_reader(reader)?;
            let projected = declared.project_recipes_in_reader(&paths, reader)?;
            projected.source_generated_recipe_in_reader(
                &fixture.projection,
                0,
                reader,
                |_, _, _, _| {
                    std::fs::write(
                        fixture.working_directory.join("mmakefile"),
                        b"root:\n\t@printf changed\n",
                    )
                    .map_err(|_| "could not mutate captured Make input during callback")?;
                    Ok("private-derived-sentinel".to_owned())
                },
            )
        },
    );

    let Err(error) = result else {
        panic!("changed Make input allowed a composed callback result to escape");
    };
    assert!(
        error.contains("changed"),
        "unexpected transaction error: {error}"
    );
    assert!(
        !error.contains("private-derived-sentinel"),
        "private callback data appeared in the refusal"
    );
    assert!(
        !fixture
            .working_directory
            .join("mutation-composition-executed")
            .exists()
    );
}

#[test]
fn retained_reader_propagates_callback_errors_and_enforces_size_and_control_limits() {
    let fixture = Fixture::new();
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
    let callback_called = Cell::new(false);

    let callback_error: Result<(), String> = fixture.projection.with_retained_child_source_reader(
        &roots,
        &request,
        &snapshot,
        limits(1024 * 1024 * 1024),
        || Ok(()),
        |_, _| {
            callback_called.set(true);
            Err("synthetic composed callback refusal".into())
        },
    );
    assert!(callback_called.get());
    assert_refused(callback_error, "synthetic composed callback refusal");

    callback_called.set(false);
    let size_refusal = fixture.projection.with_retained_child_source_reader(
        &roots,
        &request,
        &snapshot,
        limits(1),
        || Ok(()),
        |_, _| {
            callback_called.set(true);
            Ok(())
        },
    );
    assert_refused(size_refusal, "retained-data limit exceeded");
    assert!(!callback_called.get());

    let control_checks = Cell::new(0usize);
    let control_refusal = fixture.projection.with_retained_child_source_reader(
        &roots,
        &request,
        &snapshot,
        limits(1024 * 1024 * 1024),
        || {
            control_checks.set(control_checks.get() + 1);
            Err("synthetic composed control refusal".into())
        },
        |_, _| {
            callback_called.set(true);
            Ok(())
        },
    );
    assert_refused(control_refusal, "synthetic composed control refusal");
    assert!(control_checks.get() > 0);
    assert!(!callback_called.get());
}
