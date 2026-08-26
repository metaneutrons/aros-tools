use aros_transpiler::{
    dirs::DirVars, generate_cmake, parse_mmakefile_with_dirs_and_context_and_fetches,
    DependencyGraph, ModuleType, TargetContext,
};
use std::path::{Path, PathBuf};

const NOUVEAU_DIR: &str = "workbench/hidds/nouveau";
const SOURCE_PREFIX: &str = "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau";

fn source_root() -> PathBuf {
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
fn production_nouveau_drm_is_closed_and_canonical_for_all_current_architectures() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let mmakefile = root.join(NOUVEAU_DIR).join("mmakefile.src");

    let expected_defines = [
        "__KERNEL__",
        "CONFIG_NOUVEAU_DEBUG=5",
        "CONFIG_NOUVEAU_DEBUG_DEFAULT=3",
        "CONFIG_DRM_NOUVEAU_GSP_DEFAULT=1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let expected_includes = [
        "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
        "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/uapi",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/include",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/include/nvkm",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/nvkm",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/nvkm/subdev/gsp",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let expected_options = [
        "-O2",
        "-Wno-uninitialized",
        "-Wno-strict-aliasing",
        "-Wno-unused-but-set-variable",
        "-Wno-unused-variable",
        "-Wno-unused-function",
        "-Wno-missing-braces",
        "-std=gnu11",
        "-fno-strict-aliasing",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &mmakefile,
            &root,
            &dirs,
            &context,
            &[],
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse Nouveau DRM: {error:#}"));
        assert!(
            parsed
                .skipped_programs
                .iter()
                .all(|diagnostic| !diagnostic.contains("hidd-nouveau-drm")),
            "{cpu}: {:#?}",
            parsed.skipped_programs
        );
        assert!(
            parsed
                .partial_source_lists
                .iter()
                .all(|diagnostic| !diagnostic.contains("hidd-nouveau-drm")),
            "{cpu}: {:#?}",
            parsed.partial_source_lists
        );

        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "hidd-nouveau-drm")
            .unwrap_or_else(|| panic!("{cpu}: missing Nouveau DRM target: {:#?}", parsed.targets));
        assert_eq!(target.target_name, "drm_nouveau", "{cpu}");
        assert_eq!(target.module_type, ModuleType::LinkLib, "{cpu}");
        assert_eq!(target.source_files.len(), 825, "{cpu}");
        assert_eq!(
            target.source_files.first().map(String::as_str),
            Some("${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/display/drm_dp_helper"),
            "{cpu}"
        );
        assert_eq!(
            target.source_files.get(66).map(String::as_str),
            Some("${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm-aros/nouveau/nouveau_aros_stubs"),
            "{cpu}"
        );
        assert_eq!(
            target.source_files.last().map(String::as_str),
            Some("${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/nvkm/subdev/volt/nv40"),
            "{cpu}"
        );
        let missing_sources = target
            .source_files
            .iter()
            .filter(|source| {
                !source.starts_with(SOURCE_PREFIX)
                    || !root
                        .join(NOUVEAU_DIR)
                        .join(
                            source
                                .strip_prefix(SOURCE_PREFIX)
                                .unwrap()
                                .trim_start_matches('/'),
                        )
                        .with_extension("c")
                        .is_file()
            })
            .collect::<Vec<_>>();
        assert!(
            missing_sources.is_empty(),
            "{cpu}: source lane contains non-materializable sources: {missing_sources:#?}"
        );
        assert!(target.cxx_source_files.is_empty(), "{cpu}");
        assert!(target.objc_source_files.is_empty(), "{cpu}");
        assert!(target.asm_source_files.is_empty(), "{cpu}");
        assert!(target.use_libs.is_empty(), "{cpu}");
        assert!(target.dependencies.is_empty(), "{cpu}");
        assert!(target.linklib_output_dir.is_none(), "{cpu}");
        assert!(target.canonical_linklib_eligible, "{cpu}");
        assert!(target.canonical_linklib_output, "{cpu}");
        assert_eq!(target.defines, expected_defines, "{cpu}");
        assert_eq!(target.include_dirs, expected_includes, "{cpu}");
        assert_eq!(target.compile_options, expected_options, "{cpu}");

        let mut graph = DependencyGraph::new();
        graph.add_target(target.clone());
        let cmake = generate_cmake(&graph);
        let target_at = cmake
            .find("aros_add_linklib(\n    TARGET drm_nouveau\n    MMAKE_ID hidd-nouveau-drm")
            .unwrap_or_else(|| panic!("{cpu}: missing generated Nouveau DRM linklib"));
        let target_end = cmake[target_at..].find("\n)\n").unwrap() + target_at;
        let target_block = &cmake[target_at..target_end];
        assert!(
            target_block.contains("    CANONICAL_OUTPUT"),
            "{cpu}: {target_block}"
        );
        assert!(
            !target_block.contains("    OUTPUT_DIR"),
            "{cpu}: {target_block}"
        );
        assert!(
            target_block.contains("    COMPILE_OPTIONS \"-O2\""),
            "{cpu}: missing legacy DRM optimization in {target_block}"
        );
        assert!(
            target_block
                .contains("${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/display/drm_dp_helper"),
            "{cpu}: {target_block}"
        );
    }
}
