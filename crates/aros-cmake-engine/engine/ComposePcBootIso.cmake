cmake_minimum_required(VERSION 3.22)

foreach(_name IN ITEMS MEDIA_CLI BUILD_ROOT SOURCE_ROOT TOOLCHAIN_ROOT ISO_PATH)
    if(NOT DEFINED ${_name} OR "${${_name}}" STREQUAL "")
        message(FATAL_ERROR "boot-iso composition requires ${_name}")
    endif()
endforeach()
if(NOT EXISTS "${MEDIA_CLI}" OR IS_DIRECTORY "${MEDIA_CLI}")
    message(FATAL_ERROR "boot-iso requires an existing media CLI executable")
endif()
if(NOT IS_DIRECTORY "${BUILD_ROOT}" OR IS_SYMLINK "${BUILD_ROOT}")
    message(FATAL_ERROR "boot-iso requires a regular build root")
endif()
set(_receipt "${BUILD_ROOT}/media-build-receipt.json")
if(NOT EXISTS "${_receipt}" OR IS_DIRECTORY "${_receipt}" OR
   IS_SYMLINK "${_receipt}")
    message(FATAL_ERROR "boot-iso requires a regular verified media receipt")
endif()
set(_artifact_parent "${BUILD_ROOT}/gen/boot-iso/artifacts")
cmake_path(IS_PREFIX BUILD_ROOT "${_artifact_parent}" NORMALIZE _owned)
if(NOT _owned OR IS_SYMLINK "${BUILD_ROOT}/gen" OR
   IS_SYMLINK "${BUILD_ROOT}/gen/boot-iso" OR
   IS_SYMLINK "${_artifact_parent}")
    message(FATAL_ERROR "boot-iso artifact path is unsafe")
endif()
file(MAKE_DIRECTORY "${_artifact_parent}")
file(SHA256 "${_receipt}" _receipt_sha256)
file(SHA256 "${MEDIA_CLI}" _cli_sha256)
set(_artifact "${_artifact_parent}/${_receipt_sha256}-${_cli_sha256}")
if(IS_SYMLINK "${_artifact}" OR
   (EXISTS "${_artifact}" AND NOT IS_DIRECTORY "${_artifact}"))
    message(FATAL_ERROR "boot-iso artifact cache path is unsafe")
endif()

# Revalidate the complete SYS tree and source/toolchain identity even when an
# identical receipt already has a cached, independently verified artifact.
execute_process(
    COMMAND "${MEDIA_CLI}" image build
        --profile pc-bios-iso --build-root "${BUILD_ROOT}"
        --receipt "${_receipt}" --source-root "${SOURCE_ROOT}"
        --toolchain-root "${TOOLCHAIN_ROOT}" --output "${_artifact}"
        --dry-run
    RESULT_VARIABLE _plan_result
    OUTPUT_VARIABLE _plan_stdout
    ERROR_VARIABLE _plan_stderr)
if(NOT _plan_result EQUAL 0)
    message(FATAL_ERROR
        "boot-iso media plan rejected its current inputs\n${_plan_stdout}${_plan_stderr}")
endif()
if(NOT EXISTS "${_artifact}")
    execute_process(
        COMMAND "${MEDIA_CLI}" image build
            --profile pc-bios-iso --build-root "${BUILD_ROOT}"
            --receipt "${_receipt}" --source-root "${SOURCE_ROOT}"
            --toolchain-root "${TOOLCHAIN_ROOT}" --output "${_artifact}"
            --apply
        RESULT_VARIABLE _build_result
        OUTPUT_VARIABLE _build_stdout
        ERROR_VARIABLE _build_stderr)
    if(NOT _build_result EQUAL 0)
        message(FATAL_ERROR
            "boot-iso composition failed\n${_build_stdout}${_build_stderr}")
    endif()
endif()
execute_process(
    COMMAND "${MEDIA_CLI}" image verify --artifact "${_artifact}"
    RESULT_VARIABLE _verify_result
    OUTPUT_VARIABLE _verify_stdout
    ERROR_VARIABLE _verify_stderr)
if(NOT _verify_result EQUAL 0)
    message(FATAL_ERROR
        "boot-iso artifact verification failed\n${_verify_stdout}${_verify_stderr}")
endif()
set(_manifest "${_artifact}/media-image.json")
set(_image "${_artifact}/aros-media.iso")
foreach(_path IN ITEMS "${_manifest}" "${_image}")
    if(NOT EXISTS "${_path}" OR IS_DIRECTORY "${_path}" OR IS_SYMLINK "${_path}")
        message(FATAL_ERROR "boot-iso artifact omits a regular file: ${_path}")
    endif()
endforeach()
file(READ "${_manifest}" _manifest_json)
string(JSON _manifest_receipt ERROR_VARIABLE _json_error
    GET "${_manifest_json}" receipt_sha256)
if(_json_error OR NOT _manifest_receipt STREQUAL _receipt_sha256)
    message(FATAL_ERROR "boot-iso artifact is not bound to the current receipt")
endif()
string(JSON _image_sha256 ERROR_VARIABLE _json_error
    GET "${_manifest_json}" image sha256)
string(LENGTH "${_image_sha256}" _digest_length)
if(_json_error OR NOT _digest_length EQUAL 64 OR
   NOT _image_sha256 MATCHES "^[0-9a-f]+$")
    message(FATAL_ERROR "boot-iso artifact omits its image digest")
endif()
file(SHA256 "${_image}" _observed_sha256)
if(NOT _image_sha256 STREQUAL _observed_sha256)
    message(FATAL_ERROR "boot-iso artifact image digest changed")
endif()

# Preserve the previous stable ISO on any failed plan, composition, read-back,
# or copy. Its new content becomes visible only after the verified copy exists.
set(_iso_pending "${ISO_PATH}.pending")
foreach(_path IN ITEMS "${ISO_PATH}" "${_iso_pending}")
    if(IS_SYMLINK "${_path}" OR (EXISTS "${_path}" AND IS_DIRECTORY "${_path}"))
        message(FATAL_ERROR "boot-iso output path is unsafe: ${_path}")
    endif()
endforeach()
file(COPY_FILE "${_image}" "${_iso_pending}" ONLY_IF_DIFFERENT)
file(SHA256 "${_iso_pending}" _pending_sha256)
if(NOT _pending_sha256 STREQUAL _image_sha256)
    message(FATAL_ERROR "boot-iso output copy differs from its verified image")
endif()
file(RENAME "${_iso_pending}" "${ISO_PATH}" RESULT _rename_result)
if(NOT _rename_result STREQUAL "0")
    message(FATAL_ERROR "boot-iso could not publish its verified ISO: ${_rename_result}")
endif()
message(STATUS "Verified PC BIOS ISO: ${ISO_PATH} (${_image_sha256})")
