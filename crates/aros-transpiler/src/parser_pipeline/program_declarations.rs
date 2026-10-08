//! `%build_prog` declarations.

use super::declaration_context::{DeclarationInputs, DeclarationOutputs};
use super::{
    collect_includes_at, declaration_flags_at, declaration_global_link_options,
    evaluate_macro_sources, evaluate_name, evaluate_output_directory, expand_file_list, macro_arg,
    record_partial_source_lists, resolve_no_argument, resolve_yes_argument, sanitize_ident,
    MakeExprContext, ModuleType, TargetDefinition,
};

pub(super) fn collect_programs(inputs: DeclarationInputs<'_>, outputs: DeclarationOutputs<'_>) {
    let DeclarationInputs {
        root,
        dirs,
        target,
        rel_dir,
        relative_path,
        joined,
        scope,
        flag_set,
        include_set,
        invocations,
        opts_arch_includes,
        opts_include_dirs,
        opts_link_options,
        opts_spec_switches,
        arch_defines,
        arch_compile_options,
        ..
    } = inputs;
    let DeclarationOutputs {
        targets,
        skipped_programs,
        unresolved_output_paths,
        partial_source_lists,
        source_inventory_patterns,
        source_inventory_needs,
        source_inventory_targets,
        ..
    } = outputs;
    // 2. Extract program definitions
    //
    // %build_prog takes progname=/A and builds one executable from all its
    // files (make.tmpl:1810). %build_progs takes files=/A and builds one per
    // file (make.tmpl:1850). Both used to match the same regex, progname was
    // never read, and every file became its own program: the four sources of
    // `%build_prog progname=SysLog` came out as colorlist, hooks, main and str
    // instead of one SysLog. Only %build_prog is handled here; %build_progs
    // needs one mmake target to carry several executables, which the target
    // model does not express yet, so it is reported instead of guessed at.
    for inv in invocations.iter().filter(|i| i.name == "build_prog") {
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
        let declaration_flags = declaration_flags_at(
            scope,
            inv.line,
            target,
            flag_set,
            opts_link_options,
            opts_spec_switches,
        );
        let declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(joined, scope, inv.line, rel_dir),
        );
        let mmake_name = sanitize_ident(&mmake_raw);

        // progname is declared /A, so a declaration without one is malformed
        // rather than something to guess a name for.
        let Some(prog_raw) = macro_arg(&inv.args, "progname") else {
            skipped_programs.push(format!(
                "{}: %build_prog mmake={mmake_raw} has no progname",
                rel_dir.display()
            ));
            continue;
        };
        let prog_name = match evaluate_name(&prog_raw, &expression_context) {
            Ok(name) => name,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} progname={prog_raw} is unresolved: {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                continue;
            }
        };

        let mut sources = match evaluate_macro_sources(&inv.args, &vars, &expression_context) {
            Ok(sources) => sources,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} progname={prog_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1
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
        let source_inventory_only = sources.is_empty() && !sources.deferred_wildcards.is_empty();
        if sources.is_empty() && !source_inventory_only {
            if sources.declared {
                // A list was given but its Make variables are unresolved.
                // Falling back to the program name here would compile the
                // wrong file, so report instead.
                skipped_programs.push(format!(
                    "{}: %build_prog mmake={mmake_raw} progname={prog_raw} has an unresolved file list",
                    rel_dir.display()
                ));
                continue;
            }
            sources.c.push(prog_name.clone());
        }

        let use_libs =
            macro_arg(&inv.args, "uselibs").map_or_else(Vec::new, |l| expand_file_list(&l, &vars));
        let always_cxx_link =
            match resolve_yes_argument(&inv.args, "alwayscxxlink", scope, dirs, inv.line) {
                Ok(value) => value,
                Err(reason) => {
                    skipped_programs.push(format!(
                        "{}:{}: %build_prog mmake={mmake_raw} {reason}",
                        rel_dir.display(),
                        inv.line + 1
                    ));
                    continue;
                }
            };
        let no_startup = match resolve_no_argument(&inv.args, "usestartup", scope, dirs, inv.line) {
            Ok(value) => value,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                continue;
            }
        };
        let detach = match resolve_yes_argument(&inv.args, "detach", scope, dirs, inv.line) {
            Ok(value) => value,
            Err(reason) => {
                skipped_programs.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                continue;
            }
        };
        let target_dir = match evaluate_output_directory(&inv.args, &expression_context) {
            Ok(directory) => directory,
            Err(reason) => {
                unresolved_output_paths.push(format!(
                    "{}:{}: %build_prog mmake={mmake_raw} {reason}",
                    rel_dir.display(),
                    inv.line + 1
                ));
                None
            }
        };

        let parsed_target = TargetDefinition {
            mmake_name,
            target_name: prog_name,
            module_type: ModuleType::Program,
            module_macro: None,
            kobj_scoped_inputs: None,
            genmodule_only: false,
            genmodule_abi: false,
            empty_archive: false,
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
            variant_32bit: false,
            declared_mod_type: None,
            mod_suffix: None,
            linklib_name: None,
            config_file: None,
            config_override_file: None,
            genmodule_linklibs: None,
            config_relative_libraries: Vec::new(),
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
