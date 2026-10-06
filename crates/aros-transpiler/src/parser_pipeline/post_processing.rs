//! Closed source-generator admission and final target adjustments.

use super::{
    capability_diagnostic, current_profile, generators, render_meta_token, Diagnostic, FetchDecl,
    MetaTargetRule, Path, TargetContext, TargetDefinition,
};
use crate::capability::mesa::{mesa20, mesa26};
use crate::parser::{join_mm_continuations, META_RULE_RE};

pub(super) struct PythonOutputContext<'a> {
    pub(super) root: &'a Path,
    pub(super) rel_dir: &'a Path,
    pub(super) relative_path: &'a Path,
    pub(super) target: Option<&'a TargetContext>,
    pub(super) content: &'a str,
    pub(super) targets: &'a mut Vec<TargetDefinition>,
    pub(super) ownership_fetches: &'a [FetchDecl],
    pub(super) capability_errors: &'a mut Vec<Diagnostic>,
    pub(super) skipped_programs: &'a mut Vec<String>,
}

pub(super) fn collect_python_outputs(
    context: PythonOutputContext<'_>,
) -> Vec<crate::ast::PythonOutputsDecl> {
    let PythonOutputContext {
        root,
        rel_dir,
        relative_path,
        target,
        content,
        targets,
        ownership_fetches,
        capability_errors,
        skipped_programs,
    } = context;
    let mut python_outputs = Vec::new();
    match generators::parse_glapi(rel_dir, target, content, targets, ownership_fetches) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            capability_errors.push(capability_diagnostic(
                relative_path,
                None,
                format!("Mesa glapi generator no longer matches its closed capability: {reason}"),
            ));
            skipped_programs.push(format!(
                "{}: Mesa glapi Python generator skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    match generators::parse_mesautil(root, rel_dir, target, content, targets, ownership_fetches) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            capability_errors.push(capability_diagnostic(
                relative_path,
                None,
                format!("Mesa utility generator no longer matches its closed capability: {reason}"),
            ));
            skipped_programs.push(format!(
                "{}: Mesa utility Python generator skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    let mesa20_required_target = match rel_dir.to_str() {
        Some("workbench/libs/mesa/libcompiler") => Some("mesa3d-linklib-compiler"),
        Some("workbench/libs/mesa/libgalliumaux") => Some("mesa3d-linklib-galliumauxiliary"),
        Some("workbench/libs/mesa/libmesa") => Some("mesa3d-linklib-mesa"),
        Some("arch/arm-native/soc/broadcom/2708/hidd/vc4gallium")
            if current_profile(target).ok() != Some("x86_64") =>
        {
            Some("linklibs-gallium_vc4")
        }
        _ => None,
    };
    let mesa26 = target.and_then(|profile| profile.mesa_version.as_deref()) == Some("26.0.0");
    let remaining = if mesa26 {
        match rel_dir.to_str() {
            Some("workbench/libs/mesa/libcompiler") => {
                mesa26::parse_compiler(root, rel_dir, target, targets, ownership_fetches)
            }
            Some("workbench/libs/mesa/libgalliumaux") => {
                mesa26::parse_galliumaux(root, rel_dir, target, targets, ownership_fetches)
            }
            Some("workbench/libs/mesa/libmesa") => {
                mesa26::parse_core(root, rel_dir, target, targets, ownership_fetches)
            }
            _ => Ok(None),
        }
    } else {
        mesa20::parse_remaining(root, rel_dir, target, content, targets, ownership_fetches)
    };
    match remaining {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            if let Some(mmake) = mesa20_required_target {
                // Source admission and every generator product form one
                // capability. A partial archive with missing generated
                // translation units is never an executable fallback.
                targets.retain(|candidate| candidate.mmake_name != mmake);
            }
            capability_errors.push(capability_diagnostic(
                relative_path,
                None,
                format!("Mesa archive/generator no longer matches its closed capability: {reason}"),
            ));
            skipped_programs.push(format!(
                "{}: Mesa archive/generator capability skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    let v3d = if mesa26 {
        mesa26::parse_v3d(root, rel_dir, target, targets, ownership_fetches)
            .map(|declaration| declaration.into_iter().collect())
    } else {
        mesa20::parse_v3d(root, rel_dir, target, content, targets, ownership_fetches)
    };
    match v3d {
        Ok(declarations) => python_outputs.extend(declarations),
        Err(reason) => {
            targets.retain(|candidate| candidate.mmake_name != "linklibs-gallium_v3d");
            capability_errors.push(capability_diagnostic(
                relative_path,
                None,
                format!(
                    "Mesa 20.0.8 V3D archive/generators no longer match their closed capability: {reason}"
                ),
            ));
            skipped_programs.push(format!(
                "{}: Mesa 20.0.8 V3D archive/generator capability skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    python_outputs
}

pub(super) fn collect_meta_rules_and_apply_llvm(
    content: &str,
    rel_dir: &Path,
    target: Option<&TargetContext>,
    targets: &mut [TargetDefinition],
    meta_rules: &mut Vec<MetaTargetRule>,
    skipped_meta_rules: &mut Vec<String>,
) {
    // 3. Extract #MM and #MM- meta-target rules
    let mm_content = join_mm_continuations(content);
    for cap in META_RULE_RE.captures_iter(&mm_content) {
        let raw_meta = &cap[1];
        let Some(meta_name) = render_meta_token(raw_meta) else {
            skipped_meta_rules.push(format!(
                "{}: #MM target {raw_meta} contains an unmapped Make variable",
                rel_dir.display()
            ));
            continue;
        };
        let deps_str = &cap[2];
        let mut deps = Vec::new();
        for raw_dep in deps_str.split_whitespace() {
            match render_meta_token(raw_dep) {
                Some(dep) => deps.push(dep),
                None => skipped_meta_rules.push(format!(
                    "{}: #MM {raw_meta} dependency {raw_dep} contains an unmapped Make variable",
                    rel_dir.display()
                )),
            }
        }

        if !deps.is_empty() {
            meta_rules.push(MetaTargetRule {
                name: meta_name,
                dependencies: deps,
            });
        }
    }

    // LLVM is a structured multi-archive provider, not a configure-time SDK
    // wildcard. Its consumers acquire both the real build edge and includes.
    if target.is_some_and(|context| context.mesa_version.as_deref() == Some("26.0.0")) {
        for declaration in targets {
            if matches!(
                declaration.mmake_name.as_str(),
                "mesa3d-linklib-galliumvm"
                    | "mesa3d-linklib-llvmpipe"
                    | "mesa3d-linklib-galliumdrawllvm"
                    | "hidd-llvmpipe"
            ) {
                declaration.use_libs.push("LLVM".to_owned());
            }
            if declaration.mmake_name == "hidd-llvmpipe"
                && rel_dir == Path::new("workbench/hidds/llvmpipe")
            {
                declaration.use_libs = [
                    "llvmpipe",
                    "galliumvm",
                    "compiler",
                    "galliumdrawllvm",
                    "galliumtess",
                    "galliumauxiliary",
                    "mesautil",
                    "LLVM",
                    "z",
                    "pthread",
                    "posixc_rel",
                    "stdc_rel",
                ]
                .map(str::to_owned)
                .to_vec();
                // The source recipe deliberately retains the complete MCJIT
                // helper archive. Preserve that semantics without importing
                // its unresolved SDK LLVM wildcard or Make shell expansion.
                declaration.link_options = [
                    "-L${AROS_BUILD_DIR}/gen/lib/mesa26.0.0",
                    "--whole-archive",
                    "-lgalliumvm",
                    "--no-whole-archive",
                ]
                .map(str::to_owned)
                .to_vec();
            }
        }
    }
}

pub(super) fn filter_generated_file_templates(
    generated_files: &mut Vec<String>,
    python_outputs: &[crate::ast::PythonOutputsDecl],
) {
    // A pattern recipe is a template, not a literal missing output. When a
    // closed Python-output capability instantiates concrete products matching
    // that template (V3D's version wrappers are the current case), keep the
    // template out of the residual generated-file report.
    let capability_outputs = python_outputs
        .iter()
        .flat_map(|declaration| {
            declaration.jobs.iter().map(|job| {
                format!(
                    "{}/{}",
                    declaration.build_root.trim_end_matches('/'),
                    job.output.trim_start_matches('/')
                )
                .replace("${AROS_BUILD_DIR}", "${CMAKE_BINARY_DIR}")
            })
        })
        .collect::<Vec<_>>();
    generated_files.retain(|report| {
        let Some((target, _)) = report.split_once(" <- ") else {
            return true;
        };
        let target = target.replace("${AROS_BUILD_DIR}", "${CMAKE_BINARY_DIR}");
        let Some((prefix, suffix)) = target.split_once('%') else {
            return true;
        };
        !capability_outputs.iter().any(|output| {
            output
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .is_some()
        })
    });
}
