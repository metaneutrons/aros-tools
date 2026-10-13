cmake_minimum_required(VERSION 3.22)
include("${CMAKE_CURRENT_LIST_DIR}/VerifySdkCompanionElf.cmake")

function(_aros_literal_runner_assert_regular path label executable)
    find_program(_literal_runner_test_program NAMES test PATHS /usr/bin /bin NO_DEFAULT_PATH NO_CACHE REQUIRED)
    execute_process(COMMAND "${_literal_runner_test_program}" -f "${path}" RESULT_VARIABLE result)
    if(NOT result EQUAL 0)
        message(FATAL_ERROR "Literal object runner: ${label} is not a regular file: ${path}")
    endif()
    if(executable)
        execute_process(COMMAND "${_literal_runner_test_program}" -x "${path}" RESULT_VARIABLE result)
        if(NOT result EQUAL 0)
            message(FATAL_ERROR "Literal object runner: ${label} is not executable: ${path}")
        endif()
    endif()
endfunction()

function(_aros_literal_runner_assert_root root label)
    if(NOT IS_ABSOLUTE "${root}" OR NOT IS_DIRECTORY "${root}")
        message(FATAL_ERROR "Literal object runner: ${label} is not an existing directory")
    endif()
    if(IS_SYMLINK "${root}")
        message(FATAL_ERROR "Literal object runner: symlink ${label} refused: ${root}")
    endif()
endfunction()

function(_aros_literal_runner_check_absolute_chain path label)
    set(cursor "${path}")
    while(TRUE)
        if(IS_SYMLINK "${cursor}")
            message(FATAL_ERROR "Literal object runner: symlink ${label} path refused: ${cursor}")
        endif()
        cmake_path(GET cursor PARENT_PATH parent)
        if(parent STREQUAL cursor)
            break()
        endif()
        set(cursor "${parent}")
    endwhile()
endfunction()

function(_aros_literal_runner_check_path path boundary label)
    cmake_path(IS_PREFIX boundary "${path}" NORMALIZE inside)
    if(NOT inside OR path STREQUAL boundary)
        message(FATAL_ERROR "Literal object runner: ${label} escapes its declared root")
    endif()
    set(cursor "${path}")
    while(NOT cursor STREQUAL boundary)
        if(IS_SYMLINK "${cursor}")
            message(FATAL_ERROR "Literal object runner: symlink ${label} path refused: ${cursor}")
        endif()
        if(EXISTS "${cursor}" AND NOT cursor STREQUAL path AND NOT IS_DIRECTORY "${cursor}")
            message(FATAL_ERROR "Literal object runner: non-directory ${label} parent: ${cursor}")
        endif()
        cmake_path(GET cursor PARENT_PATH parent)
        if(parent STREQUAL cursor)
            message(FATAL_ERROR "Literal object runner: invalid ${label} root boundary")
        endif()
        set(cursor "${parent}")
    endwhile()
endfunction()

function(_aros_literal_runner_validate_declared_path raw role)
    if(IS_ABSOLUTE "${raw}")
        set(path "${raw}")
    else()
        cmake_path(ABSOLUTE_PATH raw BASE_DIRECTORY "${SOURCE_DIRECTORY}"
            NORMALIZE OUTPUT_VARIABLE path)
    endif()
    cmake_path(ABSOLUTE_PATH path NORMALIZE OUTPUT_VARIABLE path)
    cmake_path(IS_PREFIX SOURCE_ROOT "${path}" NORMALIZE in_source)
    cmake_path(IS_PREFIX BUILD_ROOT "${path}" NORMALIZE in_build)
    if(NOT in_source AND NOT in_build)
        message(FATAL_ERROR
            "Literal object runner: ${role} path left the declared source/build roots: ${path}")
    endif()
    if(in_source)
        set(boundary "${SOURCE_ROOT}")
    else()
        set(boundary "${BUILD_ROOT}")
    endif()
    if(NOT path STREQUAL boundary)
        _aros_literal_runner_check_path("${path}" "${boundary}" "declared ${role}")
        if(EXISTS "${path}" AND NOT IS_DIRECTORY "${path}")
            message(FATAL_ERROR "Literal object runner: declared ${role} path is not a directory: ${path}")
        endif()
    endif()
endfunction()

function(_aros_literal_runner_optional_regular path label)
    if(IS_SYMLINK "${path}")
        message(FATAL_ERROR "Literal object runner: symlink ${label} refused: ${path}")
    endif()
    if(EXISTS "${path}")
        _aros_literal_runner_assert_regular("${path}" "${label}" FALSE)
    endif()
endfunction()

if(NOT DEFINED COMMAND_FILE OR "${COMMAND_FILE}" STREQUAL "" OR
   NOT DEFINED COMMAND_ROOT OR "${COMMAND_ROOT}" STREQUAL "" OR
   NOT DEFINED VERIFY_ONLY OR NOT VERIFY_ONLY MATCHES "^(ON|OFF)$")
    message(FATAL_ERROR "Literal object runner requires a command file and verify mode")
endif()
if(NOT DEFINED COMMAND_SHA256 OR NOT COMMAND_SHA256 MATCHES "^[0-9a-f]+$")
    message(FATAL_ERROR "Literal object runner requires a valid command digest")
endif()
string(LENGTH "${COMMAND_SHA256}" command_sha_length)
if(NOT command_sha_length EQUAL 64)
    message(FATAL_ERROR "Literal object runner requires a valid command digest")
endif()
cmake_path(ABSOLUTE_PATH COMMAND_ROOT NORMALIZE OUTPUT_VARIABLE command_root)
cmake_path(ABSOLUTE_PATH COMMAND_FILE NORMALIZE OUTPUT_VARIABLE command_file)
_aros_literal_runner_assert_root("${command_root}" "command root")
_aros_literal_runner_check_path("${command_file}" "${command_root}" "command file")
_aros_literal_runner_assert_regular("${command_file}" "command file" FALSE)
if(IS_SYMLINK "${command_file}")
    message(FATAL_ERROR "Literal object runner refuses a symlink command file")
endif()
file(SHA256 "${command_file}" actual_command_sha256)
if(NOT actual_command_sha256 STREQUAL COMMAND_SHA256)
    message(FATAL_ERROR "Literal object runner refuses a changed command file")
endif()
include("${command_file}")

foreach(key SOURCE SOURCE_ROOT BUILD_ROOT SOURCE_DIRECTORY OUTPUT COMPILER LANGUAGE)
    if(NOT DEFINED ${key} OR "${${key}}" STREQUAL "" OR
       "${${key}}" MATCHES "[;\n\r$]")
        message(FATAL_ERROR "Literal object runner requires one safe ${key}")
    endif()
endforeach()
if(NOT DEFINED ARGUMENTS)
    set(ARGUMENTS)
endif()
if(NOT DEFINED OUTPUT_MODE)
    set(OUTPUT_MODE OBJECT)
endif()
set(source_dependency_output "")
if(DEFINED SOURCE_DEPENDENCY_OUTPUT)
    if(NOT OUTPUT_MODE STREQUAL "OBJECT")
        message(FATAL_ERROR
            "Literal object runner: SOURCE_DEPENDENCY_OUTPUT is only valid for OBJECT")
    endif()
    if("${SOURCE_DEPENDENCY_OUTPUT}" STREQUAL "")
        message(FATAL_ERROR
            "Literal object runner: SOURCE_DEPENDENCY_OUTPUT requires one absolute path")
    endif()
    if(NOT IS_ABSOLUTE "${SOURCE_DEPENDENCY_OUTPUT}" OR
       "${SOURCE_DEPENDENCY_OUTPUT}" MATCHES "[;\n\r$]")
        message(FATAL_ERROR
            "Literal object runner: source dependency output must be one safe absolute path")
    endif()
    cmake_path(ABSOLUTE_PATH SOURCE_DEPENDENCY_OUTPUT NORMALIZE
        OUTPUT_VARIABLE source_dependency_output)
endif()
if(OUTPUT_MODE STREQUAL "OBJECT")
    set(output_extension "o")
    set(compile_mode "-c")
elseif(OUTPUT_MODE STREQUAL "ASSEMBLY" AND LANGUAGE STREQUAL "C")
    set(output_extension "s")
    set(compile_mode "-S")
elseif(OUTPUT_MODE STREQUAL "DEPENDENCIES")
    set(output_extension "d")
    if(NOT DEFINED DEPENDENCY_TARGET_OUTPUT OR
       "${DEPENDENCY_TARGET_OUTPUT}" STREQUAL "" OR
       NOT DEFINED DEPENDENCY_TRACKER_FILE OR
       "${DEPENDENCY_TRACKER_FILE}" STREQUAL "" OR DEFINED DEPFILE)
        message(FATAL_ERROR
            "Literal object runner: dependency mode requires DEPENDENCY_TARGET_OUTPUT and DEPENDENCY_TRACKER_FILE")
    endif()
else()
    message(FATAL_ERROR "Literal object runner received invalid output mode")
endif()
if(NOT OUTPUT_MODE STREQUAL "DEPENDENCIES" AND
   (NOT DEFINED DEPFILE OR "${DEPFILE}" STREQUAL "" OR
    DEFINED DEPENDENCY_TARGET_OUTPUT OR DEFINED DEPENDENCY_TRACKER_FILE))
    message(FATAL_ERROR "Literal object runner requires one ordinary DEPFILE")
endif()

_aros_literal_runner_assert_root("${SOURCE_ROOT}" "source root")
_aros_literal_runner_assert_root("${BUILD_ROOT}" "build root")
_aros_literal_runner_check_absolute_chain("${COMPILER}" "compiler driver")
_aros_literal_runner_assert_regular("${COMPILER}" "compiler driver" TRUE)
if(NOT DEFINED COMPILER_SHA256 OR NOT COMPILER_SHA256 MATCHES "^[0-9a-f]+$")
    message(FATAL_ERROR "Literal object runner requires a compiler digest")
endif()
string(LENGTH "${COMPILER_SHA256}" compiler_sha_length)
if(NOT compiler_sha_length EQUAL 64)
    message(FATAL_ERROR "Literal object runner requires a complete compiler digest")
endif()
file(SHA256 "${COMPILER}" actual_compiler_sha256)
if(NOT actual_compiler_sha256 STREQUAL COMPILER_SHA256)
    message(FATAL_ERROR "Literal object runner: compiler driver bytes changed")
endif()

if(NOT IS_ABSOLUTE "${SOURCE}" OR NOT IS_ABSOLUTE "${OUTPUT}" OR
   NOT IS_ABSOLUTE "${COMPILER}" OR NOT IS_ABSOLUTE "${SOURCE_DIRECTORY}" OR
   (NOT OUTPUT_MODE STREQUAL "DEPENDENCIES" AND NOT IS_ABSOLUTE "${DEPFILE}") OR
   (NOT source_dependency_output STREQUAL "" AND
    NOT IS_ABSOLUTE "${source_dependency_output}") OR
   (OUTPUT_MODE STREQUAL "DEPENDENCIES" AND
    (NOT IS_ABSOLUTE "${DEPENDENCY_TARGET_OUTPUT}" OR
     NOT IS_ABSOLUTE "${DEPENDENCY_TRACKER_FILE}")))
    message(FATAL_ERROR "Literal object runner paths must be absolute")
endif()
cmake_path(ABSOLUTE_PATH SOURCE NORMALIZE OUTPUT_VARIABLE SOURCE)
cmake_path(ABSOLUTE_PATH OUTPUT NORMALIZE OUTPUT_VARIABLE OUTPUT)
if(NOT OUTPUT_MODE STREQUAL "DEPENDENCIES")
    cmake_path(ABSOLUTE_PATH DEPFILE NORMALIZE OUTPUT_VARIABLE DEPFILE)
else()
    cmake_path(ABSOLUTE_PATH DEPENDENCY_TARGET_OUTPUT NORMALIZE
        OUTPUT_VARIABLE DEPENDENCY_TARGET_OUTPUT)
    cmake_path(ABSOLUTE_PATH DEPENDENCY_TRACKER_FILE NORMALIZE
        OUTPUT_VARIABLE DEPENDENCY_TRACKER_FILE)
endif()
cmake_path(ABSOLUTE_PATH SOURCE_DIRECTORY NORMALIZE OUTPUT_VARIABLE SOURCE_DIRECTORY)
cmake_path(IS_PREFIX SOURCE_ROOT "${SOURCE}" NORMALIZE source_inside)
cmake_path(IS_PREFIX SOURCE_ROOT "${SOURCE_DIRECTORY}" NORMALIZE workdir_inside)
cmake_path(IS_PREFIX BUILD_ROOT "${OUTPUT}" NORMALIZE output_in_build)
if(NOT OUTPUT_MODE STREQUAL "DEPENDENCIES")
    cmake_path(IS_PREFIX BUILD_ROOT "${DEPFILE}" NORMALIZE depfile_in_build)
endif()
cmake_path(GET SOURCE FILENAME source_name)
cmake_path(GET SOURCE PARENT_PATH source_parent)
cmake_path(GET OUTPUT FILENAME output_name)
set(gen_root "${BUILD_ROOT}/gen")
cmake_path(IS_PREFIX gen_root "${OUTPUT}" NORMALIZE output_in_gen)
if(NOT source_inside OR SOURCE STREQUAL SOURCE_ROOT OR NOT workdir_inside OR
   NOT SOURCE_DIRECTORY STREQUAL source_parent OR
   NOT source_name MATCHES "\.(c|cpp)$" OR
   NOT output_in_build OR NOT output_in_gen OR OUTPUT STREQUAL gen_root OR
   NOT output_name MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*\.${output_extension}$")
    message(FATAL_ERROR "Literal object runner received invalid source/output roles")
endif()
if(OUTPUT_MODE STREQUAL "DEPENDENCIES")
    cmake_path(IS_PREFIX BUILD_ROOT "${DEPENDENCY_TARGET_OUTPUT}" NORMALIZE
        dependency_target_in_build)
    cmake_path(IS_PREFIX gen_root "${DEPENDENCY_TARGET_OUTPUT}" NORMALIZE
        dependency_target_in_gen)
    cmake_path(GET OUTPUT PARENT_PATH dependency_output_parent)
    cmake_path(GET DEPENDENCY_TARGET_OUTPUT PARENT_PATH dependency_target_parent)
    cmake_path(GET DEPENDENCY_TARGET_OUTPUT FILENAME dependency_target_name)
    string(REGEX REPLACE "[.]d$" ".o" expected_dependency_target_name
        "${output_name}")
    if(NOT dependency_target_in_build OR NOT dependency_target_in_gen OR
       DEPENDENCY_TARGET_OUTPUT STREQUAL gen_root OR
       NOT dependency_target_name MATCHES "^[A-Za-z0-9][A-Za-z0-9_.+-]*[.]o$" OR
       NOT dependency_output_parent STREQUAL dependency_target_parent OR
       NOT dependency_target_name STREQUAL expected_dependency_target_name)
        message(FATAL_ERROR
            "Literal object runner received an invalid dependency output/target pair")
    endif()
    string(SHA256 dependency_tracker_key "${OUTPUT}")
    set(expected_dependency_tracker_file
        "${command_root}/CMakeFiles/aros-literal-dependency-tracker-${dependency_tracker_key}.d")
    if(NOT DEPENDENCY_TRACKER_FILE STREQUAL expected_dependency_tracker_file)
        message(FATAL_ERROR
            "Literal object runner received an invalid dependency tracker role")
    endif()
    _aros_literal_runner_check_path(
        "${DEPENDENCY_TRACKER_FILE}" "${command_root}" "dependency tracker")
    _aros_literal_runner_optional_regular(
        "${DEPENDENCY_TRACKER_FILE}" "dependency tracker")
else()
    if(NOT depfile_in_build OR NOT DEPFILE STREQUAL "${OUTPUT}.d")
        message(FATAL_ERROR "Literal object runner received invalid dependency output role")
    endif()
endif()
if(NOT source_dependency_output STREQUAL "")
    string(REGEX REPLACE "[.]o$" ".d" expected_source_dependency_output "${OUTPUT}")
    if(NOT source_dependency_output STREQUAL expected_source_dependency_output)
        message(FATAL_ERROR
            "Literal object runner received an invalid source dependency output/object pair")
    endif()
    cmake_path(IS_PREFIX BUILD_ROOT "${source_dependency_output}" NORMALIZE
        source_dependency_in_build)
    cmake_path(IS_PREFIX gen_root "${source_dependency_output}" NORMALIZE
        source_dependency_in_gen)
    if(NOT source_dependency_in_build OR NOT source_dependency_in_gen OR
       source_dependency_output STREQUAL gen_root)
        message(FATAL_ERROR
            "Literal object runner received a source dependency output outside build gen")
    endif()
endif()

_aros_literal_runner_check_path("${SOURCE}" "${SOURCE_ROOT}" "source")
_aros_literal_runner_assert_regular("${SOURCE}" "source" FALSE)
if(OUTPUT_MODE STREQUAL "DEPENDENCIES")
    _aros_literal_runner_check_path("${OUTPUT}" "${BUILD_ROOT}" "dependency output")
    _aros_literal_runner_check_path("${DEPENDENCY_TARGET_OUTPUT}" "${BUILD_ROOT}"
        "dependency target output")
    _aros_literal_runner_optional_regular("${OUTPUT}" "dependency output")
    _aros_literal_runner_optional_regular("${DEPENDENCY_TARGET_OUTPUT}"
        "dependency target output")
else()
    _aros_literal_runner_check_path("${OUTPUT}" "${BUILD_ROOT}" "object output")
    _aros_literal_runner_check_path("${DEPFILE}" "${BUILD_ROOT}" "dependency output")
    _aros_literal_runner_optional_regular("${OUTPUT}" "object output")
    _aros_literal_runner_optional_regular("${DEPFILE}" "dependency output")
    if(NOT source_dependency_output STREQUAL "")
        _aros_literal_runner_check_path(
            "${source_dependency_output}" "${BUILD_ROOT}" "source dependency output")
        _aros_literal_runner_optional_regular(
            "${source_dependency_output}" "source dependency output")
    endif()
endif()

set(expect_path_for "")
foreach(argument IN LISTS ARGUMENTS)
    if(NOT expect_path_for STREQUAL "")
        _aros_literal_runner_validate_declared_path("${argument}" "${expect_path_for}")
        set(expect_path_for "")
    elseif(argument STREQUAL "-I" OR argument STREQUAL "-isystem" OR
           argument STREQUAL "-iquote" OR argument STREQUAL "--sysroot" OR
           argument STREQUAL "-isysroot")
        set(expect_path_for "${argument}")
    elseif(argument MATCHES "^-I(.+)$")
        _aros_literal_runner_validate_declared_path("${CMAKE_MATCH_1}" "-I")
    elseif(argument MATCHES "^--sysroot=(.+)$")
        _aros_literal_runner_validate_declared_path("${CMAKE_MATCH_1}" "--sysroot")
    endif()
endforeach()
if(NOT expect_path_for STREQUAL "")
    message(FATAL_ERROR "Literal object runner: ${expect_path_for} is missing its validated path")
endif()

if(VERIFY_ONLY)
    if(DEFINED EXPECTED_ELF_CLASS AND EXISTS "${OUTPUT}")
        verify_sdk_companion_elf("${OUTPUT}")
    endif()
    return()
endif()

cmake_path(GET OUTPUT PARENT_PATH output_parent)
file(MAKE_DIRECTORY "${output_parent}")
if(OUTPUT_MODE STREQUAL "DEPENDENCIES")
    _aros_literal_runner_check_path("${OUTPUT}" "${BUILD_ROOT}" "dependency output")
    _aros_literal_runner_check_path("${DEPENDENCY_TARGET_OUTPUT}" "${BUILD_ROOT}"
        "dependency target output")
    _aros_literal_runner_check_path(
        "${DEPENDENCY_TRACKER_FILE}" "${command_root}" "dependency tracker")
else()
    _aros_literal_runner_check_path("${OUTPUT}" "${BUILD_ROOT}" "object output")
    _aros_literal_runner_check_path("${DEPFILE}" "${BUILD_ROOT}" "dependency output")
    if(NOT source_dependency_output STREQUAL "")
        _aros_literal_runner_check_path(
            "${source_dependency_output}" "${BUILD_ROOT}" "source dependency output")
        _aros_literal_runner_optional_regular(
            "${source_dependency_output}" "source dependency output")
        if(NOT EXISTS "${source_dependency_output}")
            message(FATAL_ERROR
                "Literal object runner: source dependency output is missing: ${source_dependency_output}")
        endif()
        file(SIZE "${source_dependency_output}" source_dependency_size)
        if(source_dependency_size GREATER 4194304)
            message(FATAL_ERROR
                "Literal object runner: source dependency output exceeds 4 MiB: ${source_dependency_output}")
        endif()
    endif()
endif()

string(RANDOM LENGTH 20 ALPHABET 0123456789abcdef suffix)
if(OUTPUT_MODE STREQUAL "DEPENDENCIES")
    set(temporary_output "${OUTPUT}.literal-${suffix}.tmp")
    set(temporary_tracker "${DEPENDENCY_TRACKER_FILE}.literal-${suffix}.tmp")
    _aros_literal_runner_check_path("${temporary_output}" "${BUILD_ROOT}"
        "temporary dependency output")
    _aros_literal_runner_check_path(
        "${temporary_tracker}" "${command_root}" "temporary dependency tracker")
    if(EXISTS "${temporary_output}" OR IS_SYMLINK "${temporary_output}" OR
       EXISTS "${temporary_tracker}" OR IS_SYMLINK "${temporary_tracker}")
        message(FATAL_ERROR "Literal object runner found a temporary output collision")
    endif()
    execute_process(
        COMMAND "${COMPILER}" ${ARGUMENTS}
            -M -MF "${temporary_output}" -MQ "${DEPENDENCY_TARGET_OUTPUT}" "${SOURCE}"
        WORKING_DIRECTORY "${SOURCE_DIRECTORY}"
        RESULT_VARIABLE compiler_result
        OUTPUT_VARIABLE compiler_stdout
        ERROR_VARIABLE compiler_stderr)
    if(NOT compiler_result EQUAL 0)
        file(REMOVE "${temporary_output}" "${temporary_tracker}")
        message(FATAL_ERROR
            "Literal dependency compiler failed (${compiler_result}) for ${SOURCE}\n"
            "${compiler_stdout}${compiler_stderr}")
    endif()
    _aros_literal_runner_check_path(
        "${temporary_output}" "${BUILD_ROOT}" "temporary dependency output")
    _aros_literal_runner_assert_regular("${temporary_output}"
        "compiler dependency output" FALSE)

    execute_process(
        COMMAND "${COMPILER}" ${ARGUMENTS}
            -M -MF "${temporary_tracker}" -MQ "${OUTPUT}" "${SOURCE}"
        WORKING_DIRECTORY "${SOURCE_DIRECTORY}"
        RESULT_VARIABLE tracker_compiler_result
        OUTPUT_VARIABLE tracker_compiler_stdout
        ERROR_VARIABLE tracker_compiler_stderr)
    if(NOT tracker_compiler_result EQUAL 0)
        file(REMOVE "${temporary_output}" "${temporary_tracker}")
        message(FATAL_ERROR
            "Literal dependency tracker compiler failed (${tracker_compiler_result}) for ${SOURCE}\n"
            "${tracker_compiler_stdout}${tracker_compiler_stderr}")
    endif()
    _aros_literal_runner_check_path("${temporary_output}" "${BUILD_ROOT}"
        "temporary dependency output")
    _aros_literal_runner_assert_regular("${temporary_output}"
        "compiler dependency output" FALSE)
    _aros_literal_runner_check_path(
        "${temporary_tracker}" "${command_root}" "temporary dependency tracker")
    _aros_literal_runner_assert_regular("${temporary_tracker}"
        "compiler dependency tracker" FALSE)
    _aros_literal_runner_check_path("${OUTPUT}" "${BUILD_ROOT}" "dependency output")
    _aros_literal_runner_check_path(
        "${DEPENDENCY_TRACKER_FILE}" "${command_root}" "dependency tracker")

    # Both compiler contexts have completed before either published file is
    # replaced. Publish the private Ninja tracker first and the public .d last.
    file(RENAME "${temporary_tracker}" "${DEPENDENCY_TRACKER_FILE}"
        RESULT tracker_rename_result)
    if(NOT tracker_rename_result STREQUAL "0")
        file(REMOVE "${temporary_output}" "${temporary_tracker}")
        message(FATAL_ERROR
            "Literal object runner could not publish dependency tracker: ${tracker_rename_result}")
    endif()
    file(RENAME "${temporary_output}" "${OUTPUT}" RESULT rename_result)
    if(NOT rename_result STREQUAL "0")
        file(REMOVE "${temporary_output}")
        message(FATAL_ERROR
            "Literal object runner could not publish dependency file: ${rename_result}")
    endif()
    _aros_literal_runner_check_path("${OUTPUT}" "${BUILD_ROOT}" "dependency output")
    _aros_literal_runner_assert_regular("${OUTPUT}" "compiler dependency output" FALSE)
    _aros_literal_runner_check_path(
        "${DEPENDENCY_TRACKER_FILE}" "${command_root}" "dependency tracker")
    _aros_literal_runner_assert_regular(
        "${DEPENDENCY_TRACKER_FILE}" "compiler dependency tracker" FALSE)
    return()
endif()

set(temporary_object "${OUTPUT}.literal-${suffix}.tmp")
set(temporary_depfile "${DEPFILE}.literal-${suffix}.tmp")
_aros_literal_runner_check_path("${temporary_object}" "${BUILD_ROOT}" "temporary object")
_aros_literal_runner_check_path("${temporary_depfile}" "${BUILD_ROOT}" "temporary dependency file")
if(EXISTS "${temporary_object}" OR IS_SYMLINK "${temporary_object}" OR
   EXISTS "${temporary_depfile}" OR IS_SYMLINK "${temporary_depfile}")
    message(FATAL_ERROR "Literal object runner found a temporary output collision")
endif()

execute_process(
    COMMAND "${COMPILER}" ${ARGUMENTS}
        -MD -MF "${temporary_depfile}" -MQ "${OUTPUT}"
        ${compile_mode} "${SOURCE}" -o "${temporary_object}"
    WORKING_DIRECTORY "${SOURCE_DIRECTORY}"
    RESULT_VARIABLE compiler_result
    OUTPUT_VARIABLE compiler_stdout
    ERROR_VARIABLE compiler_stderr)
if(NOT compiler_result EQUAL 0)
    file(REMOVE "${temporary_object}" "${temporary_depfile}")
    message(FATAL_ERROR
        "Literal object compiler failed (${compiler_result}) for ${SOURCE}\n"
        "${compiler_stdout}${compiler_stderr}")
endif()

if(DEFINED EXPECTED_ELF_CLASS OR DEFINED EXPECTED_ELF_MACHINE)
    if(NOT OUTPUT_MODE STREQUAL "OBJECT" OR NOT EXPECTED_ELF_CLASS MATCHES "^[12]$" OR
       NOT EXPECTED_ELF_MACHINE MATCHES "^[1-9][0-9]*$")
        message(FATAL_ERROR "Literal object runner: invalid companion ELF contract")
    endif()
    _aros_literal_runner_check_path("${temporary_object}" "${BUILD_ROOT}" "companion object")
    _aros_literal_runner_assert_regular("${temporary_object}" "companion object" FALSE)
    verify_sdk_companion_elf("${temporary_object}")
endif()

_aros_literal_runner_check_path("${temporary_object}" "${BUILD_ROOT}" "temporary object")
_aros_literal_runner_check_path("${temporary_depfile}" "${BUILD_ROOT}" "temporary dependency file")
_aros_literal_runner_assert_regular("${temporary_object}" "compiler output" FALSE)
_aros_literal_runner_assert_regular("${temporary_depfile}" "compiler dependency file" FALSE)
if(NOT source_dependency_output STREQUAL "")
    _aros_literal_runner_check_path(
        "${source_dependency_output}" "${BUILD_ROOT}" "source dependency output")
    _aros_literal_runner_optional_regular(
        "${source_dependency_output}" "source dependency output")
    if(NOT EXISTS "${source_dependency_output}")
        file(REMOVE "${temporary_object}" "${temporary_depfile}")
        message(FATAL_ERROR
            "Literal object runner: source dependency output disappeared: ${source_dependency_output}")
    endif()
    file(SIZE "${source_dependency_output}" source_dependency_size)
    if(source_dependency_size GREATER 4194304)
        file(REMOVE "${temporary_object}" "${temporary_depfile}")
        message(FATAL_ERROR
            "Literal object runner: source dependency output exceeds 4 MiB: ${source_dependency_output}")
    endif()
    file(READ "${source_dependency_output}" source_dependency_contents LIMIT 4194305)
    string(LENGTH "${source_dependency_contents}" source_dependency_content_length)
    if(source_dependency_content_length GREATER 4194304)
        file(REMOVE "${temporary_object}" "${temporary_depfile}")
        message(FATAL_ERROR
            "Literal object runner: source dependency output exceeds 4 MiB: ${source_dependency_output}")
    endif()
    file(APPEND "${temporary_depfile}" "\n${source_dependency_contents}")
    _aros_literal_runner_assert_regular(
        "${temporary_depfile}" "combined dependency file" FALSE)
endif()
_aros_literal_runner_check_path("${OUTPUT}" "${BUILD_ROOT}" "object output")
_aros_literal_runner_check_path("${DEPFILE}" "${BUILD_ROOT}" "dependency output")

file(RENAME "${temporary_depfile}" "${DEPFILE}" RESULT rename_result)
if(NOT rename_result STREQUAL "0")
    file(REMOVE "${temporary_object}" "${temporary_depfile}")
    message(FATAL_ERROR "Literal object runner could not publish dependency file: ${rename_result}")
endif()
file(RENAME "${temporary_object}" "${OUTPUT}" RESULT rename_result)
if(NOT rename_result STREQUAL "0")
    file(REMOVE "${temporary_object}")
    message(FATAL_ERROR "Literal object runner could not publish object output: ${rename_result}")
endif()
_aros_literal_runner_check_path("${OUTPUT}" "${BUILD_ROOT}" "object output")
_aros_literal_runner_assert_regular("${OUTPUT}" "compiler output" FALSE)
