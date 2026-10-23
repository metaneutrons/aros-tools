//! The native initial packet must precede, not summarize, source evaluation.

use super::*;
use aros_common::{
    make_initial_values::{
        MakeInitialValueFlavor, ValidatedMakeInitialValues, ValidatedMakeParserControls,
    },
    make_observer::pre_file_fragment,
};
use std::io::Write as _;
use std::process::{Command, Output, Stdio};

const ENV_VALUE: &str = "environment-$(LATER)";
const CLI_VALUE: &str = "command-$(LATER)";
const PRIVATE_ENV: &str = "synthetic-private-environment-31f4";
const PRIVATE_CLI: &str = "synthetic-private-command-9cda";
const SOURCE: &str = concat!(
    "ROOT_BEFORE := $(CC)\n",
    "CC = source-compiler\n",
    "LATER := after\n",
    "CAPTURE := $(ROOT_BEFORE)|$(CC)|$(IMPORT_ENV)|$(CLI_INPUT)|$(MAKEFILE_LIST)\n",
    "probe:\n",
);

fn native(
    source: &str,
    builtin_variables: bool,
    observer: bool,
    collision: Option<&str>,
) -> Output {
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let mut command = Command::new(make);
    command.args(["-r", "-n", "-s", "--no-print-directory"]);
    if !builtin_variables {
        command.arg("-R");
    }
    if observer {
        command.arg("--eval").arg(format!(
            "{}$(info NATIVE_LEVEL=$(origin MAKELEVEL)|$(flavor MAKELEVEL)|$(value MAKELEVEL))\n",
            pre_file_fragment()
        ));
    }
    command
        .args(["-f", "/dev/stdin", "probe", "CLI_INPUT=command-$(LATER)"])
        .arg(format!("PRIVATE_CLI={PRIVATE_CLI}"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("IMPORT_ENV", ENV_VALUE)
        .env("PRIVATE_ENV", PRIVATE_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(name) = collision {
        command.arg(format!("{name}=collision"));
    }
    let mut child = command.spawn().unwrap();
    let source = format!(
        "{source}\n$(info OBSERVED=$(CAPTURE))\n$(info HELPERS=$(origin __AROS_NAMESPACE_ITEM)|$(origin __AROS_INITIAL_VALUES_ITEM)|$(origin __AROS_INITIAL_VALUES_NL))\n"
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn observed(output: &Output, key: &str) -> String {
    assert!(output.status.success(), "synthetic Make query failed");
    let text = std::str::from_utf8(&output.stdout).unwrap();
    let values: Vec<_> = text
        .lines()
        .filter_map(|line| line.strip_prefix(key))
        .collect();
    assert_eq!(values.len(), 1, "expected exactly one selected data record");
    values[0].to_owned()
}

#[test]
fn pre_file_defaults_and_explicit_inputs_replay_the_native_source_in_order() {
    for builtin_variables in [true, false] {
        let output = native(SOURCE, builtin_variables, true, None);
        assert!(output.status.success(), "native initial observation failed");
        let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
        let values =
            ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap();
        let controls =
            ValidatedMakeParserControls::from_make_stdout(&output.stdout, &namespace).unwrap();
        for name in [
            "ROOT_BEFORE",
            "CAPTURE",
            "LATER",
            "__AROS_NAMESPACE_ITEM",
            "__AROS_INITIAL_VALUES_ITEM",
            "__AROS_INITIAL_VALUES_NL",
        ] {
            assert!(
                namespace.proves_absent(name),
                "observer is not pre-file or polluted its namespace"
            );
        }
        assert_eq!(
            namespace.origin("IMPORT_ENV"),
            Some(MakeVariableOrigin::Environment)
        );
        assert_eq!(
            namespace.origin("CLI_INPUT"),
            Some(MakeVariableOrigin::CommandLine)
        );
        assert_eq!(
            namespace.origin("CC"),
            builtin_variables.then_some(MakeVariableOrigin::Default)
        );
        assert!(!values.entries().any(|value| matches!(
            value.origin,
            MakeVariableOrigin::Environment | MakeVariableOrigin::CommandLine
        )));
        let packet = std::str::from_utf8(&output.stdout).unwrap();
        assert!(
            !packet.contains(ENV_VALUE),
            "raw environment must not enter the retained packet"
        );
        assert!(
            !packet.contains(CLI_VALUE),
            "raw command inputs must not enter the retained packet"
        );
        assert!(!packet.contains(PRIVATE_ENV));
        assert!(!packet.contains(PRIVATE_CLI));

        let mut initial: Vec<_> = values
            .entries()
            .chain(controls.entries())
            .map(|entry| MakeInitialValue {
                name: &entry.name,
                value: &entry.value,
                flavor: match entry.flavor {
                    MakeInitialValueFlavor::Simple => MakeInitialFlavor::Simple,
                    MakeInitialValueFlavor::Recursive => MakeInitialFlavor::Recursive,
                },
                origin: entry.origin,
            })
            .collect();
        initial.extend([
            MakeInitialValue {
                name: "IMPORT_ENV",
                value: ENV_VALUE,
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Environment,
            },
            MakeInitialValue {
                name: "CLI_INPUT",
                value: CLI_VALUE,
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::CommandLine,
            },
        ]);
        let dirs = DirVars::default();
        let cwd = std::env::current_dir().unwrap();
        let paths = MakeProgramPaths {
            dirs: &dirs,
            source_root: &cwd,
            relative_dir: Path::new(""),
            top_directory: &cwd,
            working_directory: &cwd,
        };
        let mut program =
            MakeVariableProgram::new_source_program(&paths, &namespace, &initial).unwrap();
        program
            .read_file("/dev/stdin", SOURCE, &paths, &|_, _| Ok(None))
            .unwrap();
        assert_eq!(
            program.evaluate("$(CAPTURE)", &paths).unwrap(),
            observed(&output, "OBSERVED=")
        );
        let unobserved = native(SOURCE, builtin_variables, false, None);
        assert_eq!(
            observed(&output, "OBSERVED="),
            observed(&unobserved, "OBSERVED=")
        );
        assert_eq!(
            observed(&output, "HELPERS="),
            "undefined|undefined|undefined"
        );

        let missing_environment: Vec<_> = initial
            .iter()
            .copied()
            .filter(|value| value.name != "IMPORT_ENV")
            .collect();
        let mut incomplete =
            MakeVariableProgram::new_source_program(&paths, &namespace, &missing_environment)
                .unwrap();
        assert!(incomplete
            .read_file("/dev/stdin", SOURCE, &paths, &|_, _| Ok(None))
            .is_err());
        assert!(
            incomplete.events().is_empty(),
            "unmeasured environment must not yield partial source state"
        );
    }
    let with_defaults = native(SOURCE, true, true, None);
    let without_defaults = native(SOURCE, false, true, None);
    assert_ne!(
        observed(&with_defaults, "OBSERVED="),
        observed(&without_defaults, "OBSERVED="),
        "the -R counter must change actual source evaluation"
    );
}

#[test]
fn pre_file_observer_refuses_helper_collisions_before_publishing_a_packet() {
    for helper in [
        "__AROS_NAMESPACE_ITEM",
        "__AROS_INITIAL_VALUES_ITEM",
        "__AROS_INITIAL_VALUES_NL",
    ] {
        let output = native(SOURCE, true, true, Some(helper));
        assert!(!output.status.success());
        let stdout = std::str::from_utf8(&output.stdout).unwrap();
        assert!(!stdout.contains("AROS_MAKE_INITIAL_NAMESPACE="));
        assert!(!stdout.contains("AROS_MAKE_INITIAL_VALUES="));
    }
}

#[test]
fn native_simple_environment_flavor_keeps_append_timing() {
    let source = concat!(
        "LATER := first\n",
        "MAKELEVEL += $(LATER)\n",
        "LATER := second\n",
        "CAPTURE := $(MAKELEVEL)\n",
        "probe:\n",
    );
    let output = native(source, true, true, None);
    let state = observed(&output, "NATIVE_LEVEL=");
    let fields: Vec<_> = state.split('|').collect();
    assert_eq!(fields.len(), 3);
    assert_eq!(&fields[..2], &["environment", "simple"]);
    let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
    let controls =
        ValidatedMakeParserControls::from_make_stdout(&output.stdout, &namespace).unwrap();
    let parser_initial: Vec<_> = controls
        .entries()
        .map(|entry| MakeInitialValue {
            name: &entry.name,
            value: &entry.value,
            flavor: match entry.flavor {
                MakeInitialValueFlavor::Simple => MakeInitialFlavor::Simple,
                MakeInitialValueFlavor::Recursive => MakeInitialFlavor::Recursive,
            },
            origin: entry.origin,
        })
        .collect();
    let dirs = DirVars::default();
    let cwd = std::env::current_dir().unwrap();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &cwd,
        relative_dir: Path::new(""),
        top_directory: &cwd,
        working_directory: &cwd,
    };
    let initial = |flavor| MakeInitialValue {
        name: "MAKELEVEL",
        value: fields[2],
        flavor,
        origin: MakeVariableOrigin::Environment,
    };
    let mut measured_initial = vec![initial(MakeInitialFlavor::Simple)];
    measured_initial.extend(parser_initial.iter().copied());
    let mut measured =
        MakeVariableProgram::new_source_program(&paths, &namespace, &measured_initial).unwrap();
    measured
        .read_file("/dev/stdin", source, &paths, &|_, _| Ok(None))
        .unwrap();
    assert_eq!(
        measured.evaluate("$(CAPTURE)", &paths).unwrap(),
        observed(&output, "OBSERVED=")
    );
    let mut guessed_initial = vec![initial(MakeInitialFlavor::Recursive)];
    guessed_initial.extend(parser_initial.iter().copied());
    let mut guessed =
        MakeVariableProgram::new_source_program(&paths, &namespace, &guessed_initial).unwrap();
    guessed
        .read_file("/dev/stdin", source, &paths, &|_, _| Ok(None))
        .unwrap();
    assert_ne!(
        guessed.evaluate("$(CAPTURE)", &paths).unwrap(),
        observed(&output, "OBSERVED="),
        "assuming recursive environment flavor must change the answer"
    );
}

#[test]
fn administrative_dependencies_are_not_normalized_or_invented() {
    let source = "CAPTURE := $(MAKEFLAGS)\nprobe:\n";
    let output = native(source, true, true, None);
    assert!(output.status.success());
    let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
    let values = ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap();
    let controls =
        ValidatedMakeParserControls::from_make_stdout(&output.stdout, &namespace).unwrap();
    let initial: Vec<_> = values
        .entries()
        .chain(controls.entries())
        .map(|entry| MakeInitialValue {
            name: &entry.name,
            value: &entry.value,
            flavor: match entry.flavor {
                MakeInitialValueFlavor::Simple => MakeInitialFlavor::Simple,
                MakeInitialValueFlavor::Recursive => MakeInitialFlavor::Recursive,
            },
            origin: entry.origin,
        })
        .collect();
    let dirs = DirVars::default();
    let cwd = std::env::current_dir().unwrap();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &cwd,
        relative_dir: Path::new(""),
        top_directory: &cwd,
        working_directory: &cwd,
    };
    let mut program =
        MakeVariableProgram::new_source_program(&paths, &namespace, &initial).unwrap();
    let result = program.read_file("/dev/stdin", source, &paths, &|_, _| Ok(None));
    assert!(
        matches!(result, Err(MakeExprError::InvalidSyntax { detail, .. })
        if detail.contains("unsafe Make variable `-*-eval-flags-*-`")
            && detail.contains("initial Make variable value or absence has not been measured"))
    );
    assert!(program.events().is_empty());
}

#[test]
fn literal_private_input_counter_really_reaches_make_but_not_the_packet() {
    let output = native(
        "CAPTURE := $(PRIVATE_CLI)|$(PRIVATE_ENV)\nprobe:\n",
        true,
        true,
        None,
    );
    assert_eq!(
        observed(&output, "OBSERVED="),
        format!("{PRIVATE_CLI}|{PRIVATE_ENV}")
    );
    let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
    let values = ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap();
    let _controls =
        ValidatedMakeParserControls::from_make_stdout(&output.stdout, &namespace).unwrap();
    assert!(!values
        .entries()
        .any(|entry| entry.value.contains(PRIVATE_CLI) || entry.value.contains(PRIVATE_ENV)));
}
