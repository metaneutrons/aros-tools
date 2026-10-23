//! The opt-in shell policy binds each captured effect, not only final controls.

use super::*;
use aros_common::{
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::ValidatedMakeNamespace,
    measure_tree_content_cas,
};
use std::{collections::BTreeMap, os::unix::fs::PermissionsExt, path::Path};

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
    ValidatedMakeNamespace::from_measurement(
        "v1:3;.VARIABLES|default;MAKE_VERSION|default;.SHELLFLAGS|default;end",
    )
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
        &[MakeInitialValue {
            name: ".SHELLFLAGS",
            value: "-c",
            flavor: MakeInitialFlavor::Recursive,
            origin: aros_common::make_namespace::MakeVariableOrigin::Default,
        }],
    )
    .unwrap();
    program
        .require_captured_shell_policy("/bin/sh", &["-c", "-ec"], &BTreeMap::new())
        .unwrap();
    program
}

struct TestSearch {
    path: std::path::PathBuf,
    programs: aros_common::captured_programs::CapturedProgramSet,
}

impl CapturedShellSearch for TestSearch {
    fn path(&self) -> &Path {
        &self.path
    }
    fn revalidate(&self) -> Result<(), String> {
        self.programs.transaction(|_| Ok(()))
    }
    fn validate_command(&self, name: &str) -> Result<(), String> {
        if self
            .programs
            .identity_for_role(name)
            .is_some_and(|identity| identity.invocation_path() == self.path.join(name))
        {
            Ok(())
        } else {
            Err("missing explicitly selected command".into())
        }
    }
}

struct MutatingSearch {
    original: Arc<dyn CapturedShellSearch>,
    tool: std::path::PathBuf,
}

impl CapturedShellSearch for MutatingSearch {
    fn path(&self) -> &Path {
        self.original.path()
    }
    fn revalidate(&self) -> Result<(), String> {
        self.original.revalidate()
    }
    fn validate_command(&self, name: &str) -> Result<(), String> {
        self.original.validate_command(name)?;
        // Isolated deterministic race: drift after the per-effect check passed.
        std::fs::write(&self.tool, b"changed during modeled effect")
            .map_err(|_| "cannot inject isolated tool drift".to_owned())
    }
}

fn test_search(path: &str, role: &str) -> Arc<dyn CapturedShellSearch> {
    test_search_word(path, role, "sed")
}

fn test_search_word(path: &str, role: &str, word: &str) -> Arc<dyn CapturedShellSearch> {
    use aros_common::captured_programs::{
        CapturedProgramSet, ProgramRootSelection, ProgramSelection,
    };
    let path = Path::new(path).to_owned();
    let programs = CapturedProgramSet::capture(
        &[ProgramRootSelection {
            root: path.clone(),
            content_sha256: measure_tree_content_cas(&path)
                .unwrap()
                .payload_digest_excluding(None),
        }],
        &[ProgramSelection {
            role: role.into(),
            root_index: 0,
            invocation_path: path.join(word),
            executable_sha256: aros_common::sha256_bytes(b"#!/bin/sh\nexit 0\n"),
        }],
    )
    .unwrap();
    Arc::new(TestSearch { path, programs })
}

#[test]
fn transliteration_keeps_external_role_binding_and_staged_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = root.join("source");
    let tools = root.join("tools");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&tools).unwrap();
    std::fs::write(tools.join("tr"), b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(tools.join("tr"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        source.join("Root.mk"),
        "SHELL := /bin/sh\nVALUE := $(shell echo MiXeD-123 | tr A-Z a-z)\n",
    )
    .unwrap();
    let captured = snapshot(&source);
    let dirs = DirVars::default();
    let paths = paths(&source, &dirs);
    let inherited = tools.to_str().unwrap();
    let mut valid = search_program_bound(
        &source,
        &dirs,
        inherited,
        test_search_word(inherited, "tr", "tr"),
    );
    valid
        .read_captured_file("Root.mk", &paths, &captured)
        .unwrap();
    assert_eq!(valid.evaluate("$(VALUE)", &paths).unwrap(), "mixed-123");

    let mut wrong_role = search_program_bound(
        &source,
        &dirs,
        inherited,
        test_search_word(inherited, "different", "tr"),
    );
    assert!(wrong_role
        .read_captured_file("Root.mk", &paths, &captured)
        .err()
        .unwrap()
        .contains("missing explicitly selected command"));
    assert!(wrong_role.events().is_empty());

    let mut drifted = search_program_bound(
        &source,
        &dirs,
        inherited,
        test_search_word(inherited, "tr", "tr"),
    );
    std::fs::write(tools.join("tr"), b"changed selected tool").unwrap();
    assert!(drifted
        .read_captured_file("Root.mk", &paths, &captured)
        .is_err());
    assert!(drifted.events().is_empty());
    captured.verify().unwrap();
}

fn search_program(root: &Path, dirs: &DirVars, inherited_path: &str) -> MakeVariableProgram {
    search_program_selected(root, dirs, inherited_path, "sed")
}

fn search_program_selected(
    root: &Path,
    dirs: &DirVars,
    inherited_path: &str,
    role: &str,
) -> MakeVariableProgram {
    search_program_bound(
        root,
        dirs,
        inherited_path,
        test_search(inherited_path, role),
    )
}

fn search_program_bound(
    root: &Path,
    dirs: &DirVars,
    inherited_path: &str,
    search: Arc<dyn CapturedShellSearch>,
) -> MakeVariableProgram {
    let measured = ValidatedMakeNamespace::from_measurement(
        "v1:4;.VARIABLES|default;MAKE_VERSION|default;.SHELLFLAGS|default;PATH|environment;end",
    )
    .unwrap();
    let mut program = MakeVariableProgram::new_source_program(
        &paths(root, dirs),
        &measured,
        &[
            MakeInitialValue {
                name: ".SHELLFLAGS",
                value: "-c",
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Default,
            },
            MakeInitialValue {
                name: "PATH",
                value: inherited_path,
                flavor: MakeInitialFlavor::Recursive,
                origin: MakeVariableOrigin::Environment,
            },
        ],
    )
    .unwrap();
    program
        .require_captured_shell_policy(
            "/bin/sh",
            &["-c"],
            &BTreeMap::from([("PATH".into(), inherited_path.into())]),
        )
        .unwrap();
    program
        .bind_captured_shell_source_path_prefixes(Arc::clone(&search))
        .unwrap();
    assert!(program
        .bind_captured_shell_source_path_prefixes(search)
        .is_err());
    program
}

#[test]
fn inherited_external_role_is_required_and_revalidated_even_without_path_change() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = root.join("source");
    let tools = root.join("tools");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&tools).unwrap();
    std::fs::write(tools.join("sed"), b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(tools.join("sed"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        source.join("Root.mk"),
        "SHELL := /bin/sh\nVALUE := $(shell echo before | sed 's/before/after/')\n",
    )
    .unwrap();
    let captured = snapshot(&source);
    let dirs = DirVars::default();
    let paths = paths(&source, &dirs);
    let inherited = tools.to_str().unwrap();
    let mut valid = search_program(&source, &dirs, inherited);
    valid
        .read_captured_file("Root.mk", &paths, &captured)
        .unwrap();
    assert_eq!(valid.evaluate("$(VALUE)", &paths).unwrap(), "after");

    let mut wrong_role = search_program_selected(&source, &dirs, inherited, "different");
    assert!(wrong_role
        .read_captured_file("Root.mk", &paths, &captured)
        .err()
        .unwrap()
        .contains("missing explicitly selected command"));
    assert!(wrong_role.events().is_empty());

    let mut raced = search_program_bound(
        &source,
        &dirs,
        inherited,
        Arc::new(MutatingSearch {
            original: test_search(inherited, "sed"),
            tool: tools.join("sed"),
        }),
    );
    assert!(raced
        .read_captured_file("Root.mk", &paths, &captured)
        .is_err());
    assert!(
        raced.events().is_empty(),
        "post-effect drift must prevent staged state publication"
    );
    captured.verify().unwrap();
    std::fs::write(tools.join("sed"), b"#!/bin/sh\nexit 0\n").unwrap();

    let mut changed = search_program(&source, &dirs, inherited);
    std::fs::write(tools.join("sed"), b"different tool").unwrap();
    assert!(changed
        .read_captured_file("Root.mk", &paths, &captured)
        .is_err());
    assert!(changed.events().is_empty());
    captured.verify().unwrap(); // The Source snapshot did not change: the search binding detected this.
}

#[test]
fn bound_shell_policy_requires_external_search_but_legacy_data_is_not_native_proof() {
    for (command, expected) in [
        ("echo before | sed 's/before/after/'", "after"),
        ("echo MiXeD | tr A-Z a-z", "mixed"),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        std::fs::write(
            root.join("Root.mk"),
            format!("SHELL := /bin/sh\nVALUE := $(shell {command})\n"),
        )
        .unwrap();
        let captured = snapshot(&root);
        let dirs = DirVars::default();
        let paths = paths(&root, &dirs);
        let mut bound = source_program(&root, &dirs);
        let error = bound
            .read_captured_file("Root.mk", &paths, &captured)
            .err()
            .unwrap();
        assert!(error.contains("lacks a bound search capability"));
        assert!(bound.events().is_empty());

        let mut data_only =
            MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
        data_only
            .read_captured_file("Root.mk", &paths, &captured)
            .unwrap();
        assert_eq!(data_only.evaluate("$(VALUE)", &paths).unwrap(), expected);
        assert!(
            !data_only.captured_shell_policy_bound(),
            "data-only interpretation cannot satisfy the native policy gate"
        );
    }
}

#[test]
fn source_search_prefix_preserves_inherited_lookup_and_refuses_shadowing_per_effect() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let inherited = root.join("sealed-tools").to_str().unwrap().to_owned();
    for directory in ["first", "second", "shadow", "sealed-tools"] {
        std::fs::create_dir(root.join(directory)).unwrap();
    }
    std::fs::write(root.join("sealed-tools/sed"), b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(
        root.join("sealed-tools/sed"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(root.join("shadow/sed"), b"not even executable").unwrap();
    std::os::unix::fs::symlink(root.join("first"), root.join("linked")).unwrap();
    let pipeline = "VALUE := $(shell echo before | sed 's/before/after/')\n";
    let prefix = |value: &str| format!("SHELL := /bin/sh\nexport PATH := {value}:{inherited}\n");
    let cases = [
        (
            "valid",
            format!(
                "{}{pipeline}",
                prefix(&format!(
                    "{}/first:{}/second",
                    root.display(),
                    root.display()
                ))
            ),
            true,
        ),
        (
            "absent",
            format!(
                "{}{pipeline}",
                prefix(&format!("{}/missing/descendant", root.display()))
            ),
            true,
        ),
        (
            "shadow",
            format!(
                "{}{pipeline}",
                prefix(&format!(
                    "{}/first:{}/shadow",
                    root.display(),
                    root.display()
                ))
            ),
            false,
        ),
        (
            "linked",
            format!(
                "{}{pipeline}",
                prefix(&format!("{}/linked", root.display()))
            ),
            false,
        ),
        (
            "relative",
            format!("{}{pipeline}", prefix("relative")),
            false,
        ),
        ("empty", format!("{}{pipeline}", prefix("")), false),
        (
            "outside",
            format!("{}{pipeline}", prefix("/unbound-search-root")),
            false,
        ),
        (
            "replacement",
            format!(
                "SHELL := /bin/sh\nexport PATH := {}/first\n{pipeline}",
                root.display()
            ),
            false,
        ),
        (
            "transient",
            format!(
                "{}{pipeline}export PATH := {inherited}\n",
                prefix(&format!("{}/shadow", root.display()))
            ),
            false,
        ),
        (
            "locale",
            format!(
                "{}export LC_ALL := tr_TR.UTF-8\n{pipeline}",
                prefix(&format!("{}/first", root.display()))
            ),
            false,
        ),
    ];
    for (name, source, _) in &cases {
        std::fs::write(root.join(format!("{name}.mk")), source).unwrap();
    }
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    for (name, _, pass) in cases {
        let mut program = search_program(&root, &dirs, &inherited);
        let result = program.read_captured_file(&format!("{name}.mk"), &paths, &captured);
        if pass {
            result.unwrap();
            assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "after");
            assert!(program
                .bind_captured_shell_source_path_prefixes(test_search(&inherited, "sed"))
                .is_err());
        } else {
            assert!(
                result.is_err(),
                "{name} must refuse instead of changing executable selection"
            );
            assert!(
                program.events().is_empty(),
                "failed Source effects must roll back"
            );
        }
    }
    std::fs::write(root.join("first/sed"), b"late shadow").unwrap();
    let mut program = search_program(&root, &dirs, &inherited);
    assert!(program
        .read_captured_file("valid.mk", &paths, &captured)
        .is_err());
}

#[test]
fn policy_accepts_the_selected_shell_and_rejects_transient_controls_per_effect() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(
        root.join("ordinary.mk"),
        concat!("SHELL := /bin/sh\n", "VALUE := $(shell printf '%s' ok)\n",),
    )
    .unwrap();
    std::fs::write(
        root.join("transient.mk"),
        concat!(
            "SHELL := /bin/sh\n",
            "BEFORE := $(shell printf ok)\n",
            "SHELL := /tmp/untrusted-shell\n",
            "DURING := $(shell printf hidden)\n",
            "SHELL := /bin/sh\n",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("hook.mk"),
        concat!(
            "SHELL := /bin/sh\n",
            "export BASH_ENV := /tmp/private-shell-hook\n",
            "DURING := $(shell printf hidden)\n",
            "unexport BASH_ENV\n",
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("nested.mk"),
        concat!(
            "SHELL = $(shell printf /bin/sh)\n",
            "DURING := $(shell printf hidden)\n",
        ),
    )
    .unwrap();

    for (name, control) in [
        ("path.mk", "PATH := /tmp/unselected-programs"),
        ("locale.mk", "LC_ALL := tr_TR.UTF-8"),
    ] {
        std::fs::write(root.join(name), format!("SHELL := /bin/sh\nexport {control}\nDURING := $(shell echo before | sed 's/before/after/')\n")).unwrap();
    }
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);

    let mut ordinary = source_program(&root, &dirs);
    ordinary
        .read_captured_file("ordinary.mk", &paths, &captured)
        .unwrap();
    assert_eq!(ordinary.evaluate("$(VALUE)", &paths).unwrap(), "ok");
    assert_eq!(ordinary.evaluate("$(.SHELLSTATUS)", &paths).unwrap(), "0");
    let mut errexit = MakeVariableProgram::new_source_program(
        &paths,
        &namespace(),
        &[MakeInitialValue {
            name: ".SHELLFLAGS",
            value: "-ec",
            flavor: MakeInitialFlavor::Recursive,
            origin: aros_common::make_namespace::MakeVariableOrigin::Default,
        }],
    )
    .unwrap();
    errexit
        .require_captured_shell_policy("/bin/sh", &["-c", "-ec"], &BTreeMap::new())
        .unwrap();
    let error = errexit
        .read_captured_file("ordinary.mk", &paths, &captured)
        .err()
        .unwrap();
    assert!(error.contains("alternative execution flags"));

    for input in [
        "transient.mk",
        "hook.mk",
        "nested.mk",
        "path.mk",
        "locale.mk",
    ] {
        let mut program = source_program(&root, &dirs);
        let before = program.events().to_vec();
        assert!(
            program
                .read_captured_file(input, &paths, &captured)
                .is_err(),
            "{input} must be refused at its captured shell effect"
        );
        assert_eq!(program.events(), before);
        assert!(program.captured_shell_policy_bound());
        assert_eq!(program.evaluate("$(DURING)", &paths).unwrap(), "");
    }
}

#[test]
fn policy_checks_the_effective_locale_before_bytewise_globbing() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("a"), b"a").unwrap();
    std::fs::write(root.join("b"), b"b").unwrap();
    std::fs::write(
        root.join("Root.mk"),
        "SHELL := /bin/sh\nVALUE := $(shell for name in [a-z]*; do printf '%s' \"$$name\"; done)\n",
    )
    .unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let namespace = ValidatedMakeNamespace::from_measurement(
        "v1:4;.VARIABLES|default;MAKE_VERSION|default;.SHELLFLAGS|default;LC_ALL|environment;end",
    )
    .unwrap();
    for locale in ["C", "tr_TR.UTF-8"] {
        let mut program = MakeVariableProgram::new_source_program(
            &paths,
            &namespace,
            &[
                MakeInitialValue {
                    name: ".SHELLFLAGS",
                    value: "-c",
                    flavor: MakeInitialFlavor::Recursive,
                    origin: aros_common::make_namespace::MakeVariableOrigin::Default,
                },
                MakeInitialValue {
                    name: "LC_ALL",
                    value: locale,
                    flavor: MakeInitialFlavor::Recursive,
                    origin: aros_common::make_namespace::MakeVariableOrigin::Environment,
                },
            ],
        )
        .unwrap();
        program
            .require_captured_shell_policy(
                "/bin/sh",
                &["-c"],
                &BTreeMap::from([("LC_ALL".into(), locale.into())]),
            )
            .unwrap();
        let result = program.read_captured_file("Root.mk", &paths, &captured);
        if locale == "C" {
            result.unwrap();
            assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "ab");
        } else {
            assert!(result.err().unwrap().contains("bytewise POSIX locale"));
        }
    }
}

#[test]
fn policy_must_be_bound_once_before_source_input_in_measured_source_mode() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("plain.mk"), "VALUE := stable\n").unwrap();
    let captured = snapshot(&root);
    let dirs = DirVars::default();
    let paths = paths(&root, &dirs);
    let inherited = BTreeMap::new();

    let mut source = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    assert!(source
        .require_captured_shell_policy("/bin/sh", &["-c"], &inherited)
        .is_ok());
    assert!(source
        .require_captured_shell_policy("/bin/sh", &["-c"], &inherited)
        .is_err());

    let mut read = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    read.read_captured_file("plain.mk", &paths, &captured)
        .unwrap();
    assert!(read
        .require_captured_shell_policy("/bin/sh", &["-c"], &inherited)
        .is_err());

    let mut effected = MakeVariableProgram::new_source_program(&paths, &namespace(), &[]).unwrap();
    effected.execute("VALUE := earlier", 1, &paths).unwrap();
    assert!(effected
        .require_captured_shell_policy("/bin/sh", &["-c"], &inherited)
        .is_err());

    let mut projection = MakeVariableProgram::new_with_namespace(
        &TargetContext::default(),
        &[],
        &paths,
        &namespace(),
    )
    .unwrap();
    assert!(projection
        .require_captured_shell_policy("/bin/sh", &["-c"], &inherited)
        .is_err());
}

fn direct_source_search_fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    String,
    Arc<dyn CapturedShellSearch>,
) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let tools = root.join("sealed-tools");
    let first = root.join("first");
    let shadow = root.join("shadow");
    let linked_candidate = root.join("linked-candidate");
    for directory in [&tools, &first, &shadow, &linked_candidate] {
        std::fs::create_dir(directory).unwrap();
    }
    std::fs::write(tools.join("mkdir"), b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(tools.join("mkdir"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(shadow.join("mkdir"), b"not executable").unwrap();
    std::os::unix::fs::symlink(&first, root.join("linked")).unwrap();
    std::os::unix::fs::symlink(tools.join("mkdir"), linked_candidate.join("mkdir")).unwrap();
    std::fs::write(root.join("Root.mk"), "VALUE := captured\n").unwrap();
    let inherited = tools.to_str().unwrap().to_owned();
    let search = test_search_word(&inherited, "mkdir", "mkdir");
    (temporary, root, inherited, search)
}

#[test]
fn source_search_api_accepts_exact_path_and_safe_prefixes_with_the_sealed_suffix() {
    let (_temporary, root, inherited, search) = direct_source_search_fixture();
    let captured = snapshot(&root);
    let first = root.join("first");
    let absent = root.join("missing").join("child");

    let ((), exact_reads) = captured
        .transaction(|reader| {
            validate_captured_source_commands(search.as_ref(), &inherited, &["mkdir"], reader)
        })
        .unwrap();
    assert!(exact_reads.reads().is_empty());

    let current = format!("{}:{}:{inherited}", first.display(), absent.display());
    let ((), prefixed_reads) = captured
        .transaction(|reader| {
            validate_captured_source_commands(search.as_ref(), &current, &["mkdir"], reader)
        })
        .unwrap();
    assert!(prefixed_reads.reads().is_empty());
    captured.verify().unwrap();
}

#[test]
fn source_search_api_refuses_unsafe_prefixes_and_earlier_mkdir_candidates() {
    let (_temporary, root, inherited, search) = direct_source_search_fixture();
    let captured = snapshot(&root);
    let first = root.join("first");
    let shadow = root.join("shadow");
    let linked = root.join("linked");
    let linked_candidate = root.join("linked-candidate");
    let outside = tempfile::tempdir().unwrap();
    let cases = [
        (
            "non-executable earlier candidate",
            format!("{}:{}:{inherited}", first.display(), shadow.display()),
        ),
        (
            "linked directory",
            format!("{}:{inherited}", linked.display()),
        ),
        (
            "linked earlier candidate",
            format!("{}:{inherited}", linked_candidate.display()),
        ),
        ("replaced PATH", first.to_string_lossy().into_owned()),
        ("relative directory", format!("relative:{inherited}")),
        ("empty directory entry", format!(":{inherited}")),
        (
            "unbound directory",
            format!("{}:{inherited}", outside.path().display()),
        ),
        ("absent PATH", String::new()),
    ];

    for (name, current) in cases {
        let result = captured.transaction(|reader| {
            validate_captured_source_commands(search.as_ref(), &current, &["mkdir"], reader)
        });
        assert!(result.is_err(), "{name} must be refused");
    }
}

#[test]
fn source_search_api_requires_selected_bare_command_roles_and_bounded_requests() {
    let (_temporary, root, inherited, search) = direct_source_search_fixture();
    let captured = snapshot(&root);

    let missing_role = captured.transaction(|reader| {
        validate_captured_source_commands(search.as_ref(), &inherited, &["sed"], reader)
    });
    assert!(missing_role
        .unwrap_err()
        .contains("missing explicitly selected command"));

    for command in ["mkdir/child", "mkdir:other", ""] {
        let result = captured.transaction(|reader| {
            validate_captured_source_commands(search.as_ref(), &inherited, &[command], reader)
        });
        assert!(
            result.is_err(),
            "{command:?} must not be accepted as a bare command"
        );
    }

    let too_many_commands = vec!["mkdir"; 65];
    assert!(captured
        .transaction(|reader| {
            validate_captured_source_commands(
                search.as_ref(),
                &inherited,
                &too_many_commands,
                reader,
            )
        })
        .unwrap_err()
        .contains("exceeds its query bound"));

    let oversized_path = format!("{}:{inherited}", "x".repeat(128 * 1024));
    assert!(captured
        .transaction(|reader| {
            validate_captured_source_commands(search.as_ref(), &oversized_path, &["mkdir"], reader)
        })
        .unwrap_err()
        .contains("exceeds its query bound"));

    let first = root.join("first").to_string_lossy().into_owned();
    let too_many_directories = std::iter::repeat_n(first.as_str(), 257)
        .collect::<Vec<_>>()
        .join(":");
    let current = format!("{too_many_directories}:{inherited}");
    assert!(captured
        .transaction(|reader| {
            validate_captured_source_commands(search.as_ref(), &current, &["mkdir"], reader)
        })
        .unwrap_err()
        .contains("exceeds its query bound"));
}

#[test]
fn source_search_api_rechecks_late_shadow_when_file_read_replay_is_unchanged() {
    let (_temporary, root, inherited, search) = direct_source_search_fixture();
    let captured = snapshot(&root);
    let current = format!("{}:{inherited}", root.join("first").display());
    let (suffix, reads) = captured
        .transaction(|reader| {
            let checkpoint = reader.read_checkpoint();
            validate_captured_source_commands(search.as_ref(), &current, &["mkdir"], reader)?;
            reader.read("Root.mk")?;
            reader.provisional_reads_since(checkpoint)
        })
        .unwrap();
    assert_eq!(suffix.reads().len(), 1);
    assert_eq!(reads.reads(), suffix.reads());

    std::fs::write(root.join("first/mkdir"), b"late shadow").unwrap();
    assert!(captured.verify_read_groups(&[&suffix]).is_err());

    let fresh = snapshot(&root);
    fresh.verify_read_groups(&[&suffix]).unwrap();
    let rechecked = fresh.transaction(|reader| {
        validate_captured_source_commands(search.as_ref(), &current, &["mkdir"], reader)
    });
    assert!(rechecked
        .unwrap_err()
        .contains("earlier external-command candidate"));
}
