cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_fixture "${_temp_root}/aros-substitute-header-${_suffix}")
file(MAKE_DIRECTORY "${_fixture}")

set(INPUT "${_fixture}/input.h")
set(OUTPUT "${_fixture}/output.h")
set(SUBSTITUTIONS
    "@MAJOR_VERSION@" "1"
    "@MINOR_VERSION@" "23"
    "@PATCH_VERSION@" "4")
file(WRITE "${INPUT}"
    "#define VERSION @MAJOR_VERSION@.@MINOR_VERSION@.@PATCH_VERSION@\n")
include("${CMAKE_CURRENT_LIST_DIR}/../SubstituteHeader.cmake")
file(READ "${OUTPUT}" _actual)
set(_expected "#define VERSION 1.23.4\n")
if(NOT _actual STREQUAL _expected)
    message(FATAL_ERROR
        "literal template substitution mismatch:\nexpected=${_expected}\nactual=${_actual}")
endif()

file(REMOVE_RECURSE "${_fixture}")
message(STATUS "literal substituted-header test passed")
