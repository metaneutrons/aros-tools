cmake_minimum_required(VERSION 3.22)

find_program(MESA_CXX_COMPAT_COMPILER NAMES clang++ REQUIRED)

cmake_path(GET CMAKE_CURRENT_LIST_DIR PARENT_PATH _cmake_dir)
cmake_path(GET _cmake_dir PARENT_PATH _source_dir)
set(_compat_dir
    "${_source_dir}/workbench/libs/mesa/libcompiler/cxx-compat")
set(_compat_header "${_compat_dir}/new")
if(NOT EXISTS "${_compat_header}" OR IS_DIRECTORY "${_compat_header}")
    message(FATAL_ERROR "Mesa placement-new compatibility header is missing")
endif()

if(DEFINED ENV{TMPDIR} AND IS_DIRECTORY "$ENV{TMPDIR}")
    set(_test_parent "$ENV{TMPDIR}")
else()
    set(_test_parent "/tmp")
endif()
file(REAL_PATH "${_test_parent}" _test_parent)
string(RANDOM LENGTH 24 ALPHABET 0123456789abcdef _test_nonce)
set(_test_root "${_test_parent}/aros-mesa-cxx-compat-${_test_nonce}")
if(EXISTS "${_test_root}")
    message(FATAL_ERROR "Mesa C++ compatibility test root already exists")
endif()
file(MAKE_DIRECTORY "${_test_root}")
set(_test_source "${_test_root}/placement-new.cpp")
file(WRITE "${_test_source}" [=[
#include <new>
struct value { explicit value(int input) : number(input) {} int number; };
alignas(value) unsigned char storage[sizeof(value)];
int exercise_placement_new()
{
    value *placed = new (storage) value(42);
    int result = placed->number;
    placed->~value();
    return result;
}
]=])

execute_process(
    COMMAND "${MESA_CXX_COMPAT_COMPILER}"
            -std=gnu++14 -ffreestanding -fsyntax-only -nostdinc++
            "-I${_compat_dir}" "${_test_source}"
    RESULT_VARIABLE _positive_result
    ERROR_VARIABLE _positive_error)

execute_process(
    COMMAND "${MESA_CXX_COMPAT_COMPILER}"
            -std=gnu++11 -ffreestanding -fsyntax-only -nostdinc++
            "-I${_compat_dir}" "${_test_source}"
    RESULT_VARIABLE _negative_result
    ERROR_VARIABLE _negative_error)
file(REMOVE_RECURSE "${_test_root}")
if(NOT _positive_result EQUAL 0)
    message(FATAL_ERROR
        "Mesa placement-new positive compile failed: ${_positive_error}")
endif()
if(_negative_result EQUAL 0)
    message(FATAL_ERROR
        "Mesa placement-new header accepted the unaudited C++11 dialect")
endif()
if(NOT _negative_error MATCHES
   "requires the audited C\\+\\+14 dialect")
    message(FATAL_ERROR
        "Mesa placement-new negative compile failed for the wrong reason: ${_negative_error}")
endif()

message(STATUS "Mesa private C++14 placement-new compatibility test passed")
