cmake_minimum_required(VERSION 3.22)

set(_GIA_MODULE_DIR "${CMAKE_CURRENT_LIST_DIR}")
include("${_GIA_MODULE_DIR}/GrubIsoAssets.cmake")

if(NOT DEFINED GIA_ACTION OR NOT GIA_ACTION STREQUAL "stage")
    message(FATAL_ERROR "RunGrubIsoAssets requires GIA_ACTION=stage")
endif()
if(NOT DEFINED CONTRACT OR NOT EXISTS "${CONTRACT}" OR IS_DIRECTORY "${CONTRACT}" OR
   IS_SYMLINK "${CONTRACT}")
    message(FATAL_ERROR "RunGrubIsoAssets requires a regular existing CONTRACT")
endif()
_aros_grub_iso_assets_real_path("${CONTRACT}" _contract)
include("${_contract}")

foreach(_required IN ITEMS
        GIA_MODE GIA_SOURCE_ROOT GIA_BUILD_ROOT GIA_BINARY_DIR GIA_SYS_DIR
        GIA_HOST_PC GIA_HOST_EFI64 GIA_HOST_EFI32 GIA_STAMP)
    if(NOT DEFINED ${_required} OR "${${_required}}" STREQUAL "")
        message(FATAL_ERROR "GRUB2 ISO asset contract omits ${_required}")
    endif()
endforeach()
if(NOT DEFINED GIA_PRODUCTS OR "${GIA_PRODUCTS}" STREQUAL "")
    message(FATAL_ERROR "GRUB2 ISO asset contract omits its products")
endif()
foreach(_value IN ITEMS
        "mode|${GIA_MODE}" "source root|${GIA_SOURCE_ROOT}" "build root|${GIA_BUILD_ROOT}"
        "binary dir|${GIA_BINARY_DIR}" "SYS dir|${GIA_SYS_DIR}"
        "host PC|${GIA_HOST_PC}" "host EFI64|${GIA_HOST_EFI64}"
        "host EFI32|${GIA_HOST_EFI32}" "stamp|${GIA_STAMP}")
    string(REPLACE "|" ";" _parts "${_value}")
    list(GET _parts 0 _label)
    list(GET _parts 1 _path)
    _aros_grub_iso_assets_safe_value("GRUB2 ISO runner ${_label}" "${_path}")
endforeach()
if(NOT GIA_MODE STREQUAL "x86_64")
    message(FATAL_ERROR "GRUB2 ISO asset contract differs from the audited identity")
endif()

foreach(_pair IN ITEMS
        "source|${GIA_SOURCE_ROOT}" "build|${GIA_BUILD_ROOT}" "binary|${GIA_BINARY_DIR}"
        "sys|${GIA_SYS_DIR}" "pc|${GIA_HOST_PC}" "efi64|${GIA_HOST_EFI64}"
        "efi32|${GIA_HOST_EFI32}" "stamp|${GIA_STAMP}")
    string(REPLACE "|" ";" _parts "${_pair}")
    list(GET _parts 0 _label)
    list(GET _parts 1 _path)
    _aros_grub_iso_assets_real_path("${_path}" _resolved_${_label})
endforeach()
if(NOT _resolved_source STREQUAL GIA_SOURCE_ROOT OR
   NOT _resolved_build STREQUAL GIA_BUILD_ROOT OR
   NOT _resolved_binary STREQUAL GIA_BINARY_DIR OR
   NOT _resolved_sys STREQUAL GIA_SYS_DIR OR
   NOT _resolved_pc STREQUAL GIA_HOST_PC OR
   NOT _resolved_efi64 STREQUAL GIA_HOST_EFI64 OR
   NOT _resolved_efi32 STREQUAL GIA_HOST_EFI32 OR
   NOT _resolved_stamp STREQUAL GIA_STAMP)
    message(FATAL_ERROR "GRUB2 ISO asset contract has escaped or substituted paths")
endif()

set(_expected_binary "${GIA_BUILD_ROOT}/gen/grub2-iso-assets/x86_64")
set(_expected_sys "${GIA_BUILD_ROOT}/SYS")
set(_expected_pc "${GIA_BUILD_ROOT}/hosttools/grub2/pc")
set(_expected_efi64 "${GIA_BUILD_ROOT}/hosttools/grub2/efi-x86_64")
set(_expected_efi32 "${GIA_BUILD_ROOT}/hosttools/grub2/efi-i386")
set(_expected_stamp "${_expected_binary}/.grub2-iso-assets.stamp")
set(_expected_contract "${GIA_BUILD_ROOT}/.aros-grub2-iso-assets-contract.cmake")
foreach(_pair IN ITEMS
        "binary|${_expected_binary}|${GIA_BINARY_DIR}"
        "sys|${_expected_sys}|${GIA_SYS_DIR}"
        "pc|${_expected_pc}|${GIA_HOST_PC}"
        "efi64|${_expected_efi64}|${GIA_HOST_EFI64}"
        "efi32|${_expected_efi32}|${GIA_HOST_EFI32}"
        "stamp|${_expected_stamp}|${GIA_STAMP}"
        "contract|${_expected_contract}|${_contract}")
    string(REPLACE "|" ";" _parts "${_pair}")
    list(GET _parts 0 _label)
    list(GET _parts 1 _expected)
    list(GET _parts 2 _actual)
    _aros_grub_iso_assets_real_path("${_expected}" _expected_real)
    if(NOT _actual STREQUAL _expected_real)
        message(FATAL_ERROR "GRUB2 ISO ${_label} path differs from the audited layout")
    endif()
endforeach()

if(NOT EXISTS "${GIA_SOURCE_ROOT}" OR NOT IS_DIRECTORY "${GIA_SOURCE_ROOT}" OR
   IS_SYMLINK "${GIA_SOURCE_ROOT}" OR NOT EXISTS "${GIA_BUILD_ROOT}" OR
   NOT IS_DIRECTORY "${GIA_BUILD_ROOT}" OR IS_SYMLINK "${GIA_BUILD_ROOT}")
    message(FATAL_ERROR "GRUB2 ISO source or build root is unavailable")
endif()
foreach(_pair IN ITEMS
        "binary|${GIA_BINARY_DIR}" "sys|${GIA_SYS_DIR}" "pc|${GIA_HOST_PC}"
        "efi64|${GIA_HOST_EFI64}" "efi32|${GIA_HOST_EFI32}" "stamp|${GIA_STAMP}")
    string(REPLACE "|" ";" _parts "${_pair}")
    list(GET _parts 0 _label)
    list(GET _parts 1 _path)
    cmake_path(IS_PREFIX GIA_BUILD_ROOT "${_path}" NORMALIZE _owned)
    if(NOT _owned OR _path STREQUAL GIA_BUILD_ROOT)
        message(FATAL_ERROR "GRUB2 ISO ${_label} path escapes the build tree")
    endif()
    _aros_grub_iso_assets_reject_symlink_components(
        "${GIA_BUILD_ROOT}" "${_path}" "GRUB2 ISO ${_label} path")
endforeach()
cmake_path(IS_PREFIX GIA_BINARY_DIR "${GIA_SYS_DIR}" NORMALIZE _binary_contains_sys)
cmake_path(IS_PREFIX GIA_SYS_DIR "${GIA_BINARY_DIR}" NORMALIZE _sys_contains_binary)
if(_binary_contains_sys OR _sys_contains_binary)
    message(FATAL_ERROR "GRUB2 ISO asset private and SYS roots overlap")
endif()

set(_host_mmake "${GIA_SOURCE_ROOT}/${_AROS_GRUB_ISO_ASSETS_HOST_MMAKE_RELATIVE}")
set(_pc_manifest "${GIA_SOURCE_ROOT}/${_AROS_GRUB_ISO_ASSETS_PC_MANIFEST}")
set(_efi64_manifest "${GIA_SOURCE_ROOT}/${_AROS_GRUB_ISO_ASSETS_EFI64_MANIFEST}")
set(_efi32_manifest "${GIA_SOURCE_ROOT}/${_AROS_GRUB_ISO_ASSETS_EFI32_MANIFEST}")
foreach(_source_file IN ITEMS "${_host_mmake}" "${_pc_manifest}" "${_efi64_manifest}" "${_efi32_manifest}")
    _aros_grub_iso_assets_require_regular("${_source_file}" "GRUB2 ISO source input")
    _aros_grub_iso_assets_reject_symlink_components(
        "${GIA_SOURCE_ROOT}" "${_source_file}" "GRUB2 ISO source input")
endforeach()
file(SHA256 "${_host_mmake}" _host_mmake_before)
file(SHA256 "${_pc_manifest}" _pc_manifest_before)
file(SHA256 "${_efi64_manifest}" _efi64_manifest_before)
file(SHA256 "${_efi32_manifest}" _efi32_manifest_before)

_aros_grub_iso_assets_collect_manifest(
    "${GIA_SOURCE_ROOT}" "${_AROS_GRUB_ISO_ASSETS_PC_MANIFEST}"
    "i386-pc" 273 8 _pc_products)
_aros_grub_iso_assets_collect_manifest(
    "${GIA_SOURCE_ROOT}" "${_AROS_GRUB_ISO_ASSETS_EFI64_MANIFEST}"
    "x86_64-efi" 268 0 _efi64_products)
_aros_grub_iso_assets_collect_manifest(
    "${GIA_SOURCE_ROOT}" "${_AROS_GRUB_ISO_ASSETS_EFI32_MANIFEST}"
    "i386-efi" 269 0 _efi32_products)

function(_gia_require_executable path label)
    _aros_grub_iso_assets_require_regular("${path}" "${label}")
    execute_process(
        COMMAND /bin/test -x "${path}"
        RESULT_VARIABLE _result)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${label} is not executable")
    endif()
endfunction()

function(_gia_validate_host_products prefix platform products label)
    if(NOT EXISTS "${prefix}" OR NOT IS_DIRECTORY "${prefix}" OR IS_SYMLINK "${prefix}")
        message(FATAL_ERROR "${label} host prefix is unavailable")
    endif()
    _aros_grub_iso_assets_reject_symlink_components(
        "${GIA_BUILD_ROOT}" "${prefix}" "${label} host prefix")
    _gia_require_executable("${prefix}/grub-mkimage" "${label} grub-mkimage")
    _aros_grub_iso_assets_reject_symlink_components(
        "${prefix}" "${prefix}/grub-mkimage" "${label} grub-mkimage")
    foreach(_relative IN LISTS products)
        _aros_grub_iso_assets_validate_relative("${label} manifest product" "${_relative}")
        set(_input "${prefix}/${_relative}")
        _aros_grub_iso_assets_require_regular("${_input}" "${label} host product")
        _aros_grub_iso_assets_reject_symlink_components(
            "${prefix}" "${_input}" "${label} host product")
    endforeach()
endfunction()

function(_gia_require_modules products platform label)
    foreach(_module IN LISTS ARGN)
        set(_needed "lib/grub/${platform}/${_module}.mod")
        list(FIND products "${_needed}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR "${label} is missing required GRUB module ${_module}")
        endif()
    endforeach()
endfunction()

_gia_validate_host_products("${GIA_HOST_PC}" "i386-pc" "${_pc_products}" "PC")
_gia_validate_host_products("${GIA_HOST_EFI64}" "x86_64-efi" "${_efi64_products}" "EFI64")
_gia_validate_host_products("${GIA_HOST_EFI32}" "i386-efi" "${_efi32_products}" "EFI32")
set(_common_modules
    fshelp part_msdos part_amiga part_gpt fat ntfs ntfscomp affs sfs ext2
    hfsplus iso9660 minicmd xzio)
_gia_require_modules("${_pc_products}" "i386-pc" "PC" biosdisk ${_common_modules})
_gia_require_modules("${_efi64_products}" "x86_64-efi" "EFI64" ${_common_modules})
_gia_require_modules("${_efi32_products}" "i386-efi" "EFI32" ${_common_modules})

set(_expected_products "")
foreach(_relative IN LISTS _pc_products)
    string(REGEX REPLACE "^lib/grub/i386-pc/" "" _name "${_relative}")
    list(APPEND _expected_products "${GIA_SYS_DIR}/boot/grub/i386-pc/${_name}")
endforeach()
list(APPEND _expected_products
    "${GIA_SYS_DIR}/boot/grub/i386-pc/core.img"
    "${GIA_SYS_DIR}/boot/grub/i386-pc/grub2_eltorito")
foreach(_relative IN LISTS _efi64_products)
    string(REGEX REPLACE "^lib/grub/x86_64-efi/" "" _name "${_relative}")
    list(APPEND _expected_products "${GIA_SYS_DIR}/EFI/BOOT/grub/x86_64-efi/${_name}")
endforeach()
list(APPEND _expected_products "${GIA_SYS_DIR}/EFI/BOOT/BOOTX64.EFI")
foreach(_relative IN LISTS _efi32_products)
    string(REGEX REPLACE "^lib/grub/i386-efi/" "" _name "${_relative}")
    list(APPEND _expected_products "${GIA_SYS_DIR}/EFI/BOOT/grub/i386-efi/${_name}")
endforeach()
list(APPEND _expected_products
    "${GIA_SYS_DIR}/EFI/BOOT/BOOTIA32.EFI"
    "${GIA_BINARY_DIR}/grub2.mods")
list(REMOVE_DUPLICATES _expected_products)
list(LENGTH _expected_products _expected_count)
if(NOT _expected_count EQUAL 832 OR NOT GIA_PRODUCTS STREQUAL _expected_products)
    message(FATAL_ERROR "GRUB2 ISO asset contract product inventory differs from the audited set")
endif()
foreach(_product IN LISTS GIA_PRODUCTS)
    cmake_path(IS_PREFIX GIA_SYS_DIR "${_product}" NORMALIZE _in_sys)
    cmake_path(IS_PREFIX GIA_BINARY_DIR "${_product}" NORMALIZE _in_private)
    if((NOT _in_sys AND NOT _in_private) OR IS_SYMLINK "${_product}")
        message(FATAL_ERROR "GRUB2 ISO asset product escapes its owner: ${_product}")
    endif()
    if(_in_sys)
        _aros_grub_iso_assets_reject_symlink_components(
            "${GIA_SYS_DIR}" "${_product}" "GRUB2 ISO SYS product")
    else()
        _aros_grub_iso_assets_reject_symlink_components(
            "${GIA_BINARY_DIR}" "${_product}" "GRUB2 ISO private product")
    endif()
endforeach()

function(_gia_run label)
    execute_process(
        COMMAND ${ARGN}
        WORKING_DIRECTORY "${GIA_BINARY_DIR}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    if(NOT _result EQUAL 0)
        message(FATAL_ERROR "${label} failed (${_result})\n${_stdout}${_stderr}")
    endif()
endfunction()

function(_gia_copy_checked input output)
    get_filename_component(_parent "${output}" DIRECTORY)
    file(MAKE_DIRECTORY "${_parent}")
    file(COPY_FILE "${input}" "${output}" ONLY_IF_DIFFERENT)
    _aros_grub_iso_assets_require_regular("${output}" "GRUB2 ISO staged product")
    file(SHA256 "${input}" _input_sha256)
    file(SHA256 "${output}" _output_sha256)
    if(NOT _input_sha256 STREQUAL _output_sha256)
        message(FATAL_ERROR "GRUB2 ISO copy changed ${input}")
    endif()
endfunction()

# The private root is the only recursive deletion.  System products are
# individually removed from the exact contract inventory, so staging cannot
# wipe unrelated SYS content when a Ninja repair is requested.
file(REMOVE_RECURSE "${GIA_BINARY_DIR}")
foreach(_product IN LISTS GIA_PRODUCTS)
    if(NOT _product STREQUAL "${GIA_BINARY_DIR}/grub2.mods")
        file(REMOVE "${_product}")
    endif()
endforeach()
file(MAKE_DIRECTORY "${GIA_BINARY_DIR}/pc" "${GIA_BINARY_DIR}/efi64"
    "${GIA_BINARY_DIR}/efi32")

set(_pc_module_dir "${GIA_HOST_PC}/lib/grub/i386-pc")
set(_efi64_module_dir "${GIA_HOST_EFI64}/lib/grub/x86_64-efi")
set(_efi32_module_dir "${GIA_HOST_EFI32}/lib/grub/i386-efi")
_gia_run("creating BIOS core.img"
    "${GIA_HOST_PC}/grub-mkimage"
    -O i386-pc -p /boot/grub -d "${_pc_module_dir}"
    -o "${GIA_BINARY_DIR}/pc/core.img" biosdisk ${_common_modules})
_gia_run("creating x86_64 EFI GRUB image"
    "${GIA_HOST_EFI64}/grub-mkimage"
    -O x86_64-efi -p /EFI/BOOT/grub -d "${_efi64_module_dir}"
    -o "${GIA_BINARY_DIR}/efi64/grub.efi" ${_common_modules})
_gia_run("creating i386 EFI GRUB image"
    "${GIA_HOST_EFI32}/grub-mkimage"
    -O i386-efi -p /EFI/BOOT/grub -d "${_efi32_module_dir}"
    -o "${GIA_BINARY_DIR}/efi32/grub.efi" ${_common_modules})
foreach(_private_image IN ITEMS
        "${GIA_BINARY_DIR}/pc/core.img"
        "${GIA_BINARY_DIR}/efi64/grub.efi"
        "${GIA_BINARY_DIR}/efi32/grub.efi")
    _aros_grub_iso_assets_require_regular("${_private_image}" "generated GRUB image")
endforeach()

foreach(_relative IN LISTS _pc_products)
    string(REGEX REPLACE "^lib/grub/i386-pc/" "" _name "${_relative}")
    _gia_copy_checked("${GIA_HOST_PC}/${_relative}"
        "${GIA_SYS_DIR}/boot/grub/i386-pc/${_name}")
endforeach()
_gia_copy_checked("${GIA_BINARY_DIR}/pc/core.img"
    "${GIA_SYS_DIR}/boot/grub/i386-pc/core.img")
_gia_require_executable(/bin/cat "GRUB2 ISO concatenation tool")
# Create the concatenated image with an explicit output file.  Keeping this
# outside a shell preserves argument boundaries and prevents an inherited PATH
# from choosing the concatenation tool.
execute_process(
    COMMAND /bin/cat "${GIA_SYS_DIR}/boot/grub/i386-pc/cdboot.img"
        "${GIA_BINARY_DIR}/pc/core.img"
    OUTPUT_FILE "${GIA_BINARY_DIR}/pc/grub2_eltorito"
    RESULT_VARIABLE _eltorito_result
    ERROR_VARIABLE _eltorito_error)
if(NOT _eltorito_result EQUAL 0)
    message(FATAL_ERROR "creating GRUB2 El Torito image failed (${_eltorito_result})\n${_eltorito_error}")
endif()
file(SIZE "${GIA_SYS_DIR}/boot/grub/i386-pc/cdboot.img" _cdboot_size)
file(SIZE "${GIA_BINARY_DIR}/pc/core.img" _core_size)
file(SIZE "${GIA_BINARY_DIR}/pc/grub2_eltorito" _eltorito_size)
math(EXPR _expected_eltorito_size "${_cdboot_size} + ${_core_size}")
if(NOT _eltorito_size EQUAL _expected_eltorito_size)
    message(FATAL_ERROR "GRUB2 El Torito image is not cdboot.img concatenated with core.img")
endif()
_gia_copy_checked("${GIA_BINARY_DIR}/pc/grub2_eltorito"
    "${GIA_SYS_DIR}/boot/grub/i386-pc/grub2_eltorito")

foreach(_relative IN LISTS _efi64_products)
    string(REGEX REPLACE "^lib/grub/x86_64-efi/" "" _name "${_relative}")
    _gia_copy_checked("${GIA_HOST_EFI64}/${_relative}"
        "${GIA_SYS_DIR}/EFI/BOOT/grub/x86_64-efi/${_name}")
endforeach()
_gia_copy_checked("${GIA_BINARY_DIR}/efi64/grub.efi"
    "${GIA_SYS_DIR}/EFI/BOOT/BOOTX64.EFI")
foreach(_relative IN LISTS _efi32_products)
    string(REGEX REPLACE "^lib/grub/i386-efi/" "" _name "${_relative}")
    _gia_copy_checked("${GIA_HOST_EFI32}/${_relative}"
        "${GIA_SYS_DIR}/EFI/BOOT/grub/i386-efi/${_name}")
endforeach()
_gia_copy_checked("${GIA_BINARY_DIR}/efi32/grub.efi"
    "${GIA_SYS_DIR}/EFI/BOOT/BOOTIA32.EFI")

list(JOIN _common_modules " " _module_line)
file(WRITE "${GIA_BINARY_DIR}/grub2.mods" "${_module_line}\n")

foreach(_product IN LISTS GIA_PRODUCTS)
    _aros_grub_iso_assets_require_regular("${_product}" "declared GRUB2 ISO asset")
endforeach()
file(SHA256 "${_host_mmake}" _host_mmake_after)
file(SHA256 "${_pc_manifest}" _pc_manifest_after)
file(SHA256 "${_efi64_manifest}" _efi64_manifest_after)
file(SHA256 "${_efi32_manifest}" _efi32_manifest_after)
if(NOT _host_mmake_before STREQUAL _host_mmake_after OR
   NOT _pc_manifest_before STREQUAL _pc_manifest_after OR
   NOT _efi64_manifest_before STREQUAL _efi64_manifest_after OR
   NOT _efi32_manifest_before STREQUAL _efi32_manifest_after)
    message(FATAL_ERROR "GRUB2 ISO staging modified its source inputs")
endif()
file(WRITE "${GIA_STAMP}" "GRUB2 ISO assets: x86_64\n")
