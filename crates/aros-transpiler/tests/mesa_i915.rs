use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars,
    parse_mmakefile_with_dirs_and_context_and_fetches, TargetContext,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const I915_BASENAME_DIGEST: &str =
    "bee7eef93c1df90d4356d8610fe0157288a9a09d9ec0846bf096506a18fa8d69";

const I915_BASENAMES: &[&str] = &[
    "i915_blit.c",
    "i915_clear.c",
    "i915_context.c",
    "i915_debug.c",
    "i915_debug_fp.c",
    "i915_flush.c",
    "i915_fpc_emit.c",
    "i915_fpc_optimize.c",
    "i915_fpc_translate.c",
    "i915_prim_emit.c",
    "i915_prim_vbuf.c",
    "i915_query.c",
    "i915_resource_buffer.c",
    "i915_resource.c",
    "i915_resource_texture.c",
    "i915_screen.c",
    "i915_state.c",
    "i915_state_derived.c",
    "i915_state_dynamic.c",
    "i915_state_emit.c",
    "i915_state_fpc.c",
    "i915_state_immediate.c",
    "i915_state_sampler.c",
    "i915_state_static.c",
    "i915_surface.c",
];

const SOFTPIPE_BASENAME_DIGEST: &str =
    "588a62fb5c38aeff71cb45f320bf5400ec66a784823e7dde3f4580fb6fe8d30b";

const SOFTPIPE_BASENAMES: &[&str] = &[
    "sp_buffer.c",
    "sp_clear.c",
    "sp_context.c",
    "sp_compute.c",
    "sp_draw_arrays.c",
    "sp_fence.c",
    "sp_flush.c",
    "sp_fs_exec.c",
    "sp_image.c",
    "sp_prim_vbuf.c",
    "sp_quad_blend.c",
    "sp_quad_depth_test.c",
    "sp_quad_fs.c",
    "sp_quad_pipe.c",
    "sp_quad_stipple.c",
    "sp_query.c",
    "sp_screen.c",
    "sp_setup.c",
    "sp_state_blend.c",
    "sp_state_clip.c",
    "sp_state_derived.c",
    "sp_state_image.c",
    "sp_state_rasterizer.c",
    "sp_state_sampler.c",
    "sp_state_shader.c",
    "sp_state_so.c",
    "sp_state_surface.c",
    "sp_state_vertex.c",
    "sp_surface.c",
    "sp_tex_sample.c",
    "sp_tex_tile_cache.c",
    "sp_texture.c",
    "sp_tile_cache.c",
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

fn basename_digest(basenames: &[String]) -> String {
    let mut hasher = Sha256::new();
    for basename in basenames {
        hasher.update(basename.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

#[test]
fn production_i915_is_exact_for_all_current_architectures() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let fetch_file = root.join("workbench/libs/mesa/mmakefile.src");
    let i915_file = root.join("workbench/devs/monitors/IntelGMA/i915/mmakefile.src");

    for (cpu, platform, float_abi) in [
        ("x86_64", "pc", ""),
        ("arm", "raspi", "hard"),
        ("aarch64", "raspi", ""),
    ] {
        let context = target_context(cpu, platform, float_abi);
        let fetches = collect_mmakefile_fetches_with_context(&fetch_file, &root, &context)
            .expect("collect the central Mesa fetch declaration");
        assert_eq!(fetches.len(), 3, "{cpu}: {fetches:#?}");
        let mesa_fetch = fetches
            .iter()
            .find(|fetch| fetch.name == "mesa3d-fetch")
            .unwrap();
        assert_eq!(mesa_fetch.destination, "${AROS_PORTS_DIR}/mesa", "{cpu}");

        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &i915_file, &root, &dirs, &context, &fetches,
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse i915: {error:#}"));

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
        let dependency_rule = parsed
            .meta_rules
            .iter()
            .find(|rule| rule.name == "intelgma-linklibs-gallium_i915")
            .unwrap_or_else(|| panic!("{cpu}: missing i915 dependency rule"));
        assert_eq!(dependency_rule.dependencies, ["mesa3d-fetch"], "{cpu}");
        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "intelgma-linklibs-gallium_i915")
            .unwrap_or_else(|| panic!("{cpu}: missing i915 target: {:#?}", parsed.targets));

        assert_eq!(target.source_files.len(), 25, "{cpu}");
        let basenames = target
            .source_files
            .iter()
            .map(|source| {
                let stem = source
                    .rsplit('/')
                    .next()
                    .unwrap_or_else(|| panic!("{cpu}: source without basename: {source}"));
                format!("{stem}.c")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            basenames,
            I915_BASENAMES
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
            "{cpu}"
        );
        assert_eq!(basename_digest(&basenames), I915_BASENAME_DIGEST, "{cpu}");
        assert!(
            target.source_files.iter().all(|source| source
                .starts_with("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/i915/")),
            "{cpu}: {:#?}",
            target.source_files
        );

        assert_eq!(
            target.linklib_output_dir.as_deref(),
            Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8"),
            "{cpu}"
        );
        assert!(!target.canonical_linklib_output, "{cpu}");
        assert_eq!(
            target
                .compile_options
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            [
                "-std=gnu11",
                "-fno-strict-aliasing",
                "-Wno-unused-value",
                "-Wno-unused-variable",
                "-Wno-strict-aliasing",
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            "{cpu}: {:#?}",
            target.compile_options
        );

        assert_eq!(
            target
                .include_dirs
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            [
                "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
                "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            "{cpu}: {:#?}",
            target.include_dirs
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
            "MAPI_MODE_GLAPI",
            "MAPI_MODE_UTIL",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        if cpu == "x86_64" {
            expected_defines.extend(["USE_X86_64_ASM", "USE_SSE41"]);
        }
        assert_eq!(
            target
                .defines
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            expected_defines,
            "{cpu}: {:#?}",
            target.defines
        );
        for x86_define in ["USE_X86_64_ASM", "USE_SSE41"] {
            assert_eq!(
                target.defines.contains(&x86_define.to_owned()),
                cpu == "x86_64",
                "{cpu}: {x86_define}: {:#?}",
                target.defines
            );
        }
    }
}

#[test]
fn production_softpipe_is_exact_for_all_current_architectures() {
    let root = source_root();
    let dirs = DirVars::load(&root);
    let fetch_file = root.join("workbench/libs/mesa/mmakefile.src");
    let softpipe_file = root.join("workbench/hidds/softpipe/mmakefile.src");

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
            &softpipe_file,
            &root,
            &dirs,
            &context,
            &fetches,
        )
        .unwrap_or_else(|error| panic!("{cpu}: parse softpipe: {error:#}"));

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
        let dependency_rule = parsed
            .meta_rules
            .iter()
            .find(|rule| rule.name == "linklibs-gallium_softpipe")
            .unwrap_or_else(|| panic!("{cpu}: missing softpipe dependency rule"));
        assert_eq!(
            dependency_rule.dependencies,
            ["mesa3dgl-linklibs", "mesa3d-fetch"],
            "{cpu}"
        );
        let target = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "linklibs-gallium_softpipe")
            .unwrap_or_else(|| panic!("{cpu}: missing softpipe target: {:#?}", parsed.targets));

        let basenames = target
            .source_files
            .iter()
            .map(|source| {
                let stem = source
                    .rsplit('/')
                    .next()
                    .unwrap_or_else(|| panic!("{cpu}: source without basename: {source}"));
                format!("{stem}.c")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            basenames,
            SOFTPIPE_BASENAMES
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
            "{cpu}"
        );
        assert_eq!(
            basename_digest(&basenames),
            SOFTPIPE_BASENAME_DIGEST,
            "{cpu}"
        );
        assert!(
            target.source_files.iter().all(|source| source
                .starts_with("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/softpipe/")),
            "{cpu}: {:#?}",
            target.source_files
        );

        assert_eq!(
            target.linklib_output_dir.as_deref(),
            Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8"),
            "{cpu}"
        );
        assert!(!target.canonical_linklib_output, "{cpu}");
        assert_eq!(
            target
                .compile_options
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            ["-std=gnu11", "-fno-strict-aliasing"]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            "{cpu}: {:#?}",
            target.compile_options
        );

        for required in [
            "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
            "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
            "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/src/compiler/nir",
        ] {
            assert!(
                target.include_dirs.contains(&required.to_owned()),
                "{cpu}: missing {required}: {:#?}",
                target.include_dirs
            );
        }

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
            "MAPI_MODE_GLAPI",
            "MAPI_MODE_UTIL",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        if cpu == "x86_64" {
            expected_defines.extend(["USE_X86_64_ASM", "USE_SSE41"]);
        }
        assert_eq!(
            target
                .defines
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            expected_defines,
            "{cpu}: {:#?}",
            target.defines
        );
        for x86_define in ["USE_X86_64_ASM", "USE_SSE41"] {
            assert_eq!(
                target.defines.contains(&x86_define.to_owned()),
                cpu == "x86_64",
                "{cpu}: {x86_define}: {:#?}",
                target.defines
            );
        }
    }
}
