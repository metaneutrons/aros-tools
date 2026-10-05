cmake_minimum_required(VERSION 3.22)
project(NativePackagePublicationFixture LANGUAGES C)

foreach(_required IN ITEMS AROS_ENGINE_DIR NATIVE_PACKAGE_CASE)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "native package fixture requires ${_required}")
    endif()
endforeach()

set(AROS_SOURCE_DIR "${CMAKE_CURRENT_BINARY_DIR}/aros-source")
file(MAKE_DIRECTORY
    "${AROS_SOURCE_DIR}/compiler/include/asm"
    "${AROS_SOURCE_DIR}/compiler/arossupport/include"
    "${AROS_SOURCE_DIR}/compiler/startup")
file(WRITE "${AROS_SOURCE_DIR}/compiler/include/asm/cpu.h" "/* fixture */\n")
file(WRITE "${AROS_SOURCE_DIR}/compiler/include/asm.c"
    [=[void asm_fixture(void) {
    __asm__ __volatile__(".ascii \"#define AROS_FIXTURE 1\\n\"");
}
]=])
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/startup.c" "void _start(void) {}\n")
file(WRITE "${AROS_SOURCE_DIR}/compiler/startup/detach.c" "void _detach(void) {}\n")
set(AROS_TARGET_CPU x86_64)
set(AROS_TARGET_PLATFORM pc)
set(AROS_CMAKE_ENGINE_DIR "${AROS_ENGINE_DIR}")
configure_file("${CMAKE_CURRENT_LIST_DIR}/mock-romtool.sh"
    "${CMAKE_CURRENT_BINARY_DIR}/mock-romtool.sh" COPYONLY)
file(CHMOD "${CMAKE_CURRENT_BINARY_DIR}/mock-romtool.sh"
    PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE
                WORLD_READ WORLD_EXECUTE)
set(AROS_ROMTOOL_BIN "${CMAKE_CURRENT_BINARY_DIR}/mock-romtool.sh")
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/mock-tool.sh" "#!/bin/sh\nexit 0\n")
file(CHMOD "${CMAKE_CURRENT_BINARY_DIR}/mock-tool.sh"
    PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE GROUP_READ GROUP_EXECUTE
                WORLD_READ WORLD_EXECUTE)
set(AROS_GENMODULE_BIN "${CMAKE_CURRENT_BINARY_DIR}/mock-tool.sh")
set(AROS_COLLECT_BIN "${CMAKE_CURRENT_BINARY_DIR}/mock-tool.sh")
include("${AROS_ENGINE_DIR}/AROS.cmake")

file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/member.bin" "fixture member\n")
add_executable(package-member IMPORTED GLOBAL)
set_property(TARGET package-member PROPERTY IMPORTED_LOCATION
    "${CMAKE_CURRENT_BINARY_DIR}/member.bin")

set(_native_package FALSE)
set(_package_name native-package)
set(_modules package-member)
set(_member_names fixture.bin)
if(NATIVE_PACKAGE_CASE MATCHES "^(missing-member|no-present|absent-limit|invalid-limit|exact|overflow)$")
    set(_native_package TRUE)
    set(AROS_NATIVE_BUILD_CONTRACT_VALIDATED TRUE)
    set(AROS_NATIVE_BUILD_PACKAGE_TARGET native-package)
    if(DEFINED NATIVE_PACKAGE_LIMIT AND NOT NATIVE_PACKAGE_LIMIT STREQUAL "__ABSENT__")
        set(AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES "${NATIVE_PACKAGE_LIMIT}")
    endif()
endif()

if(NATIVE_PACKAGE_CASE STREQUAL "missing-member")
    list(APPEND _modules required-missing-module)
    list(APPEND _member_names required.bin)
elseif(NATIVE_PACKAGE_CASE STREQUAL "no-present")
    set(_modules "")
    set(_member_names "")
elseif(NATIVE_PACKAGE_CASE STREQUAL "exact")
    set(AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES 5)
elseif(NATIVE_PACKAGE_CASE STREQUAL "overflow")
    set(AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES 4)
elseif(NATIVE_PACKAGE_CASE STREQUAL "invalid-limit")
    set(AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES 0)
elseif(NATIVE_PACKAGE_CASE STREQUAL "absent-limit")
    unset(AROS_NATIVE_BUILD_PACKAGE_LIMIT_BYTES)
elseif(NATIVE_PACKAGE_CASE STREQUAL "partial")
    list(APPEND _modules not-configured-module)
    list(APPEND _member_names optional.bin)
    set(_package_name nonnative-partial-package)
else()
    message(FATAL_ERROR "unknown native package fixture case: ${NATIVE_PACKAGE_CASE}")
endif()

aros_make_package(
    NAME "${_package_name}"
    OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/published.pkg"
    MODULES ${_modules}
    MEMBER_NAMES ${_member_names})
