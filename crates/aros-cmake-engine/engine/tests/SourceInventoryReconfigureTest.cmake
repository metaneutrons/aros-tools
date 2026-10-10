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
file(GLOB _fetch_stamps "${_build}/CMakeFiles/aros-fetch/*.stamp")
list(LENGTH _fetch_stamps _fetch_stamp_count)
if(NOT _fetch_stamp_count EQUAL 1 OR
   EXISTS "${_build}/Ports/fixture/.fixture-fetched" OR
   NOT EXISTS "${_build}/reconfigured.txt")
    message(FATAL_ERROR
        "fetch completion was not isolated from its source payload\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

set(_counter_build "${_build}/counterprobe")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_counter_build}" -G Ninja
        -DTEST_OMIT_REPRESENTATION=ON
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS}
    RESULT_VARIABLE _counter_result
    OUTPUT_VARIABLE _counter_stdout
    ERROR_VARIABLE _counter_stderr)
if(_counter_result EQUAL 0 OR
   NOT _counter_stderr MATCHES "source-inventory fetch failed \\(24\\)" OR
   EXISTS "${_counter_build}/Ports/fixture/package/source.c")
    message(FATAL_ERROR
        "missing representation was not rejected at the fetch boundary\n"
        "${_counter_stdout}\n${_counter_stderr}")
endif()

file(REMOVE_RECURSE "${_build}")
message(STATUS "fetched source inventory reconfigure test passed")
