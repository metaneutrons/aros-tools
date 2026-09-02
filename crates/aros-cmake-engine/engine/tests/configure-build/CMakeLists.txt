cmake_minimum_required(VERSION 3.22)
project(ConfigureBuildFixture C)

if(NOT AROS_REPO_ROOT)
    message(FATAL_ERROR "AROS_REPO_ROOT is required")
endif()
if(NOT CMAKE_HOST_UNIX)
    message(FATAL_ERROR "configure-build fixture requires a POSIX host")
endif()

set(_host_compiler "${CMAKE_C_COMPILER}")
set(_fixture_dir "${CMAKE_CURRENT_LIST_DIR}")
set(_fixture_tools "${CMAKE_BINARY_DIR}/fixture-tools")
file(MAKE_DIRECTORY "${_fixture_tools}")
file(COPY_FILE "${_fixture_dir}/mock-tool.sh"
    "${_fixture_tools}/mock-tool.sh")
file(COPY_FILE "${_fixture_dir}/mock-linker.sh"
    "${_fixture_tools}/mock-linker.sh")
file(CHMOD
    "${_fixture_tools}/mock-tool.sh"
    "${_fixture_tools}/mock-linker.sh"
    PERMISSIONS
        OWNER_READ OWNER_WRITE OWNER_EXECUTE
        GROUP_READ GROUP_EXECUTE
        WORLD_READ WORLD_EXECUTE)

# Host ADFlib remains a real compilation regression. Target ADFlib and
# WirelessManager exercise the same runner contracts with deterministic mock
# tools, so the fixture does not require an installed AROS cross toolchain.
set(AROS_HOST_CC "${_host_compiler}")
set(CMAKE_C_COMPILER "${_fixture_tools}/mock-tool.sh")
set(CMAKE_AR "${_fixture_tools}/mock-tool.sh")
set(CMAKE_RANLIB "${_fixture_tools}/mock-tool.sh")
set(AROS_LLD_BIN "${_fixture_tools}/mock-linker.sh")
set(AROS_SDK_INCLUDE_DIR "${CMAKE_BINARY_DIR}/fixture-sdk")
set(AROS_CONFIGURE_SOURCE_ROOT "${AROS_REPO_ROOT}")
include("${AROS_REPO_ROOT}/cmake/ConfigureBuild.cmake")

set(_source_root "${AROS_REPO_ROOT}")
if(CONFIGURE_BUILD_CASE STREQUAL "symlink-input" OR
   CONFIGURE_BUILD_CASE STREQUAL "bad-inventory")
    set(_source_root "${CMAKE_BINARY_DIR}/input-source")
    file(MAKE_DIRECTORY "${_source_root}/tools")
    file(COPY "${AROS_REPO_ROOT}/tools/ADFlib" DESTINATION "${_source_root}/tools")
    if(CONFIGURE_BUILD_CASE STREQUAL "symlink-input")
        file(REMOVE "${_source_root}/tools/ADFlib/src/adf_env.c")
        file(CREATE_LINK "${AROS_REPO_ROOT}/tools/ADFlib/src/adf_env.c"
            "${_source_root}/tools/ADFlib/src/adf_env.c" SYMBOLIC)
    else()
        file(APPEND "${_source_root}/tools/ADFlib/adflib-configure.inputs"
            "missing.c\n")
    endif()
endif()
set(AROS_CONFIGURE_SOURCE_ROOT "${_source_root}")

set(_headers
    adf_defs.h adf_blk.h adf_err.h adf_str.h adflib.h adf_bitm.h
    adf_cache.h adf_dir.h adf_disk.h adf_dump.h adf_env.h adf_file.h
    adf_hd.h adf_link.h adf_raw.h adf_salv.h adf_util.h defendian.h
    hd_blk.h prefix.h adf_nativ.h)

set(_adflib_source "${_source_root}/tools/ADFlib")
set(_adflib_manifest "${_adflib_source}/adflib-configure.inputs")
set(_host_binary "${CMAKE_BINARY_DIR}/gen/configure/tools/ADFlib/host")
set(_host_prefix "${CMAKE_BINARY_DIR}/hosttools")
set(_target_binary "${CMAKE_BINARY_DIR}/gen/configure/tools/ADFlib/target")
set(_target_prefix "${CMAKE_BINARY_DIR}/SYS/Developer")
set(_host_id host-adflib)
set(_target_id linklib-adflib)

if(CONFIGURE_BUILD_CASE STREQUAL "escape-binary")
    set(_host_binary "${CMAKE_BINARY_DIR}/escaped")
elseif(CONFIGURE_BUILD_CASE STREQUAL "wrong-identity")
    set(_target_id wrong-adflib-target)
elseif(CONFIGURE_BUILD_CASE STREQUAL "symlink-binary")
    set(_escaped_configure "${CMAKE_BINARY_DIR}/../configure-escaped")
    file(MAKE_DIRECTORY "${_escaped_configure}" "${CMAKE_BINARY_DIR}/gen")
    file(CREATE_LINK "${_escaped_configure}"
        "${CMAKE_BINARY_DIR}/gen/configure" SYMBOLIC)
endif()

set(_host_installed "${_host_prefix}/lib/libadf.a")
set(_target_installed "${_target_prefix}/lib/libadf.a")
foreach(_header IN LISTS _headers)
    list(APPEND _host_installed "${_host_prefix}/include/${_header}")
    list(APPEND _target_installed "${_target_prefix}/include/${_header}")
endforeach()
list(APPEND _host_installed "${_host_prefix}/lib/pkgconfig/adflib.pc")
list(APPEND _target_installed "${_target_prefix}/lib/pkgconfig/adflib.pc")

# aros_build_configure asks a link-library target where its archive is, so
# the fixture has to be a target rather than a bare path. An imported archive
# is the honest stand-in here: the fixture stages the file with a custom
# command and cannot compile a real one, because its C compiler is a mock.
set(_mui_archive "${CMAKE_BINARY_DIR}/liblinklibs-mui.a")
# Not an empty file: the runner rejects an empty dependency archive, and an
# `!<arch>` header is what a real one starts with.
set(_mui_stub "${CMAKE_BINARY_DIR}/fixture-mui-stub")
file(WRITE "${_mui_stub}" "!<arch>\n")
add_custom_command(
    OUTPUT "${_mui_archive}"
    COMMAND "${CMAKE_COMMAND}" -E copy "${_mui_stub}" "${_mui_archive}"
    VERBATIM)
add_custom_target(configure-fixture-mui DEPENDS "${_mui_archive}")
add_library(linklibs-mui STATIC IMPORTED)
set_target_properties(linklibs-mui PROPERTIES
    IMPORTED_LOCATION "${_mui_archive}")

aros_build_configure(
    MMAKE_ID "${_host_id}"
    MODE adflib-host
    SOURCE_DIR "${_adflib_source}"
    BINARY_DIR "${_host_binary}"
    INSTALL_PREFIX "${_host_prefix}"
    INPUT_MANIFEST "${_adflib_manifest}"
    PRIVATE_PRODUCTS "${_host_binary}/build/libadf.a"
    INSTALL_PRODUCTS ${_host_installed})

aros_build_configure(
    MMAKE_ID "${_target_id}"
    MODE adflib-target
    SOURCE_DIR "${_adflib_source}"
    BINARY_DIR "${_target_binary}"
    INSTALL_PREFIX "${_target_prefix}"
    INPUT_MANIFEST "${_adflib_manifest}"
    PRIVATE_PRODUCTS "${_target_binary}/build/libadf.a"
    INSTALL_PRODUCTS ${_target_installed}
    PROVIDED_LIBRARY adf)

set(_wireless_source "${_source_root}/workbench/network/WirelessManager")
set(_wireless_manifest
    "${_wireless_source}/wirelessmanager-configure.inputs")
set(_wireless_binary
    "${CMAKE_BINARY_DIR}/gen/configure/workbench/network/WirelessManager")
set(_wireless_prefix "${CMAKE_BINARY_DIR}/SYS")
aros_build_configure(
    MMAKE_ID workbench-network-wirelessmanager
    MODE wirelessmanager
    SOURCE_DIR "${_wireless_source}"
    BINARY_DIR "${_wireless_binary}"
    INSTALL_PREFIX "${_wireless_prefix}"
    INPUT_MANIFEST "${_wireless_manifest}"
    PRIVATE_PRODUCTS
        "${_wireless_binary}/source/wpa_supplicant/wpa_supplicant"
        "${_wireless_binary}/source/wpa_supplicant/wpa_passphrase"
        "${_wireless_binary}/source/wpa_supplicant/wpa_cli"
    INSTALL_PRODUCTS "${_wireless_prefix}/C/WirelessManager"
    DEPENDENCY_TARGETS linklibs-mui)
