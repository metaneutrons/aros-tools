use std::fs;
use std::process::{Command, Stdio};

fn diagnostic_code(result: &std::process::Output) -> String {
    let document: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(document["schema"], "aros-tool-diagnostics-v1");
    document["diagnostics"][0]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn invocation_and_observability_failures_use_stable_codes() {
    let invalid = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "--unknown-option"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert_eq!(diagnostic_code(&invalid), "RM0001");

    let missing_log_file = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args([
            "--diagnostic-format",
            "json",
            "--log-level",
            "info",
            "pkg",
            "list",
            "unused.pkg",
        ])
        .output()
        .unwrap();
    assert!(!missing_log_file.status.success());
    assert_eq!(diagnostic_code(&missing_log_file), "RM0002");

    let directory = tempfile::tempdir().unwrap();
    let missing_input = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "pkg", "create", "--output"])
        .arg(directory.path().join("output.pkg"))
        .arg(directory.path().join("missing.elf"))
        .output()
        .unwrap();
    assert!(!missing_input.status.success());
    assert_eq!(diagnostic_code(&missing_input), "RM0101");

    let malformed = directory.path().join("malformed.pkg");
    fs::write(&malformed, b"not a package").unwrap();
    let invalid_package = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "pkg", "list"])
        .arg(&malformed)
        .output()
        .unwrap();
    assert!(!invalid_package.status.success());
    assert_eq!(diagnostic_code(&invalid_package), "RM0201");
}

#[test]
fn invalid_member_fails_with_stable_json_and_preserves_destination() {
    let directory = tempfile::tempdir().unwrap();
    let member = directory.path().join("broken.bin");
    let output = directory.path().join("kickstart.pkg");
    fs::write(&member, b"not an ELF object").unwrap();
    fs::write(&output, b"existing package sentinel").unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "pkg", "create", "--output"])
        .arg(&output)
        .arg(&member)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(fs::read(output).unwrap(), b"existing package sentinel");
    let document: serde_json::Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(document["schema"], "aros-tool-diagnostics-v1");
    assert_eq!(document["diagnostics"][0]["code"], "RM0201");
    assert_eq!(document["diagnostics"][0]["stage"], "integrity_validation");
}

#[test]
fn publication_failure_is_nonzero_and_keeps_existing_target() {
    let directory = tempfile::tempdir().unwrap();
    let member = directory.path().join("exec.library");
    let output = directory.path().join("kickstart.pkg");
    fs::write(&member, b"\x7fELFpayload").unwrap();
    fs::create_dir(&output).unwrap();
    fs::write(output.join("sentinel"), b"existing target").unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "pkg", "create", "--output"])
        .arg(&output)
        .arg(&member)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "RM0301");
    assert_eq!(
        fs::read(output.join("sentinel")).unwrap(),
        b"existing target"
    );
}

#[test]
fn extract_preflights_every_member_and_never_partially_overwrites() {
    let directory = tempfile::tempdir().unwrap();
    let exec = directory.path().join("exec.library");
    let dos = directory.path().join("dos.library");
    let package = directory.path().join("kickstart.pkg");
    let output = directory.path().join("extracted");
    fs::write(&exec, b"\x7fELFexec").unwrap();
    fs::write(&dos, b"\x7fELFdos").unwrap();
    let create = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["pkg", "create", "--basename", "--output"])
        .arg(&package)
        .args([&exec, &dos])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    fs::create_dir(&output).unwrap();
    fs::write(output.join("dos.library"), b"sentinel").unwrap();

    let extract = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "pkg", "extract"])
        .arg(&package)
        .args(["--directory"])
        .arg(&output)
        .output()
        .unwrap();

    assert!(!extract.status.success());
    assert_eq!(diagnostic_code(&extract), "RM0301");
    assert_eq!(fs::read(output.join("dos.library")).unwrap(), b"sentinel");
    assert!(!output.join("exec.library").exists());
}

#[test]
fn explicit_jsonl_log_uses_the_stable_schema() {
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("empty.pkg");
    let log = directory.path().join("romtool.jsonl");
    let create = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--log-level", "info", "--log-format", "jsonl", "--log-file"])
        .arg(&log)
        .args(["pkg", "create", "--output"])
        .arg(&package)
        .arg("--allow-non-elf")
        .arg(directory.path().join("member"))
        .output()
        .unwrap();
    assert!(!create.status.success());

    fs::write(directory.path().join("member"), b"\x7fELFpayload").unwrap();
    let success = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--log-level", "info", "--log-format", "jsonl", "--log-file"])
        .arg(&log)
        .args(["pkg", "create", "--output"])
        .arg(&package)
        .arg(directory.path().join("member"))
        .output()
        .unwrap();
    assert!(success.status.success());

    let records: Vec<serde_json::Value> = fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[0]["schema"], "aros-romtool-log-v1");
    assert_eq!(records[0]["event"], "invocation.start");
    assert!(records
        .iter()
        .any(|record| record["event"] == "invocation.complete"));
    assert!(records[0].get("timestamp").is_none());
    assert!(records[0].get("hostname").is_none());
}

#[test]
fn version_comes_from_the_crate_package() {
    let result = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .arg("--version")
        .output()
        .unwrap();

    assert!(result.status.success());
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        format!("aros-romtool {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn create_is_no_clobber_by_default_and_digest_cas_is_explicit() {
    let directory = tempfile::tempdir().unwrap();
    let member = directory.path().join("exec.library");
    let output = directory.path().join("kickstart.pkg");
    fs::write(&member, b"\x7fELFpayload").unwrap();
    fs::write(&output, b"sentinel").unwrap();

    let refused = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "pkg", "create", "--output"])
        .arg(&output)
        .arg(&member)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"sentinel");

    let wrong = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "pkg", "create", "--output"])
        .arg(&output)
        .args(["--replace-if-sha256", &"0".repeat(64)])
        .arg(&member)
        .output()
        .unwrap();
    assert!(!wrong.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"sentinel");

    let digest = aros_common::sha256_bytes(b"sentinel").to_string();
    let replaced = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["pkg", "create", "--output"])
        .arg(&output)
        .args(["--replace-if-sha256", &digest])
        .arg(&member)
        .output()
        .unwrap();
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_ne!(fs::read(output).unwrap(), b"sentinel");
}

#[cfg(unix)]
#[test]
fn create_rejects_a_symlinked_output_parent() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let member = directory.path().join("exec.library");
    fs::write(&member, b"\x7fELFpayload").unwrap();
    symlink(outside.path(), directory.path().join("escape")).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--diagnostic-format", "json", "pkg", "create", "--output"])
        .arg(directory.path().join("escape/kickstart.pkg"))
        .arg(&member)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "RM0301");
    assert!(!outside.path().join("kickstart.pkg").exists());
}

#[cfg(unix)]
#[test]
fn interrupted_tree_publication_is_invisible_and_next_run_cleans_it() {
    let directory = tempfile::tempdir().unwrap();
    let member = directory.path().join("exec.library");
    let package = directory.path().join("kickstart.pkg");
    let destination = directory.path().join("extracted");
    let log = directory.path().join("recovery.jsonl");
    fs::write(&member, b"\x7fELFpayload").unwrap();
    let create = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["pkg", "create", "--output"])
        .arg(&package)
        .arg(&member)
        .output()
        .unwrap();
    assert!(create.status.success());

    let interrupted = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .env("AROS_PUBLICATION_TEST_CRASH_AT", "tree-before-rename")
        .args(["pkg", "extract"])
        .arg(&package)
        .args(["--directory"])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(!interrupted.status.success());
    assert!(!destination.exists());

    let recovered = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["--log-level", "warn", "--log-format", "jsonl", "--log-file"])
        .arg(&log)
        .args(["pkg", "extract"])
        .arg(&package)
        .args(["--directory"])
        .arg(&destination)
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert_eq!(
        fs::read(destination.join("exec.library")).unwrap(),
        b"\x7fELFpayload"
    );
    let leftovers: Vec<_> = fs::read_dir(directory.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains("tree-stage"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    let records: Vec<serde_json::Value> = fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(records.iter().any(|record| {
        record["event"] == "publication.recovery"
            && record["message"]
                .as_str()
                .is_some_and(|message| message.contains("removed_tree_stage"))
    }));
}

#[cfg(unix)]
#[test]
fn post_rename_sync_failure_reports_uncertain_without_deleting_extraction() {
    let directory = tempfile::tempdir().unwrap();
    let member = directory.path().join("exec.library");
    let package = directory.path().join("kickstart.pkg");
    let destination = directory.path().join("extracted");
    fs::write(&member, b"\x7fELFpayload").unwrap();
    assert!(Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["pkg", "create", "--output"])
        .arg(&package)
        .arg(&member)
        .status()
        .unwrap()
        .success());

    let result = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .env(
            "AROS_PUBLICATION_TEST_FAIL_AT",
            "tree-after-rename-before-sync",
        )
        .args(["--diagnostic-format", "json", "pkg", "extract"])
        .arg(&package)
        .args(["--directory"])
        .arg(&destination)
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert_eq!(diagnostic_code(&result), "RM0301");
    assert!(String::from_utf8_lossy(&result.stderr).contains("complete destination was retained"));
    assert_eq!(
        fs::read(destination.join("exec.library")).unwrap(),
        b"\x7fELFpayload"
    );
}

#[cfg(unix)]
#[test]
fn no_clobber_file_crashes_leave_owned_recoverable_state_not_orphans() {
    let directory = tempfile::tempdir().unwrap();
    let member = directory.path().join("exec.library");
    fs::write(&member, b"\x7fELFpayload").unwrap();

    for (index, crash_point) in [
        "after-stage-before-journal-update",
        "file-after-committed-before-rename",
    ]
    .into_iter()
    .enumerate()
    {
        let output = directory.path().join(format!("kickstart-{index}.pkg"));
        let crashed = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
            .env("AROS_PUBLICATION_TEST_CRASH_AT", crash_point)
            .args(["pkg", "create", "--output"])
            .arg(&output)
            .arg(&member)
            .output()
            .unwrap();
        assert!(!crashed.status.success(), "{crash_point}");
        assert!(!output.exists(), "{crash_point}");
        let interrupted_names: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".aros-stage-") || name.contains(".aros-file-"))
            .collect();
        assert!(!interrupted_names.is_empty(), "{crash_point}");

        let recovered = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
            .args(["pkg", "create", "--output"])
            .arg(&output)
            .arg(&member)
            .output()
            .unwrap();
        assert!(
            recovered.status.success(),
            "{crash_point}: {}",
            String::from_utf8_lossy(&recovered.stderr)
        );
        assert!(output.exists());
        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".aros-stage-") || name.contains(".aros-file-"))
            .collect();
        assert!(leftovers.is_empty(), "{crash_point}: {leftovers:?}");
    }
}

#[cfg(unix)]
#[test]
fn closed_stdout_after_commit_does_not_change_success() {
    let directory = tempfile::tempdir().unwrap();
    let member = directory.path().join("exec.library");
    let output = directory.path().join("kickstart.pkg");
    fs::write(&member, b"\x7fELFpayload").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .args(["pkg", "create", "--output"])
        .arg(&output)
        .arg(&member)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let status = child.wait().unwrap();
    assert!(status.success());
    assert!(output.exists());
}

#[cfg(unix)]
#[test]
fn closed_help_pipe_is_normal_termination_not_a_panic() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aros-romtool"))
        .arg("--help")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    assert!(child.wait().unwrap().success());
}
