use aros_transpiler::{
    dirs::DirVars, parse_mmakefile_with_dirs_and_context, ModuleType, TargetContext,
};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

fn target_context(cpu: &str, platform: &str, float_abi: &str) -> TargetContext {
    TargetContext {
        cpu: Some(cpu.to_owned()),
        platform: Some(platform.to_owned()),
        family: Some(String::new()),
        variant: Some(String::new()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(if cpu == "x86_64" { "i386" } else { "" }.to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(float_abi.to_owned()),
    }
}

#[test]
fn softfloat_selects_one_complete_specialization_for_each_current_cpu() {
    let root = root();
    let dirs = DirVars::load(&root);
    let mmakefile = root.join("compiler/softfloat/mmakefile.src");
    let specialized_sources = [
        "softfloat_raiseFlags",
        "s_f16UIToCommonNaN",
        "s_commonNaNToF16UI",
        "s_propagateNaNF16UI",
        "s_f32UIToCommonNaN",
        "s_commonNaNToF32UI",
        "s_propagateNaNF32UI",
        "s_f64UIToCommonNaN",
        "s_commonNaNToF64UI",
        "s_propagateNaNF64UI",
        "extF80M_isSignalingNaN",
        "s_extF80MToCommonNaN",
        "s_commonNaNToExtF80M",
        "s_propagateNaNExtF80M",
        "f128M_isSignalingNaN",
        "s_f128MToCommonNaN",
        "s_commonNaNToF128M",
        "s_propagateNaNF128M",
    ];

    for (cpu, platform, float_abi, specialization) in [
        ("x86_64", "pc", "", "8086-SSE"),
        ("arm", "raspi", "hard", "ARM-VFPv2"),
        ("aarch64", "raspi", "", "ARM-VFPv2"),
    ] {
        let parsed = parse_mmakefile_with_dirs_and_context(
            &mmakefile,
            &root,
            &dirs,
            &target_context(cpu, platform, float_abi),
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse SoftFloat: {error:#}"));
        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "linklibs-softfloat")
            .unwrap_or_else(|| panic!("{cpu}: missing SoftFloat target: {:#?}", parsed.targets));

        assert_eq!(target.target_name, "softfloat", "{cpu}");
        assert_eq!(target.module_type, ModuleType::LinkLib, "{cpu}");
        let specialization_root =
            format!("${{AROS_PORTS_DIR}}/libsoftfloat/SoftFloat-3e/source/{specialization}");
        assert!(
            target
                .include_dirs
                .iter()
                .any(|path| path == &specialization_root),
            "{cpu}: missing specialization include: {:#?}",
            target.include_dirs
        );
        for source in specialized_sources {
            let expected = format!("{specialization_root}/{source}");
            assert!(
                target.source_files.iter().any(|path| path == &expected),
                "{cpu}: missing specialization source {expected}: {:#?}",
                target.source_files
            );
        }
        let specialization_count = target
            .source_files
            .iter()
            .filter(|path| path.starts_with(&format!("{specialization_root}/")))
            .count();
        assert_eq!(
            specialization_count,
            specialized_sources.len(),
            "{cpu}: duplicate or incomplete specialization source set"
        );
    }
}
