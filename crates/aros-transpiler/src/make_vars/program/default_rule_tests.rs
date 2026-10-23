use super::{MakeFileEventKind, MakeFileProgram, MakeProgramPaths, MakeVariableProgram};
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

fn program(undefined: &[&str]) -> MakeVariableProgram {
    MakeVariableProgram::new(&TargetContext::default(), undefined)
        .expect("synthetic initial Make state is valid")
}

fn interpret(source: &str) -> MakeFileProgram {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    program(&[])
        .read_file("main.mk", source, &paths, &|_, _| Ok(None))
        .expect("synthetic Make source parses")
}

fn default_rules(file: &MakeFileProgram) -> Vec<(usize, &RuleHeader)> {
    file.events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match &event.kind {
            MakeFileEventKind::DefaultRule(header) => Some((index, header)),
            _ => None,
        })
        .collect()
}

fn native_make(root: &Path, file: &str, goal: Option<&str>) -> std::process::Output {
    use std::process::Command;

    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let mut command = Command::new(make);
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var("PATH").expect("PATH is available"))
        .env("LC_ALL", "C")
        .args(["-r", "-n", "-s", "--no-print-directory", "-f", file])
        .arg(format!("--old-file={file}"));
    if let Some(goal) = goal {
        command.arg(goal);
    }
    command
        .output()
        .expect("GNU Make is available for the fixture")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("GNU Make output is UTF-8")
}

#[test]
fn single_colon_headers_freeze_prerequisites_but_keep_recipe_text_deferred() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let source = concat!(
        "VERSION := before\n",
        ".DEFAULT: prerequisite-$(VERSION) | order-$(VERSION); ",
        "inline $@ $(UNBOUND) $(VERSION)\n",
        "VERSION := after\n",
        ".DEFAULT: second-$(VERSION)\n",
        "\tfollowing $@ $(UNBOUND) $(VERSION)\n",
    );

    let file = program
        .read_file("main.mk", source, &paths, &|_, _| Ok(None))
        .expect("unknown deferred recipe references are not expanded while reading");
    let rules = default_rules(&file);
    assert_eq!(rules.len(), 2);
    assert_eq!(
        rules[0].1,
        &RuleHeader::Rule {
            targets: ".DEFAULT".into(),
            target_pattern: None,
            prerequisites: "prerequisite-before".into(),
            order_only: "order-before".into(),
            double_colon: false,
            inline_recipe: Some("inline $@ $(UNBOUND) $(VERSION)".into()),
        }
    );
    assert_eq!(
        rules[1].1,
        &RuleHeader::Rule {
            targets: ".DEFAULT".into(),
            target_pattern: None,
            prerequisites: "second-after".into(),
            order_only: String::new(),
            double_colon: false,
            inline_recipe: None,
        }
    );
    assert_eq!(file.events[rules[0].0].input, "main.mk");
    assert_eq!(file.events[rules[0].0].source_line, 1);
    assert_eq!(file.events[rules[1].0].source_line, 3);

    let recipes = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Recipe { rule_event, text } => Some((*rule_event, text.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recipes,
        [(rules[1].0, "following $@ $(UNBOUND) $(VERSION)")]
    );
    assert!(
        !file.events.iter().any(|event| matches!(
            &event.kind,
            MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) if targets == ".DEFAULT"
        )),
        ".DEFAULT has a distinct retained event kind"
    );
    assert_eq!(program.evaluate("$(VERSION)", &paths).unwrap(), "after");
}

#[test]
fn double_colon_default_headers_remain_independent_and_distinct() {
    let source = concat!(
        "VERSION := first\n",
        ".DEFAULT:: first-$(VERSION)\n",
        "\tfirst $@ $(UNBOUND)\n",
        "VERSION := later\n",
        ".DEFAULT:: second-$(VERSION)\n",
        "\tsecond $@ $(UNBOUND)\n",
    );
    let file = interpret(source);
    let rules = default_rules(&file);
    assert_eq!(rules.len(), 2);
    for (index, prerequisites) in [(0, "first-first"), (1, "second-later")] {
        assert!(matches!(
            rules[index].1,
            RuleHeader::Rule {
                targets,
                target_pattern: None,
                prerequisites: actual,
                double_colon: true,
                inline_recipe: None,
                ..
            } if targets == ".DEFAULT" && actual == prerequisites
        ));
    }
    let recipes = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Recipe { rule_event, text } => Some((*rule_event, text.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recipes,
        [
            (rules[0].0, "first $@ $(UNBOUND)"),
            (rules[1].0, "second $@ $(UNBOUND)"),
        ]
    );
}

#[test]
fn header_forms_keep_none_empty_inline_and_bare_reset_distinct() {
    let source = concat!(
        ".DEFAULT: original-prerequisite\n",
        "\toriginal $@\n",
        ".DEFAULT: additional-prerequisite\n",
        ".DEFAULT: | additional-order-only\n",
        ".DEFAULT: ;\n",
        ".DEFAULT:\n",
    );
    let file = interpret(source);
    let rules = default_rules(&file);
    assert_eq!(rules.len(), 5);

    let fields = rules
        .iter()
        .map(|(_, header)| match header {
            RuleHeader::Rule {
                prerequisites,
                order_only,
                double_colon,
                inline_recipe,
                ..
            } => (
                prerequisites.as_str(),
                order_only.as_str(),
                *double_colon,
                inline_recipe.as_deref(),
            ),
            RuleHeader::TargetAssignment { .. } => panic!("default event must be a rule"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        fields,
        [
            ("original-prerequisite", "", false, None),
            ("additional-prerequisite", "", false, None),
            ("", "additional-order-only", false, None),
            ("", "", false, Some("")),
            ("", "", false, None),
        ]
    );
    let recipes = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Recipe { rule_event, text } => Some((*rule_event, text.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(recipes, [(rules[0].0, "original $@")]);
}

#[test]
fn included_and_eval_default_rules_keep_their_event_and_recipe_provenance() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let file = program
        .read_file(
            "main.mk",
            concat!(
                "include child.mk\n",
                "define generated_default\n",
                ".DEFAULT: eval-prerequisite\n",
                "\teval-generated $$$$@ $$(UNBOUND)\n",
                "endef\n",
                "$(eval $(call generated_default))\n",
            ),
            &paths,
            &|requested, from| {
                if (requested, from) == ("child.mk", "main.mk") {
                    Ok(Some(
                        "CHILD := captured\n.DEFAULT: child-$(CHILD)\n\tchild $@ $(UNBOUND)\n"
                            .into(),
                    ))
                } else {
                    Err("unexpected synthetic include".into())
                }
            },
        )
        .expect("included and generated rule data parse");

    let rules = default_rules(&file);
    assert_eq!(rules.len(), 2);
    assert!(matches!(
        rules[0].1,
        RuleHeader::Rule { prerequisites, .. } if prerequisites == "child-captured"
    ));
    assert!(matches!(
        rules[1].1,
        RuleHeader::Rule {
            prerequisites,
            inline_recipe: None,
            ..
        } if prerequisites == "eval-prerequisite"
    ));
    let child = &file.events[rules[0].0];
    assert_eq!(child.input, "child.mk");
    assert_eq!(child.source_line, 1);
    assert!(!child.generated);
    let generated = &file.events[rules[1].0];
    assert_eq!(generated.input, "main.mk");
    assert_eq!(generated.source_line, 5);
    assert!(generated.generated);

    let include = file
        .events
        .iter()
        .find(|event| matches!(&event.kind, MakeFileEventKind::Include { requested, .. } if requested == "child.mk"))
        .expect("include event is retained");
    assert_eq!(include.input, "main.mk");
    assert_eq!(include.source_line, 0);
    let recipes = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Recipe { rule_event, text } => Some((
                *rule_event,
                event.input.as_str(),
                event.source_line,
                event.generated,
                text.as_str(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recipes,
        [
            (rules[0].0, "child.mk", 2, false, "child $@ $(UNBOUND)"),
            (
                rules[1].0,
                "main.mk",
                5,
                true,
                "eval-generated $$@ $(UNBOUND)"
            ),
        ]
    );
}

#[test]
fn expanded_single_default_word_allows_ascii_space_around_the_target() {
    let source = concat!(
        "empty :=\n",
        "space := $(empty) $(empty)\n",
        "special := .DEFAULT\n",
        "$(space)$(special)$(space): dynamic-prerequisite\n",
    );
    let file = interpret(source);
    let rules = default_rules(&file);
    assert_eq!(rules.len(), 1);
    assert!(matches!(
        rules[0].1,
        RuleHeader::Rule { targets, prerequisites, .. }
            if targets == " .DEFAULT " && prerequisites == "dynamic-prerequisite"
    ));
}

#[test]
fn unsupported_default_forms_and_unknown_prerequisites_are_refused() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for source in [
        ".DEFAULT: LABEL = value\n",
        ".DEFAULT: %.o: source.c\n",
        ".DEFAULT other: prerequisite\n",
        ".DEFAULT .PHONY: prerequisite\n",
        ".DEFAULT: first\n.DEFAULT:: second\n",
        ".DEFAULT: $(UNMEASURED)\n",
        ".DEFAULT: known | $(UNMEASURED)\n",
    ] {
        let mut program = program(&[]);
        assert!(
            program
                .read_file("invalid.mk", source, &paths, &|_, _| Ok(None))
                .is_err(),
            "unsupported or unresolved default declaration was accepted: {source:?}"
        );
        assert!(program.events().is_empty());
    }
}

#[test]
fn inactive_default_rules_are_not_expanded_or_retained() {
    let source = concat!(
        "ifeq (no,yes)\n",
        ".DEFAULT: $(error UNREACHED_PREREQUISITE)\n",
        "\t$(error UNREACHED_RECIPE)\n",
        "endif\n",
    );
    let file = interpret(source);
    assert!(default_rules(&file).is_empty());
    assert!(!file
        .events
        .iter()
        .any(|event| matches!(&event.kind, MakeFileEventKind::Recipe { .. })));
}

#[test]
fn mixed_colon_modes_across_include_or_eval_fail_atomically() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for (source, child) in [
        (
            "TRANSIENT := added\n.DEFAULT: first\ninclude child.mk\n",
            Some(".DEFAULT:: second\n"),
        ),
        (
            "TRANSIENT := added\n.DEFAULT: first\n$(eval .DEFAULT:: second)\n",
            None,
        ),
    ] {
        let mut program = program(&[]);
        program.execute("BASE := stable", 0, &paths).unwrap();
        let before = program.events().to_vec();
        let includes = |requested: &str, from: &str| {
            if (requested, from) == ("child.mk", "main.mk") {
                Ok(child.map(str::to_owned))
            } else {
                Ok(None)
            }
        };
        assert!(program
            .read_file("main.mk", source, &paths, &includes)
            .is_err());
        assert_eq!(program.events(), before);
        assert_eq!(program.evaluate("$(BASE)", &paths).unwrap(), "stable");
        assert!(program.evaluate("$(TRANSIENT)", &paths).is_err());

        let retry = program
            .read_file(
                "retry.mk",
                ".DEFAULT:: retry-prerequisite\n",
                &paths,
                &|_, _| Ok(None),
            )
            .expect("failed mode mixing does not leak into the next read");
        assert!(matches!(
            default_rules(&retry).as_slice(),
            [(
                _,
                RuleHeader::Rule {
                    double_colon: true,
                    ..
                }
            )]
        ));
    }
}

#[test]
fn native_make_keeps_default_goal_explicit_and_implicit_precedence_and_default_prereqs_separate() {
    use std::fs;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = concat!(
        ".DEFAULT: fallback-prerequisite\n",
        "\techo fallback-$@\n",
        "ordinary:\n",
        "\techo ordinary\n",
        "explicit: explicit-prerequisite\n",
        "\techo explicit-$@\n",
        "%.out: %.src\n",
        "\techo implicit-$@\n",
        "explicit-prerequisite:\n",
        "\techo explicit-prerequisite\n",
        "fallback-prerequisite:\n",
        "\techo fallback-prerequisite\n",
    );
    fs::write(root.join("rules.mk"), source).unwrap();
    fs::write(root.join("implicit.src"), "synthetic input\n").unwrap();

    // Retained source data identifies .DEFAULT separately; these native runs
    // independently establish GNU Make's goal and recipe selection behavior.
    let file = interpret(source);
    let rules = default_rules(&file);
    assert_eq!(rules.len(), 1);
    assert!(matches!(
        rules[0].1,
        RuleHeader::Rule { prerequisites, .. } if prerequisites == "fallback-prerequisite"
    ));
    let first_ordinary = file.events.iter().find_map(|event| match &event.kind {
        MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) => Some(targets.as_str()),
        _ => None,
    });
    assert_eq!(first_ordinary, Some("ordinary"));

    let default_goal = native_make(&root, "rules.mk", None);
    assert!(default_goal.status.success());
    assert_eq!(stdout(&default_goal), "echo ordinary\n");

    let explicit = native_make(&root, "rules.mk", Some("explicit"));
    assert!(explicit.status.success());
    assert_eq!(
        stdout(&explicit),
        "echo explicit-prerequisite\necho explicit-explicit\n"
    );

    let implicit = native_make(&root, "rules.mk", Some("implicit.out"));
    assert!(implicit.status.success());
    assert_eq!(stdout(&implicit), "echo implicit-implicit.out\n");

    let fallback = native_make(&root, "rules.mk", Some("unknown-target"));
    assert!(fallback.status.success());
    assert_eq!(stdout(&fallback), "echo fallback-unknown-target\n");

    let explicit_default = native_make(&root, "rules.mk", Some(".DEFAULT"));
    assert!(explicit_default.status.success());
    assert_eq!(
        stdout(&explicit_default),
        "echo fallback-prerequisite\necho fallback-.DEFAULT\n"
    );
}

#[test]
fn native_make_distinguishes_prerequisite_only_empty_inline_and_bare_reset() {
    use std::fs;

    let base = ".DEFAULT: retained\n\techo fallback-$@\n";
    let cases = [
        ("prerequisite-only", ".DEFAULT: additional\n"),
        ("order-only", ".DEFAULT: | ordered\n"),
        ("empty-inline", ".DEFAULT: ;\n"),
        ("bare-reset", ".DEFAULT:\n"),
    ];
    for (name, suffix) in cases {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = format!(
            "{base}{suffix}retained:\n\techo retained\nadditional:\n\techo additional\nordered:\n\techo ordered\n"
        );
        fs::write(root.join("reset.mk"), &source).unwrap();

        let file = interpret(&source);
        let rules = default_rules(&file);
        assert_eq!(rules.len(), 2, "{name}");
        assert!(matches!(
            rules[0].1,
            RuleHeader::Rule {
                inline_recipe: None,
                ..
            }
        ));
        match name {
            "prerequisite-only" => assert!(matches!(
                rules[1].1,
                RuleHeader::Rule { prerequisites, inline_recipe: None, .. }
                    if prerequisites == "additional"
            )),
            "order-only" => assert!(matches!(
                rules[1].1,
                RuleHeader::Rule { order_only, inline_recipe: None, .. }
                    if order_only == "ordered"
            )),
            "empty-inline" => assert!(matches!(
                rules[1].1,
                RuleHeader::Rule { inline_recipe: Some(recipe), .. } if recipe.is_empty()
            )),
            "bare-reset" => assert!(matches!(
                rules[1].1,
                RuleHeader::Rule {
                    prerequisites,
                    order_only,
                    inline_recipe: None,
                    ..
                } if prerequisites.is_empty() && order_only.is_empty()
            )),
            _ => unreachable!(),
        }

        let unknown = native_make(&root, "reset.mk", Some("unknown-target"));
        if name == "bare-reset" {
            assert_eq!(unknown.status.code(), Some(2));
            assert!(stdout(&unknown).is_empty());
        } else if name == "empty-inline" {
            assert!(unknown.status.success());
            assert!(stdout(&unknown).is_empty());
        } else {
            assert!(unknown.status.success());
            assert_eq!(stdout(&unknown), "echo fallback-unknown-target\n");
        }

        let explicit_default = native_make(&root, "reset.mk", Some(".DEFAULT"));
        assert!(explicit_default.status.success(), "{name}");
        let expected = match name {
            "prerequisite-only" => "echo retained\necho additional\necho fallback-.DEFAULT\n",
            "order-only" => "echo retained\necho ordered\necho fallback-.DEFAULT\n",
            "empty-inline" | "bare-reset" => "echo retained\n",
            _ => unreachable!(),
        };
        assert_eq!(stdout(&explicit_default), expected, "{name}");
    }
}

#[test]
fn native_double_colon_without_first_recipe_does_not_use_a_later_recipe_for_unknown_targets() {
    use std::fs;

    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = concat!(
        ".DEFAULT:: first-prerequisite\n",
        ".DEFAULT:: second-prerequisite\n",
        "\techo later-$@\n",
        "first-prerequisite:\n",
        "\techo first-prerequisite\n",
        "second-prerequisite:\n",
        "\techo second-prerequisite\n",
    );
    fs::write(root.join("double.mk"), source).unwrap();

    let file = interpret(source);
    let rules = default_rules(&file);
    assert_eq!(rules.len(), 2);
    assert!(rules.iter().all(|(_, header)| matches!(
        header,
        RuleHeader::Rule {
            double_colon: true,
            ..
        }
    )));
    let recipes = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Recipe { rule_event, text }
                if rules.iter().any(|(index, _)| index == rule_event) =>
            {
                Some((*rule_event, text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(recipes, [(rules[1].0, "echo later-$@")]);

    let unknown = native_make(&root, "double.mk", Some("unknown-target"));
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stdout(&unknown).is_empty());
}
