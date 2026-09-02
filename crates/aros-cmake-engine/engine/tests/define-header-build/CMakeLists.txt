cmake_minimum_required(VERSION 3.22)
project(DefineHeaderBuild NONE)

if(NOT TEST_DEPENDENCY OR NOT EXISTS "${TEST_DEPENDENCY}")
    message(FATAL_ERROR "TEST_DEPENDENCY must name the fixture input")
endif()

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
include("${CMAKE_CURRENT_LIST_DIR}/../../AROS.cmake")

aros_generate_defines_header(
    OWNER test-defines
    OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/nested/opt_test.h"
    DEFINES "AH_HAS_RF 1" "AH_MASK 0x20" "AH_MODE TOKEN_2"
    DEPENDS "${TEST_DEPENDENCY}")
