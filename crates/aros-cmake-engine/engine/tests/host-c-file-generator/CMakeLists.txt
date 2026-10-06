cmake_minimum_required(VERSION 3.22)
project(HostCFileGeneratorFixture C)

include("${AROS_HOST_C_FILE_GENERATOR_ENGINE}/HostCFileGenerator.cmake")
set(AROS_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}")
if(HCF_SOURCE_ROOT_OVERRIDE)
    set(AROS_SOURCE_DIR "${HCF_SOURCE_ROOT_OVERRIDE}")
endif()
set(AROS_HOST_CC "${HCF_HOST_CC}")
set(_tool_source "${CMAKE_CURRENT_SOURCE_DIR}/generator.c")
if(HCF_TOOL_SOURCE_OVERRIDE)
    set(_tool_source "${HCF_TOOL_SOURCE_OVERRIDE}")
endif()
if(HCF_TOOL_SHA256_OVERRIDE)
    set(_tool_sha256 "${HCF_TOOL_SHA256_OVERRIDE}")
else()
    file(SHA256 "${_tool_source}" _tool_sha256)
endif()

set(_input_directory "${HCF_INPUT_DIRECTORY}")
set(_input_manifest "${HCF_INPUT_MANIFEST_JSON}")
set(_output "${CMAKE_BINARY_DIR}/gen/fixture-result.txt")
set(_arguments @INPUT_DIRECTORY@ @OUTPUT_DIRECTORY@ fixture-result.txt)
set(_source_manifest_arg "")
if(HCF_SOURCE_MANIFEST_JSON)
    set(_source_manifest_arg SOURCE_MANIFEST_JSON "${HCF_SOURCE_MANIFEST_JSON}")
endif()
if(HCF_PROBE STREQUAL "traversal")
    set(_arguments @INPUT_DIRECTORY@ @OUTPUT_DIRECTORY@ ../escape)
elseif(HCF_PROBE STREQUAL "argument-injection")
    set(_arguments @INPUT_DIRECTORY@ @OUTPUT_DIRECTORY@ "$HOME")
elseif(HCF_REAL_REFERENCE)
    set(_output "${CMAKE_BINARY_DIR}/gen/en_GB_ISO8859-1.c")
    set(_arguments @INPUT_DIRECTORY@ @OUTPUT_DIRECTORY@ en_GB_ISO8859-1 --emit-c)
endif()
if(HCF_OUTPUT_OVERRIDE)
    set(_output "${HCF_OUTPUT_OVERRIDE}")
endif()

aros_host_c_file_generator(
    NAME host-c-fixture
    TOOL_SOURCE "${_tool_source}"
    TOOL_SHA256 "${_tool_sha256}"
    OUTPUT "${_output}"
    INPUT_DIRECTORY "${_input_directory}"
    INPUT_MANIFEST_JSON "${_input_manifest}"
    ${_source_manifest_arg}
    COMPILE_FLAGS -g -Wall -Werror -Wunused -O2
    ARGUMENTS ${_arguments})

get_target_property(_private_tool host-c-fixture AROS_HOST_C_FILE_GENERATOR_TOOL)
file(WRITE "${CMAKE_BINARY_DIR}/private-tool-path.txt" "${_private_tool}\n")
if(HCF_PROBE STREQUAL "owner-collision")
    aros_host_c_file_generator(
        NAME host-c-collision
        TOOL_SOURCE "${_tool_source}"
        TOOL_SHA256 "${_tool_sha256}"
        OUTPUT "${_output}"
        INPUT_DIRECTORY "${_input_directory}"
        INPUT_MANIFEST_JSON "${_input_manifest}"
        COMPILE_FLAGS -g -Wall -Werror -Wunused -O2
        ARGUMENTS @INPUT_DIRECTORY@ @OUTPUT_DIRECTORY@ fixture-result.txt)
elseif(HCF_PROBE STREQUAL "owner-collision-case")
    aros_host_c_file_generator(
        NAME host-c-case-collision
        TOOL_SOURCE "${_tool_source}"
        TOOL_SHA256 "${_tool_sha256}"
        OUTPUT "${CMAKE_BINARY_DIR}/gen/FIXTURE-RESULT.txt"
        INPUT_DIRECTORY "${_input_directory}"
        INPUT_MANIFEST_JSON "${_input_manifest}"
        COMPILE_FLAGS -g -Wall -Werror -Wunused -O2
        ARGUMENTS @INPUT_DIRECTORY@ @OUTPUT_DIRECTORY@ FIXTURE-RESULT.txt)
endif()
