use super::*;
use crate::native_owner_projection::{
    RetainedChildSourceBatchLimits, RetainedChildSourceBatchRequest,
};
use aros_common::{
    make_input_snapshot::MakeInputBatchLimits,
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
        context_sha256: sha256_bytes(b"independent batch child context"),
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
        "could not retain synthetic native observation",
    )
}

fn limits(children: usize, maximum_retained_bytes: usize) -> RetainedChildSourceBatchLimits {
    RetainedChildSourceBatchLimits {
        inputs: MakeInputBatchLimits {
            maximum_children: children,
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

fn add_second_build_owner(fixture: &Fixture) {
    let second = fixture.build_root.join("two");
    require(
        std::fs::create_dir_all(&second),
        "could not create second synthetic build owner",
    );
    require(
        std::fs::write(second.join("mmakefile"), SOURCE_INPUT),
        "could not write second synthetic generated Makefile",
    );
    require(
        std::fs::write(second.join("extra.mk"), "INCLUDED := second-include\n"),
        "could not write second synthetic include",
    );
}

#[test]
fn retained_child_source_batch_parses_each_child_in_one_root_transaction() {
    let fixture = Fixture::new();
    add_second_build_owner(&fixture);
    let environment = synthetic_environment();
    let first_invocation = invocation(&fixture.working_directory, &environment);
    let second_cwd = fixture.build_root.join("two");
    let second_invocation = invocation(&second_cwd, &environment);
    let first_observation = retained_observation(&first_invocation);
    let second_observation = retained_observation(&second_invocation);
    let dirs = DirVars::default();
    let first_paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let second_paths = fixture.paths(&dirs, Path::new("two"), &second_cwd);
    let requests = [
        RetainedChildSourceBatchRequest {
            paths: &first_paths,
            generated_owner: "one/mmakefile",
            observation: &first_observation,
        },
        RetainedChildSourceBatchRequest {
            paths: &second_paths,
            generated_owner: "two/mmakefile",
            observation: &second_observation,
        },
    ];
    let control_checks = Cell::new(0usize);
    let matched = require(
        fixture.projection.read_retained_child_sources(
            &["root".into()],
            &requests,
            &fixture.snapshot(),
            limits(2, 1024 * 1024 * 1024),
            || {
                control_checks.set(control_checks.get() + 1);
                Ok(())
            },
        ),
        "retained child source batch could not be parsed",
    );
    // Batch boundaries plus each child's before/setup/after control checks.
    assert_eq!(control_checks.get(), 8);
    assert_eq!(matched.len(), 2);
    let retained_sizes = matched
        .iter()
        .map(|child| {
            child
                .retained_estimate_bytes()
                .expect("batch retained estimate")
        })
        .collect::<Vec<_>>();
    let second_variable_bytes = matched[1].program.retained_storage_bytes();
    let second_file_bytes = retained_sizes[1]
        .checked_sub(second_variable_bytes)
        .unwrap();
    assert!(retained_sizes.iter().all(|size| *size > 0));
    let mut matched = matched.into_iter();
    for (child, owner) in matched.by_ref().zip(["one/mmakefile", "two/mmakefile"]) {
        let (coordinates, _program, file, reads) = child.into_parts();
        assert_eq!(coordinates.generated_owner(), owner);
        assert!(!file.events.is_empty());
        assert_eq!(
            reads
                .reads()
                .iter()
                .map(|read| read.requested.as_str())
                .collect::<Vec<_>>(),
            ["mmakefile", "extra.mk"]
        );
    }

    let individual_cap = *retained_sizes.iter().max().unwrap();
    for index in 0..requests.len() {
        assert!(
            fixture
                .projection
                .read_retained_child_sources(
                    &["root".into()],
                    &requests[index..=index],
                    &fixture.snapshot(),
                    limits(1, individual_cap),
                    || Ok(()),
                )
                .is_ok(),
            "each child must fit the measured per-child retained-data cap"
        );
    }
    let combined_cap = retained_sizes[0].checked_add(retained_sizes[1]).unwrap();
    assert!(
        fixture
            .projection
            .read_retained_child_sources(
                &["root".into()],
                &requests,
                &fixture.snapshot(),
                limits(2, combined_cap),
                || Ok(()),
            )
            .is_ok(),
        "both children must fit the low cap equal to their measured sum"
    );
    let Err(error) = fixture.projection.read_retained_child_sources(
        &["root".into()],
        &requests,
        &fixture.snapshot(),
        limits(2, combined_cap - 1),
        || Ok(()),
    ) else {
        panic!("one byte below the combined child estimate must refuse");
    };
    assert_eq!(
        error,
        format!(
            "retained child source batch at index 1: retained-data limit exceeded (prior={}, variable={second_variable_bytes}, file={second_file_bytes}, next={combined_cap}, limit={})",
            retained_sizes[0], combined_cap - 1,
        )
    );
}

#[test]
fn retained_child_source_batch_late_parser_failure_returns_no_partial_children() {
    let fixture = Fixture::new();
    add_second_build_owner(&fixture);
    require(
        std::fs::write(fixture.build_root.join("two/mmakefile"), b"VALUE := \xff\n"),
        "could not write invalid late child input",
    );
    let environment = synthetic_environment();
    let first_invocation = invocation(&fixture.working_directory, &environment);
    let second_cwd = fixture.build_root.join("two");
    let second_invocation = invocation(&second_cwd, &environment);
    let first_observation = retained_observation(&first_invocation);
    let second_observation = retained_observation(&second_invocation);
    let dirs = DirVars::default();
    let first_paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let second_paths = fixture.paths(&dirs, Path::new("two"), &second_cwd);
    let requests = [
        RetainedChildSourceBatchRequest {
            paths: &first_paths,
            generated_owner: "one/mmakefile",
            observation: &first_observation,
        },
        RetainedChildSourceBatchRequest {
            paths: &second_paths,
            generated_owner: "two/mmakefile",
            observation: &second_observation,
        },
    ];
    let control_checks = Cell::new(0usize);
    let result = fixture.projection.read_retained_child_sources(
        &["root".into()],
        &requests,
        &fixture.snapshot(),
        limits(2, 1024 * 1024 * 1024),
        || {
            control_checks.set(control_checks.get() + 1);
            Ok(())
        },
    );
    let error = result
        .err()
        .expect("late invalid input must refuse the batch");
    assert!(error.starts_with("retained child source batch at index 1: "));
    assert_eq!(control_checks.get(), 8);
}
