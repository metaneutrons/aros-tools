cmake_minimum_required(VERSION 3.22)

# Materialises one declared `%copy_includes` wildcard after its owning fetched
# source tree exists.  The caller has already tied this script to the fetch
# completion stamp; keeping that dependency outside this script makes a cold
# Ninja graph deterministic and avoids configure-time port probes.

foreach(_required IN ITEMS
        AROS_STAGE_HEADERS_SOURCE
        AROS_STAGE_HEADERS_DEST
        AROS_STAGE_HEADERS_PATTERN
        AROS_STAGE_HEADERS_FLATTEN
        AROS_STAGE_HEADERS_SDK_ROOT
        AROS_STAGE_HEADERS_GEN_ROOT
        AROS_STAGE_HEADERS_STAMP)
    if(NOT DEFINED ${_required})
        message(FATAL_ERROR "StageHeaderGlob.cmake requires ${_required}")
    endif()
endforeach()

if(NOT IS_DIRECTORY "${AROS_STAGE_HEADERS_SOURCE}")
    message(FATAL_ERROR
        "fetched header source directory does not exist: ${AROS_STAGE_HEADERS_SOURCE}")
endif()
if(NOT AROS_STAGE_HEADERS_PATTERN MATCHES "[*?\\[]")
    message(FATAL_ERROR
        "fetched header staging requires a glob pattern: ${AROS_STAGE_HEADERS_PATTERN}")
endif()

cmake_path(ABSOLUTE_PATH AROS_STAGE_HEADERS_SDK_ROOT NORMALIZE
    OUTPUT_VARIABLE _sdk_root)
cmake_path(ABSOLUTE_PATH AROS_STAGE_HEADERS_GEN_ROOT NORMALIZE
    OUTPUT_VARIABLE _gen_root)
cmake_path(ABSOLUTE_PATH AROS_STAGE_HEADERS_STAMP NORMALIZE
    OUTPUT_VARIABLE _stamp)

set(_excludes "")
if(AROS_STAGE_HEADERS_EXCLUDES)
    string(REPLACE "|" ";" _excludes "${AROS_STAGE_HEADERS_EXCLUDES}")
endif()

file(GLOB _matches LIST_DIRECTORIES FALSE
    RELATIVE "${AROS_STAGE_HEADERS_SOURCE}"
    "${AROS_STAGE_HEADERS_SOURCE}/${AROS_STAGE_HEADERS_PATTERN}")
list(SORT _matches)
if(NOT _matches)
    # Match the configure-time %copy_includes path: an empty pattern is a
    # recorded no-op, not a failure. This keeps a cold fetched-port build from
    # acquiring different semantics simply because its source tree did not
    # exist during CMake configuration.
    get_filename_component(_stamp_dir "${_stamp}" DIRECTORY)
    file(MAKE_DIRECTORY "${_stamp_dir}")
    file(TOUCH "${_stamp}")
    return()
endif()

set(_staged 0)
foreach(_relative IN LISTS _matches)
    if(_relative MATCHES "(^|/)\\.\\.(/|$)")
        message(FATAL_ERROR "fetched header path escapes source directory: ${_relative}")
    endif()
    if(AROS_STAGE_HEADERS_FLATTEN)
        get_filename_component(_published "${_relative}" NAME)
    else()
        set(_published "${_relative}")
    endif()
    if(_published IN_LIST _excludes)
        continue()
    endif()

    if(AROS_STAGE_HEADERS_DEST STREQUAL ".")
        set(_header_path "${_published}")
    else()
        set(_header_path "${AROS_STAGE_HEADERS_DEST}/${_published}")
    endif()
    cmake_path(NORMAL_PATH _header_path)
    if(IS_ABSOLUTE "${_header_path}" OR _header_path MATCHES "(^|/)\\.\\.(/|$)")
        message(FATAL_ERROR "fetched header destination escapes its include root: ${_header_path}")
    endif()

    set(_source "${AROS_STAGE_HEADERS_SOURCE}/${_relative}")
    foreach(_root IN ITEMS "${_sdk_root}" "${_gen_root}")
        set(_destination "${_root}/${_header_path}")
        cmake_path(NORMAL_PATH _destination)
        cmake_path(IS_PREFIX _root "${_destination}" NORMALIZE _inside)
        if(NOT _inside OR _destination STREQUAL _root)
            message(FATAL_ERROR
                "fetched header destination escapes its include root: ${_destination}")
        endif()
        get_filename_component(_destination_dir "${_destination}" DIRECTORY)
        file(MAKE_DIRECTORY "${_destination_dir}")
        execute_process(
            COMMAND "${CMAKE_COMMAND}" -E copy_if_different
                "${_source}" "${_destination}"
            COMMAND_ERROR_IS_FATAL ANY)
    endforeach()
    math(EXPR _staged "${_staged} + 1")
endforeach()

if(_staged EQUAL 0)
    # `filter-out` is allowed to remove every match, just like in the
    # configure-time path above.
    get_filename_component(_stamp_dir "${_stamp}" DIRECTORY)
    file(MAKE_DIRECTORY "${_stamp_dir}")
    file(TOUCH "${_stamp}")
    return()
endif()

get_filename_component(_stamp_dir "${_stamp}" DIRECTORY)
file(MAKE_DIRECTORY "${_stamp_dir}")
file(TOUCH "${_stamp}")
