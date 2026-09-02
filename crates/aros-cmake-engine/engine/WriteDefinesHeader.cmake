cmake_minimum_required(VERSION 3.22)

if(NOT DEFINED BINARY_ROOT OR NOT DEFINED OUTPUT OR NOT DEFINED DEFINES)
    message(FATAL_ERROR
        "WriteDefinesHeader.cmake requires BINARY_ROOT, OUTPUT and DEFINES")
endif()

cmake_path(ABSOLUTE_PATH BINARY_ROOT NORMALIZE OUTPUT_VARIABLE _binary_root)
cmake_path(ABSOLUTE_PATH OUTPUT
    BASE_DIRECTORY "${_binary_root}" NORMALIZE OUTPUT_VARIABLE _output)
cmake_path(IS_PREFIX _binary_root "${_output}" NORMALIZE _inside_build)
if(NOT _inside_build OR _output STREQUAL _binary_root)
    message(FATAL_ERROR
        "defines-header output escapes the build tree: ${_output}")
endif()

set(_content "")
set(_names "")
foreach(_definition IN LISTS DEFINES)
    if(NOT _definition MATCHES
       "^([A-Za-z_][A-Za-z0-9_]*) ([A-Za-z0-9_+.,:/<>=!&|%*~?@#^()-]+)$")
        message(FATAL_ERROR
            "invalid literal define payload: '${_definition}'")
    endif()
    set(_name "${CMAKE_MATCH_1}")
    if(_name IN_LIST _names)
        message(FATAL_ERROR "duplicate literal define: ${_name}")
    endif()
    list(APPEND _names "${_name}")
    string(APPEND _content "#define ${_definition}\n")
endforeach()
if(NOT _names)
    message(FATAL_ERROR "a defines header must contain at least one definition")
endif()

get_filename_component(_output_dir "${_output}" DIRECTORY)
file(MAKE_DIRECTORY "${_output_dir}")
string(SHA256 _temporary_key "${_output}")
set(_temporary "${_output}.${_temporary_key}.tmp")
file(WRITE "${_temporary}" "${_content}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${_temporary}" "${_output}"
    COMMAND_ERROR_IS_FATAL ANY)
file(REMOVE "${_temporary}")
