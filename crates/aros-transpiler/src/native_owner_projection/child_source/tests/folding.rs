use super::*;
use crate::native_owner_projection::{
    RetainedChildSourceBatchLimits, RetainedChildSourceBatchRequest, RetainedChildSourceFoldLimits,
};
use aros_common::{
    make_input_snapshot::MakeInputBatchLimits,
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    sha256_bytes,
};
use std::{cell::Cell, path::Path, rc::Rc};

fn retained_observation(invocation: &NativeMakeInvocation) -> RetainedMakeObservation {
    let (stdout, environment_names) = pre_file_observation(invocation);
    let invocation_bytes = require(
        invocation.to_bytes(),
        "could not encode synthetic native invocation",
    );
    let binding = MakeObservationBinding {
        phase: MakeObservationPhase::PreFirstFile,
        context_sha256: sha256_bytes(b"independent fold child context"),
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

fn make_input_limits(maximum_children: usize) -> MakeInputBatchLimits {
    MakeInputBatchLimits {
        maximum_children,
        maximum_read_bytes: 1024 * 1024,
        maximum_reads: 128,
        maximum_queries: 128,
        maximum_query_names: 4096,
        maximum_query_bytes: 1024 * 1024,
        maximum_record_bytes: 1024 * 1024,
    }
}

fn fold_limits(
    maximum_children: usize,
    maximum_child_retained_bytes: usize,
    maximum_projected_bytes: usize,
) -> RetainedChildSourceFoldLimits {
    RetainedChildSourceFoldLimits {
        inputs: make_input_limits(maximum_children),
        maximum_child_retained_bytes,
        maximum_projected_bytes,
    }
}

fn batch_limits(
    maximum_children: usize,
    maximum_retained_bytes: usize,
) -> RetainedChildSourceBatchLimits {
    RetainedChildSourceBatchLimits {
        inputs: make_input_limits(maximum_children),
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

struct DropTrackedState {
    children: usize,
    payload: Box<[u8]>,
    drops: Rc<Cell<usize>>,
}

impl Drop for DropTrackedState {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

fn drop_tracked_state(drops: &Rc<Cell<usize>>) -> DropTrackedState {
    DropTrackedState {
        children: 0,
        payload: Vec::new().into_boxed_slice(),
        drops: Rc::clone(drops),
    }
}

fn append_child_payload(payload: &mut Box<[u8]>, index: usize) {
    let mut bytes = payload.to_vec();
    bytes.push(index as u8);
    *payload = bytes.into_boxed_slice();
}

fn assert_refused_and_dropped<T>(result: &Result<T, String>, drops: &Cell<usize>) {
    assert!(result.is_err(), "failed fold returned a result state");
    assert_eq!(drops.get(), 1, "failed fold did not drop its staged state");
}

#[test]
fn retained_child_source_fold_streams_two_children_with_individual_and_projected_caps() {
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
    let snapshot = fixture.snapshot();
    let retained = require(
        fixture.projection.read_retained_child_sources(
            &["root".into()],
            &requests,
            &snapshot,
            batch_limits(2, 1024 * 1024 * 1024),
            || Ok(()),
        ),
        "could not measure synthetic child retained estimates",
    );
    let retained_sizes = retained
        .iter()
        .map(|child| {
            child
                .retained_estimate_bytes()
                .expect("batch retained estimate")
        })
        .collect::<Vec<_>>();
    let maximum_single_child = *retained_sizes.iter().max().unwrap();
    assert!(retained_sizes.iter().all(|size| *size > 0));

    let projected_base = std::mem::size_of::<Box<[u8]>>();
    let projected_cap = projected_base + 2;
    let control_checks = Cell::new(0usize);
    let folded = require(
        fixture.projection.fold_retained_child_sources(
            &["root".into()],
            &requests,
            &snapshot,
            fold_limits(2, maximum_single_child, projected_cap),
            || {
                control_checks.set(control_checks.get() + 1);
                Ok(())
            },
            (
                Vec::<u8>::new().into_boxed_slice(),
                |payload, index, _child| {
                    append_child_payload(payload, index);
                    Ok(std::mem::size_of::<Box<[u8]>>() + payload.len())
                },
            ),
        ),
        "two children did not fold within the measured working and projected caps",
    );
    assert_eq!(folded.children(), 2);
    assert_eq!(folded.maximum_child_retained_bytes(), maximum_single_child);
    assert_eq!(folded.projected_bytes(), projected_base + 2);
    assert_eq!(folded.into_value().as_ref(), &[0, 1]);
    // The shared batch checks both transaction boundaries and each child's
    // before, setup, and after stages.
    assert_eq!(control_checks.get(), 8);

    assert!(
        fixture
            .projection
            .read_retained_child_sources(
                &["root".into()],
                &requests,
                &snapshot,
                batch_limits(2, maximum_single_child),
                || Ok(()),
            )
            .is_err(),
        "the vector API must still enforce its aggregate cap across both children"
    );
}

#[test]
fn retained_child_source_fold_rejects_one_below_working_cap_before_callback() {
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
    let snapshot = fixture.snapshot();
    let retained = require(
        fixture.projection.read_retained_child_sources(
            &["root".into()],
            &requests,
            &snapshot,
            batch_limits(2, 1024 * 1024 * 1024),
            || Ok(()),
        ),
        "could not measure synthetic child retained estimates",
    );
    let retained_sizes = retained
        .iter()
        .map(|child| {
            child
                .retained_estimate_bytes()
                .expect("batch retained estimate")
        })
        .collect::<Vec<_>>();
    let maximum_index = usize::from(retained_sizes[0] < retained_sizes[1]);
    let maximum_single_child = retained_sizes[maximum_index];
    let ordered_requests = if maximum_index == 0 {
        [
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
        ]
    } else {
        [
            RetainedChildSourceBatchRequest {
                paths: &second_paths,
                generated_owner: "two/mmakefile",
                observation: &second_observation,
            },
            RetainedChildSourceBatchRequest {
                paths: &first_paths,
                generated_owner: "one/mmakefile",
                observation: &first_observation,
            },
        ]
    };
    let calls = Cell::new(0usize);
    let result = fixture.projection.fold_retained_child_sources(
        &["root".into()],
        &ordered_requests,
        &snapshot,
        fold_limits(
            2,
            maximum_single_child - 1,
            std::mem::size_of::<Box<[u8]>>() + 2,
        ),
        || Ok(()),
        (
            Vec::<u8>::new().into_boxed_slice(),
            |payload, index, _child| {
                calls.set(calls.get() + 1);
                append_child_payload(payload, index);
                Ok(std::mem::size_of::<Box<[u8]>>() + payload.len())
            },
        ),
    );
    assert!(
        result.is_err(),
        "one byte below the largest child estimate was accepted"
    );
    assert!(result
        .err()
        .unwrap()
        .contains("working child limit exceeded"));
    assert_eq!(
        calls.get(),
        0,
        "working cap refusal happened after the callback"
    );
}

#[test]
fn retained_child_source_fold_rejects_projected_cap_after_callback_and_drops_state() {
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
    let snapshot = fixture.snapshot();
    let retained = require(
        fixture.projection.read_retained_child_sources(
            &["root".into()],
            &requests,
            &snapshot,
            batch_limits(2, 1024 * 1024 * 1024),
            || Ok(()),
        ),
        "could not measure synthetic child retained estimates",
    );
    let maximum_single_child = retained
        .iter()
        .map(|child| {
            child
                .retained_estimate_bytes()
                .expect("batch retained estimate")
        })
        .max()
        .unwrap();
    let drops = Rc::new(Cell::new(0usize));
    let calls = Cell::new(0usize);
    let result = fixture.projection.fold_retained_child_sources(
        &["root".into()],
        &requests,
        &snapshot,
        fold_limits(
            2,
            maximum_single_child,
            std::mem::size_of::<DropTrackedState>() + 1,
        ),
        || Ok(()),
        (drop_tracked_state(&drops), |state, _index, _child| {
            calls.set(calls.get() + 1);
            state.children += 1;
            append_child_payload(&mut state.payload, state.children - 1);
            Ok(std::mem::size_of::<DropTrackedState>() + state.payload.len())
        }),
    );
    assert!(result
        .as_ref()
        .err()
        .unwrap()
        .contains("projected-data limit exceeded"));
    assert_refused_and_dropped(&result, &drops);
    assert_eq!(
        calls.get(),
        2,
        "projected cap was not checked after the second callback"
    );
}

#[test]
fn retained_child_source_fold_refuses_late_invalid_child_without_returning_state() {
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
    let snapshot = fixture.snapshot();
    let drops = Rc::new(Cell::new(0usize));
    let calls = Cell::new(0usize);
    let result = fixture.projection.fold_retained_child_sources(
        &["root".into()],
        &requests,
        &snapshot,
        fold_limits(
            2,
            1024 * 1024 * 1024,
            std::mem::size_of::<DropTrackedState>() + 2,
        ),
        || Ok(()),
        (drop_tracked_state(&drops), |state, _index, _child| {
            calls.set(calls.get() + 1);
            state.children += 1;
            append_child_payload(&mut state.payload, state.children - 1);
            Ok(std::mem::size_of::<DropTrackedState>() + state.payload.len())
        }),
    );
    assert!(result
        .as_ref()
        .err()
        .unwrap()
        .starts_with("retained child source batch at index 1:"));
    assert_refused_and_dropped(&result, &drops);
    assert_eq!(
        calls.get(),
        1,
        "the late invalid child was not reached after child one"
    );
}

#[test]
fn retained_child_source_fold_refuses_late_callback_error_without_returning_state() {
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
    let snapshot = fixture.snapshot();
    let drops = Rc::new(Cell::new(0usize));
    let calls = Cell::new(0usize);
    let result = fixture.projection.fold_retained_child_sources(
        &["root".into()],
        &requests,
        &snapshot,
        fold_limits(
            2,
            1024 * 1024 * 1024,
            std::mem::size_of::<DropTrackedState>() + 2,
        ),
        || Ok(()),
        (drop_tracked_state(&drops), |state, index, _child| {
            calls.set(calls.get() + 1);
            state.children += 1;
            append_child_payload(&mut state.payload, index);
            if index == 1 {
                return Err("synthetic fold callback refusal".into());
            }
            Ok(std::mem::size_of::<DropTrackedState>() + state.payload.len())
        }),
    );
    let error = result.as_ref().err().unwrap();
    assert!(error.contains("projection refused"));
    assert!(!error.contains("synthetic fold callback refusal"));
    assert_refused_and_dropped(&result, &drops);
    assert_eq!(
        calls.get(),
        2,
        "late callback error did not follow child one"
    );
}

#[test]
fn retained_child_source_fold_refuses_root_change_after_callback_without_returning_state() {
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
    let snapshot = fixture.snapshot();
    let changed_path = fixture.build_root.join("two/extra.mk");
    let drops = Rc::new(Cell::new(0usize));
    let calls = Cell::new(0usize);
    let result = fixture.projection.fold_retained_child_sources(
        &["root".into()],
        &requests,
        &snapshot,
        fold_limits(
            2,
            1024 * 1024 * 1024,
            std::mem::size_of::<DropTrackedState>() + 2,
        ),
        || Ok(()),
        (drop_tracked_state(&drops), |state, index, child| {
            calls.set(calls.get() + 1);
            state.children += 1;
            append_child_payload(&mut state.payload, index);
            drop(child);
            if state.children == 1 {
                std::fs::write(&changed_path, "INCLUDED := changed-after-callback\n")
                    .map_err(|error| error.to_string())?;
            }
            Ok(std::mem::size_of::<DropTrackedState>() + state.payload.len())
        }),
    );
    assert_refused_and_dropped(&result, &drops);
    assert!(calls.get() >= 1, "root changed before the first callback");
}
