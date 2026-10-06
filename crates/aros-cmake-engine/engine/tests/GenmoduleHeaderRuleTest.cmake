cmake_minimum_required(VERSION 3.24)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

find_program(_ninja NAMES ninja ninja-build)
if(NOT _ninja)
    message(FATAL_ERROR "GenmoduleHeaderRuleTest requires Ninja")
endif()
find_program(_host_cc NAMES cc)
if(NOT _host_cc)
    message(FATAL_ERROR "GenmoduleHeaderRuleTest requires a host cc compiler")
endif()

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_root "$ENV{TMPDIR}/aros-genmodule-header-rule-${_suffix}")
else()
    set(_root "/tmp/aros-genmodule-header-rule-${_suffix}")
endif()
get_filename_component(_root "${_root}" ABSOLUTE)
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

set(_fixture "${CMAKE_CURRENT_LIST_DIR}/genmodule-header-rule")
set(_reference_project "${CMAKE_CURRENT_LIST_DIR}/reference-genmodule")
set(_reference_source "${AROS_TEST_TREE}/tools/genmodule")
set(_reference_build "${_root}/reference-build")
set(_host_genmodule "${_reference_build}/genmodule")
set(_build "${_root}/build")

# Build the unmodified, source-owned C implementation with the host compiler.
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_reference_project}"
        -B "${_reference_build}" -G Ninja
        "-DAROS_GENMODULE_SOURCE_DIR=${_reference_source}"
        "-DCMAKE_C_COMPILER=${_host_cc}"
    RESULT_VARIABLE _reference_configure_result
    OUTPUT_VARIABLE _reference_configure_stdout
    ERROR_VARIABLE _reference_configure_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _reference_configure_result EQUAL 0)
    message(FATAL_ERROR
        "reference genmodule configure failed (${_reference_configure_result})\n"
        "${_reference_configure_stdout}\n${_reference_configure_stderr}")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_reference_build}"
        --target genmodule --parallel 2
    RESULT_VARIABLE _reference_build_result
    OUTPUT_VARIABLE _reference_build_stdout
    ERROR_VARIABLE _reference_build_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _reference_build_result EQUAL 0 OR
   NOT EXISTS "${_host_genmodule}" OR IS_DIRECTORY "${_host_genmodule}")
    message(FATAL_ERROR
        "reference genmodule build failed (${_reference_build_result})\n"
        "${_reference_build_stdout}\n${_reference_build_stderr}")
endif()

function(_configure_fixture build_dir source_root probe config expected)
    set(_geninc_arg "")
    if(ARGC GREATER 5 AND NOT "${ARGV5}" STREQUAL "")
        set(_geninc_arg "-DAROS_TEST_GENINC_DIR=${ARGV5}")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${build_dir}" -G Ninja
            "-DAROS_SOURCE_DIR=${source_root}"
            "-DAROS_ENGINE_DIR=${AROS_TEST_ENGINE_DIR}"
            "-DAROS_HOST_GENMODULE=${_host_genmodule}"
            "-DAROS_HEADER_PROBE=${probe}"
            "-DAROS_HEADER_CONFIG=${config}"
            ${_geninc_arg}
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    set(_output "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT _output MATCHES "${expected}")
        message(FATAL_ERROR
            "${probe} header-rule configure did not fail as expected "
            "(${_result}); expected /${expected}/\n${_output}")
    endif()
endfunction()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_ENGINE_DIR=${AROS_TEST_ENGINE_DIR}"
        "-DAROS_HOST_GENMODULE=${_host_genmodule}"
        -DAROS_HEADER_PROBE=positive
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "genmodule header-rule fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target all-header-rules --parallel 4
    RESULT_VARIABLE _cold_result
    OUTPUT_VARIABLE _cold_stdout
    ERROR_VARIABLE _cold_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _cold_result EQUAL 0)
    message(FATAL_ERROR
        "cold header generation failed (${_cold_result})\n"
        "${_cold_stdout}\n${_cold_stderr}")
endif()

set(_expected_stamps
    "${_build}/gen/rom/kernel/.includes-generated"
    "${_build}/gen/workbench/libs/gl/.includes-generated"
    "${_build}/gen/workbench/tools/SysExplorer/.includes-generated"
    "${_build}/gen/workbench/prefs/network/.includes-generated"
    "${_build}/gen/workbench/libs/vhi/.includes-generated")
set(_expected_headers)

function(_append_expected_headers module declaring layout)
    set(_headers
        "clib/${module}_protos.h"
        "inline/${module}.h"
        "defines/${module}.h"
        "defines/${module}_LVO.h"
        "proto/${module}.h")
    if(layout STREQUAL "Full" OR layout STREQUAL "PrivateOnly")
        set(_private_root "${_build}/gen/${declaring}/include")
        foreach(_header IN LISTS _headers)
            list(APPEND _expected_headers "${_private_root}/${_header}")
        endforeach()
        list(APPEND _expected_headers "${_private_root}/${module}_libdefs.h")
    endif()
    if(layout STREQUAL "Full" OR layout STREQUAL "PublicOnly")
        foreach(_root "${_build}/GENINCDIR" "${_build}/SDK/include")
            foreach(_header IN LISTS _headers)
                list(APPEND _expected_headers "${_root}/${_header}")
            endforeach()
        endforeach()
    endif()
    set(_expected_headers "${_expected_headers}" PARENT_SCOPE)
endfunction()

_append_expected_headers(clocksource "rom/kernel" Full)
_append_expected_headers(gl "workbench/libs/gl" Full)
_append_expected_headers(sysexp "workbench/tools/SysExplorer" PrivateOnly)
_append_expected_headers(netprefs "workbench/prefs/network" PrivateOnly)
_append_expected_headers(vhi "workbench/libs/vhi" PublicOnly)

foreach(_output IN LISTS _expected_stamps _expected_headers)
    if(NOT EXISTS "${_output}" OR IS_DIRECTORY "${_output}")
        message(FATAL_ERROR "reference genmodule did not create ${_output}")
    endif()
endforeach()
list(LENGTH _expected_headers _header_count)
if(NOT _header_count EQUAL 54)
    message(FATAL_ERROR "expected 54 concrete header byproducts, got ${_header_count}")
endif()

file(GLOB_RECURSE _actual_headers LIST_DIRECTORIES FALSE
    "${_build}/gen/*.h"
    "${_build}/GENINCDIR/*.h"
    "${_build}/SDK/include/*.h")
list(SORT _expected_headers)
list(SORT _actual_headers)
if(NOT _actual_headers STREQUAL _expected_headers)
    message(FATAL_ERROR
        "reference genmodule output set differs from the declared headers\n"
        "expected: ${_expected_headers}\nactual: ${_actual_headers}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target all-header-rules --parallel 4
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _noop_result EQUAL 0 OR
   "${_noop_stdout}\n${_noop_stderr}" MATCHES "Generating public headers for")
    message(FATAL_ERROR
        "unchanged header-rule build was not a no-op (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()

# Missing a declared byproduct must rerun the rule and recreate every output.
file(REMOVE "${_build}/GENINCDIR/proto/clocksource.h")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target kernel-clocksource-gen-includes
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _repair_result EQUAL 0 OR
   NOT "${_repair_stdout}\n${_repair_stderr}" MATCHES
       "Generating public headers for clocksource")
    message(FATAL_ERROR
        "missing byproduct did not regenerate (${_repair_result})\n"
        "${_repair_stdout}\n${_repair_stderr}")
endif()
foreach(_output IN LISTS _expected_stamps _expected_headers)
    if(NOT EXISTS "${_output}" OR IS_DIRECTORY "${_output}")
        message(FATAL_ERROR "regeneration did not restore ${_output}")
    endif()
endforeach()

_configure_fixture("${_root}/duplicate-build" "${AROS_TEST_TREE}"
    duplicate "rom/kernel/clocksource.conf" "already claimed by")
_configure_fixture("${_root}/path-build" "${AROS_TEST_TREE}"
    path "rom/kernel/clocksource.conf" "contains traversal")

set(_invalid_source "${_root}/invalid-source")
file(MAKE_DIRECTORY "${_invalid_source}/rom/kernel")
file(READ "${AROS_TEST_TREE}/rom/kernel/clocksource.conf" _invalid_config)
string(REPLACE "##begin config" "##begin config\nunsupported_key value"
    _invalid_config "${_invalid_config}")
if(_invalid_config STREQUAL "")
    message(FATAL_ERROR "could not prepare invalid genmodule config probe")
endif()
file(WRITE "${_invalid_source}/rom/kernel/clocksource.conf" "${_invalid_config}")
_configure_fixture("${_root}/config-build" "${_invalid_source}"
    positive "rom/kernel/clocksource.conf" "unsupported genmodule config option")

set(_symlink_build "${_root}/symlink-build")
set(_outside "${_root}/outside")
file(MAKE_DIRECTORY "${_symlink_build}" "${_outside}")
file(CREATE_LINK "${_outside}" "${_symlink_build}/redirect" SYMBOLIC
    RESULT _symlink_result)
if(NOT _symlink_result STREQUAL "0")
    message(FATAL_ERROR "could not create output-root ancestor symlink: ${_symlink_result}")
endif()
_configure_fixture("${_symlink_build}" "${AROS_TEST_TREE}"
    symlink "rom/kernel/clocksource.conf" "configured output root path"
    "${_symlink_build}/redirect/include")

message(STATUS
    "genmodule header-rule test passed: five real layouts, 54 declared headers, "
    "no-op, missing-byproduct recovery, and duplicate/path/config/symlink rejection")
file(REMOVE_RECURSE "${_root}")
