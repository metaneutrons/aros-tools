//! Closed source-generator admission and final target adjustments.

use super::{
    capability_diagnostics_for_targets, current_profile, generators, render_meta_token,
    unique_mmake_owners, Diagnostic, FetchDecl, MetaTargetRule, Path, TargetContext,
    TargetDefinition,
};
use crate::capability::mesa::{mesa20, mesa26};
use crate::parser::{join_mm_continuations, META_RULE_RE};

pub(super) struct PythonOutputContext<'a> {
    pub(super) root: &'a Path,
    pub(super) rel_dir: &'a Path,
    pub(super) relative_path: &'a Path,
    pub(super) target: Option<&'a TargetContext>,
    pub(super) content: &'a str,
    pub(super) invocation_owners: &'a std::collections::BTreeMap<String, usize>,
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
        invocation_owners,
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
            let owners = unique_mmake_owners(invocation_owners, &["mesa3d-linklib-glapi"])
                .unwrap_or_default();
            capability_errors.extend(capability_diagnostics_for_targets(
                relative_path,
                None,
                owners,
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
            let owners = unique_mmake_owners(
                invocation_owners,
                &["mesa3d-linklib-mesautil", "mesa3d-linklib-mesadevutil"],
            )
            .unwrap_or_default();
            capability_errors.extend(capability_diagnostics_for_targets(
                relative_path,
                None,
                owners,
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
            let owners = mesa20_required_target
                .and_then(|mmake| unique_mmake_owners(invocation_owners, &[mmake]))
                .unwrap_or_default();
            if let Some(mmake) = mesa20_required_target {
                // Source admission and every generator product form one
                // capability. A partial archive with missing generated
                // translation units is never an executable fallback.
                targets.retain(|candidate| candidate.mmake_name != mmake);
            }
            capability_errors.extend(capability_diagnostics_for_targets(
                relative_path,
                None,
                owners,
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
            let owners = unique_mmake_owners(invocation_owners, &["linklibs-gallium_v3d"])
                .unwrap_or_default();
            targets.retain(|candidate| candidate.mmake_name != "linklibs-gallium_v3d");
            capability_errors.extend(capability_diagnostics_for_targets(
                relative_path,
                None,
                owners,
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
    make_meta_providers: &mut Vec<String>,
) -> Vec<Diagnostic> {
    let mut unresolved_rules = Vec::new();
    let no_globals = std::collections::BTreeMap::new();
    let globals = target.map_or(&no_globals, |target| &target.native_metamake_globals);
    // A dependency word that is exactly one policy global with an empty value
    // expands to nothing, as in MetaMake's word list.
    let empty_global = |raw: &str| {
        raw.strip_prefix("$(")
            .and_then(|rest| rest.strip_suffix(')'))
            .is_some_and(|name| globals.get(name).is_some_and(String::is_empty))
    };
    let mm_content = join_mm_continuations(content);
    for cap in META_RULE_RE.captures_iter(&mm_content) {
        let raw_meta = &cap[1];
        let Some(meta_name) = crate::parser::render_meta_token_with(raw_meta, globals) else {
            skipped_meta_rules.push(format!(
                "{}: #MM target {raw_meta} contains an unmapped Make variable",
                rel_dir.display()
            ));
            continue;
        };
        let deps_str = &cap[2];
        let dep_words = deps_str
            .split_whitespace()
            .filter(|raw| !empty_global(raw))
            .collect::<Vec<_>>();
        let mut deps = Vec::new();
        for raw_dep in dep_words.iter().copied() {
            match crate::parser::render_meta_token_with(raw_dep, globals) {
                Some(dep) => deps.push(dep),
                None => skipped_meta_rules.push(format!(
                    "{}: #MM {raw_meta} dependency {raw_dep} contains an unmapped Make variable",
                    rel_dir.display()
                )),
            }
        }

        if deps.len() != dep_words.len() {
            unresolved_rules.push(super::source_meta_provider_diagnostic(
                &rel_dir.join("mmakefile.src"),
                &meta_name,
                format!("MetaMake provider {meta_name} has unresolved prerequisites; a concrete endpoint cannot discard source dependencies"),
            ));
        }

        let virtual_target = cap[0].starts_with("#MM-");
        if !virtual_target {
            make_meta_providers.push(meta_name.clone());
        }
        if (virtual_target || !deps.is_empty()) && deps.len() == dep_words.len() {
            meta_rules.push(MetaTargetRule {
                name: meta_name,
                dependencies: deps,
            });
        }
    }

    // A bare #MM marker names the ordinary Make target on the next line.
    // MetaMake treats that provider as nonvirtual even when its prerequisites
    // are empty. Record its existence without inventing an executable recipe.
    let mut marker = false;
    for line in mm_content.lines() {
        if marker {
            if let Some((name, _)) = line.split_once(':') {
                if let Some(name) = name.split_whitespace().next().and_then(render_meta_token) {
                    make_meta_providers.push(name);
                }
            }
        }
        if let Some(name) = line.trim().strip_prefix("#MM ") {
            // A named no-colon marker is also a nonvirtual Make provider.
            // The closed aggregate scanner verifies its unique source-local
            // ordinary rule; only the bare marker requires the next line.
            if !name.contains(':') {
                if let Some(name) = render_meta_token(name.trim()) {
                    make_meta_providers.push(name);
                }
            }
        }
        marker = line.trim() == "#MM";
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

    unresolved_rules
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unresolved_meta_prerequisite_retains_only_source_proven_selector_owner() {
        let mut rules = Vec::new();
        let mut skipped = Vec::new();
        let mut providers = Vec::new();
        let failures = collect_meta_rules_and_apply_llvm(
            "#MM- workbench-linux-$(CPU) : known $(UNKNOWN_DEPENDENCY)\n",
            Path::new("arch/all-linux"),
            None,
            &mut [],
            &mut rules,
            &mut skipped,
            &mut providers,
        );
        assert!(rules.is_empty(), "a partial edge list is not a provider");
        assert!(providers.is_empty());
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].context.as_ref().unwrap().target.as_deref(),
            Some("workbench-linux-${AROS_TARGET_CPU}")
        );
        assert!(failures[0].message.contains("unresolved prerequisites"));
        assert!(!skipped.is_empty());
    }

    #[test]
    fn native_policy_globals_resolve_meta_names_and_empty_words() {
        let source = "#MM- linklibs-x-includes : sdk-includes-$(AROS_TOOLCHAIN_RELEASE) $(CROSSTOOLS_PORTS_INCLUDES) $(UNBOUND)\n#MM- linklibs-y : sdk-includes-$(AROS_TOOLCHAIN_RELEASE) $(CROSSTOOLS_PORTS_INCLUDES)\n";
        let context = TargetContext {
            native_metamake_globals: [
                ("AROS_TOOLCHAIN_RELEASE".to_owned(), "0".to_owned()),
                ("CROSSTOOLS_PORTS_INCLUDES".to_owned(), String::new()),
            ]
            .into(),
            ..TargetContext::default()
        };
        let mut rules = Vec::new();
        let mut skipped = Vec::new();
        let mut providers = Vec::new();
        let failures = collect_meta_rules_and_apply_llvm(
            source,
            Path::new("compiler/x"),
            Some(&context),
            &mut [],
            &mut rules,
            &mut skipped,
            &mut providers,
        );
        // A variable the policy does not bind stays unresolved.
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].context.as_ref().unwrap().target.as_deref(),
            Some("linklibs-x-includes")
        );
        // The empty global contributes no word; the bound one its value.
        assert_eq!(
            rules
                .iter()
                .find(|rule| rule.name == "linklibs-y")
                .map(|rule| rule.dependencies.clone()),
            Some(vec!["sdk-includes-0".to_owned()])
        );

        // Without a native policy nothing changes: both rules are unresolved.
        let mut rules = Vec::new();
        let failures = collect_meta_rules_and_apply_llvm(
            source,
            Path::new("compiler/x"),
            None,
            &mut [],
            &mut rules,
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert_eq!(failures.len(), 2);
        assert!(rules.is_empty());
    }
}
