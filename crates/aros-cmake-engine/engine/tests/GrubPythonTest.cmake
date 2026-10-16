cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
file(REAL_PATH "${_temp_root}" _temp_root_real)

set(_root "")
foreach(_attempt RANGE 1 5)
    string(RANDOM LENGTH 20 ALPHABET 0123456789abcdef _suffix)
    set(_candidate "${_temp_root}/aros-grub-python-${_suffix}")
    if(NOT EXISTS "${_candidate}" AND NOT IS_SYMLINK "${_candidate}")
        set(_root "${_candidate}")
        break()
    endif()
endforeach()
if(_root STREQUAL "")
    message(FATAL_ERROR "Could not allocate a private GRUB Python test directory")
endif()

file(MAKE_DIRECTORY "${_root}")
file(REAL_PATH "${_root}" _root_real)
get_filename_component(_root_parent "${_root_real}" DIRECTORY)
if(IS_SYMLINK "${_root}" OR NOT _root_parent STREQUAL _temp_root_real OR
   NOT _root_real MATCHES "/aros-grub-python-${_suffix}$")
    message(FATAL_ERROR "GRUB Python test directory is not an owned direct temporary child")
endif()
set(_root "${_root_real}")

function(_validate_owned_root)
    if(NOT IS_DIRECTORY "${_root}" OR IS_SYMLINK "${_root}")
        message(FATAL_ERROR "GRUB Python test directory is no longer an owned directory")
    endif()
    file(REAL_PATH "${_root}" _resolved_root)
    get_filename_component(_resolved_parent "${_resolved_root}" DIRECTORY)
    if(NOT _resolved_root STREQUAL _root_real OR
       NOT _resolved_parent STREQUAL _temp_root_real OR
       NOT _resolved_root MATCHES "/aros-grub-python-${_suffix}$")
        message(FATAL_ERROR "GRUB Python test directory no longer matches its private path")
    endif()
endfunction()

find_program(_real_python NAMES python3
    PATHS /opt/homebrew/bin /usr/local/bin /usr/bin /opt/local/bin
    NO_DEFAULT_PATH)
if(NOT _real_python)
    file(REMOVE_RECURSE "${_root}")
    message(FATAL_ERROR "GRUB Python regression tests require an installed python3")
endif()

set(_case_script "${CMAKE_CURRENT_LIST_DIR}/grub-python/Case.cmake")
set(_cases
    "preferred-with-fallback|-|success"
    "fallback-warning|GRUB: preferred Python 3.14 is not installed|success"
    "first-fallback-wins|-|success"
    "second-fallback-selected|-|success"
    "preferred-notice-quiet|GRUB: preferred Python 3.14 is not installed|quiet"
    "probe-records|-|success"
    "real-installed-python|-|success"
    "python-search-isolation|-|success"
    "preferred-wrong-minor|GRUB preferred Python path does not provide Python 3.14|failure"
    "invalid-selection|GRUB Python selection kind is invalid|failure"
    "missing-all|GRUB requires Python 3.14 or an installed compatible python3|failure"
    "nonexecutable-preferred|GRUB Python is not a regular executable|failure"
    "invalid-first-fallback|GRUB Python is not a regular executable|failure"
    "dangling-preferred|GRUB Python is not a regular executable|failure"
    "malformed-output|GRUB Python probe failed|failure"
    "old-version|GRUB Python probe failed|failure"
    "wrong-major|GRUB Python probe failed|failure"
    "probe-failure|GRUB Python probe failed|failure"
    "probe-timeout|GRUB Python probe failed|failure"
    "changed-bytes|GRUB2 runner Python selection changed or was substituted|failure"
    "forged-path|GRUB2 runner Python selection changed or was substituted|failure"
    "forged-selection|GRUB2 runner Python selection changed or was substituted|failure"
    "forged-real|GRUB2 runner Python selection changed or was substituted|failure"
    "forged-sha256|GRUB2 runner Python selection changed or was substituted|failure"
    "forged-version|GRUB2 runner Python version changed|failure")

set(_case_count 0)
set(_failure_count 0)
set(_failures "")
foreach(_spec IN LISTS _cases)
    math(EXPR _case_count "${_case_count} + 1")
    string(REPLACE "|" ";" _parts "${_spec}")
    list(GET _parts 0 _case)
    list(GET _parts 1 _expected_message)
    list(GET _parts 2 _expectation)
    if(_expected_message STREQUAL "-")
        set(_expected_message "")
    endif()
    set(_case_root "${_root}/${_case}")
    set(_command
        "${CMAKE_COMMAND}" "-DGRUB_PYTHON_CASE=${_case}"
        "-DGRUB_PYTHON_CASE_ROOT=${_case_root}"
        "-DGRUB_PYTHON_REAL=${_real_python}"
        -P "${_case_script}")
    set(_sentinel "${_case_root}/changed-executable-ran")
    if(_case STREQUAL "python-search-isolation")
        set(_poison_dir "${_case_root}/poison")
        set(_sentinel "${_case_root}/poison-module-imported")
        execute_process(
            COMMAND "${CMAKE_COMMAND}" -E env
                "PYTHONPATH=${_poison_dir}"
                "GRUB_PYTHON_TEST_SENTINEL=${_sentinel}" ${_command}
            RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    elseif(_case STREQUAL "changed-bytes")
        execute_process(
            COMMAND "${CMAKE_COMMAND}" -E env
                "GRUB_PYTHON_TEST_SENTINEL=${_sentinel}" ${_command}
            RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    else()
        execute_process(
            COMMAND ${_command}
            RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    endif()
    set(_log "${_stdout}${_stderr}")
    set(_case_failed FALSE)
    if(_expectation STREQUAL "success")
        if(NOT "${_result}" STREQUAL "0")
            set(_case_failed TRUE)
            set(_reason "expected success, got ${_result}")
        elseif(NOT "${_expected_message}" STREQUAL "")
            string(FIND "${_log}" "${_expected_message}" _message_position)
            if(_message_position LESS 0)
                set(_case_failed TRUE)
                set(_reason "expected diagnostic '${_expected_message}' was absent")
            endif()
        endif()
    elseif(_expectation STREQUAL "quiet")
        if(NOT "${_result}" STREQUAL "0")
            set(_case_failed TRUE)
            set(_reason "expected success, got ${_result}")
        else()
            string(FIND "${_log}" "${_expected_message}" _message_position)
            if(NOT _message_position LESS 0)
                set(_case_failed TRUE)
                set(_reason "preferred selection emitted the fallback notice")
            endif()
        endif()
    else()
        if("${_result}" STREQUAL "0")
            set(_case_failed TRUE)
            set(_reason "expected a nonzero result")
        else()
            string(FIND "${_log}" "${_expected_message}" _message_position)
            if(_message_position LESS 0)
                set(_case_failed TRUE)
                set(_reason "expected failure reason '${_expected_message}' was absent")
            endif()
        endif()
    endif()
    if(_case STREQUAL "changed-bytes" AND EXISTS "${_sentinel}")
        set(_case_failed TRUE)
        set(_reason "changed interpreter executed before its identity was rejected")
    endif()
    if(_case_failed)
        math(EXPR _failure_count "${_failure_count} + 1")
        if(NOT "${_reason}" STREQUAL "")
            list(APPEND _failures "${_case}: ${_reason}\n${_log}")
        else()
            list(APPEND _failures "${_case}: unexpected test failure\n${_log}")
        endif()
        message(STATUS "GRUB Python case ${_case}: FAILED")
    else()
        message(STATUS "GRUB Python case ${_case}: passed")
    endif()
endforeach()

_validate_owned_root()
file(REMOVE_RECURSE "${_root}")
if(NOT _failure_count EQUAL 0)
    string(JOIN "\n" _failure_report ${_failures})
    message(FATAL_ERROR
        "GRUB Python regression test: ${_failure_count}/${_case_count} cases failed\n${_failure_report}")
endif()
message(STATUS "GRUB Python regression tests passed (${_case_count} cases)")
