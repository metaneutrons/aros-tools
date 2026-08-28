use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars, generate_cmake,
    parse_mmakefile_with_dirs_and_context_and_fetches, DependencyGraph, TargetContext,
};
use std::path::{Path, PathBuf};

const STATIC_SOURCES: &[&str] = &[
    "anon_file",
    "bitscan",
    "blob",
    "build_id",
    "crc32",
    "dag",
    "debug",
    "disk_cache",
    "double",
    "fast_idiv_by_const",
    "format/u_format",
    "format/u_format_bptc",
    "format/u_format_etc",
    "format/u_format_latc",
    "format/u_format_other",
    "format/u_format_rgtc",
    "format/u_format_s3tc",
    "format/u_format_tests",
    "format/u_format_yuv",
    "format/u_format_zs",
    "half_float",
    "hash_table",
    "mesa-sha1",
    "os_time",
    "os_file",
    "os_socket",
    "os_misc",
    "u_process",
    "sha1/sha1",
    "ralloc",
    "rand_xor",
    "rb_tree",
    "register_allocate",
    "rgtc",
    "set",
    "slab",
    "softfloat",
    "sparse_array",
    "string_buffer",
    "strtod",
    "u_atomic",
    "u_math",
    "u_queue",
    "u_vector",
    "u_debug",
    "u_debug_memory",
    "u_cpu_detect",
    "u_mm",
    "vma",
];
const GENERATED_SOURCES: &[&str] = &["format_srgb", "format/u_format_table"];

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
fn production_mesautil_is_cold_fetch_exact_for_all_current_architectures() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let fetch_file = root.join("workbench/libs/mesa/mmakefile.src");
    let zlib_fetch_file = root.join("workbench/libs/z/mmakefile.src");
    let utility_file = root.join("workbench/libs/mesa/libmesautil/mmakefile.src");

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
        ("riscv64", "opensbi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let mut fetches = collect_mmakefile_fetches_with_context(&fetch_file, &root, &context)
            .expect("collect the central Mesa fetch declaration");
        fetches.extend(
            collect_mmakefile_fetches_with_context(&zlib_fetch_file, &root, &context)
                .expect("collect the zlib fetch declaration"),
        );
        assert_eq!(fetches.len(), 4, "{cpu}: {fetches:#?}");

        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &utility_file,
            &root,
            &dirs,
            &context,
            &fetches,
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse libmesautil: {error:#}"));

        assert!(
            parsed.skipped_local_make_includes.is_empty(),
            "{cpu}: {:#?}",
            parsed.skipped_local_make_includes
        );
        assert!(
            parsed.partial_source_lists.is_empty(),
            "{cpu}: {:#?}",
            parsed.partial_source_lists
        );
        assert!(
            parsed
                .skipped_programs
                .iter()
                .all(|diagnostic| !diagnostic.contains("Mesa utility Python generator")),
            "{cpu}: {:#?}",
            parsed.skipped_programs
        );

        let utility_rule = parsed
            .meta_rules
            .iter()
            .find(|rule| rule.name == "mesa3d-linklib-mesautil")
            .unwrap_or_else(|| panic!("{cpu}: missing mesautil dependency rule"));
        assert_eq!(
            utility_rule.dependencies,
            [
                "mesa3d-linklib-mesautil-generated",
                "zlib-fetch",
                "workbench-libs-z-geninc",
            ],
            "{cpu}"
        );
        let dev_utility_rule = parsed
            .meta_rules
            .iter()
            .find(|rule| rule.name == "mesa3d-linklib-mesadevutil")
            .unwrap_or_else(|| panic!("{cpu}: missing mesadevutil dependency rule"));
        assert_eq!(
            dev_utility_rule.dependencies,
            ["zlib-fetch", "workbench-libs-z-geninc",],
            "{cpu}"
        );

        let expected_sources = STATIC_SOURCES
            .iter()
            .map(|source| format!("${{AROS_PORTS_DIR}}/mesa/mesa-20.0.8/src/util/{source}"))
            .chain(GENERATED_SOURCES.iter().map(|source| {
                format!("${{AROS_BUILD_DIR}}/gen/workbench/libs/mesa/20.0.8/src/util/{source}")
            }))
            .collect::<Vec<_>>();
        let expected_common_defines = [
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
        let expected_includes = [
            "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
            "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util",
            "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/util",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util/format",
            "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/util/format",
            "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7",
        ];

        for (mmake_name, target_name, embedded_device) in [
            ("mesa3d-linklib-mesautil", "mesautil", false),
            ("mesa3d-linklib-mesadevutil", "mesadevutil", true),
        ] {
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake_name)
                .unwrap_or_else(|| panic!("{cpu}: missing {mmake_name}: {:#?}", parsed.targets));
            assert_eq!(target.target_name, target_name, "{cpu}: {mmake_name}");
            assert_eq!(target.source_files, expected_sources, "{cpu}: {mmake_name}");
            assert!(target.cxx_source_files.is_empty(), "{cpu}: {mmake_name}");
            assert!(target.objc_source_files.is_empty(), "{cpu}: {mmake_name}");
            assert!(target.asm_source_files.is_empty(), "{cpu}: {mmake_name}");
            assert_eq!(
                target.linklib_output_dir.as_deref(),
                Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8"),
                "{cpu}: {mmake_name}"
            );
            assert!(!target.canonical_linklib_output, "{cpu}: {mmake_name}");

            let mut expected_defines = expected_common_defines.to_vec();
            if cpu == "x86_64" {
                expected_defines.extend(["USE_X86_64_ASM", "USE_SSE41"]);
            }
            expected_defines.extend(["MAPI_MODE_GLAPI", "MAPI_MODE_UTIL"]);
            if embedded_device {
                expected_defines.push("EMBEDDED_DEVICE");
            }
            assert_eq!(
                target
                    .defines
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected_defines,
                "{cpu}: {mmake_name}"
            );
            assert_eq!(
                target
                    .include_dirs
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                expected_includes,
                "{cpu}: {mmake_name}"
            );
            assert_eq!(
                target.compile_options,
                ["-std=gnu11", "-fno-strict-aliasing"],
                "{cpu}: {mmake_name}"
            );
        }

        assert_eq!(STATIC_SOURCES.len() + GENERATED_SOURCES.len(), 51, "{cpu}");

        let [generated] = parsed.python_outputs.as_slice() else {
            panic!(
                "{cpu}: expected one strict Python output group: {:#?}",
                parsed.python_outputs
            );
        };
        assert_eq!(
            generated.owner, "mesa3d-linklib-mesautil-generated",
            "{cpu}"
        );
        assert_eq!(
            generated.source_root, "${AROS_PORTS_DIR}/mesa/mesa-20.0.8",
            "{cpu}"
        );
        assert_eq!(
            generated.build_root, "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8",
            "{cpu}"
        );
        assert_eq!(generated.fetch_target, "mesa3d-fetch", "{cpu}");
        assert_eq!(
            generated.source_inputs,
            [
                "src/util/format/u_format.csv",
                "src/util/format/u_format_pack.py",
                "src/util/format/u_format_parse.py",
            ],
            "{cpu}"
        );
        assert_eq!(
            generated.local_patch_files,
            ["${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff"],
            "{cpu}"
        );
        assert_eq!(
            generated.consumers,
            ["mesa3d-linklib-mesautil", "mesa3d-linklib-mesadevutil"],
            "{cpu}"
        );
        assert_eq!(generated.jobs.len(), 2, "{cpu}");
        assert_eq!(generated.jobs[0].script, "src/util/format_srgb.py", "{cpu}");
        assert_eq!(generated.jobs[0].output, "src/util/format_srgb.c", "{cpu}");
        assert_eq!(
            generated.jobs[0].arguments,
            ["${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util/format/u_format.csv"],
            "{cpu}"
        );
        assert_eq!(
            generated.jobs[1].script, "src/util/format/u_format_table.py",
            "{cpu}"
        );
        assert_eq!(
            generated.jobs[1].output, "src/util/format/u_format_table.c",
            "{cpu}"
        );
        assert_eq!(
            generated.jobs[1].arguments,
            ["${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util/format/u_format.csv"],
            "{cpu}"
        );

        let mut graph = DependencyGraph::new();
        for declaration in parsed.targets.clone() {
            graph.add_target(declaration);
        }
        for declaration in parsed.python_outputs.clone() {
            graph.add_python_outputs(declaration);
        }
        for rule in parsed.meta_rules.clone() {
            graph.add_meta_rule(rule);
        }
        graph.add_fetches(fetches.clone());
        assert!(graph.resolve_port_source_fetches().is_empty(), "{cpu}");
        let cmake = generate_cmake(&graph);

        let fetch_at = cmake
            .find("aros_fetch_archive(NAME \"mesa3d-fetch\"")
            .unwrap_or_else(|| panic!("{cpu}: missing Mesa fetch call"));
        let generator_at = cmake
            .find("aros_generate_python_outputs(\n    OWNER mesa3d-linklib-mesautil-generated")
            .unwrap_or_else(|| panic!("{cpu}: missing utility generator call"));
        let mesautil_at = cmake
            .find("aros_add_linklib(\n    TARGET mesautil\n    MMAKE_ID mesa3d-linklib-mesautil")
            .unwrap_or_else(|| panic!("{cpu}: missing mesautil linklib call"));
        let mesadevutil_at = cmake
            .find(
                "aros_add_linklib(\n    TARGET mesadevutil\n    MMAKE_ID mesa3d-linklib-mesadevutil",
            )
            .unwrap_or_else(|| panic!("{cpu}: missing mesadevutil linklib call"));
        let binding_at = cmake
            .find(
                "aros_bind_python_output_consumers(\n    OWNER \"mesa3d-linklib-mesautil-generated\"",
            )
            .unwrap_or_else(|| panic!("{cpu}: missing Python consumer binding"));
        assert!(fetch_at < generator_at, "{cpu}");
        assert!(generator_at < mesautil_at, "{cpu}");
        assert!(generator_at < mesadevutil_at, "{cpu}");
        assert!(mesautil_at < binding_at, "{cpu}");
        assert!(mesadevutil_at < binding_at, "{cpu}");
        assert!(!cmake.contains("SOURCE_SHA256"));
        assert!(!cmake.contains("LOCAL_PATCH_SHA256"));
        assert!(cmake.contains(
            "        SCRIPT \"src/util/format_srgb.py\"\n\
             \x20       OUTPUT \"src/util/format_srgb.c\"\n\
             \x20       ARGUMENTS \"${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util/format/u_format.csv\""
        ));
        assert!(cmake.contains(
            "        SCRIPT \"src/util/format/u_format_table.py\"\n\
             \x20       OUTPUT \"src/util/format/u_format_table.c\"\n\
             \x20       ARGUMENTS \"${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util/format/u_format.csv\""
        ));
        assert!(cmake
            .contains("    CONSUMERS \"mesa3d-linklib-mesautil\" \"mesa3d-linklib-mesadevutil\""));
        assert!(!cmake.contains("add_custom_target(\"mesa3d-linklib-mesautil-generated\")"));
    }
}
