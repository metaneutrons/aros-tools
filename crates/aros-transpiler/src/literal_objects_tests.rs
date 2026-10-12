use super::*;
use crate::dirs::DirVars;
use crate::make_vars::collect_vars;
use std::fs;
use tempfile::tempdir;

fn fixture(
    source: &str,
    symlink_source: bool,
) -> (Vec<LiteralObjectGroupDecl>, Vec<LiteralObjectRejection>) {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("compiler/libinit")).unwrap();
    fs::write(
        root.join("compiler/libinit/real.c"),
        "int entry(void) { return 0; }\n",
    )
    .unwrap();
    if symlink_source {
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.join("compiler/libinit/real.c"),
            root.join("compiler/libinit/entry.c"),
        )
        .unwrap();
    } else {
        fs::write(
            root.join("compiler/libinit/entry.c"),
            "int entry(void) { return 0; }\n",
        )
        .unwrap();
    }
    let joined = crate::parser::join_continuations(source);
    let scope = collect_vars(&joined);
    let dirs = DirVars::load(root);
    collect_from_snapshot(
        &scope,
        &dirs,
        root,
        Path::new("compiler/libinit"),
        None,
        &joined,
    )
}

fn source(recipe: &str) -> String {
    format!(
        "CFLAGS := -O0\n\
         $(GENDIR)/$(CURDIR)/entry.o: $(SRCDIR)/$(CURDIR)/entry.c | $(GENDIR)/$(CURDIR)\n\
         \t{recipe}\n\
         CFLAGS := -O2 -DORDER=first -DORDER=second -UOLD -mabi=lp64d\n\
         entry-owner : $(GENDIR)/compiler/libinit/entry.o\n"
    )
}

#[test]
fn recipe_free_order_only_augmentation_never_claims_an_object_producer() {
    let augmentation = "$(GENDIR)/$(CURDIR)/entry.o: | $(GENDIR)/$(CURDIR)\n";
    let (groups, rejected) = fixture(augmentation, false);
    assert!(groups.is_empty());
    assert!(rejected.is_empty(), "{rejected:#?}");
    let inline = "$(GENDIR)/$(CURDIR)/entry.o: | $(GENDIR)/$(CURDIR); touch $@\n";
    let (groups, rejected) = fixture(inline, false);
    assert!(groups.is_empty());
    assert!(
        !rejected.is_empty(),
        "an inline recipe must not be hidden as a prerequisite-only fragment"
    );
    let claimed = format!("{augmentation}\nentry-owner: $(GENDIR)/$(CURDIR)/entry.o\n");
    let (groups, rejected) = fixture(&claimed, false);
    assert!(groups.is_empty());
    assert!(
        rejected
            .iter()
            .any(|rejection| rejection.reason.contains("missing producer")),
        "{rejected:#?}"
    );
    let conflicting = format!(
        "{augmentation}{}",
        source("$(TARGET_CC) $(CFLAGS) -c $< -o $@")
    );
    let (groups, rejected) = fixture(&conflicting, false);
    assert!(groups.is_empty());
    assert!(
        rejected
            .iter()
            .any(|rejection| rejection.reason.contains("more than one literal producer")),
        "{rejected:#?}"
    );
}

#[test]
fn accepts_warning_flags_but_not_driver_forwarding() {
    let (groups, errors) = fixture(
        &source("@$(TARGET_CC) -Wall -Werror -Wno-pointer-sign -Wno-parentheses -c $< -o $@"),
        false,
    );
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(
        groups[0].objects[0].arguments,
        ["-Wall", "-Werror", "-Wno-pointer-sign", "-Wno-parentheses"]
    );
    for unsafe_flag in ["-Wa,option", "-Wp,option", "-Wl,option"] {
        let (groups, errors) = fixture(
            &source(&format!("@$(TARGET_CC) {unsafe_flag} -c $< -o $@")),
            false,
        );
        assert!(groups.is_empty());
        assert!(!errors.is_empty());
    }
}

#[test]
fn caller_target_specific_flags_cannot_be_discarded() {
    let text = format!(
        "{}\nparent : CFLAGS = -DCALLER\nparent : entry-owner\n",
        source("@$(TARGET_CC) $(CFLAGS) -c $< -o $@")
    );
    let (groups, errors) = fixture(&text, false);
    assert!(groups.is_empty());
    assert!(
        errors
            .iter()
            .any(|error| error.reason.contains("target-specific")),
        "{errors:#?}"
    );
}

#[test]
fn preserves_duplicate_flag_order_and_uses_final_assignment() {
    let (groups, rejections) = fixture(&source("@$(TARGET_CC) $(CFLAGS) -c $< -o $@"), false);
    assert!(rejections.is_empty(), "{rejections:#?}");
    assert_eq!(groups.len(), 1, "{groups:#?}");
    let object = &groups[0].objects[0];
    assert_eq!(object.source, "${AROS_SOURCE_DIR}/compiler/libinit/entry.c");
    assert_eq!(object.language, "C");
    assert_eq!(
        object.arguments,
        [
            "-O2",
            "-DORDER=first",
            "-DORDER=second",
            "-UOLD",
            "-mabi=lp64d"
        ]
    );
}

#[test]
fn unknown_flags_are_not_treated_as_empty() {
    let (groups, rejections) =
        fixture(&source("@$(TARGET_CC) $(UNKNOWN_FLAGS) -c $< -o $@"), false);
    assert!(groups.is_empty());
    assert!(
        rejections.iter().any(|item| {
            item.reason
                .contains("cannot resolve final literal compiler flags")
                && item.reason.contains("UNKNOWN_FLAGS")
        }),
        "{rejections:#?}"
    );
}

#[test]
fn admitted_target_role_is_explicit_and_cannot_be_rebound() {
    let source = "TARGET_CC = $(NATIVE_TARGET_CC)\n";
    let lines = source.lines().collect::<Vec<_>>();
    let controls = scan_source_controls(&lines);
    let root = tempdir().unwrap();
    let mut dirs = DirVars::load(root.path());
    assert!(driver_binding_issue(&lines, &controls, None, &dirs).is_some());
    dirs.bind_native_target_tool_roles();
    assert!(driver_binding_issue(&lines, &controls, None, &dirs).is_none());
    let changed = format!("{source}NATIVE_TARGET_CC := wrapper\n");
    let changed_lines = changed.lines().collect::<Vec<_>>();
    assert!(driver_binding_issue(
        &changed_lines,
        &scan_source_controls(&changed_lines),
        None,
        &dirs
    )
    .is_some());
    let states = [ConditionalTruth::Unknown];
    assert!(driver_binding_issue(&lines, &controls, Some(&states), &dirs).is_some());
}

#[test]
fn rejects_local_driver_rebinding_and_includes_that_may_rebind_it() {
    for prefix in [
        "TARGET_CC := wrapper\n",
        "define TARGET_CC\nwrapper\nendef\n",
        "undefine TARGET_CXX\n",
        "include unresolved-fragment.mk\n",
    ] {
        let (groups, rejections) = fixture(
            &format!("{prefix}{}", source("@$(TARGET_CC) -c $< -o $@")),
            false,
        );
        assert!(groups.is_empty(), "{prefix}");
        assert!(!rejections.is_empty(), "{prefix}");
    }
}

#[test]
fn rejects_source_symlink_components() {
    let (groups, rejections) = fixture(&source("@$(TARGET_CC) -c $< -o $@"), true);
    #[cfg(unix)]
    {
        assert!(groups.is_empty());
        assert!(rejections
            .iter()
            .any(|item| item.reason.contains("symlink component")));
    }
    #[cfg(not(unix))]
    {
        let _ = (groups, rejections);
    }
}

#[test]
fn rejects_duplicate_output_and_conflicting_aggregate_owner() {
    let duplicate = source("@$(TARGET_CC) -c $< -o $@").replace(
        "entry-owner :",
        "$(GENDIR)/$(CURDIR)/entry.o: $(SRCDIR)/$(CURDIR)/entry.c\n\
         \t@$(TARGET_CC) -c $< -o $@\n\
         entry-owner :",
    );
    let (groups, rejections) = fixture(&duplicate, false);
    assert!(groups.is_empty());
    assert!(rejections
        .iter()
        .any(|item| item.reason.contains("more than one literal producer")));

    let conflicting = source("@$(TARGET_CC) -c $< -o $@").replace(
        "entry-owner :",
        "other-owner : $(GENDIR)/compiler/libinit/entry.o\nentry-owner :",
    );
    let (groups, rejections) = fixture(&conflicting, false);
    assert!(groups.is_empty());
    assert!(rejections
        .iter()
        .any(|item| item.reason.contains("conflicting aggregate owners")));
}

#[test]
fn define_bodies_and_false_conditionals_are_inert() {
    let body = source("@$(TARGET_CC) -c $< -o $@");
    let joined_source = format!("define HIDDEN\n{body}\nendef\nifeq (a,b)\n{body}\nendif\n{body}");
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("compiler/libinit")).unwrap();
    fs::write(root.join("compiler/libinit/entry.c"), "int entry(void);\n").unwrap();
    let joined = crate::parser::join_continuations(&joined_source);
    let dirs = DirVars::load(root);
    let scope = crate::make_vars::collect_vars(&joined);
    let mut states = vec![ConditionalTruth::True; joined.lines().count()];
    // The target-agnostic variable scanner deliberately treats conditions
    // as unknown; this known-false branch exercises the collector's state
    // input in the same way the pipeline supplies target-evaluated states.
    let false_branch_start = joined
        .lines()
        .position(|line| line == "ifeq (a,b)")
        .unwrap();
    let false_branch_end = joined.lines().position(|line| line == "endif").unwrap();
    states[false_branch_start..=false_branch_end].fill(ConditionalTruth::False);
    let (groups, rejections) = collect_from_snapshot(
        &scope,
        &dirs,
        root,
        Path::new("compiler/libinit"),
        Some(&states),
        &joined,
    );
    assert_eq!(groups.len(), 1, "{groups:#?}\n{rejections:#?}");
    assert!(rejections.is_empty(), "{rejections:#?}");
}
