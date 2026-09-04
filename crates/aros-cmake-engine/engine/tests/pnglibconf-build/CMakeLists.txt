cmake_minimum_required(VERSION 3.22)
project(PngLibconfBuild NONE)

if(NOT TEST_PREBUILT OR NOT EXISTS "${TEST_PREBUILT}")
    message(FATAL_ERROR "TEST_PREBUILT must name the fixture prebuilt header")
endif()

include("${CMAKE_CURRENT_LIST_DIR}/../../PngLibconf.cmake")

set(_ports_dir "${CMAKE_CURRENT_BINARY_DIR}/Ports")
set(_prebuilt
    "${_ports_dir}/libpng/libpng-1.6.58/scripts/pnglibconf.h.prebuilt")
set(_fetch_stamp "${CMAKE_CURRENT_BINARY_DIR}/libpng.fetch")
get_filename_component(_prebuilt_dir "${_prebuilt}" DIRECTORY)
add_custom_command(
    OUTPUT "${_fetch_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${_prebuilt_dir}"
    COMMAND "${CMAKE_COMMAND}" -E copy "${TEST_PREBUILT}" "${_prebuilt}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_fetch_stamp}"
    DEPENDS "${TEST_PREBUILT}"
    VERBATIM)
add_custom_target(libpng-fetch DEPENDS "${_fetch_stamp}")
set_property(TARGET libpng-fetch PROPERTY
    AROS_FETCH_COMPLETION_STAMP "${_fetch_stamp}")

add_custom_target(workbench-libs-png-generated)
aros_stage_pnglibconf(
    NAME fixture-pnglibconf-stage
    FETCH_TARGET libpng-fetch
    OWNER workbench-libs-png-generated
    PORTS_DIR "${_ports_dir}"
    SDK_INCLUDE_DIR "${CMAKE_CURRENT_BINARY_DIR}/SDK/include")
