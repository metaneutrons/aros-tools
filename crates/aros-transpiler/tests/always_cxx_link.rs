use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars, generate_cmake,
    parse_mmakefile_with_dirs_and_context_and_fetches, DependencyGraph, ModuleType, TargetContext,
};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../..")
        .canonicalize()
        .unwrap()
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
fn production_hidds_preserve_always_cxx_link_for_every_current_architecture() {
    let root = root();
    let dirs = DirVars::load(&root);
    let mesa_mmakefile = root.join("workbench/libs/mesa/mmakefile.src");

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
        ("riscv64", "opensbi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(&mesa_mmakefile, &root, &context)
            .unwrap_or_else(|error| panic!("{cpu}: collect Mesa fetches: {error:#}"));

        for (mmake, relative) in [
            ("hidd-softpipe", "workbench/hidds/softpipe/mmakefile.src"),
            ("hidd-nouveau", "workbench/hidds/nouveau/mmakefile.src"),
        ] {
            let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
                &root.join(relative),
                &root,
                &dirs,
                &context,
                &fetches,
            )
            .unwrap_or_else(|error| panic!("{cpu}: parse {mmake}: {error:#}"));
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake)
                .unwrap_or_else(|| panic!("{cpu}: missing {mmake}: {:#?}", parsed.targets));

            assert_eq!(target.module_type, ModuleType::Hidd, "{cpu}: {mmake}");
            assert!(target.always_cxx_link, "{cpu}: {mmake}");

            let mut graph = DependencyGraph::new();
            graph.add_target(target.clone());
            let cmake = generate_cmake(&graph);
            let start = cmake
                .find(&format!(
                    "aros_add_hidd(\n    TARGET {}\n    MMAKE_ID {mmake}",
                    target.target_name
                ))
                .unwrap_or_else(|| panic!("{cpu}: missing emitted {mmake} declaration"));
            let end = cmake[start..].find("\n)\n").unwrap() + start;
            assert!(
                cmake[start..end].contains("    ALWAYS_CXX_LINK"),
                "{cpu}: emitted {mmake} declaration:\n{}",
                &cmake[start..end]
            );
        }
    }
}
