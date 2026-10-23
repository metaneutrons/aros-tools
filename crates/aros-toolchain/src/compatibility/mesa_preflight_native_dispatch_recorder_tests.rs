//! Consumer tests for the supervised Root recorder phase.
//!
//! The default frontend below is a deliberately narrow shell stub. It checks
//! the generated capture command and supplies bounded invocation data, but it
//! does not test the shipped transpiler's capture semantics. The transpiler
//! package's `native_capture_cli` integration tests cover that executable.

use super::*;

#[path = "mesa_preflight_native_dispatch_recipe_execution_tests.rs"]
mod original_recipe_execution;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const CAPTURE_FILES: [&str; 4] = [
    "pending-command.json",
    "recorder.stdout",
    "recorder.stderr",
    "recorder.status.json",
];

#[derive(Clone, Copy)]
enum StubMode {
    Success,
    ExitSeven,
    ExtraEvidence,
    WrongArguments,
    WrongExports,
    MissingExports,
}

struct ObserverControl {
    invocation_counter: Option<PathBuf>,
    uses_shipped_frontend: bool,
}

fn quote_shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn recorder_payload(fixture: &Fixture, inputs: &DispatchInputs, mode: StubMode) -> Vec<u8> {
    let final_word = if matches!(mode, StubMode::WrongArguments) {
        "Fixture.other-quick"
    } else {
        "Fixture.build-quick"
    };
    let environment = [
        (
            "ROOT_INPUTS".to_owned(),
            format!(
                "{} {} {} {}",
                fixture.output_root.join("configure/Makefile").display(),
                fixture
                    .output_root
                    .join("configure/config/make.cfg")
                    .display(),
                inputs.dispatch_root.join("capture-override.mk").display(),
                inputs.dispatch_root.join("default-fence.mk").display(),
            ),
        ),
        ("OPAQUE-DATA".into(), "retained$literal".into()),
    ]
    .into_iter()
    .filter_map(|(name, value)| {
        if name == "ROOT_INPUTS" && matches!(mode, StubMode::MissingExports) {
            return None;
        }
        let value = if name == "ROOT_INPUTS" && matches!(mode, StubMode::WrongExports) {
            "unverified replacement".into()
        } else {
            value
        };
        Some((name, value))
    });
    aros_common::make_native_invocation::NativeMakeInvocation::new(
        [
            "env".to_owned(),
            "DISPATCH_SENTINEL=held".to_owned(),
            inputs.outputs.executable.to_string_lossy().into_owned(),
            "--fixture".to_owned(),
            final_word.to_owned(),
        ],
        fixture.output_root.join("configure"),
        environment,
    )
    .expect("construct fixed mock recorder data")
    .to_bytes()
    .expect("encode fixed mock recorder data")
}

fn capture_stub_script(
    fixture: &Fixture,
    inputs: &DispatchInputs,
    mode: StubMode,
    invocation_counter: &Path,
) -> String {
    let expected_output = inputs.dispatch_root.join("pending-command.json");
    let expected_program = &inputs.outputs.executable;
    let mut script = format!(
        "#!/bin/sh\numask 077\nprintf 'capture\\n' >> {}\n",
        quote_shell_word(&invocation_counter.display().to_string()),
    );
    for (position, expected, status) in [
        ("$#", "8", 40),
        ("$1", "--capture-make-invocation", 41),
        ("$2", expected_output.to_str().unwrap(), 42),
        ("$3", "--", 43),
        ("$4", "env", 44),
        ("$5", "DISPATCH_SENTINEL=held", 45),
        ("$6", expected_program.to_str().unwrap(), 46),
        ("$7", "--fixture", 47),
        ("$8", "Fixture.build-quick", 48),
    ] {
        if position == "$#" {
            writeln!(script, "[ {position} -eq {expected} ] || exit {status}")
                .expect("append shell argument check");
        } else {
            writeln!(
                script,
                "[ \"{position}\" = {} ] || exit {status}",
                quote_shell_word(expected)
            )
            .expect("append shell argument check");
        }
    }
    match mode {
        StubMode::Success
        | StubMode::WrongArguments
        | StubMode::WrongExports
        | StubMode::MissingExports => {
            let payload = String::from_utf8(recorder_payload(fixture, inputs, mode))
                .expect("mock invocation encoding is UTF-8");
            writeln!(
                script,
                "printf '%s' {} > \"$2\"",
                quote_shell_word(&payload)
            )
            .expect("append mock recorder payload command");
        }
        StubMode::ExitSeven => {
            script.push_str(
                "printf 'mock recorder stdout\\n'\nprintf 'mock recorder stderr\\n' >&2\nexit 7\n",
            );
        }
        StubMode::ExtraEvidence => {
            let payload = String::from_utf8(recorder_payload(fixture, inputs, mode))
                .expect("mock invocation encoding is UTF-8");
            let extra = inputs.dispatch_root.join("unexpected-recorder-output");
            writeln!(
                script,
                "printf '%s' {} > \"$2\"\nprintf 'unexpected' > {}",
                quote_shell_word(&payload),
                quote_shell_word(&extra.display().to_string()),
            )
            .expect("append mock recorder evidence commands");
        }
    }
    script
}

fn select_observer(
    fixture: &Fixture,
    inputs: &mut DispatchInputs,
    mode: StubMode,
    use_shipped_frontend_when_configured: bool,
) -> ObserverControl {
    let configured_frontend = use_shipped_frontend_when_configured
        .then(|| std::env::var_os("AROS_TEST_ROOT_CAPTURE_FRONTEND"))
        .flatten();
    let (observer, control) = configured_frontend.map_or_else(
        || {
            let observer = fixture
                .output_root
                .with_file_name("native-dispatch-recorder-frontend");
            let invocation_counter = fixture
                .output_root
                .with_file_name("native-dispatch-recorder-counter");
            let script = capture_stub_script(fixture, inputs, mode, &invocation_counter);
            write_executable(&observer, script.as_bytes());
            (
                fs::canonicalize(&observer).expect("canonical mock capture frontend"),
                ObserverControl {
                    invocation_counter: Some(invocation_counter),
                    uses_shipped_frontend: false,
                },
            )
        },
        |configured_frontend| {
            let requested = PathBuf::from(configured_frontend);
            let observer = fs::canonicalize(&requested).unwrap_or_else(|error| {
                panic!("cannot resolve configured capture frontend {requested:?}: {error}")
            });
            assert!(observer.is_absolute(), "capture frontend must be absolute");
            (
                observer,
                ObserverControl {
                    invocation_counter: None,
                    uses_shipped_frontend: true,
                },
            )
        },
    );
    inputs.observer_sha256 = sha256_bytes(&fs::read(&observer).expect("read selected frontend"));
    inputs.observer = observer;
    control
}

fn fenced_recorder_fixture(
    fixture: &mut Fixture,
    mode: StubMode,
    use_shipped_frontend_when_configured: bool,
) -> (
    ObservedFencedMesaNativeDispatch,
    DispatchInputs,
    ObserverControl,
) {
    let (launch, mut inputs) = observed_launch(fixture, "build-quick");
    let control = select_observer(
        fixture,
        &mut inputs,
        mode,
        use_shipped_frontend_when_configured,
    );
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let candidate = observe_mesa_native_dispatch(
        launch,
        &request,
        &inputs.host_query(),
        &inputs.host_build(),
        &inputs.launch_query(),
        &inputs.dispatch_selection(),
        &cancellation,
    )
    .expect("observe native dispatch with selected capture frontend");
    let fenced = candidate
        .observe_fenced(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &inputs.dispatch_selection(),
            &cancellation,
        )
        .expect("observe complete fenced Make invocation");
    (fenced, inputs, control)
}

fn entry_names(directory: &Path) -> BTreeSet<OsString> {
    fs::read_dir(directory)
        .expect("read private dispatch evidence")
        .map(|entry| entry.expect("read evidence entry").file_name())
        .collect()
}

fn assert_capture_file_modes(directory: &Path) {
    for name in CAPTURE_FILES {
        assert_eq!(
            fs::metadata(directory.join(name))
                .expect("capture output exists")
                .permissions()
                .mode()
                & 0o7777,
            0o600,
            "{name} must remain private"
        );
    }
}

#[test]
fn recorder_capture_binds_exact_child_data_and_revalidates_fresh_evidence() {
    let mut fixture = Fixture::new();
    let (fenced, inputs, control) = fenced_recorder_fixture(&mut fixture, StubMode::Success, true);
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let selected = inputs.dispatch_selection();
    let before = entry_names(&inputs.dispatch_root);

    let recorded = fenced
        .capture_recorder(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &selected,
            &cancellation,
        )
        .expect("capture the selected Root recorder invocation");

    let expected_arguments = vec![
        "env".to_owned(),
        "DISPATCH_SENTINEL=held".to_owned(),
        inputs.outputs.executable.to_str().unwrap().to_owned(),
        "--fixture".to_owned(),
        "Fixture.build-quick".to_owned(),
    ];
    assert_eq!(
        recorded.captured().raw().arguments(),
        expected_arguments.as_slice()
    );
    assert_eq!(
        recorded.captured().raw().working_directory(),
        fixture.output_root.join("configure").as_path()
    );
    assert_eq!(
        recorded.captured().invocation().argument_zero(),
        Some(inputs.outputs.executable.to_str().unwrap())
    );
    assert_eq!(
        recorded.captured().invocation().arguments(),
        ["--fixture".to_owned(), "Fixture.build-quick".to_owned()].as_slice()
    );
    assert_eq!(
        recorded.captured().invocation().environment()["DISPATCH_SENTINEL"],
        "held"
    );
    assert_eq!(
        recorded.captured().invocation().environment()["ROOT_INPUTS"],
        format!(
            "{} {}",
            fixture.output_root.join("configure/Makefile").display(),
            fixture
                .output_root
                .join("configure/config/make.cfg")
                .display()
        )
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &fs::read(inputs.dispatch_root.join("recorder.status.json")).unwrap()
        )
        .unwrap()["exit_code"],
        0
    );
    let mut expected_after = before;
    for name in CAPTURE_FILES {
        expected_after.insert(OsString::from(name));
    }
    assert_eq!(entry_names(&inputs.dispatch_root), expected_after);
    assert_capture_file_modes(&inputs.dispatch_root);
    assert!(
        !fixture
            .output_root
            .join("configure/native-dispatch-root-recipe-ran")
            .exists(),
        "the selected original MetaMake child must remain unlaunched"
    );
    assert_no_completed_preflight(&fixture);
    recorded
        .revalidate(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &selected,
            &cancellation,
        )
        .expect("unchanged recorder capture remains owned");

    if control.uses_shipped_frontend {
        let exports = recorded.captured().raw().environment();
        let input_list = exports
            .get("ROOT_INPUTS")
            .expect("real frontend captures deferred Source exports");
        assert!(input_list.contains(
            inputs
                .dispatch_root
                .join("capture-override.mk")
                .to_str()
                .unwrap()
        ));
        assert!(input_list.contains(
            inputs
                .dispatch_root
                .join("default-fence.mk")
                .to_str()
                .unwrap()
        ));
        let CompatibilityEnvironment::SealedHostTools { host_tools, .. } = &fixture.environment
        else {
            unreachable!();
        };
        assert_eq!(
            exports.get("PATH").map(String::as_str),
            host_tools.root.to_str()
        );
    } else {
        assert_eq!(
            fs::read(control.invocation_counter.unwrap()).unwrap(),
            b"capture\n",
            "the mock frontend should run exactly once"
        );
        assert_eq!(
            recorded.captured().invocation().environment()["OPAQUE-DATA"],
            "retained$literal"
        );
    }

    fs::write(
        inputs.dispatch_root.join("pending-command.json"),
        b"late mutation",
    )
    .unwrap();
    assert!(recorded
        .revalidate(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &selected,
            &cancellation,
        )
        .is_err());
}

#[test]
fn recorder_extended_output_keeps_old_read_closure_and_evidence_immutable() {
    for mutation in ["none", "old-input", "recorder-evidence"] {
        let mut fixture = Fixture::new();
        let (fenced, inputs, _control) =
            fenced_recorder_fixture(&mut fixture, StubMode::Success, false);
        let request = fixture.request();
        let cancellation = CancellationToken::default();
        let host_query = inputs.host_query();
        let host_build = inputs.host_build();
        let launch_query = inputs.launch_query();
        let dispatch = inputs.dispatch_selection();
        let recorded = fenced
            .capture_recorder(
                &request,
                &host_query,
                &host_build,
                &launch_query,
                &dispatch,
                &cancellation,
            )
            .unwrap();
        let origin = crate::compatibility::MesaNativeDirectOrigin {
            host_query: &host_query,
            host_build: &host_build,
            launch_query: &launch_query,
            dispatch: &dispatch,
        };
        let before = measure_tree_content_cas(&fixture.output_root).unwrap();
        fs::write(
            fixture.output_root.join("configure/direct-fresh"),
            b"fresh selected data\n",
        )
        .unwrap();
        let after = measure_tree_content_cas(&fixture.output_root).unwrap();
        assert!(before.matches_fresh_outputs(&after, &["configure/direct-fresh"], &[]));
        // Ordinary recorder validation must continue to reject new outputs.
        assert!(recorded
            .revalidate(
                &request,
                &host_query,
                &host_build,
                &launch_query,
                &dispatch,
                &cancellation
            )
            .is_err());
        match mutation {
            "old-input" => fs::write(
                fixture.output_root.join("configure/config/make.cfg"),
                b"changed original\n",
            )
            .unwrap(),
            "recorder-evidence" => fs::write(
                inputs.dispatch_root.join("pending-command.json"),
                b"changed capture\n",
            )
            .unwrap(),
            _ => {}
        }
        // A freshly supplied post-output digest cannot adopt changed old
        // Source reads or recorder bytes. The caller also checks fresh paths.
        let current = measure_tree_content_cas(&fixture.output_root).unwrap();
        let result =
            recorded.revalidate_extended_output(&request, &origin, &current, &cancellation);
        assert_eq!(result.is_ok(), mutation == "none");
    }
}

#[test]
fn recorder_nonzero_frontend_retains_logs_without_pending_capture_or_status() {
    let mut fixture = Fixture::new();
    let (fenced, inputs, control) =
        fenced_recorder_fixture(&mut fixture, StubMode::ExitSeven, false);
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let selected = inputs.dispatch_selection();

    assert!(fenced
        .capture_recorder(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &selected,
            &cancellation,
        )
        .is_err());

    assert_eq!(
        fs::read(control.invocation_counter.unwrap()).unwrap(),
        b"capture\n"
    );
    assert_eq!(
        fs::read(inputs.dispatch_root.join("recorder.stdout")).unwrap(),
        b"mock recorder stdout\n"
    );
    let recorder = inputs.dispatch_root.join("capture-override.mk");
    let recipe_line = fs::read_to_string(&recorder)
        .unwrap()
        .lines()
        .position(|line| line.starts_with("\t@+"))
        .expect("the generated static-pattern recorder recipe remains present")
        + 1;
    let expected_stderr = format!(
        "mock recorder stderr\nmake: *** [{}:{recipe_line}: build-quick] Error 7\n",
        recorder.display()
    );
    assert_eq!(
        fs::read(inputs.dispatch_root.join("recorder.stderr")).unwrap(),
        expected_stderr.as_bytes()
    );
    assert!(!inputs.dispatch_root.join("pending-command.json").exists());
    assert!(!inputs.dispatch_root.join("recorder.status.json").exists());
    assert!(!fixture
        .output_root
        .join("configure/native-dispatch-root-recipe-ran")
        .exists());
    assert_no_completed_preflight(&fixture);
}

#[test]
fn recorder_refuses_extra_evidence_and_mismatched_captured_data() {
    for mode in [
        StubMode::ExtraEvidence,
        StubMode::WrongArguments,
        StubMode::WrongExports,
        StubMode::MissingExports,
    ] {
        let mut fixture = Fixture::new();
        let (fenced, inputs, control) = fenced_recorder_fixture(&mut fixture, mode, false);
        let request = fixture.request();
        let cancellation = CancellationToken::default();
        let selected = inputs.dispatch_selection();

        assert!(fenced
            .capture_recorder(
                &request,
                &inputs.host_query(),
                &inputs.host_build(),
                &inputs.launch_query(),
                &selected,
                &cancellation,
            )
            .is_err());

        assert_eq!(
            fs::read(control.invocation_counter.unwrap()).unwrap(),
            b"capture\n"
        );
        for name in [
            "pending-command.json",
            "recorder.stdout",
            "recorder.stderr",
            "recorder.status.json",
        ] {
            assert!(
                inputs.dispatch_root.join(name).is_file(),
                "completed mock process retains {name} as unpromoted evidence"
            );
        }
        if matches!(mode, StubMode::ExtraEvidence) {
            assert!(inputs
                .dispatch_root
                .join("unexpected-recorder-output")
                .is_file());
        }
        assert!(!fixture
            .output_root
            .join("configure/native-dispatch-root-recipe-ran")
            .exists());
        assert_no_completed_preflight(&fixture);
    }
}

#[test]
fn recorder_refuses_preexisting_pending_capture_before_frontend_launch() {
    let mut fixture = Fixture::new();
    let (fenced, inputs, control) = fenced_recorder_fixture(&mut fixture, StubMode::Success, false);
    let pending = inputs.dispatch_root.join("pending-command.json");
    fs::write(&pending, b"foreign preexisting capture").unwrap();
    let request = fixture.request();
    let cancellation = CancellationToken::default();
    let selected = inputs.dispatch_selection();

    assert!(fenced
        .capture_recorder(
            &request,
            &inputs.host_query(),
            &inputs.host_build(),
            &inputs.launch_query(),
            &selected,
            &cancellation,
        )
        .is_err());

    assert_eq!(fs::read(&pending).unwrap(), b"foreign preexisting capture");
    assert!(!control.invocation_counter.unwrap().exists());
    for name in ["recorder.stdout", "recorder.stderr", "recorder.status.json"] {
        assert!(!inputs.dispatch_root.join(name).exists());
    }
    assert!(!fixture
        .output_root
        .join("configure/native-dispatch-root-recipe-ran")
        .exists());
    assert_no_completed_preflight(&fixture);
}
