cmake_minimum_required(VERSION 3.22)

foreach(_input IN ITEMS SYS_DIR STAGE_DIR CONFIG_SOURCE STARTUP_SOURCE
        CPU_SIGNATURE GRUB2_STAMP GRUB2_PRIVATE_IMAGE)
    if(NOT DEFINED ${_input} OR "${${_input}}" STREQUAL "")
        message(FATAL_ERROR "boot-iso requires ${_input}")
    endif()
endforeach()
if(NOT CPU_SIGNATURE MATCHES "^[A-Za-z0-9_+-]+$")
    message(FATAL_ERROR "boot-iso has an invalid CPU signature")
endif()
if(NOT IS_DIRECTORY "${SYS_DIR}" OR IS_SYMLINK "${SYS_DIR}")
    message(FATAL_ERROR "boot-iso requires an existing regular SYS directory; build AROS first")
endif()
get_filename_component(_build_dir "${SYS_DIR}" DIRECTORY)
if(NOT STAGE_DIR STREQUAL "${_build_dir}/gen/boot-iso/stage" OR
   NOT GRUB2_STAMP STREQUAL
       "${_build_dir}/gen/grub2-iso-assets/x86_64/.grub2-iso-assets.stamp" OR
   NOT GRUB2_PRIVATE_IMAGE STREQUAL
       "${_build_dir}/gen/grub2-iso-assets/x86_64/pc/grub2_eltorito" OR
   IS_SYMLINK "${STAGE_DIR}" OR
   IS_SYMLINK "${_build_dir}/gen" OR
   IS_SYMLINK "${_build_dir}/gen/boot-iso" OR
   IS_SYMLINK "${_build_dir}/gen/grub2-iso-assets" OR
   IS_SYMLINK "${_build_dir}/gen/grub2-iso-assets/x86_64" OR
   IS_SYMLINK "${_build_dir}/gen/grub2-iso-assets/x86_64/pc")
    message(FATAL_ERROR "boot-iso refuses an unsafe staging directory")
endif()
foreach(_directory IN ITEMS "${SYS_DIR}/boot" "${SYS_DIR}/boot/pc"
        "${SYS_DIR}/boot/grub" "${SYS_DIR}/boot/grub/i386-pc"
        "${SYS_DIR}/S")
    if(IS_SYMLINK "${_directory}" OR
       (EXISTS "${_directory}" AND NOT IS_DIRECTORY "${_directory}"))
        message(FATAL_ERROR "boot-iso refuses an unsafe SYS directory: ${_directory}")
    endif()
endforeach()
foreach(_output IN ITEMS "${SYS_DIR}/S/Startup-Sequence"
        "${SYS_DIR}/boot/grub/grub.cfg")
    if(IS_SYMLINK "${_output}" OR
       (EXISTS "${_output}" AND IS_DIRECTORY "${_output}"))
        message(FATAL_ERROR "boot-iso refuses an unsafe SYS output: ${_output}")
    endif()
endforeach()
foreach(_file IN ITEMS "${CONFIG_SOURCE}" "${STARTUP_SOURCE}"
        "${SYS_DIR}/boot/pc/bootstrap"
        "${SYS_DIR}/boot/grub/i386-pc/grub2_eltorito")
    if(NOT EXISTS "${_file}" OR IS_DIRECTORY "${_file}" OR IS_SYMLINK "${_file}")
        message(FATAL_ERROR "boot-iso is missing a regular input: ${_file}")
    endif()
endforeach()
foreach(_grub_input IN ITEMS "${GRUB2_STAMP}" "${GRUB2_PRIVATE_IMAGE}")
    if(NOT EXISTS "${_grub_input}" OR IS_DIRECTORY "${_grub_input}" OR
       IS_SYMLINK "${_grub_input}")
        message(FATAL_ERROR "boot-iso requires a regular audited GRUB input: ${_grub_input}")
    endif()
endforeach()
file(STRINGS "${GRUB2_STAMP}" _grub_stamp)
list(LENGTH _grub_stamp _stamp_lines)
if(NOT _stamp_lines EQUAL 2)
    message(FATAL_ERROR "boot-iso GRUB asset stamp has an invalid inventory")
endif()
list(GET _grub_stamp 0 _stamp_identity)
list(GET _grub_stamp 1 _stamp_digest_line)
if(NOT _stamp_identity MATCHES "^GRUB2 ISO assets: [0-9][0-9.]* x86_64$" OR
   NOT _stamp_digest_line MATCHES "^El Torito SHA256: [0-9a-f]+$")
    message(FATAL_ERROR "boot-iso GRUB asset stamp has an invalid identity")
endif()
string(REPLACE "El Torito SHA256: " "" _expected_grub_sha256 "${_stamp_digest_line}")
string(LENGTH "${_expected_grub_sha256}" _expected_grub_digest_length)
if(NOT _expected_grub_digest_length EQUAL 64)
    message(FATAL_ERROR "boot-iso GRUB asset stamp has an invalid digest")
endif()
foreach(_grub_input IN ITEMS "${GRUB2_PRIVATE_IMAGE}"
        "${SYS_DIR}/boot/grub/i386-pc/grub2_eltorito")
    file(SHA256 "${_grub_input}" _observed_grub_sha256)
    if(NOT _observed_grub_sha256 STREQUAL _expected_grub_sha256)
        message(FATAL_ERROR "boot-iso GRUB input changed after audited staging: ${_grub_input}")
    endif()
endforeach()
file(STRINGS "${CONFIG_SOURCE}" _modules REGEX "^    module2 /")
if(NOT _modules)
    message(FATAL_ERROR "boot-iso has no AROS modules in its GRUB configuration")
endif()
foreach(_module IN LISTS _modules)
    string(REGEX REPLACE "^    module2 /" "" _relative "${_module}")
    if(NOT _relative MATCHES "^boot/[A-Za-z0-9_./-]+$" OR
       _relative MATCHES "\\.\\.")
        message(FATAL_ERROR "boot-iso has an unsafe module path")
    endif()
    set(_file "${SYS_DIR}/${_relative}")
    if(NOT EXISTS "${_file}" OR IS_DIRECTORY "${_file}" OR IS_SYMLINK "${_file}")
        message(FATAL_ERROR "boot-iso is missing a regular module: ${_file}")
    endif()
endforeach()
file(REMOVE_RECURSE "${STAGE_DIR}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_directory "${SYS_DIR}" "${STAGE_DIR}"
    RESULT_VARIABLE _stage_result)
if(NOT _stage_result EQUAL 0)
    message(FATAL_ERROR "boot-iso could not isolate the SYS tree")
endif()
# MetaMake's `boot` target writes `$(CPU)\n` as AROS.boot. Without this
# signature, DOS correctly rejects even a present, mountable CD as unbootable.
set(_boot_signature "${STAGE_DIR}/AROS.boot")
if(IS_SYMLINK "${_boot_signature}" OR
   (EXISTS "${_boot_signature}" AND IS_DIRECTORY "${_boot_signature}"))
    message(FATAL_ERROR "boot-iso refuses an unsafe AROS.boot")
endif()
if(EXISTS "${_boot_signature}")
    file(SIZE "${_boot_signature}" _signature_size)
    string(LENGTH "${CPU_SIGNATURE}\n" _expected_signature_size)
    if(NOT _signature_size EQUAL _expected_signature_size)
        message(FATAL_ERROR "boot-iso has a mismatched AROS.boot")
    endif()
    file(READ "${_boot_signature}" _existing_signature)
    if(NOT _existing_signature STREQUAL "${CPU_SIGNATURE}\n")
        message(FATAL_ERROR "boot-iso has a mismatched AROS.boot")
    endif()
else()
    file(WRITE "${_boot_signature}" "${CPU_SIGNATURE}\n")
endif()
file(MAKE_DIRECTORY "${STAGE_DIR}/S" "${STAGE_DIR}/boot/grub")
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${STARTUP_SOURCE}" "${STAGE_DIR}/S/Startup-Sequence"
    RESULT_VARIABLE _startup_result)
if(NOT _startup_result EQUAL 0)
    message(FATAL_ERROR "boot-iso could not stage Startup-Sequence")
endif()
execute_process(
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
        "${CONFIG_SOURCE}" "${STAGE_DIR}/boot/grub/grub.cfg"
    RESULT_VARIABLE _config_result)
if(NOT _config_result EQUAL 0)
    message(FATAL_ERROR "boot-iso could not stage grub.cfg")
endif()
