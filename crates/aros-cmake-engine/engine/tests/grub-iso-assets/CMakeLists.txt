cmake_minimum_required(VERSION 3.22)
project(GrubIsoAssetsFixture NONE)

if(NOT AROS_REPO_ROOT)
    message(FATAL_ERROR "AROS_REPO_ROOT is required")
endif()

set(AROS_GRUB_SOURCE_ROOT "${AROS_REPO_ROOT}")

function(_fixture_write_if_changed path contents)
    if(EXISTS "${path}")
        file(READ "${path}" _existing)
        if(_existing STREQUAL "${contents}")
            return()
        endif()
    endif()
    file(WRITE "${path}" "${contents}")
endfunction()

function(_fixture_host_prefix prefix manifest)
    file(STRINGS "${AROS_REPO_ROOT}/${manifest}" _entries)
    foreach(_relative IN LISTS _entries)
        if(_relative STREQUAL "grub-mkimage")
            continue()
        endif()
        get_filename_component(_parent "${prefix}/${_relative}" DIRECTORY)
        file(MAKE_DIRECTORY "${_parent}")
        _fixture_write_if_changed("${prefix}/${_relative}"
            "fixture ${manifest}: ${_relative}\n")
    endforeach()
    file(MAKE_DIRECTORY "${prefix}")
    _fixture_write_if_changed("${prefix}/grub-mkimage" [=[#!/bin/sh
out=
while [ "$#" -gt 0 ]; do
    if [ "$1" = "-o" ]; then
        out="$2"
        shift 2
    else
        shift
    fi
done
[ -n "$out" ] || exit 2
printf 'fixture grub image\n' > "$out"
]=])
    file(CHMOD "${prefix}/grub-mkimage"
        PERMISSIONS OWNER_READ OWNER_WRITE OWNER_EXECUTE
                    GROUP_READ GROUP_EXECUTE WORLD_READ WORLD_EXECUTE)
endfunction()

set(_host_root "${CMAKE_BINARY_DIR}/hosttools/grub2")
_fixture_host_prefix("${_host_root}/pc" "cmake/manifests/grub-2.12-pc.install")
_fixture_host_prefix("${_host_root}/efi-x86_64" "cmake/manifests/grub-2.12-efi64.install")
_fixture_host_prefix("${_host_root}/efi-i386" "cmake/manifests/grub-2.12-efi32.install")

add_custom_target(grub2-host)
add_custom_target(grub2-efi-host)
add_custom_target(grub2-efi32-host)

if(GRUB_ISO_ASSETS_CASE STREQUAL "symlink-binary")
    set(_escaped "${CMAKE_BINARY_DIR}/../grub-iso-assets-escaped")
    file(MAKE_DIRECTORY "${_escaped}" "${CMAKE_BINARY_DIR}/gen")
    file(CREATE_LINK "${_escaped}" "${CMAKE_BINARY_DIR}/gen/grub2-iso-assets" SYMBOLIC)
elseif(GRUB_ISO_ASSETS_CASE STREQUAL "symlink-contract")
    file(CREATE_LINK
        "${AROS_REPO_ROOT}/arch/all-pc/boot/grub2-host/mmakefile.src"
        "${CMAKE_BINARY_DIR}/.aros-grub2-iso-assets-contract.cmake" SYMBOLIC)
endif()

include("${AROS_REPO_ROOT}/cmake/GrubIsoAssets.cmake")
aros_stage_grub2_iso_assets(
    MODE x86_64
    BINARY_DIR "${CMAKE_BINARY_DIR}/gen/grub2-iso-assets/x86_64"
    SYS_DIR "${CMAKE_BINARY_DIR}/SYS"
    HOST_PC "${_host_root}/pc"
    HOST_EFI64 "${_host_root}/efi-x86_64"
    HOST_EFI32 "${_host_root}/efi-i386")

if(NOT GRUB_ISO_ASSETS_CASE STREQUAL "")
    message(FATAL_ERROR "grub-iso-assets fixture case ${GRUB_ISO_ASSETS_CASE} unexpectedly configured")
endif()

get_property(_direct_dependencies TARGET aros-grub2-iso-assets
    PROPERTY MANUALLY_ADDED_DEPENDENCIES)
foreach(_expected IN ITEMS grub2-host grub2-efi-host grub2-efi32-host)
    if(NOT _expected IN_LIST _direct_dependencies)
        message(FATAL_ERROR "asset staging is missing direct dependency ${_expected}")
    endif()
endforeach()
get_property(_products GLOBAL PROPERTY AROS_GRUB2_ISO_ASSET_PRODUCTS)
list(LENGTH _products _product_count)
if(NOT _product_count EQUAL 832)
    message(FATAL_ERROR "asset staging declared ${_product_count} products, expected 832")
endif()
