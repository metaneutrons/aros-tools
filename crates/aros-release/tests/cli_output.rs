use std::fs;
use std::process::{Command, Stdio};

#[test]
fn help_treats_an_early_closed_pipe_as_successful_delivery() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aros-release"))
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Drop the only reader before waiting, forcing the CLI through the
    // BrokenPipe branch instead of relying on scheduler timing in a shell
    // pipeline such as `aros-release --help | head`.
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}

#[test]
fn logging_precedence_distinguishes_file_only_from_explicit_off() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("missing.tar.zst");
    let manifest = directory.path().join("missing.json");
    let run = |log: &std::path::Path, level: Option<&str>, environment_off: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros-release"));
        command
            .args(["--log-file"])
            .arg(log)
            .args(["--log-format=jsonl", "verify", "--archive"])
            .arg(&archive)
            .args(["--manifest"])
            .arg(&manifest);
        if let Some(level) = level {
            command.arg(format!("--log-level={level}"));
        }
        if environment_off {
            command.env("AROS_RELEASE_LOG_LEVEL", "off");
        }
        command.output().unwrap()
    };
    let file_only = directory.path().join("file-only.jsonl");
    assert!(!run(&file_only, None, false).status.success());
    assert!(file_only.is_file());
    assert!(fs::read_to_string(&file_only)
        .unwrap()
        .contains("invocation.start"));

    let explicit_off = directory.path().join("explicit-off.jsonl");
    assert!(!run(&explicit_off, Some("off"), false).status.success());
    assert!(!explicit_off.exists());

    let environment_off = directory.path().join("environment-off.jsonl");
    assert!(!run(&environment_off, None, true).status.success());
    assert!(!environment_off.exists());
}
