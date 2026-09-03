cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_build "${_temp_root}/aros source inventory ${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/source-inventory-reconfigure")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "source-inventory initial configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target fixture
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "source-inventory ordered rebuild failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()
if(NOT EXISTS "${_build}/Ports/fixture/.fixture-fetched" OR
   NOT EXISTS "${_build}/reconfigured.txt")
    message(FATAL_ERROR
        "fetch completion did not trigger the required CMake regeneration\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

file(REMOVE_RECURSE "${_build}")
message(STATUS "fetched source inventory reconfigure test passed")
