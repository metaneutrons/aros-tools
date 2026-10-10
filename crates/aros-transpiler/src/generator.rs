use crate::ast::ModuleType;
use crate::graph::DependencyGraph;
use std::collections::HashSet;
use std::fmt::Write;

mod endpoints;
mod external;
mod generated;
mod header;
mod meta;
mod rules;
mod targets;
pub use header::generated_header;

/// Names reserved by typed CMake producers, also used before normalization.
pub(crate) fn concrete_endpoint_names(graph: &DependencyGraph) -> HashSet<String> {
    endpoints::collect_endpoint_names(graph)
}

/// Renders one value as a quoted CMake argument.
///
/// A string-literal define such as `AROS_ARCHITECTURE="pc"` carries quotes of
/// its own. Emitted verbatim they would end the CMake string early and the
/// value would be read as several arguments, so they are escaped here. `$` is
/// left alone, since a value may legitimately reference a CMake variable.
fn cmake_arg(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// SDK text operations contain literal pkg-config `${...}` references, not
/// CMake path expressions. Preserve dollars through configure evaluation.
fn cmake_literal_arg(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$");
    format!("\"{escaped}\"")
}

/// Writes one `aros_build_configure` block.
///
/// Called twice from `generate_cmake` with disjoint halves of the same list: a
/// declaration that publishes an archive interface has to precede its
/// consumers, and one that links an in-tree link library has to follow that
/// library's target. No declaration needs both today. If one ever does, the
/// generated file says so where it cannot be missed, because this generator has
/// no way to satisfy both orderings at once.
fn emit_configure_builds(
    out: &mut String,
    declarations: Vec<&crate::ast::ConfigureBuildDecl>,
    heading: &str,
) {
    if declarations.is_empty() {
        return;
    }
    writeln!(
        out,
        "# =============================================================================\n\
         # {heading}\n\
         # ============================================================================="
    )
    .unwrap();
    let mut declarations = declarations;
    declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
    for declaration in declarations {
        if declaration.provided_library.is_some() && !declaration.dependency_targets.is_empty() {
            writeln!(
                out,
                "message(FATAL_ERROR\n    \"{}: a configure build cannot both publish an archive \\\n\
                 interface and consume a link-library target; the transpiler has no \\\n\
                 declaration order that satisfies both\")",
                declaration.mmake_name
            )
            .unwrap();
            continue;
        }
        writeln!(out, "aros_build_configure(").unwrap();
        writeln!(out, "    MMAKE_ID {}", declaration.mmake_name).unwrap();
        writeln!(out, "    MODE {}", cmake_arg(&declaration.mode)).unwrap();
        writeln!(out, "    SOURCE_DIR {}", cmake_arg(&declaration.source_dir)).unwrap();
        writeln!(out, "    BINARY_DIR {}", cmake_arg(&declaration.binary_dir)).unwrap();
        writeln!(
            out,
            "    INSTALL_PREFIX {}",
            cmake_arg(&declaration.install_prefix)
        )
        .unwrap();
        writeln!(
            out,
            "    INPUT_MANIFEST {}",
            cmake_arg(&declaration.input_manifest)
        )
        .unwrap();
        let private_products = declaration
            .private_products
            .iter()
            .map(|product| cmake_arg(product))
            .collect::<Vec<_>>();
        writeln!(out, "    PRIVATE_PRODUCTS {}", private_products.join(" ")).unwrap();
        let install_products = declaration
            .install_products
            .iter()
            .map(|product| cmake_arg(product))
            .collect::<Vec<_>>();
        writeln!(out, "    INSTALL_PRODUCTS {}", install_products.join(" ")).unwrap();
        if !declaration.dependency_targets.is_empty() {
            let targets = declaration
                .dependency_targets
                .iter()
                .map(|target| cmake_arg(target))
                .collect::<Vec<_>>();
            writeln!(out, "    DEPENDENCY_TARGETS {}", targets.join(" ")).unwrap();
        }
        if let Some(library) = &declaration.provided_library {
            writeln!(out, "    PROVIDED_LIBRARY {}", cmake_arg(library)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }
}

/// Whether a public header deliberately lives below a foreign architecture
/// directory but is part of the architecture-independent SDK API.
///
/// `hidd/unixio.h` is consumed by the native PC and Sam440 serial/parallel
/// drivers as well as hosted targets. Keep this exception exact so unrelated
/// foreign CPU and ASM headers retain the collision protection in AROS.cmake.
fn copy_includes_allows_foreign_arch(decl: &crate::copy_includes::CopyIncludesDecl) -> bool {
    decl.source_dir == "arch/all-unix/hidd/unixio/include"
        && decl.dest == "hidd"
        && decl.patterns == ["*.h"]
        && decl.excludes.is_empty()
        && decl.flatten
}

/// Generates modern CMake code from the parsed dependency graph.
///
/// # Panics
///
/// Panics if JSON serialization of a source-text operation fails. The closed
/// operation model contains only tagged enum variants and strings, without
/// fallible custom serializers or non-finite numeric values.
#[must_use]
pub fn generate_cmake(graph: &DependencyGraph) -> String {
    let mut out = String::new();

    // A kickstart member is linked into one image with the others, so it needs
    // a second artefact built without the compiler spec's default link set and
    // with its library bases made local (config/make.tmpl:2743). Marked here
    // because the module targets are emitted before the package declarations.
    // Carries the kickstart's architecture, because a module can be a member of
    // another architecture's kickstart and must not grow a second artefact
    // here for that.
    let mut kickstart_members: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for package in graph.packages.iter().filter(|p| p.is_kickstart) {
        for member in &package.resolved {
            let arches = kickstart_members.entry(member.target.clone()).or_default();
            if !arches.contains(&package.arch) {
                arches.push(package.arch.clone());
            }
        }
    }

    let mut all_targets = endpoints::collect_endpoint_names(graph);

    external::emit_fetches(&mut out, graph);
    external::emit_external_cmake(&mut out, graph);
    external::emit_configure_and_grub_builds(&mut out, graph);
    external::emit_lane_declarations(&mut out, graph, &mut all_targets);
    generated::emit_script_outputs(&mut out, graph);
    generated::emit_python_outputs(&mut out, graph);
    generated::emit_include_copies(&mut out, graph);
    generated::emit_flexcat_headers_and_ilbm(&mut out, graph);
    generated::emit_flexcat_sources(&mut out, graph);
    let sdk_paths = match generated::emit_sdk_object_groups(&mut out, graph) {
        Ok(paths) => paths,
        Err(fatal) => return fatal,
    };
    if let Some(fatal) = generated::emit_literal_objects_and_archives(&mut out, graph, &sdk_paths) {
        return fatal;
    }
    targets::emit_concrete_targets(&mut out, graph, &kickstart_members);
    targets::emit_post_target_bindings(&mut out, graph);
    targets::emit_late_helper_bindings(&mut out, graph);
    rules::emit_define_and_setup_rules(&mut out, graph);
    rules::emit_genmodule_and_text_rules(&mut out, graph);
    rules::emit_source_text_and_copy_rules(&mut out, graph);
    rules::emit_sdk_asset_rules(&mut out, graph);
    rules::emit_host_header_rules(&mut out, graph);
    rules::emit_host_file_generators(&mut out, graph);
    rules::emit_header_transforms(&mut out, graph);
    rules::emit_icon_targets(&mut out, graph);
    rules::emit_catalogs(&mut out, graph);
    meta::emit_meta_banner(&mut out);

    let all_metas: HashSet<&str> = graph.meta_targets.keys().map(String::as_str).collect();
    let mut meta_rules: Vec<_> = graph.meta_targets.iter().collect();
    meta_rules.sort_by_key(|(name, _)| (*name).clone());

    // The dedicated ABI builder already orders its archive after the exact
    // genmodule includes/FD outputs and after any public headers discovered in
    // its config.  The legacy `<mmake>-linklib -> <mmake>-includes` meta edge
    // additionally reaches the global `includes-generate-deps` closure, which
    // makes a focused ABI archive download every unrelated port.  Suppress
    // only that redundant edge; the public `<mmake>-includes` meta target
    // retains its complete historic behaviour when requested explicitly.
    let mut redundant_abi_include_edges: Vec<(String, String)> = graph
        .targets
        .values()
        .filter(|target| target.module_type == ModuleType::Abi)
        .map(|target| {
            (
                format!("{}-linklib", target.mmake_name),
                format!("{}-includes", target.mmake_name),
            )
        })
        .collect();

    // An implicit edge that closes a cycle with declared edges is left out; the
    // report keeps the decision visible in the generated file.
    let cycle_edges = meta::implicit_cycle_edges(
        &all_targets,
        &all_metas,
        &meta_rules,
        &graph.explicit_meta_edges,
    );
    for (owner, dependency) in &cycle_edges {
        writeln!(
            out,
            "# cycle: implicit edge {owner} -> {dependency} yields to declared edges"
        )
        .unwrap();
    }
    redundant_abi_include_edges.extend(cycle_edges);

    meta::emit_meta_declarations(&mut out, &all_targets, &meta_rules);
    meta::emit_assembly_header_rules(&mut out, graph);
    meta::emit_arch_endpoint_effects(&mut out, graph);
    meta::emit_meta_dependencies(
        &mut out,
        &all_targets,
        &all_metas,
        &meta_rules,
        &redundant_abi_include_edges,
    );
    meta::emit_host_headers_and_objects(&mut out, graph);
    meta::emit_link_set_and_packages(&mut out, graph);

    out
}

#[cfg(test)]
#[path = "generator_tests.rs"]
mod tests;
