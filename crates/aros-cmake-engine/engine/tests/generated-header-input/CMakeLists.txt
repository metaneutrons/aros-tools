cmake_minimum_required(VERSION 3.24)
project(GeneratedHeaderInputFixture C)

if(NOT DEFINED AROS_ENGINE_DIR OR "${AROS_ENGINE_DIR}" STREQUAL "")
    message(FATAL_ERROR "AROS_ENGINE_DIR is required")
endif()
if(NOT DEFINED AROS_HEADER_INPUT_PROBE OR "${AROS_HEADER_INPUT_PROBE}" STREQUAL "")
    set(AROS_HEADER_INPUT_PROBE chain)
endif()

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
    "int aros_generated_header_input_startup;\n")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/detach.c"
    "int aros_generated_header_input_detach;\n")

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
include("${AROS_ENGINE_DIR}/AROS.cmake")

if(AROS_HEADER_INPUT_PROBE STREQUAL "chain")
    file(WRITE "${CMAKE_BINARY_DIR}/inputs/seed.h"
        "before\n/* FEATURE_TOKEN */\nafter\n")
    aros_transform_header(
        NAME copy-stage
        INPUT "${CMAKE_BINARY_DIR}/inputs/seed.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/chain/seed.h"
        COPY_ONLY)
    aros_transform_header(
        NAME whole-stage
        INPUT "${AROS_SDK_INCLUDE_DIR}/chain/seed.h"
        OUTPUT "${CMAKE_BINARY_DIR}/gen/chain/whole.h"
        GENERATED_INPUT_OWNER copy-stage
        WHOLE_LINE_CONTAINING MATCH FEATURE_TOKEN
        REPLACEMENT [=[#if defined(PLATFORM)
#define FEATURE_TOKEN
/* literal; ${prefix} */
#else
#undef FEATURE_TOKEN
#endif]=])
    aros_transform_header(
        NAME sdk-mirror
        INPUT "${CMAKE_BINARY_DIR}/gen/chain/whole.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/chain/mirror.h"
        GENERATED_INPUT_OWNER whole-stage
        COPY_ONLY)
elseif(AROS_HEADER_INPUT_PROBE STREQUAL "unknown-producer")
    # An existing fake input and a same-named ordinary phony target must not
    # substitute for a transform output registered by aros_transform_header.
    file(MAKE_DIRECTORY "${AROS_SDK_INCLUDE_DIR}/chain")
    file(WRITE "${AROS_SDK_INCLUDE_DIR}/chain/fake.h" "cached input\n")
    add_custom_target(fake-producer)
    aros_transform_header(
        NAME unknown-consumer
        INPUT "${AROS_SDK_INCLUDE_DIR}/chain/fake.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/chain/consumer.h"
        GENERATED_INPUT_OWNER fake-producer
        COPY_ONLY)
elseif(AROS_HEADER_INPUT_PROBE STREQUAL "false-owner")
    file(MAKE_DIRECTORY "${AROS_SDK_INCLUDE_DIR}/chain")
    file(WRITE "${AROS_SDK_INCLUDE_DIR}/chain/fake.h" "cached input\n")
    aros_transform_header(
        NAME false-owner-consumer
        INPUT "${AROS_SDK_INCLUDE_DIR}/chain/fake.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/chain/consumer.h"
        GENERATED_INPUT_OWNER OFF
        COPY_ONLY)
elseif(AROS_HEADER_INPUT_PROBE STREQUAL "wrong-producer")
    file(MAKE_DIRECTORY "${AROS_SDK_INCLUDE_DIR}/chain")
    file(WRITE "${CMAKE_BINARY_DIR}/inputs/seed.h" "seed input\n")
    file(WRITE "${AROS_SDK_INCLUDE_DIR}/chain/produced.h" "cached input\n")
    aros_transform_header(
        NAME real-producer
        INPUT "${CMAKE_BINARY_DIR}/inputs/seed.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/chain/produced.h"
        COPY_ONLY)
    aros_transform_header(
        NAME wrong-consumer
        INPUT "${AROS_SDK_INCLUDE_DIR}/chain/produced.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/chain/consumer.h"
        GENERATED_INPUT_OWNER someone-else
        COPY_ONLY)
elseif(AROS_HEADER_INPUT_PROBE STREQUAL "source-input")
    file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/source-input.h" "source input\n")
    aros_transform_header(
        NAME source-consumer
        INPUT "${AROS_SOURCE_DIR}/compiler/startup/source-input.h"
        OUTPUT "${AROS_SDK_INCLUDE_DIR}/chain/consumer.h"
        GENERATED_INPUT_OWNER source-producer
        COPY_ONLY)
else()
    message(FATAL_ERROR "unknown AROS_HEADER_INPUT_PROBE=${AROS_HEADER_INPUT_PROBE}")
endif()
