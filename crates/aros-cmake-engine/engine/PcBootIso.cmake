include_guard(GLOBAL)

# Compose a BIOS-bootable PC ISO from the audited GRUB El Torito image. The
# default AROS build stays unchanged; this is an explicit distribution target.
function(aros_add_pc_boot_iso)
    if(NOT AROS_TARGET_CPU STREQUAL "x86_64" OR
       NOT AROS_TARGET_PLATFORM STREQUAL "pc" OR
       NOT TARGET aros-grub2-iso-assets)
        message(FATAL_ERROR "boot-iso requires audited x86_64-pc GRUB2 assets")
    endif()
    if(TARGET boot-iso)
        message(FATAL_ERROR "boot-iso is already declared")
    endif()
    find_program(AROS_MKISOFS_BIN NAMES mkisofs genisoimage
        HINTS "/opt/homebrew/bin" "/usr/bin" "/usr/local/bin")
    if(NOT AROS_MKISOFS_BIN)
        message(STATUS "boot-iso unavailable: install mkisofs or genisoimage")
        return()
    endif()

    set(_modules_file "${AROS_SOURCE_DIR}/arch/x86_64-pc/boot/modules.default")
    if(NOT EXISTS "${_modules_file}" OR IS_DIRECTORY "${_modules_file}" OR
       IS_SYMLINK "${_modules_file}")
        message(FATAL_ERROR "boot-iso requires a regular x86_64-pc modules.default")
    endif()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${_modules_file}")
    file(STRINGS "${_modules_file}" _module_lines)
    set(_module_commands "")
    set(_module_inputs "")
    foreach(_line IN LISTS _module_lines)
        string(STRIP "${_line}" _line)
        if(_line STREQUAL "" OR _line MATCHES "^#")
            continue()
        endif()
        string(REPLACE "@arch.dir@" "pc" _line "${_line}")
        string(REPLACE ".@pkg.fmt@" "" _line "${_line}")
        if(NOT _line MATCHES "^/boot/[A-Za-z0-9_./-]+$" OR
           _line MATCHES "\\.\\." OR _line MATCHES "@")
            message(FATAL_ERROR "boot-iso has an unsupported module path: ${_line}")
        endif()
        list(APPEND _module_inputs "${CMAKE_BINARY_DIR}/SYS${_line}")
        string(APPEND _module_commands "    module2 ${_line}\n")
    endforeach()
    if(NOT _module_inputs)
        message(FATAL_ERROR "boot-iso has no modules")
    endif()
    list(GET _module_inputs 0 _first_module)
    if(NOT _first_module STREQUAL "${CMAKE_BINARY_DIR}/SYS/boot/pc/kernel")
        message(FATAL_ERROR "boot-iso requires the kernel ELF as its first module")
    endif()
    set(_startup_source "${AROS_SOURCE_DIR}/workbench/s/Startup-Sequence")
    set(_grub_config "${CMAKE_BINARY_DIR}/gen/boot-iso/grub.cfg")
    set(_stage_dir "${CMAKE_BINARY_DIR}/gen/boot-iso/stage")
    file(MAKE_DIRECTORY "${CMAKE_BINARY_DIR}/gen/boot-iso")
    string(CONCAT _grub_content
        "set default=0\n"
        "set timeout=5\n"
        "insmod all_video\n"
        "insmod multiboot2\n"
        "menuentry 'AROS x86-64 (native graphics)' {\n"
        "    multiboot2 /boot/pc/bootstrap ATA=32bit debug=serial\n"
        "${_module_commands}"
        "}\n"
        "menuentry 'AROS x86-64 (VESA 800x600x32)' {\n"
        "    multiboot2 /boot/pc/bootstrap vesa=800x600x32 ATA=32bit nomonitors debug=serial\n"
        "${_module_commands}"
        "}\n")
    file(WRITE "${_grub_config}" "${_grub_content}")

    set(_iso_temp "${AROS_BOOT_ISO}.tmp")
    add_custom_target(boot-iso
        COMMAND "${CMAKE_COMMAND}"
            "-DSYS_DIR=${CMAKE_BINARY_DIR}/SYS"
            "-DSTAGE_DIR=${_stage_dir}"
            "-DCONFIG_SOURCE=${_grub_config}"
            "-DSTARTUP_SOURCE=${_startup_source}"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/PreparePcBootIso.cmake"
        COMMAND "${CMAKE_COMMAND}" -E rm -f "${_iso_temp}"
        COMMAND "${AROS_MKISOFS_BIN}" -o "${_iso_temp}"
            -b boot/grub/i386-pc/grub2_eltorito
            -c boot/grub/boot.catalog
            -no-emul-boot -boot-load-size 4 -boot-info-table
            -allow-leading-dots -iso-level 4
            -V "AROS Live CD" -p "The AROS Dev Team" -l -J -r
            "${_stage_dir}"
        COMMAND "${CMAKE_COMMAND}" "-DISO_PATH=${_iso_temp}"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/VerifyPcBootIso.cmake"
        COMMAND "${CMAKE_COMMAND}" -E rename "${_iso_temp}" "${AROS_BOOT_ISO}"
        DEPENDS "${_modules_file}" "${_startup_source}" "${_grub_config}"
            "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/PreparePcBootIso.cmake"
            "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/VerifyPcBootIso.cmake"
        COMMENT "Packaging existing AROS SYS tree as BIOS-bootable PC ISO -> ${AROS_BOOT_ISO}"
        VERBATIM)
    add_dependencies(boot-iso aros-grub2-iso-assets)
endfunction()
