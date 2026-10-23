//! A target-free observation prelude must preserve selected-root semantics.

use super::*;
use aros_common::{
    make_initial_values::{
        MakeInitialValueFlavor, ValidatedMakeInitialValues, ValidatedMakeParserControls,
    },
    make_observer::{pre_file_fragment, source_prelude_fragment, validate_source_prelude_inputs},
};
use std::{
    io::Write as _,
    process::{Command, Output, Stdio},
};

fn native(source: &str, prelude: Option<&str>, silent: bool, initial_list: Option<bool>) -> Output {
    let mut environment =
        std::collections::BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())]);
    let mut command_variables = std::collections::BTreeMap::new();
    match initial_list {
        Some(false) => {
            environment.insert("MAKEFILE_LIST".into(), "environment-list".into());
        }
        Some(true) => {
            command_variables.insert("MAKEFILE_LIST".into(), "command-list".into());
        }
        None => {}
    }
    if prelude.is_some() {
        validate_source_prelude_inputs(&environment, &command_variables).unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let observer = directory.path().join("observer.mk");
    if let Some(prelude) = prelude {
        std::fs::write(&observer, prelude).unwrap();
    }
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let mut command = Command::new(make);
    command.args(["-r", "-n", "--no-print-directory"]);
    if silent {
        command.arg("-s");
    }
    if prelude.is_some() {
        command.arg("-f").arg(&observer).arg("-o").arg(&observer);
    }
    command
        .args([
            "-f",
            "/dev/stdin",
            "-o",
            "/dev/stdin",
            "-o",
            "/dev/null",
            "probe",
        ])
        .env_clear()
        .envs(&environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.args(
        command_variables
            .iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{source}\n$(info OBSERVED=$(CAPTURE))\n").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn observed(output: &Output) -> &str {
    assert!(
        output.status.success(),
        "synthetic source prelude query failed"
    );
    let rows: Vec<_> = std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("OBSERVED="))
        .collect();
    assert_eq!(rows.len(), 1);
    rows[0]
}

fn replay(output: &Output, source: &str, initial_list: Option<bool>) -> String {
    let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
    let values = ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap();
    let controls =
        ValidatedMakeParserControls::from_make_stdout(&output.stdout, &namespace).unwrap();
    let mut initial: Vec<_> = values
        .entries()
        .chain(controls.entries())
        .map(|value| MakeInitialValue {
            name: &value.name,
            value: &value.value,
            origin: value.origin,
            flavor: match value.flavor {
                MakeInitialValueFlavor::Simple => MakeInitialFlavor::Simple,
                MakeInitialValueFlavor::Recursive => MakeInitialFlavor::Recursive,
            },
        })
        .collect();
    if initial_list == Some(true) {
        initial.push(MakeInitialValue {
            name: "MAKEFILE_LIST",
            value: "command-list",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::CommandLine,
        });
    } else {
        let list = initial
            .iter()
            .find(|value| value.name == "MAKEFILE_LIST")
            .unwrap();
        assert_eq!(list.value, "");
        assert_eq!(list.origin, MakeVariableOrigin::File);
        assert!(matches!(list.flavor, MakeInitialFlavor::Simple));
    }
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
        .read_file("/dev/stdin", source, &paths, &|name, from| {
            if name == "/dev/null" && from == "/dev/stdin" {
                Ok(Some(String::new()))
            } else {
                Err("unexpected synthetic include".into())
            }
        })
        .unwrap();
    program.evaluate("$(CAPTURE)", &paths).unwrap()
}

#[test]
fn source_flags_are_observer_free_and_keep_actual_quiet_selection() {
    let source = concat!(
        "MODE := audible\n",
        "ifneq (,$(findstring s,$(MAKEFLAGS)))\nMODE := quiet\nendif\n",
        "FIRST := $(MAKEFILE_LIST)\ninclude /dev/null\n",
        "CAPTURE := $(MODE)|$(MAKEFLAGS)|$(FIRST)|$(MAKEFILE_LIST)\nprobe:\n",
    );
    let prelude = source_prelude_fragment();
    let mut answers = Vec::new();
    for silent in [false, true] {
        let measured = native(source, Some(&prelude), silent, None);
        let ordinary = native(source, None, silent, None);
        assert_eq!(observed(&measured), observed(&ordinary));
        assert_eq!(replay(&measured, source, None), observed(&ordinary));
        assert!(!observed(&measured).contains("--eval"));
        answers.push(observed(&measured).to_owned());
    }
    assert_ne!(
        answers[0], answers[1],
        "the actual native quiet-mode counter must change the answer"
    );
    assert!(answers[0].starts_with("audible|"));
    assert!(answers[1].starts_with("quiet|"));
}

#[test]
fn first_selected_file_list_matches_missing_environment_and_command_inputs() {
    let source = "FIRST := $(MAKEFILE_LIST)\ninclude /dev/null\nCAPTURE := $(FIRST)|$(MAKEFILE_LIST)\nprobe:\n";
    let prelude = source_prelude_fragment();
    for initial_list in [None, Some(false), Some(true)] {
        let measured = native(source, Some(&prelude), true, initial_list);
        let ordinary = native(source, None, true, initial_list);
        assert_eq!(observed(&measured), observed(&ordinary));
        assert_eq!(replay(&measured, source, initial_list), observed(&ordinary));
    }
    let contaminated = native(source, Some(&pre_file_fragment()), true, None);
    let ordinary = native(source, None, true, None);
    assert_ne!(
        observed(&contaminated),
        observed(&ordinary),
        "omitting the prelude reset must retain the extra observer input"
    );
}

#[test]
fn every_observer_input_must_be_protected_against_native_makefile_remaking() {
    for omitted in [None, Some(0), Some(1), Some(2), Some(3)] {
        let directory = tempfile::tempdir().unwrap();
        let observer = directory.path().join("observer.mk");
        let root = directory.path().join("root.mk");
        let included = directory.path().join("extra.mk");
        let marker = directory.path().join("remake-marker");
        std::fs::write(&observer, source_prelude_fragment()).unwrap();
        std::fs::write(&included, "").unwrap();
        std::fs::write(
            &root,
            concat!(
                "include extra.mk\n",
                ".PHONY: force\nforce:\n",
                "%: force\n\t$(file >remake-marker,recipe-expanded)\n",
                "probe: ;\n",
            ),
        )
        .unwrap();
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        let mut command = Command::new(make);
        command
            .args(["-r", "-n", "-s", "--no-print-directory"])
            .arg("-f")
            .arg(&observer)
            .arg("-f")
            .arg(&root)
            .arg("probe")
            .current_dir(directory.path())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap());
        // GNU's old-file option uses the read identity, not a canonical path
        // substitute. Case 3 deliberately supplies the wrong absolute alias.
        let include_identity = if omitted == Some(3) {
            included.as_path()
        } else {
            Path::new("extra.mk")
        };
        for (index, input) in [observer.as_path(), root.as_path(), include_identity]
            .iter()
            .enumerate()
        {
            if omitted != Some(index) {
                command.arg("-o").arg(input);
            }
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "synthetic remake query failed");
        assert_eq!(
            marker.exists(),
            omitted.is_some(),
            "dry-run alone still expands remake recipes; every input needs explicit protection"
        );
        if omitted.is_some() {
            assert_eq!(
                std::fs::read_to_string(&marker).unwrap(),
                "recipe-expanded\n"
            );
        }
    }
}

#[test]
fn preloaded_makefiles_are_refused_before_an_observer_can_erase_their_history() {
    let environment = std::collections::BTreeMap::from([
        ("PATH".into(), std::env::var("PATH").unwrap()),
        ("MAKEFILES".into(), "/dev/null".into()),
    ]);
    let assignments = std::collections::BTreeMap::new();
    let mut launches = 0;
    if validate_source_prelude_inputs(&environment, &assignments).is_ok() {
        launches += 1;
    }
    assert_eq!(
        launches, 0,
        "reject the exact launch inputs before starting Make"
    );
    let directory = tempfile::tempdir().unwrap();
    let observer = directory.path().join("observer.mk");
    std::fs::write(&observer, source_prelude_fragment()).unwrap();
    let unchecked_counter = |observe: bool| {
        let make = if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        };
        let mut command = Command::new(make);
        command.args(["-r", "-n", "-s", "--no-print-directory"]);
        if observe {
            command.arg("-f").arg(&observer).arg("-o").arg(&observer);
        }
        let mut child = command
            .args([
                "-f",
                "/dev/stdin",
                "-o",
                "/dev/stdin",
                "-o",
                "/dev/null",
                "probe",
            ])
            .env_clear()
            .envs(&environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"CAPTURE := $(MAKEFILE_LIST)\nprobe:\n$(info OBSERVED=$(CAPTURE))\n")
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let plain = unchecked_counter(false);
    let corrupted_history = unchecked_counter(true);
    assert_eq!(observed(&plain), "/dev/null /dev/stdin");
    assert_eq!(observed(&corrupted_history), "/dev/stdin");
    assert_ne!(
        observed(&plain),
        observed(&corrupted_history),
        "the rejected preload demonstrably changes the native result"
    );
}
