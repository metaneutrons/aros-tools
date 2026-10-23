use super::{MakeFileEventKind, MakeProgramPaths, MakeVariableProgram};
use crate::{dirs::DirVars, make_rule_header::RuleHeader, parser::TargetContext};
use std::{path::Path, process::Command};

fn context(dirs: &DirVars) -> MakeProgramPaths<'_> {
    MakeProgramPaths {
        dirs,
        source_root: Path::new("/synthetic/source"),
        relative_dir: Path::new("unit"),
        top_directory: Path::new("/synthetic/build"),
        working_directory: Path::new("/synthetic/build"),
    }
}

fn program() -> MakeVariableProgram {
    MakeVariableProgram::new(&TargetContext::default(), &[]).unwrap()
}

fn native_frame(main: &str, child: &str) -> std::process::Output {
    native_frame_with_prefix(main, child, None)
}

fn native_frame_with_prefix(main: &str, child: &str, prefix: Option<&str>) -> std::process::Output {
    let temporary = tempfile::tempdir().unwrap();
    std::fs::write(temporary.path().join("main.mk"), main).unwrap();
    std::fs::write(temporary.path().join("child.mk"), child).unwrap();
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let mut command = Command::new(make);
    command
        .current_dir(temporary.path())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("LC_ALL", "C")
        .args([
            "-r",
            "-n",
            "-s",
            "--no-print-directory",
            "-f",
            "main.mk",
            "--old-file=main.mk",
            "prior",
        ]);
    if let Some(prefix) = prefix {
        command.arg(format!(".RECIPEPREFIX={prefix}"));
    }
    command.output().unwrap()
}

#[test]
fn empty_rules_skip_all_tail_forms_before_expansion_and_keep_recipe_ownership() {
    let dirs = DirVars::default();
    for header in [
        "$(EMPTY): $(error UNREACHED_PREREQUISITE) missing",
        "$(EMPTY): VALUE := $(error UNREACHED_ASSIGNMENT)",
        "$(EMPTY): %.o: $(error UNREACHED_STATIC_PREREQUISITE)",
        "$(EMPTY):: $(error UNREACHED_DOUBLE_PREREQUISITE)",
        ": $(error UNREACHED_LITERAL_TAIL)",
        "$(EMPTY): | $(error UNREACHED_ORDER_ONLY) ; $(error UNREACHED_INLINE)",
    ] {
        let source = format!(
            "EMPTY :=\nprior:\n\tprior recipe\n{header}\n\t$(error UNREACHED_RECIPE)\nVALUE := kept\n\t$(error STILL_IGNORED)\nlater:\n\tlater recipe\n"
        );
        let file = program()
            .read_file("main.mk", &source, &context(&dirs), &|_, _| {
                Err("unexpected include".into())
            })
            .unwrap();
        assert_eq!(
            file.events
                .iter()
                .filter(|e| matches!(e.kind, MakeFileEventKind::IgnoredEmptyRule))
                .count(),
            1
        );
        let rules = file
            .events
            .iter()
            .enumerate()
            .filter_map(|(at, e)| match &e.kind {
                MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) => {
                    Some((at, targets.as_str()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rules.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
            ["prior", "later"]
        );
        let recipes = file
            .events
            .iter()
            .filter_map(|e| match &e.kind {
                MakeFileEventKind::Recipe { rule_event, text } => {
                    Some((*rule_event, text.as_str()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            recipes,
            [(rules[0].0, "prior recipe"), (rules[1].0, "later recipe")]
        );
        assert!(
            crate::make_rule_header::parse(": ignored").is_err(),
            "standalone syntax API stays strict"
        );
    }
}

#[test]
fn nonempty_or_unproved_targets_do_not_skip_tail_guards() {
    let dirs = DirVars::default();
    for source in [
        "TARGET := real\n$(TARGET): $(error REQUIRED_TAIL)\n",
        "$(UNMEASURED): $(error REQUIRED_TAIL)\n",
        "TARGET := real\n$(TARGET): %.o: %.c | a | b\n",
        "EMPTY :=\n$(EMPTY) real: $(error REQUIRED_TAIL)\n",
        "TARGET := left\u{a0}right\n$(TARGET): ignored\n",
        "\u{a0}: ignored\n",
    ] {
        assert!(
            program()
                .read_file("counter.mk", source, &context(&dirs), &|_, _| Ok(None))
                .is_err(),
            "accepted synthetic counter: {source:?}"
        );
    }
}

#[test]
fn included_orphan_recipe_cannot_attach_to_a_parent_rule_or_empty_suppression() {
    let dirs = DirVars::default();
    for prefix in ["prior:\n", "EMPTY :=\n$(EMPTY): ignored\n"] {
        let source = format!("{prefix}include child.mk\n");
        let native = native_frame(&source, "\t$(info ORPHAN)\n");
        assert_eq!(native.status.code(), Some(2));
        assert!(native.stdout.is_empty());
        assert!(String::from_utf8_lossy(&native.stderr)
            .contains("recipe commences before first target"));
        assert!(program()
            .read_file("main.mk", &source, &context(&dirs), &|_, _| {
                Ok(Some("\torphan recipe\n".into()))
            })
            .unwrap_err()
            .to_string()
            .contains("no preceding rule"));
    }
}

#[test]
fn include_and_eval_boundaries_flush_both_rule_and_empty_recipe_states() {
    let dirs = DirVars::default();
    for (source, child) in [
        (
            "EMPTY :=\ninclude child.mk\n\torphan\n",
            "$(EMPTY): ignored\n",
        ),
        (
            "EMPTY :=\n$(EMPTY): ignored\ninclude child.mk\n\torphan\n",
            "child-target:\n",
        ),
        (
            "EMPTY :=\nprior:\n$(eval $(EMPTY): ignored)\n\torphan\n",
            "",
        ),
        ("EMPTY :=\n$(eval $(EMPTY): ignored)\n\torphan\n", ""),
    ] {
        let native = native_frame(source, child);
        assert_eq!(native.status.code(), Some(2));
        assert!(native.stdout.is_empty());
        assert!(String::from_utf8_lossy(&native.stderr)
            .contains("recipe commences before first target"));
        assert!(program()
            .read_file("main.mk", source, &context(&dirs), &|_, _| {
                Ok(Some(child.into()))
            })
            .unwrap_err()
            .to_string()
            .contains("no preceding rule"));
    }
}

#[test]
fn target_prefix_expands_once_and_inactive_rule_is_not_evaluated() {
    let dirs = DirVars::default();
    let paths = context(&dirs);
    let mut program = program();
    let file = program
        .read_file(
            "main.mk",
            concat!(
                "COUNT :=\nTARGET = $(eval COUNT += x)real\n",
                "$(TARGET): prerequisite\n",
                "ifeq (no,yes)\n$(UNMEASURED): $(error UNREACHED)\nendif\n",
            ),
            &paths,
            &|_, _| Ok(None),
        )
        .unwrap();
    assert_eq!(program.evaluate("$(COUNT)", &paths).unwrap(), "x");
    assert_eq!(
        file.events
            .iter()
            .filter(|e| matches!(&e.kind,
        MakeFileEventKind::Rule(RuleHeader::Rule { targets, .. }) if targets == "real"))
            .count(),
        1
    );
}

#[test]
fn a_later_failure_does_not_publish_empty_rule_side_effects() {
    let dirs = DirVars::default();
    let paths = context(&dirs);
    let mut program = program();
    program.execute("COUNT := prior", 1, &paths).unwrap();
    let before = program.events().to_vec();
    assert!(program
        .read_file(
            "main.mk",
            concat!(
                "EMPTY = $(eval COUNT += transient)\n",
                "$(EMPTY): $(error UNREACHED)\n\t$(error UNREACHED_RECIPE)\n",
                "real: $(error REQUIRED_FAILURE)\n",
            ),
            &paths,
            &|_, _| Ok(None)
        )
        .is_err());
    assert_eq!(program.events(), before);
    assert_eq!(program.evaluate("$(COUNT)", &paths).unwrap(), "prior");
}

#[test]
fn changed_recipe_prefix_cannot_turn_ignored_recipes_into_skipped_statements() {
    let dirs = DirVars::default();
    for declaration in [
        ".RECIPEPREFIX = >\n",
        "export .RECIPEPREFIX := >\n",
        "override .RECIPEPREFIX = >\n",
        "$(eval .RECIPEPREFIX = >)\n",
        "define .RECIPEPREFIX\n>\nendef\n",
    ] {
        let source = format!("{declaration}EMPTY :=\n$(EMPTY): ignored\n\tmalformed\n");
        assert!(program()
            .read_file("main.mk", &source, &context(&dirs), &|_, _| Ok(None))
            .is_err());
        let native = native_frame(&source, "");
        assert_eq!(native.status.code(), Some(2));
        assert!(native.stdout.is_empty());
    }
    // Inactive statements do not change parsing policy or evaluate their RHS.
    assert!(program()
        .read_file(
            "main.mk",
            concat!(
                "ifeq (no,yes)\n.RECIPEPREFIX := $(error UNREACHED)\nendif\n",
                "EMPTY :=\n$(EMPTY): ignored\n\t$(error IGNORED)\n",
            ),
            &context(&dirs),
            &|_, _| Ok(None)
        )
        .is_ok());
}

#[test]
fn measured_initial_recipe_prefix_must_be_bound_and_default() {
    use super::{MakeInitialFlavor, MakeInitialValue};
    use aros_common::make_namespace::{
        MakeNamespaceEntry, MakeNamespaceObservation, MakeVariableOrigin, ValidatedMakeNamespace,
    };
    let dirs = DirVars::default();
    let paths = context(&dirs);
    let ns = ValidatedMakeNamespace::validate(MakeNamespaceObservation {
        version: 1,
        entries: [".VARIABLES", "MAKE_VERSION", ".RECIPEPREFIX"]
            .map(|name| MakeNamespaceEntry {
                name: name.into(),
                origin: if name == ".RECIPEPREFIX" {
                    MakeVariableOrigin::CommandLine
                } else {
                    MakeVariableOrigin::Default
                },
            })
            .into(),
    })
    .unwrap();
    let source = "EMPTY :=\n$(EMPTY): ignored\n\tmalformed\nprior:\n";
    for (value, flavor, success) in [
        ("", MakeInitialFlavor::Simple, true),
        ("", MakeInitialFlavor::Recursive, true),
        (">", MakeInitialFlavor::Simple, false),
        ("$(EMPTY)", MakeInitialFlavor::Recursive, false),
    ] {
        let initial = [MakeInitialValue {
            name: ".RECIPEPREFIX",
            value,
            flavor,
            origin: MakeVariableOrigin::CommandLine,
        }];
        let mut replay = MakeVariableProgram::new_source_program(&paths, &ns, &initial).unwrap();
        assert_eq!(
            replay
                .read_file("main.mk", source, &paths, &|_, _| Ok(None))
                .is_ok(),
            success
        );
        if value != "$(EMPTY)" {
            let native = native_frame_with_prefix(source, "", Some(value));
            assert_eq!(native.status.success(), success);
            assert!(native.stdout.is_empty());
        }
    }
    let mut missing = MakeVariableProgram::new_source_program(&paths, &ns, &[]).unwrap();
    assert!(missing
        .read_file("main.mk", source, &paths, &|_, _| Ok(None))
        .unwrap_err()
        .to_string()
        .contains("proved default .RECIPEPREFIX"));
    let mut target = TargetContext::default();
    target
        .make_variables
        .insert(".RECIPEPREFIX".into(), ">".into());
    let mut legacy = MakeVariableProgram::new(&target, &[]).unwrap();
    assert!(legacy
        .read_file("main.mk", source, &paths, &|_, _| Ok(None))
        .is_err());
}

#[test]
fn native_empty_target_tail_and_recipe_counters_match_source_reader() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    for (case, header, success) in [
        (
            "ordinary",
            "$(EMPTY): $(error UNREACHED_PREREQUISITE)",
            true,
        ),
        (
            "assignment",
            "$(EMPTY): VALUE := $(error UNREACHED_ASSIGNMENT)",
            true,
        ),
        (
            "static",
            "$(EMPTY): %.o: $(error UNREACHED_STATIC_PREREQUISITE)",
            true,
        ),
        (
            "double",
            "$(EMPTY):: $(error UNREACHED_DOUBLE_PREREQUISITE)",
            true,
        ),
        ("literal", ": $(error UNREACHED_LITERAL)", true),
        ("nonempty", "real: $(error REQUIRED_COUNTER)", false),
    ] {
        let source =
            format!("EMPTY :=\nprior:\n\t$(info PRIOR)\n{header}\n\t$(error UNREACHED_RECIPE)\n");
        let file = format!("{case}.mk");
        std::fs::write(root.join(&file), &source).unwrap();
        let observed = Command::new(make)
            .current_dir(root)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap())
            .env("LC_ALL", "C")
            .args(["-r", "-n", "-s", "--no-print-directory", "-f", &file])
            .arg(format!("--old-file={file}"))
            .arg("prior")
            .output()
            .unwrap();
        assert_eq!(observed.status.success(), success, "native case {case}");
        let dirs = DirVars::default();
        assert_eq!(
            program()
                .read_file("main.mk", &source, &context(&dirs), &|_, _| Ok(None))
                .is_ok(),
            success
        );
        if success {
            assert_eq!(observed.stdout, b"PRIOR\n");
        } else {
            assert!(String::from_utf8_lossy(&observed.stderr).contains("REQUIRED_COUNTER"));
        }
    }
}
