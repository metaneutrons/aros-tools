include_guard(GLOBAL)

include(CMakeParseArguments)
include("${CMAKE_CURRENT_LIST_DIR}/GrubSourceLock.cmake")

set(_AROS_GRUB_BUILD_MODULE_DIR "${CMAKE_CURRENT_LIST_DIR}")

# The current GRUB 2.12 host-tool contract is audited specifically for a
# native Apple-Silicon Homebrew host.  Keep this capability separate from the
# x86_64-pc *target* profile: a Linux host can build that target's ordinary
# tree, but must not be offered an unaudited GRUB/ISO lane.
set(AROS_GRUB2_HOST_LANES_AVAILABLE FALSE)
if(CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin" AND
   CMAKE_HOST_SYSTEM_PROCESSOR MATCHES "^(arm64|aarch64)$")
    set(AROS_GRUB2_HOST_LANES_AVAILABLE TRUE)
endif()

# Closed GRUB 2.12 host-tool builder.  The legacy declarations all consume the
# same patched upstream source, but their PC, EFI64 and EFI32 build trees must
# never share an install prefix: GRUB installs host programs into --bindir.
set(_AROS_GRUB2_PATCH_RELATIVE
    "arch/all-pc/boot/grub2-aros/grub-2.12-aros.diff")

# Resolve pre-existing path components physically, then append a non-existing
# tail.  This deliberately accepts macOS' /tmp -> /private/tmp alias while
# making a real symlink escape visible before an output tree is removed.
function(_aros_grub_real_path path output)
    set(_candidate "${path}")
    cmake_path(ABSOLUTE_PATH _candidate NORMALIZE OUTPUT_VARIABLE _candidate)
    set(_tail "")
    while(NOT EXISTS "${_candidate}" AND NOT IS_SYMLINK "${_candidate}")
        cmake_path(GET _candidate FILENAME _component)
        cmake_path(GET _candidate PARENT_PATH _parent)
        if(_component STREQUAL "" OR _parent STREQUAL _candidate)
            message(FATAL_ERROR "cannot resolve physical path ${path}")
        endif()
        list(PREPEND _tail "${_component}")
        set(_candidate "${_parent}")
    endwhile()
    if(IS_SYMLINK "${_candidate}" AND NOT EXISTS "${_candidate}")
        message(FATAL_ERROR "refusing dangling symlink in path ${path}")
    endif()
    file(REAL_PATH "${_candidate}" _resolved)
    foreach(_component IN LISTS _tail)
        set(_resolved "${_resolved}/${_component}")
    endforeach()
    cmake_path(NORMAL_PATH _resolved)
    set(${output} "${_resolved}" PARENT_SCOPE)
endfunction()

function(_aros_grub_safe_value label value)
    foreach(_needle IN ITEMS ";" "\"" "\n" "\r" "$" "[" "]")
        string(FIND "${value}" "${_needle}" _position)
        if(NOT _position EQUAL -1)
            message(FATAL_ERROR "${label} contains an unsafe value")
        endif()
    endforeach()
endfunction()

function(_aros_grub_require_executable path label)
    if(NOT EXISTS "${path}" OR IS_DIRECTORY "${path}")
        message(FATAL_ERROR "${label} is unavailable")
    endif()
    execute_process(
        COMMAND /bin/test -x "${path}"
        RESULT_VARIABLE _executable_result)
    if(NOT _executable_result EQUAL 0)
        message(FATAL_ERROR "${label} is not executable")
    endif()
endfunction()

# A canonical path is safe to use only if every existing component beneath its
# canonical owner is a directory rather than a symlink.  The owner itself is
# already a real path, so this does not reject the harmless /tmp spelling.
function(_aros_grub_reject_symlink_components root path label)
    cmake_path(IS_PREFIX root "${path}" NORMALIZE _owned)
    if(NOT _owned)
        message(FATAL_ERROR "${label} escapes its owner")
    endif()
    file(RELATIVE_PATH _relative "${root}" "${path}")
    if(_relative STREQUAL "")
        return()
    endif()
    string(REPLACE "/" ";" _components "${_relative}")
    set(_cursor "${root}")
    foreach(_component IN LISTS _components)
        set(_cursor "${_cursor}/${_component}")
        if(IS_SYMLINK "${_cursor}")
            message(FATAL_ERROR "${label} contains a symlinked path component")
        endif()
    endforeach()
endfunction()

function(_aros_grub_write_if_changed path content)
    if(EXISTS "${path}")
        file(READ "${path}" _previous)
    else()
        set(_previous "")
    endif()
    if(NOT "${_previous}" STREQUAL "${content}")
        file(WRITE "${path}" "${content}")
    endif()
endfunction()

function(_aros_grub_product_paths mode binary_dir install_prefix install_manifest output)
    if(mode STREQUAL "pc")
        set(_platform_dir "i386-pc")
        set(_private_relative
            "build/grub-mkimage"
            "build/grub-core/boot.img"
            "build/grub-core/cdboot.img"
            "build/grub-core/diskboot.img"
            "build/grub-core/kernel.img"
            "build/grub-core/normal.mod"
            "build/grub-core/biosdisk.mod"
            "build/grub-core/affs.mod"
            "build/grub-core/sfs.mod"
            "build/grub-core/xzio.mod"
            "build/grub-core/command.lst"
            "build/grub-core/fs.lst"
            "build/grub-core/moddep.lst")
    elseif(mode STREQUAL "efi64")
        set(_platform_dir "x86_64-efi")
        set(_private_relative
            "build/grub-mkimage"
            "build/grub-core/kernel.img"
            "build/grub-core/normal.mod"
            "build/grub-core/efi_gop.mod"
            "build/grub-core/affs.mod"
            "build/grub-core/sfs.mod"
            "build/grub-core/xzio.mod"
            "build/grub-core/command.lst"
            "build/grub-core/fs.lst"
            "build/grub-core/moddep.lst")
    elseif(mode STREQUAL "efi32")
        set(_platform_dir "i386-efi")
        set(_private_relative
            "build/grub-mkimage"
            "build/grub-core/kernel.img"
            "build/grub-core/normal.mod"
            "build/grub-core/efi_gop.mod"
            "build/grub-core/affs.mod"
            "build/grub-core/sfs.mod"
            "build/grub-core/xzio.mod"
            "build/grub-core/command.lst"
            "build/grub-core/fs.lst"
            "build/grub-core/moddep.lst")
    else()
        message(FATAL_ERROR "unsupported GRUB2 mode ${mode}")
    endif()
    set(_products "")
    foreach(_relative IN LISTS _private_relative)
        list(APPEND _products "${binary_dir}/${_relative}")
    endforeach()
    foreach(_relative IN LISTS install_manifest)
        list(APPEND _products "${install_prefix}/${_relative}")
    endforeach()
    set(${output} "${_products}" PARENT_SCOPE)
endfunction()

# aros_build_grub2(
#     MMAKE_ID <grub2-host|grub2-efi-host|grub2-efi32-host>
#     MODE <pc|efi64|efi32>
#     BINARY_DIR <private lane root>
#     INSTALL_PREFIX <private, lane-specific host-tool prefix>)
#
# The arguments are intentionally narrow.  The future transpiler integration
# may select a lane, but cannot substitute a different source, patch, target
# triple, install tree or product manifest.
function(aros_build_grub2)
    set(one_value_args MMAKE_ID MODE BINARY_DIR INSTALL_PREFIX)
    cmake_parse_arguments(GB "" "${one_value_args}" "" ${ARGN})
    if(GB_UNPARSED_ARGUMENTS OR GB_KEYWORDS_MISSING_VALUES)
        message(FATAL_ERROR "aros_build_grub2 received malformed arguments")
    endif()
    foreach(_required IN ITEMS MMAKE_ID MODE BINARY_DIR INSTALL_PREFIX)
        if(NOT GB_${_required})
            message(FATAL_ERROR "aros_build_grub2 requires ${_required}")
        endif()
    endforeach()
    if(NOT GB_MMAKE_ID MATCHES "^[A-Za-z0-9_.+-]+$")
        message(FATAL_ERROR "${GB_MMAKE_ID}: invalid GRUB2 target name")
    endif()
    if(TARGET "${GB_MMAKE_ID}")
        message(FATAL_ERROR "${GB_MMAKE_ID}: GRUB2 target was already declared")
    endif()

    if(GB_MODE STREQUAL "pc")
        set(_expected_id "grub2-host")
        set(_lane "pc")
        set(_configure_target "x86_64")
        set(_platform "pc")
        set(_target_triple "i386-pc-linux-gnu")
        set(_target_isa_flags "--target=i386-pc-linux-gnu -march=i486 -m32")
        set(_link_format "-melf_i386")
        set(_expected_file_count 615)
        set(_install_manifest_relative "cmake/manifests/grub-2.12-pc.install")
        set(_platform_dir "i386-pc")
    elseif(GB_MODE STREQUAL "efi64")
        set(_expected_id "grub2-efi-host")
        set(_lane "efi-x86_64")
        set(_configure_target "x86_64")
        set(_platform "efi")
        set(_target_triple "x86_64-pc-linux-gnu")
        set(_target_isa_flags "--target=x86_64-pc-linux-gnu")
        set(_link_format "-melf_x86_64")
        set(_expected_file_count 591)
        set(_install_manifest_relative "cmake/manifests/grub-2.12-efi64.install")
        set(_platform_dir "x86_64-efi")
    elseif(GB_MODE STREQUAL "efi32")
        set(_expected_id "grub2-efi32-host")
        set(_lane "efi-i386")
        set(_configure_target "i386")
        set(_platform "efi")
        set(_target_triple "i386-pc-linux-gnu")
        set(_target_isa_flags "--target=i386-pc-linux-gnu -march=i486 -m32")
        set(_link_format "-melf_i386")
        set(_expected_file_count 593)
        set(_install_manifest_relative "cmake/manifests/grub-2.12-efi32.install")
        set(_platform_dir "i386-efi")
    else()
        message(FATAL_ERROR "${GB_MMAKE_ID}: unsupported GRUB2 mode ${GB_MODE}")
    endif()
    if(NOT GB_MMAKE_ID STREQUAL _expected_id)
        message(FATAL_ERROR
            "${GB_MMAKE_ID}: target identity differs from the audited ${GB_MODE} capability")
    endif()

    if(NOT CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin" OR
       NOT CMAKE_HOST_SYSTEM_PROCESSOR MATCHES "^(arm64|aarch64)$")
        message(FATAL_ERROR
            "${GB_MMAKE_ID}: audited GRUB2 host lanes require an arm64 Darwin host")
    endif()

    foreach(_name IN ITEMS BINARY_DIR INSTALL_PREFIX)
        _aros_grub_safe_value("${GB_MMAKE_ID}: ${_name}" "${GB_${_name}}")
    endforeach()

    if(DEFINED AROS_GRUB_SOURCE_ROOT)
        set(_source_root_raw "${AROS_GRUB_SOURCE_ROOT}")
    else()
        set(_source_root_raw "${CMAKE_SOURCE_DIR}")
    endif()
    _aros_grub_safe_value("${GB_MMAKE_ID}: source root" "${_source_root_raw}")
    cmake_path(ABSOLUTE_PATH _source_root_raw NORMALIZE
        OUTPUT_VARIABLE _source_root_logical)
    cmake_path(ABSOLUTE_PATH CMAKE_BINARY_DIR NORMALIZE
        OUTPUT_VARIABLE _build_root_logical)
    if(NOT EXISTS "${_source_root_logical}" OR
       NOT IS_DIRECTORY "${_source_root_logical}")
        message(FATAL_ERROR "${GB_MMAKE_ID}: GRUB2 source root is unavailable")
    endif()
    _aros_grub_real_path("${_source_root_logical}" _source_root)
    _aros_grub_real_path("${_build_root_logical}" _build_root)
    if(IS_SYMLINK "${_source_root}" OR IS_SYMLINK "${_build_root}")
        message(FATAL_ERROR "${GB_MMAKE_ID}: source or build root may not be a symlink")
    endif()

    set(_patch_logical "${_source_root_logical}/${_AROS_GRUB2_PATCH_RELATIVE}")
    if(NOT EXISTS "${_patch_logical}" OR IS_DIRECTORY "${_patch_logical}" OR
       IS_SYMLINK "${_patch_logical}")
        message(FATAL_ERROR "${GB_MMAKE_ID}: audited GRUB2 patch is unavailable")
    endif()
    _aros_grub_real_path("${_patch_logical}" _patch)
    set(_expected_patch "${_source_root}/${_AROS_GRUB2_PATCH_RELATIVE}")
    _aros_grub_real_path("${_expected_patch}" _expected_patch)
    cmake_path(IS_PREFIX _source_root "${_patch}" NORMALIZE _patch_owned)
    if(NOT _patch_owned OR NOT _patch STREQUAL _expected_patch)
        message(FATAL_ERROR "${GB_MMAKE_ID}: GRUB2 patch escaped the source tree")
    endif()
    file(SHA256 "${_patch}" _actual_patch_sha256)

    set(_install_manifest_logical
        "${_source_root_logical}/${_install_manifest_relative}")
    if(NOT EXISTS "${_install_manifest_logical}" OR
       IS_DIRECTORY "${_install_manifest_logical}" OR
       IS_SYMLINK "${_install_manifest_logical}")
        message(FATAL_ERROR
            "${GB_MMAKE_ID}: audited GRUB2 install manifest is unavailable")
    endif()
    _aros_grub_real_path("${_install_manifest_logical}" _install_manifest)
    set(_expected_install_manifest
        "${_source_root}/${_install_manifest_relative}")
    _aros_grub_real_path("${_expected_install_manifest}" _expected_install_manifest)
    cmake_path(IS_PREFIX _source_root "${_install_manifest}" NORMALIZE
        _install_manifest_owned)
    file(SHA256 "${_install_manifest}" _actual_install_manifest_sha256)
    if(NOT _install_manifest_owned OR
       NOT _install_manifest STREQUAL _expected_install_manifest)
        message(FATAL_ERROR
            "${GB_MMAKE_ID}: GRUB2 install manifest differs from the audited product set")
    endif()
    file(STRINGS "${_install_manifest}" _install_relative)
    list(LENGTH _install_relative _install_manifest_count)
    set(_sorted_install_relative "${_install_relative}")
    list(SORT _sorted_install_relative)
    if(NOT _install_manifest_count EQUAL _expected_file_count OR
       NOT "${_install_relative}" STREQUAL "${_sorted_install_relative}")
        message(FATAL_ERROR
            "${GB_MMAKE_ID}: GRUB2 install manifest count or ordering is invalid")
    endif()
    foreach(_relative IN LISTS _install_relative)
        _aros_grub_safe_value("${GB_MMAKE_ID}: install-manifest product" "${_relative}")
        string(FIND "${_relative}" "\\" _backslash)
        if(_relative STREQUAL "" OR _relative MATCHES "^/" OR
           _relative MATCHES "(^|/)\\.\\.(/|$)" OR NOT _backslash EQUAL -1)
            message(FATAL_ERROR
                "${GB_MMAKE_ID}: GRUB2 install manifest contains an unsafe path")
        endif()
    endforeach()

    set(_binary_input "${GB_BINARY_DIR}")
    cmake_path(ABSOLUTE_PATH _binary_input BASE_DIRECTORY "${_build_root_logical}"
        NORMALIZE OUTPUT_VARIABLE _binary_logical)
    set(_prefix_input "${GB_INSTALL_PREFIX}")
    cmake_path(ABSOLUTE_PATH _prefix_input BASE_DIRECTORY "${_build_root_logical}"
        NORMALIZE OUTPUT_VARIABLE _prefix_logical)
    _aros_grub_real_path("${_binary_logical}" _binary_dir)
    _aros_grub_real_path("${_prefix_logical}" _install_prefix)
    set(_configure_root "${_build_root}/gen/configure")
    _aros_grub_real_path("${_configure_root}" _configure_root)
    cmake_path(IS_PREFIX _build_root "${_configure_root}" NORMALIZE _configure_owned)
    cmake_path(IS_PREFIX _configure_root "${_binary_dir}" NORMALIZE _binary_owned)
    cmake_path(IS_PREFIX _build_root "${_install_prefix}" NORMALIZE _prefix_owned)
    if(NOT _configure_owned OR _configure_root STREQUAL _build_root OR
       NOT _binary_owned OR _binary_dir STREQUAL _configure_root OR
       NOT _prefix_owned OR _install_prefix STREQUAL _build_root)
        message(FATAL_ERROR "${GB_MMAKE_ID}: GRUB2 build root escapes the build tree")
    endif()
    _aros_grub_reject_symlink_components("${_build_root}" "${_configure_root}"
        "${GB_MMAKE_ID}: configure root")
    _aros_grub_reject_symlink_components("${_configure_root}" "${_binary_dir}"
        "${GB_MMAKE_ID}: binary directory")
    _aros_grub_reject_symlink_components("${_build_root}" "${_install_prefix}"
        "${GB_MMAKE_ID}: install prefix")
    cmake_path(IS_PREFIX _binary_dir "${_install_prefix}" NORMALIZE _binary_contains_prefix)
    cmake_path(IS_PREFIX _install_prefix "${_binary_dir}" NORMALIZE _prefix_contains_binary)
    if(_binary_contains_prefix OR _prefix_contains_binary)
        message(FATAL_ERROR "${GB_MMAKE_ID}: binary directory overlaps install prefix")
    endif()
    set(_expected_binary "${_build_root}/gen/configure/arch/all-pc/boot/grub2-host/${_lane}")
    set(_expected_prefix "${_build_root}/hosttools/grub2/${_lane}")
    _aros_grub_real_path("${_expected_binary}" _expected_binary)
    _aros_grub_real_path("${_expected_prefix}" _expected_prefix)
    if(NOT _binary_dir STREQUAL _expected_binary)
        message(FATAL_ERROR
            "${GB_MMAKE_ID}: binary identity differs from the audited ${GB_MODE} capability")
    endif()
    if(NOT _install_prefix STREQUAL _expected_prefix)
        message(FATAL_ERROR
            "${GB_MMAKE_ID}: install-prefix identity differs from the audited ${GB_MODE} capability")
    endif()

    set(_archive_logical "${_build_root_logical}/downloads/grub-${_AROS_GRUB2_VERSION}.tar.xz")
    set(_archive "${_build_root}/downloads/grub-${_AROS_GRUB2_VERSION}.tar.xz")
    _aros_grub_real_path("${_archive}" _archive)
    cmake_path(IS_PREFIX _build_root "${_archive}" NORMALIZE _archive_owned)
    if(NOT _archive_owned OR _archive STREQUAL _build_root)
        message(FATAL_ERROR "${GB_MMAKE_ID}: GRUB2 archive escapes the build tree")
    endif()
    _aros_grub_reject_symlink_components("${_build_root}" "${_archive}"
        "${GB_MMAKE_ID}: archive path")

    if(DEFINED AROS_GRUB2_XZ_PREFIX)
        set(_xz_prefix_logical "${AROS_GRUB2_XZ_PREFIX}")
    else()
        set(_xz_prefix_logical "/opt/homebrew/opt/xz")
    endif()
    _aros_grub_safe_value("${GB_MMAKE_ID}: xz prefix" "${_xz_prefix_logical}")
    if(NOT EXISTS "${_xz_prefix_logical}" OR NOT IS_DIRECTORY "${_xz_prefix_logical}")
        message(FATAL_ERROR "${GB_MMAKE_ID}: audited Homebrew xz dependency is unavailable")
    endif()
    _aros_grub_real_path("${_xz_prefix_logical}" _xz_prefix)
    _aros_grub_real_path("/opt/homebrew/opt/xz" _expected_xz_prefix)
    if(NOT _xz_prefix STREQUAL _expected_xz_prefix OR
       NOT EXISTS "${_xz_prefix}/include/lzma.h" OR
       NOT EXISTS "${_xz_prefix}/lib/liblzma.dylib" OR
       NOT EXISTS "${_xz_prefix}/lib/pkgconfig/liblzma.pc")
        message(FATAL_ERROR
            "${GB_MMAKE_ID}: audited Homebrew xz dependency is incomplete or substituted")
    endif()

    if(DEFINED AROS_GRUB2_LLVM_PREFIX)
        set(_llvm_prefix_logical "${AROS_GRUB2_LLVM_PREFIX}")
    else()
        set(_llvm_prefix_logical "/opt/homebrew/opt/llvm")
    endif()
    _aros_grub_safe_value("${GB_MMAKE_ID}: llvm prefix" "${_llvm_prefix_logical}")
    if(NOT EXISTS "${_llvm_prefix_logical}" OR NOT IS_DIRECTORY "${_llvm_prefix_logical}")
        message(FATAL_ERROR "${GB_MMAKE_ID}: audited Homebrew LLVM dependency is unavailable")
    endif()
    _aros_grub_real_path("${_llvm_prefix_logical}" _llvm_prefix)
    _aros_grub_real_path("/opt/homebrew/opt/llvm" _expected_llvm_prefix)
    if(NOT _llvm_prefix STREQUAL _expected_llvm_prefix)
        message(FATAL_ERROR "${GB_MMAKE_ID}: audited Homebrew LLVM dependency was substituted")
    endif()
    set(_host_cc "/usr/bin/cc")
    set(_host_cxx "/usr/bin/c++")
    set(_patch_tool "/usr/bin/patch")
    set(_make_tool "/usr/bin/make")
    set(_file_tool "/usr/bin/file")
    set(_otool_tool "/usr/bin/otool")
    set(_install_tool "/opt/homebrew/opt/coreutils/bin/ginstall")
    set(_mkdir_tool "/opt/homebrew/opt/coreutils/bin/gmkdir")
    set(_awk_tool "/opt/homebrew/opt/gawk/bin/gawk")
    set(_pkg_config_tool "/opt/homebrew/opt/pkgconf/bin/pkg-config")
    set(_yacc_tool "/usr/bin/bison")
    set(_lex_tool "/usr/bin/flex")
    set(_makeinfo_tool "/opt/homebrew/opt/texinfo/bin/makeinfo")
    set(_python_tool "/opt/homebrew/opt/python@3.14/bin/python3")
    set(_sed_tool "/opt/homebrew/opt/gnu-sed/bin/gsed")
    set(_msgfmt_tool "/opt/homebrew/opt/gettext/bin/msgfmt")
    set(_msgmerge_tool "/opt/homebrew/opt/gettext/bin/msgmerge")
    set(_xgettext_tool "/opt/homebrew/opt/gettext/bin/xgettext")
    set(_ar_tool "/usr/bin/ar")
    set(_ranlib_tool "/usr/bin/ranlib")
    set(_nm_tool "/usr/bin/nm")
    set(_cmp_tool "/usr/bin/cmp")
    set(_grep_tool "/usr/bin/grep")
    set(_target_clang "/opt/homebrew/opt/llvm/bin/clang")
    set(_target_ld "/opt/homebrew/opt/lld/bin/ld.lld")
    set(_target_objcopy "/opt/homebrew/opt/llvm/bin/llvm-objcopy")
    set(_target_ranlib "/opt/homebrew/opt/llvm/bin/llvm-ranlib")
    set(_target_nm "/opt/homebrew/opt/llvm/bin/llvm-nm")
    set(_target_strip "/opt/homebrew/opt/llvm/bin/llvm-strip")
    set(_host_tools
        "${_host_cc}" "${_host_cxx}" "${_patch_tool}" "${_make_tool}"
        "${_file_tool}" "${_otool_tool}" "${_install_tool}" "${_mkdir_tool}"
        "${_awk_tool}" "${_pkg_config_tool}" "${_yacc_tool}" "${_lex_tool}"
        "${_makeinfo_tool}" "${_python_tool}" "${_sed_tool}" "${_msgfmt_tool}"
        "${_msgmerge_tool}" "${_xgettext_tool}" "${_ar_tool}" "${_ranlib_tool}"
        "${_nm_tool}" "${_cmp_tool}" "${_grep_tool}" "${_target_clang}"
        "${_target_ld}" "${_target_objcopy}" "${_target_ranlib}" "${_target_nm}"
        "${_target_strip}")
    foreach(_tool IN LISTS _host_tools)
        _aros_grub_require_executable("${_tool}"
            "${GB_MMAKE_ID}: audited host tool ${_tool}")
    endforeach()
    set(_host_path_entries
        "/opt/homebrew/opt/gettext/bin"
        "/opt/homebrew/opt/texinfo/bin"
        "/opt/homebrew/opt/gawk/bin"
        "/opt/homebrew/opt/pkgconf/bin"
        "/opt/homebrew/opt/python@3.14/bin"
        "/opt/homebrew/opt/gnu-sed/bin"
        "/opt/homebrew/opt/coreutils/bin"
        "/opt/homebrew/opt/llvm/bin"
        "/opt/homebrew/opt/lld/bin"
        "/usr/bin"
        "/bin"
        "/usr/sbin"
        "/sbin")
    foreach(_path_entry IN LISTS _host_path_entries)
        if(NOT EXISTS "${_path_entry}" OR NOT IS_DIRECTORY "${_path_entry}")
            message(FATAL_ERROR
                "${GB_MMAKE_ID}: audited host PATH entry ${_path_entry} is unavailable")
        endif()
    endforeach()
    list(JOIN _host_path_entries ":" _host_path)

    _aros_grub_product_paths("${GB_MODE}" "${_binary_logical}" "${_prefix_logical}"
        "${_install_relative}" _products_logical)
    _aros_grub_product_paths("${GB_MODE}" "${_binary_dir}" "${_install_prefix}"
        "${_install_relative}" _products_physical)
    set(_private_products_physical "")
    set(_install_products_physical "")
    foreach(_product IN LISTS _products_physical)
        cmake_path(IS_PREFIX _binary_dir "${_product}" NORMALIZE _is_private)
        cmake_path(IS_PREFIX _install_prefix "${_product}" NORMALIZE _is_install)
        if(_is_private)
            list(APPEND _private_products_physical "${_product}")
        elseif(_is_install)
            list(APPEND _install_products_physical "${_product}")
        else()
            message(FATAL_ERROR "${GB_MMAKE_ID}: product escaped its private owner")
        endif()
    endforeach()
    set(_stamp_logical "${_binary_logical}/.grub2-${_lane}.stamp")
    set(_stamp "${_binary_dir}/.grub2-${_lane}.stamp")
    list(APPEND _products_logical "${_stamp_logical}")

    get_property(_registered_outputs GLOBAL PROPERTY AROS_GRUB2_REGISTERED_OUTPUTS)
    foreach(_product IN LISTS _products_physical _stamp)
        if(_product IN_LIST _registered_outputs)
            message(FATAL_ERROR "${GB_MMAKE_ID}: GRUB2 product is already owned: ${_product}")
        endif()
        list(APPEND _registered_outputs "${_product}")
    endforeach()
    set_property(GLOBAL PROPERTY AROS_GRUB2_REGISTERED_OUTPUTS "${_registered_outputs}")

    # Fetch is shared between lanes, but its contract still fixes the one
    # permitted source URL, archive SHA and output location.  Preserve the
    # logical CMake spelling for Ninja, and use the physical spelling only in
    # the runner's containment checks.
    get_property(_fetch_declared GLOBAL PROPERTY AROS_GRUB2_FETCH_DECLARED)
    if(NOT _fetch_declared)
        if(TARGET grub2-aros--fetch OR TARGET grub2-aros-fetch)
            message(FATAL_ERROR "GRUB2 fetch target name is already owned")
        endif()
        set(_fetch_contract_logical "${_build_root_logical}/.aros-grub2-fetch-contract.cmake")
        set(_fetch_contract "${_build_root}/.aros-grub2-fetch-contract.cmake")
        string(CONCAT _fetch_content
            "# Generated closed GRUB2 fetch contract.  Do not edit.\n"
            "set(GB_BUILD_ROOT [==[${_build_root}]==])\n"
            "set(GB_ARCHIVE [==[${_archive}]==])\n"
            "set(GB_SOURCE_URL [==[${_AROS_GRUB2_SOURCE_URL}]==])\n"
            "set(GB_ARCHIVE_SHA256 [==[${_AROS_GRUB2_ARCHIVE_SHA256}]==])\n")
        _aros_grub_write_if_changed("${_fetch_contract}" "${_fetch_content}")
        add_custom_command(
            OUTPUT "${_archive_logical}"
            COMMAND "${CMAKE_COMMAND}" -DGB_ACTION=fetch
                "-DCONTRACT=${_fetch_contract_logical}"
                -P "${_AROS_GRUB_BUILD_MODULE_DIR}/RunGrubBuild.cmake"
            DEPENDS "${_fetch_contract_logical}"
                "${_AROS_GRUB_BUILD_MODULE_DIR}/RunGrubBuild.cmake"
            COMMENT "Fetching audited GRUB 2.12 source"
            VERBATIM)
        add_custom_target(grub2-aros--fetch DEPENDS "${_archive_logical}")
        add_custom_target(grub2-aros-fetch DEPENDS grub2-aros--fetch)
        set_property(GLOBAL PROPERTY AROS_GRUB2_FETCH_DECLARED TRUE)
        set_property(GLOBAL PROPERTY AROS_GRUB2_FETCH_BUILD_ROOT "${_build_root}")
        set_property(GLOBAL PROPERTY AROS_GRUB2_FETCH_ARCHIVE "${_archive}")
        set_property(GLOBAL PROPERTY AROS_GRUB2_FETCH_SOURCE_ROOT "${_source_root}")
    else()
        get_property(_fetch_build_root GLOBAL PROPERTY AROS_GRUB2_FETCH_BUILD_ROOT)
        get_property(_fetch_archive GLOBAL PROPERTY AROS_GRUB2_FETCH_ARCHIVE)
        get_property(_fetch_source_root GLOBAL PROPERTY AROS_GRUB2_FETCH_SOURCE_ROOT)
        if(NOT _fetch_build_root STREQUAL _build_root OR
           NOT _fetch_archive STREQUAL _archive OR
           NOT _fetch_source_root STREQUAL _source_root)
            message(FATAL_ERROR "${GB_MMAKE_ID}: GRUB2 lanes disagree about their shared source")
        endif()
    endif()

    set(_contract_logical "${_build_root_logical}/.aros-${GB_MMAKE_ID}-grub2-contract.cmake")
    set(_contract "${_build_root}/.aros-${GB_MMAKE_ID}-grub2-contract.cmake")
    set(_contract_content "# Generated closed GRUB2 build contract.  Do not edit.\n")
    macro(_aros_grub_contract_set name value)
        _aros_grub_safe_value("${GB_MMAKE_ID}: contract ${name}" "${value}")
        string(APPEND _contract_content "set(${name} [==[${value}]==])\n")
    endmacro()
    _aros_grub_contract_set(GB_MODE "${GB_MODE}")
    _aros_grub_contract_set(GB_MMAKE_ID "${GB_MMAKE_ID}")
    _aros_grub_contract_set(GB_SOURCE_ROOT "${_source_root}")
    _aros_grub_contract_set(GB_BUILD_ROOT "${_build_root}")
    _aros_grub_contract_set(GB_BINARY_DIR "${_binary_dir}")
    _aros_grub_contract_set(GB_INSTALL_PREFIX "${_install_prefix}")
    _aros_grub_contract_set(GB_ARCHIVE "${_archive}")
    _aros_grub_contract_set(GB_PATCH "${_patch}")
    _aros_grub_contract_set(GB_INSTALL_MANIFEST "${_install_manifest}")
    _aros_grub_contract_set(GB_SOURCE_URL "${_AROS_GRUB2_SOURCE_URL}")
    _aros_grub_contract_set(GB_ARCHIVE_SHA256 "${_AROS_GRUB2_ARCHIVE_SHA256}")
    _aros_grub_contract_set(GB_PATCH_SHA256 "${_actual_patch_sha256}")
    # Keep the audited /opt/homebrew/opt spellings in the environment and
    # tool commands.  Their physical resolutions are used above only for
    # containment/identity validation, just as CMake's logical OUTPUT paths
    # remain the names Ninja owns.
    _aros_grub_contract_set(GB_XZ_PREFIX "/opt/homebrew/opt/xz")
    _aros_grub_contract_set(GB_HOST_PATH "${_host_path}")
    _aros_grub_contract_set(GB_HOST_CC "${_host_cc}")
    _aros_grub_contract_set(GB_HOST_CXX "${_host_cxx}")
    _aros_grub_contract_set(GB_PATCH_TOOL "${_patch_tool}")
    _aros_grub_contract_set(GB_MAKE "${_make_tool}")
    _aros_grub_contract_set(GB_FILE "${_file_tool}")
    _aros_grub_contract_set(GB_OTOOL "${_otool_tool}")
    _aros_grub_contract_set(GB_INSTALL_TOOL "${_install_tool}")
    _aros_grub_contract_set(GB_MKDIR_TOOL "${_mkdir_tool}")
    _aros_grub_contract_set(GB_AWK "${_awk_tool}")
    _aros_grub_contract_set(GB_PKG_CONFIG "${_pkg_config_tool}")
    _aros_grub_contract_set(GB_YACC "${_yacc_tool}")
    _aros_grub_contract_set(GB_LEX "${_lex_tool}")
    _aros_grub_contract_set(GB_MAKEINFO "${_makeinfo_tool}")
    _aros_grub_contract_set(GB_PYTHON "${_python_tool}")
    _aros_grub_contract_set(GB_SED "${_sed_tool}")
    _aros_grub_contract_set(GB_MSGFMT "${_msgfmt_tool}")
    _aros_grub_contract_set(GB_MSGMERGE "${_msgmerge_tool}")
    _aros_grub_contract_set(GB_XGETTEXT "${_xgettext_tool}")
    _aros_grub_contract_set(GB_AR "${_ar_tool}")
    _aros_grub_contract_set(GB_RANLIB "${_ranlib_tool}")
    _aros_grub_contract_set(GB_NM "${_nm_tool}")
    _aros_grub_contract_set(GB_CMP "${_cmp_tool}")
    _aros_grub_contract_set(GB_GREP "${_grep_tool}")
    _aros_grub_contract_set(GB_TARGET_CLANG "${_target_clang}")
    _aros_grub_contract_set(GB_TARGET_LD "${_target_ld}")
    _aros_grub_contract_set(GB_TARGET_OBJCOPY "${_target_objcopy}")
    _aros_grub_contract_set(GB_TARGET_RANLIB "${_target_ranlib}")
    _aros_grub_contract_set(GB_TARGET_NM "${_target_nm}")
    _aros_grub_contract_set(GB_TARGET_STRIP "${_target_strip}")
    _aros_grub_contract_set(GB_CONFIGURE_TARGET "${_configure_target}")
    _aros_grub_contract_set(GB_PLATFORM "${_platform}")
    _aros_grub_contract_set(GB_TARGET_TRIPLE "${_target_triple}")
    _aros_grub_contract_set(GB_TARGET_ISA_FLAGS "${_target_isa_flags}")
    _aros_grub_contract_set(GB_LINK_FORMAT "${_link_format}")
    _aros_grub_contract_set(GB_PLATFORM_DIR "${_platform_dir}")
    _aros_grub_contract_set(GB_EXPECTED_FILE_COUNT "${_expected_file_count}")
    _aros_grub_contract_set(GB_INSTALL_MANIFEST_SHA256 "${_actual_install_manifest_sha256}")
    _aros_grub_contract_set(GB_STAMP "${_stamp}")
    foreach(_product IN LISTS _private_products_physical)
        _aros_grub_safe_value("${GB_MMAKE_ID}: private product" "${_product}")
        string(APPEND _contract_content
            "list(APPEND GB_PRIVATE_PRODUCTS [==[${_product}]==])\n")
    endforeach()
    foreach(_product IN LISTS _install_products_physical)
        _aros_grub_safe_value("${GB_MMAKE_ID}: installed product" "${_product}")
        string(APPEND _contract_content
            "list(APPEND GB_INSTALL_PRODUCTS [==[${_product}]==])\n")
    endforeach()
    _aros_grub_write_if_changed("${_contract}" "${_contract_content}")

    add_custom_command(
        OUTPUT ${_products_logical}
        COMMAND "${CMAKE_COMMAND}" -DGB_ACTION=build
            "-DCONTRACT=${_contract_logical}"
            -P "${_AROS_GRUB_BUILD_MODULE_DIR}/RunGrubBuild.cmake"
        DEPENDS "${_archive_logical}" "${_patch_logical}"
            "${_install_manifest_logical}" "${_contract_logical}"
            "${_AROS_GRUB_BUILD_MODULE_DIR}/RunGrubBuild.cmake"
        COMMENT "Building audited GRUB2 ${GB_MODE} host lane"
        VERBATIM)
    add_custom_target("${GB_MMAKE_ID}" DEPENDS ${_products_logical})
    add_dependencies("${GB_MMAKE_ID}" grub2-aros--fetch)
endfunction()
