use super::{
    capture_kobj_scoped_inputs, capture_kobj_scoped_inputs_for_module,
    capture_kobj_scoped_inputs_with_known_source_includes, collect_bounded_include_matches,
    KobjModuleArgs, KobjScopedInputs, ScopedMakeWords,
};
use crate::dirs::DirVars;
use crate::parser::TargetContext;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixture() -> (TempDir, PathBuf, DirVars, TargetContext) {
    let temp = tempfile::tempdir().expect("temporary source tree");
    let root = temp.path().to_path_buf();
    fs::create_dir_all(root.join("arch/pc/kernel")).expect("make option directory");
    let dirs = DirVars::load(&root);
    let target = TargetContext {
        platform: Some("pc".to_owned()),
        cpu: Some("x86_64".to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        ..TargetContext::default()
    };
    (temp, root, dirs, target)
}

fn capture(
    source: &str,
    line: usize,
    module: &str,
    flavour: Option<&str>,
    root: &Path,
    dirs: &DirVars,
    target: &TargetContext,
) -> KobjScopedInputs {
    capture_kobj_scoped_inputs(
        source,
        line,
        module,
        flavour,
        root,
        Path::new("rom/kernel/mmakefile.src"),
        dirs,
        target,
    )
}

fn capture_with_uselibs(
    source: &str,
    line: usize,
    module: &str,
    raw_uselibs: Option<&str>,
    root: &Path,
    dirs: &DirVars,
    target: &TargetContext,
) -> KobjScopedInputs {
    capture_kobj_scoped_inputs_for_module(
        source,
        line,
        KobjModuleArgs {
            module_name: module,
            flavour: None,
            raw_uselibs,
            raw_funcinstr: None,
        },
        root,
        Path::new("rom/kernel/mmakefile.src"),
        dirs,
        target,
        &[],
    )
}

fn capture_with_funcinstr(
    source: &str,
    line: usize,
    raw_funcinstr: Option<&str>,
    root: &Path,
    dirs: &DirVars,
    target: &TargetContext,
) -> KobjScopedInputs {
    capture_kobj_scoped_inputs_for_module(
        source,
        line,
        KobjModuleArgs {
            module_name: "kernel",
            flavour: None,
            raw_uselibs: None,
            raw_funcinstr,
        },
        root,
        Path::new("rom/kernel/mmakefile.src"),
        dirs,
        target,
        &[],
    )
}

fn exact(value: &ScopedMakeWords) -> (&str, &[String]) {
    match value {
        ScopedMakeWords::Exact { raw, words, .. } => (raw, words),
        other => panic!("expected exact Make words, got {other:?}"),
    }
}

fn known_empty(value: &ScopedMakeWords) {
    assert!(
        matches!(value, ScopedMakeWords::KnownEmpty { .. }),
        "{value:?}"
    );
}

fn unresolved(value: &ScopedMakeWords) {
    assert!(
        matches!(value, ScopedMakeWords::Unresolved { .. }),
        "{value:?}"
    );
}

#[test]
fn wildcard_include_traversal_errors_cannot_publish_a_successful_subset() {
    let matches = [
        Ok(PathBuf::from("first/make.opts")),
        Err("unreadable source directory"),
        Ok(PathBuf::from("last/make.opts")),
    ];
    assert_eq!(
        collect_bounded_include_matches(matches.into_iter(), 3),
        Err("unreadable source directory")
    );
    let matches = [
        Ok::<_, &str>(PathBuf::from("first/make.opts")),
        Ok(PathBuf::from("last/make.opts")),
    ];
    assert_eq!(
        collect_bounded_include_matches(matches.into_iter(), 1).unwrap(),
        [PathBuf::from("first/make.opts")]
    );
}

#[test]
fn repeated_make_opts_preserve_include_and_flag_order_duplicates() {
    let (_temp, root, dirs, target) = fixture();
    fs::write(
        root.join("arch/pc/kernel/make.opts"),
        "USER_LDFLAGS += -static -static\n",
    )
    .expect("write options");
    let source = "-include $(SRCDIR)/arch/$(ARCH)/kernel/make.opts\n-include $(SRCDIR)/arch/$(ARCH)/kernel/make.opts\n%build_module modname=kernel flavour=debug\n";
    let result = capture(source, 2, "kernel", Some("debug"), &root, &dirs, &target);
    assert_eq!(result.defname, "kernel_debug");
    assert_eq!(result.included_make_opts.len(), 2);
    assert_eq!(
        exact(&result.user_ldflags).1,
        ["-static", "-static", "-static", "-static"]
    );
}

#[test]
fn uncertain_configuration_flavor_cannot_select_an_include_path() {
    let (_temp, root, dirs, target) = fixture();
    fs::create_dir_all(root.join("arch/early/kernel")).expect("early include directory");
    fs::create_dir_all(root.join("arch/late/kernel")).expect("late include directory");
    fs::write(
        root.join("arch/early/kernel/make.opts"),
        "USER_LDFLAGS := -early\n",
    )
    .expect("write early options");
    fs::write(
        root.join("arch/late/kernel/make.opts"),
        "USER_LDFLAGS := -late\n",
    )
    .expect("write late options");
    let target = TargetContext {
        make_variables: BTreeMap::from([("PATH".to_owned(), String::new())]),
        ..target
    };

    let uncertain = "EARLY := arch/early/kernel/make.opts\nPATH += $(EARLY)\nEARLY := arch/late/kernel/make.opts\n-include $(SRCDIR)/$(PATH)\n%build_module modname=kernel\n";
    let result = capture(uncertain, 4, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_ldflags);
    assert!(result.included_make_opts.is_empty());

    let simple = "EARLY := arch/early/kernel/make.opts\nPATH := $(EARLY)\nEARLY := arch/late/kernel/make.opts\n-include $(SRCDIR)/$(PATH)\n%build_module modname=kernel\n";
    let result = capture(simple, 4, "kernel", None, &root, &dirs, &target);
    assert_eq!(
        result
            .included_make_opts
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["arch/early/kernel/make.opts"]
    );
    assert_eq!(exact(&result.user_ldflags).1, ["-early"]);

    let recursive = "EARLY := arch/early/kernel/make.opts\nPATH = $(EARLY)\nEARLY := arch/late/kernel/make.opts\n-include $(SRCDIR)/$(PATH)\n%build_module modname=kernel\n";
    let result = capture(recursive, 4, "kernel", None, &root, &dirs, &target);
    assert_eq!(
        result
            .included_make_opts
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["arch/late/kernel/make.opts"]
    );
    assert_eq!(exact(&result.user_ldflags).1, ["-late"]);
}

#[test]
fn source_local_recursive_overrides_are_evaluated_at_the_declaration() {
    let (_temp, root, dirs, target) = fixture();
    let source =
        "FLAGS = -first\nUSER_LDFLAGS = $(FLAGS)\n%build_module modname=kernel\nFLAGS = -later\n";
    let result = capture(source, 2, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_ldflags).1, ["-first"]);
}

#[test]
fn simple_assignment_freezes_its_local_reference_before_later_override() {
    let (_temp, root, dirs, target) = fixture();
    let source = "FLAGS = -initial\nUSER_LDFLAGS := $(FLAGS)\nFLAGS = -later\n%build_module modname=kernel\n";
    let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_ldflags).1, ["-initial"]);
}

#[test]
fn make_opts_replacement_and_following_local_append_keep_source_order() {
    let (_temp, root, dirs, target) = fixture();
    fs::write(
        root.join("arch/pc/kernel/make.opts"),
        "USER_LDFLAGS := -from-opts\n",
    )
    .expect("write options");
    let source = "USER_LDFLAGS := -before\n-include $(SRCDIR)/arch/$(ARCH)/kernel/make.opts\nUSER_LDFLAGS += -after\n%build_module modname=kernel\n";
    let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_ldflags).1, ["-from-opts", "-after"]);
}

#[test]
fn unknown_conditional_input_is_unresolved_until_a_proven_replacement() {
    let (_temp, root, dirs, target) = fixture();
    let source = "USER_LDFLAGS += -prefix\nifeq ($(NOT_IN_SOURCE_CONTRACT),1)\nUSER_LDFLAGS += -conditional\nendif\n%build_module modname=kernel\n";
    let result = capture(source, 4, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_ldflags);

    let reset = "ifeq ($(NOT_IN_SOURCE_CONTRACT),1)\nUSER_LDFLAGS += -conditional\nendif\nUSER_LDFLAGS := -reset\n%build_module modname=kernel\n";
    let result = capture(reset, 4, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_ldflags).1, ["-reset"]);
}

#[test]
fn simple_expansion_retains_dependency_taint_until_input_is_replaced() {
    let (_temp, root, dirs, target) = fixture();
    let source = "FLAGS = -base\nifeq ($(UNKNOWN),1)\nFLAGS = -conditional\nendif\nUSER_OBJS := $(FLAGS)\nFLAGS := -later\n%build_module modname=kernel\n";
    let result = capture(source, 6, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);

    let replaced = "FLAGS = -base\nifeq ($(UNKNOWN),1)\nFLAGS = -conditional\nendif\nUSER_OBJS := $(FLAGS)\nFLAGS := -later\nUSER_OBJS := replacement.o\n%build_module modname=kernel\n";
    let result = capture(replaced, 7, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_objects).1, ["replacement.o"]);
}

#[test]
fn unresolved_include_taint_survives_freezing_but_literal_reset_clears_it() {
    let (_temp, root, dirs, target) = fixture();
    let source = "-include $(SRCDIR)/config/local.mk\nUSER_OBJS := $(FLAGS)\nFLAGS = -later\n%build_module modname=kernel\n";
    let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);

    let replaced = "-include $(SRCDIR)/config/local.mk\nUSER_OBJS := $(FLAGS)\nFLAGS = -later\nUSER_OBJS := replacement.o\n%build_module modname=kernel\n";
    let result = capture(replaced, 4, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_objects).1, ["replacement.o"]);

    let literal = "-include $(SRCDIR)/config/local.mk\nUSER_OBJS := literal.o\n%build_module modname=kernel\n";
    let result = capture(literal, 2, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_objects).1, ["literal.o"]);
}

#[test]
fn include_taint_flows_through_recursive_values_and_simple_appends() {
    let (_temp, root, dirs, target) = fixture();
    let recursive = "-include $(SRCDIR)/config/local.mk\nINTERMEDIATE = $(FLAGS)\nUSER_OBJS := $(INTERMEDIATE)\nFLAGS = -later\n%build_module modname=kernel\n";
    let result = capture(recursive, 4, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);

    let appended = "-include $(SRCDIR)/config/local.mk\nUSER_OBJS := base.o\nUSER_OBJS += $(FLAGS)\nFLAGS = -later\n%build_module modname=kernel\n";
    let result = capture(appended, 4, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);
}

#[test]
fn append_and_optional_set_do_not_clear_an_unknown_assignment() {
    let (_temp, root, dirs, target) = fixture();
    let source = "ifeq ($(UNKNOWN),1)\nUSER_OBJS = conditional.o\nendif\nUSER_OBJS += suffix.o\n%build_module modname=kernel\n";
    let result = capture(source, 4, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);

    let source = "ifeq ($(UNKNOWN),1)\nUSER_OBJS = conditional.o\nendif\nUSER_OBJS ?= fallback.o\n%build_module modname=kernel\n";
    let result = capture(source, 4, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);
}

#[test]
fn missing_required_include_is_unknown_but_missing_optional_include_is_empty() {
    let (_temp, root, dirs, target) = fixture();
    let required = "include $(SRCDIR)/arch/pc/kernel/make.opts\n%build_module modname=kernel\n";
    let result = capture(required, 1, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);
    unresolved(&result.user_ldflags);

    let optional = "-include $(SRCDIR)/arch/pc/kernel/make.opts\n%build_module modname=kernel\n";
    let result = capture(optional, 1, "kernel", None, &root, &dirs, &target);
    known_empty(&result.user_objects);
    known_empty(&result.user_ldflags);
}

#[test]
fn unresolved_optional_include_is_unknown_and_reset_can_clear_it() {
    let (_temp, root, dirs, target) = fixture();
    let unresolved_source =
        "-include $(UNKNOWN_OPTS_ROOT)/make.opts\n%build_module modname=kernel\n";
    let result = capture(unresolved_source, 1, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);

    let reset =
        "-include $(UNKNOWN_OPTS_ROOT)/make.opts\nUSER_OBJS :=\n%build_module modname=kernel\n";
    let result = capture(reset, 2, "kernel", None, &root, &dirs, &target);
    known_empty(&result.user_objects);
}

#[test]
fn unresolved_arbitrary_include_expression_cannot_become_known_empty() {
    let (_temp, root, dirs, target) = fixture();
    let source = "-include $(UNKNOWN_FRAGMENT)\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);
    unresolved(&result.user_ldflags);
}

#[test]
fn non_makeopts_fragments_need_explicit_caller_binding() {
    let (_temp, root, dirs, target) = fixture();
    let source = "include $(SRCDIR)/config/make.cfg\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_objects);

    let source = "-include $(TOP)/config/make.cfg\n%build_module modname=kernel\n";
    let result = capture_kobj_scoped_inputs_with_known_source_includes(
        source,
        1,
        "kernel",
        None,
        &root,
        Path::new("rom/kernel/mmakefile.src"),
        &dirs,
        &target,
        &["$(TOP)/config/make.cfg".to_owned()],
    );
    known_empty(&result.user_objects);
}

#[test]
fn mixed_makeopts_and_non_makeopts_include_list_is_unresolved() {
    let (_temp, root, dirs, target) = fixture();
    fs::write(
        root.join("arch/pc/kernel/make.opts"),
        "USER_LDFLAGS := -from-options\n",
    )
    .expect("write options");
    let source = "-include $(SRCDIR)/arch/pc/kernel/make.opts $(SRCDIR)/config/local.mk\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_ldflags);
}

#[test]
fn target_context_selects_make_opts_conditionals() {
    let (_temp, root, dirs, target) = fixture();
    fs::write(
        root.join("arch/pc/kernel/make.opts"),
        "ifeq ($(AROS_TARGET_CPU),x86_64)\nUSER_LDFLAGS += -cpu\nelse\nUSER_LDFLAGS += -other\nendif\n",
    )
    .expect("write options");
    let source = "-include $(SRCDIR)/arch/$(ARCH)/kernel/make.opts\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_ldflags).1, ["-cpu"]);
}

#[test]
fn defname_libs_uses_the_computed_flavoured_name() {
    let (_temp, root, dirs, target) = fixture();
    let source = "kernel_LIBS := wrong.a\nkernel_debug_LIBS := right.a\n%build_module modname=kernel flavour=debug\n";
    let result = capture(source, 2, "kernel", Some("debug"), &root, &dirs, &target);
    assert_eq!(result.defname, "kernel_debug");
    assert_eq!(exact(&result.defname_libs).1, ["right.a"]);

    let source =
        "kernel_LIBS := correct.a\nkernel_debug_LIBS := wrong.a\n%build_module modname=kernel\n";
    let result = capture(source, 2, "kernel", None, &root, &dirs, &target);
    assert_eq!(result.defname, "kernel");
    assert_eq!(exact(&result.defname_libs).1, ["correct.a"]);
}

#[test]
fn additional_kobj_link_variables_capture_exact_empty_and_unresolved_states() {
    let (_temp, root, dirs, target) = fixture();
    let source = "KOBJ_LDFLAGS := -Wl,--gc-sections -Wl,--gc-sections\nKERNEL_KOBJ_LDSCRIPT := kernel.ld\nFUNCINSTR_LIBS := instr.a instr.a\n%build_module modname=kernel\n";
    let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
    assert_eq!(
        exact(&result.kobj_ldflags).1,
        ["-Wl,--gc-sections", "-Wl,--gc-sections"]
    );
    assert_eq!(exact(&result.kernel_kobj_ldscript).1, ["kernel.ld"]);
    assert_eq!(exact(&result.funcinstr_libs).1, ["instr.a", "instr.a"]);
    known_empty(&result.use_libs);

    let empty = capture(
        "%build_module modname=kernel\n",
        0,
        "kernel",
        None,
        &root,
        &dirs,
        &target,
    );
    unresolved(&empty.kobj_ldflags);
    unresolved(&empty.kernel_kobj_ldscript);
    unresolved(&empty.funcinstr_libs);

    let conditional = "ifeq ($(UNBOUND_FEATURE),1)\nFUNCINSTR_LIBS := optional.a\nendif\n%build_module modname=kernel\n";
    let result = capture(conditional, 3, "kernel", None, &root, &dirs, &target);
    unresolved(&result.funcinstr_libs);

    let included = "-include $(SRCDIR)/config/local.mk\nKERNEL_KOBJ_LDSCRIPT := $(LDSCRIPT)\nLDSCRIPT = later.ld\n%build_module modname=kernel\n";
    let result = capture(included, 3, "kernel", None, &root, &dirs, &target);
    unresolved(&result.kernel_kobj_ldscript);
}

#[test]
fn required_global_link_inputs_distinguish_absent_from_explicit_empty() {
    let (_temp, root, dirs, target) = fixture();
    let absent = capture(
        "%build_module modname=kernel\n",
        0,
        "kernel",
        None,
        &root,
        &dirs,
        &target,
    );
    for value in [
        &absent.kobj_ldflags,
        &absent.kernel_kobj_ldscript,
        &absent.funcinstr_libs,
    ] {
        let ScopedMakeWords::Unresolved { reason, .. } = value else {
            panic!("absent global input must remain unresolved: {value:?}");
        };
        assert!(
            reason.contains("no source or target definition"),
            "{reason}"
        );
    }

    let explicit_empty = "KOBJ_LDFLAGS :=\nKERNEL_KOBJ_LDSCRIPT :=\nFUNCINSTR_LIBS :=\n%build_module modname=kernel\n";
    let result = capture(explicit_empty, 3, "kernel", None, &root, &dirs, &target);
    known_empty(&result.kobj_ldflags);
    known_empty(&result.kernel_kobj_ldscript);
    known_empty(&result.funcinstr_libs);

    let target_nonempty = TargetContext {
        make_variables: [
            ("KOBJ_LDFLAGS".to_owned(), "-target-flag".to_owned()),
            ("KERNEL_KOBJ_LDSCRIPT".to_owned(), "target.ld".to_owned()),
            ("FUNCINSTR_LIBS".to_owned(), "target-instr.a".to_owned()),
        ]
        .into_iter()
        .collect(),
        ..target
    };
    let result = capture(
        "%build_module modname=kernel\n",
        0,
        "kernel",
        None,
        &root,
        &dirs,
        &target_nonempty,
    );
    assert_eq!(exact(&result.kobj_ldflags).1, ["-target-flag"]);
    assert_eq!(exact(&result.kernel_kobj_ldscript).1, ["target.ld"]);
    assert_eq!(exact(&result.funcinstr_libs).1, ["target-instr.a"]);

    let result = capture(
        "KOBJ_LDFLAGS :=\nKERNEL_KOBJ_LDSCRIPT :=\nFUNCINSTR_LIBS :=\n%build_module modname=kernel\n",
        3,
        "kernel",
        None,
        &root,
        &dirs,
        &target_nonempty,
    );
    known_empty(&result.kobj_ldflags);
    known_empty(&result.kernel_kobj_ldscript);
    known_empty(&result.funcinstr_libs);
}

#[test]
fn macro_uselibs_preserves_order_duplicates_and_only_guards_references() {
    let (_temp, root, dirs, target) = fixture();
    let source = "LOCAL_LIBS := -lfirst -lsecond\n%build_module modname=kernel\n";
    let result = capture_with_uselibs(
        source,
        1,
        "kernel",
        Some("$(LOCAL_LIBS) -lrepeat -lrepeat"),
        &root,
        &dirs,
        &target,
    );
    assert_eq!(
        exact(&result.use_libs).1,
        ["-lfirst", "-lsecond", "-lrepeat", "-lrepeat"]
    );

    let unresolved_include = "-include $(SRCDIR)/config/unbound.mk\n%build_module modname=kernel\n";
    let absent = capture_with_uselibs(unresolved_include, 1, "kernel", None, &root, &dirs, &target);
    known_empty(&absent.use_libs);

    let literal = capture_with_uselibs(
        unresolved_include,
        1,
        "kernel",
        Some("-lliteral -lliteral"),
        &root,
        &dirs,
        &target,
    );
    assert_eq!(exact(&literal.use_libs).1, ["-lliteral", "-lliteral"]);

    let referenced = capture_with_uselibs(
        unresolved_include,
        1,
        "kernel",
        Some("$(LIBS_FROM_CONFIG)"),
        &root,
        &dirs,
        &target,
    );
    unresolved(&referenced.use_libs);

    let conditional = "ifeq ($(UNBOUND_FEATURE),1)\nMACRO_LIBS := -conditional\nendif\n%build_module modname=kernel\n";
    let referenced = capture_with_uselibs(
        conditional,
        3,
        "kernel",
        Some("$(MACRO_LIBS)"),
        &root,
        &dirs,
        &target,
    );
    unresolved(&referenced.use_libs);
}

#[test]
fn function_instrumentation_uses_macro_or_source_default_without_guessing() {
    let (_temp, root, dirs, target) = fixture();
    let source = "%build_module modname=kernel\n";
    let explicit_no = capture_with_funcinstr(source, 0, Some("no"), &root, &dirs, &target);
    assert_eq!(exact(&explicit_no.function_instrumentation).1, ["no"]);
    let explicit_yes = capture_with_funcinstr(source, 0, Some("yes"), &root, &dirs, &target);
    assert_eq!(exact(&explicit_yes.function_instrumentation).1, ["yes"]);

    let source_default = "TARGET_FUNCINSTR := yes\n%build_module modname=kernel\n";
    let defaulted = capture_with_funcinstr(source_default, 1, None, &root, &dirs, &target);
    assert_eq!(exact(&defaulted.function_instrumentation).1, ["yes"]);

    let unbound = capture_with_funcinstr(source, 0, None, &root, &dirs, &target);
    unresolved(&unbound.function_instrumentation);

    let explicit_unbound = capture_with_funcinstr(
        source,
        0,
        Some("$(UNKNOWN_SELECTOR)"),
        &root,
        &dirs,
        &target,
    );
    unresolved(&explicit_unbound.function_instrumentation);

    let explicit_unbound_target_ref = capture_with_funcinstr(
        source,
        0,
        Some("$(TARGET_FUNCINSTR)"),
        &root,
        &dirs,
        &target,
    );
    unresolved(&explicit_unbound_target_ref.function_instrumentation);

    let indirect_unbound_source = "SELECTOR := $(UNKNOWN_SELECTOR)\n%build_module modname=kernel\n";
    let indirect_unbound = capture_with_funcinstr(
        indirect_unbound_source,
        1,
        Some("$(SELECTOR)"),
        &root,
        &dirs,
        &target,
    );
    unresolved(&indirect_unbound.function_instrumentation);

    let bound_source = "SELECTOR := yes\n%build_module modname=kernel\n";
    let bound = capture_with_funcinstr(bound_source, 1, Some("$(SELECTOR)"), &root, &dirs, &target);
    assert_eq!(exact(&bound.function_instrumentation).1, ["yes"]);

    let mut bound_target = target.clone();
    bound_target
        .make_variables
        .insert("TARGET_FUNCINSTR".to_owned(), "no".to_owned());
    let bound_default = capture_with_funcinstr(source, 0, None, &root, &dirs, &bound_target);
    assert_eq!(exact(&bound_default.function_instrumentation).1, ["no"]);
    let bound_explicit_target_ref = capture_with_funcinstr(
        source,
        0,
        Some("$(TARGET_FUNCINSTR)"),
        &root,
        &dirs,
        &bound_target,
    );
    assert_eq!(
        exact(&bound_explicit_target_ref.function_instrumentation).1,
        ["no"]
    );

    let indirect_target = TargetContext {
        make_variables: std::collections::BTreeMap::from([(
            "SELECTOR".to_owned(),
            "$(UNKNOWN_SELECTOR)".to_owned(),
        )]),
        ..target.clone()
    };
    let indirect_target = capture_with_funcinstr(
        source,
        0,
        Some("$(SELECTOR)"),
        &root,
        &dirs,
        &indirect_target,
    );
    unresolved(&indirect_target.function_instrumentation);

    let conditional =
        "ifeq ($(UNKNOWN),1)\nTARGET_FUNCINSTR := yes\nendif\n%build_module modname=kernel\n";
    let referenced = capture_with_funcinstr(
        conditional,
        3,
        Some("$(TARGET_FUNCINSTR)"),
        &root,
        &dirs,
        &target,
    );
    unresolved(&referenced.function_instrumentation);
}

#[test]
fn undefined_variables_are_make_empty_only_in_a_complete_source_view() {
    let (_temp, root, dirs, target) = fixture();
    let source = "USER_LDFLAGS = $(NOT_DEFINED_HERE) -static\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.user_ldflags).1, ["-static"]);

    let source = "%build_module modname=kernel\n";
    let result = capture(source, 0, "kernel", None, &root, &dirs, &target);
    known_empty(&result.user_ldflags);
}

#[test]
fn unresolved_include_inside_known_false_branch_does_not_poison_scope() {
    let (_temp, root, dirs, mut target) = fixture();
    target
        .make_variables
        .insert("FEATURE".to_owned(), "0".to_owned());
    let source = "ifeq ($(FEATURE),1)\ninclude $(SRCDIR)/missing/make.opts\nendif\n%build_module modname=kernel\n";
    let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
    known_empty(&result.user_objects);
}

#[test]
fn native_config_binding_replaces_source_at_both_root_spellings_and_keeps_order() {
    let (_temp, root, dirs, target) = fixture();
    fs::create_dir_all(root.join("config")).expect("configuration directory");
    fs::write(
        root.join("config/original.cfg"),
        "USER_LDFLAGS += -original\n",
    )
    .expect("write original configuration");
    fs::write(
        root.join("config/native.mk"),
        "USER_LDFLAGS += -config\nKOBJ_LDFLAGS += -kobj\nKERNEL_KOBJ_LDSCRIPT := kernel.ld\nFUNCINSTR_LIBS += instr.a\n",
    )
    .expect("write native configuration");
    let target = TargetContext {
        make_include_bindings: BTreeMap::from([(
            "config/original.cfg".to_owned(),
            "config/native.mk".to_owned(),
        )]),
        ..target
    };
    let source = "-include $(SRCDIR)/config/original.cfg\nUSER_LDFLAGS += -local\n-include $(TOP)/config/original.cfg\n%build_module modname=kernel\n";
    let result = capture(source, 3, "kernel", None, &root, &dirs, &target);

    assert_eq!(
        exact(&result.user_ldflags).1,
        ["-config", "-local", "-config"]
    );
    assert_eq!(exact(&result.kobj_ldflags).1, ["-kobj", "-kobj"]);
    assert_eq!(exact(&result.kernel_kobj_ldscript).1, ["kernel.ld"]);
    assert_eq!(exact(&result.funcinstr_libs).1, ["instr.a", "instr.a"]);
    assert!(result.included_make_opts.is_empty());
    assert_eq!(
        result
            .included_configuration_files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["config/native.mk", "config/native.mk"]
    );
}

#[test]
fn missing_optional_original_with_a_binding_remains_unresolved() {
    let (_temp, root, dirs, target) = fixture();
    fs::create_dir_all(root.join("config")).expect("configuration directory");
    fs::write(root.join("config/native.mk"), "USER_LDFLAGS := -config\n")
        .expect("write native configuration");
    let target = TargetContext {
        make_include_bindings: BTreeMap::from([(
            "config/original.cfg".to_owned(),
            "config/native.mk".to_owned(),
        )]),
        ..target
    };
    let source = "-include $(SRCDIR)/config/original.cfg\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);

    unresolved(&result.user_ldflags);
    unresolved(&result.kobj_ldflags);
    assert!(result.included_configuration_files.is_empty());
}

#[test]
fn nested_unmapped_include_in_a_replacement_stays_unresolved() {
    let (_temp, root, dirs, target) = fixture();
    fs::create_dir_all(root.join("config")).expect("configuration directory");
    fs::write(
        root.join("config/original.cfg"),
        "ignored by native replacement\n",
    )
    .expect("write original configuration");
    fs::write(
        root.join("config/native.mk"),
        "-include $(SRCDIR)/config/nested.cfg\n",
    )
    .expect("write native configuration");
    let target = TargetContext {
        make_include_bindings: BTreeMap::from([(
            "config/original.cfg".to_owned(),
            "config/native.mk".to_owned(),
        )]),
        ..target
    };
    let source = "-include $(SRCDIR)/config/original.cfg\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);

    unresolved(&result.user_ldflags);
    assert_eq!(result.included_configuration_files.len(), 1);
}

#[test]
fn configuration_replacement_cycle_escape_and_symlink_are_rejected() {
    let (_temp, root, dirs, target) = fixture();
    fs::create_dir_all(root.join("config")).expect("configuration directory");
    fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
    fs::write(
        root.join("config/cycle.mk"),
        "-include $(SRCDIR)/config/original.cfg\n",
    )
    .expect("write cyclic replacement");
    let target = TargetContext {
        make_include_bindings: BTreeMap::from([(
            "config/original.cfg".to_owned(),
            "config/cycle.mk".to_owned(),
        )]),
        ..target
    };
    let source = "-include $(SRCDIR)/config/original.cfg\n%build_module modname=kernel\n";
    let cycle = capture(source, 1, "kernel", None, &root, &dirs, &target);
    unresolved(&cycle.user_ldflags);

    let (_temp, root, dirs, target) = fixture();
    fs::create_dir_all(root.join("config")).expect("configuration directory");
    fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
    let target = TargetContext {
        make_include_bindings: BTreeMap::from([(
            "config/original.cfg".to_owned(),
            "../outside.mk".to_owned(),
        )]),
        ..target
    };
    let escaped = capture(source, 1, "kernel", None, &root, &dirs, &target);
    unresolved(&escaped.user_ldflags);

    #[cfg(unix)]
    {
        let (_temp, root, dirs, target) = fixture();
        fs::create_dir_all(root.join("config")).expect("configuration directory");
        fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
        fs::write(root.join("config/real.mk"), "USER_LDFLAGS := -native\n")
            .expect("write replacement target");
        std::os::unix::fs::symlink(root.join("config/real.mk"), root.join("config/link.mk"))
            .expect("create replacement symlink");
        let target = TargetContext {
            make_include_bindings: BTreeMap::from([(
                "config/original.cfg".to_owned(),
                "config/link.mk".to_owned(),
            )]),
            ..target
        };
        let symlink = capture(source, 1, "kernel", None, &root, &dirs, &target);
        unresolved(&symlink.user_ldflags);
    }
}

#[test]
fn mixed_mapped_configuration_and_make_opts_include_is_refused_as_a_unit() {
    let (_temp, root, dirs, target) = fixture();
    fs::create_dir_all(root.join("config")).expect("configuration directory");
    fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
    fs::write(root.join("config/native.mk"), "USER_LDFLAGS := -native\n")
        .expect("write native configuration");
    fs::write(
        root.join("arch/pc/kernel/make.opts"),
        "USER_LDFLAGS := -make-opts\n",
    )
    .expect("write make.opts");
    let target = TargetContext {
        make_include_bindings: BTreeMap::from([(
            "config/original.cfg".to_owned(),
            "config/native.mk".to_owned(),
        )]),
        ..target
    };
    let source = "-include $(SRCDIR)/config/original.cfg $(SRCDIR)/arch/pc/kernel/make.opts\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);

    unresolved(&result.user_ldflags);
    assert!(result.included_make_opts.is_empty());
    assert!(result.included_configuration_files.is_empty());
}

#[test]
fn inactive_mapped_include_is_ignored_and_unsafe_original_paths_are_refused() {
    let (_temp, root, dirs, mut target) = fixture();
    target
        .make_variables
        .insert("FEATURE".to_owned(), "0".to_owned());
    target.make_include_bindings = BTreeMap::from([(
        "config/missing.cfg".to_owned(),
        "config/native.mk".to_owned(),
    )]);
    let inactive = "ifeq ($(FEATURE),1)\n-include $(SRCDIR)/config/missing.cfg\nendif\n%build_module modname=kernel\n";
    let result = capture(inactive, 3, "kernel", None, &root, &dirs, &target);
    known_empty(&result.user_ldflags);
    assert!(result.included_configuration_files.is_empty());

    fs::create_dir_all(root.join("config")).expect("configuration directory");
    fs::write(root.join("config/original.cfg"), "ignored\n").expect("write original");
    fs::write(root.join("config/native.mk"), "USER_LDFLAGS := -native\n")
        .expect("write native configuration");
    target.make_include_bindings = BTreeMap::from([(
        "config/original.cfg".to_owned(),
        "config/native.mk".to_owned(),
    )]);
    let unsafe_source = "-include $(SRCDIR)/../config/original.cfg\n%build_module modname=kernel\n";
    let result = capture(unsafe_source, 1, "kernel", None, &root, &dirs, &target);
    unresolved(&result.user_ldflags);
    assert!(result.included_configuration_files.is_empty());
}

#[test]
fn undefine_is_declaration_scoped_known_empty_and_carries_source_provenance() {
    let (_temp, root, dirs, _) = fixture();
    let target = TargetContext {
        make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-target".to_owned())]),
        ..TargetContext::default()
    };
    let source = "KOBJ_LDFLAGS := -source\n%build_module modname=kernel\nundefine KOBJ_LDFLAGS\n%build_module modname=kernel\n";

    let before = capture(source, 1, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&before.kobj_ldflags).1, ["-source"]);

    let after = capture(source, 3, "kernel", None, &root, &dirs, &target);
    let ScopedMakeWords::KnownEmpty { raw, source } = &after.kobj_ldflags else {
        panic!(
            "undefine must be a proven empty source value: {:?}",
            after.kobj_ldflags
        );
    };
    assert_eq!(raw.as_deref(), Some(""));
    assert_eq!(
        source,
        &[super::KobjSourceRef {
            path: "rom/kernel/mmakefile.src".to_owned(),
            line: 3,
        }]
    );
}

#[test]
fn undefine_allows_set_if_unset_to_replace_a_target_default() {
    let (_temp, root, dirs, _) = fixture();
    let target = TargetContext {
        make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-target".to_owned())]),
        ..TargetContext::default()
    };
    let source = "undefine KOBJ_LDFLAGS\nKOBJ_LDFLAGS ?= -source\n%build_module modname=kernel\n";
    let result = capture(source, 2, "kernel", None, &root, &dirs, &target);

    assert_eq!(exact(&result.kobj_ldflags).1, ["-source"]);
}

#[test]
fn unknown_conditional_undefine_is_unresolved_but_inactive_undefine_is_ignored() {
    let (_temp, root, dirs, _) = fixture();
    let target = TargetContext {
        make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-target".to_owned())]),
        ..TargetContext::default()
    };
    let source =
        "ifeq ($(UNKNOWN_FEATURE),1)\nundefine KOBJ_LDFLAGS\nendif\n%build_module modname=kernel\n";
    let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
    unresolved(&result.kobj_ldflags);

    let target = TargetContext {
        make_variables: [
            ("KOBJ_LDFLAGS".to_owned(), "-target".to_owned()),
            ("FEATURE".to_owned(), "0".to_owned()),
        ]
        .into_iter()
        .collect(),
        ..TargetContext::default()
    };
    let source =
        "ifeq ($(FEATURE),1)\nundefine KOBJ_LDFLAGS\nendif\n%build_module modname=kernel\n";
    let result = capture(source, 3, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.kobj_ldflags).1, ["-target"]);
}

#[test]
fn assignments_after_undefine_rebind_with_recursive_append_or_simple_freeze() {
    let (_temp, root, dirs, target) = fixture();
    let recursive = "FLAGS := -early\nKOBJ_LDFLAGS := -before\nundefine KOBJ_LDFLAGS\nKOBJ_LDFLAGS += $(FLAGS)\nFLAGS := -late\n%build_module modname=kernel\n";
    let result = capture(recursive, 5, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.kobj_ldflags).1, ["-late"]);

    let simple = "FLAGS := -early\nundefine KOBJ_LDFLAGS\nKOBJ_LDFLAGS := $(FLAGS)\nFLAGS := -late\n%build_module modname=kernel\n";
    let result = capture(simple, 4, "kernel", None, &root, &dirs, &target);
    assert_eq!(exact(&result.kobj_ldflags).1, ["-early"]);
}

#[test]
fn configured_append_flavor_uncertainty_is_refused_and_source_resets_are_honored() {
    let (_temp, root, dirs, _) = fixture();
    let configured = TargetContext {
        make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-configured".to_owned())]),
        ..TargetContext::default()
    };
    let uncertain =
        "LATER := -early\nKOBJ_LDFLAGS += $(LATER)\nLATER := -late\n%build_module modname=kernel\n";
    let result = capture(uncertain, 3, "kernel", None, &root, &dirs, &configured);
    let ScopedMakeWords::Unresolved { reason, .. } = &result.kobj_ldflags else {
        panic!("configuration-only += must not guess the variable flavor");
    };
    assert!(reason.contains("flavor is unknown"));

    let literal = "KOBJ_LDFLAGS += -literal\n%build_module modname=kernel\n";
    let result = capture(literal, 1, "kernel", None, &root, &dirs, &configured);
    assert_eq!(exact(&result.kobj_ldflags).1, ["-configured", "-literal"]);

    let simple = "KOBJ_LDFLAGS := -source\nLATER := -early\nKOBJ_LDFLAGS += $(LATER)\nLATER := -late\n%build_module modname=kernel\n";
    let result = capture(simple, 4, "kernel", None, &root, &dirs, &configured);
    assert_eq!(exact(&result.kobj_ldflags).1, ["-source", "-early"]);

    let recursive = "KOBJ_LDFLAGS = -source\nLATER := -early\nKOBJ_LDFLAGS += $(LATER)\nLATER := -late\n%build_module modname=kernel\n";
    let result = capture(recursive, 4, "kernel", None, &root, &dirs, &configured);
    assert_eq!(exact(&result.kobj_ldflags).1, ["-source", "-late"]);

    let transitive = "LATER := -early\nFLAGS += $(LATER)\nKOBJ_LDFLAGS := $(FLAGS)\nLATER := -late\n%build_module modname=kernel\n";
    let transitive_context = TargetContext {
        make_variables: BTreeMap::from([("FLAGS".to_owned(), "-configured".to_owned())]),
        ..TargetContext::default()
    };
    let result = capture(
        transitive,
        4,
        "kernel",
        None,
        &root,
        &dirs,
        &transitive_context,
    );
    let ScopedMakeWords::Unresolved { reason, .. } = &result.kobj_ldflags else {
        panic!("simple assignments must retain flavor uncertainty from their inputs");
    };
    assert!(reason.contains("flavor"));

    let reset = "LATER := -early\nKOBJ_LDFLAGS += $(LATER)\nLATER := -late\nKOBJ_LDFLAGS := -reset\n%build_module modname=kernel\n";
    let result = capture(reset, 4, "kernel", None, &root, &dirs, &configured);
    assert_eq!(exact(&result.kobj_ldflags).1, ["-reset"]);

    let mut inactive_context = configured;
    inactive_context
        .make_variables
        .insert("FEATURE".to_owned(), "0".to_owned());
    let inactive =
        "ifeq ($(FEATURE),1)\nKOBJ_LDFLAGS += $(LATER)\nendif\n%build_module modname=kernel\n";
    let result = capture(inactive, 3, "kernel", None, &root, &dirs, &inactive_context);
    assert_eq!(exact(&result.kobj_ldflags).1, ["-configured"]);
}

#[test]
fn dynamic_undefine_syntax_does_not_fall_back_to_a_target_value() {
    let (_temp, root, dirs, _) = fixture();
    let target = TargetContext {
        make_variables: BTreeMap::from([("KOBJ_LDFLAGS".to_owned(), "-target".to_owned())]),
        ..TargetContext::default()
    };
    let source = "undefine $(DYNAMIC_NAME)\n%build_module modname=kernel\n";
    let result = capture(source, 1, "kernel", None, &root, &dirs, &target);

    unresolved(&result.kobj_ldflags);
}
