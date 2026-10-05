cmake_minimum_required(VERSION 3.24)
project(MalformedWholeLineHelperFixture C)

if(NOT DEFINED AROS_ENGINE_DIR OR "${AROS_ENGINE_DIR}" STREQUAL "")
    message(FATAL_ERROR "AROS_ENGINE_DIR is required")
endif()

set(_fixture_modules "${CMAKE_BINARY_DIR}/fixture-modules")
file(MAKE_DIRECTORY "${_fixture_modules}")
file(WRITE "${_fixture_modules}/BootstrapSDK.cmake"
    "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
set(CMAKE_MODULE_PATH "${_fixture_modules}")
set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source")
file(MAKE_DIRECTORY "${AROS_SOURCE_DIR}/compiler/startup" "${CMAKE_BINARY_DIR}/inputs")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/startup.c"
    "int aros_whole_line_startup;\n")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/detach.c"
    "int aros_whole_line_detach;\n")

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
include("${AROS_ENGINE_DIR}/AROS.cmake")

aros_transform_header(
    NAME malformed-whole-line
    INPUT "${CMAKE_BINARY_DIR}/inputs/input.h"
    OUTPUT "${AROS_SDK_INCLUDE_DIR}/malformed.h"
    WHOLE_LINE_CONTAINING
    MATCH TOKEN
    REPLACEMENT replacement
    SUBSTITUTIONS token replacement)
