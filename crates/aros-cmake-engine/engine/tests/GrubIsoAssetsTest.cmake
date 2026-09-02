cmake_minimum_required(VERSION 3.22)

get_filename_component(_repo "${CMAKE_CURRENT_LIST_DIR}/../.." ABSOLUTE)
set(_fixture "${CMAKE_CURRENT_LIST_DIR}/grub-iso-assets")

string(RANDOM LENGTH 10 ALPHABET 0123456789abcdef _suffix)
set(_root "/tmp/aros-grub-iso-assets-${_suffix}")
file(REMOVE_RECURSE "${_root}")
file(MAKE_DIRECTORY "${_root}")

function(_assets_configure name expect_success expected_message)
    set(_build "${_root}/${name}")
    execute_process(
        COMMAND "${CMAKE_COMMAND}" -S "${_fixture}" -B "${_build}" -G Ninja
            "-DAROS_REPO_ROOT=${_repo}"
            "-DGRUB_ISO_ASSETS_CASE=${name}"
        RESULT_VARIABLE _result
        OUTPUT_VARIABLE _stdout
        ERROR_VARIABLE _stderr)
    set(_log "${_stdout}${_stderr}")
    if(expect_success AND NOT _result EQUAL 0)
        message(FATAL_ERROR "grub-iso-assets ${name} configure failed (${_result})\n${_log}")
    elseif(NOT expect_success AND _result EQUAL 0)
        message(FATAL_ERROR "grub-iso-assets ${name} unexpectedly configured")
    endif()
    if(NOT "${expected_message}" STREQUAL "")
        string(FIND "${_log}" "${expected_message}" _found)
        if(_found LESS 0)
            message(FATAL_ERROR "grub-iso-assets ${name} missed '${expected_message}'\n${_log}")
        endif()
    endif()
    set(CONFIGURED_BUILD "${_build}" PARENT_SCOPE)
endfunction()

function(_append_platform_outputs manifest platform sys_root include_images output)
    file(STRINGS "${_repo}/${manifest}" _entries)
    set(_outputs "")
    foreach(_relative IN LISTS _entries)
        if(_relative MATCHES "^lib/grub/${platform}/[^/]+\\.mod$" OR
           _relative MATCHES "^lib/grub/${platform}/(command|fs|moddep)\\.lst$" OR
           (include_images AND _relative MATCHES "^lib/grub/${platform}/[^/]+\\.img$"))
            string(REGEX REPLACE "^lib/grub/${platform}/" "" _name "${_relative}")
            list(APPEND _outputs "${sys_root}/${_name}")
        endif()
    endforeach()
    set(${output} "${_outputs}" PARENT_SCOPE)
endfunction()

file(SHA256 "${_repo}/arch/all-pc/boot/grub2-host/mmakefile.src" _source_before)
file(SHA256 "${_repo}/cmake/manifests/grub-2.12-pc.install" _pc_manifest_before)
file(SHA256 "${_repo}/cmake/manifests/grub-2.12-efi64.install" _efi64_manifest_before)
file(SHA256 "${_repo}/cmake/manifests/grub-2.12-efi32.install" _efi32_manifest_before)

_assets_configure("" TRUE "")
set(_build "${CONFIGURED_BUILD}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target aros-grub2-iso-assets
    RESULT_VARIABLE _build_result
    OUTPUT_VARIABLE _build_stdout
    ERROR_VARIABLE _build_stderr)
if(NOT _build_result EQUAL 0)
    message(FATAL_ERROR "GRUB2 ISO assets build failed\n${_build_stdout}${_build_stderr}")
endif()

set(_expected "")
_append_platform_outputs("cmake/manifests/grub-2.12-pc.install" "i386-pc"
    "${_build}/SYS/boot/grub/i386-pc" TRUE _pc_outputs)
list(APPEND _expected ${_pc_outputs}
    "${_build}/SYS/boot/grub/i386-pc/core.img"
    "${_build}/SYS/boot/grub/i386-pc/grub2_eltorito")
_append_platform_outputs("cmake/manifests/grub-2.12-efi64.install" "x86_64-efi"
    "${_build}/SYS/EFI/BOOT/grub/x86_64-efi" FALSE _efi64_outputs)
list(APPEND _expected ${_efi64_outputs} "${_build}/SYS/EFI/BOOT/BOOTX64.EFI")
_append_platform_outputs("cmake/manifests/grub-2.12-efi32.install" "i386-efi"
    "${_build}/SYS/EFI/BOOT/grub/i386-efi" FALSE _efi32_outputs)
list(APPEND _expected ${_efi32_outputs}
    "${_build}/SYS/EFI/BOOT/BOOTIA32.EFI"
    "${_build}/gen/grub2-iso-assets/x86_64/grub2.mods")
list(REMOVE_DUPLICATES _expected)
list(LENGTH _expected _expected_count)
if(NOT _expected_count EQUAL 832)
    message(FATAL_ERROR "test expected ${_expected_count} staged products, expected 832")
endif()
foreach(_product IN LISTS _expected)
    if(NOT EXISTS "${_product}" OR IS_DIRECTORY "${_product}" OR IS_SYMLINK "${_product}")
        message(FATAL_ERROR "GRUB2 ISO assets omitted ${_product}")
    endif()
endforeach()

file(SIZE "${_build}/SYS/boot/grub/i386-pc/cdboot.img" _cdboot_size)
file(SIZE "${_build}/SYS/boot/grub/i386-pc/core.img" _core_size)
file(SIZE "${_build}/SYS/boot/grub/i386-pc/grub2_eltorito" _eltorito_size)
math(EXPR _expected_eltorito_size "${_cdboot_size} + ${_core_size}")
if(NOT _eltorito_size EQUAL _expected_eltorito_size)
    message(FATAL_ERROR "GRUB2 El Torito output is not the expected concatenation")
endif()
file(READ "${_build}/gen/grub2-iso-assets/x86_64/grub2.mods" _mods)
if(NOT _mods STREQUAL "fshelp part_msdos part_amiga part_gpt fat ntfs ntfscomp affs sfs ext2 hfsplus iso9660 minicmd xzio\n")
    message(FATAL_ERROR "GRUB2 module list differs from the audited core set")
endif()

execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target aros-grub2-iso-assets
    RESULT_VARIABLE _noop_result
    OUTPUT_VARIABLE _noop_stdout
    ERROR_VARIABLE _noop_stderr)
set(_noop_log "${_noop_stdout}${_noop_stderr}")
if(NOT _noop_result EQUAL 0 OR NOT _noop_log MATCHES "no work to do")
    message(FATAL_ERROR "GRUB2 ISO assets no-op check failed\n${_noop_log}")
endif()

_assets_configure("" TRUE "")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${CONFIGURED_BUILD}" --target aros-grub2-iso-assets
    RESULT_VARIABLE _reconfigure_result
    OUTPUT_VARIABLE _reconfigure_stdout
    ERROR_VARIABLE _reconfigure_stderr)
set(_reconfigure_log "${_reconfigure_stdout}${_reconfigure_stderr}")
if(NOT _reconfigure_result EQUAL 0 OR NOT _reconfigure_log MATCHES "no work to do")
    message(FATAL_ERROR "GRUB2 ISO assets rebuilt after a no-op reconfigure\n${_reconfigure_log}")
endif()

set(_repair "${_build}/SYS/EFI/BOOT/grub/i386-efi/fat.mod")
set(_unrelated_sys_product "${_build}/SYS/unrelated-preserved.txt")
file(WRITE "${_unrelated_sys_product}" "must survive GRUB2 asset repair\n")
file(REMOVE "${_repair}")
execute_process(
    COMMAND "${CMAKE_COMMAND}" --build "${_build}" --target aros-grub2-iso-assets
    RESULT_VARIABLE _repair_result
    OUTPUT_VARIABLE _repair_stdout
    ERROR_VARIABLE _repair_stderr)
if(NOT _repair_result EQUAL 0 OR NOT EXISTS "${_repair}" OR
   NOT EXISTS "${_unrelated_sys_product}")
    message(FATAL_ERROR "GRUB2 ISO asset repair check failed\n${_repair_stdout}${_repair_stderr}")
endif()

_assets_configure("symlink-contract" FALSE
    "contract path contains a symlinked path component")

file(SHA256 "${_repo}/arch/all-pc/boot/grub2-host/mmakefile.src" _source_after)
file(SHA256 "${_repo}/cmake/manifests/grub-2.12-pc.install" _pc_manifest_after)
file(SHA256 "${_repo}/cmake/manifests/grub-2.12-efi64.install" _efi64_manifest_after)
file(SHA256 "${_repo}/cmake/manifests/grub-2.12-efi32.install" _efi32_manifest_after)
if(NOT _source_before STREQUAL _source_after OR
   NOT _pc_manifest_before STREQUAL _pc_manifest_after OR
   NOT _efi64_manifest_before STREQUAL _efi64_manifest_after OR
   NOT _efi32_manifest_before STREQUAL _efi32_manifest_after)
    message(FATAL_ERROR "GRUB2 ISO asset staging modified its source tree")
endif()

_assets_configure("symlink-binary" FALSE "contains a symlinked path component")

file(REMOVE_RECURSE "${_root}")
message(STATUS "GRUB2 ISO asset staging test passed")
