use super::{MakeProgramPaths, MakeVariableProgram};
use crate::{
    dirs::DirVars,
    make_vars::{MakeInitialFlavor, MakeInitialValue},
    parser::TargetContext,
};
use aros_common::make_child_command::LiteralMakeChildCommand;
use aros_common::make_namespace::{
    MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write as _,
    path::Path,
    process::{Command, Stdio},
};

fn namespace(entries: &[(&str, MakeVariableOrigin)]) -> ValidatedMakeNamespace {
    let mut all_entries = vec![
        MakeNamespaceEntry {
            name: ".VARIABLES".into(),
            origin: MakeVariableOrigin::Default,
        },
        MakeNamespaceEntry {
            name: "MAKE_VERSION".into(),
            origin: MakeVariableOrigin::Default,
        },
    ];
    all_entries.extend(entries.iter().map(|(name, origin)| MakeNamespaceEntry {
        name: (*name).into(),
        origin: *origin,
    }));
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: all_entries,
    })
    .unwrap()
}

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/source"),
        relative_dir: Path::new("unit"),
        top_directory: Path::new("/build"),
        working_directory: Path::new("/build"),
    }
}

fn environment_names(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).into()).collect()
}

/// Run only a fixed synthetic Makefile and print only the named environment
/// fields. No Source files, recipes, or ambient environment are involved.
fn native_make_output(
    body: &str,
    environment: &[(&str, &str)],
    command_line: &[&str],
    fields: &[&str],
) -> String {
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let keys = fields
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(",");
    let python = format!(
        "import os; keys=({keys},); print('AROS_TEST_ENV=' + '|'.join(k + '=' + (os.environ[k] if k in os.environ else '<missing>') for k in keys))"
    );
    let source = format!(
        "{body}\n.PHONY: codex_export_probe\ncodex_export_probe:\n\t@python3 -c \"{python}\"\n"
    );
    let mut command = Command::new(make);
    command
        .args([
            "-rR",
            "--no-print-directory",
            "-s",
            "-f",
            "-",
            "codex_export_probe",
        ])
        .args(command_line)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap());
    for (name, value) in environment {
        // Install only the fixed synthetic values declared by each test.
        command.env(name, value);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "synthetic GNU Make export query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn native_environment_fields(
    body: &str,
    environment: &[(&str, &str)],
    command_line: &[&str],
    fields: &[&str],
) -> String {
    let text = native_make_output(body, environment, command_line, fields);
    let rows = text
        .lines()
        .filter_map(|line| line.strip_prefix("AROS_TEST_ENV="))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1, "unexpected synthetic probe output: {text}");
    rows[0].to_owned()
}

#[test]
fn measured_environment_and_command_line_origins_select_source_exports() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let namespace = namespace(&[
        ("ENV_REPLACED", MakeVariableOrigin::Environment),
        ("HAS-DASH", MakeVariableOrigin::Environment),
        ("1DIGIT", MakeVariableOrigin::Environment),
        ("CLI_VALUE", MakeVariableOrigin::CommandLine),
    ]);
    let initial = [
        MakeInitialValue {
            name: "ENV_REPLACED",
            value: "environment-value",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
        MakeInitialValue {
            name: "HAS-DASH",
            value: "dash-value",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
        MakeInitialValue {
            name: "1DIGIT",
            value: "digit-value",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
        MakeInitialValue {
            name: "CLI_VALUE",
            value: "command-line-value",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::CommandLine,
        },
    ];
    let body = concat!(
        "SEED := first\n",
        "export SIMPLE := $(SEED)\n",
        "export RECURSIVE = $(SEED)\n",
        "ENV_REPLACED := file-value\n",
        "CLI_VALUE := file-ignored\n",
        "SEED := later\n",
    );
    let initial_environment_names = environment_names(&["ENV_REPLACED", "HAS-DASH", "1DIGIT"]);
    let incomplete_environment_names = environment_names(&["ENV_REPLACED", "HAS-DASH"]);
    assert!(MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &incomplete_environment_names,
    )
    .is_err());
    let unknown_environment_names =
        environment_names(&["ENV_REPLACED", "HAS-DASH", "1DIGIT", "NOT_MEASURED"]);
    assert!(MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &unknown_environment_names,
    )
    .is_err());
    let mut program = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &initial_environment_names,
    )
    .unwrap();
    program
        .read_file("root.mk", body, &paths, &|_, _| Ok(None))
        .unwrap();
    let projected = program.source_export_values(&paths).unwrap();
    let expected = BTreeMap::from([
        ("1DIGIT".into(), "digit-value".into()),
        ("CLI_VALUE".into(), "command-line-value".into()),
        ("ENV_REPLACED".into(), "file-value".into()),
        ("HAS-DASH".into(), "dash-value".into()),
        ("RECURSIVE".into(), "later".into()),
        ("SIMPLE".into(), "first".into()),
    ]);
    assert_eq!(projected, expected);
    let command = LiteralMakeChildCommand::parse("env EXTRA=prefix /bin/true").unwrap();
    let child_environment = command.apply_environment(&projected).unwrap();
    assert_eq!(child_environment["HAS-DASH"], "dash-value");
    assert_eq!(child_environment["1DIGIT"], "digit-value");
    assert_eq!(child_environment["EXTRA"], "prefix");
    assert!(LiteralMakeChildCommand::parse("env HAS-DASH=bad /bin/true").is_err());
    assert!(LiteralMakeChildCommand::parse("env 1DIGIT=bad /bin/true").is_err());
    assert_eq!(
        native_environment_fields(
            body,
            &[
                ("ENV_REPLACED", "environment-value"),
                ("HAS-DASH", "dash-value"),
                ("1DIGIT", "digit-value"),
            ],
            &["CLI_VALUE=command-line-value"],
            &[
                "1DIGIT",
                "CLI_VALUE",
                "ENV_REPLACED",
                "HAS-DASH",
                "RECURSIVE",
                "SIMPLE"
            ],
        ),
        "1DIGIT=digit-value|CLI_VALUE=command-line-value|ENV_REPLACED=file-value|HAS-DASH=dash-value|RECURSIVE=later|SIMPLE=first"
    );
}

#[test]
fn command_line_auto_export_uses_make_eligible_names() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let namespace = namespace(&[
        ("CLI_VALUE", MakeVariableOrigin::CommandLine),
        ("HAS-DASH", MakeVariableOrigin::CommandLine),
        ("1DIGIT", MakeVariableOrigin::CommandLine),
    ]);
    let initial = [
        MakeInitialValue {
            name: "CLI_VALUE",
            value: "command-line-value",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::CommandLine,
        },
        MakeInitialValue {
            name: "HAS-DASH",
            value: "dash-command-line",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::CommandLine,
        },
        MakeInitialValue {
            name: "1DIGIT",
            value: "digit-command-line",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::CommandLine,
        },
    ];
    let no_initial_environment_names = environment_names(&[]);
    let mut program = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &no_initial_environment_names,
    )
    .unwrap();
    assert_eq!(
        program.source_export_values(&paths).unwrap(),
        BTreeMap::from([("CLI_VALUE".into(), "command-line-value".into())])
    );
    assert_eq!(
        native_environment_fields(
            "",
            &[],
            &[
                "CLI_VALUE=command-line-value",
                "HAS-DASH=dash-command-line",
                "1DIGIT=digit-command-line",
            ],
            &["CLI_VALUE", "HAS-DASH", "1DIGIT"],
        ),
        "CLI_VALUE=command-line-value|HAS-DASH=<missing>|1DIGIT=<missing>"
    );

    // The initial environment membership survives a command-line override:
    // Make exports the final command-line value for an originally inherited
    // name even when that name cannot be auto-exported from CLI origin alone.
    let inherited_names = environment_names(&["HAS-DASH", "1DIGIT"]);
    let mut inherited = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &inherited_names,
    )
    .unwrap();
    assert_eq!(
        inherited.source_export_values(&paths).unwrap(),
        BTreeMap::from([
            ("1DIGIT".into(), "digit-command-line".into()),
            ("CLI_VALUE".into(), "command-line-value".into()),
            ("HAS-DASH".into(), "dash-command-line".into()),
        ])
    );
    assert_eq!(
        native_environment_fields(
            "",
            &[
                ("HAS-DASH", "environment-dash"),
                ("1DIGIT", "environment-digit"),
            ],
            &[
                "CLI_VALUE=command-line-value",
                "HAS-DASH=dash-command-line",
                "1DIGIT=digit-command-line",
            ],
            &["HAS-DASH", "1DIGIT"],
        ),
        "HAS-DASH=dash-command-line|1DIGIT=digit-command-line"
    );

    let mut missing_provenance =
        MakeVariableProgram::new_source_program(&paths, &namespace, &initial).unwrap();
    let before = missing_provenance.events().to_vec();
    assert!(missing_provenance.source_export_values(&paths).is_err());
    assert_eq!(missing_provenance.events(), before);
}

#[test]
fn untouched_environment_exports_preserve_raw_data_without_expanding_it() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let namespace = namespace(&[
        ("RAW_ENV", MakeVariableOrigin::Environment),
        ("RAW_REFERENCE", MakeVariableOrigin::Environment),
        ("SIMPLE_DOLLAR", MakeVariableOrigin::Environment),
    ]);
    let raw = "$(eval INJECTED := bad)";
    let reference = "$(LATER)";
    let simple = "$literal$(LATER)";
    let initial = [
        MakeInitialValue {
            name: "RAW_ENV",
            value: raw,
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
        MakeInitialValue {
            name: "RAW_REFERENCE",
            value: reference,
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
        MakeInitialValue {
            name: "SIMPLE_DOLLAR",
            value: simple,
            flavor: MakeInitialFlavor::Simple,
            origin: MakeVariableOrigin::Environment,
        },
    ];
    let source = "LATER := filevalue\n";
    let inherited_names = environment_names(&["RAW_ENV", "RAW_REFERENCE", "SIMPLE_DOLLAR"]);
    let mut projected = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &inherited_names,
    )
    .unwrap();
    projected
        .read_file("root.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();
    let events_before = projected.events().to_vec();
    assert_eq!(
        projected.source_export_values(&paths).unwrap(),
        BTreeMap::from([
            ("RAW_ENV".into(), raw.into()),
            ("RAW_REFERENCE".into(), reference.into()),
            ("SIMPLE_DOLLAR".into(), simple.into()),
        ])
    );
    assert_eq!(projected.events(), events_before);
    assert_eq!(projected.evaluate("$(INJECTED)", &paths).unwrap(), "");
    assert_eq!(
        projected.evaluate("$(RAW_REFERENCE)", &paths).unwrap(),
        "filevalue"
    );
    assert_eq!(
        projected.evaluate("$(SIMPLE_DOLLAR)", &paths).unwrap(),
        simple
    );

    let native = native_make_output(
        "LATER := filevalue\n$(info AROS_TEST_MAKE_VALUE=$(RAW_REFERENCE))",
        &[("RAW_REFERENCE", reference)],
        &[],
        &["RAW_REFERENCE"],
    );
    assert!(native
        .lines()
        .any(|line| line == "AROS_TEST_MAKE_VALUE=filevalue"));
    assert!(native
        .lines()
        .any(|line| line == "AROS_TEST_ENV=RAW_REFERENCE=$(LATER)"));

    // Evaluating this recursive Make value would run its $(eval ...) payload;
    // the export projection above treats the measured environment as data.
    let mut evaluated = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &inherited_names,
    )
    .unwrap();
    evaluated
        .read_file("root.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();
    let events_before_evaluation = evaluated.events().to_vec();
    assert_eq!(evaluated.evaluate("$(RAW_ENV)", &paths).unwrap(), "");
    assert_eq!(evaluated.evaluate("$(INJECTED)", &paths).unwrap(), "bad");
    assert_ne!(evaluated.events(), events_before_evaluation);
}

#[test]
fn includes_conditionals_flavors_and_command_line_priority_are_source_ordered() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let namespace = namespace(&[
        ("MODE", MakeVariableOrigin::CommandLine),
        ("CLI_LOCK", MakeVariableOrigin::CommandLine),
    ]);
    let initial = [
        MakeInitialValue {
            name: "MODE",
            value: "enabled",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::CommandLine,
        },
        MakeInitialValue {
            name: "CLI_LOCK",
            value: "command-line-value",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::CommandLine,
        },
    ];
    let body = concat!(
        "ROOT := early\n",
        "CLI_LOCK := file-ignored\n",
        "include selected.mk\n",
        "ifeq ($(MODE),enabled)\n",
        "export BRANCH := selected\n",
        "else\n",
        "export BRANCH := rejected\n",
        "endif\n",
        "export FROZEN := $(ROOT)\n",
        "export LIVE = $(ROOT)\n",
        "ROOT := later\n",
    );
    let no_initial_environment_names = environment_names(&[]);
    let mut program = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &no_initial_environment_names,
    )
    .unwrap();
    program
        .read_file("root.mk", body, &paths, &|name, _| {
            Ok((name == "selected.mk").then(|| "export INCLUDED := $(ROOT)\n".into()))
        })
        .unwrap();
    assert_eq!(
        program.source_export_values(&paths).unwrap(),
        BTreeMap::from([
            ("BRANCH".into(), "selected".into()),
            ("CLI_LOCK".into(), "command-line-value".into()),
            ("FROZEN".into(), "early".into()),
            ("INCLUDED".into(), "early".into()),
            ("LIVE".into(), "later".into()),
            ("MODE".into(), "enabled".into()),
        ])
    );
}

#[test]
fn unexport_undefine_redefinition_and_empty_export_match_gnu_make() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let namespace = namespace(&[
        ("ENV_UNEXPORT", MakeVariableOrigin::Environment),
        ("ENV_UNDEFINED", MakeVariableOrigin::Environment),
        ("ENV_REDEFINED", MakeVariableOrigin::Environment),
    ]);
    let initial = [
        MakeInitialValue {
            name: "ENV_UNEXPORT",
            value: "before-unexport",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
        MakeInitialValue {
            name: "ENV_UNDEFINED",
            value: "before-undefine",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
        MakeInitialValue {
            name: "ENV_REDEFINED",
            value: "before-redefinition",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
    ];
    let body = concat!(
        "unexport ENV_UNEXPORT\n",
        "ENV_UNEXPORT := after-unexport\n",
        "undefine ENV_UNDEFINED\n",
        "undefine ENV_REDEFINED\n",
        "ENV_REDEFINED := recreated\n",
        "export EMPTY_CREATED\n",
    );
    let initial_environment_names =
        environment_names(&["ENV_UNEXPORT", "ENV_UNDEFINED", "ENV_REDEFINED"]);
    let mut program = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &namespace,
        &initial,
        &initial_environment_names,
    )
    .unwrap();
    program
        .read_file("root.mk", body, &paths, &|_, _| Ok(None))
        .unwrap();
    assert_eq!(
        program.source_export_values(&paths).unwrap(),
        BTreeMap::from([("EMPTY_CREATED".into(), String::new())])
    );
    assert_eq!(
        native_environment_fields(
            body,
            &[
                ("ENV_UNEXPORT", "before-unexport"),
                ("ENV_UNDEFINED", "before-undefine"),
                ("ENV_REDEFINED", "before-redefinition"),
            ],
            &[],
            &[
                "ENV_UNEXPORT",
                "ENV_UNDEFINED",
                "ENV_REDEFINED",
                "EMPTY_CREATED"
            ],
        ),
        "ENV_UNEXPORT=<missing>|ENV_UNDEFINED=<missing>|ENV_REDEFINED=<missing>|EMPTY_CREATED="
    );
}

#[test]
fn unmeasured_values_and_uncertain_unexport_refuse_without_projection_mutation() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let unmeasured_namespace = namespace(&[("UNMEASURED_ENV", MakeVariableOrigin::Environment)]);
    let unmeasured_names = environment_names(&["UNMEASURED_ENV"]);
    let mut unmeasured = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &unmeasured_namespace,
        &[],
        &unmeasured_names,
    )
    .unwrap();
    let before = unmeasured.events().to_vec();
    assert!(unmeasured.source_export_values(&paths).is_err());
    assert_eq!(unmeasured.events(), before);

    let known_namespace = namespace(&[("KNOWN_ENV", MakeVariableOrigin::Environment)]);
    let known_initial = [MakeInitialValue {
        name: "KNOWN_ENV",
        value: "measured",
        flavor: MakeInitialFlavor::Recursive,
        origin: MakeVariableOrigin::Environment,
    }];
    let known_environment_names = environment_names(&["KNOWN_ENV"]);
    let mut uncertain = MakeVariableProgram::new_source_program_with_environment(
        &paths,
        &known_namespace,
        &known_initial,
        &known_environment_names,
    )
    .unwrap();
    uncertain
        .observe("unexport KNOWN_ENV", 7, None, &paths)
        .unwrap();
    let before_projection = uncertain.events().to_vec();
    assert!(uncertain.source_export_values(&paths).is_err());
    assert_eq!(uncertain.events(), before_projection);
    assert_eq!(
        uncertain.evaluate("$(KNOWN_ENV)", &paths).unwrap(),
        "measured"
    );
}

#[test]
fn source_exports_require_supported_context_and_retain_uncertain_export_history() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut legacy = MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap();
    assert!(legacy.source_export_values(&paths).is_err());
    let overridden = namespace(&[("ENV", MakeVariableOrigin::EnvironmentOverride)]);
    let mut unsupported =
        MakeVariableProgram::new_source_program(&paths, &overridden, &[]).unwrap();
    assert!(unsupported.source_export_values(&paths).is_err());

    let ns = namespace(&[("ENV", MakeVariableOrigin::Environment)]);
    let initial = [MakeInitialValue {
        name: "ENV",
        value: "original",
        flavor: MakeInitialFlavor::Recursive,
        origin: MakeVariableOrigin::Environment,
    }];
    let mut uncertain = MakeVariableProgram::new_source_program(&paths, &ns, &initial).unwrap();
    uncertain.observe("undefine ENV", 0, None, &paths).unwrap();
    uncertain.execute("ENV := replacement", 1, &paths).unwrap();
    assert_eq!(uncertain.evaluate("$(ENV)", &paths).unwrap(), "replacement");
    // Resolving the value does not establish whether the inherited export
    // property was destroyed by the earlier unresolved undefine.
    assert!(uncertain.source_export_values(&paths).is_err());
    uncertain.execute("export ENV", 2, &paths).unwrap();
    assert_eq!(
        uncertain.source_export_values(&paths).unwrap(),
        BTreeMap::from([("ENV".into(), "replacement".into()),])
    );
}
