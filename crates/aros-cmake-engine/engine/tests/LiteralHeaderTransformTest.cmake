cmake_minimum_required(VERSION 3.22)

find_program(_ninja NAMES ninja ninja-build REQUIRED)
find_program(_mkfifo NAMES mkfifo PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_base "$ENV{TMPDIR}")
else()
    set(_temp_base "/tmp")
endif()
file(REAL_PATH "${_temp_base}" _temp_base_real)
if(_temp_base_real STREQUAL "/")
    message(FATAL_ERROR "literal-header test requires a non-root temporary directory")
endif()
string(RANDOM LENGTH 20 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_base_real}/aros-literal-header-${_suffix}")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

set(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/..")
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/literal-header-transform")
set(_build "${_root}/build")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DAROS_ENGINE_DIR=${_engine_dir}"
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr
    TIMEOUT 60)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "literal-header fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target literal-header-transform
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr
    TIMEOUT 60)
if(NOT _build_result EQUAL 0 OR
   NOT "${_build_stdout}\n${_build_stderr}" MATCHES "Testing literal whole-line header transform")
    message(FATAL_ERROR
        "literal-header fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

set(_output_root "${_build}/SDK/include")
set(_expected_positive
    "#define FIRST;\n#define SECOND \${prefix}\n#define FIRST;\n#define SECOND \${prefix}\npreserve this line\n#define FIRST;\n#define SECOND \${prefix}")
file(READ "${_output_root}/nested/positive.h" _actual_positive)
if(NOT "${_actual_positive}" STREQUAL "${_expected_positive}")
    message(FATAL_ERROR
        "whole-line output lost newlines, semicolons, or literal dollar data\n"
        "expected=[${_expected_positive}]\nactual=[${_actual_positive}]")
endif()
set(_expected_no_match_hex "6e6f206d617463680d0a7365636f6e64206c696e65")
file(READ "${_output_root}/nested/no-match.h" _actual_no_match_hex HEX)
string(TOLOWER "${_actual_no_match_hex}" _actual_no_match_hex)
if(NOT "${_actual_no_match_hex}" STREQUAL "${_expected_no_match_hex}")
    message(FATAL_ERROR "sed no-match semantics changed input bytes")
endif()

set(_malformed_helper_build "${_root}/malformed-helper")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        -S "${CMAKE_CURRENT_LIST_DIR}/literal-header-transform-helper"
        -B "${_malformed_helper_build}" -G Ninja
        "-DAROS_ENGINE_DIR=${_engine_dir}"
    RESULT_VARIABLE _malformed_helper_result
    OUTPUT_VARIABLE _malformed_helper_stdout
    ERROR_VARIABLE _malformed_helper_stderr
    TIMEOUT 60)
set(_malformed_helper_output
    "${_malformed_helper_stdout}\n${_malformed_helper_stderr}")
string(FIND "${_malformed_helper_output}"
    "transformed header requires exactly one operation" _malformed_helper_message)
if(_malformed_helper_result EQUAL 0 OR _malformed_helper_message LESS 0)
    message(FATAL_ERROR
        "production helper accepted a mixed whole-line/template mode "
        "(${_malformed_helper_result})\n${_malformed_helper_output}")
endif()

set(_runner "${_engine_dir}/ReplaceHeaderLine.cmake")
set(_input_root "${_build}/inputs")
set(_replacement "${_build}/replacement.txt")
set(_sentinel "${_output_root}/sentinel.h")
function(_expect_failure label input output token replacement expected)
    execute_process(
        COMMAND "${CMAKE_COMMAND}"
            "-DINPUT=${input}"
            "-DOUTPUT=${output}"
            "-DTOKEN=${token}"
            "-DREPLACEMENT_FILE=${replacement}"
            "-DINPUT_ROOT=${_input_root}"
            "-DOUTPUT_ROOT=${_output_root}"
            "-DBINARY_ROOT=${_build}"
            -P "${_runner}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 5)
    set(_output "${_stdout}\n${_stderr}")
    string(FIND "${_output}" "${expected}" _expected_at)
    if(_result EQUAL 0 OR _expected_at LESS 0)
        message(FATAL_ERROR
            "${label} was not refused with '${expected}' (${_result})\n${_output}")
    endif()
    file(READ "${_sentinel}" _sentinel_actual)
    if(NOT "${_sentinel_actual}" STREQUAL "old output must remain intact\n")
        message(FATAL_ERROR "${label} changed an existing output before refusal")
    endif()
endfunction()

execute_process(COMMAND "${_mkfifo}" "${_input_root}/fifo.h"
    RESULT_VARIABLE _fifo_result TIMEOUT 5)
if(NOT _fifo_result EQUAL 0)
    message(FATAL_ERROR "cannot create FIFO counterprobe (${_fifo_result})")
endif()
_expect_failure("FIFO input" "${_input_root}/fifo.h" "${_sentinel}" TOKEN
    "${_replacement}" "not a regular file")

file(CREATE_LINK "${_input_root}/input.h" "${_input_root}/linked.h" SYMBOLIC)
_expect_failure("symlink input" "${_input_root}/linked.h" "${_sentinel}" TOKEN
    "${_replacement}" "crosses a symlink")

set(_linked_output_root "${_build}/linked-output-root")
file(CREATE_LINK "${_output_root}" "${_linked_output_root}" SYMBOLIC)
set(_safe_output_root "${_output_root}")
set(_output_root "${_linked_output_root}")
_expect_failure("symlink output root" "${_input_root}/input.h"
    "${_linked_output_root}/sentinel.h"
    TOKEN "${_replacement}" "crosses a symlink")
set(_output_root "${_safe_output_root}")

string(REPEAT "x" 1048577 _oversized_text)
set(_oversized_replacement "${_build}/oversized-replacement.txt")
file(WRITE "${_oversized_replacement}" "${_oversized_text}")
_expect_failure("oversized replacement" "${_input_root}/input.h" "${_sentinel}"
    TOKEN "${_oversized_replacement}" "exceeds 1 MiB")

execute_process(
    COMMAND "${CMAKE_COMMAND}" "-DINPUT=${_input_root}/input.h"
        "-DOUTPUT=${_sentinel}" -P "${_runner}"
    RESULT_VARIABLE _malformed_result
    OUTPUT_VARIABLE _malformed_stdout
    ERROR_VARIABLE _malformed_stderr
    TIMEOUT 5)
if(_malformed_result EQUAL 0 OR
   NOT "${_malformed_stderr}" MATCHES "requires INPUT, OUTPUT, TOKEN")
    message(FATAL_ERROR "malformed runner arguments were accepted")
endif()
file(READ "${_sentinel}" _sentinel_actual)
if(NOT "${_sentinel_actual}" STREQUAL "old output must remain intact\n")
    message(FATAL_ERROR "malformed invocation changed the existing output")
endif()

file(GLOB _temporary_outputs "${_output_root}/nested/*.tmp")
if(_temporary_outputs)
    message(FATAL_ERROR "runner left temporary outputs: ${_temporary_outputs}")
endif()

message(STATUS
    "literal whole-line transform passed: custom-command output, byte-preserving no-match, "
    "multiline/semicolon/literal-dollar replacement, FIFO/symlink/size refusals, "
    "and unchanged prior output on failure")
file(REMOVE_RECURSE "${_root}")
