mod common;

use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars,
    parse_mmakefile_with_dirs_and_context_and_fetches, TargetContext,
};

fn context(cpu: &str, platform: &str, float_abi: &str) -> TargetContext {
    TargetContext {
        cpu: Some(cpu.to_owned()),
        platform: Some(platform.to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(if cpu == "x86_64" { "i386" } else { "" }.to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(float_abi.to_owned()),
        mesa_version: Some("26.0.0".to_owned()),
        ..TargetContext::default()
    }
}

// These recipes still include the broad mesa.cfg fragment. Record the exact
// unsupported archive diagnostic instead of accepting a partial source list.
#[test]
fn mesa26_i915_and_softpipe_report_unresolved_archive_inventories() {
    let root = common::source_root();
    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let profile = context(cpu, platform, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        for (recipe, target_name) in [
            (
                "workbench/devs/monitors/IntelGMA/i915/mmakefile.src",
                "intelgma-linklibs-gallium_i915",
            ),
            (
                "workbench/hidds/softpipe/mmakefile.src",
                "linklibs-gallium_softpipe",
            ),
        ] {
            let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
                &root.join(recipe),
                &root,
                &DirVars::load(&root),
                &profile,
                &fetches,
            )
            .unwrap();
            assert!(
                parsed.capability_errors.is_empty(),
                "{cpu}/{recipe}: {parsed:#?}"
            );
            assert!(
                parsed.partial_source_lists.is_empty(),
                "{cpu}/{recipe}: {parsed:#?}"
            );
            assert!(
                parsed
                    .targets
                    .iter()
                    .all(|target| target.mmake_name != target_name),
                "{cpu}/{recipe}: unsupported archive was emitted"
            );
            if target_name == "linklibs-gallium_softpipe" {
                assert!(
                    parsed
                        .targets
                        .iter()
                        .any(|target| target.mmake_name == "hidd-softpipe"),
                    "{cpu}/{recipe}: the module declaration changed; review its archive edge"
                );
            }
            assert!(
                parsed.skipped_programs.iter().any(|reason| {
                    reason.contains(target_name)
                        && reason.contains("unresolved Make variable(s): top_srcdir")
                }),
                "{cpu}/{recipe}: missing explicit fail-closed diagnostic: {:#?}",
                parsed.skipped_programs
            );
        }
    }
}
