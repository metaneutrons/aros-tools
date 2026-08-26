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
