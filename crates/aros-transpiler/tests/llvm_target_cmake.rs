mod common;

use aros_transpiler::{
    dirs::DirVars, generate_cmake, parse_mmakefile_with_dirs_and_context, DependencyGraph,
    TargetContext,
};

fn context() -> TargetContext {
    TargetContext {
        cpu: Some("x86_64".to_owned()),
        platform: Some("pc".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some("i386".to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(String::new()),
        mesa_version: Some("26.0.0".to_owned()),
        ..TargetContext::default()
    }
}

#[test]
fn target_llvm_has_real_closed_fetch_host_tool_and_component_products() {
    let root = common::source_root();
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/llvm/mmakefile.src"),
        &root,
        &DirVars::load(&root),
        &context(),
    )
    .unwrap();
    assert!(
        parsed.capability_errors.is_empty(),
        "{:#?}",
        parsed.capability_errors
    );
    let [declaration] = parsed.external_cmake.as_slice() else {
        panic!("missing real Target LLVM: {:#?}", parsed.skipped_programs);
    };
    assert_eq!(declaration.library_products.len(), 44);
    assert_eq!(declaration.build_targets.len(), 44);
    assert_eq!(declaration.header_products.len(), 8);
    assert!(!declaration
        .header_products
        .iter()
        .any(|path| path.ends_with("/Config/config.h")));
    assert_eq!(declaration.install_components, ["llvm-headers"]);
    assert_eq!(declaration.host_tools, ["LLVM_TABLEGEN=llvm-tblgen@11.0.0"]);
    assert!(declaration.library_group);
    assert!(declaration
        .options
        .contains(&"-DLLVM_ENABLE_RTTI=ON".to_owned()));
    let [fetch] = parsed.fetches.as_slice() else {
        panic!("must own only the selected LLVM11 source");
    };
    assert_eq!(fetch.archive, "llvm-11.0.0.src");
    assert!(fetch
        .checksums
        .contains("913f68c898dfb4a03b397c5e11c6a2f39d0f22ed7665c9cefa87a34423a72469"));
    assert!(!fetch.patches.contains("wildcard"));
    let mut graph = DependencyGraph::new();
    graph.add_external_cmake(declaration.clone());
    let cmake = generate_cmake(&graph);
    for required in [
        "LIBRARY_GROUP",
        "BUILD_TARGETS",
        "INSTALL_COMPONENTS",
        "HOST_TOOLS",
        "libLLVMExecutionEngine.a",
        "libLLVMRuntimeDyld.a",
    ] {
        assert!(
            cmake.contains(required),
            "missing generated contract {required}"
        );
    }
}

#[test]
fn target_llvm_rejects_an_explicit_unqualified_version_selector() {
    let root = common::source_root();
    let mut target = context();
    target.target_llvm_ver = Some("21.1.0".to_owned());
    let error = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/llvm/mmakefile.src"),
        &root,
        &DirVars::load(&root),
        &target,
    )
    .unwrap_err();
    assert!(error.to_string().contains("requires 11.0.0, not 21.1.0"));
}

#[test]
fn target_llvm_rejects_changed_recipe_instead_of_emitting_empty_meta_success() {
    let root = common::source_root();
    let temp = tempfile::tempdir().unwrap();
    let relative = "workbench/libs/llvm/mmakefile.src";
    let destination = temp.path().join(relative);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let source = std::fs::read_to_string(root.join(relative)).unwrap();
    std::fs::write(
        &destination,
        source.replace("-DLLVM_ENABLE_RTTI=ON", "-DLLVM_ENABLE_RTTI=OFF"),
    )
    .unwrap();
    let error = parse_mmakefile_with_dirs_and_context(
        &destination,
        temp.path(),
        &DirVars::load(temp.path()),
        &context(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unsupported upstream recipe drift"),
        "{error}"
    );
}
