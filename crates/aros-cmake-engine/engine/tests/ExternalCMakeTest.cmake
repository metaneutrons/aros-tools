cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/EngineTestTree.cmake")

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros external cmake ${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/external-cmake")
find_program(_external_test_clang NAMES clang)
if(NOT _external_test_clang)
    message(FATAL_ERROR
        "ExternalCMakeTest requires Clang because its target contract uses "
        "Clang's -target option")
endif()

function(_configure case expect_success expected_message)
    set(_build "${_root}/${case}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}"
        "-DAROS_SOURCE_DIR=${AROS_TEST_TREE}"
        "-DAROS_RUST_TOOLS_DIR=${AROS_TEST_TOOLS_DIR}"
        ${AROS_TEST_TOOL_ARGS} -G Ninja
        "-DCMAKE_C_COMPILER=${_external_test_clang}"
        "-DEXTERNAL_TEST_CLANG=${_external_test_clang}"
        "-DEXTERNAL_CMAKE_CASE=${case}"
        ${ARGN}
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
if(_noop_log MATCHES "Building external CMake target")
    message(FATAL_ERROR
        "unchanged SDK rebuilt external archives:\n${_noop_log}")
endif()

# ABI/header changes must invalidate nested archives without requiring a clean
# build. Timestamp-only updates and unchanged reconfiguration must not do so.
function(_sdk_build expected_rebuild)
    execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_success_build}"
        --target external-consumer RESULT_VARIABLE _result
        OUTPUT_VARIABLE _out ERROR_VARIABLE _err)
    set(_rebuilt FALSE)
    if("${_out}${_err}" MATCHES "Building external CMake target fixture-external")
        set(_rebuilt TRUE)
    endif()
    if(NOT _result EQUAL 0 OR NOT "${_rebuilt}" STREQUAL "${expected_rebuild}")
        message(FATAL_ERROR "SDK rebuild contract (${expected_rebuild}) failed: ${_out}${_err}")
    endif()
endfunction()
file(SHA256 "${_archive}" _original_archive_digest)
_configure(success TRUE "")
_sdk_build(FALSE)
file(TOUCH "${_success_build}/SDK/include/fixture-abi.h")
_sdk_build(FALSE)
file(WRITE "${_success_build}/SDK/include/fixture-abi.h"
    "#define FIXTURE_ABI_SIZE 232\n")
_sdk_build(TRUE)
file(SHA256 "${_archive}" _changed_archive_digest)
if(_original_archive_digest STREQUAL _changed_archive_digest)
    message(FATAL_ERROR "SDK ABI change did not reach the compiled external archive")
endif()
_sdk_build(FALSE)
file(WRITE "${_success_build}/SDK/include/new-public-header.h" "/* new API */\n")
_sdk_build(TRUE)
file(REMOVE "${_success_build}/SDK/include/new-public-header.h")
_sdk_build(TRUE)
_sdk_build(FALSE)

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

# A component library group builds only its declared targets, installs only
# its declared header components, and stages only its declared archives.
_configure(component-success TRUE "")
# Selection probes are configure-only: keep the collector populated while
# independently selecting the compiler-driver and direct-link rule. The latter
# uses an executable sentinel, never claims to execute it as an actual LLD.
_configure(component-driver TRUE "" "-DAROS_LLD_BIN=FALSE")
_configure(component-collector TRUE ""
    "-DAROS_LLD_BIN=${_external_test_clang}")
set(_components_build "${_root}/component-success")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_components_build}"
        --target external-consumer
    RESULT_VARIABLE _components_result
    OUTPUT_VARIABLE _components_stdout
    ERROR_VARIABLE _components_stderr)
if(NOT _components_result EQUAL 0)
    message(FATAL_ERROR
        "external-cmake component build failed (${_components_result})\n"
        "${_components_stdout}\n${_components_stderr}")
endif()
string(FIND "${_components_stdout}\n${_components_stderr}"
    "Generating fixture public SDK header" _public_header_barrier_index)
string(FIND "${_components_stdout}\n${_components_stderr}"
    "Building external CMake target fixture-components"
    _component_producer_index)
if(_public_header_barrier_index LESS 0 OR _component_producer_index LESS 0 OR
   _public_header_barrier_index GREATER _component_producer_index)
    message(FATAL_ERROR
        "component producer did not wait for public SDK header generation:\n"
        "${_components_stdout}\n${_components_stderr}")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_components_build}"
        --target external-component-link-consumer
    RESULT_VARIABLE _component_link_result
    OUTPUT_VARIABLE _component_link_stdout
    ERROR_VARIABLE _component_link_stderr)
if(NOT _component_link_result EQUAL 0)
    message(FATAL_ERROR
        "external component consumer link failed (${_component_link_result})\n"
        "${_component_link_stdout}\n${_component_link_stderr}")
endif()
set(_component_binary
    "${_components_build}/gen/external-cmake/fixture-components")
set(_left_archive "${_components_build}/install/lib/libleft.a")
set(_right_archive "${_components_build}/install/lib/libright.a")
set(_left_header "${_components_build}/install/include/left.h")
set(_right_header "${_components_build}/install/include/right.h")
foreach(_product IN ITEMS
        "${_left_archive}" "${_right_archive}"
        "${_left_header}" "${_right_header}"
        "${_component_binary}/lib/libleft.a"
        "${_component_binary}/lib/libright.a"
        "${_component_binary}/lib/libunlisted.a")
    if(NOT EXISTS "${_product}" OR IS_DIRECTORY "${_product}")
        message(FATAL_ERROR "component external product is missing: ${_product}")
    endif()
endforeach()
if(EXISTS "${_components_build}/install/lib/libunlisted.a")
    message(FATAL_ERROR
        "undeclared archive was copied from the component private binary directory")
endif()
if(EXISTS "${_component_binary}/lib/libnot_selected.a")
    message(FATAL_ERROR
        "component build included a target outside its explicit target list")
endif()
if(EXISTS "${_components_build}/install/include/unselected.h")
    message(FATAL_ERROR "unselected component header was installed")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_components_build}"
        --target external-consumer
    RESULT_VARIABLE _components_noop_result
    OUTPUT_VARIABLE _components_noop_stdout
    ERROR_VARIABLE _components_noop_stderr)
if(NOT _components_noop_result EQUAL 0)
    message(FATAL_ERROR
        "component external CMake no-op failed (${_components_noop_result})\n"
        "${_components_noop_stdout}\n${_components_noop_stderr}")
endif()
set(_components_noop_log
    "${_components_noop_stdout}\n${_components_noop_stderr}")
if(_components_noop_log MATCHES "Building external CMake target")
    message(FATAL_ERROR
        "second component external CMake build was not a no-op:\n"
        "${_components_noop_log}")
endif()

file(REMOVE "${_right_archive}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_components_build}"
        --target fixture-components
    RESULT_VARIABLE _components_restore_result
    OUTPUT_VARIABLE _components_restore_stdout
    ERROR_VARIABLE _components_restore_stderr)
if(NOT _components_restore_result EQUAL 0 OR NOT EXISTS "${_right_archive}")
    message(FATAL_ERROR
        "deleted component archive was not restored (${_components_restore_result})\n"
        "${_components_restore_stdout}\n${_components_restore_stderr}")
endif()

_configure(component-missing-product TRUE "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_root}/component-missing-product"
        --target fixture-components
    RESULT_VARIABLE _component_missing_result
    OUTPUT_VARIABLE _component_missing_stdout
    ERROR_VARIABLE _component_missing_stderr)
if(_component_missing_result EQUAL 0)
    message(FATAL_ERROR "missing component archive unexpectedly built")
endif()
set(_component_missing_log
    "${_component_missing_stdout}\n${_component_missing_stderr}")
string(FIND "${_component_missing_log}" "Error copying file"
    _component_missing_found)
if(_component_missing_found LESS 0)
    message(FATAL_ERROR
        "missing component archive missed copy diagnostic:\n"
        "${_component_missing_log}")
endif()

_configure(missing-fetch FALSE "missing fetch target")
_configure(escape-binary FALSE "binary directory must be a private child")
_configure(overlap-prefix FALSE "binary directory overlaps prefix")
_configure(escape-product FALSE "product escapes install prefix")
_configure(unsafe-option FALSE "unsafe external CMake option")
_configure(toolchain-override FALSE "overrides a forced toolchain setting")
_configure(collision FALSE "is already owned by")
_configure(binary-collision FALSE "binary directory is already owned")
_configure(component-invalid-target FALSE "invalid external build/install component")
_configure(component-invalid-install FALSE "invalid external build/install component")
_configure(component-group-no-targets FALSE
    "component library group requires explicit build targets")
_configure(component-group-no-components FALSE
    "selective build requires explicit install components")
_configure(component-host-tool-missing FALSE
    "missing verified cross-toolchain host tool")
_configure(component-host-tool-wrong-version FALSE
    "is not runnable host LLVM")
_configure(component-host-tool-version-prefix FALSE
    "is not runnable host LLVM")
_configure(component-host-tool-duplicate-key FALSE
    "duplicate host-tool binding")

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
