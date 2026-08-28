//! The two hand-written Python generator families: glapi and mesautil.
//!
//! Mesa generates C from XML and CSV with Python scripts, and the legacy
//! recipes that do it are shell text. They are not treated as a command
//! language: the complete semantic block of each recipe is fingerprinted,
//! while source profiles and fetch declarations are checked semantically. On
//! drift the transpiler fails with an update-required diagnostic.

use super::super::{normalized_make_capability_block, require_text_fingerprint};
use crate::ast::{ModuleType, PythonGeneratorJob, PythonOutputsDecl, TargetDefinition};
use crate::fetch::FetchDecl;
use crate::fingerprints::fingerprint;
use crate::parser::TargetContext;
use std::path::Path;

/// Admits the one hand-written Python generator family needed by Mesa 20.0.8
/// libglapi.
///
/// The legacy recipes are not treated as a general command language.  Their
/// complete semantic block is fingerprinted because it expands into fixed
/// Python jobs. The selected source/flag profile and central fetch declaration
/// are checked semantically.
pub(crate) fn parse_glapi(
    relative_dir: &Path,
    target: Option<&TargetContext>,
    make_source: &str,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> std::result::Result<Option<PythonOutputsDecl>, String> {
    const GLAPI_DIR: &str = "workbench/libs/mesa/libglapi";
    const GLAPI_MMAKE: &str = "mesa3d-linklib-glapi";
    const GLAPI_FETCH: &str = "mesa3d-fetch";
    const SOURCE_ROOT: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8";
    const BUILD_ROOT: &str = "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8";
    const XML: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi/glapi/gen/gl_and_es_API.xml";

    if relative_dir != Path::new(GLAPI_DIR) {
        return Ok(None);
    }

    let Some(profile) = target else {
        return Err(
            "Mesa glapi generator capability requires a concrete target profile".to_owned(),
        );
    };
    let profile_key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    let x86_64 = match profile_key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => true,
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard"))
        | (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some(""))
        | (Some("riscv64"), Some("opensbi"), Some("llvm"), Some(""), Some("1"), Some("")) => false,
        _ => {
            return Err(format!(
                "Mesa glapi generator capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
                profile.cpu.as_deref().unwrap_or("<unset>"),
                profile.platform.as_deref().unwrap_or("<unset>"),
                profile.toolchain.as_deref().unwrap_or("<unset>"),
                profile.cpu32.as_deref().unwrap_or("<unset>"),
                profile.use_mmu.as_deref().unwrap_or("<unset>"),
                profile.float_abi.as_deref().unwrap_or("<unset>")
            ));
        }
    };

    let matching_targets = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == GLAPI_MMAKE)
        .collect::<Vec<_>>();
    let [glapi] = matching_targets.as_slice() else {
        return Err(format!(
            "requires exactly one {GLAPI_MMAKE} declaration, found {}",
            matching_targets.len()
        ));
    };
    let expected_sources = [
        "glapi/glapi_dispatch",
        "glapi/glapi_entrypoint",
        "glapi/glapi_getproc",
        "glapi/glapi_nop",
        "glapi/glapi",
        "u_current",
        "u_execmem",
    ]
    .into_iter()
    .map(|source| format!("{SOURCE_ROOT}/src/mapi/{source}"))
    .collect::<Vec<_>>();
    let expected_asm = if x86_64 {
        vec![format!("{BUILD_ROOT}/src/mapi/glapi/glapi_x86-64")]
    } else {
        Vec::new()
    };
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
    if x86_64 {
        expected_defines.extend(["USE_X86_64_ASM", "USE_SSE41"]);
    }
    expected_defines.extend(["MAPI_MODE_GLAPI", "MAPI_MODE_UTIL"]);
    let mut expected_includes = vec![
        "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc",
        "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/include/GL",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src",
        "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/mapi",
        "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi",
        "${CMAKE_BINARY_DIR}/gen/workbench/libs/mesa/20.0.8/src/mapi/glapi",
        "${CMAKE_SOURCE_DIR}/workbench/libs/mesa",
    ];
    if x86_64 {
        expected_includes.push("${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa");
    }
    let target_contract_ok = glapi.target_name == "glapi"
        && glapi.module_type == ModuleType::LinkLib
        && glapi.source_files == expected_sources
        && glapi.cxx_source_files.is_empty()
        && glapi.objc_source_files.is_empty()
        && glapi.asm_source_files == expected_asm
        && glapi.linklib_output_dir.as_deref() == Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8")
        && !glapi.canonical_linklib_output
        && glapi
            .defines
            .iter()
            .map(String::as_str)
            .eq(expected_defines)
        && glapi
            .include_dirs
            .iter()
            .map(String::as_str)
            .eq(expected_includes)
        && glapi.compile_options == ["-std=gnu11", "-fno-strict-aliasing"];
    if !target_contract_ok {
        return Err("Mesa glapi source, flag, include or output contract differs from the audited capability".to_owned());
    }

    let generator_block = normalized_make_capability_block(
        make_source,
        "$(top_builddir)/$(CUR_MESADIR)/glapi/glapitemp.h:",
        "%build_linklib",
    )
    .ok_or_else(|| "Mesa glapi generator recipe block is missing".to_owned())?;
    require_text_fingerprint(
        "workbench/libs/mesa/libglapi/mmakefile.src generator block",
        &generator_block,
        fingerprint("glapi-generator-capability")?,
        "Mesa glapi generator",
    )?;

    let matching_fetches = fetches
        .iter()
        .filter(|fetch| fetch.name == GLAPI_FETCH)
        .collect::<Vec<_>>();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake={GLAPI_FETCH} declaration, found {}",
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
            "central Mesa 20.0.8 fetch declaration differs from the audited glapi capability"
                .to_owned(),
        );
    }

    let mut jobs = vec![
        PythonGeneratorJob {
            script: "src/mapi/glapi/gen/gl_apitemp.py".to_owned(),
            output: "src/mapi/glapi/glapitemp.h".to_owned(),
            arguments: vec!["-f".to_owned(), XML.to_owned()],
        },
        PythonGeneratorJob {
            script: "src/mapi/glapi/gen/gl_table.py".to_owned(),
            output: "src/mapi/glapi/glapitable.h".to_owned(),
            arguments: vec!["-f".to_owned(), XML.to_owned()],
        },
        PythonGeneratorJob {
            script: "src/mapi/glapi/gen/gl_procs.py".to_owned(),
            output: "src/mapi/glapi/glprocs.h".to_owned(),
            arguments: vec!["-c".to_owned(), "-f".to_owned(), XML.to_owned()],
        },
    ];
    if x86_64 {
        jobs.push(PythonGeneratorJob {
            script: "src/mapi/glapi/gen/gl_x86-64_asm.py".to_owned(),
            output: "src/mapi/glapi/glapi_x86-64.s".to_owned(),
            arguments: vec!["-f".to_owned(), XML.to_owned()],
        });
    }

    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-glapi-generate".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: GLAPI_FETCH.to_owned(),
        source_inputs: vec!["src/mapi/glapi/gen/gl_and_es_API.xml".to_owned()],
        jobs,
        driver_script: None,
        python_packages: Vec::new(),
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff".to_owned(),
        ],
        consumers: vec![GLAPI_MMAKE.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

/// Admits the two Mesa 20.0.8 utility archives and their two live generated C
/// sources. The dead `u_format_pack.h` rule is intentionally outside the
/// supported block: it is absent from `MESA_UTIL_GENERATED_FILES` and is not a
/// prerequisite of either archive.
pub(crate) fn parse_mesautil(
    relative_dir: &Path,
    target: Option<&TargetContext>,
    make_source: &str,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> std::result::Result<Option<PythonOutputsDecl>, String> {
    const MESAUTIL_DIR: &str = "workbench/libs/mesa/libmesautil";
    const MESAUTIL_MMAKE: &str = "mesa3d-linklib-mesautil";
    const MESADEVUTIL_MMAKE: &str = "mesa3d-linklib-mesadevutil";
    const MESA_FETCH: &str = "mesa3d-fetch";
    const SOURCE_ROOT: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8";
    const BUILD_ROOT: &str = "${AROS_BUILD_DIR}/gen/workbench/libs/mesa/20.0.8";
    const CSV: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/util/format/u_format.csv";
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

    if relative_dir != Path::new(MESAUTIL_DIR) {
        return Ok(None);
    }

    let Some(profile) = target else {
        return Err(
            "Mesa utility generator capability requires a concrete target profile".to_owned(),
        );
    };
    let profile_key = (
        profile.cpu.as_deref(),
        profile.platform.as_deref(),
        profile.toolchain.as_deref(),
        profile.cpu32.as_deref(),
        profile.use_mmu.as_deref(),
        profile.float_abi.as_deref(),
    );
    let x86_64 = match profile_key {
        (Some("x86_64"), Some("pc"), Some("llvm"), Some("i386"), Some("1"), Some("")) => true,
        (Some("arm"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some("hard"))
        | (Some("aarch64"), Some("raspi"), Some("llvm"), Some(""), Some("1"), Some(""))
        | (Some("riscv64"), Some("opensbi"), Some("llvm"), Some(""), Some("1"), Some("")) => false,
        _ => {
            return Err(format!(
                "Mesa utility generator capability does not support target profile cpu={} platform={} toolchain={} cpu32={} use_mmu={} float_abi={}",
                profile.cpu.as_deref().unwrap_or("<unset>"),
                profile.platform.as_deref().unwrap_or("<unset>"),
                profile.toolchain.as_deref().unwrap_or("<unset>"),
                profile.cpu32.as_deref().unwrap_or("<unset>"),
                profile.use_mmu.as_deref().unwrap_or("<unset>"),
                profile.float_abi.as_deref().unwrap_or("<unset>")
            ));
        }
    };

    let matching_mesautil = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == MESAUTIL_MMAKE)
        .collect::<Vec<_>>();
    let [mesautil] = matching_mesautil.as_slice() else {
        return Err(format!(
            "requires exactly one {MESAUTIL_MMAKE} declaration, found {}",
            matching_mesautil.len()
        ));
    };
    let matching_mesadevutil = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == MESADEVUTIL_MMAKE)
        .collect::<Vec<_>>();
    let [mesadevutil] = matching_mesadevutil.as_slice() else {
        return Err(format!(
            "requires exactly one {MESADEVUTIL_MMAKE} declaration, found {}",
            matching_mesadevutil.len()
        ));
    };

    let mut expected_sources = STATIC_SOURCES
        .iter()
        .map(|source| format!("{SOURCE_ROOT}/src/util/{source}"))
        .collect::<Vec<_>>();
    expected_sources.extend([
        format!("{BUILD_ROOT}/src/util/format_srgb"),
        format!("{BUILD_ROOT}/src/util/format/u_format_table"),
    ]);
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
    if x86_64 {
        expected_defines.extend(["USE_X86_64_ASM", "USE_SSE41"]);
    }
    expected_defines.extend(["MAPI_MODE_GLAPI", "MAPI_MODE_UTIL"]);
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
    let target_contract_ok = |declaration: &TargetDefinition, name: &str, embedded_device: bool| {
        let mut defines = expected_defines.clone();
        if embedded_device {
            defines.push("EMBEDDED_DEVICE");
        }
        declaration.target_name == name
            && declaration.module_type == ModuleType::LinkLib
            && declaration.source_files == expected_sources
            && declaration.cxx_source_files.is_empty()
            && declaration.objc_source_files.is_empty()
            && declaration.asm_source_files.is_empty()
            && declaration.linklib_output_dir.as_deref()
                == Some("${AROS_BUILD_DIR}/gen/lib/mesa20.0.8")
            && !declaration.canonical_linklib_output
            && declaration.defines.iter().map(String::as_str).eq(defines)
            && declaration
                .include_dirs
                .iter()
                .map(String::as_str)
                .eq(expected_includes)
            && declaration.compile_options == ["-std=gnu11", "-fno-strict-aliasing"]
    };
    if !target_contract_ok(mesautil, "mesautil", false)
        || !target_contract_ok(mesadevutil, "mesadevutil", true)
    {
        return Err(
            "Mesa utility source, flag, include or output contract differs from the audited capability"
                .to_owned(),
        );
    }

    let generator_block = normalized_make_capability_block(
        make_source,
        "$(top_builddir)/$(CUR_MESADIR)/%.c:",
        "%common",
    )
    .ok_or_else(|| "Mesa utility generator recipe block is missing".to_owned())?;
    require_text_fingerprint(
        "workbench/libs/mesa/libmesautil/mmakefile.src generator block",
        &generator_block,
        fingerprint("mesautil-generator-capability")?,
        "Mesa utility generator",
    )?;

    let matching_fetches = fetches
        .iter()
        .filter(|fetch| fetch.name == MESA_FETCH)
        .collect::<Vec<_>>();
    let [fetch] = matching_fetches.as_slice() else {
        return Err(format!(
            "requires exactly one %fetch mmake={MESA_FETCH} declaration, found {}",
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
            "central Mesa 20.0.8 fetch declaration differs from the audited utility capability"
                .to_owned(),
        );
    }

    Ok(Some(PythonOutputsDecl {
        owner: "mesa3d-linklib-mesautil-generated".to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: MESA_FETCH.to_owned(),
        source_inputs: vec![
            "src/util/format/u_format.csv".to_owned(),
            "src/util/format/u_format_pack.py".to_owned(),
            "src/util/format/u_format_parse.py".to_owned(),
        ],
        jobs: vec![
            PythonGeneratorJob {
                script: "src/util/format_srgb.py".to_owned(),
                output: "src/util/format_srgb.c".to_owned(),
                arguments: vec![CSV.to_owned()],
            },
            PythonGeneratorJob {
                script: "src/util/format/u_format_table.py".to_owned(),
                output: "src/util/format/u_format_table.c".to_owned(),
                arguments: vec![CSV.to_owned()],
            },
        ],
        driver_script: None,
        python_packages: Vec::new(),
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff".to_owned(),
        ],
        consumers: vec![MESAUTIL_MMAKE.to_owned(), MESADEVUTIL_MMAKE.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{parse_glapi, parse_mesautil};
    use crate::parser::TargetContext;
    use crate::parser::{
        collect_mmakefile_fetches_with_context, parse_mmakefile_with_dirs_and_context_and_fetches,
    };
    use crate::testing::{dirs, root, target_context};
    use aros_common::read_source;
    use std::path::Path;

    #[test]
    fn glapi_python_capability_rejects_recipe_source_fetch_and_profile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/libs/mesa/libglapi");
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
        let parse = |content: &str,
                     targets: &[crate::ast::TargetDefinition],
                     fetches: &[crate::fetch::FetchDecl],
                     profile: &TargetContext| {
            parse_glapi(relative_dir, Some(profile), content, targets, fetches).unwrap_err()
        };

        let changed_content = content.replace("gl_table.py", "unreviewed_table.py");
        assert!(parse(
            &changed_content,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("unsupported upstream recipe drift"));

        let mut changed_targets = parsed.targets.clone();
        let glapi = changed_targets
            .iter_mut()
            .find(|target| target.mmake_name == "mesa3d-linklib-glapi")
            .unwrap();
        glapi.source_files.pop();
        assert!(
            parse(&content, &changed_targets, &central_fetches, &profile)
                .contains("source, flag, include or output contract")
        );

        let mut changed_fetches = central_fetches.clone();
        changed_fetches[0].patches = "mesa-20.0.8-unreviewed.diff:mesa-20.0.8:-p1".to_owned();
        assert!(parse(&content, &parsed.targets, &changed_fetches, &profile)
            .contains("fetch declaration differs"));
        assert!(parse(&content, &parsed.targets, &[], &profile).contains("exactly one"));

        let mut changed_profile = profile;
        changed_profile.toolchain = Some("gnu".to_owned());
        assert!(parse(
            &content,
            &parsed.targets,
            &central_fetches,
            &changed_profile
        )
        .contains("does not support target profile"));
    }

    #[test]
    fn mesautil_python_capability_rejects_recipe_source_fetch_and_profile_drift() {
        let root = root();
        let relative_dir = Path::new("workbench/libs/mesa/libmesautil");
        let profile = target_context("x86_64", "pc", "");
        let mut central_fetches = collect_mmakefile_fetches_with_context(
            &root.join("workbench/libs/mesa/mmakefile.src"),
            &root,
            &profile,
        )
        .unwrap();
        central_fetches.extend(
            collect_mmakefile_fetches_with_context(
                &root.join("workbench/libs/z/mmakefile.src"),
                &root,
                &profile,
            )
            .unwrap(),
        );
        let parsed = parse_mmakefile_with_dirs_and_context_and_fetches(
            &root.join(relative_dir).join("mmakefile.src"),
            &root,
            &dirs(),
            &profile,
            &central_fetches,
        )
        .unwrap();
        let content = read_source(&root.join(relative_dir).join("mmakefile.src")).unwrap();
        let parse = |content: &str,
                     targets: &[crate::ast::TargetDefinition],
                     fetches: &[crate::fetch::FetchDecl],
                     profile: &TargetContext| {
            parse_mesautil(relative_dir, Some(profile), content, targets, fetches).unwrap_err()
        };

        let changed_content =
            content.replace("$(Q)$(PYTHON)  $^ > $@", "$(Q)python-unreviewed $^ > $@");
        assert!(parse(
            &changed_content,
            &parsed.targets,
            &central_fetches,
            &profile
        )
        .contains("unsupported upstream recipe drift"));

        let mut changed_targets = parsed.targets.clone();
        let mesautil = changed_targets
            .iter_mut()
            .find(|target| target.mmake_name == "mesa3d-linklib-mesautil")
            .unwrap();
        mesautil.source_files.pop();
        assert!(
            parse(&content, &changed_targets, &central_fetches, &profile)
                .contains("source, flag, include or output contract")
        );

        let mut changed_targets = parsed.targets.clone();
        let mesadevutil = changed_targets
            .iter_mut()
            .find(|target| target.mmake_name == "mesa3d-linklib-mesadevutil")
            .unwrap();
        mesadevutil
            .defines
            .retain(|define| define != "EMBEDDED_DEVICE");
        assert!(
            parse(&content, &changed_targets, &central_fetches, &profile)
                .contains("source, flag, include or output contract")
        );

        let mut changed_fetches = central_fetches.clone();
        changed_fetches[0].patches = "mesa-20.0.8-unreviewed.diff:mesa-20.0.8:-p1".to_owned();
        assert!(parse(&content, &parsed.targets, &changed_fetches, &profile)
            .contains("fetch declaration differs"));
        assert!(parse(&content, &parsed.targets, &[], &profile).contains("exactly one"));

        let mut changed_profile = profile;
        changed_profile.toolchain = Some("gnu".to_owned());
        assert!(parse(
            &content,
            &parsed.targets,
            &central_fetches,
            &changed_profile
        )
        .contains("does not support target profile"));
    }
}
