//! The Nouveau DRM and Gallium lanes.
//!
//! Two `%build_module` declarations whose source lists are Make manifests of
//! 825 and 105 files, read rather than globbed, plus a compile contract whose
//! flags and includes the mmakefile decides. Both are pinned on the manifests
//! and the mmakefile, and both count their sources: a manifest that grows a
//! file silently would otherwise pass.

use super::{file_has_sha256, manifest_inventory};
use crate::ast::{ModuleType, TargetDefinition};
// Nouveau Gallium compiles Mesa sources, so it shares the Mesa compile
// contract and base defines. The coupling is real rather than accidental, and
// it now reads as one capability using another rather than as two families
// sharing a file.
use super::mesa::{base_defines, CompileContract};
use crate::parser::TargetContext;
use crate::pins::pin;
use crate::sources::EvaluatedSources;
use std::path::Path;

const NOUVEAU_DRM_DIR: &str = "workbench/hidds/nouveau";
pub(crate) const DRM_MMAKE: &str = "hidd-nouveau-drm";
const NOUVEAU_DRM_MMAKEFILE: &str = "workbench/hidds/nouveau/mmakefile.src";
const NOUVEAU_DRM_SOURCE_MANIFEST: &str = "workbench/hidds/nouveau/sources.drm.mak";
const NOUVEAU_DRM_CORE_SOURCE_COUNT: usize = 67;
const NOUVEAU_DRM_NVIDIA_SOURCE_COUNT: usize = 758;
const NOUVEAU_DRM_TOTAL_SOURCE_COUNT: usize =
    NOUVEAU_DRM_CORE_SOURCE_COUNT + NOUVEAU_DRM_NVIDIA_SOURCE_COUNT;
const NOUVEAU_DRM_SOURCE_PREFIX: &str = "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau";
pub(crate) const GALLIUM_MMAKE: &str = "hidd-nouveau-gallium";
const NOUVEAU_GALLIUM_SOURCE_MANIFEST: &str =
    "workbench/hidds/nouveau/nouveau-gallium-20.0.8.sources";
const NOUVEAU_GALLIUM_C_SOURCE_COUNT: usize = 81;
const NOUVEAU_GALLIUM_CXX_SOURCE_COUNT: usize = 24;
const NOUVEAU_GALLIUM_SOURCE_PREFIX: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/gallium";

/// Selects the three profiles for which the DRM-side Nouveau source snapshot
/// was audited.  The archive has no architecture-specific source lane, but a
/// concrete profile is still required so unsupported configurations cannot
/// silently inherit this closed capability.
fn nouveau_current_profile(
    target: Option<&TargetContext>,
) -> std::result::Result<&'static str, String> {
    let Some(profile) = target else {
        return Err("Nouveau archive capability requires a concrete target profile".to_owned());
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
            "Nouveau archive capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

/// Loads the two literal source inventories kept with the Nouveau DRM port.
/// This deliberately does not broaden the local-Make include parser: the
/// capability owns this exact, SHA-pinned fragment and nothing else in the
/// surrounding mixed DRM/Gallium makefile.
pub(crate) fn drm_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<EvaluatedSources>, String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) || mmake != DRM_MMAKE {
        return Ok(None);
    }
    nouveau_current_profile(target)?;

    let core = manifest_inventory(root, NOUVEAU_DRM_SOURCE_MANIFEST, "AROS_DRM_CORE_SOURCES")?;
    let nvidia = manifest_inventory(root, NOUVEAU_DRM_SOURCE_MANIFEST, "AROS_DRM_NVIDIA_SOURCES")?;
    if core.len() != NOUVEAU_DRM_CORE_SOURCE_COUNT
        || nvidia.len() != NOUVEAU_DRM_NVIDIA_SOURCE_COUNT
    {
        return Err(format!(
            "{NOUVEAU_DRM_SOURCE_MANIFEST} source inventory has {} core and {} NVIDIA entries, expected {NOUVEAU_DRM_CORE_SOURCE_COUNT} and {NOUVEAU_DRM_NVIDIA_SOURCE_COUNT}",
            core.len(),
            nvidia.len()
        ));
    }

    let mut sources = EvaluatedSources {
        declared: true,
        ..EvaluatedSources::default()
    };
    for source in core.into_iter().chain(nvidia) {
        let physical_source = root.join(NOUVEAU_DRM_DIR).join(format!("{source}.c"));
        if !physical_source.is_file() {
            return Err(format!(
                "{NOUVEAU_DRM_SOURCE_MANIFEST} declares missing C source {}",
                physical_source.display()
            ));
        }
        sources
            .c
            .push(format!("{NOUVEAU_DRM_SOURCE_PREFIX}/{source}"));
    }
    if sources.c.len() != NOUVEAU_DRM_TOTAL_SOURCE_COUNT {
        return Err(format!(
            "{NOUVEAU_DRM_SOURCE_MANIFEST} materialized {} C sources, expected {NOUVEAU_DRM_TOTAL_SOURCE_COUNT}",
            sources.c.len()
        ));
    }
    Ok(Some(sources))
}

pub(crate) struct NouveauDrmCompileContract {
    pub(crate) defines: Vec<String>,
    pub(crate) includes: Vec<String>,
    pub(crate) options: Vec<String>,
}

/// The compile inputs of the legacy `hidd-nouveau-drm` target, written as an
/// explicit CMake contract so each source can be materialised on a cold tree.
///
/// The legacy target inherits the LLVM toolchain's normal-build `-O2` through
/// `OPTIMIZATION_CFLAGS`; it is not optional for this source snapshot.
/// `drm_edid.c` uses an `__always_inline` table lookup in a `BUILD_BUG_ON`,
/// which Clang cannot reduce at `-O0`.
pub(crate) fn drm_compile_contract(
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<NouveauDrmCompileContract>, String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) || mmake != DRM_MMAKE {
        return Ok(None);
    }
    nouveau_current_profile(target)?;
    Ok(Some(NouveauDrmCompileContract {
        defines: [
            "__KERNEL__",
            "CONFIG_NOUVEAU_DEBUG=5",
            "CONFIG_NOUVEAU_DEBUG_DEFAULT=3",
            "CONFIG_DRM_NOUVEAU_GSP_DEFAULT=1",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        includes: [
            "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
            "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/include/uapi",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/include",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/include/nvkm",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/nvkm",
            "${CMAKE_SOURCE_DIR}/workbench/hidds/nouveau/drm/nouveau/nvkm/subdev/gsp",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        options: [
            "-O2",
            "-Wno-uninitialized",
            "-Wno-strict-aliasing",
            "-Wno-unused-but-set-variable",
            "-Wno-unused-variable",
            "-Wno-unused-function",
            "-Wno-missing-braces",
            "-std=gnu11",
            "-fno-strict-aliasing",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    }))
}

pub(crate) fn validate_drm(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
) -> std::result::Result<(), String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) {
        return Ok(());
    }
    if !file_has_sha256(root, NOUVEAU_DRM_MMAKEFILE, pin("nouveau-drm-mmakefile"))
        || !file_has_sha256(
            root,
            NOUVEAU_DRM_SOURCE_MANIFEST,
            pin("nouveau-drm-source-manifest"),
        )
    {
        return Err(
            "mmakefile or sources.drm.mak differs from the audited Nouveau DRM capability"
                .to_owned(),
        );
    }
    let expected_sources = drm_sources(root, relative_dir, DRM_MMAKE, target)?
        .ok_or_else(|| format!("missing source capability for {DRM_MMAKE}"))?;
    let expected_flags = drm_compile_contract(relative_dir, DRM_MMAKE, target)?
        .ok_or_else(|| format!("missing compile capability for {DRM_MMAKE}"))?;
    let matching = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == DRM_MMAKE)
        .collect::<Vec<_>>();
    let [declaration] = matching.as_slice() else {
        return Err(format!(
            "requires exactly one {DRM_MMAKE} declaration, found {}",
            matching.len()
        ));
    };
    let exact = declaration.target_name == "drm_nouveau"
        && declaration.module_type == ModuleType::LinkLib
        && !declaration.genmodule_only
        && !declaration.empty_archive
        && declaration.source_files == expected_sources.c
        && declaration.cxx_source_files.is_empty()
        && declaration.objc_source_files.is_empty()
        && declaration.asm_source_files.is_empty()
        && declaration.use_libs.is_empty()
        && declaration.dependencies.is_empty()
        && declaration.dir_path == relative_dir
        && declaration.target_dir.is_none()
        && !declaration.variant_32bit
        && declaration.link_libs.is_empty()
        && declaration.declared_mod_type.is_none()
        && declaration.mod_suffix.is_none()
        && declaration.linklib_name.is_none()
        && declaration.genmodule_linklibs.is_none()
        && declaration.linklib_output_dir.is_none()
        && declaration.canonical_linklib_output
        && declaration.canonical_linklib_eligible
        && declaration.compiler_flags.is_empty()
        && declaration.arch_modules.is_empty()
        && declaration.arch_includes.is_empty()
        && declaration.undefines.is_empty()
        && declaration.link_options.is_empty()
        && declaration.arch_sources.is_empty()
        && declaration.arch_defines.is_empty()
        && declaration.arch_compile_options.is_empty()
        && declaration.defines == expected_flags.defines
        && (declaration.include_dirs == expected_flags.includes)
        && declaration.compile_options == expected_flags.options;
    if !exact {
        return Err(
            "source, language, flag, include or canonical-output contract differs from the audited Nouveau DRM capability"
                .to_owned(),
        );
    }
    Ok(())
}

/// Loads the exact Mesa 20.0.8 Nouveau Gallium source lanes.  The upstream
/// `Makefile.sources` lives below the fetched port tree and cannot be read on
/// a cold configure, so the AROS port keeps this versioned, literal inventory
/// beside the declaring mmakefile.  It deliberately names only extensionless
/// stems: the C/C++ lane is the declaration's source-language authority.
pub(crate) fn gallium_sources(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<EvaluatedSources>, String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) || mmake != GALLIUM_MMAKE {
        return Ok(None);
    }
    nouveau_current_profile(target)?;

    let c = manifest_inventory(
        root,
        NOUVEAU_GALLIUM_SOURCE_MANIFEST,
        "NOUVEAU20_GALLIUM_C_SOURCES",
    )?;
    let cxx = manifest_inventory(
        root,
        NOUVEAU_GALLIUM_SOURCE_MANIFEST,
        "NOUVEAU20_GALLIUM_CXX_SOURCES",
    )?;
    if c.len() != NOUVEAU_GALLIUM_C_SOURCE_COUNT || cxx.len() != NOUVEAU_GALLIUM_CXX_SOURCE_COUNT {
        return Err(format!(
            "{NOUVEAU_GALLIUM_SOURCE_MANIFEST} has {} C and {} C++ entries, expected {NOUVEAU_GALLIUM_C_SOURCE_COUNT} and {NOUVEAU_GALLIUM_CXX_SOURCE_COUNT}",
            c.len(),
            cxx.len()
        ));
    }

    let materialize = |sources: Vec<String>, language: &str| {
        sources
            .into_iter()
            .map(|source| {
                if Path::new(&source).extension().is_some() {
                    Err(format!(
                        "{NOUVEAU_GALLIUM_SOURCE_MANIFEST} {language} inventory must contain extensionless stems: {source}"
                    ))
                } else {
                    Ok(format!("{NOUVEAU_GALLIUM_SOURCE_PREFIX}/{source}"))
                }
            })
            .collect::<std::result::Result<Vec<_>, _>>()
    };
    Ok(Some(EvaluatedSources {
        c: materialize(c, "C")?,
        cxx: materialize(cxx, "C++")?,
        declared: true,
        ..EvaluatedSources::default()
    }))
}

/// The concrete compile contract for the Mesa 20.0.8 Nouveau Gallium port.
/// Its C++ lane is intentionally an ordinary C++14 lane, not the tiny Mesa
/// compiler `cxx-compat/new` shim: Nouveau uses the real STL container API.
/// A target toolchain must therefore provide its own compatible C++ headers
/// and runtime before this archive can be built.
pub(crate) fn gallium_compile_contract(
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<CompileContract>, String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) || mmake != GALLIUM_MMAKE {
        return Ok(None);
    }
    let profile = nouveau_current_profile(target)?;
    Ok(Some(CompileContract {
        defines: base_defines(profile),
        includes: [
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
        .collect(),
        options: [
            "$<$<COMPILE_LANGUAGE:C>:-std=gnu11>",
            "$<$<COMPILE_LANGUAGE:CXX>:-std=gnu++14>",
            "-fno-strict-aliasing",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    }))
}

pub(crate) fn validate_gallium(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
) -> std::result::Result<(), String> {
    if relative_dir != Path::new(NOUVEAU_DRM_DIR) {
        return Ok(());
    }
    if !file_has_sha256(root, NOUVEAU_DRM_MMAKEFILE, pin("nouveau-drm-mmakefile"))
        || !file_has_sha256(
            root,
            NOUVEAU_GALLIUM_SOURCE_MANIFEST,
            pin("nouveau-gallium-source-manifest"),
        )
    {
        return Err(
            "mmakefile or Nouveau Gallium source manifest differs from the audited capability"
                .to_owned(),
        );
    }
    let expected_sources = gallium_sources(root, relative_dir, GALLIUM_MMAKE, target)?
        .ok_or_else(|| format!("missing source capability for {GALLIUM_MMAKE}"))?;
    let expected_flags = gallium_compile_contract(relative_dir, GALLIUM_MMAKE, target)?
        .ok_or_else(|| format!("missing compile capability for {GALLIUM_MMAKE}"))?;
    let matching = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == GALLIUM_MMAKE)
        .collect::<Vec<_>>();
    let [declaration] = matching.as_slice() else {
        return Err(format!(
            "requires exactly one {GALLIUM_MMAKE} declaration, found {}",
            matching.len()
        ));
    };
    let exact = declaration.target_name == "gallium_nouveau"
        && declaration.module_type == ModuleType::LinkLib
        && !declaration.genmodule_only
        && !declaration.empty_archive
        && declaration.source_files == expected_sources.c
        && declaration.cxx_source_files == expected_sources.cxx
        && declaration.objc_source_files.is_empty()
        && declaration.asm_source_files.is_empty()
        && declaration.use_libs.is_empty()
        && declaration.dependencies.is_empty()
        && declaration.dir_path == relative_dir
        && declaration.target_dir.is_none()
        && !declaration.variant_32bit
        && declaration.link_libs.is_empty()
        && declaration.declared_mod_type.is_none()
        && declaration.mod_suffix.is_none()
        && declaration.linklib_name.is_none()
        && declaration.genmodule_linklibs.is_none()
        && declaration.linklib_output_dir.is_none()
        && declaration.canonical_linklib_output
        && declaration.canonical_linklib_eligible
        && declaration.compiler_flags.is_empty()
        && declaration.arch_modules.is_empty()
        && declaration.arch_includes.is_empty()
        && declaration.undefines.is_empty()
        && declaration.link_options.is_empty()
        && declaration.arch_sources.is_empty()
        && declaration.arch_defines.is_empty()
        && declaration.arch_compile_options.is_empty()
        && declaration.defines == expected_flags.defines
        && declaration.include_dirs == expected_flags.includes
        && declaration.compile_options == expected_flags.options;
    if !exact {
        return Err(
            "source, language, flag, include or canonical-output contract differs from the audited Nouveau Gallium capability"
                .to_owned(),
        );
    }
    Ok(())
}
