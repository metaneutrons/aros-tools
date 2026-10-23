//! Captured shell data remains bounded, source-defined and transactional.

use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::ValidatedMakeNamespace,
    measure_tree_content_cas,
};
use std::{path::Path, process::Command};

fn snapshot(root: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        root,
        &[MakeInputRootSelection {
            root: root.to_owned(),
            content_sha256: measure_tree_content_cas(root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap()
}

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::from_measurement("v1:2;.VARIABLES|default;MAKE_VERSION|default;end")
        .unwrap()
}

fn paths<'a>(root: &'a Path, dirs: &'a DirVars) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn native_observation(root: &Path) -> String {
    let output = Command::new(if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    })
    .current_dir(root)
    .env_clear()
    .env("PATH", std::env::var("PATH").unwrap())
    .env("LC_ALL", "C")
    .args([
        "-r",
        "-n",
        "-s",
        "--no-print-directory",
        "-f",
        "root.mk",
        "--old-file=root.mk",
        "probe",
    ])
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "synthetic native shell query failed"
    );
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let observations = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OBSERVED="))
        .collect::<Vec<_>>();
    assert_eq!(observations.len(), 1);
    observations[0].to_owned()
}

#[test]
fn source_shell_loop_matches_gnu_make_and_records_status_at_assignment_time() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("data")).unwrap();
    std::fs::write(root.join("data/alpha.txt"), "synthetic alpha").unwrap();
    std::fs::write(root.join("data/space file.txt"), "synthetic space").unwrap();
    std::fs::write(
        root.join("root.mk"),
        concat!(
            "BASE := data\n",
            "SCAN = $(shell cd \"$(BASE)\"; for renamed in $(1); do if [ -f \"$$renamed\" ]; then printf '%s\\n' \"$$renamed\"; fi; done)\n",
            "SCANNED := $(call SCAN,alpha.txt \"space file.txt\" absent.txt)\n",
            "STATUS_AFTER_SCAN := $(origin .SHELLSTATUS)|$(flavor .SHELLSTATUS)|$(.SHELLSTATUS)\n",
            "RAN_TRUE := $(shell true)\n",
            "STATUS_AFTER_TRUE := $(origin .SHELLSTATUS)|$(flavor .SHELLSTATUS)|$(.SHELLSTATUS)\n",
            "RAN_FALSE := $(shell false)\n",
            "STATUS_AFTER_FALSE := $(origin .SHELLSTATUS)|$(flavor .SHELLSTATUS)|$(.SHELLSTATUS)\n",
            "probe:\n",
            "\t$(info OBSERVED=$(SCANNED)|$(STATUS_AFTER_TRUE)|$(STATUS_AFTER_FALSE))\n",
        ),
    )
    .unwrap();

    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &captured)
        .unwrap();

    assert_eq!(
        program.evaluate("$(SCANNED)", &paths).unwrap(),
        "alpha.txt space file.txt"
    );
    assert_eq!(
        program.evaluate("$(STATUS_AFTER_SCAN)", &paths).unwrap(),
        "override|simple|0"
    );
    assert_eq!(
        program.evaluate("$(STATUS_AFTER_TRUE)", &paths).unwrap(),
        "override|simple|0"
    );
    assert_eq!(
        program.evaluate("$(STATUS_AFTER_FALSE)", &paths).unwrap(),
        "override|simple|1"
    );
    assert_eq!(
        native_observation(&root),
        "alpha.txt space file.txt|override|simple|0|override|simple|1"
    );
}

#[test]
fn captured_shell_refusals_roll_back_values_status_and_events() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("data")).unwrap();
    std::fs::write(root.join("data/alpha.txt"), "synthetic input").unwrap();
    std::fs::write(
        root.join("good.mk"),
        "VALUE := stable\nINITIAL := $(shell false)\n",
    )
    .unwrap();
    for (name, command) in [
        (
            "unsupported.mk",
            "VALUE := changed\nSTAGED := $(shell true)\nBAD := $(shell uname)\n",
        ),
        (
            "unbound.mk",
            "VALUE := changed\nSTAGED := $(shell true)\nBAD := $(shell printf '%s' \"$$UNBOUND_PRIVATE\")\n",
        ),
    ] {
        std::fs::write(root.join(name), command).unwrap();
    }
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("good.mk", &paths, &captured)
        .unwrap();
    let prior_events = program.events().to_vec();

    for name in ["unsupported.mk", "unbound.mk"] {
        assert!(program.read_captured_file(name, &paths, &captured).is_err());
        assert_eq!(program.events(), prior_events);
        assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "stable");
        assert_eq!(program.evaluate("$(STAGED)", &paths).unwrap(), "");
        assert_eq!(program.evaluate("$(.SHELLSTATUS)", &paths).unwrap(), "1");
    }
}

#[cfg(unix)]
#[test]
fn captured_shell_refuses_symlink_file_tests_atomically() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("data")).unwrap();
    std::fs::write(root.join("data/target.txt"), "synthetic input").unwrap();
    symlink("target.txt", root.join("data/link.txt")).unwrap();
    std::fs::write(
        root.join("good.mk"),
        "VALUE := stable\nINITIAL := $(shell false)\n",
    )
    .unwrap();
    std::fs::write(
        root.join("symlink.mk"),
        "VALUE := changed\nSTAGED := $(shell true)\nBAD := $(shell cd data; if [ -f link.txt ]; then printf unsafe; fi)\n",
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    program
        .read_captured_file("good.mk", &paths, &captured)
        .unwrap();
    let prior_events = program.events().to_vec();

    assert!(program
        .read_captured_file("symlink.mk", &paths, &captured)
        .is_err());
    assert_eq!(program.events(), prior_events);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "stable");
    assert_eq!(program.evaluate("$(STAGED)", &paths).unwrap(), "");
    assert_eq!(program.evaluate("$(.SHELLSTATUS)", &paths).unwrap(), "1");
}
