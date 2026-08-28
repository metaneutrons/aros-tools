use std::fs;
use std::process::{Command, Output};

const fn aros() -> &'static str {
    env!("CARGO_BIN_EXE_aros")
}

fn command() -> Command {
    let mut command = Command::new(aros());
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env_remove("AROS_DIAGNOSTIC_FORMAT")
        .env_remove("AROS_LOG_LEVEL")
        .env_remove("AROS_LOG_FORMAT")
        .env_remove("AROS_LOG_FILE");
    command
}

fn json(output: &Output) -> serde_json::Value {
    assert!(!output.status.success());
    serde_json::from_slice(&output.stderr).unwrap()
}

#[test]
fn invalid_invocation_is_one_versioned_json_diagnostic() {
    let output = command()
        .arg("--diagnostic-format=json")
        .arg("--definitely-invalid")
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(value["diagnostics"][0]["code"], "AR0001");
    assert_eq!(value["diagnostics"][0]["stage"], "invocation");
}

#[test]
fn enabled_logging_without_a_file_is_an_observability_error() {
    let output = command()
        .args(["--diagnostic-format=json", "--log-level=info", "info"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0002");
    assert_eq!(value["diagnostics"][0]["stage"], "observability");
}

#[test]
fn repository_discovery_has_its_own_stable_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .args(["--diagnostic-format=json", "info"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0101");
    assert_eq!(value["diagnostics"][0]["stage"], "repository_discovery");
    assert_eq!(value["diagnostics"][0]["context"]["mode"], "info");
}

#[test]
fn command_failure_and_local_jsonl_log_are_structured_and_separate() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("aros.jsonl");
    let output = command()
        .args([
            "setup",
            "--local",
            "/tmp",
            "--diagnostic-format=json",
            "--log-format=jsonl",
            "--log-file",
        ])
        .arg(&log)
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0401");
    assert_eq!(value["diagnostics"][0]["context"]["mode"], "setup");

    let records: Vec<serde_json::Value> = fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema"], "aros-cli-log-v1");
    assert_eq!(records[0]["event"], "invocation.start");
    assert_eq!(records[1]["event"], "diagnostic");
    assert_eq!(records[1]["diagnostic_code"], "AR0401");
    assert!(records
        .iter()
        .all(|record| record.get("timestamp").is_none()));
}

#[test]
fn help_succeeds_without_repository_discovery() {
    let directory = tempfile::tempdir().unwrap();
    let output = command()
        .current_dir(directory.path())
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("OBSERVABILITY:"));
}

#[test]
fn board_is_the_only_physical_board_command() {
    let board = command().args(["board", "--help"]).output().unwrap();
    assert!(board.status.success());
    assert!(board.stderr.is_empty());

    let pi = command()
        .args(["--diagnostic-format=json", "pi", "--help"])
        .output()
        .unwrap();
    assert!(pi.stdout.is_empty());
    let value = json(&pi);
    assert_eq!(value["diagnostics"][0]["code"], "AR0001");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("unrecognized subcommand 'pi'"));
}

#[test]
fn clean_rejects_a_preset_path_before_touching_the_filesystem() {
    let output = command()
        .args([
            "clean",
            "--preset",
            "../outside",
            "--diagnostic-format=json",
        ])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0901");
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid CMake preset"));
}

#[cfg(unix)]
#[test]
fn child_exit_status_is_preserved_as_structured_context() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let tool = directory.path().join("sccache");
    fs::write(&tool, "#!/bin/sh\necho raw-child-error >&2\nexit 23\n").unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let output = command()
        .env("PATH", directory.path())
        .args(["ccache", "--clear", "--diagnostic-format=json"])
        .output()
        .unwrap();

    assert!(output.stdout.is_empty());
    let value = json(&output);
    assert_eq!(value["diagnostics"][0]["code"], "AR0301");
    assert_eq!(value["diagnostics"][0]["context"]["tool"], "sccache");
    assert_eq!(value["diagnostics"][0]["context"]["exit_code"], 23);
    assert!(value["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("stderr:\nraw-child-error"));
}
