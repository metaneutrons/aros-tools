cmake_minimum_required(VERSION 3.22)
project(ObjectiveCLanguageFixture LANGUAGES C)

include("${CMAKE_CURRENT_LIST_DIR}/../../EnableLLVMObjectiveC.cmake")

get_property(_enabled_languages GLOBAL PROPERTY ENABLED_LANGUAGES)
if(NOT "OBJC" IN_LIST _enabled_languages)
    message(FATAL_ERROR "Clang did not enable the Objective-C language")
endif()
if(NOT CMAKE_OBJC_COMPILER STREQUAL CMAKE_C_COMPILER)
    message(FATAL_ERROR
        "Objective-C compiler ${CMAKE_OBJC_COMPILER} differs from C compiler "
        "${CMAKE_C_COMPILER}")
endif()

add_library(objc-probe OBJECT probe.m)
target_compile_options(objc-probe PRIVATE -target "${OBJC_TEST_TRIPLE}")
if(CMAKE_SYSTEM_PROCESSOR STREQUAL "arm")
    target_compile_options(objc-probe PRIVATE
        -mcpu=cortex-a7 -mfpu=neon-vfpv4)
endif()
