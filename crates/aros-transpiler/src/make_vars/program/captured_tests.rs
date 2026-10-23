//! Captured input composition preserves GNU read spellings and atomic state.

use super::*;
use aros_common::{
    make_initial_values::{
        MakeInitialValueFlavor, ValidatedMakeInitialValues, ValidatedMakeParserControls,
    },
    make_input_snapshot::{MakeInputRootSelection, MakeInputSnapshot},
    make_namespace::{MakeNamespaceEntry, MakeNamespaceObservation},
    make_observer::{source_prelude_fragment, validate_source_prelude_inputs},
    measure_tree_content_cas,
};
use std::{collections::BTreeMap, process::Command};

fn snapshot(root: &Path, directories: &[&str]) -> MakeInputSnapshot {
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
    .with_include_directories(
        &directories
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

#[test]
fn captured_orphan_blank_recipe_lines_match_gnu_without_admitting_commands() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(
        root.join("blank.mk"),
        "\t\n\t  \n\t# ignored before any rule \\\n\t@printf 'must-not-run'\nVALUE := retained\n\t \t\n\t # ignored after assignment\nprobe: ;\n",
    )
    .unwrap();
    std::fs::write(
        root.join("orphan.mk"),
        "\t@printf 'must-not-run'\nprobe: ;\n",
    )
    .unwrap();
    std::fs::write(root.join("escaped.mk"), "\t\\#not-a-comment\nprobe: ;\n").unwrap();
    std::fs::write(root.join("unicode.mk"), "\t\u{a0}\nprobe: ;\n").unwrap();
    std::fs::write(
        root.join("after-comment.mk"),
        "\t# not continued\n\t@printf 'must-not-run'\nprobe: ;\n",
    )
    .unwrap();
    let captured = snapshot(&root, &[]);
    let namespace = ValidatedMakeNamespace::from_measurement(
        "v1:2;.VARIABLES|default;MAKE_VERSION|default;end",
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
    for (name, accepted) in [
        ("blank.mk", true),
        ("orphan.mk", false),
        ("escaped.mk", false),
        ("unicode.mk", false),
        ("after-comment.mk", false),
    ] {
        let native = Command::new(if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        })
        .current_dir(&root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .args(["-rR", "-n", "--no-print-directory", "-f", name, "probe"])
        .output()
        .unwrap();
        assert_eq!(native.status.success(), accepted, "native oracle: {name}");
        let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
        let result = program.read_captured_file(name, &paths, &captured);
        if accepted {
            let (file, reads) = result.unwrap();
            assert_eq!(reads.reads().len(), 1);
            assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "retained");
            assert!(file.events.iter().any(|event| event.source_line == 7));
            assert!(file
                .events
                .iter()
                .all(|event| matches!(event.source_line, 4 | 7)));
        } else {
            assert!(String::from_utf8_lossy(&native.stderr)
                .contains("recipe commences before first target"));
            assert!(result
                .unwrap_err()
                .contains("Make recipe has no preceding rule"));
            assert!(program.events().is_empty());
        }
    }
}

#[test]
fn captured_blank_recipe_after_rule_keeps_ownership_and_disables_implicit_search() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("x.in"), "input\n").unwrap();
    for (name, tail) in [
        ("absent.mk", "x.out:\n"),
        ("empty.mk", "x.out:\n\t\n"),
        ("blank.mk", "x.out:\n\t  \n"),
        ("comment.mk", "x.out:\n\t# retained recipe comment\n"),
    ] {
        std::fs::write(
            root.join(name),
            format!("%.out: %.in\n\t@printf 'implicit\\n'\n{tail}"),
        )
        .unwrap();
    }
    let captured = snapshot(&root, &[]);
    let namespace = ValidatedMakeNamespace::from_measurement(
        "v1:2;.VARIABLES|default;MAKE_VERSION|default;end",
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
    for (name, expected_recipe, expected_native) in [
        ("absent.mk", None, "printf 'implicit\\n'\n"),
        ("empty.mk", Some(""), ""),
        ("blank.mk", Some("  "), ""),
        (
            "comment.mk",
            Some("# retained recipe comment"),
            "# retained recipe comment\n",
        ),
    ] {
        let native = Command::new(if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        })
        .current_dir(&root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .args([
            "-rR",
            "-n",
            "-s",
            "--no-print-directory",
            "-f",
            name,
            "x.out",
        ])
        .output()
        .unwrap();
        assert!(native.status.success(), "native oracle: {name}");
        assert_eq!(
            String::from_utf8(native.stdout).unwrap(),
            expected_native,
            "{name}"
        );
        let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
        let (file, _) = program.read_captured_file(name, &paths, &captured).unwrap();
        let recipes = file
            .events
            .iter()
            .filter_map(|event| match &event.kind {
                MakeFileEventKind::Recipe { rule_event, text } if event.source_line == 3 => {
                    assert_eq!(file.events[*rule_event].source_line, 2);
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            recipes,
            expected_recipe.into_iter().collect::<Vec<_>>(),
            "{name}"
        );
    }
}

#[test]
fn captured_comments_preserve_raw_input_identity_without_decoding_semantic_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let root_bytes = b"# raw comment \xa9\nVALUE := root # another \xff\nHASH := $(subst #,X,a#b)\ninclude child.mk\nprobe:\n\t$(info OBSERVED=$(VALUE)|$(HASH))\n";
    let child_bytes = b"VALUE += child # raw include comment \xa9\n";
    std::fs::write(root.join("root.mk"), root_bytes).unwrap();
    std::fs::write(root.join("child.mk"), child_bytes).unwrap();
    let invalid_cases: &[(&str, &[u8])] = &[
        ("assignment.mk", b"VALUE := partial\nBROKEN := \xff\n"),
        ("escaped.mk", b"VALUE := partial\nBROKEN := \\#\xff\n"),
        ("recipe.mk", b"VALUE := partial\nprobe:\n\t# \xff\n"),
        ("inline.mk", b"VALUE := partial\nprobe: ; echo # \xff\n"),
        (
            "definition.mk",
            b"VALUE := partial\ndefine RAW\n# \xff\nendef\n",
        ),
        (
            "expression.mk",
            b"VALUE := partial\nBROKEN := $(subst #,X,a\xff)\n",
        ),
    ];
    for (name, bytes) in invalid_cases {
        std::fs::write(root.join(name), bytes).unwrap();
    }
    let captured = snapshot(&root, &[]);
    let namespace = ValidatedMakeNamespace::from_measurement(
        "v1:2;.VARIABLES|default;MAKE_VERSION|default;end",
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
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
    let (_, reads) = program
        .read_captured_file("root.mk", &paths, &captured)
        .unwrap();
    let replay = program.evaluate("$(VALUE)", &paths).unwrap();
    assert_eq!(replay, "root  child ");
    assert_eq!(program.evaluate("$(HASH)", &paths).unwrap(), "aXb");
    assert_eq!(reads.reads().len(), 2);
    for (read, expected_name, bytes) in [
        (&reads.reads()[0], "root.mk", root_bytes.as_slice()),
        (&reads.reads()[1], "child.mk", child_bytes.as_slice()),
    ] {
        assert_eq!(read.identity, expected_name);
        assert_eq!(read.content_sha256, Some(aros_common::sha256_bytes(bytes)));
    }
    let output = Command::new(if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    })
    .current_dir(&root)
    .env_clear()
    .env("PATH", std::env::var("PATH").unwrap())
    .args([
        "-r",
        "-n",
        "-s",
        "--no-print-directory",
        "-f",
        "root.mk",
        "--old-file=root.mk",
        "--old-file=child.mk",
        "probe",
    ])
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "synthetic native comment query failed"
    );
    assert_eq!(
        std::str::from_utf8(&output.stdout).unwrap(),
        format!("OBSERVED={replay}|aXb\n")
    );
    for (name, _) in invalid_cases {
        let prior_events = program.events().len();
        let failure = program
            .read_captured_file(name, &paths, &captured)
            .unwrap_err();
        assert!(failure.contains("UTF-8"), "wrong refusal for {name}");
        assert_eq!(program.events().len(), prior_events);
        assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), replay);
    }
}

#[test]
fn captured_source_include_search_matches_native_order_and_read_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("first")).unwrap();
    std::fs::create_dir(root.join("second")).unwrap();
    std::fs::create_dir(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/parent.mk"), "include child.mk\n").unwrap();
    std::fs::write(
        root.join("nested/child.mk"),
        "VALUE := wrong-parent-relative\n",
    )
    .unwrap();
    std::fs::write(
        root.join("root.mk"),
        concat!(
            "VALUE := before\nFIRST := $(MAKEFILE_LIST)\ninclude nested/parent.mk\n",
            "CAPTURE := $(VALUE)|$(FIRST)|$(MAKEFILE_LIST)\n",
            "probe:\n\t$(info OBSERVED=$(CAPTURE))\n",
        ),
    )
    .unwrap();
    for (directory, value) in [("first", "alpha"), ("second", "beta")] {
        std::fs::write(
            root.join(directory).join("child.mk"),
            format!("VALUE += {value}\n"),
        )
        .unwrap();
    }
    let observer_directory = tempfile::tempdir().unwrap();
    let observer = observer_directory.path().join("observer.mk");
    std::fs::write(
        &observer,
        format!(
            "{}\n$(info SEARCH=$(.INCLUDE_DIRS))\n",
            source_prelude_fragment()
        ),
    )
    .unwrap();
    let environment = BTreeMap::from([("PATH".into(), std::env::var("PATH").unwrap())]);
    validate_source_prelude_inputs(&environment, &BTreeMap::new()).unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let mut answers = Vec::new();
    for order in [["first", "second"], ["second", "first"]] {
        let captured = snapshot(&root, &order);
        let selected = format!("{}/child.mk", order[0]);
        let output = Command::new(if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        })
        .current_dir(&root)
        .env_clear()
        .envs(&environment)
        .args(["-r", "-n", "-s", "--no-print-directory", "-I-"])
        .arg(format!("-I./{}", order[0]))
        .arg(format!("-I./{}", order[1]))
        .arg("-f")
        .arg(&observer)
        .arg("-o")
        .arg(&observer)
        .args(["-f", "root.mk", "--old-file=root.mk"])
        .arg("--old-file=nested/parent.mk")
        .arg(format!("--old-file={selected}"))
        .arg("probe")
        .output()
        .unwrap();
        assert!(
            output.status.success(),
            "synthetic native include query failed"
        );
        let stdout = std::str::from_utf8(&output.stdout).unwrap();
        let search = stdout
            .lines()
            .find_map(|line| line.strip_prefix("SEARCH="))
            .unwrap();
        assert_eq!(search.split_whitespace().collect::<Vec<_>>(), order);
        let namespace = ValidatedMakeNamespace::from_make_stdout(&output.stdout).unwrap();
        let values =
            ValidatedMakeInitialValues::from_make_stdout(&output.stdout, &namespace).unwrap();
        let controls =
            ValidatedMakeParserControls::from_make_stdout(&output.stdout, &namespace).unwrap();
        let initial = values
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
            .collect::<Vec<_>>();
        let mut program =
            MakeVariableProgram::new_source_program(&paths, &namespace, &initial).unwrap();
        let (events, reads) = program
            .read_captured_file("root.mk", &paths, &captured)
            .unwrap();
        let answer = program.evaluate("$(CAPTURE)", &paths).unwrap();
        let native = stdout
            .lines()
            .find_map(|line| line.strip_prefix("OBSERVED="))
            .unwrap();
        assert_eq!(answer, native);
        assert!(answer.ends_with(&format!("root.mk nested/parent.mk {selected}")));
        assert!(events.events.iter().any(|event| matches!(&event.kind, MakeFileEventKind::Include { requested, resolved_identity: Some(identity), .. } if requested == "child.mk" && identity == &selected)));
        assert!(events.events.iter().any(|event| event.input == selected));
        assert_eq!(reads.reads()[2].requested, "child.mk");
        assert_eq!(reads.reads()[2].identity, selected);
        assert_eq!(
            reads.old_file_arguments(),
            [
                "--old-file=root.mk".to_owned(),
                "--old-file=nested/parent.mk".to_owned(),
                format!("--old-file={selected}")
            ]
        );
        answers.push(answer);
    }
    assert_ne!(
        answers[0], answers[1],
        "search-order counter must change the answer"
    );
}

#[test]
fn captured_source_refusal_never_publishes_partial_variables_or_events() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("good.mk"), "VALUE := original\n").unwrap();
    std::fs::write(
        root.join("bad.mk"),
        "VALUE := changed\ninclude missing.mk\n",
    )
    .unwrap();
    let captured = snapshot(&root, &[]);
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .into_iter()
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .collect(),
    })
    .unwrap();
    let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
    program
        .read_captured_file("good.mk", &paths, &captured)
        .unwrap();
    let before = program.events().to_vec();
    assert!(program
        .read_captured_file("bad.mk", &paths, &captured)
        .unwrap_err()
        .contains("required Make include is absent"));
    assert_eq!(program.events(), before);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "original");
    std::fs::write(root.join("good.mk"), "VALUE := tampered\n").unwrap();
    assert!(program
        .read_captured_file("good.mk", &paths, &captured)
        .is_err());
    assert_eq!(program.events(), before);
    assert_eq!(program.evaluate("$(VALUE)", &paths).unwrap(), "original");
}

#[test]
fn captured_file_suffix_refusal_matches_native_regular_file_lookup() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    std::fs::write(root.join("file.mk"), "VALUE := selected\n").unwrap();
    let dirs = DirVars::default();
    let paths = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let namespace = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION"]
            .into_iter()
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: MakeVariableOrigin::Default,
            })
            .collect(),
    })
    .unwrap();
    let captured = snapshot(&root, &[]);
    for (identity, accepted) in [("file.mk", true), ("file.mk/", false), ("file.mk/.", false)] {
        let output = Command::new(if cfg!(target_os = "macos") {
            "gmake"
        } else {
            "make"
        })
        .current_dir(&root)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .args([
            "-r",
            "-n",
            "-s",
            "--no-print-directory",
            "-I-",
            "-f",
            identity,
        ])
        .arg("--eval=probe:")
        .arg("probe")
        .output()
        .unwrap();
        assert_eq!(
            output.status.success(),
            accepted,
            "native file lookup counter for {identity}"
        );
        let mut program = MakeVariableProgram::new_source_program(&paths, &namespace, &[]).unwrap();
        assert_eq!(
            program
                .read_captured_file(identity, &paths, &captured)
                .is_ok(),
            accepted
        );
        if !accepted {
            assert!(program.events().is_empty());
        }
    }
}
