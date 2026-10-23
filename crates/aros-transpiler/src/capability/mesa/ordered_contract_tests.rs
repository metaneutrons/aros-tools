#[path = "../../../tests/support/mesa_configured_components.rs"]
mod mesa_configured_components;

use super::{evaluate_ordered_mesa_compiler_options, OrderedMesaCompilerOptions};
use crate::dirs::DirVars;
use crate::make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError};
use crate::make_vars::collect_vars_with_context;
use crate::parser::TargetContext;
use mesa_configured_components::{evaluate_ordered_mesa_contract, OrderedMesaContract};
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

// Exact recursive formula shape from config/target.cfg.in and
// config/make.cfg.in. The CONFIG_* assignments below are synthetic
// configure-time snapshots; their -g/-gxx atoms are not extra source formulas.
// LLVM_NOWARN follows USER_CFLAGS in CFLAGS and does not enter CXXFLAGS.
const COMPILER_OPTIONS_FIXTURE: &str = "\
OPTIMIZATION_CFLAGS := -O2\n\
CONFIG_CFLAGS := -DCONFIG_C=1 -UCONFIG_C -g\n\
CONFIG_CXXFLAGS := -DCONFIG_CXX=1 -UCONFIG_CXX -gxx\n\
CONFIG_CPPFLAGS := -DCONFIG_CPP=1 -UCONFIG_CPP\n\
LLVM_NOWARN := -Wno-implicit-int -Wno-missing-field-initializers\n\
TARGET_CFLAGS = $(strip $(OPTIMIZATION_CFLAGS) $(CONFIG_CFLAGS) $(USER_CFLAGS))\n\
TARGET_CXXFLAGS = $(strip $(OPTIMIZATION_CFLAGS) $(CONFIG_CXXFLAGS) $(USER_CXXFLAGS))\n\
TARGET_CPPFLAGS = $(strip $(USER_INCLUDES) $(CONFIG_CPPFLAGS) $(USER_CPPFLAGS))\n\
CFLAGS = $(strip $(TARGET_ISA_CFLAGS) $(TARGET_CFLAGS) $(LLVM_NOWARN))\n\
CXXFLAGS = $(strip $(TARGET_ISA_CXXFLAGS) $(TARGET_CXXFLAGS))\n\
CPPFLAGS = $(strip $(TARGET_CPPFLAGS))\n\
USER_INCLUDES := -Iearly -Iearly\n\
USER_CFLAGS := -std=gnu11 -DORDER=1 -UORDER -DORDER=1\n\
USER_CXXFLAGS := -std=gnu++17 -DCXXORDER=1 -UCXXORDER -DCXXORDER=1\n\
USER_CPPFLAGS := -DUSER_CPP=1 -UUSER_CPP -DUSER_CPP=1\n\
GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)\n\
USER_CPPFLAGS += -DPOST_GCA=1\n\
USER_CFLAGS += -DPOST_CFLAGS=1\n\
USER_INCLUDES += -Ilater\n";

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

fn compiler_options_target() -> TargetContext {
    let mut target = arm_context(Some(""));
    target.make_variables.insert(
        "TARGET_ISA_CXXFLAGS".to_owned(),
        "-marm -mfloat-abi=hard -mabi=ilp32".to_owned(),
    );
    // These configured snapshots must not replace the recursive source
    // definitions above when local USER_* values are evaluated.
    target
        .make_variables
        .insert("TARGET_CFLAGS".to_owned(), "-frozen-target-c".to_owned());
    target.make_variables.insert(
        "TARGET_CXXFLAGS".to_owned(),
        "-frozen-target-cxx".to_owned(),
    );
    target.make_variables.insert(
        "TARGET_CPPFLAGS".to_owned(),
        "-frozen-target-cpp".to_owned(),
    );
    target
}

fn evaluate_compiler_options_fixture(
    source: &str,
    target: &TargetContext,
) -> Result<OrderedMesaCompilerOptions, MakeExprError> {
    let scope = collect_vars_with_context(source, target);
    let dirs = DirVars::default();
    let declaration_context = MakeExprContext::new(
        &scope,
        &dirs,
        source.lines().count(),
        Path::new("."),
        Path::new("mesa-compiler-options-fixture"),
    );
    let gca_line = source
        .lines()
        .position(|line| line.starts_with("GCA_ABI_FLAGS :="))
        .expect("fixture has the GCA immediate assignment");
    let gca_assignment_context = MakeExprContext::new(
        &scope,
        &dirs,
        gca_line + 1,
        Path::new("."),
        Path::new("mesa-compiler-options-fixture"),
    );
    evaluate_ordered_mesa_compiler_options(&declaration_context, &gca_assignment_context)
}

#[test]
fn gca_value_is_read_from_the_frozen_source_assignment_not_reconstructed() {
    let source = "CFLAGS := -DC=1\nCXXFLAGS := -DCXX=1\nCPPFLAGS :=\nUSER_CFLAGS :=\nUSER_CPPFLAGS := -DBEFORE\nUSER_INCLUDES :=\nTARGET_ISA_CFLAGS := -marm\nGCA_ABI_FLAGS := -DSOURCE=1 -USOURCE -DSOURCE=1\nUSER_CPPFLAGS := -DAFTER\n";
    let options = evaluate_compiler_options_fixture(source, &TargetContext::default()).unwrap();
    assert_eq!(options.gca_abi_flags, "-DSOURCE=1 -USOURCE -DSOURCE=1");
    assert!(!options.gca_abi_flags.contains("BEFORE"));
    assert!(!options.gca_abi_flags.contains("AFTER"));
    assert!(!options.gca_abi_flags.contains("-marm"));
}

#[test]
fn unselected_gca_is_empty_but_a_selected_missing_value_refuses() {
    let source = "CFLAGS := -DC=1\nCXXFLAGS := -DCXX=1\nCPPFLAGS :=\nUSER_CFLAGS :=\nUSER_CPPFLAGS := -DCOPIED_FORMULA_MUST_NOT_BE_USED\nUSER_INCLUDES :=\nTARGET_ISA_CFLAGS := -marm\n";
    let target = TargetContext::default();
    let scope = collect_vars_with_context(source, &target);
    let dirs = DirVars::default();
    let context = MakeExprContext::new(
        &scope,
        &dirs,
        source.lines().count(),
        Path::new("."),
        Path::new("fixture"),
    );
    assert!(evaluate_ordered_mesa_compiler_options(&context, &context).is_err());
    let unselected = super::evaluate_ordered_mesa_options(
        &context,
        None,
        "$(CFLAGS) $(CPPFLAGS)",
        "$(CXXFLAGS) $(CPPFLAGS)",
    )
    .unwrap();
    assert!(unselected.gca_abi_flags.is_empty());
    assert_eq!(unselected.c, ["-DC=1"]);
    assert_eq!(unselected.cxx, ["-DCXX=1"]);
}

#[test]
fn recipe_projection_places_lto_before_cflags_and_safety_before_cppflags() {
    let source = "TARGET_ISA_CFLAGS := -marm\nCFLAGS := -DC=1\nCXXFLAGS := -DCXX=1\nCPPFLAGS := -DCPP=1\nUSER_CFLAGS := -DUSER=1\nUSER_CPPFLAGS := -DGCA=1\nUSER_INCLUDES := -Iinclude\nGCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)\n";
    let mut target = TargetContext::default();
    for (key, value) in [
        ("TARGET_LTO", "yes"),
        ("LTO_CFLAGS", "-flto -flto"),
        ("SAFETY_CFLAGS", "-DSAFETY=1 -DSAFETY=1"),
    ] {
        target.make_variables.insert(key.into(), value.into());
    }
    let evaluate = |target: &TargetContext| {
        let scope = collect_vars_with_context(source, target);
        let dirs = DirVars::default();
        let c_reads = std::cell::Cell::new(0);
        let cxx_reads = std::cell::Cell::new(0);
        let lookup = |name: &str| {
            match name {
                "CFLAGS" => c_reads.set(c_reads.get() + 1),
                "CXXFLAGS" => cxx_reads.set(cxx_reads.get() + 1),
                _ => {}
            }
            None
        };
        let context = MakeExprContext::new(
            &scope,
            &dirs,
            source.lines().count(),
            Path::new("."),
            Path::new("fixture"),
        )
        .with_lookup(&lookup);
        let gca = MakeExprContext::new(&scope, &dirs, 8, Path::new("."), Path::new("fixture"));
        let result = super::evaluate_ordered_mesa_recipe_options(&context, Some(&gca));
        if result.is_ok() {
            // One final language projection each; only C is also used by
            // the separate bridge recipe. No discarded preliminary vectors.
            assert_eq!(c_reads.get(), 2);
            assert_eq!(cxx_reads.get(), 1);
        }
        result
    };
    let enabled = evaluate(&target).unwrap();
    assert_eq!(
        enabled.c,
        [
            "-flto",
            "-flto",
            "-DC=1",
            "-DSAFETY=1",
            "-DSAFETY=1",
            "-DCPP=1"
        ]
    );
    assert_eq!(
        enabled.cxx,
        [
            "-flto",
            "-flto",
            "-DCXX=1",
            "-DSAFETY=1",
            "-DSAFETY=1",
            "-DCPP=1"
        ]
    );
    for key in ["LTO_CFLAGS", "SAFETY_CFLAGS"] {
        let saved = target.make_variables.remove(key).unwrap();
        assert!(matches!(
            evaluate(&target).unwrap_err(),
            MakeExprError::UnresolvedVariables { names, .. } if names.iter().any(|name| name == key)
        ));
        target.make_variables.insert(key.into(), saved);
    }
    target
        .make_variables
        .insert("TARGET_LTO".into(), "no".into());
    let disabled = evaluate(&target).unwrap();
    assert_eq!(disabled.c, enabled.c[2..]);
    assert_eq!(disabled.vc4_bridge, enabled.vc4_bridge);
    assert_eq!(disabled.gca_abi_flags, enabled.gca_abi_flags);
    target
        .make_variables
        .insert("TARGET_LTO".into(), "guessed".into());
    assert!(evaluate(&target).is_err());
    target.make_variables.remove("TARGET_LTO");
    assert!(evaluate(&target).is_err());
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

#[test]
fn projects_c_cxx_and_vc4_bridge_in_source_order_and_freezes_gca_assignment() {
    let options =
        evaluate_compiler_options_fixture(COMPILER_OPTIONS_FIXTURE, &compiler_options_target())
            .unwrap();

    assert_eq!(
        options.c,
        [
            "-marm",
            "-mfloat-abi=hard",
            "-march=armv7-a",
            "-O2",
            "-DCONFIG_C=1",
            "-UCONFIG_C",
            "-g",
            "-std=gnu11",
            "-DORDER=1",
            "-UORDER",
            "-DORDER=1",
            "-DPOST_CFLAGS=1",
            "-Wno-implicit-int",
            "-Wno-missing-field-initializers",
            "-Iearly",
            "-Iearly",
            "-Ilater",
            "-DCONFIG_CPP=1",
            "-UCONFIG_CPP",
            "-DUSER_CPP=1",
            "-UUSER_CPP",
            "-DUSER_CPP=1",
            "-DPOST_GCA=1",
        ]
    );
    assert_eq!(
        options.cxx,
        [
            "-marm",
            "-mfloat-abi=hard",
            "-mabi=ilp32",
            "-O2",
            "-DCONFIG_CXX=1",
            "-UCONFIG_CXX",
            "-gxx",
            "-std=gnu++17",
            "-DCXXORDER=1",
            "-UCXXORDER",
            "-DCXXORDER=1",
            "-Iearly",
            "-Iearly",
            "-Ilater",
            "-DCONFIG_CPP=1",
            "-UCONFIG_CPP",
            "-DUSER_CPP=1",
            "-UUSER_CPP",
            "-DUSER_CPP=1",
            "-DPOST_GCA=1",
        ]
    );
    assert_eq!(
        options.vc4_bridge,
        [
            "-marm",
            "-mfloat-abi=hard",
            "-march=armv7-a",
            "-O2",
            "-DCONFIG_C=1",
            "-UCONFIG_C",
            "-g",
            "-std=gnu11",
            "-DORDER=1",
            "-UORDER",
            "-DORDER=1",
            "-DPOST_CFLAGS=1",
            "-Wno-implicit-int",
            "-Wno-missing-field-initializers",
            "-std=gnu11",
            "-DORDER=1",
            "-UORDER",
            "-DORDER=1",
            "-DPOST_CFLAGS=1",
            "-Iearly",
            "-Iearly",
            "-Ilater",
            "-DCONFIG_CPP=1",
            "-UCONFIG_CPP",
            "-DUSER_CPP=1",
            "-UUSER_CPP",
            "-DUSER_CPP=1",
            "-DPOST_GCA=1",
            "-DUSER_CPP=1",
            "-UUSER_CPP",
            "-DUSER_CPP=1",
            "-DPOST_GCA=1",
            "-Iearly",
            "-Iearly",
            "-Ilater",
        ]
    );
    assert_eq!(
        options.gca_abi_flags,
        "-marm -mfloat-abi=hard -march=armv7-a -DUSER_CPP=1 -UUSER_CPP -DUSER_CPP=1"
    );
}

#[test]
fn explicit_empty_compiler_inputs_remain_known_empty() {
    let source = COMPILER_OPTIONS_FIXTURE
        .replace(
            "USER_CXXFLAGS := -std=gnu++17 -DCXXORDER=1 -UCXXORDER -DCXXORDER=1\n",
            "USER_CXXFLAGS :=\n",
        )
        .replace("USER_INCLUDES := -Iearly -Iearly\n", "USER_INCLUDES :=\n")
        .replace("USER_INCLUDES += -Ilater\n", "");
    let options = evaluate_compiler_options_fixture(&source, &compiler_options_target()).unwrap();

    assert!(!options.c.contains(&"-Iearly".to_owned()));
    assert!(!options.cxx.contains(&"-Iearly".to_owned()));
    assert!(options.cxx.contains(&"-mabi=ilp32".to_owned()));
    assert!(options.cxx.contains(&"-DCONFIG_CPP=1".to_owned()));
    assert!(!options.cxx.contains(&"-std=gnu++17".to_owned()));
    assert!(!options.cxx.contains(&"-DCXXORDER=1".to_owned()));
}

#[test]
fn missing_recursive_inputs_are_rejected_instead_of_defaulted() {
    let missing_cpp_configuration =
        COMPILER_OPTIONS_FIXTURE.replace("CONFIG_CPPFLAGS := -DCONFIG_CPP=1 -UCONFIG_CPP\n", "");
    let error =
        evaluate_compiler_options_fixture(&missing_cpp_configuration, &compiler_options_target())
            .unwrap_err();
    assert!(matches!(
        error,
        MakeExprError::UnresolvedVariables { names, .. }
            if names.iter().any(|name| name == "CONFIG_CPPFLAGS")
    ));

    let mut missing_cxx_isa = compiler_options_target();
    missing_cxx_isa.make_variables.remove("TARGET_ISA_CXXFLAGS");
    let error =
        evaluate_compiler_options_fixture(COMPILER_OPTIONS_FIXTURE, &missing_cxx_isa).unwrap_err();
    assert!(matches!(
        error,
        MakeExprError::UnresolvedVariables { names, .. }
            if names.iter().any(|name| name == "TARGET_ISA_CXXFLAGS")
    ));
}
