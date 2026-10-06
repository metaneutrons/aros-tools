//! Audited Mesa 26 archive source inventories.

use super::{
    fingerprint, glapi_sources, has_archive_sources, inventory_stems, profile,
    require_file_fingerprint, require_file_fingerprint_one_of, target_identity, Mesa26Target,
    BUILD_ROOT, LLVMPPIPE_SOURCE_NAMES, SOURCE_ROOT,
};
use crate::parser::TargetContext;
use crate::sources::EvaluatedSources;
use std::path::Path;

pub(super) fn archive_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> Result<Option<EvaluatedSources>, String> {
    if let Some(sources) = glapi_sources(root, relative_dir, mmake, target)? {
        return Ok(Some(sources));
    }
    let Some(identity) = target_identity(relative_dir, mmake) else {
        return Ok(None);
    };
    if !has_archive_sources(identity) {
        return Ok(None);
    }
    let Some(profile) = profile(target)? else {
        return Ok(None);
    };
    match identity {
        Mesa26Target::GalliumVmLibrary => {
            require_file_fingerprint(
                root,
                "workbench/libs/mesa/libgalliumvm/mmakefile.src",
                fingerprint("mesa26-gallivm-recipe")?,
                "Mesa 26 Gallivm source closure",
            )?;
            require_file_fingerprint(
                root,
                "workbench/libs/mesa/libgalliumvm/gallivm-26.0.0.sources",
                fingerprint("mesa26-gallivm-manifest")?,
                "Mesa 26 Gallivm source inventory",
            )?;
            require_file_fingerprint(
                root,
                "workbench/libs/mesa/mesa.cfg",
                fingerprint("mesa26-config")?,
                "Mesa 26 Gallivm configuration",
            )?;
            let manifest = "workbench/libs/mesa/libgalliumvm/gallivm-26.0.0.sources";
            let prefix = format!("{SOURCE_ROOT}/src/gallium/auxiliary/gallivm");
            return Ok(Some(EvaluatedSources {
                c: inventory_stems(root, manifest, "MESA26_GALLIVM_C_SOURCES", ".c", &prefix)?,
                cxx: inventory_stems(
                    root,
                    manifest,
                    "MESA26_GALLIVM_CXX_SOURCES",
                    ".cpp",
                    &prefix,
                )?,
                declared: true,
                ..EvaluatedSources::default()
            }));
        }
        Mesa26Target::GalliumDrawLlvmLibrary | Mesa26Target::GalliumTessLibrary => {
            require_file_fingerprint(
                root,
                "workbench/libs/mesa/libgalliumaux/mmakefile.src",
                fingerprint("mesa26-galliumaux-recipe")?,
                "Mesa 26 Gallium LLVM source closure",
            )?;
            require_file_fingerprint(
                root,
                "workbench/libs/mesa/libgalliumaux/galliumaux-26.0.0.sources",
                fingerprint("mesa26-galliumaux-manifest")?,
                "Mesa 26 Gallium LLVM source inventory",
            )?;
            require_file_fingerprint(
                root,
                "workbench/libs/mesa/mesa.cfg",
                fingerprint("mesa26-config")?,
                "Mesa 26 Gallium LLVM configuration",
            )?;
            let manifest = "workbench/libs/mesa/libgalliumaux/galliumaux-26.0.0.sources";
            let prefix = format!("{SOURCE_ROOT}/src/gallium/auxiliary");
            let sources = match identity {
                Mesa26Target::GalliumDrawLlvmLibrary => EvaluatedSources {
                    c: inventory_stems(
                        root,
                        manifest,
                        "MESA26_GALLIUMAUX_DRAW_LLVM_C_SOURCES",
                        ".c",
                        &prefix,
                    )?,
                    declared: true,
                    ..EvaluatedSources::default()
                },
                Mesa26Target::GalliumTessLibrary => EvaluatedSources {
                    cxx: inventory_stems(
                        root,
                        manifest,
                        "MESA26_GALLIUMAUX_TESS_CXX_SOURCES",
                        ".cpp",
                        &prefix,
                    )?,
                    declared: true,
                    ..EvaluatedSources::default()
                },
                _ => unreachable!("closed LLVM target selection"),
            };
            return Ok(Some(sources));
        }
        Mesa26Target::LlvmPipeLibrary => {
            require_file_fingerprint(
                root,
                "workbench/libs/mesa/libllvmpipe/mmakefile.src",
                fingerprint("mesa26-llvmpipe-recipe")?,
                "Mesa 26 llvmpipe source closure",
            )?;
            require_file_fingerprint(
                root,
                "workbench/libs/mesa/mesa.cfg",
                fingerprint("mesa26-config")?,
                "Mesa 26 llvmpipe configuration",
            )?;
            return Ok(Some(EvaluatedSources {
                c: LLVMPPIPE_SOURCE_NAMES
                    .iter()
                    .map(|source| format!("{SOURCE_ROOT}/src/gallium/drivers/llvmpipe/{source}"))
                    .collect(),
                declared: true,
                ..EvaluatedSources::default()
            }));
        }
        _ => {}
    }
    if identity == Mesa26Target::EglLibrary {
        require_file_fingerprint(
            root,
            "workbench/libs/egl/mmakefile.src",
            fingerprint("mesa26-egl-recipe")?,
            "Mesa 26 EGL source closure",
        )?;
        require_file_fingerprint(
            root,
            "workbench/libs/mesa/mesa.cfg",
            fingerprint("mesa26-config")?,
            "Mesa 26 EGL configuration",
        )?;
        let mut sources = [
            "eglapi",
            "eglarray",
            "eglconfig",
            "eglconfigdebug",
            "eglcontext",
            "eglcurrent",
            "egldevice",
            "egldisplay",
            "eglglobals",
            "eglimage",
            "egllog",
            "eglsurface",
            "eglsync",
        ]
        .map(|stem| format!("{SOURCE_ROOT}/src/egl/main/{stem}"))
        .to_vec();
        sources.extend(
            ["egl_arosmesa", "emul_arosc", "tls"]
                .map(|stem| format!("${{AROS_SOURCE_DIR}}/workbench/libs/egl/{stem}")),
        );
        return Ok(Some(EvaluatedSources {
            c: sources,
            declared: true,
            ..EvaluatedSources::default()
        }));
    }
    if identity == Mesa26Target::V3dLinkLibrary && profile != "aarch64" {
        return Ok(None);
    }
    if identity == Mesa26Target::Vc4LinkLibrary && profile == "x86_64" {
        return Ok(None);
    }
    if identity == Mesa26Target::MesaSse41 {
        require_file_fingerprint(
            root,
            "workbench/libs/mesa/libmesa/mmakefile.src",
            fingerprint("mesa26-core-recipe")?,
            "Mesa 26 empty SSE4.1 compatibility archive",
        )?;
        require_file_fingerprint(
            root,
            "workbench/libs/mesa/mesa.cfg",
            fingerprint("mesa26-config")?,
            "Mesa 26 empty SSE4.1 compatibility archive",
        )?;
        return Ok(Some(EvaluatedSources {
            declared: true,
            ..EvaluatedSources::default()
        }));
    }
    let family = match identity {
        Mesa26Target::CompilerLibrary => "compiler",
        Mesa26Target::MesaCoreLibrary => "core",
        Mesa26Target::GalliumAuxLibrary => "galliumaux",
        Mesa26Target::MesaUtilLibrary | Mesa26Target::MesaDevUtilLibrary => "util",
        Mesa26Target::V3dLinkLibrary if profile == "aarch64" => "v3d",
        Mesa26Target::Vc4LinkLibrary if profile == "arm" || profile == "aarch64" => "vc4",
        _ => return Ok(None),
    };
    let (recipe, manifest, recipe_pin, manifest_pin) = match family {
        "compiler" => (
            "workbench/libs/mesa/libcompiler/mmakefile.src",
            "workbench/libs/mesa/libcompiler/compiler-26.0.0.sources",
            "mesa26-compiler-recipe",
            "mesa26-compiler-manifest",
        ),
        "core" => (
            "workbench/libs/mesa/libmesa/mmakefile.src",
            "workbench/libs/mesa/libmesa/mesa-26.0.0.sources",
            "mesa26-core-recipe",
            "mesa26-core-manifest",
        ),
        "galliumaux" => (
            "workbench/libs/mesa/libgalliumaux/mmakefile.src",
            "workbench/libs/mesa/libgalliumaux/galliumaux-26.0.0.sources",
            "mesa26-galliumaux-recipe",
            "mesa26-galliumaux-manifest",
        ),
        "util" => (
            "workbench/libs/mesa/libmesautil/mmakefile.src",
            "workbench/libs/mesa/libmesautil/mesautil-26.0.0.sources",
            "mesa26-util-recipe",
            "mesa26-util-manifest",
        ),
        "v3d" => (
            "arch/arm-native/soc/broadcom/2708/hidd/v3d/mmakefile.src",
            "arch/arm-native/soc/broadcom/2708/hidd/v3d/v3d-26.0.0.sources",
            "mesa26-v3d-recipe",
            "mesa26-v3d-manifest",
        ),
        "vc4" => (
            "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/mmakefile.src",
            "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/vc4-26.0.0.sources",
            "mesa26-vc4-recipe",
            "mesa26-vc4-manifest",
        ),
        _ => unreachable!("closed family selection"),
    };
    if family == "v3d" {
        // Upstream added v3d_dt to the HIDD, not to the closed Mesa archive.
        // Both reviewed recipes expand to the same archive and generator jobs.
        require_file_fingerprint_one_of(
            root,
            recipe,
            &[
                fingerprint(recipe_pin)?,
                fingerprint("mesa26-v3d-recipe-device-tree")?,
            ],
            family,
        )?;
    } else {
        require_file_fingerprint(root, recipe, fingerprint(recipe_pin)?, family)?;
    }
    require_file_fingerprint(root, manifest, fingerprint(manifest_pin)?, family)?;
    require_file_fingerprint(
        root,
        "workbench/libs/mesa/mesa.cfg",
        fingerprint("mesa26-config")?,
        family,
    )?;
    let mut sources = EvaluatedSources {
        declared: true,
        ..EvaluatedSources::default()
    };
    match family {
        "core" => {
            sources.c = inventory_stems(
                root,
                manifest,
                "MESA26_CORE_STATIC_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src/mesa"),
            )?;
            sources.c.extend(inventory_stems(
                root,
                manifest,
                "MESA26_CORE_GENERATED_C_SOURCES",
                ".c",
                &format!("{BUILD_ROOT}/src/mesa"),
            )?);
            sources.c.push(
                "${AROS_SOURCE_DIR}/workbench/libs/mesa/libmesa/mesa_sse_minmax_fallback"
                    .to_owned(),
            );
            sources.cxx = inventory_stems(
                root,
                manifest,
                "MESA26_CORE_STATIC_CXX_SOURCES",
                ".cpp",
                &format!("{SOURCE_ROOT}/src/mesa"),
            )?;
        }
        "util" => {
            sources.c = inventory_stems(
                root,
                manifest,
                "MESA26_UTIL_STATIC_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src/util"),
            )?;
            if profile == "aarch64" {
                sources.c.extend(inventory_stems(
                    root,
                    manifest,
                    "MESA26_UTIL_ARM64_C_SOURCES",
                    ".c",
                    &format!("{SOURCE_ROOT}/src/util"),
                )?);
            }
            sources.c.extend(inventory_stems(
                root,
                manifest,
                "MESA26_UTIL_C11_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src"),
            )?);
            sources.c.extend(inventory_stems(
                root,
                manifest,
                "MESA26_UTIL_GENERATED_C_SOURCES",
                ".c",
                &format!("{BUILD_ROOT}/src/util"),
            )?);
            sources.c.extend(inventory_stems(
                root,
                manifest,
                "MESA26_UTIL_AROS_FALLBACK_C_SOURCES",
                ".c",
                "${AROS_SOURCE_DIR}",
            )?);
            sources.cxx = inventory_stems(
                root,
                manifest,
                "MESA26_UTIL_STATIC_CXX_SOURCES",
                ".cpp",
                &format!("{SOURCE_ROOT}/src/util"),
            )?;
        }
        "compiler" => {
            for variable in [
                "MESA26_COMPILER_NIR_GENERATED_C_SOURCES",
                "MESA26_COMPILER_GLSL_GENERATED_C_SOURCES",
                "MESA26_COMPILER_SPIRV_GENERATED_C_SOURCES",
            ] {
                sources.c.extend(inventory_stems(
                    root,
                    manifest,
                    variable,
                    ".c",
                    &format!("{BUILD_ROOT}/src/compiler"),
                )?);
            }
            for variable in [
                "MESA26_COMPILER_GLSL_C_SOURCES",
                "MESA26_COMPILER_NIR_C_SOURCES",
                "MESA26_COMPILER_SPIRV_C_SOURCES",
            ] {
                sources.c.extend(inventory_stems(
                    root,
                    manifest,
                    variable,
                    ".c",
                    &format!("{SOURCE_ROOT}/src/compiler"),
                )?);
            }
            sources.c.extend(inventory_stems(
                root,
                manifest,
                "MESA26_COMPILER_GLCPP_GENERATED_C_SOURCES",
                ".c",
                &format!("{BUILD_ROOT}/src/compiler"),
            )?);
            for variable in [
                "MESA26_COMPILER_GLCPP_C_SOURCES",
                "MESA26_COMPILER_CORE_C_SOURCES",
            ] {
                sources.c.extend(inventory_stems(
                    root,
                    manifest,
                    variable,
                    ".c",
                    &format!("{SOURCE_ROOT}/src/compiler"),
                )?);
            }
            sources.cxx = inventory_stems(
                root,
                manifest,
                "MESA26_COMPILER_GLSL_GENERATED_CXX_SOURCES",
                ".cpp",
                &format!("{BUILD_ROOT}/src/compiler"),
            )?;
            sources.cxx.extend(inventory_stems(
                root,
                manifest,
                "MESA26_COMPILER_GLSL_CXX_SOURCES",
                ".cpp",
                &format!("{SOURCE_ROOT}/src/compiler"),
            )?);
        }
        "galliumaux" => {
            sources.c = inventory_stems(
                root,
                manifest,
                "MESA26_GALLIUMAUX_STATIC_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src/gallium/auxiliary"),
            )?;
            sources.c.extend(inventory_stems(
                root,
                manifest,
                "MESA26_GALLIUMAUX_GENERATED_C_SOURCES",
                ".c",
                &format!("{BUILD_ROOT}/src/gallium/auxiliary"),
            )?);
        }
        "v3d" => {
            for (variable, base) in [
                (
                    "MESA26_V3D_DRIVER_C_SOURCES",
                    format!("{SOURCE_ROOT}/src/gallium/drivers/v3d"),
                ),
                (
                    "MESA26_V3D_COMPILER_C_SOURCES",
                    format!("{SOURCE_ROOT}/src/broadcom/compiler"),
                ),
                (
                    "MESA26_V3D_BROADCOM_C_SOURCES",
                    format!("{SOURCE_ROOT}/src/broadcom"),
                ),
                (
                    "MESA26_V3D_GENERATED_C_SOURCES",
                    super::super::V3D_AROS_BUILD_DIR.to_owned(),
                ),
            ] {
                sources
                    .c
                    .extend(inventory_stems(root, manifest, variable, ".c", &base)?);
            }
        }
        "vc4" => {
            sources.c = inventory_stems(
                root,
                manifest,
                "MESA26_VC4_DRIVER_C_SOURCES",
                ".c",
                &format!("{SOURCE_ROOT}/src/gallium/drivers/vc4"),
            )?;
            sources.c.push("${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/vc4_tiling_lt_neon".to_owned());
        }
        _ => unreachable!("closed family selection"),
    }
    Ok(Some(sources))
}
