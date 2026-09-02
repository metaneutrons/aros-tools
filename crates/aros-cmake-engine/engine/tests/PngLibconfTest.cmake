cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_fixture "${_temp_root}/aros-pnglibconf-${_suffix}")
set(_prebuilt "${_fixture}/prebuilt.h")
set(_writer "${CMAKE_CURRENT_LIST_DIR}/../WritePngLibconf.cmake")
file(MAKE_DIRECTORY "${_fixture}")

set(_input_content [=[/* libpng prebuilt */
#define PNG_EASY_ACCESS_SUPPORTED
/*#undef PNG_ERROR_NUMBERS_SUPPORTED*/
#define PNG_ERROR_TEXT_SUPPORTED
]=])
set(_expected_content [=[/* libpng prebuilt */
#define PNG_EASY_ACCESS_SUPPORTED
#if defined(__AROS__)
#define PNG_ERROR_NUMBERS_SUPPORTED
#else
/*#undef PNG_ERROR_NUMBERS_SUPPORTED*/
#endif
#define PNG_ERROR_TEXT_SUPPORTED
]=])
file(WRITE "${_prebuilt}" "${_input_content}")

set(_writer_output "${_fixture}/SDK/include/pnglibconf.h")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DBINARY_ROOT=${_fixture}"
        "-DINPUT=${_prebuilt}"
        "-DOUTPUT=${_writer_output}"
        -P "${_writer}"
    RESULT_VARIABLE _writer_result
    OUTPUT_VARIABLE _writer_stdout
    ERROR_VARIABLE _writer_stderr)
if(NOT _writer_result EQUAL 0)
    message(FATAL_ERROR
        "pnglibconf writer failed (${_writer_result})\n"
        "${_writer_stdout}\n${_writer_stderr}")
endif()
file(READ "${_writer_output}" _writer_actual)
if(NOT _writer_actual STREQUAL _expected_content)
    message(FATAL_ERROR
        "pnglibconf writer changed bytes outside the legacy replacement\n"
        "expected=${_expected_content}\nactual=${_writer_actual}")
endif()

set(_missing "${_fixture}/missing.h")
file(WRITE "${_missing}" "#define PNG_ERROR_TEXT_SUPPORTED\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DBINARY_ROOT=${_fixture}"
        "-DINPUT=${_missing}"
        "-DOUTPUT=${_fixture}/SDK/include/missing.h"
        -P "${_writer}"
    RESULT_VARIABLE _missing_result
    OUTPUT_QUIET ERROR_QUIET)
if(_missing_result EQUAL 0)
    message(FATAL_ERROR "pnglibconf writer accepted a header without its token")
endif()

set(_duplicate "${_fixture}/duplicate.h")
file(WRITE "${_duplicate}"
    "/*#undef PNG_ERROR_NUMBERS_SUPPORTED*/\n"
    "/*#undef PNG_ERROR_NUMBERS_SUPPORTED*/\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DBINARY_ROOT=${_fixture}"
        "-DINPUT=${_duplicate}"
        "-DOUTPUT=${_fixture}/SDK/include/duplicate.h"
        -P "${_writer}"
    RESULT_VARIABLE _duplicate_result
    OUTPUT_QUIET ERROR_QUIET)
if(_duplicate_result EQUAL 0)
    message(FATAL_ERROR "pnglibconf writer accepted duplicate token lines")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DBINARY_ROOT=${_fixture}"
        "-DINPUT=${_prebuilt}"
        "-DOUTPUT=${_fixture}-outside.h"
        -P "${_writer}"
    RESULT_VARIABLE _escape_result
    OUTPUT_QUIET ERROR_QUIET)
if(_escape_result EQUAL 0 OR EXISTS "${_fixture}-outside.h")
    message(FATAL_ERROR "pnglibconf writer accepted an output outside BINARY_ROOT")
endif()

# Exercise the actual fetch-to-output dependency under Ninja.  The source only
# appears through the fixture fetch target, as it does for the real libpng
# archive, so a missing order edge is observable here.
set(_build_root "${_temp_root}/aros-pnglibconf-build-${_suffix}")
set(_build "${_build_root}/build")
file(MAKE_DIRECTORY "${_build_root}")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        -S "${CMAKE_CURRENT_LIST_DIR}/pnglibconf-build"
        -B "${_build}"
        -G Ninja
        "-DTEST_PREBUILT=${_prebuilt}"
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "pnglibconf fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target workbench-libs-png-generated
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR
        "pnglibconf fixture build failed (${_build_result})\n"
        "${_build_stdout}\n${_build_stderr}")
endif()
set(_built_header "${_build}/SDK/include/pnglibconf.h")
if(NOT EXISTS "${_built_header}" OR
   EXISTS "${_build}/GENINCDIR/pnglibconf.h")
    message(FATAL_ERROR
        "pnglibconf fixture did not publish exactly the SDK-only header")
endif()
file(READ "${_built_header}" _built_actual)
if(NOT _built_actual STREQUAL _expected_content)
    message(FATAL_ERROR "pnglibconf fixture output differs from the legacy recipe")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}"
        --target workbench-libs-png-generated
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
if(NOT _noop_result EQUAL 0 OR
   _noop_stdout MATCHES "Generating libpng pnglibconf.h")
    message(FATAL_ERROR
        "pnglibconf fixture did not settle to a no-op\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()

file(REMOVE_RECURSE "${_fixture}" "${_build_root}")
message(STATUS "libpng pnglibconf staging test passed")
