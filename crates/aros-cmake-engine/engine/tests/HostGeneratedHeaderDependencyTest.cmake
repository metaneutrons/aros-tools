cmake_minimum_required(VERSION 3.22)

get_filename_component(_repo "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/host-generated-header-dependency")

# The producer registration lives in AROS.cmake, while the dependency
# finalizer is independently exercisable in this focused project. Keep both
# halves under one regression contract.
file(READ "${_repo}/cmake/AROS.cmake" _aros_module)
string(FIND "${_aros_module}"
    "set_property(GLOBAL APPEND PROPERTY AROS_32BIT_TARGETS \"\${target}\")"
    _registration)
if(_registration LESS 0)
    message(FATAL_ERROR
        "companion-CPU targets are no longer registered for generated headers")
endif()

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
set(_root "$ENV{TMPDIR}")
if(NOT _root)
    set(_root "/tmp")
endif()
set(_build "${_root}/aros-host-header-${_suffix}")
file(REMOVE_RECURSE "${_build}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
        "-DAROS_REPO_ROOT=${_repo}"
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "host-generated-header fixture configure failed\n"
        "${_configure_stdout}${_configure_stderr}")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target companion --parallel 8
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "companion target raced its generated header\n"
        "${_build_stdout}${_build_stderr}")
endif()
if(NOT EXISTS "${_build}/generated/aros/i386/libcall.h")
    message(FATAL_ERROR "fixture did not publish the generated i386 header")
endif()
file(REMOVE_RECURSE "${_build}")
message(STATUS "host-generated companion-header dependency test passed")
