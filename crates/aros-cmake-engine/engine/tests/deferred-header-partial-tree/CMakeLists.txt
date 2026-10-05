cmake_minimum_required(VERSION 3.22)
project(DeferredHeaderPartialTree LANGUAGES C)

# The fixture tests fetched header ordering, not SDK bootstrap.
list(INSERT CMAKE_MODULE_PATH 0
    "${CMAKE_CURRENT_LIST_DIR}/../source-resolution/cmake")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_PORTS_DIR "${CMAKE_BINARY_DIR}/Ports")
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")

set(_port "${AROS_PORTS_DIR}/fixture")
set(_stamp "${CMAKE_BINARY_DIR}/fixture-fetch.stamp")
file(MAKE_DIRECTORY "${_port}")
if(NOT EXISTS "${_stamp}")
    # An interrupted fetch left a misleading source directory and stale file.
    file(WRITE "${_port}/zlib.h" "#error stale unfetched header\n")
endif()
add_custom_command(OUTPUT "${_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E copy
        "${CMAKE_CURRENT_LIST_DIR}/zlib.h" "${_port}/zlib.h"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_stamp}"
    DEPENDS "${CMAKE_CURRENT_LIST_DIR}/zlib.h"
    VERBATIM)
add_custom_target(fixture-fetch DEPENDS "${_stamp}")
set_property(TARGET fixture-fetch PROPERTY AROS_FETCH_DESTINATION "${_port}")
set_property(TARGET fixture-fetch PROPERTY AROS_FETCH_COMPLETION_STAMP "${_stamp}")
set_property(GLOBAL APPEND PROPERTY AROS_FETCH_TARGETS fixture-fetch)

if(AMBIGUOUS_FETCH_OWNER)
    add_custom_target(fixture-fetch-duplicate)
    set_property(TARGET fixture-fetch-duplicate PROPERTY AROS_FETCH_DESTINATION "${_port}/")
    set_property(GLOBAL APPEND PROPERTY AROS_FETCH_TARGETS fixture-fetch-duplicate)
endif()

aros_copy_includes(
    NAME fixture-headers DEST "." SOURCE "${_port}" PATTERNS "zlib.h")
add_library(consumer STATIC "${CMAKE_CURRENT_LIST_DIR}/consumer.c")
target_include_directories(consumer PRIVATE "${AROS_SDK_INCLUDE_DIR}")
add_dependencies(consumer fixture-headers)
