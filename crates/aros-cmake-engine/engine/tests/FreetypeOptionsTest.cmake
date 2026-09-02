cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-freetype-options-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/freetype-options")
set(_build "${_root}/build")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "FreeType options fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target
        freetype-consumer freetype-demo-consumer
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "FreeType options consumer raced its generated header (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()

set(_output "${_build}/SDK/include/freetype/config/ftoption.h")
file(READ "${_output}" _content)
foreach(_expected IN ITEMS
        "/*define FT_CONFIG_OPTION_ENVIRONMENT_PROPERTIES*/"
        "#define FT_CONFIG_OPTION_SUBPIXEL_RENDERING"
        "#define FT_CONFIG_OPTION_SYSTEM_ZLIB"
        "#define FT_CONFIG_OPTION_USE_PNG")
    string(FIND "${_content}" "${_expected}" _expected_at)
    if(_expected_at EQUAL -1)
        message(FATAL_ERROR "FreeType options output omitted ${_expected}")
    endif()
endforeach()

file(REMOVE_RECURSE "${_root}")
message(STATUS "FreeType option consumer-ordering test passed")
