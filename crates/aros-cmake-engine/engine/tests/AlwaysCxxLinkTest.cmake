cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_build "${_temp_root}/aros-always-cxx-link-${_suffix}")
set(_source "${CMAKE_CURRENT_LIST_DIR}/always-cxx-link")

foreach(_mode IN ITEMS development locked)
    if(_mode STREQUAL "locked")
        set(_locked ON)
    else()
        set(_locked OFF)
    endif()
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_source}"
            -B "${_build}-${_mode}" -G Ninja
            "-DTEST_LOCKED_TOOLCHAIN=${_locked}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR
            "always-cxx-link ${_mode} fixture configure failed (${_result})\n"
            "${_stdout}\n${_stderr}")
    endif()
endforeach()

# The locked fixture deliberately uses its host compiler for this small C
# object, after checking the exact direct-link template separately. Building
# it proves that the linker-visible sysroot file is a concrete output,
# not just a dependency-only placeholder.
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}-locked"
        --target aros-cxx-startup
    RESULT_VARIABLE _startup_result
    OUTPUT_VARIABLE _startup_stdout
    ERROR_VARIABLE _startup_stderr)
if(NOT _startup_result EQUAL 0)
    message(FATAL_ERROR
        "always-cxx-link locked cxx-startup build failed (${_startup_result})\n"
        "${_startup_stdout}\n${_startup_stderr}")
endif()
if(NOT EXISTS "${_build}-locked/SYS/Developer/lib/cxx-startup.o")
    message(FATAL_ERROR
        "always-cxx-link locked fixture did not publish Developer/lib/cxx-startup.o")
endif()

file(REMOVE_RECURSE "${_build}-development" "${_build}-locked")
message(STATUS "always C++ linker contract test passed")
