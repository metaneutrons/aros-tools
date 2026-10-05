use crate::ast::ModuleType;
use crate::graph::DependencyGraph;
use std::collections::HashSet;
use std::fmt::Write;

mod header;
pub use header::generated_header;

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
#[expect(
    clippy::too_many_lines,
    reason = "ordered CMake emission is one linear serialization transaction; file-size and golden-output gates bound it"
)]
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

    let mut all_targets: HashSet<String> =
        graph
            .targets
            .iter()
            .filter(|(_, target)| target.module_type != ModuleType::ModuleHeaders)
            .map(|(name, _)| name)
            .chain(graph.icon_targets.keys())
            .cloned()
            .chain(graph.catalogs.iter().map(|catalog| catalog.mmake.clone()))
            .chain(
                graph
                    .sdk_text_rules
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(graph.host_header_rules.iter().flat_map(|declaration| {
                [declaration.owner.clone(), declaration.setup_owner.clone()]
            }))
            .chain(
                graph
                    .host_file_generators
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .source_text_rules
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(graph.sdk_file_copies.iter().map(|rule| rule.owner.clone()))
            .chain(graph.sdk_asset_rules.iter().map(|rule| rule.owner.clone()))
            .chain(
                graph
                    .source_value_rules
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .literal_object_groups
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph.source_archives.iter().flat_map(|archive| {
                    [archive.declaration.owner.clone(), archive.provider_target()]
                }),
            )
            .chain(
                graph
                    .sdk_object_groups
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .host_header_aggregates
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .genmodule_header_rules
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .directory_setups
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .genmodule_writefiles_rules
                    .iter()
                    .map(|rule| rule.owner.clone()),
            )
            .chain(
                graph
                    .flexcat_sources
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .flexcat_headers
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .ilbm_sources
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .header_transforms
                    .iter()
                    .map(|transform| transform.name.clone()),
            )
            .chain(
                graph
                    .define_headers
                    .iter()
                    .map(|header| header.owner.clone()),
            )
            .chain(
                graph
                    .copy_directories
                    .iter()
                    .map(|declaration| declaration.name.clone()),
            )
            .chain(
                graph
                    .python_outputs
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(
                graph
                    .script_outputs
                    .iter()
                    .map(|declaration| declaration.owner.clone()),
            )
            .chain(graph.fetches.iter().map(|fetch| fetch.name.clone()))
            .chain(
                graph
                    .external_cmake
                    .iter()
                    .map(|declaration| declaration.mmake_name.clone()),
            )
            .chain(
                graph
                    .external_cmake
                    .iter()
                    .map(|declaration| declaration.provider_target.clone()),
            )
            .chain(
                graph
                    .configure_builds
                    .iter()
                    .map(|declaration| declaration.mmake_name.clone()),
            )
            .chain(
                graph
                    .configure_builds
                    .iter()
                    .filter_map(|declaration| declaration.provider_target.clone()),
            )
            .chain(
                graph
                    .grub_builds
                    .iter()
                    .map(|declaration| declaration.mmake_name.clone()),
            )
            .chain(
                graph
                    .ahi_builds
                    .iter()
                    .map(|declaration| declaration.mmake_name.clone()),
            )
            .chain(
                graph
                    .arch_endpoint_effects
                    .iter()
                    .map(|effect| effect.endpoint.clone()),
            )
            .collect();
    for header in &graph.assembly_headers {
        all_targets.insert(header.owner.clone());
        all_targets.insert(header.aggregate_owner.clone());
    }

    // The closed GRUB2 helper creates one shared source-fetch endpoint and
    // exposes the legacy alias itself.  Keep both names in the endpoint
    // registry so #MM edges retain their original ordering rather than being
    // silently filtered as unknown meta dependencies.
    if !graph.grub_builds.is_empty() {
        all_targets.insert("grub2-aros--fetch".to_owned());
        all_targets.insert("grub2-aros-fetch".to_owned());
    }

    // AHI invokes the explicitly materialised host `sfdc` tool through its
    // closed helper.  It has no legacy #MM declaration of its own, but keeping
    // the endpoint visible prevents an explicit future meta edge from being
    // silently discarded during generated-target filtering.
    if !graph.ahi_builds.is_empty() {
        all_targets.insert("host-sfdc".to_owned());
    }

    // Full genmodule and ABI declarations create product targets inside the
    // CMake helper rather than as independent AST declarations. They are still
    // real dependency endpoints: a raw `-lstdc`, for example, must order its
    // consumer after `compiler-stdc-linklib`. Keep these generated products in
    // the endpoint registry so the meta-edge filter cannot discard them.
    for (mmake, target) in &graph.targets {
        match target.module_type {
            ModuleType::Abi => {
                all_targets.insert(format!("{mmake}-linklib"));
            }
            ModuleType::Library
                if target.genmodule_only
                    || target
                        .genmodule_linklibs
                        .as_ref()
                        .is_some_and(|metadata| metadata.enabled) =>
            {
                all_targets.insert(format!("{mmake}-linklib"));
                if !target.genmodule_only
                    && target
                        .genmodule_linklibs
                        .as_ref()
                        .is_some_and(|metadata| metadata.enabled && metadata.has_relative)
                {
                    all_targets.insert(format!("{mmake}-linklib-rel"));
                }
            }
            _ => {}
        }
    }

    // Third-party source fetching is emitted before header staging.  Most
    // copies still happen at configure time, but a cache-empty fetched port
    // needs its owning fetch target to exist before CMake can declare the
    // build-time copy rules.
    if !graph.fetches.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Third-party source fetching (from %fetch)\n\
             # ============================================================================="
        )
        .unwrap();
        for f in &graph.fetches {
            write!(
                out,
                "aros_fetch_archive(NAME \"{}\" ARCHIVE \"{}\" SUFFIXES \"{}\" ORIGINS \"{}\"\n\
                 \x20   CHECKSUMS \"{}\" LOCATION \"{}\" DESTINATION \"{}\" BASE \"{}\" PATCH_ORIGINS \"{}\" PATCHES \"{}\"",
                f.name,
                f.archive,
                f.suffixes,
                f.origins,
                f.checksums,
                f.location,
                f.destination,
                f.base,
                f.patch_origins,
                f.patches
            )
            .unwrap();
            let external_audit = graph
                .external_cmake
                .iter()
                .find(|declaration| declaration.fetch_target == f.name)
                .map(|declaration| {
                    (
                        declaration.source_dir.as_str(),
                        declaration.local_patch_files.as_slice(),
                    )
                });
            let python_audit = graph
                .python_outputs
                .iter()
                .find(|declaration| declaration.fetch_target == f.name)
                .map(|declaration| {
                    (
                        declaration.audited_source_dir.as_str(),
                        declaration.local_patch_files.as_slice(),
                    )
                });
            if let Some((source_dir, local_patch_files)) = external_audit.or(python_audit) {
                if local_patch_files.is_empty() {
                    writeln!(out, ")").unwrap();
                    continue;
                }
                let patch_files: Vec<_> = local_patch_files
                    .iter()
                    .map(|path| cmake_arg(path))
                    .collect();
                write!(
                    out,
                    "\n    SOURCE_DIR {}\n    LOCAL_PATCH_FILES {}",
                    cmake_arg(source_dir),
                    patch_files.join(" ")
                )
                .unwrap();
            }
            writeln!(out, ")").unwrap();
        }
        writeln!(out).unwrap();
    }

    // Audited external CMake projects must exist before ordinary consumers
    // are declared. The helper creates both the mmake workflow endpoint and a
    // distinct linkable interface target, so an explicit `uselibs=` edge can
    // bind immediately and a same-named #MM rule must not manufacture a
    // duplicate phony target later.
    if !graph.external_cmake.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Capability-checked external CMake builds\n\
             # ============================================================================="
        )
        .unwrap();
        let mut declarations: Vec<_> = graph.external_cmake.iter().collect();
        declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
        for declaration in declarations {
            writeln!(out, "aros_build_external_cmake(").unwrap();
            // MMAKE identities have already passed the strict capability
            // profile's target-name validation. Keep the canonical unquoted
            // spelling used by every other generated declaration so
            // aros-verify can pair the declaration with its realised target.
            writeln!(out, "    MMAKE_ID {}", declaration.mmake_name).unwrap();
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
                "    FETCH_TARGET {}",
                cmake_arg(&declaration.fetch_target)
            )
            .unwrap();
            writeln!(
                out,
                "    PROVIDED_LIBRARY {}",
                cmake_arg(&declaration.provided_library)
            )
            .unwrap();
            let products: Vec<_> = declaration
                .library_products
                .iter()
                .map(|product| cmake_arg(product))
                .collect();
            writeln!(out, "    LIBRARY_PRODUCTS {}", products.join(" ")).unwrap();
            let headers: Vec<_> = declaration
                .header_products
                .iter()
                .map(|header| cmake_arg(header))
                .collect();
            writeln!(out, "    HEADER_PRODUCTS {}", headers.join(" ")).unwrap();
            let auxiliary: Vec<_> = declaration
                .auxiliary_products
                .iter()
                .map(|product| cmake_arg(product))
                .collect();
            if !auxiliary.is_empty() {
                writeln!(out, "    AUXILIARY_PRODUCTS {}", auxiliary.join(" ")).unwrap();
            }
            let includes: Vec<_> = declaration
                .public_include_dirs
                .iter()
                .map(|include| cmake_arg(include))
                .collect();
            writeln!(out, "    PUBLIC_INCLUDE_DIRS {}", includes.join(" ")).unwrap();
            let options: Vec<_> = declaration
                .options
                .iter()
                .map(|option| cmake_arg(option))
                .collect();
            writeln!(out, "    OPTIONS {}", options.join(" ")).unwrap();
            for (keyword, values) in [
                ("BUILD_TARGETS", &declaration.build_targets),
                ("INSTALL_COMPONENTS", &declaration.install_components),
                ("HOST_TOOLS", &declaration.host_tools),
                ("COMPILE_DEFINES", &declaration.compile_defines),
            ] {
                if !values.is_empty() {
                    let values: Vec<_> = values.iter().map(|value| cmake_arg(value)).collect();
                    writeln!(out, "    {keyword} {}", values.join(" ")).unwrap();
                }
            }
            if declaration.library_group {
                writeln!(out, "    LIBRARY_GROUP").unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    // Local projects admitted from `%build_with_configure` use a closed
    // runner contract rather than arbitrary shell text.  Declarations precede
    // ordinary consumers so a published archive interface can bind exactly
    // like an in-tree link library.
    //
    // A declaration that consumes a link library cannot stand here, because
    // aros_build_configure asks that target where its archive is: see the
    // second block after the concrete targets.
    emit_configure_builds(
        &mut out,
        graph
            .configure_builds
            .iter()
            .filter(|declaration| declaration.dependency_targets.is_empty())
            .collect(),
        "Capability-checked configure-style builds",
    );

    // GRUB2's legacy configure declarations are host-tool lanes with a
    // substantially narrower contract than the local-source helper above.
    // The emitted selector cannot carry arbitrary source paths, flags or
    // command text: GrubBuild.cmake pins those internally.  Emit the real
    // targets before #MM fallback utility targets so their aliases and edges
    // bind to the actual build products.
    if !graph.grub_builds.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Capability-checked GRUB2 host-tool lanes\n\
             # ============================================================================="
        )
        .unwrap();
        writeln!(out, "if(AROS_GRUB2_HOST_LANES_AVAILABLE)").unwrap();
        let mut declarations: Vec<_> = graph.grub_builds.iter().collect();
        declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
        for declaration in declarations {
            writeln!(out, "aros_build_grub2(").unwrap();
            writeln!(out, "    MMAKE_ID {}", declaration.mmake_name).unwrap();
            writeln!(out, "    VERSION {}", cmake_arg(&declaration.version)).unwrap();
            writeln!(out, "    MODE {}", cmake_arg(&declaration.mode)).unwrap();
            writeln!(out, "    BINARY_DIR {}", cmake_arg(&declaration.binary_dir)).unwrap();
            writeln!(
                out,
                "    INSTALL_PREFIX {}",
                cmake_arg(&declaration.install_prefix)
            )
            .unwrap();
            writeln!(out, ")\n").unwrap();
        }
        writeln!(out, "else()").unwrap();
        writeln!(
            out,
            "    message(STATUS \"⏭️  AROS: audited GRUB2 host-tool lanes are unavailable on this build host\")"
        )
        .unwrap();
        writeln!(out, "endif()\n").unwrap();
    }

    // Capability-checked Python/stdout generators are declared before their
    // compile targets.  This registers each build-tree output while source
    // lanes are still being resolved, so a generated `.s` file is retained on
    // a clean configure even though it does not exist yet. Consumers are bound
    // in a second phase after all concrete targets have been created.
    // Codegen options that belong to one architecture lane's own sources.
    // Declared before every target, as a global keyed by the lane and the file,
    // so aros_resolve_arch_sources can apply them where it resolves the file and
    // no builder signature has to learn a field it only forwards.
    {
        let mut entries: Vec<String> = Vec::new();
        for target in graph.targets.values() {
            for (tag, dir, file, option) in &target.arch_source_options {
                let entry = format!("{tag}|{dir}|{file}|{option}");
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
            }
        }
        entries.sort();
        if !entries.is_empty() {
            writeln!(
                out,
                "# ---- Per-lane codegen options (USER_CFLAGS of a %build_archspecific) ----"
            )
            .unwrap();
            writeln!(out, "aros_set_arch_source_options(").unwrap();
            for entry in entries {
                writeln!(out, "    {}", cmake_arg(&entry)).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    // Full macro ABI production is source-owned and independent of modtype.
    // Register it before builders consult the generic scaffolding helper.
    {
        let mut abi_owners: Vec<_> = graph
            .targets
            .values()
            .filter(|target| target.genmodule_abi)
            .map(|target| &target.mmake_name)
            .collect();
        abi_owners.sort();
        for owner in abi_owners {
            writeln!(out, "aros_set_module_abi({})", cmake_arg(owner)).unwrap();
            all_targets.insert(format!("{owner}-includes"));
        }
    }
    // Reference genmodule must receive the same override as MetaMake. HPET's
    // clocksource base type and resident priority live only in that file.
    {
        let mut named: Vec<(&String, &String)> = graph
            .targets
            .iter()
            .filter_map(|(mmake, target)| {
                target
                    .config_override_file
                    .as_ref()
                    .map(|config| (mmake, config))
            })
            .collect();
        named.sort();
        if !named.is_empty() {
            writeln!(
                out,
                "# ---- Explicit genmodule overrides (confoverride=) ----"
            )
            .unwrap();
            for (mmake, config) in named {
                writeln!(
                    out,
                    "aros_set_module_config_override({} {})",
                    cmake_arg(mmake),
                    cmake_arg(config)
                )
                .unwrap();
            }
            writeln!(out).unwrap();
        }
    }

    // The genmodule config a declaration names with `conffile=`. Declared
    // before every target, because the module builders consult it while they
    // are read.
    {
        let mut named: Vec<(&String, &String)> = graph
            .targets
            .iter()
            .filter_map(|(mmake, target)| target.config_file.as_ref().map(|config| (mmake, config)))
            .collect();
        named.sort();
        if !named.is_empty() {
            writeln!(out, "# ---- Explicit genmodule configs (conffile=) ----").unwrap();
            for (mmake, config) in named {
                writeln!(
                    out,
                    "aros_set_module_config({} {})",
                    cmake_arg(mmake),
                    cmake_arg(config)
                )
                .unwrap();
            }
            writeln!(out).unwrap();
        }
    }

    // Files an exact Python recipe generates. Emitted here, beside the fetched
    // generators and before every target, because aros_resolve_sources consults
    // the generator registry before it probes the filesystem and a generated
    // source does not exist when CMake configures.
    if !graph.script_outputs.is_empty() {
        writeln!(out, "# ---- Files generated by an exact Python recipe ----").unwrap();
        let mut declarations: Vec<_> = graph.script_outputs.iter().collect();
        declarations.sort_by(|left, right| left.owner.cmp(&right.owner));
        for decl in declarations {
            writeln!(out, "aros_generate_intree_script_outputs(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&decl.owner)).unwrap();
            writeln!(out, "    SCRIPT {}", cmake_arg(&decl.script)).unwrap();
            let outputs: Vec<String> = decl.outputs.iter().map(|o| cmake_arg(o)).collect();
            writeln!(out, "    OUTPUTS {}", outputs.join(" ")).unwrap();
            if decl.stdout {
                writeln!(out, "    STDOUT").unwrap();
            }
            if let Some(directory) = &decl.working_directory {
                writeln!(out, "    WORKING_DIRECTORY {}", cmake_arg(directory)).unwrap();
            }
            if !decl.arguments.is_empty() {
                let args: Vec<String> = decl.arguments.iter().map(|a| cmake_arg(a)).collect();
                writeln!(out, "    ARGUMENTS {}", args.join(" ")).unwrap();
            }
            if !decl.depends.is_empty() {
                let deps: Vec<String> = decl.depends.iter().map(|d| cmake_arg(d)).collect();
                writeln!(out, "    DEPENDS {}", deps.join(" ")).unwrap();
            }
            if !decl.dependency_targets.is_empty() {
                let targets: Vec<String> = decl
                    .dependency_targets
                    .iter()
                    .map(|target| cmake_arg(target))
                    .collect();
                writeln!(out, "    DEPENDENCY_TARGETS {}", targets.join(" ")).unwrap();
            }
            writeln!(out, ")").unwrap();
        }
        writeln!(out).unwrap();
    }

    if !graph.python_outputs.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Capability-checked fetched Python generators\n\
             # ============================================================================="
        )
        .unwrap();
        let mut declarations: Vec<_> = graph.python_outputs.iter().collect();
        declarations.sort_by(|left, right| left.owner.cmp(&right.owner));
        for declaration in declarations {
            writeln!(out, "aros_generate_python_outputs(").unwrap();
            writeln!(out, "    OWNER {}", declaration.owner).unwrap();
            writeln!(
                out,
                "    SOURCE_ROOT {}",
                cmake_arg(&declaration.source_root)
            )
            .unwrap();
            writeln!(out, "    BUILD_ROOT {}", cmake_arg(&declaration.build_root)).unwrap();
            writeln!(
                out,
                "    FETCH_TARGET {}",
                cmake_arg(&declaration.fetch_target)
            )
            .unwrap();
            if let Some(driver) = declaration.driver_script.as_ref() {
                writeln!(out, "    DRIVER_SCRIPT {}", cmake_arg(driver)).unwrap();
            }
            if declaration.requires_flex_bison {
                writeln!(out, "    REQUIRE_FLEX_BISON").unwrap();
            }
            if !declaration.python_packages.is_empty() {
                let fetch_targets = declaration
                    .python_packages
                    .iter()
                    .map(|package| cmake_arg(&package.fetch_target))
                    .collect::<Vec<_>>();
                let source_roots = declaration
                    .python_packages
                    .iter()
                    .map(|package| cmake_arg(&package.source_root))
                    .collect::<Vec<_>>();
                let python_paths = declaration
                    .python_packages
                    .iter()
                    .map(|package| cmake_arg(&package.python_path))
                    .collect::<Vec<_>>();
                writeln!(out, "    PACKAGE_FETCH_TARGETS {}", fetch_targets.join(" ")).unwrap();
                writeln!(out, "    PACKAGE_SOURCE_ROOTS {}", source_roots.join(" ")).unwrap();
                writeln!(out, "    PACKAGE_PYTHON_PATHS {}", python_paths.join(" ")).unwrap();
            }
            if !declaration.source_inputs.is_empty() {
                let inputs = declaration
                    .source_inputs
                    .iter()
                    .map(|input| cmake_arg(input))
                    .collect::<Vec<_>>();
                writeln!(out, "    SOURCE_INPUTS {}", inputs.join(" ")).unwrap();
            }
            if !declaration.local_inputs.is_empty() {
                let inputs = declaration
                    .local_inputs
                    .iter()
                    .map(|input| cmake_arg(input))
                    .collect::<Vec<_>>();
                writeln!(out, "    LOCAL_INPUTS {}", inputs.join(" ")).unwrap();
            }
            for job in &declaration.jobs {
                writeln!(out, "    JOB").unwrap();
                if job.local_script {
                    writeln!(out, "        LOCAL_SCRIPT").unwrap();
                }
                writeln!(out, "        SCRIPT {}", cmake_arg(&job.script)).unwrap();
                writeln!(out, "        OUTPUT {}", cmake_arg(&job.output)).unwrap();
                if !job.depends_on_outputs.is_empty() {
                    let dependencies = job
                        .depends_on_outputs
                        .iter()
                        .map(|dependency| cmake_arg(dependency))
                        .collect::<Vec<_>>();
                    writeln!(out, "        DEPENDS_ON_OUTPUTS {}", dependencies.join(" ")).unwrap();
                }
                if !job.arguments.is_empty() {
                    let arguments = job
                        .arguments
                        .iter()
                        .map(|argument| cmake_arg(argument))
                        .collect::<Vec<_>>();
                    writeln!(out, "        ARGUMENTS {}", arguments.join(" ")).unwrap();
                }
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    // SDK header staging.  In-tree sources are copied at configure time;
    // explicit files from a not-yet-fetched port become Ninja outputs.
    if !graph.copy_includes.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # SDK header staging (from %copy_includes)\n\
             # ============================================================================="
        )
        .unwrap();
        // Generic headers first, architecture-specific last: both land under
        // the same SDK name and the last copy wins. compiler/include ships a
        // portable asm/cpu.h that arch/<cpu>-all/include has to override, and
        // without a defined order which one survived depended on parse order.
        let mut ordered: Vec<&_> = graph.copy_includes.iter().collect();
        ordered.sort_by_key(|d| usize::from(d.source_dir.contains("/arch/")));
        for decl in ordered {
            let patterns: Vec<String> = decl.patterns.iter().map(|p| cmake_arg(p)).collect();
            let excludes: Vec<String> = decl.excludes.iter().map(|p| cmake_arg(p)).collect();
            writeln!(
                out,
                "aros_copy_includes(NAME \"{}\" DEST \"{}\" SOURCE \"{}\"{}{}{}{})",
                decl.name,
                decl.dest,
                decl.source_dir,
                if decl.proven_empty {
                    " PROVEN_EMPTY".to_owned()
                } else {
                    format!(" PATTERNS {}", patterns.join(" "))
                },
                if excludes.is_empty() {
                    String::new()
                } else {
                    format!(" EXCLUDES {}", excludes.join(" "))
                },
                if decl.flatten { " FLATTEN" } else { "" },
                if copy_includes_allows_foreign_arch(decl) {
                    " ALLOW_FOREIGN_ARCH"
                } else {
                    ""
                }
            )
            .unwrap();
        }
        writeln!(out).unwrap();
    }

    // Recursive directory staging.  Unlike source lists, these are real
    // output-producing MetaMake targets.  Emit them before the #MM fallback
    // pass so their legacy identity receives a CMake custom target rather
    // than an empty phony placeholder.
    if !graph.copy_directories.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Recursive directory staging (from %copy_dir_recursive)\n\
             # ============================================================================="
        )
        .unwrap();
        let mut declarations: Vec<_> = graph.copy_directories.iter().collect();
        declarations.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.source.cmp(&right.source))
        });
        for declaration in declarations {
            writeln!(out, "aros_copy_dir_recursive(").unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&declaration.name)).unwrap();
            writeln!(out, "    SOURCE {}", cmake_arg(&declaration.source)).unwrap();
            writeln!(
                out,
                "    DESTINATION {}",
                cmake_arg(&declaration.destination)
            )
            .unwrap();
            if !declaration.dependencies.is_empty() {
                let dependencies = declaration
                    .dependencies
                    .iter()
                    .map(|dependency| cmake_arg(dependency))
                    .collect::<Vec<_>>();
                writeln!(out, "    DEPENDS {}", dependencies.join(" ")).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    // Hand-written Make rules that stage headers. These cannot be transpiled,
    // so they are declared for CMake to check against its list of rules that
    // have a static counterpart. A rule appearing upstream that nobody has
    // handled then surfaces at configure time instead of as a missing header.
    if !graph.adhoc_header_rules.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Hand-written header staging rules (need a static CMake counterpart)\n\
             # ============================================================================="
        )
        .unwrap();
        for rule in &graph.adhoc_header_rules {
            writeln!(
                out,
                "aros_adhoc_header_rule(FILE {} LINE {} ROOT {} DEST {} PREREQS {})",
                cmake_arg(&rule.file),
                rule.line,
                cmake_arg(&rule.root),
                cmake_arg(&rule.dest),
                cmake_arg(&rule.prereqs)
            )
            .unwrap();
        }
        writeln!(out).unwrap();
    }

    // Header-only hand-written FlexCat rules are ordinary output-producing
    // #MM prerequisites. Declare them before concrete targets and let the
    // generated meta edge propagate their include directory and ordering.
    if !graph.flexcat_headers.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Header-only FlexCat rules\n\
             # ============================================================================="
        )
        .unwrap();
        let mut declarations: Vec<_> = graph.flexcat_headers.iter().collect();
        declarations.sort_by(|left, right| {
            left.owner
                .cmp(&right.owner)
                .then_with(|| left.declaring_dir.cmp(&right.declaring_dir))
                .then_with(|| left.line.cmp(&right.line))
        });
        for declaration in declarations {
            writeln!(out, "aros_declare_flexcat_header(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(
                out,
                "    DIRECTORY {}",
                cmake_arg(&declaration.declaring_dir)
            )
            .unwrap();
            writeln!(out, "    HEADER {}", cmake_arg(&declaration.header)).unwrap();
            writeln!(
                out,
                "    DESCRIPTION {}",
                cmake_arg(&declaration.description)
            )
            .unwrap();
            writeln!(
                out,
                "    HEADER_TEMPLATE {}",
                cmake_arg(&declaration.header_template)
            )
            .unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    // Historic preference editors include C files emitted by ilbmtoc. These
    // owners must exist before concrete targets so their ordinary #MM edges
    // can publish the private generated include directory while being bound.
    if !graph.ilbm_sources.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Embedded ILBM-to-C include sources\n\
             # ============================================================================="
        )
        .unwrap();
        let mut declarations: Vec<_> = graph.ilbm_sources.iter().collect();
        declarations.sort_by(|left, right| {
            left.owner
                .cmp(&right.owner)
                .then_with(|| left.declaring_dir.cmp(&right.declaring_dir))
                .then_with(|| left.line.cmp(&right.line))
        });
        for declaration in declarations {
            writeln!(out, "aros_declare_ilbm_sources(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(
                out,
                "    DIRECTORY {}",
                cmake_arg(&declaration.declaring_dir)
            )
            .unwrap();
            let inputs = declaration
                .pairs
                .iter()
                .map(|pair| cmake_arg(&pair.input))
                .collect::<Vec<_>>();
            let outputs = declaration
                .pairs
                .iter()
                .map(|pair| cmake_arg(&pair.output))
                .collect::<Vec<_>>();
            writeln!(out, "    INPUTS {}", inputs.join(" ")).unwrap();
            writeln!(out, "    OUTPUTS {}", outputs.join(" ")).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    // Paired hand-written FlexCat rules must register their generated C source
    // before concrete targets resolve source lanes. The matching CMake helper
    // substitutes the build-tree output for the nominal source-tree locale.c;
    // consumers are attached after targets exist below.
    if !graph.flexcat_sources.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Paired FlexCat source/header/catalog rules\n\
             # ============================================================================="
        )
        .unwrap();
        let mut declarations: Vec<_> = graph.flexcat_sources.iter().collect();
        declarations.sort_by(|left, right| {
            left.owner
                .cmp(&right.owner)
                .then_with(|| left.declaring_dir.cmp(&right.declaring_dir))
                .then_with(|| left.line.cmp(&right.line))
        });
        for declaration in declarations {
            writeln!(out, "aros_declare_flexcat_sources(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(
                out,
                "    DIRECTORY {}",
                cmake_arg(&declaration.declaring_dir)
            )
            .unwrap();
            writeln!(out, "    SOURCE {}", cmake_arg(&declaration.source)).unwrap();
            writeln!(out, "    HEADER {}", cmake_arg(&declaration.header)).unwrap();
            writeln!(
                out,
                "    DESCRIPTION {}",
                cmake_arg(&declaration.description)
            )
            .unwrap();
            writeln!(
                out,
                "    HEADER_TEMPLATE {}",
                cmake_arg(&declaration.header_template)
            )
            .unwrap();
            writeln!(
                out,
                "    SOURCE_TEMPLATE {}",
                cmake_arg(&declaration.source_template)
            )
            .unwrap();
            if let (Some(destination), Some(name), Some(source_dir)) = (
                declaration.catalog_destination.as_ref(),
                declaration.catalog_name.as_ref(),
                declaration.catalog_source_dir.as_ref(),
            ) {
                writeln!(out, "    CATALOG_DESTINATION {}", cmake_arg(destination)).unwrap();
                writeln!(out, "    CATALOG_NAME {}", cmake_arg(name)).unwrap();
                writeln!(out, "    CATALOG_SOURCE_DIR {}", cmake_arg(source_dir)).unwrap();
                let languages = declaration
                    .languages
                    .iter()
                    .map(|language| cmake_arg(language))
                    .collect::<Vec<_>>();
                writeln!(out, "    LANGUAGES {}", languages.join(" ")).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    // Object owners must precede program/module consumers. Shared ordinary
    // aggregates reference the same declaration, not duplicate Ninja outputs.
    let mut sdk_objects = std::collections::BTreeMap::new();
    let mut sdk_paths = std::collections::BTreeMap::new();
    for group in &graph.sdk_object_groups {
        for object in &group.objects {
            for path in [&object.output, &object.intermediate] {
                let key = path.to_ascii_lowercase();
                if let Some(previous) = sdk_paths.insert(key, object) {
                    if previous != object {
                        // Public generation can be called without selection.
                        // Never silently choose the first conflicting owner.
                        return format!(
                            "message(FATAL_ERROR {})\n",
                            cmake_arg(&format!("Conflicting SDK object ownership: {path}"))
                        );
                    }
                }
            }
            sdk_objects.entry(object.output.as_str()).or_insert(object);
        }
    }
    for object in sdk_objects.values() {
        let digest = aros_common::sha256_bytes(object.output.as_bytes());
        let name = format!("aros-sdk-object-{}", digest.as_str());
        writeln!(out, "aros_compile_sdk_object(").unwrap();
        let source = if object.source.starts_with("${AROS_SOURCE_DIR}/") {
            object.source.clone()
        } else {
            format!("${{AROS_SOURCE_DIR}}/{}", object.source)
        };
        for (key, value) in [
            ("NAME", name.as_str()),
            ("SOURCE", source.as_str()),
            ("INTERMEDIATE", object.intermediate.as_str()),
            ("OUTPUT", object.output.as_str()),
            ("LANGUAGE", object.language.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        for (key, values) in [
            ("DEFINES", &object.defines),
            ("UNDEFINES", &object.undefines),
            ("OPTIONS", &object.options),
            ("INCLUDES", &object.includes),
        ] {
            if !values.is_empty() {
                writeln!(out, "    {key}").unwrap();
                for value in values {
                    writeln!(out, "        {}", cmake_arg(value)).unwrap();
                }
            }
        }
        writeln!(out, ")\n").unwrap();
    }
    let mut sdk_groups: Vec<_> = graph.sdk_object_groups.iter().collect();
    sdk_groups.sort_by(|left, right| left.owner.cmp(&right.owner));
    for group in sdk_groups {
        writeln!(out, "aros_sdk_object_group(").unwrap();
        writeln!(out, "    NAME {}", cmake_arg(&group.owner)).unwrap();
        writeln!(out, "    OBJECTS").unwrap();
        for object in &group.objects {
            writeln!(out, "        {}", cmake_arg(&object.output)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }
    if !graph.sdk_object_groups.is_empty() {
        writeln!(out, "if(AROS_NATIVE_BUILD_CONTRACT_VALIDATED)\n    aros_bind_source_sdk_program_inputs()\nendif()\n").unwrap();
    }

    let mut literal_objects = std::collections::BTreeMap::new();
    for group in &graph.literal_object_groups {
        for object in &group.objects {
            let key = object.output.to_ascii_lowercase();
            if sdk_paths.contains_key(&key) {
                return format!(
                    "message(FATAL_ERROR {})\n",
                    cmake_arg(&format!(
                        "Literal and SDK object ownership conflict: {}",
                        object.output
                    ))
                );
            }
            if let Some(previous) = literal_objects.insert(key, object) {
                if previous != object {
                    return format!(
                        "message(FATAL_ERROR {})\n",
                        cmake_arg(&format!(
                            "Conflicting literal object ownership: {}",
                            object.output
                        ))
                    );
                }
            }
        }
    }
    for object in literal_objects.values() {
        let digest = aros_common::sha256_bytes(object.output.as_bytes());
        let name = format!("aros-literal-object-{}", digest.as_str());
        writeln!(out, "aros_compile_literal_object(").unwrap();
        for (key, value) in [
            ("NAME", name.as_str()),
            ("SOURCE", object.source.as_str()),
            ("OUTPUT", object.output.as_str()),
            ("LANGUAGE", object.language.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        if !object.arguments.is_empty() {
            writeln!(out, "    ARGUMENTS").unwrap();
            for value in &object.arguments {
                writeln!(out, "        {}", cmake_arg(value)).unwrap();
            }
        }
        writeln!(out, ")\n").unwrap();
    }
    let mut literal_groups: Vec<_> = graph.literal_object_groups.iter().collect();
    literal_groups.sort_by(|left, right| left.owner.cmp(&right.owner));
    for group in literal_groups {
        writeln!(
            out,
            "aros_literal_object_group(\n    NAME {}\n    OBJECTS",
            cmake_arg(&group.owner)
        )
        .unwrap();
        for object in &group.objects {
            writeln!(out, "        {}", cmake_arg(&object.output)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    let mut source_archives: Vec<_> = graph.source_archives.iter().collect();
    source_archives.sort_by(|left, right| left.declaration.owner.cmp(&right.declaration.owner));
    for archive in source_archives {
        writeln!(out, "aros_source_archive(").unwrap();
        writeln!(out, "    NAME {}", cmake_arg(&archive.declaration.owner)).unwrap();
        writeln!(out, "    OUTPUT {}", cmake_arg(&archive.declaration.output)).unwrap();
        writeln!(
            out,
            "    AR_FLAGS {}",
            cmake_arg(&archive.command.flags.join(" "))
        )
        .unwrap();
        writeln!(out, "    OBJECTS").unwrap();
        for object in &archive.members {
            writeln!(out, "        {}", cmake_arg(object)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    // 1. Concrete Module Targets. HashMap iteration is deliberately avoided:
    // reproducible generated CMake is required for meaningful comparisons,
    // and declaration order can decide which producer claims an output first.
    let mut concrete_targets: Vec<_> = graph.targets.values().collect();
    concrete_targets.sort_by(|a, b| a.mmake_name.cmp(&b.mmake_name));
    for target in concrete_targets {
        let macro_name = match target.module_type {
            ModuleType::Library => "aros_add_library",
            ModuleType::Abi => "aros_add_module_abi",
            ModuleType::ModuleHeaders => "aros_add_module_headers",
            ModuleType::Device => "aros_add_device",
            ModuleType::Resource => "aros_add_resource",
            ModuleType::Hidd => "aros_add_hidd",
            ModuleType::Datatype => "aros_add_datatype",
            ModuleType::Gadget => "aros_add_gadget",
            ModuleType::Mcc => "aros_add_mcc",
            ModuleType::Program => "aros_add_program",
            ModuleType::ProgramGroup => "aros_add_programs",
            ModuleType::SimpleModule => "aros_add_module_simple",
            ModuleType::LinkLib => "aros_add_linklib",
            _ => "aros_add_custom_target",
        };

        // Every declaration is emitted, whatever architecture its sources belong
        // to. Restricting the emission was tempting, but a hard-coded
        // `if(AROS_TARGET_PLATFORM STREQUAL "pc")` around the declaration made
        // 46 targets disappear from the build graph entirely, so nothing could
        // report on them: they were neither built nor listed as skipped, and a
        // newly added arch/ directory would have joined them silently.
        //
        // aros_gate_arch() in cmake/AROS.cmake does the filtering instead. It
        // reads AROS_ARCH_SOURCE_DIRS, so it covers every architecture rather
        // than the four spelled out here, and it excludes the target from `all`
        // while keeping it nameable, which is what makes it possible to ask
        // whether a foreign-architecture target would build.
        if let Some(form) = target.module_macro {
            writeln!(
                out,
                "aros_record_module_macro(OWNER {} FORM {})",
                target.mmake_name,
                form.cmake_form()
            )
            .unwrap();
        }
        if target.module_type == ModuleType::ModuleHeaders {
            writeln!(out, "{macro_name}(").unwrap();
            writeln!(out, "    TARGET {}", cmake_arg(&target.target_name)).unwrap();
            writeln!(out, "    MMAKE_ID {}", cmake_arg(&target.mmake_name)).unwrap();
            writeln!(
                out,
                "    DIRECTORY {}",
                cmake_arg(&format!(
                    "${{AROS_SOURCE_DIR}}/{}",
                    target.dir_path.display()
                ))
            )
            .unwrap();
            if let Some(kind) = target.declared_mod_type.as_deref() {
                writeln!(out, "    MODTYPE {}", cmake_arg(kind)).unwrap();
            }
            if let Some(suffix) = target.mod_suffix.as_deref() {
                writeln!(out, "    MODSUFFIX {}", cmake_arg(suffix)).unwrap();
            }
            writeln!(out, ")\n").unwrap();
            continue;
        }
        writeln!(out, "{macro_name}(").unwrap();
        writeln!(out, "    TARGET {}", target.target_name).unwrap();
        writeln!(out, "    MMAKE_ID {}", target.mmake_name).unwrap();
        if target.genmodule_only {
            writeln!(out, "    GENMODULE_ONLY").unwrap();
        }
        if target.always_cxx_link {
            writeln!(out, "    ALWAYS_CXX_LINK").unwrap();
        }
        if target.no_startup {
            writeln!(out, "    NO_STARTUP").unwrap();
        }
        if target.detach {
            writeln!(out, "    DETACH").unwrap();
        }
        if target.empty_archive {
            writeln!(out, "    EMPTY_ARCHIVE").unwrap();
        }
        // The 32-bit flavour of an archive, which the declaration states by
        // pointing libdir/objdir at a 32-bit location and setting
        // `ISA_FLAGS := $(ISA_32_FLAGS)`. That value is an Autoconf one with no
        // counterpart here, so CMake substitutes the 32-bit form of the triple
        // it already chooses per CPU (cmake/AROS.cmake:301). Without it
        // gen/lib32 holds 64-bit objects, and the 32-bit PC bootstrap cannot
        // link against them.
        if target.variant_32bit {
            writeln!(out, "    VARIANT_32BIT").unwrap();
        }
        if let Some(linklib_name) = &target.linklib_name {
            writeln!(out, "    LINKLIB_NAME {}", cmake_arg(linklib_name)).unwrap();
        }
        if let Some(selected) = graph
            .native_selected_client_archives
            .as_ref()
            .filter(|_| target.module_type == ModuleType::Library && !target.genmodule_only)
        {
            // Public ABI headers are still needed by the runtime. Native
            // selection must not publish an unrequested client archive into
            // the same namespace as a selected implementation archive.
            let normal = selected.contains(&format!("{}-linklib", target.mmake_name));
            let relative = selected.contains(&format!("{}-linklib-rel", target.mmake_name));
            match (normal, relative) {
                (false, false) => writeln!(out, "    NO_CLIENT_ARCHIVES").unwrap(),
                (false, true) => writeln!(out, "    NO_NORMAL_CLIENT_ARCHIVE").unwrap(),
                (true, false) => writeln!(out, "    NO_RELATIVE_CLIENT_ARCHIVE").unwrap(),
                (true, true) => {}
            }
        }
        if let Some(genmodule) = target
            .genmodule_linklibs
            .as_ref()
            .filter(|metadata| metadata.enabled)
        {
            writeln!(out, "    GENMODULE_LINKLIBS").unwrap();
            if !genmodule.source_files.is_empty() {
                let sources: Vec<String> = genmodule
                    .source_files
                    .iter()
                    .map(|source| cmake_arg(source))
                    .collect();
                writeln!(out, "    LINKLIB_SOURCES {}", sources.join(" ")).unwrap();
            }
            if !genmodule.object_sources.is_empty() {
                let sources: Vec<String> = genmodule
                    .object_sources
                    .iter()
                    .map(|source| cmake_arg(source))
                    .collect();
                writeln!(out, "    LINKLIB_OBJECT_SOURCES {}", sources.join(" ")).unwrap();
            }
        }
        if target.canonical_linklib_output {
            writeln!(out, "    CANONICAL_OUTPUT").unwrap();
        }
        if let Some(output_dir) = &target.linklib_output_dir {
            writeln!(out, "    OUTPUT_DIR {}", cmake_arg(output_dir)).unwrap();
        }
        if !target.link_libs.is_empty() {
            let libs: Vec<String> = target.link_libs.iter().map(|l| cmake_arg(l)).collect();
            writeln!(out, "    LIBS {}", libs.join(" ")).unwrap();
        }
        if let Some(arches) = kickstart_members.get(&target.mmake_name) {
            let arches: Vec<String> = arches.iter().map(|a| cmake_arg(a)).collect();
            writeln!(out, "    KICKSTART_MEMBER {}", arches.join(" ")).unwrap();
        }
        if let Some(mod_type) = &target.declared_mod_type {
            writeln!(out, "    MODTYPE {}", cmake_arg(mod_type)).unwrap();
        }
        if let Some(suffix) = &target.mod_suffix {
            writeln!(out, "    MODSUFFIX {}", cmake_arg(suffix)).unwrap();
        }
        writeln!(
            out,
            "    DIRECTORY \"${{AROS_SOURCE_DIR}}/{}\"",
            target.dir_path.display()
        )
        .unwrap();
        if let Some(target_dir) = &target.target_dir {
            writeln!(out, "    INSTALL_DIR {}", cmake_arg(target_dir)).unwrap();
        }

        for (keyword, sources) in [
            ("SOURCES", &target.source_files),
            ("CXX_SOURCES", &target.cxx_source_files),
            ("OBJC_SOURCES", &target.objc_source_files),
            ("ASM_SOURCES", &target.asm_source_files),
        ] {
            if !sources.is_empty() {
                let quoted: Vec<String> = sources.iter().map(|source| cmake_arg(source)).collect();
                writeln!(out, "    {keyword} {}", quoted.join(" ")).unwrap();
            }
        }

        if !target.use_libs.is_empty() {
            writeln!(out, "    USELIBS {}", target.use_libs.join(" ")).unwrap();
        }

        if !target.include_dirs.is_empty() {
            let quoted: Vec<String> = target.include_dirs.iter().map(|d| cmake_arg(d)).collect();
            writeln!(out, "    INCLUDES {}", quoted.join(" ")).unwrap();
        }

        // Architecture-conditional includes are emitted as `<tag>|<path>` pairs.
        // CMake keeps the ones whose tag applies to the configured target; see
        // aros_arch_include_tags() in cmake/AROS.cmake.
        if !target.arch_includes.is_empty() {
            let pairs: Vec<String> = target
                .arch_includes
                .iter()
                .map(|(tag, dir)| cmake_arg(&format!("{tag}|{dir}")))
                .collect();
            writeln!(out, "    ARCH_INCLUDES {}", pairs.join(" ")).unwrap();
        }

        // Preprocessor state the sources depend on, from USER_CPPFLAGS /
        // USER_CFLAGS. Quoted so a value containing a CMake variable survives.
        if !target.defines.is_empty() {
            let quoted: Vec<String> = target.defines.iter().map(|d| cmake_arg(d)).collect();
            writeln!(out, "    DEFINES {}", quoted.join(" ")).unwrap();
        }
        if !target.undefines.is_empty() {
            let quoted: Vec<String> = target.undefines.iter().map(|d| cmake_arg(d)).collect();
            writeln!(out, "    UNDEFINES {}", quoted.join(" ")).unwrap();
        }
        // Architecture source overrides as "<tag>|<dir>|<f1>,<f2>,...".
        // CMake keeps the tags that apply, drops the same-named generic
        // sources and puts the architecture ones first, as the reference build
        // does (config/make.tmpl:1661).
        if !target.arch_sources.is_empty() {
            let entries: Vec<String> = target
                .arch_sources
                .iter()
                .map(|(tag, dir, files)| cmake_arg(&format!("{tag}|{dir}|{}", files.join(","))))
                .collect();
            writeln!(out, "    ARCH_SOURCES {}", entries.join(" ")).unwrap();
        }

        // Architecture-conditional flags from a make.opts, same "<tag>|<value>"
        // shape as ARCH_INCLUDES.
        if !target.arch_defines.is_empty() {
            let pairs: Vec<String> = target
                .arch_defines
                .iter()
                .map(|(tag, d)| cmake_arg(&format!("{tag}|{d}")))
                .collect();
            writeln!(out, "    ARCH_DEFINES {}", pairs.join(" ")).unwrap();
        }
        if !target.arch_compile_options.is_empty() {
            let pairs: Vec<String> = target
                .arch_compile_options
                .iter()
                .map(|(tag, o)| cmake_arg(&format!("{tag}|{o}")))
                .collect();
            writeln!(out, "    ARCH_COMPILE_OPTIONS {}", pairs.join(" ")).unwrap();
        }

        if !target.compile_options.is_empty() {
            let quoted: Vec<String> = target
                .compile_options
                .iter()
                .map(|o| cmake_arg(o))
                .collect();
            writeln!(out, "    COMPILE_OPTIONS {}", quoted.join(" ")).unwrap();
        }

        // Only a standalone-executable link uses these, and only a program can
        // be one. Emitted for anything else they corrupt the call: a keyword a
        // builder does not accept is read as one more value of the preceding
        // multi-value argument, and `DEFINES ... DRIVER_LINK_OPTIONS -static`
        // duly reached the compiler as `-DDRIVER_LINK_OPTIONS -D-static`.
        let standalone_capable = target.module_type == ModuleType::Program;
        if standalone_capable && !target.driver_link_options.is_empty() {
            let options: Vec<String> = target
                .driver_link_options
                .iter()
                .map(|option| cmake_arg(option))
                .collect();
            writeln!(out, "    DRIVER_LINK_OPTIONS {}", options.join(" ")).unwrap();
        }
        if standalone_capable && !target.isa_link_options.is_empty() {
            let options: Vec<String> = target
                .isa_link_options
                .iter()
                .map(|option| cmake_arg(option))
                .collect();
            writeln!(out, "    ISA_LINK_OPTIONS {}", options.join(" ")).unwrap();
        }
        if !target.link_options.is_empty() {
            let quoted: Vec<String> = target
                .link_options
                .iter()
                .map(|option| cmake_arg(option))
                .collect();
            writeln!(out, "    LINK_OPTIONS {}", quoted.join(" ")).unwrap();
        }

        writeln!(out, ")").unwrap();
        for product in graph
            .sdk_program_outputs
            .iter()
            .filter(|product| product.owner == target.mmake_name)
        {
            writeln!(
                out,
                "aros_register_sdk_program_file(NAME {} OUTPUT {})",
                cmake_arg(&product.owner),
                cmake_arg(&product.output)
            )
            .unwrap();
        }
        if target
            .module_macro
            .is_some_and(|form| form != crate::ast::ModuleMacroForm::AbiOnly)
        {
            writeln!(out, "aros_record_module_kobj_sources(").unwrap();
            writeln!(out, "    OWNER {}", target.mmake_name).unwrap();
            writeln!(
                out,
                "    DIRECTORY \"${{AROS_SOURCE_DIR}}/{}\"",
                target.dir_path.display()
            )
            .unwrap();
            for (keyword, sources) in [
                ("SOURCES", &target.source_files),
                ("CXX_SOURCES", &target.cxx_source_files),
                ("OBJC_SOURCES", &target.objc_source_files),
                ("ASM_SOURCES", &target.asm_source_files),
            ] {
                if !sources.is_empty() {
                    let quoted: Vec<_> = sources.iter().map(|source| cmake_arg(source)).collect();
                    writeln!(out, "    {keyword} {}", quoted.join(" ")).unwrap();
                }
            }
            if !target.arch_sources.is_empty() {
                let entries: Vec<_> = target
                    .arch_sources
                    .iter()
                    .map(|(tag, dir, files)| cmake_arg(&format!("{tag}|{dir}|{}", files.join(","))))
                    .collect();
                writeln!(out, "    ARCH_SOURCES {}", entries.join(" ")).unwrap();
            }
            writeln!(out, ")").unwrap();
            if let Some(inputs) = &target.kobj_scoped_inputs {
                let json = serde_json::to_string(inputs)
                    .expect("KOBJ input metadata contains only serializable source facts");
                writeln!(out, "aros_record_module_kobj_inputs(").unwrap();
                writeln!(out, "    OWNER {}", target.mmake_name).unwrap();
                writeln!(out, "    JSON {}", cmake_literal_arg(&json)).unwrap();
                writeln!(out, ")").unwrap();
            }
        }
        writeln!(out).unwrap();
    }

    // Mesa 26's ARM pipe HIDDs import core functions across module
    // boundaries. Only the audited VC4 declaration enables the single
    // GalliumCoreAPI provider/consumer graph; the helper validates its exact
    // source inputs and verified target tools before it creates any output.
    if graph
        .targets
        .get("linklibs-gallium_vc4")
        .is_some_and(|target| {
            target
                .defines
                .iter()
                .any(|value| value == "AROS_MESA_MAJOR=26")
        })
    {
        writeln!(out, "aros_build_mesa26_gallium_core_api()\n").unwrap();
    }

    // Python output groups had to register their clean-tree products before
    // source resolution. Their compile consumers exist now, so attach the
    // explicit owner edges without relying on include discovery.
    if !graph.python_outputs.is_empty() {
        let mut declarations: Vec<_> = graph.python_outputs.iter().collect();
        declarations.sort_by(|left, right| left.owner.cmp(&right.owner));
        for declaration in declarations {
            if declaration.consumers.is_empty() {
                continue;
            }
            let consumers = declaration
                .consumers
                .iter()
                .map(|consumer| cmake_arg(consumer))
                .collect::<Vec<_>>();
            writeln!(out, "aros_bind_python_output_consumers(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    // Same two-step for the in-tree script generators: the declaration above
    // registers the outputs, and the binding orders the consumer's compiles
    // after the generator. That ordering is what covers a generated *header*
    // the rule does not name -- udis86's script writes itab.h beside the
    // declared itab.c, and every object of the archive includes it.
    if !graph.script_outputs.is_empty() {
        let mut declarations: Vec<_> = graph.script_outputs.iter().collect();
        declarations.sort_by(|left, right| left.owner.cmp(&right.owner));
        for declaration in declarations {
            if declaration.consumers.is_empty() {
                continue;
            }
            let consumers = declaration
                .consumers
                .iter()
                .map(|consumer| cmake_arg(consumer))
                .collect::<Vec<_>>();
            writeln!(out, "aros_bind_python_output_consumers(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    // A configure-style build that links an in-tree link library has to be
    // declared after that library's target exists, for the same reason the AHI
    // block below does: aros_build_configure asks the target where its archive
    // is. WirelessManager's wpa_supplicant links libmui, which was spelled as
    // `<build root>/liblinklibs-mui.a` until linklibs-mui became canonical;
    // after that the declaration only kept working because a file from an
    // earlier configuration was still lying in the build root (OPEN-POINTS 44).
    emit_configure_builds(
        &mut out,
        graph
            .configure_builds
            .iter()
            .filter(|declaration| !declaration.dependency_targets.is_empty())
            .collect(),
        "Configure-style builds that consume a link library",
    );

    // Emitted after every concrete target, not with the other
    // capability-checked builds: aros_build_ahi asks the three link-library
    // targets where their archives are, and a linklib's archive name and
    // directory depend on whether anything named it -- linklibs-libm is
    // private while linklibs-amiga and linklibs-mui are canonical. Declared
    // before the targets exist, the helper could only guess a filename, and
    // the guess broke the moment a consumer promoted one of them.
    // The AHI subsystem is another configure-style build syntactically, but
    // it needs a fixed AROS source/product closure and the private host sfdc
    // compiler.  Its helper intentionally accepts neither arbitrary options
    // nor a command string, so do not collapse it into aros_build_configure.
    if !graph.ahi_builds.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Capability-checked AHI subsystem builds\n\
             # ============================================================================="
        )
        .unwrap();
        let mut declarations: Vec<_> = graph.ahi_builds.iter().collect();
        declarations.sort_by(|left, right| left.mmake_name.cmp(&right.mmake_name));
        for declaration in declarations {
            writeln!(out, "aros_build_ahi(").unwrap();
            writeln!(out, "    MMAKE_ID {}", declaration.mmake_name).unwrap();
            writeln!(out, "    MODE {}", cmake_arg(&declaration.mode)).unwrap();
            writeln!(out, "    BINARY_DIR {}", cmake_arg(&declaration.binary_dir)).unwrap();
            writeln!(
                out,
                "    INSTALL_PREFIX {}",
                cmake_arg(&declaration.install_prefix)
            )
            .unwrap();
            writeln!(out, "    HOST_SFDC {}", cmake_arg(&declaration.host_sfdc)).unwrap();
            writeln!(out, "    HOST_PERL {}", cmake_arg(&declaration.host_perl)).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    // The source substitution above only registers output ownership. Bind the
    // exact compile targets once they have been declared, so a direct request
    // for NListtree/NListviews orders its generated locale.c/.h and keeps the
    // generated header on a private quoted-include path.
    if !graph.flexcat_sources.is_empty() {
        let mut declarations: Vec<_> = graph.flexcat_sources.iter().collect();
        declarations.sort_by(|left, right| left.owner.cmp(&right.owner));
        for declaration in declarations {
            if declaration.consumers.is_empty() {
                continue;
            }
            let consumers = declaration
                .consumers
                .iter()
                .map(|consumer| cmake_arg(consumer))
                .collect::<Vec<_>>();
            writeln!(out, "aros_bind_flexcat_source_consumers(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    // A declaration can link a provider whose reproducible lexical sort key
    // follows the consumer (Atheros' device precedes its HAL, for example).
    // Resolve those forward references only after every concrete target and
    // generated link-library product has had a chance to exist.
    writeln!(out, "aros_finalize_link_libraries()\n").unwrap();

    // Declaration-owned literal define headers. Concrete compile targets have
    // already been declared, so the helper can attach direct dependencies and
    // the output directory as a private include path without deferred target
    // lookup. The owner is a real output target, never a configure-time phony.
    if !graph.define_headers.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Generated headers from literal define fragments\n\
             # ============================================================================="
        )
        .unwrap();
        let mut headers: Vec<_> = graph.define_headers.iter().collect();
        headers.sort_by(|left, right| {
            left.owner
                .cmp(&right.owner)
                .then_with(|| left.file.cmp(&right.file))
                .then_with(|| left.line.cmp(&right.line))
        });
        for header in headers {
            writeln!(out, "aros_generate_defines_header(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&header.owner)).unwrap();
            writeln!(out, "    OUTPUT {}", cmake_arg(&header.output)).unwrap();
            let definitions: Vec<_> = header
                .definitions
                .iter()
                .map(|definition| cmake_arg(definition))
                .collect();
            writeln!(out, "    DEFINES {}", definitions.join(" ")).unwrap();
            if !header.dependencies.is_empty() {
                let dependencies: Vec<_> = header
                    .dependencies
                    .iter()
                    .map(|dependency| cmake_arg(dependency))
                    .collect();
                writeln!(out, "    DEPENDS {}", dependencies.join(" ")).unwrap();
            }
            if !header.consumers.is_empty() {
                let consumers: Vec<_> = header
                    .consumers
                    .iter()
                    .map(|consumer| cmake_arg(consumer))
                    .collect();
                writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    if !graph.bison_outputs.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Generated C sources from exact host-Bison recipes\n\
             # ============================================================================="
        )
        .unwrap();
        let mut outputs: Vec<_> = graph.bison_outputs.iter().collect();
        outputs.sort_by(|left, right| left.output.cmp(&right.output));
        for declaration in outputs {
            writeln!(out, "aros_generate_bison_output(").unwrap();
            writeln!(out, "    OWNER {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(out, "    INPUT {}", cmake_arg(&declaration.input)).unwrap();
            writeln!(out, "    OUTPUT {}", cmake_arg(&declaration.output)).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    for declaration in &graph.directory_setups {
        writeln!(out, "aros_prepare_directories(").unwrap();
        writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
        let directories = declaration
            .directories
            .iter()
            .map(|directory| cmake_arg(directory))
            .collect::<Vec<_>>();
        writeln!(out, "    DIRECTORIES {}", directories.join(" ")).unwrap();
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.genmodule_header_rules {
        writeln!(out, "aros_genmodule_header_stamp(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("LAYOUT", declaration.layout.as_str()),
            ("DECLARING_DIR", declaration.declaring_dir.as_str()),
            ("CONFIG", declaration.config.as_str()),
            ("MODULE", declaration.module.as_str()),
            ("MODTYPE", declaration.modtype.as_str()),
            ("INCLUDE_NAME", declaration.include_name.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.genmodule_writefiles_rules {
        writeln!(out, "aros_genmodule_writefiles_stamp(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("CONFIG", declaration.config.as_str()),
            ("MODULE", declaration.module.as_str()),
            ("MODTYPE", declaration.modtype.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.sfd_header_rules {
        writeln!(out, "aros_generate_sfd_headers(").unwrap();
        writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
        writeln!(out, "    FILE {}", cmake_arg(&declaration.file)).unwrap();
        writeln!(
            out,
            "    FILE_SHA256 {}",
            cmake_arg(&declaration.file_sha256)
        )
        .unwrap();
        writeln!(out, "    JOBS").unwrap();
        for job in &declaration.jobs {
            let encoded = format!(
                "{}|{}|{}|{}|{}",
                job.mode, job.target, job.input, job.input_sha256, job.sdk_output
            );
            writeln!(out, "        {}", cmake_arg(&encoded)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.sdk_text_rules {
        writeln!(out, "aros_transform_sdk_text(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("INPUT", declaration.input.as_str()),
            ("OUTPUT", declaration.output.as_str()),
            ("FETCH", declaration.fetch_owner.as_str()),
            ("FILE", declaration.file.as_str()),
            ("FILE_SHA256", declaration.file_sha256.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        writeln!(out, "    OPERATIONS").unwrap();
        for operation in &declaration.operations {
            writeln!(
                out,
                "        {}",
                cmake_literal_arg(&operation.cmake_argument())
            )
            .unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.source_text_rules {
        for product in &declaration.outputs {
            writeln!(out, "aros_transform_source_text(").unwrap();
            for (key, value) in [
                ("NAME", declaration.owner.as_str()),
                ("INPUT", product.input.as_str()),
                ("OUTPUT", product.output.as_str()),
                ("FETCH", declaration.fetch_owner.as_str()),
            ] {
                writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
            }
            let operations = serde_json::to_string(&product.operations)
                .expect("closed source text operations serialize as JSON");
            // The closed scanner reserves this deferred root for source Make
            // directory expressions. Other dollars (for example pkg-config
            // ${prefix}) remain literal; never expose arbitrary CMake names.
            let operations_argument = cmake_literal_arg(&operations)
                .replace("\\${AROS_BUILD_DIR}", "${AROS_BUILD_DIR}")
                .replace("\\${AROS_SDK_INCLUDE_DIR}", "${AROS_SDK_INCLUDE_DIR}")
                .replace("\\${AROS_GENINC_DIR}", "${AROS_GENINC_DIR}");
            writeln!(out, "    OPERATIONS_JSON {operations_argument}").unwrap();
            if let Some(mode) = &product.mode {
                writeln!(out, "    MODE {}", cmake_arg(mode)).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    for declaration in &graph.source_value_rules {
        writeln!(out, "aros_extract_source_value(").unwrap();
        let mut marker_hex = String::with_capacity(declaration.marker.len() * 2);
        for byte in declaration.marker.as_bytes() {
            write!(marker_hex, "{byte:02x}").unwrap();
        }
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("INPUT", declaration.input.as_str()),
            ("OUTPUT", declaration.output.as_str()),
            ("MARKER_HEX", marker_hex.as_str()),
            ("FILE", declaration.file.as_str()),
            ("FILE_SHA256", declaration.file_sha256.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }
    for declaration in &graph.sdk_file_copies {
        writeln!(out, "aros_stage_sdk_files(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("SOURCE", declaration.source_dir.as_str()),
            ("DESTINATION", declaration.destination.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        if let Some(fetch) = &declaration.fetch_owner {
            writeln!(out, "    FETCH {}", cmake_arg(fetch)).unwrap();
        }
        writeln!(out, "    FILES").unwrap();
        for file in &declaration.files {
            writeln!(out, "        {}", cmake_literal_arg(file)).unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    let mut pending_assets: Vec<_> = graph.sdk_asset_rules.iter().collect();
    pending_assets.sort_by(|a, b| a.owner.cmp(&b.owner));
    let asset_owners: std::collections::BTreeSet<_> = pending_assets
        .iter()
        .map(|rule| rule.owner.as_str())
        .collect();
    let mut emitted_assets = std::collections::BTreeSet::new();
    while !pending_assets.is_empty() {
        let next = pending_assets.iter().position(|rule| {
            graph
                .sdk_asset_dependencies(&rule.owner, None)
                .is_ok_and(|dependencies| {
                    dependencies.iter().all(|owner| {
                        !asset_owners.contains(owner.as_str()) || emitted_assets.contains(owner)
                    })
                })
        });
        let Some(index) = next else {
            writeln!(out, "message(FATAL_ERROR \"SDK assets require unique concrete input producers and an acyclic file graph\")").unwrap();
            break;
        };
        let declaration = pending_assets.remove(index);
        for operation in &declaration.operations {
            writeln!(out, "aros_sdk_asset_rule(").unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
            match &operation.operation {
                crate::sdk_asset_rules::SdkAssetOperation::Copy { input, output } => {
                    let producers = graph.sdk_asset_producers(None);
                    let producer = producers[&input.to_ascii_lowercase()]
                        .iter()
                        .next()
                        .unwrap();
                    writeln!(out, "    INPUT {}", cmake_arg(input)).unwrap();
                    writeln!(out, "    OUTPUT {}", cmake_arg(output)).unwrap();
                    writeln!(out, "    PRODUCER {}", cmake_arg(producer)).unwrap();
                }
                crate::sdk_asset_rules::SdkAssetOperation::WriteText { output, text } => {
                    let mut hex = String::new();
                    for byte in text.as_bytes() {
                        write!(hex, "{byte:02x}").unwrap();
                    }
                    writeln!(out, "    OUTPUT {}", cmake_arg(output)).unwrap();
                    writeln!(out, "    TEXT_HEX {}", cmake_literal_arg(&hex)).unwrap();
                }
            }
            writeln!(out, ")\n").unwrap();
        }
        emitted_assets.insert(declaration.owner.clone());
    }

    for declaration in &graph.host_header_aggregates {
        writeln!(out, "aros_host_header_aggregate(").unwrap();
        writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
        writeln!(out, "    TOOL {}", cmake_arg(&declaration.tool)).unwrap();
        writeln!(
            out,
            "    SOURCE {}",
            cmake_arg(&format!("${{AROS_SOURCE_DIR}}/{}", declaration.tool_source))
        )
        .unwrap();
        if !declaration.host_compile_flags.is_empty() {
            writeln!(
                out,
                "    COMPILE_FLAGS {}",
                declaration
                    .host_compile_flags
                    .iter()
                    .map(|flag| cmake_literal_arg(flag))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .unwrap();
        }
        writeln!(out, ")\n").unwrap();
        for header in &declaration.headers {
            writeln!(out, "aros_host_header_aggregate_output(").unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&declaration.owner)).unwrap();
            writeln!(out, "    HEADER {}", cmake_literal_arg(&header.header)).unwrap();
            if header.generated_mirror {
                writeln!(out, "    GENERATED_MIRROR").unwrap();
            }
            if !header.arguments.is_empty() {
                writeln!(
                    out,
                    "    ARGUMENTS {}",
                    header
                        .arguments
                        .iter()
                        .map(|argument| cmake_literal_arg(argument))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
                .unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    for declaration in &graph.host_header_rules {
        writeln!(out, "aros_host_header_rule(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.as_str()),
            ("SETUP_NAME", declaration.setup_owner.as_str()),
            ("TOOL_SOURCE", declaration.tool_source.as_str()),
            ("TOOL_OUTPUT", declaration.tool_output.as_str()),
            ("SOURCE_WORKDIR", declaration.source_workdir.as_str()),
            ("HEADER", declaration.header.as_str()),
            ("PRIMARY_OUTPUT", declaration.primary_output.as_str()),
            ("SDK_OUTPUT", declaration.sdk_output.as_str()),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(value)).unwrap();
        }
        if declaration.use_configured_host_cflags {
            writeln!(out, "    USE_CONFIGURED_HOST_CFLAGS").unwrap();
        }
        for (key, values) in [
            ("COMPILE_FLAGS", &declaration.host_compile_flags),
            ("SOURCE_PREREQUISITES", &declaration.source_prerequisites),
            ("SETUP_DIRECTORIES", &declaration.setup_directories),
        ] {
            if !values.is_empty() {
                let values = values
                    .iter()
                    .map(|value| cmake_arg(value))
                    .collect::<Vec<_>>();
                writeln!(out, "    {key} {}", values.join(" ")).unwrap();
            }
        }
        writeln!(out, ")\n").unwrap();
    }

    for declaration in &graph.host_file_generators {
        let manifest = serde_json::to_string(&declaration.inputs.iter().map(|input| {
            serde_json::json!({"filename": input.filename, "sha256": input.sha256, "size": input.size})
        }).collect::<Vec<_>>()).expect("host inputs serialize");
        let source_manifest = serde_json::to_string(
            &graph
                .host_file_generator_source_digests
                .iter()
                .map(|(path, sha256)| serde_json::json!({"path": path, "sha256": sha256}))
                .collect::<Vec<_>>(),
        )
        .expect("source inputs serialize");
        let tool_sha = graph
            .host_file_generator_source_digests
            .get(&declaration.tool_source)
            .expect("admitted source-owned host tool must have its sealed digest");
        writeln!(out, "aros_host_c_file_generator(").unwrap();
        for (key, value) in [
            ("NAME", declaration.owner.clone()),
            (
                "TOOL_SOURCE",
                format!("${{AROS_SOURCE_DIR}}/{}", declaration.tool_source),
            ),
            ("TOOL_SHA256", tool_sha.clone()),
            (
                "OUTPUT",
                format!("${{AROS_BUILD_DIR}}/{}", declaration.output),
            ),
            (
                "INPUT_DIRECTORY",
                "${AROS_NATIVE_HOST_INPUT_DIRECTORY}".into(),
            ),
        ] {
            writeln!(out, "    {key} {}", cmake_arg(&value)).unwrap();
        }
        writeln!(
            out,
            "    INPUT_MANIFEST_JSON {}",
            cmake_literal_arg(&manifest)
        )
        .unwrap();
        writeln!(
            out,
            "    SOURCE_MANIFEST_JSON {}",
            cmake_literal_arg(&source_manifest)
        )
        .unwrap();
        for (key, values) in [
            ("COMPILE_FLAGS", &declaration.compile_flags),
            ("ARGUMENTS", &declaration.arguments),
        ] {
            writeln!(
                out,
                "    {key} {}",
                values
                    .iter()
                    .map(|value| cmake_literal_arg(value))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    // Safe hand-written header transforms.  Concrete consumers have already
    // been declared, while fetch targets were emitted first, so CMake can bind
    // both sides directly without deferred target-name guessing.
    if !graph.header_transforms.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Generated headers from safe literal transforms\n\
             # ============================================================================="
        )
        .unwrap();
        let mut transforms: Vec<_> = graph.header_transforms.iter().collect();
        transforms.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.file.cmp(&right.file))
                .then_with(|| left.line.cmp(&right.line))
        });
        for transform in transforms {
            writeln!(out, "aros_transform_header(").unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&transform.name)).unwrap();
            writeln!(out, "    INPUT {}", cmake_arg(&transform.input)).unwrap();
            writeln!(out, "    OUTPUT {}", cmake_arg(&transform.output)).unwrap();
            if let Some(owner) = &transform.generated_input_owner {
                writeln!(out, "    GENERATED_INPUT_OWNER {}", cmake_arg(owner)).unwrap();
            }
            if transform.copy_only {
                writeln!(out, "    COPY_ONLY").unwrap();
            } else if !transform.substitutions.is_empty() {
                let substitutions: Vec<_> = transform
                    .substitutions
                    .iter()
                    .map(|value| cmake_arg(value))
                    .collect();
                writeln!(out, "    SUBSTITUTIONS {}", substitutions.join(" ")).unwrap();
            } else {
                if transform.replace_whole_line_containing {
                    writeln!(out, "    WHOLE_LINE_CONTAINING").unwrap();
                }
                writeln!(out, "    MATCH {}", cmake_arg(&transform.match_text)).unwrap();
                writeln!(
                    out,
                    "    REPLACEMENT {}",
                    cmake_literal_arg(&transform.replacement)
                )
                .unwrap();
            }
            if !transform.dependencies.is_empty() {
                let dependencies: Vec<_> = transform
                    .dependencies
                    .iter()
                    .map(|dependency| cmake_arg(dependency))
                    .collect();
                writeln!(out, "    DEPENDS {}", dependencies.join(" ")).unwrap();
            }
            if !transform.consumers.is_empty() {
                let consumers: Vec<_> = transform
                    .consumers
                    .iter()
                    .map(|consumer| cmake_arg(consumer))
                    .collect();
                writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            }
            writeln!(out, ")\n").unwrap();
        }
    }

    // Workbench .info resources. Identities are declared separately from
    // their output rules so an unresolved or architecture-empty declaration
    // remains a real, nameable target and keeps its #MM edges.
    if !graph.icon_targets.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Workbench icons (from %build_icons)\n\
             # ============================================================================="
        )
        .unwrap();

        let mut icon_targets: Vec<_> = graph.icon_targets.values().collect();
        icon_targets.sort_by(|a, b| a.mmake.cmp(&b.mmake));
        for target in icon_targets {
            writeln!(out, "aros_declare_icon_target(").unwrap();
            // Deliberately unquoted: aros-verify reads the token as written.
            writeln!(out, "    MMAKE_ID {}", target.mmake).unwrap();
            writeln!(out, "    DIRECTORY {}", cmake_arg(&target.directory)).unwrap();
            writeln!(out, ")").unwrap();
        }
        writeln!(out).unwrap();

        let mut icons: Vec<_> = graph.icons.iter().collect();
        icons.sort_by(|a, b| {
            a.srcdir
                .cmp(&b.srcdir)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.dir.cmp(&b.dir))
                .then_with(|| a.mmake.cmp(&b.mmake))
                .then_with(|| a.condition.cmp(&b.condition))
        });
        for icon in icons {
            if let Some(condition) = &icon.condition {
                writeln!(out, "if({condition})").unwrap();
            }
            writeln!(out, "aros_build_icons(").unwrap();
            writeln!(out, "    MMAKE_ID {}", icon.mmake).unwrap();
            writeln!(out, "    DIRECTORY {}", cmake_arg(&icon.srcdir)).unwrap();
            writeln!(out, "    DESTINATION {}", cmake_arg(&icon.dir)).unwrap();
            writeln!(out, "    FORMAT {}", cmake_arg(&icon.fmt)).unwrap();
            if let Some(iconset) = &icon.iconset {
                writeln!(out, "    ICONSET {}", cmake_arg(iconset)).unwrap();
            }
            if !icon.icons.is_empty() {
                let values: Vec<_> = icon.icons.iter().map(|v| cmake_arg(v)).collect();
                writeln!(out, "    ICONS {}", values.join(" ")).unwrap();
            }
            if !icon.images.is_empty() {
                let values: Vec<_> = icon.images.iter().map(|v| cmake_arg(v)).collect();
                writeln!(out, "    IMAGES {}", values.join(" ")).unwrap();
            }
            writeln!(out, ")").unwrap();
            if icon.condition.is_some() {
                writeln!(out, "endif()").unwrap();
            }
            writeln!(out).unwrap();
        }
    }

    // Translated Locale catalogs. Each declaration owns every requested
    // `.catalog` plus its optional generated source/header; unresolved
    // declarations are deliberately absent and remain visible in the skip and
    // coverage reports rather than becoming phony stubs.
    if !graph.catalogs.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Locale catalogs (from %build_catalogs)\n\
             # ============================================================================="
        )
        .unwrap();

        let mut catalogs: Vec<_> = graph.catalogs.iter().collect();
        catalogs.sort_by(|a, b| {
            a.mmake
                .cmp(&b.mmake)
                .then_with(|| a.declaring_dir.cmp(&b.declaring_dir))
                .then_with(|| a.line.cmp(&b.line))
        });
        for catalog in catalogs {
            writeln!(out, "aros_build_catalogs(").unwrap();
            // Deliberately unquoted: aros-verify reads this token as written.
            writeln!(out, "    MMAKE_ID {}", catalog.mmake).unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&catalog.name)).unwrap();
            writeln!(out, "    SUBDIR {}", cmake_arg(&catalog.subdir)).unwrap();
            writeln!(out, "    DIRECTORY {}", cmake_arg(&catalog.declaring_dir)).unwrap();
            writeln!(out, "    SOURCE_DIR {}", cmake_arg(&catalog.srcdir)).unwrap();
            writeln!(out, "    DESTINATION {}", cmake_arg(&catalog.dir)).unwrap();
            writeln!(out, "    DESCRIPTION {}", cmake_arg(&catalog.description)).unwrap();
            if let Some(source) = &catalog.source {
                writeln!(out, "    SOURCE {}", cmake_arg(source)).unwrap();
            }
            if !catalog.consumers.is_empty() {
                let consumers: Vec<_> = catalog
                    .consumers
                    .iter()
                    .map(|consumer| cmake_arg(consumer))
                    .collect();
                writeln!(out, "    CONSUMERS {}", consumers.join(" ")).unwrap();
            }
            writeln!(
                out,
                "    SOURCE_DESCRIPTION {}",
                cmake_arg(&catalog.source_description)
            )
            .unwrap();
            let languages: Vec<_> = catalog
                .catalogs
                .iter()
                .map(|language| cmake_arg(language))
                .collect();
            writeln!(out, "    LANGUAGES {}", languages.join(" ")).unwrap();
            writeln!(out, ")\n").unwrap();
        }
    }

    // 2. Meta-Targets derived from #MM and #MM-
    writeln!(
        out,
        "\n# ============================================================================="
    )
    .unwrap();
    writeln!(out, "# Declarative Meta-Targets (#MM and #MM-)").unwrap();
    writeln!(
        out,
        "# =============================================================================\n"
    )
    .unwrap();

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
    let redundant_abi_include_edges: Vec<(String, String)> = graph
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

    // Phase one declares every meta target. The old single-pass form checked
    // `if(TARGET dep)` while iterating a HashMap, so a meta dependency that was
    // declared later in the random iteration order was permanently omitted.
    for (meta_name, _) in &meta_rules {
        // `clean` and `install` are generator-provided target names which
        // CMake refuses in add_custom_target(). They remain valid dependency
        // tokens, but cannot have separately declared utility targets here.
        if !matches!(meta_name.as_str(), "clean" | "install")
            && !all_targets.contains(meta_name.as_str())
        {
            let grub_meta = meta_name.contains("grub2");
            if grub_meta {
                writeln!(out, "if(AROS_GRUB2_HOST_LANES_AVAILABLE)").unwrap();
            }
            writeln!(out, "if(NOT TARGET {})", cmake_arg(meta_name)).unwrap();
            writeln!(out, "    add_custom_target({})", cmake_arg(meta_name)).unwrap();
            writeln!(out, "endif()").unwrap();
            if grub_meta {
                writeln!(out, "endif()").unwrap();
            }
        }
    }
    writeln!(out).unwrap();

    for header in &graph.assembly_headers {
        writeln!(
            out,
            "# Source assembly header from {}:{}",
            header.file, header.line
        )
        .unwrap();
        writeln!(out, "aros_generate_assembly_header(").unwrap();
        for (field, value) in [
            ("NAME", &header.owner),
            ("AGGREGATE", &header.aggregate_owner),
            ("SOURCE", &header.source),
            ("ASSEMBLY", &header.assembly_output),
            ("OUTPUT", &header.header_output),
            ("HEADER_ROOT", &header.header_root),
            ("TOKEN", &header.token),
        ] {
            writeln!(out, "    {field} {}", cmake_arg(value)).unwrap();
        }
        if !header.arguments.is_empty() {
            writeln!(out, "    ARGUMENTS").unwrap();
            for argument in &header.arguments {
                writeln!(out, "        {}", cmake_arg(argument)).unwrap();
            }
        }
        let dependencies: Vec<_> = header
            .aggregate_dependencies
            .iter()
            .filter(|dependency| {
                !graph
                    .assembly_headers
                    .iter()
                    .any(|candidate| &candidate.owner == *dependency)
            })
            .collect();
        if !dependencies.is_empty() {
            writeln!(out, "    DEPENDS").unwrap();
            for dependency in dependencies {
                writeln!(out, "        {}", cmake_arg(dependency)).unwrap();
            }
        }
        writeln!(out, ")\n").unwrap();
    }
    // Physical architecture metadata effects are not virtual aliases. Their
    // prerequisite aliases now exist, but their own names were deliberately
    // excluded from phase one's empty-target declarations.
    let mut arch_effects: Vec<_> = graph.arch_endpoint_effects.iter().collect();
    arch_effects.sort_by(|a, b| a.endpoint.cmp(&b.endpoint));
    for effect in arch_effects {
        use crate::arch_endpoint_effects::ArchEndpointEffectData;
        writeln!(
            out,
            "# Architecture effect from {}:{}",
            effect.recipe, effect.line
        )
        .unwrap();
        match &effect.data {
            ArchEndpointEffectData::ArchModuleObjects {
                mainmmake,
                module_sources,
                directory,
                ..
            } => {
                writeln!(out, "aros_bind_arch_source_endpoint(\n    ENDPOINT {}\n    OWNER {}\n    INCLUDE_OWNER {}\n    DIRECTORY \"${{AROS_SOURCE_DIR}}/{}\"\n    BASENAMES {}\n)",
                    cmake_arg(&effect.endpoint), cmake_arg(mainmmake), cmake_arg(&effect.dependencies[0]), directory,
                    module_sources.iter().map(|name| cmake_arg(name)).collect::<Vec<_>>().join(" ")).unwrap();
            }
            ArchEndpointEffectData::EmptyLinklibAggregate { .. } => {
                writeln!(
                    out,
                    "aros_empty_arch_linklib(NAME {} INCLUDE_TARGET {})",
                    cmake_arg(&effect.endpoint),
                    cmake_arg(&effect.dependencies[0])
                )
                .unwrap();
            }
            ArchEndpointEffectData::SetArchIncludes {
                tag,
                modname,
                maindir,
                priority_token,
                include_dirs,
                definitions,
                ..
            } => {
                writeln!(out, "aros_set_archincludes_endpoint(\n    NAME {}\n    MODNAME {}\n    MAINDIR {}\n    PRIORITY {}\n    TAG {}",
                    cmake_arg(&effect.endpoint), cmake_arg(modname), cmake_arg(maindir),
                    cmake_arg(priority_token), cmake_arg(tag)).unwrap();
                if !include_dirs.is_empty() {
                    writeln!(
                        out,
                        "    INCLUDE_DIRS {}",
                        include_dirs
                            .iter()
                            .map(|path| cmake_arg(path))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                    .unwrap();
                }
                if !definitions.is_empty() {
                    writeln!(
                        out,
                        "    DEFINITIONS {}",
                        definitions
                            .iter()
                            .map(|value| cmake_arg(value))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                    .unwrap();
                }
                writeln!(out, ")").unwrap();
            }
        }
    }

    // Phase two attaches edges after every possible endpoint exists. This also
    // runs for a meta name that is already a concrete/icon target: fourteen
    // icon targets carry their own outputs and #MM children at the same time.
    for (meta_name, deps) in &meta_rules {
        let mut valid_deps: Vec<&String> = deps
            .iter()
            .filter(|dep| {
                *dep != *meta_name
                    && (all_targets.contains(dep.as_str())
                        || all_metas.contains(dep.as_str())
                        || dep.contains("${"))
                    && !redundant_abi_include_edges
                        .iter()
                        .any(|(linklib, includes)| linklib == *meta_name && includes == *dep)
            })
            .collect();
        valid_deps.sort();
        if valid_deps.is_empty() {
            continue;
        }
        writeln!(out, "if(TARGET {})", cmake_arg(meta_name)).unwrap();
        writeln!(
            out,
            "    foreach(dep IN ITEMS {})",
            valid_deps
                .iter()
                .map(|s| cmake_arg(s))
                .collect::<Vec<_>>()
                .join(" ")
        )
        .unwrap();
        writeln!(out, "        if(TARGET \"${{dep}}\")").unwrap();
        writeln!(
            out,
            "            aros_add_target_dependency({} \"${{dep}}\")",
            cmake_arg(meta_name)
        )
        .unwrap();
        writeln!(out, "        endif()").unwrap();
        writeln!(out, "    endforeach()").unwrap();
        writeln!(out, "endif()\n").unwrap();
    }
    // Public headers a host tool writes. Emitted before everything that
    // compiles, because a source may include one.
    if !graph.host_generated_headers.is_empty() {
        writeln!(out, "# ---- Headers written by a host tool ----").unwrap();
        for header in &graph.host_generated_headers {
            writeln!(out, "aros_host_generated_header(").unwrap();
            writeln!(out, "    TOOL {}", cmake_arg(&header.tool)).unwrap();
            writeln!(out, "    SOURCE \"${{AROS_SOURCE_DIR}}/{}\"", header.source).unwrap();
            writeln!(out, "    HEADER {}", cmake_arg(&header.header)).unwrap();
            if !header.arguments.is_empty() {
                let args: Vec<String> = header.arguments.iter().map(|a| cmake_arg(a)).collect();
                writeln!(out, "    ARGUMENTS {}", args.join(" ")).unwrap();
            }
            writeln!(out, ")").unwrap();
        }
        writeln!(out).unwrap();
    }

    // A flat binary wrapped as a relocatable object, and the target that links
    // it. config/make.tmpl:1552.
    if !graph.binary_objects.is_empty() {
        writeln!(
            out,
            "# =============================================================================\n\
             # Flat binaries wrapped as objects (from %rule_link_binary)\n\
             # ============================================================================="
        )
        .unwrap();
        for decl in &graph.binary_objects {
            writeln!(out, "aros_link_binary_object(").unwrap();
            writeln!(out, "    NAME {}", cmake_arg(&decl.name)).unwrap();
            writeln!(out, "    OUTPUT {}", cmake_arg(&decl.output)).unwrap();
            writeln!(
                out,
                "    DIRECTORY \"${{AROS_SOURCE_DIR}}/{}\"",
                decl.directory
            )
            .unwrap();
            let sources: Vec<String> = decl.sources.iter().map(|s| cmake_arg(s)).collect();
            writeln!(out, "    SOURCES {}", sources.join(" ")).unwrap();
            writeln!(out, "    START {}", cmake_arg(&decl.start)).unwrap();
            if !decl.ldflags.is_empty() {
                let flags: Vec<String> = decl.ldflags.iter().map(|f| cmake_arg(f)).collect();
                writeln!(out, "    LDFLAGS {}", flags.join(" ")).unwrap();
            }
            writeln!(out, "    CONSUMER {}", cmake_arg(&decl.consumer)).unwrap();
            if !decl.arch_tag.is_empty() {
                writeln!(out, "    ARCH_TAG {}", cmake_arg(&decl.arch_tag)).unwrap();
            }
            writeln!(out, ")").unwrap();
        }
        writeln!(out).unwrap();
    }

    // The section-ordering script a kickstart member's partial link needs.
    // Declared before the package section for the same reason as the default
    // link set: aros_link_kickstart and the member objects it asks for are
    // created while that section is read.
    if !graph.kickstart_kobj_ldscript.is_empty() {
        let tokens: Vec<String> = graph
            .kickstart_kobj_ldscript
            .iter()
            .map(|token| cmake_arg(token))
            .collect();
        writeln!(
            out,
            "aros_set_kickstart_kobj_ldscript({})\n",
            tokens.join(" ")
        )
        .unwrap();
    }

    // The compiler spec's default link set, in spec order. Declared before the
    // package section, because aros_link_kickstart resolves it while this file
    // is being read: the kickstart link is one of its consumers, and with the
    // declaration at the end it saw an empty set and linked no libraries at
    // all. Applied to ordinary targets by CMakeLists.txt once every target
    // exists.
    //
    // Each item is `<name>|<archive target>|<switches that must be absent>|
    // <switches that must be present>`, the switch lists comma-separated.
    if !graph.default_link_set.is_empty() {
        writeln!(out, "aros_set_default_link_set(").unwrap();
        for item in &graph.default_link_set {
            writeln!(
                out,
                "    {}",
                cmake_arg(&format!(
                    "{}|{}|{}|{}",
                    item.name,
                    item.archive,
                    item.require_absent.join(","),
                    item.require_present.join(",")
                ))
            )
            .unwrap();
        }
        writeln!(out, ")\n").unwrap();
    }

    // Packages and the kickstart link, last: both check whether each member
    // is a configured target that produces a file, so the targets have to
    // exist by the time CMake reaches these calls.
    if !graph.packages.is_empty() {
        writeln!(
            out,
            "# ---- Packages and kickstart, from %make_package / %link_kickstart ----"
        )
        .unwrap();
        for pkg in &graph.packages {
            if pkg.resolved.is_empty() {
                continue;
            }
            let func = if pkg.is_kickstart {
                "aros_link_kickstart"
            } else {
                "aros_make_package"
            };
            writeln!(out, "{func}(").unwrap();
            writeln!(out, "    NAME {}", pkg.mmake).unwrap();
            writeln!(out, "    OUTPUT {}", cmake_arg(&pkg.output)).unwrap();
            if !pkg.arch.is_empty() {
                writeln!(out, "    ARCH {}", cmake_arg(&pkg.arch)).unwrap();
            }
            if !pkg.uselibs.is_empty() {
                let libs: Vec<String> = pkg.uselibs.iter().map(|l| cmake_arg(l)).collect();
                writeln!(out, "    USELIBS {}", libs.join(" ")).unwrap();
            }
            let ids: Vec<String> = pkg
                .resolved
                .iter()
                .map(|member| cmake_arg(&member.target))
                .collect();
            writeln!(out, "    MODULES {}", ids.join(" ")).unwrap();
            if !pkg.is_kickstart {
                let names: Vec<String> = pkg
                    .resolved
                    .iter()
                    .map(|member| cmake_arg(&member.runtime_name))
                    .collect();
                writeln!(out, "    MEMBER_NAMES {}", names.join(" ")).unwrap();
            }
            writeln!(out, ")").unwrap();
        }
        writeln!(out).unwrap();
    }

    out
}

#[cfg(test)]
#[path = "generator_tests.rs"]
mod tests;
