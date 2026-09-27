mod common;

use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars, generate_cmake,
    parse_mmakefile_with_dirs_and_context_and_fetches, DependencyGraph, TargetContext,
};

fn mesa26_source_root() -> std::path::PathBuf {
    std::env::var_os("AROS_TEST_MESA26_SOURCE_ROOT")
        .map_or_else(common::source_root, std::path::PathBuf::from)
}

fn context(cpu: &str, platform: &str, cpu32: &str, float_abi: &str) -> TargetContext {
    TargetContext {
        cpu: Some(cpu.to_owned()),
        platform: Some(platform.to_owned()),
        toolchain: Some("llvm".to_owned()),
        cpu32: Some(cpu32.to_owned()),
        use_mmu: Some("1".to_owned()),
        float_abi: Some(float_abi.to_owned()),
        mesa_version: Some("26.0.0".to_owned()),
        ..TargetContext::default()
    }
}

#[test]
fn mesa26_gallivm_remains_retired_without_target_llvm() {
    let root = mesa26_source_root();
    let source =
        std::fs::read_to_string(root.join("workbench/libs/mesa/libgalliumvm/mmakefile.src"))
            .expect("Gallivm source boundary");
    assert!(source.contains("intentionally retired"));
    assert!(source.contains("target LLVM runtime"));
    assert!(!source.contains("%build_linklib"));
    assert!(!source
        .lines()
        .any(|line| { line.trim_start().starts_with("#MM") && line.contains("galliumvm") }));
}

#[test]
fn mesa26_v3d_has_closed_aarch64_driver_and_generators() {
    let root = mesa26_source_root();
    let profile = context("aarch64", "raspi", "", "");
    let fetches = collect_mmakefile_fetches_with_context(
        &root.join("workbench/libs/mesa/mmakefile.src"),
        &root,
        &profile,
    )
    .unwrap();
    let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
        &root.join("arch/arm-native/soc/broadcom/2708/hidd/v3d/mmakefile.src"),
        &root,
        &DirVars::load(&root),
        &profile,
        &fetches,
    )
    .unwrap();
    assert!(parsed.capability_errors.is_empty(), "{parsed:#?}");
    let driver = parsed
        .targets
        .iter()
        .find(|candidate| candidate.mmake_name == "linklibs-gallium_v3d")
        .expect("V3D driver archive");
    assert_eq!(driver.source_files.len(), 67);
    assert!(driver.source_files.iter().all(|source| {
        !source.starts_with("${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/v3d/")
    }));
    assert!(driver.defines.contains(&"AROS_MESA26_V3D=1".to_owned()));
    assert!(driver
        .include_dirs
        .contains(&"${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler".to_owned()));
    let driver_rule = parsed
        .meta_rules
        .iter()
        .find(|rule| rule.name == "linklibs-gallium_v3d")
        .expect("V3D archive build order");
    assert!(driver_rule
        .dependencies
        .contains(&"mesa3d-linklib-mesautil-generated".to_owned()));
    assert!(driver_rule
        .dependencies
        .contains(&"mesa3d-linklib-compiler-generated".to_owned()));
    assert_eq!(
        driver.linklib_output_dir.as_deref(),
        Some("${AROS_BUILD_DIR}/gen/lib/mesa26.0.0")
    );
    let generator = parsed
        .python_outputs
        .iter()
        .find(|candidate| candidate.owner == "linklibs-gallium_v3d-generated")
        .expect("V3D generators");
    assert_eq!(generator.jobs.len(), 22);
    let hidd = parsed
        .targets
        .iter()
        .find(|candidate| candidate.mmake_name == "hidd-v3d")
        .expect("V3D HIDD module");
    assert!(hidd
        .include_dirs
        .contains(&"${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include".to_owned()));
}

#[test]
fn mesa26_vc4_has_closed_driver_on_raspberry_pi_profiles() {
    let root = mesa26_source_root();
    for (cpu, float_abi) in [("arm", "hard"), ("aarch64", "")] {
        let profile = context(cpu, "raspi", "", float_abi);
        let fetches = collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/mmakefile.src"),
            &root,
            &DirVars::load(&root),
            &profile,
            &fetches,
        )
        .unwrap();
        assert!(parsed.capability_errors.is_empty(), "{cpu}: {parsed:#?}");
        let driver = parsed
            .targets
            .iter()
            .find(|candidate| candidate.mmake_name == "linklibs-gallium_vc4")
            .expect("VC4 driver archive");
        assert_eq!(driver.source_files.len(), 43, "{cpu}: {driver:#?}");
        assert!(driver
            .source_files
            .iter()
            .any(|source| source.ends_with("/vc4_tiling_lt_neon")));
        assert!(driver
            .source_files
            .iter()
            .any(|source| source.ends_with("/vc4_tiling_lt")));
        assert!(driver.defines.contains(&"AROS_MESA_MAJOR=26".to_owned()));
        assert!(driver
            .include_dirs
            .contains(&"${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/drivers/vc4".to_owned()));
        let hidd = parsed
            .targets
            .iter()
            .find(|candidate| candidate.mmake_name == "hidd-vc4gallium")
            .expect("VC4 HIDD");
        assert!(hidd
            .source_files
            .iter()
            .all(|source| !source.contains("/src/gallium/drivers/vc4/")));
        assert!(hidd
            .include_dirs
            .contains(&"${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include".to_owned()));
        assert!(hidd.include_dirs.contains(
            &"${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/galliumcoreapi".to_owned()
        ));
    }
}

#[test]
fn mesa26_gallium_hidd_resolves_current_pipe_headers() {
    let root = mesa26_source_root();
    let profile = context("aarch64", "raspi", "", "");
    let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
        &root.join("workbench/hidds/gallium/mmakefile.src"),
        &root,
        &DirVars::load(&root),
        &profile,
        &[],
    )
    .unwrap();
    assert!(parsed.capability_errors.is_empty(), "{parsed:#?}");
    let hidd = parsed
        .targets
        .iter()
        .find(|candidate| candidate.mmake_name == "hidd-gallium")
        .expect("Gallium HIDD");
    assert!(hidd
        .include_dirs
        .contains(&"${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include".to_owned()));
}

#[test]
fn mesa26_glapi_has_a_closed_two_stage_product_on_all_release_profiles() {
    let root = mesa26_source_root();
    for (cpu, platform, cpu32, float_abi) in [
        ("x86_64", "pc", "i386", ""),
        ("arm", "raspi", "", "hard"),
        ("aarch64", "raspi", "", ""),
    ] {
        let context = context(cpu, platform, cpu32, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &context,
        )
        .unwrap();
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join("workbench/libs/mesa/libglapi/mmakefile.src"),
            &root,
            &DirVars::load(&root),
            &context,
            &fetches,
        )
        .unwrap();
        assert!(parsed.capability_errors.is_empty(), "{cpu}: {parsed:#?}");
        let [target] = parsed.targets.as_slice() else {
            panic!("{cpu}: expected one glapi archive: {parsed:#?}");
        };
        assert_eq!(target.mmake_name, "mesa3d-linklib-glapi");
        assert_eq!(
        target.source_files,
        [
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa/glapi/shared-glapi/core",
            "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/mesa/glapi/shared-glapi/public_glapi_wrappers",
        ],
    );
        assert!(target.asm_source_files.is_empty());
        assert_eq!(
            target.linklib_output_dir.as_deref(),
            Some("${AROS_BUILD_DIR}/gen/lib/mesa26.0.0"),
        );
        assert!(target
            .defines
            .contains(&"MAPI_MODE_SHARED_GLAPI".to_owned()));
        assert_eq!(
            target.compile_options,
            ["-std=gnu11", "-fno-strict-aliasing"]
        );

        let [generated] = parsed.python_outputs.as_slice() else {
            panic!("{cpu}: expected one glapi generator: {parsed:#?}");
        };
        assert_eq!(generated.fetch_target, "mesa3d-fetch");
        assert_eq!(generated.jobs.len(), 2);
        assert_eq!(
            generated.jobs[1].depends_on_outputs,
            ["src/mesa/glapi/shared-glapi/shared_glapi_mapi_tmp.h"],
        );
        assert!(generated.jobs[1].local_script);
        assert!(!generated.requires_flex_bison);
        assert_eq!(generated.local_inputs.len(), 1);

        let mut graph = DependencyGraph::new();
        graph.add_target(target.clone());
        graph.add_python_outputs(generated.clone());
        graph.add_fetches(fetches);
        let cmake = generate_cmake(&graph);
        assert!(cmake.contains("LOCAL_SCRIPT"), "{cpu}: {cmake}");
        assert!(
            cmake.contains(
                "DEPENDS_ON_OUTPUTS \"src/mesa/glapi/shared-glapi/shared_glapi_mapi_tmp.h\""
            ),
            "{cpu}: {cmake}"
        );
        assert!(cmake.contains("LOCAL_INPUTS \"${AROS_SOURCE_DIR}/workbench/libs/mesa/libglapi/public_glapi_required_symbols.txt\""), "{cpu}: {cmake}");
    }
}

#[test]
fn mesa26_glapi_rejects_changed_fetch_and_unsupported_version() {
    let root = mesa26_source_root();
    let dirs = DirVars::load(&root);
    let mut profile = context("x86_64", "pc", "i386", "");
    let mut fetches = collect_mmakefile_fetches_with_context(
        &root.join("workbench/libs/mesa/mmakefile.src"),
        &root,
        &profile,
    )
    .unwrap();
    fetches[0].checksums.push_str("-changed");
    let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
        &root.join("workbench/libs/mesa/libglapi/mmakefile.src"),
        &root,
        &dirs,
        &profile,
        &fetches,
    )
    .unwrap();
    assert!(parsed.python_outputs.is_empty());
    assert!(parsed
        .capability_errors
        .iter()
        .any(|error| error.message.contains("central Mesa 26.0.0 fetch")));

    profile.mesa_version = Some("26.0.1".to_owned());
    let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
        &root.join("workbench/libs/mesa/libglapi/mmakefile.src"),
        &root,
        &dirs,
        &profile,
        &[],
    )
    .unwrap();
    assert!(parsed.python_outputs.is_empty());
    assert!(!parsed.capability_errors.is_empty());
}

#[test]
fn mesa26_mesautil_has_closed_archives_and_four_generators() {
    let root = mesa26_source_root();
    for (cpu, platform, cpu32, float_abi) in [
        ("x86_64", "pc", "i386", ""),
        ("arm", "raspi", "", "hard"),
        ("aarch64", "raspi", "", ""),
    ] {
        let profile = context(cpu, platform, cpu32, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join("workbench/libs/mesa/libmesautil/mmakefile.src"),
            &root,
            &DirVars::load(&root),
            &profile,
            &fetches,
        )
        .unwrap();
        assert!(parsed.capability_errors.is_empty(), "{cpu}: {parsed:#?}");
        assert_eq!(parsed.targets.len(), 2, "{cpu}: {parsed:#?}");
        for target in &parsed.targets {
            assert!(target.source_files.len() > 90, "{cpu}: {target:#?}");
            assert_eq!(target.cxx_source_files.len(), 4);
            assert_eq!(
                target.linklib_output_dir.as_deref(),
                Some("${AROS_BUILD_DIR}/gen/lib/mesa26.0.0")
            );
            assert!(target
                .include_dirs
                .contains(&"${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src".to_owned()));
            assert!(target
                .defines
                .contains(&"HAVE_FUNC_ATTRIBUTE_PACKED".to_owned()));
            assert!(target
                .source_files
                .iter()
                .all(|source| !source.contains("..")));
            let has_neon = target
                .source_files
                .iter()
                .any(|source| source.ends_with("/blake3/blake3_neon"));
            assert_eq!(has_neon, cpu == "aarch64");
        }
        let [generated] = parsed.python_outputs.as_slice() else {
            panic!("{cpu}: expected one Mesa util generator: {parsed:#?}");
        };
        assert_eq!(generated.jobs.len(), 4);
        assert_eq!(
            generated
                .jobs
                .iter()
                .map(|job| job.output.as_str())
                .collect::<Vec<_>>(),
            [
                "src/util/format/u_format_gen.h",
                "src/util/format/u_format_pack.h",
                "src/util/format/u_format_table.c",
                "src/util/format_srgb.c",
            ]
        );
        assert!(!generated.requires_flex_bison);
        assert_eq!(generated.python_packages.len(), 1);
        assert_eq!(
            generated.python_packages[0].fetch_target,
            "mesa3d-pyyaml-fetch"
        );
        assert_eq!(generated.python_packages[0].python_path, "lib");
        let mut graph = DependencyGraph::new();
        for target in parsed.targets {
            graph.add_target(target);
        }
        graph.add_python_outputs(generated.clone());
        graph.add_fetches(fetches);
        let cmake = generate_cmake(&graph);
        assert!(
            cmake.contains("src/util/format/u_format_gen.h"),
            "{cpu}: {cmake}"
        );
        assert!(cmake.contains("PACKAGE_FETCH_TARGETS"));
        assert!(cmake.contains("mesa3d-pyyaml-fetch"));
    }
}

#[test]
fn mesa26_mesautil_rejects_missing_or_changed_pyyaml_fetch() {
    let root = mesa26_source_root();
    let profile = context("x86_64", "pc", "i386", "");
    let fetches = collect_mmakefile_fetches_with_context(
        &root.join("workbench/libs/mesa/mmakefile.src"),
        &root,
        &profile,
    )
    .unwrap();
    let parse = |fetches: &[_]| {
        parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join("workbench/libs/mesa/libmesautil/mmakefile.src"),
            &root,
            &DirVars::load(&root),
            &profile,
            fetches,
        )
        .unwrap()
    };
    let without = fetches
        .iter()
        .filter(|fetch| fetch.name != "mesa3d-pyyaml-fetch")
        .cloned()
        .collect::<Vec<_>>();
    assert!(!parse(&without).capability_errors.is_empty());
    let mut altered = fetches;
    let pyyaml = altered
        .iter_mut()
        .find(|fetch| fetch.name == "mesa3d-pyyaml-fetch")
        .unwrap();
    pyyaml.checksums = "pyyaml-6.0.3.tar.gz=sha256:0000".to_owned();
    assert!(!parse(&altered).capability_errors.is_empty());
}

#[test]
fn mesa26_remaining_archives_parse_on_release_profiles() {
    let root = mesa26_source_root();
    for (cpu, platform, cpu32, float_abi) in [
        ("x86_64", "pc", "i386", ""),
        ("arm", "raspi", "", "hard"),
        ("aarch64", "raspi", "", ""),
    ] {
        let profile = context(cpu, platform, cpu32, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        for (directory, mmake) in [
            ("libcompiler", "mesa3d-linklib-compiler"),
            ("libmesa", "mesa3d-linklib-mesa"),
            ("libgalliumaux", "mesa3d-linklib-galliumauxiliary"),
        ] {
            let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
                &root
                    .join("workbench/libs/mesa")
                    .join(directory)
                    .join("mmakefile.src"),
                &root,
                &DirVars::load(&root),
                &profile,
                &fetches,
            )
            .unwrap();
            assert!(
                parsed.capability_errors.is_empty(),
                "{cpu}/{directory}: {:#?}",
                parsed.capability_errors
            );
            assert!(
                parsed
                    .targets
                    .iter()
                    .any(|target| target.mmake_name == mmake),
                "{cpu}/{directory}: {:#?}",
                parsed.targets
            );
        }
    }
}

#[test]
fn mesa26_galliumaux_declares_all_six_generator_outputs() {
    let root = mesa26_source_root();
    let profile = context("x86_64", "pc", "i386", "");
    let fetches = collect_mmakefile_fetches_with_context(
        &root.join("workbench/libs/mesa/mmakefile.src"),
        &root,
        &profile,
    )
    .unwrap();
    let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
        &root.join("workbench/libs/mesa/libgalliumaux/mmakefile.src"),
        &root,
        &DirVars::load(&root),
        &profile,
        &fetches,
    )
    .unwrap();
    assert!(parsed.capability_errors.is_empty(), "{parsed:#?}");
    let [generated] = parsed.python_outputs.as_slice() else {
        panic!("expected one galliumaux generator: {parsed:#?}");
    };
    assert_eq!(generated.jobs.len(), 6);
}

#[test]
fn mesa26_compiler_declares_all_29_generator_outputs() {
    let root = mesa26_source_root();
    let profile = context("x86_64", "pc", "i386", "");
    let fetches = collect_mmakefile_fetches_with_context(
        &root.join("workbench/libs/mesa/mmakefile.src"),
        &root,
        &profile,
    )
    .unwrap();
    let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
        &root.join("workbench/libs/mesa/libcompiler/mmakefile.src"),
        &root,
        &DirVars::load(&root),
        &profile,
        &fetches,
    )
    .unwrap();
    assert!(parsed.capability_errors.is_empty(), "{parsed:#?}");
    let [generated] = parsed.python_outputs.as_slice() else {
        panic!("expected one compiler generator: {parsed:#?}");
    };
    assert_eq!(generated.jobs.len(), 29);
    assert!(generated.requires_flex_bison);
    assert_eq!(generated.python_packages.len(), 2);
}
