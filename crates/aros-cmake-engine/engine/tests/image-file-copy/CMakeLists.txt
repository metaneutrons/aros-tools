cmake_minimum_required(VERSION 3.22)
project(ImageFileCopyFixture NONE)

if(NOT DEFINED AROS_ENGINE_DIR OR "${AROS_ENGINE_DIR}" STREQUAL "")
    message(FATAL_ERROR "image file copy fixture requires AROS_ENGINE_DIR")
endif()

set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source tree")
if(AROS_IMAGE_COPY_PROBE STREQUAL "tree-root-source" OR
   AROS_IMAGE_COPY_PROBE STREQUAL "source-inside-destination")
    # The build directory lives inside the checkout, as in a real build.
    set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}")
endif()
set(AROS_SYS_DIR "${CMAKE_BINARY_DIR}/SYS")
set(AROS_DEVELOPER_DIR "${AROS_SYS_DIR}/Developer")
set(AROS_DEVELOPER_FD_DIR "${AROS_DEVELOPER_DIR}/SDK/fd")
set(AROS_DEVELOPER_LIB_DIR "${AROS_DEVELOPER_DIR}/lib")
set(_source_root "${AROS_SOURCE_DIR}/db")
file(MAKE_DIRECTORY "${_source_root}" "${AROS_SYS_DIR}" "${AROS_DEVELOPER_FD_DIR}"
    "${AROS_DEVELOPER_LIB_DIR}")
file(WRITE "${_source_root}/general.config" "GUI PANEL=NONE;OPENGUI=NO\r\n")
file(WRITE "${_source_root}/netdb" "hosts;with;semicolons\r\n")

include("${AROS_ENGINE_DIR}/SdkFileCopies.cmake")

set(_destination "${AROS_SYS_DIR}/System/Network/AROSTCP/db")
set(_probe "${AROS_IMAGE_COPY_PROBE}")
if(_probe STREQUAL "developer-destination")
    aros_stage_sdk_files(NAME stage-db KIND image SOURCE "${_source_root}"
        DESTINATION "${AROS_DEVELOPER_DIR}/S" FILES netdb)
elseif(_probe STREQUAL "outside-image")
    aros_stage_sdk_files(NAME stage-db KIND image SOURCE "${_source_root}"
        DESTINATION "${CMAKE_BINARY_DIR}/elsewhere" FILES netdb)
elseif(_probe STREQUAL "image-root")
    aros_stage_sdk_files(NAME stage-db KIND image SOURCE "${_source_root}"
        DESTINATION "${AROS_SYS_DIR}" FILES netdb)
elseif(_probe STREQUAL "tree-root-source")
    file(WRITE "${AROS_SOURCE_DIR}/LICENSE" "licence text\r\n")
    aros_stage_sdk_files(NAME stage-db KIND image SOURCE "${AROS_SOURCE_DIR}"
        DESTINATION "${AROS_SYS_DIR}/Locale/Help/English" FILES LICENSE)
elseif(_probe STREQUAL "source-inside-destination")
    file(MAKE_DIRECTORY "${AROS_SYS_DIR}/System/inner")
    file(WRITE "${AROS_SYS_DIR}/System/inner/netdb" "staged output\r\n")
    aros_stage_sdk_files(NAME stage-db KIND image
        SOURCE "${AROS_SYS_DIR}/System/inner"
        DESTINATION "${AROS_SYS_DIR}/System" FILES netdb)
elseif(_probe STREQUAL "unknown-kind")
    aros_stage_sdk_files(NAME stage-db KIND other SOURCE "${_source_root}"
        DESTINATION "${_destination}" FILES netdb)
elseif(_probe STREQUAL "duplicate-kind")
    aros_stage_sdk_files(NAME stage-db KIND image KIND image SOURCE "${_source_root}"
        DESTINATION "${_destination}" FILES netdb)
elseif(_probe STREQUAL "traversal")
    aros_stage_sdk_files(NAME stage-db KIND image SOURCE "${_source_root}"
        DESTINATION "${_destination}" FILES ../netdb)
elseif(_probe STREQUAL "missing-source-file")
    aros_stage_sdk_files(NAME stage-db KIND image SOURCE "${_source_root}"
        DESTINATION "${_destination}" FILES absent)
elseif(_probe)
    message(FATAL_ERROR "unknown image file copy probe ${_probe}")
else()
    aros_stage_sdk_files(NAME stage-db KIND image SOURCE "${_source_root}"
        DESTINATION "${_destination}" FILES general.config netdb)
endif()
