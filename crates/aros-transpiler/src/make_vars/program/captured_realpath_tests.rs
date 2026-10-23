//! Captured path functions and the selected Source's actual root-tools guard.

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

fn native(cwd: &Path, file: &str, protected: &[String]) -> String {
    let output = Command::new(if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    })
    .current_dir(cwd)
    .env_clear()
    .env("PATH", std::env::var("PATH").unwrap())
    .env("LC_ALL", "C")
    .args(["-r", "-n", "-s", "--no-print-directory", "-I-", "-f", file])
    .args(protected)
    .arg("probe")
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "native captured realpath fixture failed"
    );
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    let observed = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OBSERVED="))
        .collect::<Vec<_>>();
    assert_eq!(observed.len(), 1);
    observed[0].to_owned()
}

#[test]
fn captured_realpath_matches_native_order_absence_dot_and_directory_suffixes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let cwd = root.join("cwd");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::create_dir(cwd.join("dir")).unwrap();
    std::fs::write(cwd.join("file.mk"), "data\n").unwrap();
    std::fs::write(cwd.join("comma,name.mk"), "comma\n").unwrap();
    std::fs::write(cwd.join("dollar$name.mk"), "dollar\n").unwrap();
    std::fs::write(cwd.join("unicode\u{a0}name.mk"), "nbsp\n").unwrap();
    std::fs::write(cwd.join("unicode\u{2003}name.mk"), "em-space\n").unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &cwd,
    };
    let ns = namespace();
    let mut values = Vec::new();
    for expression in [
        "$(realpath file.mk)",
        "$(realpath ./dir/../file.mk)",
        "$(realpath dir/)",
        "$(realpath dir/.)",
        "$(realpath file.mk/)",
        "$(realpath file.mk/.)",
        "$(realpath file.mk//././)",
        "$(realpath comma,name.mk)",
        "$(realpath dollar$$name.mk)",
        "$(realpath $(realpath dollar$$name.mk))",
        "$(realpath unicode\u{a0}name.mk)",
        "$(realpath unicode\u{2003}name.mk)",
        "$(realpath )",
        "$(realpath missing.mk)",
        "$(realpath file.mk missing.mk dir file.mk)",
        "$(realpath . ..)",
        "$(realpath $(wildcard *.mk))",
    ] {
        let source = format!("CAPTURE := {expression}\nprobe:\n\t$(info OBSERVED=$(CAPTURE))\n");
        std::fs::write(cwd.join("root.mk"), source).unwrap();
        let captured = capture(&root, &cwd);
        let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
        let (_, reads) = program
            .read_captured_file("root.mk", &paths, &captured)
            .unwrap();
        let actual = program.evaluate("$(CAPTURE)", &paths).unwrap();
        assert_eq!(
            actual,
            native(&cwd, "root.mk", &reads.old_file_arguments()),
            "expression {expression}"
        );
        values.push(actual);
    }
    assert_ne!(values[0], values[2]);
    let captured = capture(&root, &cwd);
    let mut memory_only = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
    assert!(memory_only.evaluate("$(realpath file.mk)", &paths).is_err());
    assert_eq!(
        memory_only
            .evaluate_captured("$(realpath file.mk)", &paths, &captured)
            .unwrap()
            .0,
        cwd.join("file.mk").to_str().unwrap()
    );
}

#[test]
fn captured_realpath_preserves_assignment_timing_and_refuses_stale_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(
        root.join("root.mk"),
        "FROZEN := $(realpath later.mk)\nLATER = $(realpath later.mk)\n",
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let ns = namespace();
    let original = capture(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &original)
        .unwrap();
    assert_eq!(program.evaluate("$(FROZEN)", &paths).unwrap(), "");
    assert_eq!(
        program
            .evaluate_captured("$(LATER)", &paths, &original)
            .unwrap()
            .0,
        ""
    );
    std::fs::write(root.join("later.mk"), "new selected file\n").unwrap();
    assert!(program
        .evaluate_captured("$(LATER)", &paths, &original)
        .is_err());
    let next = capture(&root, &root);
    assert_eq!(
        program
            .evaluate_captured("$(LATER)", &paths, &next)
            .unwrap()
            .0,
        root.join("later.mk").to_str().unwrap()
    );
    assert_eq!(program.evaluate("$(FROZEN)", &paths).unwrap(), "");
    assert!(program.evaluate("$(LATER)", &paths).is_err());
}

#[test]
fn actual_source_root_tools_guard_distinguishes_root_and_leaf_without_a_path_table() {
    let source_root = crate::testing::mesa26_root();
    let configuration =
        aros_common::text::read_source(&source_root.join("config/make.cfg.in")).unwrap();
    let lines = configuration.lines().collect::<Vec<_>>();
    let starts = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("ifeq ($(realpath"))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(
        starts.len(),
        1,
        "select the unique actual source root guard"
    );
    let start = starts[0];
    assert_eq!(lines[start + 2], "endif");
    let guard = format!("{}\n", lines[start..start + 3].join("\n"));
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/make-root-tools.mk"),
        "ROOT_BOUND := source-selected\n",
    )
    .unwrap();
    std::fs::write(root.join("configuration.mk"), guard).unwrap();
    let body = format!(
        "TOP := {}\nSRCDIR := {}\ninclude configuration.mk\nCAPTURE := $(ROOT_BOUND)\nprobe:\n\t$(info OBSERVED=$(CAPTURE))\n",
        root.display(),
        root.display()
    );
    std::fs::write(root.join("Makefile"), &body).unwrap();
    std::fs::write(root.join("leaf.mk"), &body).unwrap();
    let captured = capture(&root, &root);
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let ns = namespace();
    for (file, expected) in [("Makefile", "source-selected"), ("leaf.mk", "")] {
        let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
        let (_, reads) = program.read_captured_file(file, &paths, &captured).unwrap();
        let actual = program.evaluate("$(CAPTURE)", &paths).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(actual, native(&root, file, &reads.old_file_arguments()));
        assert_eq!(reads.reads().len(), if file == "Makefile" { 3 } else { 2 });
    }
}
