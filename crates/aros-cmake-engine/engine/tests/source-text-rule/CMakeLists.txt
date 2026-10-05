cmake_minimum_required(VERSION 3.22)
project(SourceTextRuleFixture NONE)

if(NOT DEFINED AROS_ENGINE_DIR OR NOT DEFINED AROS_SOURCE_TEXT_FIXTURE_DIR)
    message(FATAL_ERROR "source text fixture requires engine and fixture directories")
endif()

set(_developer_root "${CMAKE_BINARY_DIR}/SYS/Developer/include")
set(_default_gen_root "${CMAKE_BINARY_DIR}/GENINCDIR")
set(_source_gen_root "${CMAKE_BINARY_DIR}/gen/include")
set(_sdk_root "${CMAKE_BINARY_DIR}/SDK/include")
set(_hosttools_root "${CMAKE_BINARY_DIR}/hosttools")
set(_fetch_root "${CMAKE_BINARY_DIR}/Ports/freetype")
if(DEFINED AROS_SOURCE_TEXT_TEST_OUTSIDE_ROOT AND
   NOT "${AROS_SOURCE_TEXT_TEST_OUTSIDE_ROOT}" STREQUAL "")
    set(_outside_root "${AROS_SOURCE_TEXT_TEST_OUTSIDE_ROOT}")
else()
    set(_outside_root "${CMAKE_BINARY_DIR}/outside")
endif()
set(AROS_DEVELOPER_INCLUDE_DIR "${_developer_root}")
set(AROS_SDK_INCLUDE_DIR "${_sdk_root}")
set(AROS_GENINC_DIR "${_default_gen_root}")
if(AROS_SOURCE_TEXT_INCLUDE_ROOT_ALIAS)
    set(AROS_SDK_INCLUDE_DIR "${_developer_root}")
endif()
set(_configured_sdk_root "${AROS_SDK_INCLUDE_DIR}")

if(AROS_SOURCE_TEXT_PROBE STREQUAL "configured-root-symlink")
    file(MAKE_DIRECTORY "${_outside_root}/gen/include")
    set(AROS_GENINC_DIR "${CMAKE_BINARY_DIR}/redirect/gen/include")
    file(CREATE_LINK "${_outside_root}" "${CMAKE_BINARY_DIR}/redirect"
        SYMBOLIC RESULT _root_link_result)
    if(NOT _root_link_result STREQUAL "0")
        message(FATAL_ERROR "could not create configured-root symlink: ${_root_link_result}")
    endif()
elseif(AROS_SOURCE_TEXT_PROBE STREQUAL "overlapping-generated-roots")
    set(AROS_GENINC_DIR "${CMAKE_BINARY_DIR}/gen")
    file(MAKE_DIRECTORY "${AROS_GENINC_DIR}/include")
else()
    file(MAKE_DIRECTORY "${_default_gen_root}")
endif()
if(AROS_SOURCE_TEXT_GEN_ROOT_ALIAS)
    set(AROS_GENINC_DIR "${_source_gen_root}")
endif()
set(_configured_gen_root "${AROS_GENINC_DIR}")
file(MAKE_DIRECTORY
    "${_developer_root}" "${_source_gen_root}" "${_sdk_root}" "${_hosttools_root}")
file(MAKE_DIRECTORY "${_fetch_root}")

set(_stamp "${_fetch_root}/.complete")
set(_header_input "${_fetch_root}/include/config/ftoption.h.in")
set(_helper_input "${_fetch_root}/builds/unix/cross-helper.in")
set(_configured_gen_input "${_fetch_root}/include/config/generated.in")
set(_source_gen_input "${_fetch_root}/include/config/source-generated.in")
set(_sdk_input "${_fetch_root}/include/config/sdk.in")
set(_aligned_input "${_fetch_root}/include/config/aligned.in")
set(_line_search_input "${_fetch_root}/include/config/line-search.in")
if(NOT AROS_SOURCE_TEXT_PROBE)
    add_custom_command(
        OUTPUT "${_stamp}"
        COMMAND "${CMAKE_COMMAND}"
            "-DFETCH_ROOT=${_fetch_root}"
            -P "${AROS_SOURCE_TEXT_FIXTURE_DIR}/CreateInputs.cmake"
        DEPENDS "${AROS_SOURCE_TEXT_FIXTURE_DIR}/CreateInputs.cmake"
        COMMENT "Fetching source text fixture files"
        VERBATIM)
    add_custom_target(fetch-freetype DEPENDS "${_stamp}")
else()
    add_custom_target(fetch-freetype)
endif()
set_property(TARGET fetch-freetype PROPERTY AROS_FETCH_DESTINATION "${_fetch_root}")
set_property(TARGET fetch-freetype PROPERTY AROS_FETCH_COMPLETION_STAMP "${_stamp}")

include("${AROS_ENGINE_DIR}/SourceTextRules.cmake")

set(_simple_operations [=[
[
  {"kind":"replace_all","token":"@CONFIG@","replacement":"source-local-config"}
]
]=])

if(AROS_SOURCE_TEXT_PROBE)
    if(AROS_SOURCE_TEXT_PROBE STREQUAL "wrong-mode")
        set(_output "${_hosttools_root}/cross-helper")
        file(WRITE "${_output}" "old product")
        aros_transform_source_text(
            NAME probe INPUT "${_helper_input}" OUTPUT "${_output}"
            FETCH fetch-freetype OPERATIONS_JSON "${_simple_operations}" MODE 755)
    elseif(AROS_SOURCE_TEXT_PROBE STREQUAL "malformed-operations")
        set(_output "${_configured_gen_root}/old.h")
        file(WRITE "${_output}" "old product")
        set(_malformed [=[[{"kind":"replace_all","token":"x","replacement":"y","extra":"z"}]]=])
        aros_transform_source_text(
            NAME probe INPUT "${_configured_gen_input}" OUTPUT "${_output}"
            FETCH fetch-freetype OPERATIONS_JSON "${_malformed}")
    elseif(AROS_SOURCE_TEXT_PROBE STREQUAL "output-symlink")
        file(MAKE_DIRECTORY "${_outside_root}")
        file(WRITE "${_outside_root}/preserved.h" "old product")
        file(CREATE_LINK "${_outside_root}" "${_default_gen_root}/redirect"
            SYMBOLIC RESULT _link_result)
        if(NOT _link_result STREQUAL "0")
            message(FATAL_ERROR "could not create output symlink: ${_link_result}")
        endif()
        aros_transform_source_text(
            NAME probe INPUT "${_configured_gen_input}"
            OUTPUT "${_configured_gen_root}/redirect/preserved.h"
            FETCH fetch-freetype OPERATIONS_JSON "${_simple_operations}")
    elseif(AROS_SOURCE_TEXT_PROBE STREQUAL "configured-root-symlink")
        file(WRITE "${_outside_root}/gen/include/preserved.h" "old product")
        aros_transform_source_text(
            NAME probe INPUT "${_configured_gen_input}"
            OUTPUT "${AROS_GENINC_DIR}/preserved.h"
            FETCH fetch-freetype OPERATIONS_JSON "${_simple_operations}")
    elseif(AROS_SOURCE_TEXT_PROBE STREQUAL "duplicate-output")
        set(_output "${_configured_gen_root}/duplicate.h")
        file(WRITE "${_output}" "old product")
        aros_transform_source_text(
            NAME probe INPUT "${_configured_gen_input}" OUTPUT "${_output}"
            FETCH fetch-freetype OPERATIONS_JSON "${_simple_operations}")
        aros_transform_source_text(
            NAME probe INPUT "${_configured_gen_input}" OUTPUT "${_output}"
            FETCH fetch-freetype OPERATIONS_JSON "${_simple_operations}")
    elseif(AROS_SOURCE_TEXT_PROBE STREQUAL "outside-output")
        set(_output "${_outside_root}/outside.h")
        file(WRITE "${_output}" "old product")
        aros_transform_source_text(
            NAME probe INPUT "${_configured_gen_input}" OUTPUT "${_output}"
            FETCH fetch-freetype OPERATIONS_JSON "${_simple_operations}")
    elseif(AROS_SOURCE_TEXT_PROBE STREQUAL "overlapping-generated-roots")
        set(_output "${_configured_gen_root}/include/preserved.h")
        file(WRITE "${_output}" "old product")
        aros_transform_source_text(
            NAME probe INPUT "${_configured_gen_input}" OUTPUT "${_output}"
            FETCH fetch-freetype OPERATIONS_JSON "${_simple_operations}")
    elseif(AROS_SOURCE_TEXT_PROBE STREQUAL "fifo-input")
        set(_output "${_configured_gen_root}/old.h")
        file(WRITE "${_output}" "old product")
        file(MAKE_DIRECTORY "${_fetch_root}/include/config")
        find_program(_mkfifo NAMES mkfifo PATHS /usr/bin /bin NO_DEFAULT_PATH REQUIRED)
        execute_process(COMMAND "${_mkfifo}" "${_configured_gen_input}"
            RESULT_VARIABLE _mkfifo_result)
        if(NOT _mkfifo_result EQUAL 0)
            message(FATAL_ERROR "could not create input FIFO")
        endif()
        aros_transform_source_text(
            NAME probe INPUT "${_configured_gen_input}" OUTPUT "${_output}"
            FETCH fetch-freetype OPERATIONS_JSON "${_simple_operations}")
    else()
        message(FATAL_ERROR "unknown source text fixture probe ${AROS_SOURCE_TEXT_PROBE}")
    endif()
    return()
endif()

set(_header_operations [=[
[
  {"kind":"replace_whole_line_containing","token":"FIRST","replacement":"#define SECOND intermediate"},
  {"kind":"replace_whole_line_containing","token":"SECOND","replacement":"#define SECOND final"},
  {"kind":"replace_whole_line_containing","token":"FT_CONFIG_OPTION_USE_PNG","replacement":"#define FT_CONFIG_OPTION_USE_PNG"},
  {"kind":"replace_whole_line_containing","token":"FT_CONFIG_OPTION_USE_BZIP2","replacement":"#define FT_CONFIG_OPTION_USE_BZIP2"}
]
]=])
set(_helper_operations [=[
[
  {"kind":"replace_all","token":"@CROSS@","replacement":"/Developer/bin/cross-tool"},
  {"kind":"replace_all","token":"@SYSROOT@","replacement":"${AROS_SYSROOT}"},
  {"kind":"replace_all","token":"@SEMI@","replacement":"one;two"}
]
]=])
set(_aligned_operations [=[
[
  {"kind":"replace_all","token":"AB","replacement":"X"}
]
]=])
set(_line_search_operations [=[
[
  {"kind":"replace_whole_line_containing","token":"AB","replacement":"DONE"}
]
]=])

aros_transform_source_text(
    NAME freetype-source-text
    INPUT "${_header_input}"
    OUTPUT "${_developer_root}/freetype/config/ftoption.h"
    FETCH fetch-freetype
    OPERATIONS_JSON "${_header_operations}")
aros_transform_source_text(
    NAME freetype-source-text
    INPUT "${_helper_input}"
    OUTPUT "${_hosttools_root}/cross-helper"
    FETCH fetch-freetype
    OPERATIONS_JSON "${_helper_operations}"
    MODE 744)
aros_transform_source_text(
    NAME freetype-source-text
    INPUT "${_configured_gen_input}"
    OUTPUT "${_configured_gen_root}/config/generated.h"
    FETCH fetch-freetype
    OPERATIONS_JSON "${_simple_operations}")
aros_transform_source_text(
    NAME freetype-source-text
    INPUT "${_source_gen_input}"
    OUTPUT "${_source_gen_root}/config/source-generated.h"
    FETCH fetch-freetype
    OPERATIONS_JSON "${_simple_operations}")
aros_transform_source_text(
    NAME freetype-source-text
    INPUT "${_sdk_input}"
    OUTPUT "${_configured_sdk_root}/freetype/config/sdk-option.h"
    FETCH fetch-freetype
    OPERATIONS_JSON "${_simple_operations}")
aros_transform_source_text(
    NAME freetype-source-text
    INPUT "${_aligned_input}"
    OUTPUT "${_configured_gen_root}/config/aligned.bin"
    FETCH fetch-freetype
    OPERATIONS_JSON "${_aligned_operations}")
aros_transform_source_text(
    NAME freetype-source-text
    INPUT "${_line_search_input}"
    OUTPUT "${_configured_gen_root}/config/line-search.bin"
    FETCH fetch-freetype
    OPERATIONS_JSON "${_line_search_operations}")

add_custom_target(all-source-text)
add_dependencies(all-source-text freetype-source-text)
