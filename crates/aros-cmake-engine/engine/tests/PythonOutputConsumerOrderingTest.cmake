cmake_minimum_required(VERSION 3.22)

# Generated private headers need an explicit owner edge even when only an
# ordinary source header includes them and the consumer lives elsewhere.
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
set(_root "${_temp_root}/aros python consumer ordering ${_suffix}")
set(_source "${_root}/source")
file(MAKE_DIRECTORY "${_source}/provider" "${_source}/consumer")
get_filename_component(PYTHON_GENERATORS "${CMAKE_CURRENT_LIST_DIR}/../PythonGenerators.cmake" ABSOLUTE)
set(_project [=[
cmake_minimum_required(VERSION 3.22)
project(ExplicitOutputConsumer C)
include("@PYTHON_GENERATORS@")
add_subdirectory(provider)
add_subdirectory(consumer)
if(BIND_OUTPUT_CONSUMER)
    aros_bind_python_output_consumers(
        OWNER fixture-generated CONSUMERS fixture-consumer)
endif()
]=])
string(CONFIGURE "${_project}" _project @ONLY)
file(WRITE "${_source}/CMakeLists.txt" "${_project}")
file(WRITE "${_source}/provider/CMakeLists.txt" [=[
add_custom_command(
    OUTPUT "${CMAKE_BINARY_DIR}/generated/values.h"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${CMAKE_BINARY_DIR}/generated"
    COMMAND "${CMAKE_COMMAND}" -E copy
        "${CMAKE_CURRENT_SOURCE_DIR}/values.h" "${CMAKE_BINARY_DIR}/generated/values.h"
    DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/values.h"
    VERBATIM)
add_custom_target(fixture-generated DEPENDS "${CMAKE_BINARY_DIR}/generated/values.h")
set_property(TARGET fixture-generated PROPERTY AROS_PYTHON_OUTPUT_OWNER TRUE)
]=])
file(WRITE "${_source}/provider/values.h" "#define GENERATED_VALUE 42\n")
file(WRITE "${_source}/consumer/CMakeLists.txt" [=[
add_library(fixture-consumer STATIC consumer.c)
target_include_directories(fixture-consumer PRIVATE "${CMAKE_BINARY_DIR}")
]=])
file(WRITE "${_source}/consumer/ordinary.h" "#include <generated/values.h>\n")
file(WRITE "${_source}/consumer/consumer.c" "#include \"ordinary.h\"\nint answer(void) { return GENERATED_VALUE; }\n")

foreach(_case missing-edge declared-edge)
    set(_build "${_root}/${_case}")
    if(_case STREQUAL "declared-edge")
        set(_bind ON)
    else()
        set(_bind OFF)
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
            "-DBIND_OUTPUT_CONSUMER=${_bind}"
        TIMEOUT 30 RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${_case}: configure failed (${_result})\n${_stdout}\n${_stderr}")
    endif()
    if(EXISTS "${_build}/generated/values.h")
        message(FATAL_ERROR "${_case}: generated header was not cold")
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target fixture-consumer --parallel 8
        TIMEOUT 30 RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    if(_bind)
        if(NOT _result EQUAL 0 OR NOT EXISTS "${_build}/generated/values.h")
            message(FATAL_ERROR "${_case}: declared owner did not precede compilation (${_result})\n${_stdout}\n${_stderr}")
        endif()
    else()
        if(NOT "${_result}" MATCHES "^[1-9][0-9]*$" OR
           NOT "${_stdout}\n${_stderr}" MATCHES "generated/values.h" OR
           EXISTS "${_build}/generated/values.h")
            message(FATAL_ERROR "${_case}: counterprobe did not fail for the absent generated header (${_result})\n${_stdout}\n${_stderr}")
        endif()
    endif()
endforeach()
file(REMOVE_RECURSE "${_root}")
message(STATUS "explicit cross-directory Python output consumer ordering test passed")
