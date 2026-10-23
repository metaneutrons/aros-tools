//! Captured source export projection over generic synthetic inputs.

use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin},
    measure_tree_content_cas,
};
use std::{
    collections::BTreeMap,
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

fn paths<'a>(dirs: &'a DirVars, root: &'a Path, cwd: &'a Path) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: cwd,
    }
}

fn capture_snapshot(root: &Path, cwd: &Path) -> MakeInputSnapshot {
    let content_sha256 = measure_tree_content_cas(root)
        .unwrap()
        .payload_digest_excluding(None);
    MakeInputSnapshot::capture(
        cwd,
        &[MakeInputRootSelection {
            root: root.into(),
            content_sha256,
        }],
    )
    .unwrap()
    .with_include_directories(&[])
    .unwrap()
}

fn native_export(cwd: &Path, source: &str, name: &str) -> String {
    assert!(name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    // Execute only this synthetic printf recipe, never a selected Source rule.
    // The shell reads the actual exported child value, not a Make expansion.
    let input = format!("{source}\nprobe:\n\t@printf 'OBSERVED=%s\\n' \"$${name}\"\n");
    let mut child = Command::new(make)
        .current_dir(cwd)
        .args(["-rR", "-s", "--no-print-directory", "-f", "-", "probe"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LC_ALL", "C")
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
    assert!(
        output.status.success(),
        "synthetic GNU Make export query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let values = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OBSERVED="))
        .collect::<Vec<_>>();
    assert_eq!(
        values.len(),
        1,
        "unexpected synthetic Make output: {stdout}"
    );
    values[0].to_owned()
}

#[test]
fn recursive_exports_preserve_wildcard_sort_and_realpath_argument_order() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let cwd = root.join("work");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::create_dir(cwd.join("generated")).unwrap();
    for name in ["a.c", "z.c"] {
        std::fs::write(cwd.join("generated").join(name), "source\n").unwrap();
    }
    let source = "export COMBINED = $(wildcard generated/*.c)|$(realpath generated/z.c generated/a.c absent.c)\n";
    std::fs::write(cwd.join("root.mk"), source).unwrap();

    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &cwd);
    let snapshot = capture_snapshot(&root, &cwd);
    let mut program =
        MakeVariableProgram::new_source_program(&paths, &namespace(&[]), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();

    let (values, _reads) = program
        .source_export_values_captured(&paths, &snapshot)
        .unwrap();
    let expected = format!(
        "generated/a.c generated/z.c|{} {}",
        cwd.join("generated/z.c").display(),
        cwd.join("generated/a.c").display()
    );
    assert_eq!(
        values,
        BTreeMap::from([("COMBINED".into(), expected.clone())])
    );
    assert_eq!(native_export(&cwd, source, "COMBINED"), expected);
    program.execute("unexport COMBINED", 1, &paths).unwrap();
    assert!(program
        .source_export_values_captured(&paths, &snapshot)
        .unwrap()
        .0
        .is_empty());
    assert_eq!(
        native_export(&cwd, &format!("{source}unexport COMBINED\n"), "COMBINED"),
        ""
    );
}

#[test]
fn frozen_and_recursive_exports_use_their_respective_captured_inventories() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("a.src"), "source\n").unwrap();
    std::fs::write(
        root.join("root.mk"),
        "export FROZEN := $(wildcard *.src)\nexport LATER = $(wildcard *.src)\n",
    )
    .unwrap();

    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let original = capture_snapshot(&root, &root);
    let mut program =
        MakeVariableProgram::new_source_program(&paths, &namespace(&[]), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &original)
        .unwrap();
    let (initial, _) = program
        .source_export_values_captured(&paths, &original)
        .unwrap();
    assert_eq!(initial["FROZEN"], "a.src");
    assert_eq!(initial["LATER"], "a.src");

    let events_before_stale_read = program.events().to_vec();
    std::fs::write(root.join("b.src"), "later source\n").unwrap();
    assert!(program
        .source_export_values_captured(&paths, &original)
        .is_err());
    assert_eq!(program.events(), events_before_stale_read);

    let fresh = capture_snapshot(&root, &root);
    let (updated, _) = program
        .source_export_values_captured(&paths, &fresh)
        .unwrap();
    assert_eq!(updated["FROZEN"], "a.src");
    assert_eq!(updated["LATER"], "a.src b.src");
}

#[test]
fn inherited_environment_stays_literal_while_a_file_override_expands() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("a.c"), "source\n").unwrap();
    std::fs::write(
        root.join("root.mk"),
        "OVERRIDE = $(wildcard *.c)|$(realpath a.c)\n",
    )
    .unwrap();

    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let ns = namespace(&[
        ("RAW_ENV", MakeVariableOrigin::Environment),
        ("OVERRIDE", MakeVariableOrigin::Environment),
    ]);
    let raw = "$(wildcard *.c)|$(realpath a.c)";
    let initial = [
        MakeInitialValue {
            name: "RAW_ENV",
            value: raw,
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
        MakeInitialValue {
            name: "OVERRIDE",
            value: "from-environment",
            flavor: MakeInitialFlavor::Recursive,
            origin: MakeVariableOrigin::Environment,
        },
    ];
    let snapshot = capture_snapshot(&root, &root);
    let mut program = MakeVariableProgram::new_source_program(&paths, &ns, &initial).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();

    let (values, _) = program
        .source_export_values_captured(&paths, &snapshot)
        .unwrap();
    assert_eq!(values["RAW_ENV"], raw);
    assert_eq!(
        values["OVERRIDE"],
        format!("a.c|{}", root.join("a.c").display())
    );
}

#[test]
fn source_exports_require_captured_filesystem_and_matching_cwd() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let other_cwd = root.join("other");
    std::fs::create_dir(&other_cwd).unwrap();
    std::fs::write(root.join("visible.txt"), "captured source\n").unwrap();
    std::fs::write(
        root.join("root.mk"),
        "export FILES = $(wildcard *.txt)|$(realpath visible.txt)\n",
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = capture_snapshot(&root, &root);
    let mut program =
        MakeVariableProgram::new_source_program(&paths, &namespace(&[]), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();

    let events_before = program.events().to_vec();
    assert!(program.source_export_values(&paths).is_err());
    let wrong_cwd_snapshot = capture_snapshot(&root, &other_cwd);
    assert!(program
        .source_export_values_captured(&paths, &wrong_cwd_snapshot)
        .is_err());
    assert_eq!(program.events(), events_before);
}

#[test]
fn failed_export_expansion_rolls_back_intermediate_eval_effects() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("visible.txt"), "captured source\n").unwrap();
    std::fs::write(
        root.join("root.mk"),
        "export A_SAFE = $(wildcard *.txt)\nexport Z_FAIL = $(eval LEAK := staged)after\n",
    )
    .unwrap();
    let dirs = DirVars::default();
    let paths = paths(&dirs, &root, &root);
    let snapshot = capture_snapshot(&root, &root);
    let mut program =
        MakeVariableProgram::new_source_program(&paths, &namespace(&[]), &[]).unwrap();
    program
        .read_captured_file("root.mk", &paths, &snapshot)
        .unwrap();

    let events_before = program.events().to_vec();
    assert!(program
        .source_export_values_captured(&paths, &snapshot)
        .is_err());
    assert_eq!(program.events(), events_before);
    assert_eq!(program.evaluate("$(LEAK)", &paths).unwrap(), "");
}
