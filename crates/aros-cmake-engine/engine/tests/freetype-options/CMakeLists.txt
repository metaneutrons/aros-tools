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
add_custom_target(freetype-options-owner)

file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/consumer.c"
    "#include <freetype/config/ftoption.h>\nint consumer(void) { return 0; }\n")
add_library(freetype-consumer STATIC
    "${CMAKE_CURRENT_BINARY_DIR}/consumer.c")
target_include_directories(freetype-consumer PRIVATE "${AROS_SDK_INCLUDE_DIR}")
add_library(freetype-demo-consumer STATIC
    "${CMAKE_CURRENT_BINARY_DIR}/consumer.c")
target_include_directories(freetype-demo-consumer PRIVATE "${AROS_SDK_INCLUDE_DIR}")

aros_stage_freetype_options(
    NAME freetype-options
    FETCH_TARGET freetype-fetch
    OWNER freetype-options-owner
    CONSUMERS freetype-consumer freetype-demo-consumer
    INPUT "${_input}"
    OUTPUT "${AROS_SDK_INCLUDE_DIR}/freetype/config/ftoption.h")
