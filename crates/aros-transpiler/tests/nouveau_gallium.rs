use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars, generate_cmake,
    parse_mmakefile_with_dirs_and_context_and_fetches, DependencyGraph, ModuleType, TargetContext,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const NOUVEAU_DIR: &str = "workbench/hidds/nouveau";
const MESA_SOURCE_PREFIX: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium";
const MMAKE_SHA256: &str = "4c0fd8b41d3590b4303c84be7c670220567b8b86e7e29fd6d05c4a36c7d4ee56";
const SOURCE_MANIFEST_SHA256: &str =
    "86ffb0c1e959615833b9d7b937dfcaf237c5f25da8d5706d8354ba5314acc15f";

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

fn sha256(path: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

#[test]
fn production_nouveau_gallium_is_closed_and_canonical_for_all_current_architectures() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let mmakefile = root.join(NOUVEAU_DIR).join("mmakefile.src");
    let mesa_mmakefile = root.join("workbench/libs/mesa/mmakefile.src");

    assert_eq!(sha256(&mmakefile), MMAKE_SHA256);
    assert_eq!(
        sha256(
            &root
                .join(NOUVEAU_DIR)
                .join("nouveau-gallium-20.0.8.sources")
        ),
        SOURCE_MANIFEST_SHA256
    );

    let expected_includes = [
        "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
        "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/nouveau",
        "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/libdrm/nouveau",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/libdrm",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/libdrm/nouveau",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/uapi/drm",
        "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let expected_options = [
        "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>",
        "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>",
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
        let fetches = collect_mmakefile_fetches_with_context(&mesa_mmakefile, &root, &context)
            .unwrap_or_else(|error| panic!("{cpu}: collect Mesa fetch: {error:#}"));
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &mmakefile, &root, &dirs, &context, &fetches,
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse Nouveau Gallium: {error:#}"));
        assert!(
            parsed
                .skipped_programs
                .iter()
                .all(|diagnostic| !diagnostic.contains("hidd-nouveau-gallium")),
            "{cpu}: {:#?}",
            parsed.skipped_programs
        );
        assert!(
            parsed
                .partial_source_lists
                .iter()
                .all(|diagnostic| !diagnostic.contains("hidd-nouveau-gallium")),
            "{cpu}: {:#?}",
            parsed.partial_source_lists
        );

        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "hidd-nouveau-gallium")
            .unwrap_or_else(|| {
                panic!(
                    "{cpu}: missing Nouveau Gallium target: {:#?}",
                    parsed.targets
                )
            });
        assert_eq!(target.target_name, "gallium_nouveau", "{cpu}");
        assert_eq!(target.module_type, ModuleType::LinkLib, "{cpu}");
        assert_eq!(target.source_files.len(), 81, "{cpu}");
        assert_eq!(target.cxx_source_files.len(), 24, "{cpu}");
        assert_eq!(
            target.source_files.first().map(String::as_str),
            Some("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/winsys/nouveau/drm/nouveau_drm_winsys"),
            "{cpu}"
        );
        assert_eq!(
            target.source_files.last().map(String::as_str),
            Some(
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/nouveau/nvc0/nve4_compute"
            ),
            "{cpu}"
        );
        assert_eq!(
            target.cxx_source_files.first().map(String::as_str),
            Some("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/nouveau/codegen/nv50_ir"),
            "{cpu}"
        );
        assert_eq!(
            target.cxx_source_files.last().map(String::as_str),
            Some("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/nouveau/codegen/nv50_ir_target_nvc0"),
            "{cpu}"
        );
        assert!(
            target
                .source_files
                .iter()
                .chain(&target.cxx_source_files)
                .all(|source| source.starts_with(MESA_SOURCE_PREFIX)),
            "{cpu}: {target:#?}"
        );
        assert!(target.objc_source_files.is_empty(), "{cpu}");
        assert!(target.asm_source_files.is_empty(), "{cpu}");
        assert!(target.use_libs.is_empty(), "{cpu}");
        assert!(target.dependencies.is_empty(), "{cpu}");
        assert!(target.linklib_output_dir.is_none(), "{cpu}");
        assert!(target.canonical_linklib_eligible, "{cpu}");
        assert!(target.canonical_linklib_output, "{cpu}");
        assert_eq!(target.include_dirs, expected_includes, "{cpu}");
        assert_eq!(target.compile_options, expected_options, "{cpu}");
        assert!(
            !target
                .include_dirs
                .iter()
                .any(|include| include.contains("cxx-compat")),
            "{cpu}: Nouveau requires a real target C++ standard library"
        );

        let mut expected_defines = [
            "__STDC_CONSTANT_MACROS",
            "__STDC_FORMAT_MACROS",
            "__STDC_LIMIT_MACROS",
            "_GNU_SOURCE",
            "HAVE_PTHREAD",
            "HAVE_TIMESPEC_GET",
            "POSIXC_SLOWSTACK_VAARGS",
            "USE_GCC_ATOMIC_BUILTINS",
            "HAVE_ZLIB",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        if cpu == "x86_64" {
            expected_defines.extend(["USE_X86_64_ASM".to_owned(), "USE_SSE41".to_owned()]);
        }
        expected_defines.extend(["MAPI_MODE_GLAPI".to_owned(), "MAPI_MODE_UTIL".to_owned()]);
        assert_eq!(target.defines, expected_defines, "{cpu}");

        let mut graph = DependencyGraph::new();
        graph.add_fetches(fetches);
        graph.add_target(target.clone());
        assert!(graph.resolve_port_source_fetches().is_empty(), "{cpu}");
        assert!(
            graph
                .meta_targets
                .get("hidd-nouveau-gallium")
                .is_some_and(|dependencies| dependencies.contains("mesa3d-fetch")),
            "{cpu}: {:#?}",
            graph.meta_targets
        );
        let cmake = generate_cmake(&graph);
        let target_at = cmake
            .find(
                "aros_add_linklib(\n    TARGET gallium_nouveau\n    MMAKE_ID hidd-nouveau-gallium",
            )
            .unwrap_or_else(|| panic!("{cpu}: missing generated Nouveau Gallium linklib"));
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
            target_block.contains("CXX_SOURCES \"${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/nouveau/codegen/nv50_ir\""),
            "{cpu}: {target_block}"
        );
        assert!(
            target_block.contains("COMPILE_OPTIONS \"$<$<COMPILE_LANGUAGE:C>:-std=gnu11>\" \"$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>\" \"-fno-strict-aliasing\""),
            "{cpu}: {target_block}"
        );
    }
}
