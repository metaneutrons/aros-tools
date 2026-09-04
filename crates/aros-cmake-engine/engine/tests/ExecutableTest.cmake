cmake_minimum_required(VERSION 3.22)

include("${CMAKE_CURRENT_LIST_DIR}/../Executable.cmake")

string(RANDOM LENGTH 10 ALPHABET 0123456789abcdef _suffix)
set(_root "/tmp/aros-executable-${_suffix}")
set(_executable "${_root}/tool")
set(_plain "${_root}/plain-file")
file(REMOVE_RECURSE "${_root}")
file(MAKE_DIRECTORY "${_root}")
file(WRITE "${_executable}" "#!/bin/sh\nexit 0\n")
file(CHMOD "${_executable}" PERMISSIONS
    OWNER_READ OWNER_WRITE OWNER_EXECUTE
    GROUP_READ GROUP_EXECUTE
    WORLD_READ WORLD_EXECUTE)

aros_path_is_executable("${_executable}" _executable_result)
if(NOT _executable_result)
    message(FATAL_ERROR "executable compatibility check rejected an executable file")
endif()

aros_path_is_executable("${_root}/missing" _missing_result)
if(_missing_result)
    message(FATAL_ERROR "executable compatibility check accepted a missing path")
endif()

aros_path_is_executable("${_root}" _directory_result)
if(_directory_result)
    message(FATAL_ERROR "executable compatibility check accepted a directory")
endif()

file(WRITE "${_plain}" "not executable\n")
file(CHMOD "${_plain}" PERMISSIONS
    OWNER_READ OWNER_WRITE GROUP_READ WORLD_READ)
aros_path_is_executable("${_plain}" _plain_result)
if(CMAKE_VERSION VERSION_GREATER_EQUAL "3.29")
    if(_plain_result)
        message(FATAL_ERROR "CMake 3.29+ check accepted a non-executable file")
    endif()
elseif(NOT _plain_result)
    message(FATAL_ERROR
        "pre-3.29 compatibility check must defer permission validation to invocation")
endif()

file(REMOVE_RECURSE "${_root}")
