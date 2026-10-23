//! Synthetic native observation and replay coverage.
//!
//! These tests exercise data consistency for a closed GNU Make invocation.
//! They do not establish production executable, receipt, file-ownership or
//! complete invocation binding.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::{Command, Output},
};

use aros_common::{
    make_initial_values::{
        MakeInitialValueFlavor, ValidatedMakeEnvironmentValues, ValidatedMakeInitialValues,
        MAKE_ENVIRONMENT_VALUES_METADATA_KEY,
    },
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace, MAKE_NAMESPACE_METADATA_KEY},
    make_observer::{
        command_line_values_fragment, environment_values_fragment, source_prelude_fragment,
        validate_source_prelude_inputs,
    },
    make_startup_arguments::LiteralMakeStartupArguments,
};
use aros_transpiler::{
    dirs::DirVars,
    make_vars::{MakeProgramPaths, MakeVariableProgram},
};

const CLI_OVERRIDE: &str = "SYNTHETIC-ALIAS=literal-cli-shadow";
const PRIVATE_ENV_VALUE: &str = "synthetic-private-environment-77c2";
const SYNTHETIC_ENV_RAW: &str = "prefix-$(LATER)";

const fn make_program() -> &'static str {
    if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    }
}

fn original_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "PATH".to_owned(),
            std::env::var("PATH").expect("test runner provides PATH"),
        ),
        ("SYNTHETIC_ENV".to_owned(), SYNTHETIC_ENV_RAW.to_owned()),
        (
            "SYNTHETIC-ALIAS".to_owned(),
            "synthetic-original-environment-shadow".to_owned(),
        ),
        ("PRIVATE_ENV".to_owned(), PRIVATE_ENV_VALUE.to_owned()),
    ])
}

fn startup_arguments(observer: &Path, source: &Path, command_line: &[&str]) -> Vec<String> {
    let mut arguments = [
        "-r".to_owned(),
        "-n".to_owned(),
        "-s".to_owned(),
        "--no-print-directory".to_owned(),
        "-f".to_owned(),
        observer
            .to_str()
            .expect("temporary path is UTF-8")
            .to_owned(),
        "-o".to_owned(),
        observer
            .to_str()
            .expect("temporary path is UTF-8")
            .to_owned(),
        "-f".to_owned(),
        source.to_str().expect("temporary path is UTF-8").to_owned(),
        "-o".to_owned(),
        source.to_str().expect("temporary path is UTF-8").to_owned(),
        "synthetic-goal".to_owned(),
    ]
    .to_vec();
    arguments.extend(command_line.iter().map(|argument| (*argument).to_owned()));
    arguments
}

fn observer_source(
    command_line_names: &BTreeSet<String>,
    environment_names: Option<&BTreeSet<String>>,
) -> String {
    let mut source = source_prelude_fragment();
    source.push_str(
        &command_line_values_fragment(command_line_names)
            .expect("synthetic CLI observer names are safe"),
    );
    if let Some(names) = environment_names {
        source.push_str(
            &environment_values_fragment(names)
                .expect("synthetic environment observer names are safe"),
        );
    }
    source
}

fn run_native(arguments: &[String], cwd: &Path, environment: &BTreeMap<String, String>) -> Output {
    let startup = LiteralMakeStartupArguments::admit(arguments)
        .expect("synthetic startup arguments use the admitted literal grammar");
    validate_source_prelude_inputs(environment, &BTreeMap::new())
        .expect("the exact synthetic environment has no active input controls");

    Command::new(make_program())
        .current_dir(cwd)
        .env_clear()
        .envs(environment)
        .args(startup.arguments())
        .output()
        .expect("run controlled synthetic Make query")
}

fn environment_names(namespace: &ValidatedMakeNamespace) -> BTreeSet<String> {
    namespace
        .to_observation()
        .entries
        .into_iter()
        .filter(|entry| entry.origin == MakeVariableOrigin::Environment)
        .map(|entry| entry.name)
        .collect()
}

fn contains_bytes(output: &Output, needle: &str) -> bool {
    let needle = needle.as_bytes();
    output
        .stdout
        .windows(needle.len())
        .any(|window| window == needle)
        || output
            .stderr
            .windows(needle.len())
            .any(|window| window == needle)
}

fn observed_line(output: &Output, prefix: &str) -> String {
    assert!(
        output.status.success(),
        "synthetic Make query failed: override_guard={}, membership_guard={}, syntax={}, payload_expansion={}",
        contains_bytes(output, "Environment-override origin"),
        contains_bytes(output, "environment variable membership"),
        contains_bytes(output, "unterminated"),
        contains_bytes(output, "SHOULD_NOT_RUN")
    );
    let text = std::str::from_utf8(&output.stdout).expect("synthetic Make stdout is UTF-8");
    let rows = text
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "expected one synthetic result line");
    rows[0].to_owned()
}

fn replace_metadata_value(stdout: &[u8], metadata_key: &str, from: &str, to: &str) -> Vec<u8> {
    let text = std::str::from_utf8(stdout).expect("synthetic Make stdout is UTF-8");
    let prefix = format!("{metadata_key}=");
    let mut changed = false;
    let lines = text
        .lines()
        .map(|line| {
            if let Some(value) = line.strip_prefix(&prefix) {
                let replacement = value.replace(from, to);
                if replacement != value {
                    changed = true;
                    return format!("{prefix}{replacement}");
                }
            }
            line.to_owned()
        })
        .collect::<Vec<_>>();
    assert!(changed, "expected synthetic metadata field was not present");
    let mut output = lines.join("\n");
    if text.ends_with('\n') {
        output.push('\n');
    }
    output.into_bytes()
}

fn without_metadata(stdout: &[u8], metadata_key: &str) -> Vec<u8> {
    let text = std::str::from_utf8(stdout).expect("synthetic Make stdout is UTF-8");
    let prefix = format!("{metadata_key}=");
    let mut removed = false;
    let lines = text
        .lines()
        .filter(|line| {
            if line.starts_with(&prefix) {
                removed = true;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
    assert!(removed, "expected synthetic metadata line was not present");
    let mut output = lines.join("\n");
    if text.ends_with('\n') {
        output.push('\n');
    }
    output.into_bytes()
}

fn make_paths<'a>(root: &'a Path, dirs: &'a DirVars) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn read_selected_source(
    program: &mut MakeVariableProgram,
    source_path: &Path,
    paths: &MakeProgramPaths<'_>,
) {
    let source = fs::read_to_string(source_path).expect("read exact synthetic selected source");
    let identity = source_path.to_str().expect("temporary path is UTF-8");
    program
        .read_file(identity, &source, paths, &|_, _| Ok(None))
        .expect("replay exact selected synthetic source");
}

#[test]
fn native_environment_observation_replays_exact_source_and_fails_closed_on_mismatch() {
    let directory = tempfile::tempdir().expect("temporary synthetic Make directory");
    let root = directory
        .path()
        .canonicalize()
        .expect("canonical temp directory");
    let observer = root.join("observer.mk");
    let selected_source = root.join("selected.mk");
    let source = concat!(
        "LATER := source-later\n",
        "SYNTHETIC_FILE := source-file-value\n",
        "SYNTHETIC-ALIAS := source-file-shadow\n",
        "ABSENT-DASH ?= first-dash\n",
        "ABSENT-DASH ?= must-not-replace\n",
        "1ABSENT ?= first-digit\n",
        "1ABSENT += appended\n",
        "RESULT := $(SYNTHETIC_ENV)|$(SYNTHETIC_FILE)|$(SYNTHETIC-ALIAS)|$(MAKELEVEL)|$(MFLAGS)|$(ABSENT-DASH)|$(1ABSENT)\n",
        ".PHONY: synthetic-goal\n",
        "synthetic-goal:\n",
        "\t$(info NATIVE_RESULT=$(RESULT))\n",
    );
    fs::write(&selected_source, source).expect("write synthetic selected source");

    let environment = original_environment();
    let arguments = startup_arguments(&observer, &selected_source, &[CLI_OVERRIDE]);
    let startup = LiteralMakeStartupArguments::admit(&arguments)
        .expect("synthetic startup arguments use the literal grammar");
    validate_source_prelude_inputs(&environment, &BTreeMap::new())
        .expect("the exact synthetic environment has no active input controls");
    let command_line_names = startup.command_line_names().clone();

    // First pass measures namespace names and origins only. The legacy packet
    // remains default/file-only and must not reveal the private environment
    // sentinel. The environment-value fragment is intentionally absent.
    fs::write(&observer, observer_source(&command_line_names, None))
        .expect("write names-only observer");
    let names_only = run_native(&arguments, &root, &environment);
    let first_result = observed_line(&names_only, "NATIVE_RESULT=");
    let first_namespace = ValidatedMakeNamespace::from_make_stdout(&names_only.stdout)
        .expect("parse names-only namespace");
    let expected_environment_names = environment_names(&first_namespace);
    assert!(expected_environment_names.contains("SYNTHETIC_ENV"));
    assert!(expected_environment_names.contains("PRIVATE_ENV"));
    assert!(expected_environment_names.contains("PATH"));
    assert!(!expected_environment_names.contains("SYNTHETIC-ALIAS"));
    assert!(first_namespace.proves_absent("ABSENT-DASH"));
    assert!(first_namespace.proves_absent("1ABSENT"));
    assert!(first_result.ends_with("|first-dash|first-digit appended"));
    assert_eq!(
        first_namespace.origin("SYNTHETIC-ALIAS"),
        Some(MakeVariableOrigin::CommandLine)
    );
    assert!(!contains_bytes(
        &names_only,
        MAKE_ENVIRONMENT_VALUES_METADATA_KEY
    ));
    assert!(!contains_bytes(&names_only, PRIVATE_ENV_VALUE));
    let defaults =
        ValidatedMakeInitialValues::from_make_stdout(&names_only.stdout, &first_namespace)
            .expect("parse default-only initial packet");
    assert!(!defaults
        .entries()
        .any(|entry| entry.value.contains(PRIVATE_ENV_VALUE)));
    assert!(ValidatedMakeEnvironmentValues::from_make_stdout(
        &names_only.stdout,
        &first_namespace,
        &expected_environment_names,
    )
    .is_err());

    // Second pass uses identical argv, environment, cwd and selected source;
    // only the opt-in environment packet is added to the observer prelude.
    fs::write(
        &observer,
        observer_source(&command_line_names, Some(&expected_environment_names)),
    )
    .expect("write opt-in environment observer");
    let measured = run_native(&arguments, &root, &environment);
    let native_result = observed_line(&measured, "NATIVE_RESULT=");
    assert!(
        first_result == native_result,
        "observer changed source result"
    );
    assert!(!native_result.contains(PRIVATE_ENV_VALUE));

    let namespace = ValidatedMakeNamespace::from_make_stdout(&measured.stdout)
        .expect("parse complete native namespace");
    assert_eq!(namespace, first_namespace, "observer changed namespace");
    assert_eq!(environment_names(&namespace), expected_environment_names);

    let environment_values = ValidatedMakeEnvironmentValues::from_make_stdout(
        &measured.stdout,
        &namespace,
        &expected_environment_names,
    )
    .expect("parse complete opt-in environment packet");
    let observed_names = environment_values
        .entries()
        .map(|entry| entry.name.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(observed_names, expected_environment_names);
    let synthetic = environment_values
        .entries()
        .find(|entry| entry.name == "SYNTHETIC_ENV")
        .expect("synthetic environment value was observed");
    assert!(
        synthetic.value == SYNTHETIC_ENV_RAW,
        "raw environment value changed"
    );
    assert_eq!(synthetic.flavor, MakeInitialValueFlavor::Recursive);
    assert_eq!(synthetic.origin, MakeVariableOrigin::Environment);
    let private = environment_values
        .entries()
        .find(|entry| entry.name == "PRIVATE_ENV")
        .expect("private synthetic environment value was explicitly observed");
    assert!(
        private.value == PRIVATE_ENV_VALUE,
        "private value was not preserved"
    );

    let mut missing_names = expected_environment_names.clone();
    assert!(missing_names.remove("SYNTHETIC_ENV"));
    let mut extra_names = expected_environment_names.clone();
    extra_names.insert("SYNTHETIC_EXTRA".to_owned());
    for invalid_names in [&missing_names, &extra_names] {
        assert!(ValidatedMakeEnvironmentValues::from_make_stdout(
            &measured.stdout,
            &namespace,
            invalid_names,
        )
        .is_err());

        // The native fragment must reject an incomplete/extra name set before
        // emitting the environment packet or any raw environment value.
        fs::write(
            &observer,
            observer_source(&command_line_names, Some(invalid_names)),
        )
        .expect("write intentionally incomplete observer");
        let refused = run_native(&arguments, &root, &environment);
        assert!(!refused.status.success());
        assert!(!contains_bytes(
            &refused,
            MAKE_ENVIRONMENT_VALUES_METADATA_KEY
        ));
        assert!(!contains_bytes(&refused, PRIVATE_ENV_VALUE));
    }

    let mut wrong_cli_arguments = arguments;
    wrong_cli_arguments.retain(|argument| argument != CLI_OVERRIDE);
    let wrong_startup = LiteralMakeStartupArguments::admit(&wrong_cli_arguments)
        .expect("synthetic wrong CLI expectation remains a valid startup");
    let original_environment_names = environment.keys().cloned().collect::<BTreeSet<_>>();
    let dirs = DirVars::default();
    let paths = make_paths(&root, &dirs);
    let oversized_original_names = (0..5000)
        .map(|index| format!("UNBOUND_{index}"))
        .collect::<BTreeSet<_>>();
    assert!(MakeVariableProgram::from_native_source_observation(
        &paths,
        &measured.stdout,
        &startup,
        &oversized_original_names,
        &expected_environment_names,
    )
    .is_err());
    assert!(MakeVariableProgram::from_native_source_observation(
        &paths,
        &measured.stdout,
        &wrong_startup,
        &original_environment_names,
        &expected_environment_names,
    )
    .is_err());

    let omitted_packet = without_metadata(&measured.stdout, MAKE_ENVIRONMENT_VALUES_METADATA_KEY);
    assert!(MakeVariableProgram::from_native_source_observation(
        &paths,
        &omitted_packet,
        &startup,
        &original_environment_names,
        &expected_environment_names,
    )
    .is_err());

    let changed_namespace = replace_metadata_value(
        &measured.stdout,
        MAKE_NAMESPACE_METADATA_KEY,
        "SYNTHETIC_ENV|environment;",
        "SYNTHETIC_ENV|file;",
    );
    assert!(MakeVariableProgram::from_native_source_observation(
        &paths,
        &changed_namespace,
        &startup,
        &original_environment_names,
        &expected_environment_names,
    )
    .is_err());

    let changed_origin = replace_metadata_value(
        &measured.stdout,
        MAKE_ENVIRONMENT_VALUES_METADATA_KEY,
        "SYNTHETIC_ENV|environment|recursive|",
        "SYNTHETIC_ENV|file|recursive|",
    );
    assert!(MakeVariableProgram::from_native_source_observation(
        &paths,
        &changed_origin,
        &startup,
        &original_environment_names,
        &expected_environment_names,
    )
    .is_err());

    // Packets are consistency data, not authenticated receipts. This mutation
    // remains parseable; replay's selected-source result comparison detects the
    // changed raw value because the source consumes SYNTHETIC_ENV.
    let changed_raw = replace_metadata_value(
        &measured.stdout,
        MAKE_ENVIRONMENT_VALUES_METADATA_KEY,
        "SYNTHETIC_ENV|environment|recursive|prefix-$(LATER)",
        "SYNTHETIC_ENV|environment|recursive|tampered-$(LATER)",
    );
    let mut changed_program = MakeVariableProgram::from_native_source_observation(
        &paths,
        &changed_raw,
        &startup,
        &original_environment_names,
        &expected_environment_names,
    )
    .expect("synthetically altered but structurally valid packet parses");
    read_selected_source(&mut changed_program, &selected_source, &paths);
    let changed_result = changed_program
        .evaluate("$(RESULT)", &paths)
        .expect("evaluate tampered synthetic replay");
    assert!(
        changed_result != native_result,
        "changed raw packet matched native result"
    );

    let mut program = MakeVariableProgram::from_native_source_observation(
        &paths,
        &measured.stdout,
        &startup,
        &original_environment_names,
        &expected_environment_names,
    )
    .expect("compose native initial state without ambient environment reads");
    read_selected_source(&mut program, &selected_source, &paths);
    let replayed_result = program
        .evaluate("$(RESULT)", &paths)
        .expect("evaluate synthetic source result");
    assert!(
        replayed_result == native_result,
        "native source replay differed"
    );
    let exports = program.source_export_values(&paths).unwrap();
    assert_eq!(
        exports.get("SYNTHETIC-ALIAS").map(String::as_str),
        Some("literal-cli-shadow")
    );
    assert_eq!(
        exports.get("SYNTHETIC_ENV").map(String::as_str),
        Some(SYNTHETIC_ENV_RAW)
    );
}

#[test]
fn native_environment_helper_collision_refuses_before_raw_packet_output() {
    let directory = tempfile::tempdir().expect("temporary helper-collision directory");
    let root = directory
        .path()
        .canonicalize()
        .expect("canonical temp directory");
    let observer = root.join("observer.mk");
    let selected_source = root.join("selected.mk");
    fs::write(
        &selected_source,
        ".PHONY: synthetic-goal\nsynthetic-goal:\n",
    )
    .expect("write empty synthetic goal");

    let sentinel = "synthetic-environment-helper-private-921d";
    let mut environment = original_environment();
    environment.insert(
        "__AROS_ENVIRONMENT_VALUES_ITEM".to_owned(),
        sentinel.to_owned(),
    );
    let arguments = startup_arguments(&observer, &selected_source, &[CLI_OVERRIDE]);
    let startup = LiteralMakeStartupArguments::admit(&arguments)
        .expect("synthetic startup arguments use the literal grammar");
    let command_line_names = startup.command_line_names().clone();
    validate_source_prelude_inputs(&environment, &BTreeMap::new())
        .expect("the exact synthetic environment has no active input controls");

    fs::write(&observer, observer_source(&command_line_names, None))
        .expect("write names-only helper counter");
    let names_only = run_native(&arguments, &root, &environment);
    assert!(
        names_only.status.success(),
        "names-only helper counter failed"
    );
    assert!(!contains_bytes(&names_only, sentinel));
    let namespace = ValidatedMakeNamespace::from_make_stdout(&names_only.stdout)
        .expect("parse helper-collision namespace");
    let mut expected = environment_names(&namespace);
    assert!(expected.remove("__AROS_ENVIRONMENT_VALUES_ITEM"));

    fs::write(
        &observer,
        observer_source(&command_line_names, Some(&expected)),
    )
    .expect("write intentionally colliding environment observer");
    let refused = run_native(&arguments, &root, &environment);
    assert!(!refused.status.success());
    assert!(!contains_bytes(
        &refused,
        MAKE_ENVIRONMENT_VALUES_METADATA_KEY
    ));
    assert!(!contains_bytes(&refused, sentinel));
}

#[test]
fn recursive_environment_effect_text_roundtrips_without_source_consumption() {
    let directory = tempfile::tempdir().expect("temporary raw environment directory");
    let root = directory
        .path()
        .canonicalize()
        .expect("canonical temp directory");
    let observer = root.join("observer.mk");
    let selected_source = root.join("selected.mk");
    fs::write(
        &selected_source,
        ".PHONY: synthetic-goal\nsynthetic-goal:\n",
    )
    .expect("write source that does not consume raw value");

    let raw = "literal $(error SHOULD_NOT_RUN), (paren) %|;\n\tend";
    let environment = BTreeMap::from([
        (
            "PATH".to_owned(),
            std::env::var("PATH").expect("test runner provides PATH"),
        ),
        ("SYNTHETIC_RAW_ENV".to_owned(), raw.to_owned()),
    ]);
    let arguments = startup_arguments(
        &observer,
        &selected_source,
        &["SYNTHETIC_CLI=literal-value"],
    );
    let startup = LiteralMakeStartupArguments::admit(&arguments)
        .expect("synthetic startup arguments use the literal grammar");
    let command_line_names = startup.command_line_names().clone();
    validate_source_prelude_inputs(&environment, &BTreeMap::new())
        .expect("the exact synthetic environment has no active input controls");

    fs::write(&observer, observer_source(&command_line_names, None))
        .expect("write names-only raw-value observer");
    let names_only = run_native(&arguments, &root, &environment);
    assert!(
        names_only.status.success(),
        "names-only raw-value query failed"
    );
    assert!(!contains_bytes(&names_only, "SHOULD_NOT_RUN"));
    let namespace = ValidatedMakeNamespace::from_make_stdout(&names_only.stdout)
        .expect("parse raw-value namespace");
    let expected = environment_names(&namespace);
    assert!(expected.contains("SYNTHETIC_RAW_ENV"));

    fs::write(
        &observer,
        observer_source(&command_line_names, Some(&expected)),
    )
    .expect("write opt-in raw-value observer");
    let measured = run_native(&arguments, &root, &environment);
    assert!(
        measured.status.success(),
        "raw environment capture failed: override_guard={}, membership_guard={}, syntax={}, payload_expansion={}",
        contains_bytes(&measured, "Environment-override origin"),
        contains_bytes(&measured, "environment variable membership"),
        contains_bytes(&measured, "unterminated"),
        contains_bytes(&measured, "SHOULD_NOT_RUN")
    );
    let measured_namespace = ValidatedMakeNamespace::from_make_stdout(&measured.stdout)
        .expect("parse opt-in raw-value namespace");
    assert_eq!(measured_namespace, namespace);

    let values = ValidatedMakeEnvironmentValues::from_make_stdout(
        &measured.stdout,
        &measured_namespace,
        &expected,
    )
    .expect("parse raw environment packet");
    let record = values
        .entries()
        .find(|entry| entry.name == "SYNTHETIC_RAW_ENV")
        .expect("raw synthetic environment value was observed");
    assert!(record.value == raw, "raw metacharacters did not roundtrip");
    assert_eq!(record.flavor, MakeInitialValueFlavor::Recursive);
    assert_eq!(record.origin, MakeVariableOrigin::Environment);
}

#[test]
fn native_environment_override_refuses_before_sensitive_packet_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let observer = root.join("observer.mk");
    let selected = root.join("selected.mk");
    fs::write(&selected, "synthetic-goal:\n").unwrap();
    let environment = original_environment();
    let mut arguments = startup_arguments(&observer, &selected, &[]);
    fs::write(&observer, observer_source(&BTreeSet::new(), None)).unwrap();
    let initial = run_native(&arguments, &root, &environment);
    assert!(initial.status.success());
    let namespace = ValidatedMakeNamespace::from_make_stdout(&initial.stdout).unwrap();
    fs::write(
        &observer,
        observer_source(&BTreeSet::new(), Some(&environment_names(&namespace))),
    )
    .unwrap();
    arguments.insert(0, "-e".into());
    assert!(LiteralMakeStartupArguments::admit(&arguments).is_err());
    // Deliberately bypass only with this fixed synthetic flag and empty target:
    // prove the fragment's independent early refusal, not permission to use -e.
    let refused = Command::new(make_program())
        .args(&arguments)
        .current_dir(&root)
        .env_clear()
        .envs(&environment)
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(contains_bytes(
        &refused,
        "Environment-override origin is unsupported"
    ));
    assert!(!contains_bytes(
        &refused,
        MAKE_ENVIRONMENT_VALUES_METADATA_KEY
    ));
    assert!(!contains_bytes(&refused, PRIVATE_ENV_VALUE));
}
