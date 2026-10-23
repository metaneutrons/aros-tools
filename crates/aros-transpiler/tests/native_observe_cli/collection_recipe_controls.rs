//! Version-six collection observes recipe controls on the actual child.

use aros_common::{
    make_initial_values::{ValidatedMakeRecipeControls, MAKE_RECIPE_CONTROLS_METADATA_KEY},
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
    make_native_invocation::PrivateNativeCaptureDirectory,
    sha256_bytes, Sha256Digest,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

const COLLECTION_FLAG: &str = "--observe-make-launch-collection";
const CONTROL_NAME: &str = "control.json";
const SELECTED_MAKEFILE: &str = "selected.mk";
const SELECTED_ARGUMENT_ZERO: &str = "selected-gmake-argv0";

fn require_gnu_make_42_or_later() -> PathBuf {
    let program = super::find_make_program();
    let output = Command::new(&program)
        .env_clear()
        .env("LC_ALL", "C")
        .arg("--version")
        .output()
        .expect("query the selected GNU Make version");
    assert!(
        output.status.success(),
        "selected GNU Make version query failed"
    );
    let version_output =
        String::from_utf8(output.stdout).expect("selected GNU Make version output is UTF-8");
    let mut fields = version_output
        .lines()
        .next()
        .expect("selected GNU Make has a version line")
        .split_whitespace();
    assert_eq!(fields.next(), Some("GNU"), "selected Make must be GNU Make");
    assert_eq!(
        fields.next(),
        Some("Make"),
        "selected Make must be GNU Make"
    );
    let version = fields.next().expect("GNU Make version is present");
    let mut components = version.split('.');
    let major = components
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .expect("GNU Make major version is numeric");
    let minor = components
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .expect("GNU Make minor version is numeric");
    assert!(
        (major, minor) >= (4, 2),
        "native recipe-control fixture requires selected GNU Make >= 4.2; found {major}.{minor}"
    );
    program
}

fn private_directory(path: &Path) {
    fs::create_dir(path).expect("create private test directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("set private test directory mode");
}

fn observation_fixture() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().expect("native recipe-control fixture");
    let root = super::private_root(&temporary);
    let cwd = root.join("working");
    private_directory(&cwd);
    fs::write(
        cwd.join(SELECTED_MAKEFILE),
        b"$(file >source-parse-marker,parsed)\n.PHONY: all\nall:\n\t@printf recipe >source-recipe-marker\n",
    )
    .expect("write selected source fixture");
    let output_root = root.join("collection");
    private_directory(&output_root);
    (temporary, root, cwd, output_root)
}

fn control_value(
    output_root: &Path,
    program: &Path,
    executable_sha256: &Sha256Digest,
    version: u64,
    phase: &str,
    maximum_calls: usize,
) -> Value {
    let mut value = json!({
        "version": version,
        "output_root": output_root,
        "program": program,
        "executable_sha256": executable_sha256.as_str(),
        "context_sha256": super::context_digest(),
        "phase": phase,
        "maximum_calls": maximum_calls,
    });
    if version >= 4 {
        value["argument_zero"] = SELECTED_ARGUMENT_ZERO.into();
    }
    value
}

fn write_control(root: &Path, value: &Value) -> (PathBuf, Sha256Digest) {
    let bytes = serde_json::to_vec(value).expect("encode versioned collection control");
    PrivateNativeCaptureDirectory::open(root)
        .expect("bind private control directory")
        .write_new(OsStr::new(CONTROL_NAME), &bytes)
        .expect("retain private collection control");
    let path = root.join(CONTROL_NAME);
    super::assert_private_file(&path);
    (path, sha256_bytes(&bytes))
}

fn make_environment(root: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "PATH".into(),
            std::env::var("PATH").expect("test runner provides PATH"),
        ),
        ("LC_ALL".into(), "C".into()),
        (
            "PRIVATE_RECIPE_CONTROL_SENTINEL".into(),
            "private-fixture-value".into(),
        ),
        ("AROS_TRANSPILER_LOG_LEVEL".into(), "trace".into()),
        (
            "AROS_TRANSPILER_LOG_FILE".into(),
            root.join("must-not-log").to_string_lossy().into_owned(),
        ),
    ])
}

fn make_arguments() -> Vec<String> {
    [
        "-r",
        "-R",
        "-n",
        "-f",
        SELECTED_MAKEFILE,
        "CLI_VALUE=private-fixture-value",
        "all",
    ]
    .map(str::to_owned)
    .to_vec()
}

fn run_collection(
    control: &Path,
    control_sha256: &Sha256Digest,
    cwd: &Path,
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aros-transpiler"))
        .env_clear()
        .envs(environment)
        .current_dir(cwd)
        .arg(COLLECTION_FLAG)
        .arg(control)
        .arg(control_sha256.as_str())
        .arg("--")
        .args(arguments)
        .output()
        .expect("run versioned native observation collection entrypoint")
}

fn started_record(output_root: &Path) -> Value {
    let path = output_root.join("call-0000/observation-started.json");
    super::assert_private_file(&path);
    serde_json::from_slice(&fs::read(path).expect("read observation start record"))
        .expect("parse observation start record")
}

fn assert_no_source_effects(cwd: &Path) {
    assert!(
        !cwd.join("source-parse-marker").exists(),
        "selected source input was parsed during observation"
    );
    assert!(
        !cwd.join("source-recipe-marker").exists(),
        "selected source recipe ran during observation"
    );
}

fn assert_v6_recipe_controls(output_root: &Path) {
    let started = started_record(output_root);
    assert_eq!(started["version"].as_u64(), Some(3));
    assert_eq!(started["phase"].as_str(), Some("first-input-prelude"));
    assert_eq!(started["recipe_controls"].as_bool(), Some(true));
    assert_eq!(
        started["argument_zero"].as_str(),
        Some(SELECTED_ARGUMENT_ZERO)
    );

    let stdout_path = output_root.join("call-0000/initial.stdout");
    super::assert_private_file(&stdout_path);
    let stdout = fs::read(stdout_path).expect("read actual child initial observation");
    let namespace = ValidatedMakeNamespace::from_make_stdout(&stdout)
        .expect("parse actual child GNU Make namespace");
    let controls = ValidatedMakeRecipeControls::from_make_stdout(&stdout, &namespace)
        .expect("parse actual child validated recipe controls");
    let mut entries = controls.entries();
    let shellflags = entries
        .next()
        .expect("actual child observation contains .SHELLFLAGS");
    assert!(
        entries.next().is_none(),
        "recipe-control packet has one member"
    );
    assert_eq!(shellflags.name, ".SHELLFLAGS");
    assert!(
        matches!(
            shellflags.origin,
            MakeVariableOrigin::Default | MakeVariableOrigin::File
        ),
        "observed .SHELLFLAGS has an eligible GNU Make origin"
    );
    assert!(
        !shellflags.value.is_empty(),
        "observed .SHELLFLAGS is nonempty"
    );
    assert!(
        String::from_utf8_lossy(&stdout).contains(MAKE_RECIPE_CONTROLS_METADATA_KEY),
        "actual child stdout retains the recipe-control metadata packet"
    );
}

#[test]
fn version_six_collection_captures_validated_recipe_controls_in_the_actual_child() {
    let program = require_gnu_make_42_or_later();
    let executable_sha256 = super::measure_executable(&program);

    // Both accepted endpoints are exercised through the real frontend child.
    for maximum_calls in [1, 4096] {
        let (_temporary, root, cwd, output_root) = observation_fixture();
        let value = control_value(
            &output_root,
            &program,
            &executable_sha256,
            6,
            "first-input-prelude",
            maximum_calls,
        );
        let (control, digest) = write_control(&root, &value);
        let environment = make_environment(&root);
        let output = run_collection(&control, &digest, &cwd, &make_arguments(), &environment);
        assert!(
            output.status.success() && output.stdout.is_empty() && output.stderr.is_empty(),
            "version-six GNU Make observation did not complete privately"
        );
        assert_v6_recipe_controls(&output_root);
        super::assert_private_file(&output_root.join("call-0000/call-completed.json"));
        assert_no_source_effects(&cwd);
        assert!(
            !root.join("must-not-log").exists(),
            "private version-six entrypoint opened normal logging"
        );
    }
}

#[test]
fn version_four_collection_retains_legacy_start_and_stdout_without_recipe_controls() {
    let program = require_gnu_make_42_or_later();
    let executable_sha256 = super::measure_executable(&program);
    let (_temporary, root, cwd, output_root) = observation_fixture();
    let value = control_value(
        &output_root,
        &program,
        &executable_sha256,
        4,
        "first-input-prelude",
        1,
    );
    let (control, digest) = write_control(&root, &value);
    let environment = make_environment(&root);
    let output = run_collection(&control, &digest, &cwd, &make_arguments(), &environment);
    assert!(
        output.status.success() && output.stdout.is_empty() && output.stderr.is_empty(),
        "legacy version-four observation did not complete privately"
    );

    let started = started_record(&output_root);
    assert_eq!(started["version"].as_u64(), Some(2));
    assert_eq!(started["phase"].as_str(), Some("first-input-prelude"));
    assert_eq!(
        started["argument_zero"].as_str(),
        Some(SELECTED_ARGUMENT_ZERO)
    );
    assert!(started.get("recipe_controls").is_none());

    let stdout = fs::read(output_root.join("call-0000/initial.stdout"))
        .expect("read legacy actual child initial observation");
    assert!(
        !String::from_utf8_lossy(&stdout).contains(MAKE_RECIPE_CONTROLS_METADATA_KEY),
        "version-four stdout unexpectedly contains the opt-in recipe-control packet"
    );
    let namespace = ValidatedMakeNamespace::from_make_stdout(&stdout)
        .expect("parse legacy actual child namespace");
    assert!(
        ValidatedMakeRecipeControls::from_make_stdout(&stdout, &namespace).is_err(),
        "version-four observation unexpectedly validates recipe controls"
    );
    assert_no_source_effects(&cwd);
}

#[test]
fn version_six_pre_first_file_phase_refuses_before_claiming_or_starting_observation() {
    let program = require_gnu_make_42_or_later();
    let executable_sha256 = super::measure_executable(&program);
    let (_temporary, root, cwd, output_root) = observation_fixture();
    let value = control_value(
        &output_root,
        &program,
        &executable_sha256,
        6,
        "pre-first-file",
        1,
    );
    let (control, digest) = write_control(&root, &value);
    let environment = make_environment(&root);
    let output = run_collection(&control, &digest, &cwd, &make_arguments(), &environment);
    super::assert_refused(&output);
    assert_eq!(
        fs::read_dir(&output_root)
            .expect("inspect collection after phase refusal")
            .count(),
        0,
        "unsupported phase claimed a collection slot"
    );
    assert_no_source_effects(&cwd);
}

#[test]
fn version_six_rejects_capacity_outside_one_through_4096_before_claiming() {
    let program = require_gnu_make_42_or_later();
    let executable_sha256 = super::measure_executable(&program);

    for maximum_calls in [0, 4097] {
        let (_temporary, root, cwd, output_root) = observation_fixture();
        let value = control_value(
            &output_root,
            &program,
            &executable_sha256,
            6,
            "first-input-prelude",
            maximum_calls,
        );
        let (control, digest) = write_control(&root, &value);
        let environment = make_environment(&root);
        let output = run_collection(&control, &digest, &cwd, &make_arguments(), &environment);
        super::assert_refused(&output);
        assert_eq!(
            fs::read_dir(&output_root)
                .expect("inspect collection after capacity refusal")
                .count(),
            0,
            "out-of-range capacity claimed a collection slot"
        );
        assert_no_source_effects(&cwd);
    }
}
