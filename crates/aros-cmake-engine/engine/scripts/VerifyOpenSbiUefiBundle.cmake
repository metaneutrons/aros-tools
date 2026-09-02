cmake_minimum_required(VERSION 3.22)

# Verify and manifest an AROS OpenSBI/UEFI removable-media payload.

if(NOT DEFINED BUNDLE_DIR OR BUNDLE_DIR STREQUAL "")
    message(FATAL_ERROR "VerifyOpenSbiUefiBundle.cmake requires -DBUNDLE_DIR=<directory>")
endif()
if(NOT MODEL STREQUAL "milk-v-titan")
    message(FATAL_ERROR "Unsupported OpenSBI/UEFI board model '${MODEL}'")
endif()

set(_files
    "uefi-loader|EFI/BOOT/BOOTRISCV64.EFI"
    "kernel-image|EFI/AROS/Image"
    "bsp-package|aros-bsp.pkg"
    "command-line|aros.cmd"
    "startup-script|startup.nsh")

foreach(_spec IN LISTS _files)
    string(REPLACE "|" ";" _parts "${_spec}")
    list(GET _parts 1 _name)
    set(_path "${BUNDLE_DIR}/${_name}")
    if(NOT EXISTS "${_path}" OR IS_DIRECTORY "${_path}")
        message(FATAL_ERROR "OpenSBI/UEFI bundle is missing ${_name}: ${_path}")
    endif()
endforeach()

foreach(_name IN ITEMS EFI/BOOT/BOOTRISCV64.EFI EFI/AROS/Image)
    file(SIZE "${BUNDLE_DIR}/${_name}" _size)
    if(_size EQUAL 0)
        message(FATAL_ERROR "OpenSBI/UEFI bundle contains an empty ${_name}")
    endif()
endforeach()
file(READ "${BUNDLE_DIR}/EFI/BOOT/BOOTRISCV64.EFI" _dos_magic
    OFFSET 0 LIMIT 2 HEX)
string(TOLOWER "${_dos_magic}" _dos_magic)
if(NOT _dos_magic STREQUAL "4d5a")
    message(FATAL_ERROR
        "BOOTRISCV64.EFI lacks the RISC-V Image/PE MZ header (got ${_dos_magic})")
endif()
file(READ "${BUNDLE_DIR}/EFI/BOOT/BOOTRISCV64.EFI" _pe_header
    OFFSET 64 LIMIT 6 HEX)
string(TOLOWER "${_pe_header}" _pe_header)
if(NOT _pe_header STREQUAL "504500006450")
    message(FATAL_ERROR
        "BOOTRISCV64.EFI lacks the PE signature and RISC-V 64 machine id at offset 64 (got ${_pe_header})")
endif()
file(SIZE "${BUNDLE_DIR}/aros-bsp.pkg" _bsp_size)
if(_bsp_size EQUAL 0)
    message(FATAL_ERROR "OpenSBI/UEFI bundle contains an empty aros-bsp.pkg")
endif()

file(READ "${BUNDLE_DIR}/startup.nsh" _startup)
if(NOT _startup STREQUAL "BOOTRISCV64.EFI\n")
    message(FATAL_ERROR "startup.nsh must contain exactly BOOTRISCV64.EFI")
endif()

file(SHA256 "${BUNDLE_DIR}/EFI/BOOT/BOOTRISCV64.EFI" _loader_sha)
file(SHA256 "${BUNDLE_DIR}/EFI/AROS/Image" _image_sha)
if(NOT "${_loader_sha}" STREQUAL "${_image_sha}")
    message(FATAL_ERROR "BOOTRISCV64.EFI and EFI/AROS/Image must be byte-identical")
endif()

set(_layout
    "aros-board-sd-partition-v1\nscheme=mbr\nfilesystem=fat32\nstart_lba=2048\nsize_bytes=67108864\nlabel=AROSBOOT\n")
string(SHA256 _layout_sha "${_layout}")
set(_manifest
    "format_version = 1\n\n[board]\nname = \"milk-v-titan\"\nmodel = \"milk-v-titan\"\ntransport = \"uefi-esp\"\n\n[partition]\nscheme = \"mbr\"\nfilesystem = \"fat32\"\nstart_lba = 2048\nsize_bytes = 67108864\nlabel = \"AROSBOOT\"\nlayout_sha256 = \"${_layout_sha}\"\n")

foreach(_spec IN LISTS _files)
    string(REPLACE "|" ";" _parts "${_spec}")
    list(GET _parts 0 _role)
    list(GET _parts 1 _name)
    file(SHA256 "${BUNDLE_DIR}/${_name}" _sha)
    string(APPEND _manifest
        "\n[[files]]\nrole = \"${_role}\"\nsource = \"${_name}\"\ndestination = \"${_name}\"\nsha256 = \"${_sha}\"\n")
endforeach()

set(_manifest_path "${BUNDLE_DIR}/boot-bundle.toml")
if(WRITE_MANIFEST)
    file(WRITE "${_manifest_path}" "${_manifest}")
elseif(NOT EXISTS "${_manifest_path}")
    message(FATAL_ERROR "OpenSBI/UEFI bundle is missing boot-bundle.toml")
else()
    file(READ "${_manifest_path}" _actual_manifest)
    if(NOT _actual_manifest STREQUAL _manifest)
        message(FATAL_ERROR "boot-bundle.toml does not match the staged payload")
    endif()
endif()

message(STATUS "OpenSBI/UEFI payload verified: ${BUNDLE_DIR}")
