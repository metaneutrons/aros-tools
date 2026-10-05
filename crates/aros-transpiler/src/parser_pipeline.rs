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
use crate::ast::ModuleMacroForm;
use crate::capability::mesa::mesa26;
use crate::module_paths::implicit_module_header_meta_rules;

#[path = "parser_pipeline/post_processing.rs"]
mod post_processing;

fn capability_diagnostic_with_owner(
    relative_path: &Path,
    line: Option<usize>,
    owner: Option<&str>,
    message: impl Into<String>,
) -> Diagnostic {
    match owner {
        Some(owner) => capability_diagnostic_for_target(relative_path, line, owner, message),
        None => capability_diagnostic(relative_path, line, message),
    }
}

/// A rejected source-written MetaMake provider may retain its already
/// rendered selector spelling. This proves ownership, not executability;
/// graph selection must still bind every selector or reject the endpoint.
fn source_meta_provider_diagnostic(
    relative_path: &Path,
    owner: &str,
    reason: String,
) -> Diagnostic {
    let mut residual = owner.to_owned();
    for selector in [
        "AROS_TARGET_CPU",
        "AROS_TARGET_PLATFORM",
        "AROS_TARGET_LEGACY_PLATFORM",
        "AROS_TARGET_FAMILY",
        "AROS_TARGET_VARIANT",
        "AROS_TARGET_CPU32",
    ] {
        residual = residual.replace(&format!("${{{selector}}}"), "selector");
    }
    let mut diagnostic = capability_diagnostic(relative_path, None, reason);
    if !residual.is_empty()
        && residual
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        diagnostic = diagnostic.with_context(aros_common::DiagnosticContext {
            target: Some(owner.to_owned()),
            ..Default::default()
        });
    }
    diagnostic
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LiteralObjectSourceAnchor {
    source: std::path::PathBuf,
    line: usize,
}

fn rejection_source_location<'a>(
    fallback: &'a Path,
    anchors: Option<&'a [Option<LiteralObjectSourceAnchor>]>,
    joined_line: usize,
) -> (&'a Path, Option<usize>) {
    joined_line
        .checked_sub(1)
        .and_then(|index| anchors.and_then(|anchors| anchors.get(index)))
        .and_then(Option::as_ref)
        .map_or((fallback, None), |anchor| {
            (anchor.source.as_path(), Some(anchor.line))
        })
}

fn source_rejection_diagnostics(
    path: &Path,
    line: Option<usize>,
    fallback_owner: Option<&str>,
    message: String,
    proofs: Option<Vec<crate::source_rule_ownership::SourceRuleOwnership>>,
) -> Vec<Diagnostic> {
    match proofs {
        Some(proofs) if !proofs.is_empty() => proofs
            .into_iter()
            .map(|proof| {
                capability_diagnostic_with_owner(
                    path,
                    line,
                    Some(&proof.owner),
                    format!(
                        "{message}; exact source consumer chain: {}",
                        proof.chain.join(" -> ")
                    ),
                )
            })
            .collect(),
        _ => vec![capability_diagnostic_with_owner(
            path,
            line,
            fallback_owner,
            message,
        )],
    }
}

fn rejected_rule_owner_proofs(
    owner: &str,
    configuration_is_complete: bool,
    snapshot: (&str, Option<&[crate::make_vars::ConditionalTruth]>),
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    source_location: (&Path, &Path),
    line: usize,
) -> Option<Vec<crate::source_rule_ownership::SourceRuleOwnership>> {
    if !configuration_is_complete || exact_mmake_target(owner).is_some() {
        return None;
    }
    crate::source_rule_ownership::attribute_rejected_rule_owners(
        snapshot.0,
        scope,
        dirs,
        source_location.0,
        source_location.1,
        snapshot.1,
        line,
    )
}

#[derive(Default)]
struct ReconstructedLiteralSource {
    text: String,
    physical_anchors: Vec<Option<LiteralObjectSourceAnchor>>,
}

/// Reconstructs the exact native-configuration expansion while retaining the
/// original file and physical line for every emitted line. A mismatch means
/// the include provenance cannot safely support a source location.
fn literal_object_source_anchors(
    root: &Path,
    source: &Path,
    source_text: &str,
    scan: &crate::local_make_includes::LocalMakeIncludeScan,
    joined_snapshot: &str,
) -> Option<Vec<Option<LiteralObjectSourceAnchor>>> {
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let mut next_fragment = 0;
    let reconstructed = reconstruct_literal_source(
        &canonical_root,
        source,
        source_text,
        &scan.fragments,
        &mut next_fragment,
    )?;
    if next_fragment != scan.fragments.len() || reconstructed.text != scan.expanded {
        return None;
    }

    let joined = join_continuations(&reconstructed.text);
    if joined != joined_snapshot {
        return None;
    }

    let physical_lines = reconstructed.text.split_inclusive('\n').collect::<Vec<_>>();
    if physical_lines.len() != reconstructed.physical_anchors.len() {
        return None;
    }

    // Keep this expression in lockstep with parser::join_continuations. It
    // tells us which adjacent physical lines became one parser line; the
    // joined bytes themselves are independently checked above.
    let continuation = Regex::new(r"\\[ \t]*\r?\n[ \t]*").ok()?;
    let mut logical_anchors = Vec::new();
    for (index, physical_line) in physical_lines.iter().enumerate() {
        if index == 0 {
            logical_anchors.push(reconstructed.physical_anchors[index].clone());
            continue;
        }
        let previous_line = physical_lines[index - 1];
        let boundary = previous_line.len();
        let mut pair = String::with_capacity(boundary + physical_line.len());
        pair.push_str(previous_line);
        pair.push_str(physical_line);
        let is_continuation = continuation
            .find_iter(&pair)
            .any(|matched| matched.start() < boundary && matched.end() >= boundary);
        if !is_continuation {
            logical_anchors.push(reconstructed.physical_anchors[index].clone());
        }
    }
    if logical_anchors.len() != joined_snapshot.lines().count() {
        return None;
    }
    Some(logical_anchors)
}

fn reconstruct_literal_source(
    root: &Path,
    source: &Path,
    source_text: &str,
    fragments: &[crate::local_make_includes::IncludedLocalMakeFragment],
    next_fragment: &mut usize,
) -> Option<ReconstructedLiteralSource> {
    let mut reconstructed = ReconstructedLiteralSource::default();
    for (index, chunk) in source_text.split_inclusive('\n').enumerate() {
        let line = index + 1;
        let fragment = fragments.get(*next_fragment);
        if let Some(fragment) = fragment {
            if fragment.included_from == source && fragment.include_line < line {
                return None;
            }
            if fragment.included_from == source && fragment.include_line == line {
                if fragment.path.is_absolute()
                    || fragment
                        .path
                        .components()
                        .any(|component| !matches!(component, std::path::Component::Normal(_)))
                {
                    return None;
                }
                let fragment_path = fragment.path.clone();
                *next_fragment += 1;
                let resolved_fragment_path = root.join(&fragment_path);
                if std::fs::canonicalize(&resolved_fragment_path)
                    .ok()
                    .as_deref()
                    != Some(resolved_fragment_path.as_path())
                    || !std::fs::metadata(&resolved_fragment_path).ok()?.is_file()
                {
                    return None;
                }
                reconstructed
                    .text
                    .push_str(if fragment.generated_output.is_some() {
                        "# Verified source configuration template\n"
                    } else {
                        "# Verified source configuration include\n"
                    });
                reconstructed.physical_anchors.push(None);

                let mut fragment_text = std::fs::read_to_string(&resolved_fragment_path).ok()?;
                if let Some(substitutions) = &fragment.template_substitutions {
                    fragment.generated_output.as_ref()?;
                    for (token, value) in substitutions {
                        if token.is_empty() || !fragment_text.contains(token) {
                            return None;
                        }
                        fragment_text = fragment_text.replace(token, value);
                    }
                }
                let expanded = reconstruct_literal_source(
                    root,
                    &fragment_path,
                    &fragment_text,
                    fragments,
                    next_fragment,
                )?;
                let has_text = !expanded.text.is_empty();
                let ends_with_newline = expanded.text.ends_with('\n');
                reconstructed.text.push_str(&expanded.text);
                reconstructed
                    .physical_anchors
                    .extend(expanded.physical_anchors);
                if has_text && !ends_with_newline {
                    reconstructed.text.push('\n');
                }
                continue;
            }
        }
        reconstructed.text.push_str(chunk);
        reconstructed
            .physical_anchors
            .push(Some(LiteralObjectSourceAnchor {
                source: source.to_path_buf(),
                line,
            }));
    }
    if fragments
        .get(*next_fragment)
        .is_some_and(|fragment| fragment.included_from == source)
    {
        return None;
    }
    Some(reconstructed)
}

pub(super) fn invocation_owner_registry(
    invocations: &[Invocation],
    scope: &crate::make_vars::VarScope,
    dirs: &crate::dirs::DirVars,
    root: &Path,
    rel_dir: &Path,
) -> std::collections::BTreeMap<String, usize> {
    let mut owners = std::collections::BTreeMap::new();
    for invocation in invocations
        .iter()
        .filter(|invocation| is_concrete_build_invocation(&invocation.name))
    {
        let Some(raw) = macro_arg(&invocation.args, "mmake") else {
            continue;
        };
        let expression_context = MakeExprContext::new(scope, dirs, invocation.line, root, rel_dir);
        let Some(owner) = evaluate_name(&raw, &expression_context)
            .ok()
            .and_then(|name| exact_mmake_target(&name))
        else {
            continue;
        };
        *owners.entry(owner).or_insert(0) += 1;
    }
    owners
}

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
    let (packages, skipped_packages) = crate::packages::collect_packages_with_scope(
        &joined,
        &rel_dir,
        &scope,
        dirs,
        root,
        package_line_states,
    );
    // Collected from `joined`, not from `content`: the declaration line has to
    // be in the same coordinate system as `scope`, which is built from the
    // joined and locally-included text. Read against the raw file the line
    // numbers drift with every continuation and every inlined fragment, so the
    // positional flag lookup below would read some other declaration's flags.
    let (mut arch_sources, skipped_arch_sources) = collect_arch_sources(&joined, &rel_dir, target);
    crate::arch_sources::bind_declaration_context(&mut arch_sources, &joined, &scope, &rel_dir)?;
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
        let mmake_owner = exact_mmake_target(&mmake_raw);
        let headers_projection = if let Err(reason) = if inv.name == "build_module_abi" {
            Ok(false)
        } else {
            apply_mesa_compile_contract(
                &rel_dir,
                &mmake_name,
                target,
                &mut declaration_flags,
                &mut declaration_includes,
            )
        } {
            capability_errors.push(capability_diagnostic_with_owner(
                &relative_path,
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
            match mesa26::runtime_module_name(root, &rel_dir, &mmake_name, &mod_raw, target) {
                Ok(Some(name)) => name,
                Ok(None) => sanitize_ident(&mod_raw),
                Err(reason) => {
                    capability_errors.push(capability_diagnostic_with_owner(
                        &relative_path,
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
        let (sources, source_inventory_only) = if is_abi || genmodule_only || headers_projection {
            (EvaluatedSources::default(), false)
        } else {
            let mut sources = match mesa26::module_sources(root, &rel_dir, &mmake_name, target) {
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
                        &relative_path,
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
                &mut partial_source_lists,
                &mut source_inventory_patterns,
                &mut source_inventory_needs,
                &sources,
                &relative_path,
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
        };
        if source_inventory_only {
            source_inventory_targets.push((&parsed_target).into());
        } else {
            targets.push(parsed_target);
        }
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
            &mut source_inventory_needs,
            &sources,
            &relative_path,
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
        let no_startup = match resolve_no_argument(&inv.args, "usestartup", &scope, dirs, inv.line)
        {
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
        let detach = match resolve_yes_argument(&inv.args, "detach", &scope, dirs, inv.line) {
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
        };
        if source_inventory_only {
            source_inventory_targets.push((&parsed_target).into());
        } else {
            targets.push(parsed_target);
        }
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
        let mmake_owner = exact_mmake_target(&mmake_raw);
        let mesa20_capability_sources = match remaining_linklib_sources(
            root,
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                        &relative_path,
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
        let mesa26_sources = match mesa26::archive_sources(root, &rel_dir, &mmake_name, target) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                    &relative_path,
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
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                        &relative_path,
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
            &rel_dir,
            &mmake_name,
            target,
        ) {
            Ok(sources) => sources,
            Err(reason) => {
                capability_errors.push(capability_diagnostic_with_owner(
                    &relative_path,
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
            &rel_dir,
            &mmake_name,
            target,
            &mut declaration_flags,
            &mut declaration_includes,
        ) {
            capability_errors.push(capability_diagnostic_with_owner(
                &relative_path,
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
                capability_errors.push(capability_diagnostic_with_owner(
                    &relative_path,
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
                capability_errors.push(capability_diagnostic_with_owner(
                    &relative_path,
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
            && sse41::validate_static_contract(root, &content).is_ok())
        .then(|| sse41::profile(&rel_dir, target).ok().flatten())
        .flatten();
        let empty_archive = mesa26_empty_sse41 || mesa_sse41_profile == Some(false);
        if mesa26_empty_sse41 {
            let Ok(Some(contract)) = mesa26::compile_contract(&rel_dir, &mmake_name, target) else {
                capability_errors.push(capability_diagnostic_with_owner(
                    &relative_path,
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
            &mut partial_source_lists,
            &mut source_inventory_patterns,
            &mut source_inventory_needs,
            &sources,
            &relative_path,
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
        let no_startup = if is_program_group {
            match resolve_no_argument(&inv.args, "usestartup", &scope, dirs, inv.line) {
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
            match resolve_yes_argument(&inv.args, "detach", &scope, dirs, inv.line) {
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
        };
        if source_inventory_only {
            source_inventory_targets.push((&parsed_target).into());
        } else {
            targets.push(parsed_target);
        }
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
    let mut parsed = ParsedMmakefile {
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
    for rejection in crate::native_meta_providers::validate(&parsed) {
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

#[cfg(test)]
mod literal_object_source_span_tests {
    #[test]
    fn all_proven_owners_get_diagnostics_and_unknown_proof_is_not_suppressed() {
        let path = std::path::Path::new("arch/example/mmakefile.src");
        let proofs = ["owner-a", "owner-b"]
            .into_iter()
            .map(|owner| crate::source_rule_ownership::SourceRuleOwnership {
                owner: owner.into(),
                chain: vec!["output.o".into(), owner.into()],
            })
            .collect();
        let diagnostics = super::source_rejection_diagnostics(
            path,
            Some(7),
            None,
            "unimplemented producer".into(),
            Some(proofs),
        );
        assert_eq!(diagnostics.len(), 2);
        for (diagnostic, owner) in diagnostics.iter().zip(["owner-a", "owner-b"]) {
            assert_eq!(
                diagnostic.context.as_ref().unwrap().target.as_deref(),
                Some(owner)
            );
            assert_eq!(diagnostic.location.as_ref().unwrap().line, Some(7));
            assert!(diagnostic.message.contains(&format!("output.o -> {owner}")));
        }
        for proofs in [None, Some(Vec::new())] {
            let diagnostics = super::source_rejection_diagnostics(
                path,
                None,
                None,
                "unimplemented producer".into(),
                proofs,
            );
            assert_eq!(diagnostics.len(), 1);
            assert!(diagnostics[0].context.is_none());
        }
    }

    #[test]
    fn rejected_source_meta_provider_keeps_only_known_selector_ownership() {
        let path = std::path::Path::new("compiler/include/mmakefile.src");
        for owner in ["plain-owner", "includes-asm_h-${AROS_TARGET_CPU}"] {
            let diagnostic =
                super::source_meta_provider_diagnostic(path, owner, "unimplemented".into());
            assert_eq!(diagnostic.context.unwrap().target.as_deref(), Some(owner));
        }
        for owner in [
            "",
            "${ARBITRARY}-owner",
            "$(shell command)",
            "../owner",
            "name;other",
        ] {
            let diagnostic =
                super::source_meta_provider_diagnostic(path, owner, "unimplemented".into());
            assert!(diagnostic.context.is_none(), "unsafe selector {owner}");
        }
    }

    #[test]
    fn path_valued_rejected_rule_owner_uses_exact_source_chain_proof() {
        let tree = tempfile::tempdir().unwrap();
        let source = Path::new("arch/example/mmakefile.src");
        let snapshot = "build/objects/unit.o: src/unit.c\nbuild/tools/helper: build/objects/unit.o\n#MM canonical-owner : build/tools/helper\n";
        let (scope, line_states) = crate::make_vars::collect_vars_impl(
            snapshot,
            Some(&crate::parser::TargetContext::default()),
        );
        let dirs = crate::dirs::DirVars::load(tree.path());
        let proofs = super::rejected_rule_owner_proofs(
            "build/tools/helper",
            true,
            (snapshot, Some(&line_states)),
            &scope,
            &dirs,
            (tree.path(), Path::new("arch/example")),
            2,
        )
        .expect("path-valued Make target must be attributed through its exact #MM chain");

        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].owner, "canonical-owner");
        assert_eq!(
            proofs[0].chain,
            vec![
                "build/tools/helper".to_owned(),
                "canonical-owner".to_owned()
            ]
        );
        let diagnostics = super::source_rejection_diagnostics(
            source,
            Some(2),
            Some("build/tools/helper"),
            "rejected producer".into(),
            Some(proofs),
        );
        assert_eq!(
            diagnostics[0]
                .context
                .as_ref()
                .and_then(|context| context.target.as_deref()),
            Some("canonical-owner")
        );
        assert!(diagnostics[0]
            .message
            .contains("exact source consumer chain: build/tools/helper -> canonical-owner"));

        assert!(super::rejected_rule_owner_proofs(
            "canonical-owner",
            true,
            (snapshot, Some(&line_states)),
            &scope,
            &dirs,
            (tree.path(), Path::new("arch/example")),
            2,
        )
        .is_none());
        let canonical = super::source_rejection_diagnostics(
            source,
            Some(2),
            Some("canonical-owner"),
            "existing diagnostic".into(),
            None,
        );
        assert_eq!(
            canonical[0]
                .context
                .as_ref()
                .and_then(|context| context.target.as_deref()),
            Some("canonical-owner")
        );
    }

    #[test]
    fn rejected_rule_owner_proof_keeps_incomplete_and_unknown_snapshots_unowned() {
        let tree = tempfile::tempdir().unwrap();
        let snapshot = "build/objects/unit.o: src/unit.c\nbuild/tools/helper: build/objects/unit.o\n#MM canonical-owner : build/tools/helper\nifeq ($(UNKNOWN),yes)\nconditional-consumer: build/tools/helper\nendif\n";
        let (scope, line_states) = crate::make_vars::collect_vars_impl(
            snapshot,
            Some(&crate::parser::TargetContext::default()),
        );
        let dirs = crate::dirs::DirVars::load(tree.path());
        for configuration_is_complete in [false, true] {
            assert!(super::rejected_rule_owner_proofs(
                "build/tools/helper",
                configuration_is_complete,
                (snapshot, Some(&line_states)),
                &scope,
                &dirs,
                (tree.path(), Path::new("arch/example")),
                2,
            )
            .is_none());
        }
    }

    #[test]
    fn rejected_rule_owner_proof_cannot_ignore_optional_generated_dependency_includes() {
        let tree = tempfile::tempdir().unwrap();
        let snapshot = "build/objects/unit.o: src/unit.c\nbuild/tools/helper: build/objects/unit.o\n#MM canonical-owner : build/tools/helper\n-include build/objects/unit.d\n";
        let (scope, line_states) = crate::make_vars::collect_vars_impl(
            snapshot,
            Some(&crate::parser::TargetContext::default()),
        );
        let dirs = crate::dirs::DirVars::load(tree.path());
        assert!(
            super::rejected_rule_owner_proofs(
                "build/tools/helper",
                true,
                (snapshot, Some(&line_states)),
                &scope,
                &dirs,
                (tree.path(), Path::new("arch/example")),
                2,
            )
            .is_none(),
            "an optional .d include still has unmodeled future Make semantics"
        );
    }

    use super::{join_continuations, literal_object_source_anchors};
    use crate::local_make_includes::{inline_native_make_configuration, LocalMakeIncludeLimits};
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn continuation_rejection_anchor_uses_first_physical_line() {
        let tree = tempfile::tempdir().unwrap();
        let source = Path::new("arch/example/mmakefile.src");
        let text = "before := value\noutput.o: first.o \\\n  second.o\nafter := value\n";
        let scan = inline_native_make_configuration(
            text,
            tree.path(),
            source,
            LocalMakeIncludeLimits::default(),
            &BTreeMap::new(),
        );
        let joined = join_continuations(&scan.expanded);
        let anchors = literal_object_source_anchors(tree.path(), source, text, &scan, &joined)
            .expect("unmodified source must have an exact line map");

        assert_eq!(anchors.len(), 3);
        assert_eq!(anchors[0].as_ref().unwrap().line, 1);
        assert_eq!(anchors[1].as_ref().unwrap().line, 2);
        assert_eq!(anchors[2].as_ref().unwrap().line, 4);
    }

    #[test]
    fn generated_template_lines_keep_source_template_locations_and_reject_drift() {
        let tree = tempfile::tempdir().unwrap();
        let source = Path::new("compiler/include/mmakefile.src");
        let template = Path::new("compiler/include/geninc.cfg.in");
        std::fs::create_dir_all(tree.path().join("compiler/include")).unwrap();
        std::fs::write(
            tree.path().join(template),
            "%common\nEXECSMP=\"@ENABLE_EXECSMP@\"\n",
        )
        .unwrap();
        let templates = BTreeMap::from([(
            "compiler/include/geninc.cfg".into(),
            aros_common::native_make_template::ResolvedGeneratedMakeTemplate {
                template_relative: template.to_string_lossy().into_owned(),
                expanded_text: "%common\nEXECSMP=\"\"\n".into(),
                substitutions: BTreeMap::from([("@ENABLE_EXECSMP@".into(), String::new())]),
            },
        )]);
        let text = "include $(TOP)/$(CURDIR)/geninc.cfg\noutput.o: input.c\n";
        let scan = crate::local_make_includes::inline_native_make_configuration_with_templates(
            text,
            tree.path(),
            source,
            LocalMakeIncludeLimits::default(),
            &BTreeMap::new(),
            &templates,
        );
        assert!(scan.issues.is_empty(), "{:?}", scan.issues);
        let joined = join_continuations(&scan.expanded);
        let anchors =
            literal_object_source_anchors(tree.path(), source, text, &scan, &joined).unwrap();
        assert!(anchors[0].is_none());
        assert_eq!(anchors[1].as_ref().unwrap().source, template);
        assert_eq!(anchors[2].as_ref().unwrap().line, 2);
        assert_eq!(anchors[3].as_ref().unwrap().source, source);
        assert_eq!(anchors[3].as_ref().unwrap().line, 2);
        assert_eq!(
            super::rejection_source_location(source, Some(&anchors), 4),
            (source, Some(2)),
            "header/directory rejections must use physical source lines after expansion"
        );
        assert_eq!(
            super::rejection_source_location(source, Some(&anchors), 1),
            (source, None)
        );
        assert_eq!(
            super::rejection_source_location(source, Some(&anchors), 0),
            (source, None)
        );
        assert_eq!(
            super::rejection_source_location(source, None, 4),
            (source, None)
        );
        std::fs::write(tree.path().join(template), "%common\nEXECSMP=\"changed\"\n").unwrap();
        assert!(literal_object_source_anchors(tree.path(), source, text, &scan, &joined).is_none());
    }

    #[test]
    fn included_configuration_lines_keep_fragment_and_parent_origins() {
        let tree = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tree.path().join("arch/example")).unwrap();
        let source = Path::new("arch/example/mmakefile.src");
        let fragment = Path::new("arch/example/native.mk");
        let text = "before := value\ninclude $(SRCDIR)/config/aros.cfg\nafter := value\n";
        std::fs::create_dir_all(tree.path().join("config")).unwrap();
        std::fs::write(tree.path().join("config/aros.cfg"), "ORIGINAL := config\n").unwrap();
        std::fs::write(
            tree.path().join(fragment),
            "# fragment comment\nFRAGMENT_FLAGS := one \\\n  two\n",
        )
        .unwrap();
        let bindings = BTreeMap::from([(
            "config/aros.cfg".to_owned(),
            fragment.to_string_lossy().into_owned(),
        )]);
        let scan = inline_native_make_configuration(
            text,
            tree.path(),
            source,
            LocalMakeIncludeLimits::default(),
            &bindings,
        );
        assert!(scan.issues.is_empty(), "{:?}", scan.issues);
        let joined = join_continuations(&scan.expanded);
        let anchors = literal_object_source_anchors(tree.path(), source, text, &scan, &joined)
            .expect("included source must reconstruct byte-for-byte");

        assert_eq!(anchors[0].as_ref().unwrap().source, source);
        assert_eq!(anchors[0].as_ref().unwrap().line, 1);
        assert!(
            anchors[1].is_none(),
            "include placeholder has no source line"
        );
        assert_eq!(anchors[2].as_ref().unwrap().source, fragment);
        assert_eq!(anchors[2].as_ref().unwrap().line, 1);
        assert_eq!(anchors[3].as_ref().unwrap().source, fragment);
        assert_eq!(anchors[3].as_ref().unwrap().line, 2);
        assert_eq!(anchors[4].as_ref().unwrap().source, source);
        assert_eq!(anchors[4].as_ref().unwrap().line, 3);

        std::fs::write(tree.path().join(fragment), "FRAGMENT_FLAGS := changed\n").unwrap();
        assert!(
            literal_object_source_anchors(tree.path(), source, text, &scan, &joined,).is_none(),
            "changed include bytes must suppress physical locations"
        );
    }

    #[test]
    #[ignore = "requires AROS_P4_SOURCE_ROOT to point at the P4 source checkout"]
    fn actual_p4_sifive_rule_maps_to_its_physical_source_line() {
        let root = std::path::PathBuf::from(
            std::env::var_os("AROS_P4_SOURCE_ROOT").expect("set P4 source root"),
        );
        let source = Path::new("arch/riscv-native/sifive_u/boot/mmakefile.src");
        let text = std::fs::read_to_string(root.join(source)).unwrap();
        let bindings = BTreeMap::from([(
            "config/aros.cfg".to_owned(),
            "arch/riscv-esp32p4/native-kobj-config.mk".to_owned(),
        )]);
        let scan = inline_native_make_configuration(
            &text,
            &root,
            source,
            LocalMakeIncludeLimits::default(),
            &bindings,
        );
        assert!(scan.issues.is_empty(), "{:?}", scan.issues);
        let joined = join_continuations(&scan.expanded);
        let anchors = literal_object_source_anchors(&root, source, &text, &scan, &joined)
            .expect("actual P4 source must reconstruct exactly");
        let matching = joined
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains("$(TARGETDIR)/core.bin.o:"))
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1);
        let anchor = anchors[matching[0].0].as_ref().unwrap();
        assert_eq!(anchor.source, source);
        assert_eq!(anchor.line, 61);
    }
}
