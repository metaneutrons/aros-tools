cmake_minimum_required(VERSION 3.22)

include("${CMAKE_CURRENT_LIST_DIR}/Executable.cmake")

set(_AROS_SFDC_VERSION "1.3")
set(_AROS_SFDC_DATE "2004-11-12")

function(_aros_sfdc_real_path path output)
    set(_candidate "${path}")
    cmake_path(ABSOLUTE_PATH _candidate NORMALIZE OUTPUT_VARIABLE _candidate)
    set(_tail "")
    while(NOT EXISTS "${_candidate}" AND NOT IS_SYMLINK "${_candidate}")
        cmake_path(GET _candidate FILENAME _component)
        cmake_path(GET _candidate PARENT_PATH _parent)
        if(_component STREQUAL "" OR _parent STREQUAL _candidate)
            message(FATAL_ERROR "cannot resolve physical path ${path}")
        endif()
        list(PREPEND _tail "${_component}")
        set(_candidate "${_parent}")
    endwhile()
    if(IS_SYMLINK "${_candidate}" AND NOT EXISTS "${_candidate}")
        message(FATAL_ERROR "refusing dangling symlink in path ${path}")
    endif()
    file(REAL_PATH "${_candidate}" _resolved)
    foreach(_component IN LISTS _tail)
        set(_resolved "${_resolved}/${_component}")
    endforeach()
    cmake_path(NORMAL_PATH _resolved)
    set(${output} "${_resolved}" PARENT_SCOPE)
endfunction()

if(NOT DEFINED CONTRACT OR NOT EXISTS "${CONTRACT}" OR
   IS_DIRECTORY "${CONTRACT}" OR IS_SYMLINK "${CONTRACT}")
    message(FATAL_ERROR "RunSfdcHostTool requires a regular existing CONTRACT")
endif()
include("${CONTRACT}")

set(_required
    SFDC_MMAKE_ID SFDC_SOURCE_ROOT SFDC_BUILD_ROOT SFDC_SOURCE_DIR
    SFDC_BINARY_DIR SFDC_OUTPUT SFDC_INPUT_MANIFEST
    SFDC_INPUT_MANIFEST_SHA256 SFDC_PERL SFDC_VERSION SFDC_DATE
    SFDC_OUTPUT_SHA256 SFDC_INPUT_RELATIVE SFDC_INPUT_SHA256)
foreach(_name IN LISTS _required)
    if(NOT DEFINED ${_name} OR "${${_name}}" STREQUAL "")
        message(FATAL_ERROR "host-sfdc runner contract omits ${_name}")
    endif()
endforeach()
if(NOT SFDC_MMAKE_ID STREQUAL "host-sfdc" OR
   NOT SFDC_VERSION STREQUAL _AROS_SFDC_VERSION OR
   NOT SFDC_DATE STREQUAL _AROS_SFDC_DATE)
    message(FATAL_ERROR "host-sfdc runner contract differs from audited identity")
endif()
foreach(_name IN ITEMS SOURCE_ROOT BUILD_ROOT SOURCE_DIR BINARY_DIR OUTPUT
        INPUT_MANIFEST PERL)
    if("${SFDC_${_name}}" MATCHES "[;\"$\\\\\r\n]" OR
       "${SFDC_${_name}}" MATCHES "==\\]")
        message(FATAL_ERROR "host-sfdc runner contract has an unsafe ${_name}")
    endif()
endforeach()
if(NOT IS_ABSOLUTE "${SFDC_PERL}" OR SFDC_PERL MATCHES "[[:space:]]")
    message(FATAL_ERROR "host-sfdc runner requires an absolute safe PERL path")
endif()

foreach(_name IN ITEMS SOURCE_ROOT BUILD_ROOT SOURCE_DIR)
    if(NOT EXISTS "${SFDC_${_name}}" OR NOT IS_DIRECTORY "${SFDC_${_name}}")
        message(FATAL_ERROR "host-sfdc runner lacks directory ${_name}")
    endif()
endforeach()
if(NOT EXISTS "${SFDC_INPUT_MANIFEST}" OR IS_DIRECTORY "${SFDC_INPUT_MANIFEST}" OR
   IS_SYMLINK "${SFDC_INPUT_MANIFEST}")
    message(FATAL_ERROR "host-sfdc runner lacks a regular input manifest")
endif()
aros_path_is_executable("${SFDC_PERL}" _sfdc_perl_executable)
if(NOT _sfdc_perl_executable)
    message(FATAL_ERROR "host-sfdc runner lacks its declared Perl interpreter")
endif()

_aros_sfdc_real_path("${SFDC_SOURCE_ROOT}" _source_root)
_aros_sfdc_real_path("${SFDC_BUILD_ROOT}" _build_root)
_aros_sfdc_real_path("${SFDC_SOURCE_DIR}" _source_dir)
_aros_sfdc_real_path("${SFDC_BINARY_DIR}" _binary_dir)
_aros_sfdc_real_path("${SFDC_OUTPUT}" _output)
_aros_sfdc_real_path("${SFDC_INPUT_MANIFEST}" _manifest)
_aros_sfdc_real_path("${SFDC_PERL}" _perl)
cmake_path(IS_PREFIX _source_root "${_source_dir}" NORMALIZE _source_owned)
cmake_path(IS_PREFIX _source_dir "${_manifest}" NORMALIZE _manifest_owned)
cmake_path(IS_PREFIX _build_root "${_binary_dir}" NORMALIZE _binary_owned)
cmake_path(IS_PREFIX _build_root "${_output}" NORMALIZE _output_owned)
if(NOT _source_owned OR _source_dir STREQUAL _source_root OR
   NOT _manifest_owned OR NOT _binary_owned OR _binary_dir STREQUAL _build_root OR
   NOT _output_owned OR _output STREQUAL _build_root)
    message(FATAL_ERROR "host-sfdc runner contract escaped its owning tree")
endif()
set(_expected_source "${_source_root}/tools/sfdc")
set(_expected_manifest "${_expected_source}/sfdc-host.inputs")
set(_expected_binary "${_build_root}/gen/hosttools/sfdc")
set(_expected_output "${_build_root}/hosttools/sfdc")
foreach(_pair IN ITEMS
        "${_expected_source}|_expected_source"
        "${_expected_manifest}|_expected_manifest"
        "${_expected_binary}|_expected_binary"
        "${_expected_output}|_expected_output")
    string(REPLACE "|" ";" _parts "${_pair}")
    list(GET _parts 0 _path)
    list(GET _parts 1 _variable)
    _aros_sfdc_real_path("${_path}" _physical)
    set(${_variable} "${_physical}")
endforeach()
if(NOT _source_dir STREQUAL _expected_source OR
   NOT _manifest STREQUAL _expected_manifest OR
   NOT _binary_dir STREQUAL _expected_binary OR
   NOT _output STREQUAL _expected_output)
    message(FATAL_ERROR "host-sfdc runner contract differs from audited paths")
endif()

file(SHA256 "${_manifest}" _actual_manifest_sha256)
if(NOT _actual_manifest_sha256 STREQUAL SFDC_INPUT_MANIFEST_SHA256)
    message(FATAL_ERROR "host-sfdc input manifest changed after configuration; rerun CMake")
endif()
file(STRINGS "${_manifest}" _manifest_lines ENCODING UTF-8)
if(NOT _manifest_lines)
    message(FATAL_ERROR "host-sfdc input manifest is empty")
endif()
list(LENGTH SFDC_INPUT_RELATIVE _relative_count)
list(LENGTH SFDC_INPUT_SHA256 _hash_count)
if(NOT _manifest_lines STREQUAL SFDC_INPUT_RELATIVE OR
   NOT _relative_count EQUAL _hash_count)
    message(FATAL_ERROR
        "host-sfdc input inventory differs from the configuration snapshot; rerun CMake")
endif()

set(_manifest_paths "")
set(_output_content "#!${_perl} -w\n")
set(_input_index 0)
foreach(_line IN LISTS _manifest_lines)
    if(NOT _line MATCHES "^([A-Za-z0-9_.+/-]+)$")
        message(FATAL_ERROR "malformed host-sfdc input-manifest line '${_line}'")
    endif()
    set(_relative "${CMAKE_MATCH_1}")
    list(GET SFDC_INPUT_SHA256 ${_input_index} _digest)
    math(EXPR _input_index "${_input_index} + 1")
    string(LENGTH "${_digest}" _digest_length)
    if(NOT _digest_length EQUAL 64 OR IS_ABSOLUTE "${_relative}" OR
       _relative MATCHES "(^|/)[.][.]?(/|$)" OR
       _relative IN_LIST _manifest_paths)
        message(FATAL_ERROR "unsafe host-sfdc input-manifest line '${_line}'")
    endif()
    list(APPEND _manifest_paths "${_relative}")
    set(_input "${_source_dir}/${_relative}")
    cmake_path(NORMAL_PATH _input)
    if(NOT EXISTS "${_input}" OR IS_DIRECTORY "${_input}" OR IS_SYMLINK "${_input}")
        message(FATAL_ERROR "host-sfdc input ${_relative} is missing or symlinked")
    endif()
    _aros_sfdc_real_path("${_input}" _input_real)
    cmake_path(IS_PREFIX _source_dir "${_input_real}" NORMALIZE _input_owned)
    if(NOT _input_owned)
        message(FATAL_ERROR "host-sfdc input ${_relative} escaped its source tree")
    endif()
    file(SHA256 "${_input_real}" _actual_input_sha256)
    if(NOT _actual_input_sha256 STREQUAL _digest)
        message(FATAL_ERROR
            "host-sfdc input ${_relative} changed after configuration; rerun CMake")
    endif()
    file(READ "${_input_real}" _input_content)
    if(_relative STREQUAL "main.pl")
        string(REGEX REPLACE "^#![^\n]*\n" "" _input_content "${_input_content}")
    endif()
    string(REPLACE "SFDC_VERSION" "${_AROS_SFDC_VERSION}"
        _input_content "${_input_content}")
    string(REPLACE "SFDC_DATE" "${_AROS_SFDC_DATE}"
        _input_content "${_input_content}")
    string(APPEND _output_content "${_input_content}")
endforeach()
string(SHA256 _actual_output_sha256 "${_output_content}")
if(NOT _actual_output_sha256 STREQUAL SFDC_OUTPUT_SHA256)
    message(FATAL_ERROR "host-sfdc runner output contract is not reproducible")
endif()

# Creating only private build-tree paths is safe after the physical ownership
# checks above.  Refuse an output symlink rather than replacing a referent.
cmake_path(GET _output PARENT_PATH _output_parent)
file(MAKE_DIRECTORY "${_binary_dir}" "${_output_parent}")
_aros_sfdc_real_path("${_binary_dir}" _binary_dir_after)
_aros_sfdc_real_path("${_output_parent}" _output_parent_after)
cmake_path(IS_PREFIX _build_root "${_binary_dir_after}" NORMALIZE _binary_after_owned)
cmake_path(IS_PREFIX _build_root "${_output_parent_after}" NORMALIZE _output_parent_owned)
if(NOT _binary_after_owned OR NOT _output_parent_owned OR
   IS_SYMLINK "${_binary_dir}" OR IS_SYMLINK "${_output_parent}")
    message(FATAL_ERROR "host-sfdc private output path escaped through a symlink")
endif()
if(IS_SYMLINK "${_output}" OR IS_DIRECTORY "${_output}")
    message(FATAL_ERROR "host-sfdc output is not a regular private file")
endif()
# Do not overwrite an existing hard link in place: removing the owned output
# first keeps a malicious or stale hard link from modifying any source file.
file(REMOVE "${_output}")
file(WRITE "${_output}" "${_output_content}")
file(CHMOD "${_output}" PERMISSIONS
    OWNER_READ OWNER_WRITE OWNER_EXECUTE
    GROUP_READ GROUP_EXECUTE
    WORLD_READ WORLD_EXECUTE)
if(NOT EXISTS "${_output}" OR IS_DIRECTORY "${_output}" OR IS_SYMLINK "${_output}")
    message(FATAL_ERROR "host-sfdc did not create a regular output")
endif()
file(SHA256 "${_output}" _written_output_sha256)
if(NOT _written_output_sha256 STREQUAL SFDC_OUTPUT_SHA256)
    message(FATAL_ERROR "host-sfdc wrote a non-reproducible output")
endif()

execute_process(
    COMMAND "${_perl}" -c "${_output}"
    RESULT_VARIABLE _perl_result
    OUTPUT_VARIABLE _perl_stdout
    ERROR_VARIABLE _perl_stderr)
if(NOT _perl_result EQUAL 0)
    message(FATAL_ERROR
        "host-sfdc output failed its declared Perl interpreter (${_perl_result})\n"
        "${_perl_stdout}${_perl_stderr}")
endif()
