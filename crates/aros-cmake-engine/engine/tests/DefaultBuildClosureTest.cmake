cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-default-build-closure-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/default-build-closure")

function(_configure_case case_name should_succeed expected_message)
    set(_case_build "${_root}/${case_name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_case_build}" -G Ninja
            "-DCLOSURE_CASE=${case_name}"
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
            ${AROS_TEST_TOOL_ARGS}
        RESULT_VARIABLE _configure_result
        OUTPUT_VARIABLE _configure_stdout
        ERROR_VARIABLE _configure_stderr)
    set(_configure_log "${_configure_stdout}\n${_configure_stderr}")
    if(should_succeed)
        if(NOT _configure_result EQUAL 0)
            message(FATAL_ERROR
                "${case_name}: configure failed (${_configure_result})\n${_configure_log}")
        endif()
    else()
        if(_configure_result EQUAL 0)
            message(FATAL_ERROR "${case_name}: configure unexpectedly succeeded")
        endif()
        string(FIND "${_configure_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "${case_name}: expected '${expected_message}'\n${_configure_log}")
        endif()
        message(STATUS "${case_name}: rejected (${expected_message})")
    endif()
    set(_DEFAULT_CLOSURE_BUILD "${_case_build}" PARENT_SCOPE)
endfunction()

function(_build_selected_all build_dir label)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${build_dir}"
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    if(NOT _build_result EQUAL 0)
        message(FATAL_ERROR
            "${label}: implicit all build failed (${_build_result})\n"
            "${_build_stdout}\n${_build_stderr}")
    endif()
    set(_archive "${build_dir}/libselected-sdk-member.a")
    if(NOT EXISTS "${_archive}" OR IS_DIRECTORY "${_archive}" OR IS_SYMLINK "${_archive}")
        message(FATAL_ERROR "${label}: implicit all omitted regular selected archive ${_archive}")
    endif()
endfunction()

_configure_case(classic TRUE "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_DEFAULT_CLOSURE_BUILD}"
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "default build closure fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()
if(EXISTS "${_DEFAULT_CLOSURE_BUILD}/libmanual-member.a")
    message(FATAL_ERROR "unqualified build compiled the manual target")
endif()
if(NOT EXISTS "${_DEFAULT_CLOSURE_BUILD}/libdefault-member.a" OR
   NOT EXISTS "${_DEFAULT_CLOSURE_BUILD}/liblinked-member.a")
    message(FATAL_ERROR
        "unqualified build did not compile the reachable target and its link dependency")
endif()

_configure_case(native-build-no-root TRUE "")
_build_selected_all("${_DEFAULT_CLOSURE_BUILD}" native-build-no-root)
_configure_case(native-consumer-no-root TRUE "")
_build_selected_all("${_DEFAULT_CLOSURE_BUILD}" native-consumer-no-root)
_configure_case(native-build-with-root TRUE "")
_configure_case(native-consumer-with-root TRUE "")
_configure_case(classic-missing-root FALSE
    "MetaMake default root is not a translated target: AROS")
_configure_case(native-unvalidated FALSE
    "MetaMake default root is not a translated target: AROS")
_configure_case(helper-false FALSE
    "MetaMake default root is not a translated target: AROS")
_configure_case(native-missing-helper FALSE
    "MetaMake default closure: native source selection validator is required")

file(REMOVE_RECURSE "${_root}")
message(STATUS "default MetaMake build closure test passed (9 configure cases, 3 builds)")
