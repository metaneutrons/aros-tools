cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros runtime probes ${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/runtime-probes")
get_filename_component(_module
    "${CMAKE_CURRENT_LIST_DIR}/../RuntimeProbes.cmake" ABSOLUTE)

function(_configure case expect_success expected_message)
    set(_build "${_root}/${case}")
    if(case STREQUAL "invalid-env")
        set(_environment_command "${CMAKE_COMMAND}" -E env
            AROS_LLVMPIPE_RUNTIME_PROBE=maybe)
    elseif(case STREQUAL "reset-off")
        set(_environment_command "${CMAKE_COMMAND}" -E env
            AROS_LLVMPIPE_RUNTIME_PROBE=0)
    else()
        set(_environment_command "${CMAKE_COMMAND}" -E env
            --unset=AROS_LLVMPIPE_RUNTIME_PROBE)
    endif()
    execute_process(
        COMMAND ${_environment_command} "${CMAKE_COMMAND}"
            -S "${_source}" -B "${_build}" -G Ninja
            "-DRUNTIME_PROBES_MODULE=${_module}"
            "-DRUNTIME_PROBE_CASE=${case}"
            "-DAROS_LLVMPIPE_RUNTIME_PROBE=ON"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "runtime-probes ${case} configure failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR
            "runtime-probes ${case} configure unexpectedly succeeded")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        set(_log "${_stdout}\n${_stderr}")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "runtime-probes ${case} missed '${expected_message}':\n${_log}")
        endif()
    endif()
endfunction()

# The option's default must remain OFF even when all qualification inputs are
# absent. This configure intentionally does not pass the cache option.
set(_default_build "${_root}/default-off")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env
        --unset=AROS_LLVMPIPE_RUNTIME_PROBE "${CMAKE_COMMAND}"
        -S "${_source}" -B "${_default_build}" -G Ninja
        "-DRUNTIME_PROBES_MODULE=${_module}"
        -DRUNTIME_PROBE_CASE=default-off
    RESULT_VARIABLE _default_result
    OUTPUT_VARIABLE _default_stdout
    ERROR_VARIABLE _default_stderr)
if(NOT _default_result EQUAL 0)
    message(FATAL_ERROR
        "runtime-probes default-off configure failed (${_default_result})\n"
        "${_default_stdout}\n${_default_stderr}")
endif()

_configure(enabled TRUE "")
_configure(reset-off TRUE "")
foreach(_case IN ITEMS wrong-cpu wrong-platform wrong-mesa
        missing-hidd missing-gl-linklib missing-gl-provider missing-llvm-producer missing-lld)
    _configure("${_case}" FALSE
        "llvmpipe runtime probe requires the native PC LLVM11/Mesa26 producers")
endforeach()
_configure(invalid-env FALSE
    "AROS_LLVMPIPE_RUNTIME_PROBE must be 0 or 1")

file(REMOVE_RECURSE "${_root}")
message(STATUS "runtime probe opt-in and wiring tests passed")
