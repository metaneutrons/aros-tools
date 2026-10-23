use super::{parse, project_complete_c};
use crate::dirs::DirVars;
use crate::make_expr::MakeExprContext;
use crate::make_vars::collect_vars_impl;
use crate::parser::{join_continuations, TargetContext};
use std::path::Path;

const OWNER: &str = "mesa3d-linklib-galliumdrawllvm";
const DECLARATION: &str = "%build_linklib mmake=mesa3d-linklib-galliumdrawllvm libname=galliumdrawllvm files=\"a b\" objdir=/obj srcdir=/src usetree=yes";
const BASE: &str = "\
OBJS := /obj/a.o /obj/b.o
DFILES := /obj/a.d /obj/b.d
mesa3d-linklib-galliumdrawllvm_C_FILES := /src/a /src/b
CPPFLAGS_FOR_RULE := -DFIRST=1 -DSECOND=2 -DFIRST=1
$(OBJS) $(DFILES): mesa3d-linklib-galliumdrawllvm_MC_EXTRA_CPPFLAGS:=-DFIRST=1 -DSECOND=2 -DFIRST=1
%build_linklib mmake=mesa3d-linklib-galliumdrawllvm libname=galliumdrawllvm files=\"a b\" objdir=/obj srcdir=/src usetree=yes
";

fn project(source: &str) -> Result<Vec<String>, String> {
    let joined = join_continuations(source);
    let target_context = TargetContext::default();
    let (scope, _) = collect_vars_impl(&joined, Some(&target_context));
    let dirs = DirVars::from_config_text("");
    let lines = joined.lines().collect::<Vec<_>>();
    let rule_line = lines
        .iter()
        .position(|line| line.contains("_MC_EXTRA_CPPFLAGS:"))
        .ok_or_else(|| "fixture has no target-specific rule".to_owned())?;
    let declaration_line = lines
        .iter()
        .position(|line| line.starts_with("%build_linklib "))
        .ok_or_else(|| "fixture has no build_linklib declaration".to_owned())?;
    let rule_text = lines[rule_line];
    let rule = parse(rule_text).ok_or_else(|| "fixture rule did not parse".to_owned())?;
    let declaration_args = lines[declaration_line]
        .strip_prefix("%build_linklib ")
        .ok_or_else(|| "fixture declaration did not parse".to_owned())?;
    let source_root = Path::new("/source");
    let relative_dir = Path::new("fixture");
    let rule_context = MakeExprContext::new(&scope, &dirs, rule_line, source_root, relative_dir);
    let declaration_context =
        MakeExprContext::new(&scope, &dirs, declaration_line, source_root, relative_dir);

    project_complete_c(&rule, &rule_context, &declaration_context, declaration_args)
}

fn replace_once(source: &str, before: &str, after: &str) -> String {
    assert!(
        source.contains(before),
        "fixture did not contain {before:?}"
    );
    source.replacen(before, after, 1)
}

#[test]
fn parser_accepts_only_plain_target_references_and_literal_immediate_assignment() {
    let parsed = parse(
        "$(GALLDRAWLLVMOFILES) $(GALLDRAWLLVMDFILES): mesa3d-linklib-galliumdrawllvm_MC_EXTRA_CPPFLAGS:=-DDRAW_LLVM_AVAILABLE=1",
    )
    .unwrap();
    assert_eq!(parsed.owner, OWNER);
    assert_eq!(
        parsed.targets,
        "$(GALLDRAWLLVMOFILES) $(GALLDRAWLLVMDFILES)"
    );
    assert_eq!(parsed.value, "-DDRAW_LLVM_AVAILABLE=1");

    for line in [
        "$(GALLDRAWLLVMOFILES): owner_MC_EXTRA_CPPFLAGS=-DNO_IMMEDIATE",
        "$(GALLDRAWLLVMOFILES): owner_MC_EXTRA_CPPFLAGS+=-DAPPEND",
        "$(call expand,TARGETS): owner_MC_EXTRA_CPPFLAGS:=-DYNAMIC_TARGET",
        "${GALLDRAWLLVMOFILES}: owner_MC_EXTRA_CPPFLAGS:=-DBRACE_REFERENCE",
        "$(GALLDRAWLLVMOFILES): $(owner)_MC_EXTRA_CPPFLAGS:=-DDYNAMIC_OWNER",
        "$(GALLDRAWLLVMOFILES): owner_MC_CFLAGS:=-DWRONG_SUFFIX",
        "$(A) $(B) $(C) $(D) $(E) $(F) $(G) $(H) $(I): owner_MC_EXTRA_CPPFLAGS:=-DTOO_MANY",
    ] {
        assert!(parse(line).is_none(), "unexpectedly parsed {line:?}");
    }
}

#[test]
fn complete_c_object_and_dependency_set_preserves_option_order_and_duplicates() {
    let options = project(BASE).unwrap();
    assert_eq!(
        options,
        vec![
            "-DFIRST=1".to_owned(),
            "-DSECOND=2".to_owned(),
            "-DFIRST=1".to_owned(),
        ]
    );
}

#[test]
fn partial_extra_and_duplicate_target_sets_are_rejected() {
    let partial = replace_once(BASE, "$(OBJS) $(DFILES)", "$(OBJS)");
    assert!(project(&partial).is_err());

    let extra = replace_once(
        BASE,
        "OBJS := /obj/a.o /obj/b.o",
        "OBJS := /obj/a.o /obj/b.o /obj/c.o",
    );
    let extra = replace_once(&extra, "$(OBJS) $(DFILES)", "$(OBJS) $(DFILES) /obj/c.o");
    assert!(project(&extra).is_err());

    let duplicate = replace_once(BASE, "$(OBJS) $(DFILES)", "$(OBJS) $(OBJS) $(DFILES)");
    assert!(project(&duplicate).is_err());
}

#[test]
fn source_vector_must_match_declaration_files_in_order() {
    let changed_source = replace_once(
        BASE,
        "mesa3d-linklib-galliumdrawllvm_C_FILES := /src/a /src/b",
        "mesa3d-linklib-galliumdrawllvm_C_FILES := /src/b /src/a",
    );
    assert!(project(&changed_source).is_err());

    let missing_source = replace_once(
        BASE,
        "mesa3d-linklib-galliumdrawllvm_C_FILES := /src/a /src/b",
        "OTHER_C_FILES := /src/a /src/b",
    );
    assert!(project(&missing_source).is_err());
}

#[test]
fn unsafe_paths_and_non_c_lanes_are_rejected() {
    for (from, to) in [
        ("objdir=/obj", "objdir=relative/obj"),
        ("objdir=/obj", "objdir=/tmp/../obj"),
        ("srcdir=/src", "srcdir=\"/source with spaces\""),
        ("srcdir=/src", "srcdir=$(UNRESOLVED)"),
        ("files=\"a b\"", "files=\"../a b\""),
        ("files=\"a b\"", "files=\"a.c b\""),
        ("files=\"a b\"", "files=\"a nested/b\""),
    ] {
        let source = replace_once(BASE, from, to);
        assert!(project(&source).is_err(), "accepted {to:?}");
    }

    for (key, value) in [
        ("objcfiles", "a"),
        ("cxxfiles", "a"),
        ("asmfiles", "a"),
        ("objs", "/other/external.o"),
    ] {
        let declaration = format!("{DECLARATION} {key}=\"{value}\"");
        let source = replace_once(BASE, DECLARATION, &declaration);
        assert!(project(&source).is_err(), "accepted non-C {key}={value}");
    }
}

#[test]
fn required_arguments_must_be_unique_and_usetree_must_be_yes() {
    for (from, to) in [
        (" srcdir=/src", ""),
        (" files=\"a b\"", " files=\"a b\" files=\"a b\""),
        (" usetree=yes", " usetree=no"),
    ] {
        let source = replace_once(BASE, from, to);
        assert!(
            project(&source).is_err(),
            "accepted declaration edit {to:?}"
        );
    }
}

#[test]
fn compiler_role_is_absent_or_the_literal_target() {
    let explicit_target = replace_once(BASE, "usetree=yes", "usetree=yes compiler=target");
    assert!(project(&explicit_target).is_ok());

    for role in ["host", "kernel", "$(COMPILER)"] {
        let declaration =
            replace_once(BASE, "usetree=yes", &format!("usetree=yes compiler={role}"));
        assert!(project(&declaration).is_err(), "accepted compiler={role}");
    }
}

#[test]
fn rule_targets_use_rule_position_and_sources_use_declaration_position() {
    let timed = replace_once(
        BASE,
        "OBJS := /obj/a.o /obj/b.o",
        "OBJS := /obj/a.o /obj/b.o\nmesa3d-linklib-galliumdrawllvm_C_FILES := /src/wrong-a /src/wrong-b",
    );
    let timed = replace_once(
        &timed,
        "mesa3d-linklib-galliumdrawllvm_C_FILES := /src/a /src/b",
        "OTHER_C_FILES := /src/a /src/b",
    );
    let timed = replace_once(
        &timed,
        "CPPFLAGS_FOR_RULE := -DFIRST=1 -DSECOND=2 -DFIRST=1",
        "CPPFLAGS_FOR_RULE := -DFROZEN=1",
    );
    let timed = replace_once(
        &timed,
        "%build_linklib mmake=mesa3d-linklib-galliumdrawllvm",
        "OBJS := /obj/later.o\nmesa3d-linklib-galliumdrawllvm_C_FILES := /src/a /src/b\nCPPFLAGS_FOR_RULE := -DLATER=1\n%build_linklib mmake=mesa3d-linklib-galliumdrawllvm",
    );
    assert_eq!(
        project(&timed).unwrap(),
        vec!["-DFIRST=1", "-DSECOND=2", "-DFIRST=1"]
    );
}

#[test]
fn target_rule_parser_rejects_empty_or_malformed_owner_names() {
    for owner in [
        "_MC_EXTRA_CPPFLAGS",
        "9owner_MC_EXTRA_CPPFLAGS",
        "bad;owner_MC_EXTRA_CPPFLAGS",
    ] {
        let line = format!("$(OBJECTS): {owner}:=-DVALUE=1");
        assert!(parse(&line).is_none(), "unexpectedly parsed {line:?}");
    }
}

#[test]
fn direct_rule_value_is_required_to_resolve_to_at_least_one_word() {
    let empty = replace_once(BASE, ":=-DFIRST=1 -DSECOND=2 -DFIRST=1", ":=");
    assert!(project(&empty).is_err());

    let unknown = replace_once(
        BASE,
        ":=-DFIRST=1 -DSECOND=2 -DFIRST=1",
        ":=$(TARGET_LOCAL_FLAGS)",
    );
    assert!(project(&unknown).is_err());
    for quoted in [r#"-DNAME="a b""#, "-DNAME='a b'", r"-DNAME=a\ b"] {
        let changed = replace_once(
            BASE,
            ":=-DFIRST=1 -DSECOND=2 -DFIRST=1",
            &format!(":={quoted}"),
        );
        assert!(project(&changed)
            .unwrap_err()
            .contains("shell quoting or escapes"));
    }
    for shell in [
        "-DNAME=x|cat",
        "-DNAME=x&cat",
        "-DNAME=*",
        "-DNAME=?",
        "-DNAME=~",
        "-DNAME=`id`",
        "-DNAME=x;id",
        "-DNAME=<file",
    ] {
        let changed = replace_once(
            BASE,
            ":=-DFIRST=1 -DSECOND=2 -DFIRST=1",
            &format!(":={shell}"),
        );
        assert!(project(&changed).unwrap_err().contains("shell-inert"));
    }
}

#[test]
fn effective_objdir_and_callsite_overrides_cannot_change_the_proven_set() {
    for definition in ["/other", "", "$(UNKNOWN)"] {
        let source = format!("{OWNER}_OBJDIR := {definition}\n{BASE}");
        assert!(project(&source).is_err(), "{definition}");
    }
    assert!(project(&format!("{OWNER}_OBJDIR := /obj\n{BASE}")).is_ok());
    for argument in [
        "cflags=-DOTHER",
        "cppflags=-DOTHER",
        "cppflags=",
        "dflags=-DOTHER",
        "includedir=/other",
        "lto=yes",
        "lto=no",
    ] {
        assert!(
            project(&replace_once(
                BASE,
                "usetree=yes",
                &format!("usetree=yes {argument}")
            ))
            .is_err(),
            "{argument}"
        );
    }
}

#[test]
fn real_make_appends_extra_flags_to_objects_and_dependency_targets() {
    use std::{fs, process::Command};
    let temporary = tempfile::tempdir().unwrap();
    let makefile = temporary.path().join("target-options.mk");
    // These immediate target-scope assignments mirror the separately sealed
    // rule_compile_multi fragment. No compiler or output artifact is invoked.
    let source = r"
OBJECTS := /obj/a.o /obj/b.o
DEPENDENCIES := /obj/a.d /obj/b.d
$(OBJECTS) $(DEPENDENCIES): lib_MC_EXTRA_CPPFLAGS := -DFIRST=1 -DSECOND=2 -DFIRST=1
lib_MC_CPPFLAGS := -DBASE=1
$(OBJECTS) $(DEPENDENCIES): lib_MC_CPPFLAGS := $(strip $(lib_MC_CPPFLAGS) $(lib_MC_EXTRA_CPPFLAGS))
$(OBJECTS) $(DEPENDENCIES):
	@printf '%s\n' 'flags=<$(lib_MC_CPPFLAGS)>'
";
    let make = if cfg!(target_os = "macos") {
        "gmake"
    } else {
        "make"
    };
    let run = |text: &str| {
        fs::write(&makefile, text).unwrap();
        let output = Command::new(make)
            .args(["-rR", "--no-print-directory", "-s", "-j1", "-f"])
            .arg(&makefile)
            .args(["/obj/a.o", "/obj/b.o", "/obj/a.d", "/obj/b.d"])
            .env_remove("MAKEFLAGS")
            .env_remove("MFLAGS")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let expected = format!("flags=<-DBASE=1 {}>\n", project(BASE).unwrap().join(" "));
    assert_eq!(run(source), expected.repeat(4));
    let partial = source.replacen(
        "$(OBJECTS) $(DEPENDENCIES): lib_MC_EXTRA_CPPFLAGS",
        "$(firstword $(OBJECTS)): lib_MC_EXTRA_CPPFLAGS",
        1,
    );
    assert_eq!(
        run(&partial),
        format!("{expected}{}", "flags=<-DBASE=1>\n".repeat(3))
    );
}
