//! Unit tests for Make variable scoping and conditional evaluation.

use super::*;
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext};
use std::path::Path;

#[test]
fn closed_target_selectors_seed_internal_make_configuration() {
    let context = TargetContext {
        make_variables: [("CONFIG_ONLY".into(), "config-value".into())].into(),
        cpu: Some("arm".into()),
        platform: Some("raspi".into()),
        family: Some("amiga".into()),
        variant: Some("debug".into()),
        toolchain: Some("gnu".into()),
        cpu32: Some("1".into()),
        use_mmu: Some("1".into()),
        float_abi: Some("hard".into()),
        mesa_version: Some("3".into()),
        target_llvm_ver: Some("20".into()),
        target_llvm_runtimes_style: Some("per-target".into()),
        target_rust: Some("1".into()),
        target_rust_ver: Some("1.85".into()),
        ..TargetContext::default()
    };
    let scope = collect_vars_with_context("", &context);

    for (name, expected) in [
        ("CPU", "arm"),
        ("AROS_TARGET_CPU", "arm"),
        ("ARCH", "raspi"),
        ("AROS_TARGET_ARCH", "raspi"),
        // configure.in: a variant replaces the machine except on pc.
        ("AROS_TARGET_PLATFORM", "debug-arm"),
        ("FAMILY", "amiga"),
        ("AROS_TARGET_FAMILY", "amiga"),
        ("AROS_TARGET_VARIANT", "debug"),
        ("AROS_TOOLCHAIN", "gnu"),
        ("AROS_TARGET_CPU32", "1"),
        ("USE_MMU", "1"),
        ("GCC_CONFIG_FLOAT_ABI", "hard"),
        ("OPT_MESAGL", "3"),
        ("TARGET_LLVM_VER", "20"),
        ("TARGET_LLVM_RUNTIMES_STYLE", "per-target"),
        ("TARGET_RUST", "1"),
        ("TARGET_RUST_VER", "1.85"),
    ] {
        assert_eq!(
            scope.raw_at(name, usize::MAX).as_deref(),
            Some(expected),
            "{name}"
        );
    }
    assert_eq!(
        scope.raw_at("CONFIG_ONLY", usize::MAX).as_deref(),
        Some("config-value")
    );
}

#[test]
fn source_local_selector_assignment_overrides_only_its_alias() {
    let context = TargetContext {
        cpu: Some("arm".into()),
        ..TargetContext::default()
    };

    let cpu_scope = collect_vars_with_context("CPU := local-cpu\n", &context);
    assert_eq!(
        cpu_scope.raw_at("CPU", usize::MAX).as_deref(),
        Some("local-cpu")
    );
    assert_eq!(
        cpu_scope.raw_at("AROS_TARGET_CPU", usize::MAX).as_deref(),
        Some("arm")
    );

    let target_cpu_scope =
        collect_vars_with_context("AROS_TARGET_CPU := local-target-cpu\n", &context);
    assert_eq!(
        target_cpu_scope.raw_at("CPU", usize::MAX).as_deref(),
        Some("arm")
    );
    assert_eq!(
        target_cpu_scope
            .raw_at("AROS_TARGET_CPU", usize::MAX)
            .as_deref(),
        Some("local-target-cpu")
    );
}

#[test]
fn absent_selectors_stay_absent_and_unknown_or_opaque_locals_block_fallback() {
    let context = TargetContext {
        // Selector aliases are internal projections, not contract-map
        // values. A malformed direct caller cannot inject them here.
        make_variables: [
            ("CPU".into(), "untrusted-cpu".into()),
            ("AROS_TARGET_CPU".into(), "untrusted-target-cpu".into()),
        ]
        .into(),
        ..TargetContext::default()
    };
    let empty_scope = collect_vars_with_context("", &context);
    for selector in CLOSED_TARGET_MAKE_SELECTORS {
        assert_eq!(empty_scope.raw_at(selector, usize::MAX), None, "{selector}");
    }
    assert_eq!(empty_scope.raw_at("AROS_HOST_ARCH", usize::MAX), None);

    let temp = tempfile::tempdir().expect("temporary source root");
    let dirs = DirVars::load(temp.path());
    let known_context = TargetContext {
        cpu: Some("arm".into()),
        ..TargetContext::default()
    };
    let unresolved_conditional = "ifeq ($(UNRESOLVED_SELECTOR),yes)\nCPU := branch-cpu\nendif\n";
    let conditional_scope = collect_vars_with_context(unresolved_conditional, &known_context);
    assert!(conditional_scope.conditionally_assigned_before("CPU", usize::MAX));
    let conditional_expr = MakeExprContext::new(
        &conditional_scope,
        &dirs,
        usize::MAX,
        temp.path(),
        Path::new("."),
    );
    assert!(evaluate_make_expr("$(CPU)", &conditional_expr).is_err());

    let opaque_scope = collect_vars_with_context("define CPU\nopaque\nendef\n", &known_context);
    assert!(opaque_scope.conditionally_assigned_before("CPU", usize::MAX));
    let opaque_expr = MakeExprContext::new(
        &opaque_scope,
        &dirs,
        usize::MAX,
        temp.path(),
        Path::new("."),
    );
    assert!(evaluate_make_expr("$(CPU)", &opaque_expr).is_err());
}

#[test]
fn quoted_configured_empty_value_is_not_an_unquoted_empty_value() {
    // geninc.cfg.in keeps quotes around ENABLE_EXECSMP. GNU Make does
    // not remove those quotes from its variable value or equality RHS.
    let quoted =
        "EXECSMP=\"\"\nifneq ($(strip $(EXECSMP)),\"\")\nSMP := yes\nelse\nSMP := no\nendif\n";
    let (scope, _) = collect_vars_impl(quoted, Some(&TargetContext::default()));
    assert_eq!(scope.raw_at("SMP", usize::MAX).as_deref(), Some("no"));
    let unquoted = quoted.replacen("EXECSMP=\"\"", "EXECSMP=", 1);
    let (scope, _) = collect_vars_impl(&unquoted, Some(&TargetContext::default()));
    assert_eq!(scope.raw_at("SMP", usize::MAX).as_deref(), Some("yes"));
}

#[test]
fn make_error_guards_stop_the_proof_only_when_they_may_run() {
    let guarded = "B ?= d1001\nifeq ($(B),bad)\n$(error bad board)\nendif\nifeq ($(B),d1001)\nF := one\nelse\n$(error unsupported $(B))\nendif\n";
    let known = TargetContext {
        make_variables: [("B".into(), "d1001".into())].into(),
        ..TargetContext::default()
    };
    let (scope, _) = collect_vars_impl(guarded, Some(&known));
    assert_eq!(scope.raw_at("F", usize::MAX).as_deref(), Some("one"));

    let rejected = TargetContext {
        make_variables: [("B".into(), "bad".into())].into(),
        ..TargetContext::default()
    };
    let (scope, states) = collect_vars_impl(guarded, Some(&rejected));
    assert_eq!(scope.raw_at("F", usize::MAX), None);
    assert!(states[4..]
        .iter()
        .all(|state| *state == ConditionalTruth::Unknown));

    let unknown = "ifeq ($(UNSET_SELECTOR),x)\n$(error stop)\nendif\nF := two\n";
    let (scope, _) = collect_vars_impl(unknown, Some(&TargetContext::default()));
    assert_eq!(scope.raw_at("F", usize::MAX), None);

    assert!(is_make_error_directive("$(error bad)"));
    assert!(!is_make_error_directive("$(errors x)"));
    assert!(!is_make_error_directive("X := $(error bad)"));
}

#[test]
fn legacy_platform_follows_configure_variant_rule() {
    let context = |platform: &str, variant: Option<&str>| TargetContext {
        platform: Some(platform.into()),
        cpu: Some("riscv".into()),
        variant: variant.map(str::to_owned),
        ..TargetContext::default()
    };
    assert_eq!(
        context("esp32p4", Some("")).legacy_platform().as_deref(),
        Some("esp32p4-riscv")
    );
    assert_eq!(
        context("esp32p4", Some("smp")).legacy_platform().as_deref(),
        Some("smp-riscv")
    );
    assert_eq!(
        context("pc", Some("smp")).legacy_platform().as_deref(),
        Some("pc-riscv")
    );
    assert_eq!(context("esp32p4", None).legacy_platform(), None);
    assert_eq!(
        context("esp32p4", Some("smp"))
            .value_of("AROS_TARGET_PLATFORM")
            .as_deref(),
        Some("smp-riscv")
    );
}

#[test]
fn configured_smp_value_is_cut_at_its_make_comment() {
    // configure substitutes "#define __AROSEXEC_SMP__" into geninc.cfg.in.
    // GNU Make ends the value at '#', so EXECSMP is a single quote and
    // compiler/include selects its SMP execbase lane.
    let source = "EXECSMP=\"#define __AROSEXEC_SMP__\"\nifneq ($(strip $(EXECSMP)),\"\")\nSMP := yes\nelse\nSMP := no\nendif\n";
    let (scope, _) = collect_vars_impl(source, Some(&TargetContext::default()));
    assert_eq!(scope.raw_at("EXECSMP", usize::MAX).as_deref(), Some("\""));
    assert_eq!(scope.raw_at("SMP", usize::MAX).as_deref(), Some("yes"));
}

#[test]
fn directive_quotes_do_not_strip_quotes_from_expanded_variables() {
    let source = "VALUE=\"x\"\n";
    let context = TargetContext::default();
    let (scope, _) = collect_vars_impl(source, Some(&context));
    for (arguments, expected) in [
        ("($(VALUE),\"x\")", ConditionalTruth::True),
        ("($(VALUE),x)", ConditionalTruth::False),
        ("\"$(VALUE)\" 'x'", ConditionalTruth::False),
        ("'$(VALUE)' '\"x\"'", ConditionalTruth::True),
        ("\"x\" 'x'", ConditionalTruth::True),
    ] {
        assert_eq!(
            evaluate_conditional("ifeq", arguments, &scope, &context, usize::MAX),
            expected,
            "{arguments}"
        );
    }
}
#[test]
fn recipe_commands_cannot_change_make_conditionals_or_variable_scope() {
    let context = TargetContext::default();
    let source = "ifeq (0,1)\n\telse\n#MM false-consumer : copy-owner\n\tendif\nFILES := inactive\nendif\nFILES := genuine\n\tFILES := shell-data\n\tundefine FILES\n";
    let (scope, states) = collect_vars_impl(source, Some(&context));
    assert_eq!(states[2], ConditionalTruth::False);
    assert_eq!(states[4], ConditionalTruth::False);
    assert_eq!(
        scope.raw_at("FILES", usize::MAX).as_deref(),
        Some("genuine")
    );
    assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));

    let unconfigured = collect_vars(source);
    assert_eq!(
        unconfigured.raw_at("FILES", usize::MAX).as_deref(),
        Some("genuine")
    );
    let (forward, forward_states) =
        collect_vars_impl_with_forward_locals(source, Some(&context), true);
    assert_eq!(forward_states[2], ConditionalTruth::False);
    assert_eq!(
        forward.raw_at("FILES", usize::MAX).as_deref(),
        Some("genuine")
    );
}

#[test]
fn define_bodies_are_inert_across_value_path_and_line_state_scans() {
    let context = TargetContext {
        make_variables: [
            ("LOCAL".into(), "configured".into()),
            ("NESTED".into(), "configured-nested".into()),
        ]
        .into(),
        ..TargetContext::default()
    };
    let source = "FILES := before\n\
define TEMPLATE\n\
LOCAL := from-define\n\
# HIDDEN_COMMENT := from-comment\n\
\telse\n\
\tendif\n\
\tFILES := from-recipe\n\
ifeq (0,1)\n\
override export private define NESTED\n\
FILES := from-nested-define\n\
endef\n\
FILES := still-in-define\n\
endef\n\
FROZEN := $(LOCAL)\n\
FROZEN_NESTED := $(NESTED)\n\
FILES := outside\n";
    let outside_line = source
        .lines()
        .position(|line| line == "FILES := outside")
        .expect("outside assignment");
    let frozen_line = source
        .lines()
        .position(|line| line == "FROZEN := $(LOCAL)")
        .expect("frozen assignment");
    let nested_frozen_line = source
        .lines()
        .position(|line| line == "FROZEN_NESTED := $(NESTED)")
        .expect("nested frozen assignment");

    let (context_scope, context_states) = collect_vars_impl(source, Some(&context));
    let (forward_scope, forward_states) =
        collect_vars_impl_with_forward_locals(source, Some(&context), true);
    let (context_free_scope, context_free_states) = collect_vars_impl(source, None);

    for (scope, states) in [
        (&context_scope, &context_states),
        (&forward_scope, &forward_states),
        (&context_free_scope, &context_free_states),
    ] {
        assert_eq!(
            scope.raw_at("FILES", usize::MAX).as_deref(),
            Some("outside")
        );
        assert_eq!(states.len(), source.lines().count());
        assert_eq!(states[outside_line], ConditionalTruth::True);
        assert_eq!(states[frozen_line], ConditionalTruth::True);
        assert_eq!(states[nested_frozen_line], ConditionalTruth::True);
        assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));
    }
    assert_eq!(
        context_scope.path_raw_at("FROZEN", usize::MAX).as_deref(),
        Some("configured")
    );
    assert_eq!(
        forward_scope.path_raw_at("FROZEN", usize::MAX).as_deref(),
        Some("configured")
    );
    assert_eq!(
        context_scope
            .path_raw_at("FROZEN_NESTED", usize::MAX)
            .as_deref(),
        Some("configured-nested")
    );
    assert_eq!(
        forward_scope
            .path_raw_at("FROZEN_NESTED", usize::MAX)
            .as_deref(),
        Some("configured-nested")
    );
    assert_eq!(
        context_free_scope
            .path_raw_at("FROZEN", usize::MAX)
            .as_deref(),
        Some("$(LOCAL)")
    );
    assert_eq!(
        context_free_scope
            .path_raw_at("FROZEN_NESTED", usize::MAX)
            .as_deref(),
        Some("$(NESTED)")
    );
    assert!(!context_scope.is_known_local("LOCAL"));
    assert!(!forward_scope.is_known_local("LOCAL"));
    assert!(!forward_scope.is_known_local("HIDDEN_COMMENT"));
}

#[test]
fn definitions_in_false_or_unknown_branches_do_not_assign_or_mark_locals() {
    let context = TargetContext::default();
    let source = "ifeq (0,1)\n\
define FALSE_TEMPLATE\n\
FILES := from-false-definition\n\
FALSE_LOCAL := yes\n\
endef\n\
endif\n\
ifeq ($(UNKNOWN),enabled)\n\
define UNKNOWN_TEMPLATE\n\
FILES := from-unknown-definition\n\
UNKNOWN_LOCAL := yes\n\
endef\n\
endif\n\
FILES := outside\n";
    let (scope, states) = collect_vars_impl_with_forward_locals(source, Some(&context), true);
    let (context_free, context_free_states) = collect_vars_impl(source, None);
    let false_line = source
        .lines()
        .position(|line| line == "define FALSE_TEMPLATE")
        .expect("false define line");
    let unknown_line = source
        .lines()
        .position(|line| line == "define UNKNOWN_TEMPLATE")
        .expect("unknown define line");
    let outside_line = source
        .lines()
        .position(|line| line == "FILES := outside")
        .expect("outside assignment");

    assert_eq!(states[false_line], ConditionalTruth::False);
    assert_eq!(states[unknown_line], ConditionalTruth::Unknown);
    assert_eq!(states[outside_line], ConditionalTruth::True);
    assert_eq!(context_free_states[outside_line], ConditionalTruth::True);
    for scope in [&scope, &context_free] {
        assert_eq!(
            scope.raw_at("FILES", usize::MAX).as_deref(),
            Some("outside")
        );
        assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));
    }
    assert!(!scope.is_known_local("FALSE_LOCAL"));
    assert!(!scope.is_known_local("UNKNOWN_LOCAL"));
}

#[test]
fn malformed_define_bodies_remain_suppressed_through_eof() {
    let context = TargetContext::default();
    for source in [
        "define OPEN\nFILES := hidden\nLEAKED := hidden\n",
        "define MALFORMED_END\nFILES := hidden\nendef extra\nFILES := leaked\n",
    ] {
        let (scope, states) = collect_vars_impl_with_forward_locals(source, Some(&context), true);
        let (context_free, context_free_states) = collect_vars_impl(source, None);
        assert_eq!(states.len(), source.lines().count());
        assert_eq!(context_free_states.len(), source.lines().count());
        for scope in [&scope, &context_free] {
            assert_eq!(scope.raw_at("FILES", usize::MAX), None);
            assert_eq!(scope.raw_at("LEAKED", usize::MAX), None);
        }
        assert!(!scope.is_known_local("FILES"));
        assert!(!scope.is_known_local("LEAKED"));
    }
}

#[test]
fn active_define_headers_shadow_old_and_configured_values_until_replaced() {
    let context = TargetContext {
        make_variables: [("FILES".into(), "configured".into())].into(),
        ..TargetContext::default()
    };
    for (source, previous) in [
        (
            "FILES := previous\ndefine FILES\nfrom-body\nendef\nifdef FILES\nendif\n",
            "previous",
        ),
        (
            "FILES := previous\noverride export define FILES\nfrom-body\nendef\nifdef FILES\nendif\n",
            "previous",
        ),
        (
            "FILES := previous\noverride export private define FILES\nfrom-body\nendef\nifdef FILES\nendif\n",
            "previous",
        ),
        (
            "define FILES\nfrom-body\nendef\nifdef FILES\nendif\n",
            "configured",
        ),
    ] {
        let scope = collect_vars_with_context(source, &context);
        let ifdef_line = source
            .lines()
            .position(|line| line == "ifdef FILES")
            .expect("ifdef line");
        let define_line = source
            .lines()
            .position(|line| line.ends_with("define FILES"))
            .expect("define line");
        assert_eq!(
            scope.raw_at("FILES", define_line).as_deref(),
            Some(previous)
        );
        assert_eq!(scope.raw_at("FILES", usize::MAX), None);
        assert_eq!(scope.path_raw_at("FILES", usize::MAX), None);
        assert!(!scope.snapshot(usize::MAX).contains_key("FILES"));
        assert!(scope.conditionally_assigned_before("FILES", usize::MAX));
        assert_eq!(
            evaluate_conditional("ifdef", "FILES", &scope, &context, ifdef_line),
            ConditionalTruth::Unknown
        );
    }

    let inactive =
        "FILES := known\nifeq (0,1)\noverride export define FILES\nfrom-body\nendef\nendif\n";
    let inactive_scope = collect_vars_with_context(inactive, &context);
    assert_eq!(
        inactive_scope.raw_at("FILES", usize::MAX).as_deref(),
        Some("known")
    );
    assert!(!inactive_scope.conditionally_assigned_before("FILES", usize::MAX));

    let inactive_default =
        "ifeq (1,0)\noverride export private define FILES\nfrom-body\nendef\nendif\n";
    let inactive_default_scope = collect_vars_with_context(inactive_default, &context);
    assert_eq!(
        inactive_default_scope
            .raw_at("FILES", usize::MAX)
            .as_deref(),
        Some("configured")
    );
    assert!(!inactive_default_scope.conditionally_assigned_before("FILES", usize::MAX));

    let unknown = "ifeq ($(UNKNOWN),1)\ndefine FILES\nfrom-body\nendef\nendif\n";
    let unknown_scope = collect_vars_with_context(unknown, &context);
    assert_eq!(unknown_scope.raw_at("FILES", usize::MAX), None);
    assert!(unknown_scope.conditionally_assigned_before("FILES", usize::MAX));

    let replaced = "FILES := previous\ndefine FILES\nfrom-body\nendef\nFILES := replacement\n";
    let replaced_scope = collect_vars_with_context(replaced, &context);
    assert_eq!(
        replaced_scope.raw_at("FILES", usize::MAX).as_deref(),
        Some("replacement")
    );
    assert_eq!(
        replaced_scope.path_raw_at("FILES", usize::MAX).as_deref(),
        Some("replacement")
    );
    assert!(!replaced_scope.conditionally_assigned_before("FILES", usize::MAX));

    let alias_source = "FILES := previous\ndefine FILES\nfrom-body\nendef\nALIAS := $(FILES)\nFILES := replacement\nUSER_CPPFLAGS := $(ALIAS)\n";
    for forward_locals in [false, true] {
        let (alias_scope, _) =
            collect_vars_impl_with_forward_locals(alias_source, Some(&context), forward_locals);
        assert_eq!(
            alias_scope.raw_at("FILES", usize::MAX).as_deref(),
            Some("replacement")
        );
        assert_eq!(alias_scope.raw_at("ALIAS", usize::MAX), None);
        assert_eq!(alias_scope.raw_at("USER_CPPFLAGS", usize::MAX), None);
        assert!(alias_scope.conditionally_assigned_before("ALIAS", usize::MAX));
        assert!(alias_scope.conditionally_assigned_before("USER_CPPFLAGS", usize::MAX));
        let flags = crate::flags::collect_flags_at(&alias_scope, usize::MAX);
        assert!(flags.skipped.contains(&"$(USER_CPPFLAGS)".to_owned()));
    }

    let dynamic = "define $(DYNAMIC_NAME)\nvalue\nendef\n";
    let dynamic_scope = collect_vars_with_context(dynamic, &context);
    assert_eq!(dynamic_scope.raw_at("FILES", usize::MAX), None);
    assert!(dynamic_scope.conditionally_assigned_before("FILES", usize::MAX));
}

#[test]
fn unconditional_replacement_clears_conditional_guard_but_not_frozen_aliases() {
    let context = TargetContext::default();
    let replacement = "FILES := initial\n\
ifeq ($(UNKNOWN),enabled)\n\
FILES := conditional\n\
else\n\
FILES := alternative\n\
endif\n\
FILES := proven\n\
ALIAS := $(FILES)\n\
FILES := later\n";
    for (scope, _) in [
        collect_vars_impl(replacement, None),
        collect_vars_impl(replacement, Some(&context)),
        collect_vars_impl_with_forward_locals(replacement, Some(&context), true),
    ] {
        assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));
        assert!(!scope.path_is_conditional_at("FILES", usize::MAX));
        assert_eq!(scope.raw_at("FILES", usize::MAX).as_deref(), Some("later"));
        assert!(!scope.conditionally_assigned_before("ALIAS", usize::MAX));
        assert_eq!(scope.raw_at("ALIAS", usize::MAX).as_deref(), Some("proven"));
    }

    let frozen_alias = "FILES := initial\n\
ifeq ($(UNKNOWN),enabled)\n\
FILES := conditional\n\
endif\n\
ALIAS := $(FILES)\n\
FILES := proven\n\
USER_CPPFLAGS := $(ALIAS)\n";
    for (scope, _) in [
        collect_vars_impl(frozen_alias, None),
        collect_vars_impl(frozen_alias, Some(&context)),
        collect_vars_impl_with_forward_locals(frozen_alias, Some(&context), true),
    ] {
        assert!(!scope.conditionally_assigned_before("FILES", usize::MAX));
        assert_eq!(scope.raw_at("FILES", usize::MAX).as_deref(), Some("proven"));
        assert!(scope.conditionally_assigned_before("ALIAS", usize::MAX));
        assert_eq!(scope.raw_at("ALIAS", usize::MAX), None);
        assert!(scope.conditionally_assigned_before("USER_CPPFLAGS", usize::MAX));
        assert_eq!(scope.raw_at("USER_CPPFLAGS", usize::MAX), None);
        let flags = crate::flags::collect_flags_at(&scope, usize::MAX);
        assert!(flags.skipped.contains(&"$(USER_CPPFLAGS)".to_owned()));
        assert!(!flags.defines.contains(&"INITIAL".to_owned()));
    }

    let reset_flags = "USER_CPPFLAGS := -DINITIAL\n\
ifeq ($(UNKNOWN),enabled)\n\
USER_CPPFLAGS := -DOPTIONAL\n\
endif\n\
USER_CPPFLAGS := -DRESET\n";
    for (scope, _) in [
        collect_vars_impl(reset_flags, None),
        collect_vars_impl(reset_flags, Some(&context)),
        collect_vars_impl_with_forward_locals(reset_flags, Some(&context), true),
    ] {
        assert!(!scope.conditionally_assigned_before("USER_CPPFLAGS", usize::MAX));
        let flags = crate::flags::collect_flags_at(&scope, usize::MAX);
        assert!(flags.defines.contains(&"RESET".to_owned()));
        assert!(!flags.skipped.contains(&"$(USER_CPPFLAGS)".to_owned()));
    }
}

#[test]
fn later_reset_cannot_resolve_an_earlier_uncertain_ifdef() {
    let context = TargetContext {
        make_variables: [("FEATURE".into(), String::new())].into(),
        ..TargetContext::default()
    };
    let text = "ifeq ($(UNKNOWN),x)\nFEATURE := 1\nendif\nifdef FEATURE\nSELECTED := maybe\nendif\nFEATURE :=\n";
    let scope = collect_vars_with_context(text, &context);
    assert!(matches!(
        evaluate_conditional("ifdef", "FEATURE", &scope, &context, 3),
        ConditionalTruth::Unknown
    ));
    assert!(matches!(
        evaluate_conditional("ifdef", "FEATURE", &scope, &context, 7),
        ConditionalTruth::False
    ));
}

#[test]
fn ifeq_recursive_expansion_is_line_scoped_and_reset_aware() {
    let context = TargetContext {
        make_variables: [("FEATURE".into(), String::new())].into(),
        ..TargetContext::default()
    };
    let text = "ifeq ($(UNKNOWN),x)\nFEATURE := 1\nendif\nifeq ($(strip $(FEATURE)),1)\nendif\nFEATURE := 1\nifeq ($(strip $(FEATURE)),1)\nendif\nFEATURE := 0\nifeq ($(FEATURE),1)\n";
    let scope = collect_vars_with_context(text, &context);

    assert!(matches!(
        evaluate_conditional("ifeq", "($(strip $(FEATURE)),1)", &scope, &context, 3),
        ConditionalTruth::Unknown
    ));
    assert!(matches!(
        evaluate_conditional("ifeq", "($(strip $(FEATURE)),1)", &scope, &context, 6),
        ConditionalTruth::True
    ));
    assert!(matches!(
        evaluate_conditional("ifeq", "($(FEATURE),1)", &scope, &context, 9),
        ConditionalTruth::False
    ));
}

#[test]
fn first_line_path_assignment_freezes_configured_default_before_local_override() {
    let context = TargetContext {
        make_variables: [("CURRENT_DEVICE".into(), "timer".into())].into(),
        ..TargetContext::default()
    };
    let text = "FROZEN_DEVICE := $(CURRENT_DEVICE)\nCURRENT_DEVICE := unproven\n";
    let scope = collect_vars_with_context(text, &context);

    assert_eq!(
        scope.path_raw_at("FROZEN_DEVICE", 1).as_deref(),
        Some("timer")
    );
    assert_eq!(
        scope.path_raw_at("CURRENT_DEVICE", 1).as_deref(),
        Some("timer")
    );
    assert_eq!(
        scope.path_raw_at("CURRENT_DEVICE", 2).as_deref(),
        Some("unproven")
    );
}

#[test]
fn undefine_clears_a_source_value_and_allows_later_set_if_unset() {
    let scope =
        collect_vars("VALUE := before\nundefine VALUE\nVALUE ?= fallback\nVALUE = rebound\n");

    assert_eq!(scope.raw_at("VALUE", 1).as_deref(), Some("before"));
    assert_eq!(scope.raw_at("VALUE", 2).as_deref(), Some(""));
    assert!(!scope.snapshot(2).contains_key("VALUE"));
    assert_eq!(scope.raw_at("VALUE", 3).as_deref(), Some("fallback"));
    assert_eq!(scope.raw_at("VALUE", 4).as_deref(), Some("rebound"));
    assert_eq!(
        scope.snapshot(3).get("VALUE"),
        Some(&vec!["fallback".to_owned()])
    );
}

#[test]
fn undefine_masks_target_defaults_and_undefined_append_is_recursive() {
    let context = TargetContext {
        make_variables: [
            ("TARGET_DEFAULT".into(), "-target".into()),
            ("APPENDED".into(), "-configured".into()),
        ]
        .into(),
        ..TargetContext::default()
    };
    let text = "undefine TARGET_DEFAULT\nTARGET_DEFAULT ?= -source\nLATER := -early\nAPPENDED := -before\nundefine APPENDED\nAPPENDED += $(LATER)\nLATER := -late\n";
    let scope = collect_vars_with_context(text, &context);

    assert_eq!(scope.raw_at("TARGET_DEFAULT", 1).as_deref(), Some(""));
    assert_eq!(
        scope.raw_at("TARGET_DEFAULT", 2).as_deref(),
        Some("-source")
    );
    assert_eq!(scope.raw_at("APPENDED", 6).as_deref(), Some("$(LATER)"));
    assert_eq!(scope.raw_at("LATER", 6).as_deref(), Some("-early"));
    assert_eq!(scope.raw_at("APPENDED", 7).as_deref(), Some("$(LATER)"));
}

#[test]
fn undefine_is_empty_when_expanded_immediately_and_later_rebinding_is_separate() {
    let context = TargetContext {
        make_variables: [("VALUE".into(), "-target".into())].into(),
        ..TargetContext::default()
    };
    let text = "undefine VALUE\nFROZEN := $(VALUE)\nVALUE ?= -fallback\n";
    let scope = collect_vars_with_context(text, &context);

    assert_eq!(scope.raw_at("FROZEN", 3).as_deref(), Some(""));
    assert_eq!(scope.path_raw_at("FROZEN", 3).as_deref(), Some(""));
    assert_eq!(scope.raw_at("VALUE", 2).as_deref(), Some(""));
    assert_eq!(scope.raw_at("VALUE", 3).as_deref(), Some("-fallback"));

    let text = "undefine VALUE\nVALUE ?= -restored\nFROZEN := $(VALUE)\nVALUE := -later\n";
    let scope = collect_vars_with_context(text, &context);
    assert_eq!(scope.raw_at("FROZEN", 4).as_deref(), Some("-restored"));
    assert_eq!(scope.raw_at("VALUE", 3).as_deref(), Some("-restored"));
    assert_eq!(scope.raw_at("VALUE", 4).as_deref(), Some("-later"));
}

#[test]
fn configured_append_references_are_flavor_uncertain_but_literal_append_is_not() {
    let context = TargetContext {
        make_variables: [("VALUE".into(), "-configured".into())].into(),
        ..TargetContext::default()
    };
    let text = "LATER := -early\nVALUE += $(LATER)\nLATER := -late\n";
    let scope = collect_vars_with_context(text, &context);
    assert!(scope
        .flavor_uncertainty_reason_at("VALUE", 3)
        .is_some_and(|reason| reason.contains("configuration value")));

    let literal = collect_vars_with_context("VALUE += -literal\n", &context);
    assert_eq!(literal.flavor_uncertainty_reason_at("VALUE", 1), None);
    assert_eq!(
        literal.raw_at("VALUE", 1).as_deref(),
        Some("-configured -literal")
    );
}

#[test]
fn source_assignment_establishes_flavor_and_simple_freezes_uncertain_values() {
    let context = TargetContext {
        make_variables: [("VALUE".into(), "-configured".into())].into(),
        ..TargetContext::default()
    };
    let simple = "VALUE := -source\nLATER := -early\nVALUE += $(LATER)\nLATER := -late\n";
    let scope = collect_vars_with_context(simple, &context);
    assert_eq!(scope.raw_at("VALUE", 4).as_deref(), Some("-source -early"));
    assert_eq!(scope.flavor_uncertainty_reason_at("VALUE", 4), None);

    let recursive = "VALUE = -source\nLATER := -early\nVALUE += $(LATER)\nLATER := -late\n";
    let scope = collect_vars_with_context(recursive, &context);
    assert_eq!(
        scope.raw_at("VALUE", 4).as_deref(),
        Some("-source $(LATER)")
    );
    assert_eq!(scope.flavor_uncertainty_reason_at("VALUE", 4), None);

    let transitive = "LATER := -early\nVALUE += $(LATER)\nFROZEN := $(VALUE)\nLATER := -late\n";
    let scope = collect_vars_with_context(transitive, &context);
    assert!(scope
        .flavor_uncertainty_reason_at("FROZEN", 4)
        .is_some_and(|reason| reason.contains("freezes a value")));

    let reset = "LATER := -early\nVALUE += $(LATER)\nVALUE := -reset\n";
    let scope = collect_vars_with_context(reset, &context);
    assert_eq!(scope.flavor_uncertainty_reason_at("VALUE", 3), None);
    assert_eq!(scope.raw_at("VALUE", 3).as_deref(), Some("-reset"));
}

#[test]
fn unsupported_dynamic_undefine_forms_are_rejected() {
    assert_eq!(undefine_directive("undefine VALUE"), Ok(Some("VALUE")));
    assert!(undefine_directive("undefine $(NAME)").is_err());
    assert!(undefine_directive("undefine VALUE OTHER").is_err());
}
