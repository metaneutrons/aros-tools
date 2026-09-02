cmake_minimum_required(VERSION 3.22)

include("${CMAKE_CURRENT_LIST_DIR}/../TransitiveHeaderBindings.cmake")

set(_fixture "${CMAKE_CURRENT_LIST_DIR}/transitive-header-bindings")
set_property(GLOBAL PROPERTY AROS_STAGED_HEADER_BINDINGS
    "GL/gla.h|gl-owner||${_fixture}/gla.h"
    "GL/gl.h|mesa-owner|0123456789abcdef|${_fixture}/not-fetched/gl.h")

_aros_collect_transitive_header_bindings(
    _owners _hashes "${_fixture}/root.conf")

if(NOT _owners STREQUAL "gl-owner;mesa-owner")
    message(FATAL_ERROR "unexpected transitive owners: ${_owners}")
endif()
if(NOT _hashes STREQUAL "0123456789abcdef")
    message(FATAL_ERROR "unexpected deferred hashes: ${_hashes}")
endif()

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _wrapper_suffix)
set(_wrapper_temp "$ENV{TMPDIR}")
if(NOT _wrapper_temp)
    set(_wrapper_temp "${CMAKE_CURRENT_BINARY_DIR}")
endif()
set(_wrapper "${_wrapper_temp}/aros-transitive-header-${_wrapper_suffix}.c")
file(WRITE "${_wrapper}"
    "/* generated source wrapper */\n#include \"${_fixture}/gla.h\"\n")
_aros_collect_transitive_header_bindings(
    _wrapper_owners _wrapper_hashes "${_wrapper}")
if(NOT _wrapper_owners STREQUAL "gl-owner;mesa-owner")
    message(FATAL_ERROR
        "quoted local source traversal lost owners: ${_wrapper_owners}")
endif()
if(NOT _wrapper_hashes STREQUAL "0123456789abcdef")
    message(FATAL_ERROR
        "quoted local source traversal lost hashes: ${_wrapper_hashes}")
endif()
file(REMOVE "${_wrapper}")

message(STATUS "transitive staged-header binding test passed")
