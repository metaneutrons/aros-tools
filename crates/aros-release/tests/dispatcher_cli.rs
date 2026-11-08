use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn release() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aros-release"));
    // The command surface must never reach a provider in these tests.
    for key in ["GH_TOKEN", "GH_HOST", "GITHUB_TOKEN", "GITHUB_EVENT_PATH"] {
        command.env_remove(key);
    }
    command
}

#[test]
fn every_dispatcher_command_has_help_text() {
    let group = release().args(["dispatcher", "--help"]).output().unwrap();
    assert!(group.status.success());
    let text = String::from_utf8_lossy(&group.stdout);
    for name in ["select", "verify-claim", "dispatch", "verify-binding"] {
        assert!(text.contains(name), "{name} missing from the group help");
        let help = release()
            .args(["dispatcher", name, "--help"])
            .output()
            .unwrap();
        assert!(help.status.success(), "{name} --help");
        assert!(
            String::from_utf8_lossy(&help.stdout).contains("Usage:"),
            "{name}"
        );
    }
    let dispatch = release()
        .args(["dispatcher", "dispatch", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&dispatch.stdout).contains("aros-toolchains#115"));
}

#[test]
fn select_without_the_job_token_fails_closed_and_writes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let output = release()
        .args(["dispatcher", "select", "--output-dir"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("AP0"));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn select_refuses_a_non_private_output_directory_before_any_provider_call() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let output = release()
        .args(["dispatcher", "select", "--output-dir"])
        .arg(directory.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("mode-0700"));
}

#[test]
fn dispatch_refuses_missing_inputs_and_unknown_arguments() {
    let output = release().args(["dispatcher", "dispatch"]).output().unwrap();
    assert!(!output.status.success());
    let missing = release()
        .args([
            "dispatcher",
            "dispatch",
            "--record",
            "/nonexistent/a",
            "--bundle",
            "/nonexistent/b",
            "--claim",
            "/nonexistent/c",
            "--claim-asset",
            "/nonexistent/d",
            "--claim-bundle-sha256",
            &"0".repeat(64),
            "--source-context",
            "/nonexistent/e",
            "--dispatcher-context",
            "/nonexistent/f",
            "--dispatcher-job",
            "/nonexistent/g",
        ])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("AP0201"));
    let unknown = release()
        .args(["dispatcher", "dispatch", "--retry", "3"])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
}

#[test]
fn verify_commands_require_every_independent_selector() {
    for command in ["verify-claim", "verify-binding"] {
        let output = release().args(["dispatcher", command]).output().unwrap();
        assert!(!output.status.success(), "{command}");
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(
            text.contains("--record") && text.contains("--source-context"),
            "{command}: {text}"
        );
    }
}
