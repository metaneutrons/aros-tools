cmake_minimum_required(VERSION 3.24)
project(HeaderTransformBoundaryFixture C)

if(NOT DEFINED AROS_ENGINE_DIR OR "${AROS_ENGINE_DIR}" STREQUAL "")
    message(FATAL_ERROR "AROS_ENGINE_DIR is required")
endif()
if(NOT DEFINED AROS_HEADER_PROBE OR "${AROS_HEADER_PROBE}" STREQUAL "")
    set(AROS_HEADER_PROBE positive)
endif()

# Load the real AROS.cmake helper without copying the full SDK into each probe.
# Its one BootstrapSDK side effect is replaced by a no-op module; the function
# under test is still the production aros_transform_header definition.
set(_fixture_modules "${CMAKE_BINARY_DIR}/fixture-modules")
file(MAKE_DIRECTORY "${_fixture_modules}")
file(WRITE "${_fixture_modules}/BootstrapSDK.cmake"
    "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
set(CMAKE_MODULE_PATH "${_fixture_modules}")
set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/input-root")
file(MAKE_DIRECTORY
    "${AROS_SOURCE_DIR}/compiler/startup"
    "${CMAKE_BINARY_DIR}/inputs")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/startup.c"
    "int aros_boundary_test_startup;\n")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/detach.c"
    "int aros_boundary_test_detach;\n")
file(WRITE "${CMAKE_BINARY_DIR}/inputs/first.h"
    "first header, line one\nfirst header, line two\n")
file(WRITE "${CMAKE_BINARY_DIR}/inputs/second.h"
    "second header, line one\nsecond header, line two\n")

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
include("${AROS_ENGINE_DIR}/AROS.cmake")

if(AROS_HEADER_PROBE STREQUAL "whole-line")
    file(WRITE "${CMAKE_BINARY_DIR}/inputs/config.h"
        "before; preserved\n/* PREFIX FEATURE_TOKEN suffix */\nafter\n")
    aros_transform_header(
        NAME whole-line-headers
        INPUT "${CMAKE_BINARY_DIR}/inputs/config.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/config.h"
        WHOLE_LINE_CONTAINING MATCH FEATURE_TOKEN
        REPLACEMENT [=[#if defined(PLATFORM)
#define FEATURE_TOKEN
/* literal; ${prefix} */
#else
#undef FEATURE_TOKEN
#endif]=])
    return()
endif()

set(_outside "${CMAKE_BINARY_DIR}/../outside-target")
if(AROS_HEADER_PROBE STREQUAL "intermediate-symlink")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/SDK/include" "${_outside}")
    file(CREATE_LINK "${_outside}"
        "${CMAKE_BINARY_DIR}/SDK/include/devices" SYMBOLIC)
elseif(AROS_HEADER_PROBE STREQUAL "root-ancestor-symlink")
    file(MAKE_DIRECTORY "${_outside}")
    file(CREATE_LINK "${_outside}" "${CMAKE_BINARY_DIR}/SDK" SYMBOLIC)
else()
    file(MAKE_DIRECTORY "${AROS_SDK_INCLUDE_DIR}" "${AROS_GENINC_DIR}")
endif()

set(_first_input "${CMAKE_BINARY_DIR}/inputs/first.h")
set(_second_input "${CMAKE_BINARY_DIR}/inputs/second.h")
set(_first_output "${AROS_SDK_INCLUDE_DIR}/devices/first.h")
set(_second_output "${AROS_SDK_INCLUDE_DIR}/devices/second.h")

if(AROS_HEADER_PROBE STREQUAL "outside-output")
    # Still below an explicitly allowed SDK root, but that root itself is
    # outside the configured build tree.
    set(AROS_SDK_INCLUDE_DIR "${CMAKE_BINARY_DIR}/../outside-sdk-root")
    set(_first_output "${AROS_SDK_INCLUDE_DIR}/devices/first.h")
elseif(AROS_HEADER_PROBE STREQUAL "outside-generated-root")
    set(_first_output "${CMAKE_BINARY_DIR}/outside/first.h")
endif()

aros_transform_header(
    NAME copy-headers
    INPUT "${_first_input}"
    OUTPUT "${_first_output}"
    COPY_ONLY)

if(AROS_HEADER_PROBE STREQUAL "duplicate-same-owner")
    aros_transform_header(
        NAME copy-headers
        INPUT "${_second_input}"
        OUTPUT "${_first_output}"
        COPY_ONLY)
elseif(NOT AROS_HEADER_PROBE MATCHES "^(intermediate-symlink|root-ancestor-symlink|outside-output|outside-generated-root)$")
    aros_transform_header(
        NAME copy-headers
        INPUT "${_second_input}"
        OUTPUT "${_second_output}"
        COPY_ONLY)
endif()
