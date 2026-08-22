use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars, generate_cmake,
    parse_mmakefile_with_dirs_and_context_and_fetches, DependencyGraph, ModuleType, TargetContext,
};
use std::path::{Path, PathBuf};

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
fn production_sse41_is_sourceful_on_x86_and_an_empty_archive_on_raspberry_pi() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let fetch_file = root.join("workbench/libs/mesa/mmakefile.src");
    let library_file = root.join("workbench/libs/mesa/libmesa/mmakefile.src");

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(&fetch_file, &root, &context)
            .expect("collect the central Mesa fetch declaration");
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &library_file,
            &root,
            &dirs,
            &context,
            &fetches,
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse libmesa: {error:#}"));

        assert!(
            parsed
                .skipped_local_make_includes
                .iter()
                .all(|diagnostic| !diagnostic.contains("mesa-sse41-20.0.8.sources")),
            "{cpu}: {:#?}",
            parsed.skipped_local_make_includes
        );
        assert!(
            parsed
                .skipped_programs
                .iter()
                .all(|diagnostic| !diagnostic.contains("Mesa SSE4.1")),
            "{cpu}: {:#?}",
            parsed.skipped_programs
        );
        let dependency = parsed
            .meta_rules
            .iter()
            .find(|rule| rule.name == "mesa3d-linklib-mesa-sse41")
            .unwrap_or_else(|| panic!("{cpu}: missing SSE4.1 fetch edge"));
        assert_eq!(dependency.dependencies, ["mesa3d-fetch"], "{cpu}");

        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "mesa3d-linklib-mesa-sse41")
            .unwrap_or_else(|| panic!("{cpu}: missing SSE4.1 target: {:#?}", parsed.targets));
        assert_eq!(target.target_name, "mesa-sse41", "{cpu}");
        assert_eq!(target.module_type, ModuleType::LinkLib, "{cpu}");
        assert_eq!(target.empty_archive, cpu != "x86_64", "{cpu}");
        let expected_sources = if cpu == "x86_64" {
            vec![
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main/streaming-load-memcpy".to_owned(),
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main/sse_minmax".to_owned(),
            ]
        } else {
            Vec::new()
        };
        assert_eq!(target.source_files, expected_sources, "{cpu}");
        assert!(target.cxx_source_files.is_empty(), "{cpu}");
        assert!(target.objc_source_files.is_empty(), "{cpu}");
        assert!(target.asm_source_files.is_empty(), "{cpu}");
        assert_eq!(
            target.linklib_output_dir.as_deref(),
            Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8"),
            "{cpu}"
        );
        assert!(!target.canonical_linklib_output, "{cpu}");
        assert!(!target.canonical_linklib_eligible, "{cpu}");

        let mut expected_defines = vec![
            "__STDC_CONSTANT_MACROS",
            "__STDC_FORMAT_MACROS",
            "__STDC_LIMIT_MACROS",
            "_GNU_SOURCE",
            "HAVE_PTHREAD",
            "HAVE_TIMESPEC_GET",
            "POSIXC_SLOWSTACK_VAARGS",
            "USE_GCC_ATOMIC_BUILTINS",
            "HAVE_ZLIB",
        ];
        if cpu == "x86_64" {
            expected_defines.extend(["USE_X86_64_ASM", "USE_SSE41"]);
        }
        expected_defines.extend(["MAPI_MODE_GLAPI", "MAPI_MODE_UTIL"]);
        assert_eq!(
            target
                .defines
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            expected_defines,
            "{cpu}"
        );
        assert_eq!(
            target
                .include_dirs
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
                "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
                "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/mesa",
                "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/mesa/main",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
                "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/glsl",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/compiler/glsl",
                "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
            ],
            "{cpu}"
        );
        let mut expected_options = vec!["-std=gnu11", "-fno-strict-aliasing"];
        if cpu == "x86_64" {
            expected_options.push("-msse4.1");
        }
        assert_eq!(
            target
                .compile_options
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            expected_options,
            "{cpu}"
        );

        let mut graph = DependencyGraph::new();
        for target in parsed.targets {
            graph.add_target(target);
        }
        for rule in parsed.meta_rules {
            graph.add_meta_rule(rule);
        }
        graph.add_fetches(fetches);
        assert!(graph.resolve_port_source_fetches().is_empty(), "{cpu}");
        let cmake = generate_cmake(&graph);
        let start = cmake
            .find(
                "aros_add_linklib(\n    TARGET mesa-sse41\n    MMAKE_ID mesa3d-linklib-mesa-sse41",
            )
            .unwrap_or_else(|| panic!("{cpu}: missing generated SSE4.1 call"));
        let call = &cmake[start..];
        let call = &call[..call
            .find("\n)\n")
            .unwrap_or_else(|| panic!("{cpu}: unterminated generated SSE4.1 call"))];
        assert_eq!(
            call.contains("\n    EMPTY_ARCHIVE"),
            cpu != "x86_64",
            "{cpu}"
        );
        assert_eq!(call.contains("\n    SOURCES "), cpu == "x86_64", "{cpu}");
        assert_eq!(call.contains("-msse4.1"), cpu == "x86_64", "{cpu}");
    }
}
