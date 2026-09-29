include_guard(GLOBAL)
include("${CMAKE_CURRENT_LIST_DIR}/Executable.cmake")

# Compose a BIOS-bootable PC ISO from the complete native AROS SYS producer
# and audited GRUB El Torito assets. The default build stays unchanged; this
# is an explicit distribution target.
function(aros_add_pc_boot_iso)
    if(NOT AROS_TARGET_CPU STREQUAL "x86_64" OR
       NOT AROS_TARGET_PLATFORM STREQUAL "pc" OR
       NOT TARGET aros-grub2-iso-assets OR NOT TARGET AROS)
        message(FATAL_ERROR
            "boot-iso requires the native AROS SYS target and audited x86_64-pc GRUB2 assets")
    endif()
    if(TARGET boot-iso)
        message(FATAL_ERROR "boot-iso is already declared")
    endif()
    find_program(AROS_XORRISO_BIN NAMES xorriso
        HINTS "/opt/homebrew/bin" "/usr/bin" "/usr/local/bin")
    if(NOT AROS_XORRISO_BIN)
        message(STATUS "boot-iso unavailable: install xorriso")
        return()
    endif()
    if(NOT AROS_MEDIA_CLI_BIN)
        set(AROS_MEDIA_CLI_BIN "${AROS_RUST_TOOLS_DIR}/aros" CACHE FILEPATH
            "aros executable used to record verified media inputs")
    endif()
    aros_path_is_executable("${AROS_MEDIA_CLI_BIN}" _media_cli_available)
    if(NOT _media_cli_available)
        message(FATAL_ERROR "boot-iso requires the aros media receipt executable")
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

    add_custom_target(boot-iso
        COMMAND "${CMAKE_COMMAND}"
            "-DSYS_DIR=${CMAKE_BINARY_DIR}/SYS"
            "-DSTAGE_DIR=${_stage_dir}"
            "-DCONFIG_SOURCE=${_grub_config}"
            "-DSTARTUP_SOURCE=${_startup_source}"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/PreparePcBootIso.cmake"
        COMMAND "${AROS_MEDIA_CLI_BIN}" image receipt
            --profile pc-bios-iso
            --build-root "${CMAKE_BINARY_DIR}"
            --source-root "${AROS_SOURCE_DIR}"
            --toolchain-root "${AROS_CROSS_TOOLCHAIN_ROOT}"
            --file "bootstrap=gen/boot-iso/stage/boot/pc/bootstrap"
            --file "grub-boot-image=gen/boot-iso/stage/boot/grub/i386-pc/grub2_eltorito"
            --tree "sys-tree=gen/boot-iso/stage"
        COMMAND "${CMAKE_COMMAND}"
            "-DMEDIA_CLI=${AROS_MEDIA_CLI_BIN}"
            "-DBUILD_ROOT=${CMAKE_BINARY_DIR}"
            "-DSOURCE_ROOT=${AROS_SOURCE_DIR}"
            "-DTOOLCHAIN_ROOT=${AROS_CROSS_TOOLCHAIN_ROOT}"
            "-DISO_PATH=${AROS_BOOT_ISO}"
            -P "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/ComposePcBootIso.cmake"
        DEPENDS "${_modules_file}" "${_startup_source}" "${_grub_config}"
            "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/PreparePcBootIso.cmake"
            "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/ComposePcBootIso.cmake"
        COMMENT "Packaging native AROS SYS tree as BIOS-bootable PC ISO -> ${AROS_BOOT_ISO}"
        VERBATIM)
    add_dependencies(boot-iso AROS aros-grub2-iso-assets)
endfunction()
