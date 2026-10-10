//! Process-boundary preflight counterprobes, not compiler qualification.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        Self {
            _temporary: temporary,
            root,
        }
    }

    fn run(&self, evidence: Option<&Path>, package_format: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aros"));
        command.current_dir(&self.root).args([
            "toolchain",
            "producer",
            "compatibility",
            "--format",
            "json",
        ]);
        for name in [
            "recipe",
            "source-lock",
            "profiles",
            "build-environment",
            "package-dir",
            "first-root",
            "second-root",
            "source-dir",
            "engine-work-dir",
            "helpers-dir",
            "cmake-program",
            "ninja-program",
            "upstream-source-dir",
            "upstream-build-dir",
            "python-cache-dir",
            "ports-lock",
            "ports-cache-dir",
            "ports-sources-dir",
            "python-environment-dir",
            "host-tools-dir",
            "cmake-build-dir",
            "c-fixture",
            "cxx-fixture",
            "standalone-output-dir",
            "reports-dir",
        ] {
            command.arg(format!("--{name}")).arg(self.root.join(name));
        }
        command.args([
            "--preset",
            "pc-x86_64",
            "--release-id",
            "local-export-test",
            "--host",
            "linux-x86_64",
            "--jobs",
            "1",
            "--timeout-seconds",
            "30",
            "--package-format",
            package_format,
        ]);
        command
            .arg("--host-tool")
            .arg(format!("make={}", self.root.join("make").display()));
        if let Some(evidence) = evidence {
            command.arg("--evidence-dir").arg(evidence);
        }
        command.output().unwrap()
    }

    fn assert_unprepared(&self) {
        for leaf in [
            "first-root",
            "second-root",
            "cmake-build-dir",
            "reports-dir",
            "standalone-output-dir",
            "host-tools-dir",
            "python-environment-dir",
            "ports-sources-dir",
        ] {
            assert!(
                !self.root.join(leaf).exists(),
                "early rejection created {leaf}"
            );
        }
    }
}

fn diagnostic(output: &Output) -> String {
    assert!(!output.status.success());
    let mut text = String::from_utf8(output.stdout.clone()).unwrap();
    text.push_str(&String::from_utf8(output.stderr.clone()).unwrap());
    text
}

#[test]
fn explicit_legacy_export_fails_before_reading_inputs_or_preparing_roots() {
    let fixture = Fixture::new();
    let export = fixture.root.join("export");
    let text = diagnostic(&fixture.run(Some(&export), "legacy-v1"));
    assert!(
        text.contains("requires --package-format family-v2"),
        "{text}"
    );
    assert!(!text.contains("recipe is unavailable"), "{text}");
    assert!(!export.exists());
    fixture.assert_unprepared();
}

#[test]
fn occupied_export_is_preserved_before_any_source_work() {
    let fixture = Fixture::new();
    let export = fixture.root.join("export");
    fs::write(&export, b"keep my existing evidence").unwrap();
    let text = diagnostic(&fixture.run(Some(&export), "family-v2"));
    assert!(text.contains("evidence output already exists"), "{text}");
    assert_eq!(fs::read(&export).unwrap(), b"keep my existing evidence");
    fixture.assert_unprepared();
}

#[test]
fn nested_export_cannot_enter_the_closed_reports_root() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.root.join("reports-dir")).unwrap();
    let export = fixture.root.join("reports-dir/export");
    let text = diagnostic(&fixture.run(Some(&export), "family-v2"));
    assert!(
        text.contains("separate from every input and execution root"),
        "{text}"
    );
    assert!(!export.exists());
    assert_eq!(
        fs::read_dir(fixture.root.join("reports-dir"))
            .unwrap()
            .count(),
        0
    );
    assert!(!fixture.root.join("first-root").exists());
}

#[test]
fn ordinary_legacy_path_and_valid_export_selection_reach_recipe_validation() {
    let fixture = Fixture::new();
    let text = diagnostic(&fixture.run(None, "legacy-v1"));
    assert!(text.contains("recipe is unavailable"), "{text}");
    assert!(!text.contains("requires --package-format"), "{text}");
    let export = fixture.root.join("export");
    let text = diagnostic(&fixture.run(Some(&export), "family-v2"));
    assert!(text.contains("recipe is unavailable"), "{text}");
    assert!(!export.exists());
    fixture.assert_unprepared();
}
