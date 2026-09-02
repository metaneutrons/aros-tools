cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-private-linklib-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/private-linklib-output")
find_program(_llvm_ar NAMES llvm-ar
    HINTS "/opt/homebrew/opt/llvm/bin" "/usr/local/opt/llvm/bin"
    REQUIRED)
find_program(_llvm_ranlib NAMES llvm-ranlib
    HINTS "/opt/homebrew/opt/llvm/bin" "/usr/local/opt/llvm/bin"
    REQUIRED)

function(_configure case expect_success expected_message)
    set(_build "${_root}/${case}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
            "-DPRIVATE_LINKLIB_CASE=${case}"
            "-DCMAKE_AR=${_llvm_ar}"
            "-DCMAKE_RANLIB=${_llvm_ranlib}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "private-linklib ${case} configure failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR
            "private-linklib ${case} configure unexpectedly succeeded")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        set(_log "${_stdout}\n${_stderr}")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "private-linklib ${case} missed diagnostic '${expected_message}':\n${_log}")
        endif()
    endif()
endfunction()

_configure(success TRUE "")

set(_success_build "${_root}/success")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
        --target first-provider empty-consumer
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "private-linklib build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()
set(_archive "${_success_build}/private/mesa20.0.8/libgallium_i915.a")
if(NOT EXISTS "${_archive}" OR IS_DIRECTORY "${_archive}")
    message(FATAL_ERROR "private archive was not created at ${_archive}")
endif()
set(_empty_archive "${_success_build}/private/mesa20.0.8/libmesa-sse41.a")
if(NOT EXISTS "${_empty_archive}" OR IS_DIRECTORY "${_empty_archive}")
    message(FATAL_ERROR "empty private archive was not created at ${_empty_archive}")
endif()
execute_process(
    COMMAND "${_llvm_ar}" t "${_empty_archive}"
    RESULT_VARIABLE _archive_list_result
    OUTPUT_VARIABLE _archive_members
    ERROR_VARIABLE _archive_list_stderr)
if(NOT _archive_list_result EQUAL 0 OR NOT _archive_members STREQUAL "")
    message(FATAL_ERROR
        "EMPTY_ARCHIVE is not a valid zero-member archive:\n"
        "${_archive_members}\n${_archive_list_stderr}")
endif()
file(SHA256 "${_empty_archive}" _empty_archive_sha256)

# A missing empty archive is a tracked product and must be repaired through a
# direct consumer build, including its link dependency.
file(REMOVE "${_empty_archive}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
        --target empty-consumer
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr)
if(NOT _repair_result EQUAL 0 OR NOT EXISTS "${_empty_archive}")
    message(FATAL_ERROR
        "empty private archive repair failed (${_repair_result})\n"
        "${_repair_stdout}\n${_repair_stderr}")
endif()
execute_process(
    COMMAND "${_llvm_ar}" t "${_empty_archive}"
    RESULT_VARIABLE _repaired_list_result
    OUTPUT_VARIABLE _repaired_members
    ERROR_VARIABLE _repaired_list_stderr)
file(SHA256 "${_empty_archive}" _repaired_sha256)
if(NOT _repaired_list_result EQUAL 0 OR NOT _repaired_members STREQUAL "" OR
   NOT "${_repaired_sha256}" STREQUAL "${_empty_archive_sha256}")
    message(FATAL_ERROR
        "repaired EMPTY_ARCHIVE differs from the original zero-member archive:\n"
        "${_repaired_members}\n${_repaired_list_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
        --target first-provider empty-consumer
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
if(NOT _noop_result EQUAL 0)
    message(FATAL_ERROR
        "private-linklib no-op build failed (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()
set(_noop_log "${_noop_stdout}\n${_noop_stderr}")
string(FIND "${_noop_log}" "ninja: no work to do." _noop_found)
if(_noop_found LESS 0)
    message(FATAL_ERROR "second private-linklib build was not a no-op:\n${_noop_log}")
endif()

# The generated header-only anchor must remain stable across an explicit
# reconfigure as well as an ordinary second build.
_configure(success TRUE "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
        --target empty-consumer
    RESULT_VARIABLE _reconfigure_noop_result
    OUTPUT_VARIABLE _reconfigure_noop_stdout
    ERROR_VARIABLE _reconfigure_noop_stderr)
if(NOT _reconfigure_noop_result EQUAL 0)
    message(FATAL_ERROR
        "empty private archive build after reconfigure failed (${_reconfigure_noop_result})\n"
        "${_reconfigure_noop_stdout}\n${_reconfigure_noop_stderr}")
endif()
set(_reconfigure_noop_log
    "${_reconfigure_noop_stdout}\n${_reconfigure_noop_stderr}")
string(FIND "${_reconfigure_noop_log}" "ninja: no work to do."
    _reconfigure_noop_found)
if(_reconfigure_noop_found LESS 0)
    message(FATAL_ERROR
        "empty private archive rebuilt after reconfigure:\n${_reconfigure_noop_log}")
endif()

_configure(collision FALSE "is already owned by first-provider")
_configure(escape FALSE "private linklib output escapes the build tree")
_configure(conflicting-modes FALSE "CANONICAL_OUTPUT and OUTPUT_DIR")
_configure(empty-with-sources FALSE "EMPTY_ARCHIVE cannot carry source inputs")
_configure(empty-without-output FALSE
    "EMPTY_ARCHIVE requires an explicit private")
_configure(unsafe-dollar FALSE "private linklib output contains unsafe syntax")
_configure(unsafe-semicolon FALSE "contains unsafe syntax")
_configure(unsafe-backslash FALSE "private linklib output contains unsafe syntax")

file(REMOVE_RECURSE "${_root}")
message(STATUS "private linklib output test passed")
