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
