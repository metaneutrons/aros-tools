//! Explicit native GNU Make observation entrypoint coverage.
//!
//! These tests verify the private evidence handoff and its fail-closed launch
//! boundary. A retained observation is data consistency, not execution proof.

#![cfg(unix)]

use aros_common::{
    make_native_observation::{
        MakeObservationBinding, MakeObservationPhase, RetainedMakeObservation,
    },
    measure_regular_file_bounded, sha256_bytes, Sha256Digest,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

const REFUSAL: &[u8] = b"native Make initial observation refused\n";
const TEST_CONTEXT: &[u8] = b"native observation CLI test context";

#[path = "native_observe_cli/first_input.rs"]
mod first_input;

#[path = "native_observe_cli/collection.rs"]
mod collection;

#[path = "native_observe_cli/collection_recipe_controls.rs"]
mod collection_recipe_controls;

fn private_root(temporary: &TempDir) -> PathBuf {
    let root = temporary
        .path()
        .canonicalize()
        .expect("canonical test root");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .expect("set private test root mode");
    root
}

fn context_digest() -> Sha256Digest {
    sha256_bytes(TEST_CONTEXT)
}

fn measure_executable(path: &Path) -> Sha256Digest {
    let (_, bytes) = measure_regular_file_bounded(path, 256 * 1024 * 1024)
        .expect("measure selected executable")
        .expect("selected executable is a regular file");
    sha256_bytes(&bytes)
}

fn find_make_program() -> PathBuf {
    let name = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let path = std::env::var_os("PATH").expect("test runner provides PATH");
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return candidate
                .canonicalize()
                .expect("canonical GNU Make executable");
        }
    }
    panic!("GNU Make executable is required for native observation coverage");
}

fn observe_command(
    root: &Path,
    cwd: &Path,
    program: &Path,
    executable_sha256: &str,
    arguments: &[&str],
    environment: &BTreeMap<String, String>,
) -> Output {
    let context = context_digest();
    let mut command = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"));
    command
        .env_clear()
        .envs(environment)
        .current_dir(cwd)
        .args(["--observe-make-invocation"])
        .arg(root)
        .arg(program)
        .arg(executable_sha256)
        .arg(context.as_str())
        .arg("--")
        .args(arguments);
    command.output().expect("run native observation entrypoint")
}

fn success_environment(root: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("PRIVATE_ENV_SECRET".into(), "env-native-sentinel".into()),
        ("AROS_TRANSPILER_LOG_LEVEL".into(), "trace".into()),
        (
            "AROS_TRANSPILER_LOG_FILE".into(),
            root.join("must-not-log").to_string_lossy().into_owned(),
        ),
        (
            "AROS_TRANSPILER_DIAGNOSTIC_FORMAT".into(),
            "must-not-parse".into(),
        ),
    ])
}

fn assert_refused(output: &Output) {
    assert!(
        !output.status.success(),
        "refused invocation unexpectedly succeeded"
    );
    assert!(output.stdout.is_empty(), "refused invocation wrote stdout");
    assert!(
        output.stderr.as_slice() == REFUSAL,
        "refusal diagnostic was not static"
    );
}

fn assert_private_file(path: &Path) {
    let metadata = fs::metadata(path).expect("retained private file exists");
    assert!(metadata.is_file(), "retained output is not a regular file");
    assert!(
        metadata.permissions().mode() & 0o7777 == 0o600,
        "retained file does not have mode 0600"
    );
}

fn assert_status(path: &Path, expected_status: i64) {
    let status: Value = serde_json::from_slice(&fs::read(path).expect("read retained status"))
        .expect("parse retained status");
    assert_eq!(status["status"].as_i64(), Some(expected_status));
    assert_eq!(status["timed_out"].as_bool(), Some(false));
    assert_eq!(status["cancelled"].as_bool(), Some(false));
    assert_eq!(status["stdout_exact"].as_bool(), Some(true));
    assert_eq!(status["stderr_exact"].as_bool(), Some(true));
}

#[test]
fn explicit_observation_retains_private_consistent_data_and_stops_before_makefiles() {
    let temporary = tempfile::tempdir().expect("temporary observation root");
    let root = private_root(&temporary);
    let cwd = root.join("working directory");
    fs::create_dir(&cwd).expect("create invocation directory");
    let makefile = "$(file >parse-side-effect.marker,parsed)\n$(shell printf shell >shell-side-effect.marker)\n.PHONY: all\nall:\n\t@printf recipe >recipe-side-effect.marker\n";
    fs::write(cwd.join("effects.mk"), makefile).expect("write synthetic Makefile");
    let program = find_make_program();
    let executable_sha256 = measure_executable(&program).to_string();
    let environment = success_environment(&root);
    let control = root.join("control directory");
    fs::create_dir(&control).expect("create controlled effect counter directory");
    fs::write(control.join("effects.mk"), makefile).expect("write controlled effect counter");
    let counter = Command::new(&program)
        .env_clear()
        .envs(&environment)
        .current_dir(&control)
        .args(["-f", "effects.mk", "all"])
        .output()
        .expect("run synthetic uninstrumented effect counter");
    assert!(counter.status.success(), "controlled effect counter failed");
    for name in [
        "parse-side-effect.marker",
        "shell-side-effect.marker",
        "recipe-side-effect.marker",
    ] {
        assert!(
            control.join(name).is_file(),
            "controlled counter did not demonstrate its effect"
        );
    }
    let arguments = ["-f", "effects.mk", "CLI_SECRET=cli-native-sentinel", "all"];

    let output = observe_command(
        &root,
        &cwd,
        &program,
        &executable_sha256,
        &arguments,
        &environment,
    );
    assert!(
        output.status.success(),
        "native observation did not complete"
    );
    assert!(
        output.stdout.is_empty(),
        "successful observation wrote stdout"
    );
    assert!(
        output.stderr.is_empty(),
        "successful observation wrote stderr"
    );
    assert!(
        !root.join("must-not-log").exists(),
        "private entry opened normal logging"
    );
    assert!(
        !cwd.join("parse-side-effect.marker").exists(),
        "selected Makefile was parsed during initial observation"
    );
    assert!(
        !cwd.join("recipe-side-effect.marker").exists(),
        "selected Make recipe ran during initial observation"
    );
    assert!(
        !cwd.join("shell-side-effect.marker").exists(),
        "selected source shell effect ran"
    );
    assert!(
        fs::metadata(&root)
            .expect("private root metadata")
            .permissions()
            .mode()
            & 0o7777
            == 0o700,
        "capture root does not have mode 0700"
    );

    let names = [
        "observation-started.json",
        "invocation.json",
        "discovery.stdout",
        "discovery.stderr",
        "discovery.status.json",
        "initial.stdout",
        "initial.stderr",
        "initial.status.json",
        "observation.json",
        "observation-selection.json",
    ];
    for name in names {
        assert_private_file(&root.join(name));
    }
    let before: Vec<_> = names
        .iter()
        .map(|name| {
            sha256_bytes(&fs::read(root.join(name)).expect("read completed private output"))
        })
        .collect();
    assert_refused(&observe_command(
        &root,
        &cwd,
        &program,
        &executable_sha256,
        &arguments,
        &environment,
    ));
    for (name, expected) in names.iter().zip(before) {
        assert!(
            sha256_bytes(&fs::read(root.join(name)).expect("read preserved private output"))
                == expected,
            "reused observation modified an existing output"
        );
    }
    assert_status(&root.join("discovery.status.json"), 2);
    assert_status(&root.join("initial.status.json"), 2);

    let document = fs::read(root.join("observation.json")).expect("read retained observation");
    let selection: Value = serde_json::from_slice(
        &fs::read(root.join("observation-selection.json")).expect("read selection metadata"),
    )
    .expect("parse selection metadata");
    assert_eq!(selection["version"].as_u64(), Some(1));
    assert!(
        selection["qualification"].as_str() == Some("instrumented-initial-observation-only"),
        "unexpected qualification label"
    );
    let selected_document_sha256 = sha256_bytes(&document);
    assert!(
        selection["document_sha256"].as_str() == Some(selected_document_sha256.as_str()),
        "selection digest does not identify retained document bytes"
    );
    let binding: MakeObservationBinding = serde_json::from_value(selection["binding"].clone())
        .expect("parse selected observation binding");
    assert_eq!(binding.phase, MakeObservationPhase::PreFirstFile);
    assert_eq!(binding.context_sha256, context_digest());
    let retained = RetainedMakeObservation::load(&document, &selected_document_sha256, &binding)
        .expect("load selected observation data");
    assert!(retained
        .invocation()
        .arguments()
        .iter()
        .map(String::as_str)
        .eq(arguments));
    assert!(
        retained.invocation().working_directory() == cwd.as_path(),
        "retained cwd changed"
    );
    assert!(
        retained.invocation().environment() == &environment,
        "retained environment changed"
    );
    assert!(retained.environment_names().contains("PRIVATE_ENV_SECRET"));
    let initial_stdout =
        std::str::from_utf8(retained.initial_stdout()).expect("retained observer stdout is UTF-8");
    assert!(
        initial_stdout.contains("CLI_SECRET|command line|recursive|cli-native-sentinel"),
        "loader did not preserve the selected command-line value"
    );
    assert!(
        initial_stdout.contains("PRIVATE_ENV_SECRET|environment|recursive|env-native-sentinel"),
        "loader did not preserve the selected environment value"
    );
}

#[test]
fn private_directory_writer_refuses_aliases_overwrite_and_replaced_roots() {
    use aros_common::make_native_invocation::PrivateNativeCaptureDirectory;
    use std::{ffi::OsStr, os::unix::fs::symlink};
    let temporary = tempfile::tempdir().expect("private writer fixture");
    let root = private_root(&temporary);
    let path = root.join("held");
    fs::create_dir(&path).expect("create held directory");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("private held mode");
    let directory = PrivateNativeCaptureDirectory::open(&path).expect("bind held directory");
    directory
        .write_new(OsStr::new("record"), b"original")
        .expect("write original private record");
    let original_sha = sha256_bytes(b"original");
    assert!(
        directory
            .read_selected(OsStr::new("record"), &original_sha, 8)
            .expect("read independently selected private record")
            == b"original",
        "selected private bytes changed"
    );
    assert!(directory
        .read_selected(OsStr::new("record"), &sha256_bytes(b"foreign"), 8)
        .is_err());
    assert!(directory
        .read_selected(OsStr::new("record"), &original_sha, 7)
        .is_err());
    assert!(directory
        .read_selected(OsStr::new("record"), &original_sha, 0)
        .is_err());
    let file_alias = path.join("file-alias");
    symlink(path.join("record"), &file_alias).expect("create owned file alias counter");
    assert!(directory
        .read_selected(OsStr::new("file-alias"), &original_sha, 8)
        .is_err());
    fs::hard_link(path.join("record"), path.join("linked-record"))
        .expect("create owned hardlink counter");
    assert!(directory
        .read_selected(OsStr::new("record"), &original_sha, 8)
        .is_err());
    assert!(directory
        .write_new(OsStr::new("record"), b"replacement")
        .is_err());
    assert!(
        fs::read(path.join("record")).expect("read original") == b"original",
        "original record was overwritten"
    );
    for name in ["../outside", "nested/record", "record/", ".", ""] {
        assert!(directory.write_new(OsStr::new(name), b"refuse").is_err());
    }
    assert!(directory
        .write_new(OsStr::new("oversized"), &vec![0; 4 * 1024 * 1024 + 1])
        .is_err());
    assert!(!path.join("oversized").exists());
    let alias = root.join("alias");
    symlink(&path, &alias).expect("create writer alias counter");
    assert!(PrivateNativeCaptureDirectory::open(&alias).is_err());
    fs::rename(&path, root.join("moved")).expect("move only owned fixture root");
    fs::create_dir(&path).expect("replace owned fixture root");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .expect("replacement private mode");
    assert!(directory.revalidate().is_err());
    assert!(directory.write_new(OsStr::new("after"), b"refuse").is_err());
    assert!(!path.join("after").exists() && !root.join("moved/after").exists());
}

#[test]
fn short_launch_control_requires_exact_private_selected_manifest_bytes() {
    use aros_common::make_native_invocation::PrivateNativeCaptureDirectory;
    use std::ffi::OsStr;
    let program = find_make_program();
    let program_sha = measure_executable(&program);
    for mode in ["valid", "digest", "unknown", "version", "public", "linked"] {
        let temporary = tempfile::tempdir().expect("short launch fixture");
        let root = private_root(&temporary);
        let output_root = root.join("data");
        fs::create_dir(&output_root).expect("create private observation output");
        fs::set_permissions(&output_root, fs::Permissions::from_mode(0o700))
            .expect("private observation mode");
        let mut control = serde_json::json!({"version":1,"output_root":output_root,
            "program":program,"executable_sha256":program_sha,"context_sha256":context_digest()});
        if mode == "unknown" {
            control["unbound"] = true.into();
        }
        if mode == "version" {
            control["version"] = 2.into();
        }
        let bytes = serde_json::to_vec(&control).expect("encode selected control fixture");
        let directory = PrivateNativeCaptureDirectory::open(&root).expect("bind control directory");
        directory
            .write_new(OsStr::new("control"), &bytes)
            .expect("retain private control");
        let mut selected_path = root.join("control");
        if mode == "public" {
            fs::set_permissions(&selected_path, fs::Permissions::from_mode(0o644))
                .expect("counter public mode");
        }
        if mode == "linked" {
            selected_path = root.join("alias");
            std::os::unix::fs::symlink(root.join("control"), &selected_path)
                .expect("counter alias");
        }
        let expected = if mode == "digest" {
            sha256_bytes(b"different bytes")
        } else {
            sha256_bytes(&bytes)
        };
        let output = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
            .env_clear()
            .env("LC_ALL", "C")
            .env("LITERAL_INPUT", "literal-$-value")
            .current_dir(&root)
            .arg("--observe-make-launch")
            .arg(&selected_path)
            .arg(expected.as_str())
            .arg("--")
            .args(["-r", "-R", "-f", "never-read.mk", "FLAG=from-parent", "all"])
            .output()
            .expect("run selected short launcher");
        if mode == "valid" {
            assert!(
                output.status.success() && output.stdout.is_empty() && output.stderr.is_empty(),
                "short launcher did not complete privately"
            );
            assert!(output_root.join("observation.json").is_file());
            let invocation = aros_common::make_native_invocation::NativeMakeInvocation::parse(
                &fs::read(output_root.join("invocation.json"))
                    .expect("read original short-launch argv"),
            )
            .expect("parse original short-launch data");
            assert!(
                invocation.arguments().iter().map(String::as_str).eq([
                    "-r",
                    "-R",
                    "-f",
                    "never-read.mk",
                    "FLAG=from-parent",
                    "all"
                ]),
                "short launch changed original argv"
            );
            assert!(
                invocation
                    .environment()
                    .get("LITERAL_INPUT")
                    .map(String::as_str)
                    == Some("literal-$-value"),
                "short launch changed inherited values"
            );
        } else {
            assert_refused(&output);
            assert!(
                !output_root.join("observation-started.json").exists(),
                "unselected control started a capture"
            );
        }
    }
}

#[test]
fn sampler_refuses_zero_budget_cancellation_and_retention_failure_without_completion() {
    use aros_common::{
        make_native_invocation::NativeMakeInvocation,
        make_native_sampler::{
            observe_initial_make, InitialMakeObservationRequest, NativeMakeObservationStage,
        },
        CancellationToken,
    };
    use std::time::Duration;
    let temporary = tempfile::tempdir().expect("sampler control fixture");
    let cwd = private_root(&temporary);
    let invocation = NativeMakeInvocation::new(
        vec!["-f".into(), "never-read.mk".into(), "all".into()],
        cwd,
        [("LC_ALL".into(), "C".into())],
    )
    .expect("literal sampler fixture");
    let program = find_make_program();
    let executable_sha256 = measure_executable(&program);
    let context_sha256 = context_digest();
    let mut request = InitialMakeObservationRequest {
        program: &program,
        executable_sha256: &executable_sha256,
        invocation: &invocation,
        context_sha256: &context_sha256,
        timeout: Duration::ZERO,
    };
    let mut launches = 0;
    assert!(
        observe_initial_make(&request, &CancellationToken::default(), |_, _| {
            launches += 1;
            Ok(())
        })
        .is_err()
    );
    assert_eq!(launches, 0);
    request.timeout = Duration::from_secs(5);
    let cancelled = CancellationToken::default();
    cancelled.cancel();
    assert!(observe_initial_make(&request, &cancelled, |_, _| {
        launches += 1;
        Ok(())
    })
    .is_err());
    assert_eq!(launches, 0);
    let refusal = observe_initial_make(&request, &CancellationToken::default(), |_, _| {
        launches += 1;
        Err("private sink detail must not escape".into())
    });
    assert_eq!(launches, 1);
    assert!(
        refusal.err().as_deref() == Some("native Make observation retention failed"),
        "retention failure diagnostic was not sanitized"
    );
    let cancellation = CancellationToken::default();
    let mut stages = 0;
    let refusal = observe_initial_make(&request, &cancellation, |stage, _| {
        stages += 1;
        if matches!(stage, NativeMakeObservationStage::Initial) {
            cancellation.cancel();
        }
        Ok(())
    });
    assert_eq!(stages, 2);
    assert!(
        refusal.err().as_deref() == Some("native Make observation was cancelled before completion"),
        "cancelled observation returned an unexpected result"
    );
}

fn fake_make(temporary: &TempDir) -> (PathBuf, Sha256Digest) {
    let path = temporary.path().join("fake-make");
    fs::write(
        &path,
        b"#!/bin/sh\nprintf 'launched' > \"$NATIVE_LAUNCH_MARKER\"\nprintf 'raw-process-diagnostic-sentinel\\n' >&2\nexit 23\n",
    )
    .expect("write fake executable");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .expect("set fake executable mode");
    let path = path.canonicalize().expect("canonical fake executable");
    let digest = measure_executable(&path);
    (path, digest)
}

fn launch_environment(root: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([(
        "NATIVE_LAUNCH_MARKER".into(),
        root.join("launched.marker").to_string_lossy().into_owned(),
    )])
}

fn run_prelaunch_refusal(
    program: &Path,
    executable_sha256: &str,
    arguments: &[&str],
    extra_environment: Option<(&str, &str)>,
) {
    let temporary = tempfile::tempdir().expect("temporary prelaunch root");
    let root = private_root(&temporary);
    let mut environment = launch_environment(&root);
    if let Some((name, value)) = extra_environment {
        environment.insert(name.to_owned(), value.to_owned());
    }
    let output = observe_command(
        &root,
        &root,
        program,
        executable_sha256,
        arguments,
        &environment,
    );
    assert_refused(&output);
    assert!(
        !root.join("launched.marker").exists(),
        "prelaunch refusal ran selected executable"
    );
    assert!(
        !root.join("observation.json").exists(),
        "refusal published an observation"
    );
    assert!(
        !root.join("discovery.stdout").exists(),
        "refusal retained launched-process output"
    );
}

#[test]
fn invalid_program_digest_startup_effects_and_preloaded_makefiles_refuse_before_launch() {
    let executable_temporary = tempfile::tempdir().expect("temporary fake executable root");
    let (program, correct_digest) = fake_make(&executable_temporary);
    let valid = ["-f", "input.mk", "all"];

    run_prelaunch_refusal(&program, &"0".repeat(64), &valid, None);
    run_prelaunch_refusal(
        &program,
        correct_digest.as_str(),
        &[
            "-f",
            "input.mk",
            "--eval",
            "$(shell touch startup.marker)",
            "all",
        ],
        None,
    );
    run_prelaunch_refusal(
        &program,
        correct_digest.as_str(),
        &[
            "-f",
            "input.mk",
            "EFFECT=$(shell touch startup.marker)",
            "all",
        ],
        None,
    );
    run_prelaunch_refusal(
        &program,
        correct_digest.as_str(),
        &valid,
        Some(("MAKEFILES", "preloaded.mk")),
    );
}

#[test]
fn launched_make_failure_retains_private_stage_data_without_publishing_or_leaking() {
    let executable_temporary = tempfile::tempdir().expect("temporary fake executable root");
    let (program, digest) = fake_make(&executable_temporary);
    let temporary = tempfile::tempdir().expect("temporary failed observation root");
    let root = private_root(&temporary);
    let environment = launch_environment(&root);
    let output = observe_command(
        &root,
        &root,
        &program,
        digest.as_str(),
        &["-f", "input.mk", "all"],
        &environment,
    );

    assert_refused(&output);
    assert!(
        root.join("launched.marker").exists(),
        "selected executable was not launched"
    );
    assert!(
        !root.join("observation.json").exists(),
        "failed process published an observation"
    );
    assert!(
        !root.join("observation-selection.json").exists(),
        "failed process published a selection"
    );
    assert_private_file(&root.join("observation-started.json"));
    assert_private_file(&root.join("invocation.json"));
    assert_private_file(&root.join("discovery.stdout"));
    assert_private_file(&root.join("discovery.stderr"));
    assert_private_file(&root.join("discovery.status.json"));
    assert_status(&root.join("discovery.status.json"), 23);
    let retained_stderr =
        fs::read(root.join("discovery.stderr")).expect("read private child stderr");
    assert!(
        retained_stderr
            .windows(b"raw-process-diagnostic-sentinel".len())
            .any(|window| window == b"raw-process-diagnostic-sentinel"),
        "private child stderr was not retained"
    );
    assert!(
        !output
            .stderr
            .windows(b"raw-process-diagnostic-sentinel".len())
            .any(|window| window == b"raw-process-diagnostic-sentinel"),
        "raw child diagnostic reached the entrypoint stderr"
    );
}

#[test]
fn frontend_signals_cancel_and_reap_the_separate_observer_process_group() {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };
    for signal in ["-TERM", "-INT"] {
        let temporary = tempfile::tempdir().expect("signal cancellation fixture");
        let root = private_root(&temporary);
        let program = root.join("waiting-observer");
        fs::write(
            &program,
            b"#!/bin/sh\nprintf '%s' \"$$\" > \"$NATIVE_TEST_PID.tmp\" || exit 1\n/bin/mv \"$NATIVE_TEST_PID.tmp\" \"$NATIVE_TEST_PID\" || exit 1\nexec /bin/sleep 30\n",
        )
        .expect("write owned waiting observer");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700))
            .expect("executable observer mode");
        let digest = measure_executable(&program);
        let pid_file = root.join("observer.pid");
        let context = context_digest();
        let mut child = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
            .env_clear()
            .env("NATIVE_TEST_PID", &pid_file)
            .current_dir(&root)
            .arg("--observe-make-invocation")
            .arg(&root)
            .arg(&program)
            .arg(digest.as_str())
            .arg(context.as_str())
            .arg("--")
            .args(["-f", "never-read.mk", "all"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start owned observation frontend");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pid_file.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().expect("inspect live frontend").is_none(),
                "frontend stopped before its observed process launched"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            pid_file.exists(),
            "owned observer did not launch before the deadline"
        );
        let pid_text = fs::read_to_string(&pid_file).expect("read owned observer pid");
        let observer_pid = pid_text
            .parse::<u32>()
            .expect("owned observer pid is numeric");
        assert!(
            observer_pid > 1,
            "owned observer did not provide a process pid"
        );
        let delivered = Command::new("/bin/kill")
            .arg(signal)
            .arg(child.id().to_string())
            .output()
            .expect("signal owned frontend");
        assert!(
            delivered.status.success(),
            "signal was not delivered to owned frontend"
        );
        let output = child.wait_with_output().expect("reap cancelled frontend");
        assert_refused(&output);
        let probe = Command::new("/bin/kill")
            .arg("-0")
            .arg(observer_pid.to_string())
            .output()
            .expect("inspect owned observer termination");
        assert!(
            !probe.status.success(),
            "cancelled frontend left its observer process running"
        );
        assert!(
            !root.join("observation.json").exists(),
            "cancelled process published observation data"
        );
        let status: Value = serde_json::from_slice(
            &fs::read(root.join("discovery.status.json")).expect("read cancelled private status"),
        )
        .expect("parse cancellation status");
        assert_eq!(status["cancelled"].as_bool(), Some(true));
    }
}
