use super::{MakeFileEventKind, MakeProgramPaths, MakeVariableProgram};
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

fn no_include(_: &str, from: &str) -> Result<Option<String>, String> {
    if from.is_empty() {
        return Err("include needs explicit source identity".into());
    }
    Ok(None)
}

#[test]
fn shellflags_assignment_is_a_variable_not_a_special_target() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut selected = program(&[]);
    selected
        .read_file(
            "main.mk",
            ".SHELLFLAGS := -ec\nprobe:\n\tprintf ok\n",
            &paths,
            &no_include,
        )
        .unwrap();
    assert_eq!(selected.evaluate("$(.SHELLFLAGS)", &paths).unwrap(), "-ec");
    for source in [
        ".ONESHELL:\n",
        ".SHELLFLAGS: prerequisite\n",
        ".RECIPEPREFIX := >\n",
    ] {
        let mut refused = program(&[]);
        assert!(refused
            .read_file("main.mk", source, &paths, &no_include)
            .is_err());
    }
}

#[test]
fn late_unsupported_statement_rolls_back_the_entire_file_read() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    program.execute("BASE := prior", 12, &paths).unwrap();
    let prior_events = program.events().to_vec();

    assert!(program
        .read_file(
            "main.mk",
            "NEW := transient\nnot a supported Make statement\n",
            &paths,
            &no_include,
        )
        .is_err());

    assert_eq!(program.events().to_vec(), prior_events);
    assert_eq!(program.evaluate("$(BASE)", &paths).unwrap(), "prior");
    assert!(program.evaluate("$(NEW)", &paths).is_err());
}

#[test]
fn nested_eval_foreach_updates_conditionals_and_freezes_rule_headers() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["ACC"]);
    let source = concat!(
        "$(foreach item,alpha beta,$(eval ACC += $(item)))\n",
        "ifeq ($(ACC),none)\n",
        "SELECT := wrong\n",
        "else ifeq ($(ACC),alpha beta)\n",
        "SELECT := chosen\n",
        "else\n",
        "SELECT := empty\n",
        "endif\n",
        "$(SELECT): $(ACC)\n",
        "\techo $$(SELECT) $$@\n",
        "SELECT := later\n",
        "ACC := changed\n",
    );

    let file = program
        .read_file("main.mk", source, &paths, &no_include)
        .unwrap();

    assert_eq!(program.evaluate("$(ACC)", &paths).unwrap(), "changed");
    assert_eq!(program.evaluate("$(SELECT)", &paths).unwrap(), "later");
    let rule_index = file
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                MakeFileEventKind::Rule(RuleHeader::Rule {
                    targets,
                    prerequisites,
                    ..
                }) if targets == "chosen" && prerequisites == "alpha beta"
            )
        })
        .expect("conditional selected the generated rule header");
    assert!(
        !file.events[rule_index].generated,
        "the rule is original source, not eval output"
    );
    assert!(matches!(
        &file.events[rule_index + 1].kind,
        MakeFileEventKind::Recipe { rule_event, text }
            if *rule_event == rule_index && text == "echo $$(SELECT) $$@"
    ));
    assert_eq!(
        file.events
            .iter()
            .filter(|event| {
                event.generated && matches!(&event.kind, MakeFileEventKind::Variable { .. })
            })
            .count(),
        2
    );
}

#[test]
fn recursive_define_call_emits_state_rule_and_inert_escaped_recipe() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let source = concat!(
        "define emit\n",
        "GENERATED := $(1)\n",
        "$(1): $$(GENERATED).src\n",
        "\tprintf '$$$$@ $$(GENERATED)'\n",
        "endef\n",
        "$(eval $(call emit,first))\n",
        "GENERATED := later\n",
        "undefine GENERATED\n",
    );

    let file = program
        .read_file("main.mk", source, &paths, &no_include)
        .unwrap();

    assert_eq!(program.evaluate("$(GENERATED)", &paths).unwrap(), "");
    let rule_index = file
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                MakeFileEventKind::Rule(RuleHeader::Rule {
                    targets,
                    prerequisites,
                    ..
                }) if targets == "first" && prerequisites == "first.src"
            )
        })
        .expect("eval output contains a rule using its preceding assignment");
    assert!(file.events[rule_index].generated);
    assert!(matches!(
        &file.events[rule_index + 1].kind,
        MakeFileEventKind::Recipe { rule_event, text }
            if *rule_event == rule_index && text == "printf '$$@ $(GENERATED)'"
    ));
    assert!(file
        .events
        .iter()
        .any(|event| matches!(&event.kind, MakeFileEventKind::Definition { .. })));
    assert_eq!(program.events().len(), 6);
    assert_eq!(
        program
            .events()
            .iter()
            .filter(|event| event.input_effect)
            .count(),
        2
    );
}

#[test]
fn includes_are_resolved_in_source_order_and_proved_absence_is_recorded() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let calls = std::cell::RefCell::new(Vec::new());
    let includes = |name: &str, from: &str| -> Result<Option<String>, String> {
        calls.borrow_mut().push((name.to_owned(), from.to_owned()));
        match (from, name) {
            ("main.mk", "required.mk") => Ok(Some("A := required\ninclude nested.mk\n".into())),
            ("required.mk", "nested.mk") => Ok(Some("B := nested\n".into())),
            ("main.mk", "optional.mk" | "absent.mk") => Ok(None),
            _ => Err(format!("unexpected synthetic include {name} from {from}")),
        }
    };
    let file = program
        .read_file(
            "main.mk",
            "include required.mk\n-include optional.mk\nsinclude absent.mk\n",
            &paths,
            &includes,
        )
        .unwrap();

    assert_eq!(
        calls.into_inner(),
        vec![
            ("required.mk".into(), "main.mk".into()),
            ("nested.mk".into(), "required.mk".into()),
            ("optional.mk".into(), "main.mk".into()),
            ("absent.mk".into(), "main.mk".into()),
        ]
    );
    assert_eq!(program.evaluate("$(A)", &paths).unwrap(), "required");
    assert_eq!(program.evaluate("$(B)", &paths).unwrap(), "nested");
    let observed = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Include {
                requested, present, ..
            } => Some((requested.as_str(), *present)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            ("required.mk", true),
            ("nested.mk", true),
            ("optional.mk", false),
            ("absent.mk", false),
        ]
    );
}

#[test]
fn missing_required_include_resolver_error_and_cycle_are_transactional() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for scenario in ["missing", "resolver error", "cycle"] {
        let mut program = program(&[]);
        program.execute("BASE := prior", 1, &paths).unwrap();
        let prior_events = program.events().to_vec();
        let source = match scenario {
            "missing" => "PRE := attempted\ninclude missing.mk\n",
            "resolver error" => "PRE := attempted\n-include broken.mk\n",
            "cycle" => "PRE := attempted\ninclude child.mk\n",
            _ => unreachable!(),
        };
        let includes = |name: &str, from: &str| -> Result<Option<String>, String> {
            match (scenario, from, name) {
                ("missing", "main.mk", "missing.mk") => Ok(None),
                ("resolver error", "main.mk", "broken.mk") => {
                    Err("synthetic resolver failure".into())
                }
                ("cycle", "main.mk", "child.mk") => {
                    Ok(Some("CHILD := attempted\ninclude main.mk\n".into()))
                }
                ("cycle", "child.mk", "main.mk") => Ok(Some("REENTERED := attempted\n".into())),
                _ => Err(format!("unexpected synthetic include {name} from {from}")),
            }
        };

        assert!(
            program
                .read_file("main.mk", source, &paths, &includes)
                .is_err(),
            "{scenario} must reject the whole read"
        );
        assert_eq!(
            program.events().to_vec(),
            prior_events,
            "{scenario} changed variable state"
        );
        assert_eq!(program.evaluate("$(BASE)", &paths).unwrap(), "prior");
        assert!(program.evaluate("$(PRE)", &paths).is_err());
        assert!(program.evaluate("$(CHILD)", &paths).is_err());
    }
}

#[test]
fn measured_include_path_expands_but_unresolved_literal_path_refuses() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    program.execute("FILE := selected", 0, &paths).unwrap();
    let calls = std::cell::Cell::new(0);
    let includes = |_: &str, _: &str| -> Result<Option<String>, String> {
        calls.set(calls.get() + 1);
        Ok(Some("INCLUDED := yes\n".into()))
    };

    program
        .read_file("main.mk", "include $(FILE).mk\n", &paths, &includes)
        .unwrap();
    assert_eq!(calls.get(), 1);
    let before = program.events().to_vec();
    for statement in [
        "include $$(FILE).mk\n",
        "include $(UNMEASURED).mk\n",
        "-include *.mk\n",
    ] {
        assert!(program
            .read_file("main.mk", statement, &paths, &includes)
            .is_err());
        assert_eq!(program.events(), before);
    }
    assert_eq!(calls.get(), 1);
    assert_eq!(program.evaluate("$(FILE)", &paths).unwrap(), "selected");
    assert_eq!(program.evaluate("$(INCLUDED)", &paths).unwrap(), "yes");
}

#[test]
fn false_branch_skips_expressions_and_includes_and_keeps_continuation_positions() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let include_calls = std::cell::Cell::new(0);
    let includes = |_: &str, _: &str| -> Result<Option<String>, String> {
        include_calls.set(include_calls.get() + 1);
        Err("inactive include must not be resolved".into())
    };
    let source = concat!(
        "ifeq (yes,no)\n",
        "$(shell unsupported)\n",
        "include forbidden.mk\n",
        "endif\n",
        "ignored: prerequisite\n",
        "continued: first \\\n",
        " second\n",
        "\tprintf ok\n",
    );

    let file = program
        .read_file("main.mk", source, &paths, &includes)
        .unwrap();

    assert_eq!(include_calls.get(), 0);
    assert_eq!(
        file.events
            .iter()
            .filter_map(|event| match &event.kind {
                MakeFileEventKind::Rule(RuleHeader::Rule {
                    targets,
                    prerequisites,
                    ..
                }) => Some((targets.as_str(), prerequisites.as_str(), event.source_line)),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![
            ("ignored", "prerequisite", 4),
            ("continued", "first second", 5)
        ]
    );
    let continued_rule = file
        .events
        .iter()
        .position(|event| {
            matches!(
                &event.kind,
                MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) if targets == "continued"
            )
        })
        .unwrap();
    assert!(matches!(
        &file.events[continued_rule + 1].kind,
        MakeFileEventKind::Recipe { rule_event, text }
            if *rule_event == continued_rule && text == "printf ok"
    ));
    assert_eq!(file.events[continued_rule + 1].source_line, 7);
}

#[test]
fn malformed_conditionals_and_definitions_reject_without_publishing_state() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    program.execute("BASE := stable", 0, &paths).unwrap();
    let prior_events = program.events().to_vec();

    for (index, source) in [
        "else\n",
        "ifeq (a,b)\nCHANGED := value\n",
        "define simple := unsupported\nendef\n",
        "define unterminated\nbody\n",
        "endef\n",
        ".SECONDEXPANSION:\n",
        "BAD := \u{0001}\n$(BAD): input\n",
    ]
    .into_iter()
    .enumerate()
    {
        assert!(program
            .read_file(
                &format!("malformed-{index}.mk"),
                source,
                &paths,
                &no_include
            )
            .is_err());
        assert_eq!(program.events().to_vec(), prior_events);
        assert_eq!(program.evaluate("$(BASE)", &paths).unwrap(), "stable");
        assert!(program.evaluate("$(CHANGED)", &paths).is_err());
    }
}

#[test]
fn proved_empty_include_list_does_not_become_a_missing_named_input() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let calls = std::cell::Cell::new(0);
    let includes = |_: &str, _: &str| -> Result<Option<String>, String> {
        calls.set(calls.get() + 1);
        Err("empty filename list must not reach resolver".into())
    };
    let file = program
        .read_file(
            "main.mk",
            "EMPTY :=\ninclude $(EMPTY)\n-include $(EMPTY)\nsinclude $(EMPTY)\nAFTER := reached\n",
            &paths,
            &includes,
        )
        .unwrap();
    assert_eq!(calls.get(), 0);
    assert_eq!(program.evaluate("$(AFTER)", &paths).unwrap(), "reached");
    assert!(!file
        .events
        .iter()
        .any(|event| matches!(event.kind, MakeFileEventKind::Include { .. })));
    // GNU Make also ends the pending rule at an empty include directive.
    assert!(program
        .read_file(
            "main.mk",
            "all:\ninclude $(EMPTY)\n\trecipe\n",
            &paths,
            &includes
        )
        .is_err());
}

#[test]
fn conditional_effects_are_ordered_and_ineligible_arms_are_lazy() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&["ABSENT"]);
    program
        .read_file(
            "main.mk",
            concat!(
                "EMPTY :=\nRAW = $(EMPTY)\n",
                "ifdef RAW\nRAW_NONEMPTY := yes\nendif\n",
                "ifndef ABSENT\nABSENCE_PROVED := yes\nendif\n",
                "ifeq ($(eval SIDE := first)$(SIDE),first)\nSELECT := yes\n",
                "else ifeq ($(UNMEASURED),bad)\nSELECT := wrong\nendif\n",
                "ifeq (no,yes)\nifdef UNMEASURED\nNEVER := wrong\nendif\nendif\n"
            ),
            &paths,
            &no_include,
        )
        .unwrap();
    for name in ["RAW_NONEMPTY", "ABSENCE_PROVED", "SELECT"] {
        assert_eq!(
            program.evaluate(&format!("$({name})"), &paths).unwrap(),
            "yes"
        );
    }
    assert_eq!(program.evaluate("$(SIDE)", &paths).unwrap(), "first");
    assert!(program.evaluate("$(NEVER)", &paths).is_err());
}

#[test]
fn definition_body_shares_normal_assignment_append_and_undefine_history() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    program
        .read_file(
            "main.mk",
            "define stored\n  left$(1)\nsecond\nendef\nstored += tail\nstored ?= ignored\n",
            &paths,
            &no_include,
        )
        .unwrap();
    assert_eq!(
        program.evaluate("$(call stored,X)", &paths).unwrap(),
        "  leftX\nsecond tail"
    );
    program
        .read_file("main.mk", "stored := replacement\n", &paths, &no_include)
        .unwrap();
    assert_eq!(
        program.evaluate("$(stored)", &paths).unwrap(),
        "replacement"
    );
    program
        .read_file(
            "main.mk",
            "undefine stored\nstored ?= restored\n",
            &paths,
            &no_include,
        )
        .unwrap();
    assert_eq!(program.evaluate("$(stored)", &paths).unwrap(), "restored");
}

#[test]
fn target_variable_immediates_freeze_but_recursive_values_remain_data() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let file = program.read_file("main.mk", "T := object\nFLAG := before\n$(T): SIMPLE := $(FLAG)\n$(T): RECURSIVE = $(FLAG)\nFLAG := after\n", &paths, &no_include).unwrap();
    let values = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Rule(RuleHeader::TargetAssignment {
                targets,
                assignment,
            }) => Some((targets.as_str(), assignment.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        [
            ("object", "SIMPLE := before"),
            ("object", "RECURSIVE = $(FLAG)")
        ]
    );
    assert_eq!(program.evaluate("$(FLAG)", &paths).unwrap(), "after");
}

#[test]
fn include_depth_and_input_line_limits_have_positive_and_counter_cases() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    for (depth, pass) in [(32, true), (33, false)] {
        let mut program = program(&[]);
        let includes = |name: &str, _: &str| -> Result<Option<String>, String> {
            let at = name
                .strip_prefix('f')
                .ok_or("bad synthetic name")?
                .parse::<usize>()
                .map_err(|error| error.to_string())?;
            Ok(Some(if at + 1 == depth {
                "LEAF := proved\n".into()
            } else {
                format!("include f{}\n", at + 1)
            }))
        };
        assert_eq!(
            program
                .read_file("f0", "include f1\n", &paths, &includes)
                .is_ok(),
            pass
        );
        assert_eq!(program.evaluate("$(LEAF)", &paths).is_ok(), pass);
    }
    let mut program = program(&[]);
    program
        .read_file("lines.mk", &"\n".repeat(65_536), &paths, &no_include)
        .unwrap();
    let before = program.events().to_vec();
    assert!(program
        .read_file("lines.mk", &"\n".repeat(65_537), &paths, &no_include)
        .is_err());
    assert_eq!(program.events(), before);
}

#[test]
fn recipe_continuations_preserve_shell_bytes_and_remove_only_recipe_prefix() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    let file = program
        .read_file(
            "main.mk",
            "all:\n\tprintf '%s' 'a\\\n\tb'\n\tprintf a\\\n  b\n",
            &paths,
            &no_include,
        )
        .unwrap();
    let recipes = file
        .events
        .iter()
        .filter_map(|event| match &event.kind {
            MakeFileEventKind::Recipe { text, .. } => Some((text.as_str(), event.source_line)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recipes,
        [("printf '%s' 'a\\\nb'", 1), ("printf a\\\n  b", 3)]
    );
    // Variable-definition continuations still use ordinary Make folding.
    program
        .read_file(
            "main.mk",
            "define folded\na\\\nb\nendef\n",
            &paths,
            &no_include,
        )
        .unwrap();
    assert_eq!(program.evaluate("$(folded)", &paths).unwrap(), "a b");
    let crlf = program
        .read_file(
            "main.mk",
            "all:\r\n\tprintf a\\\r\n\tb\r\n",
            &paths,
            &no_include,
        )
        .unwrap();
    assert!(crlf.events.iter().any(|event| matches!(&event.kind, MakeFileEventKind::Recipe { text, .. } if text == "printf a\\\nb")));
    let even = program
        .read_file(
            "main.mk",
            "all:\n\tprintf a\\\\\nAFTER := observed\n",
            &paths,
            &no_include,
        )
        .unwrap();
    assert!(even.events.iter().any(|event| matches!(&event.kind, MakeFileEventKind::Recipe { text, .. } if text == "printf a\\\\")));
    assert_eq!(program.evaluate("$(AFTER)", &paths).unwrap(), "observed");
}

#[test]
fn define_comments_are_value_data_but_endef_accepts_a_trailing_comment() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = program(&[]);
    program.read_file("main.mk", "define X\n# literal body comment\nfirst\nendef # terminator comment\nAFTER := observed\n", &paths, &no_include).unwrap();
    assert_eq!(
        program.evaluate("$(value X)", &paths).unwrap(),
        "# literal body comment\nfirst"
    );
    assert_eq!(program.evaluate("$(AFTER)", &paths).unwrap(), "observed");
    let before = program.events().to_vec();
    let nesting = "define nested\n".repeat(17);
    let error = program
        .read_file("nesting.mk", &nesting, &paths, &no_include)
        .unwrap_err();
    assert!(error.to_string().contains("nesting.mk line 17:"), "{error}");
    assert_eq!(program.events(), before);
}
