cmake_minimum_required(VERSION 3.22)
project(RuntimeRunnerFixture C)

if(NOT DEFINED RUNNER_SOURCE OR NOT EXISTS "${RUNNER_SOURCE}")
    message(FATAL_ERROR "RUNNER_SOURCE must name llvmpipe-jit-runner.c")
endif()

add_executable(runtime-runner "${RUNNER_SOURCE}" stubs.c)
target_include_directories(runtime-runner PRIVATE
    "${CMAKE_CURRENT_SOURCE_DIR}/include")
target_compile_features(runtime-runner PRIVATE c_std_99)
if(CMAKE_C_COMPILER_ID MATCHES "Clang|GNU")
    target_compile_options(runtime-runner PRIVATE -Wall -Wextra -Werror)
endif()

file(GENERATE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/runner.path"
    CONTENT "$<TARGET_FILE:runtime-runner>\n")
