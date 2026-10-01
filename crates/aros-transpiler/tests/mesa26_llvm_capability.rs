mod common;

use aros_transpiler::{
    collect_mmakefile_fetches_with_context, dirs::DirVars,
    parse_mmakefile_with_dirs_and_context_and_fetches, TargetContext,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

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

fn parse_recipe(
    root: &Path,
    recipe: &str,
    profile: &TargetContext,
) -> aros_transpiler::ast::ParsedMmakefile {
    let fetches = collect_mmakefile_fetches_with_context(
        &root.join("workbench/libs/mesa/mmakefile.src"),
        root,
        profile,
    )
    .expect("central Mesa fetches");
    let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
        &root.join(recipe),
        root,
        &DirVars::load(root),
        profile,
        &fetches,
    )
    .expect("Mesa LLVM recipe parses");
    assert!(parsed.capability_errors.is_empty(), "{recipe}: {parsed:#?}");
    parsed
}

fn assert_llvm_compile_contract(target: &aros_transpiler::TargetDefinition, context: &str) {
    for define in [
        "HAVE_LLVM=0x0b00",
        "GALLIVM_USE_ORCJIT=0",
        "LLVM_IS_SHARED=0",
        "MESA_LLVM_VERSION_STRING=\"11.0.0\"",
        "PACKAGE_VERSION=\"26.0.0\"",
        "NDEBUG",
    ] {
        assert!(
            target.defines.iter().any(|actual| actual == define),
            "{context}: missing {define}: {:?}",
            target.defines
        );
    }
    assert!(
        target
            .include_dirs
            .iter()
            .any(|include| include == "${AROS_BUILD_DIR}/gen/external-install/llvm11/include"),
        "{context}: target LLVM headers are absent: {:?}",
        target.include_dirs
    );
    assert!(target
        .compile_options
        .iter()
        .any(|option| option == "-fno-strict-aliasing"));
}

#[test]
fn mesa26_runtime_identity_uses_the_explicit_gl_config_and_override() {
    let root = common::source_root();
    let profile = context("x86_64", "pc", "i386", "");
    let parsed = parse_recipe(&root, "workbench/libs/mesa/mmakefile.src", &profile);
    let target = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "mesa3dgl-library")
        .expect("Mesa implementation library");
    assert_eq!(target.target_name, "mesa3dgl26-0");
    assert_eq!(
        target.config_file.as_deref(),
        Some("${AROS_SOURCE_DIR}/workbench/libs/gl/gl.conf")
    );
    assert_eq!(
        target.config_override_file.as_deref(),
        Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa3dgl.conf")
    );
    let config = target
        .genmodule_linklibs
        .as_ref()
        .expect("explicit genmodule facts");
    assert!(config.enabled && config.has_relative && config.inputs_exact);
    assert_eq!(config.relative_libraries, ["z1", "posixc", "stdc"]);
}

#[test]
fn mesa26_llvm_version_definitions_compile_as_adjacent_c_literals() {
    let root = common::source_root();
    let profile = context("x86_64", "pc", "i386", "");
    let parsed = parse_recipe(
        &root,
        "workbench/libs/mesa/libgalliumvm/mmakefile.src",
        &profile,
    );
    let target = parsed
        .targets
        .iter()
        .find(|target| target.mmake_name == "mesa3d-linklib-galliumvm")
        .expect("Gallivm LLVM target");
    let llvm_version = target
        .defines
        .iter()
        .find(|define| define.starts_with("MESA_LLVM_VERSION_STRING="))
        .expect("Mesa LLVM version compile definition");
    let package_version = target
        .defines
        .iter()
        .find(|define| define.starts_with("PACKAGE_VERSION="))
        .expect("Mesa package version compile definition");

    let temp = tempfile::tempdir().expect("temporary CMake project");
    std::fs::write(
        temp.path().join("CMakeLists.txt"),
        format!(
            "cmake_minimum_required(VERSION 3.16)\n\
             project(mesa26_literal_define_probe C)\n\
             add_executable(mesa26_literal_define_probe probe.c)\n\
             enable_testing()\n\
             add_test(NAME mesa26_literal_define_probe COMMAND mesa26_literal_define_probe)\n\
             target_compile_definitions(mesa26_literal_define_probe PRIVATE\n\
               [==[{llvm_version}]==]\n\
               [==[{package_version}]==]\n\
             )\n"
        ),
    )
    .expect("write CMake project");
    std::fs::write(
        temp.path().join("probe.c"),
        r#"
#include <string.h>
static const char combined_version[] = MESA_LLVM_VERSION_STRING "." PACKAGE_VERSION;
int main(void) { return strcmp(combined_version, "11.0.0.26.0.0") != 0; }
"#,
    )
    .expect("write C literal concatenation probe");

    let build_dir = temp.path().join("build");
    let configure = Command::new("cmake")
        .arg("-S")
        .arg(temp.path())
        .arg("-B")
        .arg(&build_dir)
        .output()
        .expect("run CMake configure with a real C compiler");
    assert!(
        configure.status.success(),
        "CMake configure failed:\n{}\n{}",
        String::from_utf8_lossy(&configure.stdout),
        String::from_utf8_lossy(&configure.stderr)
    );

    let build = Command::new("cmake")
        .arg("--build")
        .arg(&build_dir)
        .arg("--target")
        .arg("mesa26_literal_define_probe")
        .output()
        .expect("build C literal concatenation probe");
    assert!(
        build.status.success(),
        "C literal concatenation compile failed:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let test = Command::new("ctest")
        .arg("--test-dir")
        .arg(&build_dir)
        .arg("--output-on-failure")
        .output()
        .expect("run compiled C literal concatenation probe");
    assert!(
        test.status.success(),
        "C literal concatenation runtime check failed:\n{}\n{}",
        String::from_utf8_lossy(&test.stdout),
        String::from_utf8_lossy(&test.stderr)
    );
}

#[test]
fn mesa26_llvm_source_closures_and_flags_match_current_profiles() {
    let root: PathBuf = common::source_root();
    for (cpu, platform, cpu32, float_abi) in [
        ("x86_64", "pc", "i386", ""),
        ("arm", "raspi", "", "hard"),
        ("aarch64", "raspi", "", ""),
    ] {
        let profile = context(cpu, platform, cpu32, float_abi);
        for (recipe, mmake, c_count, cxx_count, required_source) in [
            (
                "workbench/libs/mesa/libgalliumvm/mmakefile.src",
                "mesa3d-linklib-galliumvm",
                41,
                2,
                "/src/gallium/auxiliary/gallivm/lp_bld_init",
            ),
            (
                "workbench/libs/mesa/libgalliumaux/mmakefile.src",
                "mesa3d-linklib-galliumdrawllvm",
                42,
                0,
                "/src/gallium/auxiliary/draw/draw_llvm",
            ),
            (
                "workbench/libs/mesa/libgalliumaux/mmakefile.src",
                "mesa3d-linklib-galliumtess",
                0,
                2,
                "/src/gallium/auxiliary/tessellator/p_tessellator",
            ),
            (
                "workbench/libs/mesa/libllvmpipe/mmakefile.src",
                "mesa3d-linklib-llvmpipe",
                58,
                0,
                "/src/gallium/drivers/llvmpipe/lp_jit",
            ),
        ] {
            let parsed = parse_recipe(&root, recipe, &profile);
            let target = parsed
                .targets
                .iter()
                .find(|target| target.mmake_name == mmake)
                .unwrap_or_else(|| panic!("{cpu}/{recipe}: missing {mmake}: {parsed:#?}"));
            assert_eq!(target.source_files.len(), c_count, "{cpu}/{mmake}");
            assert_eq!(target.cxx_source_files.len(), cxx_count, "{cpu}/{mmake}");
            assert!(
                target
                    .source_files
                    .iter()
                    .chain(target.cxx_source_files.iter())
                    .any(|source| source.ends_with(required_source)),
                "{cpu}/{mmake}: missing {required_source}"
            );
            assert_llvm_compile_contract(target, &format!("{cpu}/{mmake}"));
            if matches!(
                mmake,
                "mesa3d-linklib-galliumvm"
                    | "mesa3d-linklib-galliumdrawllvm"
                    | "mesa3d-linklib-llvmpipe"
            ) {
                assert!(
                    target.use_libs.iter().any(|library| library == "LLVM"),
                    "{cpu}/{mmake}: logical LLVM provider is absent: {:?}",
                    target.use_libs
                );
            }
            assert_eq!(
                target.linklib_output_dir.as_deref(),
                Some("${AROS_BUILD_DIR}/gen/lib/mesa26.0.0")
            );
            assert!(!target.canonical_linklib_output);
        }

        let parsed = parse_recipe(&root, "workbench/hidds/llvmpipe/mmakefile.src", &profile);
        let hidd = parsed
            .targets
            .iter()
            .find(|target| target.mmake_name == "hidd-llvmpipe")
            .unwrap_or_else(|| panic!("{cpu}: missing hidd-llvmpipe: {parsed:#?}"));
        assert_eq!(hidd.source_files.len(), 3, "{cpu}/hidd-llvmpipe");
        for source in [
            "workbench/hidds/llvmpipe/llvmpipe_init",
            "workbench/hidds/llvmpipe/llvmpipe_galliumclass",
            "workbench/libs/mesa/emul_arosc",
        ] {
            assert!(
                hidd.source_files
                    .iter()
                    .any(|actual| actual.ends_with(source)),
                "{cpu}/hidd-llvmpipe: missing {source}"
            );
        }
        assert_llvm_compile_contract(hidd, &format!("{cpu}/hidd-llvmpipe"));
        assert_eq!(
            hidd.link_options,
            [
                "-L${AROS_BUILD_DIR}/gen/lib/mesa26.0.0",
                "--whole-archive",
                "-lgalliumvm",
                "--no-whole-archive",
            ]
        );
        assert!(
            hidd.use_libs.iter().any(|library| library == "LLVM"),
            "{cpu}/hidd-llvmpipe: logical LLVM provider is absent: {:?}",
            hidd.use_libs
        );
    }
}
