cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_fixture "${_temp_root}/aros-transform-header-${_suffix}")
file(MAKE_DIRECTORY "${_fixture}")

set(INPUT "${_fixture}/input.h")
set(OUTPUT "${_fixture}/output.h")
set(MATCH_TEXT "literal")
set(REPLACEMENT "changed")
file(WRITE "${INPUT}"
    "literal\nprefix literal remains\nliteral suffix\n")
include("${CMAKE_CURRENT_LIST_DIR}/../TransformHeader.cmake")
file(READ "${OUTPUT}" _actual)
set(_expected "changed\nprefix literal remains\nchanged suffix\n")
if(NOT _actual STREQUAL _expected)
    message(FATAL_ERROR
        "line-anchored transform mismatch:\nexpected=${_expected}\nactual=${_actual}")
endif()

file(REMOVE_RECURSE "${_fixture}")
message(STATUS "literal transformed-header test passed")
