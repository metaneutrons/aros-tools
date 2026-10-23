use super::*;

use crate::compatibility::environment::{
    self, CompatibilityHostToolReport, ResolvedProbeEnvironment,
};
use crate::compatibility::CompatibilityEnvironment;
use aros_common::{sha256_bytes, Sha256Digest};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

fn resolved_environment(entries: &[(&str, &str)]) -> ResolvedProbeEnvironment {
    let mut variables = BTreeMap::from([("PATH".to_owned(), "/nonexistent".to_owned())]);
    variables.extend(
        entries
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned())),
    );
    environment::resolve(&CompatibilityEnvironment::Poisoned { variables }).unwrap()
}

fn canonical_env() -> PathBuf {
    Path::new("/usr/bin/env").canonicalize().unwrap()
}

fn command_environment(command: &std::process::Command) -> BTreeMap<OsString, Option<OsString>> {
    command
        .get_envs()
        .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
        .collect()
}

fn assert_digest(value: &Sha256Digest) {
    assert_eq!(value.to_string().len(), 64);
}

#[test]
fn command_runs_with_exact_resolved_environment_and_no_ambient_entries() {
    let program = canonical_env();
    let cwd = tempfile::tempdir().unwrap();
    let cwd = cwd.path().canonicalize().unwrap();
    let resolved = resolved_environment(&[
        ("LC_ALL", "C"),
        ("AROS_INVOCATION_TEST_MARKER", "only-this-value"),
    ]);
    let expected = resolved.variables.clone();
    let context = InvocationContext::new(&program, &[], &cwd, resolved).unwrap();

    let output = context.command().output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let actual: BTreeMap<_, _> = stdout
        .lines()
        .map(|entry| {
            let (name, value) = entry.split_once('=').unwrap();
            (name.to_owned(), value.to_owned())
        })
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(context.environment().variables, expected);
}

#[test]
fn command_and_digests_preserve_the_legacy_identity_inputs() {
    let program = canonical_env();
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    let arguments = vec![
        "argument with spaces".to_owned(),
        "--literal=$(not-a-shell-expression)".to_owned(),
    ];
    let resolved = resolved_environment(&[("LC_ALL", "C")]);
    let expected_environment = environment::identity(&resolved).unwrap();
    let context = InvocationContext::new(&program, &arguments, &cwd, resolved).unwrap();

    let command = context.command();
    let actual_arguments: Vec<_> = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert_eq!(actual_arguments, arguments);
    assert_eq!(context.program(), program);
    assert_eq!(context.arguments(), arguments);
    assert_eq!(context.working_directory(), cwd);
    assert_eq!(command.get_current_dir(), Some(cwd.as_path()));

    let (expected_program, _) = super::super::super::measure_executable(&program).unwrap();
    let command_sha256 = super::super::super::command_identity(&program, &arguments).unwrap();
    let invocation_bytes = crate::canonical::bytes(&serde_json::json!({
        "command": command_sha256,
        "working_directory": cwd,
    }))
    .unwrap();
    assert_eq!(context.program_sha256(), &expected_program);
    assert_eq!(
        context.invocation_sha256(),
        &sha256_bytes(&invocation_bytes)
    );
    assert_eq!(context.environment_sha256(), &expected_environment);
    assert_digest(context.program_sha256());
    assert_digest(context.invocation_sha256());
    assert_digest(context.environment_sha256());
}

#[test]
fn executable_alias_spelling_is_not_replaced_by_its_canonical_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    let program = canonical_env();
    let alias = cwd.join("selected-program-alias");
    std::os::unix::fs::symlink(&program, &alias).unwrap();
    let selected = InvocationContext::new(&alias, &[], &cwd, resolved_environment(&[])).unwrap();
    let canonical = InvocationContext::new(&program, &[], &cwd, resolved_environment(&[])).unwrap();
    assert_eq!(selected.program_sha256(), canonical.program_sha256());
    assert_ne!(selected.invocation_sha256(), canonical.invocation_sha256());
    assert_eq!(selected.command().get_program(), alias.as_os_str());
    assert_eq!(selected.program(), alias);
    selected.revalidate().unwrap();
}

#[test]
fn stage_serialization_remains_five_hashes_without_raw_invocation_data() {
    let program = canonical_env();
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    let raw_environment_value = "private-environment-value";
    let raw_argument = "private-argv-value";
    let context = InvocationContext::new(
        &program,
        &[raw_argument.to_owned()],
        &cwd,
        resolved_environment(&[("AROS_PRIVATE_TEST_VALUE", raw_environment_value)]),
    )
    .unwrap();
    let stage = super::super::Stage {
        program: context.program_sha256().clone(),
        invocation: context.invocation_sha256().clone(),
        environment: context.environment_sha256().clone(),
        stdout: sha256_bytes(b"synthetic stdout"),
        stderr: sha256_bytes(b"synthetic stderr"),
        invocation_context: context,
        name: "synthetic-stage".into(),
    };

    let value = serde_json::to_value(stage).unwrap();
    let object = value.as_object().unwrap();
    let keys: BTreeSet<_> = object.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        BTreeSet::from(["program", "invocation", "environment", "stdout", "stderr"])
    );
    for key in ["program", "invocation", "environment", "stdout", "stderr"] {
        let digest: Sha256Digest = serde_json::from_value(object[key].clone()).unwrap();
        assert_digest(&digest);
    }
    let serialized = serde_json::to_string(&value).unwrap();
    assert!(!serialized.contains(raw_environment_value));
    assert!(!serialized.contains(raw_argument));
}

#[test]
fn revalidation_rejects_a_program_changed_after_construction() {
    let temporary = tempfile::tempdir().unwrap();
    let program_path = temporary.path().join("private-probe");
    fs::write(&program_path, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&program_path, fs::Permissions::from_mode(0o700)).unwrap();
    let program = program_path.canonicalize().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    let context = InvocationContext::new(
        &program,
        &[],
        &cwd,
        resolved_environment(&[("LC_ALL", "C")]),
    )
    .unwrap();
    context.revalidate().unwrap();

    fs::write(&program, b"#!/bin/sh\nexit 1\n").unwrap();
    assert!(context.revalidate().is_err());
}

#[test]
fn constructor_rejects_noncanonical_paths_invalid_environment_and_nul_arguments() {
    let program = canonical_env();
    let temporary = tempfile::tempdir().unwrap();
    let canonical_cwd = temporary.path().canonicalize().unwrap();
    let noncanonical_cwd = temporary.path().join("child").join("..");
    fs::create_dir(temporary.path().join("child")).unwrap();
    let resolved = resolved_environment(&[]);

    assert!(InvocationContext::new(
        &PathBuf::from("usr/bin/env"),
        &[],
        &canonical_cwd,
        resolved.clone()
    )
    .is_err());
    assert!(InvocationContext::new(&program, &[], &noncanonical_cwd, resolved.clone()).is_err());

    let invalid_environment = ResolvedProbeEnvironment {
        variables: BTreeMap::from([
            ("PATH".to_owned(), "/nonexistent".to_owned()),
            ("invalid-name".to_owned(), "value".to_owned()),
        ]),
        host_tools: BTreeMap::new(),
    };
    assert!(InvocationContext::new(&program, &[], &canonical_cwd, invalid_environment).is_err());
    assert!(InvocationContext::new(
        &program,
        &["argument\0with-nul".to_owned()],
        &canonical_cwd,
        resolved,
    )
    .is_err());
}

#[test]
fn normalized_host_tool_identity_does_not_replace_the_actual_path_or_command() {
    let program = canonical_env();
    let temporary = tempfile::tempdir().unwrap();
    let cwd = temporary.path().canonicalize().unwrap();
    let first_path = temporary.path().join("host-tools-one");
    let second_path = temporary.path().join("host-tools-two");
    fs::create_dir(&first_path).unwrap();
    fs::create_dir(&second_path).unwrap();
    let report = CompatibilityHostToolReport {
        sha256: sha256_bytes(b"synthetic host command"),
        size: 22,
    };
    let make_environment = |path: &Path| ResolvedProbeEnvironment {
        variables: BTreeMap::from([
            ("PATH".to_owned(), path.to_string_lossy().into_owned()),
            ("LC_ALL".to_owned(), "C".to_owned()),
        ]),
        host_tools: BTreeMap::from([("tool".to_owned(), report.clone())]),
    };
    let first_environment = make_environment(&first_path);
    let second_environment = make_environment(&second_path);
    assert_eq!(
        environment::identity(&first_environment).unwrap(),
        environment::identity(&second_environment).unwrap()
    );
    assert_ne!(first_environment.variables, second_environment.variables);

    let first = InvocationContext::new(&program, &[], &cwd, first_environment).unwrap();
    let second = InvocationContext::new(&program, &[], &cwd, second_environment).unwrap();
    assert_eq!(first.environment_sha256(), second.environment_sha256());
    assert_eq!(
        first.environment().variables["PATH"],
        first_path.to_string_lossy()
    );
    assert_eq!(
        second.environment().variables["PATH"],
        second_path.to_string_lossy()
    );
    assert_ne!(
        command_environment(&first.command()),
        command_environment(&second.command())
    );
}
