# Implements the exact four `sed` expressions in
# workbench/libs/freetype2/mmakefile.src.  Their patterns intentionally replace
# whole lines, including a documentation line mentioning SYSTEM_ZLIB, and
# append a newline in the replacement.  Retaining that detail makes the output
# match the historic rule rather than merely enabling a similar set of macros.

# This file runs through `cmake -P`, so it does not inherit the policy scope
# established by the top-level project.  Declare the supported baseline here;
# otherwise older CMake releases evaluate `while(TRUE)` using CMP0012's legacy
# behaviour and silently skip every replacement.
cmake_minimum_required(VERSION 3.22)

foreach(_required IN ITEMS
        AROS_FREETYPE_OPTIONS_INPUT
        AROS_FREETYPE_OPTIONS_OUTPUT)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "WriteFreetypeOptions.cmake requires ${_required}")
    endif()
endforeach()
if(NOT EXISTS "${AROS_FREETYPE_OPTIONS_INPUT}" OR
   IS_DIRECTORY "${AROS_FREETYPE_OPTIONS_INPUT}")
    message(FATAL_ERROR
        "FreeType option input does not exist: ${AROS_FREETYPE_OPTIONS_INPUT}")
endif()

# Process from the final matching line backwards.  The replacement text itself
# contains the token it replaces, so moving the already-written suffix aside
# prevents a second pass from treating generated text as source input.
function(_aros_replace_freetype_option_lines input token replacement out count_out)
    set(_prefix "${input}")
    set(_suffix "")
    set(_count 0)
    while(TRUE)
        string(FIND "${_prefix}" "${token}" _token_at REVERSE)
        if(_token_at EQUAL -1)
            break()
        endif()

        string(SUBSTRING "${_prefix}" 0 "${_token_at}" _before_token)
        string(FIND "${_before_token}" "\n" _line_before REVERSE)
        if(_line_before EQUAL -1)
            set(_line_start 0)
        else()
            math(EXPR _line_start "${_line_before} + 1")
        endif()
        string(SUBSTRING "${_prefix}" 0 "${_line_start}" _before_line)

        string(SUBSTRING "${_prefix}" "${_token_at}" -1 _from_token)
        string(FIND "${_from_token}" "\n" _line_after)
        if(_line_after EQUAL -1)
            set(_after_line "")
        else()
            # Keep the source line terminator.  The historic sed replacement
            # supplies one as well, so every matched line intentionally leaves
            # one blank line after its replacement.
            string(SUBSTRING "${_from_token}" "${_line_after}" -1 _after_line)
        endif()

        set(_suffix "${replacement}\n${_after_line}${_suffix}")
        set(_prefix "${_before_line}")
        math(EXPR _count "${_count} + 1")
    endwhile()
    set(${out} "${_prefix}${_suffix}" PARENT_SCOPE)
    set(${count_out} "${_count}" PARENT_SCOPE)
endfunction()

file(READ "${AROS_FREETYPE_OPTIONS_INPUT}" _options)
foreach(_spec IN ITEMS
        "FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES|/*define FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES*/"
        "FT_CONFIG_OPTION_SUBPIXEL_RENDERING|#define FT_CONFIG_OPTION_SUBPIXEL_RENDERING"
        "FT_CONFIG_OPTION_SYSTEM_ZLIB|#define FT_CONFIG_OPTION_SYSTEM_ZLIB"
        "FT_CONFIG_OPTION_USE_PNG|#define FT_CONFIG_OPTION_USE_PNG")
    string(REPLACE "|" ";" _parts "${_spec}")
    list(GET _parts 0 _token)
    list(GET _parts 1 _replacement)
    _aros_replace_freetype_option_lines(
        "${_options}" "${_token}" "${_replacement}" _options _replacements)
    if(_replacements EQUAL 0)
        message(FATAL_ERROR
            "FreeType option input has no line containing ${_token}: "
            "${AROS_FREETYPE_OPTIONS_INPUT}")
    endif()
endforeach()

get_filename_component(_output_dir "${AROS_FREETYPE_OPTIONS_OUTPUT}" DIRECTORY)
file(MAKE_DIRECTORY "${_output_dir}")
string(SHA256 _output_hash "${AROS_FREETYPE_OPTIONS_OUTPUT}")
string(SUBSTRING "${_output_hash}" 0 16 _output_hash)
set(_temporary "${AROS_FREETYPE_OPTIONS_OUTPUT}.tmp-${_output_hash}")
file(WRITE "${_temporary}" "${_options}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${_temporary}" "${AROS_FREETYPE_OPTIONS_OUTPUT}"
    COMMAND_ERROR_IS_FATAL ANY)
file(REMOVE "${_temporary}")
