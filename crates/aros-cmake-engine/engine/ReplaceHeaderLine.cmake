cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED INPUT OR "${INPUT}" STREQUAL "" OR
   NOT DEFINED OUTPUT OR "${OUTPUT}" STREQUAL "" OR
   NOT DEFINED TOKEN OR "${TOKEN}" STREQUAL "" OR
   NOT DEFINED REPLACEMENT_FILE OR "${REPLACEMENT_FILE}" STREQUAL "" OR
   NOT DEFINED INPUT_ROOT OR "${INPUT_ROOT}" STREQUAL "" OR
   NOT DEFINED OUTPUT_ROOT OR "${OUTPUT_ROOT}" STREQUAL "" OR
   NOT DEFINED BINARY_ROOT OR "${BINARY_ROOT}" STREQUAL "")
    message(FATAL_ERROR
        "whole-line header transform requires INPUT, OUTPUT, TOKEN, "
        "REPLACEMENT_FILE, INPUT_ROOT, OUTPUT_ROOT and BINARY_ROOT")
endif()

# A semicolon is allowed: the token is only ever hex-encoded and compared as
# bytes, and source declarations match whole struct member lines such as
# `ThisTask;`. The characters below could end or extend a command or string.
foreach(_forbidden IN ITEMS "\r" "\n" "\"" "$" "`")
    string(FIND "${TOKEN}" "${_forbidden}" _forbidden_at)
    if(NOT _forbidden_at LESS 0)
        message(FATAL_ERROR "whole-line header transform: unsafe literal token")
    endif()
endforeach()

find_program(_regular_test NAMES test PATHS /usr/bin /bin
    NO_DEFAULT_PATH REQUIRED)

function(_normalize_safe_path label value out)
    if(NOT IS_ABSOLUTE "${value}" OR "${value}" MATCHES "[;\r\n\"]" OR
       "${value}" MATCHES "\\$")
        message(FATAL_ERROR
            "whole-line header transform: ${label} is not a safe absolute path: ${value}")
    endif()
    string(REPLACE "/" ";" _path_components "${value}")
    foreach(_component IN LISTS _path_components)
        if(_component STREQUAL "." OR _component STREQUAL "..")
            message(FATAL_ERROR
                "whole-line header transform: ${label} contains a dot path component: ${value}")
        endif()
    endforeach()
    cmake_path(NORMAL_PATH value OUTPUT_VARIABLE _normalized)
    if(_normalized STREQUAL "/")
        message(FATAL_ERROR
            "whole-line header transform: ${label} cannot be the filesystem root")
    endif()
    set(${out} "${_normalized}" PARENT_SCOPE)
endfunction()

function(_require_descendant label path root)
    cmake_path(IS_PREFIX root "${path}" NORMALIZE _inside)
    if(NOT _inside OR "${path}" STREQUAL "${root}")
        message(FATAL_ERROR
            "whole-line header transform: ${label} escapes its declared root")
    endif()
endfunction()

function(_reject_symlink_components label path)
    string(REPLACE "/" ";" _components "${path}")
    set(_probe "")
    foreach(_component IN LISTS _components)
        if(_component STREQUAL "")
            continue()
        endif()
        set(_probe "${_probe}/${_component}")
        if(IS_SYMLINK "${_probe}")
            message(FATAL_ERROR
                "whole-line header transform: ${label} crosses a symlink: ${_probe}")
        endif()
    endforeach()
endfunction()

function(_require_regular_file label path)
    execute_process(
        COMMAND "${_regular_test}" -f "${path}"
        RESULT_VARIABLE _regular_result
        OUTPUT_QUIET
        ERROR_QUIET
        TIMEOUT 10)
    if(NOT "${_regular_result}" STREQUAL "0")
        message(FATAL_ERROR
            "whole-line header transform: ${label} is not a regular file: ${path}")
    endif()
endfunction()

function(_hex_has_aligned_substring haystack needle out)
    string(LENGTH "${haystack}" _haystack_length)
    string(LENGTH "${needle}" _needle_length)
    set(_found FALSE)
    set(_cursor 0)
    while(_cursor LESS_EQUAL _haystack_length)
        math(EXPR _remaining "${_haystack_length} - ${_cursor}")
        if(_remaining LESS _needle_length)
            break()
        endif()
        string(SUBSTRING "${haystack}" ${_cursor} ${_needle_length} _candidate)
        if(_candidate STREQUAL needle)
            set(_found TRUE)
            break()
        endif()
        math(EXPR _cursor "${_cursor} + 2")
    endwhile()
    set(${out} "${_found}" PARENT_SCOPE)
endfunction()

function(_decode_hex_text label hex out)
    string(LENGTH "${hex}" _hex_length)
    if(_hex_length GREATER 134217728)
        message(FATAL_ERROR "whole-line header transform: decoded ${label} exceeds 64 MiB")
    endif()
    set(_decoded "")
    set(_cursor 0)
    while(_cursor LESS _hex_length)
        string(SUBSTRING "${hex}" ${_cursor} 2 _byte_hex)
        math(EXPR _byte "0x${_byte_hex}")
        if(_byte EQUAL 0)
            message(FATAL_ERROR "whole-line header transform: ${label} contains a NUL byte")
        endif()
        string(ASCII ${_byte} _character)
        string(APPEND _decoded "${_character}")
        math(EXPR _cursor "${_cursor} + 2")
    endwhile()
    set(${out} "${_decoded}" PARENT_SCOPE)
endfunction()

_normalize_safe_path("binary root" "${BINARY_ROOT}" _binary_root)
_normalize_safe_path("input root" "${INPUT_ROOT}" _input_root)
_normalize_safe_path("output root" "${OUTPUT_ROOT}" _output_root)
_normalize_safe_path("input" "${INPUT}" _input)
_normalize_safe_path("output" "${OUTPUT}" _output)
_normalize_safe_path("replacement file" "${REPLACEMENT_FILE}" _replacement_file)

foreach(_directory IN ITEMS _binary_root _input_root _output_root)
    if(NOT IS_DIRECTORY "${${_directory}}")
        message(FATAL_ERROR
            "whole-line header transform: ${_directory} is not an existing directory")
    endif()
    _reject_symlink_components("${_directory}" "${${_directory}}")
endforeach()

_require_descendant("output root" "${_output_root}" "${_binary_root}")
_require_descendant("output" "${_output}" "${_output_root}")
_require_descendant("output" "${_output}" "${_binary_root}")
_require_descendant("input" "${_input}" "${_input_root}")
_require_descendant("replacement file" "${_replacement_file}" "${_binary_root}")

foreach(_path IN ITEMS _input _replacement_file _output)
    _reject_symlink_components("${_path}" "${${_path}}")
endforeach()

get_filename_component(_output_directory "${_output}" DIRECTORY)
set(_existing_output_ancestor "${_output_directory}")
while(NOT IS_DIRECTORY "${_existing_output_ancestor}")
    get_filename_component(_parent_ancestor "${_existing_output_ancestor}" DIRECTORY)
    if(_parent_ancestor STREQUAL _existing_output_ancestor)
        message(FATAL_ERROR
            "whole-line header transform: output has no existing safe ancestor")
    endif()
    set(_existing_output_ancestor "${_parent_ancestor}")
endwhile()

_require_regular_file("input" "${_input}")
_require_regular_file("replacement file" "${_replacement_file}")
if(EXISTS "${_output}" OR IS_SYMLINK "${_output}")
    _require_regular_file("existing output" "${_output}")
endif()

# Resolve all existing roots and files after rejecting symlinks. The input
# root may be a selected source or fetch tree; output and binary roots must
# physically remain in the declared binary tree.
file(REAL_PATH "${_binary_root}" _binary_real)
file(REAL_PATH "${_input_root}" _input_root_real)
file(REAL_PATH "${_output_root}" _output_root_real)
file(REAL_PATH "${_input}" _input_real)
file(REAL_PATH "${_replacement_file}" _replacement_real)
file(REAL_PATH "${_existing_output_ancestor}" _existing_output_ancestor_real)
cmake_path(IS_PREFIX _input_root_real "${_input_real}" NORMALIZE _input_physically_inside)
cmake_path(IS_PREFIX _binary_real "${_output_root_real}" NORMALIZE _output_root_physically_inside)
cmake_path(IS_PREFIX _output_root_real "${_existing_output_ancestor_real}" NORMALIZE _output_physically_inside)
cmake_path(IS_PREFIX _binary_real "${_existing_output_ancestor_real}" NORMALIZE _output_binary_physically_inside)
cmake_path(IS_PREFIX _binary_real "${_replacement_real}" NORMALIZE _replacement_physically_inside)
if(NOT _input_physically_inside OR NOT _output_root_physically_inside OR
   NOT _output_physically_inside OR NOT _output_binary_physically_inside OR
   NOT _replacement_physically_inside)
    message(FATAL_ERROR
        "whole-line header transform: a physical path escapes its declared root")
endif()
if(EXISTS "${_output}")
    file(REAL_PATH "${_output}" _output_real)
    cmake_path(IS_PREFIX _output_root_real "${_output_real}" NORMALIZE _output_file_physically_inside)
    if(NOT _output_file_physically_inside)
        message(FATAL_ERROR
            "whole-line header transform: existing output escapes its physical root")
    endif()
endif()

file(SIZE "${_input}" _input_size)
if(_input_size GREATER 33554432)
    message(FATAL_ERROR "whole-line header transform: input exceeds 32 MiB")
endif()
file(SIZE "${_replacement_file}" _replacement_size)
if(_replacement_size GREATER 1048576)
    message(FATAL_ERROR "whole-line header transform: replacement exceeds 1 MiB")
endif()

# Read as hex so CMake's text-string handling cannot normalize CRLF bytes.
# Header data is text; NUL bytes are refused by the decoder before publication.
file(READ "${_input}" _input_hex HEX)
file(READ "${_replacement_file}" _replacement_hex HEX)
string(TOLOWER "${_input_hex}" _input_hex)
string(TOLOWER "${_replacement_hex}" _replacement_hex)
string(HEX "${TOKEN}" _token_hex)
string(TOLOWER "${_token_hex}" _token_hex)
string(LENGTH "${_input_hex}" _input_hex_length)
math(EXPR _input_length "${_input_hex_length} / 2")
set(_cursor 0)
set(_transformed_hex "")
while(_cursor LESS _input_length)
    math(EXPR _line_hex_start "${_cursor} * 2")
    set(_line_byte_length 0)
    set(_newline_found FALSE)
    math(EXPR _remaining_input "${_input_length} - ${_cursor}")
    while(_line_byte_length LESS _remaining_input)
        math(EXPR _byte_at "(${_cursor} + ${_line_byte_length}) * 2")
        string(SUBSTRING "${_input_hex}" ${_byte_at} 2 _byte_hex)
        if(_byte_hex STREQUAL "0a")
            set(_newline_found TRUE)
            break()
        endif()
        math(EXPR _line_byte_length "${_line_byte_length} + 1")
    endwhile()
    math(EXPR _line_hex_length "${_line_byte_length} * 2")
    string(SUBSTRING "${_input_hex}" ${_line_hex_start} ${_line_hex_length} _line_hex)
    if(_newline_found)
        set(_terminator_hex "0a")
        math(EXPR _next_cursor "${_cursor} + ${_line_byte_length} + 1")
    else()
        set(_terminator_hex "")
        set(_next_cursor "${_input_length}")
    endif()
    _hex_has_aligned_substring("${_line_hex}" "${_token_hex}" _line_has_token)
    if(_line_has_token)
        string(APPEND _transformed_hex "${_replacement_hex}${_terminator_hex}")
    else()
        string(APPEND _transformed_hex "${_line_hex}${_terminator_hex}")
    endif()
    set(_cursor "${_next_cursor}")
endwhile()

string(LENGTH "${_transformed_hex}" _transformed_hex_length)
math(EXPR _transformed_length "${_transformed_hex_length} / 2")
if(_transformed_length GREATER 67108864)
    message(FATAL_ERROR "whole-line header transform: output exceeds 64 MiB")
endif()

if(EXISTS "${_output}")
    file(READ "${_output}" _old_output_hex HEX)
    string(TOLOWER "${_old_output_hex}" _old_output_hex)
    if(_old_output_hex STREQUAL _transformed_hex)
        return()
    endif()
endif()

_decode_hex_text("output" "${_transformed_hex}" _transformed)

file(MAKE_DIRECTORY "${_output_directory}")
_reject_symlink_components("output" "${_output}")
file(REAL_PATH "${_output_directory}" _output_directory_real)
cmake_path(IS_PREFIX _output_root_real "${_output_directory_real}" NORMALIZE _output_parent_physically_inside)
cmake_path(IS_PREFIX _binary_real "${_output_directory_real}" NORMALIZE _output_parent_binary_physically_inside)
if(NOT _output_parent_physically_inside OR NOT _output_parent_binary_physically_inside)
    message(FATAL_ERROR
        "whole-line header transform: created output directory escapes its physical root")
endif()

string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _temporary_suffix)
set(_temporary "${_output}.${_temporary_suffix}.tmp")
if(EXISTS "${_temporary}" OR IS_SYMLINK "${_temporary}")
    message(FATAL_ERROR "whole-line header transform: temporary path already exists")
endif()
file(WRITE "${_temporary}" "${_transformed}")
_reject_symlink_components("output" "${_output}")
file(RENAME "${_temporary}" "${_output}" RESULT _rename_result)
if(NOT "${_rename_result}" STREQUAL "0")
    file(REMOVE "${_temporary}")
    message(FATAL_ERROR
        "whole-line header transform: atomic output publication failed: ${_rename_result}")
endif()
