cmake_minimum_required(VERSION 3.22)
project(AhiBuildFixture NONE)

if(NOT AROS_REPO_ROOT OR NOT HOST_PERL OR NOT AHI_FIXTURE_MODE)
    message(FATAL_ERROR "AROS_REPO_ROOT, HOST_PERL and AHI_FIXTURE_MODE are required")
endif()
if(NOT AHI_FIXTURE_MODE MATCHES "^(x86_64|arm|aarch64)$")
    message(FATAL_ERROR "unsupported fixture mode")
endif()
if(NOT DEFINED AHI_FIXTURE_CASE)
    set(AHI_FIXTURE_CASE "")
endif()
if(NOT AROS_AHI_RUNNER_BIN)
    message(FATAL_ERROR "AROS_AHI_RUNNER_BIN is required")
endif()

set(AROS_AHI_SOURCE_ROOT "${AROS_REPO_ROOT}")
set(AROS_SFDC_SOURCE_ROOT "${AROS_REPO_ROOT}")
set(_tools "${CMAKE_BINARY_DIR}/fixture-tools")
file(MAKE_DIRECTORY "${_tools}" "${CMAKE_BINARY_DIR}/hosttools"
    "${CMAKE_BINARY_DIR}/SDK/include/asm"
    "${CMAKE_BINARY_DIR}/SDK/include/aros/posixc"
    "${CMAKE_BINARY_DIR}/SDK/include/aros/stdc"
    "${CMAKE_BINARY_DIR}/GENINCDIR/libraries")
foreach(_name IN ITEMS mock-cc.sh mock-collect.sh mock-flexcat.sh mock-gmake.sh)
    file(COPY_FILE "${CMAKE_CURRENT_LIST_DIR}/${_name}" "${_tools}/${_name}"
        ONLY_IF_DIFFERENT)
    file(CHMOD "${_tools}/${_name}" PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
endforeach()
if(NOT EXISTS "${_tools}/ld.lld" AND NOT IS_SYMLINK "${_tools}/ld.lld")
    file(CREATE_LINK "${_tools}/mock-cc.sh" "${_tools}/ld.lld" SYMBOLIC)
endif()
file(CONFIGURE OUTPUT "${CMAKE_BINARY_DIR}/SDK/include/asm/io.h"
    CONTENT "/* fixture asm io */\n")
file(CONFIGURE OUTPUT "${CMAKE_BINARY_DIR}/GENINCDIR/libraries/mui.h"
    CONTENT "/* fixture mui */\n")
if(AHI_FIXTURE_MODE STREQUAL "arm" OR AHI_FIXTURE_MODE STREQUAL "aarch64")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/SDK/include/proto")
    file(CONFIGURE OUTPUT "${CMAKE_BINARY_DIR}/SDK/include/proto/dma.h"
        CONTENT "/* fixture dma */\n")
    file(CONFIGURE OUTPUT "${CMAKE_BINARY_DIR}/SDK/include/proto/mbox.h"
        CONTENT "/* fixture mbox */\n")
endif()
file(COPY_FILE "${CMAKE_CURRENT_LIST_DIR}/mock-flexcat.sh"
    "${CMAKE_BINARY_DIR}/hosttools/flexcat" ONLY_IF_DIFFERENT)
file(CHMOD "${CMAKE_BINARY_DIR}/hosttools/flexcat" PERMISSIONS
    OWNER_READ OWNER_WRITE OWNER_EXECUTE
    GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
# aros_build_ahi asks each link-library target where its archive is, so the
# fixture declares targets rather than only staging files. Imported archives
# are the honest stand-in: this fixture's C compiler is a mock, so it cannot
# build a real one.
foreach(_name IN ITEMS amiga libm mui)
    file(CONFIGURE OUTPUT "${CMAKE_BINARY_DIR}/liblinklibs-${_name}.a"
        CONTENT "fixture ${_name} archive\n")
    add_library("linklibs-${_name}" STATIC IMPORTED)
    set_target_properties("linklibs-${_name}" PROPERTIES
        IMPORTED_LOCATION "${CMAKE_BINARY_DIR}/liblinklibs-${_name}.a")
endforeach()

if(AHI_FIXTURE_CASE STREQUAL "symlink-binary")
    set(_escaped "${CMAKE_BINARY_DIR}/../ahi-fixture-escaped")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/gen/configure/workbench/devs" "${_escaped}")
    file(CREATE_LINK "${_escaped}"
        "${CMAKE_BINARY_DIR}/gen/configure/workbench/devs/AHI" SYMBOLIC)
elseif(AHI_FIXTURE_CASE STREQUAL "relative-perl")
    set(HOST_PERL perl)
endif()

set(CMAKE_C_COMPILER "${_tools}/mock-cc.sh")
set(CMAKE_AR "${_tools}/mock-cc.sh")
set(CMAKE_RANLIB "${_tools}/mock-cc.sh")
set(CMAKE_OBJCOPY "${_tools}/mock-cc.sh")
set(CMAKE_STRIP "${_tools}/mock-cc.sh")
if(AHI_FIXTURE_CASE STREQUAL "missing-collector")
    set(AROS_COLLECT_BIN "${_tools}/missing-aros-collect")
else()
    set(AROS_COLLECT_BIN "${_tools}/mock-collect.sh")
endif()
if(AHI_FIXTURE_CASE STREQUAL "missing-runner")
    set(AROS_AHI_RUNNER_BIN "${_tools}/missing-aros-ahi-runner")
endif()
set(AROS_LLD_BIN "${_tools}/ld.lld")
set(AROS_AHI_MAKE "${_tools}/mock-gmake.sh")
set(AROS_AHI_FLEXCAT "${CMAKE_BINARY_DIR}/hosttools/flexcat")
set(AROS_AHI_BUILD_TRIPLET "aarch64-apple-darwin")

include("${AROS_REPO_ROOT}/cmake/SfdcHostTool.cmake")
include("${AROS_REPO_ROOT}/cmake/AhiBuild.cmake")
aros_build_host_sfdc(PERL "${HOST_PERL}")
aros_build_ahi(
    MMAKE_ID workbench-devs-AHI-subsystem
    MODE "${AHI_FIXTURE_MODE}"
    BINARY_DIR "${CMAKE_BINARY_DIR}/gen/configure/workbench/devs/AHI/${AHI_FIXTURE_MODE}"
    INSTALL_PREFIX "${CMAKE_BINARY_DIR}/SYS"
    HOST_SFDC "${AROS_HOST_SFDC}"
    HOST_PERL "${HOST_PERL}")

if(NOT AHI_FIXTURE_CASE STREQUAL "")
    message(FATAL_ERROR "fixture case ${AHI_FIXTURE_CASE} unexpectedly configured")
endif()
