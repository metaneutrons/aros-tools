cmake_minimum_required(VERSION 3.22)
project(FullGenmoduleAbiContract C)

# The full engine normally loads BootstrapSDK to populate broad SDK headers.
# This fixture only exercises genmodule output ownership, so keep that one
# bootstrap side effect out while loading the production AROS.cmake functions.
set(_bootstrap_stub_dir "${CMAKE_CURRENT_BINARY_DIR}/bootstrap-stub")
file(MAKE_DIRECTORY "${_bootstrap_stub_dir}")
file(WRITE "${_bootstrap_stub_dir}/BootstrapSDK.cmake"
    "function(aros_bootstrap_sdk_includes)\nendfunction()\n")
list(INSERT CMAKE_MODULE_PATH 0 "${_bootstrap_stub_dir}")

set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
include("${AROS_TEST_ENGINE_DIR}/AROS.cmake")

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

set(_miami_dir
    "${AROS_SOURCE_DIR}/workbench/network/stacks/AROSTCP/MUI.MiamiPanel")
set(_miami_config "${_miami_dir}/MUI.MiamiPanel.conf")
aros_set_module_config(probe-mui "${_miami_config}")
aros_add_library(
    TARGET MUI
    MMAKE_ID probe-mui
    DIRECTORY "${_miami_dir}"
    GENMODULE_LINKLIBS
    SOURCES "${_miami_dir}/muimiamipanel_init.c")

if(NOT TARGET probe-mui-includes OR NOT TARGET includes-MUI)
    message(FATAL_ERROR "MUI.MiamiPanel noincludes did not retain its includes aliases")
endif()
if(TARGET probe-mui-fd OR TARGET probe-mui-fd-generated)
    message(FATAL_ERROR "MUI.MiamiPanel noincludes incorrectly created an FD target")
endif()

get_target_property(_mui_archive_sources probe-mui-linklib SOURCES)
list(FILTER _mui_archive_sources INCLUDE REGEX "_getlibbase\\.c$")
list(LENGTH _mui_archive_sources _mui_getlibbase_count)
if(NOT _mui_getlibbase_count EQUAL 1)
    message(FATAL_ERROR
        "MUI.MiamiPanel noincludes lost its normal getlibbase archive source: "
        "${_mui_archive_sources}")
endif()
list(GET _mui_archive_sources 0 _mui_getlibbase_source)
if(NOT _mui_getlibbase_source MATCHES "/linklib/MUI_getlibbase\\.c$")
    message(FATAL_ERROR
        "MUI.MiamiPanel archive source is not the normal getlibbase output: "
        "${_mui_getlibbase_source}")
endif()
get_source_file_property(_mui_getlibbase_generated
    "${_mui_getlibbase_source}" GENERATED)
if(NOT _mui_getlibbase_generated)
    message(FATAL_ERROR
        "MUI.MiamiPanel getlibbase was not marked as a generated archive source")
endif()
file(WRITE "${CMAKE_BINARY_DIR}/mui-getlibbase-source.txt"
    "${_mui_getlibbase_source}")
