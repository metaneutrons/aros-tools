use std::fs;
use std::process::Command;

const fn collector() -> &'static str {
    env!("CARGO_BIN_EXE_aros-collect")
}

#[test]
fn direct_mode_emits_one_versioned_json_diagnostic_document() {
    let output = Command::new(collector())
        .arg("--diagnostic-format=json")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(value["diagnostics"][0]["code"], "AC0001");
    assert_eq!(value["diagnostics"][0]["stage"], "invocation");
}

#[test]
fn json_selection_also_applies_to_later_observability_option_errors() {
    let output = Command::new(collector())
        .arg("--diagnostic-format=json")
        .arg("--log-level=unsupported")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(value["diagnostics"][0]["code"], "AC0001");
}

#[test]
fn jsonl_logging_is_local_structured_and_separate_from_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    let log = directory.path().join("collector.jsonl");
    let missing_linker = directory.path().join("missing-ld.lld");
    let linked_output = directory.path().join("output.o");
    let output = Command::new(collector())
        .arg("--diagnostic-format=json")
        .arg("--log-level=info")
        .arg("--log-format=jsonl")
        .arg("--log-file")
        .arg(&log)
        .arg("--ld")
        .arg(&missing_linker)
        .arg("--")
        .arg("-o")
        .arg(&linked_output)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["diagnostics"][0]["code"], "AC0301");

    let lines = fs::read_to_string(log).unwrap();
    let records: Vec<serde_json::Value> = lines
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema"], "aros-collect-log-v1");
    assert_eq!(records[0]["event"], "invocation.start");
    assert_eq!(records[1]["event"], "diagnostic");
    assert_eq!(records[1]["diagnostic_code"], "AC0301");
    assert_eq!(records[1]["diagnostic_stage"], "first_link");
    assert!(records
        .iter()
        .all(|record| record.get("timestamp").is_none()));
}

#[cfg(unix)]
#[test]
fn collect_aros_alias_uses_the_same_json_diagnostic_contract() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let alias = directory.path().join("collect-aros");
    fs::copy(collector(), &alias).unwrap();
    fs::set_permissions(&alias, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(directory.path().join("ld.lld"), b"").unwrap();
    fs::write(directory.path().join("llvm-strip"), b"").unwrap();

    let output = Command::new(alias)
        .arg("--diagnostic-format=json")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(value["diagnostics"][0]["code"], "AC0001");
    assert_eq!(value["diagnostics"][0]["stage"], "invocation");
}
