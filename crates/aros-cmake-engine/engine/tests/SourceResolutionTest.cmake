cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_build "${_temp_root}/aros source resolution ${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/source-resolution")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}"
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS} -G Ninja
    RESULT_VARIABLE _result
    OUTPUT_VARIABLE _stdout
    ERROR_VARIABLE _stderr)
if(NOT _result EQUAL 0)
    message(FATAL_ERROR
        "source-resolution configure failed (${_result})\n${_stdout}\n${_stderr}")
endif()

file(REMOVE_RECURSE "${_build}")
message(STATUS "generated and fetched source resolution test passed")
