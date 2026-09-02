cmake_minimum_required(VERSION 3.22)

# Implements the two exact version-header substitutions in
# workbench/classes/datatypes/heic/mmakefile.src.  These headers are private
# to the fetched ports, so their values deliberately remain tied to the
# corresponding archive versions.

foreach(_required IN ITEMS
        AROS_HEIC_VERSION_KIND
        AROS_HEIC_VERSION_INPUT
        AROS_HEIC_VERSION_OUTPUT)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "WriteHeicVersionHeader.cmake requires ${_required}")
    endif()
endforeach()
if(NOT EXISTS "${AROS_HEIC_VERSION_INPUT}" OR
   IS_DIRECTORY "${AROS_HEIC_VERSION_INPUT}")
    message(FATAL_ERROR
        "HEIC version-header input does not exist: ${AROS_HEIC_VERSION_INPUT}")
endif()

function(_aros_replace_heic_version_token input token replacement out)
    string(FIND "${input}" "${token}" _token_offset)
    if(_token_offset EQUAL -1)
        message(FATAL_ERROR
            "HEIC version-header input has no ${token} token: "
            "${AROS_HEIC_VERSION_INPUT}")
    endif()
    string(REPLACE "${token}" "${replacement}" _result "${input}")
    set(${out} "${_result}" PARENT_SCOPE)
endfunction()

file(READ "${AROS_HEIC_VERSION_INPUT}" _header)
if(AROS_HEIC_VERSION_KIND STREQUAL "DE265")
    # LIBDE265VERSION=1.1.1 and DE265_NUMERIC_VERSION=0x01010100 in the
    # MetaMake recipe. The template supplies the surrounding quotes.
    _aros_replace_heic_version_token(
        "${_header}" "@NUMERIC_VERSION@" "0x01010100" _header)
    _aros_replace_heic_version_token(
        "${_header}" "@PACKAGE_VERSION@" "1.1.1" _header)
elseif(AROS_HEIC_VERSION_KIND STREQUAL "HEIF")
    # LIBHEIFVERSION=1.23.1. The historic recipe deliberately leaves the
    # plugin directory empty for the statically linked AROS datatype.
    _aros_replace_heic_version_token(
        "${_header}" "@PROJECT_VERSION_MAJOR@" "1" _header)
    _aros_replace_heic_version_token(
        "${_header}" "@PROJECT_VERSION_MINOR@" "23" _header)
    _aros_replace_heic_version_token(
        "${_header}" "@PROJECT_VERSION_PATCH@" "1" _header)
    _aros_replace_heic_version_token(
        "${_header}" "@PLUGIN_DIRECTORY@" "" _header)
else()
    message(FATAL_ERROR
        "WriteHeicVersionHeader.cmake received unsupported kind: "
        "${AROS_HEIC_VERSION_KIND}")
endif()

get_filename_component(_output_dir "${AROS_HEIC_VERSION_OUTPUT}" DIRECTORY)
file(MAKE_DIRECTORY "${_output_dir}")
string(SHA256 _output_hash "${AROS_HEIC_VERSION_OUTPUT}")
string(SUBSTRING "${_output_hash}" 0 16 _output_hash)
set(_temporary "${AROS_HEIC_VERSION_OUTPUT}.tmp-${_output_hash}")
file(WRITE "${_temporary}" "${_header}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${_temporary}" "${AROS_HEIC_VERSION_OUTPUT}"
    COMMAND_ERROR_IS_FATAL ANY)
file(REMOVE "${_temporary}")
