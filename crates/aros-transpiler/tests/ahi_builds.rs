use aros_transpiler::dirs::DirVars;
use aros_transpiler::{
    generate_cmake, parse_mmakefile_with_dirs_and_context, AhiBuildDecl, DependencyGraph,
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

fn declarations_for(context: &TargetContext) -> Vec<AhiBuildDecl> {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/devs/AHI/mmakefile.src"),
        &root,
        &dirs,
        context,
    )
    .unwrap();
    assert!(
        parsed
            .skipped_programs
            .iter()
            .all(|diagnostic| !diagnostic.contains("%build_with_configure")),
        "{:#?}",
        parsed.skipped_programs
    );
    parsed.ahi_builds
}

#[test]
fn ahi_contract_selects_the_one_exact_current_architecture_lane() {
    for (cpu, platform, float_abi, mode) in [
        ("x86_64", "pc", "", "x86_64"),
        ("arm", "raspi", "hard", "arm"),
        ("aarch64", "raspi", "", "aarch64"),
    ] {
        let declarations = declarations_for(&target_context(cpu, platform, float_abi));
        assert_eq!(declarations.len(), 1, "{cpu}: {declarations:#?}");
        let declaration = &declarations[0];
        assert_eq!(declaration.mmake_name, "workbench-devs-AHI-subsystem");
        assert_eq!(declaration.mode, mode);
        assert_eq!(
            declaration.binary_dir,
            format!("${{AROS_BUILD_DIR}}/gen/configure/workbench/devs/AHI/{mode}")
        );
        assert_eq!(declaration.install_prefix, "${AROS_BUILD_DIR}/SYS");
        assert_eq!(declaration.host_sfdc, "${AROS_HOST_SFDC}");
        assert_eq!(declaration.host_perl, "${AROS_HOST_PERL}");
    }
}

#[test]
fn ahi_contract_emits_a_closed_host_tool_api_and_preserves_the_mmake_edge() {
    let declarations = declarations_for(&target_context("x86_64", "pc", ""));
    let mut graph = DependencyGraph::new();
    for declaration in declarations {
        graph.add_ahi_build(declaration);
    }
    graph.add_meta_rule(aros_transpiler::ast::MetaTargetRule {
        name: "workbench-devs-AHI".to_owned(),
        dependencies: vec!["workbench-devs-AHI-subsystem".to_owned()],
    });

    let cmake = generate_cmake(&graph);
    assert_eq!(cmake.matches("aros_build_ahi(").count(), 1, "{cmake}");
    assert!(cmake.contains(concat!(
        "# =============================================================================\n",
        "# Capability-checked AHI subsystem builds\n",
        "# ============================================================================="
    )));
    assert!(cmake.contains(concat!(
        "MMAKE_ID workbench-devs-AHI-subsystem\n",
        "    MODE \"x86_64\"\n",
        "    BINARY_DIR \"${AROS_BUILD_DIR}/gen/configure/workbench/devs/AHI/x86_64\"\n",
        "    INSTALL_PREFIX \"${AROS_BUILD_DIR}/SYS\"\n",
        "    HOST_SFDC \"${AROS_HOST_SFDC}\"\n",
        "    HOST_PERL \"${AROS_HOST_PERL}\""
    )));
    assert!(cmake.contains("foreach(dep IN ITEMS \"workbench-devs-AHI-subsystem\")"));
    assert!(!cmake.contains("add_custom_target(\"workbench-devs-AHI-subsystem\")"));
}

#[test]
fn ahi_contract_fails_closed_for_an_unapproved_profile() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/devs/AHI/mmakefile.src"),
        &root,
        &dirs,
        &target_context("arm", "raspi", "soft"),
    )
    .unwrap();

    assert!(parsed.ahi_builds.is_empty());
    assert!(parsed.skipped_programs.iter().any(|diagnostic| {
        diagnostic.contains("%build_with_configure")
            && diagnostic.contains("AHI subsystem capability only supports")
    }));
}
