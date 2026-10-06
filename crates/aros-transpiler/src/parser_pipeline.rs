//! End-to-end MetaMake file translation pipeline.

use super::{
    all_sources_are_fetch_owned, apply_mesa_compile_contract, capability_diagnostic,
    capability_diagnostic_for_target, capability_diagnostics_for_targets, collect_arch_decls,
    collect_arch_sources, collect_copy_includes_with_scope, collect_fetches_with_scope_and_context,
    collect_flags, collect_flexcat_source_rules, collect_ilbm_sources, collect_includes,
    collect_includes_at, collect_make_opts, collect_vars, collect_vars_impl,
    collect_vars_impl_with_forward_locals, collector_forward_local_prelude, copy_directories,
    current_profile, declaration_flags_at, declaration_global_link_options,
    declaration_owned_port_scope, evaluate_linklib_list, evaluate_macro_sources,
    evaluate_macro_sources_with_files, evaluate_make_expr, evaluate_name,
    evaluate_output_directory, exact_mmake_target, expand_file_list,
    expected_ahi_profile_exclusion, expected_grub_profile_exclusion, external_cmake, generators,
    implicit_module_meta_rules, inline_collector_make_includes, inline_local_make_includes,
    is_concrete_build_invocation, is_explicit_genmodule_only, join_continuations, literal_defines,
    macro_arg, map_linklib_object_sources, merge_named_link_flags, read_genmodule_linklib_config,
    read_genmodule_linklib_config_files, read_source, record_partial_source_lists,
    remaining_linklib_sources, render_meta_token, resolve_generated_linklib_sources,
    resolve_module_suffix, resolve_module_target_dir, resolve_no_argument, resolve_yes_argument,
    safe_build_tree_output_directory, sanitize_ident, select_target_invocations, sse41,
    unique_mmake_owners, wildcard_c_sources, Diagnostic, EvaluatedSources, FetchDecl,
    GenmoduleConfigFacts, GenmoduleLinklibs, HashSet, Invocation, LocalMakeFragmentPolicy,
    LocalMakeIncludeLimits, MakeExprContext, MetaTargetRule, ModuleType, ParsedMmakefile, Path,
    Regex, Result, TargetContext, TargetDefinition, PRIVATE_LIBDIR,
};
use crate::capability::mesa::mesa26;

#[path = "parser_pipeline/build_macro_declarations.rs"]
mod build_macro_declarations;
#[path = "parser_pipeline/declaration_context.rs"]
mod declaration_context;
#[path = "parser_pipeline/module_declarations.rs"]
mod module_declarations;
#[path = "parser_pipeline/post_processing.rs"]
mod post_processing;
#[path = "parser_pipeline/program_declarations.rs"]
mod program_declarations;
#[path = "parser_pipeline/source_scope.rs"]
mod source_scope;

#[cfg(test)]
#[path = "parser_pipeline/literal_object_source_span_tests.rs"]
mod literal_object_source_span_tests;

use declaration_context::{DeclarationInputs, DeclarationOutputs};
pub use source_scope::architecture_scope_positions;
pub(super) use source_scope::invocation_owner_registry;
use source_scope::{
    capability_diagnostic_with_owner, collect_native_packages, literal_object_source_anchors,
    rejected_rule_owner_proofs, rejection_source_location, source_meta_provider_diagnostic,
    source_rejection_diagnostics,
};

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
    let (content, source_sha256) = aros_common::text::read_source_with_sha256(path)?;
    let disabled_meta_owners = crate::native_meta_providers::disabled_owners(&content);
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
    let (mut fetches, mut skipped_fetches) =
        collect_fetches_with_scope_and_context(&content, &rel_dir, &collector_scope, target);
    let llvm_capability =
        crate::capability::llvm::admit(root, &rel_dir, target).map_err(|message| {
            aros_common::ArosError::Configuration {
                file: relative_path.display().to_string(),
                message,
            }
        })?;
    if let Some((fetch, _)) = &llvm_capability {
        fetches.clear();
        fetches.push(fetch.clone());
        skipped_fetches.clear();
    }
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
    let mut copy_scan =
        collect_copy_includes_with_scope(&content, &rel_dir, &collector_scope, target);
    // USER_CPPFLAGS / USER_CFLAGS apply to every rule in the mmakefile, so the
    // same set is attached to each target parsed out of it.
    let mut flag_set = collect_flags(&content);
    let fallback_package_states = conditional_line_states
        .is_none()
        .then(|| collect_vars_impl(&joined, None).1);
    let package_line_states = conditional_line_states
        .as_deref()
        .or(fallback_package_states.as_deref())
        .expect("package scan has selected or conservative conditional states");
    // A selected native configuration binds includes such as a board's
    // rules file. Package members read through those bindings, so the
    // declaration is evaluated in the same native scope as the architecture
    // sources below; the classic scope above never saw the bound text.
    let native_package_context = target.filter(|context| {
        (!context.make_include_bindings.is_empty() || !context.generated_make_templates.is_empty())
            && (content.contains("%make_package") || content.contains("%link_kickstart"))
    });
    let (packages, skipped_packages) = native_package_context.map_or_else(
        || {
            crate::packages::collect_packages_with_scope(
                &joined,
                &rel_dir,
                &scope,
                dirs,
                root,
                package_line_states,
            )
        },
        |context| collect_native_packages(&content, context, dirs, root, &relative_path, &rel_dir),
    );
    // Collected from `joined`, not from `content`: the declaration line has to
    // be in the same coordinate system as `scope`, which is built from the
    // joined and locally-included text. Read against the raw file the line
    // numbers drift with every continuation and every inlined fragment, so the
    // positional flag lookup below would read some other declaration's flags.
    let native_arch_context = target.filter(|context| {
        !context.make_include_bindings.is_empty() || !context.generated_make_templates.is_empty()
    });
    let (arch_sources, skipped_arch_sources) = if let Some(context) =
        native_arch_context.filter(|_| content.contains("%build_archspecific"))
    {
        match crate::assembly_headers::native_configuration_snapshot(
            &content,
            context,
            dirs,
            root,
            &relative_path,
        ) {
            Ok(snapshot) => {
                let native_joined = snapshot.joined;
                let (native_scope, native_states) =
                    collect_vars_impl(&native_joined, Some(context));
                let (mut declarations, mut rejected) =
                    crate::arch_sources::collect_arch_sources_with_scope(
                        &native_joined,
                        &rel_dir,
                        Some(context),
                        &native_scope,
                        &native_states,
                        dirs,
                        root,
                    );
                crate::arch_sources::bind_declaration_context(
                    &mut declarations,
                    &native_joined,
                    &native_scope,
                    &rel_dir,
                )?;
                // Configuration insertion changes offsets, never ownership. A
                // declaration that does not start on a physical recipe line
                // (inserted configuration or a continuation tail) has no owner;
                // the file's architecture lanes are then rejected as a whole.
                let unowned = declarations
                    .iter()
                    .filter(|declaration| {
                        snapshot
                            .physical_owner_lines
                            .get(declaration.line)
                            .copied()
                            .flatten()
                            .is_none()
                    })
                    .map(|declaration| declaration.line + 1)
                    .collect::<Vec<_>>();
                if unowned.is_empty() {
                    for declaration in &mut declarations {
                        declaration.line = snapshot.physical_owner_lines[declaration.line]
                            .expect("ownership was checked above");
                    }
                    (declarations, rejected)
                } else {
                    rejected.push(format!(
                        "{}: %build_archspecific at native scope line(s) {} has no physical source owner; included configuration cannot declare architecture sources, fix upstream by moving the declaration into the recipe",
                        relative_path.display(),
                        unowned
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    (Vec::new(), rejected)
                }
            }
            Err(reason) => (
                Vec::new(),
                vec![format!(
                    "{}: native architecture context is unproven: {reason}",
                    relative_path.display()
                )],
            ),
        }
    } else {
        let (mut declarations, rejected) = collect_arch_sources(&joined, &rel_dir, target);
        crate::arch_sources::bind_declaration_context(
            &mut declarations,
            &joined,
            &scope,
            &rel_dir,
        )?;
        (declarations, rejected)
    };
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
    // Preserve source-declared owners before capability checks can reject and
    // remove their TargetDefinition from the translated graph.
    let invocation_owners = invocation_owner_registry(&invocations, &scope, dirs, root, &rel_dir);
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
    let (literal_header_copies, literal_header_copy_rejections) =
        crate::literal_header_copies::collect(
            &invocations,
            &scope,
            dirs,
            root,
            &rel_dir,
            copy_directory_line_states,
        );
    let mut native_graph_errors = Vec::new();
    copy_scan.transforms.extend(literal_header_copies);
    for rejection in literal_header_copy_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            None,
            rejection.owner.as_deref(),
            format!(
                "literal header copy is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let (static_header_copies, static_header_copy_rejections) =
        crate::static_header_copies::collect(
            &joined,
            &scope,
            dirs,
            root,
            &rel_dir,
            copy_directory_line_states,
        );
    copy_scan.transforms.extend(static_header_copies);
    for rejection in static_header_copy_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            None,
            rejection.owner.as_deref(),
            format!(
                "static header copy is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let (directory_setups, directory_setup_rejections) =
        crate::directory_setup::collect_directory_setups_with_context(
            &joined,
            &rel_dir,
            &scope,
            dirs,
            root,
            copy_directory_line_states,
        );
    for rejection in directory_setup_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            None,
            Some(&rejection.owner),
            format!(
                "directory setup recipe is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let genmodule_header_scan =
        crate::genmodule_header_rules::collect_genmodule_header_rules_with_context(
            &joined,
            root,
            &rel_dir,
            &scope,
            dirs,
            copy_directory_line_states,
        );
    for rejection in genmodule_header_scan.rejected {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            Some(rejection.line),
            Some(&rejection.owner),
            format!(
                "genmodule header stamp is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let genmodule_writefiles_scan =
        crate::genmodule_writefiles_rules::collect_genmodule_writefiles_rules_with_context(
            &joined,
            root,
            &rel_dir,
            &scope,
            dirs,
            copy_directory_line_states,
        );
    for rejection in genmodule_writefiles_scan.rejected {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            Some(rejection.line),
            Some(&rejection.owner),
            format!(
                "genmodule writefiles stamp is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let (host_header_rules, host_header_rule_rejections) =
        crate::host_header_rules::collect_host_header_rules(
            &joined,
            root,
            &rel_dir,
            &scope,
            dirs,
            copy_directory_line_states,
        );
    for rejection in host_header_rule_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            None,
            Some(&rejection.owner),
            format!(
                "host-C header rule is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let (mut sdk_text_rules, sdk_text_rejections) =
        crate::sdk_text_rules::collect_sdk_text_rules_with_physical_source(
            &joined,
            &local_make_scan.expanded,
            root,
            &rel_dir,
            &scope,
            dirs,
            copy_directory_line_states,
        );
    for rule in &mut sdk_text_rules {
        source_sha256.as_str().clone_into(&mut rule.file_sha256);
    }
    let sdk_text_anchors = (!sdk_text_rejections.is_empty())
        .then(|| literal_object_source_anchors(root, path, &content, &local_make_scan, &joined))
        .flatten();
    for rejection in sdk_text_rejections {
        let (diagnostic_path, diagnostic_line) =
            rejection_source_location(&relative_path, sdk_text_anchors.as_deref(), rejection.line);
        let mut diagnostic = capability_diagnostic_with_owner(
            diagnostic_path,
            diagnostic_line,
            Some(&rejection.owner),
            format!(
                "SDK text rule is outside its closed capability: {}",
                rejection.reason
            ),
        );
        if rejection.disabled_owner_only {
            diagnostic.context.get_or_insert_with(Default::default).mode =
                Some(crate::sdk_text_rules::DISABLED_OWNER_DIAGNOSTIC_MODE.to_owned());
        }
        native_graph_errors.push(diagnostic);
    }
    let (source_text_rules, source_text_rejections) =
        crate::source_text_rules::collect_source_text_rules_with_context(
            &joined,
            root,
            &rel_dir,
            &scope,
            dirs,
            copy_directory_line_states,
        );
    for rejection in source_text_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            Some(rejection.line),
            Some(&rejection.owner),
            format!(
                "source text rule is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let (mut source_value_rules, source_value_rejections) =
        crate::source_value_rules::collect_source_value_rules_with_context(
            &joined,
            root,
            &rel_dir,
            &scope,
            dirs,
            copy_directory_line_states,
        );
    for rule in &mut source_value_rules {
        source_sha256.as_str().clone_into(&mut rule.file_sha256);
    }
    for rejection in source_value_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            Some(rejection.line),
            Some(&rejection.owner),
            format!(
                "source value rule is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let (sdk_file_copies, sdk_file_copy_rejections) = crate::sdk_file_copies::collect_from_snapshot(
        &invocations,
        &scope,
        dirs,
        root,
        &rel_dir,
        copy_directory_line_states,
        &fetches,
        &joined,
    );
    for rejection in sdk_file_copy_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            Some(rejection.line),
            Some(&rejection.owner),
            format!(
                "SDK file copy is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let (sdk_asset_rules, sdk_asset_rejections) = crate::sdk_asset_rules::collect_from_snapshot(
        &joined,
        &scope,
        dirs,
        root,
        &rel_dir,
        copy_directory_line_states,
    );
    for rejection in sdk_asset_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            Some(rejection.line),
            Some(&rejection.owner),
            format!(
                "SDK asset rule is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    let (sdk_object_groups, sdk_object_rejections) = crate::sdk_objects::collect_from_snapshot(
        &invocations,
        &scope,
        dirs,
        root,
        &rel_dir,
        copy_directory_line_states,
        &joined,
    );
    // Literal recipes execute after all source assignments have been read.
    // Their native configuration is a separate, explicitly bound scope; it
    // must not silently expand the global configuration of unrelated macros.
    let literal_configuration =
        crate::local_make_includes::inline_native_make_configuration_with_templates(
            &content,
            root,
            &relative_path,
            LocalMakeIncludeLimits::default(),
            &target.map_or_else(std::collections::BTreeMap::new, |target| {
                target.make_include_bindings.clone()
            }),
            &target.map_or_else(std::collections::BTreeMap::new, |target| {
                target.generated_make_templates.clone()
            }),
        );
    let literal_joined = join_continuations(&literal_configuration.expanded);
    let (literal_scope, literal_states) = collect_vars_impl(&literal_joined, target);
    // A complete source-bound configuration can prove a copy macro's list
    // empty. Do not turn unknown variables or failed include expansion into
    // empty providers; the ordinary unresolved diagnostic remains otherwise.
    if literal_configuration.issues.is_empty() {
        let empty_copies = crate::copy_includes::collect_proven_empty(
            &literal_joined,
            &literal_scope,
            &literal_states,
            target,
            &rel_dir,
        );
        if !empty_copies.is_empty()
            && empty_copies.len()
                == super::macro_invocations(&literal_joined)
                    .iter()
                    .filter(|invocation| invocation.name == "copy_includes")
                    .count()
        {
            // Every source copy macro is now accounted for by this stronger
            // proof; retain skipped diagnostics whenever any other call exists.
            copy_scan.skipped.clear();
        }
        for declaration in empty_copies {
            // The closed scope proves the value at macro expansion time;
            // never let the legacy fallback borrow a later assignment.
            copy_scan
                .decls
                .retain(|existing| existing.name != declaration.name);
            copy_scan.decls.push(declaration);
        }
    }
    let (literal_object_groups, literal_object_rejections) =
        crate::literal_objects::collect_from_snapshot(
            &literal_scope,
            dirs,
            root,
            &rel_dir,
            Some(&literal_states),
            &literal_joined,
        );
    let literal_object_anchors = literal_object_source_anchors(
        root,
        &relative_path,
        &content,
        &literal_configuration,
        &literal_joined,
    );
    // Keep partial capabilities visible without laundering them into real
    // providers. Only the explicitly bound native scope may project roles.
    let mut source_archive_projections = Vec::new();
    let mut source_archive_commands = std::collections::BTreeMap::new();
    let mut source_compile_projections = Vec::new();
    let mut layered_header_projections = Vec::new();
    let mut source_header_pipelines = Vec::new();
    let mut source_directory_groups = std::collections::BTreeMap::new();
    if let Some(context) = target.filter(|context| {
        !context.make_include_bindings.is_empty() || !context.generated_make_templates.is_empty()
    }) {
        let (archives, archive_rejections) = crate::source_archive_rules::collect_from_snapshot(
            &literal_joined,
            &literal_scope,
            dirs,
            root,
            &rel_dir,
            Some(&literal_states),
        );
        let (compiles, compile_rejections) = crate::source_compile_rules::collect_from_snapshot(
            &literal_joined,
            &literal_scope,
            dirs,
            root,
            &rel_dir,
            Some(&literal_states),
            &archives,
        );
        source_archive_projections = archives;
        if !source_archive_projections.is_empty() {
            match crate::source_archive_command::prove(&literal_joined, &literal_scope, dirs, root)
            {
                Ok(command) => {
                    for archive in &source_archive_projections {
                        source_archive_commands.insert(
                            (archive.file.clone(), archive.owner.clone()),
                            command.clone(),
                        );
                    }
                }
                Err(reason) => {
                    for archive in &source_archive_projections {
                        native_graph_errors.push(capability_diagnostic_with_owner(
                            &relative_path,
                            None,
                            Some(&archive.owner),
                            format!("Source archive command is unproven: {reason}"),
                        ));
                    }
                }
            }
        }
        source_compile_projections = compiles;
        let (headers, header_rejections) = crate::layered_header_copies::collect(
            &literal_joined,
            &literal_scope,
            dirs,
            root,
            &rel_dir,
            context,
            Some(&literal_states),
        );
        layered_header_projections = headers;
        let (mut pipelines, pipeline_rejections) = crate::source_header_pipeline::collect(
            &literal_joined,
            &literal_scope,
            dirs,
            root,
            &rel_dir,
            Some(&literal_states),
        );
        for pipeline in &mut pipelines {
            let (path, line) = rejection_source_location(
                &relative_path,
                literal_object_anchors.as_deref(),
                pipeline.line,
            );
            pipeline.diagnostic_location = Some(aros_common::SourceLocation {
                path: path.to_string_lossy().into_owned(),
                line,
                column: None,
            });
            let (path, line) = rejection_source_location(
                &relative_path,
                literal_object_anchors.as_deref(),
                pipeline.sdk_rule_line,
            );
            pipeline.sdk_rule_location = Some(aros_common::SourceLocation {
                path: path.to_string_lossy().into_owned(),
                line,
                column: None,
            });
        }
        source_header_pipelines = pipelines;
        let (directory_groups, directory_rejections) =
            crate::source_directory_rules::collect_source_directory_groups(
                &literal_joined,
                &literal_scope,
                dirs,
                root,
                &rel_dir,
                &literal_states,
            );
        for group in directory_groups {
            source_directory_groups.insert(
                (
                    relative_path.to_string_lossy().into_owned(),
                    group.owner.clone(),
                ),
                group,
            );
        }
        for rejection in pipeline_rejections {
            let (path, line) = rejection_source_location(
                &relative_path,
                literal_object_anchors.as_deref(),
                rejection.line,
            );
            native_graph_errors.push(capability_diagnostic_with_owner(
                path,
                line,
                rejection.owner.as_deref(),
                format!("Source header pipeline is unproven: {}", rejection.reason),
            ));
        }
        // A local directory group is not a global `setup` target. Report
        // refusal against its proven aggregate consumer, if one exists.
        for rejection in directory_rejections {
            for aggregate in &layered_header_projections {
                if aggregate
                    .unresolved_prerequisites
                    .contains(&rejection.owner)
                {
                    let (path, line) = rejection_source_location(
                        &relative_path,
                        literal_object_anchors.as_deref(),
                        rejection.source_line,
                    );
                    native_graph_errors.push(capability_diagnostic_with_owner(
                        path,
                        line,
                        Some(&aggregate.owner),
                        format!(
                            "Source-local directory prerequisite {} is unproven: {}",
                            rejection.owner, rejection.reason
                        ),
                    ));
                }
            }
        }
        let rejections = archive_rejections
            .into_iter()
            .map(|rejection| {
                (
                    Some(rejection.owner),
                    rejection.line,
                    format!(
                        "Source archive is outside its closed capability: {}",
                        rejection.reason
                    ),
                )
            })
            .chain(compile_rejections.into_iter().map(|rejection| {
                (
                    Some(rejection.owner),
                    rejection.line,
                    format!(
                        "Source compile is outside its closed capability: {}",
                        rejection.reason
                    ),
                )
            }))
            .chain(header_rejections.into_iter().map(|rejection| {
                (
                    rejection.owner,
                    rejection.line,
                    format!(
                        "Layered header copies are outside their closed capability: {}",
                        rejection.reason
                    ),
                )
            }));
        for (owner, line, message) in rejections {
            let proofs = (owner.as_deref().is_none_or(|owner| owner == "<unknown>")
                && literal_configuration.issues.is_empty())
            .then(|| {
                crate::source_rule_ownership::attribute_rejected_rule_owners(
                    &literal_joined,
                    &literal_scope,
                    dirs,
                    root,
                    &rel_dir,
                    Some(&literal_states),
                    line,
                )
            })
            .flatten();
            let (path, line) =
                rejection_source_location(&relative_path, literal_object_anchors.as_deref(), line);
            native_graph_errors.extend(source_rejection_diagnostics(
                path,
                line,
                owner.as_deref(),
                message,
                proofs,
            ));
        }
    }
    let literal_owners: std::collections::BTreeSet<_> = literal_object_groups
        .iter()
        .map(|group| group.owner.as_str())
        .chain(
            literal_object_rejections
                .iter()
                .map(|rejection| rejection.owner.as_str()),
        )
        .collect();
    for rejection in sdk_object_rejections {
        // A literal compiler recipe is a distinct capability, not a failed
        // SDK compile/stage pair. Retain that capability's precise diagnostic.
        if literal_owners.contains(rejection.owner.as_str()) {
            continue;
        }
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            Some(rejection.line),
            Some(&rejection.owner),
            format!(
                "SDK object producer is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
    for rejection in literal_object_rejections {
        // Diagnostic ownership is not producer admission. Preserve unknown
        // identity unless the same complete source snapshot proves all owners
        // and excludes unknown/dynamic alternative consumers. A resolved
        // filesystem path is still not a MetaMake owner; try the same bounded
        // source-consumer proof for every non-canonical owner spelling.
        let ownership = rejected_rule_owner_proofs(
            &rejection.owner,
            literal_configuration.issues.is_empty(),
            (&literal_joined, Some(&literal_states)),
            &literal_scope,
            dirs,
            (root, &rel_dir),
            rejection.line,
        );
        let (diagnostic_path, diagnostic_line) = rejection_source_location(
            &relative_path,
            literal_object_anchors.as_deref(),
            rejection.line,
        );
        native_graph_errors.extend(source_rejection_diagnostics(
            diagnostic_path,
            diagnostic_line,
            Some(&rejection.owner),
            format!(
                "literal object producer is outside its closed capability: {}",
                rejection.reason
            ),
            ownership,
        ));
    }
    let (copy_directories, skipped_copy_directories) = copy_directories::collect(
        &invocations,
        &scope,
        dirs,
        root,
        &rel_dir,
        copy_directory_line_states,
    );
    let mut external_cmake = Vec::new();
    if let Some((_, declaration)) = &llvm_capability {
        external_cmake.push(declaration.clone());
    }
    for invocation in invocations
        .iter()
        .filter(|invocation| invocation.name == "build_with_cmake")
    {
        if llvm_capability.is_some()
            && macro_arg(&invocation.args, "mmake").as_deref() == Some("workbench-libs-llvm")
        {
            continue;
        }
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
                let mmake_raw = macro_arg(&invocation.args, "mmake");
                let mmake = mmake_raw
                    .as_ref()
                    .map_or_else(String::new, |name| format!(" mmake={name}"));
                let mmake_owner = mmake_raw
                    .as_deref()
                    .and_then(|raw| evaluate_name(raw, &expression_context).ok())
                    .and_then(|name| exact_mmake_target(&name));
                if matches!(
                    rel_dir.to_str(),
                    Some("compiler/cunit" | "workbench/classes/datatypes/heic")
                ) {
                    capability_errors.push(capability_diagnostic_with_owner(
                        &relative_path,
                        Some(invocation.line + 1),
                        mmake_owner.as_deref(),
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
        let expression_context =
            MakeExprContext::new(&scope, dirs, invocation.line, root, &rel_dir);
        let mmake_owner = macro_arg(&invocation.args, "mmake")
            .as_deref()
            .and_then(|raw| evaluate_name(raw, &expression_context).ok())
            .and_then(|name| exact_mmake_target(&name));
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
                                capability_errors.push(capability_diagnostic_with_owner(
                                    &relative_path,
                                    Some(invocation.line + 1),
                                    mmake_owner.as_deref(),
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
                        capability_errors.push(capability_diagnostic_with_owner(
                            &relative_path,
                            Some(invocation.line + 1),
                            mmake_owner.as_deref(),
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
                    capability_errors.push(capability_diagnostic_with_owner(
                        &relative_path,
                        Some(invocation.line + 1),
                        mmake_owner.as_deref(),
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
    let mut source_inventory_needs = Vec::new();
    let mut source_inventory_targets: Vec<crate::ast::InventoryTargetIdentity> = Vec::new();
    let mut skipped_client_archives: Vec<String> = Vec::new();
    let mut unresolved_output_paths: Vec<String> = Vec::new();
    let re_libs = Regex::new(r#"uselibs=(?:"([^"]+)"|([^\s\\]+))"#).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("internal uselibs matcher is invalid: {error}"),
        )
    })?;

    let capture_kobj_inputs = |inv: &Invocation, fallback_name: &str| {
        target.map(|target| {
            let expression_scope = MakeExprContext::new(&scope, dirs, inv.line, root, &rel_dir);
            let name = macro_arg(&inv.args, "modname")
                .ok_or_else(|| "missing source modname".to_owned())
                .and_then(|raw| {
                    evaluate_make_expr(&raw, &expression_scope).map_err(|error| error.to_string())
                });
            let flavour = macro_arg(&inv.args, "flavour")
                .map(|raw| {
                    evaluate_make_expr(&raw, &expression_scope).map_err(|error| error.to_string())
                })
                .transpose();
            let raw_uselibs = macro_arg(&inv.args, "uselibs");
            let raw_funcinstr = macro_arg(&inv.args, "funcinstr");
            let mut captured = crate::kobj_scoped_inputs::capture_kobj_scoped_inputs_for_module(
                &joined,
                inv.line,
                crate::kobj_scoped_inputs::KobjModuleArgs {
                    module_name: name.as_deref().unwrap_or(fallback_name),
                    flavour: flavour.as_ref().ok().and_then(|value| value.as_deref()),
                    raw_uselibs: raw_uselibs.as_deref(),
                    raw_funcinstr: raw_funcinstr.as_deref(),
                },
                root,
                &relative_path,
                dirs,
                target,
                &[],
            );
            let issue = name.err().or_else(|| flavour.err()).or_else(|| {
                (!skipped_local_make_includes.is_empty()).then(|| {
                    format!(
                        "unresolved local Make fragments: {}",
                        skipped_local_make_includes.join("; ")
                    )
                })
            });
            if let Some(reason) = issue {
                for input in [
                    &mut captured.user_objects,
                    &mut captured.defname_libs,
                    &mut captured.user_ldflags,
                    &mut captured.use_libs,
                    &mut captured.kobj_ldflags,
                    &mut captured.kernel_kobj_ldscript,
                    &mut captured.funcinstr_libs,
                    &mut captured.function_instrumentation,
                ] {
                    let (raw, source) = match input {
                        crate::kobj_scoped_inputs::ScopedMakeWords::KnownEmpty { raw, source }
                        | crate::kobj_scoped_inputs::ScopedMakeWords::Unresolved {
                            raw,
                            source,
                            ..
                        } => (raw.clone(), source.clone()),
                        crate::kobj_scoped_inputs::ScopedMakeWords::Exact {
                            raw, source, ..
                        } => (Some(raw.clone()), source.clone()),
                    };
                    *input = crate::kobj_scoped_inputs::ScopedMakeWords::Unresolved {
                        raw,
                        reason: format!("KOBJ declaration scope is incomplete: {reason}"),
                        source,
                    };
                }
            }
            captured
        })
    };

    // The three declaration passes run consecutively over one shared scope,
    // appending to the same accumulators in declaration order.
    let declaration_inputs = DeclarationInputs {
        root,
        dirs,
        target,
        rel_dir: &rel_dir,
        relative_path: &relative_path,
        parent_dir,
        content: &content,
        joined: &joined,
        scope: &scope,
        flag_set: &flag_set,
        include_set: &include_set,
        invocations: &invocations,
        fetches: &fetches,
        re_libs: &re_libs,
        opts_arch_includes: &opts_arch_includes,
        opts_include_dirs: &opts_include_dirs,
        opts_link_options: &opts_link_options,
        opts_spec_switches: &opts_spec_switches,
        arch_defines: &arch_defines,
        arch_compile_options: &arch_compile_options,
        capture_kobj_inputs: &capture_kobj_inputs,
    };
    let mut declaration_outputs = DeclarationOutputs {
        meta_rules: &mut meta_rules,
        targets: &mut targets,
        capability_errors: &mut capability_errors,
        skipped_programs: &mut skipped_programs,
        skipped_client_archives: &mut skipped_client_archives,
        unresolved_output_paths: &mut unresolved_output_paths,
        partial_source_lists: &mut partial_source_lists,
        source_inventory_patterns: &mut source_inventory_patterns,
        source_inventory_needs: &mut source_inventory_needs,
        source_inventory_targets: &mut source_inventory_targets,
    };
    module_declarations::collect_modules(declaration_inputs, declaration_outputs.reborrow());
    program_declarations::collect_programs(declaration_inputs, declaration_outputs.reborrow());
    build_macro_declarations::collect_build_macros(declaration_inputs, declaration_outputs);

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

    let sse41_result =
        if target.and_then(|profile| profile.mesa_version.as_deref()) == Some("26.0.0") {
            mesa26::validate_empty_sse41(root, &rel_dir, target, &targets, &ownership_fetches)
        } else {
            sse41::validate(
                root,
                &rel_dir,
                target,
                &content,
                &targets,
                &ownership_fetches,
            )
        };
    if let Err(reason) = sse41_result {
        // The ordinary parser may have resolved part of this declaration, but
        // executable empty-archive support and the target-only ISA flag are
        // admitted as one atomic capability. Any drift removes the target.
        let owners = unique_mmake_owners(&invocation_owners, &[sse41::MMAKE]).unwrap_or_default();
        targets.retain(|candidate| candidate.mmake_name != sse41::MMAKE);
        capability_errors.extend(capability_diagnostics_for_targets(
            &relative_path,
            None,
            owners,
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
            let owners = targets
                .iter()
                .filter(|candidate| candidate.mmake_name == crate::capability::nouveau::DRM_MMAKE)
                .map(|candidate| candidate.mmake_name.clone())
                .collect::<Vec<_>>();
            targets
                .retain(|candidate| candidate.mmake_name != crate::capability::nouveau::DRM_MMAKE);
            capability_errors.extend(capability_diagnostics_for_targets(
                &relative_path,
                None,
                owners,
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
            let owners = targets
                .iter()
                .filter(|candidate| {
                    candidate.mmake_name == crate::capability::nouveau::GALLIUM_MMAKE
                })
                .map(|candidate| candidate.mmake_name.clone())
                .collect::<Vec<_>>();
            targets.retain(|candidate| {
                candidate.mmake_name != crate::capability::nouveau::GALLIUM_MMAKE
            });
            capability_errors.extend(capability_diagnostics_for_targets(
                &relative_path,
                None,
                owners,
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

    let python_outputs =
        post_processing::collect_python_outputs(post_processing::PythonOutputContext {
            root,
            rel_dir: &rel_dir,
            relative_path: &relative_path,
            target,
            content: &content,
            invocation_owners: &invocation_owners,
            targets: &mut targets,
            ownership_fetches: &ownership_fetches,
            capability_errors: &mut capability_errors,
            skipped_programs: &mut skipped_programs,
        });

    // Paired FlexCat recipes are normal Make rules rather than a MetaMake
    // macro.  Parse them after all concrete source lists are known, so the
    // graph can bind their generated `locale.c` only to real consumers.
    let flexcat_scan = collect_flexcat_source_rules(&content, root, &rel_dir, &scope, dirs);
    let ilbm_scan = collect_ilbm_sources(&content, root, &rel_dir, &scope, dirs);

    let mut make_meta_providers = Vec::new();
    let implicit_meta_rule_count = meta_rules.len();
    native_graph_errors.extend(post_processing::collect_meta_rules_and_apply_llvm(
        &content,
        &rel_dir,
        target,
        &mut targets,
        &mut meta_rules,
        &mut skipped_meta_rules,
        &mut make_meta_providers,
    ));
    // This pass appends only handwritten #MM/#MM- declarations. Keep their
    // edge origin even when their dependencies duplicate generated aliases.
    let explicit_meta_rules = meta_rules[implicit_meta_rule_count..].to_vec();
    let sfd_header_scan = crate::sfd_header_rules::collect_sfd_header_rules_with_context(
        &joined,
        root,
        &rel_dir,
        &scope,
        dirs,
        copy_directory_line_states,
        &make_meta_providers,
    );
    for rejection in sfd_header_scan.rejected {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            Some(rejection.line),
            Some(&rejection.owner),
            format!(
                "SFD header rule is outside its closed capability: {}",
                rejection.reason
            ),
        ));
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
    let (host_header_aggregates, host_aggregate_rejections) =
        crate::host_header_aggregates::collect(
            &joined,
            &rel_dir,
            &scope,
            dirs,
            root,
            copy_directory_line_states,
        );
    for rejection in host_aggregate_rejections {
        native_graph_errors.push(capability_diagnostic_with_owner(
            &relative_path,
            None,
            Some(&rejection.owner),
            format!(
                "host-header aggregate is outside its closed capability: {}",
                rejection.reason
            ),
        ));
    }
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

    post_processing::filter_generated_file_templates(
        &mut copy_scan.generated_files,
        &python_outputs,
    );

    let sdk_program_outputs = dirs
        .expand("$(AROS_DEVELOPER)/bin")
        .map(|bin| {
            let mut outputs = Vec::new();
            for target in &targets {
                if target.module_type == ModuleType::Program
                    && target.target_dir.as_ref() == Some(&bin)
                {
                    outputs.push(crate::graph::SdkProgramOutput {
                        owner: target.mmake_name.clone(),
                        output: format!("${{AROS_DEVELOPER_BIN_DIR}}/{}", target.target_name),
                        directory: target.dir_path.clone(),
                    });
                }
            }
            for target in &source_inventory_targets {
                if target.module_type == ModuleType::Program
                    && target.target_dir.as_ref() == Some(&bin)
                {
                    outputs.push(crate::graph::SdkProgramOutput {
                        owner: target.mmake_name.clone(),
                        output: format!("${{AROS_DEVELOPER_BIN_DIR}}/{}", target.target_name),
                        directory: target.dir_path.clone(),
                    });
                }
            }
            outputs.sort_by(|a, b| (&a.owner, &a.output).cmp(&(&b.owner, &b.output)));
            outputs.dedup();
            outputs
        })
        .unwrap_or_default();
    // The scope joins logical values but retains physical source line ids.
    // Neither inlined input positions nor ambient values qualify an effect.
    let effect_scan = crate::arch_endpoint_effects::collect_source_arch_endpoint_effects(
        &content,
        &relative_path,
        root,
        target,
    )
    .map_err(|message| aros_common::ArosError::Configuration {
        file: relative_path.display().to_string(),
        message,
    })?;
    let (assembly_headers, assembly_header_rejections) = target
        .filter(|context| {
            !context.make_include_bindings.is_empty()
                || !context.generated_make_templates.is_empty()
        })
        .map(|context| {
            crate::assembly_headers::collect_from_snapshot(&content, context, dirs, root, &rel_dir)
        })
        .unwrap_or_default();
    let mut parsed = ParsedMmakefile {
        assembly_headers,
        assembly_header_rejections,
        arch_endpoint_effects: effect_scan.effects,
        arch_endpoint_rejections: effect_scan.rejected,
        source_sha256: Some(source_sha256.as_str().to_owned()),
        disabled_meta_owners,
        host_header_aggregates,
        directory_setups,
        genmodule_header_rules: genmodule_header_scan.declarations,
        genmodule_writefiles_rules: genmodule_writefiles_scan.declarations,
        host_file_generators: Vec::new(),
        host_header_rules,
        sdk_text_rules,
        sfd_header_rules: sfd_header_scan.declarations,
        source_text_rules,
        source_value_rules,
        sdk_file_copies,
        sdk_asset_rules,
        sdk_program_outputs,
        sdk_object_groups,
        literal_object_groups,
        source_archive_projections,
        source_archive_commands,
        source_compile_projections,
        layered_header_projections,
        source_header_pipelines,
        source_directory_groups,
        capability_errors,
        native_graph_errors,
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
        explicit_meta_rules,
        make_meta_providers,
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
        source_inventory_needs,
        source_inventory_targets,
        skipped_client_archives,
        unresolved_output_paths,
        packages,
        skipped_packages,
    };
    if let Some(target) = target {
        for declaration in target
            .host_file_generators
            .iter()
            .filter(|declaration| Path::new(&declaration.recipe) == relative_path)
        {
            match crate::host_c_file_rules::validate_source_rule(
                &joined, root, &rel_dir, declaration, copy_directory_line_states,
            ) {
                Ok(()) => parsed.host_file_generators.push(declaration.clone()),
                Err(reason) => parsed.native_graph_errors.push(capability_diagnostic_with_owner(
                    &relative_path, None, Some(&declaration.owner),
                    format!("source-owned host-C file generator is outside its closed capability: {reason}"),
                )),
            }
        }
    }
    for rejection in crate::native_meta_providers::validate(&parsed, target) {
        parsed
            .native_graph_errors
            .push(source_meta_provider_diagnostic(
                &relative_path,
                &rejection.owner,
                rejection.reason,
            ));
    }
    Ok(parsed)
}
