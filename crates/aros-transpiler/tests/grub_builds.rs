use aros_transpiler::dirs::DirVars;
use aros_transpiler::{
    generate_cmake, parse_mmakefile_with_dirs_and_context, DependencyGraph, GrubBuildDecl,
    TargetContext,
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

fn declarations_for(context: &TargetContext) -> Vec<GrubBuildDecl> {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("arch/all-pc/boot/grub2-host/mmakefile.src"),
        &root,
        &dirs,
        context,
    )
    .unwrap();
    let mut declarations = parsed.grub_builds;
    declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
    declarations
}

#[test]
fn grub2_contract_selects_the_three_exact_x86_host_lanes() {
    let declarations = declarations_for(&target_context("x86_64", "pc", ""));
    assert_eq!(declarations.len(), 3, "{declarations:#?}");
    assert_eq!(
        declarations
            .iter()
            .map(|declaration| declaration.mmake_name.as_str())
            .collect::<Vec<_>>(),
        ["grub2-efi-host", "grub2-efi32-host", "grub2-host"]
    );

    let efi64 = &declarations[0];
    assert_eq!(efi64.mode, "efi64");
    assert_eq!(
        efi64.binary_dir,
        "${AROS_BUILD_DIR}/gen/configure/arch/all-pc/boot/grub2-host/efi-x86_64"
    );
    assert_eq!(
        efi64.install_prefix,
        "${AROS_BUILD_DIR}/hosttools/grub2/efi-x86_64"
    );

    let efi32 = &declarations[1];
    assert_eq!(efi32.mode, "efi32");
    assert_eq!(
        efi32.binary_dir,
        "${AROS_BUILD_DIR}/gen/configure/arch/all-pc/boot/grub2-host/efi-i386"
    );
    assert_eq!(
        efi32.install_prefix,
        "${AROS_BUILD_DIR}/hosttools/grub2/efi-i386"
    );

    let pc = &declarations[2];
    assert_eq!(pc.mode, "pc");
    assert_eq!(
        pc.binary_dir,
        "${AROS_BUILD_DIR}/gen/configure/arch/all-pc/boot/grub2-host/pc"
    );
    assert_eq!(pc.install_prefix, "${AROS_BUILD_DIR}/hosttools/grub2/pc");
}

#[test]
fn grub2_contract_emits_real_lanes_and_preserves_the_fetch_alias_edge() {
    let declarations = declarations_for(&target_context("x86_64", "pc", ""));
    let mut graph = DependencyGraph::new();
    for declaration in declarations {
        graph.add_grub_build(declaration);
    }
    graph.add_meta_rule(aros_transpiler::ast::MetaTargetRule {
        name: "grub2-host".to_owned(),
        dependencies: vec!["grub2-aros-fetch".to_owned()],
    });

    let cmake = generate_cmake(&graph);
    assert_eq!(cmake.matches("aros_build_grub2(").count(), 3, "{cmake}");
    assert!(cmake.contains("if(AROS_GRUB2_HOST_LANES_AVAILABLE)"));
    assert!(cmake.contains("audited GRUB2 host-tool lanes are unavailable on this build host"));
    assert!(cmake.contains("    MMAKE_ID grub2-host\n    MODE \"pc\""));
    assert!(cmake.contains("    MMAKE_ID grub2-efi-host\n    MODE \"efi64\""));
    assert!(cmake.contains("    MMAKE_ID grub2-efi32-host\n    MODE \"efi32\""));
    assert!(cmake.contains(
        "BINARY_DIR \"${AROS_BUILD_DIR}/gen/configure/arch/all-pc/boot/grub2-host/efi-i386\""
    ));
    assert!(cmake.contains("foreach(dep IN ITEMS \"grub2-aros-fetch\")"));
    assert!(!cmake.contains("add_custom_target(\"grub2-host\")"));
}

#[test]
fn grub2_contract_fails_closed_outside_the_x86_profile() {
    for (cpu, platform, float_abi) in [("arm", "raspi", "hard"), ("aarch64", "raspi", "")] {
        let root = root();
        let dirs = DirVars::load(&root);
        let parsed = parse_mmakefile_with_dirs_and_context(
            &root.join("arch/all-pc/boot/grub2-host/mmakefile.src"),
            &root,
            &dirs,
            &target_context(cpu, platform, float_abi),
        )
        .unwrap();
        assert!(
            parsed.grub_builds.is_empty(),
            "{cpu}: {:#?}",
            parsed.grub_builds
        );
        assert!(parsed.skipped_programs.iter().any(|diagnostic| {
            diagnostic.contains("%build_with_configure")
                && diagnostic.contains("GRUB2 host-tool capability only supports")
        }));
        assert!(
            parsed.capability_errors.is_empty(),
            "{cpu}: an intentional profile exclusion was classified as drift: {:#?}",
            parsed.capability_errors
        );
    }
}
