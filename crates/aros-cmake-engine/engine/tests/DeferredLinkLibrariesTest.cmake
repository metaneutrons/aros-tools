cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-deferred-link-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/deferred-link-libraries")
foreach(_link_mode IN ITEMS direct-lld compiler-driver)
    set(_build "${_root}/${_link_mode}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}"
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
            ${AROS_TEST_TOOL_ARGS} -G Ninja
            "-DDEFERRED_LINK_MODE=${_link_mode}"
        RESULT_VARIABLE _configure_result
        OUTPUT_VARIABLE _configure_stdout
        ERROR_VARIABLE _configure_stderr)
    if(NOT _configure_result EQUAL 0)
        message(FATAL_ERROR
            "deferred link ${_link_mode} fixture configure failed "
            "(${_configure_result})\n${_configure_stdout}\n${_configure_stderr}")
    endif()
endforeach()

file(REMOVE_RECURSE "${_root}")
message(STATUS "deferred target-link binding test passed")
