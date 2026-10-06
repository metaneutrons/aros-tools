use super::cmake_arg;
use crate::graph::DependencyGraph;
use std::collections::HashSet;
use std::fmt::Write;

/// Emits the banner above the meta-target section.
pub(super) fn emit_meta_banner(out: &mut String) {
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
}

/// Phase one of the meta targets: declares every meta target.
pub(super) fn emit_meta_declarations(
    out: &mut String,
    all_targets: &HashSet<String>,
    meta_rules: &[(&String, &HashSet<String>)],
) {
    // Phase one declares every meta target. The old single-pass form checked
    // `if(TARGET dep)` while iterating a HashMap, so a meta dependency that was
    // declared later in the random iteration order was permanently omitted.
    for (meta_name, _) in meta_rules {
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
}

/// Emits assembly header rules.
pub(super) fn emit_assembly_header_rules(out: &mut String, graph: &DependencyGraph) {
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
}

/// Emits architecture endpoint effects.
pub(super) fn emit_arch_endpoint_effects(out: &mut String, graph: &DependencyGraph) {
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
}

/// Phase two of the meta targets: adds the dependency edges.
pub(super) fn emit_meta_dependencies(
    out: &mut String,
    all_targets: &HashSet<String>,
    all_metas: &HashSet<&str>,
    meta_rules: &[(&String, &HashSet<String>)],
    redundant_abi_include_edges: &[(String, String)],
) {
    // Phase two attaches edges after every possible endpoint exists. This also
    // runs for a meta name that is already a concrete/icon target: fourteen
    // icon targets carry their own outputs and #MM children at the same time.
    for (meta_name, deps) in meta_rules {
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
}

/// Emits host-generated headers, binary objects and the kickstart linker script.
pub(super) fn emit_host_headers_and_objects(out: &mut String, graph: &DependencyGraph) {
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
}

/// Emits the default link set and the package declarations.
pub(super) fn emit_link_set_and_packages(out: &mut String, graph: &DependencyGraph) {
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
}
