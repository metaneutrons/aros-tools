//! The Mesa SSE4.1 archive.
//!
//! One `%build_linklib` declaration that has to be compiled with `-msse4.1` and
//! a source list of its own, on x86 only. What is decided here is whether the
//! recipe block, the local and config Make context, the source manifest, the
//! central fetch edge and the local patch are all the audited inputs, and which
//! of the two supported profiles applies.

use super::super::{
    normalized_make_capability_block, require_file_fingerprint, require_text_fingerprint,
};
use crate::ast::{ModuleType, TargetDefinition};
use crate::fetch::FetchDecl;
use crate::fingerprints::fingerprint;
use crate::parser::{join_mm_continuations, TargetContext, META_RULE_RE};
use std::fs;
use std::path::Path;

pub(crate) const DIRECTORY: &str = "workbench/libs/mesa/libmesa";
pub(crate) const MMAKE: &str = "mesa3d-linklib-mesa-sse41";
pub(crate) const INCLUDES: &[&str] = &[
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
];

pub(crate) fn sources(x86_64: bool) -> Vec<String> {
    if x86_64 {
        vec![
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main/streaming-load-memcpy".to_owned(),
            "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main/sse_minmax".to_owned(),
        ]
    } else {
        Vec::new()
    }
}

pub(crate) fn defines(x86_64: bool) -> Vec<String> {
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
    if x86_64 {
        defines.extend(["USE_X86_64_ASM".to_owned(), "USE_SSE41".to_owned()]);
    }
    defines.extend(["MAPI_MODE_GLAPI".to_owned(), "MAPI_MODE_UTIL".to_owned()]);
    defines
}

pub(crate) fn compile_options(x86_64: bool) -> Vec<String> {
    let mut options = vec!["-std=gnu11".to_owned(), "-fno-strict-aliasing".to_owned()];
    if x86_64 {
        options.push("-msse4.1".to_owned());
    }
    options
}

/// Classifies the three target profiles covered by the audited Mesa 20.0.8
/// SSE4.1 declaration. The boolean is true only for the profile which has
/// actual SSE sources; the two Raspberry Pi profiles intentionally archive no
/// objects but still publish the library required by the common link line.
pub(crate) fn profile(
    relative_dir: &Path,
    target: Option<&TargetContext>,
) -> std::result::Result<Option<bool>, String> {
    if relative_dir != Path::new(DIRECTORY) {
        return Ok(None);
    }
    let Some(profile) = target else {
        return Err("Mesa SSE4.1 capability requires a concrete target profile".to_owned());
    };
    let key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    match key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => {
            Ok(Some(true))
        }
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard"))
        | (
            Some("aarch64"),
            Some("raspi"),
            Some("llvm"),
            Some(""),
            Some("1"),
            Some(""),
        ) => Ok(Some(false)),
        _ => Err(format!(
            "Mesa SSE4.1 capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
            profile.cpu.as_deref().unwrap_or("<unset>"),
            profile.platform.as_deref().unwrap_or("<unset>"),
            profile.toolchain.as_deref().unwrap_or("<unset>"),
            profile.cpu32.as_deref().unwrap_or("<unset>"),
            profile.use_mmu.as_deref().unwrap_or("<unset>"),
            profile.float_abi.as_deref().unwrap_or("<unset>")
        )),
    }
}

pub(crate) fn fetch_edge_is_supported(make_source: &str) -> bool {
    let joined = join_mm_continuations(make_source);
    let matching = META_RULE_RE
        .captures_iter(&joined)
        .filter(|capture| &capture[1] == MMAKE)
        .collect::<Vec<_>>();
    let [edge] = matching.as_slice() else {
        return false;
    };
    !edge[0].starts_with("#MM-") && edge[2].split_whitespace().eq(["mesa3d-fetch"])
}

pub(crate) fn validate_static_contract(
    root: &Path,
    make_source: &str,
) -> std::result::Result<(), String> {
    let Some(block) = normalized_make_capability_block(
        make_source,
        "MESA3D_GALLIUM_SSE41_SOURCES :=",
        "%build_linklib mmake=mesa3d-linklib-mesa-sse41",
    ) else {
        return Err("Mesa SSE4.1 recipe block is missing; the transpiler capability must be reviewed and updated".to_owned());
    };
    let Some(local_context) = normalized_make_capability_block(
        make_source,
        "include $(SRCDIR)/config/aros.cfg",
        "%common",
    ) else {
        return Err("Mesa SSE4.1 local Make context is missing; the transpiler capability must be reviewed and updated".to_owned());
    };
    let Ok(mesa_config) = fs::read_to_string(root.join("workbench/libs/mesa/mesa.cfg")) else {
        return Err("Mesa SSE4.1 cannot read workbench/libs/mesa/mesa.cfg; the transpiler capability must be reviewed and updated".to_owned());
    };
    let Some(config_context) = normalized_make_capability_block(
        &mesa_config,
        "aros_mesadir :=",
        "MESA3DGL_GALLIUMCORE :=",
    ) else {
        return Err("Mesa SSE4.1 configuration context is missing; the transpiler capability must be reviewed and updated".to_owned());
    };
    require_text_fingerprint(
        "workbench/libs/mesa/libmesa/mmakefile.src SSE4.1 block",
        &block,
        fingerprint("mesa-sse41-capability"),
        "Mesa SSE4.1",
    )?;
    require_text_fingerprint(
        "workbench/libs/mesa/libmesa/mmakefile.src local context",
        &local_context,
        fingerprint("mesa-sse41-local-context"),
        "Mesa SSE4.1",
    )?;
    require_text_fingerprint(
        "workbench/libs/mesa/mesa.cfg compile context",
        &config_context,
        fingerprint("mesa-sse41-config-context"),
        "Mesa SSE4.1",
    )?;
    if !fetch_edge_is_supported(make_source) {
        return Err("Mesa SSE4.1 fetch edge differs from the supported shape; the transpiler capability must be reviewed and updated".to_owned());
    }
    require_file_fingerprint(
        root,
        "workbench/libs/mesa/libmesa/mesa-sse41-20.0.8.sources",
        fingerprint("mesa-sse41-manifest"),
        "Mesa SSE4.1",
    )?;
    Ok(())
}

pub(crate) fn validate(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    make_source: &str,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> std::result::Result<(), String> {
    let Some(x86_64) = profile(relative_dir, target)? else {
        return Ok(());
    };
    validate_static_contract(root, make_source)?;

    let matching_targets = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == MMAKE)
        .collect::<Vec<_>>();
    let [sse41] = matching_targets.as_slice() else {
        return Err(format!(
            "requires exactly one {MMAKE} declaration, found {}",
            matching_targets.len()
        ));
    };
    let expected_sources = sources(x86_64);
    let expected_defines = defines(x86_64);
    let expected_options = compile_options(x86_64);
    let target_contract_ok = sse41.target_name == "mesa-sse41"
        && sse41.module_type == ModuleType::LinkLib
        && !sse41.genmodule_only
        && sse41.empty_archive != x86_64
        && sse41.source_files == expected_sources
        && sse41.cxx_source_files.is_empty()
        && sse41.objc_source_files.is_empty()
        && sse41.asm_source_files.is_empty()
        && sse41.use_libs.is_empty()
        && sse41.dependencies.is_empty()
        && sse41.dir_path == relative_dir
        && sse41.target_dir.is_none()
        && !sse41.variant_32bit
        && sse41.link_libs.is_empty()
        && sse41.declared_mod_type.is_none()
        && sse41.mod_suffix.is_none()
        && sse41.linklib_name.is_none()
        && sse41.genmodule_linklibs.is_none()
        && sse41.linklib_output_dir.as_deref() == Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8")
        && !sse41.canonical_linklib_output
        && !sse41.canonical_linklib_eligible
        && sse41.compiler_flags.is_empty()
        && sse41.arch_modules.is_empty()
        && sse41.arch_includes.is_empty()
        && sse41.undefines.is_empty()
        && sse41.link_options.is_empty()
        && sse41.arch_sources.is_empty()
        && sse41.arch_defines.is_empty()
        && sse41.arch_compile_options.is_empty()
        && sse41
            .defines
            .iter()
            .map(String::as_str)
            .eq(expected_defines.iter().map(String::as_str))
        && sse41
            .include_dirs
            .iter()
            .map(String::as_str)
            .eq(INCLUDES.iter().copied())
        && sse41
            .compile_options
            .iter()
            .map(String::as_str)
            .eq(expected_options.iter().map(String::as_str));
    if !target_contract_ok {
        return Err("Mesa SSE4.1 source, empty-archive, flag, include or output contract differs from the audited capability".to_owned());
    }

    let matching_fetches = fetches
        .iter()
        .filter(|fetch| fetch.name == "mesa3d-fetch")
        .collect::<Vec<_>>();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake=mesa3d-fetch declaration, found {}",
            matching_fetches.len()
        ));
    };
    let origin_words = fetch.origins.split_whitespace().collect::<Vec<_>>();
    if fetch.archive != "mesa-20.0.8"
        || fetch.suffixes != "tar.xz tar.gz"
        || origin_words
            != [
                "cache://",
                "https://archive.mesa3d.org/",
                "https://archive.mesa3d.org/older-versions/20.x",
            ]
        || fetch.location != "${AROS_PORTS_SOURCE_DIR}"
        || fetch.destination != "${AROS_PORTS_DIR}/mesa"
        || !fetch.base.is_empty()
        || fetch.patch_origins != "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
        || fetch.patches != "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1"
        || fetch.dir != "workbench/libs/mesa"
    {
        return Err(
            "central Mesa 20.0.8 fetch declaration differs from the audited SSE4.1 capability"
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{
        collect_mmakefile_fetches_with_context, parse_mmakefile_with_dirs_and_context_and_fetches,
    };
    use crate::testing::{dirs, root, target_context, TempTree};
    use aros_common::read_source;
    use std::path::Path;

    #[test]
    fn mesa_sse41_capability_rejects_recipe_target_fetch_and_profile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/libs/mesa/libmesa");
        let profile = target_context("x86_64", "pc", "");
        let central_fetches = collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join(relative_dir).join("mmakefile.src"),
            &root,
            &dirs(),
            &profile,
            &central_fetches,
        )
        .unwrap();
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let validate = |content: &str,
                        targets: &[crate::ast::TargetDefinition],
                        fetches: &[crate::fetch::FetchDecl],
                        profile: &TargetContext| {
            validate(
                &root,
                relative_dir,
                Some(profile),
                content,
                targets,
                fetches,
            )
            .unwrap_err()
        };

        let changed_content = content.replace(
            "TARGET_ISA_CFLAGS += -msse4.1",
            "TARGET_ISA_CFLAGS += -msse4.2",
        );
        assert!(validate(
            &changed_content,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("unsupported upstream recipe drift"));

        let changed_local_context = content.replace(
            "-iquote $(top_builddir)/$(CUR_MESADIR)/main",
            "-iquote $(top_builddir)/$(CUR_MESADIR)/unreviewed-main",
        );
        assert!(validate(
            &changed_local_context,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("unsupported upstream recipe drift"));

        let changed_manifest_include = content.replace(
            "include $(SRCDIR)/$(CURDIR)/mesa-sse41-20.0.8.sources",
            "include $(SRCDIR)/$(CURDIR)/mesa-sse41-unreviewed.sources",
        );
        assert!(validate(
            &changed_manifest_include,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("unsupported upstream recipe drift"));

        let changed_intervening_context = content.replace(
            "MESA3D_GALLIUM_SSE41_SOURCES :=",
            "USER_CFLAGS += -funreviewed\n\nMESA3D_GALLIUM_SSE41_SOURCES :=",
        );
        assert!(validate(
            &changed_intervening_context,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("unsupported upstream recipe drift"));

        let disabled_fetch_edge = content.replace(
            "#MM mesa3d-linklib-mesa-sse41 : mesa3d-fetch",
            "#MM- mesa3d-linklib-mesa-sse41 : mesa3d-fetch",
        );
        assert!(validate(
            &disabled_fetch_edge,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("fetch edge differs from the supported shape"));

        let mut changed_targets = parsed.targets.clone();
        let sse41 = changed_targets
            .iter_mut()
            .find(|target| target.mmake_name == MMAKE)
            .unwrap();
        sse41.source_files.pop();
        assert!(
            validate(&content, &changed_targets, &central_fetches, &profile)
                .contains("source, empty-archive, flag, include or output contract")
        );

        let mut changed_fetches = central_fetches.clone();
        changed_fetches[0].patches = "mesa-20.0.8-unreviewed.diff:mesa-20.0.8:-p1".to_owned();
        assert!(
            validate(&content, &parsed.targets, &changed_fetches, &profile)
                .contains("fetch declaration differs")
        );

        let mut changed_profile = profile;
        changed_profile.toolchain = Some("gnu".to_owned());
        assert!(validate(
            &content,
            &parsed.targets,
            &central_fetches,
            &changed_profile
        )
        .contains("does not support target profile"));

        let fixture_tree = TempTree::new();
        for relative in [
            "workbench/libs/mesa/mesa.cfg",
            "workbench/libs/mesa/mesa-20.0.8-aros.diff",
            "workbench/libs/mesa/libmesa/mesa-sse41-20.0.8.sources",
        ] {
            let destination = fixture_tree.0.join(relative);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(root.join(relative), destination).unwrap();
        }
        assert!(validate_static_contract(&fixture_tree.0, &content).is_ok());
        let config_path = fixture_tree.0.join("workbench/libs/mesa/mesa.cfg");
        let changed_config = read_source(&config_path).unwrap().replace(
            "aros_mesadir := workbench/libs/mesa",
            "aros_mesadir := workbench/libs/mesa-unreviewed",
        );
        fs::write(config_path, changed_config).unwrap();
        assert!(validate_static_contract(&fixture_tree.0, &content).is_err());
    }
}
