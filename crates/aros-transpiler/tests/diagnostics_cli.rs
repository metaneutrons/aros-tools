use serde_json::Value;
use std::fs;
use std::process::Command;

#[test]
fn json_diagnostics_are_versioned_structured_and_checkout_independent() {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir(source.path().join("mmakefile.src")).unwrap();
    let output = source.path().join("out/generated.cmake");

    let result = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
        .arg("--source-dir")
        .arg(source.path())
        .arg("--output")
        .arg(output)
        .arg("--diagnostic-format")
        .arg("json")
        .output()
        .unwrap();

    assert!(!result.status.success());
    let stderr = String::from_utf8(result.stderr).unwrap();
    let document: Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(document["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(document["diagnostics"][0]["code"], "AT0003");
    assert_eq!(document["diagnostics"][0]["severity"], "error");
    assert_eq!(
        document["diagnostics"][0]["location"]["path"],
        "mmakefile.src"
    );
    assert!(!stderr.contains(source.path().to_string_lossy().as_ref()));
}

#[test]
fn invalid_invocation_uses_the_shared_json_contract() {
    let result = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
        .arg("--diagnostic-format=json")
        .arg("--not-a-real-option")
        .output()
        .unwrap();

    assert!(!result.status.success());
    let document: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(document["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(document["diagnostics"][0]["code"], "AT0008");
    assert_eq!(document["diagnostics"][0]["stage"], "invocation");
}

#[test]
fn enabled_logging_without_a_file_fails_closed() {
    let result = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
        .arg("--diagnostic-format=json")
        .arg("--log-level=info")
        .output()
        .unwrap();

    assert!(!result.status.success());
    let document: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(document["diagnostics"][0]["code"], "AT0009");
    assert_eq!(document["diagnostics"][0]["stage"], "observability");
}

#[test]
fn successful_invocation_writes_shared_jsonl_logs() {
    let source = tempfile::tempdir().unwrap();
    let output = source.path().join("generated.cmake");
    let log = source.path().join("transpiler.jsonl");
    let result = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
        .arg("--source-dir")
        .arg(source.path())
        .arg("--output")
        .arg(&output)
        .arg("--log-level=info")
        .arg("--log-format=jsonl")
        .arg("--log-file")
        .arg(&log)
        .output()
        .unwrap();

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let records = fs::read_to_string(log).unwrap();
    let parsed: Vec<Value> = records
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(parsed.first().unwrap()["schema"], "aros-transpiler-log-v1");
    assert_eq!(parsed.first().unwrap()["event"], "invocation.start");
    assert_eq!(parsed.last().unwrap()["event"], "invocation.complete");
}

#[test]
fn logging_precedence_distinguishes_file_only_from_explicit_off() {
    let source = tempfile::tempdir().unwrap();
    let run = |log: &std::path::Path, level: Option<&str>, environment_off: bool| {
        let output = source.path().join(format!(
            "{}.cmake",
            log.file_name().unwrap().to_string_lossy()
        ));
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"));
        command
            .args(["--source-dir"])
            .arg(source.path())
            .args(["--output"])
            .arg(output)
            .args(["--log-file"])
            .arg(log)
            .arg("--log-format=jsonl");
        if let Some(level) = level {
            command.arg(format!("--log-level={level}"));
        }
        if environment_off {
            command.env("AROS_TRANSPILER_LOG_LEVEL", "off");
        }
        command.output().unwrap()
    };
    let file_only = source.path().join("file-only.jsonl");
    assert!(run(&file_only, None, false).status.success());
    assert!(file_only.is_file());

    let explicit_off = source.path().join("explicit-off.jsonl");
    assert!(run(&explicit_off, Some("off"), false).status.success());
    assert!(!explicit_off.exists());

    let environment_off = source.path().join("environment-off.jsonl");
    assert!(run(&environment_off, None, true).status.success());
    assert!(!environment_off.exists());
}
