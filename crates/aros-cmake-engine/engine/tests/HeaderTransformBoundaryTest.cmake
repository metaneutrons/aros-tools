cmake_minimum_required(VERSION 3.24)

find_program(_ninja NAMES ninja ninja-build)
if(NOT _ninja)
    message(FATAL_ERROR "HeaderTransformBoundaryTest requires Ninja")
endif()

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-header-transform-boundary-${_suffix}")
get_filename_component(_root "${_root}" ABSOLUTE)
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

set(_fixture "${CMAKE_CURRENT_LIST_DIR}/header-transform-boundary")
set(_build "${_root}/positive-build")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DAROS_ENGINE_DIR=${CMAKE_CURRENT_LIST_DIR}/.."
        -DAROS_HEADER_PROBE=positive
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr
    TIMEOUT 180)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "copy-only helper fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target copy-headers
    RESULT_VARIABLE _cold_result
    OUTPUT_VARIABLE _cold_stdout
    ERROR_VARIABLE _cold_stderr
    TIMEOUT 180)
if(NOT _cold_result EQUAL 0 OR
   NOT "${_cold_stdout}\n${_cold_stderr}" MATCHES "Copying generated header")
    message(FATAL_ERROR
        "cold copy-only build failed or did not execute (${_cold_result})\n"
        "${_cold_stdout}\n${_cold_stderr}")
endif()

set(_first_input "${_build}/inputs/first.h")
set(_second_input "${_build}/inputs/second.h")
set(_first_output "${_build}/SDK/include/devices/first.h")
set(_second_output "${_build}/SDK/include/devices/second.h")
foreach(_pair
        "${_first_output}|first header, line one\nfirst header, line two\n"
        "${_second_output}|second header, line one\nsecond header, line two\n")
    string(FIND "${_pair}" "|" _separator)
    string(SUBSTRING "${_pair}" 0 ${_separator} _path)
    math(EXPR _value_start "${_separator} + 1")
    string(SUBSTRING "${_pair}" ${_value_start} -1 _expected)
    if(NOT EXISTS "${_path}" OR IS_DIRECTORY "${_path}")
        message(FATAL_ERROR "copy-only build omitted ${_path}")
    endif()
    file(READ "${_path}" _actual)
    if(NOT _actual STREQUAL _expected)
        message(FATAL_ERROR
            "copy-only content mismatch at ${_path}\n"
            "expected=[${_expected}]\nactual=[${_actual}]")
    endif()
endforeach()
foreach(_mirror
        "${_build}/GENINCDIR/devices/first.h"
        "${_build}/GENINCDIR/devices/second.h")
    if(EXISTS "${_mirror}" OR IS_SYMLINK "${_mirror}")
        message(FATAL_ERROR "copy-only rule created an unrequested mirror: ${_mirror}")
    endif()
endforeach()
file(GLOB_RECURSE _staged_headers LIST_DIRECTORIES FALSE
    "${_build}/SDK/include/*" "${_build}/GENINCDIR/*")
list(SORT _staged_headers)
set(_expected_headers "${_first_output}" "${_second_output}")
list(SORT _expected_headers)
if(NOT _staged_headers STREQUAL _expected_headers)
    message(FATAL_ERROR
        "copy-only output inventory differs\n"
        "expected: ${_expected_headers}\nactual: ${_staged_headers}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target copy-headers
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr
    TIMEOUT 180)
if(NOT _noop_result EQUAL 0 OR
   "${_noop_stdout}\n${_noop_stderr}" MATCHES "Copying generated header")
    message(FATAL_ERROR
        "unchanged copy-only build was not a no-op (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()

file(WRITE "${_first_input}" "first header, changed\n")
file(TOUCH "${_first_input}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target copy-headers
    RESULT_VARIABLE _mutation_result
    OUTPUT_VARIABLE _mutation_stdout
    ERROR_VARIABLE _mutation_stderr
    TIMEOUT 180)
if(NOT _mutation_result EQUAL 0 OR
   NOT "${_mutation_stdout}\n${_mutation_stderr}" MATCHES
       "Copying generated header .*/first\\.h")
    message(FATAL_ERROR
        "mutated input did not rebuild its copy-only output (${_mutation_result})\n"
        "${_mutation_stdout}\n${_mutation_stderr}")
endif()
file(READ "${_first_output}" _mutated_actual)
if(NOT _mutated_actual STREQUAL "first header, changed\n")
    message(FATAL_ERROR "mutated input content was not copied: ${_mutated_actual}")
endif()
file(READ "${_second_output}" _unchanged_actual)
if(NOT _unchanged_actual STREQUAL
   "second header, line one\nsecond header, line two\n")
    message(FATAL_ERROR "unmodified second header changed: ${_unchanged_actual}")
endif()

function(_expect_configure_failure probe expected)
    set(_build "${_root}/${probe}-build")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DAROS_ENGINE_DIR=${CMAKE_CURRENT_LIST_DIR}/.."
            "-DAROS_HEADER_PROBE=${probe}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 180)
    set(_output "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT _output MATCHES "${expected}")
        message(FATAL_ERROR
            "${probe} configure did not fail with /${expected}/ (${_result})\n"
            "${_output}")
    endif()
endfunction()

_expect_configure_failure(intermediate-symlink "crosses a symlink")
_expect_configure_failure(root-ancestor-symlink "crosses a symlink")
_expect_configure_failure(duplicate-same-owner "already owned by copy-headers")
_expect_configure_failure(outside-output "escapes binary directory")
_expect_configure_failure(outside-generated-root "escapes generated roots")

message(STATUS
    "copy-only boundary test passed: exact two-file output, no mirror, no-op, "
    "mutation rebuild, symlink/root escape, duplicate producer, and outside path")
file(REMOVE_RECURSE "${_root}")
