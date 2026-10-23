//! Explicit, consistency-only shell effects for staged Source reads.

use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeVariableOrigin, ValidatedMakeNamespace},
    measure_tree_content_cas,
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

fn namespace() -> ValidatedMakeNamespace {
    ValidatedMakeNamespace::from_measurement(
        "v1:4;.VARIABLES|default;MAKE_VERSION|default;.SHELLFLAGS|default;SHELL|default;end",
    )
    .unwrap()
}

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

fn paths<'a>(root: &'a Path, dirs: &'a DirVars) -> MakeProgramPaths<'a> {
    MakeProgramPaths {
        dirs,
        source_root: root,
        relative_dir: Path::new(""),
        top_directory: root,
        working_directory: root,
    }
}

fn source_program(root: &Path, dirs: &DirVars) -> MakeVariableProgram {
    let paths = paths(root, dirs);
    let mut program = MakeVariableProgram::new_source_program(
        &paths,
        &namespace(),
        &[
            MakeInitialValue {
                name: ".SHELLFLAGS",
                value: "-c",
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Default,
            },
            MakeInitialValue {
                name: "SHELL",
                value: "/bin/sh",
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Default,
            },
        ],
    )
    .unwrap();
    program
        .require_captured_shell_policy("/bin/sh", &["-c"], &BTreeMap::new())
        .unwrap();
    program
}

// Each command is deliberately generated per temporary root. The provider
// supplies fixture data; these names establish neither executable identity nor
// permission to execute anything.
fn opaque_command(root: &Path, suffix: &str) -> String {
    let nonce = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("temporary")
        .replace(['.', '-'], "_");
    format!("external_{nonce}_{suffix} --selected-by-provider")
}

struct ExpectedEffect {
    command: String,
    cwd: PathBuf,
    shell: String,
    shell_flags: String,
    source_exports: BTreeMap<String, String>,
    source_export_controls: BTreeMap<String, bool>,
    stdout: String,
    status: u8,
    query_error: Option<String>,
    gnu_folded: bool,
}

impl ExpectedEffect {
    fn new(
        cwd: &Path,
        command: String,
        source_exports: BTreeMap<String, String>,
        stdout: &str,
        status: u8,
    ) -> Self {
        let source_export_controls = source_exports
            .keys()
            .map(|name| (name.clone(), true))
            .collect();
        Self {
            command,
            cwd: cwd.to_owned(),
            shell: "/bin/sh".into(),
            shell_flags: "-c".into(),
            source_exports,
            source_export_controls,
            stdout: stdout.into(),
            status,
            query_error: None,
            gnu_folded: false,
        }
    }
}

struct TestEffects {
    expected: Vec<ExpectedEffect>,
    cursor: Cell<usize>,
    queries: Cell<usize>,
    revalidations: Cell<usize>,
    finishes: Cell<usize>,
    fail_revalidation: Option<usize>,
    mutate_after_query: Option<(PathBuf, Vec<u8>)>,
    mutated: Cell<bool>,
}

impl TestEffects {
    fn new(expected: Vec<ExpectedEffect>) -> Self {
        Self {
            expected,
            cursor: Cell::new(0),
            queries: Cell::new(0),
            revalidations: Cell::new(0),
            finishes: Cell::new(0),
            fail_revalidation: None,
            mutate_after_query: None,
            mutated: Cell::new(false),
        }
    }
}

impl crate::make_shell_effects::CapturedShellEffects for TestEffects {
    fn revalidate(&self) -> Result<(), String> {
        let call = self.revalidations.get() + 1;
        self.revalidations.set(call);
        if self.fail_revalidation == Some(call) {
            Err("private provider drift details".into())
        } else {
            Ok(())
        }
    }

    fn query(
        &self,
        request: &crate::make_shell_effects::ShellEffectRequest<'_>,
    ) -> Result<crate::make_shell_effects::ShellEffectOutput, String> {
        self.queries.set(self.queries.get() + 1);
        let index = self.cursor.get();
        let Some(expected) = self.expected.get(index) else {
            return Err("private provider selection mismatch".into());
        };
        if request.native_context.is_some() {
            return Err("standalone provider received a native context".into());
        }
        if request.ordinal != index
            || request.command != expected.command
            || request.working_directory != expected.cwd
            || request.shell != expected.shell
            || request.shell_flags != expected.shell_flags
            || request.source_exports != &expected.source_exports
            || request.source_export_controls != &expected.source_export_controls
        {
            return Err("private provider selection mismatch".into());
        }

        // A matching request consumes this independently selected record,
        // including when its selected execution reports an error.
        self.cursor.set(index + 1);
        if let Some((path, bytes)) = &self.mutate_after_query {
            if !self.mutated.replace(true) {
                std::fs::write(path, bytes).map_err(|_| "private mutation failed".to_owned())?;
            }
        }
        if let Some(error) = &expected.query_error {
            return Err(error.clone());
        }
        if expected.gnu_folded {
            return Ok(crate::make_shell_effects::ShellEffectOutput::GnuValue(
                crate::make_shell_effects::gnu_query::GnuShellQueryResult {
                    value: expected.stdout.clone(),
                    status: expected.status,
                },
            ));
        }
        Ok(crate::make_shell_data::ShellDataOutput {
            stdout: expected.stdout.clone(),
            status: expected.status,
        }
        .into())
    }

    fn finish(&self, effects: usize) -> Result<(), String> {
        self.finishes.set(self.finishes.get() + 1);
        if effects == self.expected.len() && self.cursor.get() == self.expected.len() {
            Ok(())
        } else {
            Err("private provider coverage details".into())
        }
    }
}

fn exports(name: &str, value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(name.into(), value.into())])
}

#[test]
fn provider_supplies_opaque_effects_in_source_include_and_input_order() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let first = opaque_command(&root, "first");
    let included = opaque_command(&root, "included");
    let changed = opaque_command(&root, "changed");
    let final_command = opaque_command(&root, "final");
    std::fs::write(
        root.join("Root.mk"),
        format!(
            "export TOKEN := before\nA := $(shell {first})\nA_STATUS := $(.SHELLSTATUS)\ninclude Nested.mk\nexport TOKEN := after\nC := $(shell {changed})\nC_STATUS := $(.SHELLSTATUS)\n"
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("Nested.mk"),
        format!("B := $(shell {included})\nB_STATUS := $(.SHELLSTATUS)\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("Tail.mk"),
        format!(
            "export TOKEN := final\nD := $(shell {final_command})\nD_STATUS := $(.SHELLSTATUS)\n"
        ),
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let provider = TestEffects::new(vec![
        ExpectedEffect::new(
            &root,
            first,
            exports("TOKEN", "before"),
            "first\n\nline\n",
            17,
        ),
        ExpectedEffect::new(
            &root,
            included,
            exports("TOKEN", "before"),
            "included\n",
            u8::MAX,
        ),
        ExpectedEffect::new(&root, changed, exports("TOKEN", "after"), "changed\n", 4),
        ExpectedEffect::new(
            &root,
            final_command,
            exports("TOKEN", "final"),
            "tail\nvalue\n",
            0,
        ),
    ]);
    let mut program = source_program(&root, &dirs);

    program
        .read_captured_files_with_shell_effects(
            &["Root.mk", "Tail.mk"],
            &paths,
            &captured,
            &provider,
        )
        .unwrap();

    assert_eq!(program.evaluate("$(A)", &paths).unwrap(), "first  line");
    assert_eq!(program.evaluate("$(B)", &paths).unwrap(), "included");
    assert_eq!(program.evaluate("$(C)", &paths).unwrap(), "changed");
    assert_eq!(program.evaluate("$(D)", &paths).unwrap(), "tail value");
    assert_eq!(program.evaluate("$(A_STATUS)", &paths).unwrap(), "17");
    assert_eq!(program.evaluate("$(B_STATUS)", &paths).unwrap(), "255");
    assert_eq!(program.evaluate("$(C_STATUS)", &paths).unwrap(), "4");
    assert_eq!(program.evaluate("$(D_STATUS)", &paths).unwrap(), "0");
    assert_eq!(provider.cursor.get(), 4);
    assert_eq!(provider.finishes.get(), 1);
    assert_eq!(provider.revalidations.get(), 2);

    // The exact same opaque command is refused by the ordinary data-only path.
    let mut default_reader = source_program(&root, &dirs);
    assert!(default_reader
        .read_captured_files(&["Root.mk", "Tail.mk"], &paths, &captured)
        .is_err());
    assert!(default_reader.events().is_empty());
}

#[test]
fn provider_mismatch_and_query_error_are_sanitized_and_postchecked() {
    for return_query_error in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let command = opaque_command(&root, "selected");
        std::fs::write(
            root.join("Root.mk"),
            format!("VALUE := $(shell {command})\n"),
        )
        .unwrap();
        let captured = snapshot(&root);
        let dirs = DirVars::default();
        let paths = paths(&root, &dirs);
        let mut expected =
            ExpectedEffect::new(&root, command.clone(), BTreeMap::new(), "private output", 0);
        if return_query_error {
            expected.query_error = Some("RAW-PRIVATE-QUERY-FAILURE".into());
        } else {
            expected.shell_flags = "-ec".into();
        }
        let provider = TestEffects::new(vec![expected]);
        let mut program = source_program(&root, &dirs);
        let before = program.events().to_vec();
        let error = program
            .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
            .unwrap_err();

        assert!(!error.contains("RAW-PRIVATE-QUERY-FAILURE"));
        assert!(!error.contains("private output"));
        assert!(error.contains("bound shell effect provider refused"));
        assert_eq!(program.events(), before);
        assert_eq!(provider.revalidations.get(), 2);
        assert_eq!(provider.finishes.get(), 0);
        assert_eq!(provider.cursor.get(), usize::from(return_query_error));
    }
}

#[test]
fn shell_controls_and_nested_control_effects_refuse_before_provider_query() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let nested = opaque_command(&root, "nested-control");
    let sources = [
        "SHELL := /unselected/shell\nVALUE := $(shell opaque-effect)\n".to_owned(),
        "SHELL := /bin/sh\n.SHELLFLAGS := -ec\nVALUE := $(shell opaque-effect)\n".to_owned(),
        format!("SHELL = $(shell {nested})\nVALUE := $(shell opaque-effect)\n"),
        "SHELL := /bin/sh\nexport BASH_ENV := /private/hook\nVALUE := $(shell opaque-effect)\n"
            .to_owned(),
    ];
    for (index, source) in sources.iter().enumerate() {
        let input = format!("Controls-{index}.mk");
        std::fs::write(root.join(&input), source).unwrap();
    }
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);

    for index in 0..sources.len() {
        let provider = TestEffects::new(Vec::new());
        let mut program = source_program(&root, &dirs);
        assert!(program
            .read_captured_files_with_shell_effects(
                &[&format!("Controls-{index}.mk")],
                &paths,
                &captured,
                &provider,
            )
            .is_err());
        assert!(program.events().is_empty());
        assert_eq!(provider.queries.get(), 0);
        assert_eq!(provider.finishes.get(), 0);
        assert_eq!(provider.revalidations.get(), 2);
    }
}

#[test]
fn later_invalid_source_rolls_back_earlier_effect_and_runs_postcheck() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let command = opaque_command(&root, "before-invalid-source");
    std::fs::write(
        root.join("First.mk"),
        format!("VALUE := $(shell {command})\n"),
    )
    .unwrap();
    std::fs::write(root.join("Invalid.mk"), "not a supported Make statement\n").unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let provider = TestEffects::new(vec![ExpectedEffect::new(
        &root,
        command,
        BTreeMap::new(),
        "captured",
        9,
    )]);
    let mut program = source_program(&root, &dirs);
    let before = program.events().to_vec();

    assert!(program
        .read_captured_files_with_shell_effects(
            &["First.mk", "Invalid.mk"],
            &paths,
            &captured,
            &provider,
        )
        .is_err());
    assert_eq!(program.events(), before);
    assert_eq!(provider.cursor.get(), 1);
    assert_eq!(provider.finishes.get(), 0);
    assert_eq!(provider.revalidations.get(), 2);
}

#[test]
fn unused_effect_refuses_finish_without_publishing_staged_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let selected = opaque_command(&root, "selected");
    let unused = opaque_command(&root, "unused");
    std::fs::write(
        root.join("Root.mk"),
        format!("VALUE := $(shell {selected})\n"),
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let provider = TestEffects::new(vec![
        ExpectedEffect::new(&root, selected, BTreeMap::new(), "selected", 1),
        ExpectedEffect::new(&root, unused, BTreeMap::new(), "unused", 2),
    ]);
    let mut program = source_program(&root, &dirs);
    let before = program.events().to_vec();
    let error = program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .unwrap_err();

    assert!(error.contains("shell effect provider coverage refused"));
    assert!(!error.contains("unused"));
    assert_eq!(program.events(), before);
    assert_eq!(provider.cursor.get(), 1);
    assert_eq!(provider.finishes.get(), 1);
    assert_eq!(provider.revalidations.get(), 2);
}

#[test]
fn provider_drift_after_interpretation_prevents_state_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let command = opaque_command(&root, "drift");
    std::fs::write(
        root.join("Root.mk"),
        format!("VALUE := $(shell {command})\n"),
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let mut provider = TestEffects::new(vec![ExpectedEffect::new(
        &root,
        command,
        BTreeMap::new(),
        "stable result",
        0,
    )]);
    provider.fail_revalidation = Some(2);
    let mut program = source_program(&root, &dirs);
    let before = program.events().to_vec();
    let error = program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .unwrap_err();

    assert!(error.contains("shell effect provider postcheck refused"));
    assert!(!error.contains("private provider drift details"));
    assert_eq!(program.events(), before);
    assert_eq!(provider.finishes.get(), 1);
    assert_eq!(provider.revalidations.get(), 2);
}

#[test]
fn source_root_mutation_during_effect_refuses_transaction_and_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let command = opaque_command(&root, "mutating-provider");
    let source_path = root.join("Root.mk");
    std::fs::write(&source_path, format!("VALUE := $(shell {command})\n")).unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let mut provider = TestEffects::new(vec![ExpectedEffect::new(
        &root,
        command,
        BTreeMap::new(),
        "result",
        0,
    )]);
    provider.mutate_after_query = Some((source_path, b"changed while interpreted\n".to_vec()));
    let mut program = source_program(&root, &dirs);
    let before = program.events().to_vec();

    assert!(program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .is_err());
    assert_eq!(program.events(), before);
    assert_eq!(provider.cursor.get(), 1);
    assert_eq!(provider.finishes.get(), 1);
    assert_eq!(provider.revalidations.get(), 2);
}

#[test]
fn stdout_outside_admitted_text_domain_is_rejected_atomically() {
    for stdout in ["x".repeat(1024 * 1024 + 1), "contains\0nul".into()] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let command = opaque_command(&root, "invalid-output");
        std::fs::write(
            root.join("Root.mk"),
            format!("VALUE := $(shell {command})\n"),
        )
        .unwrap();
        let captured = snapshot(&root);
        let dirs = DirVars::default();
        let paths = paths(&root, &dirs);
        let provider = TestEffects::new(vec![ExpectedEffect::new(
            &root,
            command,
            BTreeMap::new(),
            &stdout,
            0,
        )]);
        let mut program = source_program(&root, &dirs);
        let before = program.events().to_vec();
        let error = program
            .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
            .unwrap_err();

        assert!(error.contains("output exceeds its admitted text domain"));
        assert_eq!(program.events(), before);
        assert_eq!(provider.revalidations.get(), 2);
        assert_eq!(provider.finishes.get(), 0);
    }
}

#[test]
fn gnu_folded_values_preserve_status_and_refuse_unfolded_data_atomically() {
    for value in [
        "already  folded $literal ",
        "$(error NOT_A_SECOND_EVAL)",
        "not\nfolded",
        "reserved\u{e000}",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let command = opaque_command(&root, "gnu-value");
        std::fs::write(
            root.join("Root.mk"),
            format!("VALUE := $(shell {command})\nSTATUS := $(.SHELLSTATUS)\n"),
        )
        .unwrap();
        let captured = snapshot(&root);
        let dirs = DirVars::default();
        let paths = paths(&root, &dirs);
        let mut expected = ExpectedEffect::new(&root, command, BTreeMap::new(), value, 23);
        expected.gnu_folded = true;
        let provider = TestEffects::new(vec![expected]);
        let mut program = source_program(&root, &dirs);
        let before = program.events().to_vec();
        let result = program.read_captured_files_with_shell_effects(
            &["Root.mk"],
            &paths,
            &captured,
            &provider,
        );
        if value.contains('\u{e000}') {
            assert!(result.unwrap_err().contains("reserved internal delimiter"));
            assert_eq!(program.events(), before);
        } else if value.contains('\n') {
            assert!(result
                .unwrap_err()
                .contains("output exceeds its admitted text domain"));
            assert_eq!(program.events(), before);
        } else {
            result.unwrap();
            assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), value);
            assert_eq!(program.evaluate("$(STATUS)", &paths).unwrap(), "23");
        }
        assert_eq!(provider.revalidations.get(), 2);
    }
}

#[test]
fn shell_dollar_filenames_match_gnu_wildcard_and_abspath() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("$name.c"), "fixture\n").unwrap();
    let script = root.join("filename-script");
    std::fs::write(&script, "#!/bin/sh\nprintf '%s' '$name.c'\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let command = "./filename-script";
    std::fs::write(root.join("Root.mk"), format!(
        "SHELL := /bin/sh\nVALUE := $(wildcard $(shell {command}))\nABS := $(abspath $(shell {command}))\nprobe:\n\t$(info OBSERVED=$(VALUE)|$(ABS))\n"
    )).unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let provider = TestEffects::new(
        (0..2)
            .map(|_| ExpectedEffect::new(&root, command.into(), BTreeMap::new(), "$name.c", 0))
            .collect(),
    );
    let mut program = source_program(&root, &dirs);
    program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .unwrap();
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "$name.c");
    let absolute = format!("{}/$name.c", root.display());
    assert_eq!(program.evaluate("$(ABS)", &paths).unwrap(), absolute);
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let native = std::process::Command::new(make)
        .args(["-r", "-s", "--no-print-directory", "-f", "Root.mk", "probe"])
        .current_dir(&root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(
        String::from_utf8(native.stdout).unwrap(),
        format!("OBSERVED=$name.c|{absolute}\n")
    );
}

#[test]
fn nul_command_is_rejected_before_provider_query() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let command = opaque_command(&root, "nul-command");
    std::fs::write(
        root.join("Root.mk"),
        format!("VALUE := $(shell {command}\0)\n"),
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let provider = TestEffects::new(Vec::new());
    let mut program = source_program(&root, &dirs);
    let before = program.events().to_vec();
    let error = program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .unwrap_err();

    assert!(
        error.contains("Make input byte/line/depth limit"),
        "{error}"
    );
    assert_eq!(program.events(), before);
    assert_eq!(provider.queries.get(), 0);
    assert_eq!(provider.finishes.get(), 0);
    assert_eq!(provider.revalidations.get(), 2);
}

#[test]
fn direct_effect_nul_delimiter_is_refused_without_query() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let program = source_program(&root, &dirs);
    let staged = program.stage().unwrap();
    let provider = TestEffects::new(Vec::new());
    let session = super::captured_shell::EffectSession {
        native_context: None,
        provider: &provider,
        next: Cell::new(0),
    };
    let error = captured
        .transaction(|reader| {
            super::captured_shell::evaluate(
                "unknown\0effect",
                &std::cell::RefCell::new(reader),
                &staged,
                &paths,
                &program.expressions,
                (&program.state_work, &Cell::new(0)),
                Some(&session),
            )
        })
        .unwrap_err();
    assert!(error.contains("OS argument delimiter"));
    assert_eq!(provider.queries.get(), 0);
    assert_eq!(session.next.get(), 0);
    assert!(program.events().is_empty());
}

#[test]
fn modeled_intrinsics_also_require_selected_results_without_fallback() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(
        root.join("Root.mk"),
        "A := $(shell echo selected)\nB := $(shell false)\nSTATUS := $(.SHELLSTATUS)\n",
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let provider = TestEffects::new(vec![
        ExpectedEffect::new(
            &root,
            "echo selected".into(),
            BTreeMap::new(),
            "selected\n",
            0,
        ),
        ExpectedEffect::new(&root, "false".into(), BTreeMap::new(), "", 1),
    ]);
    let mut program = source_program(&root, &dirs);
    program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .unwrap();
    assert_eq!(provider.queries.get(), 2);
    assert_eq!(
        program.evaluate("$(A)|$(STATUS)", &paths).unwrap(),
        "selected|1"
    );
    let mut refused = source_program(&root, &dirs);
    let missing = TestEffects::new(Vec::new());
    assert!(refused
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &missing)
        .is_err());
    assert!(refused.events().is_empty());
    assert_eq!(missing.queries.get(), 1);
    assert_eq!(missing.revalidations.get(), 2);
}

#[test]
fn result_provider_cannot_supply_missing_shell_policy() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("Root.mk"), "A := $(shell echo selected)\n").unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    let provider = TestEffects::new(Vec::new());
    assert!(program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .is_err());
    assert_eq!(provider.queries.get(), 0);
    assert_eq!(provider.revalidations.get(), 0);
    assert!(program.events().is_empty());
}

#[test]
fn request_debug_omits_raw_context_values() {
    let exports = exports("PRIVATE_KEY", "PRIVATE_VALUE");
    let request = crate::make_shell_effects::ShellEffectRequest {
        native_context: None,
        ordinal: 0,
        command: "PRIVATE_COMMAND",
        working_directory: Path::new("/PRIVATE_CWD"),
        shell: "/PRIVATE_SHELL",
        shell_flags: "PRIVATE_FLAGS",
        source_exports: &exports,
        source_export_controls: &BTreeMap::new(),
    };
    assert!(!format!("{request:?}").contains("PRIVATE"));
}

#[test]
fn source_unexport_is_distinct_from_absent_export_selection() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let command = opaque_command(&root, "export-controls");
    std::fs::write(
        root.join("Root.mk"),
        format!("export TOKEN := selected\nunexport ENGINE_SHAPED\nVALUE := $(shell {command})\n"),
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let mut selected = ExpectedEffect::new(
        &root,
        command.clone(),
        exports("TOKEN", "selected"),
        "bound",
        0,
    );
    selected
        .source_export_controls
        .insert("ENGINE_SHAPED".into(), false);
    let provider = TestEffects::new(vec![selected]);
    let mut program = source_program(&root, &dirs);
    program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .unwrap();
    assert_eq!(provider.cursor.get(), 1);
    let incomplete = TestEffects::new(vec![ExpectedEffect::new(
        &root,
        command,
        exports("TOKEN", "selected"),
        "must-not-publish",
        0,
    )]);
    let mut refused = source_program(&root, &dirs);
    assert!(refused
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &incomplete)
        .is_err());
    assert!(refused.events().is_empty());
    assert_eq!(incomplete.revalidations.get(), 2);
}

#[test]
fn deferred_recursive_shell_effect_cannot_reopen_completed_provider_session() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let command = opaque_command(&root, "deferred");
    std::fs::write(
        root.join("Root.mk"),
        format!("DEFERRED = $(shell {command})\n"),
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let provider = TestEffects::new(Vec::new());
    let mut program = source_program(&root, &dirs);

    program
        .read_captured_files_with_shell_effects(&["Root.mk"], &paths, &captured, &provider)
        .unwrap();
    assert_eq!(provider.finishes.get(), 1);
    assert_eq!(provider.revalidations.get(), 2);
    assert_eq!(provider.cursor.get(), 0);
    assert_eq!(provider.queries.get(), 0);

    assert!(program.evaluate("$(DEFERRED)", &paths).is_err());
    assert_eq!(provider.cursor.get(), 0);
    assert_eq!(provider.queries.get(), 0);
    assert_eq!(provider.revalidations.get(), 2);
}

#[test]
fn captured_stdout_and_status_match_actual_renamed_script_under_gnu_make() {
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let version = Command::new(make).arg("--version").output().unwrap();
    let version = String::from_utf8_lossy(&version.stdout);
    assert!(version.contains("GNU Make"));
    let version_line = version.lines().next().unwrap_or_default();
    let (major, minor) = version_line
        .split_whitespace()
        .last()
        .and_then(|value| {
            let mut parts = value.split('.');
            Some((
                parts.next()?.parse::<u32>().ok()?,
                parts.next()?.parse::<u32>().ok()?,
            ))
        })
        .expect("GNU Make version must be explicit");
    assert!(
        (major, minor) >= (4, 2),
        "GNU Make oracle must implement .SHELLSTATUS"
    );

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let script = root.join("renamed-data-producer");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf 'alpha\\n\\nbeta $literal $$(error NOT_A_SECOND_EVAL)\\n'\nexit 23\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let command = format!("'{}'", script.display());
    std::fs::write(
        root.join("Observed.mk"),
        format!("VALUE := $(shell {command})\nSTATUS := $(.SHELLSTATUS)\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("Native.mk"),
        format!("SHELL := /bin/sh\nVALUE := $(shell {command})\nSTATUS := $(.SHELLSTATUS)\nall:\n\t@printf '%s|%s\\n' '$(VALUE)' '$(STATUS)'\n"),
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let provider = TestEffects::new(vec![ExpectedEffect::new(
        &root,
        command,
        BTreeMap::new(),
        "alpha\n\nbeta $literal $$(error NOT_A_SECOND_EVAL)\n",
        23,
    )]);
    let mut program = source_program(&root, &dirs);
    program
        .read_captured_files_with_shell_effects(&["Observed.mk"], &paths, &captured, &provider)
        .unwrap();
    let captured_value = program.evaluate("$(VALUE)", &paths).unwrap();
    let captured_status = program.evaluate("$(STATUS)", &paths).unwrap();

    let native = Command::new(make)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("LC_ALL", "C")
        .arg("--no-print-directory")
        .arg("-f")
        .arg("Native.mk")
        .arg("all")
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(native.status.success());
    assert_eq!(
        String::from_utf8(native.stdout).unwrap(),
        "alpha  beta $literal $$(error NOT_A_SECOND_EVAL)|23\n"
    );
    assert_eq!(
        captured_value,
        "alpha  beta $literal $$(error NOT_A_SECOND_EVAL)"
    );
    assert_eq!(captured_status, "23");
}
