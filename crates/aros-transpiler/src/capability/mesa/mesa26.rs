//! Closed Mesa 26.0.0 capabilities. These do not reuse Mesa 20 inventories.

use super::{inventory_stems, CompileContract};
use crate::ast::{
    ModuleType, PythonGeneratorJob, PythonOutputsDecl, PythonPackageDecl, TargetDefinition,
};
use crate::capability::{require_file_fingerprint, require_file_fingerprint_one_of};
use crate::fetch::FetchDecl;
use crate::fingerprints::fingerprint;
use crate::parser::TargetContext;
use crate::sources::EvaluatedSources;
use std::path::Path;

mod archive;

pub(crate) const SOURCE_ROOT: &str = "${AROS_PORTS_DIR}/mesa/mesa-26.0.0";
pub(crate) const BUILD_ROOT: &str = "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0";
pub(crate) const GLAPI_DIR: &str = "workbench/libs/mesa/libglapi";
pub(crate) const GLAPI_MMAKE: &str = "mesa3d-linklib-glapi";
pub(crate) const PRIVATE_LIBDIR: &str = "${AROS_BUILD_DIR}/gen/lib/mesa26.0.0";
const LLVM11_INCLUDE: &str = "${AROS_BUILD_DIR}/gen/external-install/llvm11/include";

const LLVMPPIPE_SOURCE_NAMES: &[&str] = &[
    "lp_bld_alpha",
    "lp_bld_blend_aos",
    "lp_bld_blend",
    "lp_bld_blend_logicop",
    "lp_bld_depth",
    "lp_bld_interp",
    "lp_clear",
    "lp_context",
    "lp_cs_tpool",
    "lp_draw_arrays",
    "lp_fence",
    "lp_flush",
    "lp_jit",
    "lp_linear",
    "lp_linear_fastpath",
    "lp_linear_interp",
    "lp_linear_sampler",
    "lp_memory",
    "lp_perf",
    "lp_query",
    "lp_rast",
    "lp_rast_debug",
    "lp_rast_linear",
    "lp_rast_linear_fallback",
    "lp_rast_rect",
    "lp_rast_tri",
    "lp_scene",
    "lp_scene_queue",
    "lp_screen",
    "lp_setup",
    "lp_setup_analysis",
    "lp_setup_line",
    "lp_setup_point",
    "lp_setup_rect",
    "lp_setup_tri",
    "lp_setup_vbuf",
    "lp_state_blend",
    "lp_state_clip",
    "lp_state_derived",
    "lp_state_cs",
    "lp_state_fs",
    "lp_state_fs_analysis",
    "lp_state_fs_fastpath",
    "lp_state_fs_linear",
    "lp_state_fs_linear_llvm",
    "lp_state_gs",
    "lp_state_rasterizer",
    "lp_state_sampler",
    "lp_state_setup",
    "lp_state_so",
    "lp_state_surface",
    "lp_state_tess",
    "lp_state_vertex",
    "lp_state_vs",
    "lp_surface",
    "lp_tex_sample",
    "lp_texture",
    "lp_texture_handle",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mesa26Target {
    MesaSse41,
    GalliumHidd,
    GalliumLibrary,
    EglLibrary,
    V3dHidd,
    Vc4LinkLibrary,
    Vc4Hidd,
    V3dLinkLibrary,
    MesaGlLibrary,
    CompilerLibrary,
    GalliumAuxLibrary,
    MesaCoreLibrary,
    MesaUtilLibrary,
    MesaDevUtilLibrary,
    GalliumVmLibrary,
    GalliumDrawLlvmLibrary,
    GalliumTessLibrary,
    LlvmPipeLibrary,
    LlvmPipeHidd,
}

/// Return a Mesa 26 capability identity only for an exact source-directory /
/// MetaMake pair. These functions parse every source declaration in a target
/// graph; profile support must not be checked for unrelated declarations.
fn target_identity(relative_dir: &Path, mmake: &str) -> Option<Mesa26Target> {
    match (relative_dir.to_str()?, mmake) {
        ("workbench/libs/mesa/libmesa", "mesa3d-linklib-mesa-sse41") => {
            Some(Mesa26Target::MesaSse41)
        }
        ("workbench/hidds/gallium", "hidd-gallium") => Some(Mesa26Target::GalliumHidd),
        ("workbench/libs/gallium", "workbench-libs-gallium") => Some(Mesa26Target::GalliumLibrary),
        ("workbench/libs/egl", "workbench-libs-egl") => Some(Mesa26Target::EglLibrary),
        (super::V3D_RELATIVE_DIR, "hidd-v3d") => Some(Mesa26Target::V3dHidd),
        ("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium", "linklibs-gallium_vc4") => {
            Some(Mesa26Target::Vc4LinkLibrary)
        }
        ("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium", "hidd-vc4gallium") => {
            Some(Mesa26Target::Vc4Hidd)
        }
        (super::V3D_RELATIVE_DIR, "linklibs-gallium_v3d") => Some(Mesa26Target::V3dLinkLibrary),
        ("workbench/libs/mesa", "mesa3dgl-library") => Some(Mesa26Target::MesaGlLibrary),
        ("workbench/libs/mesa/libcompiler", "mesa3d-linklib-compiler") => {
            Some(Mesa26Target::CompilerLibrary)
        }
        ("workbench/libs/mesa/libgalliumaux", "mesa3d-linklib-galliumauxiliary") => {
            Some(Mesa26Target::GalliumAuxLibrary)
        }
        ("workbench/libs/mesa/libmesa", "mesa3d-linklib-mesa") => {
            Some(Mesa26Target::MesaCoreLibrary)
        }
        ("workbench/libs/mesa/libmesautil", "mesa3d-linklib-mesautil") => {
            Some(Mesa26Target::MesaUtilLibrary)
        }
        ("workbench/libs/mesa/libmesautil", "mesa3d-linklib-mesadevutil") => {
            Some(Mesa26Target::MesaDevUtilLibrary)
        }
        ("workbench/libs/mesa/libgalliumvm", "mesa3d-linklib-galliumvm") => {
            Some(Mesa26Target::GalliumVmLibrary)
        }
        ("workbench/libs/mesa/libgalliumaux", "mesa3d-linklib-galliumdrawllvm") => {
            Some(Mesa26Target::GalliumDrawLlvmLibrary)
        }
        ("workbench/libs/mesa/libgalliumaux", "mesa3d-linklib-galliumtess") => {
            Some(Mesa26Target::GalliumTessLibrary)
        }
        ("workbench/libs/mesa/libllvmpipe", "mesa3d-linklib-llvmpipe") => {
            Some(Mesa26Target::LlvmPipeLibrary)
        }
        ("workbench/hidds/llvmpipe", "hidd-llvmpipe") => Some(Mesa26Target::LlvmPipeHidd),
        _ => None,
    }
}

const fn has_archive_sources(target: Mesa26Target) -> bool {
    matches!(
        target,
        Mesa26Target::MesaSse41
            | Mesa26Target::EglLibrary
            | Mesa26Target::Vc4LinkLibrary
            | Mesa26Target::V3dLinkLibrary
            | Mesa26Target::CompilerLibrary
            | Mesa26Target::GalliumAuxLibrary
            | Mesa26Target::MesaCoreLibrary
            | Mesa26Target::MesaUtilLibrary
            | Mesa26Target::MesaDevUtilLibrary
            | Mesa26Target::GalliumVmLibrary
            | Mesa26Target::GalliumDrawLlvmLibrary
            | Mesa26Target::GalliumTessLibrary
            | Mesa26Target::LlvmPipeLibrary
    )
}

fn common_defines(profile: &str) -> Vec<String> {
    let mut defines = [
        "__STDC_CONSTANT_MACROS",
        "__STDC_FORMAT_MACROS",
        "__STDC_LIMIT_MACROS",
        "_GNU_SOURCE",
        "HAVE_PTHREAD",
        "HAVE_TIMESPEC_GET",
        "POSIXC_SLOWSTACK_VAARGS",
        "HAVE_ZLIB",
        "HAVE_FUNC_ATTRIBUTE_PACKED",
        "HAVE_OPENGL=1",
        "HAVE_OPENGL_ES_1=1",
        "HAVE_OPENGL_ES_2=1",
        "UTIL_ARCH_LITTLE_ENDIAN=1",
        "UTIL_ARCH_BIG_ENDIAN=0",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if profile == "arm" {
        defines.push("MISSING_64BIT_ATOMICS".to_owned());
    }
    if profile == "x86_64" {
        defines.extend(["USE_X86_64_ASM", "USE_SSE41"].map(str::to_owned));
    }
    defines.extend(["MAPI_MODE_GLAPI", "MAPI_MODE_UTIL"].map(str::to_owned));
    defines
}

fn common_includes() -> Vec<String> {
    [
        "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
        "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
        "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/include",
        "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/include/GL",
        "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src",
        "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn llvm_compile_contract(profile: &str, identity: Mesa26Target) -> CompileContract {
    let mut defines = common_defines(profile);
    defines.extend(
        [
            "DRAW_LLVM_AVAILABLE=1",
            "LLVM_AVAILABLE",
            "HAVE_LLVM=0x0b00",
            "MESA_LLVM_VERSION_STRING=\"11.0.0\"",
            "GALLIVM_USE_ORCJIT=0",
            "LLVM_IS_SHARED=0",
            "PACKAGE_VERSION=\"26.0.0\"",
            "__AROS__",
        ]
        .map(str::to_owned),
    );
    if identity == Mesa26Target::LlvmPipeLibrary {
        defines.push("GALLIUM_LLVMPIPE".to_owned());
    }
    defines.push("NDEBUG".to_owned());

    let mut includes = common_includes();
    includes.extend(
        [
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/nir",
            "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src",
            "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler",
            "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/nir",
            LLVM11_INCLUDE,
        ]
        .map(str::to_owned),
    );
    match identity {
        Mesa26Target::GalliumVmLibrary => includes.extend(
            ["${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary/gallivm"].map(str::to_owned),
        ),
        Mesa26Target::GalliumDrawLlvmLibrary | Mesa26Target::GalliumTessLibrary => includes.extend(
            [
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary/util",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary/indices",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/gallium/auxiliary",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/gallium/auxiliary/util",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/gallium/auxiliary/indices",
            ]
            .map(str::to_owned),
        ),
        Mesa26Target::LlvmPipeLibrary => includes.extend(
            [
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary/util",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary/indices",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/mesa/main",
            ]
            .map(str::to_owned),
        ),
        Mesa26Target::LlvmPipeHidd => includes.extend(
            [
                "${AROS_SOURCE_DIR}/workbench/hidds/llvmpipe",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/drivers",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/include",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/util",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/util/format",
            ]
            .map(str::to_owned),
        ),
        _ => {}
    }

    CompileContract {
        defines,
        undefines: Vec::new(),
        includes,
        options: vec![
            "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>".to_owned(),
            "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++17>".to_owned(),
            "-fno-strict-aliasing".to_owned(),
        ],
    }
}

pub(crate) fn profile(target: Option<&TargetContext>) -> Result<Option<&'static str>, String> {
    let Some(target) = target else {
        return Ok(None);
    };
    if target.mesa_version.as_deref() != Some("26.0.0") {
        return Ok(None);
    }
    let key = (
        target.cpu.as_deref(),
        target.platform.as_deref(),
        target.toolchain.as_deref(),
        target.cpu32.as_deref(),
        target.use_mmu.as_deref(),
        target.float_abi.as_deref(),
    );
    match key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => {
            Ok(Some("x86_64"))
        }
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard")) => {
            Ok(Some("arm"))
        }
        (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("")) => {
            Ok(Some("aarch64"))
        }
        _ => Err(format!(
            "Mesa 26.0.0 capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            target.cpu.as_deref().unwrap_or("<unset>"),
            target.platform.as_deref().unwrap_or("<unset>"),
            target.toolchain.as_deref().unwrap_or("<unset>"),
            target.cpu32.as_deref().unwrap_or("<unset>"),
            target.use_mmu.as_deref().unwrap_or("<unset>"),
            target.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

pub(crate) fn glapi_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> Result<Option<EvaluatedSources>, String> {
    if relative_dir != Path::new(GLAPI_DIR) || mmake != GLAPI_MMAKE {
        return Ok(None);
    }
    if profile(target)?.is_none() {
        return Ok(None);
    }
    require_file_fingerprint(
        root,
        "workbench/libs/mesa/libglapi/mmakefile.src",
        fingerprint("mesa26-glapi-recipe")?,
        "Mesa 26.0.0 glapi",
    )?;
    require_file_fingerprint(
        root,
        "workbench/libs/mesa/mesa.cfg",
        fingerprint("mesa26-config")?,
        "Mesa 26.0.0 common flags",
    )?;
    Ok(Some(EvaluatedSources {
        c: vec![
            format!("{SOURCE_ROOT}/src/mesa/glapi/shared-glapi/core"),
            format!("{BUILD_ROOT}/src/mesa/glapi/shared-glapi/public_glapi_wrappers"),
        ],
        declared: true,
        ..EvaluatedSources::default()
    }))
}

pub(crate) fn runtime_module_name(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    raw_name: &str,
    target: Option<&TargetContext>,
) -> Result<Option<String>, String> {
    if relative_dir != Path::new("workbench/libs/mesa")
        || mmake != "mesa3dgl-library"
        || profile(target)?.is_none()
    {
        return Ok(None);
    }
    require_file_fingerprint(
        root,
        "workbench/libs/mesa/mesa.cfg",
        fingerprint("mesa26-config")?,
        "Mesa 26 runtime module identity",
    )?;
    if raw_name != "mesa3dgl$(MESAGLBUILD)" {
        return Err(format!("unexpected Mesa 26 runtime modname: {raw_name}"));
    }
    // The reviewed recipe derives MESAGLBUILD as major-minor, not the full
    // archive version or an unresolved Make-variable token.
    Ok(Some("mesa3dgl26-0".to_owned()))
}

pub(crate) fn module_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> Result<Option<EvaluatedSources>, String> {
    if relative_dir == Path::new("workbench/hidds/llvmpipe") && mmake == "hidd-llvmpipe" {
        if profile(target)?.is_none() {
            return Ok(None);
        }
        require_file_fingerprint(
            root,
            "workbench/hidds/llvmpipe/mmakefile.src",
            fingerprint("mesa26-llvmpipe-hidd-recipe")?,
            "Mesa 26 llvmpipe HIDD source closure",
        )?;
        require_file_fingerprint(
            root,
            "workbench/libs/mesa/mesa.cfg",
            fingerprint("mesa26-config")?,
            "Mesa 26 llvmpipe HIDD configuration",
        )?;
        return Ok(Some(EvaluatedSources {
            c: [
                "${AROS_SOURCE_DIR}/workbench/hidds/llvmpipe/llvmpipe_init".to_owned(),
                "${AROS_SOURCE_DIR}/workbench/hidds/llvmpipe/llvmpipe_galliumclass".to_owned(),
                "${AROS_SOURCE_DIR}/workbench/libs/mesa/emul_arosc".to_owned(),
            ]
            .to_vec(),
            declared: true,
            ..EvaluatedSources::default()
        }));
    }
    if relative_dir == Path::new("workbench/libs/egl") && mmake == "workbench-libs-egl" {
        archive_sources(root, relative_dir, mmake, target)
    } else {
        Ok(None)
    }
}

pub(crate) fn archive_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> Result<Option<EvaluatedSources>, String> {
    archive::archive_sources(root, relative_dir, mmake, target)
}

pub(crate) fn glapi_compile_contract(
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> Result<Option<CompileContract>, String> {
    if relative_dir != Path::new(GLAPI_DIR) || mmake != GLAPI_MMAKE {
        return Ok(None);
    }
    let Some(profile) = profile(target)? else {
        return Ok(None);
    };
    let mut defines = common_defines(profile);
    defines.extend(["NDEBUG", "MAPI_MODE_SHARED_GLAPI"].map(str::to_owned));
    let mut includes = common_includes();
    includes.extend(
        [
            "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/mesa/glapi",
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa/glapi",
            "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/mesa/glapi/glapi",
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa",
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa/glapi/glapi",
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util",
            "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/mesa/glapi/shared-glapi",
            "${AROS_SOURCE_DIR}/workbench/libs/mesa",
        ]
        .map(str::to_owned),
    );
    Ok(Some(CompileContract {
        defines,
        undefines: Vec::new(),
        includes,
        options: vec!["-std=gnu11".to_owned(), "-fno-strict-aliasing".to_owned()],
    }))
}

pub(crate) fn compile_contract(
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> Result<Option<CompileContract>, String> {
    if let Some(contract) = glapi_compile_contract(relative_dir, mmake, target)? {
        return Ok(Some(contract));
    }
    let Some(identity) = target_identity(relative_dir, mmake) else {
        return Ok(None);
    };
    let Some(profile) = profile(target)? else {
        return Ok(None);
    };
    if matches!(
        identity,
        Mesa26Target::GalliumVmLibrary
            | Mesa26Target::GalliumDrawLlvmLibrary
            | Mesa26Target::GalliumTessLibrary
            | Mesa26Target::LlvmPipeLibrary
            | Mesa26Target::LlvmPipeHidd
    ) {
        return Ok(Some(llvm_compile_contract(profile, identity)));
    }
    if matches!(
        identity,
        Mesa26Target::V3dHidd | Mesa26Target::V3dLinkLibrary
    ) && profile != "aarch64"
    {
        return Ok(None);
    }
    if identity == Mesa26Target::MesaSse41 {
        let mut defines = common_defines(profile);
        defines.push("NDEBUG".to_owned());
        return Ok(Some(CompileContract {
            defines,
            undefines: Vec::new(),
            includes: common_includes(),
            options: vec!["-std=gnu11".to_owned(), "-fno-strict-aliasing".to_owned()],
        }));
    }
    let mut defines = common_defines(profile);
    let mut includes = common_includes();
    let mut options = vec![
        "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>".to_owned(),
        "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++17>".to_owned(),
        "-fno-strict-aliasing".to_owned(),
    ];
    match identity {
        Mesa26Target::GalliumHidd => {
            includes.extend(
                [
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium",
                    "${AROS_SOURCE_DIR}/workbench/hidds/gallium",
                ]
                .map(str::to_owned),
            );
        }
        Mesa26Target::GalliumLibrary => {
            includes.push("${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include".to_owned());
        }
        Mesa26Target::EglLibrary => {
            defines.extend(
                [
                    "FEATURE_GL=1",
                    "_EGL_NATIVE_PLATFORM=_EGL_PLATFORM_AROS",
                    "_EGL_OS_AROS=1",
                    "HAVE_AROS_BACKEND",
                    "HAVE_SURFACELESS_PLATFORM",
                ]
                .map(str::to_owned),
            );
            includes.extend(
                [
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/egl/main",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa/glapi",
                    "${AROS_SOURCE_DIR}/workbench/libs/egl",
                ]
                .map(str::to_owned),
            );
        }
        Mesa26Target::V3dHidd if profile == "aarch64" => {
            defines.extend(["GCA_CONSUMER_MODULE", "AROS_MESA26_V3D=1"].map(str::to_owned));
            includes.extend(
                [
                    "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/v3d/drm-stubs",
                    "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/v3d",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/drivers/v3d",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom/compiler",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/util",
                ]
                .map(str::to_owned),
            );
        }
        Mesa26Target::Vc4LinkLibrary | Mesa26Target::Vc4Hidd
            if profile == "arm" || profile == "aarch64" =>
        {
            defines.extend(
                [
                    "GALLIUM_VC4",
                    "HAVE_STRUCT_TIMESPEC",
                    "GCA_CONSUMER_MODULE",
                    "AROS_MESA_MAJOR=26",
                ]
                .map(str::to_owned),
            );
            includes.extend([
                "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/drm_compat",
                "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/drivers",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/drivers/vc4",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/nir",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/nir",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/util",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/galliumcoreapi",
                "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/include",
            ].map(str::to_owned));
        }
        Mesa26Target::V3dLinkLibrary => {
            defines.extend(
                [
                    "USE_V3D_SIMULATOR=0",
                    "USING_V3D_SIMULATOR=0",
                    "using_v3d_simulator=0",
                    "V3D_BUILD_NEON",
                    "GCA_CONSUMER_MODULE",
                    "AROS_MESA26_V3D=1",
                ]
                .map(str::to_owned),
            );
            includes.extend(
                [
                    "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/v3d/drm-stubs",
                    "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/v3d",
                    "${AROS_BUILD_DIR}/gen/arch/arm-native/soc/broadcom/2708/hidd/v3d/cle-gen",
                    "${AROS_BUILD_DIR}/gen/arch/arm-native/soc/broadcom/2708/hidd/v3d/cle-gen/broadcom",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom/cle",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom/compiler",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom/common",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/drivers",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/drivers/v3d",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom/clif",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom/perfcntrs",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/nir",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/util",
                ]
                .map(str::to_owned),
            );
            options.extend([
                "-UHAVE_VALGRIND".to_owned(),
                "-include".to_owned(),
                "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/v3d/v3d_aros_override.h"
                    .to_owned(),
            ]);
        }
        Mesa26Target::MesaGlLibrary => {
            includes.extend(
                [
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa/main",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mapi",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/mesa",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler",
                    "${AROS_SOURCE_DIR}/workbench/libs/mesa",
                ]
                .map(str::to_owned),
            );
            if profile == "arm" || profile == "aarch64" {
                includes.push(
                    "${AROS_SOURCE_DIR}/arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"
                        .to_owned(),
                );
            }
        }
        Mesa26Target::CompilerLibrary => {
            includes.extend(
                [
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mapi",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/glsl",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/glsl/glcpp",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/spirv",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/glsl",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/glsl/glcpp",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/nir",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/spirv",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
                ]
                .map(str::to_owned),
            );
            options.push("$<$<COMPILE_LANGUAGE:CXX>:-I${AROS_SOURCE_DIR}/workbench/libs/mesa/libcompiler/cxx-compat>".to_owned());
        }
        Mesa26Target::GalliumAuxLibrary => {
            includes.extend([
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary/util",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary/indices",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/gallium/auxiliary",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/gallium/auxiliary/driver_trace",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/gallium/auxiliary/util",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/gallium/auxiliary/indices",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/glsl",
                "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/nir",
                "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/nir",
            ].map(str::to_owned));
        }
        Mesa26Target::MesaCoreLibrary => {
            defines.extend([
                "PACKAGE_VERSION=\"26.0.0\"".to_owned(),
                "PACKAGE_BUGREPORT=\"https://bugs.freedesktop.org/enter_bug.cgi?product=Mesa\""
                    .to_owned(),
            ]);
            includes.extend(
                [
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/mesa",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/mesa/main",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mapi",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/glsl",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/glsl",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/nir",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa/main",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
                ]
                .map(str::to_owned),
            );
            options.push("$<$<COMPILE_LANGUAGE:CXX>:-I${AROS_SOURCE_DIR}/workbench/libs/mesa/libcompiler/cxx-compat>".to_owned());
        }
        Mesa26Target::MesaUtilLibrary | Mesa26Target::MesaDevUtilLibrary => {
            defines.extend(
                [
                    "BLAKE3_NO_SSE2",
                    "BLAKE3_NO_SSE41",
                    "BLAKE3_NO_AVX2",
                    "BLAKE3_NO_AVX512",
                    "DETECT_OS_CYGWIN=1",
                    "DETECT_OS_POSIX=1",
                    "DETECT_OS_POSIX_LITE=1",
                    "HAVE_SYSCONF=1",
                ]
                .map(str::to_owned),
            );
            if identity == Mesa26Target::MesaDevUtilLibrary {
                defines.push("EMBEDDED_DEVICE".to_owned());
            }
            includes.extend(
                [
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/util",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mesa",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/mapi",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/auxiliary",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util/format",
                    "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/26.0.0/src/util/format",
                    "${AROS_PORTS_DIR}/zlib/chromium-da752eb2a3660cf1bf8dac620f6380b89dd953a7",
                ]
                .map(str::to_owned),
            );
        }
        Mesa26Target::MesaSse41
        | Mesa26Target::V3dHidd
        | Mesa26Target::Vc4LinkLibrary
        | Mesa26Target::Vc4Hidd
        | Mesa26Target::GalliumVmLibrary
        | Mesa26Target::GalliumDrawLlvmLibrary
        | Mesa26Target::GalliumTessLibrary
        | Mesa26Target::LlvmPipeLibrary
        | Mesa26Target::LlvmPipeHidd => return Ok(None),
    }
    defines.push("NDEBUG".to_owned());
    Ok(Some(CompileContract {
        defines,
        undefines: Vec::new(),
        includes,
        options,
    }))
}

fn verify_mesa_fetch(fetches: &[FetchDecl]) -> Result<(), String> {
    let matches = fetches
        .iter()
        .filter(|candidate| candidate.name == "mesa3d-fetch")
        .collect::<Vec<_>>();
    let [fetch] = matches.as_slice() else {
        return Err(format!(
            "Mesa 26 requires one central mesa3d-fetch, found {}",
            matches.len()
        ));
    };
    if fetch.archive != "mesa-26.0.0"
        || fetch.suffixes != "tar.xz"
        || fetch.origins
            != "https://archive.mesa3d.org/ https://archive.mesa3d.org/older-versions/26.x"
        || fetch.checksums
            != "mesa-26.0.0.tar.xz=sha256:2a44e98e64d5c36cec64633de2d0ec7eff64703ee25b35364ba8fcaa84f33f72"
        || fetch.location != "${AROS_PORTS_SOURCE_DIR}"
        || fetch.destination != "${AROS_PORTS_DIR}/mesa"
        || !fetch.base.is_empty()
        || fetch.patch_origins != "${AROS_SOURCE_DIR}/workbench/libs/mesa"
        || fetch.patches != "mesa-26.0.0-aros.diff:mesa-26.0.0:-p1"
        || fetch.dir != "workbench/libs/mesa"
    {
        return Err("central Mesa 26.0.0 fetch differs from the audited source/patch contract".to_owned());
    }

    Ok(())
}

fn verified_python_packages(fetches: &[FetchDecl]) -> Result<Vec<PythonPackageDecl>, String> {
    for name in ["mesa3d-mako-fetch", "mesa3d-markupsafe-fetch"] {
        let matching = fetches
            .iter()
            .filter(|candidate| candidate.name == name)
            .collect::<Vec<_>>();
        let [fetch] = matching.as_slice() else {
            return Err(format!("Mesa 26 requires exactly one {name} declaration"));
        };
        if !super::mesa20::fetch_is_exact(fetch, name) {
            return Err(format!(
                "Mesa 26 Python package {name} differs from its reviewed source"
            ));
        }
    }
    Ok(super::mesa20::python_packages())
}

fn verified_pyyaml_package(fetches: &[FetchDecl]) -> Result<PythonPackageDecl, String> {
    let matching = fetches
        .iter()
        .filter(|candidate| candidate.name == "mesa3d-pyyaml-fetch")
        .collect::<Vec<_>>();
    let [fetch] = matching.as_slice() else {
        return Err("Mesa 26 format generators require exactly one mesa3d-pyyaml-fetch".to_owned());
    };
    if fetch.archive != "pyyaml-6.0.3"
        || fetch.suffixes != "tar.gz"
        || fetch.origins
            != "https://files.pythonhosted.org/packages/05/8e/961c0007c59b8dd7729d542c61a4d537767a59645b82a0b521206e1e25c2"
        || fetch.checksums
            != "pyyaml-6.0.3.tar.gz=sha256:d76623373421df22fb4cf8817020cbb7ef15c725b9d5e45f17e189bfc384190f"
        || fetch.location != "${AROS_PORTS_SOURCE_DIR}"
        || fetch.destination != "${AROS_PORTS_DIR}/mesa-python"
        || !fetch.base.is_empty()
        || fetch.patch_origins != "${AROS_SOURCE_DIR}/workbench/libs/mesa"
        || fetch.patches != "::"
        || fetch.dir != "workbench/libs/mesa"
    {
        return Err("Mesa 26 PyYAML fetch differs from its audited source contract".to_owned());
    }
    Ok(PythonPackageDecl {
        fetch_target: "mesa3d-pyyaml-fetch".to_owned(),
        source_root: "${AROS_PORTS_DIR}/mesa-python/pyyaml-6.0.3".to_owned(),
        python_path: "lib".to_owned(),
    })
}

pub(crate) fn parse_glapi(
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> Result<Option<PythonOutputsDecl>, String> {
    const HEADER: &str = "src/mesa/glapi/shared-glapi/shared_glapi_mapi_tmp.h";
    if relative_dir != Path::new(GLAPI_DIR) || profile(target)?.is_none() {
        return Ok(None);
    }
    let matches = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == GLAPI_MMAKE)
        .collect::<Vec<_>>();
    let [glapi] = matches.as_slice() else {
        return Err(format!(
            "Mesa 26 glapi requires exactly one {GLAPI_MMAKE} declaration, found {}",
            matches.len()
        ));
    };
    let expected_sources = [
        format!("{SOURCE_ROOT}/src/mesa/glapi/shared-glapi/core"),
        format!("{BUILD_ROOT}/src/mesa/glapi/shared-glapi/public_glapi_wrappers"),
    ];
    let compile = glapi_compile_contract(relative_dir, GLAPI_MMAKE, target)?
        .ok_or_else(|| "Mesa 26 glapi compile contract is absent".to_owned())?;
    let defines_match = glapi.defines == compile.defines;
    let includes_match = glapi.include_dirs == compile.includes;
    let options_match = glapi.compile_options == compile.options;
    let compilation_matches = defines_match && includes_match && options_match;
    if glapi.target_name != "glapi"
        || glapi.module_type != ModuleType::LinkLib
        || glapi.source_files != expected_sources
        || !glapi.cxx_source_files.is_empty()
        || !glapi.objc_source_files.is_empty()
        || !glapi.asm_source_files.is_empty()
        || !glapi.use_libs.is_empty()
        || glapi.linklib_output_dir.as_deref() != Some(PRIVATE_LIBDIR)
        || glapi.canonical_linklib_output
        || !compilation_matches
    {
        return Err(
            "Mesa 26 glapi target differs from the audited archive, flags or output".to_owned(),
        );
    }

    verify_mesa_fetch(fetches)?;

    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-glapi-generate".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: "mesa3d-fetch".to_owned(),
        source_inputs: vec![
            "src/mesa/glapi/glapi/gen/gl_and_es_API.xml".to_owned(),
            "src/glx/libgl-symbols.txt".to_owned(),
        ],
        local_inputs: vec![
            "${AROS_SOURCE_DIR}/workbench/libs/mesa/libglapi/public_glapi_required_symbols.txt"
                .to_owned(),
        ],
        jobs: vec![
            PythonGeneratorJob {
                script: "src/mesa/glapi/mapi_abi.py".to_owned(),
                local_script: false,
                output: HEADER.to_owned(),
                arguments: vec!["header".to_owned()],
                depends_on_outputs: Vec::new(),
            },
            PythonGeneratorJob {
                script:
                    "${AROS_SOURCE_DIR}/workbench/libs/mesa/libglapi/gen_public_glapi_wrappers.sh"
                        .to_owned(),
                local_script: true,
                output: "src/mesa/glapi/shared-glapi/public_glapi_wrappers.c".to_owned(),
                arguments: vec!["wrappers".to_owned()],
                depends_on_outputs: vec![HEADER.to_owned()],
            },
        ],
        driver_script: Some(
            "${AROS_SOURCE_DIR}/workbench/libs/mesa/libglapi/mesa26_generate.py".to_owned(),
        ),
        requires_flex_bison: false,
        python_packages: Vec::new(),
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa-26.0.0-aros.diff".to_owned(),
        ],
        consumers: vec![GLAPI_MMAKE.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

pub(crate) fn parse_mesautil(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> Result<Option<PythonOutputsDecl>, String> {
    const DIR: &str = "workbench/libs/mesa/libmesautil";
    const CONSUMERS: [&str; 2] = ["mesa3d-linklib-mesautil", "mesa3d-linklib-mesadevutil"];
    if relative_dir != Path::new(DIR) || profile(target)?.is_none() {
        return Ok(None);
    }
    verify_mesa_fetch(fetches)?;
    let pyyaml = verified_pyyaml_package(fetches)?;
    for mmake in CONSUMERS {
        let matches = targets
            .iter()
            .filter(|candidate| candidate.mmake_name == mmake)
            .collect::<Vec<_>>();
        let [declaration] = matches.as_slice() else {
            return Err(format!(
                "Mesa 26 util requires exactly one {mmake} declaration, found {}",
                matches.len()
            ));
        };
        let expected_sources = archive_sources(root, relative_dir, mmake, target)?
            .ok_or_else(|| format!("Mesa 26 util source contract absent for {mmake}"))?;
        let contract = compile_contract(relative_dir, mmake, target)?
            .ok_or_else(|| format!("Mesa 26 util lacks compile contract for {mmake}"))?;
        let defines_match = declaration.defines == contract.defines;
        let includes_match = declaration.include_dirs == contract.includes;
        let options_match = declaration.compile_options == contract.options;
        if declaration.target_name
            != if mmake.ends_with("mesadevutil") {
                "mesadevutil"
            } else {
                "mesautil"
            }
            || declaration.module_type != ModuleType::LinkLib
            || declaration.source_files != expected_sources.c
            || declaration.cxx_source_files != expected_sources.cxx
            || !declaration.asm_source_files.is_empty()
            || !declaration.objc_source_files.is_empty()
            || declaration.linklib_output_dir.as_deref() != Some(PRIVATE_LIBDIR)
            || declaration.canonical_linklib_output
            || !defines_match
            || !includes_match
            || !options_match
        {
            return Err(format!(
                "Mesa 26 util target {mmake} differs from its audited source, flags or output"
            ));
        }
    }

    let yaml = format!("{SOURCE_ROOT}/src/util/format/u_format.yaml");
    let job = |script: &str, output: &str, arguments: Vec<String>| PythonGeneratorJob {
        script: script.to_owned(),
        local_script: false,
        output: output.to_owned(),
        arguments,
        depends_on_outputs: Vec::new(),
    };
    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-mesautil-generated".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: "mesa3d-fetch".to_owned(),
        source_inputs: vec![
            "src/util/format/u_format.yaml".to_owned(),
            "src/util/format/u_format_parse.py".to_owned(),
            "src/util/format/u_format_pack.py".to_owned(),
        ],
        local_inputs: Vec::new(),
        jobs: vec![
            job(
                "src/util/format/u_format_table.py",
                "src/util/format/u_format_gen.h",
                vec![yaml.clone(), "--enums".to_owned()],
            ),
            job(
                "src/util/format/u_format_table.py",
                "src/util/format/u_format_pack.h",
                vec![yaml.clone(), "--header".to_owned()],
            ),
            job(
                "src/util/format/u_format_table.py",
                "src/util/format/u_format_table.c",
                vec![yaml],
            ),
            job(
                "src/util/format_srgb.py",
                "src/util/format_srgb.c",
                Vec::new(),
            ),
        ],
        driver_script: None,
        requires_flex_bison: false,
        python_packages: vec![pyyaml],
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa-26.0.0-aros.diff".to_owned(),
        ],
        consumers: CONSUMERS.map(str::to_owned).to_vec(),
        dir_path: relative_dir.to_path_buf(),
    }))
}

fn job(script: &str, output: &str, arguments: &[&str]) -> PythonGeneratorJob {
    PythonGeneratorJob {
        script: script.to_owned(),
        local_script: false,
        output: output.to_owned(),
        arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
        depends_on_outputs: Vec::new(),
    }
}

pub(crate) fn parse_v3d(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> Result<Option<PythonOutputsDecl>, String> {
    const TARGET: &str = "linklibs-gallium_v3d";
    if relative_dir != Path::new(super::V3D_RELATIVE_DIR) || profile(target)? != Some("aarch64") {
        return Ok(None);
    }
    verify_mesa_fetch(fetches)?;
    let packages = verified_python_packages(fetches)?;
    let matching = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == TARGET)
        .collect::<Vec<_>>();
    let [declaration] = matching.as_slice() else {
        return Err("Mesa 26 V3D requires exactly one driver archive".to_owned());
    };
    let sources = archive_sources(root, relative_dir, TARGET, target)?
        .ok_or_else(|| "Mesa 26 V3D source contract is absent".to_owned())?;
    let compile = compile_contract(relative_dir, TARGET, target)?
        .ok_or_else(|| "Mesa 26 V3D compile contract is absent".to_owned())?;
    let includes_match = declaration.include_dirs == compile.includes;
    if declaration.target_name != "gallium_v3d"
        || declaration.module_type != ModuleType::LinkLib
        || declaration.source_files != sources.c
        || !declaration.cxx_source_files.is_empty()
        || !declaration.objc_source_files.is_empty()
        || !declaration.asm_source_files.is_empty()
        || declaration.linklib_output_dir.as_deref() != Some(PRIVATE_LIBDIR)
        || declaration.defines != compile.defines
        || !includes_match
        || declaration.compile_options != compile.options
    {
        return Err(
            "Mesa 26 V3D archive differs from its reviewed source/compile contract".to_owned(),
        );
    }

    let v3d = "src/gallium/drivers/v3d";
    let broadcom = "src/broadcom";
    let mut source_inputs = vec![
        format!("{broadcom}/compiler/v3d_nir_lower_algebraic.py"),
        "src/compiler/nir/nir_algebraic.py".to_owned(),
        format!("{broadcom}/cle/gen_pack_header.py"),
        format!("{broadcom}/cle/vc4_packet.xml"),
        format!("{broadcom}/cle/v3d_packet.xml"),
    ];
    let mut jobs = Vec::new();
    for (stem, directory) in [
        ("draw", v3d),
        ("emit", v3d),
        ("format_table", v3d),
        ("job", v3d),
        ("rcl", v3d),
        ("state", v3d),
        ("tfu", v3d),
        ("dump", "src/broadcom/clif"),
        ("counter", "src/broadcom/perfcntrs"),
    ] {
        let script = format!("{directory}/v3dx_{stem}.c");
        source_inputs.push(script.clone());
        for version in ["42", "71"] {
            jobs.push(job(
                &script,
                &format!("v3dx-gen/v3d{version}_{stem}.c"),
                &["v3dx-wrapper26", version],
            ));
        }
    }
    jobs.push(job(
        &format!("{broadcom}/compiler/v3d_nir_lower_algebraic.py"),
        "nir-gen/v3d_nir_lower_algebraic.c",
        &[
            "python-stdout",
            "-p",
            "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/compiler/nir",
        ],
    ));
    for (version, xml) in [
        ("21", "vc4_packet.xml"),
        ("42", "v3d_packet.xml"),
        ("71", "v3d_packet.xml"),
    ] {
        jobs.push(job(
            &format!("{broadcom}/cle/gen_pack_header.py"),
            &format!("cle-gen/broadcom/cle/v3d_packet_v{version}_pack.h"),
            &[
                "python-stdout",
                if xml == "vc4_packet.xml" {
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom/cle/vc4_packet.xml"
                } else {
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/broadcom/cle/v3d_packet.xml"
                },
                version,
            ],
        ));
    }
    Ok(Some(PythonOutputsDecl {
        owner: "linklibs-gallium_v3d-generated".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: super::V3D_AROS_BUILD_DIR.to_owned(),
        fetch_target: "mesa3d-fetch".to_owned(),
        source_inputs,
        local_inputs: vec!["${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa20_generate.py".to_owned()],
        jobs,
        driver_script: Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa26_generate.py".to_owned()),
        requires_flex_bison: false,
        python_packages: packages,
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa-26.0.0-aros.diff".to_owned(),
        ],
        consumers: vec![TARGET.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

pub(crate) fn parse_galliumaux(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> Result<Option<PythonOutputsDecl>, String> {
    const DIR: &str = "workbench/libs/mesa/libgalliumaux";
    const TARGET: &str = "mesa3d-linklib-galliumauxiliary";
    if relative_dir != Path::new(DIR) || profile(target)?.is_none() {
        return Ok(None);
    }
    verify_mesa_fetch(fetches)?;
    let packages = verified_python_packages(fetches)?;
    let matches = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == TARGET)
        .collect::<Vec<_>>();
    let [declaration] = matches.as_slice() else {
        return Err(format!("Mesa 26 galliumaux requires exactly one {TARGET}"));
    };
    let sources = archive_sources(root, relative_dir, TARGET, target)?
        .ok_or_else(|| "Mesa 26 galliumaux source contract is absent".to_owned())?;
    let compile = compile_contract(relative_dir, TARGET, target)?
        .ok_or_else(|| "Mesa 26 galliumaux compile contract is absent".to_owned())?;
    let includes_match = declaration.include_dirs == compile.includes;
    if declaration.target_name != "galliumauxiliary"
        || declaration.module_type != ModuleType::LinkLib
        || declaration.source_files != sources.c
        || declaration.cxx_source_files != sources.cxx
        || !declaration.asm_source_files.is_empty()
        || !declaration.objc_source_files.is_empty()
        || declaration.linklib_output_dir.as_deref() != Some(PRIVATE_LIBDIR)
        || declaration.canonical_linklib_output
        || declaration.defines != compile.defines
        || declaration.undefines != compile.undefines
        || !includes_match
        || declaration.compile_options != compile.options
    {
        return Err(
            "Mesa 26 galliumaux target differs from its audited archive, flags or output"
                .to_owned(),
        );
    }
    let prefix = "src/gallium/auxiliary";
    let c = "tr_util.c";
    let h = "tr_util.h";
    let trace_c = "u_tracepoints.c";
    let trace_h = "u_tracepoints.h";
    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-galliumauxiliary-generated".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: "mesa3d-fetch".to_owned(),
        source_inputs: vec![
            "src/util/perf/u_trace.py".to_owned(),
            "src/gallium/include/pipe/p_defines.h".to_owned(),
            "src/gallium/include/pipe/p_video_enums.h".to_owned(),
            "src/util/blend.h".to_owned(),
        ],
        local_inputs: vec!["${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa20_generate.py".to_owned()],
        jobs: vec![
            job(
                &format!("{prefix}/indices/u_indices_gen.py"),
                &format!("{prefix}/indices/u_indices_gen.c"),
                &["python-output", "@OUTPUT@"],
            ),
            job(
                &format!("{prefix}/indices/u_unfilled_gen.py"),
                &format!("{prefix}/indices/u_unfilled_gen.c"),
                &["python-output", "@OUTPUT@"],
            ),
            job(
                &format!("{prefix}/util/u_tracepoints.py"),
                &format!("{prefix}/util/{trace_c}"),
                &[
                    "python-dual-output",
                    trace_c,
                    trace_h,
                    "-p",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util/perf",
                    "-C",
                    "@OUT_C@",
                    "-H",
                    "@OUT_H@",
                ],
            ),
            job(
                &format!("{prefix}/util/u_tracepoints.py"),
                &format!("{prefix}/util/{trace_h}"),
                &[
                    "python-dual-output",
                    trace_c,
                    trace_h,
                    "-p",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util/perf",
                    "-C",
                    "@OUT_C@",
                    "-H",
                    "@OUT_H@",
                ],
            ),
            job(
                &format!("{prefix}/driver_trace/enums2names.py"),
                &format!("{prefix}/driver_trace/{c}"),
                &[
                    "python-dual-output",
                    c,
                    h,
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include/pipe/p_defines.h",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include/pipe/p_video_enums.h",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util/blend.h",
                    "-C",
                    "@OUT_C@",
                    "-H",
                    "@OUT_H@",
                ],
            ),
            job(
                &format!("{prefix}/driver_trace/enums2names.py"),
                &format!("{prefix}/driver_trace/{h}"),
                &[
                    "python-dual-output",
                    c,
                    h,
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include/pipe/p_defines.h",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/gallium/include/pipe/p_video_enums.h",
                    "${AROS_PORTS_DIR}/mesa/mesa-26.0.0/src/util/blend.h",
                    "-C",
                    "@OUT_C@",
                    "-H",
                    "@OUT_H@",
                ],
            ),
        ],
        driver_script: Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa26_generate.py".to_owned()),
        requires_flex_bison: false,
        python_packages: packages,
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa-26.0.0-aros.diff".to_owned(),
        ],
        consumers: vec![TARGET.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

pub(crate) fn parse_compiler(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> Result<Option<PythonOutputsDecl>, String> {
    const DIR: &str = "workbench/libs/mesa/libcompiler";
    const TARGET: &str = "mesa3d-linklib-compiler";
    if relative_dir != Path::new(DIR) || profile(target)?.is_none() {
        return Ok(None);
    }
    verify_mesa_fetch(fetches)?;
    let packages = verified_python_packages(fetches)?;
    let matches = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == TARGET)
        .collect::<Vec<_>>();
    let [declaration] = matches.as_slice() else {
        return Err(format!("Mesa 26 compiler requires exactly one {TARGET}"));
    };
    let sources = archive_sources(root, relative_dir, TARGET, target)?
        .ok_or_else(|| "Mesa 26 compiler source contract is absent".to_owned())?;
    let compile = compile_contract(relative_dir, TARGET, target)?
        .ok_or_else(|| "Mesa 26 compiler compile contract is absent".to_owned())?;
    let includes_match = declaration.include_dirs == compile.includes;
    if declaration.target_name != "compiler"
        || declaration.module_type != ModuleType::LinkLib
        || declaration.source_files != sources.c
        || declaration.cxx_source_files != sources.cxx
        || !declaration.asm_source_files.is_empty()
        || !declaration.objc_source_files.is_empty()
        || declaration.linklib_output_dir.as_deref() != Some(PRIVATE_LIBDIR)
        || declaration.canonical_linklib_output
        || declaration.defines != compile.defines
        || declaration.undefines != compile.undefines
        || !includes_match
        || declaration.compile_options != compile.options
    {
        return Err(
            "Mesa 26 compiler target differs from its audited archive, flags or output".to_owned(),
        );
    }

    let mut jobs = Vec::new();
    let compiler = "src/compiler";
    let nir = format!("{compiler}/nir");
    let glsl = format!("{compiler}/glsl");
    let glcpp = format!("{glsl}/glcpp");
    let spirv = format!("{compiler}/spirv");
    let input = |path: &str| format!("{SOURCE_ROOT}/{path}");
    jobs.extend([
        job(
            &format!("{compiler}/builtin_types_c.py"),
            &format!("{compiler}/builtin_types_data.c"),
            &["python-output", "@OUTPUT@"],
        ),
        job(
            &format!("{compiler}/builtin_types_h.py"),
            &format!("{compiler}/builtin_types.h"),
            &["python-output", "@OUTPUT@"],
        ),
        job(
            &format!("{nir}/nir_builder_opcodes_h.py"),
            &format!("{nir}/nir_builder_opcodes.h"),
            &["python-stdout", &input(&format!("{nir}/nir_opcodes.py"))],
        ),
        job(
            &format!("{nir}/nir_constant_expressions.py"),
            &format!("{nir}/nir_constant_expressions.c"),
            &["python-stdout", &input(&format!("{nir}/nir_opcodes.py"))],
        ),
        job(
            &format!("{nir}/nir_intrinsics_h.py"),
            &format!("{nir}/nir_intrinsics.h"),
            &["python-output", "--out", "@OUTPUT@"],
        ),
        job(
            &format!("{nir}/nir_intrinsics_c.py"),
            &format!("{nir}/nir_intrinsics.c"),
            &["python-output", "--out", "@OUTPUT@"],
        ),
        job(
            &format!("{nir}/nir_intrinsics_indices_h.py"),
            &format!("{nir}/nir_intrinsics_indices.h"),
            &["python-output", "--out", "@OUTPUT@"],
        ),
        job(
            &format!("{nir}/nir_opcodes_h.py"),
            &format!("{nir}/nir_opcodes.h"),
            &["python-stdout", &input(&format!("{nir}/nir_opcodes.py"))],
        ),
        job(
            &format!("{nir}/nir_opcodes_c.py"),
            &format!("{nir}/nir_opcodes.c"),
            &["python-stdout", &input(&format!("{nir}/nir_opcodes.py"))],
        ),
        job(
            &format!("{nir}/nir_opt_algebraic.py"),
            &format!("{nir}/nir_opt_algebraic.c"),
            &["python-output", "--out", "@OUTPUT@"],
        ),
    ]);
    for (output, argument) in [
        ("ir_expression_operation.h", "enum"),
        ("ir_expression_operation_constant.h", "constant"),
        ("ir_expression_operation_strings.h", "strings"),
    ] {
        jobs.push(job(
            &format!("{glsl}/ir_expression_operation.py"),
            &format!("{glsl}/{output}"),
            &["python-stdout", argument],
        ));
    }
    for (source, output, symbol) in [
        ("float64.glsl", "float64_glsl.h", "float64_source"),
        (
            "CrossPlatformSettings_piece_all.glsl",
            "cross_platform_settings_piece_all.h",
            "cross_platform_settings_piece_all_header",
        ),
        ("bc1.glsl", "bc1_glsl.h", "bc1_source"),
        ("bc4.glsl", "bc4_glsl.h", "bc4_source"),
        (
            "etc2_rgba_stitch.glsl",
            "etc2_rgba_stitch_glsl.h",
            "etc2_rgba_stitch_source",
        ),
        ("astc_decoder.glsl", "astc_glsl.h", "astc_source"),
    ] {
        jobs.push(job(
            "src/util/xxd.py",
            &format!("{glsl}/{output}"),
            &[
                "python-output",
                &input(&format!("{glsl}/{source}")),
                "@OUTPUT@",
                "-n",
                symbol,
            ],
        ));
    }
    jobs.extend([
        job(
            &format!("{glcpp}/glcpp-lex.l"),
            &format!("{glcpp}/glcpp-lex.c"),
            &["flex", "--nounistd"],
        ),
        job(
            &format!("{glcpp}/glcpp-parse.y"),
            &format!("{glcpp}/glcpp-parse.c"),
            &["bison", "glcpp-parse.c", "glcpp-parse.h", "glcpp_parser_"],
        ),
        job(
            &format!("{glcpp}/glcpp-parse.y"),
            &format!("{glcpp}/glcpp-parse.h"),
            &["bison", "glcpp-parse.c", "glcpp-parse.h", "glcpp_parser_"],
        ),
        job(
            &format!("{glsl}/glsl_lexer.ll"),
            &format!("{glsl}/glsl_lexer.cpp"),
            &["flex", "--nounistd"],
        ),
        job(
            &format!("{glsl}/glsl_parser.yy"),
            &format!("{glsl}/glsl_parser.cpp"),
            &["bison", "glsl_parser.cpp", "glsl_parser.h", "_mesa_glsl_"],
        ),
        job(
            &format!("{glsl}/glsl_parser.yy"),
            &format!("{glsl}/glsl_parser.h"),
            &["bison", "glsl_parser.cpp", "glsl_parser.h", "_mesa_glsl_"],
        ),
        job(
            &format!("{spirv}/spirv_info_gen.py"),
            &format!("{spirv}/spirv_info.c"),
            &[
                "python-dual-output",
                "spirv_info.c",
                "spirv_info.h",
                "--json",
                &input(&format!("{spirv}/spirv.core.grammar.json")),
                "--out-h",
                "@OUT_H@",
                "--out-c",
                "@OUT_C@",
            ],
        ),
        job(
            &format!("{spirv}/spirv_info_gen.py"),
            &format!("{spirv}/spirv_info.h"),
            &[
                "python-dual-output",
                "spirv_info.c",
                "spirv_info.h",
                "--json",
                &input(&format!("{spirv}/spirv.core.grammar.json")),
                "--out-h",
                "@OUT_H@",
                "--out-c",
                "@OUT_C@",
            ],
        ),
        job(
            &format!("{spirv}/vtn_gather_types_c.py"),
            &format!("{spirv}/vtn_gather_types.c"),
            &[
                "python-output",
                &input(&format!("{spirv}/spirv.core.grammar.json")),
                "@OUTPUT@",
            ],
        ),
        job(
            &format!("{spirv}/vtn_generator_ids_h.py"),
            &format!("{spirv}/vtn_generator_ids.h"),
            &[
                "python-output",
                &input(&format!("{spirv}/spir-v.xml")),
                "@OUTPUT@",
            ],
        ),
    ]);
    if jobs.len() != 29 {
        return Err(format!(
            "Mesa 26 compiler generator inventory has {} instead of 29 outputs",
            jobs.len()
        ));
    }
    let source_inputs = [
        "src/compiler/builtin_types.py",
        "src/compiler/nir/nir_opcodes.py",
        "src/compiler/nir/nir_intrinsics.py",
        "src/compiler/nir/nir_algebraic.py",
        "src/compiler/nir/nir_constant_expressions.h",
        "src/compiler/glsl/float64.glsl",
        "src/compiler/glsl/CrossPlatformSettings_piece_all.glsl",
        "src/compiler/glsl/bc1.glsl",
        "src/compiler/glsl/bc4.glsl",
        "src/compiler/glsl/etc2_rgba_stitch.glsl",
        "src/compiler/glsl/astc_decoder.glsl",
        "src/compiler/spirv/spirv.core.grammar.json",
        "src/compiler/spirv/spir-v.xml",
    ];
    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-compiler-generated".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: "mesa3d-fetch".to_owned(),
        source_inputs: source_inputs.map(str::to_owned).to_vec(),
        local_inputs: vec!["${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa20_generate.py".to_owned()],
        jobs,
        driver_script: Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa26_generate.py".to_owned()),
        requires_flex_bison: true,
        python_packages: packages,
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa-26.0.0-aros.diff".to_owned(),
        ],
        consumers: vec![TARGET.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

pub(crate) fn parse_core(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> Result<Option<PythonOutputsDecl>, String> {
    const DIR: &str = "workbench/libs/mesa/libmesa";
    const TARGET: &str = "mesa3d-linklib-mesa";
    if relative_dir != Path::new(DIR) {
        return Ok(None);
    }
    let Some(profile) = profile(target)? else {
        return Ok(None);
    };
    verify_mesa_fetch(fetches)?;
    let packages = verified_python_packages(fetches)?;
    let matches = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == TARGET)
        .collect::<Vec<_>>();
    let [declaration] = matches.as_slice() else {
        return Err(format!("Mesa 26 core requires exactly one {TARGET}"));
    };
    let sources = archive_sources(root, relative_dir, TARGET, target)?
        .ok_or_else(|| "Mesa 26 core source contract is absent".to_owned())?;
    let compile = compile_contract(relative_dir, TARGET, target)?
        .ok_or_else(|| "Mesa 26 core compile contract is absent".to_owned())?;
    let includes_match = declaration.include_dirs == compile.includes;
    if declaration.target_name != "mesa"
        || declaration.module_type != ModuleType::LinkLib
        || declaration.source_files != sources.c
        || declaration.cxx_source_files != sources.cxx
        || !declaration.asm_source_files.is_empty()
        || !declaration.objc_source_files.is_empty()
        || declaration.linklib_output_dir.as_deref() != Some(PRIVATE_LIBDIR)
        || declaration.canonical_linklib_output
        || declaration.defines != compile.defines
        || declaration.undefines != compile.undefines
        || !includes_match
        || declaration.compile_options != compile.options
    {
        return Err(
            "Mesa 26 core target differs from its audited archive, flags or output".to_owned(),
        );
    }

    let ptr = if profile == "arm" { "4" } else { "8" };
    let gen = "src/mesa/glapi/glapi/gen";
    let main = "src/mesa/main";
    let xml = format!("{SOURCE_ROOT}/{gen}/gl_and_es_API.xml");
    let api_xml = format!("{SOURCE_ROOT}/{gen}/gl_API.xml");
    let registry = format!("{SOURCE_ROOT}/src/mesa/glapi/glapi/registry/gl.xml");
    let csv = format!("{SOURCE_ROOT}/{main}/formats.csv");
    let mut jobs = vec![
        job(
            &format!("{gen}/gl_table.py"),
            &format!("{main}/dispatch.h"),
            &["python-stdout", "-m", "dispatch", "-f", &xml],
        ),
        job(
            &format!("{gen}/gl_enums.py"),
            &format!("{main}/enums.c"),
            &["python-stdout", "-f", &registry],
        ),
    ];
    for (script, output, input) in [
        ("api_exec_init.py", "api_exec_init.c", xml.as_str()),
        ("api_exec_decl_h.py", "api_exec_decl.h", xml.as_str()),
        ("api_save_init_h.py", "api_save_init.h", xml.as_str()),
        ("api_save_h.py", "api_save.h", xml.as_str()),
        (
            "api_beginend_init_h.py",
            "api_beginend_init.h",
            xml.as_str(),
        ),
        (
            "api_hw_select_init_h.py",
            "api_hw_select_init.h",
            api_xml.as_str(),
        ),
    ] {
        jobs.push(job(
            &format!("{gen}/{script}"),
            &format!("{main}/{output}"),
            &["python-stdout", "-f", input],
        ));
    }
    jobs.extend([
        job(
            &format!("{gen}/marshal_generated_h.py"),
            &format!("{main}/marshal_generated.h"),
            &["python-stdout", &xml, ptr],
        ),
        job(
            &format!("{gen}/unmarshal_table_c.py"),
            &format!("{main}/unmarshal_table.c"),
            &["python-stdout", &xml, ptr],
        ),
    ]);
    for shard in 0..8 {
        jobs.push(job(
            &format!("{gen}/marshal_generated_c.py"),
            &format!("{main}/marshal_generated{shard}.c"),
            &["python-stdout", &xml, &shard.to_string(), "8", ptr],
        ));
    }
    jobs.extend([
        job(
            &format!("{main}/get_hash_generator.py"),
            &format!("{main}/get_hash.h"),
            &["python-stdout", "-f", &xml],
        ),
        job(
            &format!("{main}/format_info.py"),
            &format!("{main}/format_info.h"),
            &["python-stdout", &csv],
        ),
        job(
            &format!("{main}/format_fallback.py"),
            &format!("{main}/format_fallback.c"),
            &["python-output", &csv, "@OUTPUT@"],
        ),
        job("VERSION", &format!("{main}/git_sha1.h"), &["mesa-git-sha1"]),
        job(
            "src/mesa/program/program_lexer.l",
            "src/mesa/program/lex.yy.c",
            &["flex", "--nounistd", "--never-interactive"],
        ),
        job(
            "src/mesa/program/program_parse.y",
            "src/mesa/program/program_parse.tab.c",
            &[
                "bison",
                "program_parse.tab.c",
                "program_parse.tab.h",
                "_mesa_program_",
            ],
        ),
        job(
            "src/mesa/program/program_parse.y",
            "src/mesa/program/program_parse.tab.h",
            &[
                "bison",
                "program_parse.tab.c",
                "program_parse.tab.h",
                "_mesa_program_",
            ],
        ),
    ]);
    if jobs.len() != 25 {
        return Err(format!(
            "Mesa 26 core generator inventory has {} instead of 25 outputs",
            jobs.len()
        ));
    }
    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-mesa-generated".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: "mesa3d-fetch".to_owned(),
        source_inputs: [
            "src/mesa/glapi/glapi/gen/gl_and_es_API.xml",
            "src/mesa/glapi/glapi/gen/gl_API.xml",
            "src/mesa/glapi/glapi/registry/gl.xml",
            "src/mesa/glapi/glapi/gen/gl_XML.py",
            "src/mesa/glapi/glapi/gen/glX_XML.py",
            "src/mesa/glapi/glapi/gen/marshal_XML.py",
            "src/mesa/glapi/glapi/gen/license.py",
            "src/mesa/glapi/glapi/gen/static_data.py",
            "src/mesa/glapi/new/genCommon.py",
            "src/mesa/main/get_hash_params.py",
            "src/mesa/main/formats.csv",
            "src/mesa/main/format_parser.py",
            "src/glx/libgl-symbols.txt",
            "VERSION",
        ]
        .map(str::to_owned)
        .to_vec(),
        local_inputs: vec!["${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa20_generate.py".to_owned()],
        jobs,
        driver_script: Some("${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa26_generate.py".to_owned()),
        requires_flex_bison: true,
        python_packages: packages,
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${AROS_SOURCE_DIR}/workbench/libs/mesa/mesa-26.0.0-aros.diff".to_owned(),
        ],
        consumers: vec![TARGET.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

pub(crate) fn validate_empty_sse41(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> Result<(), String> {
    const MMAKE: &str = "mesa3d-linklib-mesa-sse41";
    if relative_dir != Path::new("workbench/libs/mesa/libmesa") || profile(target)?.is_none() {
        return Ok(());
    }
    verify_mesa_fetch(fetches)?;
    archive_sources(root, relative_dir, MMAKE, target)?
        .ok_or_else(|| "Mesa 26 empty SSE4.1 archive source contract is absent".to_owned())?;
    let matches = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == MMAKE)
        .collect::<Vec<_>>();
    let [archive] = matches.as_slice() else {
        return Err(format!(
            "Mesa 26 requires one {MMAKE}, found {}",
            matches.len()
        ));
    };
    let compile = compile_contract(relative_dir, MMAKE, target)?
        .ok_or_else(|| "Mesa 26 empty SSE4.1 archive compile contract is absent".to_owned())?;
    let includes_match = archive.include_dirs == compile.includes;
    if archive.target_name != "mesa-sse41"
        || archive.module_type != ModuleType::LinkLib
        || !archive.empty_archive
        || !archive.source_files.is_empty()
        || !archive.cxx_source_files.is_empty()
        || !archive.asm_source_files.is_empty()
        || !archive.objc_source_files.is_empty()
        || archive.linklib_output_dir.as_deref() != Some(PRIVATE_LIBDIR)
        || archive.canonical_linklib_output
        || archive.defines != compile.defines
        || archive.undefines != compile.undefines
        || !includes_match
        || archive.compile_options != compile.options
    {
        return Err(
            "Mesa 26 SSE4.1 compatibility archive is not the audited empty archive".to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "mesa26_tests.rs"]
mod tests;
