cmake_minimum_required(VERSION 3.22)
project(DeveloperLibCopyFixture NONE)

if(NOT DEFINED AROS_ENGINE_DIR OR "${AROS_ENGINE_DIR}" STREQUAL "")
    message(FATAL_ERROR "Developer library copy fixture requires AROS_ENGINE_DIR")
endif()
message(STATUS "Developer library fixture configure marker")

set(AROS_SOURCE_DIR "${CMAKE_BINARY_DIR}/source tree")
set(_source_root "${AROS_SOURCE_DIR}/lib")
set(AROS_DEVELOPER_FD_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/SDK/fd")
set(AROS_DEVELOPER_LIB_DIR "${CMAKE_BINARY_DIR}/SYS/Developer/lib")
file(MAKE_DIRECTORY "${_source_root}" "${AROS_DEVELOPER_FD_DIR}" "${AROS_DEVELOPER_LIB_DIR}")
if(NOT EXISTS "${_source_root}/lib-a.bin")
    file(WRITE "${_source_root}/lib-a.bin" "alpha;beta\r\nomega\r\n")
endif()
if(NOT EXISTS "${_source_root}/lib-b.txt")
    file(WRITE "${_source_root}/lib-b.txt" "semicolon;payload;with;crlf\r\nsecond line\r\n")
endif()

include("${AROS_ENGINE_DIR}/SdkFileCopies.cmake")

if(AROS_DEVELOPER_LIB_COPY_PROBE STREQUAL "lib-subdir")
    aros_stage_sdk_files(
        NAME stage-developer-libraries
        SOURCE "${_source_root}"
        DESTINATION "${AROS_DEVELOPER_LIB_DIR}/unsafe"
        FILES lib-a.bin)
elseif(AROS_DEVELOPER_LIB_COPY_PROBE STREQUAL "lib-fetch")
    aros_stage_sdk_files(
        NAME stage-developer-libraries
        SOURCE "${_source_root}"
        DESTINATION "${AROS_DEVELOPER_LIB_DIR}"
        FETCH fetch-developer-libraries
        FILES lib-a.bin)
elseif(AROS_DEVELOPER_LIB_COPY_PROBE STREQUAL "duplicate-output")
    aros_stage_sdk_files(
        NAME stage-developer-libraries
        SOURCE "${_source_root}"
        DESTINATION "${AROS_DEVELOPER_LIB_DIR}"
        FILES lib-a.bin)
    aros_stage_sdk_files(
        NAME another-developer-library-stage
        SOURCE "${_source_root}"
        DESTINATION "${AROS_DEVELOPER_LIB_DIR}"
        FILES lib-a.bin)
elseif(AROS_DEVELOPER_LIB_COPY_PROBE)
    message(FATAL_ERROR "unknown Developer library copy probe ${AROS_DEVELOPER_LIB_COPY_PROBE}")
else()
    aros_stage_sdk_files(
        NAME stage-developer-libraries
        SOURCE "${_source_root}"
        DESTINATION "${AROS_DEVELOPER_LIB_DIR}"
        FILES lib-a.bin lib-b.txt)
endif()
