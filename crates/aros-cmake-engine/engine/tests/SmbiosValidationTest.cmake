cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-smbios-validation-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/smbios-validation")
set(_build "${_root}/build")
get_filename_component(_aros_source "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${_aros_source}"
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "SMBIOS validation fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "SMBIOS validation fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

execute_process(
    COMMAND "${_build}/smbios-validation"
    RESULT_VARIABLE _run_result
    OUTPUT_VARIABLE _run_stdout
    ERROR_VARIABLE _run_stderr)
if(NOT _run_result EQUAL 0)
    message(FATAL_ERROR
        "SMBIOS validation fixture failed (${_run_result})\n"
        "${_run_stdout}\n${_run_stderr}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "SMBIOS entry-point validation test passed")
