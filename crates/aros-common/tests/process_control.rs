//! Real process-boundary tests for cancellation, distinct from timeout/status.

#![cfg(unix)]

use std::fs;
use std::io;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use aros_common::{exit_signal, run_output_with_control, CancellationToken};

#[test]
fn cancellation_is_shared_one_way_and_operation_local() {
    let token = CancellationToken::default();
    let other_operation = CancellationToken::default();
    assert!(!token.is_cancelled());
    let other_thread = token.clone();
    thread::spawn(move || other_thread.cancel()).join().unwrap();
    token.cancel();
    assert!(token.is_cancelled());
    assert!(!other_operation.is_cancelled());
}

#[test]
fn cancelled_operation_never_spawns_even_a_valid_command() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("unexpected spawn");
    let token = CancellationToken::default();
    token.cancel();
    let error = run_output_with_control(
        Command::new("sh")
            .args(["-c", ": > \"$1\"", "fixture"])
            .arg(&marker),
        256,
        Duration::from_secs(1),
        &token,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
    assert!(!marker.exists());
}

#[test]
fn invalid_deadline_and_capture_limit_do_not_spawn() {
    for (limit, timeout) in [(0, Duration::from_secs(1)), (256, Duration::MAX)] {
        let error = run_output_with_control(
            &mut Command::new("aros-nonexistent-test-process"),
            limit,
            timeout,
            &CancellationToken::default(),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}

#[test]
fn missing_program_is_a_spawn_error_not_cancellation() {
    let error = run_output_with_control(
        &mut Command::new("aros-nonexistent-test-process"),
        256,
        Duration::from_secs(1),
        &CancellationToken::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn success_and_failure_keep_exact_output_without_claiming_cancellation() {
    for status in [0, 7] {
        let result = run_output_with_control(
            Command::new("sh")
                .args([
                    "-c",
                    "printf result; printf detail >&2; exit \"$1\"",
                    "fixture",
                ])
                .arg(status.to_string()),
            256,
            Duration::from_secs(5),
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(result.status.code(), Some(status));
        assert_eq!(result.stdout.exact_bytes(), Some(b"result".as_slice()));
        assert_eq!(result.stderr.exact_bytes(), Some(b"detail".as_slice()));
        assert!(!result.cancelled);
        assert!(!result.timed_out);
    }
}

#[test]
fn external_signal_is_not_a_cancellation_request() {
    let result = run_output_with_control(
        Command::new("sh").args(["-c", "kill -TERM $$"]),
        256,
        Duration::from_secs(5),
        &CancellationToken::default(),
    )
    .unwrap();
    assert_eq!(exit_signal(result.status), Some(15));
    assert!(!result.cancelled);
    assert!(!result.timed_out);
}

#[test]
fn timeout_is_distinct_from_cancellation() {
    let token = CancellationToken::default();
    let result = run_output_with_control(
        Command::new("sh").args(["-c", "sleep 30"]),
        256,
        Duration::from_millis(100),
        &token,
    )
    .unwrap();
    assert!(result.timed_out);
    assert!(!result.cancelled);
    assert!(!token.is_cancelled());
    assert_eq!(exit_signal(result.status), Some(9));
    assert!(result.elapsed < Duration::from_secs(5));
}

#[test]
fn cancellation_reaps_child_and_closes_inherited_descendant_pipes() {
    let root = tempfile::tempdir().unwrap();
    let ready = root.path().join("ready");
    let pid = root.path().join("pid");
    let token = CancellationToken::default();
    let cancel = token.clone();
    let cancel_thread = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let observed_ready = ready.exists();
        cancel.cancel();
        observed_ready
    });
    let result = run_output_with_control(
        Command::new("sh")
            .args(["-c", "printf '%s' \"$$\" > \"$1\"; sleep 30 & printf READY; printf ERROR >&2; : > \"$2\"; wait", "fixture"])
            .arg(&pid)
            .arg(root.path().join("ready")),
        256,
        Duration::from_secs(10),
        &token,
    );
    assert!(
        cancel_thread.join().unwrap(),
        "fixture must reach the running boundary"
    );
    let result = result.unwrap();
    assert!(result.cancelled);
    assert!(!result.timed_out);
    assert_eq!(exit_signal(result.status), Some(9));
    assert_eq!(result.stdout.exact_bytes(), Some(b"READY".as_slice()));
    assert_eq!(result.stderr.exact_bytes(), Some(b"ERROR".as_slice()));
    // A live sleep still owning these pipes would prevent capture completion.
    assert!(result.elapsed < Duration::from_secs(8));
    let pid = fs::read_to_string(pid).unwrap().parse().unwrap();
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    assert_eq!(
        rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::NOHANG).unwrap_err(),
        rustix::io::Errno::CHILD,
        "runner must already have reaped its direct child"
    );
}

#[test]
fn cancellation_keeps_bounded_noisy_output() {
    let root = tempfile::tempdir().unwrap();
    let ready = root.path().join("ready");
    let token = CancellationToken::default();
    let cancel = token.clone();
    let cancel_thread = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let observed_ready = ready.exists();
        cancel.cancel();
        observed_ready
    });
    let result = run_output_with_control(
        Command::new("sh")
            .args([
                "-c",
                "printf HEAD; yes x | head -c 131072; printf TAIL; : > \"$1\"; sleep 30",
                "fixture",
            ])
            .arg(root.path().join("ready")),
        1024,
        Duration::from_secs(10),
        &token,
    );
    assert!(cancel_thread.join().unwrap());
    let result = result.unwrap();
    assert!(result.cancelled);
    assert!(!result.timed_out);
    assert!(result.stdout.is_truncated());
    assert_eq!(result.stdout.total_bytes(), 131_080);
    let mut rendered = Vec::new();
    result.stdout.write_rendered(&mut rendered).unwrap();
    assert!(rendered.starts_with(b"HEAD"));
    assert!(rendered.ends_with(b"TAIL"));
    assert!(rendered.len() < 1200);
}

#[test]
fn escaped_descendant_pipes_fail_boundedly_without_losing_cancellation() {
    use rustix::process::{kill_process_group, Pid, Signal};
    for mode in ["cancel", "timeout", "exited", "input", "noisy"] {
        let root = tempfile::tempdir().unwrap();
        let pid_path = root.path().join("escaped-pid");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "escaped_pipe_fixture", "--nocapture"])
            .env("AROS_PROCESS_TEST_ESCAPE_PID", &pid_path)
            .env("AROS_PROCESS_TEST_ESCAPE_STYLE", mode);
        let token = CancellationToken::default();
        let cancel = token.clone();
        let cancel_pid = pid_path.clone();
        let cancel_thread = thread::spawn(move || {
            if mode == "cancel" {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !cancel_pid.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
                cancel.cancel();
            }
        });
        let started = Instant::now();
        let result = if mode == "input" {
            aros_common::run_output_with_input(&mut command, &vec![b'x'; 1024 * 1024], 256)
        } else {
            run_output_with_control(&mut command, 256, Duration::from_secs(2), &token)
        };
        cancel_thread.join().unwrap();
        // Only this fixture's explicitly recorded, separately created process
        // group is killed here. The runner must not claim to have contained it.
        let pid = Pid::from_raw(fs::read_to_string(&pid_path).unwrap().parse().unwrap()).unwrap();
        let cleanup = kill_process_group(pid, Signal::KILL);
        assert!(
            cleanup.is_ok() || cleanup == Err(rustix::io::Errno::SRCH),
            "{mode}: {cleanup:?}"
        );
        let error = result.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(8), "{mode}");
        assert_eq!(
            error.kind(),
            if mode == "cancel" {
                io::ErrorKind::Interrupted
            } else {
                io::ErrorKind::TimedOut
            },
            "{mode}: {error}"
        );
        assert!(error.to_string().contains("pipe"));
        if mode == "cancel" {
            assert!(error.to_string().contains("cancelled"));
        }
    }
}

#[test]
#[allow(
    clippy::zombie_processes,
    reason = "outer fixture test terminates this deliberately escaped group after proving bounded pipe cleanup"
)]
fn escaped_pipe_fixture() {
    use std::os::unix::process::CommandExt as _;
    let Some(pid_path) = std::env::var_os("AROS_PROCESS_TEST_ESCAPE_PID") else {
        return;
    };
    let style = std::env::var("AROS_PROCESS_TEST_ESCAPE_STYLE").unwrap();
    // Stay alive after readers close so the outer test explicitly cleans up its
    // group; Darwin may return EPERM for an already-dead zombie-only group.
    let script = if style == "noisy" {
        "trap '' PIPE; yes x 2>/dev/null; sleep 30"
    } else {
        "sleep 30"
    };
    let child = Command::new("sh")
        .args(["-c", script])
        .process_group(0)
        .spawn()
        .unwrap();
    fs::write(pid_path, child.id().to_string()).unwrap();
    if matches!(style.as_str(), "cancel" | "timeout") {
        thread::sleep(Duration::from_secs(30));
    }
    // The outer test cleans up this deliberately escaped group after probing
    // the runner; waiting here would invalidate the boundary being tested.
}
