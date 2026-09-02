cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-default-build-closure-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/default-build-closure")
set(_build "${_root}/build")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "default build closure fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "default build closure fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()
if(EXISTS "${_build}/libmanual-member.a")
    message(FATAL_ERROR "unqualified build compiled the manual target")
endif()
if(NOT EXISTS "${_build}/libdefault-member.a" OR
   NOT EXISTS "${_build}/liblinked-member.a")
    message(FATAL_ERROR
        "unqualified build did not compile the reachable target and its link dependency")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "default MetaMake build closure test passed")
