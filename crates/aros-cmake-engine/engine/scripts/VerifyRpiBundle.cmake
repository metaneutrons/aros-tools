cmake_minimum_required(VERSION 3.22)

# Verify (and optionally manifest) an AROS Raspberry Pi debug payload.
#
# Called from RaspberryPi.cmake after staging, so it deliberately performs no
# firmware discovery and makes no network request.

if(NOT DEFINED BUNDLE_DIR OR BUNDLE_DIR STREQUAL "")
    message(FATAL_ERROR "VerifyRpiBundle.cmake requires -DBUNDLE_DIR=<directory>")
endif()
foreach(_argument IN ITEMS MODEL CPU DTB_NAME BOOT_IMAGE_NAME BOOT_ELF_NAME
        BOOT_MAP_NAME BSP_NAME KERNEL_ADDRESS ARM_64BIT)
    if(NOT DEFINED ${_argument} OR "${${_argument}}" STREQUAL "")
        message(FATAL_ERROR "VerifyRpiBundle.cmake requires -D${_argument}=<value>")
    endif()
endforeach()

if(MODEL STREQUAL "rpi3")
    if(NOT CPU STREQUAL "arm" OR NOT DTB_NAME STREQUAL "bcm2710-rpi-3-b-plus.dtb")
        message(FATAL_ERROR "rpi3 requires the ARM32 and bcm2710-rpi-3-b-plus.dtb contract")
    endif()
elseif(MODEL STREQUAL "rpi4")
    if(NOT CPU STREQUAL "aarch64" OR NOT DTB_NAME STREQUAL "bcm2711-rpi-4-b.dtb")
        message(FATAL_ERROR "rpi4 requires the AArch64 and bcm2711-rpi-4-b.dtb contract")
    endif()
elseif(MODEL STREQUAL "rpi5")
    if(NOT CPU STREQUAL "aarch64" OR NOT DTB_NAME STREQUAL "bcm2712-rpi-5-b.dtb")
        message(FATAL_ERROR "rpi5 requires the AArch64 and bcm2712-rpi-5-b.dtb contract")
    endif()
else()
    message(FATAL_ERROR "Unsupported Raspberry Pi model '${MODEL}'")
endif()

set(_required_files
    "${BOOT_IMAGE_NAME}"
    "${BOOT_ELF_NAME}"
    "${BOOT_MAP_NAME}"
    core.debug.elf
    core.map
    "${BSP_NAME}"
    "${DTB_NAME}"
    config.txt)

foreach(_name IN LISTS _required_files)
    set(_path "${BUNDLE_DIR}/${_name}")
    if(NOT EXISTS "${_path}")
        message(FATAL_ERROR "Raspberry Pi bundle is missing ${_name}: ${_path}")
    endif()
    file(SIZE "${_path}" _size)
    if(_size EQUAL 0)
        message(FATAL_ERROR "Raspberry Pi bundle contains an empty ${_name}: ${_path}")
    endif()
endforeach()

function(_rpi_require_elf name)
    file(READ "${BUNDLE_DIR}/${name}" _magic OFFSET 0 LIMIT 4 HEX)
    string(TOLOWER "${_magic}" _magic)
    if(NOT _magic STREQUAL "7f454c46")
        message(FATAL_ERROR "${name} is not an ELF file (expected 7f454c46, got ${_magic})")
    endif()
endfunction()

_rpi_require_elf("${BOOT_ELF_NAME}")
_rpi_require_elf("core.debug.elf")

file(READ "${BUNDLE_DIR}/${DTB_NAME}" _dtb_magic OFFSET 0 LIMIT 4 HEX)
string(TOLOWER "${_dtb_magic}" _dtb_magic)
if(NOT _dtb_magic STREQUAL "d00dfeed")
    message(FATAL_ERROR
        "${DTB_NAME} does not have the flattened-device-tree magic "
        "d00dfeed (got ${_dtb_magic})")
endif()

file(READ "${BUNDLE_DIR}/config.txt" _config)
set(_config_lines
    "kernel=${BOOT_IMAGE_NAME}"
    "kernel_address=${KERNEL_ADDRESS}"
    "initramfs ${BSP_NAME} 0x00800000"
    "arm_64bit=${ARM_64BIT}"
    "gpu_mem=128")
if(MODEL STREQUAL "rpi3")
    list(APPEND _config_lines "hdmi_drive=2")
else()
    list(APPEND _config_lines "enable_uart=1")
endif()
foreach(_line IN LISTS _config_lines)
    string(FIND "${_config}" "${_line}" _found)
    if(_found LESS 0)
        message(FATAL_ERROR "config.txt is missing the required line: ${_line}")
    endif()
endforeach()

set(_manifest "# AROS-NX Raspberry Pi ${MODEL} debug payload SHA-256\n")
foreach(_name IN LISTS _required_files)
    file(SHA256 "${BUNDLE_DIR}/${_name}" _sha256)
    string(APPEND _manifest "${_sha256}  ${_name}\n")
endforeach()

set(_manifest_path "${BUNDLE_DIR}/manifest.sha256")
if(WRITE_MANIFEST)
    file(WRITE "${_manifest_path}" "${_manifest}")
elseif(NOT EXISTS "${_manifest_path}")
    message(FATAL_ERROR "Raspberry Pi bundle is missing manifest.sha256")
else()
    file(READ "${_manifest_path}" _actual_manifest)
    if(NOT "${_actual_manifest}" STREQUAL "${_manifest}")
        message(FATAL_ERROR
            "manifest.sha256 does not match the staged Raspberry Pi payload; "
            "run the rpi-artifacts target again")
    endif()
endif()

message(STATUS "Raspberry Pi debug payload verified: ${BUNDLE_DIR}")
