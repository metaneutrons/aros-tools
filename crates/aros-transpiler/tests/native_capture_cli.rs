//! Native, value-private capture boundary. No Make/Source process is launched.

#![cfg(unix)]

use aros_common::make_native_invocation::NativeMakeInvocation;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{symlink, PermissionsExt},
    },
    path::Path,
    process::Command,
};

fn command(output: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aros-transpiler"));
    command
        .env_clear()
        .current_dir(cwd)
        .args(["--capture-make-invocation"])
        .arg(output)
        .arg("--");
    command
}

fn refused(output: &std::process::Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"native Make invocation capture refused\n");
}

#[test]
fn native_capture_preserves_exact_child_data_without_logging_or_execution() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let cwd = root.join("working directory");
    fs::create_dir(&cwd).unwrap();
    let output = root.join("native.json");
    let environment = BTreeMap::from([
        ("LC_ALL".into(), "C".into()),
        (
            "HAS-DASH".into(),
            "literal-$(not executed)\nsecond line".into(),
        ),
        ("1DIGIT".into(), "a b".into()),
        ("PRIVATE_TOKEN".into(), "secret-native-sentinel".into()),
        ("AROS_TRANSPILER_LOG_LEVEL".into(), "trace".into()),
        (
            "AROS_TRANSPILER_LOG_FILE".into(),
            root.join("must-not-log").to_str().unwrap().into(),
        ),
        (
            "AROS_TRANSPILER_DIAGNOSTIC_FORMAT".into(),
            "must-not-parse".into(),
        ),
    ]);
    let arguments = [
        "-n",
        "-n",
        "TOP=literal value",
        "",
        "--help",
        "文字",
        "literal-$PATH",
    ];
    let result = command(&output, &cwd)
        .envs(&environment)
        .args(arguments)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(result.stdout.is_empty() && result.stderr.is_empty());
    let captured = NativeMakeInvocation::parse(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(captured.arguments(), arguments);
    assert_eq!(captured.working_directory(), cwd);
    assert_eq!(captured.environment(), &environment);
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    assert!(!root.join("must-not-log").exists());
    let before = fs::read(&output).unwrap();
    refused(&command(&output, &cwd).arg("changed").output().unwrap());
    assert_eq!(fs::read(&output).unwrap(), before);
}

#[test]
fn capture_refuses_public_parent_alias_and_existing_link_without_writes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let public = root.join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    refused(&command(&public.join("out"), &root).output().unwrap());
    assert!(!public.join("out").exists());
    let alias = root.join("alias");
    symlink(&root, &alias).unwrap();
    refused(&command(&alias.join("out"), &root).output().unwrap());
    assert!(!root.join("out").exists());
    let target = root.join("existing");
    fs::write(&target, b"unchanged-private-sentinel").unwrap();
    let link = root.join("link");
    symlink(&target, &link).unwrap();
    refused(&command(&link, &root).output().unwrap());
    assert_eq!(fs::read(&target).unwrap(), b"unchanged-private-sentinel");
}

#[test]
fn capture_refuses_unrepresentable_os_data_without_leaking_values() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let output = root.join("out");
    refused(
        &command(&output, &root)
            .arg(OsString::from_vec(vec![0xff]))
            .output()
            .unwrap(),
    );
    assert!(!output.exists());
    refused(
        &command(&output, &root)
            .env("PRIVATE", OsString::from_vec(vec![0xff]))
            .output()
            .unwrap(),
    );
    assert!(!output.exists());
}

#[test]
fn capture_requires_explicit_separator_and_accepts_empty_child_argv() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let output = root.join("out");
    refused(
        &Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
            .env_clear()
            .arg("--capture-make-invocation")
            .arg("private-not-a-path")
            .output()
            .unwrap(),
    );
    let result = command(&output, &root).output().unwrap();
    assert!(result.status.success() && result.stdout.is_empty() && result.stderr.is_empty());
    let captured = NativeMakeInvocation::parse(&fs::read(&output).unwrap()).unwrap();
    assert!(captured.arguments().is_empty() && captured.environment().is_empty());
}
