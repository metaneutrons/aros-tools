cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros exact python ${_suffix}")
set(_source "${_root}/source")
set(_build "${_root}/build")
file(MAKE_DIRECTORY "${_source}")

file(WRITE "${_source}/generator.py" [=[
import pathlib
import sys

if sys.argv[1] == "FAIL":
    sys.stdout.write("partial output must not survive\n")
    sys.stderr.write("intentional stdout fixture failure\n")
    raise SystemExit(7)
value = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").strip()
sys.stdout.write(f"#define EXACT_PYTHON_VALUE \"{value}-{sys.argv[2]}\"\n")
]=])
file(WRITE "${_source}/input.txt" "clean\n")

get_filename_component(_cmake_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(EXACT_PYTHON_MODULE "${_cmake_dir}/PythonGenerators.cmake")
set(_fixture [=[
cmake_minimum_required(VERSION 3.22)
project(ExactPythonFixture NONE)
include("@EXACT_PYTHON_MODULE@")

set(_fetched "${CMAKE_BINARY_DIR}/fetched/package")
set(_stamp "${CMAKE_BINARY_DIR}/fetched/.ready")
add_custom_command(
    OUTPUT "${_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${_fetched}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${CMAKE_CURRENT_SOURCE_DIR}/generator.py" "${_fetched}/generator.py"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${CMAKE_CURRENT_SOURCE_DIR}/input.txt" "${_fetched}/input.txt"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_stamp}"
    DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/generator.py"
            "${CMAKE_CURRENT_SOURCE_DIR}/input.txt"
    VERBATIM)
add_custom_target(fixture-fetch DEPENDS "${_stamp}")

aros_generate_intree_script_outputs(
    OWNER exact-python-success
    SCRIPT "${_fetched}/generator.py"
    OUTPUTS "${CMAKE_BINARY_DIR}/generated/result.h"
    STDOUT
    WORKING_DIRECTORY "${_fetched}"
    ARGUMENTS "${_fetched}/input.txt" v33
    DEPENDS "${_fetched}/generator.py" "${_fetched}/input.txt"
    DEPENDENCY_TARGETS fixture-fetch)

aros_generate_intree_script_outputs(
    OWNER exact-python-failure
    SCRIPT "${_fetched}/generator.py"
    OUTPUTS "${CMAKE_BINARY_DIR}/generated/failed.h"
    STDOUT
    WORKING_DIRECTORY "${_fetched}"
    ARGUMENTS FAIL
    DEPENDS "${_fetched}/generator.py"
    DEPENDENCY_TARGETS fixture-fetch)
]=])
string(CONFIGURE "${_fixture}" _fixture @ONLY)
file(WRITE "${_source}/CMakeLists.txt" "${_fixture}")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "exact-python configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target exact-python-success
    RESULT_VARIABLE _success_result
    OUTPUT_VARIABLE _success_stdout
    ERROR_VARIABLE _success_stderr)
if(NOT _success_result EQUAL 0)
    message(FATAL_ERROR
        "exact-python success build failed (${_success_result})\n"
        "${_success_stdout}\n${_success_stderr}")
endif()
file(READ "${_build}/generated/result.h" _result)
if(NOT _result STREQUAL "#define EXACT_PYTHON_VALUE \"clean-v33\"\n")
    message(FATAL_ERROR "exact-python output mismatch: '${_result}'")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target exact-python-failure
    RESULT_VARIABLE _failure_result
    OUTPUT_VARIABLE _failure_stdout
    ERROR_VARIABLE _failure_stderr)
if(_failure_result EQUAL 0)
    message(FATAL_ERROR "exact-python failure build unexpectedly succeeded")
endif()
set(_failure_log "${_failure_stdout}\n${_failure_stderr}")
string(FIND "${_failure_log}" "intentional stdout fixture failure" _diagnostic)
if(_diagnostic LESS 0)
    message(FATAL_ERROR "exact-python failure missed stderr:\n${_failure_log}")
endif()
if(EXISTS "${_build}/generated/failed.h"
        OR EXISTS "${_build}/generated/failed.h.aros-python-tmp")
    message(FATAL_ERROR "failed Python generator left a partial output")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "exact Python stdout generator test passed")
