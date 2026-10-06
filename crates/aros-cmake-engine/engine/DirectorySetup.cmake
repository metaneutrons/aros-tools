# Materialize the directories owned by a closed MetaMake setup target.
#
# The transpiler only emits paths rooted below AROS_BUILD_DIR,
# AROS_GENINC_DIR, AROS_SDK_INCLUDE_DIR, AROS_DEVELOPER_INCLUDE_DIR, or the
# configured AROS_DEVELOPER_LIB_DIR. This helper checks that contract again
# before creating a phony target which performs idempotent
# `cmake -E make_directory` operations.

include_guard(GLOBAL)
include(CMakeParseArguments)

# REAL_PATH requires an existing path to resolve ancestor aliases reliably.
# Resolve the nearest existing directory, then append the validated missing
# suffix without creating any directory during configuration.
function(_aros_directory_physical_path path output)
    set(_ancestor "${path}")
    set(_suffix "")
    while(NOT EXISTS "${_ancestor}")
        if(IS_SYMLINK "${_ancestor}")
            message(FATAL_ERROR "aros_prepare_directories: dangling symlink: ${_ancestor}")
        endif()
        cmake_path(GET _ancestor FILENAME _component)
        cmake_path(GET _ancestor PARENT_PATH _parent)
        if(_parent STREQUAL _ancestor OR _component STREQUAL "")
            message(FATAL_ERROR "aros_prepare_directories: no existing ancestor: ${path}")
        endif()
        if(_suffix STREQUAL "")
            set(_suffix "${_component}")
        else()
            set(_suffix "${_component}/${_suffix}")
        endif()
        set(_ancestor "${_parent}")
    endwhile()
    if(NOT IS_DIRECTORY "${_ancestor}")
        message(FATAL_ERROR "aros_prepare_directories: ancestor is not a directory: ${_ancestor}")
    endif()
    file(REAL_PATH "${_ancestor}" _physical)
    if(NOT _suffix STREQUAL "")
        cmake_path(APPEND _physical "${_suffix}")
    endif()
    set(${output} "${_physical}" PARENT_SCOPE)
endfunction()

# aros_prepare_directories(NAME <target> DIRECTORIES <absolute-path>...)
function(aros_prepare_directories)
    set(oneValueArgs NAME)
    set(multiValueArgs DIRECTORIES)
    cmake_parse_arguments(PARSE_ARGV 0 PD "" "${oneValueArgs}" "${multiValueArgs}")

    if(PD_UNPARSED_ARGUMENTS OR PD_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR
            "aros_prepare_directories: malformed arguments: "
            "${PD_UNPARSED_ARGUMENTS}${PD_KEYWORDS_MISSING_VALUES}")
    endif()
    if(NOT PD_NAME OR PD_NAME MATCHES "[^A-Za-z0-9_.+-]" OR
       PD_NAME STREQUAL "." OR PD_NAME STREQUAL "..")
        message(FATAL_ERROR
            "aros_prepare_directories: NAME must be one safe target name")
    endif()
    if(TARGET "${PD_NAME}")
        message(FATAL_ERROR
            "aros_prepare_directories: target already exists: ${PD_NAME}")
    endif()
    if(NOT PD_DIRECTORIES)
        message(FATAL_ERROR
            "aros_prepare_directories: at least one DIRECTORY is required")
    endif()
    list(LENGTH PD_DIRECTORIES _directory_count)
    if(_directory_count GREATER 4096)
        message(FATAL_ERROR "aros_prepare_directories: directory list exceeds 4096 entries")
    endif()

    set(_allowed_roots "")
    set(_root_variables AROS_BUILD_DIR AROS_GENINC_DIR AROS_DEVELOPER_INCLUDE_DIR)
    # Older bounded fixtures may omit these roots. The engine supplies them;
    # if present, validate them just as strictly as the original roots.
    if(DEFINED AROS_SDK_INCLUDE_DIR)
        list(APPEND _root_variables AROS_SDK_INCLUDE_DIR)
    endif()
    if(DEFINED AROS_DEVELOPER_LIB_DIR)
        list(APPEND _root_variables AROS_DEVELOPER_LIB_DIR)
    endif()
    foreach(_root_var IN LISTS _root_variables)
        if(NOT DEFINED ${_root_var} OR "${${_root_var}}" STREQUAL "" OR
           NOT IS_ABSOLUTE "${${_root_var}}" OR
           "${${_root_var}}" MATCHES "[;\"\r\n$]")
            message(FATAL_ERROR
                "aros_prepare_directories: ${_root_var} must be explicitly configured as absolute")
        endif()
        cmake_path(NORMAL_PATH ${_root_var})
        if(IS_SYMLINK "${${_root_var}}")
            message(FATAL_ERROR
                "aros_prepare_directories: configured root is a symlink: ${${_root_var}}")
        endif()
        list(APPEND _allowed_roots "${${_root_var}}")
    endforeach()

    set(_safe_directories "")
    foreach(_directory IN LISTS PD_DIRECTORIES)
        if(NOT IS_ABSOLUTE "${_directory}" OR
           "${_directory}" MATCHES "[;\"\r\n$]")
            message(FATAL_ERROR
                "aros_prepare_directories: directory must be a safe absolute path: ${_directory}")
        endif()
        cmake_path(NORMAL_PATH _directory)

        set(_contained FALSE)
        set(_containing_root "")
        foreach(_root IN LISTS _allowed_roots)
            cmake_path(IS_PREFIX _root "${_directory}" NORMALIZE _inside)
            if(_inside)
                set(_contained TRUE)
                set(_containing_root "${_root}")
                break()
            endif()
        endforeach()
        if(NOT _contained)
            message(FATAL_ERROR
                "aros_prepare_directories: ${_directory} escapes configured roots")
        endif()

        # Reject a symlink at every component below the configured root, even
        # if it currently resolves back inside the root. Then verify physical
        # containment to catch symlinked ancestors of either configured path.
        if(IS_SYMLINK "${_containing_root}")
            message(FATAL_ERROR
                "aros_prepare_directories: configured root is a symlink: ${_containing_root}")
        endif()
        file(RELATIVE_PATH _relative "${_containing_root}" "${_directory}")
        set(_components "")
        if(NOT _relative STREQUAL "")
            string(REPLACE "/" ";" _components "${_relative}")
        endif()
        set(_probe "${_containing_root}")
        foreach(_component IN LISTS _components)
            if(_component STREQUAL "" OR _component STREQUAL "." OR
               _component STREQUAL "..")
                message(FATAL_ERROR
                    "aros_prepare_directories: unsafe directory component in ${_directory}")
            endif()
            set(_probe "${_probe}/${_component}")
            if(IS_SYMLINK "${_probe}")
                message(FATAL_ERROR
                    "aros_prepare_directories: directory path crosses a symlink: ${_probe}")
            endif()
        endforeach()

        _aros_directory_physical_path("${_containing_root}" _root_real)
        _aros_directory_physical_path("${_directory}" _directory_real)
        cmake_path(IS_PREFIX _root_real "${_directory_real}" NORMALIZE _physical_inside)
        if(NOT _physical_inside)
            message(FATAL_ERROR
                "aros_prepare_directories: ${_directory} escapes its physical configured root")
        endif()
        list(APPEND _safe_directories "${_directory}")
    endforeach()

    list(REMOVE_DUPLICATES _safe_directories)
    set(_commands "")
    set(_root_arguments "")
    list(LENGTH _allowed_roots _root_count)
    math(EXPR _last_root "${_root_count} - 1")
    foreach(_index RANGE 0 ${_last_root})
        list(GET _allowed_roots ${_index} _configured_root)
        list(APPEND _root_arguments "-DSETUP_ROOT_${_index}=${_configured_root}")
    endforeach()
    foreach(_directory IN LISTS _safe_directories)
        _aros_directory_physical_path("${_directory}" _expected_physical)
        list(APPEND _commands
            COMMAND "${CMAKE_COMMAND}"
                "-DSETUP_DIRECTORY=${_directory}"
                "-DSETUP_ROOT_COUNT=${_root_count}" ${_root_arguments}
                "-DSETUP_EXPECTED_PHYSICAL=${_expected_physical}"
                -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/RunDirectorySetup.cmake")
    endforeach()
    add_custom_target("${PD_NAME}" ${_commands} VERBATIM)
endfunction()
