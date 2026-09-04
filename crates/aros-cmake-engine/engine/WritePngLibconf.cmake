cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED BINARY_ROOT OR NOT DEFINED INPUT OR NOT DEFINED OUTPUT)
    message(FATAL_ERROR
        "WritePngLibconf.cmake requires BINARY_ROOT, INPUT and OUTPUT")
endif()

cmake_path(ABSOLUTE_PATH BINARY_ROOT NORMALIZE OUTPUT_VARIABLE _binary_root)
cmake_path(ABSOLUTE_PATH INPUT NORMALIZE OUTPUT_VARIABLE _input)
cmake_path(ABSOLUTE_PATH OUTPUT NORMALIZE OUTPUT_VARIABLE _output)
cmake_path(IS_PREFIX _binary_root "${_input}" NORMALIZE _input_inside)
cmake_path(IS_PREFIX _binary_root "${_output}" NORMALIZE _output_inside)
if(NOT _input_inside OR NOT _output_inside OR _input STREQUAL _output)
    message(FATAL_ERROR "pnglibconf input or output escapes the build tree")
endif()
if(NOT EXISTS "${_input}" OR IS_DIRECTORY "${_input}")
    message(FATAL_ERROR "pnglibconf input does not exist: ${_input}")
endif()

# The fetched archive is integrity-checked before this writer runs.  Reject a
# symlink that points outside the binary tree nevertheless, so the static
# staging rule cannot unexpectedly read a host header.
file(REAL_PATH "${_binary_root}" _real_binary_root)
file(REAL_PATH "${_input}" _real_input)
cmake_path(IS_PREFIX _real_binary_root "${_real_input}" NORMALIZE _real_inside)
if(NOT _real_inside)
    message(FATAL_ERROR "pnglibconf input resolves outside the build tree")
endif()

file(READ "${_input}" _content)
set(_token "PNG_ERROR_NUMBERS_SUPPORTED")
string(FIND "${_content}" "${_token}" _match_at)
if(_match_at LESS 0)
    message(FATAL_ERROR
        "pnglibconf input has no ${_token} line: ${_input}")
endif()
string(LENGTH "${_token}" _token_length)
math(EXPR _after_token "${_match_at} + ${_token_length}")
string(SUBSTRING "${_content}" "${_after_token}" -1 _remaining)
string(FIND "${_remaining}" "${_token}" _second_match)
if(NOT _second_match LESS 0)
    message(FATAL_ERROR
        "pnglibconf input has more than one ${_token} line: ${_input}")
endif()

# Mirror sed's `.*TOKEN.*` match without turning the rest of this historical
# rule into a generic Make parser: replace exactly the one complete input line
# that carries the token and preserve all other bytes, including its newline.
string(SUBSTRING "${_content}" 0 "${_match_at}" _before_token)
string(FIND "${_before_token}" "\n" _line_break REVERSE)
if(_line_break LESS 0)
    set(_line_start 0)
else()
    math(EXPR _line_start "${_line_break} + 1")
endif()
string(SUBSTRING "${_content}" "${_match_at}" -1 _from_token)
string(FIND "${_from_token}" "\n" _line_end_relative)
if(_line_end_relative LESS 0)
    string(LENGTH "${_content}" _line_end)
else()
    math(EXPR _line_end "${_match_at} + ${_line_end_relative}")
endif()
string(SUBSTRING "${_content}" 0 "${_line_start}" _prefix)
string(SUBSTRING "${_content}" "${_line_end}" -1 _suffix)
set(_replacement [=[#if defined(__AROS__)
#define PNG_ERROR_NUMBERS_SUPPORTED
#else
/*#undef PNG_ERROR_NUMBERS_SUPPORTED*/
#endif]=])
set(_transformed "${_prefix}${_replacement}${_suffix}")

get_filename_component(_output_dir "${_output}" DIRECTORY)
file(MAKE_DIRECTORY "${_output_dir}")
string(SHA256 _temporary_key "${_output}")
set(_temporary "${_output}.${_temporary_key}.tmp")
file(WRITE "${_temporary}" "${_transformed}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${_temporary}" "${_output}"
    COMMAND_ERROR_IS_FATAL ANY)
file(REMOVE "${_temporary}")
