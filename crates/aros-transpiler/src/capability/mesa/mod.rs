//! What every Mesa 20.0.8 lane shares.
//!
//! Mesa arrives as a fetched port, and the declarations that build from it are
//! modelled rather than read generically: the source lists are manifests, the
//! generated files come from a Python driver, and the compile flags are decided
//! by the mmakefile and `mesa.cfg` rather than by the macro. Four lanes sit on
//! top of this module -- the SSE4.1 archive, the glapi and mesautil generators,
//! and the remaining Mesa 20 generator groups -- and Nouveau Gallium does too,
//! because it compiles Mesa sources with the same contract.
//!
//! The names here dropped their `mesa20_` prefix on the way out of `parser.rs`:
//! inside a module called `mesa` it only stuttered.

use super::manifest_inventory;

pub mod generators;
pub mod mesa20;
pub mod sse41;
use crate::parser::{EvaluatedSources, TargetContext};
use std::collections::HashSet;
use std::path::Path;

pub(crate) const SOURCE_ROOT: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8";
pub(crate) const BUILD_ROOT: &str = "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8";
pub(crate) const PRIVATE_LIBDIR: &str = "${AROS_BUILD_DIR}/gen/lib/mesa20.0.8";
pub(crate) const CXX_COMPAT_NEW: &str = "workbench/libs/mesa/libcompiler/cxx-compat/new";

pub(crate) fn inventory_stems(
    root: &Path,
    relative: &str,
    variable: &str,
    suffix: &str,
    prefix: &str,
) -> std::result::Result<Vec<String>, String> {
    manifest_inventory(root, relative, variable)?
        .into_iter()
        .map(|source| {
            source
                .strip_suffix(suffix)
                .map(|stem| format!("{prefix}/{stem}"))
                .ok_or_else(|| format!("{relative} {variable} entry lacks {suffix}: {source}"))
        })
        .collect()
}

pub(crate) fn inventory_paths(
    root: &Path,
    relative: &str,
    variable: &str,
    suffix: &str,
    prefix: &str,
) -> std::result::Result<Vec<String>, String> {
    manifest_inventory(root, relative, variable)?
        .into_iter()
        .map(|source| {
            if source.ends_with(suffix) {
                Ok(format!("{prefix}/{source}"))
            } else {
                Err(format!(
                    "{relative} {variable} entry lacks {suffix}: {source}"
                ))
            }
        })
        .collect()
}

pub(crate) fn current_profile(
    target: Option<&TargetContext>,
) -> std::result::Result<&'static str, String> {
    let Some(profile) = target else {
        return Err("Mesa 20.0.8 archive capability requires a concrete target profile".to_owned());
    };
    match (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    ) {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => {
            Ok("x86_64")
        }
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => {
            Ok("arm")
        }
        (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => {
            Ok("aarch64")
        }
        _ => Err(format!(
            "Mesa 20.0.8 archive capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

pub(crate) struct CompileContract {
    pub(crate) defines: Vec<String>,
    pub(crate) includes: Vec<String>,
    pub(crate) options: Vec<String>,
}

pub(crate) fn base_defines(profile: &str) -> Vec<String> {
    let mut defines = [
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
    if profile == "x86_64" {
        defines.extend(["USE_X86_64_ASM".to_owned(), "USE_SSE41".to_owned()]);
    }
    defines.extend(["MAPI_MODE_GLAPI".to_owned(), "MAPI_MODE_UTIL".to_owned()]);
    defines
}

pub(crate) fn compile_contract(
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<CompileContract>, String> {
    let supported = matches!(
        (relative_dir.to_str(), mmake),
        (
            Some("workbench/libs/mesa/libcompiler"),
            "mesa3d-linklib-compiler"
        ) | (
            Some("workbench/libs/mesa/libgalliumaux"),
            "mesa3d-linklib-galliumauxiliary"
        ) | (Some("workbench/libs/mesa/libmesa"), "mesa3d-linklib-mesa")
            | (
                Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
                "linklibs-gallium_vc4"
            )
    );
    if !supported {
        return Ok(None);
    }
    let profile = current_profile(target)?;
    let base = [
        "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
        "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
    ];
    let (mut defines, includes, options) = match (relative_dir.to_str(), mmake) {
        (Some("workbench/libs/mesa/libcompiler"), "mesa3d-linklib-compiler") => (
            base_defines(profile),
            base.into_iter()
                .chain([
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/glsl",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/glsl/glcpp",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/spirv",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/glsl",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/glsl/glcpp",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/spirv",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
                ])
                .map(str::to_owned)
                .collect(),
            vec![
                "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-I${CMAKE_SOURCE_DIR}/workbench/libs/mesa/libcompiler/cxx-compat>".to_owned(),
                "-fno-strict-aliasing".to_owned(),
            ],
        ),
        (
            Some("workbench/libs/mesa/libgalliumaux"),
            "mesa3d-linklib-galliumauxiliary",
        ) => (
            base_defines(profile),
            base.into_iter()
                .chain([
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary/util",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary/indices",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                ])
                .map(str::to_owned)
                .collect(),
            vec![
                "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>".to_owned(),
                "-fno-strict-aliasing".to_owned(),
            ],
        ),
        (Some("workbench/libs/mesa/libmesa"), "mesa3d-linklib-mesa") => (
            base_defines(profile),
            base.into_iter()
                .chain([
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/mesa",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/mesa/main",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/glsl",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/glsl",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main",
                    "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
                ])
                .map(str::to_owned)
                .collect(),
            vec![
                "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>".to_owned(),
                "$<$<COMPILE_LANGUAGE:CXX>:-I${CMAKE_SOURCE_DIR}/workbench/libs/mesa/libcompiler/cxx-compat>".to_owned(),
                "-fno-strict-aliasing".to_owned(),
            ],
        ),
        (
            Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
            "linklibs-gallium_vc4",
        ) if profile != "x86_64" => {
            let mut defines = base_defines(profile);
            defines.extend(
                [
                    "GALLIUM_VC4",
                    "HAVE_STRUCT_TIMESPEC",
                    "USE_ARM_ASM",
                    "GCA_CONSUMER_MODULE",
                ]
                .into_iter()
                .map(str::to_owned),
            );
            (
                defines,
                base.into_iter()
                    .chain([
                        "${CMAKE_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/drm_compat",
                        "${CMAKE_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/drivers/vc4",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/include",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium/auxiliary",
                        "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/broadcom",
                        "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/broadcom",
                        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir",
                        "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8/src/compiler/nir",
                        "${CMAKE_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/include",
                    ])
                    .map(str::to_owned)
                    .collect(),
                vec!["-std=gnu99".to_owned(), "-fno-strict-aliasing".to_owned()],
            )
        }
        _ => return Ok(None),
    };
    if mmake == "mesa3d-linklib-mesa" {
        defines.extend([
            "PACKAGE_VERSION=\"20.0.8\"".to_owned(),
            "PACKAGE_BUGREPORT=\"https://bugs.freedesktop.org/enter_bug.cgi?product=Mesa\""
                .to_owned(),
        ]);
    }
    // Keep declarations deterministic even when a legacy include repeats one
    // of the common paths.
    let mut seen = HashSet::new();
    defines.retain(|define| seen.insert(define.clone()));
    Ok(Some(CompileContract {
        defines,
        includes,
        options,
    }))
}

pub(crate) fn remaining_linklib_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<EvaluatedSources>, String> {
    let supported_declaration = matches!(
        (relative_dir.to_str(), mmake),
        (
            Some("workbench/libs/mesa/libcompiler"),
            "mesa3d-linklib-compiler"
        ) | (
            Some("workbench/libs/mesa/libgalliumaux"),
            "mesa3d-linklib-galliumauxiliary"
        ) | (Some("workbench/libs/mesa/libmesa"), "mesa3d-linklib-mesa")
            | (
                Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
                "linklibs-gallium_vc4"
            )
    );
    if !supported_declaration {
        return Ok(None);
    }
    let profile = current_profile(target)?;
    let mut sources = EvaluatedSources {
        declared: true,
        ..EvaluatedSources::default()
    };
    match (relative_dir.to_str(), mmake) {
        (Some("workbench/libs/mesa/libcompiler"), "mesa3d-linklib-compiler") => {
            const MANIFEST: &str = "workbench/libs/mesa/libcompiler/compiler-20.0.8.sources";
            sources.c = inventory_stems(
                root,
                MANIFEST,
                "MESA20_COMPILER_STATIC_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src/compiler"),
            )?;
            sources.c.extend(inventory_stems(
                root,
                MANIFEST,
                "MESA20_COMPILER_GENERATED_C_SOURCES",
                ".c",
                &format!("{BUILD_ROOT}/src/compiler"),
            )?);
            sources.cxx = inventory_stems(
                root,
                MANIFEST,
                "MESA20_COMPILER_STATIC_CXX_SOURCES",
                ".cpp",
                &format!("{SOURCE_ROOT}/src/compiler"),
            )?;
            sources.cxx.extend(inventory_stems(
                root,
                MANIFEST,
                "MESA20_COMPILER_GENERATED_CXX_SOURCES",
                ".cpp",
                &format!("{BUILD_ROOT}/src/compiler"),
            )?);
        }
        (Some("workbench/libs/mesa/libgalliumaux"), "mesa3d-linklib-galliumauxiliary") => {
            const MANIFEST: &str = "workbench/libs/mesa/libgalliumaux/galliumaux-20.0.8.sources";
            sources.c = inventory_stems(
                root,
                MANIFEST,
                "MESA20_GALLIUMAUX_STATIC_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src/gallium/auxiliary"),
            )?;
            sources.c.extend(inventory_stems(
                root,
                MANIFEST,
                "MESA20_GALLIUMAUX_GENERATED_C_SOURCES",
                ".c",
                &format!("{BUILD_ROOT}/src/gallium/auxiliary"),
            )?);
        }
        (Some("workbench/libs/mesa/libmesa"), "mesa3d-linklib-mesa") => {
            const MANIFEST: &str = "workbench/libs/mesa/libmesa/mesa-20.0.8.sources";
            sources.c = inventory_stems(
                root,
                MANIFEST,
                "MESA20_CORE_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src/mesa"),
            )?;
            for generated in [
                "main/api_exec.c",
                "main/enums.c",
                "main/format_pack.c",
                "main/format_unpack.c",
                "main/format_fallback.c",
                "main/marshal_generated.c",
                "program/program_parse.tab.c",
                "program/lex.yy.c",
            ] {
                sources.c.push(format!(
                    "{BUILD_ROOT}/src/mesa/{}",
                    generated.trim_end_matches(".c")
                ));
            }
            sources.cxx = inventory_stems(
                root,
                MANIFEST,
                "MESA20_CORE_CXX_SOURCES",
                ".cpp",
                &format!("{SOURCE_ROOT}/src/mesa"),
            )?;
            if profile == "x86_64" {
                sources.c.extend(inventory_stems(
                    root,
                    MANIFEST,
                    "MESA20_CORE_X86_64_C_SOURCES",
                    ".c",
                    &format!("{SOURCE_ROOT}/src/mesa"),
                )?);
                sources.asm = inventory_paths(
                    root,
                    MANIFEST,
                    "MESA20_CORE_X86_64_ASM_SOURCES",
                    ".S",
                    &format!("{SOURCE_ROOT}/src/mesa"),
                )?;
            }
        }
        (Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"), "linklibs-gallium_vc4")
            if profile != "x86_64" =>
        {
            const MANIFEST: &str =
                "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/vc4-20.0.8.sources";
            sources.c = inventory_stems(
                root,
                MANIFEST,
                "MESA3D_VC4_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src/gallium/drivers/vc4"),
            )?;
        }
        _ => return Ok(None),
    }
    Ok(Some(sources))
}
