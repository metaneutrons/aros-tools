cmake_minimum_required(VERSION 3.22)
project(SfdcHostToolFixture NONE)

if(NOT AROS_REPO_ROOT)
    message(FATAL_ERROR "AROS_REPO_ROOT is required")
endif()
if(NOT HOST_PERL)
    message(FATAL_ERROR "HOST_PERL must be injected explicitly")
endif()
if(NOT DEFINED SFDC_HOST_TOOL_CASE)
    set(SFDC_HOST_TOOL_CASE "")
endif()

set(AROS_SFDC_SOURCE_ROOT "${AROS_REPO_ROOT}")
if(SFDC_HOST_TOOL_CASE STREQUAL "mutable-source" OR
   SFDC_HOST_TOOL_CASE STREQUAL "bad-manifest" OR
   SFDC_HOST_TOOL_CASE STREQUAL "symlink-input")
    set(AROS_SFDC_SOURCE_ROOT "${CMAKE_BINARY_DIR}/source-root")
    if(NOT EXISTS "${AROS_SFDC_SOURCE_ROOT}/tools/sfdc")
        file(MAKE_DIRECTORY "${AROS_SFDC_SOURCE_ROOT}/tools")
        file(COPY "${AROS_REPO_ROOT}/tools/sfdc"
            DESTINATION "${AROS_SFDC_SOURCE_ROOT}/tools")
    endif()
    if(SFDC_HOST_TOOL_CASE STREQUAL "bad-manifest")
        file(APPEND
            "${AROS_SFDC_SOURCE_ROOT}/tools/sfdc/sfdc-host.inputs"
            "extra.pl\n")
    elseif(SFDC_HOST_TOOL_CASE STREQUAL "symlink-input")
        set(_copied "${AROS_SFDC_SOURCE_ROOT}/tools/sfdc/CLib.pl")
        file(REMOVE "${_copied}")
        file(CREATE_LINK "${AROS_REPO_ROOT}/tools/sfdc/CLib.pl" "${_copied}" SYMBOLIC)
    endif()
elseif(SFDC_HOST_TOOL_CASE STREQUAL "symlink-binary")
    set(_escaped "${CMAKE_BINARY_DIR}/../sfdc-hosttool-escaped")
    file(MAKE_DIRECTORY "${_escaped}" "${CMAKE_BINARY_DIR}/gen")
    file(CREATE_LINK "${_escaped}" "${CMAKE_BINARY_DIR}/gen/hosttools" SYMBOLIC)
elseif(SFDC_HOST_TOOL_CASE STREQUAL "relative-perl")
    set(HOST_PERL perl)
endif()

include("${AROS_REPO_ROOT}/cmake/SfdcHostTool.cmake")
aros_build_host_sfdc(PERL "${HOST_PERL}")
if(NOT AROS_HOST_SFDC STREQUAL "${CMAKE_BINARY_DIR}/hosttools/sfdc")
    message(FATAL_ERROR "host-sfdc did not export its private output path")
endif()

if(NOT SFDC_HOST_TOOL_CASE STREQUAL "" AND
   NOT SFDC_HOST_TOOL_CASE STREQUAL "mutable-source")
    message(FATAL_ERROR
        "sfdc-host-tool fixture case ${SFDC_HOST_TOOL_CASE} unexpectedly configured")
endif()
