cmake_minimum_required(VERSION 3.22)
project(PrivateLinklibOutput C)

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")

if(PRIVATE_LINKLIB_CASE STREQUAL "escape")
    aros_add_linklib(
        TARGET gallium_i915
        MMAKE_ID escaping-provider
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        OUTPUT_DIR "${CMAKE_BINARY_DIR}/../escape"
        SOURCES unit)
    return()
endif()

if(PRIVATE_LINKLIB_CASE STREQUAL "conflicting-modes")
    aros_add_linklib(
        TARGET gallium_i915
        MMAKE_ID conflicting-provider
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        CANONICAL_OUTPUT
        OUTPUT_DIR "${CMAKE_BINARY_DIR}/private/mesa20.0.8"
        SOURCES unit)
    return()
endif()

if(PRIVATE_LINKLIB_CASE STREQUAL "empty-with-sources")
    aros_add_linklib(
        TARGET mesa-sse41
        MMAKE_ID invalid-empty-provider
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        EMPTY_ARCHIVE
        OUTPUT_DIR "${CMAKE_BINARY_DIR}/private/mesa20.0.8"
        SOURCES unit)
    return()
endif()

if(PRIVATE_LINKLIB_CASE STREQUAL "empty-without-output")
    aros_add_linklib(
        TARGET mesa-sse41
        MMAKE_ID unowned-empty-provider
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        EMPTY_ARCHIVE)
    return()
endif()

if(PRIVATE_LINKLIB_CASE STREQUAL "unsafe-dollar")
    set(_unsafe_output "${CMAKE_BINARY_DIR}/private/$<CONFIG>")
elseif(PRIVATE_LINKLIB_CASE STREQUAL "unsafe-semicolon")
    set(_unsafe_output "${CMAKE_BINARY_DIR}/private;other")
elseif(PRIVATE_LINKLIB_CASE STREQUAL "unsafe-backslash")
    set(_unsafe_output "${CMAKE_BINARY_DIR}/private\\other")
endif()
if(DEFINED _unsafe_output)
    aros_add_linklib(
        TARGET gallium_i915
        MMAKE_ID unsafe-provider
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        OUTPUT_DIR "${_unsafe_output}"
        SOURCES unit)
    return()
endif()

set(_private_dir "${CMAKE_BINARY_DIR}/private/mesa20.0.8")
aros_add_linklib(
    TARGET gallium_i915
    MMAKE_ID first-provider
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    OUTPUT_DIR "${_private_dir}"
    SOURCES unit)

aros_add_linklib(
    TARGET mesa-sse41
    MMAKE_ID empty-provider
    DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
    EMPTY_ARCHIVE
    OUTPUT_DIR "${_private_dir}")
# The host AppleClang contributes `-Wl,-search_paths_first
# -Wl,-headerpad_max_install_names`, and ld.lld rejects both. A real build never
# sees them because it configures with a cross toolchain; this fixture uses the
# host compiler deliberately, to exercise our own archive and link rules, so it
# drops the host defaults (OPEN-POINTS 45).
set(CMAKE_C_LINK_FLAGS "")
add_executable(empty-consumer "${CMAKE_CURRENT_LIST_DIR}/empty_consumer.c")
aros_link_libraries(empty-consumer empty-provider)

get_target_property(_output_name first-provider OUTPUT_NAME)
get_target_property(_archive_dir first-provider ARCHIVE_OUTPUT_DIRECTORY)
if(NOT _output_name STREQUAL "gallium_i915")
    message(FATAL_ERROR "private archive name was not applied: ${_output_name}")
endif()
if(NOT _archive_dir STREQUAL _private_dir)
    message(FATAL_ERROR "private archive directory was not applied: ${_archive_dir}")
endif()

# Exact libc namespace requests reproduce the compiler-specs order ahead of
# the common SDK include root, independent of their order in USER_INCLUDES.
add_library(namespace-consumer STATIC "${CMAKE_CURRENT_LIST_DIR}/unit.c")
set(_other_include "${CMAKE_BINARY_DIR}/other-include")
file(MAKE_DIRECTORY "${_other_include}")
target_include_directories(namespace-consumer PRIVATE "${AROS_SDK_INCLUDE_DIR}")
aros_apply_includes(namespace-consumer
    INCLUDES
        "${_other_include}"
        "${AROS_SDK_INCLUDE_DIR}/aros/stdc"
        "${AROS_SDK_INCLUDE_DIR}/aros/posixc")
get_target_property(_include_dirs namespace-consumer INCLUDE_DIRECTORIES)
list(GET _include_dirs 0 _first_include)
list(GET _include_dirs 1 _second_include)
list(FIND _include_dirs "${AROS_SDK_INCLUDE_DIR}" _sdk_root_at)
list(FIND _include_dirs "${_other_include}" _other_at)
if(NOT _first_include STREQUAL "${AROS_SDK_INCLUDE_DIR}/aros/posixc" OR
   NOT _second_include STREQUAL "${AROS_SDK_INCLUDE_DIR}/aros/stdc" OR
   _sdk_root_at LESS 2 OR _other_at LESS _sdk_root_at)
    message(FATAL_ERROR
        "libc namespace order is wrong: ${_include_dirs}")
endif()

if(PRIVATE_LINKLIB_CASE STREQUAL "collision")
    aros_add_linklib(
        TARGET gallium_i915
        MMAKE_ID second-provider
        DIRECTORY "${CMAKE_CURRENT_LIST_DIR}"
        OUTPUT_DIR "${_private_dir}"
        SOURCES unit)
endif()
