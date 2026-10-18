use std::{collections::BTreeMap, path::Path};

use aros_transpiler::{
    dirs::DirVars,
    evaluate_make_expr, evaluate_make_list,
    make_expr::{MakeExprContext, MakeExprError},
    make_vars::{collect_vars_with_context, VarScope},
    TargetContext,
};

const BUILTIN_SELECTORS: [(&str, &str); 11] = [
    ("CFLAGS_NO_BUILTIN_FFS", "-DHAVE___BUILTIN_FFS"),
    ("CFLAGS_NO_BUILTIN_FFSLL", "-DHAVE___BUILTIN_FFSLL"),
    ("CFLAGS_NO_BUILTIN_BSWAP32", "-DHAVE___BUILTIN_BSWAP32"),
    ("CFLAGS_NO_BUILTIN_BSWAP64", "-DHAVE___BUILTIN_BSWAP64"),
    ("CFLAGS_NO_BUILTIN_CLZ", "-DHAVE___BUILTIN_CLZ"),
    ("CFLAGS_NO_BUILTIN_CLZLL", "-DHAVE___BUILTIN_CLZLL"),
    ("CFLAGS_NO_BUILTIN_CTZ", "-DHAVE___BUILTIN_CTZ"),
    ("CFLAGS_NO_BUILTIN_EXPECT", "-DHAVE___BUILTIN_EXPECT"),
    ("CFLAGS_NO_BUILTIN_POPCOUNT", "-DHAVE___BUILTIN_POPCOUNT"),
    (
        "CFLAGS_NO_BUILTIN_POPCOUNTLL",
        "-DHAVE___BUILTIN_POPCOUNTLL",
    ),
    (
        "CFLAGS_NO_BUILTIN_UNREACHABLE",
        "-DHAVE___BUILTIN_UNREACHABLE",
    ),
];

// The CPU and eleven selector branches mirror workbench/libs/mesa/mesa.cfg in
// the AROS-NX source checkout. The base marker and selector values below are
// deterministic fixture data, not results from configure or a compiler probe.
const MESA_CONFIGURATION_FIXTURE: &str = r"
MESA_BASEFLAGS := -DMESA_FIXTURE_BASE
ifneq ($(filter m68k ppc,$(CPU)),)
MESA_BASEFLAGS += -DUTIL_ARCH_LITTLE_ENDIAN=0 -DUTIL_ARCH_BIG_ENDIAN=1
else
MESA_BASEFLAGS += -DUTIL_ARCH_LITTLE_ENDIAN=1 -DUTIL_ARCH_BIG_ENDIAN=0
endif
ifeq ($(filter x86_64 aarch64 riscv64,$(CPU)),)
MESA_BASEFLAGS += -DMISSING_64BIT_ATOMICS
endif
ifneq ($(CFLAGS_NO_BUILTIN_FFS),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_FFS
endif
ifneq ($(CFLAGS_NO_BUILTIN_FFSLL),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_FFSLL
endif
ifneq ($(CFLAGS_NO_BUILTIN_BSWAP32),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_BSWAP32
endif
ifneq ($(CFLAGS_NO_BUILTIN_BSWAP64),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_BSWAP64
endif
ifneq ($(CFLAGS_NO_BUILTIN_CLZ),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_CLZ
endif
ifneq ($(CFLAGS_NO_BUILTIN_CLZLL),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_CLZLL
endif
ifneq ($(CFLAGS_NO_BUILTIN_CTZ),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_CTZ
endif
ifneq ($(CFLAGS_NO_BUILTIN_EXPECT),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_EXPECT
endif
ifneq ($(CFLAGS_NO_BUILTIN_POPCOUNT),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_POPCOUNT
endif
ifneq ($(CFLAGS_NO_BUILTIN_POPCOUNTLL),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_POPCOUNTLL
endif
ifneq ($(CFLAGS_NO_BUILTIN_UNREACHABLE),)
MESA_BASEFLAGS += -DHAVE___BUILTIN_UNREACHABLE
endif
";

fn configured_context(
    cpu: &str,
    isa_flags: Option<&str>,
    active_selector: Option<&str>,
    missing_selector: Option<&str>,
) -> TargetContext {
    let mut make_variables = BUILTIN_SELECTORS
        .iter()
        .map(|(name, _)| ((*name).to_owned(), String::new()))
        .collect::<BTreeMap<_, _>>();
    if let Some(selector) = active_selector {
        make_variables.insert(selector.to_owned(), "fixture-nonempty".to_owned());
    }
    if let Some(selector) = missing_selector {
        make_variables.remove(selector);
    }
    if let Some(isa_flags) = isa_flags {
        make_variables.insert("TARGET_ISA_CFLAGS".to_owned(), isa_flags.to_owned());
    }

    TargetContext {
        cpu: Some(cpu.to_owned()),
        make_variables,
        ..TargetContext::default()
    }
}

fn make_expression_context<'a>(scope: &'a VarScope, dirs: &'a DirVars) -> MakeExprContext<'a> {
    MakeExprContext::new(
        scope,
        dirs,
        usize::MAX,
        Path::new("/mesa-fixture"),
        Path::new("."),
    )
    .without_filesystem()
}

fn evaluate_variable(scope: &VarScope, name: &str) -> Result<Vec<String>, MakeExprError> {
    let dirs = DirVars::default();
    evaluate_make_list(
        &format!("$(strip $({name}))"),
        &make_expression_context(scope, &dirs),
    )
}

fn arm_mesa_prefix() -> Vec<String> {
    [
        "-DMESA_FIXTURE_BASE",
        "-DUTIL_ARCH_LITTLE_ENDIAN=1",
        "-DUTIL_ARCH_BIG_ENDIAN=0",
        "-DMISSING_64BIT_ATOMICS",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[test]
fn configured_builtin_selectors_distinguish_empty_nonempty_and_missing() {
    let empty_context = configured_context("arm", None, None, None);
    let empty_scope = collect_vars_with_context(MESA_CONFIGURATION_FIXTURE, &empty_context);
    assert_eq!(
        evaluate_variable(&empty_scope, "MESA_BASEFLAGS").unwrap(),
        arm_mesa_prefix(),
        "all eleven configure selectors are explicitly known empty"
    );

    for (selector, define) in BUILTIN_SELECTORS {
        let nonempty_context = configured_context("arm", None, Some(selector), None);
        let nonempty_scope =
            collect_vars_with_context(MESA_CONFIGURATION_FIXTURE, &nonempty_context);
        let mut expected = arm_mesa_prefix();
        expected.push(define.to_owned());
        assert_eq!(
            evaluate_variable(&nonempty_scope, "MESA_BASEFLAGS").unwrap(),
            expected,
            "nonempty {selector} selects only its source branch"
        );

        let missing_context = configured_context("arm", None, None, Some(selector));
        let missing_scope = collect_vars_with_context(MESA_CONFIGURATION_FIXTURE, &missing_context);
        assert!(
            missing_scope.raw_at("MESA_BASEFLAGS", usize::MAX).is_some(),
            "the fixture has a known prefix before the unresolved {selector} branch"
        );
        assert!(
            matches!(
                evaluate_variable(&missing_scope, "MESA_BASEFLAGS"),
                Err(MakeExprError::UnsafeVariable { name, .. })
                    if name == "MESA_BASEFLAGS"
            ),
            "missing {selector} must reject the whole value, not return its known prefix"
        );
    }
}

#[test]
fn configured_arm_and_aarch64_isa_flags_are_frozen_into_ordered_gca_flags() {
    // This test-only assignment mirrors the GCA_ABI_FLAGS declaration form
    // under review. The literal target flags are fixture inputs, not claimed
    // configure/compiler-probe outputs from either architecture.
    const GCA_ASSIGNMENT_FIXTURE: &str = r"
USER_CPPFLAGS := -DORDER=first -DORDER=first -UORDER -fno-strict-aliasing -DORDER=last
GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)
TARGET_ISA_CFLAGS = -march=late-source-value
USER_CPPFLAGS = -DORDER=late-source-value
";

    for (cpu, isa_flags) in [
        ("arm", "-march=armv7-a -mfloat-abi=hard"),
        ("aarch64", "-march=armv8-a"),
    ] {
        let context = configured_context(cpu, Some(isa_flags), None, None);
        let scope = collect_vars_with_context(GCA_ASSIGNMENT_FIXTURE, &context);
        let expression = "$(strip   $(GCA_ABI_FLAGS)   )";
        let dirs = DirVars::default();
        let make_context = make_expression_context(&scope, &dirs);
        let raw = evaluate_make_expr(expression, &make_context).unwrap();
        let expected = format!(
            "{isa_flags} -DORDER=first -DORDER=first -UORDER -fno-strict-aliasing -DORDER=last"
        );
        assert_eq!(raw, expected, "$(strip ...) normalizes whitespace only");
        assert_eq!(
            evaluate_make_list(expression, &make_context).unwrap(),
            expected
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            "duplicates, -D/-U order, and compile options survive unchanged for {cpu}"
        );
    }
}

#[test]
fn unknown_branch_and_unsupported_function_reject_the_complete_expression() {
    let context = configured_context("arm", None, None, None);
    let unknown_branch = r"
FLAGS := -DKNOWN_PREFIX
ifeq ($(NOT_CONFIGURED_FEATURE),enabled)
FLAGS += -DUNKNOWN_TRUE_BRANCH
else
FLAGS += -DUNKNOWN_FALSE_BRANCH
endif
";
    let scope = collect_vars_with_context(unknown_branch, &context);
    assert!(scope.raw_at("FLAGS", usize::MAX).is_some());
    assert!(matches!(
        evaluate_variable(&scope, "FLAGS"),
        Err(MakeExprError::UnsafeVariable { name, .. }) if name == "FLAGS"
    ));

    let supported_prefix_scope = collect_vars_with_context("FLAGS := -DKNOWN_PREFIX\n", &context);
    let dirs = DirVars::default();
    let make_context = make_expression_context(&supported_prefix_scope, &dirs);
    assert!(matches!(
        evaluate_make_expr(
            "$(strip -DKNOWN_PREFIX $(shell forbidden))",
            &make_context,
        ),
        Err(MakeExprError::UnsupportedFunction { name }) if name == "shell"
    ));
}

#[test]
fn bound_transport_feeds_known_selectors_and_ordered_isa_into_make_evaluation() {
    use aros_common::{
        mesa_configuration::{
            MesaConfigurationIdentity, ValidatedMesaConfiguration, MESA_COMPILER_OPTION_VARIABLES,
        },
        sha256_bytes, ArosCompilerIdentity,
    };

    // Synthetic identities exercise the transport/evaluator boundary only;
    // this fixture neither runs configure nor authenticates a compiler probe.
    let identity = MesaConfigurationIdentity {
        target_profile: "arm-raspi".into(),
        target_triple: "arm-unknown-aros".into(),
        compiler: ArosCompilerIdentity::Llvm {
            version: "11.0.0".into(),
        },
        compiler_tree_sha256: sha256_bytes(b"synthetic compiler tree"),
        source_inputs_sha256: sha256_bytes(b"synthetic source configuration"),
        configure_invocation_sha256: sha256_bytes(b"synthetic configure invocation"),
    };
    let selectors = BUILTIN_SELECTORS
        .iter()
        .map(|(name, _)| ((*name).to_owned(), serde_json::Value::Bool(true)))
        .collect::<serde_json::Map<_, _>>();
    let mut compiler_options = MESA_COMPILER_OPTION_VARIABLES
        .iter()
        .map(|name| ((*name).to_owned(), Vec::<String>::new()))
        .collect::<BTreeMap<_, _>>();
    compiler_options.insert(
        "CFLAGS_GNU11".into(),
        vec![
            "-std=gnu11".into(),
            "-DORDER=1".into(),
            "-UORDER".into(),
            "-DORDER=1".into(),
        ],
    );
    let wire = serde_json::json!({
        "version": 1,
        "identity": identity,
        "builtin_selectors": selectors,
        "target_isa_flags": ["-marm", "-DORDER=1", "-UORDER", "-DORDER=1"],
        "compiler_options": compiler_options,
        "mesa_compile_options": ["-DOPTION_BUNDLE=1"],
    });
    let bytes = serde_json::to_vec(&wire).unwrap();
    let configured = ValidatedMesaConfiguration::parse_bound(&bytes, &identity).unwrap();
    let target = TargetContext {
        cpu: Some("arm".into()),
        make_variables: configured.make_variables(),
        ..TargetContext::default()
    };
    let input = format!(
        "{MESA_CONFIGURATION_FIXTURE}\n\
         USER_CPPFLAGS := $(MESA_BASEFLAGS)\n\
         USER_CFLAGS := $(CFLAGS_GNU11) $(CFLAGS_NO_MATH_ERRNO) $(MESA_COMPILEOPTIONS)\n\
         GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)\n"
    );
    let scope = collect_vars_with_context(&input, &target);
    assert_eq!(
        evaluate_variable(&scope, "USER_CFLAGS").unwrap(),
        [
            "-std=gnu11",
            "-DORDER=1",
            "-UORDER",
            "-DORDER=1",
            "-DOPTION_BUNDLE=1"
        ]
    );
    let mut expected = vec![
        "-marm".to_owned(),
        "-DORDER=1".to_owned(),
        "-UORDER".to_owned(),
        "-DORDER=1".to_owned(),
    ];
    expected.extend(arm_mesa_prefix());
    expected.extend(BUILTIN_SELECTORS.iter().map(|(_, flag)| (*flag).to_owned()));
    assert_eq!(
        evaluate_variable(&scope, "GCA_ABI_FLAGS").unwrap(),
        expected
    );

    let mut changed = identity;
    changed.configure_invocation_sha256 = sha256_bytes(b"different target settings");
    assert!(ValidatedMesaConfiguration::parse_bound(&bytes, &changed)
        .unwrap_err()
        .to_string()
        .contains("identity does not match"));
}
