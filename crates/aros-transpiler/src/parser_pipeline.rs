//! End-to-end MetaMake file translation pipeline.

use super::{
    all_sources_are_fetch_owned, apply_mesa_compile_contract, capability_diagnostic,
    collect_arch_decls, collect_arch_sources, collect_copy_includes_with_scope,
    collect_fetches_with_scope, collect_flags, collect_flags_at, collect_flexcat_source_rules,
    collect_ilbm_sources, collect_includes, collect_includes_at, collect_make_opts, collect_vars,
    collect_vars_impl, collect_vars_impl_with_forward_locals, collector_forward_local_prelude,
    copy_directories, current_profile, declaration_flags_at, declaration_global_link_options,
    declaration_owned_port_scope, evaluate_linklib_list, evaluate_macro_sources,
    evaluate_macro_sources_with_files, evaluate_make_expr, evaluate_name,
    evaluate_output_directory, expand_file_list, expected_ahi_profile_exclusion,
    expected_grub_profile_exclusion, external_cmake, generators, implicit_module_meta_rules,
    inline_collector_make_includes, inline_local_make_includes, is_explicit_genmodule_only,
    join_continuations, join_mm_continuations, literal_defines, macro_arg,
    map_linklib_object_sources, merge_named_link_flags, read_genmodule_linklib_config, read_source,
    record_partial_source_lists, remaining_linklib_sources, render_meta_token,
    resolve_generated_linklib_sources, resolve_module_suffix, resolve_module_target_dir,
    resolve_yes_argument, safe_build_tree_output_directory, sanitize_ident,
    select_target_invocations, sse41, wildcard_c_sources, Diagnostic, EvaluatedSources, FetchDecl,
    GenmoduleConfigFacts, GenmoduleLinklibs, HashSet, LocalMakeFragmentPolicy,
    LocalMakeIncludeLimits, MakeExprContext, MetaTargetRule, ModuleType, ParsedMmakefile, Path,
    Regex, Result, TargetContext, TargetDefinition, META_RULE_RE, PRIVATE_LIBDIR,
};
use crate::capability::mesa::mesa20;

#[expect(
    clippy::too_many_lines,
    reason = "the fail-closed MetaMake translation is one ordered scope transaction; capability modules and a file-size gate bound it"
)]
pub(super) fn parse_mmakefile_impl(
    path: &Path,
    root: &Path,
    dirs: &crate::dirs::DirVars,
    target: Option<&TargetContext>,
    known_fetches: &[FetchDecl],
) -> Result<ParsedMmakefile> {
    let content = read_source(path)?;
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let rel_dir = parent_dir
        .strip_prefix(root)
        .unwrap_or(parent_dir)
        .to_path_buf();

    let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();

    // Fetch recipes expand after the complete file has been read. Their
    // collector owns the existing bounded include traversal and supplies the
    // ownership proof used by the declaration-specific port scope below.
    let mut collector_visited = HashSet::new();
    let collector_content =
        inline_collector_make_includes(&content, root, &rel_dir, &mut collector_visited, 8);
    let collector_joined = join_continuations(&collector_content);
    let collector_input = format!(
        "{}{}",
        collector_forward_local_prelude(&collector_joined),
        collector_joined
    );
    let collector_scope = target.map_or_else(
        || collect_vars(&collector_input),
        |target| collect_vars_impl(&collector_input, Some(target)).0,
    );
    let (fetches, skipped_fetches) =
        collect_fetches_with_scope(&content, &rel_dir, &collector_scope);
    let mut ownership_fetches = known_fetches.to_vec();
    ownership_fetches.extend(fetches.iter().cloned());

    // A small number of declarations keep a plain source inventory in a
    // sibling Make fragment. This remains the global default. A broader safe
    // variable scope is considered separately and adopted only when every
    // declaration is proven to compile sources owned by one of the fetches
    // above; there is deliberately no broad fallback.
    let plain_local_make_scan = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::PlainSourceLists,
    );
    let port_scope_candidate = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::SafeVariableScopes,
    );
    let port_scope_adopted = declaration_owned_port_scope(
        &plain_local_make_scan,
        &port_scope_candidate,
        target,
        dirs,
        root,
        &rel_dir,
        &ownership_fetches,
    );
    let define_scope_candidate = inline_local_make_includes(
        &content,
        root,
        &relative_path,
        LocalMakeIncludeLimits::default(),
        LocalMakeFragmentPolicy::LiteralDefineHeader,
    );
    let define_headers = (!port_scope_adopted)
        .then(|| {
            literal_defines::owned_scope(
                &plain_local_make_scan,
                &define_scope_candidate,
                target,
                dirs,
                root,
                &relative_path,
                &rel_dir,
                &content,
            )
        })
        .flatten()
        .unwrap_or_default();
    let define_scope_adopted = !define_headers.is_empty();
    let local_make_scan = if port_scope_adopted {
        port_scope_candidate
    } else if define_scope_adopted {
        define_scope_candidate
    } else {
        plain_local_make_scan
    };
    let skipped_local_make_includes = local_make_scan
        .issues
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    // Make evaluates ordinary build-macro arguments at their declaration
    // line, while `%fetch` recipes retain references until recipe execution
    // after the complete file has been read. Both use the same selected
    // conditional scope but deliberately query it at different positions.
    let joined = join_continuations(&local_make_scan.expanded);
    let (scope, conditional_line_states) = target.map_or_else(
        || (collect_vars(&joined), None),
        |target| {
            let (scope, states) = if port_scope_adopted {
                collect_vars_impl_with_forward_locals(&joined, Some(target), true)
            } else {
                collect_vars_impl(&joined, Some(target))
            };
            (scope, Some(states))
        },
    );
    let mut targets = Vec::new();
    let mut meta_rules = Vec::new();
    let mut skipped_meta_rules = Vec::new();

    // Include paths are a file-level property in Make: USER_INCLUDES applies to
    // every rule in the mmakefile, so the same set is attached to each target
    // parsed out of this file.
    let include_set = collect_includes(&content, &rel_dir);
    let arch_decls = collect_arch_decls(&content, &rel_dir);
    let mut copy_scan = collect_copy_includes_with_scope(&content, &rel_dir, &collector_scope);
    // USER_CPPFLAGS / USER_CFLAGS apply to every rule in the mmakefile, so the
    // same set is attached to each target parsed out of it.
    let mut flag_set = collect_flags(&content);
    let (packages, skipped_packages) = crate::packages::collect_packages(&content, &rel_dir);
    // Collected from `joined`, not from `content`: the declaration line has to
    // be in the same coordinate system as `scope`, which is built from the
    // joined and locally-included text. Read against the raw file the line
    // numbers drift with every continuation and every inlined fragment, so the
    // positional flag lookup below would read some other declaration's flags.
    let (mut arch_sources, skipped_arch_sources) = collect_arch_sources(&joined, &rel_dir, target);
    // A %build_archspecific file contributes to a target defined elsewhere, so
    // its own USER_INCLUDES and flags have to travel with the declaration.
    //
    // Read at the declaration's own line, not file-wide. One mmakefile can hold
    // several declarations with different flags:
    // arch/i386-all/hidd/gfx sets `USER_CFLAGS :=` before the baseline lane,
    // `$(HIDDGFX_SSE_CFLAGS)` before the SSE lane and `$(HIDDGFX_AVX_CFLAGS)`
    // before the AVX one. The file-wide value is whichever assignment happens to
    // win, and with it rgbconv_avx.c cannot compile at all.
    for d in &mut arch_sources {
        let at = collect_includes_at(&joined, &scope, d.line, &rel_dir);
        let flags = collect_flags_at(&scope, d.line);
        d.include_dirs = at.dirs;
        d.defines = flags.defines;
        d.compile_options = flags.compile_options;
    }
    // Architecture option files. Their contents are tagged with the
    // architecture they belong to, so CMake can keep the ones that apply; the
    // transpiler itself stays target-agnostic.
    let (opts_files, mut skipped_make_opts) = collect_make_opts(&content, &rel_dir, root);
    let active_tags = crate::make_opts::active_arch_tags(
        target.and_then(|target| target.platform.as_deref()),
        target.and_then(|target| target.cpu.as_deref()),
    );
    let mut undecidable_arch_link_options: Vec<String> = Vec::new();
    // Merged into every declaration's flags below rather than into `flag_set`:
    // with a target selected, declaration flags are re-collected positionally
    // from the mmakefile's own scope (collect_flags_at), so anything added to
    // `flag_set` here would be discarded. This is the same arrangement the
    // make.opts defines already use.
    let mut opts_link_options: Vec<String> = Vec::new();
    let mut opts_spec_switches: Vec<String> = Vec::new();
    let skipped_conditions = flag_set.skipped_conditions.clone();
    // Flags guarded by an `ifeq` on the CPU or platform are already tagged by
    // the flag collector; the make.opts contents are appended below.
    let mut arch_defines: Vec<(String, String)> = flag_set.arch_defines.clone();
    let mut arch_compile_options: Vec<(String, String)> = flag_set.arch_compile_options.clone();
    let mut opts_include_dirs: Vec<String> = Vec::new();
    let mut opts_arch_includes: Vec<(String, String)> = Vec::new();
    for f in &opts_files {
        let Ok(body) = read_source(&root.join(&f.path)) else {
            continue;
        };
        let opts_flags = collect_flags(&body);
        // Include paths from an option file are resolved against the including
        // mmakefile's directory, which is what Make does.
        let opts_incs = collect_includes(&body, &rel_dir);
        if let Some(tag) = &f.tag {
            for d in opts_flags.defines {
                arch_defines.push((tag.clone(), d));
            }
            for o in opts_flags.compile_options {
                arch_compile_options.push((tag.clone(), o));
            }
            for d in opts_incs.dirs {
                opts_arch_includes.push((tag.clone(), d));
            }
            // USER_LDFLAGS in a make.opts was read and then dropped, with
            // no report. arch/all-pc/kernel/make.opts:1 is
            //
            //   USER_LDFLAGS := -L$(GENDIR)/lib -lbootconsole -lacpica
            //
            // and without it kernel.resource leaves con_Putc, scr_Width,
            // the whole boot console and every Acpi* undefined. The
            // bootstrap loader forgives exactly one undefined symbol,
            // SysBase (bootstrap/elfloader.c:157), so that image cannot
            // load.
            //
            // Folded in rather than carried as a tagged lane, because the
            // graph has to see the `-L` to authorise a private archive:
            // libbootconsole.a lives in $(GENDIR)/lib, and
            // has_matching_private_link_archive compares that directory
            // against the consumer's own link options.
            if opts_flags.link_options.is_empty() {
            } else if active_tags.iter().any(|active| active == tag) {
                opts_link_options.extend(opts_flags.link_options);
                opts_spec_switches.extend(opts_flags.spec_switches);
            } else if target.is_none() {
                undecidable_arch_link_options.push(format!(
                    "{}: link options tagged {tag} cannot be decided without a target: {}",
                    f.path,
                    opts_flags.link_options.join(" ")
                ));
            }
        } else {
            // A local make.opts always applies.
            flag_set.defines.extend(opts_flags.defines);
            flag_set.compile_options.extend(opts_flags.compile_options);
            opts_link_options.extend(opts_flags.link_options);
            opts_spec_switches.extend(opts_flags.spec_switches);
            opts_include_dirs.extend(opts_incs.dirs);
        }
    }

    // Make evaluates a declaration's arguments where the declaration stands, so
    // the variable state is positional. Both scans read the same
    // continuation-joined text, which is what makes their line numbers
    // comparable.
    skipped_make_opts.extend(undecidable_arch_link_options);

    let icon_scan = crate::icons::collect_icons_all(&joined, dirs, &rel_dir);
    let catalog_scan = crate::catalogs::collect_catalogs_with_line_states(
        &joined,
        &scope,
        dirs,
        root,
        &rel_dir,
        conditional_line_states.as_deref(),
    );
    let mut skipped_programs: Vec<String> = Vec::new();
    let mut capability_errors: Vec<Diagnostic> = Vec::new();
    let invocations = select_target_invocations(
        &joined,
        conditional_line_states.as_deref(),
        &rel_dir,
        &mut skipped_programs,
    );
    // `%copy_dir_recursive` owns filesystem output, so unlike a generic
    // auxiliary macro it must not survive an inactive or unknown conditional.
    // Non-profiled parser callers still get a line-state scan: only their
    // unconditional declarations are safe to materialise.
    let fallback_copy_directory_states = conditional_line_states
        .is_none()
        .then(|| collect_vars_impl(&joined, None).1);
    let copy_directory_line_states = conditional_line_states
        .as_deref()
        .or(fallback_copy_directory_states.as_deref());
    let (copy_directories, skipped_copy_directories) = copy_directories::collect(
        &invocations,
        &scope,
        dirs,
        root,
        &rel_dir,
        copy_directory_line_states,
    );
    let mut external_cmake = Vec::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "build_with_cmake")
    {
        let expression_context =
            MakeExprContext::new(&scope, dirs, invocation.line, root, &rel_dir);
        match external_cmake::parse(
            invocation,
            &expression_context,
            &rel_dir,
            &fetches,
            target,
            &content,
        ) {
            Ok(declaration) => external_cmake.push(declaration),
            Err(reason) => {
                let mmake = macro_arg(&invocation.args, "mmake")
                    .map_or_else(String::new, |name| format!(" mmake={name}"));
                if matches!(
                    rel_dir.to_str(),
                    Some("compiler/cunit" | "workbench/classes/datatypes/heic")
                ) {
                    capability_errors.push(capability_diagnostic(
                        &relative_path,
                        Some(invocation.line + 1),
                        format!("%build_with_cmake{mmake} no longer matches its closed capability: {reason}"),
                    ));
                }
                skipped_programs.push(format!(
                    "{}:{}: %build_with_cmake{mmake} skipped: {reason}",
                    rel_dir.display(),
                    invocation.line + 1
                ));
            }
        }
    }
    let mut configure_builds = Vec::new();
    let mut grub_builds = Vec::new();
    let mut ahi_builds = Vec::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "build_with_configure")
    {
        match crate::capability::ahi::parse(root, invocation, &rel_dir, target) {
            Ok(Some(declaration)) => ahi_builds.push(declaration),
            Ok(None) => match crate::capability::grub2::parse(root, invocation, &rel_dir, target) {
                Ok(Some(declaration)) => grub_builds.push(declaration),
                Ok(None) => {
                    match crate::capability::configure::parse(root, invocation, &rel_dir, target) {
                        Ok(declaration) => configure_builds.push(declaration),
                        Err(reason) => {
                            let mmake = macro_arg(&invocation.args, "mmake")
                                .map_or_else(String::new, |name| format!(" mmake={name}"));
                            if matches!(
                                rel_dir.to_str(),
                                Some(
                                    "tools/ADFlib"
                                        | "workbench/network/WirelessManager/wpa_supplicant"
                                )
                            ) {
                                capability_errors.push(capability_diagnostic(
                                    &relative_path,
                                    Some(invocation.line + 1),
                                    format!("%build_with_configure{mmake} no longer matches its closed capability: {reason}"),
                                ));
                            }
                            skipped_programs.push(format!(
                                "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                                rel_dir.display(),
                                invocation.line + 1
                            ));
                        }
                    }
                }
                Err(reason) => {
                    let mmake = macro_arg(&invocation.args, "mmake")
                        .map_or_else(String::new, |name| format!(" mmake={name}"));
                    if !expected_grub_profile_exclusion(target) {
                        capability_errors.push(capability_diagnostic(
                            &relative_path,
                            Some(invocation.line + 1),
                            format!("%build_with_configure{mmake} no longer matches the closed GRUB2 capability: {reason}"),
                        ));
                    }
                    skipped_programs.push(format!(
                        "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                        rel_dir.display(),
                        invocation.line + 1
                    ));
                }
            },
            Err(reason) => {
                let mmake = macro_arg(&invocation.args, "mmake")
                    .map_or_else(String::new, |name| format!(" mmake={name}"));
                if !expected_ahi_profile_exclusion(target) {
                    capability_errors.push(capability_diagnostic(
                        &relative_path,
                        Some(invocation.line + 1),
                        format!("%build_with_configure{mmake} no longer matches the closed AHI capability: {reason}"),
                    ));
                }
                skipped_programs.push(format!(
                    "{}:{}: %build_with_configure{mmake} skipped: {reason}",
                    rel_dir.display(),
                    invocation.line + 1
                ));
            }
        }
    }
    let mut partial_source_lists: Vec<String> = Vec::new();
    let mut source_inventory_patterns: Vec<String> = Vec::new();
    let mut skipped_client_archives: Vec<String> = Vec::new();
    let mut unresolved_output_paths: Vec<String> = Vec::new();
    let re_libs = Regex::new(r#"uselibs=(?:"([^"]+)"|([^\s\\]+))"#).unwrap();

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
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let isa_link_options = declaration_global_link_options(
            "TARGET_ISA_LDFLAGS",
            &scope,
            dirs,
            root,
            &rel_dir,
            inv.line,
        );
        let driver_link_options =
            declaration_global_link_options("USER_LDFLAGS", &scope, dirs, root, &rel_dir, inv.line);
        let mut declaration_flags = declaration_flags_at(
            &scope,
            inv.line,
            target,
            &flag_set,
            &opts_link_options,
            &opts_spec_switches,
        );
        let mut declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
        );
        let mmake_name = sanitize_ident(&mmake_raw);
        if let Err(reason) = apply_mesa_compile_contract(
            &rel_dir,
            &mmake_name,
            target,
            &mut declaration_flags,
            &mut declaration_includes,
        ) {
            capability_errors.push(capability_diagnostic(
                &relative_path,
                Some(inv.line + 1),
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
        let mod_name = sanitize_ident(&mod_raw);
        let mod_type_owned = macro_arg(&inv.args, "modtype").unwrap_or_default();
        let mod_type_str = mod_type_owned.as_str();
        let rest = inv.args.as_str();
        let is_abi = inv.name == "build_module_abi";

        let module_type = if is_abi {
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
        let genmodule_only = is_explicit_genmodule_only(&inv.name, rest, mod_type_str);
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

        let arch_specific = match resolve_yes_argument(rest, "archspecific", &scope, dirs, inv.line)
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
            match resolve_yes_argument(rest, "alwayscxxlink", &scope, dirs, inv.line) {
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
            &scope,
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
        let mod_suffix = match resolve_module_suffix(rest, &scope, dirs, inv.line, mod_type_str) {
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
        let sources = if is_abi || genmodule_only {
            EvaluatedSources::default()
        } else {
            // The same source-list rules as every other build macro: the union
            // of all four lanes, with the reference's *.c default only when no
            // lane was declared (make.tmpl:2802).
            let mut sources = match evaluate_macro_sources(rest, &vars, &expression_context) {
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
            };
            record_partial_source_lists(
                &mut partial_source_lists,
                &mut source_inventory_patterns,
                &sources,
                &rel_dir,
                inv,
                &mmake_raw,
            );
            if sources.is_empty() {
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
            sources
        };

        let use_libs: Vec<String> = re_libs.captures(rest).map_or_else(Vec::new, |lcap| {
            let libs_str = lcap
                .get(1)
                .or_else(|| lcap.get(2))
                .map_or("", |m| m.as_str());
            expand_file_list(libs_str, &vars)
        });
        let declared_mod_type = matches!(module_type, ModuleType::Abi | ModuleType::Custom)
            .then(|| mod_type_owned.clone());

        // `conffile=` names the genmodule config, and 81 of the 83 declarations
        // that state one give a file whose stem is not modname:
        // con_handler.conf for modname=con, VMM_Handler.conf for modname=VMM.
        // Without carrying it, CMake derives `<modname>.conf`, finds nothing and
        // generates no scaffolding at all -- silently, because a module with no
        // config is a legitimate hand-written one.
        let config_file = macro_arg(&inv.args, "conffile").and_then(|raw| {
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
                            "${{CMAKE_SOURCE_DIR}}/{}/{value}",
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
        if module_type != ModuleType::Library {
            if let Some(facts) = read_genmodule_linklib_config(parent_dir, &mod_name) {
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
            read_genmodule_linklib_config(parent_dir, &mod_name).map(
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
        meta_rules.extend(implicit_module_meta_rules(
            &mmake_name,
            &mod_name,
            &include_set,
            &use_libs,
            inv.name != "build_module_library",
            inv.name != "build_module_abi",
            is_abi || genmodule_only,
        ));

        targets.push(TargetDefinition {
            mmake_name,
            target_name: mod_name,
            module_type,
            genmodule_only,
            empty_archive: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
            objc_source_files: sources.objc,
            asm_source_files: sources.asm,
            use_libs,
            dependencies: Vec::new(),
            dir_path: rel_dir.clone(),
            target_dir,
            link_libs: Vec::new(),
            variant_32bit: false,
            declared_mod_type,
            mod_suffix,
            linklib_name,
            config_file,
            genmodule_linklibs,
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
        });
    }

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
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let isa_link_options = declaration_global_link_options(
            "TARGET_ISA_LDFLAGS",
            &scope,
            dirs,
            root,
            &rel_dir,
            inv.line,
        );
        let driver_link_options =
            declaration_global_link_options("USER_LDFLAGS", &scope, dirs, root, &rel_dir, inv.line);
        let declaration_flags = declaration_flags_at(
            &scope,
            inv.line,
            target,
            &flag_set,
            &opts_link_options,
            &opts_spec_switches,
        );
        let declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
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
            &mut partial_source_lists,
            &mut source_inventory_patterns,
            &sources,
            &rel_dir,
            inv,
            &mmake_raw,
        );
        if sources.is_empty() {
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
            match resolve_yes_argument(&inv.args, "alwayscxxlink", &scope, dirs, inv.line) {
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

        targets.push(TargetDefinition {
            mmake_name,
            target_name: prog_name,
            module_type: ModuleType::Program,
            genmodule_only: false,
            empty_archive: false,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
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
            genmodule_linklibs: None,
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
        });
    }

    // 2b. The remaining build macros.
    //
    // All four share the compile model and differ only in what they link:
    // %build_prog one executable, %build_progs one per file, %build_linklib a
    // static library, %build_module_simple a module without the genmodule
    // chain. Only the link kind and the name argument change here.
    for inv in &invocations {
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
        let expression_context = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
        let isa_link_options = declaration_global_link_options(
            "TARGET_ISA_LDFLAGS",
            &scope,
            dirs,
            root,
            &rel_dir,
            inv.line,
        );
        let driver_link_options =
            declaration_global_link_options("USER_LDFLAGS", &scope, dirs, root, &rel_dir, inv.line);
        let mut declaration_flags = declaration_flags_at(
            &scope,
            inv.line,
            target,
            &flag_set,
            &opts_link_options,
            &opts_spec_switches,
        );
        let mut declaration_includes = target.map_or_else(
            || include_set.clone(),
            |_| collect_includes_at(&joined, &scope, inv.line, &rel_dir),
        );
        let mmake_name = sanitize_ident(&mmake_raw);
        let mesa20_capability_sources = match remaining_linklib_sources(
            root,
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic(
                        &relative_path,
                        Some(inv.line + 1),
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
        let nouveau_drm_capability_sources = match crate::capability::nouveau::drm_sources(
            root,
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic(
                        &relative_path,
                        Some(inv.line + 1),
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
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic(
                    &relative_path,
                    Some(inv.line + 1),
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
            &rel_dir,
            &mmake_name,
            target,
            &mut declaration_flags,
            &mut declaration_includes,
        ) {
            capability_errors.push(capability_diagnostic(
                &relative_path,
                Some(inv.line + 1),
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
        match crate::capability::nouveau::drm_compile_contract(&rel_dir, &mmake_name, target) {
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
                capability_errors.push(capability_diagnostic(
                    &relative_path,
                    Some(inv.line + 1),
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
        match crate::capability::nouveau::gallium_compile_contract(&rel_dir, &mmake_name, target) {
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
                capability_errors.push(capability_diagnostic(
                    &relative_path,
                    Some(inv.line + 1),
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
        let mesa_sse41_profile = (mmake_name == sse41::MMAKE
            && sse41::validate_static_contract(root, &content).is_ok())
        .then(|| sse41::profile(&rel_dir, target).ok().flatten())
        .flatten();
        let empty_archive = mesa_sse41_profile == Some(false);
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
                &scope,
                inv.line,
                &format!("{target_name}_LDFLAGS"),
            );
        }

        let resolved_generated_files = if module_type == ModuleType::LinkLib {
            match macro_arg(&inv.args, "files") {
                Some(files) => {
                    match resolve_generated_linklib_sources(&files, &joined, &rel_dir, |name| {
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
        let mut sources = if let Some(sources) = mesa20_capability_sources {
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
            &mut partial_source_lists,
            &mut source_inventory_patterns,
            &sources,
            &rel_dir,
            inv,
            &mmake_raw,
        );
        if sources.is_empty() && !empty_archive {
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
            match resolve_yes_argument(&inv.args, "alwayscxxlink", &scope, dirs, inv.line) {
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
        let target_dir = if is_simple_module {
            match resolve_module_target_dir(
                &inv.args,
                &scope,
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
                &scope,
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
            && (all_sources_are_fetch_owned(&sources, &fetches)
                || nouveau_drm_capability_active
                || nouveau_gallium_capability_active);
        let linklib_output_dir = if mesa_sse41_profile.is_some() || mesa20_capability_active {
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

        targets.push(TargetDefinition {
            mmake_name,
            target_name,
            module_type,
            genmodule_only: false,
            empty_archive,
            source_files: sources.c,
            cxx_source_files: sources.cxx,
            always_cxx_link,
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
            genmodule_linklibs: None,
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
        });
    }

    // %build_module_macro is invoked five times but defined nowhere in the
    // tree. Four of the five sit under arch/.unmaintained or an architecture
    // we do not build, and one carries a "converted without testing" note, so
    // the historic build cannot expand it either.
    for inv in invocations
        .iter()
        .filter(|i| i.name == "build_module_macro")
    {
        if let Some(m) = macro_arg(&inv.args, "mmake") {
            skipped_programs.push(format!(
                "{}: %build_module_macro mmake={m} (macro is not defined anywhere in the tree)",
                rel_dir.display()
            ));
        }
    }

    if let Err(reason) = sse41::validate(
        root,
        &rel_dir,
        target,
        &content,
        &targets,
        &ownership_fetches,
    ) {
        // The ordinary parser may have resolved part of this declaration, but
        // executable empty-archive support and the target-only ISA flag are
        // admitted as one atomic capability. Any drift removes the target.
        targets.retain(|candidate| candidate.mmake_name != sse41::MMAKE);
        capability_errors.push(capability_diagnostic(
            &relative_path,
            None,
            format!("Mesa SSE4.1 link library no longer matches its closed capability: {reason}"),
        ));
        skipped_programs.push(format!(
            "{}: Mesa SSE4.1 link library skipped: {reason}",
            rel_dir.display()
        ));
    }

    if targets
        .iter()
        .any(|candidate| candidate.mmake_name == crate::capability::nouveau::DRM_MMAKE)
    {
        if let Err(reason) =
            crate::capability::nouveau::validate_drm(root, &rel_dir, target, &targets)
        {
            // The DRM source fragment is intentionally admitted only as one
            // closed capability.  Do not leave a partially inferred target in
            // the graph when its recipe, inventory or canonical archive proof
            // has drifted.
            targets
                .retain(|candidate| candidate.mmake_name != crate::capability::nouveau::DRM_MMAKE);
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!(
                    "Nouveau DRM link library no longer matches its closed capability: {reason}"
                ),
            ));
            skipped_programs.push(format!(
                "{}: Nouveau DRM link library skipped: {reason}",
                rel_dir.display()
            ));
        }
    }

    if targets
        .iter()
        .any(|candidate| candidate.mmake_name == crate::capability::nouveau::GALLIUM_MMAKE)
    {
        if let Err(reason) =
            crate::capability::nouveau::validate_gallium(root, &rel_dir, target, &targets)
        {
            // The fetched Mesa lane contains a C++ source inventory. Keep it
            // atomic with its checked source and flag contract rather than
            // leaving an inferred C-only or private-output approximation in
            // the graph.
            targets.retain(|candidate| {
                candidate.mmake_name != crate::capability::nouveau::GALLIUM_MMAKE
            });
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!(
                    "Nouveau Gallium link library no longer matches its closed capability: {reason}"
                ),
            ));
            skipped_programs.push(format!(
                "{}: Nouveau Gallium link library skipped: {reason}",
                rel_dir.display()
            ));
        }
    }

    let mut python_outputs = Vec::new();
    match generators::parse_glapi(&rel_dir, target, &content, &targets, &ownership_fetches) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            capability_errors.push(capability_diagnostic(
                &relative_path,
                None,
                format!("Mesa glapi generator no longer matches its closed capability: {reason}"),
            ));
            skipped_programs.push(format!(
                "{}: Mesa glapi Python generator skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    match generators::parse_mesautil(&rel_dir, target, &content, &targets, &ownership_fetches) {
        Ok(Some(declaration)) => python_outputs.push(declaration),
        Ok(None) => {}
        Err(reason) => {
            capability_errors.push(capability_diagnostic(
                &relative_path,
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
    match mesa20::parse_remaining(
        root,
        &rel_dir,
        target,
        &content,
        &targets,
        &ownership_fetches,
    ) {
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
                &relative_path,
                None,
                format!(
                    "Mesa 20.0.8 archive/generator no longer matches its closed capability: {reason}"
                ),
            ));
            skipped_programs.push(format!(
                "{}: Mesa 20.0.8 archive/generator capability skipped: {reason}",
                rel_dir.display()
            ));
        }
    }
    match mesa20::parse_v3d(
        root,
        &rel_dir,
        target,
        &content,
        &targets,
        &ownership_fetches,
    ) {
        Ok(declarations) => python_outputs.extend(declarations),
        Err(reason) => {
            targets.retain(|candidate| candidate.mmake_name != "linklibs-gallium_v3d");
            capability_errors.push(capability_diagnostic(
                &relative_path,
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

    // Paired FlexCat recipes are normal Make rules rather than a MetaMake
    // macro.  Parse them after all concrete source lists are known, so the
    // graph can bind their generated `locale.c` only to real consumers.
    let flexcat_scan = collect_flexcat_source_rules(&content, root, &rel_dir, &scope, dirs);
    let ilbm_scan = collect_ilbm_sources(&content, root, &rel_dir, &scope, dirs);

    // 3. Extract #MM and #MM- meta-target rules
    let mm_content = join_mm_continuations(&content);
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

    // %rule_link_binary needs the file's targets, to check an explicit mmake=,
    // and the %build_archspecific object roots, which is how the reference
    // attaches an unnamed one.
    let known_target_names: Vec<String> = targets.iter().map(|t| t.mmake_name.clone()).collect();
    let arch_object_roots: Vec<(String, String, String)> = arch_sources
        .iter()
        .filter_map(|decl| {
            let maindir = decl.maindir.as_ref()?.trim_matches('/');
            let modname = decl.modname.as_ref()?;
            Some((
                format!("${{AROS_BUILD_DIR}}/gen/{maindir}/{modname}/arch"),
                decl.mainmmake.clone(),
                decl.tag.clone(),
            ))
        })
        .collect();
    let (host_generated_headers, skipped_host_generated_headers) =
        crate::host_generated_headers::collect_host_generated_headers(&content, &rel_dir);
    let (hidd_stubs, skipped_hidd_stubs) =
        crate::hidd_stubs::collect_hidd_stubs(&content, &scope, dirs, root, &rel_dir);
    let (binary_objects, skipped_binary_objects) = crate::binary_objects::collect_binary_objects(
        &content,
        &scope,
        dirs,
        root,
        &rel_dir,
        &known_target_names,
        &arch_object_roots,
    );

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
    copy_scan.generated_files.retain(|report| {
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

    Ok(ParsedMmakefile {
        capability_errors,
        targets,
        external_cmake,
        configure_builds,
        grub_builds,
        ahi_builds,
        python_outputs,
        flexcat_sources: flexcat_scan.declarations,
        flexcat_headers: flexcat_scan.headers,
        skipped_flexcat_sources: flexcat_scan.skipped,
        ilbm_sources: ilbm_scan.declarations,
        skipped_ilbm_sources: ilbm_scan.skipped,
        meta_rules,
        icon_targets: icon_scan.targets,
        icons: icon_scan.sets,
        skipped_icons: icon_scan.skipped,
        catalogs: catalog_scan.declarations,
        skipped_catalogs: catalog_scan.skipped,
        skipped_meta_rules,
        arch_decls,
        unresolved_includes: include_set.unresolved,
        copy_includes: copy_scan.decls,
        skipped_copy_includes: copy_scan.skipped,
        copy_directories,
        skipped_copy_directories,
        adhoc_header_rules: copy_scan.adhoc,
        header_transforms: copy_scan.transforms,
        bison_outputs: copy_scan.bison_outputs,
        define_headers,
        generated_file_rules: copy_scan.generated_files,
        script_outputs: copy_scan.script_outputs,
        skipped_script_outputs: copy_scan.skipped_script_outputs,
        flags: flag_set,
        arch_sources,
        skipped_arch_sources,
        binary_objects,
        skipped_binary_objects,
        hidd_stubs,
        skipped_hidd_stubs,
        host_generated_headers,
        skipped_host_generated_headers,
        fetches,
        skipped_fetches,
        skipped_make_opts,
        skipped_local_make_includes,
        skipped_conditions,
        skipped_programs,
        partial_source_lists,
        source_inventory_patterns,
        skipped_client_archives,
        unresolved_output_paths,
        packages,
        skipped_packages,
    })
}
