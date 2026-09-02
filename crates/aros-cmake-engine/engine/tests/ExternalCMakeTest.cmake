cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros external cmake ${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/external-cmake")

function(_configure case expect_success expected_message)
    set(_build "${_root}/${case}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
            "-DEXTERNAL_CMAKE_CASE=${case}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "external-cmake ${case} configure failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR
            "external-cmake ${case} configure unexpectedly succeeded")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        set(_log "${_stdout}\n${_stderr}")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "external-cmake ${case} missed '${expected_message}':\n${_log}")
        endif()
    endif()
endfunction()

_configure(success TRUE "")
set(_success_build "${_root}/success")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
        --target external-consumer
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "external-cmake build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()
set(_archive "${_success_build}/install/lib/libtiny.a")
set(_header "${_success_build}/install/include/tiny.h")
set(_metadata "${_success_build}/install/share/tiny/metadata.txt")
foreach(_product IN ITEMS "${_archive}" "${_header}" "${_metadata}")
    if(NOT EXISTS "${_product}" OR IS_DIRECTORY "${_product}")
        message(FATAL_ERROR "external product is missing: ${_product}")
    endif()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
        --target external-consumer
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
if(NOT _noop_result EQUAL 0)
    message(FATAL_ERROR
        "external-cmake no-op failed (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()
set(_noop_log "${_noop_stdout}\n${_noop_stderr}")
string(FIND "${_noop_log}" "ninja: no work to do." _noop_found)
if(_noop_found LESS 0)
    message(FATAL_ERROR
        "second external-cmake build was not a no-op:\n${_noop_log}")
endif()

file(REMOVE "${_archive}" "${_metadata}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
        --target fixture-external
    RESULT_VARIABLE _restore_result
    OUTPUT_VARIABLE _restore_stdout
    ERROR_VARIABLE _restore_stderr)
if(NOT _restore_result EQUAL 0 OR
   NOT EXISTS "${_archive}" OR NOT EXISTS "${_metadata}")
    message(FATAL_ERROR
        "deleted external product was not restored (${_restore_result})\n"
        "${_restore_stdout}\n${_restore_stderr}")
endif()

_configure(missing-fetch FALSE "missing fetch target")
_configure(escape-binary FALSE "binary directory must be a private child")
_configure(overlap-prefix FALSE "binary directory overlaps prefix")
_configure(escape-product FALSE "product escapes install prefix")
_configure(unsafe-option FALSE "unsafe external CMake option")
_configure(toolchain-override FALSE "overrides a forced toolchain setting")
_configure(collision FALSE "is already owned by")
_configure(binary-collision FALSE "binary directory is already owned")

_configure(missing-output TRUE "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_root}/missing-output"
        --target fixture-external
    RESULT_VARIABLE _output_result
    OUTPUT_VARIABLE _output_stdout
    ERROR_VARIABLE _output_stderr)
if(_output_result EQUAL 0)
    message(FATAL_ERROR "missing external product unexpectedly built")
endif()
set(_output_log "${_output_stdout}\n${_output_stderr}")
string(FIND "${_output_log}"
    "External CMake install did not produce its declared output" _output_found)
if(_output_found LESS 0)
    message(FATAL_ERROR
        "missing product missed diagnostic:\n${_output_log}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "external CMake test passed")
