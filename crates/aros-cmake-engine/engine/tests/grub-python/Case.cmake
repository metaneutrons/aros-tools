cmake_minimum_required(VERSION 3.22)

foreach(_required IN ITEMS GRUB_PYTHON_CASE GRUB_PYTHON_CASE_ROOT GRUB_PYTHON_REAL)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "Missing required case input ${_required}")
    endif()
endforeach()

include("${CMAKE_CURRENT_LIST_DIR}/../../GrubPython.cmake")
file(MAKE_DIRECTORY "${GRUB_PYTHON_CASE_ROOT}")

function(_write_fake_python path mode output)
    if(mode STREQUAL "valid")
        set(_body "printf '%s\\n' '${output}'")
    elseif(mode STREQUAL "failure")
        set(_body "printf '%s\\n' '${output}'\nexit 7")
    elseif(mode STREQUAL "timeout")
        execute_process(COMMAND /bin/test -f /bin/sleep RESULT_VARIABLE _clock_regular)
        execute_process(COMMAND /bin/test -x /bin/sleep RESULT_VARIABLE _clock_executable)
        if(NOT "${_clock_regular}" STREQUAL "0" OR
           NOT "${_clock_executable}" STREQUAL "0")
            message(FATAL_ERROR "GRUB Python timeout fixture requires regular executable /bin/sleep")
        endif()
        # Without a working timeout this fake must succeed, not fail for empty output.
        set(_body "/bin/sleep 15\nprintf '%s\\n' '${output}'")
    else()
        set(_body "printf '%s\\n' '${output}'")
    endif()

    set(_script [=[#!/bin/sh
set -eu
if [ "$#" -ne 4 ] || [ "$1" != "-I" ] || [ "$2" != "-S" ] || [ "$3" != "-c" ]; then
    exit 91
fi
case "$4" in
    *"import codecs, optparse, re, sys"*"sys.version_info.major == 3"*"sys.version_info >= (3, 11)"*"sys.version_info.releaselevel == 'final'"*"print('%d.%d.%d' % sys.version_info[:3])"*) ;;
    *) exit 92 ;;
esac
@BODY@
]=])
    string(REPLACE "@BODY@" "${_body}" _script "${_script}")
    file(WRITE "${path}" "${_script}")
    if(mode STREQUAL "nonexecutable")
        file(CHMOD "${path}" PERMISSIONS
            OWNER_READ OWNER_WRITE GROUP_READ WORLD_READ)
    else()
        file(CHMOD "${path}" PERMISSIONS
            OWNER_READ OWNER_WRITE OWNER_EXECUTE
            GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
    endif()
endfunction()

function(_write_valid_pair preferred fallback)
    _write_fake_python("${preferred}" valid "3.14.2")
    _write_fake_python("${fallback}" valid "3.12.9")
endfunction()

function(_assert_record path version real sha)
    if(NOT version STREQUAL "3.14.2")
        message(FATAL_ERROR "Fake Python probe recorded the wrong version: ${version}")
    endif()
    file(REAL_PATH "${path}" _expected_real)
    file(SHA256 "${_expected_real}" _expected_sha)
    if(NOT real STREQUAL _expected_real OR NOT sha STREQUAL _expected_sha)
        message(FATAL_ERROR
            "Python probe did not record its resolved executable and SHA-256 identity")
    endif()
endfunction()

set(_preferred "${GRUB_PYTHON_CASE_ROOT}/preferred-python")
set(_fallback "${GRUB_PYTHON_CASE_ROOT}/fallback-python")
set(_fallback_second "${GRUB_PYTHON_CASE_ROOT}/fallback-second-python")

if(GRUB_PYTHON_CASE STREQUAL "preferred-with-fallback")
    _write_valid_pair("${_preferred}" "${_fallback}")
    _aros_grub_python_choose("${_preferred}" "${_fallback}" _selected _selection)
    if(NOT _selected STREQUAL _preferred OR NOT _selection STREQUAL "preferred")
        message(FATAL_ERROR "An installed preferred interpreter was not selected")
    endif()
    _aros_grub_python_notice("${_selected}" "3.14.2" "${_selection}")

elseif(GRUB_PYTHON_CASE STREQUAL "fallback-warning")
    _write_fake_python("${_fallback}" valid "3.12.9")
    _aros_grub_python_choose("${_preferred}" "${_fallback}" _selected _selection)
    if(NOT _selected STREQUAL _fallback OR NOT _selection STREQUAL "fallback")
        message(FATAL_ERROR "An absent preferred interpreter did not select the fallback")
    endif()
    _aros_grub_python_notice("${_selected}" "3.12.9" "${_selection}")

elseif(GRUB_PYTHON_CASE STREQUAL "first-fallback-wins")
    _write_fake_python("${_fallback}" valid "3.12.9")
    _write_fake_python("${_fallback_second}" valid "3.13.7")
    set(_fallbacks "${_fallback};${_fallback_second}")
    _aros_grub_python_choose("${_preferred}" "${_fallbacks}" _selected _selection)
    if(NOT _selected STREQUAL _fallback OR NOT _selection STREQUAL "fallback")
        message(FATAL_ERROR "The first installed fallback was not selected")
    endif()

elseif(GRUB_PYTHON_CASE STREQUAL "second-fallback-selected")
    _write_fake_python("${_fallback_second}" valid "3.13.7")
    set(_fallbacks "${_fallback};${_fallback_second}")
    _aros_grub_python_choose("${_preferred}" "${_fallbacks}" _selected _selection)
    if(NOT _selected STREQUAL _fallback_second OR NOT _selection STREQUAL "fallback")
        message(FATAL_ERROR "The second fallback was not selected after the first was absent")
    endif()

elseif(GRUB_PYTHON_CASE STREQUAL "preferred-notice-quiet")
    _write_valid_pair("${_preferred}" "${_fallback}")
    _aros_grub_python_choose("${_preferred}" "${_fallback}" _selected _selection)
    _aros_grub_python_notice("${_selected}" "3.14.2" "${_selection}")

elseif(GRUB_PYTHON_CASE STREQUAL "probe-records")
    _write_fake_python("${_preferred}" valid "3.14.2")
    _aros_grub_python_probe("${_preferred}" _version _real _sha)
    _assert_record("${_preferred}" "${_version}" "${_real}" "${_sha}")

elseif(GRUB_PYTHON_CASE STREQUAL "real-installed-python")
    _aros_grub_python_probe("${GRUB_PYTHON_REAL}" _version _real _sha)
    file(REAL_PATH "${GRUB_PYTHON_REAL}" _expected_real)
    file(SHA256 "${_expected_real}" _expected_sha)
    if(NOT _version MATCHES "^3\\.[0-9]+\\.[0-9]+$" OR
       _version VERSION_LESS "3.11" OR
       NOT _real STREQUAL _expected_real OR NOT _sha STREQUAL _expected_sha)
        message(FATAL_ERROR "Installed Python probe did not record a compatible identity")
    endif()
    _aros_grub_python_revalidate("${GRUB_PYTHON_REAL}" "fallback"
        "${GRUB_PYTHON_REAL}" "fallback" "${_real}" "${_version}" "${_sha}")

elseif(GRUB_PYTHON_CASE STREQUAL "python-search-isolation")
    if(NOT DEFINED ENV{GRUB_PYTHON_TEST_SENTINEL} OR
       "$ENV{GRUB_PYTHON_TEST_SENTINEL}" STREQUAL "")
        message(FATAL_ERROR "Python isolation case has no private sentinel path")
    endif()
    set(_sentinel "$ENV{GRUB_PYTHON_TEST_SENTINEL}")
    set(_poison_dir "${GRUB_PYTHON_CASE_ROOT}/poison")
    file(MAKE_DIRECTORY "${_poison_dir}")
    set(_poison_source [=[import os

with open(os.environ["GRUB_PYTHON_TEST_SENTINEL"], "w", encoding="utf-8") as marker:
    marker.write("poison re.py imported\n")
print("poison re.py imported")
]=])
    file(WRITE "${_poison_dir}/re.py" "${_poison_source}")

    # -S suppresses startup customization while keeping PYTHONPATH active;
    # this counterproof measures ordinary module search, not isolated startup.
    execute_process(
        COMMAND "${GRUB_PYTHON_REAL}" -S -c
            "import re; print('counterproof: ' + re.__file__)"
        RESULT_VARIABLE _counterproof_result
        OUTPUT_VARIABLE _counterproof_stdout
        ERROR_VARIABLE _counterproof_stderr)
    set(_counterproof_log "${_counterproof_stdout}${_counterproof_stderr}")
    string(FIND "${_counterproof_log}" "poison re.py imported" _poison_message)
    string(FIND "${_counterproof_log}" "${_poison_dir}/re.py" _poison_path)
    if(EXISTS "${_sentinel}")
        set(_sentinel_state present)
    else()
        set(_sentinel_state absent)
    endif()
    if(NOT "${_counterproof_result}" STREQUAL "0" OR
       NOT EXISTS "${_sentinel}" OR _poison_message LESS 0 OR _poison_path LESS 0)
        message(FATAL_ERROR
            "Normal Python import did not visibly load the private poison re.py "
            "(result=${_counterproof_result}, sentinel=${_sentinel_state}, "
            "message-index=${_poison_message}, path-index=${_poison_path}; "
            "sentinel=${_sentinel})\n"
            "${_counterproof_log}")
    endif()

    file(REMOVE "${_sentinel}")
    if(EXISTS "${_sentinel}")
        message(FATAL_ERROR "Could not remove the private Python poison sentinel")
    endif()
    _aros_grub_python_probe("${GRUB_PYTHON_REAL}" _version _real _sha)
    if(EXISTS "${_sentinel}")
        message(FATAL_ERROR "Python -I -S probe imported the PYTHONPATH poison re.py")
    endif()

elseif(GRUB_PYTHON_CASE STREQUAL "preferred-wrong-minor")
    _write_fake_python("${_preferred}" valid "3.15.0")
    _aros_grub_python_probe("${_preferred}" _version _real _sha)
    _aros_grub_python_revalidate("${_preferred}" "preferred"
        "${_preferred}" "preferred" "${_real}" "${_version}" "${_sha}")

elseif(GRUB_PYTHON_CASE STREQUAL "invalid-selection")
    _write_fake_python("${_preferred}" valid "3.14.2")
    _aros_grub_python_probe("${_preferred}" _version _real _sha)
    _aros_grub_python_revalidate("${_preferred}" "invalid"
        "${_preferred}" "invalid" "${_real}" "${_version}" "${_sha}")

elseif(GRUB_PYTHON_CASE STREQUAL "missing-all")
    _aros_grub_python_choose("${_preferred}" "${_fallback}" _selected _selection)

elseif(GRUB_PYTHON_CASE STREQUAL "nonexecutable-preferred")
    _write_fake_python("${_preferred}" nonexecutable "3.14.2")
    _write_fake_python("${_fallback}" valid "3.12.9")
    _aros_grub_python_choose("${_preferred}" "${_fallback}" _selected _selection)

elseif(GRUB_PYTHON_CASE STREQUAL "invalid-first-fallback")
    _write_fake_python("${_fallback}" nonexecutable "3.12.9")
    _write_fake_python("${_fallback_second}" valid "3.13.7")
    set(_fallbacks "${_fallback};${_fallback_second}")
    _aros_grub_python_choose("${_preferred}" "${_fallbacks}" _selected _selection)

elseif(GRUB_PYTHON_CASE STREQUAL "dangling-preferred")
    _write_fake_python("${_fallback}" valid "3.12.9")
    file(CREATE_LINK "${GRUB_PYTHON_CASE_ROOT}/missing-target"
        "${_preferred}" RESULT _link_result SYMBOLIC)
    if(NOT _link_result STREQUAL "0")
        message(FATAL_ERROR "Could not create a private dangling candidate: ${_link_result}")
    endif()
    _aros_grub_python_choose("${_preferred}" "${_fallback}" _selected _selection)

elseif(GRUB_PYTHON_CASE MATCHES "^(malformed-output|old-version|wrong-major|probe-failure|probe-timeout)$")
    _write_fake_python("${_fallback}" valid "3.12.9")
    if(GRUB_PYTHON_CASE STREQUAL "malformed-output")
        _write_fake_python("${_preferred}" valid "Python 3.14.2")
    elseif(GRUB_PYTHON_CASE STREQUAL "old-version")
        _write_fake_python("${_preferred}" valid "3.10.15")
    elseif(GRUB_PYTHON_CASE STREQUAL "wrong-major")
        _write_fake_python("${_preferred}" valid "2.7.18")
    elseif(GRUB_PYTHON_CASE STREQUAL "probe-failure")
        _write_fake_python("${_preferred}" failure "3.14.2")
    else()
        _write_fake_python("${_preferred}" timeout "3.14.2")
    endif()
    _aros_grub_python_choose("${_preferred}" "${_fallback}" _selected _selection)
    if(NOT _selected STREQUAL _preferred OR NOT _selection STREQUAL "preferred")
        message(FATAL_ERROR "The invalid preferred interpreter was skipped for its fallback")
    endif()
    _aros_grub_python_probe("${_selected}" _version _real _sha)

elseif(GRUB_PYTHON_CASE STREQUAL "changed-bytes")
    _write_fake_python("${_preferred}" valid "3.14.2")
    _aros_grub_python_probe("${_preferred}" _version _real _sha)
    set(_replacement [=[#!/bin/sh
: > "$GRUB_PYTHON_TEST_SENTINEL"
printf '%s\n' '3.14.2'
]=])
    file(WRITE "${_preferred}" "${_replacement}")
    file(CHMOD "${_preferred}" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE)
    _aros_grub_python_revalidate("${_preferred}" "preferred"
        "${_preferred}" "preferred" "${_real}" "${_version}" "${_sha}")

elseif(GRUB_PYTHON_CASE MATCHES "^forged-(path|selection|real|sha256|version)$")
    _write_fake_python("${_preferred}" valid "3.14.2")
    _aros_grub_python_probe("${_preferred}" _version _real _sha)
    set(_recorded_path "${_preferred}")
    set(_recorded_selection "preferred")
    set(_recorded_real "${_real}")
    set(_recorded_version "${_version}")
    set(_recorded_sha "${_sha}")
    if(GRUB_PYTHON_CASE STREQUAL "forged-path")
        set(_recorded_path "${GRUB_PYTHON_CASE_ROOT}/other-python")
    elseif(GRUB_PYTHON_CASE STREQUAL "forged-selection")
        set(_recorded_selection "fallback")
    elseif(GRUB_PYTHON_CASE STREQUAL "forged-real")
        set(_recorded_real "${GRUB_PYTHON_CASE_ROOT}/other-python")
    elseif(GRUB_PYTHON_CASE STREQUAL "forged-sha256")
        set(_recorded_sha "0000000000000000000000000000000000000000000000000000000000000000")
    else()
        set(_recorded_version "3.15.0")
    endif()
    _aros_grub_python_revalidate("${_preferred}" "preferred"
        "${_recorded_path}" "${_recorded_selection}" "${_recorded_real}"
        "${_recorded_version}" "${_recorded_sha}")

else()
    message(FATAL_ERROR "Unknown GRUB Python regression case ${GRUB_PYTHON_CASE}")
endif()
