# Milk-V Titan OpenSBI/UEFI payload built from the upstream RISC-V boot path.

if(NOT AROS_TARGET_PLATFORM STREQUAL "opensbi")
    return()
endif()

include("${CMAKE_SOURCE_DIR}/cmake/BoardCore.cmake")

set(AROS_OPENSBI_MODEL "" CACHE STRING "Reviewed OpenSBI/UEFI board model")
set_property(CACHE AROS_OPENSBI_MODEL PROPERTY STRINGS milk-v-titan)
if(NOT AROS_OPENSBI_MODEL)
    message(STATUS "OpenSBI/UEFI board artifacts are disabled: AROS_OPENSBI_MODEL is not selected")
    return()
endif()
if(NOT AROS_OPENSBI_MODEL STREQUAL "milk-v-titan" OR
   NOT AROS_TARGET_CPU STREQUAL "riscv64")
    message(FATAL_ERROR
        "Unsupported OpenSBI/UEFI model/CPU combination: model='${AROS_OPENSBI_MODEL}', "
        "AROS_TARGET_CPU='${AROS_TARGET_CPU}'")
endif()

set(AROS_OPENSBI_ARTIFACT_DIR
    "${CMAKE_BINARY_DIR}/boot/${AROS_OPENSBI_MODEL}"
    CACHE PATH "Directory for the OpenSBI/UEFI board payload")
set(AROS_OPENSBI_CORE_KOBJ_DIR "" CACHE PATH
    "Legacy opensbi-riscv64 KOBJSDIR containing the three early core objects")

find_program(AROS_OPENSBI_OBJCOPY NAMES llvm-objcopy objcopy
    HINTS "$ENV{HOME}/.aros/toolchain/bin" "/opt/homebrew/opt/llvm/bin" "/opt/homebrew/bin")
find_program(AROS_OPENSBI_STRIP NAMES llvm-strip strip
    HINTS "$ENV{HOME}/.aros/toolchain/bin" "/opt/homebrew/opt/llvm/bin" "/opt/homebrew/bin")

set(_opensbi_work_dir "${CMAKE_BINARY_DIR}/opensbi-uefi-bootstrap")
set(_opensbi_bundle_dir "${AROS_OPENSBI_ARTIFACT_DIR}")
set(_opensbi_debug_elf "${_opensbi_work_dir}/core.debug.elf")
set(_opensbi_core_elf "${_opensbi_work_dir}/core.elf")
set(_opensbi_core_map "${_opensbi_work_dir}/core.map")
set(_opensbi_image "${_opensbi_work_dir}/Image")
set(_opensbi_bsp "${AROS_BOOT_ARCH_DIR}/aros-bsp.pkg")
set(_opensbi_command_line "${_opensbi_work_dir}/aros.cmd")
set(_opensbi_startup "${_opensbi_work_dir}/startup.nsh")
set(_opensbi_stamp "${_opensbi_bundle_dir}/.opensbi-uefi-artifacts.stamp")
set(_opensbi_linker_script
    "${CMAKE_SOURCE_DIR}/arch/riscv64-opensbi/kernel/ldscript.lds")
set(_opensbi_verify_script
    "${CMAKE_SOURCE_DIR}/cmake/scripts/VerifyOpenSbiUefiBundle.cmake")

function(_aros_opensbi_unavailable target reason)
    add_custom_target(${target}
        COMMAND "${CMAKE_COMMAND}" -E echo
                "${target} is unavailable for this configuration: ${reason}"
        COMMAND "${CMAKE_COMMAND}" -E echo
                "In a legacy opensbi-riscv64 build, run: make kernel-opensbi-riscv64"
        COMMAND "${CMAKE_COMMAND}" -E echo
                "Then set AROS_OPENSBI_CORE_KOBJ_DIR to its gen/kobjs directory."
        COMMAND "${CMAKE_COMMAND}" -E false
        VERBATIM)
endfunction()

function(_aros_opensbi_validate_kobj path label out_problem)
    if(NOT EXISTS "${path}")
        set(${out_problem} "missing legacy ${label}: ${path}" PARENT_SCOPE)
        return()
    endif()
    file(SIZE "${path}" _size)
    if(_size LESS 20)
        set(${out_problem} "legacy ${label} is too small: ${path}" PARENT_SCOPE)
        return()
    endif()
    file(READ "${path}" _header OFFSET 0 LIMIT 20 HEX)
    string(TOLOWER "${_header}" _header)
    string(SUBSTRING "${_header}" 0 8 _magic)
    string(SUBSTRING "${_header}" 8 2 _class)
    string(SUBSTRING "${_header}" 10 2 _data)
    string(SUBSTRING "${_header}" 32 4 _type)
    string(SUBSTRING "${_header}" 36 4 _machine)
    if(NOT _magic STREQUAL "7f454c46" OR NOT _class STREQUAL "02" OR
       NOT _data STREQUAL "01" OR NOT _type STREQUAL "0100" OR
       NOT _machine STREQUAL "f300")
        set(${out_problem}
            "legacy ${label} is not an ELF64 little-endian RISC-V relocatable object: ${path}"
            PARENT_SCOPE)
        return()
    endif()
    set(${out_problem} "" PARENT_SCOPE)
endfunction()

set(_opensbi_problems "")
if(NOT AROS_LLD_BIN)
    list(APPEND _opensbi_problems "ld.lld was not found")
endif()
if(NOT AROS_OPENSBI_OBJCOPY)
    list(APPEND _opensbi_problems "llvm-objcopy was not found")
endif()
if(NOT AROS_OPENSBI_STRIP)
    list(APPEND _opensbi_problems "llvm-strip was not found")
endif()

set(_opensbi_kobjs "")
if(NOT AROS_OPENSBI_CORE_KOBJ_DIR)
    list(APPEND _opensbi_problems "AROS_OPENSBI_CORE_KOBJ_DIR is not set")
else()
    get_filename_component(_opensbi_kobj_dir "${AROS_OPENSBI_CORE_KOBJ_DIR}"
        ABSOLUTE BASE_DIR "${CMAKE_BINARY_DIR}")
    foreach(_spec IN ITEMS
            "kernel_resource.o|kernel resource KOBJ"
            "exec_library.o|exec library KOBJ"
            "task_resource.o|task resource KOBJ")
        string(REPLACE "|" ";" _parts "${_spec}")
        list(GET _parts 0 _name)
        list(GET _parts 1 _label)
        set(_path "${_opensbi_kobj_dir}/${_name}")
        _aros_opensbi_validate_kobj("${_path}" "${_label}" _problem)
        if(_problem)
            list(APPEND _opensbi_problems "${_problem}")
        else()
            list(APPEND _opensbi_kobjs "${_path}")
        endif()
    endforeach()
endif()

foreach(_target IN ITEMS kernel-bsp-opensbi-riscv64 linklibs-arossupport
        linklibs-libinit linklibs-stdc-static)
    if(NOT TARGET ${_target})
        list(APPEND _opensbi_problems "CMake target '${_target}' is absent")
    endif()
endforeach()

if(_opensbi_problems)
    list(JOIN _opensbi_problems "; " _problem_text)
    message(STATUS "OpenSBI/UEFI payload is deferred: ${_problem_text}")
    foreach(_target IN ITEMS opensbi-uefi-core opensbi-uefi-artifacts
            opensbi-uefi-verify)
        _aros_opensbi_unavailable(${_target} "${_problem_text}")
    endforeach()
    return()
endif()

aros_add_board_autoinit(aros-board-autoinit)
file(GENERATE OUTPUT "${_opensbi_command_line}" CONTENT "\n")
file(GENERATE OUTPUT "${_opensbi_startup}" CONTENT "BOOTRISCV64.EFI\n")

add_custom_command(
    OUTPUT "${_opensbi_debug_elf}" "${_opensbi_core_elf}"
           "${_opensbi_core_map}" "${_opensbi_image}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${_opensbi_work_dir}"
    COMMAND "${AROS_LLD_BIN}"
            -Map "${_opensbi_core_map}"
            -T "${_opensbi_linker_script}"
            -o "${_opensbi_debug_elf}"
            ${_opensbi_kobjs}
            "$<TARGET_FILE:linklibs-arossupport>"
            "$<TARGET_FILE:aros-board-autoinit>"
            "$<TARGET_FILE:linklibs-libinit>"
            "$<TARGET_FILE:linklibs-stdc-static>"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${_opensbi_debug_elf}" "${_opensbi_core_elf}"
    COMMAND "${AROS_OPENSBI_STRIP}" --strip-unneeded -R .note -R .comment
            "${_opensbi_core_elf}"
    COMMAND "${AROS_OPENSBI_OBJCOPY}" -O binary
            "${_opensbi_core_elf}" "${_opensbi_image}"
    DEPENDS ${_opensbi_kobjs} linklibs-arossupport aros-board-autoinit
            linklibs-libinit linklibs-stdc-static "${_opensbi_linker_script}"
    COMMENT "Linking Milk-V Titan RISC-V UEFI image"
    VERBATIM COMMAND_EXPAND_LISTS)
add_custom_target(opensbi-uefi-core DEPENDS "${_opensbi_image}")

add_custom_command(
    OUTPUT "${_opensbi_stamp}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory
            "${_opensbi_bundle_dir}/EFI/BOOT" "${_opensbi_bundle_dir}/EFI/AROS"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${_opensbi_image}"
            "${_opensbi_bundle_dir}/EFI/BOOT/BOOTRISCV64.EFI"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${_opensbi_image}"
            "${_opensbi_bundle_dir}/EFI/AROS/Image"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${_opensbi_bsp}"
            "${_opensbi_bundle_dir}/aros-bsp.pkg"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${_opensbi_command_line}"
            "${_opensbi_bundle_dir}/aros.cmd"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${_opensbi_startup}"
            "${_opensbi_bundle_dir}/startup.nsh"
    COMMAND "${CMAKE_COMMAND}" -DBUNDLE_DIR="${_opensbi_bundle_dir}"
            -DMODEL="${AROS_OPENSBI_MODEL}" -DWRITE_MANIFEST=ON
            -P "${_opensbi_verify_script}"
    COMMAND "${CMAKE_COMMAND}" -E touch "${_opensbi_stamp}"
    DEPENDS "${_opensbi_image}" "${_opensbi_bsp}" "${_opensbi_command_line}"
            "${_opensbi_startup}" kernel-bsp-opensbi-riscv64
            "${_opensbi_verify_script}"
    COMMENT "Staging Milk-V Titan OpenSBI/UEFI payload"
    VERBATIM)
add_custom_target(opensbi-uefi-artifacts DEPENDS "${_opensbi_stamp}")
add_custom_target(opensbi-uefi-verify
    COMMAND "${CMAKE_COMMAND}" -DBUNDLE_DIR="${_opensbi_bundle_dir}"
            -DMODEL="${AROS_OPENSBI_MODEL}" -DWRITE_MANIFEST=OFF
            -P "${_opensbi_verify_script}"
    DEPENDS opensbi-uefi-artifacts VERBATIM)

message(STATUS
    "Milk-V Titan OpenSBI/UEFI payload: opensbi-uefi-artifacts -> ${_opensbi_bundle_dir}")
