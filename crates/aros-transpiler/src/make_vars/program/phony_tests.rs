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

fn phony_events(file: &super::MakeFileProgram) -> Vec<&MakeFileEvent> {
    file.events
        .iter()
        .filter(|event| matches!(&event.kind, MakeFileEventKind::PhonyTargets(_)))
        .collect()
}

#[test]
fn phony_names_are_ordered_data_and_keep_literal_percent_duplicates_and_empty_markers() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    let source = concat!(
        ".PHONY: default-candidate\n",
        "default-candidate: prerequisite\n",
        "SELECT := before\n",
        ".PHONY: $(SELECT)\n",
        "SELECT := after\n",
        ".PHONY: $(SELECT)\n",
        ".PHONY: duplicate literal% duplicate\n",
        ".PHONY:\n",
        "ifeq ($(SELECT),after)\n",
        ".PHONY: selected\n",
        "else\n",
        ".PHONY: skipped\n",
        "endif\n",
        "include child.mk\n",
        "$(eval .PHONY: generated)\n",
    );
    let include = |requested: &str, from: &str| {
        if (requested, from) == ("child.mk", "main.mk") {
            Ok(Some(".PHONY: included\n".to_owned()))
        } else {
            Err("unexpected synthetic include".to_owned())
        }
    };

    let file = program
        .read_file("main.mk", source, &paths, &include)
        .expect("bounded synthetic phony program parses");
    let events = phony_events(&file);
    let observed = events
        .iter()
        .map(|event| match &event.kind {
            MakeFileEventKind::PhonyTargets(names) => names.as_slice(),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            &["default-candidate"][..],
            &["before"][..],
            &["after"][..],
            &["duplicate", "literal%", "duplicate"][..],
            &[][..],
            &["selected"][..],
            &["included"][..],
            &["generated"][..],
        ]
    );
    assert_eq!(events[1].input, "main.mk");
    assert_eq!(events[1].source_line, 3);
    assert!(!events[1].generated);
    assert_eq!(events[6].input, "child.mk");
    assert_eq!(events[6].source_line, 0);
    assert!(!events[6].generated);
    assert_eq!(events[7].input, "main.mk");
    assert_eq!(events[7].source_line, 14);
    assert!(events[7].generated);

    // The special marker is retained as data, never an ordinary rule/default
    // goal. It does not alter any variable assignments either.
    assert!(!file.events.iter().any(|event| matches!(
        &event.kind,
        MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) if targets == ".PHONY"
    )));
    assert!(!file.events.iter().any(|event| matches!(
        &event.kind,
        MakeFileEventKind::Rule(RuleHeader::TargetAssignment { targets, .. }) if targets == ".PHONY"
    )));
    let first_rule = file.events.iter().find_map(|event| match &event.kind {
        MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) => Some(targets.as_str()),
        _ => None,
    });
    assert_eq!(first_rule, Some("default-candidate"));
}

#[test]
fn unsupported_phony_forms_and_unmodeled_control_targets_refuse() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let cases = [
        ".PHONY: names : %.object\n",     // static-pattern marker
        ".PHONY:: names\n",               // double-colon marker
        ".PHONY: SELECT := value\n",      // target-specific assignment
        ".PHONY: names ; echo ignored\n", // inline recipe
        ".PHONY: names | order-only\n",
        ".PHONY: .SUFFIXES\n",
        ".PHONY: ordinary .SUFFIXES\n",
        ".SECONDARY:\n", // unrelated control-target refusal remains in force
    ];

    for source in cases {
        let mut program = new_program();
        assert!(
            program
                .read_file("invalid.mk", source, &paths, &|_, _| Err(
                    "unexpected include".into()
                ))
                .is_err(),
            "unsupported synthetic phony/control form was accepted"
        );
    }
}

#[test]
fn phony_marker_clears_pending_rule_recipe_and_failed_read_rolls_back() {
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    program.execute("BASE := prior", 1, &paths).unwrap();
    let prior_events = program.events().to_vec();

    let source = concat!(
        "TRANSIENT := unpublished\n",
        "prior-rule: prerequisite\n",
        "\tthis recipe is valid before the marker\n",
        ".PHONY: marker\n",
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
fn phony_name_count_accepts_the_limit_and_refuses_one_more() {
    const NAME_LIMIT: usize = 65_536;

    let names = (0..NAME_LIMIT)
        .map(|index| format!("n{index}"))
        .collect::<Vec<_>>();
    let source = format!(".PHONY: {}\n", names.join(" "));
    let dirs = DirVars::default();
    let paths = paths(&dirs);
    let mut program = new_program();
    let file = program
        .read_file("at-limit.mk", &source, &paths, &|_, _| {
            Err("unexpected include".into())
        })
        .expect("name count at the bound is accepted");
    let events = phony_events(&file);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0].kind,
        MakeFileEventKind::PhonyTargets(values)
            if values.len() == NAME_LIMIT
                && values.first().map(String::as_str) == Some("n0")
                && values.last().map(String::as_str) == Some("n65535")
    ));

    let too_many_names = names
        .iter()
        .cloned()
        .chain(std::iter::once("extra".to_owned()))
        .collect::<Vec<_>>();
    let too_many = format!(".PHONY: {}\n", too_many_names.join(" "));
    let mut program = new_program();
    assert!(program
        .read_file("over-limit.mk", &too_many, &paths, &|_, _| Err(
            "unexpected include".into()
        ))
        .is_err());
}
