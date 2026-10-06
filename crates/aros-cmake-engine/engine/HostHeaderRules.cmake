# Build one selected named host-header chain from its source-owned Make rules.
#
# This helper creates only owner-scoped custom targets. It does not attach the
# header to ALL or to a global include root; callers connect the returned owner
# through their ordinary dependency graph.

include_guard(GLOBAL)
include(CMakeParseArguments)

# aros_host_header_rule(NAME <owner> SETUP_NAME <setup>
#   TOOL_SOURCE <source> TOOL_OUTPUT <executable> SOURCE_WORKDIR <directory>
#   HEADER <relative-header> PRIMARY_OUTPUT <geninc-header>
#   SDK_OUTPUT <sdk-header> [USE_CONFIGURED_HOST_CFLAGS]
#   [COMPILE_FLAGS <literal...>] [SOURCE_PREREQUISITES <file...>]
#   [SETUP_DIRECTORIES <directory...>])
function(aros_host_header_rule)
    set(oneValueArgs
        NAME SETUP_NAME TOOL_SOURCE TOOL_OUTPUT SOURCE_WORKDIR HEADER
        PRIMARY_OUTPUT SDK_OUTPUT)
    set(multiValueArgs COMPILE_FLAGS SOURCE_PREREQUISITES SETUP_DIRECTORIES)
    cmake_parse_arguments(PARSE_ARGV 0 HHR "USE_CONFIGURED_HOST_CFLAGS"
        "${oneValueArgs}" "${multiValueArgs}")

    if(HHR_UNPARSED_ARGUMENTS OR HHR_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_host_header_rule: malformed arguments: "
            "${HHR_UNPARSED_ARGUMENTS}${HHR_KEYWORDS_MISSING_VALUES}")
    endif()
    foreach(_required NAME SETUP_NAME TOOL_SOURCE TOOL_OUTPUT SOURCE_WORKDIR
                      HEADER PRIMARY_OUTPUT SDK_OUTPUT)
        if(NOT HHR_${_required})
            message(FATAL_ERROR "aros_host_header_rule: ${_required} is required")
        endif()
    endforeach()
    foreach(_name HHR_NAME HHR_SETUP_NAME)
        if("${${_name}}" STREQUAL "." OR "${${_name}}" STREQUAL ".." OR
           "${${_name}}" MATCHES "[^A-Za-z0-9_.+-]")
            message(FATAL_ERROR
                "aros_host_header_rule: target names must be one safe name")
        endif()
    endforeach()
    if(HHR_NAME STREQUAL HHR_SETUP_NAME)
        message(FATAL_ERROR "aros_host_header_rule: owner and setup names must differ")
    endif()
    if(NOT HHR_HEADER MATCHES "^[A-Za-z0-9_+./-]+\\.h$" OR
       HHR_HEADER MATCHES "(^|/)\\.\\.?(/|$)")
        message(FATAL_ERROR "aros_host_header_rule: HEADER must be a safe relative .h path")
    endif()
    if(HHR_USE_CONFIGURED_HOST_CFLAGS AND NOT DEFINED AROS_HOST_CFLAGS)
        message(FATAL_ERROR
            "aros_host_header_rule: source uses HOST_CFLAGS but AROS_HOST_CFLAGS is not configured")
    endif()
    if(NOT AROS_HOST_CC)
        message(FATAL_ERROR "aros_host_header_rule: AROS_HOST_CC is not configured")
    endif()
    if(NOT DEFINED AROS_SOURCE_DIR OR NOT IS_ABSOLUTE "${AROS_SOURCE_DIR}")
        message(FATAL_ERROR "aros_host_header_rule: AROS_SOURCE_DIR must be absolute")
    endif()
    foreach(_root_var AROS_BUILD_DIR AROS_GENINC_DIR AROS_SDK_INCLUDE_DIR)
        if(NOT DEFINED ${_root_var} OR NOT IS_ABSOLUTE "${${_root_var}}" OR
           "${${_root_var}}" MATCHES "[;\"\r\n$]")
            message(FATAL_ERROR
                "aros_host_header_rule: ${_root_var} must be a safe absolute path")
        endif()
        cmake_path(NORMAL_PATH ${_root_var})
        if(IS_SYMLINK "${${_root_var}}")
            message(FATAL_ERROR
                "aros_host_header_rule: configured root is a symlink: ${${_root_var}}")
        endif()
    endforeach()

    _aros_host_header_validate_source("${AROS_SOURCE_DIR}" "${HHR_SOURCE_WORKDIR}" TRUE)
    _aros_host_header_validate_source("${AROS_SOURCE_DIR}" "${HHR_TOOL_SOURCE}" FALSE)
    foreach(_source IN LISTS HHR_SOURCE_PREREQUISITES)
        _aros_host_header_validate_source("${AROS_SOURCE_DIR}" "${_source}" FALSE)
    endforeach()

    _aros_host_header_validate_output("${AROS_BUILD_DIR}/gen" "${HHR_TOOL_OUTPUT}")
    _aros_host_header_validate_output("${AROS_GENINC_DIR}" "${HHR_PRIMARY_OUTPUT}")
    _aros_host_header_validate_output("${AROS_SDK_INCLUDE_DIR}" "${HHR_SDK_OUTPUT}")
    _aros_host_header_validate_output("${CMAKE_BINARY_DIR}" "${HHR_TOOL_OUTPUT}")
    _aros_host_header_validate_output("${CMAKE_BINARY_DIR}" "${HHR_PRIMARY_OUTPUT}")
    _aros_host_header_validate_output("${CMAKE_BINARY_DIR}" "${HHR_SDK_OUTPUT}")
    cmake_path(IS_PREFIX AROS_GENINC_DIR "${HHR_PRIMARY_OUTPUT}" NORMALIZE _primary_rooted)
    cmake_path(IS_PREFIX AROS_SDK_INCLUDE_DIR "${HHR_SDK_OUTPUT}" NORMALIZE _sdk_rooted)
    if(NOT _primary_rooted OR NOT _sdk_rooted)
        message(FATAL_ERROR "aros_host_header_rule: output roots do not match configured roots")
    endif()
    if(HHR_PRIMARY_OUTPUT STREQUAL HHR_SDK_OUTPUT)
        message(FATAL_ERROR "aros_host_header_rule: primary and SDK outputs must differ")
    endif()
    if(NOT HHR_SETUP_DIRECTORIES)
        message(FATAL_ERROR "aros_host_header_rule: SETUP_DIRECTORIES is required")
    endif()
    list(LENGTH HHR_SETUP_DIRECTORIES _setup_directory_count)
    if(NOT _setup_directory_count EQUAL 2)
        message(FATAL_ERROR "aros_host_header_rule: exactly two setup directories are required")
    endif()
    foreach(_directory IN LISTS HHR_SETUP_DIRECTORIES)
        cmake_path(IS_PREFIX AROS_GENINC_DIR "${_directory}" NORMALIZE _under_geninc)
        cmake_path(IS_PREFIX AROS_SDK_INCLUDE_DIR "${_directory}" NORMALIZE _under_sdk)
        if(_under_geninc)
            set(_directory_root "${AROS_GENINC_DIR}")
        elseif(_under_sdk)
            set(_directory_root "${AROS_SDK_INCLUDE_DIR}")
        else()
            message(FATAL_ERROR
                "aros_host_header_rule: setup directory is outside include roots: ${_directory}")
        endif()
        _aros_host_header_validate_output("${_directory_root}" "${_directory}")
    endforeach()
    cmake_path(RELATIVE_PATH HHR_PRIMARY_OUTPUT BASE_DIRECTORY "${AROS_GENINC_DIR}"
        OUTPUT_VARIABLE _primary_relative)
    cmake_path(RELATIVE_PATH HHR_SDK_OUTPUT BASE_DIRECTORY "${AROS_SDK_INCLUDE_DIR}"
        OUTPUT_VARIABLE _sdk_relative)
    if(NOT _primary_relative STREQUAL HHR_HEADER OR
       NOT _sdk_relative STREQUAL HHR_HEADER)
        message(FATAL_ERROR "aros_host_header_rule: output paths do not match HEADER")
    endif()
    cmake_path(GET HHR_PRIMARY_OUTPUT PARENT_PATH _primary_parent)
    cmake_path(GET HHR_SDK_OUTPUT PARENT_PATH _sdk_parent)
    set(_expected_setup_directories "${_primary_parent}" "${_sdk_parent}")
    list(SORT _expected_setup_directories)
    set(_declared_setup_directories ${HHR_SETUP_DIRECTORIES})
    list(SORT _declared_setup_directories)
    if(NOT _declared_setup_directories STREQUAL _expected_setup_directories)
        message(FATAL_ERROR
            "aros_host_header_rule: setup directories must be the exact two output parents")
    endif()
    foreach(_flag IN LISTS HHR_COMPILE_FLAGS)
        if(NOT _flag MATCHES "^[A-Za-z0-9_./+,:=-]+$")
            message(FATAL_ERROR "aros_host_header_rule: unsafe literal compiler flag: ${_flag}")
        endif()
    endforeach()
    foreach(_argument IN LISTS AROS_HOST_CFLAGS)
        if(NOT _argument MATCHES "^[A-Za-z0-9_./+,:=-]+$")
            message(FATAL_ERROR "aros_host_header_rule: unsafe configured host flag: ${_argument}")
        endif()
    endforeach()

    set(_tool_target "aros-host-header-tool-${HHR_NAME}")
    set(_output_target "aros-host-header-output-${HHR_NAME}")
    set(_setup_action_target "aros-host-header-setup-action-${HHR_NAME}")
    foreach(_target "${HHR_SETUP_NAME}" "${HHR_NAME}"
                   "${_tool_target}" "${_output_target}" "${_setup_action_target}")
        if(TARGET "${_target}")
            message(FATAL_ERROR "aros_host_header_rule: target already exists: ${_target}")
        endif()
    endforeach()

    get_property(_claimed_outputs GLOBAL PROPERTY AROS_HOST_HEADER_RULE_OUTPUTS)
    foreach(_output "${HHR_TOOL_OUTPUT}" "${HHR_PRIMARY_OUTPUT}" "${HHR_SDK_OUTPUT}")
        list(FIND _claimed_outputs "${_output}" _claimed_index)
        if(NOT _claimed_index EQUAL -1)
            message(FATAL_ERROR
                "aros_host_header_rule: output already has a declared producer: ${_output}")
        endif()
        list(APPEND _claimed_outputs "${_output}")
    endforeach()
    set_property(GLOBAL PROPERTY AROS_HOST_HEADER_RULE_OUTPUTS "${_claimed_outputs}")

    set(_tool_parent "${HHR_TOOL_OUTPUT}")
    cmake_path(GET _tool_parent PARENT_PATH _tool_parent)
    set(_compile_flags ${HHR_COMPILE_FLAGS})
    if(HHR_USE_CONFIGURED_HOST_CFLAGS)
        list(APPEND _compile_flags ${AROS_HOST_CFLAGS})
    endif()
    add_custom_command(
        OUTPUT "${HHR_TOOL_OUTPUT}"
        COMMAND "${CMAKE_COMMAND}" -E make_directory "${_tool_parent}"
        COMMAND "${AROS_HOST_CC}" ${_compile_flags}
            "${HHR_TOOL_SOURCE}" -o "${HHR_TOOL_OUTPUT}"
        DEPENDS "${HHR_TOOL_SOURCE}"
        COMMENT "Building host header tool for ${HHR_NAME}"
        VERBATIM)
    add_custom_target("${_tool_target}" DEPENDS "${HHR_TOOL_OUTPUT}")

    list(GET HHR_SETUP_DIRECTORIES 0 _setup_directory_one)
    list(GET HHR_SETUP_DIRECTORIES 1 _setup_directory_two)
    set(_setup_commands
        COMMAND "${CMAKE_COMMAND}"
            -DHOST_HEADER_PREPARE_ONLY=ON
            "-DHOST_HEADER_PRIMARY=${HHR_PRIMARY_OUTPUT}"
            "-DHOST_HEADER_SDK=${HHR_SDK_OUTPUT}"
            "-DHOST_HEADER_BINARY_ROOT=${CMAKE_BINARY_DIR}"
            "-DHOST_HEADER_GENINC_ROOT=${AROS_GENINC_DIR}"
            "-DHOST_HEADER_SDK_ROOT=${AROS_SDK_INCLUDE_DIR}"
            "-DHOST_HEADER_SETUP_DIRECTORY_ONE=${_setup_directory_one}"
            "-DHOST_HEADER_SETUP_DIRECTORY_TWO=${_setup_directory_two}"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunHostHeader.cmake")
    add_custom_target("${_setup_action_target}" ${_setup_commands} VERBATIM)
    add_custom_target("${HHR_SETUP_NAME}")
    add_dependencies("${HHR_SETUP_NAME}" "${_setup_action_target}")

    add_custom_command(
        OUTPUT "${HHR_PRIMARY_OUTPUT}" "${HHR_SDK_OUTPUT}"
        COMMAND "${CMAKE_COMMAND}"
            "-DHOST_HEADER_TOOL=${HHR_TOOL_OUTPUT}"
            "-DHOST_HEADER_WORKDIR=${HHR_SOURCE_WORKDIR}"
            "-DHOST_HEADER_PRIMARY=${HHR_PRIMARY_OUTPUT}"
            "-DHOST_HEADER_SDK=${HHR_SDK_OUTPUT}"
            "-DHOST_HEADER_BINARY_ROOT=${CMAKE_BINARY_DIR}"
            "-DHOST_HEADER_SOURCE_ROOT=${AROS_SOURCE_DIR}"
            "-DHOST_HEADER_GENINC_ROOT=${AROS_GENINC_DIR}"
            "-DHOST_HEADER_SDK_ROOT=${AROS_SDK_INCLUDE_DIR}"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunHostHeader.cmake"
        DEPENDS "${HHR_TOOL_OUTPUT}" ${HHR_SOURCE_PREREQUISITES}
        COMMENT "Generating ${HHR_HEADER} for ${HHR_NAME}"
        VERBATIM)
    add_custom_target("${_output_target}" DEPENDS
        "${HHR_PRIMARY_OUTPUT}" "${HHR_SDK_OUTPUT}")
    add_dependencies("${_output_target}" "${HHR_SETUP_NAME}" "${_tool_target}")
    add_custom_target("${HHR_NAME}")
    add_dependencies("${HHR_NAME}" "${HHR_SETUP_NAME}" "${_output_target}")
endfunction()


function(_aros_host_header_validate_source _root _path _directory)
    if(NOT IS_ABSOLUTE "${_path}" OR "${_path}" MATCHES "[;\"\r\n$]")
        message(FATAL_ERROR "aros_host_header_rule: unsafe source path: ${_path}")
    endif()
    if("${_path}" MATCHES "(^|/)\\.\\.?(/|$)")
        message(FATAL_ERROR "aros_host_header_rule: source path has traversal components: ${_path}")
    endif()
    cmake_path(NORMAL_PATH _path)
    cmake_path(IS_PREFIX _root "${_path}" NORMALIZE _lexically_inside)
    if(NOT _lexically_inside)
        message(FATAL_ERROR "aros_host_header_rule: source path escapes AROS_SOURCE_DIR: ${_path}")
    endif()
    file(RELATIVE_PATH _relative "${_root}" "${_path}")
    if(_directory AND _path STREQUAL _root)
        return()
    endif()
    string(REPLACE "/" ";" _components "${_relative}")
    set(_probe "${_root}")
    foreach(_component IN LISTS _components)
        if(_component STREQUAL "" OR _component STREQUAL "." OR _component STREQUAL "..")
            message(FATAL_ERROR "aros_host_header_rule: unsafe source component in ${_path}")
        endif()
        set(_probe "${_probe}/${_component}")
        if(IS_SYMLINK "${_probe}")
            message(FATAL_ERROR "aros_host_header_rule: source path crosses a symlink: ${_probe}")
        endif()
    endforeach()
    if(_directory)
        if(NOT IS_DIRECTORY "${_path}")
            message(FATAL_ERROR "aros_host_header_rule: source workdir is missing: ${_path}")
        endif()
    elseif(NOT EXISTS "${_path}" OR IS_DIRECTORY "${_path}")
        message(FATAL_ERROR "aros_host_header_rule: source file is missing: ${_path}")
    endif()
    file(REAL_PATH "${_root}" _root_real)
    file(REAL_PATH "${_path}" _path_real)
    cmake_path(IS_PREFIX _root_real "${_path_real}" NORMALIZE _physically_inside)
    if(NOT _physically_inside)
        message(FATAL_ERROR "aros_host_header_rule: source path escapes physically: ${_path}")
    endif()
endfunction()

function(_aros_host_header_validate_output _root _path)
    if(NOT IS_ABSOLUTE "${_path}" OR "${_path}" MATCHES "[;\"\r\n$]")
        message(FATAL_ERROR "aros_host_header_rule: unsafe output path: ${_path}")
    endif()
    if("${_path}" MATCHES "(^|/)\\.\\.?(/|$)")
        message(FATAL_ERROR "aros_host_header_rule: output path has traversal components: ${_path}")
    endif()
    cmake_path(NORMAL_PATH _path)
    cmake_path(NORMAL_PATH _root OUTPUT_VARIABLE _normalized_root)
    cmake_path(IS_PREFIX _normalized_root "${_path}" NORMALIZE _inside)
    if(NOT _inside OR _path STREQUAL _normalized_root)
        message(FATAL_ERROR "aros_host_header_rule: output escapes its configured root: ${_path}")
    endif()
    file(RELATIVE_PATH _relative "${_normalized_root}" "${_path}")
    string(REPLACE "/" ";" _components "${_relative}")
    set(_probe "${_normalized_root}")
    foreach(_component IN LISTS _components)
        if(_component STREQUAL "" OR _component STREQUAL "." OR _component STREQUAL "..")
            message(FATAL_ERROR "aros_host_header_rule: unsafe output component in ${_path}")
        endif()
        set(_probe "${_probe}/${_component}")
        if(IS_SYMLINK "${_probe}")
            message(FATAL_ERROR "aros_host_header_rule: output path crosses a symlink: ${_probe}")
        endif()
    endforeach()
    if(EXISTS "${_path}")
        file(REAL_PATH "${_normalized_root}" _root_real)
        file(REAL_PATH "${_path}" _output_real)
        cmake_path(IS_PREFIX _root_real "${_output_real}" NORMALIZE _physical_inside)
        if(NOT _physical_inside)
            message(FATAL_ERROR "aros_host_header_rule: output escapes physically: ${_path}")
        endif()
    endif()
endfunction()
