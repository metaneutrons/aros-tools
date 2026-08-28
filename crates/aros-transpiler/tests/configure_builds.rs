use aros_transpiler::dirs::DirVars;
use aros_transpiler::{
    generate_cmake, parse_mmakefile_with_dirs_and_context, ConfigureBuildDecl, DependencyGraph,
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

fn declarations_for(
    root: &Path,
    dirs: &DirVars,
    context: &TargetContext,
) -> Vec<ConfigureBuildDecl> {
    let mut declarations = Vec::new();
    for file in [
        "tools/ADFlib/mmakefile.src",
        "workbench/network/WirelessManager/wpa_supplicant/mmakefile.src",
    ] {
        let parsed = parse_mmakefile_with_dirs_and_context(&root.join(file), root, dirs, context)
            .unwrap_or_else(|reason| panic!("{file}: {reason}"));
        assert!(
            parsed
                .skipped_programs
                .iter()
                .all(|diagnostic| !diagnostic.contains("%build_with_configure")),
            "{file}: {:#?}",
            parsed.skipped_programs
        );
        declarations.extend(parsed.configure_builds);
    }
    declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
    declarations
}

#[test]
fn configure_contracts_are_exact_for_every_current_architecture() {
    let root = root();
    let dirs = DirVars::load(&root);
    let mut profiles = Vec::new();

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
        ("riscv64", "opensbi", ""),
    ] {
        let declarations =
            declarations_for(&root, &dirs, &target_context(cpu, platform, float_abi));
        assert_eq!(declarations.len(), 3, "{cpu}: {declarations:#?}");
        profiles.push(declarations);
    }

    assert!(profiles.windows(2).all(|pair| pair[0] == pair[1]));
    let declarations = &profiles[0];
    assert_eq!(
        declarations
            .iter()
            .map(|declaration| declaration.mmake_name.as_str())
            .collect::<Vec<_>>(),
        [
            "host-adflib",
            "linklib-adflib",
            "workbench-network-wirelessmanager",
        ]
    );

    let host = &declarations[0];
    assert_eq!(host.mode, "adflib-host");
    assert_eq!(host.private_products.len(), 1);
    assert_eq!(host.install_products.len(), 23);
    assert!(host.provided_library.is_none());

    let target = &declarations[1];
    assert_eq!(target.mode, "adflib-target");
    assert_eq!(target.provided_library.as_deref(), Some("adf"));
    assert_eq!(
        target.provider_target.as_deref(),
        Some("linklib-adflib-configure-adf")
    );
    assert_eq!(target.install_products.len(), 23);

    let wireless = &declarations[2];
    assert_eq!(wireless.mode, "wirelessmanager");
    assert_eq!(wireless.private_products.len(), 3);
    assert_eq!(wireless.dependency_targets, ["linklibs-mui"]);
    assert_eq!(
        wireless.install_products,
        ["${AROS_BUILD_DIR}/SYS/C/WirelessManager"]
    );
}

#[test]
fn configure_contracts_generate_real_products_and_a_link_provider() {
    let root = root();
    let dirs = DirVars::load(&root);
    let declarations = declarations_for(&root, &dirs, &target_context("x86_64", "pc", ""));
    let mut graph = DependencyGraph::new();
    for declaration in declarations {
        graph.add_configure_build(declaration);
    }

    let cmake = generate_cmake(&graph);
    assert_eq!(cmake.matches("aros_build_configure(").count(), 3, "{cmake}");
    assert!(cmake.contains("    MMAKE_ID host-adflib\n    MODE \"adflib-host\""));
    assert!(cmake.contains("    MMAKE_ID linklib-adflib\n    MODE \"adflib-target\""));
    assert!(cmake.contains("    PROVIDED_LIBRARY \"adf\""));
    assert!(cmake
        .contains("    MMAKE_ID workbench-network-wirelessmanager\n    MODE \"wirelessmanager\""));
    // The archive path is not spelled here: CMake asks linklibs-mui where it
    // writes, because a link library moves once a consumer names it.
    assert!(cmake.contains("    DEPENDENCY_TARGETS \"linklibs-mui\""));
    assert!(!cmake.contains("liblinklibs-mui.a"));
    assert!(cmake.contains(
        "\"${AROS_BUILD_DIR}/gen/configure/workbench/network/WirelessManager/source/wpa_supplicant/wpa_cli\""
    ));
    assert!(cmake.contains("\"${AROS_BUILD_DIR}/SYS/C/WirelessManager\""));
}

#[test]
fn configure_contracts_fail_closed_for_an_unapproved_profile() {
    let root = root();
    let dirs = DirVars::load(&root);
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("tools/ADFlib/mmakefile.src"),
        &root,
        &dirs,
        &target_context("arm", "raspi", "soft"),
    )
    .unwrap();

    assert!(parsed.configure_builds.is_empty());
    assert_eq!(
        parsed
            .skipped_programs
            .iter()
            .filter(|diagnostic| diagnostic.contains("%build_with_configure"))
            .count(),
        2
    );
    assert!(parsed.skipped_programs.iter().all(|diagnostic| {
        !diagnostic.contains("%build_with_configure")
            || diagnostic.contains("does not support target profile")
    }));
}
