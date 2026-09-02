cmake_minimum_required(VERSION 3.22)
project(FullGenmoduleAbiContract NONE)

include("${CMAKE_CURRENT_LIST_DIR}/../../GenmoduleTargets.cmake")

set(_includes_stamp "${CMAKE_CURRENT_BINARY_DIR}/includes.stamp")
set(_fd_stamp "${CMAKE_CURRENT_BINARY_DIR}/fd.stamp")
add_custom_command(
    OUTPUT "${_includes_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_includes_stamp}"
    VERBATIM)
add_custom_command(
    OUTPUT "${_fd_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_fd_stamp}"
    VERBATIM)
add_custom_target(real-includes DEPENDS "${_includes_stamp}")
add_custom_target(real-fd DEPENDS "${_fd_stamp}")

_aros_bind_genmodule_abi_targets(probe real-includes real-fd)

get_property(_includes_deps TARGET probe-includes
    PROPERTY MANUALLY_ADDED_DEPENDENCIES)
foreach(_required real-includes real-fd)
    if(NOT _required IN_LIST _includes_deps)
        message(FATAL_ERROR
            "probe-includes does not depend on ${_required}: ${_includes_deps}")
    endif()
endforeach()
get_property(_fd_deps TARGET probe-fd PROPERTY MANUALLY_ADDED_DEPENDENCIES)
if(NOT "real-fd" IN_LIST _fd_deps)
    message(FATAL_ERROR "probe-fd does not depend on real-fd: ${_fd_deps}")
endif()
