cmake_minimum_required(VERSION 3.22)

get_filename_component(_repo "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
include("${_repo}/cmake/Executable.cmake")
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/sfdc-host-tool")
set(_perl "/usr/bin/perl")
if(NOT EXISTS "${_perl}")
    message(FATAL_ERROR "sfdc host-tool test requires the explicit ${_perl} interpreter")
endif()

string(RANDOM LENGTH 10 ALPHABET 0123456789abcdef _suffix)
set(_root "/tmp/aros-sfdc-host-tool-${_suffix}")
file(REMOVE_RECURSE "${_root}")
file(MAKE_DIRECTORY "${_root}")

function(_sfdc_configure name expect_success expected_message)
    set(_build "${_root}/${name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DAROS_REPO_ROOT=${_repo}"
            "-DHOST_PERL=${_perl}"
            "-DSFDC_HOST_TOOL_CASE=${name}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR "sfdc host-tool ${name} configure failed (${_result})\n${_log}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "sfdc host-tool ${name} unexpectedly configured")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "sfdc host-tool ${name} missed '${expected_message}'\n${_log}")
        endif()
    endif()
    set(CONFIGURED_BUILD "${_build}" PARENT_SCOPE)
endfunction()

file(SHA256 "${_repo}/tools/sfdc/main.pl" _main_before)
file(SHA256 "${_repo}/tools/sfdc/CLib.pl" _class_before)
file(SHA256 "${_repo}/tools/sfdc/sfdc-host.inputs" _manifest_before)
file(TIMESTAMP "${_repo}/tools/sfdc/main.pl" _main_time_before UTC)

_sfdc_configure("" TRUE "")
set(_build "${CONFIGURED_BUILD}")
set(_output "${_build}/hosttools/sfdc")
set(_hostile_path "/tmp/aros-sfdc-inherited-path-must-not-be-used")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
        "${CMAKE_COMMAND}" --build "${_build}" --target host-sfdc
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0 OR NOT EXISTS "${_output}" OR
   IS_DIRECTORY "${_output}" OR IS_SYMLINK "${_output}")
    message(FATAL_ERROR "sfdc host-tool build failed\n${_build_stdout}${_build_stderr}")
endif()
file(SHA256 "${_output}" _output_hash_before)
aros_path_is_executable("${_output}" _sfdc_output_executable)
if(NOT _sfdc_output_executable)
    message(FATAL_ERROR "sfdc host-tool output is not executable")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}" "${_output}" --version
    RESULT_VARIABLE _version_result
    OUTPUT_VARIABLE _version_stdout
    ERROR_VARIABLE _version_stderr)
set(_version_log "${_version_stdout}${_version_stderr}")
if(NOT _version_result EQUAL 0 OR
   NOT _version_log MATCHES "sfdc 1\\.3 \\(2004-11-12\\)")
    message(FATAL_ERROR "sfdc host-tool output is not runnable\n${_version_log}")
endif()

# Exercise an actual generator mode against the AHI SFD input that will later
# consume this host tool.  This remains a host-tool test; it does not wire the
# AHI configure declaration into CMake.
set(_generated_header "${_build}/generated/ahi_protos.h")
file(MAKE_DIRECTORY "${_build}/generated")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}" "${_output}"
        --quiet --mode=clib --target=x86_64-aros
        "--output=${_generated_header}"
        "${_repo}/workbench/devs/AHI/Include/SFD/ahi_lib.sfd"
    RESULT_VARIABLE _generator_result
    OUTPUT_VARIABLE _generator_stdout
    ERROR_VARIABLE _generator_stderr)
if(NOT _generator_result EQUAL 0 OR NOT EXISTS "${_generated_header}")
    message(FATAL_ERROR
        "sfdc host-tool did not generate an AHI clib header\n"
        "${_generator_stdout}${_generator_stderr}")
endif()
file(READ "${_generated_header}" _generated_header_content)
if(NOT _generated_header_content MATCHES "#ifndef CLIB_AHI_PROTOS_H")
    message(FATAL_ERROR "sfdc host-tool generated an invalid AHI clib header")
endif()

set(_contract "${_build}/.aros-host-sfdc-contract.cmake")
file(READ "${_contract}" _contract_content)
string(FIND "${_contract_content}"
    "set(SFDC_PERL [==[${_perl}]==])" _perl_position)
string(FIND "${_contract_content}"
    "set(SFDC_INPUT_MANIFEST_SHA256 [==[${_manifest_before}]==])"
    _manifest_position)
if(_perl_position LESS 0 OR _manifest_position LESS 0)
    message(FATAL_ERROR "sfdc host-tool contract is not closed")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
        "${CMAKE_COMMAND}" --build "${_build}" --target host-sfdc
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
set(_noop_log "${_noop_stdout}${_noop_stderr}")
if(NOT _noop_result EQUAL 0 OR NOT _noop_log MATCHES "no work to do")
    message(FATAL_ERROR "sfdc host-tool no-op check failed\n${_noop_log}")
endif()

_sfdc_configure("" TRUE "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
        "${CMAKE_COMMAND}" --build "${CONFIGURED_BUILD}" --target host-sfdc
    RESULT_VARIABLE _reconfigure_result
    OUTPUT_VARIABLE _reconfigure_stdout
    ERROR_VARIABLE _reconfigure_stderr)
set(_reconfigure_log "${_reconfigure_stdout}${_reconfigure_stderr}")
if(NOT _reconfigure_result EQUAL 0 OR
   NOT _reconfigure_log MATCHES "no work to do")
    message(FATAL_ERROR "sfdc host-tool rebuilt after no-op reconfigure\n${_reconfigure_log}")
endif()

file(REMOVE "${_output}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
        "${CMAKE_COMMAND}" --build "${_build}" --target host-sfdc
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr)
if(NOT _repair_result EQUAL 0 OR NOT EXISTS "${_output}")
    message(FATAL_ERROR "sfdc host-tool repair failed\n${_repair_stdout}${_repair_stderr}")
endif()
file(SHA256 "${_output}" _output_hash_after)
if(NOT _output_hash_before STREQUAL _output_hash_after)
    message(FATAL_ERROR "sfdc host-tool repair is not reproducible")
endif()

# The runner repeats containment checks immediately before its write.  A
# symlink introduced after configuration must not redirect a repair outside the
# private build tree.
file(REMOVE "${_output}")
file(REMOVE_RECURSE "${_build}/hosttools")
set(_escaped_output_root "${_root}-escaped-output")
file(MAKE_DIRECTORY "${_escaped_output_root}")
file(CREATE_LINK "${_escaped_output_root}" "${_build}/hosttools" SYMBOLIC)
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E env "PATH=${_hostile_path}"
        "${CMAKE_COMMAND}" --build "${_build}" --target host-sfdc
    RESULT_VARIABLE _symlink_result
    OUTPUT_VARIABLE _symlink_stdout
    ERROR_VARIABLE _symlink_stderr)
set(_symlink_log "${_symlink_stdout}${_symlink_stderr}")
if(_symlink_result EQUAL 0 OR
   NOT _symlink_log MATCHES "runner contract escaped its owning tree")
    message(FATAL_ERROR "sfdc host-tool runner accepted an output symlink\n${_symlink_log}")
endif()

file(SHA256 "${_repo}/tools/sfdc/main.pl" _main_after)
file(SHA256 "${_repo}/tools/sfdc/CLib.pl" _class_after)
file(SHA256 "${_repo}/tools/sfdc/sfdc-host.inputs" _manifest_after)
file(TIMESTAMP "${_repo}/tools/sfdc/main.pl" _main_time_after UTC)
if(NOT _main_before STREQUAL _main_after OR
   NOT _class_before STREQUAL _class_after OR
   NOT _manifest_before STREQUAL _manifest_after OR
   NOT _main_time_before STREQUAL _main_time_after)
    message(FATAL_ERROR "sfdc host-tool runner modified its source tree")
endif()

_sfdc_configure("bad-manifest" FALSE "input extra.pl is missing or symlinked")
_sfdc_configure("symlink-input" FALSE "input CLib.pl is missing or symlinked")
_sfdc_configure("symlink-binary" FALSE "audited paths escape their owning tree")
_sfdc_configure("relative-perl" FALSE "PERL must be an absolute path")

# Repository inventories contain paths, not fixed content hashes. A normal
# source edit must make CMake refresh the runner snapshot automatically and
# rebuild successfully without touching sfdc-host.inputs.
_sfdc_configure("mutable-source" TRUE "")
set(_mutable_build "${CONFIGURED_BUILD}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_mutable_build}" --target host-sfdc
    RESULT_VARIABLE _mutable_first_result
    OUTPUT_VARIABLE _mutable_first_stdout
    ERROR_VARIABLE _mutable_first_stderr)
if(NOT _mutable_first_result EQUAL 0)
    message(FATAL_ERROR
        "sfdc mutable-source initial build failed\n${_mutable_first_stdout}${_mutable_first_stderr}")
endif()
set(_mutable_contract "${_mutable_build}/.aros-host-sfdc-contract.cmake")
file(READ "${_mutable_contract}" _mutable_contract_before)
file(APPEND "${_mutable_build}/source-root/tools/sfdc/Dump.pl"
    "\n# source snapshot refresh regression\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_mutable_build}" --target host-sfdc
    RESULT_VARIABLE _mutable_second_result
    OUTPUT_VARIABLE _mutable_second_stdout
    ERROR_VARIABLE _mutable_second_stderr)
if(NOT _mutable_second_result EQUAL 0)
    message(FATAL_ERROR
        "sfdc mutable-source rebuild failed\n${_mutable_second_stdout}${_mutable_second_stderr}")
endif()
file(READ "${_mutable_contract}" _mutable_contract_after)
if(_mutable_contract_before STREQUAL _mutable_contract_after)
    message(FATAL_ERROR
        "sfdc mutable-source edit did not refresh the dynamic runner snapshot")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "sfdc host-tool test passed")
