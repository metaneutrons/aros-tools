use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars, generate_cmake,
    parse_mmakefile_with_dirs_and_context_and_fetches, DependencyGraph, TargetContext,
};
use std::path::{Path, PathBuf};

const GLAPI_SOURCES: &[&str] = &[
    "glapi/glapi_dispatch",
    "glapi/glapi_entrypoint",
    "glapi/glapi_getproc",
    "glapi/glapi_nop",
    "glapi/glapi",
    "u_current",
    "u_execmem",
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
fn production_glapi_is_cold_fetch_exact_for_all_current_architectures() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let fetch_file = root.join("workbench/libs/mesa/mmakefile.src");
    let glapi_file = root.join("workbench/libs/mesa/libglapi/mmakefile.src");

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(&fetch_file, &root, &context)
            .expect("collect the central Mesa fetch declaration");
        assert_eq!(fetches.len(), 3, "{cpu}: {fetches:#?}");

        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &glapi_file,
            &root,
            &dirs,
            &context,
            &fetches,
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse libglapi: {error:#}"));

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
                .all(|diagnostic| !diagnostic.contains("glapi Python generator")),
            "{cpu}: {:#?}",
            parsed.skipped_programs
        );

        let dependency_rule = parsed
            .meta_rules
            .iter()
            .find(|rule| rule.name == "mesa3d-linklib-glapi")
            .unwrap_or_else(|| panic!("{cpu}: missing libglapi dependency rule"));
        assert_eq!(
            dependency_rule.dependencies,
            ["mesa3d-fetch", "mesa3d-linklib-glapi-generate"],
            "{cpu}"
        );

        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "mesa3d-linklib-glapi")
            .unwrap_or_else(|| panic!("{cpu}: missing libglapi target: {:#?}", parsed.targets));

        let source_prefix = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi/";
        assert_eq!(
            target.source_files,
            GLAPI_SOURCES
                .iter()
                .map(|source| format!("{source_prefix}{source}"))
                .collect::<Vec<_>>(),
            "{cpu}"
        );
        let expected_asm = if cpu == "x86_64" {
            vec![
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/mapi/glapi/glapi_x86-64"
                    .to_owned(),
            ]
        } else {
            Vec::new()
        };
        assert_eq!(target.asm_source_files, expected_asm, "{cpu}");

        assert_eq!(
            target.linklib_output_dir.as_deref(),
            Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8"),
            "{cpu}"
        );
        assert!(!target.canonical_linklib_output, "{cpu}");
        assert_eq!(
            target
                .include_dirs
                .contains(&"${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa".to_owned()),
            cpu == "x86_64",
            "{cpu}: {:#?}",
            target.include_dirs
        );
        for x86_define in ["USE_X86_64_ASM", "USE_SSE41"] {
            assert_eq!(
                target.defines.contains(&x86_define.to_owned()),
                cpu == "x86_64",
                "{cpu}: {x86_define}: {:#?}",
                target.defines
            );
        }

        let [generated] = parsed.python_outputs.as_slice() else {
            panic!(
                "{cpu}: expected one strict Python output group: {:#?}",
                parsed.python_outputs
            );
        };
        assert_eq!(generated.owner, "mesa3d-linklib-glapi-generate", "{cpu}");
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
            ["src/mapi/glapi/gen/gl_and_es_API.xml"],
            "{cpu}"
        );
        assert_eq!(
            generated.local_patch_files,
            ["${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff"],
            "{cpu}"
        );
        assert_eq!(generated.consumers, ["mesa3d-linklib-glapi"], "{cpu}");

        let outputs = generated
            .jobs
            .iter()
            .map(|job| job.output.as_str())
            .collect::<Vec<_>>();
        let mut expected_outputs = vec![
            "src/mapi/glapi/glapitemp.h",
            "src/mapi/glapi/glapitable.h",
            "src/mapi/glapi/glprocs.h",
        ];
        if cpu == "x86_64" {
            expected_outputs.push("src/mapi/glapi/glapi_x86-64.s");
        }
        assert_eq!(outputs, expected_outputs, "{cpu}");

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
            .find("aros_generate_python_outputs(\n    OWNER mesa3d-linklib-glapi-generate")
            .unwrap_or_else(|| panic!("{cpu}: missing Python generator call"));
        let target_at = cmake
            .find("aros_add_linklib(\n    TARGET glapi\n    MMAKE_ID mesa3d-linklib-glapi")
            .unwrap_or_else(|| panic!("{cpu}: missing glapi linklib call"));
        let binding_at = cmake
            .find("aros_bind_python_output_consumers(\n    OWNER \"mesa3d-linklib-glapi-generate\"")
            .unwrap_or_else(|| panic!("{cpu}: missing Python consumer binding"));
        assert!(fetch_at < generator_at, "{cpu}");
        assert!(generator_at < target_at, "{cpu}");
        assert!(target_at < binding_at, "{cpu}");
        assert!(cmake.contains(
            "    SOURCE_DIR \"${AROS_PORTS_DIR}/mesa/mesa-20.0.8\"\n\
             \x20   LOCAL_PATCH_FILES \"${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff\""
        ));
        assert!(!cmake.contains("SOURCE_SHA256"));
        assert!(!cmake.contains("LOCAL_PATCH_SHA256"));
        assert!(cmake.contains(
            "        SCRIPT \"src/mapi/glapi/gen/gl_apitemp.py\"\n\
             \x20       OUTPUT \"src/mapi/glapi/glapitemp.h\"\n\
             \x20       ARGUMENTS \"-f\" \"${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi/glapi/gen/gl_and_es_API.xml\""
        ));
        assert_eq!(
            cmake.contains("OUTPUT \"src/mapi/glapi/glapi_x86-64.s\""),
            cpu == "x86_64",
            "{cpu}"
        );
        assert!(!cmake.contains("add_custom_target(\"mesa3d-linklib-glapi-generate\")"));
    }
}
