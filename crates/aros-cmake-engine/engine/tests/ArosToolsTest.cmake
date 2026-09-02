cmake_minimum_required(VERSION 3.22)

get_filename_component(_repository "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
set(_module "${_repository}/cmake/ArosTools.cmake")

if(DEFINED TEST_CASE)
    include("${_module}")
    if(TEST_CASE STREQUAL "explicit")
        set(AROS_RUST_TOOLS_DIR "${TEST_TOOLS_DIR}" CACHE PATH "" FORCE)
    elseif(NOT TEST_CASE STREQUAL "path")
        message(FATAL_ERROR "unknown test case: ${TEST_CASE}")
    endif()
    aros_configure_rust_tools()
    foreach(_name IN ITEMS genmodule transpiler collect ahi-runner fetch)
        string(TOUPPER "${_name}" _variable_suffix)
        string(REPLACE "-" "_" _variable_suffix "${_variable_suffix}")
        set(_variable "AROS_${_variable_suffix}_BIN")
        if(NOT "${${_variable}}" STREQUAL "${TEST_TOOLS_DIR}/aros-${_name}")
            message(FATAL_ERROR
                "${_variable} resolved to '${${_variable}}', expected "
                "'${TEST_TOOLS_DIR}/aros-${_name}'")
        endif()
    endforeach()
    return()
endif()

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-tools-cmake-${_suffix}")
get_filename_component(_root "${_root}" ABSOLUTE)
set(_tools "${_root}/bin")
file(MAKE_DIRECTORY "${_tools}")
foreach(_name IN ITEMS genmodule transpiler collect ahi-runner fetch verify romtool)
    set(_executable "${_tools}/aros-${_name}")
    file(WRITE "${_executable}" "#!/bin/sh\nexit 0\n")
    file(CHMOD "${_executable}" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
endforeach()

function(_run_success label)
    execute_process(
        COMMAND ${ARGN}
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "${label} failed (${_result})\n${_stdout}\n${_stderr}")
    endif()
endfunction()

_run_success("explicit suite resolution"
    "${CMAKE_COMMAND}"
    "-DTEST_CASE=explicit"
    "-DTEST_TOOLS_DIR=${_tools}"
    -P "${CMAKE_CURRENT_LIST_FILE}")

_run_success("PATH suite resolution"
    "${CMAKE_COMMAND}" -E env "PATH=${_tools}:$ENV{PATH}"
    "${CMAKE_COMMAND}"
    "-DTEST_CASE=path"
    "-DTEST_TOOLS_DIR=${_tools}"
    -P "${CMAKE_CURRENT_LIST_FILE}")

file(REMOVE "${_tools}/aros-fetch")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DTEST_CASE=explicit"
        "-DTEST_TOOLS_DIR=${_tools}"
        -P "${CMAKE_CURRENT_LIST_FILE}"
    RESULT_VARIABLE _incomplete_result
    OUTPUT_VARIABLE _incomplete_stdout
    ERROR_VARIABLE _incomplete_stderr)
set(_incomplete_log "${_incomplete_stdout}\n${_incomplete_stderr}")
if(_incomplete_result EQUAL 0 OR
   NOT _incomplete_log MATCHES "suite is incomplete" OR
   NOT _incomplete_log MATCHES "AROS_FETCH_BIN")
    message(FATAL_ERROR
        "incomplete suite did not fail with an actionable diagnostic\n"
        "${_incomplete_log}")
endif()

file(REMOVE_RECURSE "${_root}")
