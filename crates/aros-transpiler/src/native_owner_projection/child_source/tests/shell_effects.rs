use super::*;
use crate::{
    make_shell_effects::{
        CapturedShellEffects, ShellEffectOutput, ShellEffectRequest, ShellEffectSelection,
    },
    native_owner_projection::{
        RetainedChildSourceBatchRequest, RetainedChildSourceEffectBatch,
        RetainedChildSourceFoldLimits,
    },
};
use aros_common::{
    make_input_snapshot::MakeInputBatchLimits,
    make_native_observation::{MakeObservationBinding, RetainedMakeObservation},
    make_native_sampler::{
        observe_first_input_make_with_recipe_controls, InitialMakeObservationRequest,
    },
    sha256_bytes, CancellationToken,
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Copy)]
enum SelectedOutput {
    Raw(&'static str),
    GnuValue(&'static str),
    Refuse,
}

struct TestEffects {
    command: String,
    working_directory: PathBuf,
    observation: MakeObservationBinding,
    output: SelectedOutput,
    queries: Cell<usize>,
    revalidations: Cell<usize>,
    finishes: Cell<usize>,
    refuse_revalidation: Option<usize>,
}

impl TestEffects {
    fn new(
        command: &str,
        working_directory: &Path,
        observation: &MakeObservationBinding,
        output: SelectedOutput,
    ) -> Self {
        Self {
            command: command.to_owned(),
            working_directory: working_directory.to_owned(),
            observation: observation.clone(),
            output,
            queries: Cell::new(0),
            revalidations: Cell::new(0),
            finishes: Cell::new(0),
            refuse_revalidation: None,
        }
    }
}

impl CapturedShellEffects for TestEffects {
    fn revalidate(&self) -> Result<(), String> {
        self.revalidations.set(self.revalidations.get() + 1);
        if self.refuse_revalidation == Some(self.revalidations.get()) {
            Err("private test runtime changed".into())
        } else {
            Ok(())
        }
    }

    fn query(&self, request: &ShellEffectRequest<'_>) -> Result<ShellEffectOutput, String> {
        self.queries.set(self.queries.get() + 1);
        // These two engine-created exports belong to this fixture's actual
        // selected GNU invocation, not a production environment reconstruction.
        let mut expected_exports = synthetic_environment();
        expected_exports.insert("MAKELEVEL".into(), "0".into());
        expected_exports.insert("MFLAGS".into(), "-nrs --no-print-directory".into());
        assert_eq!(request.source_exports, &expected_exports);
        if request.ordinal != 0
            || request.working_directory != self.working_directory
            || request.command != self.command
            || request.shell != "/bin/sh"
            || request.shell_flags != "-c"
            || !request.source_export_controls.is_empty()
            || request.native_context != Some(&self.observation)
        {
            return Err("selected test effect did not match".into());
        }

        match self.output {
            SelectedOutput::Raw(stdout) => Ok(crate::make_shell_data::ShellDataOutput {
                stdout: stdout.into(),
                status: 0,
            }
            .into()),
            SelectedOutput::GnuValue(value) => Ok(ShellEffectOutput::GnuValue(
                crate::make_shell_effects::gnu_query::GnuShellQueryResult {
                    value: value.into(),
                    status: 0,
                },
            )),
            SelectedOutput::Refuse => Err("private selected query failure".into()),
        }
    }

    fn finish(&self, effects: usize) -> Result<(), String> {
        self.finishes.set(self.finishes.get() + 1);
        if effects == 1 && self.queries.get() == 1 {
            Ok(())
        } else {
            Err("selected test session was incomplete".into())
        }
    }
}

fn selected_gnu_make() -> PathBuf {
    let search_path = std::env::var_os("PATH").expect("test process has no PATH");
    for name in ["gmake", "make"] {
        for candidate in std::env::split_paths(&search_path).map(|directory| directory.join(name)) {
            if !candidate.is_file() {
                continue;
            }
            let Ok(program) = candidate.canonicalize() else {
                continue;
            };
            let Ok(version) = Command::new(&program).arg("--version").output() else {
                continue;
            };
            if version.status.success() && version.stdout.starts_with(b"GNU Make ") {
                let version =
                    String::from_utf8(version.stdout).expect("selected GNU version is not UTF-8");
                let mut numbers = version
                    .lines()
                    .next()
                    .unwrap()
                    .strip_prefix("GNU Make ")
                    .unwrap()
                    .split('.');
                let major = numbers.next().unwrap().parse::<u32>().unwrap();
                let minor = numbers.next().unwrap().parse::<u32>().unwrap();
                assert!(
                    (major, minor) >= (4, 2),
                    "GNU Make >= 4.2 required for status"
                );
                return program;
            }
        }
    }
    panic!("shell-effect tests require a selected GNU Make executable");
}

/// Capture actual GNU Make first-input packets. The observer stops on its own
/// stdin prelude before opening the selected Source file.
fn actual_observation(
    program: &Path,
    working_directory: &Path,
    environment: &BTreeMap<String, String>,
    context_label: &str,
) -> RetainedMakeObservation {
    let invocation = require(
        aros_common::make_native_invocation::NativeMakeInvocation::new(
            [
                "-r",
                "-n",
                "-s",
                "--no-print-directory",
                "--file=mmakefile",
                "root",
            ]
            .into_iter()
            .map(str::to_owned),
            working_directory.to_owned(),
            environment
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        ),
        "could not construct an actual shell-effect child invocation",
    );
    let executable_sha256 = sha256_bytes(&require(
        std::fs::read(program),
        "could not measure selected GNU Make",
    ));
    let context_sha256 =
        sha256_bytes(format!("{context_label}:{}", working_directory.display()).as_bytes());
    let request = InitialMakeObservationRequest {
        program,
        executable_sha256: &executable_sha256,
        invocation: &invocation,
        context_sha256: &context_sha256,
        timeout: Duration::from_secs(20),
    };
    let observation = require(
        observe_first_input_make_with_recipe_controls(
            &request,
            &CancellationToken::default(),
            |_, _| Ok(()),
        ),
        "could not observe the selected GNU Make invocation before Source",
    );
    assert_eq!(
        observation.binding().phase,
        aros_common::make_native_observation::MakeObservationPhase::FirstInputPrelude
    );
    assert_eq!(observation.binding().context_sha256, context_sha256);
    assert_eq!(observation.invocation().environment(), environment);
    observation
}

fn with_pretty_invocation(observation: &RetainedMakeObservation) -> RetainedMakeObservation {
    let canonical = require(
        observation.invocation().to_bytes(),
        "could not retain actual invocation for noncanonical packet fixture",
    );
    let value: serde_json::Value = require(
        serde_json::from_slice(&canonical),
        "actual invocation was not valid JSON",
    );
    let pretty = require(
        serde_json::to_vec_pretty(&value),
        "could not encode noncanonical invocation packet",
    );
    assert!(
        pretty != canonical,
        "pretty invocation bytes were canonical"
    );
    let mut binding = observation.binding().clone();
    binding.invocation_sha256 = sha256_bytes(&pretty);
    require(
        RetainedMakeObservation::from_captured_bytes(
            &pretty,
            observation.initial_stdout(),
            observation.environment_names(),
            &binding,
        ),
        "could not retain noncanonical invocation packet",
    )
}

fn write_effect_inputs(fixture: &Fixture) -> (PathBuf, String, String) {
    let second = fixture.build_root.join("two");
    require(
        std::fs::create_dir_all(&second),
        "could not create second shell-effect child directory",
    );
    let first_command = "touch .leaf-effect-ran";
    let second_command = "touch .leaf-effect-ran";
    require(
        std::fs::write(
            fixture.working_directory.join("mmakefile"),
            format!("FIRST_EFFECT := $(shell {first_command})\n"),
        ),
        "could not write first shell-effect input",
    );
    require(
        std::fs::write(
            second.join("mmakefile"),
            format!("SECOND_EFFECT := $(shell {second_command})\n"),
        ),
        "could not write second shell-effect input",
    );
    (second, first_command.to_owned(), second_command.to_owned())
}

fn fold_limits(maximum_children: usize) -> RetainedChildSourceFoldLimits {
    RetainedChildSourceFoldLimits {
        inputs: MakeInputBatchLimits {
            maximum_children,
            maximum_read_bytes: 1024 * 1024,
            maximum_reads: 128,
            maximum_queries: 128,
            maximum_query_names: 4096,
            maximum_query_bytes: 1024 * 1024,
            maximum_record_bytes: 1024 * 1024,
        },
        maximum_child_retained_bytes: 1024 * 1024 * 1024,
        maximum_projected_bytes: 1024 * 1024,
    }
}

fn without_recipe_controls(observation: &RetainedMakeObservation) -> RetainedMakeObservation {
    let marker = format!(
        "{}=",
        aros_common::make_initial_values::MAKE_RECIPE_CONTROLS_METADATA_KEY
    );
    let mut stdout = Vec::new();
    let mut removed = 0;
    for line in observation
        .initial_stdout()
        .split_inclusive(|byte| *byte == b'\n')
    {
        if line.starts_with(marker.as_bytes()) {
            removed += 1;
        } else {
            stdout.extend_from_slice(line);
        }
    }
    assert_eq!(removed, 1);
    let invocation = require(
        observation.invocation().to_bytes(),
        "could not retain legacy-shaped observation invocation",
    );
    let mut binding = observation.binding().clone();
    binding.stdout_sha256 = sha256_bytes(&stdout);
    require(
        RetainedMakeObservation::from_captured_bytes(
            &invocation,
            &stdout,
            observation.environment_names(),
            &binding,
        ),
        "could not form a legacy-shaped observation from the actual packet",
    )
}

struct DropTrackedProjection {
    children: usize,
    drops: Rc<Cell<usize>>,
}

impl Drop for DropTrackedProjection {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

#[test]
fn retained_child_shell_effect_fold_uses_individual_sessions_for_two_children() {
    let fixture = Fixture::new();
    let (second_cwd, first_command, second_command) = write_effect_inputs(&fixture);
    let environment = synthetic_environment();
    let program = selected_gnu_make();
    let sampled_first_observation = actual_observation(
        &program,
        &fixture.working_directory,
        &environment,
        "first-shell-effect-child",
    );
    let first_observation = with_pretty_invocation(&sampled_first_observation);
    let second_observation = actual_observation(
        &program,
        &second_cwd,
        &environment,
        "second-shell-effect-child",
    );
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
    let first_effects = TestEffects::new(
        &first_command,
        &fixture.working_directory,
        first_observation.binding(),
        SelectedOutput::Raw("raw-one\nraw-two\n"),
    );
    let second_effects = TestEffects::new(
        &second_command,
        &second_cwd,
        second_observation.binding(),
        SelectedOutput::GnuValue("gnu-value"),
    );
    let shell_flags = ["-c"];
    let selections = [
        ShellEffectSelection {
            shell: "/bin/sh",
            flags: &shell_flags,
            provider: &first_effects,
            search: None,
            observation: first_observation.binding(),
        },
        ShellEffectSelection {
            shell: "/bin/sh",
            flags: &shell_flags,
            provider: &second_effects,
            search: None,
            observation: second_observation.binding(),
        },
    ];
    let snapshot = fixture.snapshot();
    let roots = ["root".to_owned()];
    let control_checks = Cell::new(0);
    let folded =
        require(
            fixture
                .projection
                .fold_retained_child_sources_with_shell_effects(
                    &roots,
                    RetainedChildSourceEffectBatch {
                        children: &requests,
                        effects: &selections,
                    },
                    &snapshot,
                    fold_limits(2),
                    || {
                        control_checks.set(control_checks.get() + 1);
                        Ok(())
                    },
                    (Vec::<String>::new(), |values, index, child| {
                        let (_, mut program, _, _) = child.into_parts();
                        let (expression, paths) = match index {
                            0 => ("$(FIRST_EFFECT)", &first_paths),
                            1 => ("$(SECOND_EFFECT)", &second_paths),
                            _ => return Err("unexpected child index".into()),
                        };
                        values.push(program.evaluate(expression, paths).map_err(|_| {
                            "test projection could not read effect value".to_owned()
                        })?);
                        Ok(std::mem::size_of::<Vec<String>>()
                            + values
                                .iter()
                                .map(|value| std::mem::size_of::<String>() + value.len())
                                .sum::<usize>())
                    }),
                ),
            "two independently bound shell-effect sessions did not fold",
        );

    assert_eq!(folded.children(), 2);
    assert_eq!(
        folded.into_value(),
        vec!["raw-one raw-two".to_owned(), "gnu-value".to_owned()]
    );
    assert_eq!(control_checks.get(), 8);
    assert_eq!(first_effects.queries.get(), 1);
    assert_eq!(first_effects.revalidations.get(), 2);
    assert_eq!(first_effects.finishes.get(), 1);
    assert_eq!(second_effects.queries.get(), 1);
    assert_eq!(second_effects.revalidations.get(), 2);
    assert_eq!(second_effects.finishes.get(), 1);
    assert!(!fixture.working_directory.join(".leaf-effect-ran").exists());
    assert!(!second_cwd.join(".leaf-effect-ran").exists());
}

#[test]
fn retained_child_shell_effect_postcheck_refusal_never_reaches_projection() {
    let fixture = Fixture::new();
    let (_, first_command, _) = write_effect_inputs(&fixture);
    let observation = actual_observation(
        &selected_gnu_make(),
        &fixture.working_directory,
        &synthetic_environment(),
        "postcheck-shell-effect-child",
    );
    let dirs = DirVars::default();
    let paths = fixture.paths(&dirs, Path::new("one"), &fixture.working_directory);
    let requests = [RetainedChildSourceBatchRequest {
        paths: &paths,
        generated_owner: "one/mmakefile",
        observation: &observation,
    }];
    let mut provider = TestEffects::new(
        &first_command,
        &fixture.working_directory,
        observation.binding(),
        SelectedOutput::Raw("complete-query"),
    );
    provider.refuse_revalidation = Some(2);
    let flags = ["-c"];
    let effects = [ShellEffectSelection {
        shell: "/bin/sh",
        flags: &flags,
        provider: &provider,
        search: None,
        observation: observation.binding(),
    }];
    let snapshot = fixture.snapshot();
    let projection_calls = Cell::new(0);
    let drops = Rc::new(Cell::new(0));
    let result = fixture
        .projection
        .fold_retained_child_sources_with_shell_effects(
            &["root".into()],
            RetainedChildSourceEffectBatch {
                children: &requests,
                effects: &effects,
            },
            &snapshot,
            fold_limits(1),
            || Ok(()),
            (
                DropTrackedProjection {
                    children: 0,
                    drops: Rc::clone(&drops),
                },
                |_, _, _| {
                    projection_calls.set(projection_calls.get() + 1);
                    Ok(0)
                },
            ),
        );
    let Err(error) = result else {
        panic!("failed provider postcheck returned staged data");
    };
    assert_eq!(
        error,
        "retained child source batch at index 0: shell effect provider postcheck refused"
    );
    assert!(!error.contains("private test runtime"));
    assert_eq!(provider.queries.get(), 1);
    assert_eq!(provider.finishes.get(), 1);
    assert_eq!(provider.revalidations.get(), 2);
    assert_eq!(projection_calls.get(), 0);
    assert_eq!(drops.get(), 1);
    assert!(!fixture.working_directory.join(".leaf-effect-ran").exists());
}

#[test]
fn retained_child_shell_effect_fold_refuses_unpaired_packets_and_drops_late_failure() {
    let fixture = Fixture::new();
    let (second_cwd, first_command, second_command) = write_effect_inputs(&fixture);
    let environment = synthetic_environment();
    let program = selected_gnu_make();
    let first_observation = actual_observation(
        &program,
        &fixture.working_directory,
        &environment,
        "first-shell-effect-child-negative",
    );
    let second_observation = actual_observation(
        &program,
        &second_cwd,
        &environment,
        "second-shell-effect-child-negative",
    );
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
    let roots = ["root".to_owned()];
    let shell_flags = ["-c"];
    let no_projection = Cell::new(0);

    let count_provider = TestEffects::new(
        &first_command,
        &fixture.working_directory,
        first_observation.binding(),
        SelectedOutput::Raw("unused"),
    );
    let count_selection = [ShellEffectSelection {
        shell: "/bin/sh",
        flags: &shell_flags,
        provider: &count_provider,
        search: None,
        observation: first_observation.binding(),
    }];
    let count_result = fixture
        .projection
        .fold_retained_child_sources_with_shell_effects(
            &roots,
            RetainedChildSourceEffectBatch {
                children: &requests,
                effects: &count_selection,
            },
            &snapshot,
            fold_limits(2),
            || Ok(()),
            ((), |(), _, _| {
                no_projection.set(no_projection.get() + 1);
                Ok(0)
            }),
        );
    let Err(count_error) = count_result else {
        panic!("selection count mismatch was accepted");
    };
    assert_eq!(
        count_error,
        "child Source shell session count differs from selection"
    );
    assert_eq!(count_provider.queries.get(), 0);
    assert_eq!(count_provider.revalidations.get(), 0);
    assert_eq!(count_provider.finishes.get(), 0);
    assert_eq!(no_projection.get(), 0);

    let binding_first = TestEffects::new(
        &first_command,
        &fixture.working_directory,
        first_observation.binding(),
        SelectedOutput::Raw("unused"),
    );
    let binding_second = TestEffects::new(
        &second_command,
        &second_cwd,
        second_observation.binding(),
        SelectedOutput::Raw("unused"),
    );
    let swapped_bindings = [
        ShellEffectSelection {
            shell: "/bin/sh",
            flags: &shell_flags,
            provider: &binding_first,
            search: None,
            observation: second_observation.binding(),
        },
        ShellEffectSelection {
            shell: "/bin/sh",
            flags: &shell_flags,
            provider: &binding_second,
            search: None,
            observation: first_observation.binding(),
        },
    ];
    let binding_result = fixture
        .projection
        .fold_retained_child_sources_with_shell_effects(
            &roots,
            RetainedChildSourceEffectBatch {
                children: &requests,
                effects: &swapped_bindings,
            },
            &snapshot,
            fold_limits(2),
            || Ok(()),
            ((), |(), _, _| {
                no_projection.set(no_projection.get() + 1);
                Ok(0)
            }),
        );
    let Err(binding_error) = binding_result else {
        panic!("swapped shell-effect observation bindings were accepted");
    };
    assert_eq!(
        binding_error,
        "retained child source fold at index 0: shell session child binding differs"
    );
    assert_eq!(binding_first.queries.get(), 0);
    assert_eq!(binding_first.revalidations.get(), 0);
    assert_eq!(binding_first.finishes.get(), 0);
    assert_eq!(binding_second.queries.get(), 0);
    assert_eq!(binding_second.revalidations.get(), 0);
    assert_eq!(binding_second.finishes.get(), 0);
    assert_eq!(no_projection.get(), 0);

    let legacy_observation = without_recipe_controls(&first_observation);
    let legacy_provider = TestEffects::new(
        &first_command,
        &fixture.working_directory,
        legacy_observation.binding(),
        SelectedOutput::Raw("unused"),
    );
    let legacy_selection = [ShellEffectSelection {
        shell: "/bin/sh",
        flags: &shell_flags,
        provider: &legacy_provider,
        search: None,
        observation: legacy_observation.binding(),
    }];
    let first_request = [RetainedChildSourceBatchRequest {
        paths: &first_paths,
        generated_owner: "one/mmakefile",
        observation: &legacy_observation,
    }];
    let legacy_result = fixture
        .projection
        .fold_retained_child_sources_with_shell_effects(
            &roots,
            RetainedChildSourceEffectBatch {
                children: &first_request,
                effects: &legacy_selection,
            },
            &snapshot,
            fold_limits(1),
            || Ok(()),
            ((), |(), _, _| {
                no_projection.set(no_projection.get() + 1);
                Ok(0)
            }),
        );
    let Err(legacy_error) = legacy_result else {
        panic!("legacy observation without the recipe-control packet was accepted");
    };
    assert_eq!(
        legacy_error,
        "retained child source batch at index 0: native shell effects need measured recipe controls"
    );
    assert_eq!(legacy_provider.queries.get(), 0);
    assert_eq!(legacy_provider.revalidations.get(), 0);
    assert_eq!(legacy_provider.finishes.get(), 0);
    assert_eq!(no_projection.get(), 0);

    let mut unsafe_environment = synthetic_environment();
    unsafe_environment.insert("IFS".into(), "private-separator".into());
    let unsafe_observation = actual_observation(
        &program,
        &fixture.working_directory,
        &unsafe_environment,
        "shell-effect-original-environment",
    );
    let unsafe_provider = TestEffects::new(
        &first_command,
        &fixture.working_directory,
        unsafe_observation.binding(),
        SelectedOutput::Raw("unused"),
    );
    let unsafe_selection = [ShellEffectSelection {
        shell: "/bin/sh",
        flags: &shell_flags,
        provider: &unsafe_provider,
        search: None,
        observation: unsafe_observation.binding(),
    }];
    let unsafe_request = [RetainedChildSourceBatchRequest {
        paths: &first_paths,
        generated_owner: "one/mmakefile",
        observation: &unsafe_observation,
    }];
    let unsafe_environment_result = fixture
        .projection
        .fold_retained_child_sources_with_shell_effects(
            &roots,
            RetainedChildSourceEffectBatch {
                children: &unsafe_request,
                effects: &unsafe_selection,
            },
            &snapshot,
            fold_limits(1),
            || Ok(()),
            ((), |(), _, _| {
                no_projection.set(no_projection.get() + 1);
                Ok(0)
            }),
        );
    let Err(unsafe_environment_error) = unsafe_environment_result else {
        panic!("shell policy ignored the original invocation environment");
    };
    assert_eq!(
        unsafe_environment_error,
        "retained child source batch at index 0: invalid Make expression `Make variable program`: input mmakefile line 1: invalid Make expression `captured shell data`: captured shell policy rejects exported environment"
    );
    assert_eq!(unsafe_provider.queries.get(), 0);
    assert_eq!(unsafe_provider.revalidations.get(), 2);
    assert_eq!(unsafe_provider.finishes.get(), 0);
    assert_eq!(no_projection.get(), 0);

    let first_effects = TestEffects::new(
        &first_command,
        &fixture.working_directory,
        first_observation.binding(),
        SelectedOutput::Raw("first-effect"),
    );
    let second_effects = TestEffects::new(
        &second_command,
        &second_cwd,
        second_observation.binding(),
        SelectedOutput::Refuse,
    );
    let selections = [
        ShellEffectSelection {
            shell: "/bin/sh",
            flags: &shell_flags,
            provider: &first_effects,
            search: None,
            observation: first_observation.binding(),
        },
        ShellEffectSelection {
            shell: "/bin/sh",
            flags: &shell_flags,
            provider: &second_effects,
            search: None,
            observation: second_observation.binding(),
        },
    ];
    let drops = Rc::new(Cell::new(0));
    let projected_children = Cell::new(0);
    let control_checks = Cell::new(0);
    let staged_projection = DropTrackedProjection {
        children: 0,
        drops: Rc::clone(&drops),
    };
    let late_result = fixture
        .projection
        .fold_retained_child_sources_with_shell_effects(
            &roots,
            RetainedChildSourceEffectBatch {
                children: &requests,
                effects: &selections,
            },
            &snapshot,
            fold_limits(2),
            || {
                control_checks.set(control_checks.get() + 1);
                Ok(())
            },
            (staged_projection, |state, _, _| {
                state.children += 1;
                projected_children.set(projected_children.get() + 1);
                Ok(std::mem::size_of::<DropTrackedProjection>() + state.children)
            }),
        );
    let Err(late_error) = late_result else {
        panic!("late selected shell query failure returned a fold result");
    };
    assert_eq!(
        late_error,
        "retained child source batch at index 1: invalid Make expression `Make variable program`: input mmakefile line 1: invalid Make expression `captured shell data`: bound shell effect provider refused the selected effect"
    );
    assert_eq!(drops.get(), 1);
    assert_eq!(projected_children.get(), 1);
    assert_eq!(control_checks.get(), 8);
    assert_eq!(first_effects.queries.get(), 1);
    assert_eq!(first_effects.revalidations.get(), 2);
    assert_eq!(first_effects.finishes.get(), 1);
    assert_eq!(second_effects.queries.get(), 1);
    assert_eq!(second_effects.revalidations.get(), 2);
    assert_eq!(second_effects.finishes.get(), 0);
    assert!(!fixture.working_directory.join(".leaf-effect-ran").exists());
    assert!(!second_cwd.join(".leaf-effect-ran").exists());
}
