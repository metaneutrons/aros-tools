use super::{MakeProgramPaths, MakeVariableProgram};
use crate::dirs::DirVars;
use aros_common::make_namespace::{
    MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
};
use std::{
    path::Path,
    process::{Command, Stdio},
};

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/selected/source"),
        relative_dir: Path::new("owner"),
        top_directory: Path::new("/selected/build"),
        working_directory: Path::new("/selected/build/owner"),
    }
}

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .into_iter()
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .collect(),
    })
    .unwrap()
}

fn program(paths: &MakeProgramPaths<'_>) -> MakeVariableProgram {
    MakeVariableProgram::new_source_program(paths, &namespace(), &[]).unwrap()
}

fn native(source: &str, expression: &str) -> String {
    use std::io::Write;

    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let input = format!("{source}\n$(info AROS_TEST_RESULT={expression})\nprobe:\n");
    let mut child = Command::new(make)
        .args([
            "-rR",
            "--no-print-directory",
            "-s",
            "-n",
            "-f",
            "/dev/stdin",
            "probe",
        ])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "synthetic Make oracle failed");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let rows = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("AROS_TEST_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    rows[0].to_owned()
}

#[test]
fn source_names_are_ordinary_variables_and_only_measured_values_expand() {
    let dirs = DirVars::from_config_text("");
    let paths = paths(&dirs);
    let mut program = program(&paths);
    let initial = concat!(
        "${AROS_SOURCE_DIR}|$(AROS_SOURCE_DIR)|${AROS_BUILD_DIR}|$(AROS_BUILD_DIR)|",
        "${AROS_PORTS_DIR}|$(AROS_PORTS_DIR)|${CMAKE_CURRENT_BINARY_DIR}|$(CMAKE_CURRENT_BINARY_DIR)|",
        "$(shell echo '${AROS_SOURCE_DIR}')|$(shell echo '$${AROS_SOURCE_DIR}')|",
        "$(abspath ${AROS_SOURCE_DIR}/leaf)",
    );
    let mut absent_fields = vec![""; 9];
    absent_fields.extend(["${AROS_SOURCE_DIR}", "/leaf"]);
    let absent = absent_fields.join("|");
    assert_eq!(program.evaluate(initial, &paths).unwrap(), absent);
    assert_eq!(native("", initial), absent);

    let source = concat!(
        "BEFORE := ${AROS_SOURCE_DIR}|$(AROS_SOURCE_DIR)|${AROS_BUILD_DIR}|$(AROS_BUILD_DIR)|",
        "${AROS_PORTS_DIR}|$(AROS_PORTS_DIR)|${CMAKE_CURRENT_BINARY_DIR}|$(CMAKE_CURRENT_BINARY_DIR)\n",
        "AROS_SOURCE_DIR := /declared/source/tree\n",
        "AROS_BUILD_DIR := /declared/build/root\n",
        "AROS_PORTS_DIR := /declared/ports\n",
        "CMAKE_CURRENT_BINARY_DIR := /declared/build/binary\n",
        "DEFINED := ${AROS_SOURCE_DIR}|$(AROS_SOURCE_DIR)|${AROS_BUILD_DIR}|$(AROS_BUILD_DIR)|",
        "${AROS_PORTS_DIR}|$(AROS_PORTS_DIR)|${CMAKE_CURRENT_BINARY_DIR}|$(CMAKE_CURRENT_BINARY_DIR)\n",
        "SHELL_VALUE := $(shell echo '${AROS_SOURCE_DIR}')\n",
        "SHELL_LITERAL := $(shell echo '$${AROS_SOURCE_DIR}')\n",
        "ABSOLUTE := $(abspath ${AROS_SOURCE_DIR}/../leaf)|$(abspath ${AROS_BUILD_DIR}/../generated)\n",
        "undefine AROS_SOURCE_DIR\nundefine AROS_BUILD_DIR\n",
        "undefine AROS_PORTS_DIR\nundefine CMAKE_CURRENT_BINARY_DIR\n",
        "AFTER := ${AROS_SOURCE_DIR}|$(AROS_SOURCE_DIR)|${AROS_BUILD_DIR}|$(AROS_BUILD_DIR)|",
        "${AROS_PORTS_DIR}|$(AROS_PORTS_DIR)|${CMAKE_CURRENT_BINARY_DIR}|$(CMAKE_CURRENT_BINARY_DIR)\n",
    );
    program
        .read_file("root.mk", source, &paths, &|_, _| Ok(None))
        .unwrap();
    let query = "$(BEFORE)|$(DEFINED)|$(SHELL_VALUE)|$(SHELL_LITERAL)|$(ABSOLUTE)|$(AFTER)";
    let empty = ["", "", "", "", "", "", "", ""].join("|");
    let defined = [
        "/declared/source/tree",
        "/declared/source/tree",
        "/declared/build/root",
        "/declared/build/root",
        "/declared/ports",
        "/declared/ports",
        "/declared/build/binary",
        "/declared/build/binary",
    ]
    .join("|");
    let expected = format!(
        "{empty}|{defined}|/declared/source/tree|${{AROS_SOURCE_DIR}}|/declared/source/leaf|/declared/build/generated|{empty}"
    );
    assert_eq!(program.evaluate(query, &paths).unwrap(), expected);
    assert_eq!(native(source, query), expected);

    let changed_source = source.replace(
        "AROS_SOURCE_DIR := /declared/source/tree",
        "AROS_SOURCE_DIR := /changed/source/tree",
    );
    assert_ne!(native(&changed_source, query), expected);
}
