use aros_transpiler::{
    dirs::DirVars, parse_mmakefile_with_dirs_and_context, ModuleType, TargetContext,
};
use std::path::{Path, PathBuf};

const LIBDRM_SOURCES: &[&str] = &[
    "libdrm/arosdrm",
    "libdrm/arosdrmmode",
    "libdrm/nouveau/nouveau",
    "libdrm/nouveau/pushbuf",
    "libdrm/nouveau/bufctx",
    "libdrm/nouveau/abi16",
];

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
fn production_nouveau_libdrm_is_exact_for_all_current_architectures() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let mmakefile = root.join("workbench/hidds/nouveau/mmakefile.src");

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let parsed = parse_mmakefile_with_dirs_and_context(&mmakefile, &root, &dirs, &context)
            .unwrap_or_else(|error| panic!("{cpu}: parse Nouveau: {error:#}"));

        assert!(
            parsed
                .skipped_local_make_includes
                .iter()
                .all(|issue| !issue.contains("nouveau-libdrm.sources")),
            "{cpu}: {:#?}",
            parsed.skipped_local_make_includes
        );
        assert!(
            parsed
                .skipped_programs
                .iter()
                .all(|issue| !issue.contains("hidd-nouveau-libdrm")),
            "{cpu}: {:#?}",
            parsed.skipped_programs
        );

        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "hidd-nouveau-libdrm")
            .unwrap_or_else(|| panic!("{cpu}: missing libdrm target: {:#?}", parsed.targets));
        assert_eq!(target.target_name, "libdrm_nouveau", "{cpu}");
        assert_eq!(target.module_type, ModuleType::LinkLib, "{cpu}");
        assert_eq!(
            target.source_files,
            LIBDRM_SOURCES
                .iter()
                .map(|source| (*source).to_owned())
                .collect::<Vec<_>>(),
            "{cpu}"
        );
        for source in &target.source_files {
            assert!(
                root.join("workbench/hidds/nouveau")
                    .join(source)
                    .with_extension("c")
                    .is_file(),
                "{cpu}: missing {source}.c"
            );
        }

        assert_eq!(
            target.include_dirs,
            [
                "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
                "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
                "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include",
                "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/uapi",
                "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/include",
                "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm",
                "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/libdrm/nouveau",
                "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/libdrm",
            ],
            "{cpu}"
        );
        assert!(target.defines.is_empty(), "{cpu}: {:#?}", target.defines);
        assert!(
            target.compile_options.is_empty(),
            "{cpu}: {:#?}",
            target.compile_options
        );
        assert!(target.linklib_output_dir.is_none(), "{cpu}");
        assert!(!target.canonical_linklib_output, "{cpu}");
        assert!(target.canonical_linklib_eligible, "{cpu}");
    }
}
