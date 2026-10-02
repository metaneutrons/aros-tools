cmake_minimum_required(VERSION 3.22)
find_program(_clang NAMES clang REQUIRED)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "$ENV{TMPDIR}")
if(NOT _root)
    set(_root /tmp)
endif()
set(_root "${_root}/aros arch quote ${_suffix}")
foreach(_case IN ITEMS valid target-first conflict)
    set(_build "${_root}/${_case}")
    execute_process(COMMAND "${CMAKE_COMMAND}"
        -S "${CMAKE_CURRENT_LIST_DIR}/arch-quote-includes" -B "${_build}"
        -G Ninja "-DQUOTE_TEST_CASE=${_case}"
        "-DCMAKE_C_COMPILER=${_clang}" "-DCMAKE_ASM_COMPILER=${_clang}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr
        TIMEOUT 60)
    if(_case STREQUAL conflict)
        if(_result EQUAL 0 OR NOT "${_stdout}${_stderr}" MATCHES
                "Conflicting architecture quote paths")
            message(FATAL_ERROR "conflicting source quote paths were not rejected: ${_stdout}${_stderr}")
        endif()
        continue()
    endif()
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${_case} configure failed: ${_stdout}${_stderr}")
    endif()
    execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr
        TIMEOUT 60)
    if(_case STREQUAL valid)
        if(NOT _result EQUAL 0)
            message(FATAL_ERROR "quote precedence/isolation build failed: ${_stdout}${_stderr}")
        endif()
    elseif(_result EQUAL 0 OR NOT "${_stdout}${_stderr}" MATCHES
            "wrong explicit quote include")
        message(FATAL_ERROR "target-first counterprobe did not fail as expected: ${_stdout}${_stderr}")
    endif()
endforeach()
file(REMOVE_RECURSE "${_root}")
message(STATUS "architecture quote precedence, angle isolation, shared targets and counterprobes passed")
