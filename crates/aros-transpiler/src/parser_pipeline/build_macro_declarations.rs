//! Remaining build-macro declarations (`%build_progs`, `%build_linklib`, `%build_module_simple`, ...).

use super::declaration_context::{DeclarationInputs, DeclarationOutputs};
use super::{
    all_sources_are_fetch_owned, apply_mesa_compile_contract, capability_diagnostic_with_owner,
    collect_includes_at, declaration_flags_at, declaration_global_link_options,
    evaluate_macro_sources_with_files, evaluate_make_expr, evaluate_name,
    evaluate_output_directory, exact_mmake_target, expand_file_list, macro_arg,
    merge_named_link_flags, record_partial_source_lists, remaining_linklib_sources,
    resolve_generated_linklib_sources, resolve_module_suffix, resolve_module_target_dir,
    resolve_no_argument, resolve_yes_argument, safe_build_tree_output_directory, sanitize_ident,
    sse41, wildcard_c_sources, MakeExprContext, ModuleType, TargetDefinition, PRIVATE_LIBDIR,
};
use crate::ast::ModuleMacroForm;
use crate::capability::mesa::mesa26;

#[expect(
    clippy::too_many_lines,
    reason = "the declaration pass is one ordered scope transaction lifted unchanged out of the pipeline function; the file-size gate bounds it"
)]
pub(super) fn collect_build_macros(inputs: DeclarationInputs<'_>, outputs: DeclarationOutputs<'_>) {
    let DeclarationInputs {
        root,
        dirs,
        target,
        rel_dir,
        relative_path,
        parent_dir,
        content,
        joined,
        scope,
        flag_set,
        include_set,
        invocations,
        fetches,
        opts_arch_includes,
        opts_include_dirs,
        opts_link_options,
        opts_spec_switches,
        arch_defines,
        arch_compile_options,
        capture_kobj_inputs,
        ..
    } = inputs;
    let DeclarationOutputs {
        targets,
        capability_errors,
        skipped_programs,
        unresolved_output_paths,
        partial_source_lists,
        source_inventory_patterns,
        source_inventory_needs,
        source_inventory_targets,
        ..
    } = outputs;
    // 2b. The remaining build macros.
    //
    // All four share the compile model and differ only in what they link:
    // %build_prog one executable, %build_progs one per file, %build_linklib a
    // static library, %build_module_simple a module without the genmodule
    // chain. Only the link kind and the name argument change here.
    for inv in invocations {
        let (module_type, name_arg) = match inv.name.as_str() {
            "build_progs" => (ModuleType::ProgramGroup, None),
            "build_linklib" => (ModuleType::LinkLib, Some("libname")),
            "build_module_simple" => (ModuleType::SimpleModule, Some("modname")),
            _ => continue,
        };

        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let vars = scope.snapshot(inv.line);
        let expression_context = MakeExprContext::new(scope, dirs, inv.line, root, rel_dir);
        let isa_link_options = declaration_global_link_options(
            "TARGET_ISA_LDFLAGS",
            scope,
            dirs,
            root,
            rel_dir,
            inv.line,
        );
        let driver_link_options =
            declaration_global_link_options("USER_LDFLAGS", scope, dirs, root, rel_dir, inv.line);
        let mut declaration_flags = declaration_flags_at(
            scope,
            inv.line,
            target,
            flag_set,
            opts_link_options,
            opts_spec_switches,
        );
        let mut declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(joined, scope, inv.line, rel_dir),
        );
        let mmake_name = sanitize_ident(&mmake_raw);
        let mmake_owner = exact_mmake_target(&mmake_raw);
        let mesa20_capability_sources = match remaining_linklib_sources(
            root,
            rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                        relative_path,
                        Some(inv.line + 1),
                        mmake_owner.as_deref(),
                        format!(
                            "%{} mmake={mmake_raw} no longer matches the Mesa archive capability: {reason}",
                            inv.name
                        ),
                    ));
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Mesa 20.0.8 archive capability skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        let mesa20_capability_active = mesa20_capability_sources.is_some();
        let mesa26_sources = match mesa26::archive_sources(root, rel_dir, &mmake_name, target) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                    relative_path,
                    Some(inv.line + 1),
                    mmake_owner.as_deref(),
                    format!("Mesa 26 glapi capability: {reason}"),
                ));
                skipped_programs.push(format!(
                    "{}:{}: Mesa 26 glapi linklib skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                ));
                continue;
            }
        };
        let mesa26_archive_active = mesa26_sources.is_some();
        let nouveau_drm_capability_sources = match crate::capability::nouveau::drm_sources(
            root,
            rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                        relative_path,
                        Some(inv.line + 1),
                        mmake_owner.as_deref(),
                        format!(
                            "%{} mmake={mmake_raw} no longer matches the Nouveau DRM archive capability: {reason}",
                            inv.name
                        ),
                    ));
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau DRM archive capability skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        };
        let nouveau_drm_capability_active = nouveau_drm_capability_sources.is_some();
        let nouveau_gallium_capability_sources = match crate::capability::nouveau::gallium_sources(
            root,
            rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                    relative_path,
                    Some(inv.line + 1),
                    mmake_owner.as_deref(),
                    format!(
                        "%{} mmake={mmake_raw} no longer matches the Nouveau Gallium archive capability: {reason}",
                        inv.name
                    ),
                ));
                skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} Nouveau Gallium archive capability skipped: {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                continue;
            }
        };
        let nouveau_gallium_capability_active = nouveau_gallium_capability_sources.is_some();
        if let Err(reason) = apply_mesa_compile_contract(
            rel_dir,
            &mmake_name,
            target,
            &mut declaration_flags,
            &mut declaration_includes,
        ) {
            capability_errors.push(capability_diagnostic_with_owner(
                relative_path,
                Some(inv.line + 1),
                mmake_owner.as_deref(),
                format!(
                    "%{} mmake={mmake_raw} no longer matches the Mesa compile capability: {reason}",
                    inv.name
                ),
            ));
            skipped_programs.push(format!(
                "{}:{}: %{} mmake={mmake_raw} Mesa 20.0.8 compile contract skipped: {reason}",
                rel_dir.display(),
                inv.line + 1,
                inv.name
            ));
            continue;
        }
        match crate::capability::nouveau::drm_compile_contract(rel_dir, &mmake_name, target) {
            Ok(Some(contract)) => {
                declaration_flags.defines = contract.defines;
                declaration_flags.undefines.clear();
                declaration_flags.compile_options = contract.options;
                declaration_flags.link_options.clear();
                declaration_includes.dirs = contract.includes;
                declaration_includes.arch_modules.clear();
            }
            Ok(None) => {}
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                    relative_path,
                    Some(inv.line + 1),
                    mmake_owner.as_deref(),
                    format!(
                        "%{} mmake={mmake_raw} no longer matches the Nouveau DRM compile capability: {reason}",
                        inv.name
                    ),
                ));
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau DRM compile contract skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        }
        match crate::capability::nouveau::gallium_compile_contract(rel_dir, &mmake_name, target) {
            Ok(Some(contract)) => {
                declaration_flags.defines = contract.defines;
                declaration_flags.undefines.clear();
                declaration_flags.compile_options = contract.options;
                declaration_flags.link_options.clear();
                declaration_includes.dirs = contract.includes;
                declaration_includes.arch_modules.clear();
            }
            Ok(None) => {}
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                    relative_path,
                    Some(inv.line + 1),
                    mmake_owner.as_deref(),
                    format!(
                        "%{} mmake={mmake_raw} no longer matches the Nouveau Gallium compile capability: {reason}",
                        inv.name
                    ),
                ));
                skipped_programs.push(format!(
                    "{}:{}: %{} mmake={mmake_raw} Nouveau Gallium compile contract skipped: {reason}",
                    rel_dir.display(),
                    inv.line + 1,
                    inv.name
                ));
                continue;
            }
        }
        let mesa26_empty_sse41 = mmake_name == sse41::MMAKE
            && target.and_then(|profile| profile.mesa_version.as_deref()) == Some("26.0.0")
            && mesa26_archive_active;
        let mesa_sse41_profile = (mmake_name == sse41::MMAKE
            && !mesa26_empty_sse41
            && sse41::validate_static_contract(root, content).is_ok())
        .then(|| sse41::profile(rel_dir, target).ok().flatten())
        .flatten();
        let empty_archive = mesa26_empty_sse41 || mesa_sse41_profile == Some(false);
        if mesa26_empty_sse41 {
            let Ok(Some(contract)) = mesa26::compile_contract(rel_dir, &mmake_name, target) else {
                capability_errors.push(capability_diagnostic_with_owner(
                    relative_path,
                    Some(inv.line + 1),
                    mmake_owner.as_deref(),
                    "Mesa 26 empty SSE4.1 compile contract is absent".to_owned(),
                ));
                continue;
            };
            declaration_flags.defines = contract.defines;
            declaration_flags.undefines = contract.undefines;
            declaration_flags.compile_options = contract.options;
            declaration_flags.link_options.clear();
            declaration_includes.dirs = contract.includes;
            declaration_includes.arch_modules.clear();
        }
        if let Some(x86_64) = mesa_sse41_profile {
            // The ordinary local-include scanner cannot adopt mesa.cfg for
            // this file on a cold tree: the neighbouring full libmesa target
            // still depends on the not-yet-fetched upstream inventory. Admit
            // the exact declaration-local view only together with the
            // capability and profile contract validated below.
            declaration_flags.defines = sse41::defines(x86_64);
            declaration_flags.undefines.clear();
            declaration_flags.compile_options = sse41::compile_options(x86_64);
            declaration_flags.link_options.clear();
            declaration_includes.dirs = sse41::INCLUDES
                .iter()
                .map(|include| (*include).to_owned())
                .collect();
            declaration_includes.arch_modules.clear();
        }

        // %build_progs has no name of its own: each source file names its own
        // executable, so the mmake id carries the group.
        let target_name = match name_arg {
            None => mmake_name.clone(),
            Some(key) => {
                let Some(raw) = macro_arg(&inv.args, key) else {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} has no {key}",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                };
                match evaluate_name(&raw, &expression_context) {
                    Ok(name) => name,
                    Err(reason) => {
                        skipped_programs.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} {key}={raw} is unresolved: {reason}",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        continue;
                    }
                }
            }
        };
        if matches!(module_type, ModuleType::SimpleModule) {
            // config/make.tmpl appends `<modname>_LDFLAGS` only to this bare
            // module's link. Preserve that scope instead of forcing a
            // file-global USER_LDFLAGS change onto neighbouring modules.
            merge_named_link_flags(
                &mut declaration_flags,
                scope,
                inv.line,
                &format!("{target_name}_LDFLAGS"),
            );
        }

        let resolved_generated_files = if module_type == ModuleType::LinkLib {
            match macro_arg(&inv.args, "files") {
                Some(files) => {
                    match resolve_generated_linklib_sources(&files, joined, rel_dir, |name| {
                        expression_context.safe_local_raw(name)
                    }) {
                        Ok(Some(generated)) => Some(generated.sources),
                        Ok(None) => None,
                        Err(reason) => {
                            skipped_programs.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {reason}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            continue;
                        }
                    }
                }
                None => None,
            }
        } else {
            None
        };
        let capability_files = mesa_sse41_profile.map(sse41::sources);
        let mut sources = if let Some(sources) = mesa26_sources {
            sources
        } else if let Some(sources) = mesa20_capability_sources {
            sources
        } else if let Some(sources) = nouveau_drm_capability_sources {
            sources
        } else if let Some(sources) = nouveau_gallium_capability_sources {
            sources
        } else {
            match evaluate_macro_sources_with_files(
                &inv.args,
                &vars,
                &expression_context,
                capability_files
                    .as_deref()
                    .or(resolved_generated_files.as_deref()),
            ) {
                Ok(sources) => sources,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        };
        record_partial_source_lists(
            partial_source_lists,
            source_inventory_patterns,
            source_inventory_needs,
            &sources,
            relative_path,
            inv,
            &mmake_raw,
        );
        let source_inventory_only = sources.is_empty() && !sources.deferred_wildcards.is_empty();
        if sources.is_empty() && !empty_archive && !source_inventory_only {
            if sources.declared {
                skipped_programs.push(format!(
                    "{}: %{} mmake={mmake_raw} has an unresolved file list",
                    rel_dir.display(),
                    inv.name
                ));
                continue;
            }
            // %build_module_simple defaults files to every *.c in the
            // directory. The others have no default, and %build_progs even
            // declares files=/A, so a declaration without sources is
            // malformed.
            if matches!(module_type, ModuleType::SimpleModule) {
                sources.c = wildcard_c_sources(parent_dir);
            }
            if sources.is_empty() {
                skipped_programs.push(format!(
                    "{}: %{} mmake={mmake_raw} declares no sources",
                    rel_dir.display(),
                    inv.name
                ));
                continue;
            }
        }

        let use_libs =
            macro_arg(&inv.args, "uselibs").map_or_else(Vec::new, |l| expand_file_list(&l, &vars));
        let is_simple_module = matches!(module_type, ModuleType::SimpleModule);
        let always_cxx_link = if is_simple_module {
            match resolve_yes_argument(&inv.args, "alwayscxxlink", scope, dirs, inv.line) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else {
            false
        };
        let declared_mod_type = if is_simple_module {
            macro_arg(&inv.args, "modtype")
        } else {
            None
        };
        let is_program_group = matches!(module_type, ModuleType::ProgramGroup);
        let no_startup = if is_program_group {
            match resolve_no_argument(&inv.args, "usestartup", scope, dirs, inv.line) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else {
            false
        };
        let detach = if is_program_group {
            match resolve_yes_argument(&inv.args, "detach", scope, dirs, inv.line) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else {
            false
        };
        let target_dir = if is_simple_module {
            match resolve_module_target_dir(
                &inv.args,
                scope,
                dirs,
                inv.line,
                declared_mod_type.as_deref().unwrap_or_default(),
                false,
                false,
            ) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else if is_program_group {
            match evaluate_output_directory(&inv.args, &expression_context) {
                Ok(directory) => directory,
                Err(reason) => {
                    unresolved_output_paths.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    None
                }
            }
        } else {
            None
        };
        let mod_suffix = if is_simple_module {
            match resolve_module_suffix(
                &inv.args,
                scope,
                dirs,
                inv.line,
                declared_mod_type.as_deref().unwrap_or_default(),
            ) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            }
        } else {
            None
        };
        // The 32-bit flavour is told apart by where it writes, not by its
        // name: libdir=$(GENDIR)/lib32 and objdir=.../32bit.
        let variant_32bit = ["libdir", "objdir"].iter().any(|k| {
            macro_arg(&inv.args, k).is_some_and(|v| v.contains("lib32") || v.contains("32bit"))
        });
        let canonical_linklib_eligible = matches!(module_type, ModuleType::LinkLib)
            && macro_arg(&inv.args, "libdir").is_none()
            && macro_arg(&inv.args, "compiler").is_none_or(|value| value == "target")
            && !variant_32bit;
        let canonical_linklib_output = canonical_linklib_eligible
            && (all_sources_are_fetch_owned(&sources, fetches)
                || nouveau_drm_capability_active
                || nouveau_gallium_capability_active);
        let linklib_output_dir = if mesa26_archive_active {
            Some(mesa26::PRIVATE_LIBDIR.to_owned())
        } else if mesa_sse41_profile.is_some() || mesa20_capability_active {
            Some(PRIVATE_LIBDIR.to_owned())
        } else if matches!(module_type, ModuleType::LinkLib) {
            macro_arg(&inv.args, "libdir").and_then(|raw| {
                match evaluate_make_expr(&raw, &expression_context) {
                    Ok(directory) if safe_build_tree_output_directory(&directory) => {
                        Some(directory)
                    }
                    Ok(directory) => {
                        unresolved_output_paths.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} libdir={raw} resolves outside the build tree ({directory})",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        None
                    }
                    Err(reason) => {
                        unresolved_output_paths.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} libdir={raw} is unresolved: {reason}",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        None
                    }
                }
            })
        } else {
            None
        };

        let kobj_scoped_inputs = is_simple_module
            .then(|| capture_kobj_inputs(inv, &target_name))
            .flatten();
        let parsed_target = TargetDefinition {
            mmake_name,
            target_name,
            module_type,
            module_macro: is_simple_module.then_some(ModuleMacroForm::Simple),
            kobj_scoped_inputs,
            genmodule_only: false,
            genmodule_abi: false,
            empty_archive,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
            no_startup,
            detach,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit,
            declared_mod_type,
            mod_suffix,
            linklib_name: None,
            config_file: None,
            config_override_file: None,
            genmodule_linklibs: None,
            config_relative_libraries: Vec::new(),
            canonical_linklib_output,
            canonical_linklib_eligible,
            linklib_output_dir,
            compiler_flags: Vec::new(),
            include_dirs: {
                let mut d = declaration_includes.dirs.clone();
                d.extend(opts_include_dirs.iter().cloned());
                d
            },
            arch_modules: declaration_includes.arch_modules.clone(),
            arch_includes: opts_arch_includes.clone(),
            defines: declaration_flags.defines,
            undefines: declaration_flags.undefines,
            compile_options: declaration_flags.compile_options,
            link_options: declaration_flags.link_options,
            spec_switches: declaration_flags.spec_switches.clone(),
            driver_link_options: driver_link_options.clone(),
            isa_link_options: isa_link_options.clone(),
            arch_sources: Vec::new(),
            arch_defines: arch_defines.clone(),
            arch_compile_options: arch_compile_options.clone(),
            arch_source_options: Vec::new(),
            selection_headers_only: false,
        };
        if source_inventory_only {
            source_inventory_targets.push((&parsed_target).into());
        } else {
            targets.push(parsed_target);
        }
    }
}
