cmake_minimum_required(VERSION 3.22)

if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_root "$ENV{TMPDIR}")
else()
    set(_temp_root "/tmp")
endif()
string(RANDOM LENGTH 16 ALPHABET 0123456789abcdef _suffix)
set(_fixture "${_temp_root}/aros-define-header-${_suffix}")
set(_output "${_fixture}/nested/opt_test.h")
set(_writer "${CMAKE_CURRENT_LIST_DIR}/../WriteDefinesHeader.cmake")
file(MAKE_DIRECTORY "${_fixture}")

set(_defines "AH_HAS_RF 1;AH_MASK 0x20;AH_MODE TOKEN_2")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DBINARY_ROOT=${_fixture}"
        "-DOUTPUT=${_output}"
        "-DDEFINES=${_defines}"
        -P "${_writer}"
    RESULT_VARIABLE _first_result
    OUTPUT_VARIABLE _first_stdout
    ERROR_VARIABLE _first_stderr)
if(NOT _first_result EQUAL 0)
    message(FATAL_ERROR
        "initial defines-header write failed (${_first_result})\n"
        "${_first_stdout}\n${_first_stderr}")
endif()

file(READ "${_output}" _actual)
set(_expected
    "#define AH_HAS_RF 1\n#define AH_MASK 0x20\n#define AH_MODE TOKEN_2\n")
if(NOT _actual STREQUAL _expected)
    message(FATAL_ERROR
        "literal defines-header mismatch:\nexpected=${_expected}\nactual=${_actual}")
endif()

file(TIMESTAMP "${_output}" _before_noop "%s")
execute_process(COMMAND "${CMAKE_COMMAND}" -E sleep 1.1)
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DBINARY_ROOT=${_fixture}"
        "-DOUTPUT=${_output}"
        "-DDEFINES=${_defines}"
        -P "${_writer}"
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
if(NOT _noop_result EQUAL 0)
    message(FATAL_ERROR
        "no-op defines-header write failed (${_noop_result})\n"
        "${_noop_stdout}\n${_noop_stderr}")
endif()
file(TIMESTAMP "${_output}" _after_noop "%s")
if(NOT _before_noop STREQUAL _after_noop)
    message(FATAL_ERROR
        "copy_if_different replaced an unchanged defines header")
endif()

set(_outside "${_fixture}-outside.h")
execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DBINARY_ROOT=${_fixture}"
        "-DOUTPUT=${_outside}"
        "-DDEFINES=AH_ESCAPE 1"
        -P "${_writer}"
    RESULT_VARIABLE _escape_result
    OUTPUT_QUIET ERROR_QUIET)
if(_escape_result EQUAL 0 OR EXISTS "${_outside}")
    message(FATAL_ERROR "writer accepted an output outside BINARY_ROOT")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}"
        "-DBINARY_ROOT=${_fixture}"
        "-DOUTPUT=${_output}"
        "-DDEFINES=AH_BAD $VALUE"
        -P "${_writer}"
    RESULT_VARIABLE _unsafe_result
    OUTPUT_QUIET ERROR_QUIET)
if(_unsafe_result EQUAL 0)
    message(FATAL_ERROR "writer accepted an unsafe literal define")
endif()

# Exercise the complete custom-command contract under Ninja. The standalone
# writer avoids needless replacements, while the build rule advances its
# declared output after an input change so every generator settles afterward.
set(_build_source "${CMAKE_CURRENT_LIST_DIR}/define-header-build")
set(_build_root "${_temp_root}/aros-define-header-build-${_suffix}")
set(_build "${_build_root}/build")
set(_dependency "${_build_root}/input.inc")
file(MAKE_DIRECTORY "${_build_root}")
file(WRITE "${_dependency}" "literal recipe input\n")

execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_build_source}" -B "${_build}"
        -G Ninja "-DTEST_DEPENDENCY=${_dependency}"
    RESULT_VARIABLE _configure_result
    OUTPUT_VARIABLE _configure_stdout
    ERROR_VARIABLE _configure_stderr)
if(NOT _configure_result EQUAL 0)
    message(FATAL_ERROR
        "defines-header fixture configure failed (${_configure_result})\n"
        "${_configure_stdout}\n${_configure_stderr}")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target test-defines
    RESULT_VARIABLE _initial_build_result
    OUTPUT_VARIABLE _initial_build_stdout
    ERROR_VARIABLE _initial_build_stderr)
if(NOT _initial_build_result EQUAL 0)
    message(FATAL_ERROR
        "initial defines-header fixture build failed (${_initial_build_result})\n"
        "${_initial_build_stdout}\n${_initial_build_stderr}")
endif()

set(_build_output "${_build}/nested/opt_test.h")
if(NOT EXISTS "${_build_output}")
    message(FATAL_ERROR "defines-header build did not create its declared output")
endif()
file(READ "${_build_output}" _build_actual)
if(NOT _build_actual STREQUAL _expected)
    message(FATAL_ERROR
        "built defines-header mismatch:\nexpected=${_expected}\nactual=${_build_actual}")
endif()

file(TIMESTAMP "${_build_output}" _header_before_touch "%s")
execute_process(COMMAND "${CMAKE_COMMAND}" -E sleep 1.1)
execute_process(COMMAND "${CMAKE_COMMAND}" -E touch "${_dependency}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target test-defines
    RESULT_VARIABLE _rebuild_result
    OUTPUT_VARIABLE _rebuild_stdout
    ERROR_VARIABLE _rebuild_stderr)
if(NOT _rebuild_result EQUAL 0 OR
   NOT _rebuild_stdout MATCHES "Generating literal defines header")
    message(FATAL_ERROR
        "dependency touch did not rebuild the defines header once (${_rebuild_result})\n"
        "${_rebuild_stdout}\n${_rebuild_stderr}")
endif()
file(TIMESTAMP "${_build_output}" _header_after_touch "%s")
if(_header_before_touch STREQUAL _header_after_touch)
    message(FATAL_ERROR
        "dependency-only rebuild did not advance the declared output timestamp")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target test-defines
    RESULT_VARIABLE _settled_result
    OUTPUT_VARIABLE _settled_stdout
    ERROR_VARIABLE _settled_stderr)
if(NOT _settled_result EQUAL 0 OR NOT _settled_stdout MATCHES "no work to do")
    message(FATAL_ERROR
        "defines-header build did not settle to a no-op (${_settled_result})\n"
        "${_settled_stdout}\n${_settled_stderr}")
endif()

# Unix Makefiles does not infer a rebuild from a missing BYPRODUCT when only a
# separate stamp is owned. Keep the header itself as the declared output and
# prove that deleting it causes exactly one repair build with this generator.
set(_make_root "${_temp_root}/aros-define-header-make-${_suffix}")
set(_make_build "${_make_root}/build")
set(_make_dependency "${_make_root}/input.inc")
file(MAKE_DIRECTORY "${_make_root}")
file(WRITE "${_make_dependency}" "literal recipe input\n")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -S "${_build_source}" -B "${_make_build}"
        -G "Unix Makefiles" "-DTEST_DEPENDENCY=${_make_dependency}"
    RESULT_VARIABLE _make_configure_result
    OUTPUT_VARIABLE _make_configure_stdout
    ERROR_VARIABLE _make_configure_stderr)
if(NOT _make_configure_result EQUAL 0)
    message(FATAL_ERROR
        "Makefile defines-header fixture configure failed (${_make_configure_result})\n"
        "${_make_configure_stdout}\n${_make_configure_stderr}")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_make_build}" --target test-defines
    RESULT_VARIABLE _make_initial_result
    OUTPUT_VARIABLE _make_initial_stdout
    ERROR_VARIABLE _make_initial_stderr)
set(_make_output "${_make_build}/nested/opt_test.h")
if(NOT _make_initial_result EQUAL 0 OR NOT EXISTS "${_make_output}")
    message(FATAL_ERROR
        "initial Makefile defines-header build failed (${_make_initial_result})\n"
        "${_make_initial_stdout}\n${_make_initial_stderr}")
endif()
file(RENAME "${_make_output}" "${_make_root}/deleted-opt_test.h")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_make_build}" --target test-defines
    RESULT_VARIABLE _make_repair_result
    OUTPUT_VARIABLE _make_repair_stdout
    ERROR_VARIABLE _make_repair_stderr)
if(NOT _make_repair_result EQUAL 0 OR NOT EXISTS "${_make_output}" OR
   NOT _make_repair_stdout MATCHES "Generating literal defines header")
    message(FATAL_ERROR
        "Makefile build did not repair a deleted header (${_make_repair_result})\n"
        "${_make_repair_stdout}\n${_make_repair_stderr}")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_make_build}" --target test-defines
    RESULT_VARIABLE _make_settled_result
    OUTPUT_VARIABLE _make_settled_stdout
    ERROR_VARIABLE _make_settled_stderr)
if(NOT _make_settled_result EQUAL 0 OR
   _make_settled_stdout MATCHES "Generating literal defines header")
    message(FATAL_ERROR
        "repaired Makefile header did not settle (${_make_settled_result})\n"
        "${_make_settled_stdout}\n${_make_settled_stderr}")
endif()

file(REMOVE_RECURSE "${_fixture}")
file(REMOVE_RECURSE "${_build_root}")
file(REMOVE_RECURSE "${_make_root}")
message(STATUS "literal defines-header writer/build test passed")
