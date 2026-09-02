cmake_minimum_required(VERSION 3.22)
project(HostGeneratedHeaderDependency C)

if(NOT AROS_REPO_ROOT)
    message(FATAL_ERROR "AROS_REPO_ROOT is required")
endif()
include("${AROS_REPO_ROOT}/cmake/HostGeneratedHeaders.cmake")

set(_header "${CMAKE_BINARY_DIR}/generated/aros/i386/libcall.h")
add_custom_command(
    OUTPUT "${_header}"
    COMMAND "${CMAKE_COMMAND}"
        "-DOUTPUT=${_header}"
        -P "${CMAKE_CURRENT_LIST_DIR}/WriteHeader.cmake"
    DEPENDS "${CMAKE_CURRENT_LIST_DIR}/WriteHeader.cmake"
    VERBATIM)
add_custom_target(aros-host-header-fixture DEPENDS "${_header}")

add_library(companion STATIC consumer.c)
target_include_directories(companion PRIVATE "${CMAKE_BINARY_DIR}/generated")
set_property(GLOBAL APPEND PROPERTY AROS_HOST_HEADER_TARGETS
    aros-host-header-fixture)
set_property(GLOBAL APPEND PROPERTY AROS_32BIT_TARGETS companion)
aros_attach_host_header_dependencies()
