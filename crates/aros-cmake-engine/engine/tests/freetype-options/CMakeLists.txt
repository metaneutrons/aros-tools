cmake_minimum_required(VERSION 3.22)
project(FreetypeOptions C)

set(AROS_PORTS_DIR "${CMAKE_CURRENT_BINARY_DIR}/Ports")
set(AROS_SDK_INCLUDE_DIR "${CMAKE_CURRENT_BINARY_DIR}/SDK/include")
include("${CMAKE_CURRENT_LIST_DIR}/../../FreetypeOptions.cmake")

set(_input "${AROS_PORTS_DIR}/freetype/config/ftoption.h")
file(MAKE_DIRECTORY "${AROS_PORTS_DIR}/freetype/config")
file(WRITE "${_input}"
    "#define FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES\n"
    "/* FT_CONFIG_OPTION_SUBPIXEL_RENDERING */\n"
    "/* FT_CONFIG_OPTION_SYSTEM_ZLIB */\n"
    "/* FT_CONFIG_OPTION_USE_PNG */\n")

set(_fetch_stamp "${CMAKE_CURRENT_BINARY_DIR}/fetched.stamp")
add_custom_command(OUTPUT "${_fetch_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_fetch_stamp}")
add_custom_target(freetype-fetch DEPENDS "${_fetch_stamp}")
set_property(TARGET freetype-fetch PROPERTY
    AROS_FETCH_COMPLETION_STAMP "${_fetch_stamp}")
if(TEST_SOURCE_TEXT_OWNER)
    include("${CMAKE_CURRENT_LIST_DIR}/../../SourceTextRules.cmake")
    set(AROS_DEVELOPER_INCLUDE_DIR "${AROS_SDK_INCLUDE_DIR}")
    set(AROS_GENINC_DIR "${CMAKE_CURRENT_BINARY_DIR}/GENINCDIR")
    set_property(TARGET freetype-fetch PROPERTY AROS_FETCH_DESTINATION "${AROS_PORTS_DIR}/freetype")
    aros_transform_source_text(
        NAME freetype-options-owner FETCH freetype-fetch
        INPUT "${_input}" OUTPUT "${AROS_SDK_INCLUDE_DIR}/freetype/config/ftoption.h"
        OPERATIONS_JSON "[{\"kind\":\"replace_whole_line_containing\",\"token\":\"FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES\",\"replacement\":\"/*define FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES*/\"},{\"kind\":\"replace_whole_line_containing\",\"token\":\"FT_CONFIG_OPTION_SUBPIXEL_RENDERING\",\"replacement\":\"#define FT_CONFIG_OPTION_SUBPIXEL_RENDERING\"},{\"kind\":\"replace_whole_line_containing\",\"token\":\"FT_CONFIG_OPTION_SYSTEM_ZLIB\",\"replacement\":\"#define FT_CONFIG_OPTION_SYSTEM_ZLIB\"},{\"kind\":\"replace_whole_line_containing\",\"token\":\"FT_CONFIG_OPTION_USE_PNG\",\"replacement\":\"#define FT_CONFIG_OPTION_USE_PNG\"}]")
else()
    add_custom_target(freetype-options-owner)
endif()

# This fixture models only the admission boundary. It does not independently
# validate the source contract; the real consumer/build validators are tested
# separately. In subset cases, the demo target is intentionally absent.
if(TEST_SELECTION_MODE STREQUAL "BUILD_SUBSET")
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)
    function(_aros_native_source_selection_validated out_validated)
        set(${out_validated} TRUE PARENT_SCOPE)
    endfunction()
elseif(TEST_SELECTION_MODE STREQUAL "CONSUMER_SUBSET")
    set(AROS_NATIVE_CONSUMER_CONTRACT_VALIDATED TRUE)
    function(_aros_native_source_selection_validated out_validated)
        set(${out_validated} TRUE PARENT_SCOPE)
    endfunction()
elseif(TEST_SELECTION_MODE STREQUAL "MISSING_HELPER")
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)
elseif(TEST_SELECTION_MODE STREQUAL "UNVALIDATED_SUBSET")
    set(AROS_NATIVE_BUILD_CONTRACT "unvalidated-native-contract")
    function(_aros_native_source_selection_validated out_validated)
        set(${out_validated} FALSE PARENT_SCOPE)
    endfunction()
elseif(TEST_SELECTION_MODE STREQUAL "CLASSIC_MISSING")
    # No native selection: classic consumer declarations remain mandatory.
else()
    set(TEST_SELECTION_MODE "CLASSIC")
endif()

file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/consumer.c"
    "#include <freetype/config/ftoption.h>\nint consumer(void) { return 0; }\n")
add_library(freetype-consumer STATIC
    "${CMAKE_CURRENT_BINARY_DIR}/consumer.c")
target_include_directories(freetype-consumer PRIVATE "${AROS_SDK_INCLUDE_DIR}")
if(NOT TEST_SELECTION_MODE MATCHES "^(BUILD_SUBSET|CONSUMER_SUBSET|UNVALIDATED_SUBSET|MISSING_HELPER|CLASSIC_MISSING)$")
    add_library(freetype-demo-consumer STATIC
        "${CMAKE_CURRENT_BINARY_DIR}/consumer.c")
    target_include_directories(freetype-demo-consumer PRIVATE "${AROS_SDK_INCLUDE_DIR}")
endif()

aros_stage_freetype_options(
    NAME freetype-options
    FETCH_TARGET freetype-fetch
    OWNER freetype-options-owner
    CONSUMERS freetype-consumer freetype-demo-consumer
    INPUT "${_input}"
    OUTPUT "${AROS_SDK_INCLUDE_DIR}/freetype/config/ftoption.h")
