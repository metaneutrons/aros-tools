//! Root command-prefix capture covers literal invocation data, not execution.

#![cfg(unix)]

use aros_common::{make_native_invocation::NativeMakeInvocation, sha256_bytes};
use aros_transpiler::root_dispatch_capture::{
    RootDispatchCapturePlan, RootDispatchCaptureSelection,
};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const TARGET_PATTERN: &str = "%-quick";
const GOAL: &str = "build-quick";
const PROJECT: &str = "Engine";
const PREFIX_VARIABLE: &str = "COMMAND_ENV";
const PROGRAM_VARIABLE: &str = "TOOL";
const OPTIONS_VARIABLE: &str = "TOOL_OPTIONS";
const ORIGINAL_PREFIX: &str = "env ROOT_DISPATCH_CAPTURE_PREFIX_SENTINEL=from-prefix";
const OPTIONS: &str = "--literal-option";
const ROOT_EXPORT_NAME: &str = "ROOT_DISPATCH_CAPTURE_ROOT_EXPORT";
const ROOT_EXPORT_VALUE: &str = "full-root-export-value";
const PREFIX_EXPORT_NAME: &str = "ROOT_DISPATCH_CAPTURE_PREFIX_SENTINEL";
const PREFIX_EXPORT_VALUE: &str = "from-prefix";

fn private_directory(path: &Path) {
    fs::create_dir_all(path).expect("create private fixture directory");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("set private fixture directory mode");
}

fn gnu_make() -> PathBuf {
    let executable_name = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let path = std::env::var_os("PATH").expect("test runner provides PATH");
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(executable_name);
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            continue;
        }
        let canonical = candidate
            .canonicalize()
            .expect("canonical GNU Make executable");
        let version = Command::new(&canonical)
            .arg("--version")
            .output()
            .expect("run GNU Make version check");
        assert!(
            version.status.success()
                && String::from_utf8_lossy(&version.stdout).contains("GNU Make"),
            "native Root dispatch coverage requires GNU Make"
        );
        return canonical;
    }
    panic!("GNU Make executable is required for Root dispatch capture coverage");
}

fn make_root_source(program: &str) -> String {
    format!(
        "COMMAND_ENV = {ORIGINAL_PREFIX}\n\
         export COMMAND_ENV\n\
         TOOL = {program}\n\
         TOOL_OPTIONS = {OPTIONS}\n\
         export {ROOT_EXPORT_NAME} = {ROOT_EXPORT_VALUE}\n\
         export ROOT_CAPTURE_AUTOMATIC_FRAME = $@|$*|$<|$^|$?|$+|$|\n\
         $(TOOL):\n\
         {TARGET_PATTERN}:\n\
         \t@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) {PROJECT}.$@\n"
    )
}

const fn selection<'a>(
    program: &'a str,
    observer: &'a Path,
    output: &'a Path,
    working_directory: &'a Path,
    goal: &'a str,
) -> RootDispatchCaptureSelection<'a> {
    RootDispatchCaptureSelection {
        target_pattern: TARGET_PATTERN,
        goal,
        prefix_variable: PREFIX_VARIABLE,
        program_variable: PROGRAM_VARIABLE,
        options_variable: OPTIONS_VARIABLE,
        project: PROJECT,
        original_prefix: ORIGINAL_PREFIX,
        program,
        options: OPTIONS,
        observer,
        output,
        working_directory,
    }
}

fn changed_capture(
    arguments: Vec<String>,
    working_directory: PathBuf,
    environment: BTreeMap<String, String>,
) -> NativeMakeInvocation {
    NativeMakeInvocation::new(arguments, working_directory, environment)
        .expect("construct changed inert capture data")
}

fn invoke_make(
    make: &Path,
    source: &Path,
    override_file: &Path,
    goal: &str,
    working_directory: &Path,
) -> Output {
    Command::new(make)
        .args(["--no-print-directory", "-s", "-f"])
        .arg(source)
        .arg("-f")
        .arg(override_file)
        .arg(goal)
        .current_dir(working_directory)
        .env_remove(PREFIX_EXPORT_NAME)
        .output()
        .expect("run native GNU Make Root dispatch fixture")
}

#[test]
fn root_dispatch_capture_observes_command_data_without_running_the_source_tool() {
    let temporary = tempfile::tempdir().expect("temporary Root dispatch fixture");
    let root = temporary
        .path()
        .canonicalize()
        .expect("canonical temporary Root directory");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
        .expect("set private Root directory mode");

    let observer = Path::new(env!("CARGO_BIN_EXE_aros-transpiler"));
    assert!(observer.is_absolute() && observer.is_file());
    let output_directory = root.join("private capture ' output");
    private_directory(&output_directory);
    let output = output_directory.join("root invocation.json");
    let working_directory = root.clone();
    let program = root.join("original-tool.sh");
    let program_text = program.to_str().expect("fixture program path is UTF-8");
    let sentinel = root.join("original-tool-ran");
    fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf 'original tool ran' > '{}'\n",
            sentinel.to_str().expect("sentinel path is UTF-8")
        ),
    )
    .expect("write original-tool sentinel script");
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700))
        .expect("make original-tool sentinel executable");

    let root_bytes = make_root_source(program_text).into_bytes();
    let expected_sha = sha256_bytes(&root_bytes);
    let plan = RootDispatchCapturePlan::derive(
        &root_bytes,
        &expected_sha,
        selection(program_text, observer, &output, &working_directory, GOAL),
    )
    .expect("derive a source-bound Root capture override");
    assert_eq!(plan.original_sha256(), &expected_sha);

    let override_file = root.join("root-capture-override.mk");
    fs::write(&override_file, plan.override_bytes()).expect("write derived Make override");
    let override_text = std::str::from_utf8(plan.override_bytes()).expect("override is UTF-8");
    assert!(override_text.starts_with("build-quick: %-quick:\n\t@+"));
    assert!(
        !override_text.contains(":="),
        "the recorder does not change Source variables"
    );
    assert_eq!(override_text.lines().count(), 2);

    let make = gnu_make();
    let source_file = root.join("Root.mk");
    fs::write(&source_file, &root_bytes).expect("write synthetic Root makefile");
    let result = invoke_make(
        &make,
        &source_file,
        &override_file,
        GOAL,
        &working_directory,
    );
    assert!(
        result.status.success(),
        "GNU Make failed: {}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty() && result.stderr.is_empty());
    assert!(
        !sentinel.exists(),
        "capture frontend executed the selected original tool"
    );
    assert_eq!(
        fs::metadata(&output_directory)
            .expect("private capture output directory exists")
            .permissions()
            .mode()
            & 0o7777,
        0o700
    );
    assert_eq!(
        fs::metadata(&output)
            .expect("capture frontend wrote the invocation")
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );

    let raw = NativeMakeInvocation::parse(&fs::read(&output).expect("read captured invocation"))
        .expect("parse inert captured invocation");
    let expected_raw_arguments = vec![
        "env".to_owned(),
        format!("{PREFIX_EXPORT_NAME}={PREFIX_EXPORT_VALUE}"),
        program_text.to_owned(),
        OPTIONS.to_owned(),
        format!("{PROJECT}.{GOAL}"),
    ];
    assert_eq!(raw.arguments(), expected_raw_arguments);
    assert_eq!(raw.working_directory(), working_directory.as_path());
    assert_eq!(
        raw.environment().get(PREFIX_VARIABLE).map(String::as_str),
        Some(ORIGINAL_PREFIX)
    );
    assert_eq!(
        raw.environment().get(ROOT_EXPORT_NAME).map(String::as_str),
        Some(ROOT_EXPORT_VALUE)
    );
    assert!(!raw.environment().contains_key(PREFIX_EXPORT_NAME));
    assert_eq!(
        raw.environment()
            .get("ROOT_CAPTURE_AUTOMATIC_FRAME")
            .map(String::as_str),
        Some("build-quick|build|||||"),
        "the selected static pattern preserves the original automatic-variable frame"
    );

    let captured = plan
        .bind_capture(raw.clone())
        .expect("bind exact captured Root dispatch data");
    assert_eq!(captured.raw(), &raw);
    assert_eq!(captured.invocation().argument_zero(), Some(program_text));
    assert_eq!(
        captured.invocation().arguments(),
        [OPTIONS.to_owned(), format!("{PROJECT}.{GOAL}")]
    );
    assert_eq!(captured.invocation().working_directory(), working_directory);
    assert_eq!(
        captured
            .invocation()
            .environment()
            .get(PREFIX_VARIABLE)
            .map(String::as_str),
        Some(ORIGINAL_PREFIX)
    );
    assert_eq!(
        captured
            .invocation()
            .environment()
            .get(ROOT_EXPORT_NAME)
            .map(String::as_str),
        Some(ROOT_EXPORT_VALUE)
    );
    assert_eq!(
        captured
            .invocation()
            .environment()
            .get(PREFIX_EXPORT_NAME)
            .map(String::as_str),
        Some(PREFIX_EXPORT_VALUE)
    );

    let mut wrong_arguments = raw.arguments().to_vec();
    *wrong_arguments.last_mut().expect("captured namespace goal") = "Engine.other-quick".into();
    assert!(plan
        .bind_capture(changed_capture(
            wrong_arguments,
            raw.working_directory().to_owned(),
            raw.environment().clone(),
        ))
        .is_err());
    assert!(plan
        .bind_capture(raw.clone().with_argument_zero("unexpected".into()).unwrap())
        .is_err());

    assert!(plan
        .bind_capture(changed_capture(
            raw.arguments().to_vec(),
            PathBuf::from("/"),
            raw.environment().clone(),
        ))
        .is_err());

    let mut wrong_export = raw.environment().clone();
    wrong_export.insert(
        PREFIX_VARIABLE.to_owned(),
        "different instrumentation".into(),
    );
    assert!(plan
        .bind_capture(changed_capture(
            raw.arguments().to_vec(),
            raw.working_directory().to_owned(),
            wrong_export,
        ))
        .is_err());
}

#[test]
fn root_dispatch_capture_derivation_rejects_source_drift_and_ambiguous_recipes() {
    let temporary = tempfile::tempdir().expect("temporary Root derivation fixture");
    let root = temporary
        .path()
        .canonicalize()
        .expect("canonical temporary Root directory");
    let program = root.join("source-tool");
    let program_text = program.to_str().expect("fixture program path is UTF-8");
    let observer = Path::new(env!("CARGO_BIN_EXE_aros-transpiler"));
    let output = root.join("captured.json");
    let source = make_root_source(program_text);
    let source_bytes = source.as_bytes();
    let expected_sha = sha256_bytes(source_bytes);

    let mut wrong_pattern = selection(program_text, observer, &output, &root, GOAL);
    wrong_pattern.target_pattern = "%-clean";
    let error = RootDispatchCapturePlan::derive(source_bytes, &expected_sha, wrong_pattern)
        .err()
        .expect("a mismatched static pattern must fail before GNU Make can merely warn");
    assert!(error.contains("selected pattern differ"));

    for role in 0..3 {
        let mut selected = selection(program_text, observer, &output, &root, GOAL);
        match role {
            0 => selected.prefix_variable = "SHELL",
            1 => selected.program_variable = "SHELL",
            _ => selected.options_variable = "SHELL",
        }
        let error = RootDispatchCapturePlan::derive(source_bytes, &expected_sha, selected)
            .err()
            .expect("each instrumentation role must reject the reserved recipe shell");
        assert!(error.contains("cannot override the recipe shell"));
    }

    assert!(RootDispatchCapturePlan::derive(
        source_bytes,
        &sha256_bytes(b"different Root bytes"),
        selection(program_text, observer, &output, &root, GOAL),
    )
    .is_err());

    let changed_recipe = source.replace("$(TOOL_OPTIONS) Engine.$@", "$(TOOL_OPTIONS) changed.$@");
    assert!(RootDispatchCapturePlan::derive(
        changed_recipe.as_bytes(),
        &sha256_bytes(changed_recipe.as_bytes()),
        selection(program_text, observer, &output, &root, GOAL),
    )
    .is_err());

    let with_prerequisite = source.replace("%-quick:\n", "%-quick: prerequisite\n");
    assert!(RootDispatchCapturePlan::derive(
        with_prerequisite.as_bytes(),
        &sha256_bytes(with_prerequisite.as_bytes()),
        selection(program_text, observer, &output, &root, GOAL),
    )
    .is_err());

    assert!(RootDispatchCapturePlan::derive(
        source_bytes,
        &expected_sha,
        selection(program_text, observer, &output, &root, "build"),
    )
    .is_err());

    let with_extra_recipe = source.replace(
        "\t@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@\n",
        "\t@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@\n\t@echo extra\n",
    );
    assert!(RootDispatchCapturePlan::derive(
        with_extra_recipe.as_bytes(),
        &sha256_bytes(with_extra_recipe.as_bytes()),
        selection(program_text, observer, &output, &root, GOAL),
    )
    .is_err());

    let chained_recipe = source.replace("Engine.$@\n", "Engine.$@; /usr/bin/true\n");
    assert!(RootDispatchCapturePlan::derive(
        chained_recipe.as_bytes(),
        &sha256_bytes(chained_recipe.as_bytes()),
        selection(program_text, observer, &output, &root, GOAL),
    )
    .is_err());

    for refused in [
        format!("ifeq ($(UNKNOWN_SELECTION),yes)\n{source}endif\n"),
        format!("  define INERT\n{source}endef\n"),
        format!("{source}%-quick:\n\t@+$(COMMAND_ENV) $(TOOL) $(TOOL_OPTIONS) Engine.$@\n"),
        format!(".ONESHELL:\n{source}"),
        format!(".RECIPEPREFIX := >\n{source}"),
        format!("{source}build-quick: unexpected-prerequisite\n"),
        format!("{source}build-%:\n\t@echo competing\n"),
        format!("{source}build-quick: TOOL_OPTIONS := different\n"),
        format!("{source}%-quick: TOOL_OPTIONS := different\n"),
    ] {
        assert!(RootDispatchCapturePlan::derive(
            refused.as_bytes(),
            &sha256_bytes(refused.as_bytes()),
            selection(program_text, observer, &output, &root, GOAL),
        )
        .is_err());
    }
    for prefix in [
        "env VALUE='literal$dollar'",
        "env VALUE='literal#hash'",
        "env VALUE='literal\\slash'",
    ] {
        let mut selected = selection(program_text, observer, &output, &root, GOAL);
        selected.original_prefix = prefix;
        assert!(RootDispatchCapturePlan::derive(source_bytes, &expected_sha, selected).is_err());
    }
}

#[test]
#[ignore = "requires an explicit retained real Root query; inert derivation only"]
fn actual_retained_source_root_dispatch_plan() {
    let case_path = PathBuf::from(std::env::var_os("AROS_TEST_MESA_PREFLIGHT_CASE").unwrap());
    assert!(case_path.is_absolute());
    let parent = case_path.parent().unwrap();
    let case: serde_json::Value = serde_json::from_slice(&fs::read(&case_path).unwrap()).unwrap();
    let query: serde_json::Value =
        serde_json::from_slice(&fs::read(parent.join("native-launch-inputs.json")).unwrap())
            .unwrap();
    let values = &query["values"];
    let value = |name: &str| values[name].as_str().unwrap();
    let build = Path::new(value("TOP"));
    let root = build.join("Makefile");
    let matches = query["read_inputs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record[0].as_str() == root.to_str())
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1);
    let expected = aros_common::Sha256Digest::parse(matches[0][1].as_str().unwrap()).unwrap();
    let (_, original) = aros_common::measure_regular_file_bounded(&root, 8 * 1024 * 1024)
        .unwrap()
        .unwrap();
    assert_eq!(sha256_bytes(&original), expected);
    let goals = query["goals"].as_array().unwrap();
    assert_eq!(goals.len(), 1);
    let observer = Path::new(env!("CARGO_BIN_EXE_aros-transpiler"));
    let private = tempfile::tempdir().unwrap();
    let output = private
        .path()
        .canonicalize()
        .unwrap()
        .join("pending-command.json");
    let plan = RootDispatchCapturePlan::derive(
        &original,
        &expected,
        RootDispatchCaptureSelection {
            target_pattern: "%-quick",
            goal: goals[0].as_str().unwrap(),
            prefix_variable: "CALL",
            program_variable: "MMAKE",
            options_variable: "MMAKE_OPTIONS",
            project: case["native_collection"]["project"].as_str().unwrap(),
            original_prefix: value("CALL"),
            program: value("MMAKE"),
            options: value("MMAKE_OPTIONS"),
            observer,
            output: &output,
            working_directory: build,
        },
    )
    .unwrap();
    assert!(!output.exists());
    let (_, after) = aros_common::measure_regular_file_bounded(&root, 8 * 1024 * 1024)
        .unwrap()
        .unwrap();
    assert_eq!(after, original);
    eprintln!("qualification=retained-root-recipe-derivation-only-not-live-owner-or-execution root_sha256={} override_sha256={}",
        plan.original_sha256(), sha256_bytes(plan.override_bytes()));
}
