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
        mesa_version: Some("26.2.5".to_owned()),
        target_llvm_ver: Some("23.1.0".to_owned()),
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
    assert_eq!(declaration.library_products.len(), 65);
    assert_eq!(declaration.build_targets.len(), 65);
    assert_eq!(declaration.header_products.len(), 8);
    assert!(!declaration
        .header_products
        .iter()
        .any(|path| path.ends_with("/Config/config.h")));
    assert_eq!(declaration.install_components, ["llvm-headers"]);
    assert_eq!(declaration.host_tools, ["LLVM_TABLEGEN=llvm-tblgen@23.1.0"]);
    assert!(declaration.library_group);
    assert!(declaration
        .options
        .contains(&"-DLLVM_ENABLE_RTTI=ON".to_owned()));
    assert!(declaration
        .options
        .contains(&"-DCMAKE_DISABLE_PRECOMPILE_HEADERS=ON".to_owned()));
    assert_eq!(
        &declaration.options[3..6],
        [
            "-DLLVM_TARGETS_TO_BUILD=X86",
            "-DLLVM_DEFAULT_TARGET_TRIPLE=x86_64-unknown-aros",
            "-DLLVM_HOST_TRIPLE=x86_64-unknown-aros",
        ]
    );
    let [fetch] = parsed.fetches.as_slice() else {
        panic!("must own only the selected LLVM 23 source");
    };
    assert_eq!(fetch.archive, "llvm-project-23.1.0.src");
    assert!(fetch
        .checksums
        .contains("ab1f0e3ec52448c33e8782eaf0422504b87c7b016b22514653ee0d8fcee479ff"));
    assert_eq!(
        fetch.patches,
        "llvm-23.1.0.src-aros.diff:llvm-project-23.1.0.src/llvm:-p2"
    );
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
    assert!(error
        .to_string()
        .contains("supports 11.0.0 and 23.1.0, not 21.1.0"));
}

#[test]
fn target_llvm_retains_the_explicit_llvm11_legacy_provider() {
    let root = common::source_root();
    let mut target = context();
    target.target_llvm_ver = Some("11.0.0".to_owned());
    let parsed = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/llvm/mmakefile.src"),
        &root,
        &DirVars::load(&root),
        &target,
    )
    .unwrap();
    let [declaration] = parsed.external_cmake.as_slice() else {
        panic!("missing explicit LLVM 11 legacy provider");
    };
    assert_eq!(declaration.build_targets.len(), 44);
    assert_eq!(
        declaration.install_prefix,
        "${AROS_BUILD_DIR}/gen/external-install/llvm11"
    );
    assert_eq!(declaration.host_tools, ["LLVM_TABLEGEN=llvm-tblgen@11.0.0"]);
    let [fetch] = parsed.fetches.as_slice() else {
        panic!("missing explicit LLVM 11 source");
    };
    assert_eq!(fetch.archive, "llvm-11.0.0.src");
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

fn raspi_context(cpu: &str, float_abi: &str) -> TargetContext {
    TargetContext {
        cpu: Some(cpu.to_owned()),
        platform: Some("raspi".to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(String::new()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(float_abi.to_owned()),
        target_llvm_ver: Some("23.1.0".to_owned()),
        ..TargetContext::default()
    }
}

#[test]
fn raspi_target_llvm_owns_architecture_specific_complete_products() {
    let root = common::source_root();
    for (cpu, float_abi, backend, triple) in [
        ("arm", "hard", "ARM", "arm-unknown-aros"),
        ("aarch64", "", "AArch64", "aarch64-unknown-aros"),
    ] {
        let parsed = parse_mmakefile_with_dirs_and_context(
            &root.join("workbench/libs/llvm/mmakefile.src"),
            &root,
            &DirVars::load(&root),
            &raspi_context(cpu, float_abi),
        )
        .unwrap();
        assert!(
            parsed.capability_errors.is_empty(),
            "{:#?}",
            parsed.capability_errors
        );
        let [declaration] = parsed.external_cmake.as_slice() else {
            panic!("missing {backend} target LLVM provider");
        };
        let expected_count = if cpu == "arm" { 63 } else { 66 };
        assert_eq!(declaration.build_targets.len(), expected_count);
        assert_eq!(declaration.library_products.len(), expected_count);
        assert_eq!(declaration.header_products.len(), 8);
        let manifest = if cpu == "arm" {
            include_str!("../llvm23-arm-static.components")
        } else {
            include_str!("../llvm23-aarch64-static.components")
        };
        let expected = manifest
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(declaration.build_targets, expected);
        assert_eq!(
            declaration.library_products,
            expected
                .iter()
                .map(|name| {
                    format!("${{AROS_BUILD_DIR}}/gen/external-install/llvm23/lib/lib{name}.a")
                })
                .collect::<Vec<_>>()
        );
        for suffix in [
            "AsmParser",
            "CodeGen",
            "Desc",
            "Disassembler",
            "Info",
            "Utils",
        ] {
            let library = format!("LLVM{backend}{suffix}");
            assert!(
                declaration.build_targets.contains(&library),
                "missing {library}"
            );
            assert!(declaration
                .library_products
                .iter()
                .any(|path| path.ends_with(&format!("/lib{library}.a"))));
        }
        assert!(!declaration
            .build_targets
            .iter()
            .any(|name| name.starts_with("LLVMX86")));
        for option in [
            format!("-DLLVM_TARGETS_TO_BUILD={backend}"),
            format!("-DLLVM_DEFAULT_TARGET_TRIPLE={triple}"),
            format!("-DLLVM_HOST_TRIPLE={triple}"),
            "-DLLVM_ENABLE_RTTI=ON".to_owned(),
        ] {
            assert!(declaration.options.contains(&option), "missing {option}");
        }
        for header in ["llvm-c/Core.h", "llvm/Config/llvm-config.h"] {
            assert!(declaration
                .header_products
                .iter()
                .any(|path| path.ends_with(header)));
        }
        assert_eq!(declaration.install_components, ["llvm-headers"]);
        assert_eq!(declaration.host_tools, ["LLVM_TABLEGEN=llvm-tblgen@23.1.0"]);
        assert!(declaration.library_group);
    }
}

#[test]
fn raspi_target_llvm_rejects_unqualified_version_and_selector_substitutions() {
    let root = common::source_root();
    let recipe = root.join("workbench/libs/llvm/mmakefile.src");
    for (cpu, float_abi) in [("arm", "hard"), ("aarch64", "")] {
        let mut target = raspi_context(cpu, float_abi);
        target.target_llvm_ver = Some("21.1.0".to_owned());
        let error =
            parse_mmakefile_with_dirs_and_context(&recipe, &root, &DirVars::load(&root), &target)
                .unwrap_err();
        assert!(error
            .to_string()
            .contains("supports 11.0.0 and 23.1.0, not 21.1.0"));
    }
    for mut target in [
        raspi_context("arm", "soft"),
        raspi_context("aarch64", "hard"),
        raspi_context("riscv", ""),
    ] {
        // An unqualified tuple must not inherit either closed provider.
        target.target_llvm_ver = Some("23.1.0".to_owned());
        let parsed =
            parse_mmakefile_with_dirs_and_context(&recipe, &root, &DirVars::load(&root), &target)
                .unwrap();
        assert!(!parsed
            .external_cmake
            .iter()
            .any(|item| item.provided_library == "LLVM"));
    }
}

const LLVM_RECIPE: &str = "workbench/libs/llvm/mmakefile.src";

const AROS_OPTIONS_BLOCK: &str = "# Target-side libraries and tools run on AROS. Keep host TableGen separate.\n# LLVM_TARGET_ARCH takes a CPU name, not the LLVM backend name (e.g. RISCV).\nLLVM_AROS_CMAKEOPTIONS := \\\n    -DLLVM_HOST_TRIPLE=$(AROS_TARGET_CPU)-unknown-aros \\\n    -DLLVM_TARGET_ARCH=$(subst ppc,powerpc,$(AROS_TARGET_CPU))\n\n";

/// Reviewed rewrite between the qualified AROS-NX reference (LLVM 23 default,
/// no AROS target options) and the synchronised recipe (LLVM 11 default and
/// `LLVM_AROS_CMAKEOPTIONS` for the target libraries and the tools).
fn llvm_recipe_edits() -> Vec<(String, String)> {
    let target = "LLVM_TARGET_CMAKEOPTIONS := \\\n    $(LLVM_COMMON_CMAKEOPTIONS) \\\n";
    let tools = "LLVM_TOOLS_CMAKEOPTIONS := \\\n    $(LLVM_COMMON_CMAKEOPTIONS) \\\n";
    let gallivm = "# Gallivm's C++ memory manager and object cache derive from LLVM classes.\n";
    vec![
        (
            "OPT_LLVM := 23.1.0\n".to_owned(),
            "OPT_LLVM := 11.0.0\n".to_owned(),
        ),
        (gallivm.to_owned(), format!("{AROS_OPTIONS_BLOCK}{gallivm}")),
        (
            target.to_owned(),
            format!("{target}    $(LLVM_AROS_CMAKEOPTIONS) \\\n"),
        ),
        (
            tools.to_owned(),
            format!("{tools}    $(LLVM_AROS_CMAKEOPTIONS) \\\n"),
        ),
    ]
}

#[test]
fn target_llvm_admits_the_upstream_synchronised_recipe_with_unchanged_jobs() {
    let root = common::source_root();
    let original = std::fs::read_to_string(root.join(LLVM_RECIPE)).unwrap();
    let digest = aros_common::sha256_bytes(original.as_bytes()).to_string();
    let reference =
        aros_transpiler::fingerprints::fingerprint("llvm-target-recipe-with-llvm23-default")
            .unwrap();
    let synchronised =
        aros_transpiler::fingerprints::fingerprint("llvm-target-recipe-with-aros-target-options")
            .unwrap();
    let forward = digest == reference;
    assert!(
        forward || digest == synchronised,
        "recipe is in no reviewed state"
    );
    let mut text = original;
    for (before, after) in llvm_recipe_edits() {
        let (from, to) = if forward {
            (before, after)
        } else {
            (after, before)
        };
        assert_eq!(text.matches(&from).count(), 1, "{from:?}");
        text = text.replacen(&from, &to, 1);
    }
    assert_eq!(
        aros_common::sha256_bytes(text.as_bytes()).to_string(),
        if forward { synchronised } else { reference }
    );

    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join(LLVM_RECIPE);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    for patch in [
        "tools/crosstools/llvm/llvm-11.0.0.src-aros.diff",
        "tools/crosstools/llvm/llvm-23.1.0.src-aros.diff",
    ] {
        let copy = temp.path().join(patch);
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::copy(root.join(patch), copy).unwrap();
    }
    for version in ["11.0.0", "23.1.0"] {
        let mut target = context();
        target.target_llvm_ver = Some(version.to_owned());
        let parse = |recipe: &std::path::Path, base: &std::path::Path| {
            parse_mmakefile_with_dirs_and_context(recipe, base, &DirVars::load(base), &target)
                .unwrap()
        };
        let baseline = parse(&root.join(LLVM_RECIPE), &root);
        std::fs::write(&destination, &text).unwrap();
        let admitted = parse(&destination, temp.path());
        assert!(
            admitted.capability_errors.is_empty(),
            "{version}: {:#?}",
            admitted.capability_errors
        );
        let ([expected], [actual]) = (
            baseline.external_cmake.as_slice(),
            admitted.external_cmake.as_slice(),
        ) else {
            panic!("{version}: missing Target LLVM declaration");
        };
        assert_eq!(actual.options, expected.options, "{version}");
        assert_eq!(actual.build_targets, expected.build_targets, "{version}");
        assert_eq!(actual.install_prefix, expected.install_prefix, "{version}");
    }
}
