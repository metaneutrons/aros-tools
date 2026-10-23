use super::super::cmake_arg;
use crate::ast::{ModuleType, PythonOutputsDecl, TargetDefinition};
use crate::capability::mesa::configured_source_context::MesaSourceCompileContext;
use crate::capability::mesa::ordered_contract::OrderedMesaCompilerOptions;
use crate::graph::DependencyGraph;
use aros_common::mesa_configuration::MesaMakeCoordinates;
use std::fmt::Write;

pub(super) fn configured_mesa_metadata_error(graph: &DependencyGraph) -> Option<String> {
    let configured = graph.configured_mesa.as_ref()?;
    let driver = &configured.driver;
    let coordinates = &driver.recipe.make_coordinates;
    if configured.targets.is_empty() {
        return Some("configured Mesa selection has no compile targets".into());
    }
    if coordinates.validate().is_err() {
        return Some("configured Mesa has invalid measured Make coordinates".into());
    }
    if let Err(error) = driver.validate() {
        return Some(error);
    }
    for entry in configured.targets.values() {
        let context = &entry.source_context;
        for path in [&context.object_directory, &context.quote_extra]
            .into_iter()
            .chain(context.python_build_directory.as_ref())
        {
            if !path.is_absolute()
                || path.to_str().is_none_or(|value| {
                    value
                        .chars()
                        .any(|c| c.is_control() || ";$<>\"\\".contains(c))
                })
                || path.components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir | std::path::Component::CurDir
                    )
                })
            {
                return Some("configured Mesa has an unsafe source-context path".into());
            }
        }
        if !context
            .object_directory
            .starts_with(&coordinates.generated_root)
            || context.object_directory == std::path::Path::new(&coordinates.generated_root)
        {
            return Some("configured Mesa source object directory leaves measured GENDIR".into());
        }
        if context.python_build_directory.as_ref().is_some_and(|path| {
            !path.starts_with(&coordinates.generated_root)
                || path == std::path::Path::new(&coordinates.generated_root)
        }) {
            return Some("configured Mesa Python root leaves measured GENDIR".into());
        }
    }
    for group in &graph.python_outputs {
        for consumer in &group.consumers {
            if configured
                .targets
                .get(consumer)
                .is_some_and(|entry| entry.source_context.python_build_directory.is_none())
            {
                return Some("configured Mesa Python producer lacks source top_builddir".into());
            }
        }
    }
    for (mmake_name, entry) in &configured.targets {
        let contract = &entry.options;
        let mut matches = graph
            .targets
            .values()
            .filter(|target| target.mmake_name.as_str() == mmake_name.as_str());
        let Some(target) = matches.next() else {
            return Some(format!(
                "configured Mesa compile metadata has no concrete target {mmake_name}"
            ));
        };
        if matches.next().is_some() {
            return Some(format!(
                "configured Mesa compile metadata names duplicate target {mmake_name}"
            ));
        }
        if !safe_cmake_target_identifier(&target.mmake_name)
            || !safe_cmake_target_identifier(&target.target_name)
        {
            return Some(format!(
                "configured Mesa target {mmake_name} has an unsafe CMake target identifier"
            ));
        }
        if target.module_type == ModuleType::ModuleHeaders {
            return Some(format!(
                "configured Mesa target {mmake_name} does not create a compile target"
            ));
        }
        if target.variant_32bit {
            return Some(format!(
                "configured Mesa target {mmake_name} requires an unmeasured 32-bit companion compiler projection"
            ));
        }
        if !target.asm_source_files.is_empty() {
            return Some(format!(
                "configured Mesa target {mmake_name} requires an unmeasured assembler option projection"
            ));
        }
        if !target.objc_source_files.is_empty() {
            return Some(format!(
                "configured Mesa target {mmake_name} requires an unmeasured Objective-C option projection"
            ));
        }
        if mmake_name == "linklibs-gallium_vc4" && target.module_type != ModuleType::LinkLib {
            return Some(
                "configured Mesa VC4 GCA contract must belong to a link-library target".into(),
            );
        }

        for (language, options, required) in [
            ("C", &contract.c, true),
            ("CXX", &contract.cxx, true),
            ("VC4 bridge", &contract.vc4_bridge, false),
        ] {
            if required && options.is_empty() {
                return Some(format!(
                    "configured Mesa target {mmake_name} has an empty {language} sequence"
                ));
            }
            for (index, option) in options.iter().enumerate() {
                if !safe_ordered_compile_option(option) {
                    return Some(format!(
                        "configured Mesa target {mmake_name} has an unsafe {language} option at index {index}"
                    ));
                }
            }
        }
        if contract
            .gca_abi_flags
            .chars()
            .any(|character| matches!(character, '\0' | '\r' | '\n' | ';' | '$' | '<' | '>'))
        {
            return Some(format!(
                "configured Mesa target {mmake_name} has an unsafe GCA ABI literal"
            ));
        }
    }
    None
}

fn safe_cmake_target_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}

fn safe_ordered_compile_option(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('@')
        && !value
            .chars()
            .any(|character| matches!(character, '\0' | ';' | '\r' | '\n' | '$' | '<' | '>'))
}

pub(super) fn emit_configured_mesa_target_snapshot(out: &mut String) {
    writeln!(
        out,
        "get_property(_aros_configured_mesa_targets_before DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)"
    )
    .unwrap();
}

pub(super) fn emit_configured_mesa_compile_contract(
    out: &mut String,
    target: &TargetDefinition,
    contract: &OrderedMesaCompilerOptions,
    source_context: (&MesaSourceCompileContext, &MesaMakeCoordinates),
    python_owners: &[&PythonOutputsDecl],
) {
    let cmake_target = cmake_arg(&target.mmake_name);
    let fatal =
        |message: &str| format!("message(FATAL_ERROR {})\n", cmake_bracket_argument(message));

    writeln!(
        out,
        "get_property(_aros_configured_mesa_targets_after DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)"
    )
    .unwrap();
    writeln!(
        out,
        "set(_aros_configured_mesa_new_targets \"${{_aros_configured_mesa_targets_after}}\")"
    )
    .unwrap();
    writeln!(
        out,
        "foreach(_aros_configured_mesa_old_target IN LISTS _aros_configured_mesa_targets_before)"
    )
    .unwrap();
    writeln!(
        out,
        "    list(REMOVE_ITEM _aros_configured_mesa_new_targets \"${{_aros_configured_mesa_old_target}}\")"
    )
    .unwrap();
    writeln!(out, "endforeach()").unwrap();
    writeln!(out, "set(_aros_configured_mesa_main_target {cmake_target})").unwrap();
    writeln!(
        out,
        "if(NOT TARGET \"${{_aros_configured_mesa_main_target}}\")"
    )
    .unwrap();
    write!(
        out,
        "{}",
        fatal(&format!(
            "{}: configured Mesa compile target was not created by its builder",
            target.mmake_name
        ))
    )
    .unwrap();
    writeln!(out, "endif()").unwrap();
    writeln!(
        out,
        "list(FIND _aros_configured_mesa_new_targets \"${{_aros_configured_mesa_main_target}}\" _aros_configured_mesa_main_index)"
    )
    .unwrap();
    writeln!(out, "if(_aros_configured_mesa_main_index EQUAL -1)").unwrap();
    write!(
        out,
        "{}",
        fatal(&format!(
            "{}: configured Mesa compile target was not newly created by its builder",
            target.mmake_name
        ))
    )
    .unwrap();
    writeln!(out, "endif()").unwrap();
    writeln!(out, "set(_aros_configured_mesa_main_compilable FALSE)").unwrap();
    writeln!(
        out,
        "foreach(_aros_configured_mesa_target IN LISTS _aros_configured_mesa_new_targets)"
    )
    .unwrap();
    writeln!(
        out,
        "    if(NOT TARGET \"${{_aros_configured_mesa_target}}\")"
    )
    .unwrap();
    write!(
        out,
        "{}",
        fatal("configured Mesa builder reported a missing newly created target")
    )
    .unwrap();
    writeln!(out, "    endif()").unwrap();
    writeln!(
        out,
        "    get_target_property(_aros_configured_mesa_imported \"${{_aros_configured_mesa_target}}\" IMPORTED)"
    )
    .unwrap();
    writeln!(out, "    if(NOT _aros_configured_mesa_imported)").unwrap();
    writeln!(
        out,
        "        get_target_property(_aros_configured_mesa_target_type \"${{_aros_configured_mesa_target}}\" TYPE)"
    )
    .unwrap();
    writeln!(
        out,
        "        if(_aros_configured_mesa_target_type MATCHES \"^(STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY|EXECUTABLE)$\")"
    )
    .unwrap();
    writeln!(
        out,
        "            if(_aros_configured_mesa_target STREQUAL _aros_configured_mesa_main_target)"
    )
    .unwrap();
    writeln!(
        out,
        "                set(_aros_configured_mesa_main_compilable TRUE)"
    )
    .unwrap();
    writeln!(out, "            endif()").unwrap();

    {
        let (context, coordinates) = source_context;
        let declaration = format!("${{AROS_SOURCE_DIR}}/{}", target.dir_path.display());
        writeln!(out, "            _aros_bind_mesa_genmodule_context(\"${{_aros_configured_mesa_target}}\" {} {} {} {})",
            cmake_arg(&target.mmake_name), cmake_arg(&declaration),
            cmake_bracket_argument(&context.object_directory.to_string_lossy()),
            cmake_bracket_argument(&coordinates.generated_root)).unwrap();
        if let Some(nominal) = &context.python_build_directory {
            for producer in python_owners {
                writeln!(out, "            _aros_bind_mesa_python_context(\"${{_aros_configured_mesa_target}}\" {} {} {} {})",
                    cmake_arg(&producer.owner), cmake_arg(&producer.build_root),
                    cmake_bracket_argument(&nominal.to_string_lossy()),
                    cmake_bracket_argument(&coordinates.generated_root)).unwrap();
            }
        }
    }

    // Builder-installed quote roots are structural, as are its target
    // definitions. Preserve only target-level -iquote options and leave all
    // COMPILE_DEFINITIONS (including genmodule/HIDD definitions) untouched.
    writeln!(
        out,
        "            get_target_property(_aros_configured_mesa_existing_options \"${{_aros_configured_mesa_target}}\" COMPILE_OPTIONS)"
    )
    .unwrap();
    writeln!(
        out,
        "            set(_aros_configured_mesa_structural_options \"\")"
    )
    .unwrap();
    writeln!(
        out,
        "            if(_aros_configured_mesa_existing_options)"
    )
    .unwrap();
    writeln!(
        out,
        "                foreach(_aros_configured_mesa_option IN LISTS _aros_configured_mesa_existing_options)"
    )
    .unwrap();
    writeln!(
        out,
        "                    if(_aros_configured_mesa_option MATCHES \"^-iquote\")"
    )
    .unwrap();
    writeln!(
        out,
        "                        list(APPEND _aros_configured_mesa_structural_options \"${{_aros_configured_mesa_option}}\")"
    )
    .unwrap();
    writeln!(out, "                    endif()").unwrap();
    writeln!(out, "                endforeach()").unwrap();
    writeln!(out, "            endif()").unwrap();
    writeln!(
        out,
        "            set_property(TARGET \"${{_aros_configured_mesa_target}}\" PROPERTY COMPILE_OPTIONS \"\")"
    )
    .unwrap();
    // The builder inherits the directory prefix into COMPILE_OPTIONS. The
    // reset above deliberately discards unmeasured flags, so restore only the
    // separately validated source driver prefix before quote/language options.
    writeln!(
        out,
        "            if(_aros_mesa_driver_roles_bound AND _aros_mesa_sysroot_options)"
    )
    .unwrap();
    writeln!(out, "                target_compile_options(\"${{_aros_configured_mesa_target}}\" PRIVATE ${{_aros_mesa_sysroot_options}})").unwrap();
    writeln!(out, "            endif()").unwrap();
    writeln!(
        out,
        "            if(_aros_configured_mesa_structural_options)"
    )
    .unwrap();
    writeln!(
        out,
        "                target_compile_options(\"${{_aros_configured_mesa_target}}\" PRIVATE ${{_aros_configured_mesa_structural_options}})"
    )
    .unwrap();
    writeln!(out, "            endif()").unwrap();

    let helper_target = "\"${_aros_configured_mesa_target}\"";
    emit_ordered_compile_contract_call(out, helper_target, "C", &contract.c);
    emit_ordered_compile_contract_call(out, helper_target, "CXX", &contract.cxx);
    writeln!(
        out,
        "            set_property(TARGET \"${{_aros_configured_mesa_target}}\" PROPERTY AROS_MESA_CONFIGURED_CONTRACT TRUE)"
    )
    .unwrap();
    writeln!(
        out,
        "        elseif(_aros_configured_mesa_target_type STREQUAL \"UTILITY\" OR _aros_configured_mesa_target_type STREQUAL \"INTERFACE_LIBRARY\")"
    )
    .unwrap();
    writeln!(
        out,
        "            if(_aros_configured_mesa_target STREQUAL _aros_configured_mesa_main_target)"
    )
    .unwrap();
    write!(
        out,
        "{}",
        fatal(&format!(
            "{}: configured Mesa contract target is not compilable",
            target.mmake_name
        ))
    )
    .unwrap();
    writeln!(out, "            endif()").unwrap();
    writeln!(out, "        else()").unwrap();
    write!(
        out,
        "{}",
        fatal("configured Mesa builder created a target with an unknown CMake type")
    )
    .unwrap();
    writeln!(out, "        endif()").unwrap();
    writeln!(
        out,
        "    elseif(_aros_configured_mesa_target STREQUAL _aros_configured_mesa_main_target)"
    )
    .unwrap();
    write!(
        out,
        "{}",
        fatal(&format!(
            "{}: configured Mesa compile target must not be imported",
            target.mmake_name
        ))
    )
    .unwrap();
    writeln!(out, "    endif()").unwrap();
    writeln!(out, "endforeach()").unwrap();
    writeln!(out, "if(NOT _aros_configured_mesa_main_compilable)").unwrap();
    write!(
        out,
        "{}",
        fatal(&format!(
            "{}: configured Mesa compile target was not projected",
            target.mmake_name
        ))
    )
    .unwrap();
    writeln!(out, "endif()").unwrap();

    if target.mmake_name == "linklibs-gallium_vc4" {
        writeln!(
            out,
            "set_property(TARGET \"${{_aros_configured_mesa_main_target}}\" PROPERTY AROS_MESA_GCA_ABI_FLAGS {})",
            cmake_bracket_argument(&contract.gca_abi_flags)
        )
        .unwrap();
        writeln!(
            out,
            "set_property(TARGET \"${{_aros_configured_mesa_main_target}}\" PROPERTY AROS_MESA_C_OPTIONS {})",
            cmake_property_values(&contract.c)
        )
        .unwrap();
        writeln!(
            out,
            "set_property(TARGET \"${{_aros_configured_mesa_main_target}}\" PROPERTY AROS_MESA_VC4_BRIDGE_OPTIONS {})",
            cmake_property_values(&contract.vc4_bridge)
        )
        .unwrap();
    }
    writeln!(out, "unset(_aros_configured_mesa_targets_before)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_targets_after)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_new_targets)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_main_target)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_main_index)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_main_compilable)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_target)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_target_type)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_imported)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_existing_options)").unwrap();
    writeln!(out, "unset(_aros_configured_mesa_structural_options)\n").unwrap();
}

fn emit_ordered_compile_contract_call(
    out: &mut String,
    cmake_target: &str,
    language: &str,
    options: &[String],
) {
    writeln!(
        out,
        "_aros_target_ordered_compile_contract({cmake_target} {language}"
    )
    .unwrap();
    for option in options {
        writeln!(out, "    {}", cmake_arg(option)).unwrap();
    }
    writeln!(out, ")").unwrap();
}

fn cmake_property_values(values: &[String]) -> String {
    if values.is_empty() {
        return "\"\"".to_owned();
    }
    values
        .iter()
        .map(|value| cmake_arg(value))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn emit_configured_mesa_subdirectory(
    out: &mut String,
    declarations: &str,
    has_configured_vc4: bool,
    driver: &crate::capability::mesa::configured_driver::MesaConfiguredDriver,
) {
    let mut child = String::new();
    {
        let recipe = &driver.recipe;
        child.push_str("include(\"${AROS_CMAKE_ENGINE_DIR}/ConfiguredMesaDriver.cmake\")\n");
        write!(
            child,
            "_aros_bind_mesa_compiler_roles({} {} {} {} {} {}",
            cmake_bracket_argument(&recipe.target_cc),
            cmake_bracket_argument(&recipe.target_cxx),
            cmake_bracket_argument(&driver.target_triple),
            cmake_bracket_argument(&recipe.make_coordinates.build_root),
            cmake_bracket_argument(&recipe.make_coordinates.generated_root),
            cmake_bracket_argument(&recipe.make_coordinates.ports_root)
        )
        .unwrap();
        for argument in &recipe.target_sysroot {
            write!(child, " {}", cmake_bracket_argument(argument)).unwrap();
        }
        child.push_str(")\n");
    }
    child.push_str(
        "set(CMAKE_C_FLAGS \"\")\n\
         set(CMAKE_CXX_FLAGS \"\")\n\
         set(CMAKE_ASM_FLAGS \"\")\n\
         set_property(DIRECTORY PROPERTY COMPILE_OPTIONS \"\")\n\
         get_cmake_property(_aros_configured_mesa_variables VARIABLES)\n\
         foreach(_aros_configured_mesa_variable IN LISTS _aros_configured_mesa_variables)\n\
             if(_aros_configured_mesa_variable MATCHES \"^CMAKE_(C|CXX|ASM)_FLAGS_.+$\")\n\
                 set(\"${_aros_configured_mesa_variable}\" \"\")\n\
             endif()\n\
         endforeach()\n\
         unset(_aros_configured_mesa_variable)\n\
         unset(_aros_configured_mesa_variables)\n\
         include(\"${AROS_CMAKE_ENGINE_DIR}/OrderedCompileContract.cmake\")\n\n",
    );
    child.push_str("add_compile_options(${_aros_mesa_sysroot_options})\n");
    child.push_str(declarations);
    if has_configured_vc4 {
        child.push_str("aros_build_mesa26_gallium_core_api()\n\n");
    }

    writeln!(
        out,
        "set(_aros_configured_mesa_dir \"${{CMAKE_BINARY_DIR}}/CMakeFiles/aros-configured-mesa\")"
    )
    .unwrap();
    writeln!(
        out,
        "file(MAKE_DIRECTORY \"${{_aros_configured_mesa_dir}}\")"
    )
    .unwrap();
    writeln!(
        out,
        "file(WRITE \"${{_aros_configured_mesa_dir}}/CMakeLists.txt\" {})",
        cmake_bracket_argument(&child)
    )
    .unwrap();
    writeln!(
        out,
        "add_subdirectory(\"${{_aros_configured_mesa_dir}}\" \"${{_aros_configured_mesa_dir}}/build\")\n"
    )
    .unwrap();
}

pub(super) fn cmake_bracket_argument(value: &str) -> String {
    let mut equals = String::new();
    loop {
        let close = format!("]{equals}]");
        if !value.contains(&close) {
            return format!("[{equals}[{value}]{equals}]");
        }
        equals.push('=');
    }
}
