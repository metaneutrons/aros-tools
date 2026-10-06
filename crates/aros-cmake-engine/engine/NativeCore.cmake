# Direct source-derived link of a native core ELF.
#
# Unlike a kickstart member, a native core is the final linked image. Its
# inputs are the source modules' collected and localized KOBJ intermediates, the explicitly
# resolved link-library archives and the selected toolchain's verified
# builtins archive. This helper deliberately does not use a Make-built object,
# the module ELF output, raw object mirrors, or `-l` lookup.

include_guard(GLOBAL)
include(CMakeParseArguments)

function(_aros_native_core_collect_targets directory out_var)
    get_property(_targets DIRECTORY "${directory}" PROPERTY BUILDSYSTEM_TARGETS)
    get_property(_subdirs DIRECTORY "${directory}" PROPERTY SUBDIRECTORIES)
    foreach(_subdir IN LISTS _subdirs)
        _aros_native_core_collect_targets("${_subdir}" _nested)
        list(APPEND _targets ${_nested})
    endforeach()
    set(${out_var} "${_targets}" PARENT_SCOPE)
endfunction()

function(_aros_native_core_valid_name value label)
    if("${value}" STREQUAL "" OR "${value}" MATCHES "[;\\\"$\r\n]" OR
       NOT "${value}" MATCHES "^[A-Za-z0-9_.:+-]+$")
        message(FATAL_ERROR "Native core ${label} is invalid: '${value}'")
    endif()
endfunction()

# Resolve one loader-visible canonical runtime basename, such as
# `kernel.resource`, to the unique configured, non-foreign concrete module
# target which publishes it.
function(aros_resolve_native_core_module out_var runtime_name)
    if(NOT DEFINED out_var OR "${out_var}" STREQUAL "")
        message(FATAL_ERROR "aros_resolve_native_core_module requires an output variable")
    endif()
    if("${runtime_name}" STREQUAL "" OR "${runtime_name}" MATCHES "[;\\\"$\r\n]")
        message(FATAL_ERROR "Native core runtime name is invalid: '${runtime_name}'")
    endif()

    string(TOLOWER "${runtime_name}" _wanted)
    _aros_native_core_collect_targets("${CMAKE_SOURCE_DIR}" _targets)
    get_property(_default_root_known GLOBAL PROPERTY AROS_METAMAKE_DEFAULT_ROOT SET)
    get_property(_reachable GLOBAL PROPERTY AROS_METAMAKE_REACHABLE_TARGETS)
    set(_matches "")
    set(_foreign_matches "")
    foreach(_candidate IN LISTS _targets)
        get_target_property(_type "${_candidate}" TYPE)
        if(NOT _type STREQUAL "EXECUTABLE")
            continue()
        endif()
        get_target_property(_canonical "${_candidate}" AROS_CANONICAL_RUNTIME_NAME)
        if(NOT _canonical OR _canonical STREQUAL "_canonical-NOTFOUND")
            continue()
        endif()
        string(TOLOWER "${_canonical}" _candidate_name)
        if(NOT _candidate_name STREQUAL _wanted)
            continue()
        endif()
        get_target_property(_private_alternative "${_candidate}" AROS_PRIVATE_RUNTIME_ALTERNATIVE)
        if(_private_alternative)
            continue()
        endif()
        # RuntimeModuleOutputs marks unselected alternatives after the
        # MetaMake default closure is known. Honor that closure here too, so a
        # resolver scheduled before the deferred finalizer still selects the
        # same sole active owner.
        if(_default_root_known AND NOT "${_candidate}" IN_LIST _reachable)
            continue()
        endif()
        get_target_property(_foreign "${_candidate}" AROS_FOREIGN_ARCH)
        if(_foreign)
            list(APPEND _foreign_matches "${_candidate}")
        else()
            list(APPEND _matches "${_candidate}")
        endif()
    endforeach()

    list(LENGTH _matches _match_count)
    if(_match_count EQUAL 0)
        if(_foreign_matches)
            message(FATAL_ERROR
                "Native core runtime ${runtime_name} is available only from foreign target(s): ${_foreign_matches}")
        endif()
        message(FATAL_ERROR
            "Native core runtime ${runtime_name} has no concrete module target")
    elseif(_match_count GREATER 1)
        message(FATAL_ERROR
            "Native core runtime ${runtime_name} is ambiguous among targets: ${_matches}")
    endif()
    list(GET _matches 0 _match)
    set(${out_var} "${_match}" PARENT_SCOPE)
endfunction()

# Resolve canonical runtime basenames in declaration order. Duplicate names
# are rejected so a malformed contract cannot duplicate an object group.
function(aros_resolve_native_core_modules out_var)
    if(NOT DEFINED out_var OR "${out_var}" STREQUAL "" OR NOT ARGN)
        message(FATAL_ERROR
            "aros_resolve_native_core_modules requires an output variable and runtime names")
    endif()
    set(_resolved "")
    set(_seen "")
    foreach(_runtime IN LISTS ARGN)
        string(TOLOWER "${_runtime}" _identity)
        if(_identity IN_LIST _seen)
            message(FATAL_ERROR
                "Native core runtime list repeats ${_runtime}")
        endif()
        list(APPEND _seen "${_identity}")
        aros_resolve_native_core_module(_target "${_runtime}")
        list(APPEND _resolved "${_target}")
    endforeach()
    set(${out_var} "${_resolved}" PARENT_SCOPE)
endfunction()

# Return AROS_DEVELOPER_LIB_DIR as a normalized absolute path when the engine
# has configured it. An invalid configured root is a graph error, not a reason
# to fall back to a same-named archive from another directory.
function(_aros_native_core_canonical_archive_root out_var)
    if(NOT DEFINED AROS_DEVELOPER_LIB_DIR OR
       "${AROS_DEVELOPER_LIB_DIR}" STREQUAL "")
        set(${out_var} "" PARENT_SCOPE)
        return()
    endif()
    if(NOT IS_ABSOLUTE "${AROS_DEVELOPER_LIB_DIR}" OR
       "${AROS_DEVELOPER_LIB_DIR}" MATCHES "[;\\\"$\r\n]")
        message(FATAL_ERROR
            "AROS_DEVELOPER_LIB_DIR must be a safe absolute path: ${AROS_DEVELOPER_LIB_DIR}")
    endif()
    cmake_path(ABSOLUTE_PATH AROS_DEVELOPER_LIB_DIR NORMALIZE
        OUTPUT_VARIABLE _root)
    set(${out_var} "${_root}" PARENT_SCOPE)
endfunction()

# Whether target is eligible as a public native link archive. Runtime
# alternatives and archives outside the engine's canonical developer library
# directory are not suitable providers.
function(_aros_native_core_archive_is_canonical out_var target)
    get_target_property(_private "${target}" AROS_PRIVATE_RUNTIME_ALTERNATIVE)
    if(_private)
        set(${out_var} FALSE PARENT_SCOPE)
        return()
    endif()
    _aros_native_core_canonical_archive_root(_canonical_root)
    if(NOT _canonical_root)
        set(${out_var} TRUE PARENT_SCOPE)
        return()
    endif()

    get_target_property(_archive_dir "${target}" ARCHIVE_OUTPUT_DIRECTORY)
    if(NOT _archive_dir OR _archive_dir STREQUAL "_archive_dir-NOTFOUND" OR
       NOT IS_ABSOLUTE "${_archive_dir}" OR
       "${_archive_dir}" MATCHES "[;\\\"$\r\n]")
        set(${out_var} FALSE PARENT_SCOPE)
        return()
    endif()
    cmake_path(ABSOLUTE_PATH _archive_dir NORMALIZE
        OUTPUT_VARIABLE _normalized_archive_dir)
    if(_normalized_archive_dir STREQUAL _canonical_root)
        set(${out_var} TRUE PARENT_SCOPE)
    else()
        set(${out_var} FALSE PARENT_SCOPE)
    endif()
endfunction()

# Resolve one archive basename to the unique non-foreign static archive target
# whose OUTPUT_NAME publishes that name in the canonical developer lib dir.
function(aros_resolve_native_core_linklib out_var archive_name)
    if(NOT DEFINED out_var OR "${out_var}" STREQUAL "")
        message(FATAL_ERROR "aros_resolve_native_core_linklib requires an output variable")
    endif()
    _aros_native_core_valid_name("${archive_name}" "archive name")

    _aros_native_core_collect_targets("${CMAKE_SOURCE_DIR}" _targets)
    set(_matches "")
    set(_foreign_matches "")
    set(_private_matches "")
    set(_noncanonical_matches "")
    foreach(_candidate IN LISTS _targets)
        get_target_property(_type "${_candidate}" TYPE)
        if(NOT _type STREQUAL "STATIC_LIBRARY")
            continue()
        endif()
        get_target_property(_output_name "${_candidate}" OUTPUT_NAME)
        if(NOT _output_name OR _output_name STREQUAL "_output_name-NOTFOUND")
            set(_output_name "${_candidate}")
        endif()
        if(NOT _output_name STREQUAL "${archive_name}")
            continue()
        endif()
        _aros_native_core_archive_is_canonical(_canonical "${_candidate}")
        if(NOT _canonical)
            get_target_property(_private "${_candidate}" AROS_PRIVATE_RUNTIME_ALTERNATIVE)
            if(_private)
                list(APPEND _private_matches "${_candidate}")
            else()
                list(APPEND _noncanonical_matches "${_candidate}")
            endif()
            continue()
        endif()
        get_target_property(_foreign "${_candidate}" AROS_FOREIGN_ARCH)
        if(_foreign)
            list(APPEND _foreign_matches "${_candidate}")
        else()
            list(APPEND _matches "${_candidate}")
        endif()
    endforeach()

    list(LENGTH _matches _match_count)
    if(_match_count EQUAL 0)
        if(_foreign_matches)
            message(FATAL_ERROR
                "Native core archive ${archive_name} is available only from foreign target(s): ${_foreign_matches}")
        elseif(_private_matches)
            message(FATAL_ERROR
                "Native core archive ${archive_name} has only private target(s): ${_private_matches}")
        elseif(_noncanonical_matches)
            message(FATAL_ERROR
                "Native core archive ${archive_name} has no canonical static target; noncanonical target(s): ${_noncanonical_matches}")
        endif()
        message(FATAL_ERROR
            "Native core archive ${archive_name} has no static target")
    elseif(_match_count GREATER 1)
        message(FATAL_ERROR
            "Native core archive ${archive_name} is ambiguous among targets: ${_matches}")
    endif()
    list(GET _matches 0 _match)
    set(${out_var} "${_match}" PARENT_SCOPE)
endfunction()

function(aros_resolve_native_core_linklibs out_var)
    if(NOT DEFINED out_var OR "${out_var}" STREQUAL "" OR NOT ARGN)
        message(FATAL_ERROR
            "aros_resolve_native_core_linklibs requires an output variable and archive names")
    endif()
    set(_resolved "")
    set(_seen "")
    foreach(_archive IN LISTS ARGN)
        if(_archive IN_LIST _seen)
            message(FATAL_ERROR "Native core archive list repeats ${_archive}")
        endif()
        list(APPEND _seen "${_archive}")
        aros_resolve_native_core_linklib(_target "${_archive}")
        list(APPEND _resolved "${_target}")
    endforeach()
    set(${out_var} "${_resolved}" PARENT_SCOPE)
endfunction()

function(_aros_native_core_check_output out_var raw_path label)
    if(NOT IS_ABSOLUTE "${raw_path}" OR
       "${raw_path}" MATCHES "[;\\\"$\r\n]")
        message(FATAL_ERROR
            "Native core ${label} must be a safe absolute build-tree path: '${raw_path}'")
    endif()

    cmake_path(ABSOLUTE_PATH raw_path NORMALIZE OUTPUT_VARIABLE _path)
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE OUTPUT_VARIABLE _build_root)
    if(_path STREQUAL _build_root)
        message(FATAL_ERROR "Native core ${label} cannot be the build directory")
    endif()
    cmake_path(IS_PREFIX _build_root "${_path}" NORMALIZE _inside)
    if(NOT _inside)
        message(FATAL_ERROR "Native core ${label} escapes the build tree: ${_path}")
    endif()

    # Lexical containment does not catch a symlinked parent under the build
    # root. Resolve the nearest existing ancestor before accepting the output.
    file(REAL_PATH "${_build_root}" _build_root_real)
    set(_probe "${_path}")
    while(NOT EXISTS "${_probe}" AND NOT IS_SYMLINK "${_probe}")
        get_filename_component(_parent "${_probe}" DIRECTORY)
        if(_parent STREQUAL _probe)
            break()
        endif()
        set(_probe "${_parent}")
    endwhile()
    if(EXISTS "${_path}" AND IS_DIRECTORY "${_path}")
        message(FATAL_ERROR "Native core ${label} names a directory: ${_path}")
    endif()
    file(REAL_PATH "${_probe}" _probe_real)
    cmake_path(IS_PREFIX _build_root_real "${_probe_real}" NORMALIZE _physical_inside)
    if(NOT _physical_inside)
        message(FATAL_ERROR
            "Native core ${label} escapes the physical build tree through a symlink: ${_path}")
    endif()
    if(EXISTS "${_path}" OR IS_SYMLINK "${_path}")
        file(REAL_PATH "${_path}" _existing_real)
        cmake_path(IS_PREFIX _build_root_real "${_existing_real}" NORMALIZE _existing_inside)
        if(NOT _existing_inside)
            message(FATAL_ERROR
                "Native core ${label} resolves outside the physical build tree: ${_path}")
        endif()
    endif()
    set(${out_var} "${_path}" PARENT_SCOPE)
endfunction()

# aros_add_native_core(NAME <target>
#     OUTPUT <absolute-build-path> LINKER_SCRIPT <source-bound-path>
#     MODULES <resolved-module-targets...>
#     LINKLIBS <resolved-static-archive-targets...>
#     BUILTINS <explicit-archive-path> LINKER <verified-linker-path>)
function(aros_add_native_core)
    set(oneValueArgs NAME OUTPUT LINKER_SCRIPT BUILTINS LINKER)
    set(multiValueArgs MODULES LINKLIBS)
    cmake_parse_arguments(PARSE_ARGV 0 NC "" "${oneValueArgs}" "${multiValueArgs}")

    if(NC_UNPARSED_ARGUMENTS OR NC_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "aros_add_native_core received malformed arguments")
    endif()
    foreach(_required IN ITEMS NAME OUTPUT LINKER_SCRIPT MODULES LINKLIBS BUILTINS LINKER)
        if(NOT NC_${_required})
            message(FATAL_ERROR "aros_add_native_core requires ${_required}")
        endif()
    endforeach()
    _aros_native_core_valid_name("${NC_NAME}" "target name")
    if(TARGET "${NC_NAME}")
        message(FATAL_ERROR "Native core target already exists: ${NC_NAME}")
    endif()
    list(LENGTH NC_MODULES _module_count)
    list(REMOVE_DUPLICATES NC_MODULES)
    list(LENGTH NC_MODULES _unique_module_count)
    if(_module_count EQUAL 0 OR NOT _module_count EQUAL _unique_module_count)
        message(FATAL_ERROR "${NC_NAME}: native core modules are empty or duplicated")
    endif()
    list(LENGTH NC_LINKLIBS _linklib_count)
    list(REMOVE_DUPLICATES NC_LINKLIBS)
    list(LENGTH NC_LINKLIBS _unique_linklib_count)
    if(_linklib_count EQUAL 0 OR NOT _linklib_count EQUAL _unique_linklib_count)
        message(FATAL_ERROR "${NC_NAME}: native core link libraries are empty or duplicated")
    endif()

    _aros_native_core_check_output(_output "${NC_OUTPUT}" "output")
    set(_map "${_output}.map")
    _aros_native_core_check_output(_map "${_map}" "map output")
    get_property(_outputs GLOBAL PROPERTY AROS_NATIVE_CORE_OUTPUTS)
    if(_output IN_LIST _outputs OR _map IN_LIST _outputs)
        message(FATAL_ERROR "${NC_NAME}: native core output path is already claimed")
    endif()

    if(NOT DEFINED AROS_SOURCE_DIR OR NOT IS_DIRECTORY "${AROS_SOURCE_DIR}")
        message(FATAL_ERROR "${NC_NAME}: AROS_SOURCE_DIR is unavailable")
    endif()
    foreach(_input IN ITEMS LINKER_SCRIPT BUILTINS LINKER)
        if(NC_${_input} MATCHES "[;\\\"$\r\n]" OR
           NOT IS_ABSOLUTE "${NC_${_input}}" OR
           NOT EXISTS "${NC_${_input}}" OR IS_DIRECTORY "${NC_${_input}}")
            message(FATAL_ERROR
                "${NC_NAME}: missing or unsafe native core ${_input}: ${NC_${_input}}")
        endif()
    endforeach()
    execute_process(
        COMMAND "${NC_LINKER}" --version
        RESULT_VARIABLE _linker_result
        TIMEOUT 10
        OUTPUT_QUIET
        ERROR_QUIET)
    if(NOT "${_linker_result}" STREQUAL "0")
        message(FATAL_ERROR
            "${NC_NAME}: LINKER --version failed or timed out: ${NC_LINKER} (${_linker_result})")
    endif()
    file(REAL_PATH "${AROS_SOURCE_DIR}" _source_root_real)
    file(REAL_PATH "${NC_LINKER_SCRIPT}" _script_real)
    cmake_path(IS_PREFIX _source_root_real "${_script_real}" NORMALIZE _script_in_source)
    if(NOT _script_in_source)
        message(FATAL_ERROR
            "${NC_NAME}: linker script escapes AROS_SOURCE_DIR: ${NC_LINKER_SCRIPT}")
    endif()
    file(READ "${NC_BUILTINS}" _builtins_signature LIMIT 8 HEX)
    string(TOLOWER "${_builtins_signature}" _builtins_signature)
    # This checks the ordinary ar container signature only. The caller must
    # establish archive provenance from its selected toolchain contract.
    # Thin archives are deliberately rejected because their members can
    # escape the closed set of prefix-owned inputs.
    if(NOT _builtins_signature STREQUAL "213c617263683e0a")
        message(FATAL_ERROR
            "${NC_NAME}: BUILTINS lacks regular static archive magic (structural check only; provenance is caller-verified)")
    endif()

    set(_kobj_targets "")
    set(_member_objects "")
    foreach(_module IN LISTS NC_MODULES)
        _aros_native_core_valid_name("${_module}" "module target ID")
        if(NOT TARGET "${_module}")
            message(FATAL_ERROR "${NC_NAME}: native core module target is missing: ${_module}")
        endif()
        get_target_property(_module_type "${_module}" TYPE)
        if(NOT _module_type STREQUAL "EXECUTABLE")
            message(FATAL_ERROR
                "${NC_NAME}: native core member ${_module} is not a concrete runtime module")
        endif()
        get_target_property(_foreign "${_module}" AROS_FOREIGN_ARCH)
        if(_foreign)
            message(FATAL_ERROR "${NC_NAME}: native core member is foreign: ${_module}")
        endif()
        get_target_property(_private_alternative "${_module}" AROS_PRIVATE_RUNTIME_ALTERNATIVE)
        if(_private_alternative)
            message(FATAL_ERROR
                "${NC_NAME}: native core member is an unselected runtime alternative: ${_module}")
        endif()
        get_target_property(_runtime_name "${_module}" AROS_CANONICAL_RUNTIME_NAME)
        if(NOT _runtime_name OR _runtime_name STREQUAL "_runtime_name-NOTFOUND")
            message(FATAL_ERROR
                "${NC_NAME}: native core member is not a canonical runtime module: ${_module}")
        endif()
        get_target_property(_module_sources "${_module}" SOURCES)
        if(NOT _module_sources OR _module_sources STREQUAL "_module_sources-NOTFOUND")
            message(FATAL_ERROR "${NC_NAME}: native core member has no sources: ${_module}")
        endif()

        get_property(_kobj GLOBAL PROPERTY "AROS_NATIVE_KOBJ_TARGET_${_module}")
        if(NOT _kobj OR NOT TARGET "${_kobj}")
            message(FATAL_ERROR
                "${NC_NAME}: native core member has no registered KOBJ: ${_module}")
        endif()
        get_target_property(_kobj_owner "${_kobj}" AROS_NATIVE_KOBJ_OWNER)
        get_target_property(_kobj_form "${_kobj}" AROS_NATIVE_KOBJ_FORM)
        get_target_property(_kobj_file "${_kobj}" AROS_NATIVE_KOBJ_OUTPUT)
        get_property(_macro_form GLOBAL PROPERTY "AROS_MODULE_MACRO_${_module}")
        get_property(_registered_owners GLOBAL PROPERTY AROS_NATIVE_KOBJ_OWNERS)
        get_property(_registered_outputs GLOBAL PROPERTY AROS_NATIVE_KOBJ_OUTPUTS)
        if(NOT "${_kobj_owner}" STREQUAL "${_module}" OR
           NOT _module IN_LIST _registered_owners OR
           NOT _kobj_file IN_LIST _registered_outputs OR
           NOT ((_kobj_form STREQUAL "full" AND _macro_form MATCHES "^(full|runtime-only)$") OR
                (_kobj_form STREQUAL "simple" AND _macro_form STREQUAL "simple")))
            message(FATAL_ERROR
                "${NC_NAME}: native core KOBJ owner/form/registration mismatch: ${_module}")
        endif()
        _aros_native_core_check_output(_kobj_file "${_kobj_file}" "KOBJ input")
        if(_kobj_file STREQUAL _output OR _kobj_file STREQUAL _map OR
           _kobj IN_LIST _kobj_targets OR _kobj_file IN_LIST _member_objects)
            message(FATAL_ERROR "${NC_NAME}: native core KOBJ input collides or repeats")
        endif()
        list(APPEND _kobj_targets "${_kobj}")
        list(APPEND _member_objects "${_kobj_file}")
    endforeach()

    set(_archive_files "")
    set(_archive_deps "")
    foreach(_archive IN LISTS NC_LINKLIBS)
        _aros_native_core_valid_name("${_archive}" "link-library target ID")
        if(NOT TARGET "${_archive}")
            message(FATAL_ERROR "${NC_NAME}: native core archive target is missing: ${_archive}")
        endif()
        get_target_property(_archive_type "${_archive}" TYPE)
        if(NOT _archive_type STREQUAL "STATIC_LIBRARY")
            message(FATAL_ERROR
                "${NC_NAME}: native core link provider ${_archive} is not a static archive")
        endif()
        get_target_property(_archive_foreign "${_archive}" AROS_FOREIGN_ARCH)
        if(_archive_foreign)
            message(FATAL_ERROR
                "${NC_NAME}: native core link provider is foreign: ${_archive}")
        endif()
        _aros_native_core_archive_is_canonical(_archive_is_canonical "${_archive}")
        if(NOT _archive_is_canonical)
            message(FATAL_ERROR
                "${NC_NAME}: native core link provider is private or outside the canonical developer library directory: ${_archive}")
        endif()
        list(APPEND _archive_files "$<TARGET_FILE:${_archive}>")
        list(APPEND _archive_deps "${_archive}")
    endforeach()

    get_filename_component(_output_dir "${_output}" DIRECTORY)
    add_custom_command(
        OUTPUT "${_output}" "${_map}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_output_dir}"
        COMMAND "${NC_LINKER}" -Map "${_map}" -T "${NC_LINKER_SCRIPT}"
                -o "${_output}" ${_member_objects}
                ${_archive_files} "${NC_BUILTINS}"
        DEPENDS ${_kobj_targets} ${_member_objects} ${_archive_deps}
                "${NC_LINKER_SCRIPT}" "${NC_LINKER}" "${NC_BUILTINS}"
        COMMENT "Native core link ${NC_NAME}"
        COMMAND_EXPAND_LISTS
        VERBATIM)
    add_custom_target("${NC_NAME}" DEPENDS "${_output}" "${_map}")
    set_target_properties("${NC_NAME}" PROPERTIES
        AROS_NATIVE_CORE TRUE
        AROS_NATIVE_CORE_OUTPUT "${_output}"
        AROS_NATIVE_CORE_MAP "${_map}"
        AROS_NATIVE_CORE_MEMBER_IDS "${NC_MODULES}"
        AROS_NATIVE_CORE_KOBJ_TARGETS "${_kobj_targets}"
        AROS_NATIVE_CORE_KOBJ_FILES "${_member_objects}"
        AROS_NATIVE_CORE_LINKLIB_TARGETS "${NC_LINKLIBS}"
        AROS_NATIVE_CORE_BUILTINS "${NC_BUILTINS}"
        AROS_NATIVE_CORE_LINKER "${NC_LINKER}"
        AROS_NATIVE_CORE_LINKER_SCRIPT "${NC_LINKER_SCRIPT}")
    set_property(GLOBAL APPEND PROPERTY AROS_NATIVE_CORE_OUTPUTS "${_output}" "${_map}")
endfunction()
