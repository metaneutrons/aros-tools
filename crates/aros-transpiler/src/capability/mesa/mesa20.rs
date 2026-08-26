//! The remaining Mesa 20.0.8 generator groups, and the fetches they need.
//!
//! Everything the glapi and mesautil lanes do not cover: the compiler, mesa,
//! galliumaux and vc4 job groups, the Python packages the driver imports, and
//! the three fetch declarations (Mesa itself, Mako, MarkupSafe) whose exactness
//! is a precondition for emitting any of it.
//!
//! The job groups are lists rather than logic on purpose. They were derived
//! once from the recipes. The opaque recipe blocks and their source inventories
//! are fingerprinted, while the repository driver and patches are direct build
//! dependencies. Recipe drift therefore requests a transpiler update without
//! turning ordinary file changes into pins.

use super::super::{
    normalized_make_capability_block, require_file_fingerprint, require_text_fingerprint,
};
use super::{
    compile_contract, current_profile, remaining_linklib_sources, BUILD_ROOT, PRIVATE_LIBDIR,
    SOURCE_ROOT,
};
use crate::ast::{
    ModuleType, PythonGeneratorJob, PythonOutputsDecl, PythonPackageDecl, TargetDefinition,
};
use crate::fetch::FetchDecl;
use crate::fingerprints::fingerprint;
use crate::parser::TargetContext;
use std::path::Path;

pub(crate) const DRIVER: &str = "${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa20_generate.py";

pub(crate) fn generator_job(script: &str, output: &str, arguments: &[&str]) -> PythonGeneratorJob {
    PythonGeneratorJob {
        script: script.to_owned(),
        output: output.to_owned(),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
    }
}

pub(crate) fn python_packages() -> Vec<PythonPackageDecl> {
    vec![
        PythonPackageDecl {
            fetch_target: "mesa3d-mako-fetch".to_owned(),
            source_root: "${AROS_PORTS_DIR}/mesa-python/mako-1.3.10".to_owned(),
            python_path: ".".to_owned(),
        },
        PythonPackageDecl {
            fetch_target: "mesa3d-markupsafe-fetch".to_owned(),
            source_root: "${AROS_PORTS_DIR}/mesa-python/markupsafe-3.0.2".to_owned(),
            python_path: "src".to_owned(),
        },
    ]
}

pub(crate) fn fetch_is_exact(fetch: &FetchDecl, name: &str) -> bool {
    match name {
        "mesa3d-fetch" => {
            fetch.archive == "mesa-20.0.8"
                && fetch.suffixes == "tar.xz tar.gz"
                && fetch.origins.split_whitespace().eq([
                    "cache://",
                    "https://archive.mesa3d.org/",
                    "https://archive.mesa3d.org/older-versions/20.x",
                ])
                && fetch.location == "${AROS_PORTS_SOURCE_DIR}"
                && fetch.destination == "${AROS_PORTS_DIR}/mesa"
                && fetch.base.is_empty()
                && fetch.patch_origins == "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
                && fetch.patches == "mesa-20.0.8-aros.diff:mesa-20.0.8:-p1"
                && fetch.dir == "workbench/libs/mesa"
        }
        "mesa3d-mako-fetch" => {
            fetch.archive == "mako-1.3.10"
                && fetch.suffixes == "tar.gz"
                && fetch.origins
                    == "https://files.pythonhosted.org/packages/9e/38/bd5b78a920a64d708fe6bc8e0a2c075e1389d53bef8413725c63ba041535"
                && fetch.location == "${AROS_PORTS_SOURCE_DIR}"
                && fetch.destination == "${AROS_PORTS_DIR}/mesa-python"
                && fetch.base.is_empty()
                && fetch.patch_origins == "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
                && fetch.patches == "::"
                && fetch.dir == "workbench/libs/mesa"
        }
        "mesa3d-markupsafe-fetch" => {
            fetch.archive == "markupsafe-3.0.2"
                && fetch.suffixes == "tar.gz"
                && fetch.origins
                    == "https://files.pythonhosted.org/packages/b2/97/5d42485e71dfc078108a86d6de8fa46db44a1a9295e89c5d6d4a06e23a62"
                && fetch.location == "${AROS_PORTS_SOURCE_DIR}"
                && fetch.destination == "${AROS_PORTS_DIR}/mesa-python"
                && fetch.base.is_empty()
                && fetch.patch_origins == "${CMAKE_SOURCE_DIR}/workbench/libs/mesa"
                && fetch.patches == "::"
                && fetch.dir == "workbench/libs/mesa"
        }
        _ => false,
    }
}

pub(crate) fn require_fetches(fetches: &[FetchDecl]) -> std::result::Result<(), String> {
    for name in [
        "mesa3d-fetch",
        "mesa3d-mako-fetch",
        "mesa3d-markupsafe-fetch",
    ] {
        let matching = fetches
            .iter()
            .filter(|fetch| fetch.name == name)
            .collect::<Vec<_>>();
        let [fetch] = matching.as_slice() else {
            return Err(format!(
                "requires exactly one %fetch mmake={name} declaration, found {}",
                matching.len()
            ));
        };
        if !fetch_is_exact(fetch, name) {
            return Err(format!(
                "%fetch mmake={name} differs from the audited Mesa 20.0.8 generator capability"
            ));
        }
    }
    Ok(())
}

pub(crate) fn target_contract_is_exact(
    root: &Path,
    relative_dir: &Path,
    mmake: &str,
    target: Option<&TargetContext>,
    targets: &[TargetDefinition],
) -> std::result::Result<(), String> {
    let expected_sources = remaining_linklib_sources(root, relative_dir, mmake, target)?
        .ok_or_else(|| format!("missing source capability for {mmake}"))?;
    let expected_flags = compile_contract(relative_dir, mmake, target)?
        .ok_or_else(|| format!("missing compile capability for {mmake}"))?;
    let matching = targets
        .iter()
        .filter(|candidate| candidate.mmake_name == mmake)
        .collect::<Vec<_>>();
    let [declaration] = matching.as_slice() else {
        return Err(format!(
            "requires exactly one {mmake} declaration, found {}",
            matching.len()
        ));
    };
    let target_name = match mmake {
        "mesa3d-linklib-compiler" => "compiler",
        "mesa3d-linklib-galliumauxiliary" => "galliumauxiliary",
        "mesa3d-linklib-mesa" => "mesa",
        "linklibs-gallium_vc4" => "gallium_vc4",
        _ => return Err(format!("unsupported Mesa target contract {mmake}")),
    };
    let exact = declaration.target_name == target_name
        && declaration.module_type == ModuleType::LinkLib
        && !declaration.genmodule_only
        && !declaration.empty_archive
        && declaration.source_files == expected_sources.c
        && declaration.cxx_source_files == expected_sources.cxx
        && declaration.objc_source_files.is_empty()
        && declaration.asm_source_files == expected_sources.asm
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
        && declaration.linklib_output_dir.as_deref() == Some(PRIVATE_LIBDIR)
        && !declaration.canonical_linklib_output
        && !declaration.canonical_linklib_eligible
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
        return Err(format!(
            "{mmake} source, language, flag, include or private-output contract differs from the audited capability"
        ));
    }
    Ok(())
}

pub(crate) fn compiler_jobs() -> (Vec<String>, Vec<PythonGeneratorJob>) {
    const NIR: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/nir";
    const GLSL: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/glsl";
    const SPIRV: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/compiler/spirv";
    let inputs = [
        "src/compiler/nir/nir_opcodes.py",
        "src/compiler/nir/nir_intrinsics.py",
        "src/compiler/nir/nir_algebraic.py",
        "src/compiler/nir/nir_constant_expressions.h",
        "src/compiler/glsl/float64.glsl",
        "src/compiler/spirv/spirv.core.grammar.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let jobs = vec![
        generator_job(
            "src/compiler/nir/nir_builder_opcodes_h.py",
            "src/compiler/nir/nir_builder_opcodes.h",
            &["python-stdout"],
        ),
        generator_job(
            "src/compiler/nir/nir_constant_expressions.py",
            "src/compiler/nir/nir_constant_expressions.c",
            &["python-stdout"],
        ),
        generator_job(
            "src/compiler/nir/nir_intrinsics_c.py",
            "src/compiler/nir/nir_intrinsics.c",
            &["python-outdir", "--outdir", "@OUTDIR@"],
        ),
        generator_job(
            "src/compiler/nir/nir_intrinsics_h.py",
            "src/compiler/nir/nir_intrinsics.h",
            &["python-outdir", "--outdir", "@OUTDIR@"],
        ),
        generator_job(
            "src/compiler/nir/nir_opcodes_c.py",
            "src/compiler/nir/nir_opcodes.c",
            &["python-stdout"],
        ),
        generator_job(
            "src/compiler/nir/nir_opcodes_h.py",
            "src/compiler/nir/nir_opcodes.h",
            &["python-stdout"],
        ),
        generator_job(
            "src/compiler/nir/nir_opt_algebraic.py",
            "src/compiler/nir/nir_opt_algebraic.c",
            &["python-stdout"],
        ),
        generator_job(
            "src/compiler/glsl/ir_expression_operation.py",
            "src/compiler/glsl/ir_expression_operation.h",
            &["python-stdout", "enum"],
        ),
        generator_job(
            "src/compiler/glsl/ir_expression_operation.py",
            "src/compiler/glsl/ir_expression_operation_constant.h",
            &["python-stdout", "constant"],
        ),
        generator_job(
            "src/compiler/glsl/ir_expression_operation.py",
            "src/compiler/glsl/ir_expression_operation_strings.h",
            &["python-stdout", "strings"],
        ),
        generator_job(
            "src/compiler/glsl/xxd.py",
            "src/compiler/glsl/float64_glsl.h",
            &[
                "python-output",
                &format!("{GLSL}/float64.glsl"),
                "@OUTPUT@",
                "-n",
                "float64_source",
            ],
        ),
        generator_job(
            "src/compiler/glsl/glcpp/glcpp-lex.l",
            "src/compiler/glsl/glcpp/glcpp-lex.c",
            &["flex", "--nounistd"],
        ),
        generator_job(
            "src/compiler/glsl/glcpp/glcpp-parse.y",
            "src/compiler/glsl/glcpp/glcpp-parse.c",
            &["bison", "glcpp-parse.c", "glcpp-parse.h", "glcpp_parser_"],
        ),
        generator_job(
            "src/compiler/glsl/glcpp/glcpp-parse.y",
            "src/compiler/glsl/glcpp/glcpp-parse.h",
            &["bison", "glcpp-parse.c", "glcpp-parse.h", "glcpp_parser_"],
        ),
        generator_job(
            "src/compiler/glsl/glsl_lexer.ll",
            "src/compiler/glsl/glsl_lexer.cpp",
            &["flex", "--nounistd"],
        ),
        generator_job(
            "src/compiler/glsl/glsl_parser.yy",
            "src/compiler/glsl/glsl_parser.cpp",
            &["bison", "glsl_parser.cpp", "glsl_parser.h", "_mesa_glsl_"],
        ),
        generator_job(
            "src/compiler/glsl/glsl_parser.yy",
            "src/compiler/glsl/glsl_parser.h",
            &["bison", "glsl_parser.cpp", "glsl_parser.h", "_mesa_glsl_"],
        ),
        generator_job(
            "src/compiler/spirv/spirv_info_c.py",
            "src/compiler/spirv/spirv_info.c",
            &[
                "python-output",
                &format!("{SPIRV}/spirv.core.grammar.json"),
                "@OUTPUT@",
            ],
        ),
        generator_job(
            "src/compiler/spirv/vtn_gather_types_c.py",
            "src/compiler/spirv/vtn_gather_types.c",
            &[
                "python-output",
                &format!("{SPIRV}/spirv.core.grammar.json"),
                "@OUTPUT@",
            ],
        ),
    ];
    let _ = NIR;
    (inputs, jobs)
}

pub(crate) fn galliumaux_jobs() -> (Vec<String>, Vec<PythonGeneratorJob>) {
    (
        Vec::new(),
        vec![
            generator_job(
                "src/gallium/auxiliary/indices/u_indices_gen.py",
                "src/gallium/auxiliary/indices/u_indices_gen.c",
                &["python-stdout"],
            ),
            generator_job(
                "src/gallium/auxiliary/indices/u_unfilled_gen.py",
                "src/gallium/auxiliary/indices/u_unfilled_gen.c",
                &["python-stdout"],
            ),
        ],
    )
}

pub(crate) fn mesa_jobs() -> (Vec<String>, Vec<PythonGeneratorJob>) {
    const GLAPI: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi/glapi/gen";
    const MAIN: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/main";
    const XML: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mapi/glapi/gen/gl_and_es_API.xml";
    let inputs = [
        "src/mapi/glapi/gen/gl_and_es_API.xml",
        "src/mapi/glapi/gen/gl_XML.py",
        "src/mapi/glapi/gen/glX_XML.py",
        "src/mapi/glapi/gen/license.py",
        "src/mapi/glapi/gen/static_data.py",
        "src/mesa/main/get_hash_params.py",
        "src/mesa/main/formats.csv",
        "src/mesa/main/format_parser.py",
        "VERSION",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let jobs = vec![
        generator_job(
            "src/mapi/glapi/gen/gl_table.py",
            "src/mesa/main/dispatch.h",
            &["python-stdout", "-m", "remap_table", "-f", XML],
        ),
        generator_job(
            "src/mapi/glapi/gen/remap_helper.py",
            "src/mesa/main/remap_helper.h",
            &["python-stdout", "-f", XML],
        ),
        generator_job(
            "src/mapi/glapi/gen/gl_enums.py",
            "src/mesa/main/enums.c",
            &["python-stdout", "-f", XML],
        ),
        generator_job(
            "src/mapi/glapi/gen/gl_genexec.py",
            "src/mesa/main/api_exec.c",
            &["python-stdout", "-f", XML],
        ),
        generator_job(
            "src/mapi/glapi/gen/gl_marshal_h.py",
            "src/mesa/main/marshal_generated.h",
            &["python-stdout", "-f", XML],
        ),
        generator_job(
            "src/mapi/glapi/gen/gl_marshal.py",
            "src/mesa/main/marshal_generated.c",
            &["python-stdout", "-f", XML],
        ),
        generator_job(
            "src/mesa/main/get_hash_generator.py",
            "src/mesa/main/get_hash.h",
            &["python-stdout", "-f", XML],
        ),
        generator_job(
            "src/mesa/main/format_info.py",
            "src/mesa/main/format_info.h",
            &["python-stdout", &format!("{MAIN}/formats.csv")],
        ),
        generator_job(
            "src/mesa/main/format_fallback.py",
            "src/mesa/main/format_fallback.c",
            &["python-output", &format!("{MAIN}/formats.csv"), "@OUTPUT@"],
        ),
        generator_job(
            "src/mesa/main/format_pack.py",
            "src/mesa/main/format_pack.c",
            &["python-stdout", &format!("{MAIN}/formats.csv")],
        ),
        generator_job(
            "src/mesa/main/format_unpack.py",
            "src/mesa/main/format_unpack.c",
            &["python-stdout", &format!("{MAIN}/formats.csv")],
        ),
        generator_job("VERSION", "src/mesa/main/git_sha1.h", &["mesa-git-sha1"]),
        generator_job(
            "src/mesa/program/program_lexer.l",
            "src/mesa/program/lex.yy.c",
            &["flex", "--nounistd", "--never-interactive"],
        ),
        generator_job(
            "src/mesa/program/program_parse.y",
            "src/mesa/program/program_parse.tab.c",
            &[
                "bison",
                "program_parse.tab.c",
                "program_parse.tab.h",
                "_mesa_program_",
            ],
        ),
        generator_job(
            "src/mesa/program/program_parse.y",
            "src/mesa/program/program_parse.tab.h",
            &[
                "bison",
                "program_parse.tab.c",
                "program_parse.tab.h",
                "_mesa_program_",
            ],
        ),
    ];
    let _ = GLAPI;
    (inputs, jobs)
}

pub(crate) fn vc4_jobs() -> (Vec<String>, Vec<PythonGeneratorJob>) {
    const CLE: &str = "${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/broadcom/cle";
    let inputs = [
        "src/broadcom/cle/v3d_packet_v21.xml",
        "src/broadcom/cle/v3d_packet_v33.xml",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let jobs = [
        ("v3d_packet_v21.xml", "v3d_packet_v21_pack.h", "21"),
        ("v3d_packet_v33.xml", "v3d_packet_v33_pack.h", "33"),
        ("v3d_packet_v33.xml", "v3d_packet_v41_pack.h", "41"),
        ("v3d_packet_v33.xml", "v3d_packet_v42_pack.h", "42"),
    ]
    .into_iter()
    .map(|(xml, output, version)| {
        generator_job(
            "src/broadcom/cle/gen_pack_header.py",
            &format!("src/broadcom/cle/{output}"),
            &["python-stdout", &format!("{CLE}/{xml}"), version],
        )
    })
    .collect();
    (inputs, jobs)
}

pub(crate) fn parse_remaining(
    root: &Path,
    relative_dir: &Path,
    target: Option<&TargetContext>,
    make_source: &str,
    targets: &[TargetDefinition],
    fetches: &[FetchDecl],
) -> std::result::Result<Option<PythonOutputsDecl>, String> {
    if !matches!(
        relative_dir.to_str(),
        Some("workbench/libs/mesa/libcompiler")
            | Some("workbench/libs/mesa/libgalliumaux")
            | Some("workbench/libs/mesa/libmesa")
            | Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium")
    ) {
        return Ok(None);
    }
    let profile = current_profile(target)?;
    let (
        mmake,
        owner,
        recipe_fingerprint,
        manifest,
        manifest_fingerprint,
        source_inputs,
        jobs,
        packages,
    ) = match relative_dir.to_str() {
        Some("workbench/libs/mesa/libcompiler") => {
            let (inputs, jobs) = compiler_jobs();
            (
                "mesa3d-linklib-compiler",
                "mesa3d-linklib-compiler-generated",
                fingerprint("mesa20-compiler-recipe"),
                "workbench/libs/mesa/libcompiler/compiler-20.0.8.sources",
                fingerprint("mesa20-compiler-manifest"),
                inputs,
                jobs,
                python_packages(),
            )
        }
        Some("workbench/libs/mesa/libgalliumaux") => {
            let (inputs, jobs) = galliumaux_jobs();
            (
                "mesa3d-linklib-galliumauxiliary",
                "mesa3d-linklib-galliumauxiliary-generated",
                fingerprint("mesa20-galliumaux-recipe"),
                "workbench/libs/mesa/libgalliumaux/galliumaux-20.0.8.sources",
                fingerprint("mesa20-galliumaux-manifest"),
                inputs,
                jobs,
                Vec::new(),
            )
        }
        Some("workbench/libs/mesa/libmesa") => {
            let (inputs, jobs) = mesa_jobs();
            (
                "mesa3d-linklib-mesa",
                "mesa3d-linklib-mesa-generated",
                fingerprint("mesa20-core-recipe"),
                "workbench/libs/mesa/libmesa/mesa-20.0.8.sources",
                fingerprint("mesa20-core-manifest"),
                inputs,
                jobs,
                python_packages(),
            )
        }
        Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium") if profile != "x86_64" => {
            let (inputs, jobs) = vc4_jobs();
            (
                "linklibs-gallium_vc4",
                "linklibs-gallium_vc4-gen-cle",
                fingerprint("mesa20-vc4-recipe"),
                "arch/arm-native/soc/broadcom/2708/hidd/vc4gallium/vc4-20.0.8.sources",
                fingerprint("mesa20-vc4-manifest"),
                inputs,
                jobs,
                Vec::new(),
            )
        }
        Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium") => return Ok(None),
        _ => return Ok(None),
    };

    let (recipe_start, recipe_end) = match mmake {
        "mesa3d-linklib-compiler" => (
            "define local-l-or-ll-to-c-or-cpp",
            "%build_linklib mmake=mesa3d-linklib-compiler",
        ),
        "mesa3d-linklib-galliumauxiliary" => (
            "$(top_builddir)/$(CUR_MESADIR)/util/u_format_table.c:",
            "%build_linklib mmake=mesa3d-linklib-galliumauxiliary",
        ),
        "mesa3d-linklib-mesa" => ("define es-gen", "%build_linklib mmake=mesa3d-linklib-mesa"),
        "linklibs-gallium_vc4" => ("CLE_SRCDIR :=", "%build_linklib mmake=linklibs-gallium_vc4"),
        _ => unreachable!("all remaining Mesa generator lanes are enumerated"),
    };
    let recipe_block = normalized_make_capability_block(make_source, recipe_start, recipe_end)
        .ok_or_else(|| {
            format!(
                "{mmake}: generator recipe block is missing; the transpiler capability must be reviewed and updated"
            )
        })?;
    require_text_fingerprint(
        &format!("{}/mmakefile.src", relative_dir.display()),
        &recipe_block,
        recipe_fingerprint,
        mmake,
    )?;
    require_file_fingerprint(root, manifest, manifest_fingerprint, mmake)?;
    let mesa_config_path = root.join("workbench/libs/mesa/mesa.cfg");
    let mesa_config = std::fs::read_to_string(&mesa_config_path).map_err(|error| {
        format!(
            "{mmake}: cannot read {}: {error}",
            mesa_config_path.display()
        )
    })?;
    let config_context = normalized_make_capability_block(
        &mesa_config,
        "aros_mesadir :=",
        "MESA3DGL_GALLIUMCORE :=",
    )
    .ok_or_else(|| {
        format!(
            "{mmake}: Mesa compile configuration block is missing; the transpiler capability must be reviewed and updated"
        )
    })?;
    require_text_fingerprint(
        "workbench/libs/mesa/mesa.cfg compile context",
        &config_context,
        fingerprint("mesa-sse41-config-context"),
        mmake,
    )?;
    require_fetches(fetches)?;
    target_contract_is_exact(root, relative_dir, mmake, target, targets)?;

    Ok(Some(PythonOutputsDecl {
        owner: owner.to_owned(),
        source_root: SOURCE_ROOT.to_owned(),
        build_root: BUILD_ROOT.to_owned(),
        fetch_target: "mesa3d-fetch".to_owned(),
        source_inputs,
        jobs,
        driver_script: Some(DRIVER.to_owned()),
        python_packages: packages,
        audited_source_dir: SOURCE_ROOT.to_owned(),
        local_patch_files: vec![
            "${CMAKE_SOURCE_DIR}/workbench/libs/mesa/mesa-20.0.8-aros.diff".to_owned(),
        ],
        consumers: vec![mmake.to_owned()],
        dir_path: relative_dir.to_path_buf(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{root, target_context};
    use aros_common::read_source;
    use std::path::Path;

    #[test]
    fn mesa20_placement_new_shim_is_limited_to_two_cxx_lanes() {
        let profile = target_context("x86_64", "pc", "");
        let shim = "$<$<COMPILE_LANGUAGE:CXX>:-I${CMAKE_SOURCE_DIR}/workbench/libs/mesa/libcompiler/cxx-compat>";
        for (relative_dir, mmake, expects_shim) in [
            (
                "workbench/libs/mesa/libcompiler",
                "mesa3d-linklib-compiler",
                true,
            ),
            (
                "workbench/libs/mesa/libgalliumaux",
                "mesa3d-linklib-galliumauxiliary",
                false,
            ),
            ("workbench/libs/mesa/libmesa", "mesa3d-linklib-mesa", true),
        ] {
            let contract = compile_contract(Path::new(relative_dir), mmake, Some(&profile))
                .unwrap()
                .unwrap();
            assert_eq!(
                contract.options.iter().any(|option| option == shim),
                expects_shim,
                "{mmake}"
            );
            assert!(
                contract
                    .includes
                    .iter()
                    .all(|include| !include.contains("cxx-compat")),
                "{mmake}: the shim must never become a C-visible include directory"
            );
        }

        for cpu in ["arm", "aarch64"] {
            let profile = target_context(cpu, "raspi", if cpu == "arm" { "hard" } else { "" });
            let contract = compile_contract(
                Path::new("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
                "linklibs-gallium_vc4",
                Some(&profile),
            )
            .unwrap()
            .unwrap();
            assert!(
                contract.options.iter().all(|option| option != shim),
                "{cpu}"
            );
            assert!(
                contract
                    .includes
                    .iter()
                    .all(|include| !include.contains("cxx-compat")),
                "{cpu}"
            );
        }
    }

    #[test]
    fn mesa20_release_patch_and_archive_inventories_are_exact() {
        let root = root();
        let patch_relative = "workbench/libs/mesa/mesa-20.0.8-aros.diff";
        let patch = read_source(&root.join(patch_relative)).unwrap();
        for required in [
            "-#include <algorithm>",
            "st_glsl_to_tgsi_private.h mesa-20.0.8.aros/src/mesa/state_tracker/st_glsl_to_tgsi_private.h",
            "+#ifndef NDEBUG",
            "while (j > 0 && sorter(value, decls[j - 1]))",
            "while (j > 0 && sort_by_begin(value, ranges[j - 1]))",
            "int *idx_map = (int *) CALLOC(narrays + 1, sizeof(*idx_map));",
            "if (!idx_map || (narrays > 0 && !old_sizes))",
            "if (narrays > 0)\n+      memcpy(&old_sizes[0]",
            "temp_comp_access::conditionality_untouched = INT_MAX;",
            "qsort(reg_access, used_temps, sizeof(register_merge_record)",
        ] {
            assert!(
                patch.contains(required),
                "missing release-patch contract: {required}"
            );
        }
        for forbidden in [
            "+#include <memory>",
            "+#include <limits>",
            "+#include <algorithm>",
            "+   std::sort(inout_decls.begin(), inout_decls.end()",
            "+   unique_ptr<int[]>",
        ] {
            assert!(
                !patch.contains(forbidden),
                "release patch reintroduced a target STL dependency: {forbidden}"
            );
        }

        for (cpu, platform, float_abi, expected) in [
            ("x86_64", "pc", "", (239, 11, 1)),
            ("arm", "raspi", "hard", (238, 11, 0)),
            ("aarch64", "raspi", "", (238, 11, 0)),
        ] {
            let profile = target_context(cpu, platform, float_abi);
            let sources = remaining_linklib_sources(
                &root,
                Path::new("workbench/libs/mesa/libmesa"),
                "mesa3d-linklib-mesa",
                Some(&profile),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                (sources.c.len(), sources.cxx.len(), sources.asm.len()),
                expected,
                "{cpu}"
            );
            if cpu == "x86_64" {
                assert_eq!(
                    sources.asm,
                    ["${AROS_PORTS_DIR}/mesa/mesa-20.0.8/src/mesa/x86-64/xform4.S"]
                );
            }
        }

        let x86 = target_context("x86_64", "pc", "");
        for (relative, mmake, expected) in [
            (
                "workbench/libs/mesa/libcompiler",
                "mesa3d-linklib-compiler",
                (154, 105, 0),
            ),
            (
                "workbench/libs/mesa/libgalliumaux",
                "mesa3d-linklib-galliumauxiliary",
                (176, 0, 0),
            ),
        ] {
            let sources = remaining_linklib_sources(&root, Path::new(relative), mmake, Some(&x86))
                .unwrap()
                .unwrap();
            assert_eq!(
                (sources.c.len(), sources.cxx.len(), sources.asm.len()),
                expected,
                "{mmake}"
            );
        }

        for (cpu, float_abi) in [("arm", "hard"), ("aarch64", "")] {
            let profile = target_context(cpu, "raspi", float_abi);
            let sources = remaining_linklib_sources(
                &root,
                Path::new("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium"),
                "linklibs-gallium_vc4",
                Some(&profile),
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                (sources.c.len(), sources.cxx.len(), sources.asm.len()),
                (43, 0, 0)
            );
        }
    }
}
