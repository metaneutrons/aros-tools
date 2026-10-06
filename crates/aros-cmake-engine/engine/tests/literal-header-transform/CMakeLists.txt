cmake_minimum_required(VERSION 3.22)
project(LiteralHeaderTransformFixture NONE)

if(NOT DEFINED AROS_ENGINE_DIR OR "${AROS_ENGINE_DIR}" STREQUAL "")
    message(FATAL_ERROR "AROS_ENGINE_DIR is required")
endif()

set(_runner "${AROS_ENGINE_DIR}/ReplaceHeaderLine.cmake")
set(_input_root "${CMAKE_BINARY_DIR}/inputs")
set(_output_root "${CMAKE_BINARY_DIR}/SDK/include")
file(MAKE_DIRECTORY "${_input_root}" "${_output_root}")

set(_input [=[first TOKEN on a line
TOKEN on its own
preserve this line
final TOKEN without newline]=])
file(WRITE "${_input_root}/input.h" "${_input}")
file(WRITE "${_input_root}/no-match.h" "no match\r\nsecond line")
set(_replacement [=[#define FIRST;
#define SECOND ${prefix}]=])
file(WRITE "${CMAKE_BINARY_DIR}/replacement.txt" "${_replacement}")
file(WRITE "${_output_root}/sentinel.h" "old output must remain intact\n")

function(_add_transform name input output token)
    add_custom_command(
        OUTPUT "${output}"
        COMMAND "${CMAKE_COMMAND}"
            "-DINPUT=${input}"
            "-DOUTPUT=${output}"
            "-DTOKEN=${token}"
            "-DREPLACEMENT_FILE=${CMAKE_BINARY_DIR}/replacement.txt"
            "-DINPUT_ROOT=${_input_root}"
            "-DOUTPUT_ROOT=${_output_root}"
            "-DBINARY_ROOT=${CMAKE_BINARY_DIR}"
            -P "${_runner}"
        DEPENDS "${input}" "${CMAKE_BINARY_DIR}/replacement.txt" "${_runner}"
        COMMENT "Testing literal whole-line header transform ${name}"
        VERBATIM)
endfunction()

set(_positive_output "${_output_root}/nested/positive.h")
set(_no_match_output "${_output_root}/nested/no-match.h")
_add_transform(positive "${_input_root}/input.h" "${_positive_output}" TOKEN)
_add_transform(no-match "${_input_root}/no-match.h" "${_no_match_output}" TOKEN)
add_custom_target(literal-header-transform DEPENDS
    "${_positive_output}" "${_no_match_output}")
