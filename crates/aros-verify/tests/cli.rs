//! Process-boundary tests for the stable verifier diagnostics contract.

use std::process::Command;

fn json_failure(arguments: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_aros-verify"))
        .args(arguments)
        .output()
        .expect("run aros-verify");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).expect("one JSON diagnostic document")
}

#[test]
fn invalid_invocation_is_a_stable_json_document() {
    let document = json_failure(&["--diagnostic-format", "json", "--unknown-option"]);
    assert_eq!(document["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(document["diagnostics"][0]["code"], "AV0001");
    assert_eq!(document["diagnostics"][0]["stage"], "invocation");
    assert!(document["diagnostics"][0]["hint"]
        .as_str()
        .is_some_and(|hint| !hint.trim().is_empty()));
}

#[test]
fn missing_source_is_a_stable_input_diagnostic() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing-source");
    let work = directory.path().join("work");
    let generated = directory.path().join("generated-targets.cmake");
    let document = json_failure(&[
        "--diagnostic-format",
        "json",
        "--source",
        missing.to_str().unwrap(),
        "--generated",
        generated.to_str().unwrap(),
        "--work",
        work.to_str().unwrap(),
    ]);
    assert_eq!(document["diagnostics"][0]["code"], "AV0101");
    assert_eq!(document["diagnostics"][0]["stage"], "repository_discovery");
}

#[cfg(unix)]
#[test]
fn unsafe_mmakefile_node_is_a_path_scoped_source_walk_diagnostic() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let work = directory.path().join("work");
    let generated = directory.path().join("generated-targets.cmake");
    std::fs::create_dir(&source).unwrap();
    let target = source.join("target");
    std::fs::write(&target, "%build_prog mmake=hidden\n").unwrap();
    let mmakefile = source.join("mmakefile.src");
    symlink(&target, &mmakefile).unwrap();

    let document = json_failure(&[
        "--diagnostic-format",
        "json",
        "--source",
        source.to_str().unwrap(),
        "--generated",
        generated.to_str().unwrap(),
        "--work",
        work.to_str().unwrap(),
    ]);
    assert_eq!(document["diagnostics"][0]["code"], "AV0101");
    assert_eq!(document["diagnostics"][0]["stage"], "source_walk");
    assert_eq!(
        document["diagnostics"][0]["location"]["path"],
        source
            .canonicalize()
            .unwrap()
            .join("mmakefile.src")
            .display()
            .to_string()
    );
    assert!(document["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("without following links"));
}

#[test]
fn logging_requires_an_explicit_destination() {
    let document = json_failure(&[
        "--diagnostic-format",
        "json",
        "--log-level",
        "info",
        "--source",
        ".",
        "--generated",
        "missing",
        "--work",
        "missing",
    ]);
    assert_eq!(document["diagnostics"][0]["code"], "AV0002");
    assert_eq!(document["diagnostics"][0]["stage"], "observability");
}

#[test]
fn version_matches_the_workspace_package() {
    let output = Command::new(env!("CARGO_BIN_EXE_aros-verify"))
        .arg("--version")
        .output()
        .expect("run aros-verify --version");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("aros-verify {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}
