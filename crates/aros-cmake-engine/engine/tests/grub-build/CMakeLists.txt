cmake_minimum_required(VERSION 3.22)
project(GrubBuildFixture NONE)

if(NOT AROS_REPO_ROOT)
    message(FATAL_ERROR "AROS_REPO_ROOT is required")
endif()
if(NOT CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin" OR
   NOT CMAKE_HOST_SYSTEM_PROCESSOR MATCHES "^(arm64|aarch64)$")
    message(FATAL_ERROR "grub-build fixture requires the audited arm64 Darwin host")
endif()

set(AROS_GRUB_SOURCE_ROOT "${AROS_REPO_ROOT}")
set(AROS_GRUB2_XZ_PREFIX "/opt/homebrew/opt/xz")
set(AROS_GRUB2_LLVM_PREFIX "/opt/homebrew/opt/llvm")

if(GRUB_BUILD_CASE STREQUAL "symlink-binary")
    set(_escaped "${CMAKE_BINARY_DIR}/../grub-build-escaped")
    file(MAKE_DIRECTORY "${_escaped}" "${CMAKE_BINARY_DIR}/gen")
    file(CREATE_LINK "${_escaped}" "${CMAKE_BINARY_DIR}/gen/configure" SYMBOLIC)
endif()

include("${AROS_REPO_ROOT}/cmake/GrubBuild.cmake")

set(_binary_root "${CMAKE_BINARY_DIR}/gen/configure/arch/all-pc/boot/grub2-host")
set(_install_root "${CMAKE_BINARY_DIR}/hosttools/grub2")
set(_pc_id grub2-host)
if(GRUB_BUILD_CASE STREQUAL "wrong-identity")
    set(_pc_id wrong-grub2-host)
endif()

aros_build_grub2(
    MMAKE_ID "${_pc_id}"
    MODE pc
    BINARY_DIR "${_binary_root}/pc"
    INSTALL_PREFIX "${_install_root}/pc")

if(NOT GRUB_BUILD_CASE STREQUAL "")
    # Negative cases above must fail in the first declaration.  If they did
    # not, do not hide that error behind three real builds.
    message(FATAL_ERROR "grub-build fixture case ${GRUB_BUILD_CASE} unexpectedly configured")
endif()

aros_build_grub2(
    MMAKE_ID grub2-efi-host
    MODE efi64
    BINARY_DIR "${_binary_root}/efi-x86_64"
    INSTALL_PREFIX "${_install_root}/efi-x86_64")
aros_build_grub2(
    MMAKE_ID grub2-efi32-host
    MODE efi32
    BINARY_DIR "${_binary_root}/efi-i386"
    INSTALL_PREFIX "${_install_root}/efi-i386")
