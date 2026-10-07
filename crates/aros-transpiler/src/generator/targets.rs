use super::{cmake_arg, cmake_literal_arg, emit_configure_builds};
use crate::ast::ModuleType;
use crate::graph::DependencyGraph;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Emits the concrete module targets.
pub(super) fn emit_concrete_targets(
    out: &mut String,
    graph: &DependencyGraph,
    kickstart_members: &BTreeMap<String, Vec<String>>,
) {
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
            if target.selection_headers_only {
                writeln!(out, "    ALLOW_NO_PUBLIC_HEADERS").unwrap();
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
}

/// Emits bindings that need the concrete targets to exist.
pub(super) fn emit_post_target_bindings(out: &mut String, graph: &DependencyGraph) {
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
        out,
        graph
            .configure_builds
            .iter()
            .filter(|declaration| !declaration.dependency_targets.is_empty())
            .collect(),
        "Configure-style builds that consume a link library",
    );
}

/// Emits late helper bindings and finalizes link libraries.
pub(super) fn emit_late_helper_bindings(out: &mut String, graph: &DependencyGraph) {
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
}
