cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")
find_program(_clang NAMES clang REQUIRED)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_build "${_temp_root}/aros-deferred-header-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/deferred-header-partial-tree")

foreach(_pass RANGE 1 2)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
            "-DCMAKE_C_COMPILER=${_clang}"
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
            ${AROS_TEST_TOOL_ARGS}
        RESULT_VARIABLE _configure_result
        OUTPUT_VARIABLE _configure_stdout
        ERROR_VARIABLE _configure_stderr)
    if(NOT _configure_result EQUAL 0)
        message(FATAL_ERROR
            "deferred-header configure failed on pass ${_pass}\n"
            "${_configure_stdout}\n${_configure_stderr}")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target consumer
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    if(NOT _build_result EQUAL 0)
        message(FATAL_ERROR
            "deferred-header build failed on pass ${_pass}\n"
            "${_build_stdout}\n${_build_stderr}")
    endif()
endforeach()

file(READ "${_build}/SDK/include/zlib.h" _staged)
if(NOT _staged STREQUAL "#define FIXTURE_HEADER_VALUE 42\n")
    message(FATAL_ERROR "staged header does not match the completed fetch")
endif()
file(REMOVE_RECURSE "${_build}")
message(STATUS "partial fetched-header staging test passed")
