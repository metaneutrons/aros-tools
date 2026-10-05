cmake_minimum_required(VERSION 3.24)
find_program(_ninja NAMES ninja ninja-build REQUIRED)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root /tmp)
endif()
file(REAL_PATH "${_temp_root}" _temp_root)
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-header-whole-line-${_suffix}")
get_filename_component(_root "${_root}" ABSOLUTE)
if(EXISTS "${_root}" OR IS_SYMLINK "${_root}")
    message(FATAL_ERROR "test root already exists")
endif()
execute_process(COMMAND "${CMAKE_COMMAND}"
    -S "${CMAKE_CURRENT_LIST_DIR}/header-transform-boundary" -B "${_root}" -G Ninja
    "-DAROS_ENGINE_DIR=${CMAKE_CURRENT_LIST_DIR}/.." -DAROS_HEADER_PROBE=whole-line
    RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 60)
if(NOT "${_result}" STREQUAL "0")
    message(FATAL_ERROR "whole-line helper configure failed: ${_stdout}\n${_stderr}")
endif()
function(_build out)
    execute_process(COMMAND "${CMAKE_COMMAND}" --build "${_root}" --target whole-line-headers
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr TIMEOUT 60)
    if(NOT "${_result}" STREQUAL "0")
        message(FATAL_ERROR "whole-line helper build failed: ${_stdout}\n${_stderr}")
    endif()
    set(${out} "${_stdout}\n${_stderr}" PARENT_SCOPE)
endfunction()
_build(_first)
set(_expected [=[before; preserved
#if defined(PLATFORM)
#define FEATURE_TOKEN
/* literal; ${prefix} */
#else
#undef FEATURE_TOKEN
#endif
after
]=])
set(_output "${_root}/SDK/include/config.h")
file(READ "${_output}" _actual)
if(NOT "${_actual}" STREQUAL "${_expected}")
    message(FATAL_ERROR "whole-line helper lost multiline or semicolon/dollar bytes")
endif()
_build(_noop)
if("${_noop}" MATCHES "Replacing matching header lines")
    message(FATAL_ERROR "whole-line helper rebuilt unchanged output")
endif()
file(REMOVE "${_output}")
_build(_restored)
file(READ "${_output}" _actual)
if(NOT "${_actual}" STREQUAL "${_expected}" OR
   NOT "${_restored}" MATCHES "Replacing matching header lines")
    message(FATAL_ERROR "whole-line helper did not restore a missing output")
endif()
message(STATUS "production whole-line helper passed: literal multiline/semicolon/dollar bytes, no-op and missing-output rebuild")
file(REMOVE_RECURSE "${_root}")
