cmake_minimum_required(VERSION 3.22)
if(NOT ENGINE_DIR OR NOT TEST_BINARY_DIR)
    message(FATAL_ERROR "ENGINE_DIR and a fresh TEST_BINARY_DIR are required")
endif()

set(_fixture "${ENGINE_DIR}/tests/module-headers-only")
set(_build_root "${TEST_BINARY_DIR}/module-headers-only")
file(MAKE_DIRECTORY "${_build_root}")

function(_run_configure case expected)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build_root}/${case}"
            -G Ninja
            "-DENGINE_DIR=${ENGINE_DIR}"
            "-DTEST_CASE=${case}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 60)
    if(case STREQUAL "valid")
        if(NOT _result EQUAL 0)
            message(FATAL_ERROR
                "valid headers-only configure failed (${_result})\n${_stdout}\n${_stderr}")
        endif()
        execute_process(
            COMMAND "${CMAKE_COMMAND}" --build "${_build_root}/${case}"
            --target probe-mmake-includes probe-mmake-fd includes-Probe
                includes-Probe_rel includes-all aros-genmodule-public-includes
                probe-nofd-includes includes-ProbeNoFd includes-ProbeNoFd_rel
                --parallel 2
            RESULT_VARIABLE _build_result
            OUTPUT_VARIABLE _build_stdout
            ERROR_VARIABLE _build_stderr
            TIMEOUT 60)
        if(NOT _build_result EQUAL 0)
            message(FATAL_ERROR
                "headers-only output targets failed to build (${_build_result})\n"
                "${_build_stdout}\n${_build_stderr}")
        endif()
        foreach(_output probe-mmake/includes.stamp probe-mmake/fd.stamp
                probe-nofd/includes.stamp)
            if(NOT EXISTS "${_build_root}/${case}/fake-genmodule/${_output}")
                message(FATAL_ERROR "headers-only producer did not create ${_output}")
            endif()
        endforeach()
        if(EXISTS "${_build_root}/${case}/fake-genmodule/probe-nofd/fd.stamp")
            message(FATAL_ERROR "no-FD module incorrectly produced an FD output")
        endif()
    elseif(_result EQUAL 0 OR NOT "${_stdout}${_stderr}" MATCHES "${expected}")
        message(FATAL_ERROR
            "${case}: expected configure refusal matching '${expected}', got "
            "${_result}\n${_stdout}\n${_stderr}")
    endif()
endfunction()

_run_configure(valid "")
_run_configure(missing-config "missing genmodule config")
_run_configure(missing-required "MODTYPE is required")
_run_configure(runtime-input "unknown or unsupported arguments")

message(STATUS
    "Headers-only modules publish real includes/FD aliases without runtime or archive owners")
