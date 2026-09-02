cmake_minimum_required(VERSION 3.22)
project(SourceResolutionFixture LANGUAGES C ASM)

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_PORTS_DIR "${CMAKE_BINARY_DIR}/Ports")
set(AROS_PORTS_SOURCE_DIR "${CMAKE_BINARY_DIR}/portssources")
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")

set(_fetch_destination "${AROS_PORTS_DIR}/fixture")
set(_source_root "${_fetch_destination}/package")
set(_fetch_stamp "${_fetch_destination}/.fixture-fetched")
file(MAKE_DIRECTORY "${_source_root}" "${AROS_PORTS_SOURCE_DIR}")
file(WRITE "${_source_root}/generator.py"
    "import sys\nsys.stdout.write('.text\\n')\n")
file(WRITE "${_source_root}/input.txt" "fixture\n")
file(WRITE "${_source_root}/port.c" "int stable_port_source;\n")
file(WRITE "${_fetch_stamp}" "ready\n")

add_custom_target(fixture-fetch DEPENDS "${_fetch_stamp}")
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_DESTINATION "${_fetch_destination}")
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_COMPLETION_STAMP "${_fetch_stamp}")

set(_build_root "${CMAKE_BINARY_DIR}/gen/source-resolution")
aros_generate_python_outputs(
    OWNER fixture-generate
    SOURCE_ROOT "${_source_root}"
    BUILD_ROOT "${_build_root}"
    FETCH_TARGET fixture-fetch
    SOURCE_INPUTS input.txt
    JOB
        SCRIPT generator.py
        OUTPUT generated/code.s)

# The decoy uppercase candidate deliberately exists. The exact registered
# lowercase generator product must still win on both case-sensitive and
# case-insensitive hosts.
file(MAKE_DIRECTORY "${_build_root}/generated")
file(WRITE "${_build_root}/generated/code.S" ".error wrong candidate\n")
aros_resolve_sources(_resolved_asm ""
    LANGUAGE ASM SOURCES "${_build_root}/generated/code")
if(NOT _resolved_asm STREQUAL "${_build_root}/generated/code.s")
    message(FATAL_ERROR
        "registered lowercase generator output lost to filesystem candidate: ${_resolved_asm}")
endif()

# Fetched sources owned by a Python capability use the same proxy identity on
# cold and warm configures, even when the unpacked source already exists.
aros_resolve_sources(_resolved_c ""
    LANGUAGE C SOURCES "${_source_root}/port")
if(_resolved_c STREQUAL "${_source_root}/port.c" OR
   NOT _resolved_c MATCHES "/CMakeFiles/aros-port-sources/[0-9a-f]+\\.c$")
    message(FATAL_ERROR
        "fetched source did not retain its stable proxy: ${_resolved_c}")
endif()

# Build-tree include paths are retained before their generators run, but the
# fetched Ports namespace is owned exclusively by aros-fetch. Merely applying
# an include path must not create the archive extraction root and make the
# fetcher's stale-tree guard reject a cold build.
set(_consumer_source "${CMAKE_BINARY_DIR}/include-consumer.c")
set(_generated_include "${CMAKE_BINARY_DIR}/gen/not-yet-generated/include")
set(_unfetched_include "${AROS_PORTS_DIR}/unfetched/package/include")
file(WRITE "${_consumer_source}" "int include_consumer;\n")
add_library(include-consumer STATIC "${_consumer_source}")
aros_apply_includes(include-consumer
    INCLUDES "${_generated_include}" "${_unfetched_include}")
if(NOT IS_DIRECTORY "${_generated_include}")
    message(FATAL_ERROR
        "ordinary generated include directory was not materialised")
endif()
if(EXISTS "${AROS_PORTS_DIR}/unfetched")
    message(FATAL_ERROR
        "fetched include path pre-created its archive destination")
endif()
