cmake_minimum_required(VERSION 3.24)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

find_program(_ninja NAMES ninja ninja-build)
if(NOT _ninja)
    message(FATAL_ERROR "GenmoduleWritefilesRuleTest requires Ninja")
endif()
find_program(_host_cc NAMES cc)
if(NOT _host_cc)
    message(FATAL_ERROR "GenmoduleWritefilesRuleTest requires a host cc compiler")
endif()

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_root "$ENV{TMPDIR}/aros-genmodule-writefiles-rule-${_suffix}")
else()
    set(_root "/tmp/aros-genmodule-writefiles-rule-${_suffix}")
endif()
get_filename_component(_root "${_root}" ABSOLUTE)
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "temporary test root already exists: ${_root}")
endif()
file(MAKE_DIRECTORY "${_root}")

set(_fixture "${CMAKE_CURRENT_LIST_DIR}/genmodule-writefiles-rule")
set(_reference_project "${CMAKE_CURRENT_LIST_DIR}/reference-genmodule")
set(_reference_source "${AROS_TEST_TREE}/tools/genmodule")
set(_reference_build "${_root}/reference-build")
set(_host_genmodule "${_reference_build}/genmodule")
set(_build "${_root}/build")

# Build the unmodified source-owned generator with the host compiler. This
# standalone reference build is deliberately separate from the project(NONE)
# engine fixture below.
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

function(_configure_negative_fixture build_dir probe expected)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${build_dir}" -G Ninja
            "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
            "-DAROS_ENGINE_DIR=${AROS_TEST_ENGINE_DIR}"
            "-DAROS_HOST_GENMODULE=${_host_genmodule}"
            "-DAROS_WRITEFILES_PROBE=${probe}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr
        TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
    set(_output "${_stdout}\n${_stderr}")
    if(_result EQUAL 0 OR NOT _output MATCHES "${expected}")
        message(FATAL_ERROR
            "${probe} writefiles configure did not fail as expected "
            "(${_result}); expected /${expected}/\n${_output}")
    endif()
endfunction()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_ENGINE_DIR=${AROS_TEST_ENGINE_DIR}"
        "-DAROS_HOST_GENMODULE=${_host_genmodule}"
        -DAROS_WRITEFILES_PROBE=positive
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "genmodule writefiles fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

set(_config "${AROS_TEST_TREE}/workbench/libs/gl/gl.conf")
cmake_path(NORMAL_PATH _config)
string(SHA256 _signature "${_config}|gl|library")
string(SUBSTRING "${_signature}" 0 16 _short_hash)
set(_writer_root "${_build}/genmodule/linklibs/${_short_hash}")
set(_actual_gen "${_writer_root}/gen")
set(_actual_stubs "${_writer_root}/stubs")
set(_actual_include "${_writer_root}/include")

# Build only the public, source-owned endpoint. The normal and relative marker
# calls in the fixture must have resolved to this same private writer target.
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target gl-stubs --parallel 4
    RESULT_VARIABLE _cold_result
    OUTPUT_VARIABLE _cold_stdout
    ERROR_VARIABLE _cold_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _cold_result EQUAL 0)
    message(FATAL_ERROR
        "cold writefiles generation failed (${_cold_result})\n"
        "${_cold_stdout}\n${_cold_stderr}")
endif()

file(STRINGS "${_build}/writefiles-declared-source.txt" _declared_sources)
list(LENGTH _declared_sources _declared_source_count)
if(NOT _declared_source_count EQUAL 1)
    message(FATAL_ERROR
        "fixture did not report exactly one declared source for recovery: "
        "${_declared_sources}")
endif()
list(GET _declared_sources 0 _deleted_source)
if(NOT EXISTS "${_deleted_source}" OR IS_DIRECTORY "${_deleted_source}")
    message(FATAL_ERROR
        "declared writefiles source was not generated: ${_deleted_source}")
endif()
foreach(_root "${_actual_gen}" "${_actual_stubs}")
    if(NOT IS_DIRECTORY "${_root}")
        message(FATAL_ERROR "writefiles output directory was not created: ${_root}")
    endif()
endforeach()
foreach(_private_header
        "${_actual_include}/clib/gl_protos.h"
        "${_actual_include}/inline/gl.h"
        "${_actual_include}/defines/gl.h"
        "${_actual_include}/defines/gl_LVO.h"
        "${_actual_include}/proto/gl.h")
    if(NOT EXISTS "${_private_header}" OR IS_DIRECTORY "${_private_header}")
        message(FATAL_ERROR "private writer header was not generated: ${_private_header}")
    endif()
endforeach()

# Run the independent upstream command against clean reference directories.
set(_reference_gen "${_root}/reference-output/gen")
set(_reference_stubs "${_root}/reference-output/stubs")
file(MAKE_DIRECTORY "${_reference_gen}" "${_reference_stubs}")
execute_process(
    COMMAND "${_host_genmodule}" -c "${_config}"
        -d "${_reference_gen}" -l "${_reference_stubs}"
        writefiles gl library
    RESULT_VARIABLE _reference_writefiles_result
    OUTPUT_VARIABLE _reference_writefiles_stdout
    ERROR_VARIABLE _reference_writefiles_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _reference_writefiles_result EQUAL 0)
    message(FATAL_ERROR
        "reference writefiles failed (${_reference_writefiles_result})\n"
        "${_reference_writefiles_stdout}\n${_reference_writefiles_stderr}")
endif()

function(_collect_writefiles_outputs out_var gen_root stub_root)
    set(_entries)
    foreach(_pair "gen|${gen_root}" "stubs|${stub_root}")
        string(REPLACE "|" ";" _parts "${_pair}")
        list(GET _parts 0 _label)
        list(GET _parts 1 _root)
        file(GLOB_RECURSE _files LIST_DIRECTORIES FALSE "${_root}/*")
        foreach(_file IN LISTS _files)
            file(RELATIVE_PATH _relative "${_root}" "${_file}")
            file(SHA256 "${_file}" _digest)
            list(APPEND _entries "${_label}/${_relative}|${_digest}")
        endforeach()
    endforeach()
    list(SORT _entries)
    set(${out_var} "${_entries}" PARENT_SCOPE)
endfunction()

_collect_writefiles_outputs(_expected_outputs "${_reference_gen}" "${_reference_stubs}")
_collect_writefiles_outputs(_actual_outputs "${_actual_gen}" "${_actual_stubs}")
if(NOT _expected_outputs STREQUAL _actual_outputs)
    message(FATAL_ERROR
        "CMake writefiles output paths or bytes differ from upstream genmodule\n"
        "expected: ${_expected_outputs}\nactual: ${_actual_outputs}")
endif()
if(NOT _expected_outputs)
    message(FATAL_ERROR "upstream genmodule writefiles produced no outputs")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target gl-stubs --parallel 4
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _noop_result EQUAL 0 OR
   "${_noop_stdout}\n${_noop_stderr}" MATCHES
       "Generating gl[.]library client-link sources")
    message(FATAL_ERROR
        "unchanged writefiles build was not a no-op (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()

# Removing one manifest-declared source must re-run the writer and restore it.
file(REMOVE "${_deleted_source}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target gl-stubs --parallel 4
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr
    TIMEOUT ${AROS_TEST_CHILD_TIMEOUT})
if(NOT _repair_result EQUAL 0 OR
   NOT "${_repair_stdout}\n${_repair_stderr}" MATCHES
       "Generating gl[.]library client-link sources" OR
   NOT EXISTS "${_deleted_source}" OR IS_DIRECTORY "${_deleted_source}")
    message(FATAL_ERROR
        "missing generated source did not rerun and restore (${_repair_result})\n"
        "${_repair_stdout}\n${_repair_stderr}")
endif()

_configure_negative_fixture("${_root}/duplicate-build" duplicate
    "unsafe or duplicate target")
_configure_negative_fixture("${_root}/traversal-build" traversal
    "CONFIG contains traversal")
_configure_negative_fixture("${_root}/missing-config-build" missing-config
    "config is not a regular source file")
_configure_negative_fixture("${_root}/symlink-build" output-symlink
    "generated output path crosses symlink")
_configure_negative_fixture("${_root}/source-parent-symlink-build" source-parent-symlink
    "config path crosses symlink")

message(STATUS
    "genmodule writefiles rule test passed: shared writer target, exact upstream "
    "output set and bytes, no-op, missing-source recovery, and duplicate/path/config/symlink rejection")
file(REMOVE_RECURSE "${_root}")
