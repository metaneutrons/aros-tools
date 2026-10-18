use super::{evaluate_ordered_mesa_contract, OrderedMesaContract};
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, MakeExprContext, MakeExprError};
use crate::make_vars::collect_vars_with_context;
use crate::parser::TargetContext;
use std::collections::BTreeMap;
use std::path::Path;

const FIXTURE: &str = "\
MESA_BASEFLAGS := -DORDER=1 -UORDER -DORDER=1\n\
ifneq ($(CFLAGS_NO_BUILTIN_FFS),)\n\
MESA_BASEFLAGS += -DHAVE___BUILTIN_FFS\n\
endif\n\
ifeq ($(AROS_TARGET_CPU),arm)\n\
MESA_BASEFLAGS += -DUSE_ARM_ASM\n\
endif\n\
USER_CPPFLAGS := $(MESA_BASEFLAGS) -DNDEBUG\n\
USER_CFLAGS := -std=gnu11 -fno-strict-aliasing\n\
USER_CXXFLAGS :=\n\
USER_CPPFLAGS += -DGALLIUM_VC4\n\
GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)\n\
USER_CPPFLAGS += -DLATE_ASSIGNMENT\n";

fn arm_context(builtin_ffs_flag: Option<&str>) -> TargetContext {
    let mut make_variables = BTreeMap::from([(
        "TARGET_ISA_CFLAGS".to_owned(),
        "-marm -mfloat-abi=hard -march=armv7-a".to_owned(),
    )]);
    if let Some(value) = builtin_ffs_flag {
        make_variables.insert("CFLAGS_NO_BUILTIN_FFS".to_owned(), value.to_owned());
    }
    TargetContext {
        cpu: Some("arm".to_owned()),
        platform: Some("raspi".to_owned()),
        toolchain: Some("llvm".to_owned()),
        make_variables,
        ..TargetContext::default()
    }
}

fn evaluate_fixture(
    source: &str,
    target: &TargetContext,
) -> Result<OrderedMesaContract, MakeExprError> {
    let scope = collect_vars_with_context(source, target);
    let dirs = DirVars::default();
    let declaration_line = source.lines().count();
    let gca_line = source
        .lines()
        .position(|line| line.starts_with("GCA_ABI_FLAGS :="))
        .expect("fixture has the GCA immediate assignment");
    let declaration_context = MakeExprContext::new(
        &scope,
        &dirs,
        declaration_line,
        Path::new("."),
        Path::new("mesa-fixture"),
    );
    let gca_assignment_context = MakeExprContext::new(
        &scope,
        &dirs,
        gca_line,
        Path::new("."),
        Path::new("mesa-fixture"),
    );
    evaluate_ordered_mesa_contract(&declaration_context, &gca_assignment_context)
}

#[test]
fn preserves_order_duplicates_known_empty_and_gca_assignment_freeze() {
    // The explicit empty probe result selects the known-empty Mesa branch.
    let target = arm_context(Some(""));
    let contract = evaluate_fixture(FIXTURE, &target).unwrap();

    assert_eq!(
        contract.target_isa_cflags,
        ["-marm", "-mfloat-abi=hard", "-march=armv7-a"]
    );
    assert_eq!(
        contract.user_cppflags,
        [
            "-DORDER=1",
            "-UORDER",
            "-DORDER=1",
            "-DUSE_ARM_ASM",
            "-DNDEBUG",
            "-DGALLIUM_VC4",
            "-DLATE_ASSIGNMENT",
        ]
    );
    assert_eq!(contract.user_cflags, ["-std=gnu11", "-fno-strict-aliasing"]);
    assert!(contract.user_cxxflags.is_empty());
    assert!(!contract
        .user_cppflags
        .iter()
        .any(|flag| flag == "-DHAVE___BUILTIN_FFS"));
    assert_eq!(
        contract.gca_abi_flags,
        "-marm -mfloat-abi=hard -march=armv7-a -DORDER=1 -UORDER -DORDER=1 -DUSE_ARM_ASM -DNDEBUG -DGALLIUM_VC4"
    );

    let scope = collect_vars_with_context(FIXTURE, &target);
    let dirs = DirVars::default();
    let after_assignment = MakeExprContext::new(
        &scope,
        &dirs,
        FIXTURE.lines().count(),
        Path::new("."),
        Path::new("mesa-fixture"),
    );
    assert_eq!(
        evaluate_make_expr("$(GCA_ABI_FLAGS)", &after_assignment).unwrap(),
        contract.gca_abi_flags
    );
}

#[test]
fn nonempty_configured_probe_adds_only_its_source_conditioned_define() {
    let target = arm_context(Some("-fno-builtin-ffs"));
    let contract = evaluate_fixture(FIXTURE, &target).unwrap();

    assert_eq!(
        contract.user_cppflags,
        [
            "-DORDER=1",
            "-UORDER",
            "-DORDER=1",
            "-DHAVE___BUILTIN_FFS",
            "-DUSE_ARM_ASM",
            "-DNDEBUG",
            "-DGALLIUM_VC4",
            "-DLATE_ASSIGNMENT",
        ]
    );
    assert!(contract.gca_abi_flags.contains("-DHAVE___BUILTIN_FFS"));
}

#[test]
fn missing_configured_probe_or_component_is_not_assumed_empty() {
    let missing_probe = evaluate_fixture(FIXTURE, &arm_context(None)).unwrap_err();
    assert!(matches!(
        missing_probe,
        MakeExprError::UnsafeVariable { .. }
    ));

    let source_without_cxxflags = FIXTURE.replace("USER_CXXFLAGS :=\n", "");
    let missing_component =
        evaluate_fixture(&source_without_cxxflags, &arm_context(Some(""))).unwrap_err();
    assert!(matches!(
        missing_component,
        MakeExprError::UnresolvedVariables { names, .. }
            if names.iter().any(|name| name == "USER_CXXFLAGS")
    ));
}

#[test]
fn unsupported_flag_expressions_fail_the_entire_contract() {
    let source = FIXTURE.replace(
        "USER_CFLAGS := -std=gnu11 -fno-strict-aliasing",
        "USER_CFLAGS := $(shell printf unsafe)",
    );
    let error = evaluate_fixture(&source, &arm_context(Some(""))).unwrap_err();
    assert!(matches!(
        error,
        MakeExprError::UnsupportedFunction { name } if name == "shell"
    ));
}

#[test]
fn preserves_projection_hazard_as_source_data_for_the_later_boundary() {
    let source = "\
USER_CPPFLAGS := -DVALUE=one;two\n\
USER_CFLAGS :=\n\
USER_CXXFLAGS :=\n\
GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)\n";
    let contract = evaluate_fixture(source, &arm_context(Some(""))).unwrap();

    // This layer reports Make words without CMake list projection or
    // sanitization. A downstream CMake argument projection must validate this
    // semicolon-bearing word before treating it as one compile option.
    assert_eq!(contract.user_cppflags, ["-DVALUE=one;two"]);
    assert_eq!(
        contract.gca_abi_flags,
        "-marm -mfloat-abi=hard -march=armv7-a -DVALUE=one;two"
    );
}
