cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros runtime runner ${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/runtime-runner")
set(_runner_source "${CMAKE_CURRENT_LIST_DIR}/runtime/llvmpipe-jit-runner.c")
set(_build "${_root}/build")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}"
        "-DRUNNER_SOURCE=${_runner_source}"
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "runtime-runner fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "runtime-runner fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

file(READ "${_build}/runner.path" _runner)
string(STRIP "${_runner}" _runner)
if(NOT EXISTS "${_runner}")
    message(FATAL_ERROR "runtime-runner fixture did not produce ${_runner}")
endif()

function(_run_case case_name expected_status expected_output)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -E env
            "RUNNER_TEST_CASE=${case_name}"
            "RUNNER_TEST_RESULT=${_result_value}"
            "${_runner}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL expected_status)
        message(FATAL_ERROR
            "runtime-runner ${case_name} exited ${_result}, expected "
            "${expected_status}\n${_stdout}\n${_stderr}")
    endif()
    if(NOT "${_stdout}" STREQUAL "${expected_output}")
        message(FATAL_ERROR
            "runtime-runner ${case_name} event/output contract mismatch\n"
            "expected:\n${expected_output}\nactual:\n${_stdout}\n${_stderr}")
    endif()
endfunction()

string(CONCAT _load_failure_expected
    "LOAD path=SYS:Developer/Debug/Tests/graphics/gl/llvmpipe-jit\n"
    "BUG:[llvmpipe-jit] FAIL: supervisor could not load the probe\n")
set(_result_value 0)
_run_case(load-failure 20 "${_load_failure_expected}")

string(CONCAT _success_expected
    "LOAD path=SYS:Developer/Debug/Tests/graphics/gl/llvmpipe-jit\n"
    "RUN segment=4660 stack=1048576 args=0a00 length=1\n"
    "UNLOAD segment=4660\n"
    "BUG:=== LLVMPipe LLVM 11 GLSL/JIT PROBE EXIT PASS ===\n")
set(_result_value 0)
_run_case(zero-result 0 "${_success_expected}")

string(CONCAT _negative_expected
    "LOAD path=SYS:Developer/Debug/Tests/graphics/gl/llvmpipe-jit\n"
    "RUN segment=4660 stack=1048576 args=0a00 length=1\n"
    "UNLOAD segment=4660\n"
    "BUG:[llvmpipe-jit] FAIL: probe returned -7\n")
set(_result_value -7)
_run_case(negative-result 20 "${_negative_expected}")

string(CONCAT _nonzero_expected
    "LOAD path=SYS:Developer/Debug/Tests/graphics/gl/llvmpipe-jit\n"
    "RUN segment=4660 stack=1048576 args=0a00 length=1\n"
    "UNLOAD segment=4660\n"
    "BUG:[llvmpipe-jit] FAIL: probe returned 7\n")
set(_result_value 7)
_run_case(nonzero-result 20 "${_nonzero_expected}")

file(REMOVE_RECURSE "${_root}")
message(STATUS "runtime runner host contract tests passed")
