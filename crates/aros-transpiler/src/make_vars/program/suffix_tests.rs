use super::{MakeFileEvent, MakeFileEventKind, MakeProgramPaths, MakeVariableProgram};
use crate::{dirs::DirVars, make_rule_header::RuleHeader, parser::TargetContext};
use std::path::Path;

fn paths(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new("unit"),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build"),
    }
}

fn new_program() -> MakeVariableProgram {
    MakeVariableProgram::new(&TargetContext::default(), &[])
        .expect("synthetic initial Make state is valid")
}

fn suffix_events(file: &super::MakeFileProgram) -> Vec<&MakeFileEvent> {
    file.events
        .iter()
        .filter(|event| matches!(&event.kind, MakeFileEventKind::Suffixes(_)))
        .collect()
}

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn suffix_names_are_ordered_read_time_data_with_provenance() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    let source = concat!(
        "SELECT := before\n",
        ".SUFFIXES: .first $(SELECT) .first\n",
        "SELECT := after\n",
        ".SUFFIXES: $(SELECT)\n",
        "include child.mk\n",
        "$(eval .SUFFIXES: .generated)\n",
    );
    let include = |requested: &str, from: &str| {
        if (requested, from) == ("child.mk", "main.mk") {
            Ok(Some(".SUFFIXES: .included\n".to_owned()))
        } else {
            Err("unexpected synthetic include".to_owned())
        }
    };

    let file = program
        .read_file("main.mk", source, &paths, &include)
        .expect("synthetic suffix program parses");
    let events = suffix_events(&file);
    let observed = events
        .iter()
        .map(|event| match &event.kind {
            MakeFileEventKind::Suffixes(values) => values.as_slice(),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            &[".first", "before", ".first"][..],
            &["after"][..],
            &[".included"][..],
            &[".generated"][..],
        ]
    );
    assert_eq!(events[0].input, "main.mk");
    assert_eq!(events[0].source_line, 1);
    assert!(!events[0].generated);
    assert_eq!(events[2].input, "child.mk");
    assert_eq!(events[2].source_line, 0);
    assert!(!events[2].generated);
    assert_eq!(events[3].input, "main.mk");
    assert_eq!(events[3].source_line, 5);
    assert!(events[3].generated);

    assert_eq!(
        file.suffixes(Some(&names(&[".seed"])))
            .expect("known initial suffixes compose"),
        Some(names(&[
            ".seed",
            ".first",
            "before",
            ".first",
            "after",
            ".included",
            ".generated",
        ]))
    );
}

#[test]
fn empty_suffix_marker_resets_known_or_unknown_state() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();

    let before_reset = program
        .read_file(
            "before-reset.mk",
            ".SUFFIXES: .unknown\n",
            &paths,
            &|_, _| Err("unexpected include".into()),
        )
        .expect("suffix marker parses");
    assert_eq!(before_reset.suffixes(None).unwrap(), None);

    let reset_and_append = program
        .read_file(
            "reset.mk",
            ".SUFFIXES:\n.SUFFIXES: .after .after\n",
            &paths,
            &|_, _| Err("unexpected include".into()),
        )
        .expect("empty suffix marker and following append parse");
    assert_eq!(
        reset_and_append.suffixes(None).unwrap(),
        Some(names(&[".after", ".after"]))
    );
    assert_eq!(
        reset_and_append
            .suffixes(Some(&names(&[".initial", ".initial"])))
            .unwrap(),
        Some(names(&[".after", ".after"]))
    );

    let reset_only = program
        .read_file("reset-only.mk", ".SUFFIXES:\n", &paths, &|_, _| {
            Err("unexpected include".into())
        })
        .expect("empty suffix marker parses");
    assert_eq!(reset_only.suffixes(None).unwrap(), Some(Vec::new()));
    assert_eq!(
        reset_only.suffixes(Some(&names(&[".initial"]))).unwrap(),
        Some(Vec::new())
    );

    let no_marker = program
        .read_file(
            "no-marker.mk",
            "ordinary: prerequisite\n",
            &paths,
            &|_, _| Err("unexpected include".into()),
        )
        .expect("ordinary rule parses");
    assert_eq!(no_marker.suffixes(None).unwrap(), None);
    assert_eq!(
        no_marker.suffixes(Some(&names(&[".initial"]))).unwrap(),
        Some(names(&[".initial"]))
    );
}

#[test]
fn suffix_words_are_literal_data_not_special_target_operations() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    let file = program
        .read_file(
            "literal.mk",
            ".SUFFIXES:\n.SUFFIXES: .PHONY .SUFFIXES literal%\n",
            &paths,
            &|_, _| Err("unexpected include".into()),
        )
        .unwrap();
    assert_eq!(
        file.suffixes(None).unwrap(),
        Some(names(&[".PHONY", ".SUFFIXES", "literal%"]))
    );
    for malformed in ["", "two words", "line\nbreak", "\0"] {
        assert!(file.suffixes(Some(&names(&[malformed]))).is_err());
    }
    let oversized = vec!["x".repeat(4 * 1024 * 1024 + 1)];
    assert!(file.suffixes(Some(&oversized)).is_err());
}

#[test]
fn suffix_state_and_search_order_match_native_gnu_make() {
    use std::{fs, process::Command};
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let dirs = DirVars::default();
    let context = MakeProgramPaths {
        dirs: &dirs,
        source_root: &root,
        relative_dir: Path::new(""),
        top_directory: &root,
        working_directory: &root,
    };
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let query = |file: &str, database: bool, goal: Option<&str>| {
        let mut command = Command::new(make);
        command
            .current_dir(&root)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("LC_ALL", "C")
            .args(["-r", "-n", "-s", "--no-print-directory", "-f", file])
            .arg(format!("--old-file={file}"));
        if database {
            command.arg("-p");
        }
        if let Some(goal) = goal {
            command.arg(goal);
        }
        command.output().unwrap()
    };
    let declarations = ".SUFFIXES: before\nEMPTY :=\n.SUFFIXES: $(EMPTY)\n.SUFFIXES: .src .alt .src\n.SUFFIXES: .out .PHONY\nordinary-default:\n\t@echo DEFAULT\n";
    fs::write(root.join("state.mk"), declarations).unwrap();
    let mut program = new_program();
    let file = program
        .read_file("state.mk", declarations, &context, &|_, _| {
            Err("unexpected include".into())
        })
        .unwrap();
    let result = query("state.mk", true, Some("ordinary-default"));
    assert!(
        result.status.success(),
        "synthetic suffix database query failed"
    );
    let stdout = std::str::from_utf8(&result.stdout).unwrap();
    let selected = stdout
        .lines()
        .filter_map(|line| line.strip_prefix(".SUFFIXES:"))
        .collect::<Vec<_>>();
    assert_eq!(
        selected.len(),
        1,
        "native suffix declaration must be unique"
    );
    assert_eq!(
        file.suffixes(None).unwrap().unwrap(),
        selected[0]
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    );
    let default = query("state.mk", false, None);
    assert!(default.status.success());
    assert_eq!(
        std::str::from_utf8(&default.stdout).unwrap().trim(),
        "echo DEFAULT"
    );
    fs::write(root.join("sample.src"), "synthetic source\n").unwrap();
    fs::write(root.join("sample.alt"), "synthetic alternative\n").unwrap();
    let mut observations = Vec::new();
    for (index, order, expected) in [
        (0, ".src .alt .out", "echo SRC"),
        (1, ".alt .src .out", "echo ALT"),
    ] {
        let file = format!("search-{index}.mk");
        fs::write(
            root.join(&file),
            format!(
                ".SUFFIXES:\n.SUFFIXES: {order}\n.src.out:\n\t@echo SRC\n.alt.out:\n\t@echo ALT\n"
            ),
        )
        .unwrap();
        let output = query(&file, false, Some("sample.out"));
        assert!(
            output.status.success(),
            "synthetic suffix search query failed"
        );
        let observed = std::str::from_utf8(&output.stdout)
            .unwrap()
            .trim()
            .to_owned();
        assert_eq!(observed, expected);
        observations.push(observed);
    }
    assert_ne!(
        observations[0], observations[1],
        "suffix-order counter must change native selection"
    );
    assert!(
        !root.join("sample.out").exists(),
        "dry run must not build a target"
    );
}

#[test]
fn suffix_markers_are_not_rules_or_default_goals() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    let file = program
        .read_file(
            "default.mk",
            ".SUFFIXES: .c\nordinary-default: prerequisite\n",
            &paths,
            &|_, _| Err("unexpected include".into()),
        )
        .expect("suffix marker and ordinary rule parse");

    assert!(!file.events.iter().any(|event| matches!(
        &event.kind,
        MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) if targets == ".SUFFIXES"
    )));
    let first_rule = file.events.iter().find_map(|event| match &event.kind {
        MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) => Some(targets.as_str()),
        _ => None,
    });
    assert_eq!(first_rule, Some("ordinary-default"));
}

#[test]
fn inactive_suffix_controls_do_not_reset_or_expand_unknown_values() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    let file = program
        .read_file(
            "conditional.mk",
            concat!(
                ".SUFFIXES: .before\nSELECT := active\n",
                "ifeq ($(SELECT),inactive)\n.SUFFIXES:\n.SUFFIXES: $(UNMEASURED)\n",
                "else\n.SUFFIXES: .selected\nendif\n",
            ),
            &paths,
            &|_, _| Err("unexpected include".into()),
        )
        .unwrap();
    assert_eq!(suffix_events(&file).len(), 2);
    assert_eq!(
        file.suffixes(Some(&names(&[".initial"]))).unwrap(),
        Some(names(&[".initial", ".before", ".selected"]))
    );
    assert_eq!(file.suffixes(None).unwrap(), None);
}

#[test]
fn non_ascii_separator_spellings_refuse_instead_of_changing_suffix_words() {
    let dirs = DirVars::default();
    let context = paths(&dirs);
    for source in [
        ".SUFFIXES: .a\u{a0}.b\n",
        ".SUFFIXES: \u{a0}.a\n",
        ".SUFFIXES: .a\u{a0}\n",
        "\u{a0}.SUFFIXES: .a\n",
        "VALUE := .a\u{a0}.b\n.SUFFIXES: $(VALUE)\n",
    ] {
        let mut program = new_program();
        let error = program
            .read_file("unicode.mk", source, &context, &|_, _| {
                Err("unexpected include".into())
            })
            .unwrap_err();
        assert!(error.to_string().contains("non-ASCII whitespace"));
    }
    let mut program = new_program();
    let file = program
        .read_file(
            "comment.mk",
            ".SUFFIXES: .a\t.b # harmless comment \u{a0}\n",
            &context,
            &|_, _| Err("unexpected include".into()),
        )
        .unwrap();
    assert_eq!(
        file.suffixes(Some(&[])).unwrap(),
        Some(names(&[".a", ".b"]))
    );
}

#[test]
fn unsupported_suffix_marker_forms_and_control_targets_refuse() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let cases = [
        ".SUFFIXES: names : %.object\n",     // static-pattern marker
        ".SUFFIXES:: names\n",               // double-colon marker
        ".SUFFIXES: SELECT := value\n",      // target-specific assignment
        ".SUFFIXES: names ; echo ignored\n", // inline recipe
        ".SUFFIXES: names | order-only\n",
        ".SUFFIXES ordinary: .c\n", // mixed ordinary/control targets
        ".PHONY .SUFFIXES: .c\n",   // mixed control targets
        ".SUFFIXES: ordinary\n\t@echo unsupported\n", // following recipe
        ".SECONDARY: name\n",       // unrelated control target remains unmodeled
    ];

    for source in cases {
        let mut program = new_program();
        assert!(
            program
                .read_file("invalid.mk", source, &paths, &|_, _| Err(
                    "unexpected include".into()
                ))
                .is_err(),
            "unsupported synthetic suffix/control form was accepted: {source:?}"
        );
    }
}

#[test]
fn suffix_marker_clears_pending_recipe_and_failed_read_rolls_back() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    program.execute("BASE := prior", 1, &paths).unwrap();
    let prior_events = program.events().to_vec();

    let source = concat!(
        "TRANSIENT := unpublished\n",
        "prior-rule: prerequisite\n",
        "\tthis recipe is valid before the marker\n",
        ".SUFFIXES: .c\n",
        "\tthis recipe must not attach to prior-rule\n",
    );
    assert!(program
        .read_file("rollback.mk", source, &paths, &|_, _| Err(
            "unexpected include".into()
        ))
        .is_err());
    assert_eq!(program.events(), prior_events);
    assert_eq!(program.evaluate("$(BASE)", &paths).unwrap(), "prior");
    assert!(program.evaluate("$(TRANSIENT)", &paths).is_err());
}

#[test]
fn suffix_marker_name_count_accepts_the_limit_and_refuses_one_more() {
    const NAME_LIMIT: usize = 65_536;

    let names = (0..NAME_LIMIT)
        .map(|index| format!("s{index}"))
        .collect::<Vec<_>>();
    let source = format!(".SUFFIXES: {}\n", names.join(" "));
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    let file = program
        .read_file("at-limit.mk", &source, &paths, &|_, _| {
            Err("unexpected include".into())
        })
        .expect("name count at the bound is accepted");
    let events = suffix_events(&file);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0].kind,
        MakeFileEventKind::Suffixes(values)
            if values.len() == NAME_LIMIT
                && values.first().map(String::as_str) == Some("s0")
                && values.last().map(String::as_str) == Some("s65535")
    ));
    assert_eq!(file.suffixes(Some(&[])).unwrap().unwrap().len(), NAME_LIMIT);
    assert!(
        file.suffixes(Some(&["initial".to_owned()])).is_err(),
        "the aggregate result must obey its own name bound"
    );

    let too_many_names = names
        .iter()
        .cloned()
        .chain(std::iter::once("extra".to_owned()))
        .collect::<Vec<_>>();
    let too_many = format!(".SUFFIXES: {}\n", too_many_names.join(" "));
    let mut program = new_program();
    assert!(program
        .read_file("over-limit.mk", &too_many, &paths, &|_, _| Err(
            "unexpected include".into()
        ))
        .is_err());
}
