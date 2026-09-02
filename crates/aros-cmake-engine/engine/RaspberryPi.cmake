# =============================================================================
# Raspberry Pi model-specific boot payload
# =============================================================================
#
# The mmake-era Pi path has two hand-written links which are intentionally
# outside the generic module graph:
#
#   kernel/resource + exec + task -> core.elf
#   Pi bootstrap + core.bin.o     -> aros-<cpu>-raspi.img
#
# The generic CMake transpilation already builds the Pi BSP package.  This
# module brings the two remaining links into the modern graph and stages a
# deterministic *AROS payload* bundle under boot/<model>. Raspberry Pi firmware
# itself is not part of this target: provisioning it is a separate, pinned
# operation and a normal AROS build must never download it implicitly.

include("${CMAKE_SOURCE_DIR}/cmake/BoardCore.cmake")

if(NOT AROS_TARGET_PLATFORM STREQUAL "raspi")
    return()
endif()

set(AROS_RPI_MODEL "" CACHE STRING "Reviewed Raspberry Pi model: rpi3, rpi4, or rpi5")
set_property(CACHE AROS_RPI_MODEL PROPERTY STRINGS rpi3 rpi4 rpi5)

# The generic architecture presets build all AROS products but intentionally
# do not claim a physical board. Board presets select one exact model.
if(NOT AROS_RPI_MODEL)
    message(STATUS "🍓 Raspberry Pi board artifacts are disabled: AROS_RPI_MODEL is not selected")
    return()
endif()

if(AROS_RPI_MODEL STREQUAL "rpi3" AND AROS_TARGET_CPU STREQUAL "arm")
    set(_rpi_cpu arm)
    set(_rpi_cpu_label ARM32)
    set(_rpi_boot_source_dir "${CMAKE_SOURCE_DIR}/arch/arm-raspi/boot")
    set(_rpi_core_linker_script "${CMAKE_SOURCE_DIR}/arch/arm-native/kernel/ldscript.lds")
    set(_rpi_boot_linker_script "${CMAKE_SOURCE_DIR}/arch/arm-raspi/boot/ldscript-le.lds")
    set(_rpi_boot_image_name "aros-arm-raspi.img")
    set(_rpi_boot_elf_name "aros-arm-raspi.debug.elf")
    set(_rpi_boot_map_name "aros-arm-raspi.map")
    set(_rpi_bsp_name "aros-arm-bsp.rom")
    set(_rpi_package_target kernel-package-raspi-arm)
    set(_rpi_dtb_name "bcm2710-rpi-3-b-plus.dtb")
    set(_rpi_kobj_class "01")
    set(_rpi_kobj_machine "2800")
    set(_rpi_kobj_description "ELF32 little-endian ARM")
    set(_rpi_core_link_flags "")
    set(_rpi_objcopy_format elf32-littlearm)
    set(_rpi_objcopy_arch arm)
    set(_rpi_kernel_address 0x10000)
    set(_rpi_arm_64bit 0)
    set(_rpi_firmware_config
        "kernel=${_rpi_boot_image_name}\nkernel_address=${_rpi_kernel_address}\ninitramfs ${_rpi_bsp_name} 0x00800000\narm_64bit=0\nhdmi_drive=2\ngpu_mem=128\n")
    set(_rpi_extra_link_targets linklibs-aeabi)
    set(_rpi_extra_link_files "$<TARGET_FILE:linklibs-aeabi>")
    set(_rpi_core_strip_args --strip-unneeded -R .note -R .comment)
elseif((AROS_RPI_MODEL STREQUAL "rpi4" OR AROS_RPI_MODEL STREQUAL "rpi5") AND
       AROS_TARGET_CPU STREQUAL "aarch64")
    set(_rpi_cpu aarch64)
    set(_rpi_cpu_label AArch64)
    set(_rpi_boot_source_dir "${CMAKE_SOURCE_DIR}/arch/aarch64-raspi/boot")
    set(_rpi_core_linker_script "${CMAKE_SOURCE_DIR}/arch/aarch64-native/kernel/ldscript.lds")
    set(_rpi_boot_linker_script "${CMAKE_SOURCE_DIR}/arch/aarch64-raspi/boot/ldscript.lds")
    set(_rpi_boot_image_name "aros-aarch64-raspi.img")
    set(_rpi_boot_elf_name "aros-aarch64-raspi.debug.elf")
    set(_rpi_boot_map_name "aros-aarch64-raspi.map")
    set(_rpi_bsp_name "aros-aarch64-bsp.rom")
    set(_rpi_package_target kernel-package-raspi-aarch64)
    if(AROS_RPI_MODEL STREQUAL "rpi4")
        set(_rpi_dtb_name "bcm2711-rpi-4-b.dtb")
    else()
        set(_rpi_dtb_name "bcm2712-rpi-5-b.dtb")
    endif()
    set(_rpi_kobj_class "02")
    set(_rpi_kobj_machine "b700")
    set(_rpi_kobj_description "ELF64 little-endian AArch64")
    set(_rpi_core_link_flags --emit-relocs)
    set(_rpi_objcopy_format elf64-littleaarch64)
    set(_rpi_objcopy_arch aarch64)
    set(_rpi_kernel_address 0x80000)
    set(_rpi_arm_64bit 1)
    set(_rpi_firmware_config
        "kernel=${_rpi_boot_image_name}\nkernel_address=${_rpi_kernel_address}\ninitramfs ${_rpi_bsp_name} 0x00800000\nenable_uart=1\narm_64bit=1\ngpu_mem=128\n")
    set(_rpi_extra_link_targets "")
    set(_rpi_extra_link_files "")
    set(_rpi_core_strip_args --strip-debug -R .note -R .comment)
else()
    message(FATAL_ERROR
        "Unsupported Raspberry Pi model/CPU combination: model='${AROS_RPI_MODEL}', "
        "AROS_TARGET_CPU='${AROS_TARGET_CPU}'. Use rpi3 with arm, or rpi4/rpi5 with aarch64.")
endif()

set(AROS_RPI_ARTIFACT_DIR "${CMAKE_BINARY_DIR}/boot/${AROS_RPI_MODEL}"
    CACHE PATH "Directory for the Raspberry Pi AROS debug payload bundle")
set(AROS_RPI_DTB ""
    CACHE FILEPATH
    "Pinned ${_rpi_dtb_name} to stage in the Pi payload bundle (not downloaded by CMake)")
set(AROS_RPI_CORE_KOBJ_DIR ""
    CACHE PATH
    "Legacy raspi-${_rpi_cpu} KOBJSDIR containing kernel_resource.o, exec_library.o, and task_resource.o")

set(_rpi_bootstrap_dir "${CMAKE_BINARY_DIR}/rpi-bootstrap")
set(_rpi_bundle_dir "${AROS_RPI_ARTIFACT_DIR}")
set(_rpi_core_debug_elf "${_rpi_bootstrap_dir}/core.debug.elf")
set(_rpi_core_elf "${_rpi_bootstrap_dir}/core.elf")
set(_rpi_core_map "${_rpi_bootstrap_dir}/core.map")
set(_rpi_core_bin "${_rpi_bootstrap_dir}/core.bin")
set(_rpi_core_obj "${_rpi_bootstrap_dir}/core.bin.o")
set(_rpi_boot_elf "${_rpi_bootstrap_dir}/${_rpi_boot_elf_name}")
set(_rpi_boot_map "${_rpi_bootstrap_dir}/${_rpi_boot_map_name}")
set(_rpi_boot_img "${_rpi_bootstrap_dir}/${_rpi_boot_image_name}")
set(_rpi_bsp_rom "${CMAKE_BINARY_DIR}/${_rpi_bsp_name}")
set(_rpi_config "${_rpi_bootstrap_dir}/config.txt")
set(_rpi_bundle_stamp "${_rpi_bundle_dir}/.rpi-artifacts.stamp")
set(_rpi_verify_script "${CMAKE_SOURCE_DIR}/cmake/scripts/VerifyRpiBundle.cmake")

# A normal host objcopy can normally create an ELF binary-input wrapper, but
# llvm-objcopy is deliberately preferred: it is part of the pinned LLVM
# toolchain and understands the target ELF formats on every supported host.
find_program(AROS_RPI_OBJCOPY
    NAMES llvm-objcopy objcopy
    HINTS "$ENV{HOME}/.aros/toolchain/bin"
          "/opt/homebrew/opt/llvm/bin"
          "/opt/homebrew/bin"
    DOC "objcopy used to turn the Pi core ELF into a target binary object")
find_program(AROS_RPI_STRIP
    NAMES llvm-strip strip
    HINTS "$ENV{HOME}/.aros/toolchain/bin"
          "/opt/homebrew/opt/llvm/bin"
          "/opt/homebrew/bin"
    DOC "strip used for the embedded Raspberry Pi core ELF")

function(_aros_rpi_unavailable target reason)
    add_custom_target(${target}
        COMMAND "${CMAKE_COMMAND}" -E echo
                "${target} is unavailable for this configuration: ${reason}"
        COMMAND "${CMAKE_COMMAND}" -E echo
                "Set -DAROS_RPI_DTB=/path/to/${_rpi_dtb_name}. In a separate configured legacy raspi-${_rpi_cpu} build, run: make kernel-raspi-${_rpi_cpu}"
        COMMAND "${CMAKE_COMMAND}" -E echo
                "Then configure with -DAROS_RPI_CORE_KOBJ_DIR=<legacy-build>/bin/raspi-${_rpi_cpu}/gen/kobjs (same source revision/toolchain). Expected: kernel_resource.o, exec_library.o, task_resource.o; never SYS/Libs/kernel-*.{resource,library}."
        COMMAND "${CMAKE_COMMAND}" -E false
        VERBATIM)
endfunction()

# The CMake transpiler currently builds loadable module ELFs, while the legacy
# Pi core link deliberately consumes the partially linked KOBJs emitted by
# `kernel-raspi-${_rpi_cpu}`. They are not interchangeable: the KOBJs contain
# genmodule's resident/start/end glue and expose the unprefixed kernel API that
# core.elf needs.  Keep this bridge explicit until CMake models that KOBJ rule.
function(_aros_rpi_validate_kobj path label out_problem)
    if(NOT EXISTS "${path}")
        set(${out_problem} "missing legacy ${label}: ${path}" PARENT_SCOPE)
        return()
    endif()

    file(SIZE "${path}" _rpi_kobj_size)
    if(_rpi_kobj_size LESS 20)
        set(${out_problem} "legacy ${label} is too small to be a ${_rpi_kobj_description} object: ${path}" PARENT_SCOPE)
        return()
    endif()

    # Little-endian relocatable object for the selected target architecture.
    file(READ "${path}" _rpi_kobj_header OFFSET 0 LIMIT 20 HEX)
    string(TOLOWER "${_rpi_kobj_header}" _rpi_kobj_header)
    string(SUBSTRING "${_rpi_kobj_header}" 0 8 _rpi_kobj_magic)
    string(SUBSTRING "${_rpi_kobj_header}" 8 2 _rpi_actual_kobj_class)
    string(SUBSTRING "${_rpi_kobj_header}" 10 2 _rpi_kobj_data)
    string(SUBSTRING "${_rpi_kobj_header}" 32 4 _rpi_kobj_type)
    string(SUBSTRING "${_rpi_kobj_header}" 36 4 _rpi_actual_kobj_machine)
    if(NOT _rpi_kobj_magic STREQUAL "7f454c46" OR
       NOT _rpi_actual_kobj_class STREQUAL "${_rpi_kobj_class}" OR
       NOT _rpi_kobj_data STREQUAL "01" OR
       NOT _rpi_kobj_type STREQUAL "0100" OR
       NOT _rpi_actual_kobj_machine STREQUAL "${_rpi_kobj_machine}")
        set(${out_problem}
            "legacy ${label} is not a ${_rpi_kobj_description} relocatable object: ${path}"
            PARENT_SCOPE)
        return()
    endif()

    set(${out_problem} "" PARENT_SCOPE)
endfunction()

# Configure must remain useful while bootstrap host tools are absent.  Define
# the public targets anyway, but make an attempt to build them fail with the
# exact missing prerequisite instead of a confusing Ninja rule error.
set(_rpi_problems "")
if(NOT AROS_LLD_BIN)
    list(APPEND _rpi_problems "ld.lld was not found")
endif()
if(NOT AROS_RPI_OBJCOPY)
    list(APPEND _rpi_problems "llvm-objcopy (or a compatible objcopy) was not found")
endif()
if(NOT AROS_RPI_STRIP)
    list(APPEND _rpi_problems "llvm-strip (or a compatible strip) was not found")
endif()
if(NOT AROS_RPI_DTB OR NOT EXISTS "${AROS_RPI_DTB}")
    list(APPEND _rpi_problems
        "AROS_RPI_DTB does not name a local, pinned ${_rpi_dtb_name}")
elseif(NOT AROS_RPI_DTB MATCHES "(^|/)${_rpi_dtb_name}$")
    list(APPEND _rpi_problems
        "AROS_RPI_DTB must name ${_rpi_dtb_name}, not ${AROS_RPI_DTB}")
endif()

set(_rpi_core_kobjs "")
if(NOT AROS_RPI_CORE_KOBJ_DIR)
    list(APPEND _rpi_problems
        "AROS_RPI_CORE_KOBJ_DIR is not set (requires the legacy raspi-${_rpi_cpu} KOBJSDIR)")
else()
    get_filename_component(_rpi_kobj_dir
        "${AROS_RPI_CORE_KOBJ_DIR}" ABSOLUTE BASE_DIR "${CMAKE_BINARY_DIR}")
    if(NOT IS_DIRECTORY "${_rpi_kobj_dir}")
        list(APPEND _rpi_problems
            "AROS_RPI_CORE_KOBJ_DIR is not a directory: ${_rpi_kobj_dir}")
    else()
        foreach(_rpi_kobj_spec IN ITEMS
                "kernel_resource.o|kernel resource KOBJ"
                "exec_library.o|exec library KOBJ"
                "task_resource.o|task resource KOBJ")
            string(REPLACE "|" ";" _rpi_kobj_parts "${_rpi_kobj_spec}")
            list(GET _rpi_kobj_parts 0 _rpi_kobj_name)
            list(GET _rpi_kobj_parts 1 _rpi_kobj_label)
            set(_rpi_kobj_path "${_rpi_kobj_dir}/${_rpi_kobj_name}")
            _aros_rpi_validate_kobj("${_rpi_kobj_path}" "${_rpi_kobj_label}"
                _rpi_kobj_problem)
            if(_rpi_kobj_problem)
                list(APPEND _rpi_problems "${_rpi_kobj_problem}")
            else()
                list(APPEND _rpi_core_kobjs "${_rpi_kobj_path}")
            endif()
        endforeach()
    endif()
endif()

set(_rpi_required_targets
    ${_rpi_package_target}
    linklibs-arossupport
    linklibs-libinit
    linklibs-stdc-static
    ${_rpi_extra_link_targets})
foreach(_rpi_target IN LISTS _rpi_required_targets)
    if(NOT TARGET ${_rpi_target})
        list(APPEND _rpi_problems "CMake target '${_rpi_target}' is absent")
    endif()
endforeach()

if(_rpi_problems)
    list(JOIN _rpi_problems "; " _rpi_problem_text)
    message(STATUS "🍓 Raspberry Pi debug payload is deferred: ${_rpi_problem_text}")

    _aros_rpi_unavailable(rpi-core-elf "${_rpi_problem_text}")
    _aros_rpi_unavailable(rpi-bootstrap-elf "${_rpi_problem_text}")
    _aros_rpi_unavailable(rpi-boot-image "${_rpi_problem_text}")
    _aros_rpi_unavailable(rpi-bsp-package "${_rpi_problem_text}")
    _aros_rpi_unavailable(rpi-artifacts "${_rpi_problem_text}")
    _aros_rpi_unavailable(rpi-boot-verify "${_rpi_problem_text}")
    return()
endif()

# The legacy image uses these sources verbatim.  It is an OBJECT library so
# the final link retains the bootstrap's special order and linker script.
set(_rpi_bootstrap_sources
    "${_rpi_boot_source_dir}/boot.c"
    "${_rpi_boot_source_dir}/mmu.c"
    "${_rpi_boot_source_dir}/kprintf.c"
    "${_rpi_boot_source_dir}/support.c"
    "${_rpi_boot_source_dir}/vc_mb.c"
    "${_rpi_boot_source_dir}/serialdebug.c"
    "${_rpi_boot_source_dir}/elf.c"
    "${_rpi_boot_source_dir}/devicetree.c"
    "${_rpi_boot_source_dir}/vc_fb.c"
    "${_rpi_boot_source_dir}/bc/vars.c"
    "${_rpi_boot_source_dir}/bc/font8x14.c"
    "${_rpi_boot_source_dir}/bc/screen_fb.c")
if(_rpi_cpu STREQUAL "aarch64")
    list(PREPEND _rpi_bootstrap_sources "${_rpi_boot_source_dir}/startup64.S")
endif()

add_library(rpi-bootstrap-objects OBJECT ${_rpi_bootstrap_sources})
target_include_directories(rpi-bootstrap-objects PRIVATE
    "${_rpi_boot_source_dir}/include"
    "${CMAKE_SOURCE_DIR}/rom/openfirmware")
target_compile_definitions(rpi-bootstrap-objects PRIVATE
    "TARGET_SECTION_COMMENT=\"\""
    USE_UBOOT)
target_compile_options(rpi-bootstrap-objects PRIVATE
    -O2
    -fno-stack-protector
    -fno-pic)

# The transpiler does not yet model compiler/autoinit/%build_linklib. Keep one
# board-neutral bridge until that declaration is represented in the graph.
aros_add_board_autoinit(aros-board-autoinit)

# This is the selected arch/<cpu>-native kernel core.elf rule translated
# to explicit target files.  Its three inputs are the validated legacy KOBJs,
# never the similarly named CMake runtime module executables.
add_custom_command(
    OUTPUT "${_rpi_core_debug_elf}" "${_rpi_core_elf}" "${_rpi_core_map}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${_rpi_bootstrap_dir}"
    COMMAND "${AROS_LLD_BIN}" ${_rpi_core_link_flags}
            -Map "${_rpi_core_map}"
            -T "${_rpi_core_linker_script}"
            -o "${_rpi_core_debug_elf}"
            ${_rpi_core_kobjs}
            "$<TARGET_FILE:linklibs-arossupport>"
            "$<TARGET_FILE:aros-board-autoinit>"
            "$<TARGET_FILE:linklibs-libinit>"
            "$<TARGET_FILE:linklibs-stdc-static>"
            ${_rpi_extra_link_files}
    # Preserve the legacy payload behaviour (a stripped core is embedded),
    # while retaining the unstripped ELF next to the image for symbolization.
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_core_debug_elf}" "${_rpi_core_elf}"
    COMMAND "${AROS_RPI_STRIP}" ${_rpi_core_strip_args}
            "${_rpi_core_elf}"
    DEPENDS
            ${_rpi_core_kobjs}
            linklibs-arossupport
            aros-board-autoinit
            linklibs-libinit
            linklibs-stdc-static
            ${_rpi_extra_link_targets}
            "${_rpi_core_linker_script}"
    COMMENT "🍓 Linking Raspberry Pi ${_rpi_cpu_label} core ELF"
    VERBATIM
    COMMAND_EXPAND_LISTS)
add_custom_target(rpi-core-elf
    DEPENDS "${_rpi_core_debug_elf}" "${_rpi_core_elf}" "${_rpi_core_map}")

# ldscript.lds selects the embedded kernel by the literal `core.bin.o` object
# name, matching the old make rule.  Preserve that name even though it lives
# in a CMake-private working directory.
add_custom_command(
    OUTPUT "${_rpi_core_bin}" "${_rpi_core_obj}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_core_elf}" "${_rpi_core_bin}"
    COMMAND "${AROS_RPI_OBJCOPY}"
            -I binary -O ${_rpi_objcopy_format} -B ${_rpi_objcopy_arch}
            "${_rpi_core_bin}" "${_rpi_core_obj}"
    DEPENDS "${_rpi_core_elf}"
    COMMENT "🍓 Wrapping Raspberry Pi core ELF for the bootstrap"
    VERBATIM)

add_custom_command(
    OUTPUT "${_rpi_boot_elf}" "${_rpi_boot_map}" "${_rpi_boot_img}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${_rpi_bootstrap_dir}"
    COMMAND "${AROS_LLD_BIN}" --emit-relocs
            -Map "${_rpi_boot_map}"
            --entry=bootstrap
            --script="${_rpi_boot_linker_script}"
            $<TARGET_OBJECTS:rpi-bootstrap-objects>
            "${_rpi_core_obj}"
            "$<TARGET_FILE:linklibs-stdc-static>"
            ${_rpi_extra_link_files}
            -o "${_rpi_boot_elf}"
    COMMAND "${AROS_RPI_OBJCOPY}" -O binary
            "${_rpi_boot_elf}" "${_rpi_boot_img}"
    DEPENDS
            rpi-bootstrap-objects
            "${_rpi_core_obj}"
            linklibs-stdc-static
            ${_rpi_extra_link_targets}
            "${_rpi_boot_linker_script}"
    COMMENT "🍓 Linking Raspberry Pi AROS bootstrap image"
    VERBATIM
    COMMAND_EXPAND_LISTS)
add_custom_target(rpi-bootstrap-elf
    DEPENDS "${_rpi_boot_elf}" "${_rpi_boot_map}")
add_custom_target(rpi-boot-image DEPENDS "${_rpi_boot_img}")
add_custom_target(rpi-bsp-package DEPENDS ${_rpi_package_target})

# Keep the content identical to the legacy Pi config.  It intentionally
# names only AROS-owned payload files; start4.elf/fixup4.dat are provisioned
# separately and never fetched during an ordinary build.
file(GENERATE OUTPUT "${_rpi_config}" CONTENT
"${_rpi_firmware_config}")

add_custom_command(
    OUTPUT "${_rpi_bundle_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${_rpi_bundle_dir}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_boot_img}"
            "${_rpi_bundle_dir}/${_rpi_boot_image_name}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_boot_elf}"
            "${_rpi_bundle_dir}/${_rpi_boot_elf_name}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_boot_map}"
            "${_rpi_bundle_dir}/${_rpi_boot_map_name}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_core_debug_elf}"
            "${_rpi_bundle_dir}/core.debug.elf"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_core_map}"
            "${_rpi_bundle_dir}/core.map"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_bsp_rom}"
            "${_rpi_bundle_dir}/${_rpi_bsp_name}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_rpi_config}"
            "${_rpi_bundle_dir}/config.txt"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${AROS_RPI_DTB}"
            "${_rpi_bundle_dir}/${_rpi_dtb_name}"
    COMMAND "${CMAKE_COMMAND}"
            -DBUNDLE_DIR="${_rpi_bundle_dir}"
            -DMODEL="${AROS_RPI_MODEL}"
            -DCPU="${_rpi_cpu}"
            -DDTB_NAME="${_rpi_dtb_name}"
            -DBOOT_IMAGE_NAME="${_rpi_boot_image_name}"
            -DBOOT_ELF_NAME="${_rpi_boot_elf_name}"
            -DBOOT_MAP_NAME="${_rpi_boot_map_name}"
            -DBSP_NAME="${_rpi_bsp_name}"
            -DKERNEL_ADDRESS="${_rpi_kernel_address}"
            -DARM_64BIT="${_rpi_arm_64bit}"
            -DWRITE_MANIFEST=ON
            -P "${_rpi_verify_script}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_rpi_bundle_stamp}"
    DEPENDS
            "${_rpi_boot_img}"
            "${_rpi_boot_elf}"
            "${_rpi_boot_map}"
            "${_rpi_core_debug_elf}"
            "${_rpi_core_elf}"
            "${_rpi_core_map}"
            "${_rpi_bsp_rom}"
            "${_rpi_config}"
            "${AROS_RPI_DTB}"
            "${_rpi_verify_script}"
    COMMENT "🍓 Staging reproducible Raspberry Pi debug payload"
    VERBATIM)
add_custom_target(rpi-artifacts DEPENDS "${_rpi_bundle_stamp}")
add_custom_target(rpi-boot-verify
    COMMAND "${CMAKE_COMMAND}"
            -DBUNDLE_DIR="${_rpi_bundle_dir}"
            -DMODEL="${AROS_RPI_MODEL}"
            -DCPU="${_rpi_cpu}"
            -DDTB_NAME="${_rpi_dtb_name}"
            -DBOOT_IMAGE_NAME="${_rpi_boot_image_name}"
            -DBOOT_ELF_NAME="${_rpi_boot_elf_name}"
            -DBOOT_MAP_NAME="${_rpi_boot_map_name}"
            -DBSP_NAME="${_rpi_bsp_name}"
            -DKERNEL_ADDRESS="${_rpi_kernel_address}"
            -DARM_64BIT="${_rpi_arm_64bit}"
            -DWRITE_MANIFEST=OFF
            -P "${_rpi_verify_script}"
    DEPENDS rpi-artifacts
    COMMENT "🍓 Verifying Raspberry Pi debug payload"
    VERBATIM)

message(STATUS "🍓 Raspberry Pi ${AROS_RPI_MODEL} debug payload: rpi-artifacts -> ${_rpi_bundle_dir}")
