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
file(READ "${AROS_REPO_ROOT}/arch/all-pc/boot/grub2_def" _grub_version)
string(STRIP "${_grub_version}" _grub_version)
set(AROS_GRUB2_XZ_PREFIX "/opt/homebrew/opt/xz")
set(AROS_GRUB2_LLVM_PREFIX "/opt/homebrew/opt/llvm")

if(GRUB_BUILD_CASE STREQUAL "symlink-binary")
    set(_escaped "${CMAKE_BINARY_DIR}/../grub-build-escaped")
    file(MAKE_DIRECTORY "${_escaped}" "${CMAKE_BINARY_DIR}/gen")
    file(CREATE_LINK "${_escaped}" "${CMAKE_BINARY_DIR}/gen/configure" SYMBOLIC)
endif()

include("${AROS_TEST_ENGINE_DIR}/GrubBuild.cmake")

set(_binary_root "${CMAKE_BINARY_DIR}/gen/configure/arch/all-pc/boot/grub2-host")
set(_install_root "${CMAKE_BINARY_DIR}/hosttools/grub2")
set(_pc_id grub2-host)
if(GRUB_BUILD_CASE STREQUAL "wrong-identity")
    set(_pc_id wrong-grub2-host)
endif()

function(_grub_assert_python_contract id)
    _aros_grub_python_select(_actual_python _actual_selection)
    _aros_grub_python_probe("${_actual_python}" _actual_version
        _actual_realpath _actual_sha256)
    _aros_grub_python_check_selection_version(
        "${_actual_selection}" "${_actual_version}")
    if(GRUB_BUILD_CASE STREQUAL "python-fallback" AND
       NOT _actual_selection STREQUAL "fallback")
        message(FATAL_ERROR "python-fallback case did not select the fallback interpreter")
    endif()
    set(_contract "${CMAKE_BINARY_DIR}/.aros-${id}-grub2-contract.cmake")
    if(NOT EXISTS "${_contract}")
        message(FATAL_ERROR "${id} did not generate its GRUB2 build contract")
    endif()
    file(READ "${_contract}" _contract_content)
    foreach(_entry IN ITEMS
            "GB_PYTHON|${_actual_python}"
            "GB_PYTHON_SELECTION|${_actual_selection}"
            "GB_PYTHON_VERSION|${_actual_version}"
            "GB_PYTHON_REALPATH|${_actual_realpath}"
            "GB_PYTHON_SHA256|${_actual_sha256}")
        string(REPLACE "|" ";" _parts "${_entry}")
        list(GET _parts 0 _name)
        list(GET _parts 1 _expected)
        string(FIND "${_contract_content}"
            "set(${_name} [==[${_expected}]==])" _position)
        if(_position LESS 0)
            message(FATAL_ERROR
                "${id} contract ${_name} differs from the shared Python selection and probe")
        endif()
    endforeach()
endfunction()

aros_build_grub2(
    MMAKE_ID "${_pc_id}"
    MODE pc
    VERSION "${_grub_version}"
    BINARY_DIR "${_binary_root}/pc"
    INSTALL_PREFIX "${_install_root}/pc")
_grub_assert_python_contract("${_pc_id}")

if(NOT GRUB_BUILD_CASE STREQUAL "" AND
   NOT GRUB_BUILD_CASE STREQUAL "python-fallback")
    # Negative cases above must fail in the first declaration.  If they did
    # not, do not hide that error behind three real builds.
    message(FATAL_ERROR "grub-build fixture case ${GRUB_BUILD_CASE} unexpectedly configured")
endif()

aros_build_grub2(
    MMAKE_ID grub2-efi-host
    MODE efi64
    VERSION "${_grub_version}"
    BINARY_DIR "${_binary_root}/efi-x86_64"
    INSTALL_PREFIX "${_install_root}/efi-x86_64")
_grub_assert_python_contract("grub2-efi-host")
aros_build_grub2(
    MMAKE_ID grub2-efi32-host
    MODE efi32
    VERSION "${_grub_version}"
    BINARY_DIR "${_binary_root}/efi-i386"
    INSTALL_PREFIX "${_install_root}/efi-i386")
_grub_assert_python_contract("grub2-efi32-host")
