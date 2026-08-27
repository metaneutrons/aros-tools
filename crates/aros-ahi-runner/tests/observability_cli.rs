use std::fs;
use std::process::Command;

#[test]
fn arbitrary_cmake_is_reported_as_one_versioned_json_diagnostic() {
    let directory = tempfile::tempdir().unwrap();
    let contract = directory.path().join("contract.cmake");
    fs::write(&contract, "message(FATAL_ERROR injected)\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aros-ahi-runner"))
        .arg("--contract")
        .arg(&contract)
        .arg("--validate-only")
        .arg("--diagnostic-format=json")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(value["diagnostics"][0]["code"], "AH0101");
    assert_eq!(value["diagnostics"][0]["stage"], "ahi_contract_parsing");
}

#[test]
fn local_jsonl_log_is_separate_from_json_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    let contract = directory.path().join("contract.cmake");
    let log = directory.path().join("ahi.jsonl");
    fs::write(&contract, "message(FATAL_ERROR injected)\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_aros-ahi-runner"))
        .arg("--contract")
        .arg(&contract)
        .arg("--validate-only")
        .arg("--diagnostic-format=json")
        .arg("--log-level=info")
        .arg("--log-format=jsonl")
        .arg("--log-file")
        .arg(&log)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let diagnostic: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["diagnostics"][0]["code"], "AH0101");
    let records: Vec<serde_json::Value> = fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["schema"], "aros-ahi-runner-log-v1");
    assert_eq!(records[0]["event"], "invocation.start");
    assert_eq!(records[1]["event"], "diagnostic");
    assert_eq!(records[1]["diagnostic_code"], "AH0101");
}
