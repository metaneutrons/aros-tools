cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros python generator ${_suffix}")
set(_source "${_root}/source")
set(_archive "${_source}/archive")
file(MAKE_DIRECTORY "${_archive}")

set(_generator [=[
import argparse
import pathlib
import sys
import fixture_helper

parser = argparse.ArgumentParser()
parser.add_argument("--input", required=True)
parser.add_argument("--name", required=True)
parser.add_argument("--suffix", required=True)
arguments = parser.parse_args()
value = pathlib.Path(arguments.input).read_text(encoding="utf-8").strip()
if value == "FAIL":
    sys.stdout.write("#define PARTIAL_OUTPUT 1\n")
    sys.stderr.write("intentional fixture failure\n")
    raise SystemExit(7)
sys.stdout.write(
    f'#define {arguments.name} "{fixture_helper.decorate(value)}-{arguments.suffix}"\n'
)
]=])
file(WRITE "${_archive}/generator.py" "${_generator}")
file(WRITE "${_archive}/fixture_helper.py" [=[
def decorate(value):
    return value
]=])
file(WRITE "${_archive}/input.txt" "first\n")
file(WRITE "${_source}/consumer.c" [=[
#include "generated/first.h"
#include "generated/second.h"

const char *python_generator_fixture_values = FIRST_VALUE SECOND_VALUE;
]=])

get_filename_component(_cmake_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_fixture_cmake [=[
cmake_minimum_required(VERSION 3.22)
project(PythonGeneratorFixture C)

include("@PYTHON_GENERATORS@")

set(_fetch_destination "${CMAKE_BINARY_DIR}/fetched")
set(_source_root "${_fetch_destination}/package")
set(_fetch_stamp "${_fetch_destination}/.fixture-fetched")
set(AROS_PORTS_SOURCE_DIR "${CMAKE_BINARY_DIR}/portssources")
add_custom_command(
    OUTPUT "${_fetch_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory
        "${_source_root}" "${AROS_PORTS_SOURCE_DIR}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${CMAKE_CURRENT_SOURCE_DIR}/archive/generator.py"
        "${_source_root}/generator.py"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${CMAKE_CURRENT_SOURCE_DIR}/archive/fixture_helper.py"
        "${_source_root}/fixture_helper.py"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${CMAKE_CURRENT_SOURCE_DIR}/archive/input.txt"
        "${_source_root}/input.txt"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_fetch_stamp}"
    DEPENDS
        "${CMAKE_CURRENT_SOURCE_DIR}/archive/generator.py"
        "${CMAKE_CURRENT_SOURCE_DIR}/archive/fixture_helper.py"
        "${CMAKE_CURRENT_SOURCE_DIR}/archive/input.txt"
    VERBATIM)
add_custom_target(fixture-fetch DEPENDS "${_fetch_stamp}")
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_DESTINATION "${_fetch_destination}")
set_property(TARGET fixture-fetch PROPERTY
    AROS_FETCH_COMPLETION_STAMP "${_fetch_stamp}")

set(_build_root "${CMAKE_BINARY_DIR}/gen/python-fixture")

set(_script "generator.py")
set(_source_inputs "input.txt" "fixture_helper.py")
set(_first_output "generated/first.h")
if(PYTHON_GENERATOR_CASE STREQUAL "script-escape")
    set(_script "../generator.py")
elseif(PYTHON_GENERATOR_CASE STREQUAL "source-input-escape")
    set(_source_inputs "../input.txt")
elseif(PYTHON_GENERATOR_CASE STREQUAL "output-escape")
    set(_first_output "../first.h")
elseif(PYTHON_GENERATOR_CASE STREQUAL "build-root-escape")
    set(_build_root "${CMAKE_BINARY_DIR}/outside")
elseif(PYTHON_GENERATOR_CASE STREQUAL "missing-input")
    set(_source_inputs "missing.txt")
endif()

aros_generate_python_outputs(
    OWNER fixture-generate
    SOURCE_ROOT "${_source_root}"
    BUILD_ROOT "${_build_root}"
    FETCH_TARGET fixture-fetch
    SOURCE_INPUTS ${_source_inputs}
    JOB
        SCRIPT "${_script}"
        OUTPUT "${_first_output}"
        ARGUMENTS
            --input "${_source_root}/input.txt"
            --name FIRST_VALUE
            --suffix one
    JOB
        SCRIPT "generator.py"
        OUTPUT "generated/second.h"
        ARGUMENTS
            --input "${_source_root}/input.txt"
            --name SECOND_VALUE
            --suffix two)

if(PYTHON_GENERATOR_CASE STREQUAL "collision")
    aros_generate_python_outputs(
        OWNER conflicting-generate
        SOURCE_ROOT "${_source_root}"
        BUILD_ROOT "${_build_root}"
        FETCH_TARGET fixture-fetch
        SOURCE_INPUTS input.txt
        JOB
            SCRIPT generator.py
            OUTPUT generated/first.h)
endif()

# Generation is intentionally declared before its concrete consumer. This is
# the ordering needed when a generated assembly file appears in target sources
# but does not exist at configure time.
add_library(fixture-consumer STATIC
    "${CMAKE_CURRENT_SOURCE_DIR}/consumer.c"
    "${_build_root}/generated/first.h"
    "${_build_root}/generated/second.h")
target_include_directories(fixture-consumer PRIVATE "${_build_root}")
set(_consumers fixture-consumer)
if(PYTHON_GENERATOR_CASE STREQUAL "utility-consumer")
    add_custom_target(noncompiling-consumer)
    set(_consumers noncompiling-consumer)
endif()
aros_bind_python_output_consumers(
    OWNER fixture-generate
    CONSUMERS ${_consumers})
]=])
set(PYTHON_GENERATORS "${_cmake_dir}/PythonGenerators.cmake")
string(CONFIGURE "${_fixture_cmake}" _fixture_cmake @ONLY)
file(WRITE "${_source}/CMakeLists.txt" "${_fixture_cmake}")

function(_configure case expect_success expected_message)
    set(_build "${_root}/${case}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}" -B "${_build}" -G Ninja
            "-DPYTHON_GENERATOR_CASE=${case}" ${ARGN}
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "python-generator ${case} configure failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR
            "python-generator ${case} configure unexpectedly succeeded")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        set(_log "${_stdout}\n${_stderr}")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR
                "python-generator ${case} missed '${expected_message}':\n${_log}")
        endif()
    endif()
endfunction()

function(_build build target expect_success label)
    execute_process(
        COMMAND "${CMAKE_COMMAND}" --build "${build}" --target "${target}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set("${label}_RESULT" "${_result}" PARENT_SCOPE)
    set("${label}_LOG" "${_stdout}\n${_stderr}" PARENT_SCOPE)
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR
            "python-generator ${label} build failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR
            "python-generator ${label} build unexpectedly succeeded")
    endif()
endfunction()

function(_assert_contents path expected label)
    if(NOT EXISTS "${path}" OR IS_DIRECTORY "${path}")
        message(FATAL_ERROR "${label} is missing: ${path}")
    endif()
    file(READ "${path}" _actual)
    if(NOT _actual STREQUAL "${expected}")
        message(FATAL_ERROR
            "${label} contains '${_actual}', expected '${expected}'")
    endif()
endfunction()

_configure(success TRUE "")
set(_success_build "${_root}/success")
set(_first_output
    "${_success_build}/gen/python-fixture/generated/first.h")
set(_second_output
    "${_success_build}/gen/python-fixture/generated/second.h")
_build("${_success_build}" fixture-consumer TRUE initial)
_assert_contents("${_first_output}"
    "#define FIRST_VALUE \"first-one\"\n" "first generated output")
_assert_contents("${_second_output}"
    "#define SECOND_VALUE \"first-two\"\n" "second generated output")
file(GLOB_RECURSE _python_bytecode
    "${_success_build}/fetched/package/__pycache__/*"
    "${_success_build}/fetched/package/*.pyc")
if(_python_bytecode)
    message(FATAL_ERROR
        "Python generator modified its fetched source tree: ${_python_bytecode}")
endif()

_build("${_success_build}" fixture-consumer TRUE noop)
string(FIND "${noop_LOG}" "ninja: no work to do." _noop_found)
if(_noop_found LESS 0)
    message(FATAL_ERROR
        "second Python-generator build was not a Ninja no-op:\n${noop_LOG}")
endif()

# The fetched inputs are real dependencies of the simulated fetch. Once its
# completion stamp advances, all Python jobs must regenerate.
execute_process(COMMAND "${CMAKE_COMMAND}" -E sleep 1)
file(WRITE "${_archive}/input.txt" "second\n")
_build("${_success_build}" fixture-consumer TRUE refreshed)
_assert_contents("${_first_output}"
    "#define FIRST_VALUE \"second-one\"\n" "refreshed first output")
_assert_contents("${_second_output}"
    "#define SECOND_VALUE \"second-two\"\n" "refreshed second output")

# A generator that writes partial stdout and then fails must not replace either
# last known-good product, and no temporary output may survive the failure.
execute_process(COMMAND "${CMAKE_COMMAND}" -E sleep 1)
file(WRITE "${_archive}/input.txt" "FAIL\n")
_build("${_success_build}" fixture-generate FALSE failed)
string(FIND "${failed_LOG}" "Python generator failed" _failure_found)
if(_failure_found LESS 0)
    message(FATAL_ERROR
        "failed Python generator missed its diagnostic:\n${failed_LOG}")
endif()
_assert_contents("${_first_output}"
    "#define FIRST_VALUE \"second-one\"\n"
    "first output after generator failure")
_assert_contents("${_second_output}"
    "#define SECOND_VALUE \"second-two\"\n"
    "second output after generator failure")
file(GLOB_RECURSE _temporary_outputs
    "${_success_build}/gen/python-fixture/*.tmp")
if(_temporary_outputs)
    message(FATAL_ERROR
        "failed Python generator left temporary outputs: ${_temporary_outputs}")
endif()

execute_process(COMMAND "${CMAKE_COMMAND}" -E sleep 1)
file(WRITE "${_archive}/input.txt" "recovered\n")
_build("${_success_build}" fixture-consumer TRUE recovered)
_assert_contents("${_first_output}"
    "#define FIRST_VALUE \"recovered-one\"\n" "recovered first output")
_assert_contents("${_second_output}"
    "#define SECOND_VALUE \"recovered-two\"\n" "recovered second output")

file(REMOVE "${_second_output}")
_build("${_success_build}" fixture-generate TRUE restored)
_assert_contents("${_second_output}"
    "#define SECOND_VALUE \"recovered-two\"\n" "restored deleted output")

_configure(collision FALSE "owned by fixture-generate")
_configure(script-escape FALSE "SCRIPT escapes SOURCE_ROOT")
_configure(source-input-escape FALSE "SOURCE_INPUT escapes SOURCE_ROOT")
_configure(output-escape FALSE "OUTPUT escapes BUILD_ROOT")
_configure(build-root-escape FALSE "BUILD_ROOT must be a private child")
_configure(utility-consumer FALSE
    "Python-generator consumer noncompiling-consumer")
_configure(missing-python FALSE "a working Python 3 interpreter is required"
    "-DPython3_EXECUTABLE=${_root}/missing-python3")

_configure(missing-input TRUE "")
_build("${_root}/missing-input" fixture-generate FALSE missing_input)
string(FIND "${missing_input_LOG}" "required source input is missing"
    _missing_input_found)
if(_missing_input_found LESS 0)
    message(FATAL_ERROR
        "missing source input missed its diagnostic:\n${missing_input_LOG}")
endif()

file(REMOVE_RECURSE "${_root}")
message(STATUS "output-tracked Python generator test passed")
