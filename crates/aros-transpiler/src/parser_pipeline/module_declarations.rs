//! `%build_module` family declarations.

use super::declaration_context::{DeclarationInputs, DeclarationOutputs};
use super::{
    apply_mesa_compile_contract, capability_diagnostic_with_owner, collect_includes_at,
    declaration_flags_at, declaration_global_link_options, evaluate_linklib_list,
    evaluate_macro_sources, evaluate_make_expr, evaluate_name, exact_mmake_target,
    expand_file_list, implicit_module_meta_rules, is_explicit_genmodule_only, macro_arg,
    map_linklib_object_sources, read_genmodule_linklib_config, read_genmodule_linklib_config_files,
    record_partial_source_lists, render_meta_token, resolve_module_suffix,
    resolve_module_target_dir, resolve_yes_argument, sanitize_ident, wildcard_c_sources,
    EvaluatedSources, GenmoduleConfigFacts, GenmoduleLinklibs, MakeExprContext, ModuleType, Path,
    TargetDefinition,
};
use crate::ast::ModuleMacroForm;
use crate::capability::mesa::mesa26;
use crate::module_paths::implicit_module_header_meta_rules;

#[expect(
    clippy::too_many_lines,
    reason = "the declaration pass is one ordered scope transaction lifted unchanged out of the pipeline function; the file-size gate bounds it"
)]
pub(super) fn collect_modules(inputs: DeclarationInputs<'_>, outputs: DeclarationOutputs<'_>) {
    let DeclarationInputs {
        root,
        dirs,
        target,
        rel_dir,
        relative_path,
        parent_dir,
        joined,
        scope,
        flag_set,
        include_set,
        invocations,
        re_libs,
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
        meta_rules,
        targets,
        capability_errors,
        skipped_programs,
        skipped_client_archives,
        partial_source_lists,
        source_inventory_patterns,
        source_inventory_needs,
        source_inventory_targets,
        ..
    } = outputs;
    // 1. Extract module definitions
    for inv in invocations.iter().filter(|i| {
        matches!(
            i.name.as_str(),
            "build_module" | "build_module_abi" | "build_module_library"
        )
    }) {
        // The three spellings wrap the same %build_module_core, but the ABI
        // form deliberately has no runtime compilation (make.tmpl:2828).
        let Some(mmake_raw) = macro_arg(&inv.args, "mmake") else {
            continue;
        };
        let Some(mod_raw) = macro_arg(&inv.args, "modname") else {
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
        let headers_projection = if let Err(reason) = if inv.name == "build_module_abi" {
            Ok(false)
        } else {
            apply_mesa_compile_contract(
                rel_dir,
                &mmake_name,
                target,
                &mut declaration_flags,
                &mut declaration_includes,
            )
        } {
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
            if inv.name != "build_module" {
                continue;
            }
            true
        } else {
            false
        };
        let mod_name =
            match mesa26::runtime_module_name(root, rel_dir, &mmake_name, &mod_raw, target) {
                Ok(Some(name)) => name,
                Ok(None) => sanitize_ident(&mod_raw),
                Err(reason) => {
                    capability_errors.push(capability_diagnostic_with_owner(
                        relative_path,
                        Some(inv.line + 1),
                        mmake_owner.as_deref(),
                        format!("Mesa runtime identity: {reason}"),
                    ));
                    continue;
                }
            };
        let mod_type_owned = macro_arg(&inv.args, "modtype").unwrap_or_default();
        let mod_type_str = mod_type_owned.as_str();
        let rest = inv.args.as_str();
        let is_abi = inv.name == "build_module_abi";

        let module_type = if headers_projection {
            ModuleType::ModuleHeaders
        } else if is_abi {
            ModuleType::Abi
        } else {
            match mod_type_str {
                "library" => ModuleType::Library,
                "device" => ModuleType::Device,
                "resource" => ModuleType::Resource,
                "hidd" => ModuleType::Hidd,
                "datatype" => ModuleType::Datatype,
                "gadget" => ModuleType::Gadget,
                "mcc" => ModuleType::Mcc,
                _ => ModuleType::Custom,
            }
        };
        let genmodule_only =
            !headers_projection && is_explicit_genmodule_only(&inv.name, rest, mod_type_str);
        let linklib_name = match macro_arg(rest, "linklibname") {
            Some(raw) if !raw.is_empty() => match evaluate_name(&raw, &expression_context) {
                Ok(name) => Some(name),
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} linklibname={raw} is unresolved: {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                }
            },
            _ => None,
        };

        let arch_specific = match resolve_yes_argument(rest, "archspecific", scope, dirs, inv.line)
        {
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
        };
        let always_cxx_link =
            match resolve_yes_argument(rest, "alwayscxxlink", scope, dirs, inv.line) {
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
            };
        let target_dir = match resolve_module_target_dir(
            rest,
            scope,
            dirs,
            inv.line,
            mod_type_str,
            true,
            arch_specific,
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
        };
        let mod_suffix = match resolve_module_suffix(rest, scope, dirs, inv.line, mod_type_str) {
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
        };
        // An ABI skeleton has no implementation sources, and the one explicit
        // genmodule-only library is implemented entirely by generated start/end
        // files. Every other empty result keeps the existing strict source-list
        // handling: unresolved lists may never turn into generated-only modules.
        let (sources, source_inventory_only) = if is_abi || genmodule_only || headers_projection {
            (EvaluatedSources::default(), false)
        } else {
            let mut sources = match mesa26::module_sources(root, rel_dir, &mmake_name, target) {
                Ok(Some(sources)) => sources,
                Ok(None) => match evaluate_macro_sources(rest, &vars, &expression_context) {
                    Ok(sources) => sources,
                    Err(reason) => {
                        skipped_programs.push(format!(
                            "{}:{}: %{} mmake={mmake_raw} modname={mod_raw} {reason}",
                            rel_dir.display(),
                            inv.line + 1,
                            inv.name
                        ));
                        continue;
                    }
                },
                Err(reason) => {
                    capability_errors.push(capability_diagnostic_with_owner(
                        relative_path,
                        Some(inv.line + 1),
                        mmake_owner.as_deref(),
                        format!("Mesa 26 module source closure: {reason}"),
                    ));
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} Mesa 26 source closure rejected: {reason}",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
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
            let source_inventory_only =
                sources.is_empty() && !sources.deferred_wildcards.is_empty();
            if sources.is_empty() && !source_inventory_only {
                if sources.declared {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} modname={mod_raw} has an unresolved file list",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                }
                sources.c = wildcard_c_sources(parent_dir);
                if sources.is_empty() {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} modname={mod_raw} declares no sources",
                        rel_dir.display(),
                        inv.name
                    ));
                    continue;
                }
            }
            (sources, source_inventory_only)
        };

        let use_libs: Vec<String> = re_libs.captures(rest).map_or_else(Vec::new, |lcap| {
            let libs_str = lcap
                .get(1)
                .or_else(|| lcap.get(2))
                .map_or("", |m| m.as_str());
            expand_file_list(libs_str, &vars)
        });
        let declared_mod_type = matches!(
            module_type,
            ModuleType::Abi | ModuleType::Custom | ModuleType::ModuleHeaders
        )
        .then(|| mod_type_owned.clone());

        // `conffile=` names the genmodule config, and 81 of the 83 declarations
        // that state one give a file whose stem is not modname:
        // con_handler.conf for modname=con, VMM_Handler.conf for modname=VMM.
        // Without carrying it, CMake derives `<modname>.conf`, finds nothing and
        // generates no scaffolding at all -- silently, because a module with no
        // config is a legitimate hand-written one.
        let config_arg = macro_arg(&inv.args, "conffile");
        let config_file = config_arg.as_ref().and_then(|raw| {
            let raw = raw.trim().trim_matches('"');
            match evaluate_make_expr(raw, &expression_context) {
                Ok(value) => {
                    let value = value.trim().trim_matches('"').to_owned();
                    if value.is_empty() || value.contains(char::is_whitespace) {
                        skipped_programs.push(format!(
                            "{}: %{} mmake={mmake_raw} conffile={raw} is not one path",
                            rel_dir.display(),
                            inv.name
                        ));
                        None
                    } else if value.starts_with("${") || value.starts_with('/') {
                        Some(value)
                    } else {
                        // Relative to the declaring directory, as Make reads it.
                        Some(format!(
                            "${{AROS_SOURCE_DIR}}/{}/{value}",
                            rel_dir.display()
                        ))
                    }
                }
                Err(error) => {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} conffile={raw} cannot be \
                         evaluated: {error}",
                        rel_dir.display(),
                        inv.name
                    ));
                    None
                }
            }
        });
        if headers_projection && config_arg.is_some() && config_file.is_none() {
            continue;
        }
        let config_override_arg = macro_arg(&inv.args, "confoverride");
        let config_override_file = config_override_arg.as_ref().and_then(|raw| {
            let raw = raw.trim().trim_matches('"');
            match evaluate_make_expr(raw, &expression_context) {
                Ok(value) => {
                    let value = value.trim().trim_matches('"').to_owned();
                    if value.is_empty() || value.contains(char::is_whitespace) {
                        skipped_programs.push(format!(
                            "{}: %{} mmake={mmake_raw} confoverride={raw} is not one path",
                            rel_dir.display(),
                            inv.name
                        ));
                        None
                    } else if value.starts_with("${") || value.starts_with('/') {
                        Some(value)
                    } else {
                        Some(format!(
                            "${{AROS_SOURCE_DIR}}/{}/{value}",
                            rel_dir.display()
                        ))
                    }
                }
                Err(error) => {
                    skipped_programs.push(format!(
                        "{}: %{} mmake={mmake_raw} confoverride={raw} cannot be \
                         evaluated: {error}",
                        rel_dir.display(),
                        inv.name
                    ));
                    None
                }
            }
        });
        // An invalid override must not silently produce a module with the
        // wrong allocated base type or resident priority.
        if config_override_arg.is_some() && config_override_file.is_none() {
            continue;
        }

        // Upstream creates the client archive when `<mod>_LINKLIB` is
        // non-empty, and make.tmpl derives that from the file set, not from
        // `linklibname=`:
        //
        //   config/make.tmpl:2270  _LINKLIB is empty exactly when
        //                          _LINKLIBFILES, _LINKLIBAFILES,
        //                          linklibfiles= and _ARCHNLIBFILES are all
        //                          empty; linklibname= only renames it
        //   tools/genmodule/writemakefile.c:78
        //                          _LINKLIBFILES gets <mod>_getlibbase for
        //                          every LIBRARY, <mod>_autoinit under
        //                          OPTION_AUTOINIT and the stubs under
        //                          OPTION_STUBS
        //   tools/genmodule/config.c:797
        //                          a LIBRARY defaults to OPTION_AUTOINIT,
        //                          every other module type to NOAUTOINIT
        //
        // So every modtype=library module has a client archive, and so does
        // any other module whose config states `options stubs` or
        // `options autoinit` (rom/timer is the one such case in the tree).
        // Keying it on linklibname= left 100 library archives unbuilt, which
        // is what the symbol audit sees as undefined DOSBase, UtilityBase and
        // the rest: the base is defined by AROS_LIBSET in <mod>_autoinit.c
        // (compiler/include/aros/symbolsets.h:118), and that object lives in
        // exactly this archive.
        let source_path = |value: &str| {
            value
                .strip_prefix("${AROS_SOURCE_DIR}/")
                .map(|relative| root.join(relative))
                .or_else(|| {
                    Path::new(value)
                        .is_absolute()
                        .then(|| Path::new(value).to_path_buf())
                })
        };
        let config_path = config_file.as_deref().and_then(source_path);
        let override_path = config_override_file.as_deref().and_then(source_path);
        let config_facts = config_path.map_or_else(
            || read_genmodule_linklib_config(parent_dir, &mod_name, override_path.as_deref()),
            |config_path| {
                read_genmodule_linklib_config_files(&config_path, override_path.as_deref())
            },
        );
        // Only readable, exact source paths can enter the header projection.
        // Genmodule still validates their complete syntax and exported products;
        // this is not proof that the runtime or a client archive is buildable.
        if headers_projection && config_facts.is_none() {
            continue;
        }
        let config_relative_libraries = config_facts
            .as_ref()
            .map(|facts| facts.relative_libraries.clone())
            .unwrap_or_default();
        let genmodule_abi = inv.name != "build_module_library" && config_facts.is_some();
        if !matches!(module_type, ModuleType::Library | ModuleType::ModuleHeaders) {
            if let Some(facts) = config_facts.as_ref() {
                if facts.forces_client_archive {
                    skipped_client_archives.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} modname={mod_raw} modtype={mod_type_owned}: \
                         config states `options stubs` or `options autoinit`, so upstream builds \
                         lib{mod_name}.a; the generated client sources are only derived for \
                         modtype=library",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                }
            }
        }
        let genmodule_linklibs = if module_type == ModuleType::Library {
            config_facts.map(
                |GenmoduleConfigFacts {
                     has_relative,
                     relative_libraries,
                     forces_client_archive,
                 }| {
                    let mut inputs_exact = true;
                    let source_files = match evaluate_linklib_list(
                        rest,
                        "linklibfiles",
                        &vars,
                        &expression_context,
                    ) {
                        Ok(files) => files,
                        Err(error) => {
                            partial_source_lists.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {error}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            inputs_exact = false;
                            Vec::new()
                        }
                    };
                    let object_sources = match evaluate_linklib_list(
                        rest,
                        "linklibobjs",
                        &vars,
                        &expression_context,
                    ) {
                        Ok(objects) => match map_linklib_object_sources(&objects, &sources.c) {
                            Ok(mapped) => mapped,
                            Err(error) => {
                                partial_source_lists.push(format!(
                                    "{}:{}: %{} mmake={mmake_raw} {error}",
                                    rel_dir.display(),
                                    inv.line + 1,
                                    inv.name
                                ));
                                inputs_exact = false;
                                Vec::new()
                            }
                        },
                        Err(error) => {
                            partial_source_lists.push(format!(
                                "{}:{}: %{} mmake={mmake_raw} {error}",
                                rel_dir.display(),
                                inv.line + 1,
                                inv.name
                            ));
                            inputs_exact = false;
                            Vec::new()
                        }
                    };
                    GenmoduleLinklibs {
                        enabled: linklib_name.is_some()
                            || forces_client_archive
                            || module_type == ModuleType::Library
                            || !source_files.is_empty()
                            || !object_sources.is_empty(),
                        has_relative,
                        relative_libraries,
                        source_files,
                        object_sources,
                        inputs_exact,
                    }
                },
            )
        } else {
            None
        };

        // All three %build_module* forms expand the implicit MetaMake
        // aliases and architecture endpoints.  `genmodule_only` describes
        // only how sources are materialised; using it as a guard here made
        // ordinary sourceful modules lose their upstream prerequisite graph.
        let include_set = match macro_arg(rest, "include_set") {
            Some(raw) => {
                let Some(rendered) = render_meta_token(&raw) else {
                    skipped_programs.push(format!(
                        "{}:{}: %{} mmake={mmake_raw} include_set={raw} contains an unmapped Make variable",
                        rel_dir.display(),
                        inv.line + 1,
                        inv.name
                    ));
                    continue;
                };
                rendered
            }
            None => "includes-all".to_owned(),
        };
        if headers_projection {
            meta_rules.extend(implicit_module_header_meta_rules(
                &mmake_name,
                &mod_name,
                &include_set,
            ));
        } else {
            meta_rules.extend(implicit_module_meta_rules(
                &mmake_name,
                &mod_name,
                &include_set,
                &use_libs,
                inv.name != "build_module_library",
                inv.name != "build_module_abi",
                is_abi || genmodule_only,
            ));
        }

        let kobj_scoped_inputs = (!is_abi && !headers_projection)
            .then(|| capture_kobj_inputs(inv, &mod_name))
            .flatten();
        let parsed_target = TargetDefinition {
            mmake_name,
            target_name: mod_name,
            module_type,
            module_macro: Some(match inv.name.as_str() {
                "build_module" => ModuleMacroForm::Full,
                "build_module_library" => ModuleMacroForm::RuntimeOnly,
                "build_module_abi" => ModuleMacroForm::AbiOnly,
                _ => unreachable!("filtered to module-producing macro spellings"),
            }),
            kobj_scoped_inputs,
            genmodule_only,
            genmodule_abi,
            empty_archive: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
            no_startup: false,
            detach: false,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs: if headers_projection {
                Vec::new()
            } else {
                use_libs
            },
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit: false,
            declared_mod_type,
            mod_suffix,
            linklib_name,
            config_file,
            config_override_file,
            genmodule_linklibs,
            config_relative_libraries: if headers_projection {
                Vec::new()
            } else {
                config_relative_libraries
            },
            canonical_linklib_output: false,
            canonical_linklib_eligible: false,
            linklib_output_dir: None,
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
