//! Opt-in comparison against retained configured-source GNU Make measurements.
//! This evaluates source flag components, not a full SDK or compiler invocation.

#[path = "support/mesa_configured_components.rs"]
mod mesa_configured_components;

use aros_transpiler::{
    dirs::DirVars,
    make_expr::{evaluate_make_expr, evaluate_make_list, MakeExprContext, MakeExprError},
    make_vars::collect_vars_with_context,
    parser::join_continuations,
    TargetContext,
};
use mesa_configured_components::evaluate_ordered_mesa_contract;
use std::{collections::BTreeMap, fs, path::PathBuf};

const VC4: &str = "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium";

#[test]
#[ignore = "requires explicit source and retained configured ARM/AArch64 GNU Make oracle"]
fn actual_configured_source_components_equal_gnu_make() {
    let source = required_directory("AROS_TEST_MESA_SOURCE_ROOT");
    let evidence = required_directory("AROS_TEST_MESA_ORACLE_ROOT");
    let oracle: serde_json::Value = serde_json::from_slice(
        &fs::read(evidence.join("vc4-configured-flags/result.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(oracle["exit_code"], 0);
    let selected: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.join("configured-context/result.json")).unwrap())
            .unwrap();
    assert_eq!(selected["exit_code"], 0);
    let cpu = selected["values"]["AROS_TARGET_CPU"].as_str().unwrap();
    let platform = selected["values"]["AROS_TARGET_ARCH"].as_str().unwrap();
    assert!(matches!(cpu, "arm" | "aarch64"));
    assert_eq!(platform, "raspi");
    let manifest =
        aros_common::ArosToolchainManifest::load(&evidence.join("native/toolchain")).unwrap();
    assert_eq!(manifest.target_triple.split_once('-').unwrap().0, cpu);
    let mesa = fs::read_to_string(source.join("workbench/libs/mesa/mesa.cfg")).unwrap();
    let vc4 = fs::read_to_string(source.join(VC4).join("mmakefile.src")).unwrap();
    assert_eq!(
        aros_common::sha256_bytes(mesa.as_bytes()).as_str(),
        oracle["mesa_cfg_sha256"].as_str().unwrap()
    );
    assert_eq!(
        aros_common::sha256_bytes(vc4.as_bytes()).as_str(),
        oracle["source_mmake_sha256"].as_str().unwrap()
    );
    let generated = PathBuf::from(selected["values"]["GENDIR"].as_str().unwrap());
    assert!(generated.is_absolute() && generated.starts_with(evidence.join("configure-oracle")));
    let compiler_path = generated.join("config/compiler.cfg");
    let compiler = fs::read_to_string(compiler_path).unwrap();
    assert_eq!(
        aros_common::sha256_bytes(compiler.as_bytes()).as_str(),
        oracle["compiler_cfg_sha256"].as_str().unwrap()
    );
    // This generated file contains literal feature assignments. Reject duplicate
    // names; don't infer a missing feature from the selected compiler brand.
    let mut configured = BTreeMap::new();
    for line in compiler.lines() {
        if let Some((name, value)) = line.split_once(":=") {
            assert!(configured
                .insert(name.trim().to_owned(), value.trim().to_owned())
                .is_none());
        }
    }
    let values = oracle["values"].as_object().unwrap();
    let optional: serde_json::Value = serde_json::from_slice(
        &fs::read(evidence.join("additional-make-inputs/result.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(optional["original"]["exit_code"], 0);
    assert_eq!(optional["counter"]["exit_code"], 0);
    for name in ["MESA_COMPILEOPTIONS", "USER_CFLAGS", "USER_CXXFLAGS"] {
        assert_ne!(
            optional["original"]["values"][name].as_str().unwrap(),
            optional["counter"]["values"][name].as_str().unwrap(),
            "counter measurement did not change {name}"
        );
    }
    for (name, value) in values {
        assert_eq!(
            &selected["values"][name], value,
            "selected paths belong to a different configured context"
        );
        assert_eq!(
            &optional["original"]["values"][name], value,
            "additional measurement selected a different configured context"
        );
    }
    configured.insert(
        "MESA_COMPILEOPTIONS".into(),
        optional["original"]["values"]["MESA_COMPILEOPTIONS"]
            .as_str()
            .unwrap()
            .into(),
    );
    configured.insert(
        "TARGET_ISA_CFLAGS".into(),
        values["TARGET_ISA_CFLAGS"].as_str().unwrap().into(),
    );
    configured.insert(
        "PORTSDIR".into(),
        selected["values"]["PORTSDIR"].as_str().unwrap().into(),
    );
    let context = TargetContext {
        cpu: Some(cpu.into()),
        platform: Some(platform.into()),
        toolchain: Some("llvm".into()),
        mesa_version: Some("26.0.0".into()),
        make_variables: configured,
        ..TargetContext::default()
    };
    let assignment = "GCA_ABI_FLAGS := $(TARGET_ISA_CFLAGS) $(USER_CPPFLAGS)";
    assert_eq!(vc4.matches(assignment).count(), 1);
    let prefix = vc4.split_once(assignment).unwrap().0;
    let combined = format!("{mesa}\n{prefix}{assignment}\n");
    let joined = join_continuations(&combined);
    let scope = collect_vars_with_context(&joined, &context);
    let gca_line = joined
        .lines()
        .position(|line| line.starts_with("GCA_ABI_FLAGS :="))
        .unwrap();
    let dirs = DirVars::load(&source);
    let at_assignment =
        MakeExprContext::new(&scope, &dirs, gca_line, &source, std::path::Path::new(VC4));
    let components = evaluate_ordered_mesa_contract(&at_assignment, &at_assignment).unwrap();
    for (name, measured) in [
        ("TARGET_ISA_CFLAGS", components.target_isa_cflags),
        ("USER_CPPFLAGS", components.user_cppflags),
        ("USER_CFLAGS", components.user_cflags),
        ("USER_CXXFLAGS", components.user_cxxflags),
    ] {
        let expected = values[name]
            .as_str()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>();
        assert_eq!(measured, expected, "configured source component {name}");
    }
    assert_eq!(
        components.gca_abi_flags,
        values["GCA_ABI_FLAGS"].as_str().unwrap()
    );
    // The same source must react to a separately measured configuration
    // change, not merely agree with one retained constant output.
    let mut changed = context;
    changed.make_variables.insert(
        "MESA_COMPILEOPTIONS".into(),
        optional["counter"]["values"]["MESA_COMPILEOPTIONS"]
            .as_str()
            .unwrap()
            .into(),
    );
    let changed_scope = collect_vars_with_context(&joined, &changed);
    let changed_context = MakeExprContext::new(
        &changed_scope,
        &dirs,
        gca_line,
        &source,
        std::path::Path::new(VC4),
    );
    let counter = evaluate_ordered_mesa_contract(&changed_context, &changed_context).unwrap();
    assert_eq!(
        counter.user_cflags,
        optional["counter"]["values"]["USER_CFLAGS"]
            .as_str()
            .unwrap()
            .split_whitespace()
            .collect::<Vec<_>>()
    );
    assert!(counter
        .user_cflags
        .iter()
        .any(|flag| flag == "-DORACLE_COUNTER=1"));
    assert_eq!(counter.gca_abi_flags, components.gca_abi_flags);
}

fn required_directory(name: &str) -> PathBuf {
    let path = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("set {name}")));
    assert!(path.is_absolute() && path.is_dir(), "invalid {name}");
    path
}
