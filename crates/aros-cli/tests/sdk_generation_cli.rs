//! Read-only SDK generation frontend boundaries, not release qualification.

#![cfg(unix)]

use std::fs;
use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aros"))
}

#[test]
fn generation_help_exposes_independent_selection_without_execution_options() {
    let output = cli()
        .args(["sdk", "verify-generation", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for option in [
        "--inputs-dir",
        "--inputs-sha256",
        "--sdk-id",
        "--compiler-group",
        "--compiler-profile",
        "--host",
        "--release-id",
        "--selection",
        "--selection-sha256",
        "--forbidden-prefix",
    ] {
        assert!(help.contains(option), "missing selector {option}: {help}");
    }
    for option in [
        "--jobs",
        "--source-dir",
        "--cache-dir",
        "--resume",
        "--apply",
    ] {
        assert!(
            !help.contains(option),
            "unexpected execution option {option}"
        );
    }
}

#[test]
fn missing_generation_selectors_fail_before_filesystem_access() {
    let output = cli().args(["sdk", "verify-generation"]).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("--selection-sha256"));
}

#[test]
fn invalid_generation_inputs_cannot_execute_a_compiler_or_modify_inputs() {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let inputs = root.join("inputs");
    let programs = root.join("programs");
    fs::create_dir(&inputs).unwrap();
    fs::create_dir(&programs).unwrap();
    let marker = root.join("compiler-executed");
    for name in ["cc", "c++", "gcc", "clang", "cmake", "ninja", "make"] {
        let program = programs.join(name);
        fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf executed > '{}'\nexit 99\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let counterprobe = Command::new(programs.join("cc"))
        .env("PATH", &programs)
        .output()
        .unwrap();
    assert_eq!(counterprobe.status.code(), Some(99));
    assert_eq!(fs::read(&marker).unwrap(), b"executed");
    fs::remove_file(&marker).unwrap();
    let document = inputs.join("toolchain-release-inputs-v2.json");
    let original = b"{\"schema\":\"invalid-source-inputs\"}\n";
    fs::write(&document, original).unwrap();
    let selection = root.join("selection.json");
    fs::write(&selection, b"{}\n").unwrap();
    let output = cli()
        .args(["sdk", "verify-generation", "--inputs-dir"])
        .arg(&inputs)
        .args([
            "--inputs-sha256",
            aros_common::sha256_bytes(original).as_str(),
        ])
        .args(["--sdk-id", "example-sdk", "--compiler-group", "llvm"])
        .args(["--compiler-profile", "example", "--host", "linux-x86_64"])
        .args(["--release-id", "diagnostic", "--selection"])
        .arg(&selection)
        .args([
            "--selection-sha256",
            aros_common::sha256_bytes(b"{}\n").as_str(),
        ])
        .arg("--forbidden-prefix")
        .arg(root.join("forbidden-build-root"))
        .env("PATH", &programs)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!marker.exists());
    assert_eq!(fs::read(&document).unwrap(), original);
    assert_eq!(fs::read(&selection).unwrap(), b"{}\n");
    assert_eq!(fs::read_dir(&inputs).unwrap().count(), 1);
}
