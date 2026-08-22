use aros_transpiler::dirs::DirVars;
use aros_transpiler::{
    generate_cmake, parse_mmakefile_with_dirs_and_context, DependencyGraph, TargetContext,
};
use std::collections::BTreeSet;
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
fn cunit_external_contract_is_exact_for_every_current_architecture() {
    let root = root();
    let dirs = DirVars::load(&root);
    let mut declarations = Vec::new();

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let parsed = parse_mmakefile_with_dirs_and_context(
            &root.join("compiler/cunit/mmakefile.src"),
            &root,
            &dirs,
            &target_context(cpu, platform, float_abi),
        )
        .unwrap();
        assert_eq!(
            parsed.external_cmake.len(),
            1,
            "{cpu}: {:#?}",
            parsed.skipped_programs
        );
        assert!(
            parsed
                .skipped_programs
                .iter()
                .all(|diagnostic| !diagnostic.contains("%build_with_cmake")),
            "{cpu}: {:#?}",
            parsed.skipped_programs
        );
        declarations.push(parsed.external_cmake[0].clone());
    }

    assert!(declarations.windows(2).all(|pair| pair[0] == pair[1]));
    let declaration = &declarations[0];
    assert_eq!(declaration.mmake_name, "linklibs-yes-cunit");
    assert_eq!(
        declaration.provider_target,
        "linklibs-yes-cunit-external-cunit"
    );
    assert_eq!(
        declaration.source_archive,
        "${AROS_PORTS_SOURCE_DIR}/cunit-3.5.5.tar.bz2"
    );
    assert_eq!(
        declaration.binary_dir,
        "${AROS_BUILD_DIR}/gen/external-cmake/compiler/cunit"
    );
    assert_eq!(
        declaration.source_sha256,
        "a0a49b37c731303168481f387bb551b8381422d1b447d32f9e558293ceea9a10"
    );
    assert_eq!(
        declaration.local_patch_files,
        ["${CMAKE_SOURCE_DIR}/compiler/cunit/cunit-3.5.5-aros.diff"]
    );
    assert_eq!(
        declaration.local_patch_sha256,
        ["481b9d4544e7fae9f47dc821f343cbb5f417ea8abc76c1a8f9f9177ab7420197"]
    );
    assert_eq!(declaration.header_products.len(), 19);
    assert!(declaration.auxiliary_products.is_empty());
}

#[test]
fn cunit_uselib_resolves_to_link_interface_and_generator_emits_it_first() {
    let root = root();
    let dirs = DirVars::load(&root);
    let context = target_context("x86_64", "pc", "");
    let cunit = parse_mmakefile_with_dirs_and_context(
        &root.join("compiler/cunit/mmakefile.src"),
        &root,
        &dirs,
        &context,
    )
    .unwrap();
    let consumer = parse_mmakefile_with_dirs_and_context(
        &root.join("developer/debug/test/locale/mmakefile.src"),
        &root,
        &dirs,
        &context,
    )
    .unwrap();

    let mut graph = DependencyGraph::new();
    for fetch in cunit.fetches {
        graph.add_fetches(vec![fetch]);
    }
    for declaration in cunit.external_cmake {
        graph.add_external_cmake(declaration);
    }
    for rule in cunit.meta_rules.into_iter().chain(consumer.meta_rules) {
        graph.add_meta_rule(rule);
    }
    for target in consumer.targets {
        graph.add_target(target);
    }

    let unresolved = graph.resolve_use_libs();
    assert!(
        unresolved
            .iter()
            .all(|diagnostic| !diagnostic.contains("uselibs=cunit")),
        "{unresolved:#?}"
    );
    assert_eq!(
        graph.targets["test-locale-formatstring-yes-cunit"].link_libs,
        ["linklibs-yes-cunit-external-cunit"]
    );

    let cmake = generate_cmake(&graph);
    let external_at = cmake
        .find("aros_build_external_cmake(")
        .expect("external CMake declaration");
    let consumer_at = cmake
        .find("MMAKE_ID test-locale-formatstring-yes-cunit")
        .expect("ordinary consumer declaration");
    assert!(external_at < consumer_at, "{cmake}");
    assert!(cmake.contains("    MMAKE_ID linklibs-yes-cunit"));
    assert!(cmake.contains("    LIBS \"linklibs-yes-cunit-external-cunit\""));
    assert!(cmake.contains(
        "    SOURCE_SHA256 \"a0a49b37c731303168481f387bb551b8381422d1b447d32f9e558293ceea9a10\""
    ));
    assert!(cmake.contains(
        "    SOURCE_DIR \"${AROS_PORTS_DIR}/cunit/cunit-3.5.5\"\n\
         \x20   LOCAL_PATCH_FILES \"${CMAKE_SOURCE_DIR}/compiler/cunit/cunit-3.5.5-aros.diff\"\n\
         \x20   LOCAL_PATCH_SHA256 \"481b9d4544e7fae9f47dc821f343cbb5f417ea8abc76c1a8f9f9177ab7420197\""
    ));
    assert!(
        cmake.contains("\"${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/include/CUnit/Automated.h\"")
    );
    assert!(
        cmake.contains("\"${AROS_BUILD_DIR}/SYS/Developer/SDK/Extras/include/CUnit/wxWidget.h\"")
    );
    assert!(!cmake.contains("    AUXILIARY_PRODUCTS"));
    assert!(cmake.contains(
        "    OPTIONS \"-DCUNIT_DISABLE_EXAMPLES=yes\" \"-DCUNIT_DISABLE_TESTS=yes\" \"-DCMAKE_BUILD_TYPE=DEBUG\" \"-Wno-error=dev\""
    ));
    assert!(
        !cmake.contains("add_custom_target(\"linklibs-yes-cunit\")"),
        "the external helper owns the workflow endpoint: {cmake}"
    );
    assert!(cmake.contains("aros_add_target_dependency(\"linklibs-yes-cunit\" \"${dep}\")"));
}

#[test]
fn aom_external_contract_and_fetch_patch_are_profile_exact() {
    let root = root();
    let dirs = DirVars::load(&root);

    for (cpu, platform, float_abi, specific) in [
        ("x86_64", "pc", "", &["-DAOM_TARGET_CPU=generic"][..]),
        (
            "arm",
            "raspi",
            "hard",
            &[
                "-DAOM_TARGET_CPU=arm",
                "-DENABLE_NEON=0",
                "-DCONFIG_RUNTIME_CPU_DETECT=0",
            ][..],
        ),
        ("aarch64", "raspi", "", &["-DAOM_TARGET_CPU=generic"][..]),
    ] {
        let parsed = parse_mmakefile_with_dirs_and_context(
            &root.join("workbench/classes/datatypes/heic/mmakefile.src"),
            &root,
            &dirs,
            &target_context(cpu, platform, float_abi),
        )
        .unwrap();
        assert_eq!(parsed.external_cmake.len(), 1, "{cpu}");
        let declaration = parsed.external_cmake[0].clone();
        assert_eq!(declaration.mmake_name, "datatypes-heic-linklibs-aom");
        assert_eq!(
            declaration.local_patch_files,
            ["${CMAKE_SOURCE_DIR}/workbench/classes/datatypes/heic/libaom-3.12.1-aros.diff"]
        );
        assert_eq!(
            declaration.local_patch_sha256,
            ["c3caf62de4cd3524ddcf7c1b0111909c6d0f44081200324ab12090fcd8fb48ce"]
        );
        assert!(declaration.options.starts_with(&[
            "-DBUILD_SHARED_LIBS=OFF".to_owned(),
            "-DENABLE_NASM=ON".to_owned(),
            "-DENABLE_EXAMPLES=OFF".to_owned(),
            "-DENABLE_TESTS=OFF".to_owned(),
            "-DENABLE_TOOLS=OFF".to_owned(),
            "-DCONFIG_AV1_ENCODER=0".to_owned(),
            "-DCONFIG_AV1_DECODER=1".to_owned(),
            "-DCONFIG_MULTITHREAD=0".to_owned(),
            "-DCMAKE_BUILD_TYPE=Release".to_owned(),
        ]));
        assert_eq!(
            &declaration.options[declaration.options.len() - specific.len()..],
            specific
        );

        let mut graph = DependencyGraph::new();
        graph.add_fetches(parsed.fetches);
        graph.add_external_cmake(declaration);
        let cmake = generate_cmake(&graph);
        assert!(cmake.contains(
            "    SOURCE_DIR \"${AROS_PORTS_DIR}/libaom/libaom-3.12.1\"\n\
             \x20   LOCAL_PATCH_FILES \"${CMAKE_SOURCE_DIR}/workbench/classes/datatypes/heic/libaom-3.12.1-aros.diff\"\n\
             \x20   LOCAL_PATCH_SHA256 \"c3caf62de4cd3524ddcf7c1b0111909c6d0f44081200324ab12090fcd8fb48ce\""
        ));
    }
}

#[test]
fn external_and_ordinary_providers_with_the_same_name_are_ambiguous() {
    let root = root();
    let dirs = DirVars::load(&root);
    let context = target_context("x86_64", "pc", "");
    let cunit = parse_mmakefile_with_dirs_and_context(
        &root.join("compiler/cunit/mmakefile.src"),
        &root,
        &dirs,
        &context,
    )
    .unwrap();
    let zlib = parse_mmakefile_with_dirs_and_context(
        &root.join("workbench/libs/z/mmakefile.src"),
        &root,
        &dirs,
        &context,
    )
    .unwrap();
    let locale = parse_mmakefile_with_dirs_and_context(
        &root.join("developer/debug/test/locale/mmakefile.src"),
        &root,
        &dirs,
        &context,
    )
    .unwrap();

    let mut ordinary = zlib
        .targets
        .into_iter()
        .find(|target| target.mmake_name == "linklibs-z-static")
        .unwrap();
    ordinary.mmake_name = "ordinary-cunit-provider".to_owned();
    ordinary.target_name = "cunit".to_owned();
    let mut consumer = locale
        .targets
        .into_iter()
        .find(|target| target.mmake_name == "test-locale-formatstring-yes-cunit")
        .unwrap();
    consumer.use_libs = vec!["cunit".to_owned()];
    consumer.link_libs.clear();
    consumer.link_options.clear();

    let mut graph = DependencyGraph::new();
    graph.add_external_cmake(cunit.external_cmake[0].clone());
    graph.add_target(ordinary);
    graph.add_target(consumer);
    let unresolved = graph.resolve_use_libs();
    assert!(
        unresolved.iter().any(|diagnostic| diagnostic
            .contains("uselibs=cunit is ambiguous (ordinary-cunit-provider, linklibs-yes-cunit)")),
        "{unresolved:#?}"
    );
    assert!(graph.targets["test-locale-formatstring-yes-cunit"]
        .link_libs
        .is_empty());
}

#[test]
fn every_llvm_external_cmake_declaration_stays_explicitly_skipped() {
    let root = root();
    let dirs = DirVars::load(&root);
    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let relative = "tools/crosstools/llvm/mmakefile.src";
        let parsed =
            parse_mmakefile_with_dirs_and_context(&root.join(relative), &root, &dirs, &context)
                .unwrap();
        assert!(parsed.external_cmake.is_empty(), "{relative}");
        let skipped: Vec<_> = parsed
            .skipped_programs
            .into_iter()
            .filter(|diagnostic| diagnostic.contains("%build_with_cmake"))
            .collect();
        assert!(
            skipped
                .iter()
                .all(|diagnostic| diagnostic.contains("unsupported external-CMake capability")),
            "{cpu}: {skipped:#?}"
        );
        let identities: BTreeSet<_> = skipped
            .iter()
            .filter_map(|diagnostic| {
                diagnostic
                    .split(" mmake=")
                    .nth(1)
                    .and_then(|tail| tail.split_whitespace().next())
            })
            .collect();
        let mut expected = BTreeSet::from([
            "crosstools-libunwind",
            "crosstools-libunwind-release",
            "crosstools-compiler-rt",
            "crosstools-compiler-rt-release",
            "crosstools-llvm-toolchain",
        ]);
        if cpu == "x86_64" {
            expected.insert("crosstools-compiler-rt32");
            expected.insert("crosstools-compiler-rt32-release");
        }
        assert_eq!(identities, expected, "{cpu}: {skipped:#?}");
    }
}
