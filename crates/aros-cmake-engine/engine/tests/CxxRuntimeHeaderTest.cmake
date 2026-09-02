cmake_minimum_required(VERSION 3.22)

find_program(_cxx NAMES clang++ g++ c++)
if(NOT _cxx)
    message(FATAL_ERROR "C++ header compatibility test needs a C++ compiler")
endif()

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_root "${_temp_root}/aros-cxx-runtime-headers-${_suffix}")
set(_fake "${_root}/include")
set(_stdc "${CMAKE_CURRENT_LIST_DIR}/../../compiler/crt/stdc/include")
set(_posixc "${CMAKE_CURRENT_LIST_DIR}/../../compiler/crt/posixc/include")

file(MAKE_DIRECTORY "${_fake}/aros/stdc" "${_fake}/aros/types")
file(WRITE "${_fake}/aros/features.h" "/* fixture */\n")
file(WRITE "${_fake}/aros/stdc/errno.h" "/* fixture */\n")
foreach(_type IN ITEMS ptrdiff_t size_t wchar_t null)
    file(WRITE "${_fake}/aros/types/${_type}.h" "/* fixture */\n")
endforeach()

file(WRITE "${_root}/aros-first.cpp" [=[
#include <aros/types/max_align_t.h>
#include <cstddef>
struct expected_aros_max_align_t {
    long long ll;
    long double ld;
};
static_assert(sizeof(max_align_t) == sizeof(expected_aros_max_align_t),
              "AROS max_align_t must match the GCC/Clang layout");
static_assert(alignof(max_align_t) == alignof(expected_aros_max_align_t),
              "AROS max_align_t alignment must match GCC/Clang");
max_align_t aros_first_value;
]=])
file(WRITE "${_root}/compiler-first.cpp" [=[
#include <cstddef>
#include <aros/stdc/stddef.h>
max_align_t compiler_first_value;
]=])
file(WRITE "${_root}/errno.cpp" [=[
#include <aros/posixc/errno.h>
static_assert(EOWNERDEAD == 97, "EOWNERDEAD ABI value changed");
static_assert(ENOTRECOVERABLE == 98, "ENOTRECOVERABLE ABI value changed");
static_assert(__POSIXC_ELAST == ENOTRECOVERABLE, "errno range is incomplete");
]=])

foreach(_source IN ITEMS aros-first compiler-first errno)
    execute_process(
        COMMAND "${_cxx}" -std=c++11 -Wall -Wextra -Werror -fsyntax-only
            "-I${_fake}" "-I${_stdc}" "-I${_posixc}"
            "${_root}/${_source}.cpp"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        file(REMOVE_RECURSE "${_root}")
        message(FATAL_ERROR
            "C++ runtime header fixture ${_source} failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    endif()
endforeach()

file(REMOVE_RECURSE "${_root}")
message(STATUS "C++ runtime header compatibility test passed")
