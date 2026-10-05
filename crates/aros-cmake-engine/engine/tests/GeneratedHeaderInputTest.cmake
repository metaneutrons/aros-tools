cmake_minimum_required(VERSION 3.24)

find_program(_ninja NAMES ninja ninja-build REQUIRED)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root /tmp)
endif()
file(REAL_PATH "${_temp_root}" _temp_root)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-generated-header-input-${_suffix}")
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "test root already exists: ${_root}")
endif()

set(_fixture "${CMAKE_CURRENT_LIST_DIR}/generated-header-input")
set(_engine "${CMAKE_CURRENT_LIST_DIR}/..")
set(_build "${_root}/chain-build")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DAROS_ENGINE_DIR=${_engine}"
        -DAROS_HEADER_INPUT_PROBE=chain
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr
    TIMEOUT 180)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "generated-input chain configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

function(_build_chain out)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target sdk-mirror
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 180)
    set(_output "${_stdout}\n${_stderr}")
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "generated-input chain build failed (${_result})\n${_output}")
    endif()
    set(${out} "${_output}" PARENT_SCOPE)
endfunction()

_build_chain(_cold_output)
foreach(_required
        "Copying generated header .*/seed\.h"
        "Replacing matching header lines .*/whole\.h"
        "Copying generated header .*/mirror\.h")
    if(NOT _cold_output MATCHES "${_required}")
        message(FATAL_ERROR
            "cold generated-input chain omitted /${_required}/\n${_cold_output}")
    endif()
endforeach()

set(_whole "${_build}/gen/chain/whole.h")
set(_mirror "${_build}/SDK/include/chain/mirror.h")
set(_expected [=[before
#if defined(PLATFORM)
#define FEATURE_TOKEN
/* literal; ${prefix} */
#else
#undef FEATURE_TOKEN
#endif
after
]=])
foreach(_output IN ITEMS "${_whole}" "${_mirror}")
    if(NOT EXISTS "${_output}" OR IS_DIRECTORY "${_output}")
        message(FATAL_ERROR "generated-input chain omitted ${_output}")
    endif()
    file(READ "${_output}" _actual)
    if(NOT _actual STREQUAL _expected)
        message(FATAL_ERROR "generated-input chain content mismatch at ${_output}")
    endif()
endforeach()

_build_chain(_noop_output)
if(_noop_output MATCHES "Copying generated header|Replacing matching header lines")
    message(FATAL_ERROR "unchanged generated-input chain was not a no-op\n${_noop_output}")
endif()

file(REMOVE "${_whole}")
_build_chain(_repair_output)
if(NOT _repair_output MATCHES "Replacing matching header lines .*/whole\.h" OR
   NOT _repair_output MATCHES "Copying generated header .*/mirror\.h")
    message(FATAL_ERROR
        "deleted intermediate did not repair the complete chain\n${_repair_output}")
endif()
foreach(_output IN ITEMS "${_whole}" "${_mirror}")
    file(READ "${_output}" _actual)
    if(NOT _actual STREQUAL _expected)
        message(FATAL_ERROR "deleted-intermediate repair content mismatch at ${_output}")
    endif()
endforeach()

# Mutating paths after configure must not turn a proved copy into a symlink
# read or write. Test the actual Ninja command, not just configure admission.
set(_outside "${_root}/outside.h")
file(WRITE "${_outside}" "outside sentinel\n")
file(REMOVE "${_build}/inputs/seed.h" "${_build}/SDK/include/chain/seed.h")
file(CREATE_LINK "${_outside}" "${_build}/inputs/seed.h" SYMBOLIC RESULT _link_result)
if(NOT "${_link_result}" STREQUAL "0")
    message(FATAL_ERROR "could not create runtime input symlink probe")
endif()
execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target copy-stage
    RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 180)
if(_result EQUAL 0 OR NOT "${_stdout}\n${_stderr}" MATCHES "input crosses a symlink")
    message(FATAL_ERROR "post-configure input symlink was not refused\n${_stdout}\n${_stderr}")
endif()
file(REMOVE "${_build}/inputs/seed.h")
file(WRITE "${_build}/inputs/seed.h" "before\n/* FEATURE_TOKEN */\nafter\n")
_build_chain(_restored_output)
file(REMOVE "${_mirror}")
file(CREATE_LINK "${_outside}" "${_mirror}" SYMBOLIC RESULT _link_result)
execute_process(COMMAND "${CMAKE_COMMAND}"
    "-DINPUT=${_whole}" "-DOUTPUT=${_mirror}"
    "-DINPUT_ROOT=${_build}/gen" "-DOUTPUT_ROOT=${_build}/SDK/include"
    "-DBINARY_ROOT=${_build}" -P "${_engine}/CopyHeader.cmake"
    RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 180)
if(_result EQUAL 0 OR NOT "${_stdout}\n${_stderr}" MATCHES "output crosses a symlink")
    message(FATAL_ERROR "post-configure output symlink was not refused\n${_stdout}\n${_stderr}")
endif()
file(READ "${_outside}" _outside_bytes)
if(NOT _outside_bytes STREQUAL "outside sentinel\n")
    message(FATAL_ERROR "runtime symlink probe modified an outside file")
endif()

# Deterministically replace the path immediately after the actual copy command.
# This checks revalidation and cleanup without relying on scheduler timing.
configure_file("${_fixture}/copy-temp-swap.sh" "${_root}/copy-temp-swap" COPYONLY)
file(CHMOD "${_root}/copy-temp-swap" PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE)
foreach(_swap IN ITEMS staging input)
    set(_race_root "${_root}/${_swap}-swap")
    file(MAKE_DIRECTORY "${_race_root}/inputs" "${_race_root}/gen")
    file(WRITE "${_race_root}/inputs/input.h" "staging probe\n")
    execute_process(COMMAND "${CMAKE_COMMAND}" -E env
        "COPY_TEST_REAL_CMAKE=${CMAKE_COMMAND}" "COPY_TEST_SWAP=${_swap}"
        "${CMAKE_COMMAND}" "-DCMAKE_COMMAND=${_root}/copy-temp-swap"
        "-DINPUT=${_race_root}/inputs/input.h" "-DOUTPUT=${_race_root}/gen/output.h"
        "-DINPUT_ROOT=${_race_root}/inputs" "-DOUTPUT_ROOT=${_race_root}/gen"
        "-DBINARY_ROOT=${_race_root}" -P "${_engine}/CopyHeader.cmake"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 180)
    if(_result EQUAL 0 OR NOT "${_stdout}\n${_stderr}" MATCHES "crosses a symlink")
        message(FATAL_ERROR "${_swap} staging replacement was not refused\n${_stdout}\n${_stderr}")
    endif()
    if(EXISTS "${_race_root}/gen/output.h" OR IS_SYMLINK "${_race_root}/gen/output.h")
        message(FATAL_ERROR "${_swap} staging replacement published an output")
    endif()
    file(GLOB _remaining "${_race_root}/gen/output.h.*.tmp")
    if(_remaining)
        message(FATAL_ERROR "${_swap} staging replacement leaked temporary files: ${_remaining}")
    endif()
endforeach()

function(_expect_configure_failure probe expected)
    set(_failure_build "${_root}/${probe}-build")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_failure_build}" -G Ninja
            "-DAROS_ENGINE_DIR=${_engine}"
            "-DAROS_HEADER_INPUT_PROBE=${probe}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT 180)
    set(_output "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT _output MATCHES "${expected}")
        message(FATAL_ERROR
            "${probe} configure did not fail with /${expected}/ (${_result})\n${_output}")
    endif()
endfunction()

_expect_configure_failure(unknown-producer "has no registered header")
_expect_configure_failure(false-owner "has no registered header")
_expect_configure_failure(wrong-producer "owned by real-producer, not someone-else")
_expect_configure_failure(source-input "generated transform input is outside generated include")

message(STATUS
    "generated-input chain passed: cold copy/whole-line/mirror, no-op, deleted-intermediate repair, "
    "and absent/wrong owner plus source-path and runtime-symlink refusals")
file(REMOVE_RECURSE "${_root}")
