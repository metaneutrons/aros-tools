use super::{cmake_arg, emit_configure_builds};
use crate::graph::DependencyGraph;
use std::collections::HashSet;
use std::fmt::Write;

/// Emits third-party source fetching (`%fetch`).
pub(super) fn emit_fetches(out: &mut String, graph: &DependencyGraph) {
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
                 \x20   CHECKSUMS \"{}\" NORMALIZATION \"{}\" NORMALIZED_SIZE \"{}\" LOCATION \"{}\" DESTINATION \"{}\" BASE \"{}\" PATCH_ORIGINS \"{}\" PATCHES \"{}\"",
                f.name,
                f.archive,
                f.suffixes,
                f.origins,
                f.checksums,
                f.normalization,
                f.normalized_size,
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
}

/// Emits external CMake projects.
pub(super) fn emit_external_cmake(out: &mut String, graph: &DependencyGraph) {
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
}

/// Emits local configure builds that precede ordinary consumers and the GRUB2 host-tool lanes.
pub(super) fn emit_configure_and_grub_builds(out: &mut String, graph: &DependencyGraph) {
    // Local projects admitted from `%build_with_configure` use a closed
    // runner contract rather than arbitrary shell text.  Declarations precede
    // ordinary consumers so a published archive interface can bind exactly
    // like an in-tree link library.
    //
    // A declaration that consumes a link library cannot stand here, because
    // aros_build_configure asks that target where its archive is: see the
    // second block after the concrete targets.
    emit_configure_builds(
        out,
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
}

/// Emits declarations that must exist before any concrete target is read.
pub(super) fn emit_lane_declarations(
    out: &mut String,
    graph: &DependencyGraph,
    all_targets: &mut HashSet<String>,
) {
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
}
