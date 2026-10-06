use super::*;
use crate::make_vars::collect_vars_impl;
use crate::parser::{join_continuations, TargetContext};
use crate::testing::TempTree;
use std::fs;

const CONFIG: &str = "AROSDIR := $(TARGETDIR)/SYS\nAROS_DIR_DEVELOPER := Developer\nAROS_DEVELOPER := $(AROSDIR)/$(AROS_DIR_DEVELOPER)\nAROS_DIR_LIB := lib\nAROS_LIB := $(AROS_DEVELOPER)/$(AROS_DIR_LIB)\nOBJDIR := $(GENDIR)/$(CURDIR)\n";

fn fixture(source: &str) -> (TempTree, String, VarScope, DirVars, Vec<ConditionalTruth>) {
    let tree = TempTree::new();
    fs::create_dir_all(tree.0.join("config")).unwrap();
    fs::write(tree.0.join("config/make.cfg.in"), CONFIG).unwrap();
    fs::create_dir_all(tree.0.join("fixture")).unwrap();
    let joined = join_continuations(source);
    fs::write(tree.0.join("fixture/mmakefile.src"), &joined).unwrap();
    let context = TargetContext {
        make_variables: BTreeMap::from([("OBJDIR".to_owned(), "$(GENDIR)/$(CURDIR)".to_owned())]),
        ..TargetContext::default()
    };
    let (scope, states) = collect_vars_impl(&joined, Some(&context));
    let dirs = DirVars::load(&tree.0);
    (tree, joined, scope, dirs, states)
}

fn collect(source: &str) -> (Vec<SourceArchiveDecl>, Vec<SourceArchiveRejection>) {
    let (tree, joined, scope, dirs, states) = fixture(source);
    collect_from_snapshot(
        &joined,
        &scope,
        &dirs,
        &tree.0,
        Path::new("fixture"),
        Some(&states),
    )
}

fn finite_source() -> &'static str {
    "FILES := alpha beta\nOBJS := $(foreach f,$(FILES),$(OBJDIR)/$(f).o)\nARCHIVE := $(AROS_LIB)/libunusual+name.a\n#MM\narchive-owner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from=$^\n"
}

#[test]
fn finite_source_lists_keep_order_and_derive_archive_name() {
    let (declarations, rejections) = collect(finite_source());
    assert!(rejections.is_empty(), "{rejections:?}");
    assert_eq!(declarations.len(), 1);
    let declaration = &declarations[0];
    assert_eq!(declaration.owner, "archive-owner");
    assert_eq!(
        declaration.output,
        "${AROS_BUILD_DIR}/SYS/Developer/lib/libunusual+name.a"
    );
    assert!(matches!(
        &declaration.members,
        ArchiveMembers::Exact(members)
            if members == &[
                "${AROS_BUILD_DIR}/gen/fixture/alpha.o",
                "${AROS_BUILD_DIR}/gen/fixture/beta.o"
            ]
    ));
}

#[test]
fn deferred_wildcard_is_preserved_as_data_without_expansion() {
    let source = "HIDD_LIB := $(AROS_LIB)/libhiddstubs.a\nHIDD_STUBS_OBJ := $(strip $(call WILDCARD, $(GENDIR)/lib/hidd/*.o))\n#MM\nlinklibs-hiddstubs: $(HIDD_LIB)\n$(HIDD_LIB) : $(HIDD_STUBS_OBJ)\n\t%mklib_q from=$^\n";
    let (declarations, rejections) = collect(source);
    assert!(rejections.is_empty(), "{rejections:?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].owner, "linklibs-hiddstubs");
    assert_eq!(
        declarations[0].members,
        ArchiveMembers::ProducerGlob {
            root: "${AROS_BUILD_DIR}/gen/lib/hidd".into(),
            pattern: "*.o".into(),
        }
    );
}

#[test]
fn extra_or_modified_archive_recipes_are_refused() {
    for recipe in [
        "\t%mklib_q from=$^ extra=yes\n",
        "\t%mklib_q from=$^\n\t@echo extra\n",
        "\t@%mklib_q from=$^\n",
    ] {
        let source = finite_source().replace("\t%mklib_q from=$^\n", recipe);
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted recipe {recipe:?}");
        assert!(!rejections.is_empty(), "missed recipe {recipe:?}");
    }
}

fn explicit_from_source(recipe_from: &str, rule_members: &str, suffix: &str) -> String {
    format!(
        "OBJDIR := $(GENDIR)/fixture\nTOOL := gfxhiddtool\nOBJS := {rule_members}\nOTHER_OBJS := $(OBJDIR)/two.o $(OBJDIR)/one.o\nARCHIVE := $(AROS_LIB)/libgfxhiddtool.a\n#MM\narchive-owner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from={recipe_from}\n{suffix}"
    )
}

#[test]
fn explicit_from_accepts_the_same_finite_nested_member_expression() {
    let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "");
    let (declarations, rejections) = collect(&source);
    assert!(rejections.is_empty(), "{rejections:?}");
    assert_eq!(declarations.len(), 1);
    assert_eq!(
        declarations[0].members,
        ArchiveMembers::Exact(vec![concat!(
            "$",
            "{AROS_BUILD_DIR}/gen/fixture/gfxhiddtool.o"
        )
        .into()])
    );
}

#[test]
fn explicit_from_rejects_unequal_or_deferred_member_lists() {
    for source in [
        explicit_from_source("$(OBJDIR)/missing.o", "$(OBJDIR)/present.o", ""),
        explicit_from_source("$(OTHER_OBJS)", "$(OBJDIR)/one.o $(OBJDIR)/two.o", ""),
        explicit_from_source("$(OBJS)", "$(OBJDIR)/one.o $(OBJDIR)/one.o", ""),
        explicit_from_source(
            "$(OBJS)",
            "$(strip $(call WILDCARD,$(GENDIR)/lib/hidd/*.o))",
            "",
        ),
        explicit_from_source(
            "$(UNKNOWN_ARCHIVE_OBJECTS)",
            "$(UNKNOWN_ARCHIVE_OBJECTS)",
            "",
        ),
    ] {
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted {source:?}");
        assert!(!rejections.is_empty(), "missed {source:?}");
    }
}

#[test]
fn explicit_from_rejects_extra_arguments_and_automatic_variables() {
    for recipe in [
        "\t%mklib_q from=$(OBJS) extra\n",
        "\t%mklib_q from=\"$(OBJS)\"\n",
        "\t%mklib_q from=$<\n",
        "\t%mklib_q from=$(call WILDCARD,$(GENDIR)/lib/hidd/*.o)\n",
    ] {
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "")
            .replace("\t%mklib_q from=$(OBJS)\n", recipe);
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted recipe {recipe:?}");
        assert!(!rejections.is_empty(), "missed recipe {recipe:?}");
    }
}

#[test]
fn explicit_from_rejects_later_and_target_specific_rebindings() {
    for suffix in [
        "OBJDIR := $(GENDIR)/later\n",
        "OBJDIR = $(GENDIR)/later\n",
        "OBJDIR += later\n",
        "ARCHIVE: OBJDIR := $(GENDIR)/target-specific\n",
        "ARCHIVE: override OBJDIR = $(GENDIR)/target-specific\n",
        "ifeq ($(UNKNOWN_MODE),enabled)\nOBJDIR := $(GENDIR)/maybe\nendif\n",
    ] {
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", suffix);
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted suffix {suffix:?}");
        assert!(!rejections.is_empty(), "missed suffix {suffix:?}");
    }
}

#[test]
fn explicit_from_rejects_nested_rebindings_and_late_includes() {
    let nested = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "TOOL := rebound\n")
        .replace("OBJS :=", "OBJS =");
    let include = explicit_from_source(
        "$(OBJS)",
        "$(OBJDIR)/$(TOOL).o",
        "include $(SRCDIR)/config/aros.cfg\n",
    );
    let define = explicit_from_source(
        "$(OBJS)",
        "$(OBJDIR)/$(TOOL).o",
        "define OBJS\n$(OBJDIR)/other.o\nendef\n",
    );
    for source in [nested, include, define] {
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted {source:?}");
        assert!(!rejections.is_empty(), "missed {source:?}");
    }
}

#[test]
fn explicit_from_rejects_indirect_effects_and_modified_undefine() {
    for suffix in [
        "override undefine OBJS\n",
        "UNRELATED := $(call MUTATE,OBJS)\n",
        "FN := eval\nUNRELATED := $($(FN) OBJS := changed)\n",
        "UNRELATED := text %rule_makedirs dirs=extra\n",
        "%include_deps\n",
    ] {
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", suffix);
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted effects {suffix:?}");
        assert!(!rejections.is_empty(), "missed effects {suffix:?}");
    }
    let source = format!(
        "include $(SRCDIR)/config/aros.cfg\n{}",
        explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", ""),
    );
    assert!(collect(&source).0.is_empty());
}

#[test]
fn explicit_from_preserves_immediate_assignment_freezing() {
    let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "TOOL := rebound\n");
    let (declarations, rejections) = collect(&source);
    assert!(rejections.is_empty(), "{rejections:?}");
    assert!(matches!(
        declarations[0].members,
        ArchiveMembers::Exact(ref members) if members[0].ends_with("/gfxhiddtool.o")
    ));
}

#[test]
fn explicit_from_rejects_environment_owned_output_aliases() {
    for suffix in [
        "$(UNMODELED_OUTPUT) : external.o\n",
        "$(UNMODELED_OUTPUT) : external.o\n\t@echo replacement\n",
        "$(UNMODELED_OUTPUT) $(ARCHIVE) : external.o\n",
    ] {
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", suffix);
        let (declarations, rejections) = collect(&source);
        assert!(
            declarations.is_empty(),
            "accepted unknown output {suffix:?}"
        );
        assert!(!rejections.is_empty(), "missed unknown output {suffix:?}");
    }
}

#[test]
fn explicit_from_rejects_environment_sensitive_source_defaults() {
    let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", "");
    for (original, conditional) in [
        ("ARCHIVE :=", "ARCHIVE ?="),
        ("OBJS :=", "OBJS ?="),
        ("TOOL :=", "TOOL ?="),
        ("OBJDIR :=", "export OBJDIR ?="),
    ] {
        assert!(
            source.contains(original),
            "missing probe binding {original}"
        );
        let conditional_source = source.replace(original, conditional);
        let (declarations, rejections) = collect(&conditional_source);
        assert!(declarations.is_empty(), "accepted {conditional_source:?}");
        assert!(!rejections.is_empty(), "missed {conditional_source:?}");
    }
}

#[test]
fn explicit_from_rejects_known_multi_target_prerequisite_overlays() {
    for overlay in [
        "$(ARCHIVE) $(OTHER_TARGET) : external.o\n",
        "$(OTHER_TARGET) $(ARCHIVE) : external.o\n",
        "$(ARCHIVE) $(ARCHIVE) : external.o\n",
    ] {
        let suffix = format!("OTHER_TARGET := $(OBJDIR)/other.o\n{overlay}");
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", &suffix);
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted {overlay:?}");
        assert!(!rejections.is_empty(), "missed {overlay:?}");
    }
}

#[test]
fn automatic_from_rejects_known_and_unresolved_archive_overlays() {
    for suffix in [
        "OTHER_TARGET := $(OBJDIR)/other.o\n$(ARCHIVE) $(OTHER_TARGET) : external.o\n",
        "OTHER_TARGET := $(OBJDIR)/other.o\n$(OTHER_TARGET) $(ARCHIVE) : external.o\n",
        "$(UNMODELED_OUTPUT) : external.o\n",
        "$(AROS_LIB)/../lib/libgfxhiddtool.a : external.o\n",
    ] {
        let source = explicit_from_source("$^", "$(OBJDIR)/$(TOOL).o", suffix);
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted {suffix:?}");
        assert!(!rejections.is_empty(), "missed {suffix:?}");
    }
}

#[test]
fn automatic_from_keeps_unrelated_known_multi_target_rules() {
    let source = explicit_from_source(
        "$^",
        "$(OBJDIR)/$(TOOL).o",
        "$(OBJDIR)/one.o $(OBJDIR)/two.o : input.c\n",
    );
    let (declarations, rejections) = collect(&source);
    assert_eq!(declarations.len(), 1, "{rejections:?}");
    assert!(rejections.is_empty(), "{rejections:?}");
}

#[test]
fn archive_prerequisite_closure_keeps_provably_disjoint_patterns() {
    for from in ["$^", "$(OBJS)"] {
        for pattern in [
            "$(OBJDIR)/%.o",
            "%.o",
            "$(OBJDIR)/%/libgfxhiddtool.a",
            "libother%.a",
        ] {
            let source = explicit_from_source(
                from,
                "$(OBJDIR)/$(TOOL).o",
                &format!("{pattern} : input.c\n"),
            );
            let (declarations, rejections) = collect(&source);
            assert_eq!(declarations.len(), 1, "{pattern}: {rejections:?}");
            assert!(rejections.is_empty(), "{pattern}: {rejections:?}");
        }
    }
}

#[test]
fn genmf_reference_detection_does_not_hide_inline_or_unknown_templates() {
    for line in [
        "%compile_q",
        "OUTPUT := %unknown_template argument=value",
        "prefix%future_template\targument=value",
        "lib%unmodeled : input.o",
        "value := %1_name\u{2003}argument=value",
        "value := %1_name\u{1c}argument=value",
    ] {
        assert!(contains_genmf_reference(line), "missed {line:?}");
    }
    for line in ["%.o : %.c", "lib%other.a : input.o", "% : input.o", "x%/y"] {
        assert!(!contains_genmf_reference(line), "misclassified {line:?}");
    }
    for suffix in [
        "UNRELATED := prefix%unknown_template argument=value\n",
        "%future_template argument=value\n",
        "lib%unmodeled : input.o\n",
        "UNRELATED := value # %compile_q\n",
        "  # %compile_q\n",
        "other-target:\n\t%compile_q\n",
        "other-target:\n\t@echo value # %compile_q\n",
        "ifeq (no,yes)\n%future_template\nendif\n",
    ] {
        let source = explicit_from_source("$(OBJS)", "$(OBJDIR)/$(TOOL).o", suffix);
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted {suffix:?}");
        assert!(!rejections.is_empty(), "missed {suffix:?}");
    }
}

#[test]
fn archive_prerequisite_closure_rejects_compatible_or_ambiguous_patterns() {
    for from in ["$^", "$(OBJS)"] {
        for pattern in [
            "$(AROS_LIB)/lib%.a",
            "lib%.a",
            "%.a",
            "%",
            "LIB%.A",
            "lib%gfxhiddtool.a",
            "$(TARGETDIR)/%/libgfxhiddtool.a",
            "lib%%.a",
            "$(AROS_LIB)/../lib/lib%.a",
            "$(AROS_LIB)//lib%.a",
            "lib*.a",
            "lib?.a",
            "lib[ab].a",
            "lib\\%.a",
        ] {
            let source = explicit_from_source(
                from,
                "$(OBJDIR)/$(TOOL).o",
                &format!("{pattern} : external.o\n"),
            );
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted {from}: {pattern}");
            assert!(!rejections.is_empty(), "missed {from}: {pattern}");
        }
    }
}

#[test]
fn archive_pattern_matching_preserves_directory_and_empty_basename_stems() {
    let output = "${AROS_BUILD_DIR}/SYS/Developer/lib/libopenurl.a";
    assert!(archive_pattern_may_match("lib%openurl.a", output).unwrap());
    assert!(archive_pattern_may_match("${AROS_BUILD_DIR}/%/libopenurl.a", output).unwrap());
    assert!(archive_pattern_may_match("LIB%OPENURL.A", output).unwrap());
    assert!(!archive_pattern_may_match("${AROS_BUILD_DIR}/gen/%.a", output).unwrap());
    assert!(!archive_pattern_may_match("%.o", output).unwrap());
    assert!(archive_pattern_may_match("lib%%.a", output).is_err());
}

#[test]
fn archive_prerequisite_closure_rejects_unknown_pattern_conditions() {
    let source = explicit_from_source(
        "$^",
        "$(OBJDIR)/$(TOOL).o",
        "ifeq ($(UNKNOWN_ARCHIVE_MODE),enabled)\n$(OBJDIR)/%.o : input.c\nendif\n",
    );
    let (declarations, rejections) = collect(&source);
    assert!(declarations.is_empty());
    assert!(!rejections.is_empty());
}

#[test]
fn archive_prerequisite_closure_rejects_unbound_root_and_working_directory_aliases() {
    for from in ["$^", "$(OBJS)"] {
        for target in [
            "/tmp/build/SYS/Developer/lib/libgfxhiddtool.a",
            "/tmp/build/SYS/Developer/lib/lib%.a",
            "~/build/libgfxhiddtool.a",
            "~other/build/lib%.a",
            "${AROS_SOURCE_DIR}/build/SYS/Developer/lib/libgfxhiddtool.a",
            "${AROS_SOURCE_DIR}/build/SYS/Developer/lib/%.a",
            "${AROS_BUILD_DIR}/${CMAKE_CONFIG}/libgfxhiddtool.a",
            "SYS/Developer/lib/%.a",
            "libgfxhiddtool.a",
            "SYS/Developer/lib/LIBGFXHIDDTOOL.A",
        ] {
            let source = explicit_from_source(
                from,
                "$(OBJDIR)/$(TOOL).o",
                &format!("{target}: external.o\n"),
            );
            let (declarations, rejections) = collect(&source);
            assert!(declarations.is_empty(), "accepted {from}: {target}");
            assert!(!rejections.is_empty(), "missed {from}: {target}");
        }
    }
    for target in ["setup", "clean", "other.o", "relative/path/other.a"] {
        let source =
            explicit_from_source("$^", "$(OBJDIR)/$(TOOL).o", &format!("{target}: input.c\n"));
        let (declarations, rejections) = collect(&source);
        assert_eq!(declarations.len(), 1, "{target}: {rejections:?}");
        assert!(rejections.is_empty(), "{target}: {rejections:?}");
    }
}

#[test]
fn archive_proof_bytes_are_shared_and_fail_without_reset_or_overflow() {
    let mut bytes = MAX_ARCHIVE_PROOF_BYTES - 4;
    charge_archive_proof_bytes(4, &mut bytes).unwrap();
    assert_eq!(bytes, MAX_ARCHIVE_PROOF_BYTES);
    assert!(charge_archive_proof_bytes(1, &mut bytes).is_err());
    assert_eq!(bytes, MAX_ARCHIVE_PROOF_BYTES);
    let mut overflowing = usize::MAX;
    assert!(charge_archive_proof_bytes(1, &mut overflowing).is_err());
    assert_eq!(overflowing, usize::MAX);
}

#[test]
fn admitted_archive_role_defaults_are_explicit_and_not_ambient() {
    let source = format!(
        "AR = $(NATIVE_TARGET_AR) cr\nRANLIB = $(NATIVE_TARGET_RANLIB)\n{}",
        finite_source()
    );
    let (tree, joined, scope, mut dirs, states) = fixture(&source);
    let (_, rejected) = collect_from_snapshot(
        &joined,
        &scope,
        &dirs,
        &tree.0,
        Path::new("fixture"),
        Some(&states),
    );
    assert!(!rejected.is_empty());
    dirs.bind_native_target_tool_roles();
    let (decls, rejected) = collect_from_snapshot(
        &joined,
        &scope,
        &dirs,
        &tree.0,
        Path::new("fixture"),
        Some(&states),
    );
    assert_eq!(decls.len(), 1, "{rejected:?}");
    assert!(rejected.is_empty());
    let mutation = format!("{source}NATIVE_TARGET_AR := unsafe\n");
    let (tree, joined, scope, mut dirs, states) = fixture(&mutation);
    dirs.bind_native_target_tool_roles();
    let (decls, rejected) = collect_from_snapshot(
        &joined,
        &scope,
        &dirs,
        &tree.0,
        Path::new("fixture"),
        Some(&states),
    );
    assert!(decls.is_empty());
    assert!(!rejected.is_empty());
}

#[test]
fn global_and_target_specific_archive_role_overrides_are_refused() {
    for prefix in [
        "AR := unsafe-ar\n",
        "override RANLIB = unsafe-ranlib\n",
        "$(AROS_LIB)/libunusual+name.a: AR = unsafe-ar\n",
        "archive-owner: RANLIB := unsafe-ranlib\n",
        "archive-owner: override RANLIB = unsafe-ranlib\n",
        "undefine AR\n",
    ] {
        let source = format!("{prefix}{}", finite_source());
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted override {prefix:?}");
        assert!(!rejections.is_empty(), "missed override {prefix:?}");
    }
}

#[test]
fn duplicate_case_colliding_archive_outputs_are_refused() {
    let source = "FILES := one\nOBJS := $(OBJDIR)/$(FILES).o\nA := $(AROS_LIB)/libFoo.a\nB := $(AROS_LIB)/libfoo.a\n#MM\none-owner : $(A)\n$(A) : $(OBJS)\n\t%mklib_q from=$^\n#MM\ntwo-owner : $(B)\n$(B) : $(OBJS)\n\t%mklib_q from=$^\n";
    let (declarations, rejections) = collect(source);
    assert!(declarations.is_empty());
    assert!(rejections
        .iter()
        .any(|item| item.reason.contains("case-colliding")));
}

#[test]
fn missing_or_ambiguous_owner_is_refused() {
    let missing =
        "OBJS := $(OBJDIR)/x.o\nA := $(AROS_LIB)/libx.a\n$(A) : $(OBJS)\n\t%mklib_q from=$^\n";
    let (declarations, rejections) = collect(missing);
    assert!(declarations.is_empty());
    assert!(rejections
        .iter()
        .any(|item| item.reason.contains("no unique")));

    let ambiguous = "OBJS := $(OBJDIR)/x.o\nA := $(AROS_LIB)/libx.a\n#MM\none-owner : $(A)\n#MM\ntwo-owner : $(A)\n$(A) : $(OBJS)\n\t%mklib_q from=$^\n";
    let (declarations, rejections) = collect(ambiguous);
    assert!(declarations.is_empty());
    assert!(rejections
        .iter()
        .any(|item| item.reason.contains("multiple ordinary")));
}

#[test]
fn unknown_condition_and_opaque_make_control_are_refused() {
    let conditional = "ifeq ($(UNKNOWN_MODE),enabled)\n".to_owned() + finite_source() + "endif\n";
    let (declarations, rejections) = collect(&conditional);
    assert!(declarations.is_empty());
    assert!(!rejections.is_empty());

    for control in [
        "define HIDDEN\nAR := unsafe-ar\nendef\n",
        "$(eval AR := unsafe-ar)\n",
        "include $(UNMODELLED_CONFIG)\n",
    ] {
        let source = format!("{}{}", control, finite_source());
        let (declarations, rejections) = collect(&source);
        assert!(declarations.is_empty(), "accepted control {control:?}");
        assert!(!rejections.is_empty(), "missed control {control:?}");
    }
}

#[test]
fn unsafe_members_and_outside_archive_root_are_refused() {
    for source in [
        "FILES := ../escape\nOBJS := $(addprefix $(OBJDIR)/,$(FILES))\nARCHIVE := $(AROS_LIB)/libescape.a\n#MM\nowner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from=$^\n",
        "FILES := object\nOBJS := $(OBJDIR)/$(FILES).o\nARCHIVE := /tmp/libforeign.a\n#MM\nowner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from=$^\n",
        "FILES := object\nOBJS := $(OBJDIR)/$(FILES).c\nARCHIVE := $(AROS_LIB)/libbad.a\n#MM\nowner : $(ARCHIVE)\n$(ARCHIVE) : $(OBJS)\n\t%mklib_q from=$^\n",
    ] {
        let (declarations, rejections) = collect(source);
        assert!(declarations.is_empty(), "accepted unsafe source {source:?}");
        assert!(!rejections.is_empty());
    }
}

#[test]
#[ignore = "requires the isolated P4 checkout via AROS_P4_SOURCE_ROOT"]
fn actual_p4_openurl_and_hidd_archive_rules_are_source_owned() {
    let source_root = std::env::var("AROS_P4_SOURCE_ROOT")
        .expect("explicit P4 archive source probe requires AROS_P4_SOURCE_ROOT");
    let source_root = Path::new(&source_root)
        .canonicalize()
        .expect("AROS_P4_SOURCE_ROOT must resolve");
    let dirs = DirVars::load(&source_root);
    let mut seen = BTreeSet::new();
    for (relative_dir, expected_owner, expected_suffix, glob) in [
        (
            Path::new("external/openurl/libopenurl"),
            "linklibs-openurl-quick",
            "libopenurl.a",
            false,
        ),
        (
            Path::new("compiler/libhiddstubs"),
            "linklibs-hiddstubs",
            "libhiddstubs.a",
            true,
        ),
    ] {
        let content = fs::read_to_string(source_root.join(relative_dir).join("mmakefile.src"))
            .expect("read P4 archive rule source");
        let joined = crate::parser::join_continuations(&content);
        let target_context = TargetContext {
            make_variables: BTreeMap::from([(
                "OBJDIR".to_owned(),
                "$(GENDIR)/$(CURDIR)".to_owned(),
            )]),
            ..TargetContext::default()
        };
        let (scope, states) = collect_vars_impl(&joined, Some(&target_context));
        let (declarations, rejections) = collect_from_snapshot(
            &joined,
            &scope,
            &dirs,
            &source_root,
            relative_dir,
            Some(&states),
        );
        assert!(rejections.is_empty(), "{relative_dir:?}: {rejections:?}");
        assert_eq!(declarations.len(), 1, "{relative_dir:?}");
        let declaration = &declarations[0];
        assert_eq!(declaration.owner, expected_owner);
        assert!(declaration.output.ends_with(expected_suffix));
        assert_eq!(
            matches!(declaration.members, ArchiveMembers::ProducerGlob { .. }),
            glob
        );
        seen.insert(declaration.output.clone());
    }
    assert_eq!(seen.len(), 2);
}
