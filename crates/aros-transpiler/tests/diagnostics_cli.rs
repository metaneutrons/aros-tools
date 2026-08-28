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
