cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
set(_root "$ENV{TMPDIR}/aros-full-genmodule-abi-${_suffix}")
if(NOT "$ENV{TMPDIR}")
    set(_root "/tmp/aros-full-genmodule-abi-${_suffix}")
endif()
get_filename_component(_root "${_root}" ABSOLUTE)
set(_source "${CMAKE_CURRENT_LIST_DIR}/full-genmodule-abi")
set(_build "${_root}/build")
set(_reference_project "${CMAKE_CURRENT_LIST_DIR}/reference-genmodule")
set(_reference_source "${AROS_TEST_TREE}/tools/genmodule")

# Exercise the inventory parser on private source copies. The valid probe
# protects the Makefile.deps continuation syntax; the negative probes reject
# escaping paths, missing sources and duplicate inventory declarations before
# a generated executable could mask the problem.
set(_probe_root "${_root}/inventory-probes")
set(_valid_source "${_probe_root}/valid/tools/genmodule")
file(COPY "${_reference_source}/" DESTINATION "${_valid_source}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_reference_project}"
        -B "${_probe_root}/valid-build" -G Ninja
        "-DAROS_GENMODULE_SOURCE_DIR=${_valid_source}"
        -DAROS_GENMODULE_VALIDATE_ONLY=ON
    RESULT_VARIABLE _valid_probe_result
    OUTPUT_VARIABLE _valid_probe_stdout
    ERROR_VARIABLE _valid_probe_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _valid_probe_result EQUAL 0)
    message(FATAL_ERROR
        "valid genmodule inventory counterprobe failed (${_valid_probe_result})\n"
        "${_valid_probe_stdout}\n${_valid_probe_stderr}")
endif()

function(_expect_inventory_rejection _label _source_dir _build_dir _expected)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_reference_project}"
            -B "${_build_dir}" -G Ninja
            "-DAROS_GENMODULE_SOURCE_DIR=${_source_dir}"
            -DAROS_GENMODULE_VALIDATE_ONLY=ON
        RESULT_VARIABLE _probe_result
        OUTPUT_VARIABLE _probe_stdout
        ERROR_VARIABLE _probe_stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    set(_probe_output "${_probe_stdout}\n${_probe_stderr}")
    if(_probe_result EQUAL 0 OR NOT _probe_output MATCHES "${_expected}")
        message(FATAL_ERROR
            "${_label} genmodule inventory counterprobe did not fail as expected "
            "(${_probe_result})\n${_probe_output}")
    endif()
endfunction()

set(_unsafe_source "${_probe_root}/unsafe/tools/genmodule")
file(COPY "${_reference_source}/" DESTINATION "${_unsafe_source}")
file(READ "${_unsafe_source}/Makefile.deps" _unsafe_inventory)
string(REPLACE "genmodule.c" "../genmodule.c"
    _unsafe_inventory "${_unsafe_inventory}")
file(WRITE "${_unsafe_source}/Makefile.deps" "${_unsafe_inventory}")
_expect_inventory_rejection(unsafe "${_unsafe_source}"
    "${_probe_root}/unsafe-build" "unsafe GENMODULE_SRCS entry: \.\./genmodule\.c")

set(_missing_source "${_probe_root}/missing/tools/genmodule")
file(COPY "${_reference_source}/" DESTINATION "${_missing_source}")
file(READ "${_missing_source}/Makefile.deps" _missing_inventory)
string(REPLACE "writelinkentries.c" "aros-test-missing-probe.c"
    _missing_inventory "${_missing_inventory}")
file(WRITE "${_missing_source}/Makefile.deps" "${_missing_inventory}")
_expect_inventory_rejection(missing "${_missing_source}"
    "${_probe_root}/missing-build" "GENMODULE_SRCS source is missing or unsafe: aros-test-missing-probe\.c")

set(_duplicate_source "${_probe_root}/duplicate/tools/genmodule")
file(COPY "${_reference_source}/" DESTINATION "${_duplicate_source}")
file(APPEND "${_duplicate_source}/Makefile.deps"
    "\nGENMODULE_SRCS := genmodule.c\n")
_expect_inventory_rejection(duplicate "${_duplicate_source}"
    "${_probe_root}/duplicate-build" "one complete, non-empty GENMODULE_SRCS")

if(DEFINED ENV{AROS_HOST_GENMODULE} AND
   NOT "$ENV{AROS_HOST_GENMODULE}" STREQUAL "")
    # An explicit override is authoritative. If it is invalid, report that
    # path instead of silently substituting a locally built reference tool.
    set(_host_genmodule "$ENV{AROS_HOST_GENMODULE}")
    if(NOT IS_ABSOLUTE "${_host_genmodule}" OR
       NOT EXISTS "${_host_genmodule}" OR IS_DIRECTORY "${_host_genmodule}")
        message(FATAL_ERROR
            "AROS_HOST_GENMODULE override must name an existing file: "
            "${_host_genmodule}")
    endif()
else()
    set(_reference_build "${_root}/reference-build")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_reference_project}"
            -B "${_reference_build}" -G Ninja
            "-DAROS_GENMODULE_SOURCE_DIR=${_reference_source}"
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
    if(NOT _reference_build_result EQUAL 0)
        message(FATAL_ERROR
            "reference genmodule build failed (${_reference_build_result})\n"
            "${_reference_build_stdout}\n${_reference_build_stderr}")
    endif()
    set(_host_genmodule "${_reference_build}/genmodule")
    if(NOT EXISTS "${_host_genmodule}" OR IS_DIRECTORY "${_host_genmodule}")
        message(FATAL_ERROR
            "reference genmodule build did not create ${_host_genmodule}")
    endif()
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        "-DAROS_HOST_GENMODULE=${_host_genmodule}"
        ${AROS_TEST_TOOL_ARGS}
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "full genmodule ABI fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

function(_configure_archive_probe _mode _expectation _expected_pattern)
    set(_probe_build "${_root}/archive-probes/${_mode}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_probe_build}" -G Ninja
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
            "-DAROS_HOST_GENMODULE=${_host_genmodule}"
            "-DAROS_ARCHIVE_PROBE=${_mode}"
            ${AROS_TEST_TOOL_ARGS}
        RESULT_VARIABLE _probe_result
        OUTPUT_VARIABLE _probe_stdout
        ERROR_VARIABLE _probe_stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    set(_probe_output "${_probe_stdout}\n${_probe_stderr}")
    if(_expectation STREQUAL "accept")
        if(NOT _probe_result EQUAL 0)
            message(FATAL_ERROR
                "${_mode} archive probe configure failed (${_probe_result})\n${_probe_output}")
        endif()
    elseif(_expectation STREQUAL "reject")
        if(_probe_result EQUAL 0 OR
           NOT _probe_output MATCHES "${_expected_pattern}")
            message(FATAL_ERROR
                "${_mode} archive probe did not fail as expected (${_probe_result}); "
                "expected '${_expected_pattern}'\n${_probe_output}")
        endif()
    else()
        message(FATAL_ERROR "unknown archive probe expectation: ${_expectation}")
    endif()
endfunction()

# Every suppression spelling requires a source-native runtime declaration and
# cannot be used for genmodule-only declarations. Test the all-archives switch
# and both partial switches against both invalid contexts.
foreach(_context no-contract genmodule-only)
    foreach(_variant all normal relative)
        _configure_archive_probe("reject-${_context}-${_variant}" reject
            "NO_CLIENT_ARCHIVES requires a selected native runtime")
    endforeach()
endforeach()

# Canonical implementation archives and generated client archive spellings
# share one configure-time ownership ledger. Exercise primary, relative and
# LINKLIB_NAME archive collisions in both declaration orders.
foreach(_collision primary relative alias alias-relative)
    foreach(_order client-first implementation-first)
        if(_collision STREQUAL "primary")
            set(_collision_archive "libtiff\\.a")
        elseif(_collision STREQUAL "relative")
            set(_collision_archive "libtiff_rel\\.a")
        elseif(_collision STREQUAL "alias")
            set(_collision_archive "libarchive-probe-alias\\.a")
        else()
            set(_collision_archive "libarchive-probe-alias_rel\\.a")
        endif()
        _configure_archive_probe("collision-${_collision}-${_order}" reject
            "${_collision_archive}")
    endforeach()
endforeach()

# All three valid native suppression modes retain the runtime and ABI support
# graph. The all-suppressed case is built below to prove actual headers, FD,
# libdefs and generated module sources remain available without client archive
# targets.
foreach(_variant all normal-suppressed relative-suppressed)
    _configure_archive_probe("positive-${_variant}" accept "")
endforeach()

set(_archive_probe_build "${_root}/archive-probes/positive-all")
foreach(_target archive-native-includes archive-native-genmodfiles)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_archive_probe_build}"
            --target "${_target}"
        RESULT_VARIABLE _archive_build_result
        OUTPUT_VARIABLE _archive_build_stdout
        ERROR_VARIABLE _archive_build_stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    if(NOT _archive_build_result EQUAL 0)
        message(FATAL_ERROR
            "native archive-suppressed ABI target ${_target} failed "
            "(${_archive_build_result})\n${_archive_build_stdout}\n${_archive_build_stderr}")
    endif()
endforeach()

set(_archive_output_root "${_archive_probe_build}/SYS/Developer")
file(GLOB _archive_public_protos
    "${_archive_output_root}/include/clib/*_protos.h")
file(GLOB _archive_public_fds
    "${_archive_output_root}/SDK/fd/*_lib.fd")
set(_archive_genmodule_root
    "${_archive_probe_build}/genmodule/workbench/libs/tiff/archive_native")
file(GLOB _archive_libdefs "${_archive_genmodule_root}/gen/*_libdefs.h")
file(GLOB _archive_module_sources
    "${_archive_genmodule_root}/gen/*_start.c"
    "${_archive_genmodule_root}/gen/*_end.c")
foreach(_outputs _archive_public_protos _archive_public_fds
        _archive_libdefs _archive_module_sources)
    if(NOT ${_outputs})
        message(FATAL_ERROR
            "NO_CLIENT_ARCHIVES removed required ABI/genmodule output set ${_outputs}")
    endif()
endforeach()
foreach(_archive libtiff.a libtiff_rel.a
        libarchive-probe-alias.a libarchive-probe-alias_rel.a)
    if(EXISTS "${_archive_output_root}/lib/${_archive}")
        message(FATAL_ERROR
            "NO_CLIENT_ARCHIVES unexpectedly produced ${_archive_output_root}/lib/${_archive}")
    endif()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target probe-includes
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "full genmodule ABI fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

foreach(_target includes-MUI probe-mui-includes)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target "${_target}"
        RESULT_VARIABLE _noincludes_result
        OUTPUT_VARIABLE _noincludes_stdout
        ERROR_VARIABLE _noincludes_stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    if(NOT _noincludes_result EQUAL 0)
        message(FATAL_ERROR
            "MUI.MiamiPanel noincludes alias ${_target} failed "
            "(${_noincludes_result})\n${_noincludes_stdout}\n${_noincludes_stderr}")
    endif()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target probe-mui-genmodfiles
    RESULT_VARIABLE _noincludes_files_result
    OUTPUT_VARIABLE _noincludes_files_stdout
    ERROR_VARIABLE _noincludes_files_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _noincludes_files_result EQUAL 0)
    message(FATAL_ERROR
        "MUI.MiamiPanel noincludes genmodule producer failed "
        "(${_noincludes_files_result})\n"
        "${_noincludes_files_stdout}\n${_noincludes_files_stderr}")
endif()
if(NOT EXISTS "${_build}/mui-getlibbase-source.txt")
    message(FATAL_ERROR "MUI.MiamiPanel getlibbase archive source was not retained")
endif()
file(READ "${_build}/mui-getlibbase-source.txt" _getlibbase_source)
if(NOT EXISTS "${_getlibbase_source}")
    message(FATAL_ERROR
        "MUI.MiamiPanel getlibbase producer did not materialize ${_getlibbase_source}")
endif()
get_filename_component(_getlibbase_dir "${_getlibbase_source}" DIRECTORY)
get_filename_component(_mui_genmodule_root "${_getlibbase_dir}" DIRECTORY)
get_filename_component(_mui_genmodule_root "${_mui_genmodule_root}" DIRECTORY)
foreach(_absent IN ITEMS
        "${_mui_genmodule_root}/include/clib/MUI_protos.h"
        "${_mui_genmodule_root}/include/inline/MUI.h"
        "${_mui_genmodule_root}/include/defines/MUI.h"
        "${_mui_genmodule_root}/include/defines/MUI_LVO.h"
        "${_mui_genmodule_root}/include/proto/MUI.h"
        "${_mui_genmodule_root}/fd/MUI_lib.fd"
        "${_build}/SDK/include/clib/MUI_protos.h"
        "${_build}/SDK/include/inline/MUI.h"
        "${_build}/SDK/include/defines/MUI.h"
        "${_build}/SDK/include/defines/MUI_LVO.h"
        "${_build}/SDK/include/proto/MUI.h"
        "${_build}/GENINCDIR/clib/MUI_protos.h"
        "${_build}/GENINCDIR/inline/MUI.h"
        "${_build}/GENINCDIR/defines/MUI.h"
        "${_build}/GENINCDIR/defines/MUI_LVO.h"
        "${_build}/GENINCDIR/proto/MUI.h"
        "${_build}/SYS/Developer/include/clib/MUI_protos.h"
        "${_build}/SYS/Developer/include/inline/MUI.h"
        "${_build}/SYS/Developer/include/defines/MUI.h"
        "${_build}/SYS/Developer/include/defines/MUI_LVO.h"
        "${_build}/SYS/Developer/include/proto/MUI.h"
        "${_build}/SYS/Developer/SDK/fd/MUI_lib.fd")
    if(EXISTS "${_absent}")
        message(FATAL_ERROR "MUI.MiamiPanel noincludes unexpectedly created ${_absent}")
    endif()
endforeach()
file(READ "${_build}/build.ninja" _ninja_graph)
if(_ninja_graph MATCHES "MUI_protos\\.h|MUI_lib\\.fd")
    message(FATAL_ERROR "MUI.MiamiPanel noincludes declared header or FD Ninja outputs")
endif()
foreach(_stamp includes fd)
    if(NOT EXISTS "${_build}/${_stamp}.stamp")
        message(FATAL_ERROR
            "building probe-includes did not materialise ${_stamp}.stamp")
    endif()
endforeach()

# The only source-free full module has no function list.  The reference
# genmodule intentionally emits no FD for that shape, so its branch must not
# opt into the FD binder and declare an output that can never exist.
file(READ "${CMAKE_CURRENT_LIST_DIR}/../AROS.cmake" _aros_cmake)
string(FIND "${_aros_cmake}" "    if(ARG_GENMODULE_ONLY)" _source_free_start)
if(_source_free_start LESS 0)
    message(FATAL_ERROR "could not locate the source-free full-module branch")
endif()
string(SUBSTRING "${_aros_cmake}" ${_source_free_start} -1 _source_free_tail)
string(FIND "${_source_free_tail}" "        return()\n    endif()"
    _source_free_length)
if(_source_free_length LESS 0)
    message(FATAL_ERROR "could not locate the end of the source-free branch")
endif()
string(SUBSTRING "${_aros_cmake}" ${_source_free_start}
    ${_source_free_length} _source_free_branch)
if(_source_free_branch MATCHES
   "_aros_generate_module_support\\(_gm ABI|_aros_bind_genmodule_abi_targets|ARG_MMAKE_ID[}]-fd")
    message(FATAL_ERROR
        "source-free full modules must not declare a functionless FD output")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target probe-includes
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
if(NOT _noop_result EQUAL 0 OR
   NOT _noop_stdout MATCHES "no work to do")
    message(FATAL_ERROR
        "full genmodule ABI fixture was not a no-op (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "full genmodule ABI/FD target contract test passed")
