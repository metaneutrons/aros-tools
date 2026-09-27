cmake_minimum_required(VERSION 3.22)

get_filename_component(AROS_TEST_ENGINE_DIR "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)

include("${AROS_TEST_ENGINE_DIR}/GrubIsoAssets.cmake")
include("${AROS_TEST_ENGINE_DIR}/GrubSourceLock.cmake")

if(AROS_TEST_INVALID_GRUB_VERSION)
    _aros_grub2_source_lock("2.17")
    message(FATAL_ERROR "unsupported GRUB version was accepted")
endif()

foreach(_case IN ITEMS "2.12|273|268|269|832" "2.16|301|305|304|932")
    string(REPLACE "|" ";" _parts "${_case}")
    list(GET _parts 0 _version)
    list(GET _parts 1 _pc_modules)
    list(GET _parts 2 _efi64_modules)
    list(GET _parts 3 _efi32_modules)
    list(GET _parts 4 _expected_products)
    _aros_grub2_source_lock("${_version}")
    _aros_grub_iso_assets_lock("${_version}")
    if(NOT _AROS_GRUB2_VERSION STREQUAL _version OR
       NOT _AROS_GRUB_ISO_ASSETS_PRODUCT_COUNT EQUAL _expected_products)
        message(FATAL_ERROR "GRUB ${_version} source or ISO lock did not select the audited version")
    endif()
    _aros_grub_iso_assets_collect_manifest(
        "${_AROS_GRUB_ISO_ASSETS_PC_MANIFEST}"
        "i386-pc" "${_pc_modules}" 8 _pc_products)
    _aros_grub_iso_assets_collect_manifest(
        "${_AROS_GRUB_ISO_ASSETS_EFI64_MANIFEST}"
        "x86_64-efi" "${_efi64_modules}" 0 _efi64_products)
    _aros_grub_iso_assets_collect_manifest(
        "${_AROS_GRUB_ISO_ASSETS_EFI32_MANIFEST}"
        "i386-efi" "${_efi32_modules}" 0 _efi32_products)
    list(LENGTH _pc_products _pc_count)
    list(LENGTH _efi64_products _efi64_count)
    list(LENGTH _efi32_products _efi32_count)
    math(EXPR _actual_products "${_pc_count} + ${_efi64_count} + ${_efi32_count} + 5")
    if(NOT _actual_products EQUAL _expected_products)
        message(FATAL_ERROR
            "GRUB ${_version} ISO products: ${_actual_products}, expected ${_expected_products}")
    endif()
endforeach()

execute_process(
    COMMAND "${CMAKE_COMMAND}" "-DAROS_TEST_ENGINE_DIR=${AROS_TEST_ENGINE_DIR}"
        -DAROS_TEST_INVALID_GRUB_VERSION=ON -P "${CMAKE_CURRENT_LIST_FILE}"
    RESULT_VARIABLE _invalid_result
    OUTPUT_VARIABLE _invalid_stdout
    ERROR_VARIABLE _invalid_stderr)
if(_invalid_result EQUAL 0 OR
   NOT "${_invalid_stdout}${_invalid_stderr}" MATCHES "unsupported GRUB2 source version")
    message(FATAL_ERROR "GRUB source lock did not reject an unknown version")
endif()
