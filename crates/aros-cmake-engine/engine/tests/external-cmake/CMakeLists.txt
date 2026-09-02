cmake_minimum_required(VERSION 3.22)
project(ExternalCMakeFixture C)

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_PORTS_SOURCE_DIR "${CMAKE_CURRENT_LIST_DIR}/archive"
    CACHE PATH "fixture archive root")
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")

set(_archive "${CMAKE_CURRENT_LIST_DIR}/archive/source.txt")
set(_upstream "${CMAKE_CURRENT_LIST_DIR}/upstream")

add_custom_target(fixture-fetch)
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_DESTINATION "${_upstream}")
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_COMPLETION_STAMP "${_archive}")

if(EXTERNAL_CMAKE_CASE STREQUAL "missing-fetch")
    set(_fetch_target absent-fetch)
else()
    set(_fetch_target fixture-fetch)
endif()
if(EXTERNAL_CMAKE_CASE STREQUAL "escape-binary")
    set(_binary "${CMAKE_BINARY_DIR}/../escape")
else()
    set(_binary
        "${CMAKE_BINARY_DIR}/gen/external-cmake/fixture-external")
endif()
if(EXTERNAL_CMAKE_CASE STREQUAL "overlap-prefix")
    set(_prefix "${_binary}/install")
else()
    set(_prefix "${CMAKE_BINARY_DIR}/install")
endif()
set(_library "${_prefix}/lib/libtiny.a")
set(_header "${_prefix}/include/tiny.h")
set(_metadata "${_prefix}/share/tiny/metadata.txt")
if(EXTERNAL_CMAKE_CASE STREQUAL "escape-product")
    set(_library "${CMAKE_BINARY_DIR}/outside/libtiny.a")
elseif(EXTERNAL_CMAKE_CASE STREQUAL "missing-output")
    set(_header "${_prefix}/include/not-installed.h")
endif()
if(EXTERNAL_CMAKE_CASE STREQUAL "unsafe-option")
    set(_options "-Pbad.cmake")
elseif(EXTERNAL_CMAKE_CASE STREQUAL "toolchain-override")
    set(_options "-DCMAKE_C_COMPILER=/tmp/not-allowed")
else()
    set(_options "-DMINI_FEATURE=yes")
endif()

aros_build_external_cmake(
    MMAKE_ID fixture-external
    SOURCE_DIR "${_upstream}"
    BINARY_DIR "${_binary}"
    INSTALL_PREFIX "${_prefix}"
    FETCH_TARGET "${_fetch_target}"
    PROVIDED_LIBRARY tiny
    OPTIONS "${_options}"
    LIBRARY_PRODUCTS "${_library}"
    HEADER_PRODUCTS "${_header}"
    AUXILIARY_PRODUCTS "${_metadata}"
    PUBLIC_INCLUDE_DIRS "${_prefix}/include")

if(EXTERNAL_CMAKE_CASE STREQUAL "collision" OR
   EXTERNAL_CMAKE_CASE STREQUAL "binary-collision")
    if(EXTERNAL_CMAKE_CASE STREQUAL "binary-collision")
        set(_second_binary "${_binary}")
    else()
        set(_second_binary
            "${CMAKE_BINARY_DIR}/gen/external-cmake/fixture-external-second")
    endif()
    aros_build_external_cmake(
        MMAKE_ID fixture-external-second
        SOURCE_DIR "${_upstream}"
        BINARY_DIR "${_second_binary}"
        INSTALL_PREFIX "${_prefix}"
        FETCH_TARGET fixture-fetch
        PROVIDED_LIBRARY tiny
        OPTIONS -DMINI_FEATURE=yes
        LIBRARY_PRODUCTS "${_prefix}/lib/libtiny.a"
        HEADER_PRODUCTS "${_prefix}/include/tiny.h"
        AUXILIARY_PRODUCTS "${_prefix}/share/tiny/metadata.txt"
        PUBLIC_INCLUDE_DIRS "${_prefix}/include")
endif()

add_library(external-consumer STATIC consumer.c)
target_link_libraries(external-consumer PRIVATE
    fixture-external-external-tiny)
