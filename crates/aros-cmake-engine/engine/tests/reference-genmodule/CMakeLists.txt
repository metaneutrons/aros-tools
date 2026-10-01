cmake_minimum_required(VERSION 3.22)
project(AROSReferenceGenmodule C)

set(AROS_GENMODULE_SOURCE_DIR "" CACHE PATH
    "Directory containing the source-owned genmodule Makefile.deps")
option(AROS_GENMODULE_VALIDATE_ONLY
    "Validate the source inventory without defining the executable" OFF)

if(NOT IS_ABSOLUTE "${AROS_GENMODULE_SOURCE_DIR}" OR
   NOT IS_DIRECTORY "${AROS_GENMODULE_SOURCE_DIR}")
    message(FATAL_ERROR
        "AROS_GENMODULE_SOURCE_DIR must be an existing absolute directory: "
        "${AROS_GENMODULE_SOURCE_DIR}")
endif()

set(_dependency_file "${AROS_GENMODULE_SOURCE_DIR}/Makefile.deps")
if(NOT EXISTS "${_dependency_file}" OR IS_DIRECTORY "${_dependency_file}" OR
   IS_SYMLINK "${_dependency_file}")
    message(FATAL_ERROR
        "genmodule source inventory is missing or unsafe: ${_dependency_file}")
endif()

# Parse only the explicit GENMODULE_SRCS assignment. The upstream file uses a
# backslash-continued, whitespace-separated list; no globbing or Make
# evaluation is involved.
file(STRINGS "${_dependency_file}" _dependency_lines)
set(_assignment_count 0)
foreach(_line IN LISTS _dependency_lines)
    if(_line MATCHES "^GENMODULE_SRCS *:= *")
        math(EXPR _assignment_count "${_assignment_count} + 1")
    endif()
endforeach()
set(_inventory_found FALSE)
set(_inventory_complete FALSE)
set(_inventory_source_names)
foreach(_line IN LISTS _dependency_lines)
    if(NOT _inventory_found)
        if(NOT _line MATCHES "^GENMODULE_SRCS *:= *(.*)$")
            continue()
        endif()
        set(_inventory_found TRUE)
        set(_fragment "${CMAKE_MATCH_1}")
    elseif(NOT _inventory_complete)
        set(_fragment "${_line}")
    else()
        continue()
    endif()

    if(_fragment MATCHES "\\\\ *$")
        string(REGEX REPLACE "\\\\ *$" "" _fragment "${_fragment}")
        set(_continues TRUE)
    else()
        set(_continues FALSE)
        set(_inventory_complete TRUE)
    endif()
    string(STRIP "${_fragment}" _fragment)
    if(NOT _fragment STREQUAL "")
        string(REGEX REPLACE " +" ";" _fragment_tokens "${_fragment}")
        list(APPEND _inventory_source_names ${_fragment_tokens})
    endif()
    if(NOT _continues)
        break()
    endif()
endforeach()

if(NOT _inventory_found OR NOT _inventory_complete OR
   NOT _assignment_count EQUAL 1 OR
   NOT _inventory_source_names)
    message(FATAL_ERROR
        "Makefile.deps must contain one complete, non-empty GENMODULE_SRCS inventory: "
        "${_dependency_file}")
endif()

set(_source_paths)
foreach(_source_name IN LISTS _inventory_source_names)
    if(NOT _source_name MATCHES "^[A-Za-z0-9_][A-Za-z0-9_.+-]*\\.c$" OR
       _source_name MATCHES "\\.\\.")
        message(FATAL_ERROR "unsafe GENMODULE_SRCS entry: ${_source_name}")
    endif()
    set(_source_path "${AROS_GENMODULE_SOURCE_DIR}/${_source_name}")
    if(NOT EXISTS "${_source_path}" OR IS_DIRECTORY "${_source_path}" OR
       IS_SYMLINK "${_source_path}")
        message(FATAL_ERROR
            "GENMODULE_SRCS source is missing or unsafe: ${_source_name}")
    endif()
    list(FIND _source_paths "${_source_path}" _duplicate_index)
    if(NOT _duplicate_index EQUAL -1)
        message(FATAL_ERROR "duplicate GENMODULE_SRCS entry: ${_source_name}")
    endif()
    list(APPEND _source_paths "${_source_path}")
endforeach()

if(AROS_GENMODULE_VALIDATE_ONLY)
    message(STATUS
        "validated ${_inventory_source_names} from ${_dependency_file}")
    return()
endif()

add_executable(genmodule ${_source_paths})
target_include_directories(genmodule PRIVATE "${AROS_GENMODULE_SOURCE_DIR}")
# These are unmodified upstream reference sources, not tools-owned C code.
# Keep warnings visible; current Apple Clang diagnoses unused locals which
# upstream GCC accepts. Actual compile/link failures remain fatal.
target_compile_options(genmodule PRIVATE -Wall -Wunused -O2)
set_target_properties(genmodule PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}")
