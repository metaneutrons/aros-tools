cmake_minimum_required(VERSION 3.22)

find_program(_cmake NAMES cmake REQUIRED)
find_program(_ninja NAMES ninja ninja-build REQUIRED)
get_filename_component(_engine_dir "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(_fixture_dir "${CMAKE_CURRENT_LIST_DIR}/image-file-copy")

string(RANDOM LENGTH 12 ALPHABET 0123456789abcdef _suffix)
if(DEFINED ENV{TMPDIR} AND NOT "$ENV{TMPDIR}" STREQUAL "")
    set(_temp_base "$ENV{TMPDIR}")
else()
    set(_temp_base "/tmp")
endif()
cmake_path(ABSOLUTE_PATH _temp_base NORMALIZE OUTPUT_VARIABLE _temp_base)
set(_root "${_temp_base}/aros-image-file-copy-${_suffix} with spaces")
file(MAKE_DIRECTORY "${_root}")

function(_configure build probe out_result out_log)
    execute_process(
        COMMAND "${_cmake}" -S "${_fixture_dir}" -B "${build}" -G Ninja
            "-DAROS_ENGINE_DIR=${_engine_dir}" "-DAROS_IMAGE_COPY_PROBE=${probe}"
        RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
    set(${out_result} "${_result}" PARENT_SCOPE)
    set(${out_log} "${_stdout}\n${_stderr}" PARENT_SCOPE)
endfunction()

# Positive case: the listed files, and only those, are staged byte for byte
# into a nested image directory.
set(_build "${_root}/build")
_configure("${_build}" "" _result _log)
if(NOT _result EQUAL 0)
    message(FATAL_ERROR "image copy fixture configure failed\n${_log}")
endif()
execute_process(
    COMMAND "${_cmake}" --build "${_build}" --target stage-db
    RESULT_VARIABLE _result OUTPUT_VARIABLE _stdout ERROR_VARIABLE _stderr)
if(NOT _result EQUAL 0)
    message(FATAL_ERROR "image copy build failed\n${_stdout}\n${_stderr}")
endif()
set(_staged "${_build}/SYS/System/Network/AROSTCP/db")
foreach(_name IN ITEMS general.config netdb)
    execute_process(
        COMMAND "${_cmake}" -E compare_files
            "${_build}/source tree/db/${_name}" "${_staged}/${_name}"
        RESULT_VARIABLE _compare)
    if(NOT _compare EQUAL 0)
        message(FATAL_ERROR "staged image file differs from its source: ${_name}")
    endif()
endforeach()
file(GLOB _staged_files RELATIVE "${_staged}" "${_staged}/*")
list(SORT _staged_files)
if(NOT "${_staged_files}" STREQUAL "general.config;netdb")
    message(FATAL_ERROR "unexpected staged image files: ${_staged_files}")
endif()

# Counter cases: every one must be refused at configure time.
foreach(_probe_entry IN ITEMS
        "developer-destination|inside the Developer tree"
        "outside-image|escapes its declared root"
        "image-root|is the image root itself"
        "unknown-kind|unknown SDK file-copy KIND"
        "duplicate-kind|requires exactly one"
        "traversal|unsafe SDK file basename"
        "missing-source-file|local SDK file is missing")
    string(REPLACE "|" ";" _parts "${_probe_entry}")
    list(GET _parts 0 _probe)
    list(GET _parts 1 _expected)
    _configure("${_root}/probe-${_probe}" "${_probe}" _result _log)
    if(_result EQUAL 0)
        message(FATAL_ERROR "image copy probe ${_probe} was not refused")
    endif()
    string(FIND "${_log}" "${_expected}" _found)
    if(_found EQUAL -1)
        message(FATAL_ERROR
            "image copy probe ${_probe} failed without '${_expected}'\n${_log}")
    endif()
endforeach()

file(REMOVE_RECURSE "${_root}")
