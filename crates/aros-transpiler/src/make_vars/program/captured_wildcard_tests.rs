//! Native GNU comparisons for data-only captured filesystem expressions.

use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation},
    measure_tree_content_cas,
};
use std::process::Command;

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .to_vec(),
    })
    .unwrap()
}

fn capture(root: &Path, cwd: &Path) -> MakeInputSnapshot {
    MakeInputSnapshot::capture(
        cwd,
        &[MakeInputRootSelection {
            root: root.into(),
            content_sha256: measure_tree_content_cas(root)
                .unwrap()
                .payload_digest_excluding(None),
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap()
}

#[test]
fn captured_wildcards_match_native_patterns_order_and_recursive_timing() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let cwd = root.join("working");
    std::fs::create_dir(&cwd).unwrap();
    for directory in ["sub", "other"] {
        std::fs::create_dir(cwd.join(directory)).unwrap();
    }
    for file in [
        "a.c",
        "b.c",
        "é.c",
        "*.c",
        "-.c",
        "].c",
        ".hidden.c",
        "two words.c",
        "sub/a.c",
        "sub/b.h",
        "other/x.c",
    ] {
        std::fs::write(cwd.join(file), "source\n").unwrap();
    }
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &cwd,
    };
    let ns = namespace();
    let mut differences = Vec::new();
    let patterns = [
        "*.c",
        "./*.c",
        "sub/*.c",
        "sub/*.[ch]",
        "*/*.c",
        "*/*.?",
        "**/*.c",
        "a**.c",
        "***/*.c",
        "*",
        "missing*",
        "missing.c",
        "missing/sub/leaf",
        "missing/sub/*.c",
        "sub/missing/deeper/leaf/",
        "a.c/",
        "a.c//",
        "a.c/.",
        "sub/.",
        "*.c/",
        "sub*/",
        "sub/",
        "./sub/",
        "sub/../*.c",
        "*.c *.c",
        "[ab].c",
        "[!a].c",
        "[^a].c",
        "[a-z].c",
        "[z-a].c",
        "?.c",
        "??.c",
        "[.]hidden.c",
        "[**].c",
        "[z-**].c",
        ".*.c",
        ".*",
        "sub//*.c",
    ]
    .map(str::to_owned)
    .into_iter()
    .chain([
        format!("{}/*.c", cwd.display()),
        format!("{}/missing/deeper/leaf", cwd.display()),
        format!("{}/missing/deeper/*.c", cwd.display()),
        format!("{}/sub/*.[ch]", cwd.display()),
    ]);
    for pattern in patterns {
        let source = format!(
            "FROZEN := $(wildcard {pattern})\nLATER = $(wildcard {pattern})\nprobe:\n\t$(info OBSERVED=$(FROZEN)|$(LATER))\n"
        );
        std::fs::write(cwd.join("root.mk"), source).unwrap();
        let snapshot = capture(&root, &cwd);
        let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
        program
            .read_captured_file("root.mk", &paths, &snapshot)
            .unwrap();
        let actual = program
            .evaluate_captured("$(FROZEN)|$(LATER)", &paths, &snapshot)
            .unwrap()
            .0;
        let output = Command::new(if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        })
        .current_dir(&cwd)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("LC_ALL", "C")
        .args([
            "-r",
            "-n",
            "-s",
            "--no-print-directory",
            "-I-",
            "-f",
            "root.mk",
            "--old-file=root.mk",
            "probe",
        ])
        .output()
        .unwrap();
        assert!(
            output.status.success(),
            "native wildcard query failed for {pattern}"
        );
        let stdout = std::str::from_utf8(&output.stdout).unwrap();
        let expected = stdout
            .lines()
            .find_map(|line| line.strip_prefix("OBSERVED="))
            .unwrap();
        if actual != expected {
            differences.push(format!("{pattern}: actual={actual:?}, native={expected:?}"));
        }
    }
    assert!(
        differences.is_empty(),
        "native wildcard mismatches: {}",
        differences.join("; ")
    );
}

#[test]
fn captured_wildcard_changes_require_fresh_selection_and_never_use_ambient_defaults() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("root.mk"), "ifneq (\"$(wildcard Build.extra)\",\"\")\ninclude Build.extra\nendif\nFROZEN := $(wildcard *.c)\nLATER = $(wildcard *.c)\n").unwrap();
    std::fs::write(root.join("a.c"), "source\n").unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let ns = namespace();
    let snapshot = capture(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();
    assert_eq!(
        program
            .evaluate_captured("$(FROZEN)|$(LATER)", &paths, &snapshot)
            .unwrap()
            .0,
        "a.c|a.c"
    );
    assert!(program.evaluate("$(LATER)", &paths).is_err());
    let before = program.events().to_vec();
    std::fs::write(root.join("b.c"), "source\n").unwrap();
    assert!(program
        .evaluate_captured("$(LATER)", &paths, &snapshot)
        .is_err());
    assert_eq!(program.events(), before);
    let fresh = capture(&root, &root);
    assert_eq!(
        program
            .evaluate_captured("$(FROZEN)|$(LATER)", &paths, &fresh)
            .unwrap()
            .0,
        "a.c|a.c b.c"
    );
    std::fs::write(root.join("Build.extra"), "LOCAL := selected\n").unwrap();
    let fresh = capture(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
    let (_, reads) = program
        .read_captured_file("root.mk", &paths, &fresh)
        .unwrap();
    assert_eq!(program.evaluate("$(LOCAL)", &paths).unwrap(), "selected");
    assert_eq!(reads.reads()[1].identity, "Build.extra");
}
